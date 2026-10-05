# Console feature parity is not the goal

A missing feature counts as a gap only if **a support engineer would
realistically reach for it mid-ticket and currently can't**. Matching the AWS
Console screen for screen is not the goal.

## Considered options

- **The support-engineer test (chosen).** It keeps the service count and the
  per-service depth tied to what people actually triage. It's also what makes
  "no" an easy answer for features that are large but rarely read.
- **Console parity.** It's an unbounded target: the Console adds surfaces
  faster than a read-only TUI could follow. It would also pull in the
  categories below, which neboto can't or shouldn't offer.

## Consequences

- **Out of scope, and not gaps:** write/mutate operations (ADR 0003),
  dashboard builders, and template designers.
- **New services are filtered by actual user base.** Build-by-demand
  candidates live in #96.
- **Services we won't build:**
  - EOL: App Mesh (2026), Pinpoint (Oct 2026), QLDB.
  - Retired or niche: Proton, Cloud9, Data Pipeline, Clean Rooms,
    CodeCatalyst.

  #125 is the closed issue that searches land on.
- **Thin slices of very large services are fine.** SageMaker would be
  endpoints, training jobs and notebooks, not its whole surface.
