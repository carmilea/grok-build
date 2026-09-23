# Testing Patterns

**Analysis Date:** 2026-07-23

## Test Framework

**Runner:** Cargo's built-in `cargo test`. No external test runner (e.g. nextest is not pinned, though it would be compatible).

**Async runtime:** `tokio` via `#[tokio::test]`. Two flavors used:
- `#[tokio::test]` — defaults to `current_thread` runtime for most unit/integration tests.
- `#[tokio::test(flavor = "multi_thread", worker_threads = N)]` — for tests that exercise actual concurrency or spawn tasks.
- `#[tokio::test(flavor = "current_thread")]` — explicit single-thread (used when the test must prove something about runtime absence for safety).

**Assertion libraries:**
- Standard `assert!` / `assert_eq!` / `assert_ne!` — primary.
- `assert_matches` — for pattern-matching assertions (`Cargo.toml` workspace dep).
- `pretty_assertions` — for readable struct/string diffs in render/UI tests.
- `insta` — snapshot testing (only 2 files currently; not a primary pattern).

**Run commands:**
```bash
cargo test --workspace                     # All tests
cargo test -p xai-grok-telemetry          # Single crate
cargo test -- --nocapture                 # Show stdout
cargo bench                               # Run benchmarks (criterion)
```

---

## Test File Organization

**Inline unit tests:** `#[cfg(test)] mod tests { … }` at the bottom of the source file. The dominant pattern. ~1,110 files contain inline test modules.

**Separate integration tests:** `tests/` directory sibling to `src/` inside a crate. Named `test_<description>.rs` or `<feature>_integration.rs`. Used for end-to-end or subprocess tests.

**Sub-directory test modules:** Some crates place larger test suites in subdirectories:
- `crates/codegen/xai-grok-pager/src/app/acp_handler/tests/`
- `crates/codegen/xai-grok-shell/src/agent/mvp_agent/tests/`

**Shared test infrastructure:** `crates/codegen/xai-grok-test-support/` — a dedicated crate exposing:
- `MockInferenceServer` — in-process HTTP server serving `/v1/chat/completions`, `/v1/responses`, `/v1/messages` with request logging.
- `TestSandbox` / `TestSandboxBuilder` — hermetic filesystem and child process environment.
- `GrokStdioClient` / `RawStdioClient` — ACP stdio client for agent integration tests.
- `LeaderFixture` — leader-process integration (Unix only).
- `run_headless` — spawn `grok -p` against a mock server and capture output.

---

## Test Structure

