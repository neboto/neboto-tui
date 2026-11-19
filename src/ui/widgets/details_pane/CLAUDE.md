# Detail pane renderers

Rules for the `*_section_lines` / `render_*_split` code in this directory.
The architecture (section descriptors, the lazy-section pattern) is in the
root `CLAUDE.md`; flat view, wrap, `$EDITOR` and export are in
`docs/DETAIL-PANE.md`.

**Section 1 is the Overview** (label and variant `Overview`), in this order:
the resource's own id and identity rows — always, even where the header
repeats them, since `y` copies the cursor row and the header can't be
copied; then a short block answering what people open the resource for (a
security group's Exposure, a NACL's Effect, a target group's Health); then
previews of the main tabs, capped at 8 rows with a `· N more — <digit>
<Tab>` note. Every block that can grow carries a cap. A preview reuses its
tab's row formatter (`sg_rule_line`) so the two can't drift, and an Overview
that needs a lazy fetch shares the tab's on-enter hook. The one-row header
drops what doesn't fit; the Overview is where those facts live. Name
section 1 otherwise only when the resource *is* one document (an SCP's
`Document`, a dashboard's `JSON`, Cost's `Breakdown`); older panes opening
elsewhere are candidates for an Overview, not precedents. Reference:
`sg_overview_rows` (`details_pane/ec2.rs`).

**`style_detail_row` conventions** (the `(key, value)` tuples
`*_section_lines` return):
- **Key-value row**: key padded to the body's **adaptive key column**
  (`key_col_widths`: the widest key in the section by display width, clamped
  to `[KEY_COL_MIN=18, KEY_COL_MAX=48]` (keys carry their 2-space indent) and to what the pane can spare; keys
  past the cap are ellipsised, never left to push the `:` out — the flat
  view's `━━` headers split the body so sections align independently); key
  `ACCENT` orange, value conditional colour (state / ✓ / ✗ / ⚠ / white).
  Never pad keys yourself in a `*_section_lines` fn. A flush key is drawn
  two columns in, under its group header (`display_key`); a key you indent
  yourself keeps your indent, so indent only to nest. An **empty value**
  makes the row a group header — write `—` for a missing value instead.
- **Empty-key note**: empty key, non-empty value (`("", "No tags")`) →
  rendered exactly like a plain content line (`"  No tags"`, no colon); the
  two shapes are interchangeable.
- **Group header**: non-empty key, empty value, *not* leading-space → Magenta
  bold subsection label. (No `=== X ===` markers.)
- **Plain content line**: leading-space key, empty value → `Line::raw`, no
  colon. For fixed-width tables, CIDR lists, template previews. Exception:
  `⚠`/`✗`-prefixed content lines render WARNING/ERROR-coloured. A run of
  them that reads as JSON / YAML / shell is syntax-coloured at render
  (`ui/syntax.rs`, see `docs/DETAIL-PANE.md`) — emit code this way, never
  styled per pane.
- **Blank spacer**: both empty → blank line. Spacing is evened out
  centrally (`tidy_section_rows`): none at either end of a section, none
  under a group header, never two in a row — so emit them freely between
  groups and don't hand-tune them.
- **Lazy-fetch error**: always via the `error_rows(&err)` helper (blank spacer
  + `  ⚠ {msg}`) — never hand-rolled `("Error", e)` rows. (An `Error` *group
  header* is still fine for a resource's own failure data, e.g. a FAILED
  Athena query or a Glue run error.)

**`Loading…` rows.** The `Loading…` text is rewritten centrally at render time (`spin_loading_row`):
detail pane focused → animated spinner (a fetch really is in flight); list
pane focused → a static dim `· not loaded — ⏎ to open` hint, because the
unfocused preview's state is untriggered and nothing is loading. Keep emitting
plain `Loading…` from `*_section_lines` — never per-service spinners/hints.
(`style_detail_row` dims any content/value row starting `· ` — the
annotation-row convention.)

**Ownership ribbon** (`src/ownership.rs`): every detail pane shows a dim
`⛓ stack my-stack (AppServer) · terraform · team payments` line bottom-left
on the pane border, resolved **from tags alone** (`resource_ownership` —
CFN's `aws:cloudformation:*` tags, the ManagedBy convention (config
`managed_by_tags`), config `owner_tags` default `owner`/`team`, all
case-insensitive). Config `ownership_ribbon = false` hides the ribbon —
display only, the timeline lens still resolves ownership. One central hook
(`render_ownership_ribbon`, called from the `render_details_pane` wrapper) —
no per-pane wiring; its overlay guard mirrors the dispatch's early returns,
keep them in step. `Enter` on the `aws:cloudformation:stack-name` tag row
jumps to the owning stack (classifier arm 0b). The planned change-timeline
lens reuses the resolver to pick which stack's events to merge.
