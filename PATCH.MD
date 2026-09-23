# PATCH.md — Fork Patch Inventory & Upstream Sync Runbook

This is the recipe for syncing `xai-org/grok-build` upstream into this fork and
re-applying the fork's privacy patches. Follow it every sync. The threat model
and the egress-path inventory live in `SECURITY-EGRESS.md`; this file is the
*how*.

Golden rule: **never merge with a red `make audit`.** The audit is behavioral —
it checks that guards gate, not that comments exist.

Authorship rule: **the only author on this repo is the owner.** Everything
pushed — commits, merges, tags — and every PR opened lists only `carmilea`
(`Cindy Parkar <cynn.ayres@gmail.com>`, or GitHub's web-merge identity
`38557922+carmilea@users.noreply.github.com`). No bot, agent, upstream, or
co-author identities, ever. History carries no upstream commits at all
(orphan model, §3). Agents working in this repo commit under the repo's git
config identity and never add `Co-authored-by` trailers. The CI `authorship`
job (`.github/workflows/test.yml`) rejects pushes and PRs carrying any other
author; `make doctor` checks the local git identity. If a foreign identity
ever appears, rewrite before pushing — do not merge around it.

---

## 1. Patch inventory

Sixteen sites across fourteen patches (patches 1 and 7 each span two files —
the shell-side resolver and an in-crate client that has its own independent
entry points; patches 9 and 13 are features, not guards). Each carries a
`(... fork patch)` marker comment the audit/doctor greps for. If a marker
disappears in a merge, the patch is gone — re-apply.

| # | Name | Site (function) | File | Marker |
|---|------|-----------------|------|--------|
| 1 | Telemetry hard-disable | `resolve_telemetry_mode()` | `crates/codegen/xai-grok-shell/src/agent/config.rs` | `TELEMETRY HARD-DISABLED` |
| 1b | Telemetry client init hard-disabled | `init()` **and** `init_if_needed()` | `crates/codegen/xai-grok-telemetry/src/client.rs` | `TELEMETRY HARD-DISABLED` |
| 2 | Remote-fetch disable | `resolve_remote_fetch_enabled()` | `.../src/util/config/resolve/features.rs` | `REMOTE FETCH HARD-DISABLED` |
| 2b | Login beacon neutered | `fetch_login_device_flow()` | `crates/codegen/xai-grok-login/src/flow.rs` | `LOGIN-CONFIG BEACON NEUTERED` |
| 3 | Reasoning provenance guard | `build_messages_request()` | `crates/codegen/xai-grok-sampling-types/src/conversation/messages.rs` | `Provenance guard` |
| 4 | Access gate fail-open | `enforce_grok_code_access()` **and** `settings_allow_access()` | `.../src/agent/mvp_agent/mod.rs` | `REMOTE FETCH HARD-DISABLED` |
| 5 | External OTEL disabled | `resolve_external_otel_config()` | `.../src/agent/config.rs` | `EXTERNAL OTEL HARD-DISABLED` |
| 6 | Managed-config fetch disabled | `managed_config::is_fetch_enabled()` | `crates/codegen/xai-grok-shell/src/managed_config/store.rs` | `MANAGED CONFIG FETCH HARD-DISABLED` |
| 7 | Error reporting (Sentry) disabled | `is_error_reporting_disabled_sync()` | `.../src/agent/config.rs` | `ERROR REPORTING HARD-DISABLED` |
| 7b | Sentry client init hard-disabled | `sentry::init()` | `crates/codegen/xai-grok-telemetry/src/sentry.rs` | `TELEMETRY HARD-DISABLED` |
| 8 | Session sharing stripped | `handle_share_session()` | `.../src/extensions/share.rs` | `SESSION SHARING REMOVED` |
| 9 | Web-search backends (feature) | `WebSearchClient` dispatch + `spawn.rs` mapping | `xai-grok-tools/.../web_search/{types,client}.rs`, `.../acp_session_impl/spawn.rs` | `FORK PATCH 9` |
| 10 | Auto-update disabled | `run_update_if_available()` + `check_update_background()` | `crates/codegen/xai-grok-update/src/auto_update.rs` | `AUTO-UPDATE HARD-DISABLED` |
| 11 | Feedback trace upload disabled | `handle_upload_trace_with_session_dir()` | `crates/codegen/xai-grok-shell/src/extensions/feedback_trace.rs` | `FEEDBACK TRACE UPLOAD HARD-DISABLED` |
| 12 | Side-call bearer xAI-only | `byok_from_models()` | `crates/codegen/xai-grok-shell/src/agent/mvp_agent/agent_ops.rs` | `BYOK SIDE-CALL BEARER XAI-ONLY` |
| 13 | Compaction model+effort pinning (feature) | `SessionActor::compaction_sampling_config()` + `resolve_compaction_model()`/`resolve_compaction_effort()` | `.../src/util/config/resolve/compaction.rs`, `.../src/session/compaction.rs`, `.../src/agent/config.rs`, `.../src/session/helpers/session_compact.rs` | `FORK PATCH 13` |

