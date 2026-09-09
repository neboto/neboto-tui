# BACKLOG.md

Open work only. Consolidated **2026-07-30** from `ROADMAP.md`,
`COMPREHENSIVE-REVIEW.md`, `docs/service-gap-analysis.md`, `ideas.md` and
`docs/service-plans/` — all deleted in the same commit, because everything in
them that was still true was either **shipped** (and therefore documented in
`CLAUDE.md`, which is the reference) or **carried forward here**. Git history
has the originals if you want the archaeology.

Items were verified against the tree at the time of consolidation, so an entry
being here means it was genuinely not built — not merely unmarked in an old
doc. Nothing here is scheduled; pick by value.

## Ground rules (every item)

From `CLAUDE.md`, non-negotiable:

- **Read-only.** No mutating AWS calls, ever. A "query" that creates an
  execution but touches no user data (Logs Insights `StartQuery`,
  `LookupEvents`) is still fine — document it in `PERMISSIONS.md` the way SSM
  sessions are.
- Update `PERMISSIONS.md` (IAM actions) + `CLAUDE.md` (lean per-service notes)
  in the same commit.
- `cargo test` + `cargo clippy` clean (no *new* warnings) before commit.
- Every new split pane needs a mock in `src/harness_tests/mocks_*.rs`, or the
  wiring harness can't see it.
- Register new services in `App::build_services()` — the both-path gotcha.
- Pagination via `crate::aws::pagination::next_page_token`; errors via
  `sdk_error_message`.
- Verify SDK accessor names against the crate source (grep
  `~/.cargo/registry`) — field names frequently differ from the API docs.
- Commit to `main` and push. The user tests on another machine — never run the
  TUI or an emulator locally.

---

## 1. Platform features

Each is a real design item, not a gap-fill. The change timeline and ownership
lens (added 2026-08-31) are the current picks — they lean on neboto's unique
position (warm caches + jump machinery + terminal speed) for live incident
triage, which no Console tab or CSPM covers.

### Change timeline + ownership lens — shipped 2026-08-31

