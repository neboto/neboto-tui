#!/bin/sh
# neboto installer — fetches the prebuilt binary for this machine from GitHub
# Releases, verifies its SHA-256, and drops it on your PATH.
#
#   curl -fsSL https://raw.githubusercontent.com/neboto/neboto-tui/main/install.sh | sh
#
# Environment:
#   NEBOTO_VERSION      release tag to install (default: latest), e.g. v0.1.0
#   NEBOTO_INSTALL_DIR  where to put the binary (default: ~/.local/bin)
#   GITHUB_TOKEN        needed while the repo is private (any token with read
#                       access); also lifts the anonymous API rate limit
#
# Supported: Linux (x86_64, aarch64), macOS (Intel, Apple Silicon).
set -eu

REPO="neboto/neboto-tui"
BIN="neboto"
INSTALL_DIR="${NEBOTO_INSTALL_DIR:-$HOME/.local/bin}"
VERSION="${NEBOTO_VERSION:-}"
TOKEN="${GITHUB_TOKEN:-}"

say() { printf '%s\n' "$*" >&2; }
die() { say "error: $*"; exit 1; }

need() { command -v "$1" >/dev/null 2>&1 || die "'$1' is required but not installed"; }
need tar
need uname

# ---- pick the target triple ------------------------------------------------
os="$(uname -s)"
arch="$(uname -m)"
case "$os" in
  Darwin) os_part="apple-darwin" ;;
  Linux)  os_part="unknown-linux-gnu" ;;
  *) die "unsupported OS: $os (Linux and macOS only)" ;;
esac
case "$arch" in
  x86_64|amd64)  arch_part="x86_64" ;;
  arm64|aarch64) arch_part="aarch64" ;;
  *) die "unsupported architecture: $arch" ;;
esac
target="${arch_part}-${os_part}"

# ---- HTTP helpers (curl preferred, wget fallback) --------------------------
if command -v curl >/dev/null 2>&1; then
  http_get() { # url [header...]
    _url="$1"; shift
    _args=""
    for h in "$@"; do _args="$_args -H '$h'"; done
    eval curl -fsSL --retry 3 $_args "'$_url'"
  }
  http_save() { # url outfile [header...]
    _url="$1"; _out="$2"; shift 2
    _args=""
    for h in "$@"; do _args="$_args -H '$h'"; done
    eval curl -fsSL --retry 3 $_args -o "'$_out'" "'$_url'"
  }
elif command -v wget >/dev/null 2>&1; then
  http_get() {
    _url="$1"; shift
    _args=""
    for h in "$@"; do _args="$_args --header='$h'"; done
    eval wget -qO- $_args "'$_url'"
  }
  http_save() {
    _url="$1"; _out="$2"; shift 2
    _args=""
    for h in "$@"; do _args="$_args --header='$h'"; done
    eval wget -qO "'$_out'" $_args "'$_url'"
  }
else
  die "need curl or wget"
fi

api="https://api.github.com/repos/$REPO"
auth=""
[ -n "$TOKEN" ] && auth="Authorization: Bearer $TOKEN"

api_get() { # path
  if [ -n "$auth" ]; then
    http_get "$api$1" "Accept: application/vnd.github+json" "$auth"
  else
    http_get "$api$1" "Accept: application/vnd.github+json"
  fi
}

# ---- resolve the version ---------------------------------------------------
if [ -z "$VERSION" ]; then
  VERSION="$(api_get /releases/latest | tr -d '\n' | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p')"
  [ -n "$VERSION" ] || die "could not determine the latest release (private repo? set GITHUB_TOKEN)"
fi
case "$VERSION" in v*) ;; *) VERSION="v$VERSION" ;; esac

archive="${BIN}-${target}.tar.gz"
say "Installing $BIN $VERSION for $target"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

# ---- download the archive + checksum ---------------------------------------
if [ -n "$auth" ]; then
  # Private repos can't serve the browser download URL — go through the asset
  # API instead (asset ids looked up by name).
  release_json="$(api_get "/releases/tags/$VERSION" | tr -d '\n')" \
    || die "release $VERSION not found"
  asset_id() {
    printf '%s' "$release_json" \
      | grep -o '"id": *[0-9]*[^{}]*"name": *"'"$1"'"' | head -1 \
      | sed 's/^"id": *\([0-9]*\).*/\1/'
  }
  id="$(asset_id "$archive")";           [ -n "$id" ] || die "no asset $archive in $VERSION"
  sum_id="$(asset_id "$archive.sha256")"; [ -n "$sum_id" ] || die "no checksum for $archive in $VERSION"
  http_save "$api/releases/assets/$id"     "$tmp/$archive"        "Accept: application/octet-stream" "$auth"
  http_save "$api/releases/assets/$sum_id" "$tmp/$archive.sha256" "Accept: application/octet-stream" "$auth"
else
  base="https://github.com/$REPO/releases/download/$VERSION"
  http_save "$base/$archive"        "$tmp/$archive"        || die "download failed: $base/$archive"
  http_save "$base/$archive.sha256" "$tmp/$archive.sha256" || die "download failed: $base/$archive.sha256"
fi

# ---- verify -----------------------------------------------------------------
(
  cd "$tmp"
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum -c --quiet "$archive.sha256"
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 -c --quiet "$archive.sha256"
  else
    say "warning: no sha256sum/shasum found; skipping checksum verification"
  fi
) || die "checksum mismatch for $archive"

# ---- install ----------------------------------------------------------------
tar -xzf "$tmp/$archive" -C "$tmp"
[ -f "$tmp/$BIN" ] || die "archive did not contain $BIN"
mkdir -p "$INSTALL_DIR"
install -m 755 "$tmp/$BIN" "$INSTALL_DIR/$BIN" 2>/dev/null \
  || { cp "$tmp/$BIN" "$INSTALL_DIR/$BIN" && chmod 755 "$INSTALL_DIR/$BIN"; }

say "Installed $INSTALL_DIR/$BIN"
case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *) say ""
     say "Note: $INSTALL_DIR is not on your PATH. Add it, e.g.:"
     say "  export PATH=\"$INSTALL_DIR:\$PATH\"" ;;
esac
say "Run '$BIN' to start (needs configured AWS credentials)."