Patch 1 is the keystone: Mixpanel, product events, the internal OTLP span
pipeline, session metrics, and the trace-upload default all gate on it.
Patches 5 and 6 exist because the **external OTEL stream is explicitly
independent of `TelemetryMode`** — patch 1 does not cover it, and the
managed-config sync (default on) can server-push the `[telemetry] otel_*`
keys that enable it.

---

## 2. Re-application recipes

Apply these after `git merge upstream/main`, while resolving conflicts. Keep the
fork's comment style: marker on the first line, then *why* it exists and what
reverting it re-arms.

### Patch 1 — telemetry hard-disable (config.rs)

Replace the entire body of `resolve_telemetry_mode()` with:

```rust
Resolved::new(TelemetryMode::Disabled, ConfigSource::Default)
```

This intentionally drops the whole upstream resolution order (requirement pin
→ `GROK_TELEMETRY_ENABLED` → `[features] telemetry` → remote settings →
default). The remote-settings tier is the reason this patch exists. Note:
`resolve_trace_upload()` and the jemalloc heap-profile resolvers derive from
this function, so they close transitively — no separate patch needed, but the
audit asserts the `.default(mode.value.is_enabled())` derivation survives.

1.0.38 added a `DISABLE_TELEMETRY` env tier to upstream's resolution order
(alongside `GROK_TELEMETRY_ENABLED`). The whole-body replacement above drops
it along with everything else in the chain — nothing to port, just note it so
the next sync doesn't mistake the dropped tier for a missed conflict.

### Patch 1b — telemetry client init hard-disabled (xai-grok-telemetry/client.rs)

`resolve_telemetry_mode()` (patch 1) stops the shell from ever *passing* a
live mode down — but `xai_grok_telemetry::client::init()` and
`init_if_needed()` are the crate's own public entry points, callable directly
by anything that links the crate. Force `let mode = TelemetryMode::Disabled;`
(discarding the passed-in `mode` with `let _ = mode;` to keep it referenced)
at the top of **both** functions, so the client is a no-op even if a future
caller resolves and passes a live mode. Belt-and-suspenders with patch 1, not
a substitute for it.

### Patch 2 — remote-fetch disable (features.rs)

Upstream's `resolve_remote_fetch_enabled()` shape changes over time (currently
a layer-walking gate: requirements > managed > user, default true). Re-apply by
**replacing the function body**, not by hunk context: evaluate the original
layered decision into `_original` (keeps helpers live, silences dead-code
warnings), then `false`.

Knock-on effect to verify, not fix: with the fetch dead,
`resolve_zdr_access_enabled(remote=None)` resolves `false` — ZDR-team access
is blocked. That is this fork's intended posture; re-confirm each sync.

### Patch 2b — login beacon neutered (xai-grok-login/src/flow.rs)

`fetch_login_device_flow()` is an unauthenticated pre-login GET that transmits
the `agent_id` machine fingerprint. Keep the fork's shape: original body
preserved behind `let beacon_disabled = true; if beacon_disabled { return None; }`.

