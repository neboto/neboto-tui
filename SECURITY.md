# Security policy

neboto is a read-only AWS browser. The kinds of report we care most about:

- a code path that issues a **mutating** AWS call, or an IAM action in
  `PERMISSIONS.md` that isn't read-only;
- a secret value (Secrets Manager, SSM SecureString, payment material)
  reaching a log, an export, the clipboard without `Y`, or `App` state;
- anything in the release pipeline or `install.sh` that could let a
  tampered binary reach users;
- a dependency advisory we haven't picked up (Dependabot watches, but tell us).

## Reporting

Use GitHub's private vulnerability reporting:
<https://github.com/neboto/neboto-tui/security/advisories/new>. If that isn't
available to you, email <stojan@neboto.dev>. Please don't open a public issue.

You'll get an acknowledgement within a few days. Fixes ship as a patch release
with the advisory published once users have had a chance to update.

## Supported versions

The latest release only. Pre-1.0, every release may change behaviour.

## What we do on our side

- Every GitHub Action is pinned to a commit SHA and the repo rejects unpinned
  ones; Dependabot bumps the pins.
- Builds run `--locked`; the lockfile is scanned against advisories before a
  release (`docs/RELEASING.md`).
- CI runs `scripts/check-readonly.py`, which fails on any SDK operation whose
  verb isn't read-only.
- Release tarballs ship with a `neboto-<target>.sha256`; `install.sh` verifies
  it before installing.
