#!/usr/bin/env bash
# Byte-stable collation for every grep/sort/diff below: without this the
# census order varies between macOS (en_US.UTF-8) and CI Linux (C), and the
# census diff false-fails on pure reordering.
export LC_ALL=C
# =============================================================================
# audit-egress.sh — verify every known data-egress path stays disabled.
#
# Run after EVERY upstream sync (`make sync`) and before merging any PR:
#     make audit
#
# This is the tripwire for the weekly upstream merge. It fails loudly if:
#   - a fork patch guard is missing or reverted
#   - an upload gate stops being a gate
#   - the stubbed-out session-sync subsystem gains a real implementation
#   - a dormant upload API gains its first live caller
#   - any file gains a new outbound HTTP write call (POST/PUT/PATCH)
#   - any file gains a new WebSocket/gRPC/raw-socket/donate-pump/Sentry call site
#   - a local listener (egress proxy, diagnostics socket) stops binding loopback
#
# See SECURITY-EGRESS.md for what each check defends and why it matters.
#
# Exit 0 = all checks pass. Exit 1 = review required before merging.
# =============================================================================

set -uo pipefail
cd "$(dirname "$0")/.."

PASS=0
FAIL=0
BASELINE="scripts/egress-baseline.txt"
NONHTTP_BASELINE="scripts/egress-baseline-nonhttp.txt"

# -----------------------------------------------------------------------------
# Census definitions — THE single source of truth for both censuses.
#
# `make audit-baseline` regenerates the baselines by calling this script with
# --write-baseline, so the accept path and the compare path can never drift.
# They did drift once (the 1.0.45 sync widened the compare pattern here while
# the Makefile kept its own stale copy), which silently produced a baseline
# that could never match. Do not reintroduce a second copy of these patterns.
#
# Check 5 also matches `.post_json(`/`Method::POST` etc: upstream is migrating
# some POSTs off raw reqwest builder methods onto a `post_json()` helper and
# `self.request(reqwest::Method::POST, ...)`.
EGRESS_CENSUS_PATTERN='\.(post|put|patch|post_json)\(|Method::(POST|PUT|PATCH)'
EGRESS_CENSUS_EXCLUDE="/tests/\|_tests.rs\|mock_server\|test_support"

# Check 6 closes the blind spot check 5 cannot see: WebSocket sends,
# gRPC/tonic exporters, computer-hub-sdk donate pumps, Sentry envelope inits —
# plus, since the 1.0.45 sync, the raw socket transports neither shape covers.
# xai-grok-egress-proxy (SECURITY-EGRESS.md §2.12) speaks no client library at
# all: it resolves with `lookup_host`, dials with `TcpStream::connect` and
# relays with `copy_bidirectional`, so every byte it forwards was invisible to
# BOTH censuses. `UnixStream::connect` joins it for the local-socket shape.
#
# Listener binds are deliberately NOT in this regex: `TcpListener::bind` alone
# matches 39 files of loopback test fixtures and would drown the diff. Loopback
# binds are asserted behaviorally in check 7 instead.
NONHTTP_CENSUS_PATTERN='connect_async|wss://|ws://|tonic::transport|\.donate_|sentry::init|TcpStream::connect|UnixStream::connect|lookup_host|copy_bidirectional'
NONHTTP_CENSUS_EXCLUDE="/tests/\|_tests.rs\|mock_server\|test_support\|xai-proto-build"

# census <pattern> <exclude> — per-file counts of matching call sites, on stdout
census() {
  grep -rn --include='*.rs' -E "$1" crates/ \
    | grep -v "$2" \
    | sed -E 's|^(crates/[^:]*):[0-9]+:.*|\1|' \
    | sort | uniq -c | awk '{printf "%s %s\n", $1, $2}'
}

census_http()    { census "$EGRESS_CENSUS_PATTERN" "$EGRESS_CENSUS_EXCLUDE"; }
census_nonhttp() { census "$NONHTTP_CENSUS_PATTERN" "$NONHTTP_CENSUS_EXCLUDE"; }

# --write-baseline: accept the current tree as reviewed (see PATCH.md §4 —
# review every addition first; never regenerate just to make a red audit green)
if [ "${1:-}" = "--write-baseline" ]; then
  echo "Regenerating $BASELINE — review the diff before committing!"
  census_http > "$BASELINE"
  echo "Regenerating $NONHTTP_BASELINE — review the diff before committing!"
  census_nonhttp > "$NONHTTP_BASELINE"
  git diff --stat "$BASELINE" "$NONHTTP_BASELINE" || true
  exit 0
