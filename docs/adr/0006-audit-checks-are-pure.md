# Audit checks are pure functions over fetched data, and can't fetch

`@audit` (#124) is a virtual service whose rows are security findings. Every
check is a pure function: it takes resource lists (and, from #206, read-only
account facts) that neboto has **already fetched**, and returns findings. A
check never calls AWS.

The TUI audits the **warm caches**, as `@all` searches them (ADR 0004). A
check whose inputs aren't cached produces a "not checked — @x not loaded" row,
never silence. `r` in `@audit` loads the missing source services into the
cache: the same list loads you'd get by visiting them, fired by an explicit
key press rather than on open. `neboto audit` has no cache, so it loads the services the checks read
first. That's the same list load `neboto ls` does, nothing more.

## Considered options

- **Pure checks over fetched data (chosen).**
  - Every check is a unit test over mocks.
  - The read-only guard (ADR 0003) covers the audit with no extra work, since
    the audit adds no SDK calls.
  - It keeps the door open for rule packs (#208): a pack rule is a check whose
    logic comes from a file.
- **Checks that fetch what they need.** That's what Prowler-style tools do.
  Each check would become its own API client, with its own pagination,
  throttling and permission errors, and the guard would have to audit check
  code, not just the service files. Rejected. When a check needs data no list
  carries (KMS rotation, access-key age), the fetch belongs in the loader
  (#206) or a lazy section, where it's visible, capped and shared with the
  panes.
- **A full background sweep on open.** ADR 0004 rejected this for `@all`
  (throttling, noisy permission errors, a slow first paint), and the same
  objections apply here. Loading is an explicit `r`, limited to the handful
  of sources the checks read. A denied load is reported once and its checks
  stay "not checked".

## Consequences

- **Rule packs can't fetch.** #208's evaluator gets the export snapshot JSON
  and nothing else. It must never gain a "call this API" primitive. That's
  what makes a downloaded pack safe to run: the worst a pack can do is
  produce wrong findings.
- **Coverage is part of the result.** "Not checked" rows sit in the list
  beside the findings, and the headless JSON carries them as `not_checked`. A
  permission gap or an unvisited service must never read as a clean audit.
- **`@audit` is never cached** and has no client, so it isn't in
  `build_services`. `ServiceType::is_virtual()` marks it, and the paths that
  assume a client skip it: registration, the demo smoke test, watch mode.
- **Severity is the state.** High / medium / low map onto the list's `F`
  filter and `z` state sort, so the existing list features work without
  audit-specific keys.
