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
| What's still unbuilt | [`docs/BACKLOG.md`](docs/BACKLOG.md) |
| How a release is cut / how users install | [`docs/RELEASING.md`](docs/RELEASING.md) — the workflow, `install.sh` and the binstall table share the archive name |

A note on counts: descriptive numbers ("~60 types do X") rot silently here —
three of them had drifted 25–170% before anyone noticed. Prefer a grep the
reader can run over a number they have to trust.

## Commands

```bash
cargo build            # debug   (cargo build --release for release)
cargo run              # run
cargo test             # all tests; `cargo test <module>` for one
cargo clippy           # lint    (cargo check for a fast type-check)
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
  `switch_service` → exact-id pending jump, detail-focused). Guards: the
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
  (for downcasting). Heterogeneous collections are `Box<dyn Resource>`.
- **`ServiceType`** (`src/aws/service.rs`): one variant per service;
  `from_prefix()` maps `@ec2`→`EC2`; `is_global()` true for IAM / Organizations
  / Cost / CloudFront / Trusted Advisor / Global Accelerator / Health /
  Resource Explorer.

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
  Cost (`cost_group_by` keys 1–4 + `cost_period` keys 5–7, `t` cycles), WAF
  (`waf_scope`), RAM (`ram_owner` SELF/OTHER via `t`), Service Quotas
  (`quota_service_code`, `c` picker), CloudTrail (`ct_query`, `f` filter
  modal). **Cost specifics**: the drill-down (Breakdown/Regions/Forecast) is
  fetched over the **active period's window** and the `cost_drilldown` map is
  keyed by `App::cost_drilldown_key` (period + row key) — never the bare row
  key. The Forecast section is always the *current-month* forecast (labeled
  so); its MTD baseline comes from `CostDrilldown.mtd_spent` (breakdown sum
  for MTD, a fourth best-effort CE call otherwise). The 3-month period has no
  comparable prior window, so `CostLineItem.mom_delta_pct` (last two complete
  months, derived from the daily series — no extra CE calls) drives the trend
  arrows/dots and a "By month" table in Trend; the tab row shows a
  right-aligned `Σ` grand total with the aggregate trend.
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
GdDetector) share the `render_simple_split` skeleton in details_pane.rs; the
Vpc and DxLag panes fill their Subnets/Gateways/Connections sections by
**filtering sibling resources** already in `self.resources` (no extra
fetches). Deliberately still flat: `ElasticIp` (the flat view is complete),
`CognitoUser` (would need a lazy `AdminGetUser`), `EcrImage` (not a
selectable resource — rows live inside the ECR repo pane).

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
1. `render_details_pane()` (`details_pane.rs`) downcasts the selected resource
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

**`style_detail_row` conventions** (the `(key, value)` tuples
`*_section_lines` return):
- **Key-value row**: key padded to the body's **adaptive key column**
  (`key_col_widths`: the widest key in the section by display width, clamped
  to `[KEY_COL_MIN=24, KEY_COL_MAX=48]` and to what the pane can spare; keys
  past the cap are ellipsised, never left to push the `:` out — the flat
  view's `━━` headers split the body so sections align independently); key
  `ACCENT` orange, value conditional colour (state / ✓ / ✗ / ⚠ / white).
  Never pad keys yourself in a `*_section_lines` fn.
- **Empty-key note**: empty key, non-empty value (`("", "No tags")`) →
  rendered exactly like a plain content line (`"  No tags"`, no colon); the
  two shapes are interchangeable.
- **Group header**: non-empty key, empty value, *not* leading-space → Magenta
  bold subsection label. (No `=== X ===` markers.)
- **Plain content line**: leading-space key, empty value → `Line::raw`, no
  colon. For fixed-width tables, CIDR lists, template previews. Exception:
  `⚠`/`✗`-prefixed content lines render WARNING/ERROR-coloured.
- **Blank spacer**: both empty → blank line.
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
**One section deliberately has no on-enter hook**: AgentCore's runtime Agent
Card, because `GetAgentCard` is a data-plane call that reaches the running
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

Interactive sub-views render **in the detail pane** (not full-screen modals),
with `Z` toggling full-width (DetailsOnly ↔ Split). Each owns the keymap while
open via a handler that runs early in `handle_key` and returns, and is listed
in `any_pane_overlay_active` (which gates the mouse).

- **Metrics** (`m`): one `App.metrics_in_pane: Option<MetricsKind>` +
  `open_metrics_in_pane` + `render_metrics_in_pane` (`metrics_overlay.rs`).
  Opening from the **list** pane sets `details_focused`, so it must also fire
  the default section's on-enter hook (`reset_detail_section_to_default`) —
  focus without the hook leaves the pane behind the overlay focused but
  untriggered, and closing the overlay then shows a spinner for a fetch that
  never started, forever (the contains-guard blocks any retry).
  Covers EC2/EBS/Lambda/ECS (service + per-task)/RDS/ASG/CW-alarm/Cost/WAF/
  API-GW (**flavor-aware** `ApiMetricsFlavor`: REST = `ApiName` dim,
  `4XXError`/`5XXError` + cache hit/miss; HTTP = `ApiId` dim, lowercase
  `4xx`/`5xx` + `DataProcessed`; WebSocket = `ApiId`,
  Connect/Message/Client/Execution/Integration counts — the three protocols
  share `AWS/ApiGateway` but use different metric names)/
  DynamoDB/EFS/ELB/CloudFront/SQS/SNS/StepFunctions/Network Firewall/
  FSx/EKS/Kinesis/ElastiCache/OpenSearch/Cw-metric/Bedrock (foundation model +
  inference profile, `AWS/Bedrock` dim `ModelId`)/S3 (bucket size + object
  count)/Transfer (`AWS/Transfer` dim `ServerId`)/Route53 health checks
  (`AWS/Route53` dim `HealthCheckId`, global → us-east-1)/SES (`AWS/SES`,
  **account-wide, no dimensions** — one series for any SES row, keyed
  `SES_METRICS_KEY`)/MSK (`AWS/Kafka` dim `Cluster Name`; per-broker
  bytes/disk series aggregated with the SEARCH/SUM+MAX pattern)/NAT gateways
  (`AWS/NATGateway` dim `NatGatewayId`)/VPN connections (`AWS/VPN` dim `VpnId`,
  both tunnels aggregated)/DX connections (`AWS/DX` dim `ConnectionId` —
  state, bps+pps both ways, errors, light levels)/DX virtual interfaces
  (`AWS/DX` dims `ConnectionId`+`VirtualInterfaceId` — per-VIF bps/pps, the
  per-workload traffic signal; a LAG VIF's `connection_id` is the LAG id,
  which is what the dimension expects)/ECR
  repos (`AWS/ECR` dim `RepositoryName` — `RepositoryPullCount` is the
  namespace's only metric, one full-body chart)/Global Accelerator
  (`AWS/GlobalAccelerator` dim `Accelerator` = the **ARN**, published only in
  us-west-2 → `cloudwatch_us_west_2_client()`)/WorkSpaces (`AWS/WorkSpaces` dim
  `WorkspaceId`)/Cognito user pools (`AWS/Cognito` publishes per
  `(UserPool, UserPoolClient)` pair — aggregated across app clients with the
  MSK-style SEARCH/SUM `GetMetricData` pattern)/Athena workgroups
  (`AWS/Athena` dim `WorkGroup` — data scanned + per-phase timing)/Glue jobs
  (`Glue` namespace, **no `AWS/` prefix**, dims `JobName`/`JobRunId=ALL`/
  `Type=count`, Maximum stat; needs job metrics enabled)/Redshift clusters
  (`AWS/Redshift` dim `ClusterIdentifier`)/Redshift Serverless workgroups
  (`AWS/Redshift-Serverless` dim `Workgroup`)/VPC endpoints
  (`AWS/PrivateLinkEndpoints` — publishes under the full 4-dim schema, so it
  needs the SEARCH/SUM `GetMetricData` pattern keyed `VPC Endpoint Id`;
  **gateway** endpoints (S3/DDB) publish nothing → the pane explains instead
  of fetching)/Transit Gateways + attachments (`AWS/TransitGateway` dim
  `TransitGateway`, attachments add `TransitGatewayAttachment`; one
  `MetricsKind::Tgw` + one state map for both)/CW log groups (`AWS/Logs` dim
  `LogGroupName` — incoming events+bytes, the "is this group receiving
  anything" signal)/EventBridge rules (`AWS/Events` dim `RuleName` — matched/
  invoked/failed/DLQ/throttled)/Route 53 **public** hosted zones
  (`AWS/Route53` dim `HostedZoneId` = the **bare** id (path prefix stripped),
  us-east-1 like health checks; private zones publish nothing → not
  offered)/Route 53 Resolver endpoints (`AWS/Route53Resolver` dim
  `EndpointId`, regional — both direction volumes fetched, the wrong one is
  just empty)/CodeBuild projects (`AWS/CodeBuild` dim `ProjectName`). Each is
  namespace +
  dimension-set specific — see the `fetch_*_metrics` fn and `MetricsKind` arm for
  a given service. Adding one ≈ an enum variant + an open arm + a render arm
  **+ an arm in `supports_metrics_overlay()`** — that gates the status-bar
  `m` hint (not the key itself), and it lagging is exactly how DX VIFs shipped
  with a working `m` but no hint.
  Notable dimension gotchas: RDS instances vs clusters use different dims;
  S3 storage metrics are **daily** (fixed 30d window, no time-range knob; only
  offered for current-region buckets, not other-region stubs);
  CloudFront/global namespaces query us-east-1 (but Global Accelerator's
  home is us-west-2); Network Firewall aggregates
  per-AZ/per-Engine series via `GetMetricData SUM(SEARCH(...))`; ECS per-task +
  per-container needs Container Insights **enhanced observability** (empty
  otherwise → hint overlay). Simple U14-batch fetchers share
  `ec2::parse_metric_datapoints` (reads Sum/Maximum/Average, whichever the
  query asked for).
- **Charted CloudWatch dashboard** (`m` on a `CwDashboard`) — the one
  `MetricsKind` that is *not* a fixed chart set: it re-fetches the dashboard
  body, lays the widgets out on the console's own 24-column grid scaled to the
  pane (`dashboard_widget_rect`, `DASH_ROWS_PER_UNIT`), and scrolls (`j`/`k`,
  `Ctrl-d`/`Ctrl-u`, `gg`/`G` — clamped against `dashboard_grid_rows`).
  `Tab`/`Shift-Tab` walk a widget cursor (`▸` + accent border, skipping text
  panels) and `⏎` zooms one widget to the whole pane — the only way to read a
  chart in a split pane, where a grid unit is ~3 columns. `Esc` unwinds zoom →
  cursor → pane, so it never closes out from under a zoom. **Mouse**: click
  selects a widget, double-click zooms, wheel scrolls the grid — the only
  overlay with clickable content of its own, so `handle_cw_dashboard_mouse`
  runs *before* `handle_mouse`'s blanket `any_pane_overlay_active` gate. Hit
  targets come from `App.cw_dashboard_regions` (a `RefCell` the renderer fills
  as it draws, like `click_regions`), so they match what's on screen after
  scrolling and clipping; text panels are excluded, since `Tab` can't reach
  them and a click would strand the cursor. The opening range
  comes from the **body's own** `start` (`parse_dashboard_settings`, resolved
  to the smallest covering preset) and is adopted inside the fetch — the range
  isn't known until the body is read, so doing it there keeps it to one round
  trip; `[`/`]` clears `cw_dashboard_range_from_body` so a refresh can't snap
  back. Series come back in one `GetMetricData` call **per region**, keyed by
  `dashboard_query_id`. Per widget view: `timeSeries` → braille chart (with
  `annotations.horizontal` as reference lines, labelled in the **title** — the
  legend box costs 2 rows + 1 per entry and a widget-sized plot has ~4 to
  give, so an extra entry there hides the legend entirely); `singleValue` →
  big-number tiles (+ sparkline when the author asked); `gauge` → a bar against
  `yAxis.left`; `bar`/`pie` → horizontal bars. `stacked` series accumulate (top
  line = the total; unaligned lengths fall back to unstacked rather than
  inventing an alignment) and `yAxis: right` series are **rescaled onto the
  left range** with a `›` in the legend and their real scale in the title —
  ratatui charts have one y-axis, so a latency-beside-requests widget would
  otherwise draw the latency flat along the bottom (`dashboard_plot_points`).
  `text` widgets draw **unframed**
  like the console does (a border would eat two of their three rows), and their
  markdown links / `[button:…]` extensions flatten to `Label ↗`;
  `alarm` widgets resolve to state chips via a best-effort `DescribeAlarms`
  grouped by the ARN's region. A cell too small for axes degrades to a
  sparkline. Whether a tile shows the *latest* datapoint or the *sum* over the
  window follows the widget's stat (`DrawnSeries::value`) — reading the latest
  point of a `Sum` series reports one bucket as the whole day.
  Three deliberate limits: `MAX_DASHBOARD_QUERIES` (— `GetMetricData` bills per
  metric requested, so an uncapped keystroke on a hundred-series dashboard is a
  surprise charge; same reason it never fires from a section hook); a
  cross-account `accountId` on a metric line is ignored; and one bad metric-math
  expression fails the whole request, so a failure retries once **without** the
  expression queries and flags `math_error` rather than blanking the region.
- **S3 object browser** (`o`): `s3_object_browser.rs` / `S3ObjectBrowserState`.
- **DynamoDB item browser** (`i`): `ddb_item_browser.rs` / `DdbBrowserState`.
- **AgentCore memory session browser** (`i`, on a memory store):
  `memory_browser.rs` / `MemoryBrowserState`. Actors → Sessions → Events
  (`t` toggles a Records mode over long-term memory). `i` is shared with the
  DynamoDB browser — the two selections never co-occur — so a new `i` source
  needs an arm in **both** key chains **and** `supports_item_browser()`, which
  gates the status-bar hint.
- **Live log tail** (`t`, on a log group, Lambda, RDS, ECS task, Network
  Firewall, WAF web ACL, CloudTrail trail, Route 53 hosted zone (query
  logs), Step Functions state machine or execution, CodeBuild, or a
  CodePipeline run): `log_tail.rs` /
  `LogTailState`. **Adding a tail source takes two changes**: the downcast
  branch in `open_log_tail` AND its `is_selected_*` in `supports_log_tail()` —
  the latter gates the `t` key itself (and the status-bar hint), so a branch
  without the gate is dead code (this silently broke WAF + trail tails once);
  a background loop
  polls `cloudwatch::poll_log_tail` and streams `LogTailBatch` events,
  generation-guarded. Non-log-group sources resolve a group first (Lambda
  synchronously to `/aws/lambda/<name>`; ECS/NFW/CodeBuild async, handing the
  resolved group+streams back via `Event::LogTailResolved`). Tails **seed from a
  lookback window** (default last 15m); `[`/`]` narrow/widen it and restart the
  poll. `y` copies, `e` opens the current buffer in `$EDITOR`. `w` wraps long
  lines onto hanging-indent continuation rows instead of clipping (both
  modes; session-sticky, startup default via config `log_wrap`) — the follow
  window is computed **bottom-up by wrapped row count**, so the newest record
  stays fully visible; don't swap it for `Paragraph::wrap`, which anchors to
  the top and clips the tail.
- **Quick log search** (`f`, on a log group): the **same** `log_tail.rs` pane in
  `LogPaneMode::Search`. One-shot server-side `cloudwatch::search_log_events`
  (filter pattern over `[now − range, now]`, capped ~5k lines). `/`/`i` edit the
  pattern, `⇥`/`T` cycle the range preset, `⏎` re-run. `s` flips a live tail into
  search on the same group; `f` on a metric-filter row pre-seeds that filter's
  pattern.
- **Change timeline** (`W`, on **any** resource — list or detail pane):
  `App.trail_in_pane: Option<TrailLensState>` (`trail_lens.rs`; row model in
  `src/timeline.rs`) — "what changed around this resource?", four sources
  merged newest-first, each row wearing a colored badge. **CT**: one-shot
  `cloudtrail::fetch_trail_events` (`LookupEvents` filtered by
  `ResourceName`, throttled 2 TPS so no polling, cap ~50; identifiers from
  `Resource::trail_lookup_keys()`, default `[id]`; IAM types add the
  friendly name). **ALM**: alarms matched by walking the **warm CloudWatch
  cache** (any dimension value == resource id *or* name — namespaces
  disagree on which), then `DescribeAlarmHistory` per match (cap
  `MAX_TIMELINE_ALARMS=5`, StateUpdate items only); a cold cache degrades to
  a dim "alarm history unavailable" note, never an error. **CFN**: when the
  ownership resolver names a stack, `fetch_stack_events` filtered to this
  resource's logical id + stack-level rows (logical id == stack name).
  **DEP**: ECS deployments + service events straight off `EcsServiceInfo` —
  zero fetch. The fan-out is `App::seed_trail_lens` (inputs computed by
  `timeline_inputs`); sources land independently (`TrailLensLoaded` /
  `TrailLensAux`, one shared generation guard) and the lens stays **off**
  the LazyStore deliberately — like the metrics maps it refetches per open.
  CT failures take the headline error slot; aux-source failures degrade to
  dim notes. Header row 2: per-source counts + the ownership line. Keys:
  `f` cycles a source filter, `[`/`]` widen the window (`TrailRange`
  7d/30d/90d — 90d is CloudTrail's ceiling; aux sources filter client-side
  to the same cutoff), `a` toggles read events in (CT only, mutations-only
  default, filtered client-side since `ReadOnly` can't combine with
  `ResourceName`), `r` refreshes all sources, `⏎`/`v` open the row in
  `$EDITOR` (raw JSON for CT, text summary otherwise), `y` copies it.
- **Referenced by** (`U`, on any resource, either pane):
  `App.refs_in_pane: Option<RefsLensState>` (`refs_lens.rs`; matcher + row
  model in `src/references.rs`) — the reverse of ownership: which loaded
  resources mention this one. **Zero API, warm caches only** (the `@all`
  precedent): `scan_refs_lens` walks `cache.get_ref` for every
  `ServiceType` (plus the on-screen list when its cache has lapsed) and
  matches `lookup_keys` (id + distinct name) against each candidate, in
  order: **`Resource::references()`** (`(label, id-or-ARN)` pairs — the
  label becomes the `via` column; default = `security_group_ids()` as
  `Security Group`, overridden on the core compute/network/data types —
  grep `fn references`; Route 53 records add the *derived* load-balancer
  name / bucket next to the raw hostname, since whole-token matching can't
  see `my-alb` inside `my-alb-1234567890.…`), then tag values (`via Tag
  aws:cloudformation:stack-name` is how `U` on a stack lists its
  resources), then flat `details()` rows, then `search_text()`. **Rich
  split-pane types must override `references()`** — their flat `details()`
  omits most links (an instance's groups/subnet/role live only in section
  renderers), which is exactly why `U` on a security group once found
  nothing.
  Matching is **whole-token** (`value_mentions`) so a name like `web` can't
  hit `webserver`; ARN/path-shaped keys match as substrings, which is what
  lets a role name hit inside its ARN. A candidate's own name/id row is
  skipped so a same-named resource elsewhere isn't a "reference". Header
  row 2 states coverage (searched N loaded · M not loaded). Keys: `f`
  cycles a per-service filter, `r` rescans (after loading more services),
  `⏎` jumps to the row via `jump_to_cached_resource` (the helper factored
  out of the `@all` result jump — also used by the access lens), `y` copies
  the id.
- **Network access** (`N`, on anything with security groups):
  `App.access_in_pane: Option<AccessLensState>` (`access_lens.rs`) — the
  **effective** rule table across every attached group, so an instance with
  three groups is one view, not three jumps. Which groups a resource
  carries is `Resource::security_group_ids()` (default empty; overridden on
  instances, ENIs, both SG types, RDS instance/cluster, Lambda, ECS
  service, ELB, EKS (+ the managed cluster SG), ElastiCache, OpenSearch,
  MSK, Redshift cluster/workgroup, CodeBuild, resolver endpoints, VPC
  endpoints, API GW VPC links — grep `fn security_group_ids` for the
  current set; **a new SG-bearing type needs an override or `N` says "no
  security groups"**). Groups resolve from the warm EC2 / VPC caches
  (`cached_security_groups`, converting `VpcSecurityGroup`) and the rest
  from one `DescribeSecurityGroups` (`Event::AccessLensLoaded`,
  `access_generation`-guarded; off the LazyStore like the other lenses, `r`
  refetches all). `merge_rules` collapses identical `(direction, protocol,
  ports, source)` rules and lists the contributing groups; sorted inbound
  first then by port. `t` cycles inbound → outbound → both (sticky across
  opens), open-to-world sources render in the warning colour, `⏎` jumps to
  the row's group (a referenced `sg-` source wins over the contributor),
  `e` opens the whole table, `y` copies a row.
  Both lenses share the `W` plumbing: a `handle_*_key` early in
  `handle_key`, an arm in `any_pane_overlay_active`, the `details_pane`
  dispatch + ribbon guard, and a `supports_*` gate for the status-bar hint.

### Cross-service "go to resource" jump (`Enter`)

`resource_jump_target(key, value, current)` in `details_pane.rs` recognizes ARNs
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
section.

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
`EcsServiceInfo::state()` reflects rollout health (FAILED→red, IN_PROGRESS→yellow).

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
  body, CW dashboard JSON, buildspec, launch-template data, SFN definition) —
  overrides **fall through** when they have nothing, never error; ③
  `Resource::raw_content()` — **raw AWS JSON only** (CloudTrail events,
  GuardDuty / Inspector / Security Hub findings); ④ the default: the full
  detail-pane snapshot as JSON (`export::detail_json` over
  `detail_sections_snapshot()` — every section incl. lazy data, same fidelity
  as the pane), so `e` always opens something. Don't add hand-picked field
  summaries to `raw_content()` (the snapshot supersedes them) and don't make an
  override error on a miss.

### Macros (`,`, `src/macros.rs`)

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
7. Split pane: declare the section table with the `sections!` macro in the
   service file (emits the `*DetailSection` enum + a `SectionDescriptor`
   static from one list — see `src/sections.rs` and
   `docs/adr/0002-section-descriptor-table.md`), return it from
   `Resource::detail_sections()`, add the `*_section_lines` renderer (take
   the enum via `from_index(app.detail_section_idx)`) + a `render_*_split`
   that passes `descriptor_tabs(app, &THE_SECTIONS)` to the pane skeleton.
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
- **`search_text()` runs per resource per keystroke.** For services with
  large per-resource blobs (GuardDuty, Security Hub findings) build a
  **precomputed** `search_blob` at ingestion instead of walking the structure
  in the accessor.
- **Stale load streams (rapid service / profile switches)**: every list-load
  stream is spawned behind a generation-guarded forwarder
  (`load_resources_async` + `App.load_generation`, bumped by
  `load_service_resources` / `start_watch_refresh`, and at the *start* of
  `switch_profile` / `switch_region`) — a superseded stream's remaining
  events are dropped before they reach `handle_event`. The forwarder also
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
- **CLI command copy** (`C`, either pane): `Resource::cli_command()` returns
  the per-type **read** command (`aws ec2 describe-instances --instance-ids …`);
  `App::copy_cli_command` appends `--region` + `--profile` (profile
  omitted while an org role is assumed — a flag can't reproduce that session)
  and copies it. Rules: read-only commands only, never one that reveals a
  secret (Secrets Manager → `describe-secret`, SSM parameter → `get-parameter`
  without `--with-decryption`); quote values with `aws::resource::shell_quote`.
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
  `l`/`→`/`Enter` (drill into detail), `h`/`←`/`⌫`/`Ctrl-O`
  (nav-back through history), `Tab`/`Shift-Tab` (cycle sub-tabs), `1`–`9` (jump
  to sub-tab), `/` (search), `@` (search seeded with `@`, for a fast
  `@service …` switch — works from either pane, via `start_service_search`),
  `y` (copy id/ARN; with a visual selection active, copy the rows as a
  Markdown table), `C` (copy AWS CLI command), `V`/`J`/`K`/`Ctrl-A` (visual
  row selection — see below), `a` (hide-noise filter, gated by
  `is_noise()`; **noise shows by default** — `a` opts in to hiding; the arm
  must check for **no modifier**, or it eats `Ctrl-A` select-all on every
  screen that has noise rows),
  `z` (cycle list sort: load order → name ↑ → name ↓ → state,
  severity-ranked; skipped while a fuzzy query is active; reset on service
  switch). **Exception to "load order"**: the two Executions sub-tabs
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
  `l`/`→`/`Enter` (follow jump), `h`/`←`/`Esc` (back to list), `Ctrl-O`
  (nav-back through history, same as the list pane), `y`/`v`/`e`,
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
- **`src/ui/widgets/details_pane.rs`**: all split-pane renderers,
  `*_section_lines`, `resource_jump_target`/`jump_indicator`, `detail_footer`.
- **`src/ui/widgets/`**: `metrics_overlay.rs`, `s3_object_browser.rs`,
  `ddb_item_browser.rs`, `memory_browser.rs`, `log_tail.rs` (rich views);
  `*_tabs.rs` (one per multi-resource service); the `*_selector.rs` modals
  (`S`/`R`/`P`/`c` pickers); `splash.rs` (welcome screen).
- **`src/export.rs`**: `X` export (detail) / `Ctrl-X` (list) → JSON + CSV +
  Markdown, via `detail_sections_snapshot()`; also `detail_json()` — the
  sections-as-JSON serializer behind the default `e` editor view. Files land
  in the working directory unless `NEBOTO_EXPORT_DIR` redirects (the test
  harness sets it to a temp dir). The detail Markdown synthesizes
  a `## Tags` section when no captured section is named "Tags" (exact
  case-insensitive match — `contains` would hit "Stages").
