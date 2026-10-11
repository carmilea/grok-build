# Codebase Structure

**Analysis Date:** 2026-07-23

## Directory Layout

```
grok-build/
├── Cargo.toml                        # Workspace root (auto-generated, ~70 members)
├── .cargo/                           # Cargo config (linker flags, target overrides)
├── .planning/                        # GSD planning documents
│   └── codebase/                     # Codebase analysis docs (this file)
├── bin/                              # Shell scripts / thin wrappers
├── crates/
│   ├── build/                        # Build-time tooling
│   │   └── xai-proto-build/          # Protobuf codegen helper
│   ├── codegen/                      # Primary product crates (~60 crates)
│   │   ├── xai-grok-pager/           # TUI library (ratatui, diff, search, acp client)
│   │   ├── xai-grok-pager-bin/       # TUI binary entry point (main.rs)
│   │   ├── xai-grok-pager-minimal/   # Minimal pager mode
│   │   ├── xai-grok-pager-render/    # Pager rendering primitives
│   │   ├── xai-grok-pager-pty-harness/  # PTY integration for pager
│   │   ├── xai-grok-shell/           # Backend engine (session, tools, auth, upload)
│   │   ├── xai-grok-shell-base/      # Shell base types
│   │   ├── xai-grok-shell-session-support/  # Session support utilities
│   │   ├── xai-grok-agent/           # Agent logic (prompt, plugins, compaction)
│   │   ├── xai-grok-workspace/       # Workspace server + file system abstraction
│   │   ├── xai-grok-workspace-client/  # Workspace client
│   │   ├── xai-grok-workspace-types/ # Shared workspace types
│   │   ├── xai-grok-telemetry/       # TELEMETRY ENGINE (product events, Mixpanel, Sentry, OTEL)
│   │   ├── xai-mixpanel/             # Mixpanel HTTP client
│   │   ├── xai-grok-auth/            # Auth types + credential provider trait
│   │   ├── xai-grok-config/          # Config loading + managed cache
│   │   ├── xai-grok-config-types/    # Config type definitions
│   │   ├── xai-grok-models/          # Model catalog + model management
│   │   ├── xai-grok-sampler/         # Inference sampling layer
│   │   ├── xai-grok-sampling-types/  # Sampling type definitions
│   │   ├── xai-grok-tools/           # Built-in tool implementations
│   │   ├── xai-grok-tools-api/       # Tool API types
│   │   ├── xai-grok-mcp/             # MCP server protocol
│   │   ├── xai-grok-http/            # HTTP client utilities
│   │   ├── xai-grok-sandbox/         # Sandboxing
│   │   ├── xai-grok-hooks/           # Hook system
│   │   ├── xai-grok-memory/          # Memory/context persistence
│   │   ├── xai-grok-markdown/        # Markdown rendering (full)
│   │   ├── xai-grok-markdown-core/   # Markdown rendering (core)
│   │   ├── xai-grok-mermaid/         # Mermaid diagram rendering
│   │   ├── xai-grok-announcements/   # Server-pushed announcements
│   │   ├── xai-grok-env/             # Environment detection
│   │   ├── xai-grok-paths/           # Path utilities
│   │   ├── xai-grok-secrets/         # Secret scrubbing
│   │   ├── xai-grok-shared/          # Shared shell utilities
│   │   ├── xai-grok-version/         # Version string
│   │   ├── xai-grok-update/          # Auto-update
│   │   ├── xai-grok-voice/           # Voice input
│   │   ├── xai-grok-pager/           # (listed above)
│   │   ├── xai-agent-lifecycle/      # Agent lifecycle contributors
│   │   ├── xai-acp-lib/              # Agent Client Protocol library
│   │   ├── xai-chat-state/           # Chat state management
│   │   ├── xai-codebase-graph/       # Code graph analysis
│   │   ├── xai-crash-handler/        # Crash handler (SIGBUS/SIGSEGV + startup crash detection)
│   │   ├── xai-fast-worktree/        # Fast git worktree cloning
│   │   ├── xai-file-utils/           # File utilities + GCS upload queue
│   │   ├── xai-fsnotify/             # File system notifications
│   │   ├── xai-gix-status/           # Git status via gix
│   │   ├── xai-grok-subagent-resolution/  # Sub-agent resolution
│   │   ├── xai-grok-plugin-marketplace/   # Plugin marketplace
│   │   ├── xai-hooks-plugins-types/  # Hook/plugin type definitions
│   │   ├── xai-hunk-tracker/         # Code change hunk tracking
│   │   ├── xai-prompt-queue/         # Prompt queue
│   │   ├── xai-ratatui-inline/       # Ratatui inline mode
│   │   ├── xai-ratatui-textarea/     # Ratatui textarea widget
│   │   ├── xai-sqlite-journal/       # SQLite journal for session persistence
│   │   ├── xai-system-power/         # System power (battery, sleep)
│   │   ├── xai-token-estimation/     # Token counting
│   │   ├── xai-tracing-macros/       # Tracing macro helpers
│   │   ├── xai-tty-utils/            # TTY utilities
│   │   ├── xai-workflow/             # Workflow orchestration
│   │   ├── ptyctl/                   # PTY control library
│   │   └── ptyctl-cli/               # PTY control CLI
│   └── common/                       # Shared lower-level libraries
│       ├── xai-tracing/              # Tracing dispatch + fastrace + gRPC/HTTP trace clients
│       ├── xai-circuit-breaker/      # Circuit breaker pattern
│       ├── xai-computer-hub-core/    # Computer hub core
│       ├── xai-computer-hub-sdk/     # Computer hub SDK
│       ├── xai-computer-hub-mcp-adapter/  # Computer hub MCP adapter
│       ├── xai-grok-compaction/      # Context compaction algorithms
│       ├── xai-interjection-core/    # Mid-stream injection
│       ├── xai-test-utils/           # Test utilities
│       ├── xai-tool-protocol/        # Tool protocol
│       ├── xai-tool-runtime/         # Tool runtime
│       └── xai-tool-types/           # Tool types
├── prod/
│   └── mc/
│       └── cli-chat-proxy-types/     # Proxy protocol types
└── third_party/                      # Vendored dependencies
    ├── dagre_rust/                   # Dagre layout algorithm (Rust port)
    ├── graphlib_rust/                # Graph library (Rust port)
    ├── mermaid-to-svg/               # Mermaid → SVG conversion
    └── ordered_hashmap/              # Ordered hashmap
```