1.0.38 extracted the whole login flow (`remote/client.rs` in the shell crate)
into its own `xai-grok-login` crate — the function is now **private**
(`async fn fetch_login_device_flow`, no `pub`) at
`crates/codegen/xai-grok-login/src/flow.rs`, and its callers went through
`xai_grok_http::` paths (`xai_grok_http::shared_client()`,
`xai_grok_http::process_client_identifier()`, etc.) instead of the shell's own
http helpers. Re-apply the same guard shape at the new location; the
fingerprint/pattern check the audit runs is location-independent (it greps
the file the patch table names), so a future extraction just needs the table
row updated.

Upstream's tests asserting the real HTTP behavior moved inline too — they now
live in the same file, at `mod tests::login_config`
(`fetch_login_device_flow_parses_2xx_bodies`,
`fetch_login_device_flow_errors_return_none`,
`fetch_login_device_flow_sends_only_unauthenticated_headers`). Mark them
`#[ignore = "fork patch 2b: login beacon neutered"]` and keep the marker
comment next to the guard.

### Patch 3 — reasoning provenance guard (messages.rs)

Drops unsigned/foreign `thinking` blocks from Anthropic-bound requests (else
400 on model switch). Upstream rarely touches this file; the merge usually
keeps the fork's version. Verify the marker exists in the
`ConversationItem::Reasoning` arm of `build_messages_request()`.

### Patch 4 — access gate fail-open (mvp_agent/mod.rs)

Two edits, because upstream has **two writers** of `tier_allowed`:

1. In `enforce_grok_code_access()`, right after the non-xAI early pass, keep
   the fork's fail-open block: `if !resolve_remote_fetch_enabled() { tier_allowed.set(true); return; }`.
2. In `settings_allow_access()` (the chokepoint both `enforce_grok_code_access`
   and `on_remote_settings_changed` call), force `true`. Without this, a stale
   cached `allow_access = false` in `cfg.remote_settings` blocks the user even
   though no verdict can ever arrive.

### Patch 5 — external OTEL disabled (config.rs)

Replace the body of `resolve_external_otel_config()` with `None`. The external
stream (OTLP logs/metrics to a configurable collector, optional prompt-text and
tool-detail content) documents itself as "independent of `TelemetryMode`" and
activates via requirement pin > `GROK_EXTERNAL_OTEL` > `[telemetry] otel_enabled`.
Do **not** patch `external_otel_master_switch_resolved()` — the internal
pipeline keys off it, and internal exports are already dead under patch 1.

### Patch 6 — managed-config fetch disabled (managed_config/store.rs)

Replace the body of `managed_config::is_fetch_enabled()` with `false`
(evaluate-and-discard like patch 2 if warnings demand). This is the
server-push channel for org policy — including `[telemetry] otel_*` — and
bypasses patch 2 by design (own gate, overlay-free). The fork's stance: no
remote service may re-enable anything.

1.0.38 split the single `managed_config.rs` file into a directory:
`managed_config/{mod,policy,response,store,supervisor,tests}.rs`.
`is_fetch_enabled()` now lives in `store.rs`, and the layer-walking helper it
evaluates-and-discards moved to `policy.rs` as
`super::policy::managed_config_enabled_from_layers(&layers)`. Re-apply the
same evaluate-and-discard shape against the new helper:

```rust
let _original = if let Some(v) = crate::agent::config::env_bool("GROK_MANAGED_CONFIG") {
    Some(v)
} else {
    crate::config::ConfigLayers::load()
        .ok()
        .and_then(|layers| super::policy::managed_config_enabled_from_layers(&layers))
};
false
```

A file→directory split like this is a **modify/delete conflict** in git, not
a content conflict — see the §6 field note below on resolving it without
`git rm`.

### Patch 7 — error reporting disabled (config.rs)

`is_error_reporting_disabled_sync()` gates Sentry and is deliberately
independent of `resolve_telemetry_mode()` upstream (env/TOML/inherit chain) —
patch 1 never reached it. Keep the fork's `error_reporting_disabled` early
return so the original `SyncBoolFlag` chain stays live below it.

### Patch 7b — Sentry client init hard-disabled (xai-grok-telemetry/sentry.rs)

