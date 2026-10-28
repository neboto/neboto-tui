# Detail pane internals: flat view, wrap, export

Moved out of the root `CLAUDE.md` so it loads only when needed. The
split-pane architecture and `style_detail_row` conventions stayed there.

**Flat detail view** (`\`, config `detail_flat`): a sticky toggle that swaps
the tabbed view for **all sections concatenated in one scroll**, each headed by
a `━━ Name ━━━…` row (accent-bold; counts as a group header for `[[`/`]]` and
the body filter). Zero per-service code — do NOT add per-type flat handling:
`flat_detail_tick` (main loop, pre-draw) rebuilds the body from
`detail_sections_snapshot()` and a gate in `get_detail_lines()` serves it, so
every body consumer (scroll/copy/visual/search/jump) works unchanged. On focus
it fires one **trigger sweep** (reset to default section +
`cycle_detail_section_next` once around) to start every lazy fetch, and keeps
`detail_section_idx` synced to the cursor's section by the same generic
cycling. Digits/`Tab`/chip clicks become header jump
anchors (`flat_jump_to_section`). Snapshot/section code must call
`section_detail_lines()`, never `get_detail_lines()` (recursion into the flat
cache).

**Detail wrap** (`Ctrl-W`, config `detail_wrap`): render-only — long rows
continue on hanging-indent screen rows under the value column. All three
body renderers go through `layout_detail_body` (`details_pane/mod.rs`), which
takes the per-renderer row styling as a closure, scrolls in **screen** rows
bottom-up from the cursor (the log tail's trick; never `Paragraph::wrap`),
wraps by display width, and records the screen-row → logical-row map
(`App.detail_body_rows`) that mouse clicks/drags and `→` arrow regions use.
It also keeps last frame's top row while the cursor stays on screen (so `k`
moves the cursor, not the view) and marks `/` matches in place (across
spans, so a match spanning a coloured code row's tokens still marks).

**Syntax colours** (config `syntax_highlight`): render-only, and no pane
declares anything. `layout_detail_body` finds runs of plain content rows
(`CodeBlocks`; a `⚠`/`✗`/`· `/tab row ends a run) and asks
`ui::syntax::detect` what each run is: JSON by its opening bracket, shell by
a `#!sh`/`bash` line, YAML by a leading `key:` and mostly mapping/list
lines; anything else stays plain, so a CIDR list or rule table is never
coloured. A run is lexed whole (YAML block scalars carry state) and only
once one of its rows is drawn. The rows stay plain text: copy, export,
headless output and `/` filtering never see the colours, and a selected
row keeps the selection's colours. To get a block coloured, emit it as
plain content lines (`("  {line}", "")`) with a blank spacer or header
around it — nothing else.
**Footer**: renderers pass `detail_footer(…)`'s plain string to the block
as before; `render_details_pane` then repaints it (`render_detail_footer`,
from `App.detail_footer_text`) — keys coloured, fitted beside the
ownership ribbon, each hint a click target. A footer built any other way
gets none of that.
Everything that indexes the body (`j`/`k`, copy, visual, `/`, `[[`) stays
on logical rows. A new body renderer must use the helper, or wrap silently
doesn't apply to it. `w` stays watch mode; in the `/` search box `Ctrl-W`
deletes a word.

## Export

- **`src/export.rs`**: `X` export (detail) / `Ctrl-X` (list) → JSON + CSV +
  Markdown (the formats config `export_formats` enables), via
  `detail_sections_snapshot()`. A single-resource `X` waits for every lazy
  section like the selection deep export does (`pending_deep_export` with
  `single`; `Esc` cancels) — never an export of only the sections you
  opened — and all three of its files come from those sections (JSON =
  `detail_json`, CSV = long `ID,Section,Key,Value`). "Still loading" means
  a whole placeholder section **or** any single `Loading…` row
  (`any_section_unloaded`; `neboto get` waits on the same test). The list
  Markdown and CSV share `list_columns`. Also `detail_json()` — the sections-as-JSON
  serializer behind the default `e` editor view. Files land in the working
  directory, or config `export_dir`; `NEBOTO_EXPORT_DIR` overrides both (the
  test harness sets it to a temp dir). The detail Markdown synthesizes
  a `## Tags` section when no captured section is named "Tags" (exact
  case-insensitive match — `contains` would hit "Stages").