## Telemetry & Reporting Crates — Call-Out

The following directories **exclusively** house telemetry, metrics, events, reporting, or analytics code. Any change that affects data leaving the machine will touch one of these.

| Crate Path | Responsibility |
|---|---|
| `crates/codegen/xai-grok-telemetry/` | **Primary telemetry engine**: product events, Mixpanel emission, Sentry error reporting, internal OTEL span export, external OTEL stream (enterprise), structured unified log |
| `crates/codegen/xai-mixpanel/` | Lightweight Mixpanel HTTP tracking client (`track` + `engage` APIs) |
| `crates/common/xai-tracing/` | Fastrace + OpenTelemetry dispatch; gRPC/HTTP tracing clients |
| `crates/codegen/xai-tracing-macros/` | Procedural macros for tracing annotations |
| `crates/codegen/xai-crash-handler/` | SIGBUS/SIGSEGV crash handler; writes `last-crash.bin`; startup detection |

Telemetry-adjacent (controls data collection, not emission):
| Crate Path | Responsibility |
|---|---|
| `crates/codegen/xai-grok-secrets/` | Secret scrubbing applied before any external transmission |
| `crates/codegen/xai-grok-shell/src/upload/` | GCS/S3 trace artifact uploads (session artifacts per turn) |
| `crates/codegen/xai-grok-shell/src/auth/manager.rs` | ZDR / `is_data_collection_disabled()` gate |

## Directory Purposes

**`crates/codegen/xai-grok-telemetry/src/`:**
- `client.rs` — Core `TelemetryClient`, `track()`, `sync_profile()`, `init()`, `init_if_needed()`
- `events.rs` — All typed `TelemetryEvent` structs (50+ event types)
- `session_ctx.rs` — Task-local `TelemetryCtx`, `log_event()`, `log_session_event()`, `emit_event_with_origin()`
- `external/` — Enterprise OTEL stream (init, emit, config resolution, schema, redact, truncate)
  - `external/mod.rs` — `ExternalTelemetry` handle, `init()`, `emit()`, `set_identity()`, `shutdown()`
  - `external/config.rs` — Config resolution (`GROK_EXTERNAL_OTEL`, `OTEL_*` env vars)
  - `external/schema.rs` — Wire schema: `ExternalEventName` enum, `ExternalKey` enum, mapping functions
  - `external/emit.rs` — `emit_record()` → scrub → OTEL log record + metric increments
  - `external/redact.rs` — Export-time validators and `RedactingLogExporter`
  - `external/truncate.rs` — Content truncation (60 KB prompt cap, 512→128 value cap)