Same relationship to patch 7 as 1b has to 1: `is_error_reporting_disabled_sync()`
stops the shell from choosing to init Sentry, but `sentry::init()` in
`xai-grok-telemetry` is a separate crate entry point any host binary can call
with its own `Config`. Force `Config { disabled: true, ..config }` at the top
of `init()` before the host's `disabled` flag is consulted, so a host that
forgets to set `disabled: true` (or a build-baked `SENTRY_DSN`) still gets the
no-op guard back.

### Patch 8 — session sharing stripped (extensions/share.rs)

`handle_share_session()` uploads the entire conversation to a signed GCS URL
and publishes a public link; upstream re-enabled it behind a remote flag in
1.0.5. Refuse unconditionally at the top of the handler (the sole chokepoint:
`handle()` dispatches only `x.ai/share_session` here). Patch the handler
rather than deleting the file — full deletion turns every weekly sync into a
conflict in `acp_agent.rs`/`remote/client.rs` for zero extra safety, since the
audit fails if any new caller of the share path appears.

### Patch 9 — web-search backends (feature, not a guard)

Upstream's `WebSearchClient` speaks only the Responses API, which neither
Moonshot nor Anthropic serves — so `[models] web_search` was unusable without
xAI. The fork adds `WebSearchWireFormat` (`Responses` default, serde-backwards
compatible) and dispatches on the model entry's `api_backend`:
ChatCompletions → Moonshot `$web_search` builtin (two-hop: builtin tool_call,
then tool message + synthesis), Messages → Anthropic `web_search_20250305`
server tool (single hop; `x-api-key` auth via extra_headers, no Bearer;
domain filters map to `allowed_domains`/`blocked_domains` natively). The
spawn-site credential gate accepts `x-api-key` in extra_headers for Messages
entries. On syncs: if upstream refactors `web_search/`, keep the dispatch —
upstream changes to the Responses path apply cleanly alongside. Moonshot
cannot enforce `[toolset.web_search]` domain filters (warn-only, documented),
and kimi-k2.7-code rejects any temperature but 1 — Moonshot bodies omit
sampling params entirely (provider defaults apply). Both backends verified
end-to-end via `grok -p` on 2026-08-18 (kimi-code and claude-opus-48).

New upstream `WebSearchConfig::Enabled` construction sites need the fork's
`wire_format` field — `cargo check` (or the first `cargo test` build, for
sites that only appear in test code) flags them with a missing-field error.
1.0.38 added one in `xai-grok-agent/src/builder.rs`.

### Patch 10 — auto-update disabled (auto_update.rs)

Upstream's updater (default ON) downloads upstream release binaries and
installs them in place — on this fork that silently replaces the patched
build with a pristine upstream one. Early-return both entry points:
`run_update_if_available()` → `Ok(false)` (covers manual `grok update` and
exit-time apply) and `check_update_background()` →
`BackgroundUpdateCheck::none()` (also kills the release-channel phone-home).
`[cli] auto_update = false` in the user's config.toml is the config-level
belt; the code patch is the suspenders. Fork updates arrive only via this
runbook's weekly sync.

### Patch 11 — feedback trace upload disabled (extensions/feedback_trace.rs)

New in 1.0.38. `x.ai/feedback/upload-trace` tars the entire session directory
(`chat_history.jsonl`, `system_prompt.txt`, `prompt_context.json`,
`events.jsonl`, capped at 50 MiB) and PUTs it to GCS on a one-shot grant token
issued by the feedback POST — deliberately bypassing both the trace-upload
gate (`resolve_trace_upload`) and the telemetry hard-disable (patch 1), by
design, since the grant token is meant to authorize the upload on its own.

Refuse at the very top of `handle_upload_trace_with_session_dir()`, before
`parse_params` and before any archive I/O — it is the sole chokepoint
(`handle_upload_trace` and the `#[cfg(test)]` wrapper both delegate here):

```rust
let feedback_trace_upload_disabled = true;
if feedback_trace_upload_disabled {
    return Err(acp::Error::internal_error().data("trace upload is not available"));
}
```

The refusal message must match the file's own existing "trace upload is not
available" wording used elsewhere in the same handler (feedback disabled,
ZDR team, one-shot-blocked, missing session/grant) — a differently-worded
refusal is an easy tell for a caller probing which gate fired. Seven
success-path tests in `agent/mvp_agent/tests.rs` assert the real upload
completes; mark them `#[ignore = "fork patch 11: ..."]` with a marker comment
citing this patch.

