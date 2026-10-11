# Coding Conventions

**Analysis Date:** 2026-07-23

## Language & Toolchain

**Rust edition:** 2024 (declared in `[workspace.package]` in `Cargo.toml`)

**Toolchain:** Stable 1.92.0 pinned in `rust-toolchain.toml`, bumped manually one point release at a time.

**Formatter:** `rustfmt` with a single override in `rustfmt.toml`:
```toml
use_field_init_shorthand = true
```
All struct/enum literals must use field shorthand when field and variable names match.

---

## Naming Patterns

**Crates:** `xai-` prefix for in-house crates (e.g. `xai-grok-telemetry`, `xai-mixpanel`). `ptyctl`, `mermaid-to-svg`, etc. for utility crates. Third-party vendored code lives under `third_party/`.

**Files:** `snake_case.rs`. Test modules within a file named `tests` (`mod tests { … }`). Separate integration test files under `tests/` use descriptive `snake_case_description.rs`.

**Functions/Variables:** `snake_case` throughout (standard Rust).

**Types/Traits:** `PascalCase` (standard Rust).

**Constants:** `SCREAMING_SNAKE_CASE`.

**Modules:** `snake_case` directory/file names; public API re-exported via `pub use` at crate root or `lib.rs`.

---

## Code Style

**Formatting:** `rustfmt` enforced in CI. The only project-level override is `use_field_init_shorthand = true`.

**Linting:** `clippy` via `cargo clippy --all-targets`. Config in `clippy.toml` (crates/ scope) and `[workspace.lints.clippy]` in root `Cargo.toml`.

Key enforced rules in `clippy.toml`:
- `large-error-threshold = 256` — keep error types small (tonic workaround).
- **Banned methods**: `std::fs::canonicalize`, `std::path::Path::canonicalize`, `tokio::fs::canonicalize` — use `dunce::canonicalize` instead (Windows verbatim-path guard). See `clippy.toml` comment.

Workspace-level lints (`Cargo.toml` `[workspace.lints.clippy]`):
- `doc_lazy_continuation = "allow"`, `doc_overindented_list_items = "allow"` — prost-generated code.
- `needless_lifetimes = "allow"`, `too_many_arguments = "allow"`, `single_range_in_vec_init = "allow"`.
- `uninlined_format_args = "allow"` (TODO: flip to `deny` once merge queue is enabled).
- `useless_format = "allow"` — `fastrace::trace` proc-macro expansion workaround.

---

## Import Organization

No enforced import group order beyond `rustfmt` defaults. In practice files follow:
1. `std` / `core` imports
2. External crate imports (alphabetical within block)
3. Workspace-internal crate imports (e.g. `xai_grok_config`, `xai_mixpanel`)
4. `crate::` / `super::` relative imports

Glob imports (`use foo::*`) are rare and avoided except in test modules.

---

## Error Handling

**Application errors:** `anyhow::Error` / `anyhow::Result` for executable binaries and places where error context chains matter. `.context("…")` / `.with_context(|| …)` are used heavily (≈600 call sites) to add context to the error chain.

**Library error types:** `thiserror` (`#[derive(thiserror::Error)]`) for typed, recoverable errors returned from library APIs. Examples: `xai_mixpanel::Error`, `RequirementsError` in `xai-grok-config`.

Pattern:
```rust
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("JSON serialization failed: {0}")]
    Json(#[from] serde_json::Error),
}
```

**`unwrap` / `expect`:** Used freely in test code. In production code `expect` is preferred over `unwrap` and must include a meaningful message explaining why the invariant holds.

**Panics:** `panic = "abort"` in `dev` and `release` profiles. Lock poisoning handled with `lock.unwrap_or_else(|err| err.into_inner())` pattern (common in telemetry globals).

---

## Module Design

**Crate boundaries:** Each logical feature area is its own crate under `crates/codegen/` or `crates/common/`. All crates are workspace members.

**Exports:** Public APIs are re-exported at crate root via `pub use`. Internal implementation details use `pub(crate)` or `pub(super)`. The pattern of `mod foo; pub use foo::SomeType;` in `lib.rs` is standard.

**Barrel files:** `lib.rs` acts as the barrel/re-export file in most crates.

**Feature flags / optional code:** Conditional compilation via `#[cfg(unix)]`, `#[cfg(windows)]`, `#[cfg(test)]`. No Cargo feature flags observed in the workspace crates themselves.

---

## Function Design

**Size:** Functions tend to be focused. Long match arms and complex orchestration is acceptable in the agent/shell layer; utility functions are small.

**Parameters:** Builders (`TestSandboxBuilder`, etc.) and config structs for complex initialization. `impl Into<String>` for string parameters in public APIs.

**Return values:** `Result<T, E>` for fallible operations. `Option<T>` for optional values. Fire-and-forget async tasks via `tokio::spawn` (returning `JoinHandle` only when the caller needs to await or abort).

**Async:** Tokio throughout. `#[tokio::test]` for async tests. `async_trait` used for trait methods (via the `async-trait` crate).

---

## Comments

**Module-level (`//!`):** Every `lib.rs` and most `mod.rs` files have a module doc comment explaining purpose, extraction rationale, and key API contracts.

**Item-level (`///`):** All public items have doc comments. Comments describe *why* decisions were made, not just *what*.

**Inline (`//`):** Used for non-obvious logic, algorithm references, safety invariants, and cross-cutting concerns. Design decisions and known constraints are documented inline (example: the `dunce::canonicalize` ban rationale in `clippy.toml`).

