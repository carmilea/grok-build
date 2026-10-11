# Technology Stack

**Analysis Date:** 2026-07-23

## Languages

**Primary:**
- Rust (edition 2024) — all production code across the entire workspace

**Secondary:**
- None. No Python, TypeScript, or shell scripts in the workspace proper.

## Runtime

**Environment:**
- Native compiled binary. No managed runtime.
- Tokio 1.x async runtime (`features = ["full"]`) throughout.

**Package Manager:**
- Cargo (workspace resolver "2")
- Lockfile: `Cargo.lock` (present — workspace is a deliverable binary)

## Frameworks

**Core:**
- Tokio 1 — async runtime and all I/O
- Clap 4 — CLI argument parsing (`features = ["derive", "env"]`) in `xai-grok-pager`
- Ratatui 0.29 + ratatui-core 0.1 — TUI rendering
- Axum 0.8 — HTTP server used in test harnesses and internal proxy surfaces

**HTTP Client:**
- reqwest 0.12 (`features = ["rustls-tls", "stream", "json", "multipart", "http2", "blocking", "socks"]`) — single HTTP stack for all outbound calls
- reqwest-middleware 0.4.1 — middleware chain (auth retry layer in `xai-grok-auth`)

**Tracing / Observability (internal):**
- tracing 0.1 + tracing-subscriber 0.3 — structured logging
- tracing-opentelemetry 0.33 — bridge from tracing spans to OTEL
- fastrace 0.7 + fastrace-opentelemetry 0.18 + fastrace-reqwest 0.2 + fastrace-tonic 0.1 — distributed span tracing

**Build/Dev:**
- prost 0.14 + tonic 0.14 — protobuf/gRPC (proto codegen via `xai-proto-build`)
- gcloud-storage 1.3.0 — GCS client for direct bucket uploads
- async-openai 0.33.0 (patched fork at `github.com/our-forks/async-openai.git`, rev `95b52eb`) — OpenAI-compatible API types

**Testing:**
- insta 1 — snapshot testing
- mockito 1 — HTTP mock server
- wiremock 0.6 — HTTP mock server
- criterion 0.6 — benchmarks

## Key Dependencies

**Critical:**
- async-openai 0.33.0 (forked) — OpenAI-compatible request/response types for `api.x.ai` and `cli-chat-proxy.grok.com`; pinned via `[patch.crates-io]` in root `Cargo.toml`
- serde 1 + serde_json 1 — universal serialization
- oauth2 5 — OAuth2/PKCE flows for `https://auth.x.ai`
- gix 0.83 — Git operations (libgit2-free)
- tokio-tungstenite 0.27 — WebSocket relay and gateway connections

**Infrastructure:**
- tikv-jemallocator 0.6 (`features = ["profiling"]`) — production allocator
- nix 0.30 — Unix process control, PTY, signals
- rusqlite (via xai-sqlite-journal) — local session journal

## Configuration

**Environment:**
- `GROK_PRODUCTION_CLI_CHAT_PROXY_BASE_URL` — override production proxy base URL
- `GROK_PRODUCTION_ASSET_SERVER_URL` — override asset server URL
- `GROK_PRODUCTION_WS_URL` — override relay WebSocket URL
- `GROK_PRODUCTION_GATEWAY_WS_URL` — override gateway WebSocket URL
- `GROK_CLIENT_NAME` / `GROK_CLIENT_VERSION` — User-Agent identity injection
- `XAI_API_KEY` — direct API key for `api.x.ai`
- `GROK_OIDC_ISSUER` / `GROK_OIDC_CLIENT_ID` / `GROK_OIDC_SCOPES` — custom OIDC config
- `GROK_LOCAL_AUTH=1` — use localhost OAuth2 issuer in dev

**Build:**
- `Cargo.toml` (root workspace) — workspace-level `[profile.release-dist]` for distribution builds
- `[profile.release-dist]` — thin LTO, `codegen-units=1`, `debug=1`, `strip=false`
- `[profile.release-dist-jemalloc]` — alias for desktop release pipeline

## Telemetry / Data-Collection Dependencies

Every crate and dependency involved in analytics, crash reporting, tracing upload, or any phone-home behaviour:

