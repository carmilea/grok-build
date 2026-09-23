# External Integrations

**Analysis Date:** 2026-07-23

## APIs & External Services

**xAI Inference API (direct):**
- `https://api.x.ai/v1` — direct model inference (chat completions, responses) when authenticated via API key (`XAI_API_KEY`)
  - Constant: `XAI_API_BASE_URL_DEFAULT` in `crates/codegen/xai-grok-shell/src/agent/config.rs:51`
  - Overridable via `[model] base_url` config key

**cli-chat-proxy (xAI-operated API gateway):**
- `https://cli-chat-proxy.grok.com/v1` — primary inference proxy for grok.com OAuth sessions; also hosts settings, managed MCP, file uploads, and trace upload proxy
  - Constant: `PROD_CLI_CHAT_PROXY_BASE_URL` in `crates/codegen/xai-grok-env/src/lib.rs:29`
  - Overridable via `GROK_PRODUCTION_CLI_CHAT_PROXY_BASE_URL` env var

**xAI OAuth2 / Identity:**
- `https://auth.x.ai` — OAuth2 issuer (PKCE + device flow). Constant `XAI_OAUTH2_ISSUER` in `crates/codegen/xai-grok-shell/src/auth/config.rs:132`
- `https://accounts.x.ai` — accounts app origin (allowed CORS origin for loopback OAuth callback). Constant `PROD_ACCOUNTS_APP_ORIGINS` in `crates/codegen/xai-grok-shell/src/auth/config.rs:137`

**GitHub Releases (updates):**
- `xai-org-shared/grok-build` GitHub repo — update checks via `gh release list` CLI tool. Implemented in `crates/codegen/xai-grok-update/src/version.rs:179`

**Plugin Marketplace:**
- `https://github.com/xai-org/plugin-marketplace.git` — default plugin registry cloned at install time. `crates/codegen/xai-grok-pager/src/plugin_cmd.rs:1127`

**Voice / STT:**
- `wss://api.x.ai/v1/stt` — streaming speech-to-text WebSocket (gated feature). `crates/codegen/xai-grok-voice/src/stt/streaming.rs:306`

**Managed MCP connectors:**
- `https://grok.com/connectors` — browser URL shown to users to manage connectors (UI only)
- `https://cli-chat-proxy.grok.com/v1/mcp/configs` — fetch managed MCP configs. `crates/codegen/xai-grok-shell-session-support/src/managed_mcp.rs:428`
- `https://cli-chat-proxy.grok.com/v1/mcp/tools` — fetch managed MCP gateway tool catalog. `crates/codegen/xai-grok-shell-session-support/src/managed_mcp.rs:536`

## Data Storage

**Databases:**
- SQLite (local) — session journal via `xai-sqlite-journal` crate (`crates/codegen/xai-sqlite-journal/`). No external DB.

**File Storage:**
- Google Cloud Storage (GCS) — session trace/conversation upload. Three upload modes:
  - Proxy: via `https://cli-chat-proxy.grok.com/v1` with grok.com auth token (default when logged in via grok.com)
  - Direct GCS: `gs://<bucket>` using a service account key (`GROK_TRACE_UPLOAD_CREDENTIALS` / `endpoints.trace_upload_credentials`)
  - Direct S3: configurable via `endpoints.trace_upload_bucket` + `endpoints.trace_upload_region`
  - Client: `gcloud-storage 1.3.0` in workspace `Cargo.toml:152`

**Caching:**
- Local disk cache only (`~/.grok/`). No external cache service.

## Authentication & Identity

**Auth Provider:**
- xAI OAuth2 at `https://auth.x.ai` — PKCE + device-flow + loopback-callback
- Optional customer OIDC (`[grok_com_config.oidc]` config or `GROK_OIDC_ISSUER` env)
- API key (`XAI_API_KEY` env or `[model] api_key` config)
- External auth provider command (`[grok_com_config.auth_provider_command]`)

**Scopes requested from `https://auth.x.ai`:**
`openid`, `profile`, `email`, `offline_access`, `grok-cli:access`, `api:access`, `conversations:read`, `conversations:write`, `workspaces:read`, `workspaces:write`

## Monitoring & Observability

