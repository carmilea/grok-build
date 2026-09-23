<!-- FORK-README-START (keep this block contiguous; resolve upstream sync conflicts by keeping it at the top) -->

> ## 🔒 Fork: privacy-hardened grok-build
>
> This is a **fork of [xai-org/grok-build](https://github.com/xai-org/grok-build)**,
> currently synced to **1.0.38**. Everything below this section is upstream's
> README and still applies; this section is what makes the fork different.
>
> **Releases:** Linux and macOS binaries (`grok X.Y.Z-s`) are built by GitHub
> Actions on `fork-*` tags and published on this repo's Releases page.
> Windows is not shipped — upstream supports macOS and Linux build hosts
> only (see "Building from source" below).
>
> ### What the fork changes
>
> Full inventory with code sites: [PATCH.MD](PATCH.MD) (15 patches, 17 guard
> sites, re-application recipes). Threat model + per-path egress analysis:
> [SECURITY-EGRESS.md](SECURITY-EGRESS.md). Everything below is verified by
> `make audit` (29 behavioral checks) and `make doctor` on every commit.
>
> **Privacy guards (upstream features disabled):**
>
> - **Telemetry, product events, session metrics, Mixpanel, trace uploads** —
>   hard-disabled at the resolver and inside the telemetry client itself
>   (patches 1/1b). Your own external OTEL collector stays available.
> - **Remote settings & managed-config sync** — the server-push channels that
>   could re-enable any of the above are dead (2, 6); the external OTEL stream
>   is off (5); error reporting/Sentry is off (7/7b).
> - **Startup phone-home beacons** — no `GET /v1/settings` (auth + user-id +
>   email) and no `GET /v1/login-config` (machine-fingerprint `agent_id`) (2b).
> - **Access gate fails open** — no `allow_access` verdict can ever arrive, so
>   stale cached verdicts are never enforced (4).
> - **Session sharing stripped** — no uploading conversations to public links (8).
> - **Auto-update disabled** — upstream binaries can't replace the patched
>   build; updates arrive only via this repo's releases (10).
> - **Feedback trace upload refused** — upstream 1.0.38 can tar the whole
>   session directory (system prompt, history, every file the agent read) to
>   GCS while deliberately bypassing the `trace_upload` flag; the fork refuses
>   unconditionally (11).
> - **Third-party keys never reach xAI** — the "side-call bearer" for voice
>   STT / image generation / tokenization only accepts credentials from
>   xAI-hosted model entries, so a Moonshot or Anthropic key can never be sent
>   to `api.x.ai` (12).
> - **Reasoning provenance guard** — unsigned/foreign `thinking` blocks are
>   dropped from Anthropic-bound requests instead of replaying as invalid
>   signatures (3).
>
> **Fork features (added, not just disabled):**
>
> - **`web_search` for Moonshot and Anthropic** (patch 9) — upstream speaks
>   only xAI's Responses API; the fork adds Moonshot's `$web_search` builtin
>   and Anthropic's `web_search_20250305` server tool (x-api-key auth,
>   domain filters native), dispatched per `[models]` entry's `api_backend`.
> - **Compaction model pinning** (patch 13) — `[compaction] model` / `effort`
>   (or `GROK_COMPACTION_MODEL` / `GROK_COMPACTION_EFFORT`) summarize with a
>   chosen `[models]` entry and its own endpoint/key; unset keeps the session
>   model, and a failed pin retries once on the session model.
>
>   ```toml
>   [compaction]
>   model = "sonnet"   # a [models] entry id
>   effort = "xhigh"
>   ```
>
> - **`/api-model-update`** (patch 14) — refresh BYOK `[model.*]` entries
>   from each provider's live model list: new Kimi/Opus releases become
>   usable, retired ones trimmed. Dry-run by default; `apply` writes
>   (backing up `config.toml` first); a provider that errors is never
>   trimmed, and entries referenced by `[models]` roles or the session are
>   always kept. Works headless: `grok -p "/api-model-update"`.
>
> ### Install
>
> ```sh
> brew install carmilea/grok/grok                                        # macOS (arm64)
> curl -fsSL https://raw.githubusercontent.com/carmilea/grok-build/main/install.sh | sh
> ```
>
> Or grab a `.tar.gz` (Linux/macOS), `.deb`, or `.rpm` directly from this
> repo's [Releases](https://github.com/carmilea/grok-build/releases) page.
>
> ### Working on this fork
>
> ```sh
> make doctor     # verify remotes, gh auth, installs, and all patch guards
> make audit      # behavioral egress audit — MUST be clean before any merge
> make build      # release build of the repo checkout (dev)
> brew install carmilea/grok/grok   # daily driver (primary)
> make test       # fast gate: every crate the patches touch
> make sync       # fetch + merge upstream (then re-check the patches, PATCH.MD §1)
> make pr / merge-pr   # GitHub PR flow via gh (origin = this fork); private
>                        # overrides (internal remotes/flows) go in local.mk
> ```
>
> **Hard rules** (enforced in CI): `make audit` is never red at merge time, and
> everything pushed — commits, merges, tags, PRs — is authored only by the
> owner (`carmilea`); no bot, agent, upstream, or co-author identities, and
> history intentionally carries no upstream commits (orphan model, PATCH.MD
> §3). CI (`.github/workflows/test.yml`) runs the authorship check, the egress
> audit, and the fast test gate on every push and PR; `release.yml` builds the
> tagged releases and keeps the Homebrew formula current.
>
> Notes: the full shell test suite needs `RUST_MIN_STACK=16777216` (set by the
> Makefile); ~27 upstream tests fail identically on pristine `upstream/main` on
> macOS (env/platform, not the fork); tests asserting behavior the patches
> intentionally disable are `#[ignore]`d with an explanatory note.
> See [PATCH.MD](PATCH.MD) for the full sync runbook.