**Sync unit test pattern:**
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptive_behavior_name() {
        // Arrange
        let value = MyType::new(...);
        // Act
        let result = value.method();
        // Assert
        assert_eq!(result, expected);
    }
}
```

**Async test pattern:**
```rust
#[tokio::test]
async fn posts_to_events_endpoint() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/events", listener.local_addr().unwrap());
    // … set up in-process server …
    // Call production code
    // Poll / await result
}
```

**Test naming:** Descriptive snake_case names that state the invariant being tested, not just the method name. Examples: `sync_profile_is_noop_in_session_metrics_mode`, `double_opt_in_activates`, `secret_survived_in_prompt`.

**Comments in tests:** Tests document their own safety invariants and regression history inline. Canary constants are named (e.g. `const CANARY_PROMPT: &str = "CANARY_PROMPT_TEXT do not export"`) for clarity in failure messages.

---

## Mocking

**HTTP endpoints:** Two approaches:
1. **axum in-process server** — `tokio::net::TcpListener::bind("127.0.0.1:0")` for a random port, served in a `tokio::spawn`. Used for product events endpoint tests (`tests/manual_auth_emit.rs`) and S3 mock in `xai-file-utils`.
2. **mockito** — used in `crates/codegen/xai-grok-auth/src/retry_middleware.rs` for retry behavior tests.
3. **wiremock** — used in `crates/common/xai-tracing/src/http_client.rs` for traced HTTP client tests.
4. **`MockInferenceServer`** (xai-grok-test-support) — the primary mock for LLM API endpoints in agent integration tests; supports scripted SSE response sequences and request log inspection.

**What to mock:**
- External HTTP endpoints (LLM APIs, telemetry collectors, auth servers).
- Filesystem — `TestSandbox` provides isolated temp trees.
- Process environment — `EnvGuard` / `TestSandbox` baseline env; never mutate process-global env without cleanup.

**What NOT to mock:**
- The Rust standard library.
- `tokio` runtime internals.
- Production business logic under test — prefer thin wrappers with real implementations.

**OTLP collector mock:** For telemetry wire tests, `tests/otlp_collector/mod.rs` in `xai-grok-telemetry` runs a real OTLP HTTP or gRPC server in-process on a random port (axum for HTTP, tonic for gRPC). See section on telemetry testing below.

---

## Fixtures and Factories

**Test data:** Inline constant structs or factory functions. Example from `xai-grok-telemetry/src/external/tests.rs`:
```rust
fn sentinel_session_harness() -> events::SessionHarness {
    events::SessionHarness {
        session_id: "sess-1".into(),
        model_id: "grok-4".into(),
        // …
    }
}
```

**Canary constants:** Secret-shaped strings named `CANARY_*` are planted in test inputs to verify they do NOT appear in outputs (scrub/gate validation):
```rust
const CANARY_MODEL: &str = "sk-CANARYabcdefghij1234567890";
const CANARY_PROMPT: &str = "CANARY_PROMPT_TEXT do not export";
```

**Sandbox:** `TestSandbox::builder().git().build()` for git-initialized workspaces. The builder API sets all hermetic env (see below).

**Location:** No centralized fixtures directory; factories live in the same file as the tests that use them, or in `xai-grok-test-support/src/`.

---

## Test Isolation

**`TestSandbox` hermetic env:** Every integration test process inherits from `TestSandbox`'s baseline env, which hard-wires all telemetry, update, and feedback kill switches:
```
GROK_TELEMETRY_ENABLED=false
GROK_TELEMETRY_TRACE_UPLOAD=false
GROK_FEEDBACK_ENABLED=false
GROK_TRACE_UPLOAD=false
GROK_INSTRUMENTATION=disabled
OTEL_SDK_DISABLED=true
DISABLE_TELEMETRY=1
DISABLE_FEEDBACK_COMMAND=1
GROK_DISABLE_AUTOUPDATER=1
NO_PROXY=127.0.0.1,localhost,::1
```
Defined in `crates/codegen/xai-grok-test-support/src/sandbox.rs`. This ensures **no test ever phones home** through the product analytics or Mixpanel paths unless the test itself deliberately constructs and initializes a `TelemetryClient`.

**Serial tests:** `serial_test::serial` attribute used in `xai-fsnotify` for filesystem-event tests that compete for inotify watches. Declared via `use serial_test::serial; #[serial]`.

**Global state:** Telemetry globals (`TELEMETRY_CLIENT: OnceLock<Mutex<Option<TelemetryClient>>>`) are reset in tests that need to modify them. The `sync_profile_is_noop_in_session_metrics_mode` test in `client.rs` uses a RAII guard (`ClearClient`) with `Drop` to guarantee cleanup even on panic.

---

## Telemetry & Reporting Code Paths in Tests

This is the highest-coverage area in the test suite. All telemetry test code lives in `crates/codegen/xai-grok-telemetry/`.

### Unit Tests (inline in source files)

**`src/client.rs`** — Tests for `event_value` prefix stripping, `normalize_tier` mappings, `sync_profile` gate, `EmitterOrigin` prefix invariants. The `sync_profile_is_noop_in_session_metrics_mode` test runs **without a tokio runtime by design**: if the gate wrongly allows `tokio::spawn`, the test panics.

**`src/config.rs`** — Tests for `build_env_default` normalization, `TelemetryConfig::default()` layer resolution, env override precedence.

**`src/sentry.rs`** — Tests for home-dir path collapse, username segment redaction, broken-pipe panic suppression, tag/breadcrumb/extra scrubbing. All against the `before_send` function directly (no Sentry SDK network calls).

**`src/session_ctx.rs`** — Tests that `session_span` exposes the `session_id` field required by the debug-log router, that `EmitterOrigin::ALL` covers every variant, that prefix strings are stable.