fi

ok()   { printf "  \033[32mPASS\033[0m  %s\n" "$1"; PASS=$((PASS + 1)); }
bad()  { printf "  \033[31mFAIL\033[0m  %s\n" "$1"; FAIL=$((FAIL + 1)); }
note() { printf "        \033[33m%s\033[0m\n" "$1"; }

# check <description> <file> <regex>   — passes when regex is present
check() {
  local desc="$1" file="$2" pat="$3"
  if [ ! -r "$file" ]; then
    bad "$desc"; note "file missing: $file (upstream moved or deleted it)"; return
  fi
  if grep -qE "$pat" "$file"; then ok "$desc"; else
    bad "$desc"; note "expected pattern not found in $file"; note "pattern: $pat"
  fi
}

echo "== 1. Fork patch guards (behavioral, not just comments) =="

# The single source of truth every telemetry channel gates on. Must return
# Disabled unconditionally — not merely carry the comment.
check "telemetry mode hard-disabled" \
  crates/codegen/xai-grok-shell/src/agent/config.rs \
  'Resolved::new\(TelemetryMode::Disabled, ConfigSource::Default\)'

# The pattern check above is satisfiable by upstream's own final fallthrough
# line, so a fully-reverted resolve_telemetry_mode() would still pass it.
# Assert the actual behavior: the function body (comments stripped, so the
# fork's own explanatory prose mentioning these words doesn't false-fail)
# must not reference the live resolution chain (requirement pin, env,
# `[features] telemetry`, server-pushed remote_settings) at all — it must be
# a single unconditional return.
CONFIG_RS=crates/codegen/xai-grok-shell/src/agent/config.rs
if [ -r "$CONFIG_RS" ]; then
  tel_body=$(sed -n '/pub(crate) fn resolve_telemetry_mode/,/^    }$/p' "$CONFIG_RS" \
    | grep -vE '^\s*//')
  if [ -n "$tel_body" ] && ! printf '%s\n' "$tel_body" | grep -qE 'remote_settings|requirements'; then
    ok "resolve_telemetry_mode() body has no live remote_settings/requirements code"
  else
    bad "resolve_telemetry_mode() references remote_settings or requirements — patch 1 may be reverted"
    note "the marker/return-line check above passes even on upstream's own fallthrough; this is the real assertion"
  fi
else
  bad "resolve_telemetry_mode() body check"; note "file missing: $CONFIG_RS"
fi

# Patch 1b: the telemetry crate's own entry points (any caller can invoke
# them directly, bypassing resolve_telemetry_mode() entirely) must each force
# TelemetryMode::Disabled — twice, once per entry point.
TELEMETRY_CLIENT_RS=crates/codegen/xai-grok-telemetry/src/client.rs
if [ -r "$TELEMETRY_CLIENT_RS" ]; then
  n=$(grep -c 'let mode = TelemetryMode::Disabled;' "$TELEMETRY_CLIENT_RS")
  if [ "$n" -ge 2 ]; then
    ok "telemetry client init()/init_if_needed() both force Disabled ($n sites)"
  else
    bad "telemetry client hard-disable missing an entry point ($n site(s), expected >= 2)"
    note "init() and init_if_needed() must each force TelemetryMode::Disabled"
  fi
else
  bad "telemetry client hard-disable"; note "file missing: $TELEMETRY_CLIENT_RS"
fi

check "remote-settings fetch disabled" \
  crates/codegen/xai-grok-shell/src/util/config/resolve/features.rs \
  'REMOTE FETCH HARD-DISABLED'

check "login-config beacon neutered" \
  crates/codegen/xai-grok-login/src/flow.rs \
  'LOGIN-CONFIG BEACON NEUTERED'

check "reasoning provenance guard" \
  crates/codegen/xai-grok-sampling-types/src/conversation/messages.rs \
  'Provenance guard'

check "access gate fails open" \
  crates/codegen/xai-grok-shell/src/agent/mvp_agent/mod.rs \
  'REMOTE FETCH HARD-DISABLED'

