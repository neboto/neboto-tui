# Split-pane section wiring derives from one section descriptor

A split pane's section order used to be enumerated in seven parallel per-type
lists that had to agree by convention: the `walk!` snapshot, the
cycle-next/cycle-prev if-chains, reset-to-default, the digit-key blocks, the
`render_*_section_tabs` label arrays, and the lazy-trigger dispatch re-encoded
at four of those sites. The drift already broke 15 panes at once; a harness
test policed it after the fact. We replaced the convention with a module: each
resource type declares one `SectionDescriptor` (ordered labels + optional
on-enter hooks) via the `sections!` macro in its service file, exposed through
`Resource::detail_sections()`; the app holds a single `detail_section_idx`
cursor, and every consumer is a generic derivation over the descriptor.

## Considered options

- **Descriptor as data behind a Resource trait method (chosen)** — the 140
  per-type section enums leave App; cycling, reset, digit keys, snapshot,
  tab bar, and flat view become one implementation each. The `sections!`
  macro also emits a renderer-local enum with `from_index`, so `*_section_lines`
  keeps exhaustive matches and cannot drift from the descriptor (both come
  from one list). Cost: the descriptor's on-enter hooks are `fn(&mut App, …)`
  pointers, so service files name App methods.
- **Keep the enums, macro-generate the seven chains** — type-safe matches
  everywhere, but the collapse is in source only: the consumers stay seven
  generated chains, and no generic runtime consumer (descriptor-driven tab
  bar, flat sweep) is possible.
- **TypeId-keyed registry** — O(1) lookup but Rust has no distributed static
  registration; the registration list is itself a central 140-entry list.

## Consequences

- A new split pane is one `sections!` declaration + its renderer arm; do not
  add per-pane arms to cycle/reset/digit/snapshot/tab-bar code.
- The pane's default section is always index 0 — there is no per-pane default
  knob (the flat view's trigger sweep and the harness both assume it).
- On-enter hooks must be idempotent; point them at LazyMap-backed triggers.
- S3 is normalized: Tags is an unconditional seventh section (previously
  appended only when the bucket had tags — the only instance-dependent
  section list, which a static descriptor deliberately cannot express).
- The harness order-agreement test stays as a behavioral regression net; it
  is expected to be trivially green for migrated panes.
