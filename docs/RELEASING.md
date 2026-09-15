# Releasing

How a version of neboto gets built and published, and how people install it.
The mechanics live in `.github/workflows/release.yml`; this is the runbook.

## What a release is

A git tag `vX.Y.Z` on `main` that matches `version` in `Cargo.toml`. Pushing
the tag runs the Release workflow, which:

1. creates a **draft** GitHub Release with auto-generated notes (the merged
   PRs / commits since the previous tag);
2. builds `neboto` in release mode for every target in the matrix and uploads
   `neboto-<target>.tar.gz` + `neboto-<target>.sha256` (sha256sum format,
   one line per asset) into the draft;
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
- Every archive gets a **build provenance attestation**
  (`actions/attest-build-provenance`, right after the build step): a
  Sigstore-signed statement that this exact file was built by this
  workflow, from this commit, on a GitHub-hosted runner, recorded in the
  public transparency log and stored on GitHub. The build job carries
  `id-token: write` + `attestations: write` for it. Verify with
  `gh attestation verify <archive> --repo neboto/neboto-tui`; list with
  `gh attestation list --repo neboto/neboto-tui`. It covers a swapped asset
  (the `.sha256` is uploaded by the same token, so a checksum alone can't),
  not a compromised build step — the SHA pins are what cover that — and it is
  not macOS code signing. Dry runs attest too, so a `workflow_dispatch`
  exercises the plumbing before a real tag relies on it.
- Still unpinned, by design or for now: the `cross` docker image (pulled by
  tag; pin it by digest in a `Cross.toml` if this matters), the Rust
  `stable` toolchain itself, and the GitHub-hosted runner images.

## Dependency advisories

Run an advisory scan before tagging (`cargo install cargo-audit && cargo
audit`, or the OSV batch API against `Cargo.lock`). A targeted
`cargo update -p <crate>` fixes anything with a semver-compatible patch — the
v0.1.0 lock was several months stale and carried HIGH advisories in the TLS
stack (`aws-lc-sys`, `rustls`, `rustls-webpki`, `h2`, `time`) until those
were bumped.

**Don't run a bare `cargo update` without checking it builds.** As of
2026-09 `aws-smithy-types 1.7.0` changed the public `Document` enum and the
`aws-smithy-json 0.63` that every `aws-sdk-*` crate still depends on no
longer compiles against it, while the newest SDK crates require
`aws-smithy-types ^1.7` — so a full refresh resolves to a set that cannot
build, and it can't be pinned back either. Bump security crates by name until
upstream sorts that out, then retry the full refresh.

Known leftovers that a `cargo update` cannot clear, and why they're accepted:

- `rustls 0.21` / `rustls-webpki 0.101` / `h2 0.3`: the AWS SDK's legacy
  hyper-0.14 client, pulled in by the default `rustls` feature of `aws-config`
  and every `aws-sdk-*` crate. The SDK talks TLS through the modern
  hyper-1 / rustls-0.23 client at runtime; the old stack is compiled in but
  idle. Clearing it means `default-features = false` + an explicit feature
  list on all ~80 SDK crates.
- `lru 0.12` / `paste` (unmaintained): via `ratatui 0.26`. Neither is
  reachable from the TUI's usage; goes away with a ratatui upgrade.
- `aws-sdk-ec2` / `-ecs` / `-rds` (LOW, GHSA-g59m-gf8j-gjf5 — stricter
  validation of the region string): neboto only ever passes region names
  from its own `Region` enum, so it can't be reached. Cleared by the SDK
  refresh above once it builds again.

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

## Build time and cost (measured, 2026-09-15 dry run, cold cache)

| Target | Runner | Wall time | Billed at private-repo rates |
|---|---|---|---|
| x86_64-unknown-linux-gnu | ubuntu-22.04 | 92 min | 92 |
| aarch64-unknown-linux-gnu (cross) | ubuntu-22.04 | 101 min | 101 |
| aarch64-apple-darwin | macos-15 | 67 min | 670 |
| x86_64-apple-darwin | macos-15 | 57 min | 570 |
| CI (test + clippy) | ubuntu-latest | 33 min | 33 per push |

One cold release ≈ **1,430 billed minutes** against the free plan's 2,000 per
month; the macOS legs are 87% of it. `Swatinem/rust-cache` now holds ~1.4 GiB
of compiled dependencies per target, so a warm rebuild should be well under
half that, but any lockfile churn (a security bump) invalidates part of it.

**Zero-minute option (recommended while private): cut the release from a
workstation.** Set the repository variable `RELEASE_ON_CI=false` so a tag push
does nothing on GitHub (skipped jobs cost nothing), then:

```bash
git tag v0.1.0 && git push origin v0.1.0
scripts/release-local.sh v0.1.0          # draft → build 4 targets → upload → publish
```

The script builds the two macOS targets with cargo (Xcode's clang
cross-compiles either architecture from either kind of Mac) and the two Linux
targets with `cross` in Docker. Two things learned setting that up: the
released `cross` 0.2.5 (2023) fails with *"toolchain
'stable-x86_64-unknown-linux-gnu' may not be able to run on this system"*
under rustup ≥ 1.28 — install it from main instead
(`cargo install cross --git https://github.com/cross-rs/cross --rev 8c1a8aa4b661 --locked`);
and Docker Desktop's memory cap (~8 GB) gets rustc OOM-killed with the default
12 parallel jobs on the SDK crates (`could not compile aws-sdk-glue` with no
error text is the symptom) — the script passes `-j 4` (`NEBOTO_JOBS`). The
`aarch64-unknown-linux-gnu` image carries a cross-gcc, so nothing is
emulated. It refuses a dirty tree, a HEAD that isn't the tag, or a version
that doesn't match Cargo.toml, and it packs byte-compatible assets. Expect
roughly 15–25 minutes per target on a 6-core laptop, sequentially — parallel
release builds of this crate have been OOM-killed at 16 GB. Delete the
variable to hand releases back to CI (e.g. once the repo is public).

A middle ground is a **self-hosted runner** (Settings → Actions → Runners):
self-hosted minutes are free on any plan, so the unchanged workflow runs on
your own Mac with `runs-on` pointed at its labels. Don't do that once the repo
is public — a self-hosted runner on a public repo executes pull-request code
from anyone on your machine.

Two ways to keep just the macOS cost off GitHub:

- **Set the repository variable `BUILD_MACOS=false`** (Settings → Secrets and
  variables → Actions → Variables). The workflow then builds Linux only, and
  you upload the macOS tarballs from a Mac with
  `scripts/release-local.sh v0.1.0 aarch64-apple-darwin` and
  `… x86_64-apple-darwin` (cross-compiles from Apple Silicon). The script
  produces byte-compatible assets and refuses to run on a dirty tree or a
  mismatched version. Delete the variable (or set `true`) to go back.
- **Set an Actions spending limit** on the org (default is $0, which makes
  runs fail to start once the free minutes are gone). Beyond the free tier
  macOS is ~$0.08/min and Linux ~$0.008/min, so a cold four-target release is
  roughly $12 and a warm one a few dollars.

Once the repo is public, Actions are free and none of this applies.

`[profile.release] strip = true` is on: it halves the binary (256 → 139 MB
on arm64 macOS; the remaining ~135 MB is code from 80 SDK crates). Thin LTO
or `opt-level = "s"` would shrink it further at the price of longer builds.

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
  gh release download v0.1.0 -p 'neboto-aarch64-apple-darwin.*'
  shasum -a 256 -c neboto-aarch64-apple-darwin.sha256
  tar xzf neboto-aarch64-apple-darwin.tar.gz neboto && mv neboto ~/.local/bin/
  ```

  or the installer with a token (it switches to the asset API):

  ```bash
  GITHUB_TOKEN=$(gh auth token) sh install.sh
  ```

  (`curl … | sh` of `install.sh` itself doesn't work on a private repo — the
  raw URL needs auth too — so run it from a checkout.)

## Going public — checklist

Branch protection, rulesets, secret scanning and push protection all return
*"Upgrade to GitHub Pro or make this repository public"* on the free plan
while the repo is private, so the "main branch isn't protected" banner can't
be acted on yet. The day the repo flips public, in this order:

1. Rewrite history first if it's still wanted (the personal-handle commits) —
   the ruleset below blocks force pushes afterwards.
2. Apply the committed ruleset (no force push, no deletion, CI must pass):
   `gh api -X POST repos/neboto/neboto-tui/rulesets --input .github/rulesets/protect-main.json`
3. Turn on private vulnerability reporting (`SECURITY.md` and the issue
   template link to it; the API returns 404 on a private repo):
   `gh api -X PUT repos/neboto/neboto-tui/private-vulnerability-reporting`
4. Turn on secret scanning + push protection:
   `gh api -X PATCH repos/neboto/neboto-tui --input - <<< '{"security_and_analysis":{"secret_scanning":{"status":"enabled"},"secret_scanning_push_protection":{"status":"enabled"}}}'`
5. `gh variable delete RELEASE_ON_CI` — Actions are free on public repos, so
   releases go back to the workflow.
6. Merge settings: squash only, delete branches on merge —
   `gh repo edit --enable-squash-merge --enable-merge-commit=false --enable-rebase-merge=false --delete-branch-on-merge`
7. Require approval for workflows from **all** outside contributors (one
   merged typo fix shouldn't grant free workflow runs forever):
   `gh api -X PUT repos/neboto/neboto-tui/actions/permissions/fork-pr-contributor-approval --input - <<< '{"approval_policy":"all_external_contributors"}'`
   Never `pull_request_target`, never a self-hosted runner on the public repo.
8. Immutable releases — once published, a release's assets and tag can't be
   changed, which is what `install.sh` and binstall implicitly trust:
   `gh api -X PUT repos/neboto/neboto-tui/immutable-releases`
9. Protect the release tags too (the branch ruleset says nothing about tags):
   `gh api -X POST repos/neboto/neboto-tui/rulesets --input .github/rulesets/protect-release-tags.json`
10. Dependabot **security** updates (alerts alone just sit there; this opens
    a targeted PR the day a CVE lands, instead of waiting for the monthly group):
    `gh api -X PUT repos/neboto/neboto-tui/automated-security-fixes`
11. CodeQL default setup (free on public repos; analyzes Rust, the Python
    guard script and the workflow files):
    `gh api -X PATCH repos/neboto/neboto-tui/code-scanning/default-setup --input - <<< '{"state":"configured","query_suite":"default"}'`

All eleven were applied on 2026-09-15. Still on the account side, not the
API's: two-factor on the `sneboto` login and the org-wide 2FA requirement —
everything above is bypassable by whoever holds that login.

**Consequence of the branch ruleset**: the required status check applies to
direct pushes too, so a fresh commit pushed straight to `main` is rejected
until CI has passed on it. The working shape is branch → PR → CI → squash
merge (the merge settings in step 6 match). Build provenance attestations
are on the release job since 2026-09-16 (see the supply-chain rules above).

## How users install (once public)

| Method | Command |
|---|---|
| Installer script | `curl -fsSL https://raw.githubusercontent.com/neboto/neboto-tui/main/install.sh \| sh` |
| cargo-binstall | `cargo binstall neboto` (needs a crates.io publish, or `--git https://github.com/neboto/neboto-tui`) |
| Manual | download `neboto-<target>.tar.gz` + `neboto-<target>.sha256` from the Releases page, `shasum -a 256 -c` the latter, put `neboto` on `PATH` |
| From source | `cargo install --git https://github.com/neboto/neboto-tui` |

`install.sh` honours `NEBOTO_VERSION` (pin a tag), `NEBOTO_INSTALL_DIR`
(default `~/.local/bin`) and `GITHUB_TOKEN`.

## Demo media

Everything under `demo/` is recorded against the local floci emulator
(`scripts/seed-floci.sh`), so nothing on screen is a real account and nothing
needs blurring. The `⚙ localhost:4566` badge in the tab bar is the honest
signal that it is an emulator — leave it in.

- **`demo/neboto.gif`** — the loop embedded at the top of `README.md`.
  Committed; ~1.5 MB. Re-record with `vhs demo/neboto.tape` (vhs **v0.11.0**,
  see the tape header — v0.12.0 silently writes nothing). Release builds only:
  the tape runs `./target/release/neboto`, so rebuild before recording or the
  GIF shows stale behaviour.
- **`demo/neboto.mp4`** — the same take, for the README video and social
  posts. **Not committed** (`.gitignore`); it duplicates the GIF and would grow
  history by ~1 MB per re-record.
- **Embedding the video in the README.** GitHub renders an inline player only
  for videos uploaded through its web UI, never for a file path in the repo:
  1. Open any issue, PR or release description on GitHub and drag
     `demo/neboto.mp4` into the comment box (no need to submit the comment).
  2. It expands to a `https://github.com/user-attachments/assets/<uuid>` URL —
     copy it.
  3. Paste that URL on a line of its own in `README.md` where the
     `<!-- Full walkthrough video … -->` comment sits. A bare URL line is
     what GitHub turns into a player; a Markdown link is not.
  Limits: 10 MB per video, mp4/mov/webm. The asset inherits the repo's
  visibility, so upload it while private and it stays viewable after going
  public.
- **More GIFs** (one feature each — log tail, metrics, the `W` timeline) go in
  `demo/<feature>.gif` from a `demo/<feature>.tape` that shares
  `demo/neboto.toml`, and are referenced from the matching README bullet
  rather than stacked at the top: the first screen should stay one loop.
- **Recording tips**: `Hide` the boot spinner, keep scenes ≥ 1.5 s so a
  reader can parse each screen, `Set Framerate 12` keeps the GIF small, and
  check a few frames (`ffmpeg -ss <t> -i demo/neboto.mp4 -frames:v 1 f.png`)
  before committing — the status bar is where stray warnings show up.

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
