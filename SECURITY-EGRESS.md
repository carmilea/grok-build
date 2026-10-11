# Egress & Upload Audit

Authoritative inventory of every data-egress path in this fork, its gate, and
its expected state. **This is the checklist for the weekly upstream sync.**

Run `make audit` after every `make sync` and before merging any PR. It fails if
a guard is reverted, a gate stops gating, the stubbed session-sync subsystem
gains an implementation, a dormant upload API gains a caller, a local listener
stops binding loopback, or any file gains a new outbound HTTP write or
WebSocket/gRPC/raw-socket/donate-pump/Sentry-init call site.

Last full manual review: **2026-10-10**, against upstream `1.0.45`
(branch `upstream-sync-1.0.45`; patch 2 gained a second site and patch 6 was
re-homed into `xai-grok-cloud-config` — see PATCH.md §1). Re-application
recipes: **PATCH.md**.

---

## Threat model

The concern is bulk exfiltration of local context — source files, conversation
history, secrets and `.env` contents that entered the context because the agent
read them — to xAI-controlled endpoints. Not in scope: the model API calls the
tool exists to make (`api.x.ai/v1` inference), which necessarily carry prompt
content the user chose to send.

Three ways something gets snuck back in during a sync:

1. **A guard is reverted** — a patched function returns to upstream behavior.
2. **A gate is widened** — a conjunct is dropped, or an ordering changes so an
   upload method resolves before the gate is consulted.
3. **A new path appears** — a new call site, or a stub replaced by a real
   implementation.

`make audit` covers all three. Checks 1–4 and 7 are targeted; checks 5 and 6
are the catch-all censuses (HTTP writes, then WebSocket/gRPC/raw-socket/
donate/Sentry) that notice paths nobody thought to enumerate.

---

## 1. Fork patch guards

| # | Guard | Site | Expected |
|---|---|---|---|
| 1 | Telemetry hard-disable | `agent/config.rs` → `resolve_telemetry_mode()` | returns `Disabled` unconditionally |
| 2 | Remote-settings fetch | `util/config/resolve/features.rs` | fetch disabled |
| 2b | Login-config beacon | `xai-grok-login/src/flow.rs` → `fetch_login_device_flow()` | neutered |
| 3 | Reasoning provenance | `xai-grok-sampling-types` `conversation/messages.rs` | unsigned reasoning dropped |
| 4 | Access gate | `agent/mvp_agent/mod.rs` → `enforce_grok_code_access()` **and** `settings_allow_access()` | fails open |
| 5 | External OTEL | `agent/config.rs` → `resolve_external_otel_config()` | returns `None` unconditionally |
| 6 | Managed-config fetch | `xai-grok-cloud-config/src/managed_config/store.rs` → `is_fetch_enabled()` | returns `false` unconditionally |
| 7 | Error reporting (Sentry) | `agent/config.rs` → `is_error_reporting_disabled_sync()` | returns `true` unconditionally |
| 8 | Session sharing stripped | `extensions/share.rs` → `handle_share_session()` | refuses unconditionally |
| 10 | Auto-update disabled | `xai-grok-update/auto_update.rs` → `run_update_if_available()` + `check_update_background()` | both return "no update" unconditionally |
| 11 | Feedback trace upload disabled | `extensions/feedback_trace.rs` → `handle_upload_trace_with_session_dir()` | refuses unconditionally |

Guard 10 defends the fork itself: upstream's self-update (default on) would
download a pristine upstream binary and install it in place, silently
reverting every guard on this list. It also phones the release channel on a
timer. Off in code and in config (`[cli] auto_update = false`).

Guard 1 is the keystone for the *internal* channels: Mixpanel, product-events,
the internal OTEL span pipeline, session metrics, and the trace-upload default
all gate on it, and it ignores the original resolution order — including
server-pushed `remote_settings`. That server-push was the one way xAI could
enable collection without a local opt-in. It is inert.

Guard 1 does **not** cover the **external OTEL stream** (`xai-grok-telemetry`
`src/external/`, added upstream in 0.2.118+): an enterprise
bring-your-own-collector OTLP exporter (HTTP, gRPC, mTLS) documented upstream
as "independent of `TelemetryMode`", enable-able via requirement pin,
`GROK_EXTERNAL_OTEL`, or `[telemetry] otel_*` config. That is guard 5. And
because the **managed-config sync** (default on, overlay-free, not covered by
guard 2) can server-push those `otel_*` keys, guard 6 closes the fetch.
**Sentry** gates on `is_error_reporting_disabled_sync()` — an env/TOML chain
independent of guard 1 — so guard 7 welds it shut (payload was only scrubbed
crash reports, but an env var could have armed it). **Session sharing**
(re-enabled upstream in 1.0.5 behind remote `sharing_enabled`) uploads the
full conversation to a public link; guard 8 strips the handler outright rather
than rely on the remote-flag gate staying dead. **Feedback trace upload**
(new upstream in 1.0.38) tars the whole session directory and PUTs it to GCS
on a one-shot grant token that is designed to bypass both the trace-upload
gate and guard 1 — guard 11 refuses at the handler's sole chokepoint before
any parsing or archive I/O, same shape as guard 8.

