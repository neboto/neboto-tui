# Releasing

How a version of neboto gets built and published, and how people install it.
The mechanics live in `.github/workflows/release.yml`; this is the runbook.

## What a release is

A git tag `vX.Y.Z` on `main` that matches `version` in `Cargo.toml`. Pushing
the tag runs the Release workflow, which:

1. creates a **draft** GitHub Release with auto-generated notes (the merged
   PRs / commits since the previous tag);
2. builds `neboto` in release mode for every target in the matrix and uploads
   `neboto-<target>.tar.gz` + `neboto-<target>.tar.gz.sha256` into the draft;
3. flips the draft to published (and marks it *latest*) only once every
   target succeeded.

Targets today: `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`,
`aarch64-apple-darwin`, `x86_64-apple-darwin`. Linux builds run on
`ubuntu-22.04` so the binaries need glibc ≥ 2.35; arm64 Linux is
cross-compiled with `cross`, Intel macOS is cross-compiled from Apple Silicon.

The tarball layout is flat — `neboto`, `README.md`, `LICENSE`,
`PERMISSIONS.md`, `config.example.toml` — and the archive name is load-bearing:
`install.sh` and the `[package.metadata.binstall]` table in `Cargo.toml` both
derive it from `<bin>-<target>.tar.gz`. Change all three together.

## Supply-chain rules for the workflows

- Every `uses:` is pinned to a **full commit SHA**, with the human-readable
  version in a trailing comment (`# v4.4.0`). Tags and branches are mutable
  and have been retargeted in real attacks (tj-actions, March 2025); a SHA
  cannot be. The repo setting *Require actions to be pinned to a full-length
  commit SHA* is on, so an unpinned `uses:` fails the run.
- `.github/dependabot.yml` bumps the pins weekly in one grouped PR. Review the
  diff of the action itself when a bump lands — the pin only helps if the
  new SHA gets a look.
- The moving refs those actions rely on (`dtolnay/rust-toolchain@stable`,
  `taiki-e/install-action@cross`) are expressed as inputs instead
  (`toolchain: stable`, `tool: cross`) so the action code stays pinned while
  the toolchain / tool selection still works.
- Builds run `--locked`, so Rust dependencies come from `Cargo.lock` exactly.
- Still unpinned, by design or for now: the `cross` docker image (pulled by
  tag; pin it by digest in a `Cross.toml` if this matters), the Rust
  `stable` toolchain itself, and the GitHub-hosted runner images.

## Cutting a release

```bash
# 1. bump the version (Cargo.toml + Cargo.lock)
cargo set-version 0.2.0        # from cargo-edit; or edit Cargo.toml and run `cargo check`
git commit -am "Release v0.2.0"

# 2. tag + push — the tag push is the trigger
git tag v0.2.0
git push origin main v0.2.0

# 3. watch it
gh run watch
gh release view v0.2.0
```

The workflow refuses a tag whose version doesn't match `Cargo.toml`, so a
forgotten bump fails in the first job rather than shipping mislabelled
binaries. A tag with a pre-release suffix (`v0.2.0-rc.1`) is published as a
GitHub *pre-release* and never becomes *latest*.

**Something failed mid-way?** The draft stays a draft with whichever assets
made it. Fix the problem on `main`, then delete the draft and the tag and
re-tag:

```bash
gh release delete v0.2.0 --yes
git tag -d v0.2.0 && git push origin :refs/tags/v0.2.0
git tag v0.2.0 && git push origin v0.2.0
```

**Trying a toolchain / target change without a tag:** run the workflow by
hand (`gh workflow run release.yml`, or the *Run workflow* button). A manual
run is a dry run — it builds every target and keeps the archives as workflow
artifacts, but creates no release and uploads nothing.

## While the repo is private

- Releases in a private repo are visible only to collaborators. Nothing in
  the workflow changes when the repo goes public — existing releases and
  their assets become public along with it.
- GitHub Actions minutes on a private repo are metered (the `neboto` org is
  on the free plan: 2,000 minutes/month, **macOS counts 10×**, Linux 1×).
  A release runs two macOS builds, so budget several hundred billed minutes
  per release until the repo is public, where Actions are free. This is why
  CI is Linux-only.
- Installing from a private release needs a token. Either

  ```bash
  gh release download v0.1.0 -p 'neboto-aarch64-apple-darwin.tar.gz'
  tar xzf neboto-aarch64-apple-darwin.tar.gz neboto && mv neboto ~/.local/bin/
  ```

  or the installer with a token (it switches to the asset API):

  ```bash
  GITHUB_TOKEN=$(gh auth token) sh install.sh
  ```

  (`curl … | sh` of `install.sh` itself doesn't work on a private repo — the
  raw URL needs auth too — so run it from a checkout.)

## How users install (once public)

| Method | Command |
|---|---|
| Installer script | `curl -fsSL https://raw.githubusercontent.com/neboto/neboto-tui/main/install.sh \| sh` |
| cargo-binstall | `cargo binstall neboto` (needs a crates.io publish, or `--git https://github.com/neboto/neboto-tui`) |
| Manual | download the tarball for your platform from the Releases page, verify the `.sha256`, put `neboto` on `PATH` |
| From source | `cargo install --git https://github.com/neboto/neboto-tui` |

`install.sh` honours `NEBOTO_VERSION` (pin a tag), `NEBOTO_INSTALL_DIR`
(default `~/.local/bin`) and `GITHUB_TOKEN`.

## Not done yet (in rough priority order)

- **Homebrew tap** (`brew install neboto/tap/neboto`): needs a public
  `neboto/homebrew-tap` repo and a job that rewrites the formula's URLs +
  SHAs on each release. Wait for the repo to go public.
- **crates.io publish**: `cargo publish` makes `cargo install neboto` and
  plain `cargo binstall neboto` work. Public source only.
- **macOS signing / notarization**: the binaries are unsigned. Installing
  through the script or `gh` is fine (no quarantine attribute), but a tarball
  downloaded in a browser will trip Gatekeeper; users can
  `xattr -d com.apple.quarantine neboto`. Signing needs an Apple Developer
  account and secrets in the workflow.
- **musl Linux builds** for old-glibc distros / Alpine — `aws-lc-sys` needs a
  musl C toolchain, so it's a `cross` job like the arm64 one.
- **Windows**: `crossterm` and the SDK support it, but the `$EDITOR` / SSM
  session hand-offs and the browser opener are Unix-shaped. Untested.
- **CHANGELOG.md**: release notes are GitHub's auto-generated PR list today.
