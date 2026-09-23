# =============================================================================
# grok-build fork — build, test, sync, and deploy
#
# This repo is a privacy-hardened fork of github.com/xai-org/grok-build.
#   origin   = github.com/carmilea/grok-build (this fork)
#   upstream = github.com/xai-org/grok-build  (read-only)
#
# Private/local overrides (extra remotes, internal PR targets, secrets) belong
# in a gitignored local.mk — it is included below and never committed.
#
# The installed `grok` is ~/.local/bin/grok, a wrapper that injects secrets and
# execs ~/.local/bin/grok-bin, which symlinks to target/release/xai-grok-pager
# in this repo — so `make build` alone updates the installed CLI in place.
#
# Fork patches (must survive every upstream sync):
#   1. Telemetry hard-disable      agent/config.rs resolve_telemetry_mode()
#                                  + xai-grok-telemetry client.rs/sentry.rs
#   2. Phone-home beacon disable   util/config/resolve/features.rs
#                                  + remote/client.rs fetch_login_device_flow()
#   3. Reasoning provenance guard  xai-grok-sampling-types conversation/messages.rs
#                                  (drop unsigned/foreign reasoning from
#                                  Anthropic requests; else 400 on model switch)
#   4. Access gate fail-open       agent/mvp_agent/mod.rs enforce_grok_code_access()
#                                  + settings_allow_access() chokepoint
#                                  (no remote verdict can arrive; never read
#                                  stale cached allow_access)
#   5. External OTEL disable       agent/config.rs resolve_external_otel_config()
#                                  (OTLP stream is "independent of TelemetryMode";
#                                  patch 1 does NOT cover it)
#   6. Managed-config fetch off    managed_config.rs is_fetch_enabled()
#                                  (default-on server-push of org policy, incl.
#                                  [telemetry] otel_*; bypasses patch 2)
#   7. Error reporting off         agent/config.rs is_error_reporting_disabled_sync()
#                                  (Sentry; env/TOML chain bypasses patch 1)
#   8. Session sharing stripped    extensions/share.rs handle_share_session()
#                                  (full-conversation upload to a public link)
#   9. Web-search backends (feat)  xai-grok-tools web_search/{types,client}.rs
#                                  + acp_session_impl/spawn.rs — Moonshot
#                                  $web_search + Anthropic web_search_20250305
#                                  wire formats (upstream is Responses-only)
#  10. Auto-update hard-disabled   xai-grok-update auto_update.rs
#                                  run_update_if_available + check_update_background
#                                  (an upstream self-update would clobber this
#                                  fork build and every patch in place)
# Full inventory + re-application recipes: PATCH.md
#
# EGRESS AUDIT — run `make audit` after EVERY sync, before merging anything.
# `make doctor` only greps for comment strings; `make audit` verifies behavior,
# checks the upload gates, and diffs an egress census to catch new outbound
# calls nobody enumerated. Full inventory + runbook: SECURITY-EGRESS.md
# =============================================================================

SHELL := /bin/bash

# ---- Config -----------------------------------------------------------------
PAGER_CRATE   := xai-grok-pager-bin
PAGER_BIN     := target/release/xai-grok-pager
INSTALL_LINK  := $(HOME)/.local/bin/grok-bin
UPSTREAM      := upstream/main
EGRESS_BASELINE := scripts/egress-baseline.txt
EGRESS_BASELINE_NONHTTP := scripts/egress-baseline-nonhttp.txt
# Full xai-grok-shell suite needs a bigger test-thread stack on macOS debug:
# upstream's authenticated_401s_still_exhaust_after_three_retries overflows 2MiB.
export RUST_MIN_STACK := 16777216

# Private/local targets (internal remotes, internal PR flows, secret paths).
# gitignored; never committed.
-include local.mk

# Housekeeping knobs (see scripts/tidy-target.sh):
#   KEEP = incremental unit dirs to retain per crate per profile
#   DRY  = 1 to preview a prune without deleting
KEEP ?= 1
DRY  ?= 0

.DEFAULT_GOAL := help
.PHONY: help build check test test-full install version doctor audit audit-baseline sync pr merge-pr deploy disk tidy tidy-hard clean