**Error Tracking:**
- Sentry (`sentry` crate 0.42.0) — crash and panic reporting
  - DSN resolved at runtime from `SENTRY_DSN` env var, then from `option_env!("SENTRY_DSN")` compile-time bake
  - Init: `crates/codegen/xai-grok-telemetry/src/sentry.rs:32`
  - `send_default_pii = false`; home directory and username scrubbed from all events before send
  - Sample rate: 1% of traces (`TRACES_SAMPLE_RATE = 0.01` at `sentry.rs:10`)
  - Destination DSN URL: determined entirely by the `SENTRY_DSN` value baked/injected at build/run time — not visible as a hardcoded domain in source

**Logs:**
- Structured via `tracing` / `tracing-subscriber`; written to local files and forwarded to OTLP

## CI/CD & Deployment

**Hosting:**
- macOS, Linux, Windows native binary
- Released via GitHub Releases at `xai-org-shared/grok-build`

**CI Pipeline:**
- Not visible in this source tree (likely internal CI)

## Data Egress / Reporting Endpoints

This section enumerates every external endpoint the running application can contact, specifically those that transmit usage data, prompts, telemetry, or diagnostics.

---

### 1. Mixpanel — Product Analytics Events

**Endpoints:**
- `POST https://api.mixpanel.com/track` — sends named events (session start, turn completed, tool call, login, etc.)
- `POST https://api.mixpanel.com/engage` — creates/updates user profiles on Mixpanel

**Implementation:** `crates/codegen/xai-mixpanel/src/lib.rs:77` (track) and `lib.rs:108` (engage)

**Payload:** JSON-encoded, base64-wrapped form post. Includes `distinct_id` (user_id or stable `agent_id`), `agent_id`, `shell_version`, `client_type`, `client_version`, `subscription_tier`, `team_id`, `deployment_id`, OS locale, country, timestamp, and the event-specific fields defined in `crates/codegen/xai-grok-telemetry/src/events.rs`.

**Event taxonomy (selected events from `events.rs`):** `login`, `session_new`, `session_ended`, `turn_completed`, `tool_call_completed`, `model_response_received`, `permission_prompted`, `compaction_triggered`, `plugin_used`, `mcp_server_connected`, `rate_limit_hit`, `api_error`, `prompt_submitted` (length only, no text), `display_refresh_probe`, `clipboard_copy`, and ~50 more.

**Auth:** Mixpanel project token embedded at build time via `GROK_TELEMETRY_BUILD_MIXPANEL_TOKEN` or injected via `[telemetry] mixpanel_token` config. Token injected after secret-scrubbing of user-supplied properties.

**Conditions:** Only fires when `TelemetryMode::Enabled`. Disabled by default for enterprise (`TelemetryMode::Disabled`). `TelemetryMode::SessionMetrics` fires session lifecycle events but skips Mixpanel profile engage and product analytics events.

**Opt-out:** `GROK_TELEMETRY_ENABLED=0` env var, or `/privacy opt-out` slash command, or `[features] telemetry = false` in config.toml. All resolve to `TelemetryMode::Disabled`.

---

### 2. xAI Product Events API — First-Party Event Ingestion

**Endpoint:** Configurable URL stored in `events_url` field of `TelemetryConfig`. Baked in at build time via `GROK_TELEMETRY_BUILD_EVENTS_URL` env var; overridable at runtime via `GROK_TELEMETRY_EVENTS_URL` env var or `[telemetry] events_url` config key.

**Implementation:** `crates/codegen/xai-grok-telemetry/src/client.rs:203-233`

**Payload (JSON):**
```json
{
  "viewer_context": {
    "request_id": "...",
    "user_attributes": { "user_id": "...", "user_type": "LoggedIn", "country": "...", "language": "..." },
    "device_attributes": { "app_name": "Grok Code" }
  },
  "api_key": "<events_api_key>",
  "events": [{ "event_name": "...", "event_value": "...", "event_metadata": {...}, "timestamp": "..." }]
}
```

**Auth:** `x-api-key` header with `events_api_key`. Baked at build time via `GROK_TELEMETRY_BUILD_EVENTS_API_KEY`.

**Conditions:** Same as Mixpanel — only `TelemetryMode::Enabled` (or `SessionMetrics` for lifecycle events). Request timeout: 10 seconds.

---

