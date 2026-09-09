# neboto

A read-only AWS resource browser TUI. This glossary is the canonical language
for the project's own concepts — architecture and per-service detail live in
CLAUDE.md, not here.

## Language

### Detail loading

**Lazy section**:
A detail-pane section whose data is not fetched until the user first views it.
_Avoid_: on-demand section, deferred section

**Lazy**:
The three-state outcome of a lazy fetch — still loading, loaded with a value,
or failed with a message. Every lazy fetch resolves to exactly one of these.
The `*MetricsState` maps behind the `m` overlay are the one surviving
exception, and a deliberate one: they re-fetch on every open and time-range
change, so LazyMap's idempotence is wrong for them (see CLAUDE.md). Nothing
else should introduce a per-fetch state type.
_Avoid_: per-service `*State` enums, LoadState

**LazyMap**:
A keyed collection of Lazy outcomes for one detail concern (e.g. flow logs by
VPC id). Triggering a key that is already present is a no-op, so a section can
be triggered from many places safely.
_Avoid_: state map, state HashMap

**LazyStore**:
The single owner of every LazyMap. It is replaced wholesale whenever the
credentials or region change, so no lazy data can survive a switch — reset is
by construction, not by convention.
_Avoid_: account-scoped state (as a scattered convention)

**Epoch**:
The LazyStore's generation stamp. A fetch that completes under an older epoch
is dropped, never applied — stale in-flight results cannot poison a fresh
store.
_Avoid_: generation (that word belongs to the list-load stream guard,
`load_generation`)

**Apply-closure event**:
The one event (`Event::Lazy`) through which every lazy fetch delivers its
result back into the app, replacing per-fetch `*Loaded` event variants.
_Avoid_: `*Loaded` events (for lazy sections)

### Detail sections

**Section descriptor**:
A resource type's single source of truth for its detail-pane sections — the
ordered list of section definitions the pane cycles through. Everything that
consumes section order (digit keys, Tab cycling, reset, the snapshot, the tab
bar, flat view) derives from it, so order agreement holds by construction.
_Avoid_: section list, section table (when meaning the per-type declaration)

**Section definition**:
One entry in a section descriptor — a label plus an optional on-enter hook.
_Avoid_: tab (that word belongs to sub-tabs, the list-pane view switcher)

**On-enter hook**:
A section definition's optional trigger, fired whenever the section becomes
active. It points at a lazy trigger, whose LazyMap idempotence makes repeated
firing safe.
_Avoid_: section trigger dispatch (as a per-consumer re-encoding)

**Section index**:
The app's single cursor into the selected resource's section descriptor —
which section is active. There is one index, not one enum field per type.
_Avoid_: `*DetailSection` field (the retired per-type App state)

### Selection & export

**Visual selection**:
A contiguous, positional anchor-to-cursor range — over body lines in the
detail pane, over rows in the list pane. Because it is positional, anything
that changes what the positions mean (sort, filter, query edit, sub-tab
switch, reload, refresh swap, drill-in) cancels it rather than remapping it.
_Avoid_: marks, marked set (the rejected non-contiguous model), highlight

**Selection-aware verb**:
A copy/export key that operates on the visual selection when one is active
and falls back to its single-target or whole-list meaning when none is.
When a selection exists, every such verb acts on it — no verb ignores it.
_Avoid_: multi-select action

**Deep export**:
An export carrying a resource's full detail — every section, including lazy
ones. A deep export is only honest when its lazy data is loaded; it never
silently exports unloaded sections.
_Avoid_: full export, detail export (ambiguous with the pane)

**Shallow export**:
An export of list-row fields only — one row per resource, no sections
fetched. Costs zero AWS calls, so it is never capped.
_Avoid_: inventory export, list export (as distinct terms)

**Press-again export**:
The two-step deep export of not-yet-loaded data: the first press fires the
lazy triggers and says so; pressing again once they land exports complete
data. The selection survives the first press by definition.
_Avoid_: S3 dance (the historical single-resource instance)

### Macros

**Step**:
One replayable unit of a macro, recording the *meaning* of a navigation action
rather than the key that produced it — the row's id, the whole search query,
the section's name. This is the central distinction of the feature: raw
keystrokes don't survive a reordered list, an incremental `@prefix` search, or
an async round-trip.
_Avoid_: keystroke, recorded key (a `Key` step is the fallback, not the model)

**Checkpoint**:
A step derived by **diffing app state** after the fact rather than from the key
pressed — the assumed role, region, profile, service, or S3 browser location.
Keys typed inside a picker are filter text and reproduce nothing, so the
*outcome* is what gets recorded, which is also why a macro keeps working when
the picker's contents change.
_Avoid_: modal replay, recording the picker keys

**Quiescence**:
The condition a macro player waits for before injecting its next step —
nothing loading, no pending jump, no region switch in flight (`macro_ready`),
plus a short settle so an enqueued async request has raised `loading` before
quiescence is re-tested. Modals are deliberately *not* part of it: a macro may
legitimately need to drive one.
_Avoid_: sleep, delay, timeout (the gate is a state test, not a wait)

**Collapsing**:
Folding a run of steps that describe the same journey into the one that
describes its destination — a run of `j`/`k` becomes a single `SelectId`, and
browsing into and back out of S3 folders leaves one `S3Object`. Replaying the
intermediate positions would be slower and, on a list that has since
reordered, wrong.
_Avoid_: deduplication (this is about the last step winning, not equality)