**`src/external/config.rs`** — Tests for the entire config resolution matrix: double opt-in, endpoint resolution (HTTP vs gRPC), header parsing/scoping, content gate defaults, interval parsing, file config layering.

**`src/external/tests.rs`** — Largest telemetry unit test file. Uses an in-memory `TestStream` harness that wraps `ExternalTelemetry` with in-memory log/metric exporters (OpenTelemetry `InMemoryLogExporter` / `InMemoryMetricExporter`). Tests:
- **Pinned allowlists** — `external_allowed_keys_are_pinned()`, `metric_attr_keys_are_pinned()`, `event_names_are_pinned()`. These tests maintain an independent hardcoded copy of the expected keys; adding a new key without updating the pin fails the test, preventing silent schema expansion.
- **Schema snapshots** — Per-event: what attributes appear, whether they respect gate state, what metric increments fire.
- **Canary leak tests** — `tool_result_gates_off_collapses_and_reduces()` plants `"CANARY_TOOL_ARGS"` in tool parameters and asserts it does not appear in the debug representation of exported records.
- **Gate enforcement** — `user_prompt_gates_off_drops_text()`: prompt text absent when gate off. `user_prompt_gate_on_exports_scrubbed_text()`: prompt present but secret stripped when gate on.
- **Export-time validators (fail-closed)** — `validating_metric_exporter_drops_export_on_bad_attr_key()`: a rogue `"prompt"` attribute on a metric must cause the whole export to be dropped, not passed through. `redacting_log_exporter_drops_record_with_closed_gate_key()` and `redacting_log_exporter_drops_record_with_unknown_key()`.
- **Remote policy** — `remote_force_disable_stops_emission()`, `remote_gate_lock_forces_gates_off_and_never_on()`.
- **Exactly-once counting** — `one_failed_turn_increments_error_count_exactly_once()`: three events for a failed turn must produce exactly one `error.count` increment.

**`src/mixpanel` (lib.rs inline)** — `prepare_properties_scrubs_then_injects_token()`: uses a Bearer-shaped project token to verify scrub runs before token injection (not after).

### Integration Tests (tests/ directory, in-process OTLP collector)

**`tests/otlp_collector/mod.rs`** — Shared in-process OTLP collector. Two transport modes:
- **HTTP/protobuf**: axum server on a random port binding `/v1/logs` and `/v1/metrics`
- **gRPC**: tonic `LogsServiceServer` + `MetricsServiceServer` on a random port

Both store raw protobuf bytes in `Arc<Mutex<Vec<Vec<u8>>>>`. Decode helpers (`decode_logs`, `decode_metrics`, `log_records`, `metric_points`, `find_event`, `find_metric`) parse the wire protocol for assertions.

**`tests/external_otlp.rs`** — End-to-end: real double-opt-in config, real `external::init`, emit events through `log_event`, `external::flush`, assert against the in-process collector:
- Event names present (`grok_code.session_start`, `grok_code.user_prompt`, `grok_code.api_request`)
- `service.name = "grok-cli"` as a wire commitment
- Default Delta temporality
- `session.count == 1`
- **Raw byte canary scan**: `assert!(!haystack.contains("CANARY"))` on the concatenated raw protobuf bytes of both signals — canary markers must not appear anywhere in the wire payload (gates-off mode)
- Shutdown bounded at ≤ 2.5 s
- Post-shutdown silence: events emitted after `shutdown()` do not reach the collector

**`tests/external_otlp_gates_on.rs`** — Same structure, both gates ON (`OTEL_LOG_USER_PROMPTS=1`, `OTEL_LOG_TOOL_DETAILS=1`):
- Prompt body present and contains `PROMPT_MARK` — proves the gate actually exports content
- Secrets still scrubbed inside gated content (`!prompt_text.contains(SECRET_KEY)`)
- Tool verbatim name exported; file path exported but home dir collapsed
- Tool parameters exported; secrets stripped
- Cumulative temporality works
- `app.version` on metrics when `OTEL_METRICS_INCLUDE_VERSION=1`
- Identity attributes (`user.id`, `organization.id`, `team.id`, `deployment.id`) on every record and metric
- **Raw byte canary scan**: `assert!(!raw.contains(SECRET_KEY))` on all bytes — secrets must not survive even in gated content
- Remote fleet kill switch (`apply_remote_policy(force_disable: true)`) stops emission in-process; events after the kill do not reach the collector

