#!/bin/sh
# neboto installer — fetches the prebuilt binary for this machine from GitHub
# Releases, verifies its SHA-256 and (when the GitHub CLI is available) its
# signed build provenance, and drops it on your PATH.
#
#   curl -fsSL https://raw.githubusercontent.com/neboto/neboto-tui/main/install.sh | sh
#
# Prefer to read it before running it:
#   curl -fsSLO https://raw.githubusercontent.com/neboto/neboto-tui/main/install.sh
#   less install.sh && sh install.sh
#
# Environment:
#   NEBOTO_VERSION      release tag to install (default: latest), e.g. v0.1.0
#   NEBOTO_INSTALL_DIR  where to put the binary (default: ~/.local/bin)
#   NEBOTO_NO_ATTEST=1  skip the provenance check even when `gh` is present
#   GITHUB_TOKEN        lifts the anonymous API rate limit; needed if the repo
#                       is ever private (any token with read access)
#
# Provenance: every release archive from v0.2.0 on carries a build provenance
# attestation (see docs/RELEASING.md). With `gh` installed and logged in, the
# archive is verified against it and a mismatch aborts the install — a swapped
# asset can't pass, even if its .sha256 was swapped with it. Without `gh` the
# SHA-256 check still runs; `gh attestation verify <archive> --repo neboto/neboto-tui`
# does the same check by hand.
#
# Supported: Linux (x86_64, aarch64), macOS (Intel, Apple Silicon).
set -eu

REPO="neboto/neboto-tui"
BIN="neboto"
INSTALL_DIR="${NEBOTO_INSTALL_DIR:-$HOME/.local/bin}"
VERSION="${NEBOTO_VERSION:-}"
TOKEN="${GITHUB_TOKEN:-}"
# Releases cut before the workflow attested its archives. Verification is
# skipped (with a note) for these instead of failing on "no attestation found".
UNATTESTED_VERSIONS="v0.1.0"

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
    eval curl -fsSL --proto '=https' --tlsv1.2 --retry 3 $_args "'$_url'"
  }
  http_save() { # url outfile [header...]
    _url="$1"; _out="$2"; shift 2
    _args=""
    for h in "$@"; do _args="$_args -H '$h'"; done
    eval curl -fsSL --proto '=https' --tlsv1.2 --retry 3 $_args -o "'$_out'" "'$_url'"
  }
elif command -v wget >/dev/null 2>&1; then
  http_get() {
    _url="$1"; shift
    _args=""
    for h in "$@"; do _args="$_args --header='$h'"; done
    eval wget -qO- --https-only $_args "'$_url'"
  }
  http_save() {
    _url="$1"; _out="$2"; shift 2
    _args=""
    for h in "$@"; do _args="$_args --header='$h'"; done
    eval wget -qO "'$_out'" --https-only $_args "'$_url'"
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
# The tag is spliced into URLs and shell strings below — accept only tag-shaped input.
case "$VERSION" in
  *[!A-Za-z0-9.+-]*) die "refusing suspicious version string: $VERSION" ;;
esac
case "$INSTALL_DIR" in
  *"'"*) die "install dir may not contain a single quote" ;;
esac

archive="${BIN}-${target}.tar.gz"
sums="${BIN}-${target}.sha256"   # sha256sum-format lines for every asset of this target
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
  sum_id="$(asset_id "$sums")";          [ -n "$sum_id" ] || die "no checksum file $sums in $VERSION"
  case "$id$sum_id" in *[!0-9]*) die "unexpected asset id from API" ;; esac
  http_save "$api/releases/assets/$id"     "$tmp/$archive"        "Accept: application/octet-stream" "$auth"
  http_save "$api/releases/assets/$sum_id" "$tmp/$sums"    "Accept: application/octet-stream" "$auth"
else
  base="https://github.com/$REPO/releases/download/$VERSION"
  http_save "$base/$archive"        "$tmp/$archive"        || die "download failed: $base/$archive"
  http_save "$base/$sums"    "$tmp/$sums"    || die "download failed: $base/$sums"
fi

# ---- verify -----------------------------------------------------------------
(
  cd "$tmp"
  if command -v sha256sum >/dev/null 2>&1; then
    grep " $archive\$" "$sums" | sha256sum -c --quiet -
  elif command -v shasum >/dev/null 2>&1; then
    grep " $archive\$" "$sums" | shasum -a 256 -c --quiet -
  else
    say "warning: no sha256sum/shasum found; skipping checksum verification"
  fi
) || die "checksum mismatch for $archive"

# ---- verify provenance (GitHub CLI, when available) ------------------------
# The .sha256 file is uploaded by the same token as the archive, so it can't
# catch a swapped asset; the attestation is signed with an identity only the
# release workflow can hold.
attest_status="skipped"
if [ "${NEBOTO_NO_ATTEST:-}" = "1" ]; then
  attest_status="skipped (NEBOTO_NO_ATTEST=1)"
elif ! command -v gh >/dev/null 2>&1; then
  attest_status="skipped (install the GitHub CLI to verify build provenance)"
elif ! gh auth token >/dev/null 2>&1; then
  attest_status="skipped (gh is not logged in; run 'gh auth login' to verify build provenance)"
else
  unattested=0
  for v in $UNATTESTED_VERSIONS; do [ "$v" = "$VERSION" ] && unattested=1; done
  if [ "$unattested" = 1 ]; then
    attest_status="skipped ($VERSION predates attested releases)"
  elif gh attestation verify "$tmp/$archive" --repo "$REPO" >/dev/null 2>"$tmp/attest.err"; then
    attest_status="verified (signed by the release workflow of $REPO)"
  else
    say "$(cat "$tmp/attest.err")"
    die "build provenance verification FAILED for $archive — not installing"
  fi
fi
say "Provenance: $attest_status"

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