Two egress-relevant behaviors are **kept deliberately** (reviewed 2026-08-18,
see PATCH.md §2.5): the ZDR access gate resolves *closed* with remote fetch
disabled (fine for personal/BYOK use; fail-open recipe recorded if a ZDR-team
user ever appears), and `grok trace <id>` remains an ungated user-explicit
upload — inert without xAI auth, and the only way to produce a debug bundle.

Note on patch 9 (web-search backends): `web_search` sub-calls POST the search
query to the **user-configured** provider only (Moonshot `/chat/completions`
or Anthropic `/messages`, per `[models] web_search` — never xAI without an
xAI entry). Same trust boundary as inference calls: user-chosen content to a
user-chosen endpoint, out of scope for this audit. The three `.post(` sites
are accepted in the egress baseline with this justification.

Note on patch 13 (compaction model+effort pinning): `[compaction] model` sends
the compaction request — which carries the **whole conversation** — to that
`[models]` entry's endpoint instead of the session's. No new call site (it
reuses the sampler transport, so the census is unchanged), and the same trust
boundary as any model switch: the pinned entry authenticates with its own
credential at its own `base_url` (patch 12's rule), and an id that is not in
the catalog falls back to the session model rather than to any default
endpoint. Worth knowing when reviewing where a transcript can go: with a pin
set, the compaction summary's provider is a *second* provider in the session.

Note on patch 14 (api model-list refresh): `/api-model-update` adds outbound
`GET {base_url}/models` requests to the user's own configured providers. No
conversation content, no POST (so neither census counts it), user-initiated
only — see §2.11.

> The audit checks guard 1 **behaviorally** (asserts the actual
> `Resolved::new(TelemetryMode::Disabled, …)` return), not by comment. `make
> doctor` greps for comment strings only — it proves a comment exists, not that
> the code is disabled. Prefer `make audit`.

---

## 2. Upload gates

### 2.1 Per-turn artifact upload — the highest-value path

`src/upload/{turn,trace,gcs,manifest}.rs` (~3.6k lines) uploads session
artifacts at the end of **every turn**. Chokepoint:
`agent_ops.rs::trace_upload_config_with_reason()`.

```rust
if self.is_data_collection_disabled() { return (None, ZdrTeam); }   // guard 1
// (1.0.5: upstream inserts a refresh_remote_settings().await HERE —
//  a network fetch on the trace-decision path. Guard 2 (remote-fetch
//  disable) turns it into a no-op. It must never move ABOVE the kill switch.)
if !cfg.is_trace_upload_enabled()     { return (None, FeatureOff); } // guard 2
```

Returning `None` means no `PromptTraceContext` is built, which transitively
disables every turn-end upload including the memory upload. Both guards must
survive, in that order. As of 1.0.5 a sync twin,
`trace_upload_config_snapshot()`, gates subagent traces and heap-profile
uploads with the same two conjuncts OR'd — the audit asserts both.

**Payload if it ever fires:** the whole session directory — `chat_history.jsonl`,
`system_prompt.txt`, `prompt_context.json`, `events.jsonl`. Full conversation
content, including any file the agent read.

### 2.2 `resolve_trace_upload()` — SOFT gate, know this one

`agent/config.rs`. Off by default: `.default()` derives from the always-
`Disabled` telemetry mode, and the server-pushed `feature_flag` is forced to
`None` on that same branch — **xAI cannot switch it on.**

But unlike `resolve_telemetry_mode()` it does *not* hard-return `false`. Local
opt-in still works, deliberately (preserves `--local` export debugging and
self-hosted buckets):

- `GROK_TELEMETRY_TRACE_UPLOAD=1`
- `[telemetry] trace_upload = true`
- a `requirements.trace_upload` pin

Even then an upload *method* must resolve, which needs xAI auth, a deployment
key, or a GCS service-account key. With third-party providers (Moonshot/Kimi
etc.) it lands on `NoCredentials`.

**Current state:** none of these opt-ins are set — verified in `~/.grok/config.toml`
(which has `[features] telemetry = false`), env, and the `grok` wrapper.

If you want it welded shut, return `Resolved::new(false, ConfigSource::Default)`
and update `scripts/audit-egress.sh` + this file.

### 2.3 Relay sync — conversation writeback

`agent_ops.rs:relay_sync_enabled`. When on, `remote::sync` continuously POSTs
full message bodies to `/sessions/{id}/data` plus a metadata upsert. Requires
**all three**, none remotely settable:

```rust
tui_mode && relay_config_enabled && has_xai_auth
```

`relay_config_enabled` defaults false (`[relay] enabled` /
`GROK_RELAY_SYNC_ENABLED`). `has_xai_auth` means BYOK/third-party providers can
never trigger it. Dropping any conjunct widens exposure silently.

### 2.4 Feedback client — reviewed, dormant on BYOK

`src/agent/feedback_client.rs` (7 `.post(` sites as of 1.0.38, was 8; all in
the egress baseline) talks to cli-chat-proxy: session-signal sync, feedback
submission/completion/dismissal, feedback-request creation, and per-turn
deltas. 1.0.38 deleted the per-session `POST /sessions/{id}/events` method
that made up the eighth site — one fewer endpoint, not a widened gate. Never
previously given a written justification here — closing that gap:

- **Payload.** Depending on the endpoint, a submission can carry
  `last_user_message`, `last_assistant_message`, `session_cwd`, and
  per-tool `tool_outcomes` (call/failure counts). `/feedback`'s text path
  explicitly nulls the message fields before sending
  (`feedback_manager.rs::submit_text_feedback`); other submission paths
  (e.g. thumbs-up/down) do not make that guarantee — treat message content as
  in scope for this endpoint. 1.0.38 widened `/v1/feedback` further: a
  submission can now also carry base64-encoded image attachments and a
  `reasoning_effort` string. The images go out on the wire as sent; only the
  *local* trace-archive copy (`feedback_manager.rs`) strips them to a
  `<N base64 bytes stripped>` placeholder before persisting to
  `feedback.jsonl`, so one screenshot's base64 doesn't bloat every trace
  record — that's a local storage concern, not a network-payload guarantee.
  Same gate as the rest of this client.
- **`POST /sessions/{id}/turn-deltas` fires at every turn end** — it is
  **not** user-initiated, unlike `/v1/feedback` (slash command / rating UI).
  It carries only the delta counters (tool calls, failures, errors,
  cancellations, ratings, etc.), no message text.
- **Gate:** every method on this client requires `has_proxy_credentials()`
  (`agent_ops.rs`) — a deployment key **or** xAI auth. BYOK / third-party
  providers (Moonshot, Anthropic, etc.) satisfy neither, so
  `feedback_credentials()` returns `None` and no call fires. **Inert on this
  fork's BYOK posture.**
- **`Feature::Feedback` is default-on**, but that only controls whether the
  *local* feedback UI/prompts are offered — the remote submission tier it
  would talk to is the same one made dead by patch 2 (`resolve_remote_fetch_enabled()`
  hard-disabled) for anything that depends on remote settings, and by the
  BYOK credential gate above for everything else.

Accepted, reviewed: dormant under this fork's deployment model (personal/BYOK
credentials, no deployment key). Re-review if this fork is ever run with an
xAI deployment key or xAI-auth session.