- `otel_layer/` — Internal OTEL span export to cli-chat-proxy; `build_otel_layer()`, `shutdown_otel()`
  - `otel_layer/redact.rs` — Attribute redaction for internal spans
- `sentry.rs` — Sentry init, `before_send` scrub hook, `flush_on_shutdown()`
- `config.rs` — `TelemetryMode` (Disabled/SessionMetrics/Enabled), `TelemetryConfig`
- `session_metrics.rs` — Session lifecycle event structs (`TraceUploadReason` etc.)
- `memory_telemetry.rs` — Memory usage metrics
- `sampling_log.rs` — Sampling-layer structured log
- `hooks_log.rs` — Hook execution log
- `debug_log.rs` — Per-session debug firehose (local only, not transmitted)
- `unified_log.rs` — Structured log to `~/.grok/logs/` (local only, not transmitted)
- `id.rs` — Stable `agent_id()` for telemetry identity
- `redact_common.rs` — Shared redaction helpers (URL origin extraction, secret scrub)

**`crates/codegen/xai-grok-shell/src/`:**
- `agent/init.rs` — `bootstrap()`, `init_process()`, `update_telemetry_config()` — telemetry init site
- `agent/session_metrics.rs` — Session-level metrics emitters
- `session/telemetry.rs` — Permission decision labels, MCP connection spans, skill source labels
- `upload/trace.rs` — Per-turn GCS artifact uploads: tool definitions, permission events, session metadata
- `upload/gcs.rs` — `TraceExportConfigWithAuth` adapter threading live auth into GCS helpers
- `upload/turn.rs` — `PromptTraceContext`, `UploadWait`, `SyntheticTurnTraceRequest`
- `auth/manager.rs` — `is_data_collection_disabled()`, `allows_data_collection()` (ZDR gate)

**`crates/codegen/xai-grok-shell/src/session/`:**
- `telemetry.rs` — Span helpers
- `inference_metrics.rs` — Per-turn inference metric collection
- `signals.rs` — Turn result signals (feed into GCS upload manifests)

## Key File Locations

**Entry Points:**
- `crates/codegen/xai-grok-pager-bin/src/main.rs`: Primary `grok` binary (`fn main()` at line 1654)
- `crates/codegen/xai-grok-shell/src/agent/app.rs`: `run_headless()`, `run_leader()`, `run_stdio_agent()`
- `crates/codegen/xai-grok-workspace/src/bin/workspace_server.rs`: Remote workspace daemon

**Telemetry Init (call order at startup):**
1. `crates/codegen/xai-grok-pager-bin/src/main.rs:1688` — Sentry init
2. `crates/codegen/xai-grok-pager-bin/src/main.rs:130` — OTEL tracing layer install
3. `crates/codegen/xai-grok-pager-bin/src/main.rs:140` — External OTEL stream init
4. `crates/codegen/xai-grok-shell/src/agent/init.rs:172` — Product events + Mixpanel client init

**Telemetry Configuration:**
- `crates/codegen/xai-grok-telemetry/src/config.rs`: `TelemetryMode`, `TelemetryConfig`
- `crates/codegen/xai-grok-telemetry/src/external/config.rs`: `ExternalOtelConfig::resolve()`
- `crates/codegen/xai-grok-shell/src/agent/config.rs`: `resolve_telemetry_mode()`, `resolve_trace_upload()`, `resolve_external_otel_config()`

**Core Telemetry Logic:**
- `crates/codegen/xai-grok-telemetry/src/client.rs`: `track()` (dual HTTP POST: product events + Mixpanel)
- `crates/codegen/xai-grok-telemetry/src/session_ctx.rs`: `log_event()`, `emit_event_with_origin()`
- `crates/codegen/xai-grok-telemetry/src/external/emit.rs`: `emit_record()` (external OTEL)
- `crates/codegen/xai-grok-telemetry/src/sentry.rs`: `init()`, `before_send()` scrub hook
- `crates/codegen/xai-mixpanel/src/lib.rs`: `Mixpanel::track()` → POST `https://api.mixpanel.com/track`

**Event Schema:**
- `crates/codegen/xai-grok-telemetry/src/events.rs`: All 50+ typed event structs + `telemetry_event!` macro bindings
- `crates/codegen/xai-grok-telemetry/src/external/schema.rs`: External OTEL wire schema + mapping functions

