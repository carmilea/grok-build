# Architecture

**Analysis Date:** 2026-07-23

## Pattern Overview

**Overall:** Layered plugin architecture — a TUI frontend (pager) driving a backend engine (shell) via the Agent Client Protocol (ACP), with a standalone workspace server for remote/multi-agent use cases.

**Key Characteristics:**
- Rust Cargo workspace with ~70 crates split into `codegen` (product), `common` (shared libraries), `build` (tooling), and `third_party` (vendored deps)
- Process model: the pager binary spawns or connects to a shell process; they communicate over ACP (a versioned local protocol)
- Agent-server split: `xai-grok-shell` owns all session logic; `xai-grok-pager` / `xai-grok-pager-bin` own the terminal UI; `xai-grok-workspace` is a remote daemon
- All configuration resolved in a layered stack: defaults → managed policy → user config file → remote settings → env overrides

## Layers

**TUI / Frontend:**
- Purpose: Terminal rendering, user input, keyboard shortcuts, multi-agent dashboard
- Location: `crates/codegen/xai-grok-pager/`, binary entry point `crates/codegen/xai-grok-pager-bin/src/main.rs`
- Contains: ratatui-based UI, clipboard, voice, diff/search, ACP client
- Depends on: `xai-grok-shell` (backend API), `xai-grok-telemetry`, `xai-grok-auth`
- Used by: End users via the `grok` binary

**Shell Engine / Backend:**
- Purpose: Session lifecycle, inference, tool execution, permissions, trace uploads, config resolution
- Location: `crates/codegen/xai-grok-shell/src/`
- Contains: `agent/`, `session/`, `tools/`, `auth/`, `upload/`, `sampling/`, `config/`
- Depends on: Most `common` crates, `xai-grok-agent`, `xai-grok-telemetry`, `xai-grok-workspace`
- Used by: Pager (TUI), headless mode (`grok -p`), workspace server

**Agent Layer:**
- Purpose: Prompt assembly, system prompt, plugin/skill/hook discovery, compaction
- Location: `crates/codegen/xai-grok-agent/src/`
- Contains: `agent.rs`, `builder.rs`, `compaction.rs`, `prompt/`, `plugins/`
- Depends on: `xai-grok-sampling-types`, `xai-grok-config`, `xai-grok-tools`
- Used by: `xai-grok-shell`

**Workspace Server:**
- Purpose: Remote agent daemon, file system abstraction, foreign session support (Claude, Codex), permission gating
- Location: `crates/codegen/xai-grok-workspace/src/`
- Contains: `bin/workspace_server.rs`, `file_system/`, `foreign_sessions/`, `session/`, `permission/`
- Depends on: `xai-grok-shell`, `xai-grok-telemetry`
- Used by: Multi-agent and remote sessions

**Common / Shared Libraries (`crates/common/`):**
- `xai-tracing`: fastrace + OpenTelemetry dispatch helpers, gRPC/HTTP tracing clients
- `xai-circuit-breaker`: resilience pattern for external calls
- `xai-tool-runtime`, `xai-tool-protocol`, `xai-tool-types`: tool execution abstractions
- `xai-grok-compaction`: context-window compaction algorithms
- `xai-interjection-core`: mid-stream injection mechanism

## Data Flow

**User Prompt → Response:**
1. User types in pager (`xai-grok-pager`) → ACP `SendPrompt` message
2. Shell (`xai-grok-shell/src/session/`) receives message, assembles prompt context
3. Agent (`xai-grok-agent`) builds system prompt + conversation history
4. Sampling layer calls xAI API (`xai-grok-sampler`, `xai-grok-models`)
5. Response streams back through session → ACP → pager for rendering
6. Tool calls intercepted: permissions checked, tool executed, result fed back into inference loop
7. Turn-end: telemetry events emitted, trace artifacts queued for upload

**Session Persistence:**
- Local: `~/.grok/sessions/<session_id>/` — SQLite journal + JSONL chat history
- Remote: per-turn GCS trace uploads (session state archive, tool definitions, permission events)
- Session state upload: implemented as stub (`upload_session_state` returns `Failed`) in this build

**Config Resolution Stack (highest priority last):**
1. Hardcoded defaults in `TelemetryConfig::default()` / `AgentConfig::default()`
2. User config file (`~/.grok/settings.toml`, `[telemetry]` table)
3. Managed policy (`managed_config.toml` from enterprise policy sync)
4. Remote settings (fetched from xAI backend post-auth via `start_early_prefetch()`)
5. Environment variable overrides (`GROK_TELEMETRY_*`, `GROK_EXTERNAL_OTEL`, etc.)