# The external OTEL stream (OTLP logs/metrics to a configurable collector)
# documents itself as "independent of TelemetryMode" — check 1 does NOT cover
# it. Its resolver must return None unconditionally.
check "external OTEL hard-disabled" \
  crates/codegen/xai-grok-shell/src/agent/config.rs \
  'EXTERNAL OTEL HARD-DISABLED'

# The managed-config sync (default on, overlay-free) can server-push the
# [telemetry] otel_* keys that enable the external stream — and bypasses the
# remote-fetch disable by design. Its auto-fetch gate must be forced false.
# 1.0.45 moved the function out of xai-grok-shell into xai-grok-cloud-config.
MANAGED_STORE_RS=crates/codegen/xai-grok-cloud-config/src/managed_config/store.rs
check "managed-config fetch hard-disabled" \
  "$MANAGED_STORE_RS" \
  'MANAGED CONFIG FETCH HARD-DISABLED'

# The marker alone would survive a merge that kept the comment and took
# upstream's body, so assert the behavior: is_fetch_enabled() must end in an
# unconditional `false`, with every live tier (Distribution capability, the
# GROK_MANAGED_CONFIG env tier, the config layers) evaluated into the
# discarded `_original` binding rather than returned.
if [ -r "$MANAGED_STORE_RS" ]; then
  fetch_body=$(sed -n '/pub fn is_fetch_enabled/,/^}$/p' "$MANAGED_STORE_RS" | grep -vE '^\s*//')
  if [ -n "$fetch_body" ] \
    && printf '%s\n' "$fetch_body" | grep -q '_original' \
    && printf '%s\n' "$fetch_body" | grep -qE '^\s*false$'; then
    ok "is_fetch_enabled() returns false unconditionally (tiers evaluated into _original)"
  else
    bad "is_fetch_enabled() is not an unconditional false — patch 6 may be reverted"
    note "the managed-config sync is the server-push channel for [telemetry] otel_*"
  fi
else
  bad "is_fetch_enabled() body check"; note "file missing: $MANAGED_STORE_RS"
fi

# settings_allow_access is the chokepoint BOTH writers of tier_allowed
# consult (enforce_grok_code_access and on_remote_settings_changed). It must
# fail open, or a stale cached deny verdict blocks the user forever.
check "access-gate chokepoint fails open" \
  crates/codegen/xai-grok-shell/src/agent/mvp_agent/mod.rs \
  'let _ = rs;'

# Sentry gates on is_error_reporting_disabled_sync, an env/TOML chain
# independent of resolve_telemetry_mode — patch 1 does not cover it. It must
# return true (disabled) unconditionally.
check "error reporting hard-disabled" \
  crates/codegen/xai-grok-shell/src/agent/config.rs \
  'ERROR REPORTING HARD-DISABLED'

# Patch 7b: is_error_reporting_disabled_sync() stops the shell from choosing
# to init Sentry, but xai-grok-telemetry's own sentry::init() is a separate
# crate entry point any host binary can call with its own Config. Its
# forced Config must still win regardless of what the host passes in.
check "sentry client init hard-disabled" \
  crates/codegen/xai-grok-telemetry/src/sentry.rs \
  'disabled: true'

# Session sharing uploads the full conversation to a public link. Upstream
# re-enabled it in 1.0.5 behind a remote flag; the fork strips the handler —
# it must refuse unconditionally.
check "session sharing stripped" \
  crates/codegen/xai-grok-shell/src/extensions/share.rs \
  'SESSION SHARING REMOVED'

# Auto-update downloads an upstream binary and installs it in place — it would
# replace this fork build (and every patch) without anyone noticing. Both the
# apply path and the background check must be hard-disabled.
check "auto-update hard-disabled" \
  crates/codegen/xai-grok-update/src/auto_update.rs \
  'AUTO-UPDATE HARD-DISABLED'

# Patch 11: `x.ai/feedback/upload-trace` tars the whole session directory and
# PUTs it to GCS on a one-shot grant token that bypasses both the
# trace-upload gate and the telemetry hard-disable by design. Its sole
# chokepoint must refuse before any parsing or archive I/O.
check "feedback trace upload hard-disabled" \
  crates/codegen/xai-grok-shell/src/extensions/feedback_trace.rs \
  'FEEDBACK TRACE UPLOAD HARD-DISABLED'

