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
> ### Fork patches (verified by `make doctor`)
>
> 1. **Telemetry hard-disabled** — `resolve_telemetry_mode()` returns `Disabled`
>    unconditionally (Mixpanel, product-events, OTEL spans, session metrics,
>    trace uploads, Sentry); immune to server-pushed `remote_settings`.
>    Your own external OTEL collector (`GROK_EXTERNAL_OTEL`) stays available.
> 2. **Startup phone-home beacons disabled** — no `GET /v1/settings`
>    (carried auth + user-id + email) and no `GET /v1/login-config`
>    (carried an `agent_id` machine fingerprint).
> 3. **Reasoning provenance guard** — unsigned/foreign reasoning (e.g. Kimi)
>    is dropped from Anthropic requests instead of replaying as an invalid
>    `thinking` block → prevents `400 Invalid signature` after model switches.
> 4. **Access gate fails open** — with remote fetches disabled no `allow_access`
>    verdict can arrive, so stale cached verdicts are never enforced.
> 5. **`/api-model-update`** — refresh your BYOK `[model.*]` entries from each
>    provider's live model list: new Kimi/Opus releases become usable, retired
>    ones are trimmed. `/api-model-update` prints the plan and writes nothing;
>    `/api-model-update apply moonshot` writes it (backing up `config.toml`
>    first) for Moonshot only. A provider that errors is never trimmed, and
>    entries pinned by `[models]` or in use by the session are always kept.
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
> make doctor     # verify remotes, gh auth, install link, and all patch guards
> make build      # release build of the repo checkout (dev)
> brew install carmilea/grok/grok   # daily driver (primary)
> make test       # fast gate: every crate the patches touch
> make sync       # fetch + merge upstream (then re-check the patches, PATCH.MD §1)
> make pr / merge-pr   # GitHub PR flow via gh (origin = this fork); private
>                        # overrides (internal remotes/flows) go in local.mk
> ```
>
> Notes: the full shell test suite needs `RUST_MIN_STACK=16777216` (set by the
> Makefile); ~27 upstream tests fail identically on pristine `upstream/main` on
> macOS (env/platform, not the fork); tests asserting behavior the patches
> intentionally disable are `#[ignore]`d with an explanatory note.
> See the `Makefile` header for guard locations and the full runbook.

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