---

<!-- FORK-README-END -->

<div align="center">

<h1>
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://media.x.ai/v1/website/spacexai-symbol-white-transparent-0c31957f.png">
    <source media="(prefers-color-scheme: light)" srcset="https://media.x.ai/v1/website/spacexai-symbol-black-transparent-6435cf42.png">
    <img alt="SpaceXAI logo" src="https://media.x.ai/v1/website/spacexai-symbol-black-transparent-6435cf42.png" width="96">
  </picture>
  <br>
  Grok Build (<code>grok</code>)
</h1>

**Grok Build** is SpaceXAI's terminal-based AI coding agent. It runs as a
full-screen TUI that understands your codebase, edits files, executes shell
commands, searches the web, and manages long-running tasks — interactively,
headlessly for scripting/CI, or embedded in editors via the Agent Client
Protocol (ACP).

[Installing the released binary](#installing-the-released-binary) ·
[Building from source](#building-from-source) ·
[Documentation](#documentation) ·
[Repository layout](#repository-layout) ·
[Development](#development) ·
[Contributing](#contributing) ·
[License](#license)

![Grok Build TUI](https://media.x.ai/v1/website/universe-tui-screenshot-6f7a0837.png)

**Learn more about Grok Build at [x.ai/cli](https://x.ai/cli)**

This repository contains the Rust source for the `grok` CLI/TUI and its agent
runtime. It is synced periodically from the SpaceXAI monorepo.

A small `SOURCE_REV` file at the root records the full monorepo commit SHA
for the version of the code present in this tree.

</div>

---

## Installing the released binary

Prebuilt binaries are published for macOS, Linux, and Windows:

```sh
curl -fsSL https://x.ai/cli/install.sh | bash   # macOS / Linux / Git Bash
irm https://x.ai/cli/install.ps1 | iex          # Windows PowerShell
grok --version
```

See the [changelog](https://x.ai/build/changelog) for the latest fixes,
features, and improvements in each release.

## Building from source

Requirements:

- **Rust** — the toolchain is pinned by [`rust-toolchain.toml`](rust-toolchain.toml);
  `rustup` installs it automatically on first build.
- **[DotSlash](https://dotslash-cli.com)** — required so hermetic tools under
  [`bin/`](bin/) (notably [`bin/protoc`](bin/protoc)) can download and run.
  Install it and ensure `dotslash` is on your `PATH` **before** building:

  ```sh
  cargo install dotslash
  # or: prebuilt packages — https://dotslash-cli.com/docs/installation/
  /usr/bin/env dotslash --help   # sanity check
  ```

- **protoc** — proto codegen resolves [`bin/protoc`](bin/protoc) via DotSlash,
  or falls back to a `protoc` on `PATH` / `$PROTOC`.
- macOS and Linux are supported build hosts; Windows builds are best-effort
  and not currently tested from this tree.

```sh
cargo run -p xai-grok-pager-bin              # build + launch the TUI
cargo build -p xai-grok-pager-bin --release  # release binary: target/release/xai-grok-pager
cargo check -p xai-grok-pager-bin            # fast validation
```

The binary artifact is named `xai-grok-pager`; official installs ship it as
`grok`. On first launch it opens your browser to authenticate — see the
[authentication guide](crates/codegen/xai-grok-pager/docs/user-guide/02-authentication.md).

## Documentation

Full online documentation is available at
[docs.x.ai/build/overview](https://docs.x.ai/build/overview).

The user guide ships with the pager crate:
[`crates/codegen/xai-grok-pager/docs/user-guide/`](crates/codegen/xai-grok-pager/docs/user-guide/)
— getting started, keyboard shortcuts, slash commands, configuration, theming,
MCP servers, skills, plugins, hooks, headless mode, sandboxing, and more.

## Repository layout

| Path | Contents |
|------|----------|
| `crates/codegen/xai-grok-pager-bin` | Composition-root package; builds the `xai-grok-pager` binary |
| `crates/codegen/xai-grok-pager` | The TUI: scrollback, prompt, modals, rendering |
| `crates/codegen/xai-grok-shell` | Agent runtime + leader/stdio/headless entry points |
| `crates/codegen/xai-grok-tools` | Tool implementations (terminal, file edit, search, ...) |
| `crates/codegen/xai-grok-workspace` | Host filesystem, VCS, execution, checkpoints |
| `crates/codegen/...` | The rest of the CLI crate closure (config, MCP, markdown, sandbox, ...) |
| `crates/common/`, `crates/build/`, `prod/mc/` | Small shared leaf crates pulled in by the closure |
| `third_party/` | Vendored upstream source (Mermaid diagram stack) — see below |

> [!IMPORTANT]
> The root `Cargo.toml` (workspace members, dependency versions, lints,
> profiles) is **generated** — treat it as read-only. Prefer editing per-crate
> `Cargo.toml` files.

## Development

```sh
cargo check -p <crate>        # always target specific crates; full-workspace builds are slow
cargo test -p xai-grok-config # per-crate tests
cargo clippy -p <crate>       # lint config: clippy.toml at the repo root
cargo fmt --all               # rustfmt.toml at the repo root
```

## Contributing

> [!NOTE]
> External contributions are not accepted. See [`CONTRIBUTING.md`](CONTRIBUTING.md).

## License

First-party code in this repository is licensed under the **Apache License,
Version 2.0** — see [`LICENSE`](LICENSE).

Third-party and vendored code remains under its original licenses. See:

- [`THIRD-PARTY-NOTICES`](THIRD-PARTY-NOTICES) — crates.io / git dependencies,
  bundled UI themes, and **in-tree source ports** (including openai/codex and
  sst/opencode tool implementations)
- [`crates/codegen/xai-grok-tools/THIRD_PARTY_NOTICES.md`](crates/codegen/xai-grok-tools/THIRD_PARTY_NOTICES.md)
  — crate-local notice for the codex and opencode ports (license texts +
  Apache §4(b) change notice)
- [`third_party/NOTICE`](third_party/NOTICE) — vendored Mermaid-stack index