check "side-call bearer restricted to xAI hosts" \
  crates/codegen/xai-grok-shell/src/agent/mvp_agent/agent_ops.rs \
  'BYOK SIDE-CALL BEARER XAI-ONLY'

# Patch 12's behavioral half: byok_from_models() feeds the process-wide
# side-call bearer used by voice STT (wss://api.x.ai/v1/stt), image/video
# generation, /tokenize-text, and the ACP bearer export. If the xAI-host
# filter disappears, any [models] entry's key — e.g. a Moonshot key — goes
# to api.x.ai again on /voice, Ctrl+Space, /imagine, or an image_gen call.
AGENT_OPS=crates/codegen/xai-grok-shell/src/agent/mvp_agent/agent_ops.rs
if [ -r "$AGENT_OPS" ]; then
  byok_body=$(sed -n '/fn byok_from_models/,/^}$/p' "$AGENT_OPS" | grep -vE '^\s*//')
  if [ -n "$byok_body" ] && printf '%s\n' "$byok_body" | grep -q 'is_trusted_xai_https_url'; then
    ok "side-call bearer limited to xAI-hosted model entries"
  else
    bad "byok_from_models() lost the xAI-host filter — BYOK keys can leak to api.x.ai side services"
    note "expected: is_trusted_xai_https_url gate inside byok_from_models (patch 12)"
  fi
else
  bad "file missing: $AGENT_OPS (upstream moved or deleted it — re-home patch 12)"
fi

# context_snapshot.rs POSTs the verbatim system prompt, tool definitions,
# skills listing, MCP announcement, AGENTS.md sections, and workflow text to
# {xai_api_base_url}/tokenize-text. Not a separate fork patch (dead under
# patch 1, since both conjuncts derive from telemetry mode) — but a future
# edit dropping either conjunct would light up a bulk-context-upload path, so
# the audit asserts the double gate stays intact.
CONTEXT_SNAPSHOT_RS=crates/codegen/xai-grok-shell/src/session/acp_session_impl/context_snapshot.rs
if [ -r "$CONTEXT_SNAPSHOT_RS" ]; then
  if grep -qE '!self\.telemetry_enabled \|\| !xai_grok_telemetry::is_session_metrics_enabled\(\)' "$CONTEXT_SNAPSHOT_RS"; then
    ok "context-snapshot tokenize-text gated on telemetry_enabled + session_metrics"
  else
    bad "context-snapshot tokenize-text gate weakened or removed"
    note "losing either conjunct uploads the verbatim system prompt, tool definitions, skills, MCP announcement, AGENTS.md sections, and workflow text to /tokenize-text"
  fi
else
  bad "context-snapshot tokenize-text gate check"; note "file missing: $CONTEXT_SNAPSHOT_RS"
fi

echo
echo "== 2. Upload gates still gate =="

# Master gate for the per-turn artifact upload engine (src/upload/*). With no
# upload method resolved, no PromptTraceContext is built and nothing can fire.
check "per-turn upload master gate" \
  crates/codegen/xai-grok-shell/src/agent/mvp_agent/agent_ops.rs \
  'if !cfg\.is_trace_upload_enabled\(\)'

# ZDR / data-collection-disabled short-circuit ahead of the master gate.
check "data-collection kill switch precedes gate" \
  crates/codegen/xai-grok-shell/src/agent/mvp_agent/agent_ops.rs \
  'if self\.is_data_collection_disabled\(\)'

# Conversation writeback (save_session_data posts full message bodies).
# Must require ALL THREE conditions; losing any one widens exposure.
check "relay sync requires tui+config+xai-auth" \
  crates/codegen/xai-grok-shell/src/agent/mvp_agent/agent_ops.rs \
  'tui_mode && relay_config_enabled && has_xai_auth'

# trace_upload defaults off. NOTE: this is a SOFT gate by design — local
# opt-in via GROK_TELEMETRY_TRACE_UPLOAD / [telemetry] trace_upload still
# works. We assert the default is derived from the (disabled) telemetry mode.
check "trace_upload defaults to telemetry mode" \
  crates/codegen/xai-grok-shell/src/agent/config.rs \
  '\.default\(mode\.value\.is_enabled\(\)\)'

