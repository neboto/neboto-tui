# Headless subcommands (`neboto ls` / `get`, `src/headless.rs`)

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
