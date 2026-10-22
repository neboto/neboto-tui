# `@changes` is a regular service that fetches, not a virtual one

`@changes` (#241) answers "what changed just before this started?" for the
whole account, where the `W` timeline answers it for one resource. Unlike
`@audit` (ADR 0006) and `@all` (ADR 0004), it is **not** a view over warm
caches. It's an ordinary `ServiceType` with its own clients, registered in
`build_services`. Its list load queries the change sources directly over a
`[`/`]` window, and the result is cached per window like any other list.

## Considered options

- **A regular service that fetches (chosen).**
  - None of the sources has a list neboto already holds. CloudTrail write
    events, Config's configuration items, Auto Scaling activities and RDS
    events aren't rows in any other service's cache, and a warm `@cfn` or
    `@codepipeline` list holds stacks and pipelines, not their recent events.
  - As a normal service it gets the existing machinery for free: streaming
    batches, load warnings, the variant cache, watch mode, headless `ls`,
    and the read-only guard (ADR 0003), which already audits every SDK call
    in a service file.
- **A virtual service over warm caches, like `@audit`.** This would cost
  nothing, but it would almost always be empty. The question comes up
  mid-incident, when you haven't visited the sources, and the sources that
  matter most (CloudTrail, Config) have no warm list to read. Rejected.
- **Warm caches, plus a fetch for CloudTrail only.** A hybrid: what each row
  means would depend on which services you'd visited, and coverage would
  need the `–` bookkeeping `@all` has. One fetch path per source is simpler
  and gives the same answer every time.
- **Extend `W` to "every resource".** `W` merges sources filtered by one
  resource's identifiers. Account-wide, those filters disappear and the cost
  becomes unbounded, so the result needs its own caps and its own view.

## Consequences

- **Every expensive source is capped, and a cap hit is a warning.**
  CloudTrail `LookupEvents` is 2 TPS × 50 events and shared with every other
  caller, so the regional scan reads at most `CT_PAGES` pages newest first,
  with `ReadOnly=false` as its one server-side filter. When the cap is hit,
  the load warns how far back it got, so a truncated feed never looks
  complete. The other sources each have a limit too: the most recently
  touched stacks, runs per pipeline, a pipeline count, and pages of
  CodeBuild builds.
- **IAM and Organizations are looked up in us-east-1, separately.** Global
  services are recorded only in us-east-1's trail, so from any other region
  the regional scan never sees a policy change, and even in us-east-1 a busy
  hour pushes them past its cap. They get their own `EventSource` lookups
  with their own cap, and the regional scan skips those sources so nothing
  shows twice.
- **A failing source is a warning, never the load.** Sources run
  concurrently and stream when done. A denied or unconfigured source (no
  Config recorder) degrades on its own, following the warn-vs-silent rule in
  `CLAUDE.md`.
- **Noise is classification, not a filter.** Events an AWS service made on
  your behalf, and session churn (`AssumeRole*`, `ConsoleLogin`), are
  `is_noise()`: shown by default, hidden by `a`. IAM rows are never noise,
  because a policy change made by a service still changes access.
- **Opening it costs real API calls.** Each load or `r` spends up to
  `CT_PAGES` CloudTrail calls of the account's budget. The 5-minute cache
  per window keeps casual revisits free. A watch-mode refresh is a full load,
  so it spends the same budget on every interval.
- **New sources are new fetches in `changes.rs`.** Don't route them through
  another service's cache. A source whose events already appear as
  CloudTrail writes waits until it adds something the trail doesn't have.
  ECS is the example: the `UpdateService` call was already there, and the
  ECS source (#257) earned its fan-out with what the trail can't see, the
  rollout's progress and circuit-breaker rollbacks.