**State Management:**
- Global telemetry client: `OnceLock<Mutex<Option<TelemetryClient>>>` in `xai-grok-telemetry/src/client.rs`
- Session context: `tokio::task_local!` `TELEMETRY_CTX` carrying `session_id` and `prompt_index`
- Auth state: `Arc<AuthManager>` threaded through shell, holds `current()` credential snapshot
- External OTEL handle: `OnceLock<Option<Arc<ExternalTelemetry>>>` — never registered as global

## Telemetry & Data-Flow Paths

This section traces every data path by which user/usage data can leave the machine. There are four independent egress channels.

---

### Channel 1 — Product Events + Mixpanel (xAI-internal analytics)

**What leaves:** Structured event payloads (event name, metadata fields: session ID, turn number, model ID, tool name, token counts, timing, subscription tier). Prompt text is **NOT included** (`#[serde(skip)]` on `PromptSubmitted.prompt_text`). File paths are **NOT included** in default events.

**Lifecycle:**

```
call site: log_event(SomeEvent { ... })
  → crates/codegen/xai-grok-telemetry/src/session_ctx.rs:137  log_event()
      ┣━ crates/codegen/xai-grok-telemetry/src/external/mod.rs:232  external::emit()  [→ Channel 3]
      ┗━ session_ctx::emit_event_with_origin()  [line 202]
           → tokio::spawn (async task, fire-and-forget)
               → UserContext::collect()  [collects locale + timestamp from OS]
               → client::track()  [crates/codegen/xai-grok-telemetry/src/client.rs:172]
                   ┣━ HTTP POST to events_url  [xAI product events API, configured at build time]
                   ┗━ Mixpanel::track()  [crates/codegen/xai-mixpanel/src/lib.rs:60]
                        → HTTP POST https://api.mixpanel.com/track  [base64-encoded JSON]
```

**Origin:** `log_event()` / `log_session_event()` call sites throughout `xai-grok-shell/src/session/`, `xai-grok-pager/`, `xai-grok-workspace/`

**Buffering:** None (each event spawns its own `tokio::spawn`). No batching on the product-events path.

**Flush/Transmission:** HTTP POST, fire-and-forget, 10-second timeout. No retry.

**Gates:**
- `TelemetryMode::Disabled` → global client `None`, `track()` returns immediately (`client.rs:177`)
- `TelemetryMode::SessionMetrics` → only `log_session_event()` calls route to Mixpanel/product events
- `TelemetryMode::Enabled` → all `log_event()` calls fire
- ZDR team flag (`auth/manager.rs:741 is_data_collection_disabled()`) → `log_event_dual` routes external only, not product events
- Controlled by: `[features] telemetry` config key, env `GROK_TELEMETRY_ENABLED`

**Initialization:** `crates/codegen/xai-grok-shell/src/agent/init.rs:189` calls `xai_grok_telemetry::client::init()` once at process start (guarded by `std::sync::Once`) and again after auth via `update_telemetry_config()`

**Secret scrubbing:** Mixpanel path: `xai_grok_secrets::redact_json_string_values()` applied in `xai-mixpanel/src/lib.rs:51`

---

### Channel 2 — Internal OTEL Span Export (xAI observability backend)

**What leaves:** OpenTelemetry spans with structured attributes: `session_id`, tool timings, inference latency, tool names, model IDs, permission decisions. Spans are created with `tracing::info_span!()` throughout the session layer.

**Lifecycle:**

```
tracing::info_span!("agent.prompt", ...) or #[fastrace::trace]
  → tracing-opentelemetry layer  [installed in pager-bin/src/main.rs:130]
      → BatchSpanProcessor  (OpenTelemetry SDK)
          → OTLP exporter  [crates/codegen/xai-grok-telemetry/src/otel_layer/mod.rs]
              → HTTP POST to traces_url  (e.g. https://cli-chat-proxy.grok.com/v1/traces)
              → Auth header: Bearer token from AuthCredentialProvider (live grok.com token)
```

**Buffering:** BatchSpanProcessor queues spans in-memory; flushed on `shutdown_otel()` at process exit.

**Gates:** Endpoint configured in managed config / env; disabled when no traces URL is resolved.

**Redaction:** `crates/codegen/xai-grok-telemetry/src/otel_layer/redact.rs` filters attributes before export.

---

### Channel 3 — External OTEL Stream (enterprise opt-in, customer's own collector)

**What leaves:** Curated set of 18 log-record event types and 6 metric counters, sent to the **customer's own** OTLP collector (not xAI). Schema defined in `crates/codegen/xai-grok-telemetry/src/external/schema.rs`.

