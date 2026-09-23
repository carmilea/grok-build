# Codebase Concerns — Telemetry & Data Harvesting Focus

**Analysis Date:** 2026-07-23

---

## Executive Summary — Data Collection Posture

Grok CLI contains a **multi-channel telemetry system** comprising Mixpanel product analytics, an internal OpenTelemetry trace pipeline to xAI's own backend, Sentry crash reporting, and a session-trace bulk upload mechanism (to GCS/S3/proxy). The **default telemetry mode is `Disabled`** — no analytics fire out-of-the-box for a fresh install with no config. However, telemetry can be activated silently via a **remote server push** (`remote_settings.telemetry_enabled = true`) without any local config change by the user, which is the key risk. When activated, a machine-stable hardware-derived identifier (`agent_id`) is attached to every event, user locale/country is harvested, and session trace archives containing full OTEL spans (tool calls, timing, token counts) are uploaded to xAI-controlled infrastructure. No `DO_NOT_TRACK` environment variable exists; the opt-out is `GROK_TELEMETRY_ENABLED=0` or `[features] telemetry = false` in config — **but these are overridden by requirements pinned by server policy**.

---

## Telemetry & Data-Harvesting Findings

| Finding | Evidence (`path:line`) | What data | Transmitted off-machine? | Opt-out |
|---------|------------------------|-----------|--------------------------|---------|
| **Mixpanel product analytics** — full event stream | `crates/codegen/xai-mixpanel/src/lib.rs:60-82` | Event name, `distinct_id` (user_id or agent_id), country, language, shell version, subscription tier, session_id, turn_number, tool names, model IDs, error categories, MCP server names, plugin names, skill names, hook names, agent_id, team_id, deployment_id, client type/version | **Yes** → `https://api.mixpanel.com/track` and `/engage` | `GROK_TELEMETRY_ENABLED=0` or `[features] telemetry = false`. Requires `TelemetryMode::Enabled`; not fired in `SessionMetrics` mode |
| **Mixpanel user profile sync** — `engage` call on init | `crates/codegen/xai-grok-telemetry/src/client.rs:262-306` | agent_id, shell_version, app_name, client_type/version, deployment_id, team_id, subscription_tier | **Yes** → `https://api.mixpanel.com/engage` | Same as above; blocked in `SessionMetrics` mode (`sync_profile` is no-op) |
| **Internal OTEL spans** — sent to xAI cli-chat-proxy | `crates/codegen/xai-grok-telemetry/src/otel_layer/mod.rs:1,57-68` | Session spans with session_id, turn_number, tool timings, model call latencies, token counts, user_id, team_id, deployment_id, api_key_id, organization_id. Resource attrs: service.version, client.name, client.version, app.entrypoint, terminal.type | **Yes** → `config.exporter.traces_url` (e.g. `https://cli-chat-proxy.grok.com/v1/traces`). Auth headers `Authorization: Bearer <token>`, `x-userid`, `x-teamid` attached | Requires `is_session_metrics_enabled()` to be true (i.e. `SessionMetrics` or `Enabled` mode). Default mode is `Disabled` |
| **Session trace bulk upload** — OTEL archive to GCS/S3/proxy | `crates/codegen/xai-file-utils/src/upload_config.rs:10-27`; `crates/codegen/xai-grok-telemetry/src/session_metrics.rs:44-93` | Full session OTEL spans archive. Three upload methods: `Direct` (GCS service account), `Proxy` (via cli-chat-proxy with user auth token + optional deployment key), `S3` (customer bucket) | **Yes** when upload method resolved and `trace_upload = true` | `GROK_TELEMETRY_TRACE_UPLOAD=false`, `[telemetry] trace_upload = false`, or ZDR team. Default: `true` when telemetry mode is `Enabled`; `false` otherwise |
| **Product events endpoint** — xAI internal events API | `crates/codegen/xai-grok-telemetry/src/client.rs:202-233`; `config.rs:131-133` | `viewer_context` with user_id, country, language, locale; event_name, event_metadata (session_id, turn_number, model_id, tool names, error categories, etc.) | **Yes** → `events_url` baked at build time via `GROK_TELEMETRY_BUILD_EVENTS_URL`. API key sent as `x-api-key` header | Same as Mixpanel gate: `TelemetryMode::Enabled` only |
| **Sentry crash/panic reporting** | `crates/codegen/xai-grok-telemetry/src/sentry.rs:32-68` | Panic messages, exception values, stack frame filenames/paths, breadcrumbs. Scrubbed for home dir, username, secrets before send. `send_default_pii = false`. Traces sample rate 1% | **Yes** → Sentry DSN (baked at build via `option_env!("SENTRY_DSN")`; runtime override via `SENTRY_DSN` env var). No-op if DSN is empty/unset | Set `SENTRY_DSN=""` at runtime to disable. `config.disabled = true` in the `Config` struct passed at binary init disables it regardless |
| **Machine / hardware identifier** (`agent_id`) | `crates/codegen/xai-grok-telemetry/src/id.rs:21-68` | macOS: hardware serial + UUID + SEID via `mid` crate. Linux: `/etc/machine-id` + `$HOSTNAME`. Hashed to deterministic UUIDv5. Cached at `~/.grok/agent_id` (0o600). Sent in every Mixpanel event and product event | **Yes** (stamped in every event) — see `client.rs:182-184` | No opt-out; always computed and included when telemetry is active |
| **User locale and country** | `crates/codegen/xai-grok-telemetry/src/client.rs:156-168` | System language tag (e.g. `en-US`) and country code via `whoami::langs()`. Sent on every `track()` call as `country` and `language` | **Yes** (in every product/Mixpanel event) | No opt-out; collected whenever `track()` fires |
| **Subscription tier** | `crates/codegen/xai-grok-telemetry/src/client.rs:113-128` | Normalized tier label: `free`, `supergrok`, `supergrok_heavy`, `x_premium`, `api_key`, etc. | **Yes** — included in Mixpanel profile and every event | No opt-out; derived from auth JWT |
| **Session-level metadata events** (`SessionMetrics` mode) | `crates/codegen/xai-grok-telemetry/src/session_metrics.rs:9-57` | session_id, turn_number, turn_count. Fired in both `SessionMetrics` and `Enabled` modes via `log_session_event` | **Yes** (via internal OTEL pipeline) | Disable by setting `TelemetryMode::Disabled` |
| **Remote config can silently activate telemetry** | `crates/codegen/xai-grok-shell/src/agent/config.rs:2287-2296` | Server-pushed `remote_settings.telemetry_enabled` or `telemetry_mode` can flip mode to `Enabled` without user config change | N/A — this is the activation mechanism | `requirements.telemetry.pinned()` (admin requirement pin) can force-disable; `GROK_TELEMETRY_ENABLED=0` env var also wins over remote |
| **Prompt text** — external OTEL only, gated | `crates/codegen/xai-grok-telemetry/src/events.rs:864-869`; `external/schema.rs:779-791` | Raw prompt text, capped at 60 KB, secret-scrubbed. Field is `#[serde(skip)]` — never sent to Mixpanel or product events | **Conditional** — only when `OTEL_LOG_USER_PROMPTS=1` and `GROK_EXTERNAL_OTEL=1` and external OTEL is configured | Default off (`gates.log_user_prompts = false`). Remote fleet policy can force it off but never on |
| **Tool parameters and file paths** — external OTEL only, gated | `crates/codegen/xai-grok-telemetry/src/events.rs:1017-1024`; `external/schema.rs:895-913` | Tool call parameters (reduced to 4 KB / depth 2), full file paths, verbatim MCP/plugin/skill names | **Conditional** — only when `OTEL_LOG_TOOL_DETAILS=1` and `GROK_EXTERNAL_OTEL=1` | Default off (`gates.log_tool_details = false`). Remote can force off, never on |
| **Git repo detection** | `crates/codegen/xai-grok-telemetry/src/events.rs:826` | Boolean `is_git_repo` (whether cwd is inside a git repo) sent in `SessionHarness`/`SessionNew` events | **Yes** (in product events) | No opt-out; collected whenever `SessionHarness` fires |
| **Terminal environment fingerprint** | `crates/codegen/xai-grok-telemetry/src/events.rs:1198-1219` | Terminal emulator brand, tmux version, OS, display server, SSH flag, clipboard route, modifier key fate, hyperlink support, refresh rate. All in `TerminalTelemetry` struct sent in pager events | **Yes** (via Mixpanel/product events in `Enabled` mode) | No opt-out; collected with pager telemetry events |
| **Coding data sharing / model training opt-out** | `crates/codegen/xai-grok-pager/src/settings/defs.rs:1148-1165`; `registry.rs:293` | Controls xAI training data retention. Default: **opt-out** (`coding_data_sharing_opt_out: true`). Separate from product analytics toggle | Unclear what server-side behaviour this triggers; local flag stored in auth metadata | Exposed in TUI Settings → Privacy → "Coding data sharing". Default is opt-out |
| **`agent_instance_id`** — per-process ephemeral ID | `crates/codegen/xai-grok-telemetry/src/id.rs:28-32` | Random UUIDv4 per process start, not persisted | Not confirmed transmitted — needs review | N/A |