### 2.5 Voice STT + Imagine tools — the side-call bearer (patch 12)

`xai-grok-voice` streams microphone audio to `wss://api.x.ai/v1/stt`
(`src/config.rs::stt_ws_url()`, `src/stt/streaming.rs`) for speech-to-text.
The image/video tools POST prompts (and reference images) to
`api.x.ai/v1/images|/videos` (`xai-grok-tools/.../image_gen/mod.rs`,
`video_gen/mod.rs`). Both authenticate with the process-wide **side-call
bearer** (`xai-grok-login/src/side_call_bearer.rs::resolve_static_api_key`).

- **The leak patch 12 closes.** Upstream's bearer resolution falls back to
  *any* `[models]` entry's own `api_key` when no xAI login or `XAI_API_KEY`
  exists (`byok_from_models` in `agent/mvp_agent/agent_ops.rs`). On a
  BYOK-only setup that sent the third-party key (e.g. Moonshot) to
  `api.x.ai` on `/voice`, Ctrl+Space, `/imagine`, or a model-invoked
  `image_gen` call — a credential leak to a non-configured endpoint, with
  no telemetry/remote-fetch guard involved. Patch 12 restricts the
  fallback to entries whose effective API endpoint is a trusted xAI host
  (`is_trusted_xai_https_url`). On this fork both services are now inert
  under BYOK (they are xAI-only products; a foreign key would only 401).
  (An earlier revision of this row claimed voice needed xAI/deployment
  credentials like the feedback client — wrong: the side-call bearer
  explicitly accepts per-model BYOK keys.)