**TODO pattern:** `// TODO: <action> [once <condition>]` — e.g. `// TODO: -> "deny" once/if merge queue enabled`. TODOs name the blocking condition.

---

## Common Derives

Heavy use of `serde::{Serialize, Deserialize}` for config types and wire formats. `strum` for enum string representations (`strum::Display`, `strum::EnumCount`). `derive_more` for `Display`, `From`, `Deref`, etc. `documented` and `schemars` for schema generation.

Global state uses `OnceLock<Mutex<Option<T>>>` for lazily-initialized, optionally-absent singletons (e.g. `TELEMETRY_CLIENT` in `xai-grok-telemetry/src/client.rs`).

---

## Logging & Data-Collection Conventions

### Tracing (local debug logs)

All structured logging uses the **`tracing`** crate (≈4,461 macro usages: `tracing::info!`, `tracing::warn!`, `tracing::error!`, `tracing::debug!`). Key conventions:

- Structured key-value fields are preferred over format strings: `tracing::warn!(key = %value, "message")`.
- Log records stay **local** by default (written to `~/.grok/logs/unified.jsonl` or `~/.grok/debug/<session_id>.txt`).
- Debug firehose activated by `GROK_DEBUG_LOG=1` (per-session files) or `GROK_DEBUG_LOG=<path>` / `GROK_LOG_FILE=<path>` (single file). Implemented in `crates/codegen/xai-grok-telemetry/src/debug_log.rs`.
- The **unified log** (`unified.jsonl`, max 5 MB, rotating) captures cross-component session events locally. Implemented in `crates/codegen/xai-grok-telemetry/src/unified_log.rs`. This log is local-only and is never shipped automatically.

### Product Analytics (outbound to xAI)

Two outbound analytics sinks, both controlled by `TelemetryMode` (see `crates/codegen/xai-grok-telemetry/src/config.rs`):

```rust
pub enum TelemetryMode {
    Disabled,        // default — nothing sent
    SessionMetrics,  // lifecycle-only events, no content
    Enabled,         // full product telemetry
}
```

Resolved via (highest priority first): env var → `[features] telemetry` config key → default `Disabled`.

**Sink 1 — Product events endpoint:** JSON POST to a URL baked in at build time via `GROK_TELEMETRY_BUILD_EVENTS_URL` / `GROK_TELEMETRY_BUILD_EVENTS_API_KEY`. Runtime override via `GROK_TELEMETRY_EVENTS_URL` / `GROK_TELEMETRY_EVENTS_API_KEY`. Sends structured event JSON including `user_id`, `session_id`, `turn_number`, `shell_version`, `subscription_tier`.

**Sink 2 — Mixpanel:** Token baked in at build time via `GROK_TELEMETRY_BUILD_MIXPANEL_TOKEN`. Only active when `TelemetryMode::Enabled`. HTTP POST to `https://api.mixpanel.com/track`. Secret scrub applied before posting (see `crates/codegen/xai-mixpanel/src/lib.rs` `prepare_properties`).

### External OTEL Stream (outbound to customer-controlled collector)

`crates/codegen/xai-grok-telemetry/src/external/` implements an optional OTLP exporter pointed at a customer-supplied collector. **Independent gate from product analytics.** Requires a **double opt-in**:

1. `GROK_EXTERNAL_OTEL=1` (master switch)
2. At least one of `OTEL_METRICS_EXPORTER=otlp` / `OTEL_LOGS_EXPORTER=otlp` (or `console`)

Without both, the stream is inert (zero allocations, zero threads, zero sockets). Can also be activated via the `[telemetry] otel_enabled` config key, with env winning over config.

**Content gates** (both default OFF, opt-in only):
- `OTEL_LOG_USER_PROMPTS=1` — exports prompt text (scrubbed, 60 KB cap)
- `OTEL_LOG_TOOL_DETAILS=1` — exports verbatim tool names, file paths, parameters (all scrubbed)

**Remote kill switch:** `apply_remote_policy` can force-disable the stream or lock content gates off mid-run. The `force_disable` policy clears the emission gate in-process immediately.

**Credential-leak guard:** If the internal xAI firehose consumed `OTEL_EXPORTER_OTLP_*` env vars (deprecated fallback), `internal_pipeline_consumed_otel_vars = true` is set and `external::init` refuses to activate, preventing xAI auth tokens from being sent to a customer collector.

### Error Reporting (Sentry)

`crates/codegen/xai-grok-telemetry/src/sentry.rs` wraps the Sentry SDK. Activated via `SENTRY_DSN` env var (or build-time baked DSN). Disabled when `Config { disabled: true }`. Before sending:
- All home-dir paths collapsed to `~`
- Username segments redacted to `<user>`
- All string values passed through `xai_grok_secrets::redact_secrets`
- `cwd` extra field stripped
- `server_name` cleared
- Broken-pipe and disk-full panics dropped (user-environment noise)

### ZDR (Zero Data Retention)

Teams flagged as ZDR have `data_collection_disabled = true` set on the `WorkspaceHandle` (see `crates/codegen/xai-grok-workspace/src/handle.rs`). This flag:
- Prevents tool-state uploads to cloud storage
- Surfaces as `"data_collection_disabled"` skip reason in workspace upload paths
- Is distinct from `TelemetryMode` — ZDR blocks workspace data exports even when `TelemetryMode::Enabled`

### Secret Scrubbing

All telemetry paths apply `xai_grok_secrets::redact_secrets` / `redact_json_string_values` before any network send. This is enforced at multiple layers (emit time + export-time validators in the external OTEL stream).

---

*Convention analysis: 2026-07-23*
