# Contributing to neboto

Thanks for helping. neboto is a **read-only** AWS browser; that single promise
shapes every rule below.

## The one rule that never bends

neboto only issues describe / list / get-style calls. A pull request that adds
a mutating SDK call — create, delete, put, update, start, stop, modify, tag —
will not be merged, however useful the feature. CI enforces this mechanically
(`scripts/check-readonly.py` checks every SDK operation the code calls and
every IAM action in `PERMISSIONS.md`). If you believe an operation is
read-only despite its verb (CloudWatch Logs `StartQuery` is the canonical
example), add it to the script's allowlist **with a reason** in the same PR
and say so in the description.

## Before you open a PR

```bash
cargo test               # unit tests + the offline wiring harness, no AWS needed
cargo clippy             # must not add new warnings
python3 scripts/check-readonly.py
```

Then walk this checklist — it's also the PR template:

- **New or changed AWS calls?** Add the IAM actions to `PERMISSIONS.md`
  (alphabetical within the service block). The guard fails on non-read verbs.
- **New service or sub-tab?** Read `CLAUDE.md` ("Adding a new AWS service")
  first — the shapes are established and the harness expects specific
  wiring: register in `App::build_services()`, add a mock in
  `src/harness_tests/mocks_*.rs`, declare split-pane sections with the
  `sections!` macro. Then add the service's entry to `docs/SERVICES.md`: the
  API quirks you hit, the caps you chose and why, the approaches you
  rejected. That entry is the part nobody can recover from your code.
- **Touched a detail pane?** Keep the row conventions in `CLAUDE.md`
  (`style_detail_row`) — some jump classifiers key on exact row labels.
- **Colors** go through `src/ui/theme.rs` accessors, never `Color::` literals,
  or the light presets break.
- **Secrets** are never stored in `App`, logged, exported, or echoed into a
  status line. Values are fetched only on `x`/`Y`.
- **Docs**: if behaviour changed, `README.md` (keybindings, services table)
  and the relevant `docs/` file change in the same PR.

## What a good PR looks like

- One concern per PR. A new service and an unrelated refactor are two PRs.
- The description says what you tested against — a real account, the floci /
  LocalStack emulator (`scripts/seed-floci.sh`), or only the offline harness.
  All three are fine; pretending is not.
- Screenshots or a short recording for anything visual.
- Commit history doesn't matter; PRs are squash-merged and you keep
  authorship on the squash commit.

## AI-assisted contributions

Welcome, with the same bar as any other PR. The things a tool won't know are
exactly the things reviewers look for: the `docs/SERVICES.md` entry, the IAM
actions, and whether the calls are actually read-only. A PR that adds a
service without those will be sent back for them, not reviewed around them.

## Reporting bugs and requesting features

Use the issue templates. For a new service request, the most useful thing you
can give is the list of API calls you'd expect it to make and which console
pages it should replace.

## Security

Please don't open a public issue for a vulnerability — see `SECURITY.md`.

## License

By contributing you agree your contribution is licensed under the MIT license
in `LICENSE`.
