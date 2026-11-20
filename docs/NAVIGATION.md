# Navigation: search, jumps, history, keys, mouse

Moved out of the root `CLAUDE.md` so it loads only when needed. Read the
section you are touching before changing search, a jump classifier, nav
history / bookmarks, a key, a modal, an in-pane view, or a click target.

## `@all` search

**`@all <text>`** (or nebaz's spelling `@* <text>`; checked by
`parse_all_query` *before* `parse_query`) searches **every warm cache
entry** — it never fires fetches: while `App.all_search_mode`, `resources`
holds the flattened cached lists (`rebuild_all_search_resources`, rebuilt
each keystroke so background loads show up) with a parallel
`all_search_sources: Vec<ServiceType>` feeding the per-row service badge;
`active_type_filter` returns None; Enter = `jump_to_all_result` (exit mode →
`switch_service` → exact-id pending jump, detail-focused) while `l`/`→`
**peek** — `focus_details_panel` over the results, as a click does (panes
render from the resource, not the service on screen; an other-region S3
stub jumps instead). Under @all the sub-tab row is hidden and the list
pane's `Tab`/`Shift-Tab`/`H`/`L` step the **service-strip pick** instead
(`step_all_search_filter`: all → each service with matches → all; digits
only hint) — never the sub-tab handlers, which would switch the hidden
service's view; the **service strip** leads with an active `@all` chip and counts
each chip's matches (`all_search_match_counts`), `–` for a visited
service with no warm cache (`all_search_searched` — so "no match" never
reads as "not searched"); clicking a searched chip narrows the results to
that service (`all_search_service_filter`, `ClickAction::AllSearchFilter`
— never a service switch, which would change the hidden service under
the still-held `@all` query), counts are taken **before** that narrowing
(`all_search_counts`), and `z` gains a `service` sort, and `r` refreshes the peeked sections + rebuilds from the caches
instead of reloading the service underneath. Guards: the
stream handlers drop batches and **skip the cache insert** while the mode
holds foreign rows in `resources` (else a completing load would poison that
service's cache), watch mode holds off, and `prepare_service_view`
force-exits the mode on any explicit service switch.

## Cross-service "go to resource" jump (`Enter`), history, bookmarks

`resource_jump_target(key, value, current)` in `details_pane/mod.rs` recognizes ARNs
and id prefixes (`vol-`/`eni-`→EC2, `subnet-`/`vpc-`→VPC, IAM ARNs, an
`arn_jump_target` fallback for full service ARNs, ECR image URIs
`…dkr.ecr….amazonaws.com/<repo>[:tag]` via `ecr_repo_from_image_uri`, plus
`s3://`/`s3a://`/`s3n://` URIs → the S3 bucket via `s3_uri_bucket`, so Glue table
locations / crawler S3 targets / job script paths jump to their bucket, S3
REST/website endpoint hostnames (CloudFront origin domains) → the bucket via
`s3_origin_domain_bucket`, and bare
`alias/…` KMS alias references (e.g. SNS's `KmsMasterKeyId`) → KMS) → a
`JumpTarget { service, view, id }`. `Enter` follows any target incl. cross-service: `switch_service` +
`apply_jump_view` + stash `pending_jump`, resolved by `resolve_pending_jump`
once the target streams in. Resolution is **exact id, then exact name** (name-
keyed classifier arms — IAM role/policy, CFN stack, KMS alias — reduce ARNs to
names while the resource's `id()` may be the ARN); an exact resolution lands
focused in the **detail pane** (`pending_jump_focus_details` — follow-link
semantics; also the S3 cross-region drill); query-style targets that never
match exactly (e.g. ECS service→its tasks) keep landing in the filtered list,
and any keypress cancels a still-pending focus so a late async resolve can't
steal it. Jumpable rows
show a `→`. Specialized classifiers
are tried first (e.g. the ECS service→task/target-group ones, `cfn_resource_jump_target`,
`ic_jump_target`). The legacy `gd` chord still works, unadvertised. Nav-back /
the `` ` `` jump list / bookmarks restore at captured depth: `NavLocation`
records `details_focused` + the active section name (`detail_section`; both
serde-defaulted for old bookmark files), so a location saved from a detail
pane re-enters it at that section on restore. The generic capture/restore
rides `detail_sections_snapshot_with_active()` (the descriptor walk also
reports the active index) and `restore_detail_section_by_name` (the label
position in the descriptor IS the section index — a direct jump, only the
target's on-enter hook fires); unknown names fall back to the default
section. `NavLocation.region` (the capture region, global services
included, so back from a jump that switched region switches back; `None`
for a global service's bookmark and old bookmark files) makes history cross-region: picking a
region in `R` records "here" like a service switch, and restoring a
location from another region goes through the cross-region jump below
(`pending_jump_region` + `RegionSwitchRequested`), whose handler filters
and resolves on a cache hit. The footer hint, jump list and bookmark list
name the region when it isn't the current one (`other_region`). `NavLocation.credentials` (caller account +
profile + assumed org role; `None` for old entries) does the same across
accounts. **The account is the identity** (`NavCredentials::same_as`); profile
and role are only the route back (`same_route`) — a route that now reaches
another account is refused, never resolved there, since same-named rows
across accounts would open the wrong account's resource. No profile name
(env keys) = `profile: None`, re-enterable only within the run
(`switch_to_startup_credentials`, checked against `startup_account`); never
store key material to make it work across runs. A switch via
`P` / `s` / role exit records "here" once it succeeds, and restoring a place
from another session stashes it in `pending_credential_restore`, replays the
switch through the same events (profile first, then the role), and runs the
restore only once the credentials match (a place with an account waits
for `AccountInfoLoaded` after the switch — `awaiting_identity`) — a failed switch leaves the view
and the history cursor untouched.

**Cross-region jump (Resource Explorer)**: `trigger_jump` captures the selected
resource's region *before* clearing selection; if it differs from
`current_region` it sets `pending_jump_region` and sends
`Event::RegionSwitchRequested` (switch-region + reload the target service).
`resolve_pending_jump` returns early until `current_region == want`, so the
stale old-region load can't claim the jump. Resource Explorer itself
(`@explorer`, global, gated) discovers the AGGREGATOR index via `ListIndexes`
and runs `Search("*")` from that index's region.

## Clipboard, split out, CLI command copy

- **Clipboard** (`src/clipboard.rs`): every copy goes out as OSC 52 (tmux
  passthrough inside tmux), which is how it reaches the user's clipboard over
  SSH / SSM, and to the system clipboard through one handle kept for the
  process — dropping it clears the selection on Linux. Reuse it via
  `copy_to_clipboard`; under `cfg(test)` neither sink is touched
  (`App::clipboard_last` reads the copy back).
- **Split out** (`|`, `src/split.rs`): a second neboto process in a new
  pane of the host multiplexer / terminal (else a tab or window where it
  has no split; each `Launch` says which, and whether to wait for it), started at `current_location()`
  via a hidden `--at` (base64url `NavLocation`) plus the session flags, and
  restored by `restore_startup_location` through the bookmark path. No
  in-process panes: `App` owns the whole screen. Credentials follow the
  `session_launch` rules — never in the command. `|` is checked ahead of
  the in-pane views' handlers unless one is taking text
  (`in_pane_view_typing` — a new text input needs a line there); charts
  come back through `NavLens::Metrics`, the object browser through
  `s3_object_path`, other views as the pane behind them.
- **CLI command copy** (`C`, either pane; `src/aws/cli_actions.rs`, picker
  `cli_picker.rs`): `Resource::cli_command()` returns the per-type **read**
  command (`aws ec2 describe-instances --instance-ids …`) — always the first
  row — and `Resource::cli_actions()` adds operational commands in three
  tiers: Inspect, Connect (`ssm start-session`, `eks update-kubeconfig`,
  `ecs execute-command`) and Change (start/stop, force deploy, scale).
  neboto **never runs them** — `C` only copies, so the read-only guarantee
  and `PERMISSIONS.md` are untouched. `App::copy_cli_command` appends
  `--region` + `--profile` + `--endpoint-url` (an emulator session must say
  so, or a pasted change hits the real account); the profile is omitted
  while an org role is assumed (a flag can't reproduce that session), and
  **Change rows are disabled then**. One row copies straight away (the
  original `C`); more open the picker. A list visual selection offers the
  `CliAction::batchable` commands every selected row shares, ids merged
  into one list. Rules: **nothing destructive** (no terminate / delete /
  purge / deregister), never a command that reveals a secret (Secrets
  Manager → `describe-secret`, SSM parameter → `get-parameter` without
  `--with-decryption`), value-taking commands prefilled with the **current**
  value so a blind paste is a no-op; quote values with
  `aws::resource::shell_quote`.

## Keymap and visual selection

- **Navigation keymap** (spatial: `h`/`l` are pane movement, not tab cycling).
  List pane — `j`/`k`, `gg`/`G` (top/bottom), `Ctrl-d`/`Ctrl-u` (half-page),
  `l`/`→`/`Enter` (drill into detail), `⌫`/`Ctrl-O`
  (nav-back through history — **not** `h`/`←`: from the detail pane they go
  back to the list, and a second tap popping history lost your place),
  `Ctrl-F` (forward again; intercepted at the top of `handle_key` like `,`,
  since plain-`f` arms that ignore modifiers would swallow it — and going
  back from live first records "here", `park_here`, so forward has
  somewhere to return to),
  `Tab`/`Shift-Tab` (cycle sub-tabs), `1`–`9` (jump to sub-tab), `H`/`L`
  (previous/next sub-tab from **either** pane — `step_sub_tab` replays
  `Shift-Tab`/`Tab` through the list handlers and re-opens the new tab's
  first row when pressed in the detail pane — on a tab still streaming it
  waits for the first row (`pending_subtab_focus`; any key or click cancels,
  an empty load drops it), as nebaz does; gated by `has_sub_tabs`), `/` (search), `@` (search seeded with `@`, for a fast
  `@service …` switch — works from either pane, via `start_service_search`),
  `y` (copy id/ARN; with a visual selection active, copy the rows as a
  Markdown table), `C` (copy an AWS CLI command — picker), `V`/`J`/`K`/`Ctrl-A` (visual
  row selection — see below), `a` (hide-noise filter, gated by
  `is_noise()`; **noise shows by default** — `a` opts in to hiding; the arm
  must check for **no modifier**, or it eats `Ctrl-A` select-all on every
  screen that has noise rows),
  `z` (cycle list sort: load order → name ↑ → name ↓ → state,
  severity-ranked, plus `service` under `@all`; with a fuzzy query the
  default keeps match order, but a sort picked with `z` wins — `@all`
  always has query text; reset on service switch). **Exception to "load order"**: the two Executions sub-tabs
  (`"Pipeline Execution"` / `"State Machine Execution"`) default to
  **newest-first** via `execution_start_ms` — their rows span every
  pipeline / state machine and the batches arrive in `buffer_unordered`
  completion order, so raw load order interleaves runs meaninglessly. `z`
  still overrides, and the per-batch sort in the service files matches so a
  parent's own Executions section reads the same. `F` (cycle state filter through the states present in the current
  view; reset on service switch; both render corner chips in
  `resource_list.rs`). **The `F` chips and the wide list's state column
  show `Resource::state_label()`, not `state()`**: `state()` is a coarse
  colour/sort bucket most types map their native status onto (NON_COMPLIANT
  → Unavailable, `active` → Running), so a type whose mapping changes the
  word must override `state_label()` with its own vocabulary
  (`native_state_label(&self.status, || self.state())` for the plain
  status-string case; hand-written words for bool-derived states). Without
  it the chip reads "unavailable" for a Trusted Advisor "action recommended"
  check — which is how this shipped across most services once. A kind
  with **no lifecycle state in AWS** (IAM role, security group, queue, log
  group…) returns `ResourceState::stateless()` — dim `○`, blank label, `F`
  reports nothing to filter — never a constant `Available`, which paints a
  meaningless green "available" down the whole column, `` ` `` (jump list), `B`/`'` (bookmarks), `,` (macros —
  see Macros below; `⏎` runs, `n` records, `,` again stops a recording),
  `M` (message
  history — past status errors/toasts, newest first, `y`-copyable), `w` (watch
  mode — auto-refresh, `+`/`-` interval; works from the detail pane too, see
  Caching), `S`/`R`/`P`
  (service/region/profile selectors, all usable from the detail pane too).
  Detail pane — `j`/`k`, `gg`/`G`,
  `[[`/`]]` (group headers), `Tab`/`Shift-Tab` + `1`–`9` (sections),
  `H`/`L` (sub-tabs, as in the list pane),
  `l`/`→`/`Enter` (follow jump), `h`/`←`/`Esc` (back to list), `⌫`/`Ctrl-O`
  (nav-back through history, same as the list pane; the service strip's
  left-edge `←`/`→` are the clickable back/forward), `y`/`e`,
  `m`/`o`/`t` (rich views), `W`/`U`/`N` (timeline / referenced-by /
  network-access lenses, also from the list pane), `X` (export detail;
  `Ctrl-X` exports the list),
  `@` (jump to `@service` search), `/` (in-pane body search, not global),
  `\` (toggle flat detail view — all sections in one scroll; also works from
  the list pane; in flat view `1`–`9`/`Tab` jump to section headers).
  **Linewise visual selection** (vim-style, all services, **both panes**):
  `V` toggles select mode at the cursor, `J`/`K` or `Shift-↓`/`Shift-↑`
  start-and-extend, and once active plain `j`/`k`/`gg`/`G` (detail adds
  `[[`/`]]`) extend it (anchor held); **`Ctrl-A` selects all**; `y`/`c` copies
  the range, `Esc` cancels (a second `Esc` goes back to list / clears the
  query). Detail pane: backed by `App.detail_visual_anchor` +
  `detail_line_in_selection`/`detail_visual_range`/`copy_detail_selection`;
  copies the lines as text. **List pane**: backed by `App.list_visual_anchor`
  + `list_visual_range`/`list_row_in_selection`; the range is **positional**
  over `filtered_resources`, so any rebuild (`update_search` — sort, state
  filter, query edits, sub-tab switch, reload, watch swap) or drill-in
  cancels it, and watch mode holds off while one is active. While a list
  selection is active every copy/export verb operates on it: `y`/`c` copies
  the rows as the `list_markdown` Markdown table (clipboard), `Ctrl-X`
  shallow-exports just those rows, and `X` **deep-exports** them as one
  combined `neboto-<label>-deep-<ts>` trio (`export_detail_multi`: JSON
  array / `##`-chapter Markdown / long-format `ID,Section,Key,Value` CSV) —
  the press **arms** it rather than performing it (`export_selection_deep`
  fires every lazy trigger per resource, then stores the rows **by id** in
  `App.pending_deep_export`; `deep_export_tick` — a main-loop hook next to
  `flat_detail_tick` — re-walks them each iteration and writes the files once
  nothing is `Loading…`, or after a 2-minute wait limit, noting how many were
  incomplete). Ids, not positions, because the wait outlives the positional
  selection. Progress renders in its own status-bar slot
  (`App.deep_export_progress`) rather than `success_message`, which repaints
  several times a second and would flood the `M` history; `Esc` cancels, as
  does a service/region/credential switch or `r`. It used to demand a second
  `X` press instead — on a throttled API (Organizations) that is
  indistinguishable from the key doing nothing. Capped at
  `MAX_DEEP_EXPORT=50` (the zero-API table copy and shallow export are
  uncapped).
  Left-**drag** in either body selects the same range; hold **Shift** to
  bypass mouse capture for native terminal text selection instead.

## Mouse

- **Mouse**: `App::handle_mouse` maps clicks via `App.click_regions` (tab bars,
  rebuilt each frame by the tab widgets) and `App.mouse_geom` (list/detail
  rects). A sub-tab click replays the chip's key through `handle_key`.
  Status-bar key hints, the status-bar region name and the strip's profile /
  `S: services` badges are click targets too (`ClickAction::Press`, which,
  unlike a sub-tab's `Key`, presses the key where focus already is). A new
  status-bar hint is clickable for free if its key is one key (see
  `hint_click_key`; `q` is deliberately excluded). The welcome splash's
  popular-service chips and `S`/`R`/`P`/`?` rows are targets too.
  **Centered modals** (every picker, help, the CloudTrail filter, the SSM
  menu) record their rect + visible rows into `App.popup_hits` as they draw
  (`PopupHits::record_list` / `record_area`; cleared at the top of each
  frame), and `handle_modal_mouse` replays keys through `handle_key`: wheel =
  `↓`/`↑`, click highlights, double-click or a click on the highlighted row
  = `⏎`, outside / right-click = `Esc`. **A new modal** needs an arm in
  `App::open_modal` (in `handle_key`'s priority order) and
  `select_modal_row`, plus a `record_*` call in its renderer, or it ignores
  the mouse. `C`/`M` keep their own handler (`M`'s double-click copies).
  Also clickable: the tab bars' `‹`/`›` markers (act as the first hidden
  chip on that side), the search bar (`Key('/')`), the detail pane's top
  border (`Z`), and a drawn `→` (`ClickAction::FollowJump(row)`, recorded by
  `record_jump_arrows` only while the pane is focused). **In-pane views**
  (`m`/`t`/`o`/`W`/`U`/`N`) go through `handle_pane_view_mouse`: wheel =
  `↓`/`↑` over the view, click moves the cursor, double-click = `⏎` (one
  click never acts, as in the list), right-click = `Esc`, anything else is
  swallowed. Their renderers record the view rect + visible rows into the
  same `popup_hits`; a new in-pane view needs `open_pane_view` /
  `select_pane_view_row` arms. The hand-built metrics kinds take only the right-click; the spec view's
  metric rows and breakdown tables (a web ACL's rules) take clicks and the wheel. The README's Mouse
  table and the `?` help's Mouse section list the gestures; keep them in
  step. `mouse_smash_never_panics` clicks every recorded region over every
  mock.
  **Double-click** (second left press on the same cell within 400ms, tracked by
  `App.last_left_click`) replays `Enter` — drill into detail from the list,
  follow a jump link in the detail body. **Right-click** = nav-back
  (`nav_back`, mirrors `Ctrl-O`/`h`).