# Ordering: the data-collection kill switch MUST precede the config gate in
# trace_upload_config_with_reason — and any network await (upstream added a
# mid-gate refresh_remote_settings in 1.0.5) must sit BETWEEN them, never
# before the kill switch.
AGENT_OPS=crates/codegen/xai-grok-shell/src/agent/mvp_agent/agent_ops.rs
fn_body=$(sed -n '/async fn trace_upload_config_with_reason/,/^    }$/p' "$AGENT_OPS")
zdr_line=$(printf '%s\n' "$fn_body" | grep -n 'is_data_collection_disabled' | head -1 | cut -d: -f1)
gate_line=$(printf '%s\n' "$fn_body" | grep -n 'is_trace_upload_enabled' | head -1 | cut -d: -f1)
refresh_line=$(printf '%s\n' "$fn_body" | grep -n 'refresh_remote_settings' | head -1 | cut -d: -f1)
if [ -n "$zdr_line" ] && [ -n "$gate_line" ] && [ "$zdr_line" -lt "$gate_line" ] \
  && { [ -z "$refresh_line" ] || { [ "$refresh_line" -gt "$zdr_line" ] && [ "$refresh_line" -lt "$gate_line" ]; }; }; then
  ok "gate conjunct order: kill-switch < refresh < config-gate"
else
  bad "gate conjunct order violated in trace_upload_config_with_reason"
  note "kill-switch line: ${zdr_line:-missing}, refresh: ${refresh_line:-none}, config-gate: ${gate_line:-missing}"
fi

# The sync snapshot twin (gates subagent traces + heap-profile uploads) must
# carry the same two conjuncts.
twin=$(sed -n '/fn trace_upload_config_snapshot/,/^    }$/p' "$AGENT_OPS")
if printf '%s\n' "$twin" | grep -q 'is_data_collection_disabled' \
  && printf '%s\n' "$twin" | grep -q 'is_trace_upload_enabled'; then
  ok "snapshot twin carries both conjuncts"
else
  bad "trace_upload_config_snapshot lost a conjunct"
  note "the twin gates subagent trace contexts and heap-profile uploads"
fi

echo
echo "== 3. Session-sync subsystem still stubbed =="

STUB=crates/codegen/xai-grok-shell/src/session/restore_stub.rs
if [ -r "$STUB" ]; then
  n=$(grep -c 'bail!(UNAVAILABLE)' "$STUB")
  if [ "$n" -ge 4 ]; then ok "restore_stub still stubbed ($n bail sites)"; else
    bad "restore_stub gained an implementation ($n bail sites, expected >= 4)"
    note "a real remote-restore impl implies a real upload counterpart"
  fi
else
  bad "restore_stub.rs missing — session sync may have been implemented"
fi

echo
echo "== 4. Dormant upload APIs have no live callers =="

UPLOAD_METHODS='\.(batch_upload|batch_upload_json|upload_file|upload_stream|upload_multipart|upload_via_signed_url|upload_bytes_signed|get_signed_upload_url)\('
callers=$(grep -rn --include='*.rs' -E "$UPLOAD_METHODS" crates/ \
  | grep -v 'xai-file-utils\|_tests.rs\|/tests/\|mock' || true)
if [ -z "$callers" ]; then
  ok "StorageClient upload API: 0 live callers (dead code)"
else
  bad "StorageClient upload API gained a live caller"
  echo "$callers" | sed 's/^/        /'
fi

# gcs::upload_bytes() itself is not dormant — feedback_trace.rs's PUT is a
# live call (behind the patch-11 refusal above), and trace.rs/gcs.rs/
# trace_cmd.rs/review.rs are the reviewed upload paths. Its caller set must
# stay a closed, reviewed list; a new caller outside it is a new upload path
# nobody enumerated.
GCS_UPLOAD_BYTES_ALLOW='xai-file-utils/|xai-grok-shell/src/upload/|xai-grok-pager/src/trace_cmd.rs|xai-grok-shell/src/extensions/review.rs|xai-grok-shell/src/extensions/feedback_trace.rs'
gcs_callers=$(grep -rn --include='*.rs' -E 'upload_bytes\(' crates/ \
  | grep -v '_tests.rs\|/tests/\|mock' || true)
gcs_unexpected=$(printf '%s\n' "$gcs_callers" | grep -vE "$GCS_UPLOAD_BYTES_ALLOW" || true)
if [ -z "$gcs_unexpected" ]; then
  ok "gcs::upload_bytes() callers match the reviewed set (upload/, trace_cmd.rs, review.rs, feedback_trace.rs)"