**Event types (wire names):**
`grok_code.session_start`, `grok_code.session_end`, `grok_code.user_prompt`, `grok_code.turn_completed`, `grok_code.api_request`, `grok_code.api_error`, `grok_code.tool_result`, `grok_code.tool_decision`, `grok_code.mcp_server_connection`, `grok_code.permission_mode_changed`, `grok_code.skill_activated`, `grok_code.plugin_loaded`, `grok_code.compaction`, `grok_code.subagent`, `grok_code.auth`, `grok_code.internal_error`, `grok_code.model_switched`, `grok_code.contextual_tip`

**Metric counters:** `session.count`, `token.usage`, `turn.count`, `tool.decision`, `tool.usage`, `error.count`

**Lifecycle:**

```
log_event(SomeEvent) or log_session_event(SomeEvent)
  → crates/codegen/xai-grok-telemetry/src/session_ctx.rs:138  external::emit(&data)
      → data.external_record()  [mapping fn in external/schema.rs]
          → emit::emit_record()  [crates/codegen/xai-grok-telemetry/src/external/emit.rs:92]
              → scrub_string() applied to every string attribute
              → BatchLogProcessor.emit(log_record)  [queued, not blocking]
              → metric counter increments via Instruments
  [background]
  BatchLogProcessor flushes on interval (default 5s) or forced_flush() at shutdown
      → OTLP HTTP/gRPC export to customer endpoint (OTEL_EXPORTER_OTLP_ENDPOINT)
```

**Initialization:** `crates/codegen/xai-grok-pager-bin/src/main.rs:140` calls `xai_grok_telemetry::external::init()` once at process start.

**Content gates (both default OFF):**
- `OTEL_LOG_USER_PROMPTS=1` → includes prompt text (60 KB cap, secret-scrubbed) on `grok_code.user_prompt`
- `OTEL_LOG_TOOL_DETAILS=1` → includes tool parameters (4 KB / depth 2 / 20 items), full file paths, verbatim MCP/skill/plugin names on `grok_code.tool_result`

**Double opt-in required:**
- `GROK_EXTERNAL_OTEL=1` (master switch) **AND** `OTEL_METRICS_EXPORTER=otlp` and/or `OTEL_LOGS_EXPORTER=otlp`
- Neither alone activates the stream

**Remote kill switch:** Fleet policy can force-disable (`force_disable`) or lock content gates off (`lock_content_gates`) via `apply_remote_policy()` at `external/mod.rs:281`. This is tighten-only — remote cannot enable what env left off.

**Auth isolation:** External exporters carry only customer-provided `OTEL_EXPORTER_OTLP_HEADERS`. No xAI auth token is ever attached (`external/mod.rs:14-17`).

---

### Channel 4 — Sentry Crash/Error Reporting

**What leaves:** Panic messages, exception stack traces, breadcrumbs. Scrubbed before send.

**Lifecycle:**

```
panic / unhandled exception
  → sentry SDK before_send hook  [crates/codegen/xai-grok-telemetry/src/sentry.rs:193]
      → Scrubber::scrub() applied: home dir → "~", usernames → "<user>", secrets → "[REDACTED_SECRET]"
      → `cwd` key removed from event.extra
      → server_name set to None
      → broken-pipe and disk-full panics dropped entirely
  → Sentry SDK → HTTP POST to SENTRY_DSN endpoint
```

**Gate:** `SENTRY_DSN` env var (or baked at build time). Disabled when `config.disabled = true` (`is_error_reporting_disabled_sync()` in `xai-grok-shell`). Init: `pager-bin/src/main.rs:1688`.

**Traces sample rate:** 1% (`TRACES_SAMPLE_RATE = 0.01` in `sentry.rs:10`).

---

### Channel 5 — GCS Trace Artifact Uploads (per-turn session artifacts)

**What leaves:** JSON artifacts per turn: `tool_definitions.json`, `permission_events.json`, memory snapshots, session metadata. Conversation content upload (`turn_messages.json`) is currently **disabled** in this build (stub returns `chat_content_upload_disabled`). Session-state archive upload is also **disabled** (stub returns `session_state_upload_unavailable`).

**Lifecycle:**

```
Turn completes in xai-grok-shell/src/session/
  → upload/trace.rs: upload_tool_definitions(), upload_permission_events(), etc.
      ┣━ Direct path: xai_file_utils::gcs::upload_bytes() → HTTP PUT to GCS bucket or S3
      ┗━ Queue path:  UploadQueue::spawn()  [persistent disk queue under ~/.grok/upload_queue/]
           → background worker thread
               → retry on transient failures (UploadRetryPolicy::default())
               → HTTP upload to configured bucket

ZDR check: auth/manager.rs:741 is_data_collection_disabled()
  → true (ZDR team) → trace uploads skipped entirely
```