Both shipped (see the CLAUDE.md "Change timeline" bullet and "Ownership
ribbon" paragraph): `W` merges CloudTrail / alarm history / owning-stack
CFN events / ECS deployment rows into one badged, time-sorted view with an
`f` source filter and the ownership line in its header; the ownership
resolver (`src/ownership.rs`) also drives the detail-pane ribbon, the
stack-name tag jump, and config `owner_tags`. Zero new IAM either way.
Still open:

- ~~**Ownership v2 — the reverse index**~~ — subsumed by the **referenced-by
  lens** (`U`, shipped 2026-09-01, `src/references.rs`): `U` on a CFN stack
  lists every loaded resource carrying its name (via the stack-name tag),
  and the same walk answers "what uses this SG / role / key / subnet". The
  sibling **network-access lens** (`N`, `access_lens.rs`) shipped with it —
  the merged security-group rule table for any SG-bearing resource. Open
  follow-ups for both: a "zombie" filter on `U` (loaded resources with
  *zero* references — unattached SGs/volumes/EIPs), and per-type
  `security_group_ids` overrides as new SG-bearing types land.
- **Timeline: more DEP-shaped sources by demand** — Lambda version publishes,
  ASG scaling activities (`DescribeScalingActivities`), RDS events
  (`DescribeEvents`). Each is one more `TimelineSource` arm + a fetch in
  `trigger_trail_lens_load`.

### Security posture sweep (`@audit`) — shelved 2026-08-31

Deprioritized, not deleted: CSPMs (Security Hub, Prowler, Wiz…) own this
space and neboto already *displays* SH/GD/Inspector findings, so the
standalone sweep duplicates its own tabs. The two useful remnants were
folded into the items above: the explicit-partial-coverage pseudo-row
(timeline's cold-cache annotation) and cache-walk synthesis (ownership v2).
Original design kept below in case demand materializes.

One synthesized view of every public exposure: `0.0.0.0/0` (+ `::/0`) SG
ingress, public buckets, public RDS instances, public snapshots/AMIs,
wide-open resource policies.

**v1 is cache-only synthesis — zero new API calls.** A virtual service
(`src/aws/services/audit.rs`, no AWS client of its own) whose
`list_resources` walks other services' cache entries and emits findings:
severity, service, resource id (jumpable via the standard machinery), reason
("ingress 0.0.0.0/0 on :22"). `is_noise()` → low severity. For a service that
isn't cached yet, emit a dim "load @ec2 to check security groups" pseudo-row
so coverage is explicit rather than silently partial.

v2 could add checks that do call APIs (bucket ACLs, `iam:GetAccountSummary`
password policy, credential report — note `iam:GenerateCredentialReport` is a
**mutation**; use `GetCredentialReport` only and degrade gracefully).

### Logs Insights query runner

On a log group, a key (`Q`?) opens a query pane: pick a canned query (errors
over time, top messages, latency percentiles from JSON logs) or edit raw
query text, run, results in the pane.

`log_tail.rs` already has the extension point — tail and search share the pane
via `LogPaneMode`, so this is a third mode. `StartQuery` → poll
`GetQueryResults` ~1s until `Complete` (statuses Scheduled / Running /
Complete / Failed / Cancelled / Timeout), reusing the generation-guard from the
tail poll loop. Time range from the existing `⇥`/`T` presets. Results are
columnar (`Vec<Vec<ResultField>>`) — render as a fixed-width table of plain
content lines; `y` copies, `e` opens in `$EDITOR`.

**IAM**: `logs:StartQuery`, `logs:GetQueryResults`, `logs:StopQuery` (fired if
the pane closes mid-query — cleanup, non-destructive, call it out).

### Resource diff on refresh

When a resource is refreshed (`r` in the detail pane, or a watch-mode detail
refresh), show what changed — SG rule added, env var changed, task-def image
bumped.

Keep the **old** resource's `detail_sections_snapshot()` (the export walker
already produces a stable `Vec<(section, Vec<(k, v)>)>` — reuse it, don't
invent a second serialization). Diff per section by key: added / removed /
changed. If non-empty, flash "3 rows changed — D to view" and render a
transient diff view (green `+`, red `−`, yellow `~`). Exactly one previous
snapshot per resource id, cleared on service switch — this is a
"what just changed" tool, not a history DB.

Works standalone with `r`; watch mode makes it shine.

### Multi-region fan-out list

A toggle that lists the current service across **all enabled regions** with a
region column.

`ec2:DescribeRegions` once, cached → the enabled-region list. Fan out the
service's `list_resources_streaming` per region with per-region clients,
streaming results in tagged with region; cache under the existing per-region
keys so single-region views stay warm. Rows show a region prefix; drilling
sets `current_region` to the row's region first (the `pending_jump_region`
machinery already handles switch-then-resolve).

Scope v1 to a whitelist of cheap-to-list services (EC2, RDS, Lambda, EKS,
DynamoDB); Cost and IAM are global anyway. **This is the largest item in this
file** — do a design pass on cache keying and `App` state before writing code.
Also answers the gap-analysis's "all unhealthy target groups across every
region".

---

## 2. New services

**Next up** (each ≈ the 9-step `CLAUDE.md` recipe, one commit):

| Service | Crate | v1 scope |
|---|---|---|
| X-Ray (`@xray`) | `aws-sdk-xray` | Service-map nodes from `GetServiceGraph` as the list (name, type, request count, error/fault %, p50/p90; faults → red) + a trace-search sub-tab (`GetTraceSummaries`, drill → `BatchGetTraces`, raw JSON on `e`). Timebox v1 to list + summaries + JSON drill; the indented segment-tree renderer is v2. The most custom-UI item here. |
| Batch (`@batch`) | `aws-sdk-batch` | Sub-tabs Job Queues / Compute Environments / Jobs. Jobs need a queue + status filter (`f` cycles status, the ECS-task precedent; listed per selected queue, the S3 bucket→objects drill). Job pane: status reason, container detail (image, vCPU/mem, exit code), attempts, timestamps. `t` tails `/aws/batch/job`. Pairs well with watch mode. |
| App Runner | `aws-sdk-apprunner` | Services, auto-scaling configs, custom domains. Container-to-URL deployments; tractable single-file addition. |