- **Feature gates:** `Feature::VoiceMode` (default **on**); image/video
  tool configs default on. The remote tier is dead under patch 2. Users
  who *do* configure an xAI credential (`XAI_API_KEY`, xAI login, or an
  xAI-hosted `[models]` entry) can still use both features.
- **Census blind spot.** Voice is a WebSocket send, not an HTTP
  `.post()`/`.put()`/`.patch()` call — it is invisible to the check 5 census
  by construction, not because it was filtered out. Documented here as a
  pre-existing inventory gap; see PATCH.md §4 ("Watch for census blind
  spots") for the general rule (grep new subsystems for `tonic`, `ws://` /
  `wss://`, `sentry`, `.send(`). As of 1.0.38 check 6 (§5 below) covers this
  pattern, so future WebSocket/gRPC/donate-pump/Sentry additions no longer
  need to wait for someone to notice by hand.

### 2.6 Feedback trace upload — guard 11, new in 1.0.38

`x.ai/feedback/upload-trace` (`extensions/feedback_trace.rs`) tars the whole
session directory (`chat_history.jsonl`, `system_prompt.txt`,
`prompt_context.json`, `events.jsonl`, capped at 50 MiB) and PUTs it to GCS.
Its resolver deliberately ignores the live `trace_upload` flag: a one-shot
grant token issued by a successful `/v1/feedback` POST authorizes the upload
on its own, so **neither** the trace-upload gate (§2.2) nor the telemetry
hard-disable (guard 1) covers it — by upstream design, not an oversight.
Guard 11 refuses unconditionally at `handle_upload_trace_with_session_dir()`,
the sole chokepoint, before `parse_params` and before any archive I/O.
`make audit` asserts the marker; `xai_file_utils::gcs::upload_bytes()`'s
caller-allowlist check (§5) additionally asserts this file's call site isn't
joined by a second, unreviewed one.

### 2.7 Session context snapshot — /tokenize-text, dead under guard 1

`session/acp_session_impl/context_snapshot.rs` (new upstream file) POSTs the
verbatim system prompt, tool definitions, skills listing, MCP announcement,
`AGENTS.md` sections, and workflow text to
`POST {xai_api_base_url}/tokenize-text`, to record itemized per-category
token counts on a session-metrics event. Not a fork patch — kept gated:

```rust
if !self.telemetry_enabled || !xai_grok_telemetry::is_session_metrics_enabled() {
    return;
}
```

Both conjuncts derive from `resolve_telemetry_mode()` (guard 1) being always
`Disabled`, so this path is dead. `make audit` asserts the double gate stays
intact behaviorally, not just by comment — losing either conjunct would send
the entire prompt-context inventory (not the conversation itself, but
everything else that would tell you what's in it) to the tokenizer endpoint
on every session start.

### 2.8 Consent record — accepted, requires proxy auth

`extensions/consent.rs` (`x.ai/consent/record`) POSTs `{proxy_url}/consent/accept`
with `noticeId`, `version`, and the client identifier — metadata about which
consent notice the user accepted, not conversation content. Gated implicitly:
`agent.auth_manager.auth()` must resolve before the POST is built, which
requires xAI/proxy credentials — inert on this fork's BYOK posture (same
credential boundary as §2.4/§2.5). Accepted.

### 2.9 MCP server connectivity probe — accepted, same trust boundary as MCP

`xai-grok-mcp/src/servers.rs` POSTs an empty-body probe request to a
**user-configured** MCP server URL to distinguish auth failures (401) from
proxy-credential failures (407) before treating the server as unreachable.
Same trust boundary as using the MCP server at all: user-chosen endpoint,
user-chosen content. Accepted.

### 2.10 Workspace hub OIDC proactive refresh — dormant without COMPUTER_HUB_URL

`xai-grok-workspace/src/hub_auth/proactive.rs` POSTs to the computer-hub's
discovered OIDC `token_endpoint` to refresh an access token ahead of expiry.
Unreachable unless `COMPUTER_HUB_URL` is configured (the computer-hub feature
itself is opt-in and off by default). Dormant. Accepted.

### 2.11 `/api-model-update` model-list fetch — patch 14, user-initiated

`api_model_update/discovery.rs` sends one `GET {base_url}/models` per unique
`(base_url, api_backend)` pair in the user's own `[model.*]` entries, and only
when the user runs `/api-model-update`. The request has no body — no prompt,
no conversation, no filenames — and carries only that pair's own credential
(Bearer, or `x-api-key` + `anthropic-version` for Messages), so the patch 12
rule that a credential never leaves its own endpoint holds here too. `responses`
entries are skipped, so the command never queries the xAI catalog. Same trust
boundary as inference to the same entry. Accepted. Neither census pattern
matches a `GET`, which is why the path is recorded here explicitly.