else
  bad "gcs::upload_bytes() gained a caller outside the reviewed set"
  echo "$gcs_unexpected" | sed 's/^/        /'
fi

echo
echo "== 5. Outbound HTTP write census vs baseline =="

CURRENT=$(mktemp)
NONHTTP_CURRENT=$(mktemp)
trap 'rm -f "$CURRENT" "$NONHTTP_CURRENT"' EXIT
census_http > "$CURRENT"

if [ ! -r "$BASELINE" ]; then
  bad "baseline missing: $BASELINE"
  note "regenerate with: make audit-baseline"
elif diff -u "$BASELINE" "$CURRENT" > /dev/null; then
  ok "egress census unchanged ($(wc -l < "$CURRENT" | tr -d ' ') files)"
else
  bad "egress census CHANGED — new or moved outbound HTTP write calls"
  note "-- = baseline (reviewed)   ++ = current tree (NEEDS REVIEW)"
  diff -u "$BASELINE" "$CURRENT" | tail -n +3 | sed 's/^/        /'
  note "review each addition, then accept with: make audit-baseline"
fi

echo
echo "== 6. Non-HTTP egress census vs baseline =="

# Closes the blind spot check 5 cannot see (see PATCH.md §4 "Watch for census
# blind spots" and SECURITY-EGRESS.md §2.5 "Voice STT — WebSocket, census
# blind spot"): per-file counts, diffed against a reviewed baseline. The
# pattern, its exclusions and the rationale for both live with the census
# definitions at the top of this file — the same ones --write-baseline uses.
census_nonhttp > "$NONHTTP_CURRENT"

if [ ! -r "$NONHTTP_BASELINE" ]; then
  bad "baseline missing: $NONHTTP_BASELINE"
  note "regenerate with: make audit-baseline"
elif diff -u "$NONHTTP_BASELINE" "$NONHTTP_CURRENT" > /dev/null; then
  ok "non-HTTP egress census unchanged ($(wc -l < "$NONHTTP_CURRENT" | tr -d ' ') files)"
else
  bad "non-HTTP egress census CHANGED — new or moved WebSocket/gRPC/donate/sentry call sites"
  note "-- = baseline (reviewed)   ++ = current tree (NEEDS REVIEW)"
  diff -u "$NONHTTP_BASELINE" "$NONHTTP_CURRENT" | tail -n +3 | sed 's/^/        /'
  note "review each addition, then accept with: make audit-baseline"
fi

echo
echo "== 7. Local listeners stay loopback-only =="

# New upstream in 1.0.45. xai-grok-egress-proxy is a forward HTTP/CONNECT
# proxy for a sandboxed command's own traffic (SECURITY-EGRESS.md §2.12). It
# is accepted, not patched out — but only because it is unreachable from off
# the machine and cannot originate a request of its own. Both of those are
# one edit away from being false, and neither census would notice.

# 7a. The listener binds loopback on an ephemeral port, never a routable
# interface. A bind on 0.0.0.0 would publish an open proxy on the LAN.
EGRESS_PROXY_LIB=crates/codegen/xai-grok-egress-proxy/src/lib.rs
if [ -r "$EGRESS_PROXY_LIB" ]; then
  if grep -qE 'TcpListener::bind\(\(Ipv4Addr::LOCALHOST, 0\)\)' "$EGRESS_PROXY_LIB" \
    && ! grep -qE '0\.0\.0\.0|UNSPECIFIED|\[::\]' "$EGRESS_PROXY_LIB"; then
    ok "egress proxy binds loopback only"
  else
    bad "egress proxy listener is no longer loopback-only"
    note "EgressProxy::start_with must bind (Ipv4Addr::LOCALHOST, 0) and nothing routable"
  fi
else
  bad "egress proxy loopback bind check"; note "file missing: $EGRESS_PROXY_LIB"
fi

# 7b. The crate carries no HTTP client, so it can only ever relay what a local
# client handed it — it has no way to originate a call of its own. A reqwest /
# hyper / ureq dependency appearing here is a phone-home capability.
EGRESS_PROXY_TOML=crates/codegen/xai-grok-egress-proxy/Cargo.toml
if [ -r "$EGRESS_PROXY_TOML" ]; then
  if ! grep -qE '^(reqwest|hyper|ureq|isahc|tonic)\b' "$EGRESS_PROXY_TOML"; then
    ok "egress proxy has no HTTP client of its own (cannot phone home)"
  else
    bad "egress proxy gained an HTTP client dependency"
    note "the crate forwards client-supplied requests only; an HTTP client is a new originating path"
  fi