**Then, by demand:** AppSync (APIs, data sources, resolvers lazy, API keys
redacted) · Amazon MQ (brokers, configs) · Elastic Beanstalk (applications,
environments with health colour, events) · DocumentDB and Neptune (both clone
the RDS rendering approach) · Macie (findings — the GuardDuty pane shape,
jobs, bucket summary) · DataSync (tasks, locations,
executions) · Storage Gateway (gateways, volumes/shares) · CW Synthetics
(canaries, last run, `m` SuccessPercent/Duration) · EMR (clusters, steps) ·
Lake Formation · MWAA · IPAM (VPC pools/scopes/allocations, plus EC2's
BYOIP) · VPC Lattice · PrivateLink provider side (endpoint services this
account publishes) · Detective · Audit Manager · Incident Manager · License
Manager · IoT Core · Lightsail · Snow Family · DRS · QuickSight · Kendra.

**Niche, on request only:** SageMaker (a **thin slice** — endpoints, training
jobs, notebooks; the full surface is enormous and full parity is not the
goal), Timestream, MemoryDB, Keyspaces, Directory Service, ACM PCA,
Well-Architected, Comprehend/Rekognition/Textract/Transcribe/Polly/Translate.

**Do not build:** App Mesh (EOL 2026), Pinpoint (EOL Oct 2026), QLDB (EOL),
Proton, Cloud9, Data Pipeline, Clean Rooms, CodeCatalyst. Filter this whole
section by actual user base, not Console parity.

---

### Bedrock AgentCore — what's still open

`@agentcore` ships 10 sub-tabs (Runtimes / Gateways / Memory / Identity /
Tools / Policy / Evaluation / Registry / Harness / Payments), plus
configuration bundles grouped into Runtimes. **The digit keys are exhausted**:
`1`–`9` and `0` are all taken, so the next family either groups into an
existing tab or displaces one. Every family the SDK exposes is now listed;
what remains is depth on the shipped surface. This is a preview surface whose
SDK shape still moves between minor versions — check
`aws-sdk-bedrockagentcorecontrol` before starting.

- **Four policy ops look like gaps and are NOT** — checked field by field.
  `PolicySummary` / `PolicyEngineSummary` (from `ListPolicySummaries` /
  `ListPolicyEngineSummaries`) are strict subsets of what `ListPolicies` /
  `ListPolicyEngines` already return. `GetPolicyGeneration` returns exactly
  the `PolicyGeneration` that `ListPolicyGenerations` already hands us, and
  `GetPolicyGenerationSummary` returns that minus `statusReasons` — so the
  Generations section needs no per-row deepening. Don't re-add any of them.
- **Flat types that have a `Get*` behind them**: `GetBrowserProfile`,
  `GetEvaluator`, `GetDataset`, `GetOnlineEvaluationConfig`,
  `GetRegistryRecord`, `GetGatewayRule`, `GetConfigurationBundleVersion`
  (per-version components, vs the live version the pane shows today). Each
  would deepen a currently-flat surface; none is urgent.
- **`e` overrides worth adding**: a harness's system prompt (`.txt` — it is
  free text and often long) and a configuration bundle's component JSON. Both
  currently fall through to the default section-snapshot export, which works
  but buries the artifact.
- **Console deep links.** `console_url()` now returns the AgentCore console
  home, regionalized, for all nineteen types — but only the home. AWS
  publishes exactly one console URL for the service
  (`console.aws.amazon.com/bedrock-agentcore/home#`) and documents navigation
  as left-nav clicks; no per-section fragment exists in any doc, blog or
  toolkit source. To upgrade: open the console, click into each family, and
  copy the URL fragment from the address bar — then `console_home()` in
  `agentcore.rs` grows a per-family variant. Ten minutes with a browser, and
  not something to guess: a fragment that silently lands on the wrong family
  is worse than the generic link.
- **A `MemoryLocation` macro step** for the memory browser, following the
  `S3Object` collapse precedent — the browser's cursor indexes a filtered,
  paginated view, so a keystroke replay lands somewhere arbitrary. Until then
  a macro simply doesn't reproduce a memory-browser drill.