**Upload methods (mutually exclusive, resolved in `agent/config.rs:496`):**
- `UploadMethod::Proxy` — via grok.com proxy using user's auth token
- `UploadMethod::S3` — direct to customer-owned S3 bucket (ZDR-compatible)
- `UploadMethod::Direct` — direct to xAI GCS bucket with service account

**Queue:** `UploadQueue` in `xai-file-utils`, spills to disk at `~/.grok/upload_queue/`. Recovered across process restarts.

**Gates:**
- ZDR team flag → all uploads disabled
- `[telemetry] trace_upload = false` → uploads disabled (`TraceUploadReason::FeatureOff`)
- No auth → `TraceUploadReason::NoCredentials`

---

### Data NOT Transmitted

- Prompt/response text: explicitly `#[serde(skip)]` on product events and Mixpanel paths (`events.rs:869`)
- File contents: never included in any automatic upload
- Conversation history: `upload_turn_messages()` is a disabled stub
- Session state archives: `upload_session_state()` is a disabled stub
- All channels scrub secrets via `xai_grok_secrets::redact_*` before transmission

## Key Abstractions

**TelemetryClient:**
- Purpose: Core emitter for product events and Mixpanel
- Location: `crates/codegen/xai-grok-telemetry/src/client.rs`
- Pattern: Global singleton via `OnceLock<Mutex<Option<TelemetryClient>>>`

**TelemetryCtx / TelemetryEvent:**
- Purpose: Task-local session context + typed event trait for type-safe emission
- Location: `crates/codegen/xai-grok-telemetry/src/session_ctx.rs`, `events.rs`
- Pattern: `tokio::task_local!` scope set by `with_session_ctx()` wrapping session futures

**ExternalTelemetry:**
- Purpose: Enterprise OTEL stream handle
- Location: `crates/codegen/xai-grok-telemetry/src/external/mod.rs`
- Pattern: `OnceLock<Option<Arc<ExternalTelemetry>>>` — never global

**AuthManager:**
- Purpose: Live credential source, ZDR/data-collection flag carrier
- Location: `crates/codegen/xai-grok-shell/src/auth/manager.rs`
- Pattern: `Arc<AuthManager>` threaded throughout shell

**UploadQueue:**
- Purpose: Durable, disk-backed async upload queue for trace artifacts
- Location: `xai-file-utils` crate (referenced from `crates/codegen/xai-grok-shell/src/upload/trace.rs:1325`)
- Pattern: `UploadQueue::spawn()` returns a handle; background worker retries failed uploads

## Entry Points

**TUI mode (`grok`):**
- Location: `crates/codegen/xai-grok-pager-bin/src/main.rs:1654`
- Triggers: User launches `grok` CLI
- Responsibilities: Parse args, init Sentry + OTEL + external OTEL, install tracing, connect/spawn shell process, run TUI event loop

**Headless mode (`grok -p`):**
- Location: `crates/codegen/xai-grok-shell/src/agent/app.rs` via `run_headless()`
- Triggers: `grok -p` or API client spawning shell process
- Responsibilities: ACP stdio transport, session lifecycle without TUI

**Workspace server:**
- Location: `crates/codegen/xai-grok-workspace/src/bin/workspace_server.rs`
- Triggers: Spawned for remote/multi-agent scenarios
- Responsibilities: File system abstraction, foreign session proxying, remote tool execution

**Shell bootstrap:**
- Location: `crates/codegen/xai-grok-shell/src/agent/init.rs:21`
- Triggers: Called by every mode (TUI, headless, workspace)
- Responsibilities: Managed policy gate, config resolution, telemetry init, model catalog init

## Error Handling

**Strategy:** `anyhow::Result` for internal errors; typed `thiserror` enums for API boundaries; telemetry uploads are best-effort and never propagate failures to the user

**Patterns:**
- Upload failures: logged at configurable level (Error for first-party, Warn for customer S3), never surfaced in UI
- Auth failures: `AuthManager` retries transparently; terminal 401s emit `ManualAuth` event and prompt re-login
- Telemetry failures: all `track()` and `upload_bytes()` failures are swallowed (`let _ = ...`)

## Cross-Cutting Concerns

**Logging:** `tracing` crate throughout; `unified_log` in `xai-grok-telemetry` for structured per-session logs to `~/.grok/logs/`
**Validation:** Config validated in `AgentConfig::validate_model_filters()`; managed policy gate runs first (fail-closed)
**Authentication:** `AuthManager` in `xai-grok-shell/src/auth/manager.rs`; credential provider pattern via `AuthCredentialProvider` trait
**Secret scrubbing:** `xai-grok-secrets` crate; applied in Mixpanel client, Sentry before-send hook, external OTEL emit path, and otel_layer redact module

---

*Architecture analysis: 2026-07-23*
