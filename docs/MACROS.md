# Macros (`,`, `src/macros.rs`)

Moved out of the root `CLAUDE.md` so it loads only when needed.

A recorded, replayable sequence of navigation steps — the multi-account
routine ("@orgs → Accounts → that account → assume → @sh") as one keystroke.
`,` opens the picker (`⏎` run, `n` record, `d` delete); while recording, `,`
stops and prompts for a name. Saved as JSON via the same best-effort
load/save shape as `bookmarks.rs` (`$NEBOTO_MACROS` → XDG →
`~/.config/neboto/macros.json`); a parse failure yields an empty list, never
an error.

**`,` is intercepted at the top of `handle_key`**, not with the other global
binds, because the detail-pane keymap block ends in `_ => {}` + `return` — it
swallows every key it doesn't name, which is why `S`/`R`/`P`/`M`/`?` all carry
duplicate arms down there. Starting and stopping a recording has to work from
both panes, so it gets one early arm instead of a seventh duplicate. It's
gated on `!search_active && !detail_search_active &&
!any_pane_overlay_active()` so a comma typed into any picker's filter, the
CloudTrail filter modal, the S3 browser's filter or the macro name prompt
stays a literal comma. Inside an in-pane overlay `,` therefore does nothing —
`Esc` out first (deliberate: several overlays have text inputs, and
enumerating which accept a comma is more fragile than one blanket rule).

**Steps, not keystrokes.** A raw `KeyEvent` recording breaks on three things
this app does constantly, so `MacroStep` records the *meaning* wherever the
app already knows it:
- **Cursor keys → `SelectId`.** `j` means "row 2 of whatever the API returned
  today". A run of `j`/`k` collapses to one step carrying the id of the row it
  landed on, so a reordered list still replays. A missing row **aborts**
  playback with a status error — continuing would run the rest of the macro
  against the wrong account.
- **A search run → one atomic `Search`.** `update_search` re-evaluates the
  `@` prefix on every keystroke (`app.rs`), so replaying `@`,`o`,`r`,`g`,`s`
  can fire a load per partial prefix. `/` and `@` (which only *open* the bar)
  are dropped. `Enter` always records the query; `Esc` records only an
  `@service` switch, which Esc doesn't undo — an abandoned *filter* would
  otherwise replay as a list the recording never showed.
- **Picker outcomes → checkpoints.** Keys pressed inside a modal are filter
  text and reproduce nothing, so they're dropped and the result is recorded
  instead: `AssumeRole` / `ExitRole` / `SwitchRegion` / `SwitchProfile` /
  `SwitchService`. Digit/`Tab` section keys likewise record as
  `DetailSection(name)`, name-keyed like bookmark restore.
- **S3 object browsing → `S3Object { bucket, prefix, key }`.** The browser
  counts as a modal for the recorder (`any_selector_visible` includes it):
  its cursor indexes a *filtered, sorted, partially-paginated* view, so a
  keystroke replay lands somewhere arbitrary. Its location is diffed and
  **collapsed** like a cursor run — browsing into and back out of folders
  leaves one step for where you ended up — and replays through the exact
  `pending_s3_object_key` machinery an S3 bookmark restore uses. The browser
  also paginates on its **own** `loading` flag, not `App::loading`, so
  `macro_ready` tests both; without that a step fires into a half-listed
  folder.

**How recording works.** `macro_note_key` stashes the key pre-dispatch (the
search query has to be snapshotted there — `Esc` clears it), then
`macro_record_pass` classifies it one main-loop iteration later and diffs
credential state for the async checkpoints. Diffing centrally is what keeps
this out of the 20k lines of `handle_key` — the same trick
`record_message_history` uses. Two ordering rules earn their keep:
`drop_trailing_opener` removes the `s`/`R`/`P` that opened a picker once its
outcome lands (else replay leaves a modal on screen), and `SwitchService` is
emitted **only on the service picker's closing edge**, because
`current_service` also moves on every `@service` search and on the
assume-role landing, which already have their own steps.

**How playback works.** One step per main-loop iteration, gated on
`macro_ready` — `!loading && !search_needs_update && switching_region.is_none()
&& pending_jump.is_none()` plus the editor/session flags — with a 120ms settle
after each step so an enqueued async request (`OrgRoleSwitchRequested`) reaches
`handle_event` and raises `loading` before quiescence is re-tested. Modals are
deliberately **not** in the gate: a macro may legitimately need to drive one,
and gating on them would deadlock. `Key` steps are injected as `Event::Key` so
the key path can't tell a replay from a real press; the semantic steps are
applied directly.

**`-m/--macro NAME`** runs a saved macro at startup. It arms the *same*
`macro_player` the picker does — no separate playback path, so the quiescence
gate covers the startup load for free. Two wrinkles: the name is resolved by
`App::arm_startup_macro` **after** `from_parts` (not in `Cli::apply_to`, since
it names an action rather than a config value, and the saved macros have to be
loaded first), and an unknown name appends to `error_message` rather than
replacing it, so a config-parse or theme warning already in that one slot isn't
clobbered — the message lists the saved names, which is the only discovery path
before the TUI is up. `macro_supersedes_startup_load` skips `main.rs`'s initial
`load_service_resources` when the macro's first step is a service switch
(`Search("@…")` or `SwitchService`); deliberately narrow, because a macro
opening with `SelectId` still needs the startup view loaded.

**Deliberately not recorded**: `e` and `q` (`is_unrecordable`) — `e` hands the
terminal to `$EDITOR`, which drops and rebuilds the very event channel the
player writes to, and `q` ends the session mid-run. Keys inside the quota /
CloudTrail-filter / SSM-session modals are dropped with no checkpoint, so a
macro simply doesn't reproduce those (it never stalls waiting on a modal it
can't drive). In-pane overlays (`m`/`t`/`o`/`i`) record as raw keys and replay
consistently, but aren't modelled.

## Keycast (`--show-keys`, `src/keycast.rs`)

A corner box naming each key and what it did (`W  change timeline`,
`2  Deployments`), for recordings and screen shares; the demo tapes use it.
It rides the macro recorder's shape: `handle_key` snapshots a
`keycast::View` before dispatch, `keycast_tick` (main loop, after
`macro_tick`) snapshots again, and `classify` names the **difference** — an
overlay opened, the service or section changed, focus moved. A small fallback
table covers keys with no visible effect (copy, wrap, filter chips). So a new
feature gets a correct label for free if its state is in `View`; a new
in-pane view needs a name in `App::keycast_view`'s overlay chain. Typing is
never echoed: while `View.input` is set only `⏎`/`Esc` show.