- **`src/config.rs`**: TOML `Config` (`$NEBOTO_CONFIG` → XDG → `~/.neboto.toml`):
  `default_service`/`default_region`/`default_profile`, `show_banner`,
  `endpoint_url`, `watch`/`watch_interval` (start in watch mode / its cadence),
  `detail_flat` (start in the flat all-section detail view),
  `log_wrap` (start log tail/search panes with long lines wrapped),
  `theme` (presets: `dark` default, `light`, `solarized-dark/-light`,
  `gruvbox-dark/-light`, `dracula`, `nord`, `catppuccin-mocha/-latte` —
  `theme::PRESET_NAMES`; separators optional, `mocha`/`latte` also resolve) +
  `[theme_colors]` (per-color overrides by palette field name, `#rrggbb` or
  ANSI names; bad keys/values warn at startup, never fatal),
  `org_access_role`/`org_access_roles` (member-account switch role name(s),
  default `OrganizationAccountAccessRole`; a list opens the role picker),
  `controltower_audit_account` (+ optional `controltower_audit_role`,
  defaulting to the member-switch role) — the account holding the Control
  Tower Config aggregator, so the Compliance tab works from the management
  account; **quote it**, a bare 12-digit id is a TOML integer and the whole
  file then fails to parse,
  `owner_tags` (owner-ish tag keys for the ownership ribbon, default
  `["owner", "team"]`, case-insensitive) + `managed_by_tags` (ManagedBy-marker
  keys, default `["managedby", "managed-by", "managed_by"]`) +
  `ownership_ribbon` (default true; `false` hides the ribbon, timeline
  unaffected),
  `cache_ttl` (base cache freshness in seconds, default 300) +
  `[cache_ttls]` (per-service overrides keyed by any `@`-search prefix;
  unknown prefixes are ignored).
  No `default_service` ⇒ welcome splash, nothing loads.
- **`src/cli.rs`**: clap `Cli` (parsed in `main` before the runtime).
  `-s/--service`, `-r/--region`, `-p/--profile`, `--endpoint-url`,
  `--banner`/`--no-banner`, `-w/--watch`, `--theme` — each **overrides the config file**
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

### Local emulator (floci / LocalStack)

Point every client at a local emulator via `endpoint_url` in config or the
`AWS_ENDPOINT_URL` env var (env wins; resolved by `AwsClients::resolve_endpoint`).
When active, `AwsClients::build` injects dummy `test`/`test` creds, `s3_client()`
forces path-style, and the tab bar shows a `⚙ host:port` badge. Seed bulk data
with `COUNT=200 ./scripts/seed-floci.sh`, then
`AWS_ENDPOINT_URL=http://localhost:4566 cargo run`.

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