**`tests/external_otlp_guard.rs`** — Credential-leak guard: when `internal_pipeline_consumed_otel_vars = true`, `external::init` must refuse to activate even with a valid double opt-in pointed at a live collector. Waits 600 ms after emit and asserts `logs_len() == 0` and `metrics_len() == 0`.

**`tests/external_otlp_grpc.rs`** — Same pattern as `external_otlp.rs` but using gRPC transport to verify the gRPC collector path.

**`tests/external_otlp_session_ctx.rs`** — Session context injection: `turn_number` and `prompt.id` appear on log records when events are emitted inside a `with_session_ctx` scope.

**`tests/manual_auth_emit.rs`** — Wire test for the product events (non-OTEL) path: `log_event(ManualAuth { … })` must POST to the configured `events_url` with the correct JSON shape, including `event_name = "grok-shell-manual_auth"` and `reason`/`trigger`/`token_kind`/`principal` in `event_metadata`. Uses a real axum HTTP server on a random port. Mixpanel is disabled in this test (`mixpanel_enabled: false`) to avoid needing a real Mixpanel token.

### Key Observations About Telemetry Test Coverage

1. **No live xAI or Mixpanel endpoints in tests.** Build-time baked tokens (`GROK_TELEMETRY_BUILD_*`) are absent in test builds, so `TelemetryConfig::default()` has no URLs/tokens, and calls to `track()` are no-ops unless the test explicitly constructs a `TelemetryConfig` with a controlled endpoint.

2. **All integration tests run with telemetry disabled** via `TestSandbox` baseline env (`GROK_TELEMETRY_ENABLED=false`, `OTEL_SDK_DISABLED=true`, etc.). Telemetry-specific tests construct their own clients explicitly.

3. **Wire-level tests use in-process stubs**, not mocked network responses. The OTLP collector and product-events axum server are real HTTP/gRPC servers on loopback, started from the test binary. This avoids mocking the OpenTelemetry SDK internals.

4. **Canary/leak tests operate on raw bytes**, not on decoded structs. After `flush()`, the full concatenated raw protobuf bytes are checked for marker strings. This catches leaks that struct-level assertions would miss (e.g. a value serialized in a nested field).

5. **Gate and allowlist pins are maintained as independent copies.** The `external_allowed_keys_are_pinned()` test duplicates the expected key list rather than iterating `ALL_KEYS`, so adding a key to the enum without updating the test fails. This forces a deliberate review of every schema change.

6. **The `sync_profile` no-runtime test** (`sync_profile_is_noop_in_session_metrics_mode`) is designed to panic if the gate accidentally allows `tokio::spawn`; it asserts the absence of a tokio runtime as a precondition.

---

## Coverage

**Requirements:** No enforced coverage threshold in the repository.

**View coverage:**
```bash
cargo llvm-cov --workspace --html   # Requires cargo-llvm-cov
```

---

## Test Types Summary

**Unit tests:** Inline `#[cfg(test)] mod tests`. Test individual functions, pure logic, serialization, error types. Dominant pattern (~1,110 files).

**Integration tests (in-process):** `tests/*.rs` within a crate. Test crate-level behavior with real implementations but controlled environments (in-process HTTP servers, temp dirs, mock LLM responses).

**End-to-end tests (subprocess):** `crates/codegen/xai-grok-shell/tests/` and similar. Spawn the real `grok` binary as a child process against a `MockInferenceServer`. Cover full session flows, permission gates, leader/session management.

**Benchmark tests:** `benches/` in `xai-grok-markdown`, `xai-fsnotify`, `xai-ratatui-inline`. Use `criterion` (0.6).

**PTY harness tests:** `crates/codegen/xai-grok-pager-pty-harness/tests/` — interactive terminal tests driving the pager via a pseudo-TTY.

---

*Testing analysis: 2026-07-23*
