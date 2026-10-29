# Rich detail-pane views (`m` / `o` / `i` / `t` / `W` / `U` / `N`)

Moved out of the root `CLAUDE.md` so it loads only when needed.

Interactive sub-views render **in the detail pane** (not full-screen modals),
with `Z` toggling full-width (DetailsOnly ↔ Split). Each owns the keymap while
open via a handler that runs early in `handle_key` and returns, and is listed
in `any_pane_overlay_active` (which gates the mouse).

- **Metrics** (`m`): one `App.metrics_in_pane: Option<MetricsKind>` +
  `open_metrics_in_pane` + `render_metrics_in_pane` (`metrics_overlay.rs`).
  **Change markers** (#245, `c` toggles, on by default): the `W` timeline's
  events drawn as vertical lines on every time-series chart. The legend
  (one entry per event name, `×N`) goes on the chart's own bottom border
  when the renderer asks for it (`marker_caption`, the spec view), else on
  the pane's border with the `c`/`W` keys (the hand-built kinds). While any
  in-pane view is open the status bar shows no keys — each view draws its
  own and owns the keymap (`:` is off inside them). `W` from the charts hands their timeline to the lens and parks the
  view (`metrics_behind_lens`); closing the lens restores it as left. `m` runs the same fan-out as `W` once per open, into
  `App.chart_timeline` (`TimelineTarget::Markers` — never alive beside
  `trail_in_pane`, they share `trail_generation`), and the chart builders
  read a render-scoped context (`marker_lines`): **a chart drawn outside
  those builders gets no markers**, so a new time-series chart should use
  one or call `marker_lines` itself. Marker x = event time − (`App.
  metrics_window_end` − `x_max`); the end is recorded as each fetch starts,
  not at render. Not on the dashboard or Cost (their x axis isn't the fetch
  window).
  Opening from the **list** pane sets `details_focused`, so it must also fire
  the default section's on-enter hook (`reset_detail_section_to_default`) —
  focus without the hook leaves the pane behind the overlay focused but
  untriggered, and closing the overlay then shows a spinner for a fetch that
  never started, forever (the contains-guard blocks any retry).
  Covers EC2/EBS/Lambda/ECS (service + per-task)/RDS/ASG/CW-alarm/Cost/WAF/
  API-GW (**flavor-aware**, one spec table per protocol: REST = `ApiName` dim,
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
  one spec table for both)/CW log groups (`AWS/Logs` dim
  `LogGroupName` — incoming events+bytes, the "is this group receiving
  anything" signal)/EventBridge rules (`AWS/Events` dim `RuleName` — matched/
  invoked/failed/DLQ/throttled)/Route 53 **public** hosted zones
  (`AWS/Route53` dim `HostedZoneId` = the **bare** id (path prefix stripped),
  us-east-1 like health checks; private zones publish nothing → not
  offered)/Route 53 Resolver endpoints (`AWS/Route53Resolver` dim
  `EndpointId`, regional — both direction volumes fetched, the wrong one is
  just empty)/CodeBuild projects (`AWS/CodeBuild` dim `ProjectName`)/DMS tasks +
  replication instances (`AWS/DMS` — a task needs **two** dims, the
  instance's *name* and the task's ARN *resource id*; a spec table each). Each is
  namespace +
  dimension-set specific — see its spec table in `src/aws/metrics.rs`, or a
  hand-built kind's `fetch_*_metrics` fn and `MetricsKind` arm. **New kinds go through the spec table**
  (`src/aws/metrics.rs`, `MetricsKind::Spec`, rendered by
  `spec_metrics.rs`; `selected_metric_target`'s arms are the kinds on it): a `&[MetricSpec]`
  (metric, label, stat, unit, `rate`, `group` for metrics that share a
  chart) and an arm in `selected_metric_target` give the row list, the
  chart, change markers, per-metric errors, the `a` picker (`ListMetrics`
  on the resource's dimensions) and `s` stat cycling, from one
  `GetMetricData` call over `Event::Lazy`. A target's optional
  `Breakdown` adds range-total tables beside the chart (under it on a
  narrow pane), fetched in the same task — WAF SEARCHes per-rule and
  per-label totals (`BreakdownTable`, which the web ACL's Traffic section
  draws too). A target's optional `Resolve` is a lookup run once before
  the first fetch (`prepare`): WAF finds the ACL's `WebACL` value that
  way, and CloudFront uses one with no tables (its web ACL's blocked
  series and whether additional metrics are on). A `TargetFix` can set
  dimensions by name, re-key a row by label, add rows (curated set only)
  and give a row a note in place of "no data" (why it can't exist); it
  lands on the target too, so `d` rebuilds the fixed set. `Tab` moves into those tables and `⏎` on a row charts that
  row's own series (`Drill`: specs against the row's dimensions) in
  place of the set, `d` going back; the tables are refetched only for a
  new range or `r`, never for a new charted set. A spec row can be
  **metric math** over the set's other rows (`e(label, expr, unit)`,
  `{MetricName}` placeholders resolved to the request's ids by
  `fetch_series`; an expression naming a metric that isn't charted is a
  row error, not a failed call) and can carry a `Tone` so its colour
  means something (blocked is red whatever the palette order). SEARCH is
  still not a spec row: it returns many series, math returns one. A
  hand-built kind (an enum
  variant + an open arm + a render arm) is for what a spec can't say yet:
  SEARCH expressions, a fixed home region, S3's daily series, dashboards,
  Cost. Either way it needs **an arm in `supports_metrics_overlay()`** —
  that gates the status-bar `m` hint (not the key itself), and it lagging
  is exactly how DX VIFs shipped with a working `m` but no hint.
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
  selects a widget, double-click zooms, wheel scrolls the grid —
  `handle_cw_dashboard_mouse` runs *before* `handle_mouse`'s blanket
  `any_pane_overlay_active` gate (and before the generic in-pane handler). Hit
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
  Columns are the key attributes (pinned left of a `│`) then every other
  attribute, most common first, alphabetical among equals — never
  `HashMap` order, which reshuffled them per open — with widths measured
  from the data. `←`/`→` moves a column cursor and scrolls; `z` sorts the
  *loaded* rows by it (no refetch). The item view (`⏎`) cycles typed rows /
  JSON / DynamoDB JSON with `v`; `y`/`e` copy or edit the current flavour.
  Results carry the browser's `generation` (bumped per read and on
  open/close) and a stale one is dropped, so a slow page from a replaced
  query can't overwrite the new one.
  The query line is the one-line language in `dynamodb/query.rs`; one
  read loops pages (`ddb_task`, aborted by `Esc`, a new read or closing).
  `n` continues the plan that **ran** (`DdbBrowserState::ran`), not the
  text in the line. `f` filters the loaded rows locally. The item view
  has its own line cursor (`detail_cursor`, half-page `Ctrl-D`/`Ctrl-U`,
  `V`/`J`/`K` selection; `y` copies the cursor line's *value* or the
  selection, `Y` the whole item) and its own mouse path
  (`handle_ddb_item_mouse`, via `PopupHits::record_body` — a second hit
  region under the results, so the wheel over the table still steps
  items while the item is open). Quirks and limits:
  `docs/SERVICES.md` (**DynamoDB item browser**).
- **AgentCore memory session browser** (`i`, on a memory store):
  `memory_browser.rs` / `MemoryBrowserState`. Actors → Sessions → Events
  (`t` toggles a Records mode over long-term memory). `i` is shared with the
  DynamoDB browser — the two selections never co-occur — so a new `i` source
  needs an arm in **both** key chains **and** `supports_item_browser()`, which
  gates the status-bar hint.
- **Live log tail** (`t`, on a log group, Lambda, RDS, ECS task, Network
  Firewall, WAF web ACL, CloudTrail trail, Route 53 hosted zone (query
  logs), Step Functions state machine or execution, CodeBuild, a
  CodePipeline run, or a Batch job): `log_tail.rs` /
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
  `src/timeline.rs`) — "what changed around this resource?", six sources
  merged newest-first, each row wearing a colored badge. **CT**: one-shot
  `cloudtrail::fetch_trail_events` (`LookupEvents` filtered by
  `ResourceName`, throttled 2 TPS so no polling, cap ~50 — a capped lookup reports
  where it stopped (`TrailLookup::cut` → `trail_cut`), so the lens reads `N+`
  and the chart legend `N+ changes` instead of passing it off as quiet; identifiers from
  `Resource::trail_lookup_keys()`, default `[id]`; IAM types add the
  friendly name). **ALM**: alarms matched by walking the **warm CloudWatch
  cache** (any dimension value == resource id *or* name — namespaces
  disagree on which), then `DescribeAlarmHistory` per match (cap
  `MAX_TIMELINE_ALARMS=5`, StateUpdate items only); a cold cache degrades to
  a dim "alarm history unavailable" note, never an error. **CFN**: when the
  ownership resolver names a stack, `fetch_stack_events` filtered to this
  resource's logical id + stack-level rows (logical id == stack name).
  **DEP**: ECS deployments + service events straight off `EcsServiceInfo` —
  zero fetch. **ASG** / **RDS** (`trigger_timeline_ops_sources`): what AWS
  itself did, which CloudTrail never records — Auto Scaling activities
  (group, or an instance via its `aws:autoscaling:groupName` tag, filtered
  to activities naming it) and RDS events (instance/cluster; 14-day
  retention, noted on longer ranges). The fan-out is `App::seed_trail_lens` (inputs computed by
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
  refetches all). Below the SG rows come the **subnet NACLs** (#246): the
  subnets are the `Subnet` rows of `references()` (so a type that lists its
  subnets there gets NACLs for free), the ACLs come from the warm VPC cache
  (`cached_network_acls`) or `DescribeNetworkAcls` by `association.subnet-id`
  in the same task. NACL rows keep **rule order** (first match wins — never
  the SG port sort), the `32767` catch-all reads `*`, and an ACL whose first
  rule each way allows everything is labelled `allow-all`. `merge_rules` collapses identical `(direction, protocol,
  ports, source)` rules and lists the contributing groups; sorted inbound
  first then by port. `t` cycles inbound → outbound → both (sticky across
  opens), open-to-world sources render in the warning colour, a source that
  is a load balancer's own SG is labelled `⇠ ALB name` (warm ELB cache only,
  `cached_lb_security_groups`; a cold cache says so in the header), `⏎` jumps to
  the row's group (a referenced `sg-` source wins over the contributor),
  `e` opens the whole table, `y` copies a row.
  Both lenses share the `W` plumbing: a `handle_*_key` early in
  `handle_key`, an arm in `any_pane_overlay_active`, the `details_pane`
  dispatch + ribbon guard, and a `supports_*` gate for the status-bar hint.