### Patch 12 — side-call bearer xAI-only (mvp_agent/agent_ops.rs)

Added during the 1.0.38 sync after the credential-flow audit found the leak.
Upstream feeds **any** `[models]` entry's own `api_key`/`env_key` into the
process-wide "side-call bearer"
(`xai-grok-login/src/side_call_bearer.rs::resolve_static_api_key`: env →
process model key → disk). That bearer authenticates xAI side services:
voice STT (`wss://api.x.ai/v1/stt`), image/video generation
(`api.x.ai/v1/images|/videos`), `/tokenize-text`, and the ACP bearer export.
With BYOK-only credentials a third-party (e.g. Moonshot) key was sent to
`api.x.ai` on `/voice`, Ctrl+Space, `/imagine`, or a model-invoked
`image_gen` call.

The chokepoint is `byok_from_models()` (the sole source of
`set_process_static_api_key`, two call sites in `agent_ops.rs`). Gate every
tier on the entry's effective API-key endpoint being a trusted xAI host:

```rust
let url = m.api_base_url.as_deref().unwrap_or(&m.info.base_url);
if crate::util::is_trusted_xai_https_url(url) { m.own_credential() } else { None }
```

(`api_base_url` wins when set because the side-call bearer is API-key auth;
`info.base_url` is already default-resolved.) Chat requests to the entry's
own provider are unaffected. Entries with xAI endpoints keep working —
`XAI_API_KEY`, xAI login, or an xAI-hosted `[models]` entry still supply
voice/imagine. On syncs: the function is small; re-apply by re-wrapping each
`own_credential()` tier, and keep the audit's behavioral check (the body
must reference `is_trusted_xai_https_url`) as the arbiter.

### Patch 13 — compaction model+effort pinning (feature, not a guard)

Upstream compacts with whatever the session is sampling with. The fork adds
`[compaction] model` / `[compaction] effort` (env `GROK_COMPACTION_MODEL` /
`GROK_COMPACTION_EFFORT`, env > TOML) so the summarization call — the one call
whose output the rest of the session is built on — can be pinned to a chosen
`[models]` entry: chat with a cheap coding model, compact with Sonnet at
xhigh. Four pieces, all marked `FORK PATCH 13`:

1. `util/config/resolve/compaction.rs` — `resolve_compaction_model()` /
   `resolve_compaction_effort()` (`Resolved<T>`/`ConfigSource`; the TOML tier
   is read **overlay-free**, like `[auto_mode]`, so a `GROK_CONFIG` overlay
   cannot re-route the transcript). The keys are declared on
   `agent/config.rs::CompactionConfig`; `effort` stays a raw `String` there so
   a typo degrades instead of failing the whole config parse.
