#!/bin/sh
# Cut a release from this machine — no GitHub Actions minutes.
#
#   scripts/release-local.sh v0.1.0                    # all four targets, create + publish
#   scripts/release-local.sh v0.1.0 aarch64-apple-darwin   # one target, upload only
#
# Produces byte-compatible assets to the Release workflow's: a flat
# neboto-<target>.tar.gz (neboto + README.md, LICENSE, PERMISSIONS.md,
# config.example.toml) and a sha256sum-format neboto-<target>.sha256.
# install.sh and cargo-binstall key on those names.
#
# Targets: macOS builds use cargo (Xcode's clang cross-compiles either arch);
# Linux builds use `cross` (Docker). The released cross 0.2.5 (2023) can't
# install its Linux toolchain under rustup ≥ 1.28 on a Mac — install from main:
#   cargo install cross --git https://github.com/cross-rs/cross --rev 8c1a8aa4b661 --locked
# Build one target at a time: two release builds of this crate in parallel
# have been OOM-killed on a 16 GB machine. Inside Docker the limit is Docker
# Desktop's memory cap (7.75 GB here): 12 parallel rustc jobs on the SDK
# crates got the compiler killed, 4 is fine — hence NEBOTO_JOBS.
#
# Env: NEBOTO_TARGETS (space-separated, overrides the default four),
#      NEBOTO_JOBS (parallel rustc jobs for the Linux/cross builds, default 4),
#      NEBOTO_SKIP_BUILD=1 (re-pack + upload existing target/<t>/release/neboto).
set -eu

tag="${1:?usage: release-local.sh <tag> [target]}"
only="${2:-}"
bin=neboto
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
repo="$(gh repo view --json nameWithOwner -q .nameWithOwner)"

say() { printf '\033[1m%s\033[0m\n' "$*" >&2; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

case "$tag" in v[0-9]*) ;; *) die "tag must look like v0.1.0" ;; esac
crate="$(cargo metadata --no-deps --format-version 1 | sed -n 's/.*"version":"\([^"]*\)".*/\1/p' | head -1)"
[ "${tag#v}" = "$crate" ] || die "tag $tag does not match Cargo.toml version $crate — bump and commit first"
[ -z "$(git status --porcelain --untracked-files=no)" ] || die "working tree has uncommitted changes; releases build from a clean, tagged commit"
git rev-parse -q --verify "refs/tags/$tag" >/dev/null || die "tag $tag does not exist locally — git tag $tag && git push origin $tag"
[ "$(git rev-parse HEAD)" = "$(git rev-parse "$tag^{commit}")" ] || die "HEAD is not at $tag — git checkout $tag"

if [ -n "$only" ]; then
  targets="$only"
else
  targets="${NEBOTO_TARGETS:-x86_64-apple-darwin aarch64-apple-darwin x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu}"
fi

out="$root/target/release-assets"; mkdir -p "$out"

build_one() { # target
  t="$1"; archive="${bin}-${t}"
  case "$t" in
    *-apple-darwin)
      rustup target add "$t" >/dev/null
      [ -n "${NEBOTO_SKIP_BUILD:-}" ] || cargo build --release --locked --target "$t" ;;
    *-linux-*)
      command -v cross >/dev/null || die "Linux targets need cross from main (see header) and Docker running"
      [ -n "${NEBOTO_SKIP_BUILD:-}" ] || cross build --release --locked -j "${NEBOTO_JOBS:-4}" --target "$t" ;;
    *) die "unsupported target $t" ;;
  esac
  binpath="target/$t/release/$bin"
  [ -f "$binpath" ] || die "no binary at $binpath"

  stage="$(mktemp -d)"
  cp "$binpath" README.md LICENSE PERMISSIONS.md config.example.toml "$stage/"
  # COPYFILE_DISABLE keeps macOS from adding ._* resource-fork entries.
  COPYFILE_DISABLE=1 tar -czf "$out/$archive.tar.gz" -C "$stage" "$bin" README.md LICENSE PERMISSIONS.md config.example.toml
  rm -rf "$stage"
  (cd "$out" && shasum -a 256 "$archive.tar.gz" > "$archive.sha256")
  say "packed $(cat "$out/$archive.sha256")"
}

# Release: create a draft if none exists (full mode) — a single-target run
# just uploads into whatever release the tag has.
if ! gh release view "$tag" --repo "$repo" >/dev/null 2>&1; then
  [ -z "$only" ] || die "release $tag does not exist; run without a target to create it"
  prerelease=""; case "$tag" in *-*) prerelease="--prerelease" ;; esac
  gh release create "$tag" --repo "$repo" --title "$tag" --draft --verify-tag --generate-notes $prerelease
  say "created draft release $tag"
fi

for t in $targets; do
  say "==> $t"
  build_one "$t"
  gh release upload "$tag" "$out/${bin}-${t}.tar.gz" "$out/${bin}-${t}.sha256" --repo "$repo" --clobber
  say "uploaded ${bin}-${t}"
done

if [ -z "$only" ]; then
  gh release edit "$tag" --repo "$repo" --draft=false --latest
  say "published $tag"
  gh release view "$tag" --repo "$repo"
fi