- **Deeper Evaluation panes.** Evaluators/datasets are flat; `GetEvaluator`
  returns an `EvaluatorConfig` union (code-based Lambda vs LLM-as-a-judge with
  instructions + rating scale), and datasets have `ListDatasetVersions` /
  `ListDatasetExamples` behind them. Worth a pane once the shapes settle.
- **Harness metrics and log tail.** Neither was wired: the `AWS/Bedrock-AgentCore`
  metric *names* for harnesses aren't documented (the ARN-token `SEARCH` form
  is schema-free about dimensions but still needs a metric name), and the
  harness log-group path was never verified the way the runtime's
  `/aws/bedrock-agentcore/runtimes/<id>-<endpoint>` was. Both are a
  `list-metrics` / `describe-log-groups` call against a live account away —
  don't guess either one.
- **Payment connectors as first-class rows.** They're a section on the
  manager's pane because `ListPaymentConnectors` is scoped by
  `paymentManagerId`. If AWS ever adds an account-wide listing they could earn
  their own type — but **not** the instrument APIs, which stay off-limits
  (see the Payments notes in `docs/SERVICES.md`).

## 3. Per-service depth

Verified missing, roughly by value.

- **CloudFormation: stack policy** (`GetStackPolicy`) — which resources are
  update-protected.
- **CloudFormation: StackSet operation results** (`ListStackSetOperationResults`)
  — per-account/region outcome of an operation; the actual question when the
  Operations section shows FAILED. Fits the row-keyed lazy pattern IAM
  permissions uses.
- **CloudFormation: `CallAs=DELEGATED_ADMIN`** — in a delegated-admin account,
  service-managed StackSets list empty without it. Niche; would need a toggle
  like RAM's SELF/OTHER.