### 3. Sentry — Crash and Error Reporting

**Endpoint:** DSN-derived URL (Sentry ingestion URL embedded in DSN). DSN sourced from:
1. `SENTRY_DSN` environment variable (runtime)
2. `option_env!("SENTRY_DSN")` compile-time bake (`crates/codegen/xai-grok-telemetry/src/sentry.rs:41`)

**Implementation:** `crates/codegen/xai-grok-telemetry/src/sentry.rs`

**Payload:** Sentry event envelope — panics, anyhow errors, tracing-level events. Scrubbed before send: home directory replaced with `~`, username segments replaced with `<user>`, API keys/tokens redacted by `xai-grok-secrets::redact_secrets`. `cwd` extra field removed. `server_name` set to empty string.

**Filtering:** Broken-pipe panics and disk-full panics dropped (never sent). 1% trace sample rate.

**Conditions:** Fires unless `config.disabled = true` in the per-binary `sentry::Config`. `SENTRY_DSN` being empty or absent = Sentry disabled (sentry crate no-ops with empty DSN).

---

### 4. Internal OTLP Traces — cli-chat-proxy Tracing Pipeline

**Endpoint:** `https://cli-chat-proxy.grok.com/v1/traces` (referenced in `crates/codegen/xai-grok-telemetry/src/otel_layer/mod.rs:58`)

**Implementation:** `crates/codegen/xai-grok-telemetry/src/otel_layer/mod.rs` and `crates/codegen/xai-grok-telemetry/src/otlp_http.rs`

**Payload:** OTLP protobuf spans over HTTP. Span attributes are scrubbed by `crates/codegen/xai-grok-telemetry/src/otel_layer/redact.rs` — URL query parameters (tokens, codes), GCS signed-URL params, and bearer values are redacted before export.

**Conditions:** Active when `telemetry.trace_upload` is enabled and grok.com auth or a deployment key is present. Controlled by `[telemetry] trace_upload` config key and `GROK_TELEMETRY_TRACE_UPLOAD` env var.

---

### 5. External OTLP Stream — Customer-Controlled Collector

**Endpoints:** Fully user/admin-controlled. Default (if unconfigured but activated): `http://localhost:4318/v1/logs` and `http://localhost:4318/v1/metrics`.

**Implementation:** `crates/codegen/xai-grok-telemetry/src/external/` (`config.rs`, `emit.rs`, `providers.rs`)

**Activation:** Requires double opt-in:
1. `GROK_EXTERNAL_OTEL=1` env var or `[telemetry] otel_enabled = true` config
2. At least one of `OTEL_METRICS_EXPORTER=otlp` or `OTEL_LOGS_EXPORTER=otlp`

**Content gates (default off):**
- `OTEL_LOG_USER_PROMPTS=1` — enables prompt text in log records (capped at 60 KB, secret-scrubbed)
- `OTEL_LOG_TOOL_DETAILS=1` — enables tool parameters and file paths in log records

**Auth:** Via `OTEL_EXPORTER_OTLP_HEADERS` env var only — never stored in config files.

---

### 6. GCS / S3 Trace and Session Upload

**Proxy upload endpoint:** `https://cli-chat-proxy.grok.com/v1` — multipart file upload of OTLP spans and conversation traces via the proxy (uses grok.com auth token).
- File: `crates/codegen/xai-file-utils/src/storage_client.rs:423`

**Direct GCS:** `storage.googleapis.com` (via `gcloud-storage` crate) — authenticated by service account key.

**Direct S3:** Customer-configured bucket/region/endpoint.

**Conditions:** `[telemetry] trace_upload` enabled (default on for grok.com-authenticated sessions that are not ZDR teams). ZDR teams bypass all uploads.

---

### 7. Remote Settings / Feature Flags

**Endpoint:** `GET https://cli-chat-proxy.grok.com/v1/settings`
- Implementation: `crates/codegen/xai-grok-shell/src/remote/client.rs:566`
- Called at startup (`start_early_prefetch`) and on session lifecycle events

**Payload sent:** Bearer auth token in header. No user content in request.

**Data received:** `RemoteSettings` struct — boolean feature flags controlling behaviour (auto-compact thresholds, model availability, access gating, plan mode defaults, etc.). Also pushes `x.ai/settings/update` ACP notification to the pager.

