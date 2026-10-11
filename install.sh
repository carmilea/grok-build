#!/bin/sh
# install.sh — fetch and install the latest grok-build (fork) release binary.
#
#   curl -fsSL https://raw.githubusercontent.com/carmilea/grok-build/main/install.sh | sh
#
# Honors GROK_INSTALL_DIR to override the install location (default
# $HOME/.local/bin). No sudo; installs a single user-owned binary.
set -eu

REPO="carmilea/grok-build"
INSTALL_DIR="${GROK_INSTALL_DIR:-$HOME/.local/bin}"

err() {
  echo "error: $*" >&2
  exit 1
}

# ---- detect platform --------------------------------------------------------

os_raw="$(uname -s)"
case "$os_raw" in
  Linux) os="linux" ;;
  Darwin) os="macos" ;;
  *) err "unsupported OS '$os_raw'; grok-build ships Linux and macOS builds only" ;;
esac

arch_raw="$(uname -m)"
if [ "$os" = "macos" ]; then
  case "$arch_raw" in
    arm64) asset_os="macos-arm64" ;;
    *)
      err "no Intel macOS build yet (uname -m: $arch_raw); use the Linux tarball \
in a Linux VM, or build from source with 'make build'"
      ;;
  esac
else
  case "$arch_raw" in
    x86_64 | amd64) asset_os="linux-x86_64" ;;
    *) err "unsupported Linux arch '$arch_raw'; only x86_64 is built" ;;
  esac
fi

# ---- resolve latest release --------------------------------------------------

# The tag carries a fourth component — the fork iteration counter, e.g.
# `fork-1.0.45.1` — but assets mirror the three-component crate version with a
# `-s` suffix: `grok-1.0.45-s-macos-arm64.tar.gz` (PATCH.md §3 step 8). Deriving
# the filename by stripping `fork-` alone yielded `grok-1.0.45.1-s-...`, which
# 404s on every release ever cut. Read the asset name out of the release
# metadata instead, so a naming change can never silently break the installer.

release_json=""
if command -v gh >/dev/null 2>&1; then
  release_json="$(gh release view --repo "$REPO" --json tagName,assets 2>/dev/null || true)"
fi
if [ -z "$release_json" ]; then
  command -v curl >/dev/null 2>&1 || err "need 'gh' or 'curl' to resolve the latest release"
  release_json="$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" || true)"
fi
[ -n "$release_json" ] || err "could not reach the release API for $REPO"

# `tag_name` is the REST shape, `tagName` the gh --json shape.
version="$(echo "$release_json" \
  | grep -o '"tag_*[Nn]ame": *"[^"]*"' | head -1 \
  | sed -E 's/.*: *"([^"]+)"$/\1/' || true)"
[ -n "$version" ] || err "could not resolve latest release tag for $REPO"

# Strip `fork-` and the optional fourth (iteration) component. Used for the
# closing message, and to rebuild the asset name if the metadata lookup fails.
fork_version="$(echo "$version" | sed -E 's/^fork-//; s/^([0-9]+\.[0-9]+\.[0-9]+)\.[0-9]+$/\1/')-s"

asset="$(echo "$release_json" \
  | grep -o "\"[A-Za-z0-9._-]*-${asset_os}\.tar\.gz\"" | tr -d '"' | head -1 || true)"
[ -n "$asset" ] || asset="grok-${fork_version}-${asset_os}.tar.gz"
url="https://github.com/$REPO/releases/download/$version/$asset"

# ---- download & verify --------------------------------------------------------

tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT INT TERM

echo "downloading $asset ($version)..." >&2
if command -v curl >/dev/null 2>&1; then
  curl -fsSL "$url" -o "$tmpdir/$asset" || err "download failed: $url"
elif command -v wget >/dev/null 2>&1; then
  wget -q "$url" -O "$tmpdir/$asset" || err "download failed: $url"
else
  err "need 'curl' or 'wget' to download $asset"
fi

tar -tzf "$tmpdir/$asset" 2>/dev/null | grep -qx grok \
  || err "downloaded tarball does not contain a 'grok' binary; aborting"

tar -xzf "$tmpdir/$asset" -C "$tmpdir" grok

# ---- install ------------------------------------------------------------------

mkdir -p "$INSTALL_DIR"
install_path="$INSTALL_DIR/grok"
cp "$tmpdir/grok" "$install_path"
chmod +x "$install_path"

echo "installed grok $fork_version -> $install_path" >&2

case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *)
    echo "" >&2
    echo "note: $INSTALL_DIR is not on your PATH. Add it, e.g.:" >&2
    echo "  export PATH=\"$INSTALL_DIR:\$PATH\"" >&2
    ;;
esac