---

## Data Egress Endpoints

| Host / Endpoint | Purpose | Auth | Fired when |
|----------------|---------|------|------------|
| `https://api.mixpanel.com/track` | Product event ingestion | Mixpanel project token (baked at build via `GROK_TELEMETRY_BUILD_MIXPANEL_TOKEN`) | `TelemetryMode::Enabled` + Mixpanel configured |
| `https://api.mixpanel.com/engage` | User profile creation/update | Same Mixpanel token | `TelemetryMode::Enabled` + Mixpanel configured; once per init |
| `{GROK_TELEMETRY_BUILD_EVENTS_URL}` | xAI internal product events API | `x-api-key: {GROK_TELEMETRY_BUILD_EVENTS_API_KEY}` (build-baked) | `TelemetryMode::Enabled` + events_url configured |
| `{traces_url}` (typically `https://cli-chat-proxy.grok.com/v1/traces`) | Internal OTLP span export | `Authorization: Bearer <user_token>`, `x-userid`, `x-teamid`, `X-XAI-Token-Auth` | `is_session_metrics_enabled()` → `SessionMetrics` or `Enabled` mode |
| `{proxy_base_url}` (typically `cli-chat-proxy.grok.com`) | Session trace archive proxy upload | User auth token + optional deployment key | `trace_upload = true` + `UploadMethod::Proxy` |
| `storage.googleapis.com` (GCS) | Session trace archive direct upload | GCS service account key | `trace_upload = true` + `UploadMethod::Direct` |
| `s3.{region}.amazonaws.com` (S3) | Session trace archive S3 upload | AWS credentials file/content | `trace_upload = true` + `UploadMethod::S3` |
| Sentry ingest URL (from `SENTRY_DSN`) | Crash/panic reporting | Sentry project DSN (baked at build via `option_env!("SENTRY_DSN")`) | On panic/crash, if DSN set at build or runtime |
| `{OTEL_EXPORTER_OTLP_LOGS_ENDPOINT}` | External OTEL log export (customer's collector) | Customer-supplied `OTEL_EXPORTER_OTLP_HEADERS` | `GROK_EXTERNAL_OTEL=1` + exporter configured — double opt-in, not xAI-controlled |
| `{OTEL_EXPORTER_OTLP_METRICS_ENDPOINT}` | External OTEL metrics export (customer's collector) | Customer-supplied headers | Same double opt-in |

**Note:** Actual hardcoded xAI hostnames are not present in this source tree. The `cli-chat-proxy.grok.com` hostname appears only in code comments (`otel_layer/mod.rs:1`, `file-utils/src/storage_client.rs:1`). Actual endpoint values are baked at build time via `option_env!()` or set in managed config pushed from the server. The source tree itself does not leak xAI hostnames into the binary — this is by design.

---

## Identifiers Collected

| Identifier | How Derived | Persistence | Where Sent |
|-----------|-------------|-------------|-----------|
| `agent_id` | macOS: hardware serial+UUID+SEID via `mid` crate, UUIDv5-hashed. Linux: `/etc/machine-id` + `$HOSTNAME`, UUIDv5-hashed. Fallback: random UUIDv4. | `~/.grok/agent_id` (0o600 perms) | Every Mixpanel/product event as `agent_id`; used as `distinct_id` when no authenticated user_id available |
| `agent_instance_id` | Random UUIDv4, per-process | Memory only (not persisted) | Needs tracing to confirm if sent; `id.rs:29` |
| `user_id` | From xAI auth JWT / grok.com login | Auth storage (auth metadata) | Product events, Mixpanel, OTEL resource attrs |
| `team_id` | From xAI auth JWT | Auth storage | Product events, Mixpanel, OTEL |
| `organization_id` | From xAI auth JWT | Auth storage | OTEL resource attrs |
| `deployment_id` | UUIDv5 derived from deployment key | Computed at runtime | Product events, Mixpanel, OTEL |
| `api_key_id` | From xAI auth JWT | Auth storage | OTEL resource attrs |
| `session_id` | Random UUID per session | Memory + local session DB | Every product event and OTEL span |
| Country + language | `whoami::langs()` (OS locale API) | Memory only | Every Mixpanel/product event call |

---

## Consent / Opt-Out Mechanisms

### Telemetry (product analytics + trace upload)

**Default state:** `TelemetryMode::Disabled` — no analytics or trace upload fires for a clean install.

**Activation priority** (highest to lowest, from `resolve_telemetry_mode()` at `agent/config.rs:2277`):
1. `requirements.telemetry.pinned()` — admin-imposed requirement (can force enable OR disable)
2. `GROK_TELEMETRY_ENABLED=0/1/session_metrics` — environment variable
3. `[features] telemetry = true/false/"session_metrics"` in `~/.grok/config.toml`
4. `remote_settings.telemetry_mode` or `remote_settings.telemetry_enabled` — **server push**
5. Default: `Disabled`

**Critical risk:** Step 4 means the server can activate telemetry without any local opt-in action. The user has no UI indication that remote settings changed their telemetry mode. The env var (`GROK_TELEMETRY_ENABLED=0`) does win over remote, but users are unlikely to know about this.

**Opt-out methods (user-accessible):**
- Set `GROK_TELEMETRY_ENABLED=0` in shell environment (wins over remote config)
- Set `[features] telemetry = false` in `~/.grok/config.toml` (subordinate to remote config and requirements)
- Set `GROK_TELEMETRY_TRACE_UPLOAD=false` to disable trace upload specifically
- ZDR (zero-data-retention) team membership disables trace upload (`TraceUploadReason::ZdrTeam`)

**No `DO_NOT_TRACK` support.** There is no standard `DO_NOT_TRACK` or `GROK_NO_TELEMETRY` variable.

### Sentry crash reporting

- Disabled if `SENTRY_DSN` was not set at build time (binary would have empty DSN)
- Override at runtime: set `SENTRY_DSN=""` to disable
- Per-binary: `Config { disabled: true }` disables it programmatically

### Coding data sharing (training data retention)

- Separate from product analytics; controlled by `coding_data_sharing` setting in TUI Privacy settings
- Default: **opt-out** (`coding_data_sharing_opt_out: true`) — user IS opted out of training data retention by default
- Surfaced to user in TUI at Settings → Privacy → "Coding data sharing"

### External OTEL (enterprise)

- Default **off**; requires explicit double opt-in (`GROK_EXTERNAL_OTEL=1` + at least one exporter env var)
- Prompt text gate (`OTEL_LOG_USER_PROMPTS`) and tool details gate (`OTEL_LOG_TOOL_DETAILS`) both default to `false`
- Fleet remote policy can force gates **off** but never on

---

## Other Technical Debt & Concerns

### Security

**`agent_id` uses raw hardware identifiers:** The `mid` crate (macOS) reads system profiler to get hardware serial, UUID, SEID. On macOS this call takes 1–3 seconds (mitigated by disk cache). The resulting ID is a **stable, unique cross-session machine fingerprint** sent in every telemetry event. There is no rotation or expiry.
- Files: `crates/codegen/xai-grok-telemetry/src/id.rs:50-62`

**Remote config can silently enable telemetry:** The `resolve_telemetry_mode()` function gives `remote_settings` higher priority than user `config.toml` but lower than env var. A server-side flag change silently activates data collection. This is architecturally intentional (fleet management) but presents a user-consent concern.
- Files: `crates/codegen/xai-grok-shell/src/agent/config.rs:2287-2296`

**Sentry DSN baked at build time:** `option_env!("SENTRY_DSN")` means the Sentry endpoint is compiled into the released binary. Users cannot inspect it from source alone; they must inspect the binary or trust build provenance.
- Files: `crates/codegen/xai-grok-telemetry/src/sentry.rs:40-41`

**Mixpanel token baked at build time:** Same pattern with `option_env!("GROK_TELEMETRY_BUILD_MIXPANEL_TOKEN")`.
- Files: `crates/codegen/xai-grok-telemetry/src/config.rs:133`

### Fragile Areas

**Telemetry global singleton race:** `TELEMETRY_CLIENT` is a `OnceLock<Mutex<Option<TelemetryClient>>>`. `init_if_needed` is designed for post-auth re-init but has a subtle race: if two concurrent callers race on the `is_none()` check (guarded by the mutex — actually safe), but if init is never called the client is silently a no-op. Callers assuming telemetry is active may silently drop events.
- Files: `crates/codegen/xai-grok-telemetry/src/client.rs:355-386`

**`agent_id` cache tightening:** The `tighten_agent_id_cache_perms` function does a best-effort `fchmod(0o600)` on read. This means between a first-ever run (where the file doesn't exist) and the first read that tightens perms, there is no window — the file is created at 0o600 via `write_atomically`. However, upgrading from an older build that wrote 0o644 only tightens perms on next read, not proactively on upgrade.
- Files: `crates/codegen/xai-grok-telemetry/src/id.rs:83-91`

### Test Coverage Gaps

**Remote-config-activates-telemetry path** is exercised in config resolution unit tests but there is no integration test that verifies telemetry stays off when only `config.toml` sets `telemetry = false` and remote settings override it. The user-vs-remote priority could regress silently.
- Files: `crates/codegen/xai-grok-shell/src/agent/config.rs` test section

---

## Gaps / Needs Human Review

1. **Actual build-time baked values:** The source uses `option_env!()` for `GROK_TELEMETRY_BUILD_EVENTS_URL`, `GROK_TELEMETRY_BUILD_EVENTS_API_KEY`, `GROK_TELEMETRY_BUILD_MIXPANEL_TOKEN`, and `SENTRY_DSN`. This source tree does not reveal what those values are in the released binary. To determine what endpoints and keys the shipped binary contacts, the binary must be inspected with `strings` or a disassembler.
   - Relevant code: `crates/codegen/xai-grok-telemetry/src/config.rs:131-138`, `sentry.rs:40-41`

2. **`agent_instance_id` transmission:** `agent_instance_id()` generates a per-process UUID (`id.rs:29-32`). It is not visibly stamped into `track()` calls in `client.rs`, but it may be included in OTEL span attributes or session trace payloads. Full tracing of all span attribute sites was not completed.

3. **What the session trace archive contains:** The `TraceExportConfig` uploads a file archive of OTEL spans. The exact content depends on what spans the tracing subscriber recorded (controlled by `GROK_OTEL_FILTER`, default `info`). Whether tool inputs/outputs, file contents read by the agent, shell command outputs, or conversation turns are included in these spans was not fully traced. The `otel_layer/redact.rs` file suggests scrubbing is applied, but the span data scope is unclear.
   - Files: `crates/codegen/xai-file-utils/src/gcs.rs`, `crates/codegen/xai-grok-telemetry/src/otel_layer/mod.rs`

4. **Remote config delivery mechanism:** How `remote_settings` is fetched and authenticated is not traced in this analysis. If remote settings are fetched unconditionally before any user consent, the server could learn the user is running grok (via the request) even if telemetry is ultimately `Disabled`. The fetch endpoint and request contents were not identified.

5. **Heap profile uploads:** `resolve_jemalloc_heap_profile` at `agent/config.rs:2315-2328` references `jemalloc_heap_profile_enabled` from remote settings. Heap profiles include memory allocation patterns and potentially object contents. Whether heap profiles are uploaded alongside traces, and to what endpoint, was not fully traced.

6. **`coding_data_sharing_opt_out` server-side effect:** The code stores this flag locally and sends it to the server (via auth metadata persistence), but what the xAI server does with `coding_data_sharing_opt_out = false` — specifically what training pipeline opt-in this triggers — is not verifiable from source alone.

---

*Concerns audit: 2026-07-23 — primary focus: data harvesting, telemetry, and privacy*