**Opt-out:** `[features] remote_fetch = false` in config disables this fetch entirely (`crates/codegen/xai-grok-shell/src/util/config/resolve/features.rs:40`).

---

### 8. Announcement Push (Server-Initiated)

**Mechanism:** `x.ai/announcements/update` ACP notification pushed from shell to pager over the local IPC channel. The server side at `cli-chat-proxy.grok.com` pushes announcements to the shell, which forwards them to the pager. No direct pager → announcements.grok.com call.

---

### Summary Table

| Destination | Domain | Protocol | Contains User/Usage Data | Opt-Out |
|---|---|---|---|---|
| Mixpanel analytics | `api.mixpanel.com` | HTTPS POST | Yes — event names, metadata, locale, subscription tier, agent/user IDs | `GROK_TELEMETRY_ENABLED=0` or `/privacy opt-out` |
| xAI Product Events | `events_url` (baked) | HTTPS POST | Yes — same as Mixpanel | Same as above |
| Sentry crash reports | DSN URL (baked/env) | HTTPS | Yes — panic messages, stack traces (scrubbed of PII) | Empty `SENTRY_DSN` or `config.disabled=true` |
| Internal OTLP traces | `cli-chat-proxy.grok.com/v1/traces` | HTTP/OTLP | Yes — span names, durations, redacted attributes | `GROK_TELEMETRY_TRACE_UPLOAD=0` |
| External OTLP | User-configured | HTTP or gRPC | Optional — prompt text behind `OTEL_LOG_USER_PROMPTS=1` gate | Opt-in only (`GROK_EXTERNAL_OTEL=1` required) |
| GCS trace upload | `storage.googleapis.com` or proxy | HTTPS multipart | Yes — OTLP trace data, conversation context | `GROK_TELEMETRY_TRACE_UPLOAD=0` or ZDR team |
| Remote settings fetch | `cli-chat-proxy.grok.com/v1/settings` | HTTPS GET | No — only auth token in request | `[features] remote_fetch = false` |
| Inference / chat | `api.x.ai/v1` or `cli-chat-proxy.grok.com/v1` | HTTPS | Yes — prompts, model responses | N/A — core product function |
| OAuth2 | `auth.x.ai` | HTTPS | Auth codes only | N/A — required for grok.com login |
| STT streaming | `api.x.ai/v1/stt` | WSS | Yes — audio stream | Feature must be enabled in config |
| Managed MCP | `cli-chat-proxy.grok.com/v1/mcp/*` | HTTPS | No — fetch config only | Don't configure managed MCP servers |
| Update check | GitHub Releases (via `gh` CLI) | HTTPS | No — version metadata only | Disable auto-update in config |

## Environment Configuration

**Required env vars (inferred from production binary):**
None are strictly required — the binary degrades gracefully without them.

**Key optional vars:**
- `XAI_API_KEY` — direct API key auth
- `GROK_TELEMETRY_ENABLED` — `0`/`false` disables all product telemetry
- `GROK_TELEMETRY_TRACE_UPLOAD` — `0`/`false` disables trace uploads to GCS
- `GROK_TELEMETRY_EVENTS_URL` — override product-events endpoint
- `GROK_TELEMETRY_EVENTS_API_KEY` — override product-events API key
- `GROK_TELEMETRY_MIXPANEL_TOKEN` — override Mixpanel project token
- `SENTRY_DSN` — Sentry DSN (empty string disables Sentry)
- `GROK_EXTERNAL_OTEL` — master switch for external OTEL stream
- `GROK_PRODUCTION_CLI_CHAT_PROXY_BASE_URL` — override proxy URL

**Secrets location:**
- `~/.grok/auth.json` — persisted auth tokens
- `SENTRY_DSN`, `GROK_TELEMETRY_BUILD_EVENTS_API_KEY`, `GROK_TELEMETRY_BUILD_MIXPANEL_TOKEN` — injected at build time into the binary; not present on user filesystem

## Webhooks & Callbacks

**Incoming:**
- OAuth2 loopback callback server (ephemeral localhost HTTP server during login flow) — not a persistent webhook

**Outgoing:**
- None (all outbound calls are client-initiated)

---

*Integration audit: 2026-07-23*