else
  bad "egress proxy client-dependency check"; note "file missing: $EGRESS_PROXY_TOML"
fi

# 7c. The accept loop drops non-loopback peers, and resolve_public() refuses
# the whole address set unless every resolved address is public — the SSRF /
# cloud-metadata guard standing between a sandboxed command and 169.254.169.254.
EGRESS_PROXY_SERVER=crates/codegen/xai-grok-egress-proxy/src/server.rs
if [ -r "$EGRESS_PROXY_SERVER" ] && [ -r "$EGRESS_PROXY_LIB" ]; then
  resolve_body=$(sed -n '/async fn resolve_public/,/^}$/p' "$EGRESS_PROXY_LIB" | grep -vE '^\s*//')
  if grep -qE 'peer\.ip\(\)\.is_loopback\(\)' "$EGRESS_PROXY_SERVER" \
    && printf '%s\n' "$resolve_body" | grep -q 'is_public_ip'; then
    ok "egress proxy refuses non-loopback peers and non-public destinations"
  else
    bad "egress proxy lost its peer-loopback or public-destination check"
    note "server::serve must drop non-loopback peers; resolve_public must gate on ip::is_public_ip"
  fi
else
  bad "egress proxy peer/destination check"; note "file missing: $EGRESS_PROXY_SERVER or $EGRESS_PROXY_LIB"
fi

# 7d. The pre-stop lifecycle broker (SECURITY-EGRESS.md §2.13) serves an
# UNAUTHENTICATED `PUT /lifecycle/handlers/{name}` that runs an absolute-path
# argv in-guest. It binds nothing itself — it is mounted as extra_routes on the
# diagnostics server — so the listener that carries it must stay a Unix socket
# or loopback TCP.
DIAG_SERVER_LIB=crates/codegen/xai-grok-diag-server/src/lib.rs
if [ -r "$DIAG_SERVER_LIB" ]; then
  if grep -qE 'TcpListener::bind\(\(Ipv4Addr::LOCALHOST, port\)\)' "$DIAG_SERVER_LIB" \
    && grep -q 'UnixListener::bind' "$DIAG_SERVER_LIB" \
    && ! grep -qE '0\.0\.0\.0|UNSPECIFIED|\[::\]' "$DIAG_SERVER_LIB"; then
    ok "diagnostics server (lifecycle pre-stop routes) binds UDS or loopback only"
  else
    bad "diagnostics server listener is no longer loopback-only"
    note "it carries PUT /lifecycle/handlers/{name}, which registers an exec handler with no auth"
  fi
else
  bad "diagnostics server loopback bind check"; note "file missing: $DIAG_SERVER_LIB"
fi

# 7e. The broker's construction set stays closed to the in-guest workspace
# server binary. A second construction site means the pre-stop exec surface
# was mounted somewhere new — possibly on a listener nobody checked.
LIFECYCLE_BROKER_ALLOW='xai-grok-lifecycle/|xai-grok-workspace/src/bin/workspace_server.rs'
broker_sites=$(grep -rn --include='*.rs' -E 'LifecycleBroker::new\(' crates/ \
  | grep -v '_tests.rs\|/tests/\|mock' || true)
broker_unexpected=$(printf '%s\n' "$broker_sites" | grep -vE "$LIFECYCLE_BROKER_ALLOW" || true)
if [ -z "$broker_unexpected" ]; then
  ok "LifecycleBroker construction stays confined to workspace_server.rs"
else
  bad "LifecycleBroker gained a construction site outside the reviewed set"
  echo "$broker_unexpected" | sed 's/^/        /'
fi

echo
if [ "$FAIL" -eq 0 ]; then
  printf "\033[32m== EGRESS AUDIT CLEAN — %d checks passed ==\033[0m\n" "$PASS"
  exit 0
fi
printf "\033[31m== EGRESS AUDIT FAILED — %d passed, %d FAILED ==\033[0m\n" "$PASS" "$FAIL"
echo "Do not merge until each failure is explained. See SECURITY-EGRESS.md."
exit 1