| Crate | Version / Path | Role |
|---|---|---|
| `xai-mixpanel` | `crates/codegen/xai-mixpanel/` | Custom Mixpanel HTTP client — sends `track` and `engage` events to `api.mixpanel.com` |
| `xai-grok-telemetry` | `crates/codegen/xai-grok-telemetry/` | Central telemetry engine: product events + Mixpanel emission + Sentry init + OTLP spans |
| `sentry` | `0.42.0` (`features = ["panic","anyhow","tracing","backtrace","contexts","reqwest","rustls","debug-images"]`) | Crash/error reporting to a Sentry DSN. Declared in `crates/codegen/xai-grok-telemetry/Cargo.toml:41-50` |
| `opentelemetry` | `0.32` | OTEL core SDK. `Cargo.toml` line 187 |
| `opentelemetry_sdk` | `0.32.1` (`features = ["spec_unstable_metrics_views","rt-tokio-current-thread"]`) | OTEL metrics/logs pipeline. `Cargo.toml` line 191 |
| `opentelemetry-otlp` | `0.32` (`features = ["grpc-tonic","reqwest-blocking-client","tls-roots"]`) | OTLP exporter for traces, metrics, and logs. `Cargo.toml` line 189 |
| `opentelemetry-http` | `0.32` | OTEL HTTP propagator. `Cargo.toml` line 188 |
| `tracing-opentelemetry` | `0.33` | Bridge tracing spans → OTEL exporter. `Cargo.toml` line 260 |
| `fastrace` | `0.7` | Lightweight distributed tracing. `Cargo.toml` line 140 |
| `fastrace-opentelemetry` | `0.18` | fastrace → OTEL bridge. `Cargo.toml` line 141 |
| `fastrace-reqwest` | `0.2` | Inject trace context into reqwest calls. `Cargo.toml` line 142 |
| `fastrace-tonic` | `0.1` | Inject trace context into tonic/gRPC calls. `Cargo.toml` line 143 |
| `xai-tracing` | `crates/common/xai-tracing/` | Shared OTEL tracing layer (tonic + reqwest middleware); configures the internal OTLP pipeline |
| `xai-crash-handler` | `crates/codegen/xai-crash-handler/` | Local crash handler — catches Unix signals / Windows SEH, writes crash files to disk. Does NOT phone home independently; Sentry in `xai-grok-telemetry` handles remote reporting |
| `prometheus` | `0.14` (`features = ["process"]`) | Prometheus metrics client. Present in workspace deps (`Cargo.toml` line 199); used server-side |
| `gcloud-storage` | `1.3.0` | Direct GCS upload client for trace/session data. `Cargo.toml` line 152 |
| `whoami` | `1.4` | Collects OS username and locale for Mixpanel `UserContext`. `Cargo.toml` line 276 |
| `mid` | `4.0.0` | Machine-ID generation for stable `agent_id`. `xai-grok-telemetry/Cargo.toml:63` |

**Build-time baking flags** (values compiled into the binary if set at build time):

| Env Var | Effect |
|---|---|
| `GROK_TELEMETRY_BUILD_EVENTS_URL` | Bakes product-events endpoint URL at compile time (`config.rs:131`) |
| `GROK_TELEMETRY_BUILD_EVENTS_API_KEY` | Bakes product-events API key at compile time (`config.rs:132`) |
| `GROK_TELEMETRY_BUILD_MIXPANEL_TOKEN` | Bakes Mixpanel project token at compile time (`config.rs:133`) |
| `SENTRY_DSN` | Bakes Sentry DSN at compile time when `option_env!` resolves (`sentry.rs:41`) |

## Platform Requirements

**Development:**
- Rust stable (edition 2024 requires 1.85+)
- macOS aarch64 or x86_64; Linux x86_64; Windows x86_64
- `nix` crate features require POSIX platform for most PTY/process features

**Production:**
- Statically linked binary distributed via GitHub Releases (`xai-org-shared/grok-build`) or internal channel
- Build profile: `release-dist` (thin LTO, codegen-units=1, debug symbols for post-strip side-cars)

---

*Stack analysis: 2026-07-23*
