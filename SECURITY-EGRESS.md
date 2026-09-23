# Egress & Upload Audit

Authoritative inventory of every data-egress path in this fork, its gate, and
its expected state. **This is the checklist for the weekly upstream sync.**

Run `make audit` after every `make sync` and before merging any PR. It fails if
a guard is reverted, a gate stops gating, the stubbed session-sync subsystem
gains an implementation, a dormant upload API gains a caller, or any file gains
a new outbound HTTP write or WebSocket/gRPC/donate-pump/Sentry-init call site.

Last full manual review: **2026-09-22**, against upstream `1.0.38`
(merge commit `31342c06`, fork patches re-applied in `e29ee5e7`; branch
`upstream-sync-1.0.38`). Re-application recipes: **PATCH.md**.

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

`make audit` covers all three. Checks 1–4 are targeted; checks 5 and 6 are
the catch-all censuses (HTTP writes, then WebSocket/gRPC/donate/Sentry) that
notice paths nobody thought to enumerate.

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
| 6 | Managed-config fetch | `managed_config/store.rs` → `is_fetch_enabled()` | returns `false` unconditionally |
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
(post-1.0.38): **41 files, 114 call sites**.

The audit diffs the live tree against it and prints the exact files that gained
calls. This is the backstop that catches HTTP write paths not enumerated
above.

A second census, `scripts/egress-baseline-nonhttp.txt` (check 6, added in the
1.0.38 sync), closes the blind spot the HTTP regex can't see: WebSocket
connects (`connect_async`, `wss://`, `ws://`), gRPC/tonic transport, the
computer-hub-sdk donate pumps, and `sentry::init`. Current baseline: **35
files, 107 call sites** — dominated by `xai-grok-voice` (§2.5's WebSocket
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