help: ## Show this help
	@grep -E '^[a-zA-Z_-]+:.*?## ' $(MAKEFILE_LIST) | \
	  awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-12s\033[0m %s\n", $$1, $$2}'

# ---- Build & install ----------------------------------------------------------

# Fork version branding: the binary mirrors the upstream crate version with
# a "-s" suffix (1.0.38 -> 1.0.38-s; semver: the fork marker rides in the
# prerelease; xai-grok-version runs Version::parse on it). fork-* tags remain
# the release trigger and the iteration counter for multiple fork releases
# on one upstream version. option_env! is compile-time, so force a recompile
# of just the version crate when the stamp changes (target/.fork-version).
CRATE_VERSION := $(shell sed -n 's/^version = "\(.*\)"/\1/p' crates/codegen/xai-grok-shell/Cargo.toml | head -1)
FORK_TAG      := $(shell git describe --tags --match 'fork-*' --abbrev=0 2>/dev/null)
FORK_VERSION  := $(CRATE_VERSION)-s
export GROK_VERSION := $(FORK_VERSION)

build: ## Release-build the grok binary (updates installed CLI via symlink)
	@if [ "$$(cat target/.fork-version 2>/dev/null)" != "$(GROK_VERSION)" ]; then \
	  echo "version stamp: $(GROK_VERSION)"; \
	  mkdir -p target; touch crates/codegen/xai-grok-version/src/lib.rs; \
	  echo "$(GROK_VERSION)" > target/.fork-version; \
	fi
	cargo build --release -p $(PAGER_CRATE)

install: build ## Build + ensure ~/.local/bin/grok-bin points at this repo's binary
	@actual=$$(readlink $(INSTALL_LINK) 2>/dev/null || echo "(missing)"); \
	want="$(CURDIR)/$(PAGER_BIN)"; \
	if [ "$$actual" = "$$want" ]; then \
	  echo "grok-bin symlink OK -> $$actual"; \
	else \
	  echo "relinking grok-bin: $$actual -> $$want"; \
	  ln -sfn "$$want" $(INSTALL_LINK); \
	fi
	@$(HOME)/.local/bin/grok --version

deploy: install ## Alias for install (build + relink + version print)

version: ## Show crate version vs installed binary version
	@echo "crate:    $$(sed -n 's/^version = "\(.*\)"/\1/p' crates/codegen/xai-grok-shell/Cargo.toml | head -1)"
	@echo "fork tag: $(FORK_TAG) -> GROK_VERSION $(GROK_VERSION)"
	@echo "HEAD:     $$(git rev-parse --short HEAD)"
	@echo "installed: $$($(HOME)/.local/bin/grok --version 2>/dev/null || echo '(not installed)')"

# ---- Tests --------------------------------------------------------------------

check: ## cargo check --workspace
	cargo check --workspace

test: ## Fast gate: all crates the fork patches touch
	cargo test -p xai-grok-sampling-types --lib
	cargo test -p xai-grok-telemetry --lib
	cargo test -p xai-grok-tools --lib web_search
	cargo test -p xai-grok-update --lib
	cargo test -p xai-grok-login --lib
	cargo test -p xai-grok-shell --lib -- agent::config remote::client agent::mvp_agent \
	  agent::remote_config agent::app::tests telemetry features login_config \
	  extensions::feedback

test-full: ## Full xai-grok-shell lib suite (needs RUST_MIN_STACK, set above)
	cargo test -p xai-grok-shell --lib
	@echo
	@echo "NOTE: ~28 failures are pre-existing upstream env/platform issues on this"
	@echo "machine (verified against pristine upstream/main), not merge regressions:"
	@echo "  ~27 in auth::/jsonl/unified_list/workflow/claude_import, plus"
	@echo "  session::acp_session::auth_retry_budget_tests::authenticated_401s_still_exhaust_after_three_retries"
	@echo "  (fails identically on pristine 1.0.5). The prompt_queue_actor_tests"
	@echo "  drain_at_safe_point case can flake under full-suite parallelism;"
	@echo "  it passes in isolation."

# ---- Upstream sync ------------------------------------------------------------

sync: ## Fetch upstream xai-org and merge upstream/main (then re-check fork patches!)
	git fetch upstream
	git merge $(UPSTREAM) --no-edit || { \
	  echo; echo "Resolve conflicts, then RE-VERIFY every fork patch (make doctor; see PATCH.MD §1)."; \
	  echo "Re-apply pattern: search upstream for the guard sites, port the guards,"; \
	  echo "#[ignore] tests that assert remote-delivery behavior, then: make test"; \
	  exit 1; }
	@echo
	@echo "Merge done. Now: make audit  (MUST be clean) && make check && make test"
	@echo "See SECURITY-EGRESS.md for what to do if the audit is red."

# ---- PR & merge on GitHub (origin = this fork) -------------------------------
# Usage: make pr BRANCH=upstream-sync-1.0.38 TITLE="Sync 1.0.38"
#        make merge-pr PR=2
# Any internal/legacy PR flows (e.g. a company Forgejo) belong in local.mk.

BRANCH ?= $(shell git branch --show-current)
TITLE  ?= $(shell git log -1 --pretty=%s)
GITHUB_REPO := $(shell git remote get-url origin | sed -E 's|.*github.com[:/]||; s|\.git$$||')

pr: ## Push HEAD to origin:BRANCH and open a GitHub PR to main
	git push origin HEAD:refs/heads/$(BRANCH)
	@gh pr create --repo $(GITHUB_REPO) --base main --head $(BRANCH) \
	  --title "$(TITLE)" --fill || gh pr view --repo $(GITHUB_REPO) $(BRANCH)

merge-pr: ## Merge PR=N on GitHub and fast-forward local main
	@test -n "$(PR)" || { echo "usage: make merge-pr PR=<number>"; exit 1; }
	gh pr merge $(PR) --repo $(GITHUB_REPO) --merge
	git fetch origin main
	git merge --ff-only origin/main

# ---- Housekeeping -------------------------------------------------------------
# Cargo prunes old *sessions* inside an incremental unit dir but never removes
# the unit dir itself when the fingerprint changes, so every flag/feature/dep
# shift orphans one. Across 92 crates that reached 64 GB here, only ~12 GB of it
# serving the live build config. Rationale + mechanics: scripts/tidy-target.sh
#
# Ladder, cheapest first:  disk (look) -> tidy (free) -> tidy-hard -> clean

disk: ## Report target/ usage and what `make tidy` would reclaim (read-only)
	@./scripts/tidy-target.sh report

tidy: ## Prune stale incremental unit dirs — frees GBs, no rebuild cost (DRY=1 previews)
	@KEEP=$(KEEP) DRY=$(DRY) ./scripts/tidy-target.sh prune

tidy-hard: ## Drop ALL incremental caches — frees more; workspace recompiles once
	@./scripts/tidy-target.sh prune-all

clean: ## cargo clean — remove target/ entirely (full cold rebuild follows)
	cargo clean

# ---- Sanity -------------------------------------------------------------------

audit: ## Egress/upload security audit — RUN AFTER EVERY SYNC (see SECURITY-EGRESS.md)
	@./scripts/audit-egress.sh

audit-baseline: ## Accept current egress call sites as the reviewed baseline (both HTTP and non-HTTP censuses)
	@echo "Regenerating $(EGRESS_BASELINE) — review the diff before committing!"
	@grep -rn --include='*.rs' -E '\.(post|put|patch|post_json)\(|Method::(POST|PUT|PATCH)' crates/ \
	  | grep -v "/tests/\|_tests.rs\|mock_server\|test_support" \
	  | sed -E 's|^(crates/[^:]*):[0-9]+:.*|\1|' \
	  | LC_ALL=C sort | uniq -c | awk '{printf "%s %s\n", $$1, $$2}' > $(EGRESS_BASELINE)
	@echo "Regenerating $(EGRESS_BASELINE_NONHTTP) — review the diff before committing!"
	@grep -rn --include='*.rs' -E 'connect_async|wss://|ws://|tonic::transport|\.donate_|sentry::init' crates/ \
	  | grep -v "/tests/\|_tests.rs\|mock_server\|test_support\|xai-proto-build" \
	  | sed -E 's|^(crates/[^:]*):[0-9]+:.*|\1|' \
	  | LC_ALL=C sort | uniq -c | awk '{printf "%s %s\n", $$1, $$2}' > $(EGRESS_BASELINE_NONHTTP)
	@git diff --stat $(EGRESS_BASELINE) $(EGRESS_BASELINE_NONHTTP) || true

doctor: ## Verify remotes, PAT, install symlink, and fork patch guards are present
	@echo "== remotes =="; git remote -v | sort -u
	@echo "== gh (GitHub PR flow) =="; gh auth status >/dev/null 2>&1 && echo "gh authenticated" || echo "gh NOT authenticated (run: gh auth login)"
	@echo "== local.mk =="; test -r local.mk && echo "present (private overrides loaded)" || echo "absent (optional; private overrides go here)"
	@echo "== install link =="; ls -la $(INSTALL_LINK) 2>/dev/null || echo "missing (run: make install)"
	@echo "== fork patch guards =="
	@grep -q "TELEMETRY HARD-DISABLED" crates/codegen/xai-grok-shell/src/agent/config.rs && echo "1. telemetry disable      OK" || echo "1. telemetry disable      MISSING!"
	@grep -q "TELEMETRY HARD-DISABLED" crates/codegen/xai-grok-telemetry/src/client.rs && echo "1b. telemetry client init OK" || echo "1b. telemetry client init MISSING!"
	@grep -q "REMOTE FETCH HARD-DISABLED" crates/codegen/xai-grok-shell/src/util/config/resolve/features.rs && echo "2. remote-fetch disable   OK" || echo "2. remote-fetch disable   MISSING!"
	@grep -q "LOGIN-CONFIG BEACON NEUTERED" crates/codegen/xai-grok-login/src/flow.rs && echo "2b. login beacon disable  OK" || echo "2b. login beacon disable  MISSING!"
	@grep -q "Provenance guard" crates/codegen/xai-grok-sampling-types/src/conversation/messages.rs && echo "3. reasoning guard        OK" || echo "3. reasoning guard        MISSING!"
	@grep -q "REMOTE FETCH HARD-DISABLED" crates/codegen/xai-grok-shell/src/agent/mvp_agent/mod.rs && echo "4. access gate fail-open  OK" || echo "4. access gate fail-open  MISSING!"
	@grep -q "EXTERNAL OTEL HARD-DISABLED" crates/codegen/xai-grok-shell/src/agent/config.rs && echo "5. external OTEL disable  OK" || echo "5. external OTEL disable  MISSING!"
	@grep -q "MANAGED CONFIG FETCH HARD-DISABLED" crates/codegen/xai-grok-shell/src/managed_config/store.rs && echo "6. managed-config off     OK" || echo "6. managed-config off     MISSING!"
	@grep -q "ERROR REPORTING HARD-DISABLED" crates/codegen/xai-grok-shell/src/agent/config.rs && echo "7. error reporting off    OK" || echo "7. error reporting off    MISSING!"
	@grep -q "TELEMETRY HARD-DISABLED" crates/codegen/xai-grok-telemetry/src/sentry.rs && echo "7b. sentry client init    OK" || echo "7b. sentry client init    MISSING!"
	@grep -q "SESSION SHARING REMOVED" crates/codegen/xai-grok-shell/src/extensions/share.rs && echo "8. sharing stripped       OK" || echo "8. sharing stripped       MISSING!"
	@grep -q "FORK PATCH 9" crates/codegen/xai-grok-tools/src/implementations/web_search/types.rs && echo "9. web-search backends    OK" || echo "9. web-search backends    MISSING!"
	@grep -q "AUTO-UPDATE HARD-DISABLED" crates/codegen/xai-grok-update/src/auto_update.rs && echo "10. auto-update disabled  OK" || echo "10. auto-update disabled  MISSING!"
	@grep -q "FEEDBACK TRACE UPLOAD HARD-DISABLED" crates/codegen/xai-grok-shell/src/extensions/feedback_trace.rs && echo "11. feedback trace upload OK" || echo "11. feedback trace upload MISSING!"
	@grep -q "BYOK SIDE-CALL BEARER XAI-ONLY" crates/codegen/xai-grok-shell/src/agent/mvp_agent/agent_ops.rs && echo "12. side-call bearer xai-only OK" || echo "12. side-call bearer xai-only MISSING!"
