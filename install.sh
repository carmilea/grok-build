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

if command -v gh >/dev/null 2>&1; then
  version="$(gh release view --repo "$REPO" --json tagName --jq .tagName 2>/dev/null || true)"
else
  version=""
fi

if [ -z "$version" ]; then
  command -v curl >/dev/null 2>&1 || err "need 'gh' or 'curl' to resolve the latest release"
  version="$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" \
    | grep '"tag_name"' | head -1 | sed -E 's/.*"tag_name": *"([^"]+)".*/\1/')"
fi
[ -n "$version" ] || err "could not resolve latest release tag for $REPO"

fork_version="$(echo "$version" | sed -E 's/^fork-//')-s"
asset="grok-${fork_version}-${asset_os}.tar.gz"
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
