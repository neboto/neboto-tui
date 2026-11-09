# Subcommands (`neboto ls` / `get` / `open`, `src/headless.rs`, `src/open.rs`)

Moved out of the root `CLAUDE.md` so it loads only when needed.

`neboto services` / `ls <@svc>` / `get <@svc> <id|name>...` run once and
print (table / json / md / csv) instead of starting the TUI — the read-only
guarantee is the point, for scripts and agents (#65 has the roadmap:
`search`, lenses, MCP). `main` dispatches on `cli.command` before the TUI is
built. `ls` builds the same services via `App::build_services` and drains
`list_resources_streaming`'s events (`list_all`), then applies the TUI's
filters as flags. `ls --regions` repeats that per region on
`AwsClients::for_region` (same credentials, region swapped — never a
rebuild per region) and tags each row; an SCP region deny is `denied`, not a
failure. **`get` runs a terminal-less `App`** (`App::new_headless`):
it loads the list the way the main loop does, selects each resource like a
jump (`headless_select` → `align_view_to_resource_type`), then repeats the
deep-export walk (`headless_capture`: list-nav side effects + every
section's on-enter hook) and pumps `handle_event` until no section reads
`Loading…` or `--wait` runs out. So every split pane works with zero
per-service code — **don't add per-type `get` handling**; a pane that
doesn't settle is a trigger bug the TUI has too. It never calls
`handle_key`, which is where the secret reveal lives (a test holds that
line). Exports strip the panes' key hints (`press t to tail`, `x reveal ·
Y copy` — `export_rows`), so write hints in that shape. Rules: stdout carries
only the result (warnings to stderr, so `| jq` holds); JSON carries
`"schema": "neboto/v1"`; exit 0/1/2 = ok/AWS error/usage (incl. not found
or ambiguous); a bad region or profile **fails** (the TUI falls back, which
in a script would answer from the wrong account). **Output contract**: the stable half (flags,
envelopes, core keys, section/type names, prefixes, CSV headers) is pinned
by `contract_tests.rs` against `src/headless/contract.json`. Section
*bodies* are deliberately outside it, so pane work never needs a schema
change — but renaming a section label or a resource type **is** a breaking
change for scripts: the test fails, and the fix is to keep the old name or
bump `SCHEMA`, not to re-bless. Additions: `NEBOTO_BLESS=1 cargo test
contract`. Tests run against the
demo account through `AwsClients::new_demo_for_test()` — never
`demo::enable()` in a test, it's process-wide and would move every other
test's dead-endpoint clients onto fixtures. To prove a call *didn't* happen,
read `demo::TEST_LOG` (every demo request in the test process).
**`skills/neboto/SKILL.md`** is the agent-facing guide to these commands —
change it with the CLI (its `neboto …` examples are parse-checked by
`skill_examples_parse`, but prose about output shapes and exit codes isn't).

## `neboto open` / `--bookmark` (`src/open.rs`, #331)

The one subcommand that **runs the TUI**: `main` routes `Command::Open` to
`run`, not `headless::run`. It is the public way in to what the hidden
`--at` does for a `|` split — **`open`'s arguments are the stable contract**
(its flags are pinned in `contract.json` with the rest); the `NavLocation`
it builds, and `--at`'s encoding of one, are not, so never document or
script `--at`.

- `open::plan` resolves what it can before the TUI starts — the service
  (`resolve_service`, as `get`), an ARN's service and sub-tab via
  `resource_jump_target` (the `Enter` classifiers), the `--charts` range —
  and `main` exits 2 with the reason on a miss. `apply_to` starts on the
  target's service so the startup load is the one it resolves against.
- The rest resolves in `resolve_pending_jump` through the ordinary restore
  path (`restore_startup_location` → `jump_to_location`), with
  `App.pending_open` adding what a bookmark restore doesn't do: a row
  loaded but not in the filtered list goes to its sub-tab
  (`align_view_to_resource_type`, the `sub_tab_table!` an `@all` jump
  uses — `open ec2 sg-…` needs no view); `--section` matches any case
  against the resource's own labels; `--charts` on a type without them,
  an unknown section, and a resource that isn't there each say so
  (`set_open_notice` — re-shown at load completion, whose
  `error_message` clear would otherwise wipe it), the last leaving the id
  in the search bar instead of selecting row 0.
- `--metric NAME` (implies `--charts`) rides `NavLens::Metrics.metric`, so
  a `|` split from the charts and a bookmark saved there keep the metric
  cursor too (`current_nav_lens` records it past row 0). It is matched in
  the series' apply closure, against the rows that came back — a target's
  resolver can reshape the asked-for set — by label or CloudWatch metric
  name, then a unique label prefix, never a hidden row
  (`open::match_metric`). A miss is reported for `open` only; a restore
  quietly stays on row 0.
- `--only` (needs `--metric`; `NavLens::Metrics.only`) narrows the fetched
  rows in place — `SpecMetricsState::narrow_to`, the single-metric mode
  the `a` picker charts into, so `d` restores the set. It keeps the row's
  group (they share its chart) and, to a fixed point, every row a kept
  expression reads (`{MetricName}`), re-marked hidden: series hold only
  shown rows, hidden inputs live in `queries` alone, so the two are
  narrowed separately — a computed row narrowed without its inputs
  charts on the first fetch and fails on the next.
- `--bookmark NAME` picks one saved bookmark (`open::pick_bookmark`: exact
  label or id, else a unique label substring; ambiguous or none lists
  them, exit 2) and restores it like `'` would — its own sub-tab, section,
  lens, region and credentials. It starts on the bookmark's service and
  region (unless `-r` says otherwise).
- TUI flags (`-w`, `--theme`, `--no-banner`…) are top-level, not global:
  they go **before** `open`. Making them global would have `ls`/`get`
  accept and ignore them.
