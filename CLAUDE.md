# CLAUDE.md

Guidance for Claude Code working in this repository. This file is the **lean
architecture + conventions** reference — how the app is put together and the
rules that apply everywhere. It is loaded into every session, so keep it that
way: resist adding per-service prose here.

The app ("neboto") is a read-only AWS resource browser TUI (ratatui + tokio +
the AWS SDK for Rust).

**Where else to look:**

| For | Read |
|---|---|
| A specific service's quirks, caps, and "we tried that and it broke" | [`docs/SERVICES.md`](docs/SERVICES.md) — **before editing `src/aws/services/<svc>.rs`** |
| What a service actually renders today (sub-tabs, sections, fields) | the service file itself — it is the reference, and it moves faster than any doc |
| Per-service IAM actions | [`PERMISSIONS.md`](PERMISSIONS.md) |
| The project's own vocabulary (Lazy, LazyStore, epoch, deep export…) | [`CONTEXT.md`](CONTEXT.md) |
| Why a design is the way it is | [`docs/adr/`](docs/adr) |
| What's still unbuilt | GitHub Issues: `gh issue list --label area:<platform\|new-service\|service-depth\|ux\|security\|testing>`. Rejected ideas are closed `wontfix` issues linking an ADR. Not a doc: if someone could close it, it's an issue |
| How a release is cut / how users install | [`docs/RELEASING.md`](docs/RELEASING.md) — the workflow, `install.sh` and the binstall table share the archive name |
| Adding or editing an in-pane view (`m`/`o`/`i`/`t`/`W`/`U`/`N`), a metrics kind, a tail source | [`docs/RICH-VIEWS.md`](docs/RICH-VIEWS.md) |
| Macros (`,`) and keycast (`--show-keys`) | [`docs/MACROS.md`](docs/MACROS.md) |
| Headless `neboto ls` / `get` and its output contract | [`docs/HEADLESS.md`](docs/HEADLESS.md) |
| Demo mode fixtures (`--demo`) | [`src/demo/CLAUDE.md`](src/demo/CLAUDE.md) |
| Flat detail view / detail wrap / export internals | [`docs/DETAIL-PANE.md`](docs/DETAIL-PANE.md) |
| Every config option | [`docs/CONFIG.md`](docs/CONFIG.md) |

A note on counts: descriptive numbers ("~60 types do X") rot silently here —
three of them had drifted 25–170% before anyone noticed. Prefer a grep the
reader can run over a number they have to trust.

## Commands

```bash
cargo build            # debug   (cargo build --release for release)
cargo run              # run
cargo test             # all tests; `cargo test <module>` for one
cargo clippy --all-targets -- -D warnings   # lint; CI fails on any warning (cargo check for a fast type-check)
python3 scripts/check-readonly.py   # read-only guard: every SDK op + IAM action must be read-only (CI runs it)
```

## Architecture

### Event-driven async core

Synchronous terminal events bridge to async AWS calls via `tokio::sync::mpsc`
channels (`src/event.rs`, the `Event` enum). Terminal input is polled in a
`spawn_blocking` task; AWS fetches run in spawned tasks and send result events
back. The main loop (`src/main.rs`) is: *check if a load is needed → draw →
await next event → `App::handle_event()` mutates state*. The UI never blocks on
AWS.

- **State**: all in the `App` struct (`src/app.rs`). Mutated only in
  `handle_event` / `handle_key`. Widgets are **read-only** — they render from
  `&App`, never mutate it.