### 2.12 Local egress proxy — new upstream in 1.0.45, accepted

`xai-grok-egress-proxy` is a forward HTTP/CONNECT proxy the **workspace
sandbox** starts for a served folder, so a sandboxed command's own network
calls can be matched against that folder's website policy and held for a
permission card. It is not a client of anything: the crate's dependency list
carries no HTTP client at all (`httparse` + raw `tokio` sockets only), so it
**cannot phone home** — every byte it sends is a byte a local client handed it
for an origin that client named.

- **Where it listens.** `EgressProxy::start_with()` binds
  `TcpListener::bind((Ipv4Addr::LOCALHOST, 0))` — loopback, ephemeral port,
  never a routable interface — and `server::serve()` drops any accepted peer
  whose `peer.ip().is_loopback()` is false before admitting it.
- **Where it connects.** `TcpConnector::connect()` (raw `TcpStream::connect`),
  reached only through `resolve_public()`, which resolves the origin via
  `lookup_host` and refuses the whole address set if **any** address fails
  `ip::is_public_ip()` — the SSRF/metadata-endpoint guard. `connect::handle()`
  then relays with `copy_bidirectional`; `http::handle()` refuses request
  bodies outright (`ProxyError::BodyUnsupported`), so the plaintext path cannot
  carry an upload.
- **Reachability on this fork.** `WorkspaceSandbox` has no construction site
  outside `xai-grok-workspace`, and `sync_network()` starts the proxy only for
  a folder the workspace daemon serves in a *wrapped* (enforce) sandbox mode.
  The local `grok` CLI never reaches it — same dormancy boundary as §2.10.
- **Credential.** `mint_call_credential()` puts a per-call token in the child's
  `HTTP_PROXY` userinfo. It is attribution between local processes, by
  upstream's own note; it is never sent off the machine.

Accepted, not gated: this is a *restriction* on child networking, and its own
`lib.rs` is explicit that it is not an enforcement boundary on its own
("Starting this library does not confine child networking"). Treat it as
inventory, not as protection. Its raw-socket transport is invisible to the
check 5 regex and was invisible to check 6 until that census was widened (§5),
and its listener is asserted loopback-only by check 7.

### 2.13 Pre-stop lifecycle broker — new upstream in 1.0.45, accepted

`xai-grok-lifecycle` serves `POST /lifecycle/pre-stop`,
`PUT|DELETE /lifecycle/handlers/{name}` and `GET /lifecycle/handlers` and fans
a pre-stop event out to handlers declared by JSON manifests under
`DEFAULT_IMAGE_HANDLER_DIR` and to handlers registered at runtime. Handlers are
`argv` vectors run by `exec::run_exec()` — **local process execution, not
network egress**; the crate has no HTTP client and no socket of its own.

- **Where it listens — loopback only.** The broker never binds anything: it
  hands `LifecycleBroker::router()` to the in-guest diagnostics server as
  `extra_routes`, and `xai-grok-diag-server` binds either a Unix socket
  (`UnixListener::bind`, default `/tmp/workspace-server.sock`) or, on Windows
  guests, `TcpListener::bind((Ipv4Addr::LOCALHOST, port))`. There is no
  `0.0.0.0` / unspecified bind anywhere in that file.
- **Reachability on this fork.** The single non-test `LifecycleBroker::new()`
  site is `xai-grok-workspace/src/bin/workspace_server.rs` — the in-VM guest
  binary, not the `grok` CLI. Dormant here for the same reason as §2.12.
- **What a reverted bind would cost.** A routable diagnostics listener would
  expose an unauthenticated `PUT /lifecycle/handlers/{name}` that runs an
  attacker-chosen absolute-path `argv` inside the guest. Check 7 asserts the
  loopback bind and the closed broker-construction set behaviorally.

The one check-5 match in `lifecycle/src/routes.rs` is `broker.registry().put(…)`
in the `PUT /lifecycle/handlers/{name}` handler — a write to the broker's own
handler registry, inbound request handling, not an outbound call. (The axum
route builders are imported bare as `get`/`post`/`put` and never match the
`\.post(` pattern.) It is a known false positive in the check 5 census and is
accepted in the baseline on that ground.

### 2.14 Cloud-config, permission rules, file locks, agent import — 1.0.45 crate moves

Four of the six crates upstream added in 1.0.45 add no egress path of their own:

- `xai-grok-cloud-config` is the old shell-side remote-settings/managed-config
  code extracted into a crate. Its fetches are `GET`s on
  `xai_grok_http::shared_startup_blocking_client()`
  (`settings_fetch.rs`, `managed_config/supervisor.rs`), so neither census sees
  them — they are covered by guard 2 and guard 6 instead. **Guard 6 moved with
  it**: `is_fetch_enabled()` now lives at
  `xai-grok-cloud-config/src/managed_config/store.rs` and is the single
  chokepoint for the 5-minute `spawn_refresh_supervisor()` timer, the
  `revalidate_stale_start()` startup fetch, `post_login_sync()`,
  `policy_repair_pending()`, the staged-refresh apply, and the updater's own
  managed-config consult — all of which return early when it is `false`.
- `xai-grok-permission-rules` is the permission rule engine extracted out of
  the workspace crate. No HTTP client, no socket, no async runtime in its
  dependency list; its only `reqwest` mentions are doc comments explaining URL
  normalization.
- `xai-grok-file-lock` depends on `thiserror`, `tracing` and `libc` only —
  advisory `flock` under the grok home. No network surface.
- `xai-grok-external-agent-migration` reads another agent CLI's settings off
  disk and rewrites `config.toml` with `toml_edit`. No network. It can import
  `[mcp_servers]` entries, which *are* outbound endpoints once used — but only
  the ones already configured on this machine, only on explicit confirmation
  (`apply_import()` runs from the pager's import modal, never at startup), and
  only into names that do not already exist. Same trust boundary as §2.9.

### 2.16 Fork release check — `grok update --check` (patch 10c)

The only outbound request this fork **adds** rather than removes, and it is a
net reduction in exposure: it replaces the upstream release-channel lookup
with one to this repo's own releases.

| | |
|---|---|
| Path | `GET https://api.github.com/repos/carmilea/grok-build/releases/latest` |
| Site | `auto_update.rs` → `fork_latest_release_tag()`, reached only from `check_update_status()` |
| Trigger | `grok update --check` only — explicit, user-initiated, never on a timer or at startup (patch 10 kills the background check) |
| Sends | Nothing but the request itself: no auth, no version, no identifiers. A default `reqwest` client with a `grok-build-fork/<ver>` user agent and a 10s timeout |
| Replaces | `get_latest_version()` against the npm registry, the GCS channel pointer, or xai-org's GitHub — i.e. this removes the last routine contact with upstream infrastructure from the update path |
| Same endpoint as | `install.sh`, which already resolves the latest release this way |

**Census blind spot — read this before trusting a green audit.** Checks 5 and
6 match write verbs (`.post(`/`.put(`/`.patch(`) and socket transports. An
update check is a plain `GET`, so **neither census moved when this call was
added**, and neither would move if someone repointed it at upstream. The two
behavioral checks in §1 (the fork lookup must precede `get_latest_version()`,
and the endpoint must still name `carmilea/grok-build`) are the only
protection here. Both were confirmed to go red when disarmed.

### 2.15 Census entries accepted in the 1.0.45 sync

Every file the two censuses gained in the 1.0.45 baseline update, and why each
is accepted. None of them is a new way for conversation content, file content,
or credentials to leave the machine. Reviewed line-by-line against the audit's
own regexes.

**Check 5 (outbound HTTP writes), 41 files / 114 sites → 43 / 118.**

| File | Δ | What matched | Verdict |
|---|---|---|---|
| `xai-mixpanel/src/lib.rs` | 2 → 3 | Upstream collapsed the two inline `client.post("https://api.mixpanel.com/{track,engage}")` builders into one private `post(path, payload)` helper. The regex now counts the helper's one real `.post(` **plus** the two internal `self.post(` call sites: real outbound builders went **2 → 1**. | Dead regardless: patch 1b forces `TelemetryMode::Disabled` in `client::init()`/`init_if_needed()`, so `TELEMETRY_CLIENT` stays `None`, `TelemetryClient::from_config()` (the only `Mixpanel::with_client` construction site) never runs, and `track`/`sync_profile` return at their `None => return` guard before touching a client. |
| `xai-grok-lifecycle/src/routes.rs` | +1 | `broker.registry().put(name, spec)` in the `PUT /lifecycle/handlers/{name}` handler — the broker's in-memory + on-disk handler registry, inbound server-side handling. No HTTP client in the crate. | Not egress. (Supersedes §2.13's note, which attributed this match to axum's `post(` route builder; the route builders are imported as bare `post`/`put` and do not match the `\.post(` regex.) |
| `xai-grok-workspace/src/permission/sandbox_network.rs` | +2 | `sink.post(call, …)` — the in-process `ViolationSink` trait that enqueues a permission card for the local UI (`NetworkViolationSink` in `sandbox/network.rs`). | Not egress; same workspace-daemon dormancy as §2.12. |

**Check 6 (non-HTTP egress), 35 files / 107 sites → 46 / 128.** The bulk is the
cost of widening the regex for raw sockets (§5): ten of the twelve files below
carry *byte-identical* match sets at 1.0.38 and today — pre-existing code that
the old pattern simply could not see.

| File | Δ | What matched | Verdict |
|---|---|---|---|
| `xai-grok-egress-proxy/src/lib.rs` | +2 | `lookup_host` in `SystemResolver`, `TcpStream::connect` in `TcpConnector` | §2.12. The widening exists for exactly this crate. Reached only via `resolve_public()`'s all-addresses-public gate; asserted by check 7a–7c. |
| `xai-grok-egress-proxy/src/connect.rs` | +2 | the `copy_bidirectional` import plus its one use in the CONNECT relay | §2.12. Relays only what a loopback client handed it. |
| `xai-grok-shell/src/agent/proxy.rs` | 2 → 3 | pre-existing `TcpStream::connect(&proxy_addr)` in `open_connect_tunnel` — dials the user's **own** `HTTPS_PROXY`/`HTTP_PROXY` to tunnel the relay WebSocket. Sole callers: `agent/relay.rs`. | Pre-existing, newly matched. Transport for §2.3 relay sync, which needs `tui_mode && relay_config_enabled && has_xai_auth`. Not an endpoint of its own. |
| `xai-grok-hooks/src/runner/http.rs` | +1 | pre-existing `lookup_host` in `validate_hook_url()` — **SSRF defense**, run at line 182 before the `.post(url)` at 221, rejecting non-`https`, private, and link-local destinations. | Pre-existing, newly matched, and defensive. The hook POST itself is unchanged in check 5 (2 sites) and goes to a user-authored `[hooks]` URL. |
| `xai-grok-tools/.../web_fetch/ssrf.rs` | +1 | pre-existing `lookup_host` in `check_ssrf()` — resolves and refuses non-public addresses; called at the top of `web_fetch`'s request loop and again on every redirect hop. | Pre-existing, newly matched, and defensive. Direction is inbound-blocking, not outbound. |
| `xai-grok-tools/.../lsp/client.rs` | +1 | pre-existing `TcpStream::connect(&addr)` in `start_socket()` | Pre-existing, newly matched. Only reachable with an opt-in `[lsp.*] transport = "socket"` entry (default `stdio`); the address is the user's own. Same trust boundary as running the language server locally. |
| `ptyctl/src/server.rs` | +1 | `TcpStream::connect(("127.0.0.1", port))` inside `#[cfg(all(test, unix))] mod tests` | Test fixture, loopback. `ptyctl` is linked only by the PTY test harness and the standalone `ptyctl` CLI — never by the shipped `grok` binary. |
| `xai-grok-diag-server/src/lib.rs` | +1 | `UnixStream::connect(&sock)` in an in-file `#[tokio::test]` asserting the socket is `0600` | Test fixture, local socket. |
| `xai-grok-workspace/src/bin/workspace_server.rs` | 2 → 3 | the two pre-existing `wss://`/`ws://` CLI-default and doc lines, plus a new `TcpStream::connect(("127.0.0.1", port))` test helper in `mod tests` | Test fixture, loopback. |
| `xai-grok-workspace/src/worktree/strategy.rs` | +1 | `UnixStream::connect(sock)` in a `cfg(test)` fake-daemon helper | Test fixture, local socket. |
| `xai-grok-test-support/src/{uds_proxy,gated_upload_proxy,envelope_sink}.rs` | +6 / +1 / +1 | a UDS fault-injection proxy, a gated stand-in for the trace-upload endpoint, and a loopback sink that captures Sentry envelopes so tests can assert none are sent. All `127.0.0.1` / unix-socket. | Test-only. The crate is a `[dev-dependencies]` entry everywhere except `xai-grok-pager-pty-harness`, itself a test/bench crate that is "not imported into the production pager binary". It escapes the census's `test_support` filter only because the crate directory is spelled with hyphens. |
| `xai-computer-hub-sdk/src/harness.rs` | 11 → 12 | one additional `Url::parse(&format!("ws://{addr}/v1/tools"))` in a new `#[tokio::test]` (`racing_drop_and_rebuild_never_detach_a_live_session`), inside the `#[cfg(test)] mod tests` that starts at line 2518, pointed at a local mock hub | Test fixture. The non-test matches in this file are two doc comments and the error string that **refuses** plaintext `ws://` to a non-loopback host. |

---

## 3. Session-sync subsystem — stubbed upstream

`session/restore_stub.rs`: every entry point `bail!(UNAVAILABLE)`
("Remote session restore is not available in this build"). This is upstream's
own doing in public builds, not a fork patch — so it is an **assumption that
can change without anyone framing it as a policy change**.

Load-bearing: remote restore is the download half of a pair. A real
implementation implies the upload half. If a sync replaces this file, STOP and
audit before merging.

## 4. Dormant upload APIs — zero live callers

`xai-file-utils/storage_client.rs` exposes `batch_upload`, `batch_upload_json`,
`upload_file`, `upload_stream`, `upload_multipart`, `get_signed_upload_url`,
`upload_via_signed_url`, `upload_bytes_signed` → `cli-chat-proxy.grok.com/v1/storage/*`.

**Zero live callers.** Compiled dead code. The two live `StorageClient`
construction sites (`session_startup.rs`, `effects/mod.rs`) are restore
(download) paths that dead-end in the stub above.

The first live caller is the signal to investigate — that is check 4.

`xai_file_utils::gcs::upload_bytes()` is a related but distinct check in the
same section: unlike `StorageClient`'s upload API, it is **not** dead code —
`upload/{trace,gcs}.rs`, `xai-grok-pager/src/trace_cmd.rs`,
`extensions/review.rs`, and `extensions/feedback_trace.rs` (patch 11, guarded
above it) are all live, reviewed callers. The check asserts the caller set
stays exactly that closed list; a caller outside it is a new GCS upload path
nobody enumerated, not a revived dead one.

---

## 5. Egress census

`scripts/egress-baseline.txt` records the count of outbound HTTP write calls
(`.post(` / `.put(` / `.patch(` / `.post_json(` / `Method::(POST|PUT|PATCH)`)
per file across the workspace, excluding tests and mocks. Current baseline
(post-1.0.45): **43 files, 118 call sites**.

The audit diffs the live tree against it and prints the exact files that gained
calls. This is the backstop that catches HTTP write paths not enumerated
above.

A second census, `scripts/egress-baseline-nonhttp.txt` (check 6, added in the
1.0.38 sync), closes the blind spot the HTTP regex can't see: WebSocket
connects (`connect_async`, `wss://`, `ws://`), gRPC/tonic transport, the
computer-hub-sdk donate pumps, and `sentry::init` — plus, since the 1.0.45
sync, the **raw socket transports** neither of those shapes covers
(`TcpStream::connect`, `UnixStream::connect`, `lookup_host`,
`copy_bidirectional`), which is the entire transport of §2.12's egress proxy.
Listener binds are deliberately *not* in this regex — `TcpListener::bind`
alone matches 39 files of loopback test fixtures — they are asserted
behaviourally by check 7 instead. Current baseline (post-1.0.45, widened
pattern): **46 files, 128 call sites**, up from 35/107 — dominated by
`xai-grok-voice` (§2.5's WebSocket
STT), `xai-computer-hub-sdk` (its own WebSocket tool-connection protocol),
and `xai-grok-telemetry/src/sentry.rs` (guard 7b's already-neutered
`sentry::init`), plus a long tail of doc comments, loopback test fixtures,
and pre-existing gRPC/relay plumbing that predates this fork's history —
reviewed in the commit that introduced the baseline, not itemized here.

After reviewing legitimate additions, accept them:

```sh
make audit-baseline    # regenerates BOTH baselines; COMMIT the diff with justification
```

Never regenerate a baseline to make a red audit go green without reading each
addition. The baseline diff is the audit trail.

---

## Weekly sync runbook

```sh
make sync            # fetch + merge upstream
make audit           # ← must be clean before anything else
make check && make test
```

If `make audit` fails:

1. Read the named file and line. Ask what data reaches that call and who
   receives it.
2. Re-apply the guard if a patch was reverted (guard sites are listed in the
   `Makefile` header).
3. If it's a genuinely new upstream feature, decide explicitly whether to keep,
   gate, or patch it out — then update this file and the baseline in the same
   commit as the merge, so the reasoning is in the history.

Do not merge with a red audit.

## Independent corroboration

Two facts that hold even without the fork patches, useful as sanity anchors:

- Public builds carry **no telemetry credentials**. Upstream's own embedded
  docs: *"Builds from the public source tree carry no telemetry defaults:
  `events_url`, `events_api_key`, and `mixpanel_token` are unset and
  `mixpanel_enabled` is `false`, so nothing is sent unless you supply values."*
- Runtime check — `grok trace <session-id>` (no `--local`) falls back to local
  export rather than uploading:
  ```
  {"session_id":"…","status":"exported","local_path":"~/.grok/trace-exports/….tar.gz"}
  ```
  No upload URL. That's the 2.1/2.2 gate firing in the shipped binary.