2. `agent/config.rs` — `compaction_sampling_config_for()` resolves the id
   through `resolve_credentials` + `sampling_config_for_model`, exactly like a
   model switch, so the entry's own key only ever reaches its own `base_url`
   (patch 12's rule). Deliberately **no** xAI-proxy fallback tier like
   `resolve_aux_model_sampling_config` has: an unresolvable pin returns `None`
   and compaction stays on the session model.
3. `session/compaction.rs` — `SessionActor::compaction_sampling_config()`, the
   single helper both call sites (`two_pass_sample`, `run_compact_inner`) use,
   plus the one-shot retry with the session config when the pinned endpoint
   fails. Unpinned it returns `reconstruct_full_config()` unchanged.
4. `session/helpers/session_compact.rs` — all three request shapes now carry
   `sampling_config.reasoning_effort`. Upstream builds them without it, so the
   pinned effort (and an entry's own catalog effort) would otherwise resolve
   correctly and never reach the wire.

On syncs: this is a feature, so upstream will never conflict "against" it —
the risk is silent loss. A merge that takes upstream's `two_pass_sample` /
`run_compact_inner` hunks re-instates `reconstruct_full_config()` at the call
sites and the pin stops working with no error anywhere; a merge that takes
upstream's `session_compact.rs` request builders drops the effort from the
wire while the config still resolves, which looks like "xhigh does nothing".
`make doctor`'s line 13 only proves the resolver marker survived — check all
four sites, and let
`cargo test -p xai-grok-shell --lib -- session::acp_session::compaction util::config::resolve::compaction`
(both in `make test`) be the arbiter.

---

## 2.5. Standing decisions (reviewed, intentionally NOT patched)

These come up in every sync's new-egress review. Recorded once here so each
weekly review is confirm-not-redecide — revisit only if usage changes.

- **ZDR access gate.** With remote fetch disabled (patch 2),
  `resolve_zdr_access_enabled()` resolves `false`: ZDR-team users are
  *refused*. Accepted — this fork is used with personal/BYOK credentials only.
  If a ZDR-team user ever needs it, patch the resolver to fail open like
  patch 4.
- **`grok trace <id>`.** User-explicit full-session upload for support
  debugging; deliberately NOT gated on `is_trace_upload_enabled`. Inert
  without xAI auth/deployment key (BYOK falls back to local export under
  `~/.grok/trace-exports/`). Kept as the intentional escape hatch for
  producing debug bundles. See SECURITY-EGRESS.md §2.2.

---

## 3. Sync runbook (weekly)

Cadence: **weekly**, against upstream's latest monorepo sync. Budget ~30–60
min: fetch, merge, re-apply, audit, test, PR, build. Skipping weeks compounds
conflict surface — the 0.2.117 → 1.0.5 jump was 16 syncs and 268k lines.

That budget is per-sync, not per-catch-up: a multi-sync gap compounds
linearly at best. The 1.0.5 → 1.0.38 jump was 14 syncs and 555k lines
(3445 files changed) in one merge — budget **~half a day**, not 30–60 min,
for a gap that size. Falling behind on the weekly cadence is what turns a
30-minute sync into a half-day one; it does not make individual syncs cheaper.

```sh
git checkout main && git checkout -b upstream-sync-<VERSION>
git fetch upstream
git merge upstream/main --no-edit --allow-unrelated-histories
```

> **History model (since 1.0.38):** `main` is a single orphan commit — the
> fork deliberately carries **no upstream history** (no upstream commit
> hashes, no upstream bot authorship). Every sync therefore merges with
> `--allow-unrelated-histories`, which has no merge-base: **every file the
> fork has ever touched conflicts add/add**, not just the guard sites
> upstream changed. The §1 table and the §2 recipes are the resolution
> checklist; the behavioral audit is the arbiter. After the sync is verified
> (step 5), **re-squash back to a single orphan commit** (step 8) before
> pushing — this drops the upstream commits the merge pulled in, along with
> their authorship.

1. **Resolve conflicts.** Guard sites conflict because the fork patched them;
   resolve by taking upstream's structure, then apply the §2 recipes. With no
   merge-base, conflicts are bulkier: sort by the §1 table first, then sweep
   the remaining conflicted files (usually fork-side docs/tooling).
2. **Check auto-merged guard files too.** Auto-merge can silently hybridize a
   patched function when upstream edits nearby lines. Eyeball every site in
   the §1 table, not just the conflicted files.
3. **Handle new/removed guard-site files.** If upstream moves a function, the
   audit's "file missing" failure tells you where it went — port the patch to
   the new home.
4. **Extend the audit for anything new.** New telemetry/upload/sync subsystem?
   Decide: keep, gate, or patch out (§4). Add an `audit-egress.sh` check and a
   `SECURITY-EGRESS.md` row in the same commit.
5. **Verify, in this order:**
   ```sh
   make audit          # MUST be clean before anything else
   make check          # cargo check --workspace
   make test           # fast gate: all crates the patches touch
   ```
6. **Ship via GitHub PR:** `make pr BRANCH=upstream-sync-<VERSION>` then
   `make merge-pr PR=<n>` after review. `origin` is the fork
   (github.com/carmilea/grok-build); `upstream` is xai-org — never push to
   upstream. (A legacy internal PR flow, if any, belongs in a gitignored
   `local.mk` — the Makefile includes it; keep internal hosts out of the
   public repo.)
7. **Re-squash to a single orphan commit.** The sync merge pulled upstream's
   commits (and their authorship) back into history — the fork's history
   model is one orphan commit, so after the merge lands on main:
   ```sh
   git checkout main && git pull --ff-only
   git checkout --orphan re-squash main
   git commit -m "Privacy-hardened fork of xai-org/grok-build @ <VERSION>"
   git branch -f main re-squash && git checkout main && git branch -D re-squash
   git push origin --force main
   ```
   The tree is unchanged (verify: `git diff <pre-squash-main> HEAD` is empty).
   This is also why the fork carries no upstream contributor authorship.
8. **Tag the fork release** to fire the GitHub Actions release build
   (`.github/workflows/release.yml`): bump the 4th component —
   `git tag -a fork-<UPSTREAM>.<N>` (e.g. `fork-1.0.38.1`) — and push it to
   origin (your GitHub fork). The binary always mirrors the crate version with
   a `-s` suffix (`1.0.38-s`); the `fork-*` tag is the release trigger and
   the iteration counter when more than one fork release rides on the same
   upstream version.
9. **Update the installed CLI.** The daily driver is the Homebrew tap
   (primary): once the release publishes, `brew update && brew upgrade
   carmilea/grok/grok`, then confirm `grok --version` prints the new
   fork-branded version. For a repo-checkout dev build instead, `make
   install` relinks `~/.local/bin/grok-bin` (secondary; the old passthrough
   wrapper is retired at `~/.local/bin/grok.fork-build-wrapper.bak`).

---

## 4. Triaging new upstream egress

Every new network-capable feature falls into one of three classes (see
SECURITY-EGRESS.md §Threat model):

1. **Reverted guard** — a patched function returned to upstream behavior.
   Re-apply per §2.
2. **Widened gate** — a conjunct dropped or reordered (e.g. an upload method
   resolving before the gate is consulted, a network await inserted between
   conjuncts). Restore the order; assert it in the audit.
3. **New path** — new call site, new subsystem, or a stub replaced by a real
   implementation. Decide explicitly: keep (document why it's safe as-gated),
   gate (behind an existing killed gate), or patch out (new recipe in §2, new
   audit check, new PATCH.md row).

The egress censuses (`make audit` checks 5 and 6) are the catch-all for class
3. Accept reviewed additions with `make audit-baseline` (regenerates both)
and commit the baseline diff with justification — never regenerate it just to
make a red audit green.

Check 5 counts `.post(`/`.put(`/`.patch(`/`.post_json(`/`Method::(POST|PUT|PATCH)`.
Check 6 (added in the 1.0.38 sync) covers what check 5 can't: `connect_async`,
`ws://`/`wss://`, `tonic::transport`, `.donate_`, `sentry::init`. If a future
subsystem doesn't match either — a raw TCP socket, a new SDK's own transport
abstraction — widen check 6's pattern in the same commit as the baseline
update that first needs it.

---

## 5. Files that must move together

When a patch is added, removed, or re-homed, update in the **same commit**:

- `PATCH.md` (this file) — inventory + recipe
- `SECURITY-EGRESS.md` — threat-model inventory + last-review date/version
- `scripts/audit-egress.sh` — behavioral check
- `Makefile` `doctor` target — marker grep (comment-level sanity only)
- `scripts/egress-baseline.txt` and `scripts/egress-baseline-nonhttp.txt` —
  via `make audit-baseline` (regenerates both), with justification

---

## 6. Field notes

Reusable lessons from the 0.2.117 → 1.0.5 sync, plus the 1.0.5 → 1.0.38
catch-up (14 syncs, 555k lines).

### Extracted test modules

When upstream moves inline `mod tests {…}` to `#[path = "*_tests.rs"] mod tests;`, the conflict hunk spans thousands of lines but is structurally trivial: take upstream's `#[path]` side, then grep the discarded HEAD side for fork markers (`fork patch`, `HARD-DISABLED`, `#[ignore`) and port each `#[ignore]` + marker comment to the same-named test in the new extracted file.

### Auto-merged guard files

Git auto-merge keeps a patched function when upstream edits nearby lines, but it can also silently hybridize. After every merge, eyeball every site in the §1 table even when there are no conflicts, and let `make audit` (behavioral) be the arbiter.

### Test-compile errors

Public monorepo syncs can ship tests that don't compile standalone (example seen: a new test using `base64::engine::general_purpose::STANDARD.encode()` without `use base64::Engine;`). `cargo check --workspace` does NOT build test code — the first `make test` is where these surface. Fix minimally (exactly the compiler's suggestion), note it in the merge commit, and report upstream.

### New tests asserting remote behavior

New upstream tests that require remote settings/fetch/telemetry/uploads the fork disables are not regressions. `#[ignore = "<reason> by local fork patch"]` them with a marker comment citing the patch, in the same commit as the merge. Six such tests appeared in 1.0.5.

### New subsystems, new gates

Every sync is also a NEW-egress review. New upstream subsystems may gate on something other than `resolve_telemetry_mode()` (the 1.0.5 external-OTEL stream documents itself as "independent of TelemetryMode"). Grep the diff for new enable paths (`GROK_*` env vars, config keys, requirement pins, managed-config layers) before assuming existing guards cover new code.

### Crate extraction moves a patch out from under you (1.0.38)

Upstream reorganizing a module into its own crate (patch 2b's
`fetch_login_device_flow()`: shell-crate `remote/client.rs` → the new
`xai-grok-login` crate's `flow.rs`) can move a guarded function **out of the
crate it used to live in entirely**, not just to a new path within the same
one. The visible signal is identical to any other re-home — the audit's
"file missing" / "expected pattern not found" — but it's worth naming
explicitly because the instinct on a missing-file failure is to grep the
*same crate* for where the function went, and it isn't there anymore. Grep
the whole workspace by function name, not by crate.

The fix that follows from this: keep the fingerprint/behavioral assertion
location-independent (parameterize the file path, as `audit-egress.sh`
already does per-check) rather than hardcoding crate boundaries into the
check's *logic* — only the path argument should need to change.

### File → directory split is a modify/delete conflict, and `git rm` is a trap (1.0.38)

Patch 6's managed-config fetch flag: upstream split the single
`managed_config.rs` into a directory (`mod.rs` + `policy.rs` + `response.rs`
+ `store.rs` + `supervisor.rs` + `tests.rs`). Git sees this as upstream
**deleting** `managed_config.rs` while the fork side **modified** it — a
modify/delete conflict, not a content conflict, so the usual "resolve the
hunk" instinct doesn't apply. `git rm managed_config.rs` resolves the
conflict instantly and silently — and just as silently discards the fork's
guard along with the old file, since the guard's edits die with it. Resolve
by taking upstream's new directory structure, then find the split-out
function's new home (here, `store.rs`) and re-apply the patch fresh — treat
it exactly like the "new/removed guard-site files" case in the runbook
above, not like a normal conflict resolution.

### Rename detection survives a merge with ignores intact

Not every restructure is this fraught: `models/tests.rs` moved to
`agent/remote_config/manager/tests.rs` in the same sync, and git's rename
detection followed it cleanly through the merge — the fork's `#[ignore]`
markers on the tests in that file needed no manual porting. File→crate and
file→directory splits (above) are the cases that need eyes on them; a plain
same-shape rename usually doesn't.

### Census regex must track upstream's own POST-shape migrations

Upstream is migrating some POST call sites off raw `reqwest::Client::post()`
onto shared helpers — `post_json()` wrapper methods and
`self.request(reqwest::Method::POST, ...)` — file by file, not all at once
(seen in this sync: `xai-grok-tools`'s `image_gen/mod.rs` and
`video_gen/mod.rs`, mirroring the `post_json()` shape `image_edit/mod.rs`
already used). A census regex that only matches `.post(`/`.put(`/`.patch(`
goes blind to these one call site at a time. Check 5's pattern already covers
`\.(post|put|patch|post_json)\(|Method::(POST|PUT|PATCH)` — when the next
sync introduces yet another shape (a generic `send_request(method, ...)`
wrapper, say), widen the regex in the same commit as the baseline update
that first needs it, not reactively after a silent gap.