- **Search** (`App::update_search`): parses `@prefix` queries
  (`src/search/query_parser.rs`), switches service if needed, then fuzzy-matches
  (`src/search/fuzzy.rs`, SkimMatcherV2) against each `Resource::search_text()`.
  The `service:` colon syntax (alt to `@service`) only triggers when the text
  before the colon is a **known** prefix (`ServiceType::from_prefix`); otherwise
  the colon is literal, so a pasted ARN (`arn:aws:…`) fuzzy-searches instead of
  erroring "invalid service prefix". `tag:key[=value]` terms are split out by
  `split_tag_filters` as **exact** (case-insensitive) tag filters; the rest of
  the query fuzzy-matches as usual. **`@all <text>`** (checked by
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

### Service / Resource traits

- **`AwsService`** (`src/aws/service.rs`): `list_resources_streaming()` (prefer
  it — streams partial batches as `ResourcesPartiallyLoaded`),
  `list_resources()`, `get_resource_details()`.
- **`Resource`** (`src/aws/resource.rs`): `search_text()`, `details()` (flat
  key-value fallback), `console_url()`, `is_noise()`, `state()`, `as_any()`
  (for downcasting), `list_summary()` (the list's dim second column — an
  instance's type, a function's runtime; default: the id). Heterogeneous
  collections are `Box<dyn Resource>`.
- **`ServiceType`** (`src/aws/service.rs`): one variant per service;
  `from_prefix()` maps `@ec2`→`EC2`; `is_global()` true for IAM / Organizations
  / Cost / CloudFront / Trusted Advisor / Global Accelerator / Health /
  Resource Explorer.

**Virtual services**: `ServiceType::is_virtual()` (today only `Audit`,
`@audit`, `src/audit.rs`) has **no client and no cache**: it's left out of
`build_services`, and `load_service_resources` short-circuits it to compute
rows in-app from the other services' warm caches (ADR 0006). Anything that
loops over `ServiceType::all()` expecting a client must skip virtual
services (the registration test, the demo smoke test and watch mode do).

**Naming gotchas** (services people conflate — don't):
- `ServiceType::Secrets` (`@secrets`, file `config.rs`) is **Secrets Manager
  only**.
- `ServiceType::Ssm` (`@ssm`, file `ssm.rs`) is **Systems Manager** (Parameter
  Store et al.) — moved out of Secrets.
- `ServiceType::Config` (`@config`, file `awsconfig.rs`) is **AWS Config**.
- `ServiceType::Bedrock` (`@bedrock`, file `bedrock.rs`) is **Bedrock** —
  models, guardrails, knowledge bases, and the older **Bedrock Agents**
  (clients `bedrock` + `bedrock-agent`, IAM `bedrock:`).
  `ServiceType::AgentCore` (`@agentcore`, file `agentcore.rs`) is **Bedrock
  AgentCore** — the agent *hosting* platform: runtimes, gateways, memory,
  workload identity, built-in tools (clients `bedrock-agentcore-control` +
  `bedrock-agentcore`, IAM `bedrock-agentcore:`). Both say "agent" and mean
  different things: a Bedrock **Agent** is a managed orchestration config; an
  AgentCore **Runtime** is your own container. `@agents` resolves to
  AgentCore. **A third thing also says "agent"**: an AgentCore **Harness** is
  a declarative loop AWS runs (model + prompt + tools + memory), i.e. the
  managed option AgentCore Runtime is not.

- `ServiceType::Beanstalk` (`@eb`, file `beanstalk.rs`) is **Elastic
  Beanstalk** — `@eb` used to alias EventBridge, which is now `@events` /
  `@eventbridge` only. One environment list holds both **Standard** (EC2)
  and **Cluster Mode** (EKS-backed) environments; `EnvironmentTier` decides
  which (`EbDeploymentType::from_tier`).

**Standalone-regional gotcha**: `ServiceType::Route53Resolver` (`@resolver`,
file `route53resolver.rs`) is **separate** from `ServiceType::Route53` (`@r53`,
hosted zones). Resolver endpoints/rules are genuinely region-scoped (so
**not** in `is_global()` — a region switch must refetch), whereas the
hosted-zones data plane is global; one `ServiceType` can't be both for cache
keying, hence the split.

### Sub-tab system

Multi-resource services use a **view enum** (e.g. `Ec2View`, keys 1–7) + a
stateless `src/ui/widgets/*_tabs.rs` widget. Keys `1`–`9` (plus `0` for a tenth
tab — VPC's DHCP Options, AgentCore's Payments) and `Tab`/`Shift-Tab`
switch the view; the view's `resource_type_filter()` decides which resource type
the list shows. A filter may be several **pipe-separated types**
(`type_filter_matches`) for a grouped tab — VPC's Routing (route tables +
prefix lists) and Gateways (IGWs + NAT GWs), AgentCore's Runtimes (runtimes +
configuration bundles) and Payments (managers + credential providers); use
this instead of an 11th key. (`h`/`l` are reserved for spatial pane movement — see Navigation
keymap.) `AppLayout::new(area, show_banner, show_sub_tabs)` adds the 1-row tab
area; `main.rs` routes to the right tab widget. Sub-tab key blocks sit in
`handle_key()` before the global binds, guarded by `current_service == X &&
!search_active`.