- **Bedrock tabs 6–9.** Prompts / Flows / Custom Models / Imported Models are
  flat `List*` summaries (the app's remaining flat-fallback debt). Only
  **Custom Models** clearly earns a split pane (`GetCustomModel` →
  hyperparameters + training/validation metrics); leave the other three flat.
- **Cost: anomalies sub-tab** (`ce:GetAnomalies`, last 90d) — nothing shared
  with the Optimizer plumbing, so it's a standalone small item.
- **Cost: cost-allocation tags and cost categories** as group-by dimensions.
  Two features power users rely on heavily; the service groups by
  service/account/region/usage only.
- **Lambda Layers** — listed / versioned / used-by (currently just a field
  list on the function struct).
- **Route 53: `TestDNSAnswer`** as an `x`-gated action on a record (the
  Records tab landed 2026-09-03, so records are now selectable) — what Route
  53 answers for a resolver IP / EDNS client subnet, the way to debug geo,
  latency and weighted sets. Read-only (`route53:TestDNSAnswer`).
- **Route 53: `GetHealthCheckLastFailureReason`** in the health check's
  Status section — a check that is healthy now but flapped an hour ago.
- **Route 53: traffic policies, reusable delegation sets, CIDR collections**
  — all listable, all rare. Skip unless someone uses them.
- **CognitoUser split pane** — only worth it with a lazy `AdminGetUser` for
  full attributes + groups. Deliberately skipped in the U13 batch on that
  basis.
- **EcrImage as a first-class resource** — images currently render inside the
  ECR repo pane and aren't selectable; making them first-class is the
  prerequisite for an image split pane.
- **Glue tags** — no Tags section anywhere in the service; would need an N+1
  `GetTags` per resource.
- **SES reputation rates** — the bounce/complaint *rate* metrics from the
  reputation dashboard.
- **MSK `ListNodes`** — per-broker ENI/IP enumeration. Deferred because the
  section list doesn't need it.
- **Firehose** — CDC/database-source config detail and VPC-config internals
  (rare, low read value).
- **CodeArtifact** — repo upstreams / external connections need
  `DescribeRepository` (outside the v1 IAM set); applications with zero
  deployment groups don't appear in Deployments.
- **Identity Center** — application auth methods / access scopes / grants
  (`ListApplicationAuthenticationMethods`; niche until trusted identity
  propagation is in heavier use) and permission-set provisioning status
  history (`ListPermissionSetProvisioningStatus`; low signal unless a
  provision op is failing).
- **Smaller per-service P1s**, none individually worth a line elsewhere: EC2
  instance tenancy / CPU options / IMDSv2 metadata options / t-family credit
  balance, and Spot requests + placement groups + capacity reservations +
  dedicated hosts + Instance Connect endpoints as sub-tabs; EBS
  `delete_on_termination` + fast-snapshot-restore; SG owner id + per-rule
  prefix-list expansion; VPC traffic mirroring and Network Manager; ASG
  termination policies / lifecycle hooks / warm pool / suspended-process
  detail; ELB target-group stickiness + deregistration delay promoted out of
  the raw attributes blob, LB deletion-protection + cross-zone + WAF
  association, and Classic LBs if anyone still runs them; ACM renewal summary
  + CA name; RDS read-replica & Multi-AZ standby detail, RDS Proxy,
  blue/green deployments, reserved instances; S3 access points, Storage Lens,
  intelligent-tiering configs, requester-pays; SQS access policy +
  age-of-oldest-message, SNS per-subscription filter policy / DLQ /
  raw-message-delivery; Secrets replication status + version stages, SSM
  parameter split pane with policies/expiration; IAM Access Advisor
  last-accessed data, credential report (note `iam:GenerateCredentialReport`
  is a mutation — a read-only-footprint tension), instance profile +
  server certificate listings; Organizations account creation status + pending
  handshakes; CloudWatch metric streams, anomaly detection, contributor
  insights, Application Signals; ECS capacity providers + Service Connect;
  the stubbed per-instance cost estimate (`cost: None` is wired but never
  filled).

---

## 4. UX & polish

- **Tag-filter hint** — `tag:key=value` works, is unit-tested, and is listed in
  the help overlay, but nothing surfaces it where you'd actually discover it.
  A hint in the search-bar placeholder or the filtered-empty state
  (`try tag:env=prod`) is near-free and unlocks a feature that already exists.
- **Resource count badges** — the service tab bar shows names but not how
  many resources each holds. A badge (`EC2 (47)`) off the cached `len()`
  helps orientation. Careful with the tab bar's degrade-and-window logic.
- **Favourites / pinned services, persisted** — the tab bar shows
  session-visited services; there's no star-to-pin. Same for a
  recently-used list.
- **Breadcrumb trail** — after a cross-service `Enter` jump there's no visual
  trail of where you came from. The `` ` `` jump list exists but is a modal;
  a persistent status-bar breadcrumb would be discoverable. Relatedly, a
  visible `< Back` hint in the detail pane header, since new users get stuck
  there even though `h`/`←`/`⌫`/`Ctrl-O` all work.
- **Inline JSON view** — `e` hands the terminal to `$EDITOR`, a full context
  switch. A syntax-highlighted pretty-print pane would keep you in the TUI.
- **Custom CloudWatch metric query builder** — you can only view the
  predefined per-service metric sets. The CW Metrics sub-tab is a list, not a
  query builder over arbitrary namespace + dimension combinations. Related:
  alarm state-change markers on the metric timeline, and a custom start/end
  picker instead of only `[`/`]` presets.
- **Multi-resource metric dashboard** — 4–6 resources' key metrics at once
  for incident triage; today it's one resource at a time.
- **Line-wrap toggle in the detail pane** — long ARNs, policy documents and
  URLs overflow; the full value is reachable only via `y` or `e`.
- **Resource comparison** — two EC2 instances side by side, the Console's
  compare feature. No analogue.
- **Export reach** — export writes local files only. Piping to stdout,
  straight to the clipboard, or field-selecting (jq-style) on the way out
  would widen the audience. Terraform import command generation is the same
  shape as the shipped `C` CLI-command copy.
- **ASCII-only mode** — the UI leans on `●→⟳▸‹›`; terminals without good
  Unicode coverage render boxes.
- **Search history / saved searches** — no MRU across sessions, no persisted
  named queries.
- **Per-service default views** — can't say "always start ECS on Tasks".
- **Column customization** — list columns are fixed per service. The U16
  aligned-column layout deliberately deferred per-type extra columns (EC2
  instance type, Lambda runtime, …) because it needs a new trait method
  across ~100 types.
- **Throttling / backoff posture** — the real failure mode in large accounts
  is API rate limits from N+1 describes across concurrently-loading services,
  not just slowness. There's no visible retry/backoff strategy or per-service
  concurrency cap.
- **Load prioritization** — a 5000-instance account loads every service's
  list independently with no "what's visible first" heuristic, and the list is
  empty (spinner only) while it does.
- **Global alarm count** in the status bar — active alarms are visible only
  by navigating to CloudWatch.
- **Topology view** — the Console's VPC resource map / ECS service topology
  have no analogue. Even a text dependency tree ("which SGs, ENIs and target
  groups reference this instance") would add value.
- **Progressive onboarding** — `?` is comprehensive but dense; contextual
  tips ("press `m` here for metrics") would beat it for a first-timer.
- **Key rebinding** — vim defaults are good but not universal. Explicitly
  low priority: expensive relative to value.
- **Screen-reader accessibility** — absent, as in most TUIs. Noted, not
  planned.
- **Flat-view edge (cosmetic)** — with a `/` body filter active a section
  header can be filtered out while deeper rows of that section still match,
  so the synced chip lags one section.

---

## 5. Security hardening

**SSM session env forwarding — harden for static-key auth.**
`aws_env_exports()` (`app.rs`) inlines every `AWS_*` var into the launched
command, which exposes a static `AWS_SECRET_ACCESS_KEY` /
`AWS_SESSION_TOKEN` via shell history (the macOS `osascript do-script`
path), process env (`ps -E`), and scrollback. **No issue for profile/SSO
auth** — only the profile *name* is forwarded, which is the common case, and
the code comment says as much.

Preferred fix: forward only non-secret vars (`AWS_PROFILE`, `AWS_REGION`,
`AWS_CONFIG_FILE`, `AWS_SHARED_CREDENTIALS_FILE`, the SSO set) and never the
three static-key vars — the static-keys user then falls back to the safe
inline path. Optional extras: space-prefix the `do-script` command
(`HISTCONTROL=ignorespace`), use `tmux -e KEY=VAL`.

---

## 6. Testing

**Emulator smoke-test harness (floci / LocalStack).** The layer-2 wiring
harness (`src/harness_tests/`) covers registration, section order, the split
renderer dispatch and a keymap smash — all offline. What's still untested is
anything that talks to an endpoint: no service's actual load path is
exercised. A seeded-emulator smoke test would be the velocity guard before
the next round of services, since a service can currently ship with a broken
fetch and a green `cargo test`.

---

## 7. Settled decisions — do not relitigate

- **Write actions stay out.** Graduated "safe" mutations (tag edit,
  start/stop, force-new-deployment) were proposed and rejected, for two
  reasons beyond philosophy: (1) the org member-account switch pins assumed
  sessions to a `ReadOnlyAccess` session policy, so write actions would
  silently fail cross-account or the guarantee would have to be weakened;
  (2) a `PERMISSIONS.md` documenting a purely read-only IAM footprint is a
  trust asset — security teams can approve neboto precisely *because* it
  cannot mutate, and one gated write action changes that conversation
  permanently. The value was captured instead by **`C`** (CLI command copy),
  which shipped: the user never reconstructs ids/ARNs by hand, at zero risk
  to the read-only identity.
- **`@all` search is scoped to warm cache entries, deliberately.** It never
  fires fetches. A true account-wide search means a background fan-out load
  (slow, throttling-prone, permission-noisy in locked-down accounts) or
  leaning on Resource Explorer where it's configured. Scoped honestly it's
  useful; scoped as "search everything" it's a tarpit.
- **Console feature parity is not the goal.** A gap counts only if a support
  engineer would realistically reach for it mid-ticket and currently cannot.
  Write/mutate operations, dashboard builders and template designers are out
  of scope and are not gaps.
