# neboto stays read-only: no write actions, however safe

neboto issues only describe / list / get-style calls. Graduated "safe"
mutations were proposed (tag edit, start/stop, force-new-deployment) and
rejected. Two of the reasons go beyond philosophy:

1. **The org member-account switch can't carry them.** Assumed member-account
   sessions are pinned to a `ReadOnlyAccess` session policy. A write action
   would either fail silently cross-account, or the guarantee would have to be
   weakened for every session to make it work.
2. **The read-only footprint is a trust asset.** `PERMISSIONS.md` documents a
   purely read-only set of IAM actions, and security teams can approve neboto
   precisely *because* it cannot mutate. One gated write action changes that
   conversation permanently. It moves from "can't" to "won't, unless", and
   every reviewer has to re-audit the gate.

## Considered options

- **Read-only, with commands copied rather than run (chosen).** `C` copies the
  AWS CLI command for the selected resource. #34 extended it from describe
  commands to operational ones (start/stop, force deploy, scale, connect).
  The user never reconstructs ids or ARNs by hand, and neboto never holds a
  write permission.
- **Gated write actions** (a confirm prompt, an opt-in config flag). This is
  the most convenient option, but it breaks both points above, and a confirm
  prompt is exactly the control people learn to click through.
- **A separate "neboto-write" build.** It would keep the default build
  clean, but it splits the trust story ("which binary did you install?") and
  doubles the IAM documentation.

## Consequences

- No mutating SDK call is merged, whatever the feature.
  `scripts/check-readonly.py` enforces this in CI. #88 tracks a runtime
  backstop (an SDK interceptor that rejects non-read operations).
- An operation that only *looks* mutating (a Logs Insights `StartQuery`, an
  SSM session) needs an allowlist entry in the guard with its reason, and a
  note in `PERMISSIONS.md`.
- `C` never offers anything destructive (terminate, delete, purge,
  deregister) or anything that reveals a secret, and Change-tier commands are
  disabled while an org role is assumed. See "CLI command copy" in
  `CLAUDE.md`.
- Requests for write actions are answered by pointing here; #122 is the
  closed issue that searches land on.