The tab strip stays **one row**. All standard `*_tabs.rs` widgets render via
the shared `subtab_bar::render_subtab_bar(app, area, frame, &[(key, label,
is_active)])` helper: it draws the chips + records their mouse click-regions,
and when they overflow `area.width` it **scrolls a window around the active
chip** with `‹`/`›` markers (active always visible; number-key nav is
unaffected). Widgets with a trailing chip/toggle on the same row (ECS status
filter, GD/SH/Inspector severity, WAF scope, Cost period, Backup's hint) use
`subtab_bar_spans` — same bar, but returns the spans so the caller appends its
chips and renders the combined line. Five bars are deliberately bespoke (not
tab strips): `service_tabs.rs` (top-level service strip + endpoint/profile
badges), `ram_tabs.rs` (pure `t` toggle), `s3_tabs.rs` (passive Bucket chip +
`o` Objects), `sq_tabs.rs` (scope indicator + `t`/`c` hint buttons),
`ct_tabs.rs` (event-query scope + `f` filter hint).

Variations on the pattern:
- **Single-list services** (no view enum, one resource type, split detail pane):
  KMS, RAM, WorkSpaces, Trusted Advisor, Global Accelerator,
  Resource Explorer, DynamoDB, EKS, EFS, Transfer Family,
  ElastiCache, OpenSearch, Route53 Profiles, Budgets, Invoices. Several resolve id→name maps up front so
  rows read by name (the "KMS-alias pattern" — e.g. WorkSpaces resolves bundle
  and directory maps before `DescribeWorkspaces`).
- **Toggle rows** (rebuild + variant-cache the service, not resource filters):
  Cost (`cost_group_by` keys 1–6 — 5/6 are tag/cost-category, keyed — and
  key `7` the Anomalies view, `cost_anomalies`, its own cache variant;
  `cost_period` isn't a tab: `[`/`]` step it, `t` cycles, chips on the
  detail pane's border), WAF
  (`waf_scope`), RAM (`ram_owner` SELF/OTHER via `t`), Service Quotas
  (`quota_service_code`, `c` picker), X-Ray (`xray_window`, `[`/`]` in
  the list pane), CloudTrail (`ct_query`, `f` filter
  modal). Cost's drill-down / period / forecast
  keying rules live in `docs/SERVICES.md` (**Cost drill-down & periods**).
- **S3** is single-resource (buckets) with a `Bucket | ▸ Objects` mode toggle;
  the bucket's setting views are in-pane detail sections (Overview / Security /
  Operations / Website / Advanced / Metadata / Tags). `ListBuckets` is
  global but Tier-1 details are fetched only for **current-region** buckets;
  other-region buckets appear as dimmed `S3Bucket::stub`s (name + region + `→`),
  and drilling one runs `try_s3_cross_region_drill` (region-switch + reload +
  select) instead of opening a detail-less stub.

### Split detail pane

Rich resources render a console-style split pane —
**header | rule | section tab bar | rule | scrollable body** — instead of the
flat `details()`. Most rich resource types use it — grep `sections! {` for the
current set rather than trusting a count here. Simple eager panes (the U13
batch: Vpc, NatGateway, TgwAttachment, DxLag, RdsSnapshot, FsxVolume,
GdDetector) share the `render_simple_split` skeleton in `details_pane/mod.rs`
(its header is the one-row `header_line`: dot, name, state, then the
subtitle's ` · ` facts that fit). Panes that build their own header lines
pass them through `one_row_header`, which condenses them to the same row
(`every_split_pane_has_a_one_row_header` fails a pane that doesn't); the
Vpc and DxLag panes fill their Subnets/Gateways/Connections sections by
**filtering sibling resources** already in `self.resources` (no extra
fetches). Deliberately still flat: `ElasticIp` (the flat view is complete),
`CognitoUser` (would need a lazy `AdminGetUser`).

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

**How it works (section descriptors, ADR 0002):** every split pane declares
its sections **once** with the `sections!` macro in its service file
(emitting the renderer's `*DetailSection` enum + a `SectionDescriptor`
static of labels and optional `=> App::trigger_*` on-enter hooks) and
returns it from `Resource::detail_sections()`. The app keeps **one**
`detail_section_idx` cursor (no per-type `App` fields), and everything
derives generically from the descriptor — digit keys, `Tab` cycling, reset
on focus, the snapshot walk (export/flat view/bookmarks), the tab bar
(`descriptor_tabs`), and bookmark restore — so label/order agreement holds
by construction. The render path:
1. `render_details_pane()` (`details_pane/mod.rs`) downcasts the selected resource
   and early-returns to a per-type `render_*_split()`, whose tab bar renders
   `descriptor_tabs(app, &THE_SECTIONS)`.
2. `App::get_detail_lines()` is the **single source of truth** for body content
   — it downcasts and dispatches to `*_section_lines(resource,
   Enum::from_index(self.detail_section_idx), …)`. `j`/`k`, scroll, and copy
   all index into this Vec.
3. On-enter hooks fire from `set_detail_section` (digits, `Tab` cycle, chip
   clicks, bookmark restore) and from `reset_detail_section_to_default`
   (drill-in fires section 0's hook) — hooks must be idempotent (LazyMap's
   contains-guard makes that free).

**Flat detail view** (`\`, config `detail_flat`) and **detail wrap**
(`Ctrl-W`, config `detail_wrap`) — mechanics in
[`docs/DETAIL-PANE.md`](docs/DETAIL-PANE.md). The rules: zero per-service
code — do NOT add per-type flat handling; snapshot/section code must call
`section_detail_lines()`, never `get_detail_lines()` (recursion into the flat
cache); a new body renderer must go through `layout_detail_body` and pass
`detail_footer(…)`'s string to its block, or wrap, click mapping and the
coloured footer silently don't apply to it.

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
  `⚠`/`✗`-prefixed content lines render WARNING/ERROR-coloured.
- **Blank spacer**: both empty → blank line. Spacing is evened out
  centrally (`tidy_section_rows`): none at either end of a section, none
  under a group header, never two in a row — so emit them freely between
  groups and don't hand-tune them.
- **Lazy-fetch error**: always via the `error_rows(&err)` helper (blank spacer
  + `  ⚠ {msg}`) — never hand-rolled `("Error", e)` rows. (An `Error` *group
  header* is still fine for a resource's own failure data, e.g. a FAILED
  Athena query or a Glue run error.)

**Lazy-loaded sections (canonical pattern).** Many sections aren't fetched
until first viewed. **Every lazy detail section** lives on the `LazyStore`
(`src/lazy.rs`, vocabulary in `CONTEXT.md`, design in
`docs/adr/0001-apply-closure-lazy-event.md`): a `LazyMap<T>` field on
`App.lazy`; the `trigger_*_load` fn calls the generic
`App::trigger_lazy(lens, key, tx, future)`; results come back through the
single epoch-stamped `Event::Lazy` apply-closure — **no per-fetch `Event`
variants or `*State` enums**, and no reset wiring (the whole store is
replaced on any profile/org-role/region switch, which also drops stale
in-flight fetches). Renderers take `Option<&Lazy<T>>` and must cover all
three arms — fetch failures render inline via `error_rows`, never a
status-bar error. The **only** per-fetch state still outside the store is
the `*MetricsState` maps behind the `m` overlay (one per `MetricsKind`): those
deliberately
re-fetch on every open / time-range change (violating LazyMap's
contains-guard idempotence), so they keep dedicated `*Loaded` events until
they get their own design pass.
Triggers fire from the section key, the `Tab`/`Shift-Tab` cycle, and
(sometimes) list nav. Per-row variants exist (IAM permissions / Org
SCP documents are keyed by the selected *row* via a `*_row_target` classifier).
**Two sections deliberately have no on-enter hook** — Route 53 records' Test
answer (`TestDNSAnswer`; each `x` is a fresh sample of a weighted set) and
AgentCore's runtime Agent Card, because `GetAgentCard` is a data-plane call that reaches the running
agent container and can cold-start it (billable). An auto-hook would fire that
from a `Tab` press, and the flat view's trigger sweep would fire it for every
runtime you looked at — so it's `x`-gated on the Secrets Manager precedent
instead (`sections!` makes the hook optional). Use that shape for any fetch
with a real cost; the default remains an on-enter hook.
The `Loading…` text is rewritten centrally at render time (`spin_loading_row`):
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

**Per-service notes live in [`docs/SERVICES.md`](docs/SERVICES.md).** Read the
section for a service **before editing `src/aws/services/<svc>.rs`** — it holds
the API quirks, the things that were tried and broke, the caps and why they
exist, and which row labels other code keys on. None of that is recoverable
from the code. It was split out of this file (it was 55% of it) so it loads
when you're working on a service instead of in every session.

### Rich detail-pane views (`m` / `o` / `i` / `t` / `W` / `U` / `N`)

Interactive sub-views (metrics, item/object/memory browsers, log tail + search, the `W`/`U`/`N` lenses) render in the detail pane. **Before adding or editing one — or a metrics kind, tail source or lens — read [`docs/RICH-VIEWS.md`](docs/RICH-VIEWS.md).**

### Cross-service "go to resource" jump (`Enter`)

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
section. `NavLocation.region` (the capture region; `None` for global
services and old bookmark files) makes history cross-region: picking a
region in `R` records "here" like a service switch, and restoring a
location from another region goes through the cross-region jump below
(`pending_jump_region` + `RegionSwitchRequested`), whose handler filters
and resolves on a cache hit. The footer hint, jump list and bookmark list
name the region when it isn't the current one (`other_region`).

**Cross-region jump (Resource Explorer)**: `trigger_jump` captures the selected
resource's region *before* clearing selection; if it differs from
`current_region` it sets `pending_jump_region` and sends
`Event::RegionSwitchRequested` (switch-region + reload the target service).
`resolve_pending_jump` returns early until `current_region == want`, so the
stale old-region load can't claim the jump. Resource Explorer itself
(`@explorer`, global, gated) discovers the AGGREGATOR index via `ListIndexes`
and runs `Search("*")` from that index's region.

**ECS task visibility**: the streaming load fetches running tasks **plus a
capped, most-recent-first window of recently-stopped tasks** (ECS retains them
~1h) so a failed/cycled-out task doesn't vanish before you read its stop reason
/ exit codes. The ECS **service** pane has a lazy Tasks section; the cluster-wide
Tasks sub-tab has an `f` status filter (All/Running/Stopped).
`EcsServiceInfo::state()` reflects rollout health (FAILED→red, IN_PROGRESS→yellow);
past that, an ACTIVE service at 0/0 is `ResourceState::Idle` (muted `◌`,
label `scaled to 0`, sorted next to stopped) — not a healthy green row.

### `$EDITOR`

- `editor.rs` (`e`): `main.rs` fully tears down the TUI, spawns `$EDITOR`
  (falls back to `vim`), recreates on return. **Gotcha**: async-fetched content
  bound for the editor must land *before* the flag — the fetch handler stashes
  it in `App.pending_editor_content` then sets `editor_requested`; `main.rs`
  handles `editor_requested || session_requested` in one tear-down block (SSM
  inline sessions reuse it). **Second gotcha**: `e` has **two** trigger-fetch
  chains in `handle_key` — the detail-pane one (inside the `details_focused`
  block) and the list-pane one further down. A fetch keyed off the *body
  cursor row* (e.g. RDS "Log File" rows) must go in the detail-pane chain —
  in the list chain it can never fire (this shipped once as dead code).
- **What `e` opens** (`App::open_in_editor`, fixed resolution order): ①
  async-stashed `pending_editor_content` (IAM policy docs, ECS task logs, WAF
  sampled requests, secrets, S3 objects…); ② a contextual override
  (`editor_override_content` — CFN template/events, KMS / VPC-endpoint /
  OpenSearch policies **section-gated**, EC2 console log / user data (raw text, section-gated), REST-API / S3 bucket policies, SSM doc
  body, CW dashboard JSON, buildspec, launch-template data, SFN definition, DMS task/serverless settings (section-gated)) —
  overrides **fall through** when they have nothing, never error; ③
  `Resource::raw_content()` — **raw AWS JSON only** (CloudTrail events,
  GuardDuty / Inspector / Security Hub findings); ④ the default: the full
  detail-pane snapshot as JSON (`export::detail_json` over
  `detail_sections_snapshot()` — every section incl. lazy data, same fidelity
  as the pane), so `e` always opens something. Don't add hand-picked field
  summaries to `raw_content()` (the snapshot supersedes them) and don't make an
  override error on a miss.

### Macros (`,`, `src/macros.rs`)

Recorded, replayable navigation steps. **Before touching `src/macros.rs`, the recorder/player on `App`, or `,` handling, read [`docs/MACROS.md`](docs/MACROS.md)** (it also covers Keycast, which rides the same diffing shape).

### Keycast (`--show-keys`, `src/keycast.rs`)

See [`docs/MACROS.md`](docs/MACROS.md#keycast---show-keys-srckeycastrs). A new in-pane view needs a name in `App::keycast_view`'s overlay chain.

### Caching (`src/aws/cache.rs`)

- Per-`ServiceType` entry, default 5-min TTL (`effective_ttl()` overrides —
  config `cache_ttls` per-service values first, then Cost's built-in 6h).
  Base TTL is config `cache_ttl` (seconds). Checked before every fetch in
  `load_service_resources()`.
- `r` in the **list pane** invalidates + reloads the whole service **and
  replaces the `LazyStore`** (epoch bump — `refresh_current_service`): without
  this, LazyMap's contains-guard served the first fetch of every lazy detail
  section (e.g. an ECR repo's Images) for the rest of the session. When the
  detail pane is focused it re-fires the open section's trigger before the
  reload clears the selection, so the pane refetches instead of spinning over
  the emptied store (and `flat_triggered_for` is reset so the flat view's
  sweep runs again).
- `r` in the **detail pane** = targeted refresh: `refresh_selected_resource`
  re-fetches just the selected resource and swaps it in place (by `id`, same
  position) via `Event::ResourceRefreshed`. Supported types downcast to a
  `fetch_single_*` helper (ECS service/task today; their lazy sections are
  NOT cleared on the targeted path); others fall back to full refresh
  (incl. the lazy-store replacement above). Doesn't touch the cache.
- **Watch mode** (`w`): auto-refresh of the current view every `WatchState`
  interval (default 10s; `+`/`-` presets 5–300s; config `watch`/
  `watch_interval`, CLI `--watch`). Driven by the existing 250ms `Event::Tick`
  → `watch_tick()`; held over while anything is loading / a modal, search, or
  overlay is up. List refreshes stream into `App.watch_staging` (the visible
  list **never blanks**) and swap in atomically on `ResourcesFullyLoaded`,
  selection restored by id; detail focus on an ECS service/task uses the
  targeted single-resource path with `quiet` toasts. Status bar shows a green
  `⟳ watch Ns` chip — `show_loading_indicator()` stays quiet for watch
  refreshes. `load_service_resources()` drops any in-flight staging (a manual
  load supersedes the watch pass).
- **Global services** key the cache on `Region::UsEast1` (data doesn't vary by
  region). Client region and cache region are independent.
- **A region switch keeps the cache** (it's region-keyed; only a profile/role
  switch replaces it). That's what makes the **`R` fan-out** (#92) pay off:
  opening the picker with config `regions` set lists the current service in
  each uncached configured region (`start_region_fanout`, on
  `AwsClients::for_region`, via the headless `list_all`) straight into that
  region's slot (`Event::RegionFanoutLoaded`, `region_fanout_generation` —
  bumped on profile/role switches only), and the picker shows
  `region_counts()`: rows of the active sub-tab's type per region. Skipped
  for global, virtual and **variant-cached** services — every service whose
  list depends on App state is variant-cached, and a fresh `build_services`
  would list it with the default.
- **Variant key**: `CacheKey.variant` lets one service cache several result sets
  (`App::cache_variant`): Cost `{group_by}-{period}`, WAF `{scope}`, Service
  Quotas `{service_code}`.

## Adding a new AWS service

Read one existing service of the same shape first, plus its section in
[`docs/SERVICES.md`](docs/SERVICES.md) — the shapes are established (single-list,
sub-tabs, findings, toggle-row) and the notes say which precedents to follow.
When you're done, add the new service's own entry there: the API quirks you hit
and the approaches you rejected are the part nobody can recover from your code.

1. `src/aws/services/<svc>.rs`: service struct + resource struct(s) with
   `from_sdk()`.
2. Implement `AwsService` (prefer `list_resources_streaming()`) and `Resource`.
3. Add a `ServiceType` variant + `from_prefix()` aliases + `name()`/`prefix()`.
4. Add a `*_client()` to `src/aws/client.rs` (and the SDK crate to `Cargo.toml`).
5. **Register in `App::build_services()`** — the single source of truth used by
   both `App::new` and `recreate_services`. Registering in only one path means
   the service silently never loads after a region/profile switch.
6. Multi-resource: add a view enum + `resource_type_filter()`, a `*_tabs.rs`
   widget, a sub-tab key block, and `show_sub_tabs = true`.
7. Split pane: declare the section table, starting with `Overview` (see
   "Section 1 is the Overview" above), with the `sections!` macro in the
   service file (emits the `*DetailSection` enum + a `SectionDescriptor`
   static from one list — see `src/sections.rs` and
   `docs/adr/0002-section-descriptor-table.md`), return it from
   `Resource::detail_sections()`, add the `*_section_lines` renderer (take
   the enum via `from_index(app.detail_section_idx)`) + a `render_*_split`
   that passes `descriptor_tabs(app, &THE_SECTIONS)` to the pane skeleton,
   both in `src/ui/widgets/details_pane/<svc>.rs` (a new file needs a
   `mod <svc>;` + `pub use <svc>::*;` pair in `details_pane/mod.rs`), plus
   the downcast arm in `render_details_pane`'s dispatch there.
   Digit keys, Tab cycling, reset, the snapshot walk, the tab bar, and the
   flat view all derive from the descriptor — there is no per-pane wiring in
   `app.rs` to add (the legacy per-type cycle/reset/digit chains and the
   `walk!` macro are gone). Per-section lazy fetches go on `=> App::trigger_*`
   on-enter hooks in the table (the trigger must be `pub(crate)`).
8. Lazy section: a `LazyMap<T>` field on `LazyStore` (`src/lazy.rs`) + a
   `trigger_*_load` fn calling `App::trigger_lazy` — no `Event` variant, no
   `*State` enum, no reset wiring.
   **Every new split pane also needs a mock in `src/harness_tests/mocks_*.rs`**
   so the layer-2 harness covers its section wiring (see Testing).
9. Metrics/browser/tail: follow the rich-detail-pane pattern above.

## Conventions & gotchas

- **Pagination**: ops without a fluent paginator (WAFv2, API Gateway v2,
  EventBridge, hand-rolled loops) MUST advance the token via
  `crate::aws::pagination::next_page_token(resp.next_token(), &token)` — it
  stops on absent / empty / non-advancing tokens. A bare `if token.is_none()`
  can spin forever (this hung GuardDuty). Not for DynamoDB
  `last_evaluated_key` (a map, not a string).
- **Throttling** (#115): every config gets `with_throttle_retries`
  (`client.rs`) — the SDK's **adaptive** retry mode, 5 attempts, unless the
  user set their own `retry_mode`/`max_attempts`. Adaptive's client-side rate
  limiter is process-wide per service, so it paces our per-call clients
  together after the first throttle. Don't add per-client retry overrides;
  for a known slow API (Route 53 5 TPS, CloudTrail 2 TPS, IAM bursts) lower
  the fan-out (`buffered(N)` / sequential) at the call site instead.
- **Colors** (`src/ui/theme.rs`): the palette is **runtime-selected** (config
  `theme`/`[theme_colors]`, installed once via `theme::init_palette` in
  `App::new`) and read through accessors — `theme::accent()`,
  `theme::warning()`, `theme::text_primary()`, `theme::heading()`, ….
  **Never hardcode `Color::White`/`Yellow`/`Cyan`/etc. for UI text** — that's
  what breaks the light preset; route through the palette. `Color::` literals
  are also unusable in `const` items now (palette calls aren't const) — use a
  `fn` or `let`.
- **Errors** (`src/error.rs`, `thiserror`): surface the real AWS message via
  `sdk_error_message(&e)` (`ProvideErrorMetadata`), not the generic SDK
  `Display`. Errors show in the status bar (`App.error_message`) and are always
  `y`-copyable. Every status error/toast is also recorded to the reviewable
  message history (`M`, `App.message_history` — captured centrally by
  `record_message_history` diffing the fields each loop, so direct
  `error_message = Some(…)` assignments need no setter).
- **Partial failures in multi-phase loads**: a phase failure must NOT send
  `ResourceLoadError` mid-stream — that clears `loading`, and
  `handle_resources_partially_loaded` then drops every later batch (this
  silently truncated EC2/VPC/IAM/Athena/… loads). Send
  `Event::ResourceLoadWarning { service, warning }` and keep going: warnings
  accumulate in `App.load_warnings` and surface as one
  `Partial load — …` status line when the load completes. Reserve
  `ResourceLoadError` (fatal, then `return`) for first-phase/total failures
  where nothing can stream (IAM sends it only when all four core phases fail).
- **Warn vs. stay silent on a failed phase**: warn when the call would
  normally succeed, so a failure really is unusual (Security Hub's automation
  rules work fine in a standalone account). Stay silent — and put the
  explanation in `resource_list.rs`'s per-tab empty state instead — when the
  failure is *expected* for a whole class of account or region, because a
  permanent warning on every load everywhere else is just noise (GuardDuty's
  `ListMembers` and its entity-set APIs, CloudTrail's
  `InsightNotEnabledException`, Malware Protection). That is the line between
  the two conventions; pick deliberately, don't copy whichever neighbour you
  read first. **Emulators are handled centrally**: while a custom endpoint is
  active, a warning whose text says the operation is unsupported
  (`is_emulator_unsupported_warning`) goes to the `M` history only, never the
  `Partial load —` status line — floci's missing `DescribeSnapshots` is not
  news. Don't add per-service emulator special-casing.
- **`is_noise()` is only safe when the non-noise subset is normally
  non-empty.** `a` is a global session toggle, so marking a category noise
  blanks that tab for accounts where everything is in it — Security Hub's
  insights are `is_noise()`-free for exactly this reason (most accounts have
  zero *custom* insights, so folding the AWS-managed ones away empties the
  tab). Put the distinction in `search_text()` instead — a search term can't
  hide everything. `resource_list.rs` checks `hide_noise &&
  hidden_noise_count > 0` **first** in the filtered-empty chain, so a tab
  emptied by `a` says so rather than blaming a permission gap.
- **Row labels and shapes are load-bearing when a classifier keys on them.**
  The label-keyed jump classifiers (`sh_row_jump_target`, `rds_row_jump_target`,
  `ssm_row_jump_target`, `code_row_jump_target`, `org_row_jump_target`, …) match
  on a row's **key text**, and some (`org_account_scps_row_target`) match on the
  exact `("  {name}", "{id}")` tuple shape. Renaming a row or collapsing its
  shape silently kills the jump — no compile error, no test failure. Change the
  row and its table together.
- **Flatten *every* populated field of a filter/condition, don't
  first-match-win.** A range arrives as `gte` + `lte` on one condition, so
  stopping at the first populated operator silently halves it and makes a rule
  read broader than it is (GuardDuty's `condition_summary`, Security Hub's
  automation-rule number filters). Same rule for multi-shape resource blobs:
  extract all of them, not the first that matches (a GuardDuty EKS runtime
  finding populates cluster *and* workload *and* container).
- **Fall back to deprecated field aliases.** Resources created through older
  API versions still come back on the retired names, with the current field
  empty — Security Hub's `MasterId` for `administrator_id`, GuardDuty's
  `eq`/`gte` condition aliases, insight filters' `severity_normalized` /
  `keyword`. Read the current field first, then the alias, behind
  `#[allow(deprecated)]`.
- **List-time tags for services without a cheap tag call** (#29): wrap the
  service in `aws::tags::ListTimeTags::wrap(svc, clients, &["<tagging
  prefix>"])` in `build_services` and give each taggable type a
  `Resource::list_tags_slot` (its ARN, or the ARN's resource part like
  `job/NAME` when the type has none). One `tag:GetResources` runs beside the
  list and lands as `Event::ListTagsLoaded` **between the last batch and
  `ResourcesFullyLoaded`** — that order is what lets the completion handler
  cache tagged rows, so don't forward the completion early. Bedrock,
  CloudTrail, CloudWatch and Glue use it; services that already fill tags at
  list time don't need it.
- **`search_text()` runs per resource per keystroke.** For services with
  large per-resource blobs (GuardDuty, Security Hub findings) build a
  **precomputed** `search_blob` at ingestion instead of walking the structure
  in the accessor.
- **Stale load streams (rapid service / profile switches)**: every list-load
  stream is spawned behind a generation-guarded forwarder
  (`load_resources_async` + `App.load_generation`, bumped by
  `load_service_resources` / `start_watch_refresh`, and at the *start* of
  `switch_profile` / `switch_region`, all through `supersede_loads`) — a
  superseded stream's remaining events are dropped before they reach
  `handle_event`, and its fetch task is **aborted** (`App.load_task`) so it
  stops spending the account's rate limit in the background. The forwarder also
  **tags** every event it passes as `Event::LoadStream { generation, .. }`
  and `handle_event` re-checks the generation **when the event is handled**:
  while `handle_event` is blocked in a profile/region/role switch (the awaited
  client build), the old stream's events pass the forwarder and queue behind
  the switch, then land on the new credentials' empty list — a queued
  FullyLoaded cached them and cleared `loading`, so the new load's batches
  were dropped and `r` could never shake the old account's rows
  (`harness_tests/load_stream_test.rs`). The handlers are also hardened: a
  mismatched-service `FullyLoaded`/`LoadError` never clears the loading flags
  (that dropped the new load's batches — the fast-switch "empty until `r`"
  bug) and `handle_resources_fully_loaded` only caches when the event's
  service is still current (else it stamped the *other* service's on-screen
  rows into this service's cache slot). Keep both layers: don't restore an
  unconditional `loading = false` on the mismatch paths.
- **Clipboard**: `App.clipboard` is kept alive for the process — dropping the
  handle clears the selection on Linux. Reuse it via `copy_to_clipboard`.
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
- **Secrets**: the Secrets service surfaces metadata only; values are fetched
  only on `x` (reveal) / `Y` (copy) and are **never** stored in `App` or echoed
  into a message/log/row. Two extensions of the same line: where an API hands
  back a **reference** to a credential (a Secrets Manager ARN — AgentCore
  harness model keys, payment vendor credentials), show the reference and
  never resolve it; and AgentCore **Payments** has no reveal at all —
  `GetPaymentInstrument` / `GetPaymentInstrumentBalance` /
  `GetResourcePaymentToken` return spendable material and are never called.
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
  first row when pressed in the detail pane; gated by `has_sub_tabs`), `/` (search), `@` (search seeded with `@`, for a fast
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
  (nav-back through history, same as the list pane; the footer names the
  targets as clickable `⌫ back to …` / `⌃F forward to …`), `y`/`e`,
  `m`/`o`/`i`/`t` (rich views), `W`/`U`/`N` (timeline / referenced-by /
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
  (`m`/`t`/`o`/`i`/`W`/`U`/`N`) go through `handle_pane_view_mouse`: wheel =
  `↓`/`↑` over the view, click moves the cursor, double-click = `⏎` (one
  click never acts, as in the list), right-click = `Esc`, anything else is
  swallowed. Their renderers record the view rect + visible rows into the
  same `popup_hits`; a new in-pane view needs `open_pane_view` /
  `select_pane_view_row` arms. Metrics takes only the right-click. The README's Mouse
  table and the `?` help's Mouse section list the gestures; keep them in
  step. `mouse_smash_never_panics` clicks every recorded region over every
  mock.
  **Double-click** (second left press on the same cell within 400ms, tracked by
  `App.last_left_click`) replays `Enter` — drill into detail from the list,
  follow a jump link in the detail body. **Right-click** = nav-back
  (`nav_back`, mirrors `Ctrl-O`/`h`).

## Key files

- **`src/app.rs`**: `App` state, `handle_event`/`handle_key`, view/section enums,
  every `trigger_*_load`, `build_services`.
- **`src/main.rs`**: entry, main loop, tab-widget routing, tear-down for
  `$EDITOR`/SSM, overlay render dispatch.
- **`src/event.rs`**: the `Event` enum.
- **`src/commands.rs`**: `COMMANDS`, the key table the `?` help draws
  from. A row's optional `Gate` is an `App::supports_*` check plus the
  status-bar hint it shows, so the help's "Here" section and the bar's
  conditional hints come from one place. A new `supports_*` gate needs a
  row (a test greps for it); a new key needs a row or it isn't in `?`.
  The `:` picker (`command_picker.rs`) lists the rows that can run in the
  focused pane: a gated row runs its hint key, others need `.runs(…)`.
  `⏎` replays the key through the event channel, never a direct call.
- **`src/lazy.rs`**: `Lazy<T>`/`LazyMap<T>`/`LazyStore` — the deep module
  behind migrated lazy sections (see the canonical-pattern paragraph above).
- **`src/macros.rs`**: `MacroStep`/`Macro`/`MacroRecorder`/`MacroPlayer` +
  JSON load/save. The recorder/player logic lives on `App`
  (`macro_tick` and friends); the picker is
  `src/ui/widgets/macro_picker.rs`.
- **`src/aws/{service,resource,client,cache,region,pagination}.rs`**: the traits,
  SDK client factory, cache, region enum, pagination helper.
- **`src/aws/services/*.rs`**: one file per service — resource struct(s)
  (`from_sdk`), `AwsService`/`Resource` impls, and that service's `fetch_*`
  helpers + lazy `*State` types. **Read the file for a service's specifics**,
  and [`docs/SERVICES.md`](docs/SERVICES.md) for the quirks the file can't tell
  you.
- **`src/ui/widgets/details_pane/`**: the split-pane renderers, one file
  per service named like `src/aws/services/<svc>.rs` (its `render_*_split`
  + `*_section_lines`). `mod.rs` holds what they share: the
  `render_details_pane` dispatch, the split skeletons, `style_detail_row` /
  `key_col_widths` / `error_rows` / `tag_rows`, `layout_detail_body`,
  `detail_footer`, and `resource_jump_target` with its classifiers. Children
  reach `mod.rs` and each other through `use super::*`, and `mod.rs`
  re-exports them (`pub use <svc>::*`), so `details_pane::foo` paths
  elsewhere don't care which file `foo` is in. A helper one service uses
  lives in that service's file; one several use goes in `mod.rs`.
- **`src/ui/widgets/`**: `metrics_overlay.rs`, `s3_object_browser.rs`,
  `ddb_item_browser.rs`, `memory_browser.rs`, `log_tail.rs` (rich views);
  `*_tabs.rs` (one per multi-resource service); the `*_selector.rs` modals
  (`S`/`R`/`P`/`c` pickers); `splash.rs` (welcome screen).
- **`src/export.rs`**: `X` (detail) / `Ctrl-X` (list) export via
  `detail_sections_snapshot()`, plus `detail_json()` behind the default `e`
  view. Waiting/loading rules and where files land:
  [`docs/DETAIL-PANE.md`](docs/DETAIL-PANE.md#export).
- **`src/config.rs`**: TOML `Config` (`$NEBOTO_CONFIG` → XDG → `~/.neboto.toml`);
  every option is listed in [`docs/CONFIG.md`](docs/CONFIG.md). Bad values
  warn at startup, never fatal. `controltower_audit_account` must be
  **quoted** — a bare 12-digit id is a TOML integer and the whole file then
  fails to parse. `update_check` is the **only non-AWS request**: silent on
  failure, never in tests / headless / `--demo` / CI. No `default_service` ⇒
  welcome splash, nothing loads.
- **`src/cli.rs`**: clap `Cli` (parsed in `main` before the runtime).
  `-s/--service`, `-r/--region`, `-p/--profile`, `--endpoint-url`,
  `--banner`/`--no-banner`, `-w/--watch`, `--theme`, `--show-keys`,
  `--no-update-check` — each **overrides the config file**
  for the run via
  `Cli::apply_to(&mut Config)`, applied in `App::new(cli)` after `Config::load()`.
  `-m/--macro NAME` is the exception: it names an action, not a setting, so it
  bypasses `apply_to` and is applied by `App::arm_startup_macro` after the app
  is built (see Macros).
- **`src/html.rs`**: `html_to_text()` — flattens AWS HTML blobs to readable plain
  text. Applied at ingestion for Trusted Advisor check descriptions; reusable for
  any HTML-bearing field.
- **`src/aws/document.rs`**: `document_to_json` / `document_display` /
  `document_pretty` — `aws_smithy_types::Document` → JSON. The smithy crates
  ship no such converter and several services hand back untyped Documents
  (Control Tower parameters + landing-zone manifest, AgentCore agent cards and
  memory event blobs).

### Headless subcommands (`neboto ls` / `get`, `src/headless.rs`)

`neboto services` / `ls` / `get` print instead of starting the TUI. **Before touching `src/headless.rs`, `src/headless/`, or `skills/neboto/SKILL.md`, read [`docs/HEADLESS.md`](docs/HEADLESS.md)** — the output contract is pinned and section/type renames are breaking changes.

### Local emulator (floci / LocalStack)

Point every client at a local emulator via `endpoint_url` in config or the
`AWS_ENDPOINT_URL` env var (env wins; resolved by `AwsClients::resolve_endpoint`).
When active, `AwsClients::build` injects dummy `test`/`test` creds, `s3_client()`
forces path-style, and the tab bar shows a `⚙ host:port` badge. Seed bulk data
with `COUNT=200 ./scripts/seed-floci.sh`, then
`AWS_ENDPOINT_URL=http://localhost:4566 cargo run`.

### Demo mode (`--demo`, `src/demo/`)

Offline made-up account answered from fixtures at the HTTP layer — **covering a new service or section means adding fixtures, never demo branches in app code.** Details: [`src/demo/CLAUDE.md`](src/demo/CLAUDE.md) (loads automatically when working in `src/demo/`).

## Testing

Unit tests live next to the code (query parser, fuzzy matcher, Cost date math).
Mock `Resource`s must include all trait fields (notably `tags`).

**Layer-2 wiring harness** (`src/harness_tests/`, plain `cargo test` — no
emulator, no network): builds the app offline (`App::new_for_test` — default
config + clients pinned to a dead endpoint) and injects mock resources
directly. Three tests: every `ServiceType` registered in `build_services`
before *and* after `recreate_services`; for every split-pane type, digit keys
== `Tab` cycle == snapshot order and the post-focus default is the first
section (the invariant that once broke 15 panes — now held by construction
via the descriptor, the test guards regressions); and a full-keymap smash
rendered frame-by-frame to a `TestBackend` via `render_app()` (the factored
draw body in `main.rs`). Mocks live in `mocks_*.rs` batch files, one
`(ServiceType, label, Box<dyn Resource>)` entry per type, built with
`from_sdk` on minimal SDK-builder values; the registry has a size-floor
assert so lost coverage fails loudly. **A new split pane needs a mock entry
or it's invisible to the harness.**

A fourth test, `every_split_pane_type_reaches_a_split_renderer`, renders
each mock that declares `detail_sections()` and requires every section
label on screen. `render_details_pane`'s dispatch is a hand-written
downcast chain, so a type with a descriptor but **no arm** falls through
to the generic flat `details()` view — descriptor, section renderer and
lazy hooks all present and none of them ever reached. Nothing panics and
the section-order test still agrees with itself, so this is otherwise
invisible; it shipped once (Security Hub's Insights pane declared three
sections and rendered three flat rows). The tab bar is the tell, since
only the split skeleton draws it.

No AWS-integration tests (they'd need creds) — test live behaviour manually
against a real account or the emulator.

## Permissions

neboto is read-only. Per-service IAM actions are in **`PERMISSIONS.md`**.