**Session/Auth:**
- `crates/codegen/xai-grok-shell/src/auth/manager.rs`: `AuthManager` with ZDR gate
- `crates/codegen/xai-grok-shell/src/agent/init.rs`: Config resolution + telemetry init
- `crates/codegen/xai-grok-shell/src/session/mod.rs`: Session lifecycle hub

**Upload Pipeline:**
- `crates/codegen/xai-grok-shell/src/upload/trace.rs`: All per-turn artifact upload functions
- `crates/codegen/xai-grok-shell/src/upload/gcs.rs`: Auth-aware GCS adapter

**Testing:**
- `crates/codegen/xai-grok-telemetry/tests/`: Integration tests for external OTEL (gRPC, HTTP, gates)
- `crates/codegen/xai-grok-telemetry/src/external/tests.rs`: Unit tests for emit path and schema

## Naming Conventions

**Files:**
- `snake_case.rs` throughout (standard Rust)
- Crates prefixed `xai-grok-*` for product crates, `xai-*` for utilities
- Test files: `tests.rs` alongside source, or `*_tests.rs` for large test modules

**Directories:**
- Feature groupings: `agent/`, `session/`, `tools/`, `auth/`, `upload/`, `external/`
- Binaries: `src/bin/<name>.rs` or `src/main.rs`

**Types:**
- Events: `PascalCase` structs implementing `TelemetryEvent` trait
- Enums: `PascalCase` with `#[serde(rename_all = "snake_case")]` for wire format
- Global singletons: `OnceLock<...>` at module level, ALL_CAPS name

## Where to Add New Code

**New telemetry event:**
1. Add struct to `crates/codegen/xai-grok-telemetry/src/events.rs`
2. Add `telemetry_event!(MyEvent, "my_event_name")` binding at the bottom of `events.rs`
3. Optionally add `external = map_my_event` arm if the event should flow to the external OTEL stream
4. Add the mapping function in `crates/codegen/xai-grok-telemetry/src/external/schema.rs`
5. Call `log_event(MyEvent { ... })` or `log_session_event(MyEvent { ... })` at the call site

**New external OTEL event name:**
- Add variant to `ExternalEventName` enum in `external/schema.rs:27`
- Add `as_str()` branch — this is a public schema commitment

**New external OTEL attribute key:**
- Add variant to `ExternalKey` enum in `external/schema.rs` (~line 80)
- The `EXTERNAL_ALLOWED_KEYS` test pins the closed set; adding a variant trips it

**New session-layer feature:**
- Implementation: `crates/codegen/xai-grok-shell/src/session/<feature>.rs`
- Tests: `crates/codegen/xai-grok-shell/src/session/<feature>_tests.rs` or inline `#[cfg(test)]`

**New tool:**
- Built-in tool: `crates/codegen/xai-grok-tools/src/`
- Tool API types: `crates/codegen/xai-grok-tools-api/src/`

**New UI component:**
- `crates/codegen/xai-grok-pager/src/` — follow existing ratatui widget pattern

**Shared utilities:**
- Shared across pager + shell: `crates/common/<new-crate>/`
- Shell-internal helpers: `crates/codegen/xai-grok-shell/src/util/`

## Special Directories

**`crates/codegen/xai-grok-shell/src/upload/`:**
- Purpose: All egress data pipelines (GCS/S3 artifact uploads)
- Generated: No
- Committed: Yes

**`crates/codegen/xai-grok-telemetry/`:**
- Purpose: All analytics and observability emission
- Generated: No
- Committed: Yes
- CODEOWNERS: Yes (telemetry-owner review required for `external/schema.rs` changes)

**`third_party/`:**
- Purpose: Vendored Rust dependencies requiring fork or modification
- Generated: No
- Committed: Yes

**`prod/mc/`:**
- Purpose: Shared types for the cli-chat-proxy (multi-client/multi-session proxy service)
- Generated: No
- Committed: Yes

**`~/.grok/` (runtime, not committed):**
- `~/.grok/sessions/` — Local session persistence (SQLite + JSONL)
- `~/.grok/logs/` — Structured debug log firehose (`unified_log`)
- `~/.grok/upload_queue/` — Durable GCS upload queue spill files
- `~/.grok/memtrace/` — Memory trace output
- `~/.grok/crash/` — Crash dump files from `xai-crash-handler`

---

*Structure analysis: 2026-07-23*
