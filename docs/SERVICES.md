# SERVICES.md — per-service notes

**Read the section for a service before editing `src/aws/services/<svc>.rs`.**
This file holds the non-obvious bits: API quirks, things that were tried and
broke, caps and their reasons, and which rows other code keys on. The code is
the reference for *what* each service renders — sub-tab lists, section names and
field sets are all in the service file and shift more often than this document
does. What's here is the part you cannot recover by reading the code.

Split out of `CLAUDE.md` (which was 2,077 lines, 55% of it this material) so it
loads when you're working on a service rather than in every session. The
architecture, the split-pane and lazy-section mechanics, the row conventions and
the cross-cutting gotchas stayed there — start with `CLAUDE.md`, come here for
the service you're touching.

> Descriptive counts in this file are the least reliable thing in it. Three in
> the `CLAUDE.md` original had drifted 25–170% before anyone noticed. Verify
> against the tree rather than quoting a number from here.

---

- **SSM** (`@ssm`, `ssm.rs`) — sub-tabs Parameters/Documents/Fleet/Patch/
  **Associations** (5)/**Run Command** (6)/**Automation** (7)/**Maint
  Windows** (8)/**OpsItems** (9)/**Sessions** (0 — the tenth-tab key, like
  VPC's DHCP), error-tolerant per type. **Patch is a grouped tab**
  (pipe filter: compliance rollups + **patch baselines**). The eleven load
  phases stream **concurrently** (`futures::join!` on one task, atomic
  counters — no spawns), so the current sub-tab never waits behind another
  type's pagination; batch order interleaves, which is invisible because
  every SSM view type-filters. **OpsItems** (`DescribeOpsItems`, capped
  `MAX_OPS_ITEMS=200`; resolved/closed are `is_noise()`, open reads
  yellow): Overview / **Detail** (lazy `GetOpsItem` — description,
  operational data with the `/aws/resources` blob parsed into jumpable
  "Resource" ARN rows via `extract_resource_arns`, related OpsItems, SNS
  notifications; `/aws/dedup` skipped). **Sessions** (`DescribeSessions` —
  the API demands a state filter, so Active + History are two legs of one
  phase, history capped `MAX_SESSION_HISTORY=100`): flat `details()`
  (complete as flat); the "Target" row jumps to the **Fleet** tab (covers
  both `i-` and `mi-` targets, unlike the generic `i-`→EC2 arm). Commands/associations/automations put their
  **instance ids + target values in `search_text()`** ("which commands ran
  on i-…" is a plain fuzzy search), and every multi-value target renders
  **one row per value** (`push_ssm_target_rows`) so each id is individually
  Enter-jumpable — a comma-joined value only ever resolves to its first
  token. Parameter values are
  opt-in reveal `x`/copy `Y`, **never cached**; history omits `SecureString`
  values. **Associations** (State Manager, `ListAssociations`): split pane
  Overview / Targets / Executions / Tags — Executions + Tags share one lazy
  `fetch_ssm_assoc_detail` (`lazy.ssm_assoc_details` keyed by association id,
  capped `MAX_ASSOC_EXECUTIONS=20`); a Failed run status reads red. The
  **Fleet** pane is a 4-section split, one lazy fetch per section dispatched
  by `trigger_ssm_fleet_section_load`: Overview (eager) / **Associations**
  (`DescribeInstanceAssociationsStatus` — per-association status, error code,
  last run) / **Inventory** (`ListInventoryEntries` — **no fluent paginator**,
  hand-rolled token loop; `AWS:Application` sorted by name, capped
  `MAX_INVENTORY_APPS=100` with the true total, plus best-effort
  `AWS:InstanceDetailedInformation` CPU/OS rows; empty ⇒ "inventory not
  gathering" hint) / **Patches** (`DescribeInstancePatchStates` rollup +
  `DescribeInstancePatches` filtered **client-side** to actionable states
  (missing/failed/pending-reboot — the server-side State filter's value
  format is unreliable), failed-first, capped `MAX_INSTANCE_PATCH_ROWS=100`;
  no patch state ⇒ "never patched" hint). Association
  **Run Command** (`ListCommands`, newest first, capped `MAX_COMMANDS=100`):
  Overview (targets, progress counts, output S3 (jumpable) + CW log group,
  parameters) / **Invocations** (lazy `ListCommandInvocations` with
  `details=true` — per-instance status + per-plugin exit code and inline
  output snippet, char-boundary-truncated to 400). **Automation**
  (`DescribeAutomationExecutions`, capped `MAX_AUTOMATION_EXECUTIONS=100`):
  Overview (status, current step, failure message, executed-by, targets,
  outputs) / **Steps** (lazy `DescribeAutomationStepExecutions` — per-step
  action, status, duration, failure). **Maintenance Windows**
  (`DescribeMaintenanceWindows`; disabled reads gray/Stopped): Overview /
  Targets / Tasks (priority order) / History — the three lazy sections share
  one `fetch_maint_window_detail` (`lazy.ssm_mw_details` keyed by
  window id; history capped `MAX_MW_EXECUTIONS=10`). **Patch Baselines**
  (`DescribePatchBaselines`, on the grouped Patch tab): AWS-provided
  (`AWS-*`) are `is_noise()`; **`id()` is the bare `pb-…` `short_id`** while
  `baseline_id` keeps the full ARN AWS-provided baselines need for the lazy
  `GetPatchBaseline` Rules fetch (approval rules, global filters,
  approved/rejected lists, patch groups, sources) — bare-vs-ARN is why the
  Fleet pane's "Baseline" row jump resolves. Association
  ids are UUIDs (no prefix), so cross-tab jumps are label-keyed via
  `ssm_row_jump_target` ("Association ID" → Associations tab, "Document" →
  Documents tab from any SSM pane incl. commands/automations, "Parent
  Execution" → Automation tab, and a Patch-compliance row's "Instance ID" →
  Fleet — the
  drill from red compliance row to the actual missing-patch list). `s` opens the SSM
  session modal on an EC2/Fleet instance (shell/port-forward/remote-host); ECS
  Exec is `s` on an ECS task — all share the tiered `spawn_aws_session` launcher
  (tmux window → new OS window → suspend-TUI inline; clipboard fallback when
  `aws` isn't on PATH).
- **Lambda** — Config/Code/**Triggers**/Environment/Tags(+Optimizer). Two lazy
  sections: **Code** (`GetFunction`; `d` (only there) downloads + unzips the
  deployment package (zip-slip-safe) to `~/.cache/neboto/lambda/<fn>/<timestamp>`
  and copies the path; container-image functions report "no zip to download")
  and **Triggers**, which covers both trigger shapes. Poll-based sources
  (SQS/Kinesis/DynamoDB/Kafka/MQ) come from `ListEventSourceMappings`
  (`lazy.lambda_esms` keyed by function ARN) — per mapping: jumpable source
  ARN, ✓/✗/⚠ status + non-routine transition reason, last processing result
  (errors read red), batch/window/position/retries/bisect, on-failure
  destination, filter patterns. EventBridge rules (a push invoker) are a
  second group in the same section, fed by `fetch_lambda_eventbridge_rules`
  (`lazy.lambda_eb_rules`, also keyed by function ARN): rather than an
  account-wide `ListRules` + `ListTargetsByRule` scan, it calls
  `ListRuleNamesByTarget(target_arn=<function arn>)` per event bus (server-side
  filtered, cheap) then `DescribeRule` per match (capped
  `MAX_LAMBDA_EB_RULES=25`) — each rule's ARN row is the jump anchor back to
  the EventBridge Rules sub-tab. The two lazy fetches share one on-enter hook,
  `App::trigger_lambda_triggers_load`. Other push invokers (API Gateway, S3,
  SNS) still have no reverse lookup — API Gateway has no "find routes by
  backend ARN" API, so that direction would need the same account-wide scan
  this rejects for EventBridge; the empty-state hint says so.
  **Config section — Concurrency + Async Invocation.** The dead-letter queue
  arrives on `ListFunctions` (`FunctionConfiguration::dead_letter_config`), so
  it is an eager field and renders even while the rest is loading. Everything
  else is one lazy fetch (`fetch_lambda_concurrency`,
  `lazy.lambda_concurrency` keyed by function ARN) bundling three calls that
  answer one question between them — "why is this throttling and where do
  failed events go": `GetFunctionConcurrency` (reserved),
  `ListProvisionedConcurrencyConfigs` (per alias/version), and
  `GetFunctionEventInvokeConfig` (on-success/on-failure destinations, retry
  attempts, max event age). Each call is **individually** best-effort and
  pushes to `LambdaConcurrency::warnings` on failure rather than failing the
  whole section — the three are independent and a denial on one shouldn't
  blank the other two. Three renderings are deliberate: reserved `Some(0)` is
  a `⚠ every invocation is throttled` row, not "unset" (it is a kill switch,
  and `None` — sharing the account pool — is the actual unset state); a
  `ResourceNotFound` from `GetFunctionEventInvokeConfig` is the *default*
  state for a function that never had one, so it is swallowed silently rather
  than warned, and the retry/age rows then show Lambda's own defaults (2,
  6 hours) explicitly suffixed `(default)`; and when there is neither a DLQ
  nor an on-failure destination the section says so in a dim annotation row,
  because "events are dropped after the last retry" is the answer to the
  ticket and is otherwise invisible. The provisioned-concurrency qualifier is
  recovered from the tail of the returned `function_arn` — the list item
  carries no separate field for it.
  **Tags only exist on `GetFunction`** — `ListFunctions` returns none, so
  `LambdaFunction.tags` is empty at list time and the real tags live on
  `LambdaCodeInfo.tags` (the lazy Code fetch). Three consumers ride that one
  fetch: the Tags section (its own on-enter hook is the code load), the
  ownership ribbon (a Lambda-specific fallback in `render_ownership_ribbon`
  reads the loaded code info), and drill-in (the Config hook is
  `trigger_lambda_overview_enter` = concurrency + code load, so the ribbon
  appears without visiting Code/Tags first). Don't "simplify" the Config
  hook back to concurrency-only — that reintroduces the invisible-ownership
  bug this fixed.
- **Bedrock AgentCore** (`@agentcore`) — the agent *hosting* platform, a
  wholly separate service from `@bedrock` (different clients, different IAM
  prefix `bedrock-agentcore:`, different resources). Bedrock's "Agents" and
  AgentCore's "Runtimes" are unrelated things; see the naming gotcha in
  `CLAUDE.md`. Two clients: `bedrock-agentcore-control` (every resource
  definition) and `bedrock-agentcore` (data plane, used only for the read-only
  actor/session listings). 10 sub-tabs; **Runtimes**, **Identity**, **Tools**,
  **Policy**, **Evaluation** and **Payments** are grouped tabs
  (pipe-separated `resource_type_filter`s) because the service has more
  families than there are digit keys — and it has run out: `1`–`9` plus `0`
  are all taken, so the next family groups into an existing tab or replaces
  one.
  - **Load**: one phase per family, each warning independently via
    `ResourceLoadWarning` — never the trailing `ResourceLoadError` that
    `bedrock.rs` uses, which would clear `loading` and drop later batches.
    AgentCore ships in a subset of regions; in an unsupported one *every*
    phase warns, which is the intended signal.
  - **Everything is lazy.** Every `List*` in this service returns a thin
    summary — `ListAgentRuntimes` gives 7 fields, `ListMemories` gives 6 (not
    even a name) — so the eager rows are short and each section pulls its own
    `Get*` on first focus. Don't try to enrich the list load: it's breadth
    across every family and would turn one call into N.
  - **Runtime** — 7-section pane (Overview / Artifact / Network / Auth / Env /
    Endpoints / Versions) over three lazy maps: `GetAgentRuntime`
    (`agentcore_runtime_detail`, feeds the first five sections),
    `ListAgentRuntimeEndpoints`, `ListAgentRuntimeVersions`. The artifact's
    container URI is an ECR image reference, so it jumps to the repo through
    the existing `ecr_repo_from_image_uri` classifier with no extra code.
    Environment variables are shown with values: `GetAgentRuntime` is already
    the read boundary and the CLI prints them the same way — this is plain
    config, not the Secrets Manager case.
  - **Gateway** — 6-section pane (Overview / Auth / Targets / Rules / Security
    / Interceptors). `ListGatewayTargets` gives shape but no backend config,
    so each target is deepened with `GetGatewayTarget`, capped at
    `MAX_GATEWAY_TARGETS = 25` and the header says "capped" when it bites; a
    single target that won't describe is skipped rather than failing the
    section. `TargetConfiguration` is a union nested two deep
    (`TargetConfiguration::Mcp(McpTargetConfiguration::Lambda(..))`) — flatten
    to (backend kind, backend ref) so the Lambda ARN lands in a jumpable row.
    `ListGateways` carries **no ARN**. `m` prefers the one on the lazy
    `GetGateway` bundle and otherwise synthesizes
    `arn:aws:bedrock-agentcore:<region>:<account>:gateway/<id>` from
    `App.account_id` (the CodePipeline pattern), so the overlay works straight
    from the list — with no account id yet, there is still nothing to query.
  - **Memory** — 5-section pane (Overview / Strategies / Indexing / Actors /
    Sessions). `ListSessions` is scoped to one actor, so the Sessions section
    walks `ListActors` first and aggregates — same shape as the knowledge-base
    Ingestion section. Both fan-outs are capped (`MAX_MEMORY_ACTORS = 25`,
    `MAX_MEMORY_SESSIONS = 100`) and a per-actor failure is skipped, not
    fatal. `MemoryStrategy::namespaces` is deprecated in favour of
    `namespace_templates`; strategies created through the older API only
    populate the retired field, so the pane reads the current one and falls
    back. `StrategyConfiguration` has four independent arms (extraction /
    consolidation / reflection / self-managed) — flatten **all** populated
    ones; first-match-win under-reports a strategy that sets several.
  - **Tools** — Browser and Code Interpreter get identical 3-section panes
    (Overview / Config / Sessions) sharing one lazy bundle
    (`agentcore_tool_detail`) and one sessions map, because the two `Get*`
    responses differ only in the browser's extras (recording → an `s3://` row
    that jumps to the bucket, enterprise policies, signing). The AWS-managed
    defaults (`aws.browser.v1` / `aws.codeinterpreter.v1`) come back in the
    lists alongside custom ones; both are real, neither is `is_noise()`.
    Browser Profiles stay flat `details()`.
  - **Identity** — the three thinnest list ops in the service:
    `WorkloadIdentityType` is **two fields** (name + ARN) and the two provider
    items add only a vendor and timestamps, so all three panes are lazy over
    one shared `agentcore_identity_detail` bundle. It is keyed by **ARN**
    while all three `Get*` calls key off **name** — the three name spaces are
    independent and could collide.
    - **Workload Identity** (Overview / Return URLs) — `GetWorkloadIdentity`
      adds timestamps and `allowedResourceOauth2ReturnUrls`, the allow-list
      that decides whether three-legged OAuth callbacks work at all.
    - **OAuth2 Provider** (Overview / Provider / Secret) — the deep one.
      `Oauth2ProviderConfigOutput` has nine arms, but **eight are the same
      shape** (`oauthDiscovery` + `clientId`); only `CustomOauth2ProviderConfig`
      adds client-auth method, on-behalf-of token exchange, and private
      endpoints. Bind the common pair once and special-case Custom — nine
      near-identical branches is the version of this that rots. `Oauth2Discovery`
      is a two-arm union (a discovery URL *or* the server metadata inline);
      exactly one is populated, so the pane branches rather than showing an
      empty block.
    - **API Key Provider** (Overview / Secret).
    - **The Secret section is shared** by both provider kinds and shows a
      Secrets Manager **ARN** — a pointer. There is no API that returns an
      OAuth2 client secret or an API key, and the token-vault issue operations
      are not called. `secretSource` is the load-bearing field: MANAGED means
      AgentCore created and owns the secret, EXTERNAL means you did, and that
      decides who may rotate it. The section also pulls `GetTokenVault`
      (one per region, id literally `default`) for the KMS key protecting
      every provider's secret — customer-managed vs service-managed is exactly
      the sort of thing you open this pane to check.
    - **Why the tab is longer than the console's Identity page**: the console
      lists credential providers only. We also list **workload identities**,
      and that is deliberate — AgentCore mints one per runtime, gateway and
      payment manager, and each of those panes links to it by ARN
      (`GetAgentRuntime` / `GetPaymentManager` both report a
      `workloadIdentityArn`, and the classifier routes
      `workload-identity-directory/` here). Drop them for console parity and
      that jump dead-ends in an empty filtered list — `resolve_pending_jump`
      searches `filtered_resources` — and the Return URLs pane loses its only
      home. Not worth matching a console we aren't trying to clone.
    - What the load order does instead: the two credential-provider phases are
      emitted **before** `ListWorkloadIdentities`, so the tab opens on the few
      things you configured with the auto-provisioned identities below. That
      ordering is load-bearing — it's the only place in this service where
      phase order matters.
    - `is_noise()` was considered for workload identities and **rejected**.
      Two reasons, either sufficient: no field distinguishes an auto-created
      identity from one you made (not in the list, not in
      `GetWorkloadIdentity`), so `a` would hide yours too; and CLAUDE.md's
      rule bites — an account with runtimes but no credential providers is
      entirely ordinary, and `a` would blank the tab for it. If AWS ever adds
      a provenance field, that's the hook to revisit.
  - **Metrics** (`m`) — namespace `AWS/Bedrock-AgentCore`, one
    `MetricsKind::AgentCore` with an `AgentCoreMetricsFlavor` (Runtime /
    Gateway / Memory / Browser / CodeInterpreter), mirroring
    `ApiMetricsFlavor`. **Why `SEARCH` and not `GetMetricStatistics`**: AWS
    documents a different dimension set per family (gateway:
    `Operation`/`Protocol`/`Method`/`Resource`/`Name`; runtime usage:
    `Service`, `Service,Resource`, `Service,Resource,Name`) and the runtime
    *invocation* metrics' dimensions aren't documented at all — a
    fixed-dimension query against a guess matches nothing, silently. The
    schema-free `SEARCH('Namespace="…" MetricName="…" "<arn>"', …)` form
    matches the ARN as a token against dimension values whatever the schema
    is, and the outer aggregate collapses the per-operation series. The two
    usage metrics are the exception — they publish at three nesting levels, so
    a token match would sum a resource's hours together with its own
    per-endpoint breakdown; those are pinned to
    `{AWS/Bedrock-AgentCore,Resource,Service}`. Verify against a real account
    with `aws cloudwatch list-metrics --namespace AWS/Bedrock-AgentCore`
    before assuming a dimension.
  - **Log tail** (`t`, on a runtime / gateway / memory) — all three group
    names are deterministic, so there's no `Event::LogTailResolved`
    round-trip (Lambda-style sync resolution): runtime →
    `/aws/bedrock-agentcore/runtimes/<runtime_id>-<endpoint>`, gateway and
    memory → `/aws/vendedlogs/bedrock-agentcore/{gateway,memory}/APPLICATION_LOGS/<id>`.
    The runtime's endpoint comes from `agentcore_tail_endpoint()`: `DEFAULT`
    if the Endpoints section has listed it, else the first endpoint, else the
    literal `DEFAULT`. These groups exist only where observability has been
    enabled, so a tail on a runtime that never had it turns up empty — that's
    the account's state, not a bug.
  - **Preview families** (tabs 6-8 Policy / Evaluation / Registry, plus
    Harness, Payments and configuration bundles) — grouped tabs over
    `ListPolicyEngines`+`ListPolicies`,
    `ListEvaluators`+`ListOnlineEvaluationConfigs`+`ListDatasets`,
    `ListRegistries`, `ListHarnesses`, `ListConfigurationBundles` and
    `ListPaymentManagers`+`ListPaymentCredentialProviders`.
    Their load phases **stay silent on failure** — no
    `ResourceLoadWarning`, unlike the five core families. Most accounts have
    never enabled these, so a denial is the norm rather than something
    unusual, and a warning on every load everywhere would be noise; the
    per-tab empty state in `resource_list.rs` carries the whole explanation.
    That is the two sides of CLAUDE.md's warn-vs-silent rule inside one
    service — pick per family, don't copy whichever phase you read first.
    - Two panes: **Policy** (Overview / Definition — `PolicyDefinition` is a
      three-arm union, and the Cedar/statement text becomes an
      `editor_override_content` with a **`.txt`** suffix, not `.json`: Cedar
      isn't JSON and `$EDITOR` would flag every line) and **Registry**
      (Overview / Records, lazy `ListRegistryRecords`). Evaluators, online
      evaluation configs and datasets are flat.
    - `EnforcementMode::LOG_ONLY` drives `AgentCorePolicy::state()` — a
      log-only policy is healthy *and* blocking nothing, which the lifecycle
      status alone would hide. Likewise `OnlineEvaluationExecutionStatus` is
      **ENABLED/DISABLED**, an on/off switch and *not* a lifecycle status;
      treating it as one gives you a dead `FAILED` branch.
  - **Memory session browser** (`i` on a memory store) —
    `src/ui/widgets/memory_browser.rs`, modelled on `s3_object_browser.rs`:
    same in-pane placement, `Z` full-width, `/` filter, `n` paging. Three
    levels (**Actors → Sessions → Events**) plus a `t`-toggled **Records**
    mode. This exists because the event log *is* AgentCore Memory — the
    control plane describes a store's config, only `ListEvents` (with
    `include_payloads`) says what the agent actually remembered.
    - An event's payload is a **list** of parts, each a conversational turn or
      an opaque blob; render every part (a tool-call turn commonly pairs a
      message with a blob), don't take the first.
    - Records is scoped by **`memory_strategy_id`**, not namespace: namespaces
      are templates (`/strategies/{id}/actors/{actorId}`) that would need
      placeholder substitution against a specific actor. `[`/`]` cycle
      strategies; the pick list comes from the memory's already-loaded lazy
      detail, so Records on a store you never drilled into has nothing to
      scope by and says so instead of calling (a guaranteed
      `ValidationException`).
    - Row text is model output, so `trunc()` strips control characters — a
      newline in a remembered turn would otherwise corrupt the frame.
    - `supports_item_browser()` gates the `i` hint and now covers both DynamoDB
      and Memory. It is the same shape as the `t`/`supports_log_tail` pair that
      shipped broken twice; a new `i` source needs both.
  - **A2A agent card** (`x` on a runtime's Agent Card section) — the **only
    section in the app with no on-enter hook**, and deliberately so.
    `GetAgentCard` is a *data-plane* call on the same client as
    `InvokeAgentRuntime`; an A2A card is served by the agent itself, so asking
    for one reaches the container and can cold-start an idle runtime —
    billable compute. An on-enter hook would fire that from a `Tab` press, and
    the flat view's trigger sweep (`\`) would fire it for every runtime you
    looked at. So it follows the Secrets Manager precedent instead: metadata
    free, the thing behind it opt-in. The section explains the cost, and says
    when the runtime isn't A2A (those serve no card). The `x` arm lives in the
    **detail-pane** chain — it's section-gated, so the list-pane chain could
    never reach it.
  - **Harness** (tab 9) — the *other* kind of agent: a declarative loop AWS
    runs for you (model + system prompt + tools + memory binding), where a
    **runtime** is your own container and the loop is your code. Both say
    "agent"; three things in this service now do. 9-section pane (Overview /
    Model / Prompt / Tools / Skills / Memory / Environment / Endpoints /
    Versions) shaped deliberately like the runtime's, because
    `ListHarnessEndpoints` / `ListHarnessVersions` mirror the runtime ops
    exactly. One `GetHarness` bundle feeds the first seven sections.
    - Five unions to flatten, all of them multi-arm:
      `HarnessModelConfiguration` (Bedrock / OpenAI / Gemini / LiteLLM),
      `HarnessToolConfiguration` (gateway / browser / code-interpreter /
      remote MCP / inline function), `HarnessSkill` (AWS skills / Git / S3 /
      path), `HarnessMemoryConfiguration` (an existing AgentCore store /
      a managed one / disabled) and `HarnessTruncationStrategyConfiguration`.
      The tool and skill arms carry ARNs and `s3://` URIs, so they land in
      jumpable rows through the existing classifiers with no extra code.
    - **Third-party model keys are ARNs, not keys.** `apiKeyArn` on the
      OpenAI/Gemini/LiteLLM arms points at Secrets Manager; the API returns
      the pointer and the pane shows the pointer. Nothing resolves it.
    - A remote-MCP tool's `headers` map is rendered **names only** — the
      values are bearer tokens as often as not.
    - The system prompt is free text and routinely multi-line, so it renders
      one content row per line: `/`, the visual selection and copy all work
      over it normally.
  - **Configuration bundles** (in the **Runtimes** tab, no key of their own) —
    a versioned blob of component configuration that a runtime endpoint's
    traffic split points at (`TrafficSplitEntry.configurationBundle`). That
    reference is the whole reason they're grouped with runtimes rather than
    given the last digit: a bundle means nothing except next to the endpoint
    routing it. 3-section pane (Overview / Components / Versions) over
    `GetConfigurationBundle` + `ListConfigurationBundleVersions`. Components
    are untyped `Document`s keyed by component name, so the pane pretty-prints
    them (`aws::document::document_pretty`) instead of inventing a schema.
    No status field — `state()` is deliberately empty rather than faked.
  - **Payments** (tab 0) — **metadata only, and that is a rule, not a
    limitation.** The data plane carries `GetPaymentInstrument`,
    `GetPaymentInstrumentBalance` and `GetResourcePaymentToken`; none is
    called and none may be. They return spendable material, which is the same
    line neboto draws around Secrets Manager values — except there isn't even
    an `x` reveal here, because no read of an instrument belongs in a browser.
    - Two listed types: **payment managers** (3-section pane: Overview / Auth
      / Connectors) and the account's **payment credential providers**
      (Overview / Vendor). `ListPaymentConnectors` is scoped by
      `paymentManagerId` — there is no account-wide connector listing — so
      connectors are a lazy section on the manager's pane, not a third type.
      Each connector is deepened with `GetPaymentConnector` for its credential
      wiring, capped at `MAX_PAYMENT_CONNECTORS = 25` like gateway targets.
    - `PaymentProviderConfigurationOutput` (Coinbase CDP / Stripe-Privy)
      returns every credential as a `Secret { secretArn }` — pointers only.
      The Vendor section separates them into a "Secret References" group with
      a note saying so, because rows labelled "App Secret" read like the
      secret otherwise.
    - The credential provider has no id of its own: its **ARN is its
      identity** (and the `agentcore_payment_detail` map key), while
      `GetPaymentCredentialProvider` looks up by *name*. Both are carried.
  - **Resource policies** (`Resource Policy` section on the Runtime and
    Gateway panes) — who *outside this account* can invoke the thing.
    `GetResourcePolicy` is documented as supported "only for AgentCore Runtime
    and Gateway", so the section exists on exactly those two and nothing has to
    probe-and-fail elsewhere.
    - **No policy attached is the normal, good case** and comes back as
      `ResourceNotFoundException` — mapped to an empty `Loaded`, not an
      `Error`, so the pane says "access is governed by IAM identity policies
      alone" instead of showing a scary red row. Matched on the wire code
      (`e.code()`), not `into_service_error()`, which would consume the
      `SdkError` that `sdk_error_message` needs for the real-failure path.
    - The document is parsed rather than dumped: `Statement` is a bare object
      as often as a list, and `Principal` / `Action` / condition values are
      each `"x"` or `["x","y"]` interchangeably — handle both forms or half of
      real policies render empty. `NotPrincipal` / `NotAction` are labelled
      `NOT …` because they mean the inverse and would otherwise read as an
      ordinary allow-list.
    - Overview leads with an **External Access** verdict computed against
      `App.account_id`. With no caller identity resolved it says *unknown*
      rather than "none" — claiming no external access when you can't tell is
      the one wrong answer here. `Service:` principals are AWS, not third
      parties, so they don't count as external; `*` always does.
    - `e` opens the raw document (`.json`). Parsing failures fall back to
      showing it inline rather than erroring.
  - **Policy generations** (Generations section on the Policy Engine pane) —
    AgentCore's policy *generator*: you describe what an agent should be
    allowed to do and it emits Cedar **assets**, one per fragment of the
    description. This is where a `PolicyDefinition::PolicyGeneration` policy
    finally leads; before it existed, the Policy pane printed
    "generation `<id>` · asset `<id>`" and there was nowhere to go.
    - Scoped by `policyEngineId`, hence a section rather than a tab. Assets
      need a second call per generation (`ListPolicyGenerationAssets`), so
      that pass is capped at `MAX_POLICY_GENERATIONS = 15`, newest first — a
      generation history is append-only and the recent end is the live one.
      A generation past the cap says "assets not listed" rather than showing
      as empty, because an un-listed generation must not read as a clean one.
    - **`Finding` is the point of the section.** Four of the seven
      `FindingType` values mean the generated policy does not do what the
      fragment asked *and fails open*: `AllowAll`, `DenyNone`,
      `NotTranslatable` (the fragment silently produced nothing) and
      `Invalid`. Those are counted in a "Flagged Assets" line at the top and
      marked `⚠` per asset. `AllowNone` / `DenyAll` are equally a mismatch but
      fail **closed** — an over-restrictive policy announces itself the first
      time something is denied, while the other four leave you believing a
      control exists when it doesn't. Flagging all six would warn on most
      generations and train people to ignore the marker.
    - Each asset shows the **source fragment** next to the generated policy —
      the two together are what makes a bad generation legible; the Cedar
      alone doesn't tell you what it was meant to say. `e` exports every
      generated policy in the section with its fragment and findings as
      comment headers (`.txt`, since Cedar isn't JSON).
    - `PolicyGeneration.resource` is a one-arm union carrying an ARN, so the
      Target row jumps through the generic classifier.
  - **Policy Engine** — Overview / Policies, and the Policies section takes
    **no fetch**: the engine's policies are already in `App.resources` from
    the same load, so it filters siblings on `engine_id` (the Vpc/DxLag
    precedent). Overview counts *enforcing* policies separately from total,
    because a `LOG_ONLY` policy is healthy and blocking nothing — an engine
    whose policies are all log-only reads as configured but enforces zero.
  - **`W` (CloudTrail lens)** — every type overrides `trail_lookup_keys()` to
    `[id, arn]` (gateway: `[id, name]`, it has no eager ARN;
    payment-credential provider: `[arn, name]`, its id *is* the ARN).
    CloudTrail's `ResourceName` for AgentCore may be either form depending on
    the event, and keys are tried in order until one returns something, so
    carrying both costs a second lookup only on a miss. The default `[id]`
    alone would have missed silently — `W` would just look empty.
  - **Tags** — every one of the fifteen panes has a Tags section, all fed by
    a single `agentcore_tags` `LazyMap` keyed by ARN (`ListTagsForResource`
    takes any AgentCore ARN, so one fetcher serves the service). The ARN comes
    from `App::selected_agentcore_arn`, which every family answers eagerly
    except the gateway — that reuses the account+region synthesis the metrics
    overlay and the resource-policy section already use.
    - **`tag:` search deliberately does NOT cover AgentCore**, and this is the
      ceiling the design accepts rather than an oversight. `split_tag_filters`
      matches `Resource::tags()`, a *synchronous* field populated when the list
      loads; no AgentCore `List*` returns tags (only `GetDataset`,
      `GetPaymentManager` and `GetPaymentCredentialProvider` do), so filling it
      would mean one `ListTagsForResource` per resource across every family on
      every load — a ~15-call list load becomes N. That is the same trade the
      other fourteen lazy tag maps in `LazyStore` already declined. The empty
      Tags section says so out loud, because "`tag:` doesn't match" otherwise
      reads as a broken query rather than a documented limit.
    - The section is threaded as one extra `Option<&Lazy<Vec<(String,String)>>>`
      parameter per `*_section_lines`, bound **once** in `get_detail_lines`
      ahead of the AgentCore arms. Two renderers cross clippy's argument limit
      as a result and carry an explicit `allow` — each parameter is one lazy
      map the pane reads, and a struct no other pane uses would hide that.
  - **Not built**: nothing from the deferred families remains — see
    `docs/BACKLOG.md` for what's still open (console deep links, deeper
    evaluation panes, a few flat types with a `Get*` behind them).
  - **`console_url()` is the service home, not a deep link.** AWS documents
    exactly one console URL for AgentCore
    (`https://console.aws.amazon.com/bedrock-agentcore/home#`) and every
    console walkthrough then says "select Gateways from the left navigation
    pane" — no per-section or per-resource fragment is published anywhere.
    All nineteen types therefore return the same regionalized home URL. That
    still beats the "No console URL for this resource type" error `o` used to
    give, and a guessed `#/runtimes` that lands on the wrong family or on
    nothing would be worse than either. Confirming a real fragment against a
    live console makes this a one-line upgrade per family — that is the whole
    remaining work, and it needs a browser, not a doc.
- **Bedrock** (`@bedrock`) — 9 sub-tabs, two clients (`bedrock` +
  `bedrock-agent`); load is error-tolerant per resource type. Foundation
  Models / Inference Profiles, plus **Prompts / Flows / Custom Models / Imported
  Models** (sub-tabs 6–9, plain streaming lists off `List*` summaries), are flat
  `details()`. Foundation Models + Inference Profiles support `m` metrics
  (`AWS/Bedrock`, dim `ModelId` = the model/profile id;
  `MetricsKind::BedrockModel`). The three list APIs that return only thin
  summaries — **Guardrails**, **Knowledge Bases**, **Agents** — deepen via lazy
  `Get*` fetches on first focus.
  - **Guardrails** — 7-section split pane (Overview / Content / Topics / Words /
    Sensitive / Grounding / Advanced) backed by a lazy `GetGuardrail`
    (`lazy.guardrail_detail` keyed by guardrail id): content filters, denied
    topics, custom + managed word lists, PII + regex, contextual grounding,
    cross-region, KMS, blocked messaging.
  - **Knowledge Bases** — 5-section split pane (Overview / Config / Vector Store
    / Data Sources / Ingestion) with **three** independent lazy fetches:
    `GetKnowledgeBase` (`lazy.kb_detail` — feeds the first three sections: KB
    type, embedding model, and the 8-way vector-store union flattened to rows in
    `storage_rows`), `ListDataSources`+`GetDataSource` per source
    (`lazy.kb_data_sources` — connector config + chunking strategy), and
    `ListIngestionJobs` aggregated across data sources (`lazy.kb_ingestion`,
    capped 10/source). `trigger_kb_section_load` dispatches the right fetch for
    the current section.
  - **Agents** — 4-section split pane (Overview / Action Groups / Aliases /
    Knowledge Bases). Overview is a lazy `GetAgent` (`lazy.agent_detail` —
    foundation model, instruction, role, guardrail, memory, orchestration, idle
    TTL); the other three are lazy `ListAgentActionGroups` /
    `ListAgentAliases` / `ListAgentKnowledgeBases` (sub-lists needing a version
    use `DRAFT`). `trigger_agent_section_load` dispatches per section.
- **FSx** (`@fsx`) — sub-tabs File Systems/Volumes; volumes are first-class
  (fuzzy-searchable, own detail pane, own `m`). Storage metric **names +
  dimensions differ per FS type**, so they're **discovered** via
  `ListMetrics(AWS/FSx)` rather than hard-coded. `m` charts storage/throughput/
  latency + a storage-composition breakdown; `f` toggles volume↔file-system
  graph.
- **EKS** (`@eks`) — the showcase: eight-section split pane. **Tier 1
  (AWS-API depth) only** — live Kubernetes workloads (pods/nodes via the cluster
  API server) are deliberately out of scope. Add-on "update-available" costs one
  `DescribeAddonVersions` per add-on; Insights (`UPGRADE_READINESS`) is the
  headline. `t` tails `/aws/eks/<name>/cluster`.
- **RAM** (`@ram`) — owner-scope toggle (`t`, SELF/OTHER-ACCOUNTS), variant-cached.
- **Trusted Advisor** (`@ta`, `trusted_advisor.rs`) — single-list of
  checks (legacy Support API, us-east-1, Business/Enterprise plan required),
  scope toggle (`t`, Account/Organization, variant-cached). **Organization
  scope is a client-side fan-out**, because the console's organizational view
  has no readable API (it's a console-only report generator) and the newer
  `trustedadvisor` org APIs return only the Enterprise-Priority subset — so
  it runs `organizations:ListAccounts` and fetches every active account's
  `DescribeTrustedAdvisorCheckSummaries` via `assume_config_for_account`
  (`org_access_roles` tried in order per account; the management account —
  identified by `DescribeOrganization` — uses base creds, since it can't
  assume the member role into itself). Concurrency is capped at 4 (Support
  API TPS is small); progress streams as empty-batch `LoadProgress` ticks
  ("n/N accounts"). Rollup: worst status wins (`worst_status`, ranked
  error < warning < ok < not_available), counts sum, and the per-account
  breakdown rides `TaCheck.org_accounts` (sorted worst-first) — no lazy
  fetches in org scope. Per-account failures degrade to *aggregated*
  warnings bucketed by cause (no support plan / role hop failed / other,
  split by `is_assume_error`) — each names **every** skipped account, not a
  sample: the status line truncates visually but `M` holds the full text,
  and "which accounts and why" is what the user needs to fix
  `org_access_roles` coverage. Fatal only when **no** account answers, or
  `ListAccounts` itself fails (not the management account). Org rows set
  `TaCheck.org_scope` and swap to the reduced `TA_ORG_CHECK_SECTIONS`
  descriptor (Summary/Accounts — the per-instance-descriptor pattern CFN
  deleted stacks established): flagged-*resource* detail stays per-account
  (`DescribeTrustedAdvisorCheckResult` needs that account's creds), so the
  Accounts section points at the member-account switch instead of fetching.
  `support:Describe*` had to be added to the read-only session supplement
  policy — the AWS-managed `ReadOnlyAccess` doesn't cover the Support API,
  so every member-account call would otherwise die on the session policy.
  Search in org scope also matches affected (error/warning) account
  names/ids — deliberately not all accounts, or every check would match
  every account name. **Priority scope** (third `t` position) is different
  data, not a filter: Trusted Advisor Priority's curated org recommendations
  via the newer `trustedadvisor` API (`aws-sdk-trustedadvisor`, pinned
  us-east-1) — the only readable slice of that API's org surface, and it
  answers only on Enterprise Support with Priority enabled, from the
  management account or a delegated admin (denial → friendly hint). Rows are
  `TaRecommendation` (its own resource type + Overview/Accounts/Resources
  descriptor, lazy per section keyed by recommendation **ARN** — the
  identifier every `trustedadvisor` org call takes). Active + closed are
  both fetched; closed map to `ResourceState::Stopped` so `F` separates the
  console's Active/Closed tabs; default sort is active-first, then worst
  status, then last-updated. Affected resources are capped at
  `TA_REC_RESOURCES_CAP` (200) with a truncation note, and rows are grouped
  by `account_id · region`. **Per-account filtering is search**: the list
  API has no account filter, so the load does the console selector's
  client-side join itself — one `ListOrganizationRecommendationAccounts`
  per recommendation (buffer_unordered 4; fine because Priority lists are
  curated-small — do NOT copy this enrichment onto an unbounded list),
  names best-effort from `ListAccounts`, results into
  `TaRecommendation.affected_accounts` ("name (id)") feeding
  `search_text` — so `/prod` or `/123456789012` filters recommendations to
  the ones hitting that account. Enrichment failures stay silent (the row
  just isn't matchable by account; the lazy Accounts section still errors
  inline when viewed). The lifecycle `Update*` operations are mutations and
  are never called.
- **EventBridge** (`@events`) — six sub-tabs: Rules / Event Buses / **Archives**
  / **Replays** / **Schedules** / **Pipes**, three clients (events + scheduler +
  pipes). `EbEventBus` has a 3-section split pane (Overview / **Permissions** /
  Tags) — the resource policy is loaded eagerly by `ListEventBuses`, so
  Permissions pretty-prints it inline (no lazy fetch). Archives / Replays /
  Schedules / Pipes stream as error-tolerant batches after the rules (a
  scheduler/pipes permission gap records a warning, never breaks the core
  load). Archives + Replays are flat `details()` (source bus/archive ARN rows
  jump). **Schedules** (EventBridge Scheduler): `ListSchedules` summaries carry
  **no expression**, so the 2-section split pane (Overview / Target) is fed by
  one lazy `GetSchedule` (`lazy.eb_schedule_detail` keyed by ARN, triggered on
  drill-in — Overview itself needs it). **Pipes**: `ListPipes` already returns
  source/enrichment/target, so Overview is eager; Configuration + Tags read a
  lazy `DescribePipe` (`lazy.eb_pipe_detail` — filter patterns, role, logging,
  KMS, tags). `arn_jump_target` routes `events`/`scheduler`/`pipes` ARNs to the
  right sub-tab (`JumpView::Eb`).
- **RDS** (`@rds`) — six sub-tabs: Instances / Clusters / **Snapshots** (3) /
  **Param Groups** (4) / **Option Groups** (5) / **Subnet Groups** (6), all
  streamed as best-effort batches after the instances+clusters core.
  Snapshots unify `DescribeDBSnapshots` + `DescribeDBClusterSnapshots`
  (self-owned) into one `RdsSnapshot` (**automated/awsbackup snapshots are
  `is_noise()`** so `a` hides the daily churn). The instance pane is Config /
  Storage / Network / **Perf Insights** (key 4, lazy `pi:GetResourceMetrics` —
  `db.load.avg` grouped by `db.wait_event`, only when PI is enabled; uses
  `dbi_resource_id`, not the DB id; PI has its own regional client
  `AwsClients::pi_client`) / **Backups** (eager — retention/window/latest
  restorable + the DB's snapshots **filtered from sibling `RdsSnapshot` rows**,
  no fetch) / **Maintenance** (eager `PendingModifiedValues` pre-flattened in
  `from_sdk`, CA cert expiry, + lazy `DescribePendingMaintenanceActions`
  keyed by ARN — `lazy.rds_pending_maintenance`, shared with clusters) /
  **Events** (lazy `DescribeEvents`, the API's 14-day ceiling, newest-first
  capped `MAX_RDS_EVENTS=100`, `lazy.rds_events` keyed `kind:id`) / **Logs**
  (lazy `DescribeDBLogFiles` — the native error/postgresql.log.* files,
  newest-first capped `MAX_RDS_LOG_FILES=100`; **`e` on a "Log File" row**
  downloads its tail via `DownloadDBLogFilePortion` into `$EDITOR` — distinct
  from `t`, which tails the *CloudWatch export* when enabled) / Tags. The
  cluster pane mirrors it (Config / Endpoints / Members / Backups /
  Maintenance / Events / Tags): Config adds serverless-v2 ACU range, storage
  and replication; Members shows writer/reader roles (writer first). A
  non-empty pending-modifications list puts a ⚠ count in both headers.
  **Param Groups** unify instance + cluster groups (like snapshots;
  `default.*` are `is_noise()`): Overview / **Parameters** (lazy
  `DescribeDB(Cluster)Parameters`, `lazy.rds_parameters` keyed
  `RdsParamGroup::params_key` — user-overridden first under a "Modified"
  header, capped `MAX_RDS_PARAMETERS=500`, `·static` marks reboot-to-apply).
  **Option Groups** (`default:*` noise) and **Subnet Groups** are flat
  `details()` (complete as flat; subnet/VPC/SG rows jump via the generic
  classifier). Cross-tab jumps are label-keyed via `rds_row_jump_target`
  ("Writer"/"Reader"/"Source DB"/"Read Replica"/"Replica Source" → Instances,
  "Cluster"/"Source Cluster"/"Replication Source" → Clusters, "Snapshot" →
  Snapshots, "Parameter Group"/"Option Group"/"Subnet Group" → their
  sub-tabs, status suffixes stripped).
- **Route 53** (`@r53`) — sub-tabs Zones / **Records** / Health Checks.
  Zones stream first, then health checks stream as a second batch
  (`ListHealthChecks`, tags batched 10-at-a-time via `ListTagsForResources`).
  **Zone tags are batched the same way at list time** — `ListHostedZones`
  returns none, and without the eager fill `Resource::tags()` is empty for
  the ribbon / `tag:` filters / `U` / exports. **The tag APIs take the bare
  `Z…` id**, not the `/hostedzone/Z…` path `ListHostedZones` hands out:
  `GetHostedZone` and friends tolerate the path, `ListTagsForResource(s)`
  reject it, and the lazy bundle used to swallow that rejection into "No
  tags" (#26). `resolve_zone_tags` / `fetch_zone_tags` strip it; the bundle
  now carries `tags_error` and the Tags section renders it via `error_rows`.
  **Records is a real cross-zone list**, not a second view of the zones (it
  used to be: same type filter, the only difference was firing the selected
  zone's records fetch, which `Enter` on the zone already did). Each
  `R53Record` is a `Resource` — name column = FQDN (trailing dot stripped),
  the dim second column is the **value** (`resource_list::id_cell` special-
  cases records; the compound id `<TYPE> <name> [<set>] @<zone>` is a key,
  not a label), and `state()` is the record **type** so `F` cycles A / CNAME
  / TXT and `z`'s state sort groups by type. Rows are mirrored in from
  `lazy.r53_zone_records` — the same map the zone pane's Records section
  fills — by `App::sync_r53_record_rows` (keyed by zone id, idempotent),
  called on tab entry, after each zone's fetch lands, and from
  `handle_resources_fully_loaded` (a reload / watch swap rebuilds the list
  from zones + health checks alone, and after `r` the store is empty so the
  tab re-triggers). The sync re-stamps the cache when no load is streaming,
  so `@all` and the `U` lens see records. **Fetch shape**:
  `trigger_r53_all_records_load` inserts every zone key as Loading and walks
  them in ONE task **sequentially** — Route 53 throttles at 5 req/s
  account-wide; a hundred concurrent `ListResourceRecordSets` would mostly be
  SDK retries — sending a hand-built `Event::Lazy` per zone whose closure
  applies to the map *and* syncs. Budgets: zones over
  `MAX_R53_TAB_ZONE_RECORDS` (3 000; the count is free from
  `ListHostedZones`) and zones past `MAX_R53_TAB_ZONES` (100) per trigger
  are skipped, counted in `App.r53_records_skipped`, and named in the tab
  strip + the empty state; the zone pane still loads any of them on demand.
  `⏎` on a record opens its flat `details()`; `Zone ID` jumps to the zone
  (`r53_row_jump_target`, record-selected only — the zone's own Info row has
  the same label), `Health Check` to the check, and `Alias Target` /
  `Target` (a CNAME's single value) to where the hostname leads:
  `R53Record::dns_target` recognises ELB DNS names (name recovered by
  stripping `dualstack.`/`internal-` and the generated `-<suffix>`; ALB
  suffixes are decimal, NLB hex, so ≥8 hex-digit tail), `*.cloudfront.net`,
  S3 website endpoints (bucket = record name for an alias, embedded for a
  CNAME), and `*.awsglobalaccelerator.com`. Load balancers resolve by exact
  name → detail pane; the others land on the filtered list (a
  distribution's `name()` is its first alias, not the `d….cloudfront.net`
  domain). `references()` emits the derived LB name / bucket alongside the
  raw hostname because whole-token matching can't see `my-alb` inside
  `my-alb-1234567890.…` — that pair is what makes `U` on a load balancer list
  the DNS names pointing at it — plus the record's own `Name`, so `U` on a
  CloudFront distribution whose alias *is* the name finds it. Timeline (`W`)
  keys on the **zone** (`ChangeResourceRecordSets` is logged against the
  hosted zone). No per-record console deep link exists; `console_url` opens
  the zone's record list.
  `R53HealthCheck` split pane: Overview / Status (lazy `GetHealthCheckStatus` —
  per-region observations) / Tags. `m` metrics use the **global** `AWS/Route53`
  namespace (dim `HealthCheckId`) queried in us-east-1 like CloudFront.
  Disabled checks show a dimmed state; calculated/alarm checks report no
  per-region status. `R53HostedZone` split pane:
  Records / **Sharing** / **Info** / Tags. Sharing, Info and Tags share one
  lazy bundle (`fetch_zone_detail`, `lazy.r53_zone_detail` keyed by zone id
  — the same bundling Resolver endpoints/rules use): `GetHostedZone` for
  **every** zone now (name servers off the delegation set; VPC associations
  for private zones), `ListVPCAssociationAuthorizations` (private), `GetDNSSEC`
  (public only — it errors on private zones), `ListQueryLoggingConfigs`,
  `GetHostedZoneLimit` (`MAX_RRSETS_BY_ZONE`, so Info shows `n / 10000 (x%)`
  with a `⚠` from 90%), and `ListTagsForResource`. Only the sharing calls
  are fatal; the Info extras are best-effort and degrade to `· status
  unavailable` / `✗ not enabled` rows. **Sharing** (private zones only —
  public zones short-circuit to "not shared") answers "who is this
  zone shared to": `GetHostedZone`'s `VPCs` list (live associations) plus
  `ListVPCAssociationAuthorizations` (cross-account VPCs authorized but not
  necessarily associated — AWS never auto-deletes the authorization once
  used, so a VPC can appear in **both** groups; that overlap is flagged inline
  as a dangling-authorization warning, not treated as an error). Neither API
  reports the owning account, only VPC id + region, so rows are labeled by id
  only; same-region VPC ids are `Enter`-jumpable via the generic `vpc-`
  classifier, cross-region ones are shown as plain text (no bespoke
  cross-region jump — that's a deliberate simplification, not an oversight).
  **Query-log tail**: `t` on a zone tails its query-log group, which lives in
  **us-east-1** regardless of the current region — the region is parsed from
  the config's log-group ARN (`parse_log_group_arn`) and passed as the tail's
  region override, the CloudTrail-trail precedent. The group is only known
  after the Info bundle loads, so `supports_log_tail` (and the `t` hint) turn
  on then; pressing `t` earlier explains which of the two it is (not loaded
  vs. logging off).
  **Record routing policy.** `R53Record` used to carry only name/type/TTL/
  values/alias, which meant every record in a weighted, latency, failover or
  geolocation set rendered as an *identical* row — the set identifier is the
  only thing distinguishing them, and it was dropped. All the discriminators
  now come off the same `ListResourceRecordSets` response (zero extra calls):
  `set_identifier`, `weight`, `region`, `failover`, `geo_location`,
  `geo_proximity`, `cidr_routing`, `multi_value_answer`, `health_check_id`,
  `traffic_policy_instance_id`, plus the alias `evaluate_target_health`.
  `routing_policy()` names the policy by which discriminator is populated
  (exactly one ever is); `routing_summary()` renders the `↳ weighted · set
  "blue" · weight 10` annotation line under the table row, and returns `None`
  for a simple record so the common case stays a single line. Two details
  worth keeping: weight **0** is spelled out as "never served" (it is a
  deliberate drain, not a missing value), and a geolocation `country_code` of
  `"*"` is Route 53's catch-all marker, rendered "default (everywhere else)"
  rather than a literal asterisk. The health-check id is emitted as a
  **labelled** `("      Health Check", id)` row rather than a `↳` annotation
  precisely so `App::r53_row_jump_target` can key on the label and send `⏎` to
  the Health Checks sub-tab — bare check ids are UUIDs with no prefix for the
  generic classifier to recognize. Renaming that row kills the jump silently.
  **Pagination** (`fetch_zone_records`, shared by the pane and the tab): the
  manual `ListResourceRecordSets` loop once advanced only `start_record_name`
  + `start_record_type`. When a page boundary falls *inside* a set of
  weighted/latency records — which all share one name and type — those two
  markers restart at the top of the set and the loop never advances. It also
  carries `start_record_identifier` (the response's `next_record_identifier`),
  with a non-advancing-marker guard breaking the loop if all three repeat.
- **Route 53 zone pane, Records section** — rows are `TYPE relname : value ·
  ttl` with the name **zone-relative** (`R53Record::relative_name`: `@` apex,
  `www`, `*.dev`). The FQDN repeated the zone on every row and one long name
  dragged the adaptive key column to `KEY_COL_MAX`, leaving values ~18
  columns in a split pane (#17 follow-up). `⏎` on any record row opens the
  record's own pane on the Records tab (`r53_zone_record_row_target`,
  resolved against the zone's loaded records — members of a weighted/latency
  set share the key, so the row value picks between them); the same-service
  jump path calls `enter_r53_records_tab` because the tab's rows come from
  the lazy map, not the list load, and `R53Record::search_text` leads with
  the compound id so the pending jump survives the fuzzy filter. Values
  longer than the pane still clip — wrapping is #23.
- **Route 53 Resolver** (`@resolver`) — rule detail fetch **must** pass the
  `ResolverRuleId` filter to `ListResolverRuleAssociations`. `rslvr-` id jump
  routing is checked longest-prefix-first: `rslvr-rr-` (rule ids) → the Rules
  sub-tab, else the bare `rslvr-` (`in-`/`out-` endpoint ids) fallback → the
  Endpoints sub-tab — checking the bare prefix first would misroute rule ids.
  **Delegation (2025-06)**: a DELEGATE rule has an outbound
  `resolver_endpoint_id` but **no** `target_ips` (the API rejects them) — its
  payload is `delegation_record`, so don't key "has an endpoint" on
  `rule_type == FORWARD` (that hid every DELEGATE rule's endpoint once); use
  `ResolverRule::uses_endpoint`. `INBOUND_DELEGATION` is a third endpoint
  direction, Do53-only, sharing the console's inbound-endpoints route.
  `TargetAddress` carries *either* `ip` or `ipv6` — an IPv6 target has an
  empty `ip`, which is why targets are kept as `ResolverTarget` structs and
  formatted by `Display` (pre-formatting `ip:port` rendered them as `:53`).
  The endpoint pane's Rules section is zero-API: it filters the sibling rule
  rows by `resolver_endpoint_id` (only outbound endpoints carry rules).
- **Route53 Profiles** (`@profiles`, `route53profiles.rs`) — standalone
  regional single-list service (same regional rationale as Resolver: a
  Profile bundles DNS config for VPCs in one region). `ListProfiles` returns
  only thin summaries (id/arn/name/share_status); the split pane's four
  sections (Overview / **VPCs** / **Resources** / Tags) share **one** lazy
  fetch (`fetch_profile_detail`, `lazy.r53_profile_details` keyed by profile
  id — `GetProfile` for status/owner/times, `ListProfileAssociations` for VPC
  associations, `ListProfileResourceAssociations` for the DNS resources
  bundled into the profile: private hosted zones, Resolver rules, or DNS
  Firewall rule groups). A VPC association's `resource_id` is actually the
  VPC's **ARN**, not a bare id — rendered as-is, since the generic classifier
  tokenizes on non-alphanumeric characters and finds the embedded `vpc-…`
  token regardless. A bundled resource's `resource_type` is a free-text
  string, shown verbatim; only hosted-zone and Resolver-rule ARNs are
  Enter-jumpable (DNS Firewall rule groups aren't modeled as their own
  resource in this app, so those rows just display). There is **no** reverse
  lookup ("which profiles include this VPC/zone") — `ListProfileResourceAssociations`
  only filters by `profile_id`/`resource_type`, never by a resource ARN, so
  that query would require enumerating every profile; not worth the cost.
  Hosted-zone ARN jumping (`arn:aws:route53:::hostedzone/Z…` → the Zones
  sub-tab, reconstructing the `/hostedzone/ID` form `R53HostedZone::id()`
  actually stores) was added to `arn_jump_target` as part of this work — it
  didn't exist before and nothing else in the app referenced a hosted-zone
  ARN.
- **Transfer Family** (`@transfer`) — N+1 streaming load (`DescribeServer` per
  server); Users lazy. `m` metrics (`AWS/Transfer`, dim `ServerId`):
  bytes/files in+out and on-upload workflow success/failure.
- **Step Functions** (`@sfn`, `step_functions.rs`) — sub-tabs **State
  Machines** / **Executions**. `ListStateMachines` streams first, then a
  best-effort execution phase: `ListExecutions` per **STANDARD** machine
  (EXPRESS retains none), `buffer_unordered(8)`, capped
  `MAX_EXECUTION_MACHINES=100` × `MAX_EXECUTIONS_PER_MACHINE=25`, each batch
  sorted newest-first; both caps and any per-machine
  failure warn rather than erroring. **Executions are first-class
  resources**, not rows inside the machine's pane — that's what makes "which
  workflow is running / what broke" a list operation. `SfnExecution::name()`
  is `<machine> / <execution>` (the sub-tab spans every machine, so a bare
  UUID doesn't identify a row — hence the `misnamed_getters` allow),
  `is_noise()` = SUCCEEDED, and **`f`** cycles an
  All/Running/Failed status filter (the ECS Tasks precedent) on top of the
  generic `a`/`F`/`z`. Execution split pane: **Overview** (status, duration —
  elapsed for a running one — state-machine ARN jump, map run, failure
  error+cause, redrive) / **Input** / **Output** (both lazy
  `DescribeExecution`, `lazy.sfn_exec_details`; `e` is section-gated to open
  the raw JSON payload, since these blobs outgrow the pane; an oversized
  payload written to S3 is flagged, and a missing output distinguishes
  "still running" from "failed") / **History** (lazy `GetExecutionHistory`,
  `lazy.sfn_history`). History fetches with **`reverse_order(true)`** and
  keeps the newest `MAX_HISTORY_EVENTS=1000` — a cap that dropped the *tail*
  would drop the failure. It renders two views of one fetch: a derived
  **States** timeline (`build_spans` pairs StateEntered/StateExited, closing
  the newest open span of that name so Map/Parallel branches don't
  cross-match, and attaches the invoked resource + error of whatever
  happened while the state was open) — the span with no exit is what the
  execution is doing *right now* — over the raw **Events** stream. The state
  machine's own **Executions** section filters **sibling** `SfnExecution`
  rows (zero fetch, the Vpc-Subnets pattern) and its ARN rows jump into the
  execution's pane; Details gained the logging block (level, log group,
  execution-data flag, X-Ray) — the same group **`t`** tails.
  `t` resolves it from `DescribeStateMachine` asynchronously
  (`Src::SfnStateMachine` → `resolve_sfn_log_group`, WAF-shaped) unless a
  Details/Definition visit already cached it, and an execution's `t` tails
  its **state machine's** group (executions have none of their own — `s`
  flips to search to narrow by ARN). Logging is OFF by default on Step
  Functions, so both the section and the `t` failure say so explicitly. The
  `"states"` arm in `arn_jump_target` routes `stateMachine:` ARNs (by name,
  version/alias suffix stripped) and `execution:` ARNs (by full ARN — the
  execution's `id()`) to the right sub-tab via `JumpView::Sfn`. `m` stays
  state-machine-only (`AWS/States`, dim `StateMachineArn`).
- **Kinesis** (`@kinesis`, `kinesis.rs`) — sub-tabs **Data Streams** / **Firehose**
  (both in one file, one `ServiceType`, error-tolerant per type). Data Streams
  stream first (N+1 `DescribeStreamSummary`, Details/Consumers/Tags split pane);
  Firehose streams as a second batch (`ListDeliveryStreams` has no fluent
  paginator — hand-rolled `has_more_delivery_streams` loop — then N+1
  `DescribeDeliveryStream`). `FirehoseStream` 4-section split pane Overview /
  Destination / Processing / Tags(lazy). `parse_destination` flattens the SDK's
  per-destination-type union (Extended S3 / S3 / Redshift / OpenSearch / HTTP /
  Splunk / Snowflake / Iceberg) into rows — source (Kinesis/MSK ARN),
  destination ARN (bucket / domain), and each processor's transform-Lambda ARN
  are jumpable via the generic ARN classifier. Overview shows failure reason
  (when `*_FAILED`), encryption, and the CloudWatch error-logging group;
  Destination adds data-format conversion (Parquet/ORC), dynamic partitioning,
  and source-record S3 backup; Processing lists the record processors (Lambda
  transform + params, metadata extraction, etc.). **`t`** live-tails the error
  log group (`error_log_group()` from the destination's
  `cloud_watch_logging_options`, resolved synchronously like Lambda/RDS). `m`
  metrics (`AWS/Firehose`, dim `DeliveryStreamName`): incoming records/bytes,
  delivered records, DeliveryToS3.Success, **DataFreshness** (delivery-lag
  health signal), throttled records.
- **Athena** (`@athena`, `athena.rs`) — browse-only, one `ServiceType`, five
  sub-tabs streamed as sequential error-tolerant batches (a permission gap or a
  federated-catalog failure records a status message and moves on):
  **Workgroups** (`ListWorkGroups` → N+1 `GetWorkGroup`; split pane Overview /
  Configuration / Tags(lazy `ListTagsForResource` — the tag ARN is built in
  `trigger_athena_workgroup_tags_load` from `account_id` + region since the SDK
  returns none)), **Data Catalogs** (`ListDataCatalogs`, flat `details()`),
  **Databases** (`ListDatabases` per discovered catalog, keyed `catalog/db`;
  split pane Overview / Tables(lazy `ListTableMetadata` — columns, partition
  keys, table type, `location`)), **Recent Queries** (`ListQueryExecutions` per
  workgroup capped → `BatchGetQueryExecution` chunks of 50; split pane Overview /
  Query / Statistics — status, **data scanned** cost proxy, per-phase timing,
  result-reuse), **Saved Queries** (`ListNamedQueries` → `BatchGetNamedQuery`;
  split pane Overview / Query). No query execution; SQL renders inline as
  plain content lines (`push_sql_lines`) — `e`/copy get the full text. `m` on a
  **workgroup** charts `AWS/Athena` (dim `WorkGroup`): data scanned, total/
  engine/queue/planning time, DPU consumed (capacity WGs only; empty when the
  workgroup has metric publishing disabled). `fmt_bytes`/`fmt_millis` helpers
  live in `athena.rs` (details_pane has its own `fmt_bytes`).
- **Glue** (`@glue`, `glue.rs`) — browse-only, one `ServiceType`, five sub-tabs
  streamed as sequential error-tolerant batches (a permission gap on any one API
  records a status and moves on). **All sub-tabs load eagerly — no lazy sections,
  so no `*State` HashMaps or `*Loaded` events.** **Databases** (`GetDatabases`,
  flat `details()`); **Tables** (first-class + fuzzy-searchable, an N+1
  `GetTables` per database **capped at `MAX_TABLES=2000`** total — when hit it
  logs how many databases went unscanned; split pane Overview / Schema (columns +
  partition keys) / Storage (SerDe, location, formats)); **Crawlers**
  (`GetCrawlers`; split pane Overview / Targets / Configuration — `target_rows`
  is pre-flattened in `from_sdk` across S3/JDBC/DynamoDB/Catalog/Delta/Iceberg/
  Mongo targets so `s3://` paths render jumpable); **Jobs** (`GetJobs`; split pane
  Overview / Command / Arguments); **Job Runs** (an N+1 `GetJobRuns` per job
  capped at `MAX_JOB_RUNS_PER_JOB=20`, most-recent-first like the ECS
  stopped-task window; split pane Overview / Arguments — state, duration,
  **DPU-hours** cost proxy, error). **`t`** on a job run tails its CloudWatch log
  group (`log_group()` → the run's own group or `/aws-glue/jobs/output`, stream =
  run id) via the synchronous `Src::Group` log-tail path. `m` on a **job**
  charts the `Glue` namespace (no `AWS/` prefix; dims `JobName`/`JobRunId=ALL`/
  `Type=count`, Maximum stat — the aggregates are cumulative counters): bytes/
  records read, elapsed time, completed/failed tasks, stages. Empty unless the
  job has **job metrics enabled**. Duration renders via the details_pane-local
  `fmt_secs`.
- **SES** (`@ses`, `ses.rs`, **sesv2**) — browse-only, one `ServiceType`, three
  sub-tabs streamed as sequential error-tolerant batches. **Identities**
  (`ListEmailIdentities` summaries enriched by an N+1 `GetEmailIdentity`;
  split pane Overview / DKIM / MAIL FROM / Tags — verification, DKIM
  status+tokens, custom MAIL FROM, feedback forwarding, tags all come from the
  one `GetEmailIdentity`, so no lazy sections); **Configuration Sets**
  (`ListConfigurationSets` + N+1 `GetConfigurationSet` — split pane Overview
  (sending/reputation/TLS/tracking/suppression/VDM posture; `state()` flags a
  sending-paused set) / **Event Destinations** (lazy
  `GetConfigurationSetEventDestinations`, `lazy.ses_event_dests` keyed by set
  name) / Tags); **Suppression List**
  (`ListSuppressedDestinations` — address/reason/last-update, flat; state is
  `Unavailable` so bounced/complained addresses read as a warning). **Account
  posture** (send quota, sending enabled, production access) is a one-shot
  `GetAccount` in phase 0 delivered on `Event::SesAccountLoaded` into
  `App.ses_account` and folded into every identity's Overview (the single lazy-ish
  bit; **a zero-identity account won't surface it** — documented limitation). `m`
  charts **account-wide** `AWS/SES` metrics (Send/Delivery/Bounce/Complaint/
  Reject/Rendering Failures) — **no dimensions**, so one series serves any SES row
  (`MetricsKind::Ses`, cached under the fixed `SES_METRICS_KEY`, not per-resource).
- **MSK** (`@msk`, `msk.rs`, **aws-sdk-kafka**) — browse-only, single-list of
  Kafka clusters (provisioned **and** serverless) with an Overview / Networking /
  Monitoring / Config / Tags split pane. The load is a **single** paginated
  `ListClustersV2` — it already returns the full nested `provisioned`/`serverless`
  config (broker type/count, storage, auth modes, encryption, subnets/SGs, open
  monitoring), so **no N+1 describe**. `from_sdk` flattens both cluster-type
  unions into one `MskCluster` (serverless populates only subnets/SGs + SASL/IAM
  auth). **Config** is the one lazy section: `DescribeConfigurationRevision`
  (`lazy.msk_config` keyed by cluster ARN) prints the applied `server.properties`;
  clusters with no custom configuration (serverless / defaults) short-circuit to
  "using MSK defaults" with no fetch. Networking rows render subnet-/sg- ids as
  key-value rows so the generic classifier makes them jumpable. `m` metrics
  (`AWS/Kafka`, dim `Cluster Name`): GlobalTopicCount / GlobalPartitionCount
  (cluster-level), plus **per-broker** BytesInPerSec / BytesOutPerSec / KafkaData
  LogsDiskUsed aggregated across brokers with the **SEARCH/SUM** (and MAX for
  disk) `GetMetricData` pattern Network Firewall uses — the schema form
  (`{AWS/Kafka,"Cluster Name","Broker ID"}`) restricts the search to broker-level
  series so per-topic series don't double-count. `ListNodes` (per-broker ENI/IP
  enumeration) is deferred — the section list doesn't need it.
- **Redshift** (`@redshift`, `redshift.rs`, two clients: `redshift` +
  `redshiftserverless`) — sub-tabs **Clusters / Serverless / Snapshots**,
  streamed as three error-tolerant batches. **Clusters**: one paginated
  `DescribeClusters` — the response carries the full config (nodes, endpoint,
  VPC/SGs, encryption, IAM roles, maintenance, snapshot retention, **tags**),
  so no N+1 and an all-eager Overview / Network / Config / Tags split pane;
  `state()` maps the rich status set (paused→Stopped, resizing/modifying→
  Pending, hardware-failure/incompatible-*→Unavailable). **Serverless**:
  `ListNamespaces` first (a failure degrades to a per-pane hint), then
  `ListWorkgroups` **joined by namespace name** — one `RedshiftWorkgroup`
  carries both the compute/network side and its namespace's data plane
  (db, admin, KMS, IAM roles, log exports), so the pane is Overview /
  Namespace / Network (all eager, no lazy `*State`). **Snapshots**:
  `DescribeClusterSnapshots` + serverless `ListSnapshots` unified into one
  `RedshiftSnapshot` (newest first), flat `details()` (complete as flat);
  **automated snapshots are `is_noise()`** so `a` hides the daily churn.
  VPC / subnet / SG / KMS / IAM-role rows are key-value so the generic
  classifier makes them Enter-jumpable. `m` on a cluster charts
  `AWS/Redshift` (dim `ClusterIdentifier`: CPU, disk-used %, connections,
  health, IOPS); on a workgroup `AWS/Redshift-Serverless` (dim `Workgroup`:
  RPU capacity, compute-seconds billing proxy, connections, queries
  running/queued) — both share one `redshift_metrics_time_range`.
- **Firewall Manager** (`@fms`, `fms.rs`) — browse-only, four sub-tabs
  streamed as sequential error-tolerant batches: **Policies** (`ListPolicies`
  → per-policy `GetPolicy` **and** `ListComplianceStatus`, both best-effort in
  one enrichment pass; split pane Overview / Scope (include/exclude account+OU
  maps, resource-tag scoping, resource-set ids) / **Config** (the
  `managed_service_data` per-type policy JSON pre-flattened by the generic
  `msd_rows` walker — works for all security-service types, capped 300 rows;
  nested values the classifier can route (`arn:`/`sg-`/`subnet-`/`vpc-`/
  `s3://`) keep key-value form so WAF rule groups, NFW references, and SGs
  are Enter-jumpable — the wafv2 `rulegroup` ARN arm in `arn_jump_target`
  exists for this; `e` on Config opens the raw JSON via an editor override)
  / **Compliance**
  (the payoff section: per-account compliant/violator matrix from the eager
  rollup, then a lazy `GetComplianceDetail` **violator drill** per
  non-compliant account — worst accounts first, capped `MAX_DRILL_ACCOUNTS=10`
  with the overflow reported; violator rows are key-value so resource ids
  jump-classify, though cross-account ids only resolve when they live in the
  browsed account) / Tags (lazy `ListTagsForResource` on the policy ARN)).
  A policy with any non-compliant account reads **red** (`state()` →
  Unavailable, the AWS Config rule mapping) and fuzzy-matches "noncompliant";
  the Overview carries a one-line verdict. **Apps Lists** / **Protocols
  Lists** (the `List*` summaries carry the full entries — flat `details()`),
  **Resource Sets** (`ListResourceSets` — **no fluent paginator**, hand-rolled
  `next_page_token` loop; split pane Overview / Members, one lazy fetch =
  `GetResourceSet` + `ListResourceSetResources`). **Admin gotcha**: nearly
  every FMS API only answers from the FMS administrator / delegated-admin
  account — phase 0 probes `GetAdminAccount` into `App.fms_admin` (folded into
  each policy's Overview; cleared on profile switch), and on success adds two
  best-effort context calls: `GetNotificationChannel` (SNS topic, jumpable)
  and `ListMemberAccounts` (count — also shown by an all-org Scope). Resource-
  set member URIs render as key-value rows (account → ARN) so same-account
  members jump. Every phase warns
  instead of erroring, and only an all-phases-empty failure goes fatal;
  `resource_list.rs` renders a "not the Firewall Manager administrator" empty
  state (with the admin account id when visible). `OUT_OF_ADMIN_SCOPE`
  policies/sets read as unavailable. No CloudWatch namespace → no `m`.
- **CodeSuite** (`@code`) — Repos/Build Projects/Pipelines/**Deployments**(4)/
  **Artifacts**(5)/**Executions**(6)/**Pull Requests**(7), five clients. A
  file-path buildspec is
  pulled lazily from the
  source repo; a `CODEPIPELINE`-source project has no repo, so it's found via the
  pipeline's CodeCommit Source stage. `t` tails a build's logs. A sub-service
  failure emits `ResourceLoadWarning` per phase; only an all-phases-empty load
  goes fatal.
  - **Pull Requests** (tab 7) — first-class rows on the Executions precedent:
    `name()` is `<repo> #<id> <title>` (the tab spans repos), `id()` the PR
    id (unique per account+region), newest-first default sort via the same
    `execution_start_ms` hook. **The eager phase is OPEN-only** — one
    `ListPullRequests` per repo (most return an empty id list and cost
    nothing further), then **`GetPullRequest` per id — there is no batch
    variant** — behind a 50-repo cap and a global 300-lookup atomic budget;
    each cap warns when hit, and a failure warning carries a sample error
    message (a bare count can't distinguish throttling from a permission
    gap). The phase runs at `buffer_unordered(4)`, not the other phases' 8:
    the first cut listed CLOSED eagerly too at 8-wide and throttled into
    "failed for 24 repos" partial loads on a real account — don't re-widen
    it or re-add an eager closed pass. **Closed PRs are a per-repo lazy
    fetch** (`cc_repo_closed_prs`, cap 10, the repo pane's Pull Requests
    section hook) rendered as summary rows with a `(MERGED)`/`(CLOSED)`
    suffix in the key — the suffix is what keeps the digits-only
    `cc_repo_pr_row_jump_target` from offering a jump to a row that doesn't
    exist on the sub-tab. The tab's empty state explains the open-only
    scope (`resource_list.rs`). Split pane **Overview** (targets, merge metadata, description,
    and a lazy Approvals group — `EvaluatePullRequestApprovalRules` +
    `GetPullRequestApprovalStates`, each independently best-effort; PRs
    without approval rules say so instead of erroring) / **Activity**
    (`DescribePullRequestEvents`, cap 200 — exactly one metadata struct is
    populated per event type, all flattened into the detail cell) /
    **Comments** (`GetCommentsForPullRequest`, cap 100 threads, inline
    threads keyed `path:line`) / **Changes** (`GetDifferences` merge-base →
    source-tip, cap 500 files; an empty merge base falls back to the
    destination commit). **`e` on Changes builds a real unified patch**:
    `GetBlob` both sides per file (cap 50 files / 1 MB per blob, binary
    detected via non-UTF-8) diffed locally with the `similar` crate —
    CodeCommit has no patch API — assembled in a spawned task that sends
    `Event::CcPrPatchLoaded` (the async-editor-stash pattern; content must
    land before `editor_requested`). The repo pane's own Pull Requests
    section shows Open (sibling rows, zero fetch) + Recently Closed (the
    per-repo lazy fetch above); `Enter` on an open `#<id>` row jumps to the
    PR via `cc_repo_pr_row_jump_target` (the digits-only `("  #<id>", …)`
    row shape is load-bearing).
  - **Repos** (CodeCommit) — split pane Details / **Branches** / **Commits** /
    **Pull Requests** / **Triggers** / README. **There is no git-log API**: history is a
    first-parent walk from a branch tip, one `GetCommit` per commit — that's
    why the Commits walk is sequential and capped (`MAX_CC_COMMITS`, with a
    `+`-suffixed count and an annotation when truncated) rather than
    paginated. Branches resolves tips via `GetBranch` per branch
    (`buffer_unordered(8)`, capped `MAX_CC_BRANCHES`) then one
    `BatchGetCommits` pass (≤100 ids/call) for tip metadata. **`Enter` on a
    branch row re-keys the Commits walk to that branch** and switches
    sections (`try_cc_branch_commits` in the detail-pane Enter chain): the
    walk map is keyed `repo\u{1}branch`, and `App.cc_commits_branch` holds
    the picked `(repo, branch)` — filtered by repo name at read time, so a
    stale entry for another repo is ignored and **no reset wiring exists**;
    the branch-row shape `("  {name}", tip-info)` is load-bearing for that
    handler (the default marker lives in the *value* for this reason). Tags +
    triggers come from **one** `fetch_repo_extras` feeding both the Details
    Tags group and the Triggers section (both sections' on-enter hook — the
    contains-guard makes the second free); each half is independently
    best-effort so a denied `ListTagsForResource` doesn't blank triggers.
    Repo-level `Resource::tags()` stays empty (tags are lazy, so the `tag:`
    search filter can't match repos — a per-repo N+1 in the list load wasn't
    worth it). `e` on Commits opens a git-log-style text of the walk (bodies
    included); on README it opens the markdown (both via
    `pending_editor_content`).
  - **Pipelines** — `ListPipelines` summaries (six fields, **no ARN**); the
    Stages section is a lazy `GetPipeline` + `GetPipelineState` that also
    carries **Tags** (`ListTagsForResource` on an ARN built from
    `account_id` + region, the Athena-workgroup pattern — the SDK returns
    none, which is why that tab used to be permanently empty). An action's
    **whole** configuration map is kept: `action_target` picks the
    provider's primary key and the row **label it renders under** (`Project`
    / `Repository` / `Application` / `Stack` / `Service` / `Function` /
    `State Machine` / `Bucket` / `ECR Repository` / `Source`), which is what
    `App::code_row_jump_target` keys on — so those labels and that table
    must stay in step. Everything else renders verbatim via `other_config`,
    minus `token`/`secret`/`password`-ish keys (`GetPipeline` masks
    `OAuthToken`, nothing masks a third-party equivalent).
  - **Executions** (tab 6) — pipeline runs are **first-class resources**
    (`CodePipelineExecution`, `resource_type()` `"Pipeline Execution"`), not
    text inside the pipeline pane: that's what makes "which run is going /
    what broke" a list operation with `a` (noise = Succeeded **and**
    Superseded), `z`/`F`, fuzzy search, and an `f` All/Running/Failed filter
    (`ExecStatusFilter`, shared with Step Functions). `name()` is
    `<pipeline> / <execution id>` (the tab spans every pipeline, so a bare
    UUID doesn't identify a row — hence the `misnamed_getters` allow) and
    `id()` is the execution id. Loaded as a best-effort phase after the five
    lists: `ListPipelineExecutions` per pipeline, `buffer_unordered(8)`,
    capped `MAX_EXECUTION_PIPELINES=100` × `MAX_EXECUTIONS_PER_PIPELINE=20`,
    sorted newest-first; caps and per-pipeline failures warn. Split pane **Overview** (status + `status_summary`, trigger,
    mode/type, rollback target, stop reason, source revisions, and a
    per-stage ✓/⚠/✗ verdict) / **Actions** / **Artifacts** — all three fed
    by one lazy `ListActionExecutions` filtered to the run
    (`lazy.code_action_executions`, keyed by execution id). **Actions** is
    the payoff: per action the provider-labeled target, the **Build ID**
    (`external_execution_id`, `<project>:<uuid>` for CodeBuild), error code
    + wrapped message, the console URL, and the CloudWatch **log stream
    ARN**. Artifacts shows per-action input/output artifacts plus the
    namespaced `output_variables`. The pipeline's own Executions section
    filters **sibling** rows (zero fetch, the Vpc-Subnets pattern), which is
    why it has no on-enter hook.
  - **`t` on a run** tails the failing action's build logs with **no resolve
    call** — `log_stream_arn` is a full stream ARN, so
    `log_group_and_stream()` splits it and reuses `Src::CodeBuildDirect`.
    `selected_code_action_execution` maps the Actions cursor onto an action
    (via `code_exec_actions_lines_indexed`, the
    `code_build_builds_lines_indexed` shape), falling back to the first
    action that logged anything so `t` also works from Overview.
  - **Jumps**: `code_row_jump_target` (renamed from
    `code_pipeline_action_jump_target`) serves both the pipeline's Stages
    section and a run's Actions section; `Build ID` splits at `:` and jumps
    to the **project**, landing on its **Builds** section via
    `jump_landing_section` → `pending_jump_section` (builds are not
    first-class, so the project's build list is as deep as a jump goes).
    `arn_jump_target` gained `codebuild` / `codecommit` / `codeartifact`
    (jump by ARN — those `id()`s *are* the ARN), `codepipeline` (by name,
    first path segment) and `codedeploy` (`deploymentgroup:<app>/<group>` →
    the `"<app> / <group>"` **name**, since `CodeDeployGroup::id()` is an
    opaque UUID). Before this there were **no** CodeSuite ARN arms at all.
  - **Deployments** (CodeDeploy) — one row per `application / group`
    (`ListApplications`→`ListDeploymentGroups`→`BatchGetDeploymentGroups`), list
    state = last-attempted deployment status. 3-section split pane Overview /
    Targets (EC2 tag filters, ASGs, ECS services) / **Deployments** (lazy
    `ListDeployments`+`BatchGetDeployments`, keyed by group id — status, instance
    summary counts, error, rollback). Apps with zero groups don't appear.
  - **Artifacts** (CodeArtifact) — one row per repo (`ListRepositories`, all
    domains). 2-section split pane Overview / **Packages** (lazy `ListPackages`,
    each enriched with its default display version via `ListPackageVersions`,
    capped at 40 lookups / 300 packages). Repo id is the ARN (repo names repeat
    across domains); upstreams need `DescribeRepository` (out of v1 IAM).
- **WAF** — split panes for Web ACL / IP set / rule group, one lazy fetch each,
  run with the scope's wafv2 client (**CLOUDFRONT → us-east-1**). Every
  per-scope consumer (metrics, insights, sampled requests, log tail) keys off
  `App.waf_scope`, so the listed rows must always match it —
  `recreate_services` re-applies the scope like its sibling toggles (it once
  didn't: after a region/profile switch in CLOUDFRONT scope the rebuilt
  service listed regional ACLs under the CloudFront tab *and* cached them
  under the CloudFront variant key, and metrics queried `Region="Global"` in
  us-east-1 for ACLs that publish regionally). `m`/Traffic
  metrics: the `WebACL` dimension value is tried as the **ACL name first, then
  the `VisibilityConfig` metric name** (best-effort `GetWebACL`) — accounts
  publish under one or the other, so `fetch_waf_metrics` probes rather than
  guesses. **The `Region` dimension exists only for REGIONAL scope** — the
  docs' dimension table says Region is "required for all protected resource
  types *except* CloudFront distributions", so CLOUDFRONT-scope series (in
  us-east-1) have no Region dimension at all and every query (stat series +
  both SEARCH schemas) must omit it; `Region="Global"` matches nothing and
  shipped once as permanently-empty CloudFront charts while Insights (a WAF
  API, not CloudWatch) happily showed traffic. Charts are five `Rule=ALL`
  `GetMetricStatistics` series (Allowed /
  Blocked / Counted / **CAPTCHA / Challenge**; no IAM beyond what every
  metrics pane needs); the "Requests by Rule" table is a **best-effort**
  paginated `GetMetricData` per-rule SEARCH (schema
  `{AWS/WAFV2,Region,Rule,WebACL}`, minus `Region` for CLOUDFRONT, dynamic
  label `${PROP('Dim.Rule')}`;
  managed rule-group rollups + `Default_Action` included, Rule=ALL excluded) —
  a denial degrades to a `rule_note` hint, never kills the charts.
  `WafMetricsState` has an `Error` arm (only when *nothing* came back and a
  call failed) — surfaced via `error_rows`, never an eternal "Loading…".
  The same GetMetricData call also SEARCHes the label schema
  (`{AWS/WAFV2,LabelName,LabelNamespace,Region,WebACL}`, same Region rule) into a
  "Requests by Label" table (bot categories / attack signals; hidden when no
  labels fired). The web ACL pane's **Insights** section (key 6) is a lazy
  `fetch_waf_insights` (`lazy.waf_insights` keyed by ACL name): one
  `GetSampledRequests` per sampler — default action + rules by priority,
  capped `MAX_INSIGHT_SAMPLERS=10` with the overflow reported — aggregated
  (weight-scaled) into action percentages and top-10 rules / client IPs /
  countries / URIs / user agents / labels over the API's fixed 3-hour window.
  Samplers with `sampled_requests_enabled=false` are counted, not errored;
  the raw sample dump stays on the existing editor path. **`t`** (detail pane —
  the list-pane `t` is the scope toggle; a tail source must be in
  `supports_log_tail()` or the key never reaches `open_log_tail`) tails the
  ACL's `aws-waf-logs-…` CloudWatch group (async `resolve_waf_log_group` via
  `GetLoggingConfiguration`; S3/Firehose-only destinations explain instead) —
  a CLOUDFRONT-scope group lives in **us-east-1**, honoured by the
  `log_tail_region` override that `tail_logs_client()` applies to the tail,
  reseed, and search paths.
- **CloudFront** (`@cloudfront`) — four sub-tabs: **Distributions / Functions /
  Policies / OACs** (`CloudFrontView`, `JumpView::Cf`). Phase 0 fetches all
  cache / origin-request / response-headers policies (managed + custom) — they
  feed both the behaviors' **policy id→name resolution** and the Policies
  sub-tab rows (`fetch_policies`; best-effort — a permission gap warns and
  behaviors fall back to raw ids); distributions stream next, then the policy
  rows, then Functions + OACs as error-tolerant batches.
  - **Distributions** — parsed eagerly from the `DistributionSummary` (no extra
    describe), 7-section split pane Details / Origins / Behaviors / **Errors**
    (custom error responses) / Restrictions / **Invalidations** / Tags(lazy).
    Invalidations is a lazy `ListInvalidations` (newest-first, capped
    `MAX_INVALIDATIONS=20` with a "latest N of M" header; each row's paths come
    from a per-id `GetInvalidation`, capped 8 shown with "+N more"; in-progress
    rows read ⚠; `lazy.cf_invalidations` keyed by distribution id). Behaviors render as
    per-behavior blocks: policies by name (rows jump to the Policies sub-tab
    via `cf_row_jump_target`), **Lambda@Edge + CloudFront Function
    associations** (both ARNs jump — `arn_jump_target` routes
    `cloudfront:function/…` to the Functions sub-tab), legacy
    TTLs/forwarded-values, trusted key groups/signers (⚠ signed-URL gate).
    Origins add OAC (row jumps to the OACs sub-tab), Origin Shield, timeouts,
    custom-header count, and origin groups (failover); S3 origin domains (all
    endpoint generations incl. legacy dash-region + website) jump to the bucket
    via `s3_origin_domain_bucket`. Details adds TLS min protocol / SSL method +
    a ⚠ staging flag. WAF row jumps to WAF in **CLOUDFRONT scope**; metrics use
    the global namespace via `cloudwatch_us_east_1_client()` — 8 charts incl.
    BytesUploaded and the "additional monitoring" pair CacheHitRate +
    OriginLatency (empty unless additional metrics are enabled — normal, not
    an error).
  - **Functions** — `ListFunctions` over both stages (DEVELOPMENT lists all,
    LIVE marks published; unpublished renders as a ⚠ pending state). 2-section
    split pane Overview / **Code** — Code is a lazy `GetFunction`
    (`lazy.cf_function_code` keyed by name, LIVE preferred) rendering the actual
    source inline; `e` on the Code section opens it as `.js`.
  - **Policies** — one flat-detail row per policy (kind Cache / Origin Request /
    Response Headers) with the full flattened config: TTLs + cache-key params,
    forwarded headers/cookies/query strings, CORS (⚠ wildcard origin),
    security headers (HSTS/CSP/frame/referrer), custom + removed headers.
    AWS-managed policies are `is_noise()` (hide with `a`).
  - **OACs** — `ListOriginAccessControls`, flat details (signing
    behavior/protocol, origin type).
- **API Gateway** (`@apigw`) — five sub-tabs: REST v1 / HTTP+WS v2 / custom
  domains / **Usage Plans** (4) / **VPC Links** (5). REST + HTTP panes lead
  with an eager **Overview** section (endpoint type, default-endpoint
  disabled flag, policy presence, binary media/compression; HTTP adds CORS +
  WebSocket route-selection) — all inline on the list responses, no extra
  fetch. Routes/methods resolve their backend (Lambda/HTTP); REST integration
  lookups are capped at `MAX_INTEGRATION_LOOKUPS=120`. The REST pane's
  **Deployments** section (key 5) rides the same combined details fetch
  (newest-first, capped `MAX_DEPLOYMENTS=25` with a "latest N of M" header).
  Stage rows surface an active **canary** split (⚠) and the access-log
  format; `t` tails a stage's access logs. Usage plans (REST-only) are
  Overview / API Stages / Keys — Keys is a lazy `GetUsagePlanKeys` whose
  **key values are never captured** (id/name only). VPC links merge both
  control planes (v1 NLB-target links + v2 subnet/SG links, distinct id
  spaces) into one flat-detail list; a FAILED link reads red, and NLB ARNs /
  subnet / SG rows are Enter-jumpable. API ids are opaque (no prefix), so
  domain-mapping rows (`API (REST|HTTP)` keys) and usage-plan stage rows jump
  to their API via the specialized `apigw_row_jump_target` +
  `JumpView::ApiGw`; custom domains show their mutual-TLS truststore
  (`s3://` value → jumps to the bucket).
- **CloudWatch** — Alarms (metric `CwAlarm` + rule-based `CwCompositeAlarm`,
  `is_noise()` hides OK), Dashboards (lazy `GetDashboard`; `m` **charts** the
  dashboard — see below; the detail pane is a single **JSON** section, since
  the widget-summary list it used to lead with was a text stand-in for a
  dashboard nobody could see and `m` now draws the real thing — the alarm ARNs
  stay there because that jump is the one cross-reference the grid can't
  follow, `⏎` being zoom), Metrics (key 4 —
  **lazy + grafted** onto `self.resources`, not cached), Log groups
  (Streams/Filters lazy; `t` tails, `f` searches). Composite-alarm children and
  dashboard alarm-widget ARNs are `Enter`-jumpable.
  - **Cross-Account (key 5, OAM)** — Observability Access Manager sinks +
    links in one grouped tab (`OAM Sink|OAM Link`), because an account is
    normally one side or the other: the **sink** lives on the monitoring
    account, the **link** on each source account. Not a standalone
    `ServiceType` — `src/aws/services/oam.rs` holds the resources/fetches and
    the CloudWatch streaming load runs both `ListSinks` and `ListLinks` as a
    best-effort phase (each warns independently; empty results are normal and
    silent — the tab's empty state explains rather than a load warning). Sink
    pane: Details / Policy (lazy `GetSinkPolicy` — the raw JSON plus a parsed
    "who may link" summary: org ids/paths + accounts from the principal and
    condition blocks, `oam:ResourceTypes` as friendly telemetry names, and a
    `⚠` when the principal is `*` with no org condition; parse failures keep
    the raw text, summary rows just stay empty) / Attached Links (lazy
    `ListAttachedLinks`, sorted by label — the per-source-account rows show
    the account id parsed from the link ARN). Link pane: Details (sink ARN +
    monitoring-account id + shared telemetry types, all from `ListLinks` —
    zero extra fetches) / Configuration (lazy `GetLink` — label template,
    metric-namespace / log-group sharing filters, tags). Attached-link and
    sink ARNs point at *other accounts*, so nothing here is `Enter`-jumpable
    on purpose. No CloudWatch namespace of its own → no `m`.
  - **Streams (key 6)** — CloudWatch Metric Streams, the continuous-export
    centralisation path (`ListMetricStreams`; Filters section lazy via
    `GetMetricStream` — namespace include/exclude filters, delivery role,
    linked-accounts flag, extra-statistics configs). Include and exclude
    filters are mutually exclusive on the API, so the section renders
    whichever side is populated and says "everything else" for the rest.
  - **Insights (key 7)** — grouped tab (`CW Anomaly Detector|CW Insight
    Rule`). Anomaly detectors are **flat** panes (everything the API returns
    fits; the typed `SingleMetric`/`MetricMath` structs are read first, the
    deprecated top-level fields behind `#[allow(deprecated)]` as fallback —
    old-API detectors come back on those with both structs empty; the id is
    synthesised from metric/expression + dims since the API has none).
    Contributor Insights rules are a small split (Details / Definition —
    definition JSON arrives with the list, no lazy fetch; `e` opens it via
    `raw_content`).
  - **Account Policies (key 8)** — account-level **Logs** policies
    (`DescribeAccountPolicies`, one call per policy type: data protection,
    subscription filter, field index, transformer, metric extraction). The
    row id is synthesised `type:name` since names are only unique per type.
    Per-type failures are deliberately **silent** (newer types don't exist in
    every region/partition — a warning would fire everywhere); only all five
    failing warns, with a sample error. `e` opens the policy document.
  - **Charted dashboards** (`m`, `fetch_dashboard_metrics`) — the widget grid
    drawn for real, not summarised. Three things the dashboard body does that
    cost time to rediscover:
    ① `properties.metrics` abbreviates every token it can as `"."`, meaning
    "same as the line above at this position" — `parse_widget_metrics` expands
    those against the previous **metric** line (an `{"expression":…}` line has
    no tokens and must not reset the baseline). Not expanding them charts a
    namespace-less metric that returns no data, which looks exactly like a
    quiet resource.
    ② `x`/`y` are optional; a body without them is auto-flowed by the console
    left-to-right wrapping at column 24, so the parser flows them too —
    defaulting to (0,0) stacks every widget on top of itself.
    ③ A widget's own `period` can be finer than the selected range supports (a
    1-minute widget over 7d), which CloudWatch rejects outright — the period is
    raised to the range's floor.
    ④ Metric-math ids (`{"id":"m1"}`, referenced by `{"expression":"m1+m2"}`)
    are only unique **within** a widget — every widget starts over at `m1` —
    while `GetMetricData` needs them unique per request. `dashboard_query_id`
    namespaces them by widget index and `rewrite_expression` rewrites the
    expression text to match, skipping identifiers inside single-quoted strings
    (a `SEARCH('… MetricName="Errors"', 'Sum')` argument is data, not a
    reference). One bad expression fails the *whole* call, so a region that
    errors is retried once with the expression queries dropped and sets
    `math_error` — better a dashboard missing its math than a blank one.
    ⑤ A `SEARCH()` returns **many** series under one query id, separated only by
    label, which is why `series` maps id → `Vec<CwSeries>`. It also **requires
    an explicit `Period` on the query** — without one the call fails
    `Period is required when using SEARCH`, and because validation is
    request-wide that one widget blanks every other widget in its region.
    `build_dashboard_queries` therefore sets a period on *every* expression
    query (plain math ignores it). AWS's own Network Firewall dashboard is
    built entirely from SEARCH expressions, which is how this surfaced — and
    why it isn't caught by the all-expressions-dropped fallback: with nothing
    but expressions there is no plain query left to retry with.
    Note the query cap counts *queries*, not returned metrics: one SEARCH can
    match hundreds, and `GetMetricData` bills for each.
    ⑥ Series labels can carry **dynamic-label** syntax
    (`${PROP('Dim.AvailabilityZone')}`, `${MAX}`, …). `GetMetricData` normally
    substitutes these and returns finished text, so the renderer prefers the
    **API's** label over the widget's — preferring the widget's is what showed
    raw `${PROP(…)}` on single-series widgets — with
    `resolve_dynamic_label` as a local fallback (a token it can't answer is
    dropped, never shown raw).
    ⑦ Body-level `start` (`"-PT3H"`) is the window the author views their own
    dashboard through and becomes the opening range, resolved to the *smallest
    covering* preset (rounding down would show less than they needed).
    Absolute timestamps and month/year designators yield nothing — the presets
    are all relative to now. It's adopted **inside** the fetch, because the
    range isn't known until the body has been read and doing it in the caller
    would cost a second round trip; `[`/`]` clears the flag so a refresh can't
    snap back. `periodOverride: "inherit"` discards widget periods entirely.
    ⑧ `stacked` widgets accumulate their series (the top line is the total);
    series of differing length fall back to unstacked, since there's no honest
    way to pair points that don't share a timestamp. `yAxis: "right"` metrics
    are rescaled onto the left axis's range (ratatui charts have one y-axis) —
    marked `›` in the legend with their true scale in the title, which is how
    you'd read a dual-axis chart anyway.
    ⑨ Text widgets are written in CloudWatch's markdown, which adds a **button**
    extension (`[button:primary:Label](url)`) on top of ordinary links.
    `flatten_md` renders both as text (`[ Label ↗ ]` / `Label ↗`) and drops the
    URL: nothing in the pane can follow a link and a console URL is far wider
    than the widget. The Raw section (and `e`) still has the untouched body.
    Widgets can pin their own `region`, so queries are grouped and each group
    runs against `cloudwatch_client_for(region)`; a region that fails is named
    in the header and the rest still render. Cross-**account** (`accountId` on a
    metric line) is not supported. The series cap is `MAX_DASHBOARD_QUERIES`,
    because `GetMetricData` bills per metric requested.
    `Tab`/click move a widget cursor, `⏎`/double-click zoom one widget to the
    whole pane; click targets are recorded by the renderer into
    `App.cw_dashboard_regions`, and the handler runs before `handle_mouse`'s
    overlay gate.
    Rendering is `metrics_overlay::render_cw_dashboard_metrics_overlay`:
    `timeSeries` → chart, `singleValue` → big-number tiles, `gauge` → a bar
    against `yAxis.left`, `bar`/`pie` → horizontal bars (a pie has no honest
    terminal form and the share it conveys reads fine as bars), `text` →
    unframed markdown, `alarm` → state chips from a best-effort
    `DescribeAlarms` grouped by the ARN's region (unresolved alarms still list
    by name). Logs Insights and `explorer`/`custom` widgets keep their box and
    say what they are.
- **CloudFormation Exports** (key 3) — Imports lazy; AWS *errors* (not empty)
  when nothing imports, mapped to a clean "not imported".
- **CloudFormation Deleted stacks** (Deleted sub-tab, key 4) —
  `DescribeStacks` **never returns deleted stacks**; the tab comes from a
  separate `ListStacks` phase filtered to `DELETE_COMPLETE` (retained ~90
  days, sorted newest-deletion-first in the service file since the API order
  is 90 days of CI churn). Rows are thin `StackSummary` stubs (`deleted:
  true` on `CfnStack` — no tags/outputs/parameters), with a reduced
  4-section pane (`CFN_DELETED_STACK_SECTIONS`): Drift/Changes only error on
  a deleted stack, and Outputs/Parameters/Tags are gone from the API.
  **Every CFN stack lazy fetch is keyed and addressed by the stack ID
  (ARN), never the name** — deleted stacks can only be fetched by ID, and
  delete → recreate → delete yields several stacks sharing one name (a
  name-keyed LazyMap collides). Resources/Events/Template still work by ID
  post-delete; a `DELETE_SKIPPED` resource status is the "left behind"
  signal (`decorate_cfn_status` renders it `⚠ … (retained)`), and its
  Physical ID row jumps to the orphaned live resource via
  `cfn_resource_jump_target` — which checks the section against the
  **deleted** enum when `stack.deleted` (the two descriptors differ).
  The console URL needs `&filteringStatus=deleted` to resolve a deleted
  stack ID.
- **CloudFormation Events cap** — the Events section fetches at most
  `CFN_EVENTS_CAP` (500) events; the API retains the full 90-day history and
  returns newest-first, so truncation drops the oldest and the renderer
  appends a `· showing the most recent N` note when the cap is hit. Drift
  results only exist after a console-side `DetectStackDrift` run (a mutation
  neboto never calls) — the empty Drift section distinguishes NOT_CHECKED
  from a genuinely clean IN_SYNC.
- **CloudFormation change sets (Changes section)** — `DescribeChangeSet` runs
  with **`IncludePropertyValues=true`**: each change renders an action line
  (`+`/`−`/`~`/`↷`, replacement + `PolicyAction` markers), a "Physical ID"
  row for the live resource being touched (that exact label is load-bearing —
  `cfn_resource_jump_target` keys on it, Changes-section arm, so the row
  Enter-jumps to the resource; routing = `cfn_type_jump_target`'s explicit
  table, shared with the Resources section, then
  `cfn_service_fallback_jump_target` maps any other `AWS::<svc>::<kind>` by
  its service token to a query-style landing — only Custom:: and unbrowsed
  services stay non-jumpable), plus per-property diff rows (`before → after`, values `clip_inline`d to 40 chars — property
  values can be whole policies), a `Dynamic` evaluation reads "resolved at
  execution", `RequiresRecreation != Never` gets a `⚠ recreation:` prefix,
  and `ChangeSource`/`CausingEntity` append "via …". The full before/after
  resource JSON (`BeforeContext`/`AfterContext` — the console's "JSON
  changes") is **not rendered inline**: `e` on the Changes section opens
  `change_sets_json` (contexts embedded as parsed JSON, unit-tested; falls
  back to the raw string per-context). Validations = `DescribeChangeSetHooks`
  (planned hook invocations) + `ListHookResults(CHANGE_SET, id)` (statuses
  once run) — **both best-effort**; accounts without hooks or the IAM actions
  silently render without those rows, so don't "fix" their absence with a
  load warning.
- **CloudFormation progress rollup** (`f` on the Events section, both
  descriptors) — flips the body from the chronological stream to a
  per-resource "what's left" rollup (`cfn_progress_rollup`, unit-tested):
  latest status per logical ID, grouped In progress (longest-running first,
  with elapsed) / Not started / Failed (reason inline) / Complete, plus an
  Operation + counts header. The **operation window** is bounded walking
  newest-first to the stack-level `*_IN_PROGRESS` event whose reason contains
  `"User Initiated"` (rollback phases don't carry it, so a failed create and
  its rollback stay one window) or to the previous op's terminal stack event;
  no marker found ⇒ a `window truncated` note, never silently mixing
  operations. **Not started** = template resource set minus event-seen IDs
  (`parse_template_resource_ids` → `CfnTemplateResource`: serde_json for
  JSON, an indent scan of the top-level `Resources:` block for YAML — no
  YAML dependency; `Type:`/`Condition:` are only read at an entry's
  first-child indent so `Type: forward` inside Properties can't shadow
  them; unparseable ⇒ rollup renders without the group + a note), and
  **only while a CREATE/DELETE op is `*_IN_PROGRESS` over a complete
  window** — an update touches only changed resources ("no events" ≠
  "pending", noted in the header instead), a settled op has nothing left
  (`Condition:`-false resources never emit events and read as eternally
  "not started" otherwise — shipped once against a finished stack's
  condition-gated IAM policies), and a truncated window makes the seen-set
  unreliable. `Condition:`-carrying entries that do appear mid-create are
  annotated "conditional — skipped if its condition is false" (conditions
  can't be evaluated client-side). While the
  op is `*_IN_PROGRESS` and the section is active, `cfn_progress_tick`
  (250ms Tick, 10s gate) refetches events via **`retrigger_lazy`** — the
  contains-guard-skipping, no-Loading-flash variant of `trigger_lazy` added
  for exactly this (the pane keeps the stale rollup until the fresh result
  overwrites it; epoch stamping still drops cross-credential strays). The
  toggle (`App.cfn_events_progress`) is sticky like the flat view; the
  footer advertises `f progress` / `f events` on the Events section.
- **IAM** — the `List*` APIs **omit attributes** even though they exist on the
  returned type (documented AWS behavior, not an SDK gap): `ListRoles` drops
  Tags + PermissionsBoundary + RoleLastUsed, `ListUsers` drops Tags +
  PermissionsBoundary, `ListPolicies` drops Tags. So every Tags section (and
  the role's Last Activity / both boundary rows) renders from the **lazy**
  detail bundle (`GetRole` / `GetUser` / `ListPolicyTags`), never from
  `resource.tags()` — and the `tag:` search filter can't match IAM
  roles/users/policies, since eager per-entity Gets would be N extra calls per
  load. Don't "fix" a blank Tags section by reading the list output; it's
  empty by API design. Groups genuinely have no tags at all.
- **IAM Identity Providers** (Providers sub-tab, key 6) — SAML + OIDC in one
  list; both list calls are unpaginated, and the OIDC one returns **only
  ARNs**, so everything else rides the lazy `Get*Provider` bundle. A
  provider's `name` is everything after `…provider/` in the ARN — for OIDC
  that's the full issuer host **+ path** (EKS issuers are
  `…/id/HEX`), which is why the `:saml-provider/`/`:oidc-provider/` jump
  classifiers don't path-strip like `:role/` does. The SAML metadata document
  is fetched but only its **size** is kept.
- **IAM Account Settings** (Account sub-tab, key 7) — one synthetic row
  (`iam-account-settings`) built eagerly from `GetAccountSummary` +
  `GetAccountPasswordPolicy` + `ListAccountAliases`; call failures render
  inline in the pane sections, never as load warnings. Two things people
  trip on: `GetAccountPasswordPolicy` returns **NoSuchEntity when the account
  uses the AWS default policy** — that's the normal case, rendered as
  "default applies", not an error; and the row's list **state colour is a
  root-credential report** (red = root access keys exist, yellow = root MFA
  off), driven off the summary map's `AccountAccessKeysPresent` /
  `AccountMFAEnabled`.
- **IAM Access Analyzer** (Analyzer sub-tab, key 5) — an **extension** of the
  IAM service (second client + a late streaming step), **not** a new
  `ServiceType`; fully error-tolerant so no-analyzer / permission gaps never
  break the core IAM load. v1 external-access findings only (the phase skips
  unused-access analyzers, whose `ListFindings` errors; `ListFindingsV2`
  would cover both kinds if that ever gets built).
- **Security groups** — EC2 `SecurityGroup` and VPC `VpcSecurityGroup` share one
  Inbound/Outbound/Used-By/Tags split pane. "Used By" (ENIs) is lazy.
- **VPC extras** (beyond the core subnet/SG/NACL/endpoint tabs) — tab 3
  **Routing** and tab 4 **Gateways** are *grouped* tabs (pipe filters:
  route tables + prefix lists; IGWs + NAT GWs + **egress-only IGWs**); tab 5
  **Peering**; tab 9 **VPN** groups connections + **VGWs** + **customer
  gateways** (CGW rows stream from the same describe that feeds the VPN
  ip/asn enrichment map); tab `0` **DHCP options sets**. All stream as late
  best-effort phases, and **every phase with a paginated API paginates** (the
  local `stream_phase!` macro streams each page as its own batch — a bare
  `.send()` silently truncated >1000-item accounts). EO-IGW / VGW / CGW are
  flat `details()` (complete as
  flat); `eigw-`/`vgw-`/`cgw-` ids are Enter-jumpable. The VPC pane has a
  lazy **Flow Logs** section (key 4, before Tags — `DescribeFlowLogs`
  filtered `resource-id`=vpc, `lazy.vpc_flow_logs` keyed by VPC id): status /
  traffic type / destination per log, ⚠ delivery errors, and an explicit
  "traffic is not being captured" empty state (VPC-level logs only). The
  Overview's "DNS / DHCP" group leads with the enableDnsSupport /
  enableDnsHostnames flags — a lazy per-VPC `DescribeVpcAttribute` pair
  (`lazy.vpc_dns_attrs`; triggered on drill-in too, since Overview is the
  default section).
  - **DHCP options sets** — eager Overview / VPCs / Tags split pane; the VPCs
    section filters sibling `Vpc` rows by `dhcp_options_id` (no fetch), and the
    VPC pane's Overview folds the resolved options in as a "DNS / DHCP" group
    (`pretty_dhcp_key` labels). `dopt-` ids are Enter-jumpable.
  - **Peering connections** — eager Overview / Tags pane: status (+message,
    expiry for pending), per-side Requester/Accepter blocks (VPC id jumpable,
    all CIDRs deduped across `cidr_block`+`cidr_block_set`, owner, region, DNS
    resolution flags). Cross-account/region sides only resolve locally.
    `pcx-` ids (e.g. route-table targets) are Enter-jumpable.
  - **Managed prefix lists** — AWS-managed lists (`owner == "AWS"`) are
    `is_noise()`. Overview / **Entries** (the one lazy VPC section:
    `GetManagedPrefixListEntries` paginated, `lazy.pl_entries` keyed by pl id) /
    Tags. `pl-` ids (route destinations, SG rules) jump to the Routing tab.
- **Compute Optimizer lens** (B7) — a lazy **Optimizer** section on five panes:
  EC2 instance (key 8), EBS volume (5), Lambda (6), ASG (5), ECS service (7).
  Not a `ServiceType`: `aws/services/computeoptimizer.rs` flattens the per-ARN
  `Get*Recommendations` into one `OptimizerRec`; `details_pane::optimizer_lines`
  is the shared row-builder. `App.optimizer_recs` is keyed by ARN (region+account
  are inside the ARN, so no staleness across switches); enrollment is a one-shot
  `GetEnrollmentStatus` (`App.co_enrollment`, reset on region/profile switch) —
  unenrolled accounts get a hint row and no per-resource calls. EC2/EBS ARNs are
  built from region + `account_id`; `AsgGroup.arn` is captured from the SDK
  (contains a UUID). "No recommendation" (`Loaded(None)`) is a normal state, not
  an error.
- **EC2 instance Console section** (key 6, `GetConsoleOutput`) — the system
  log the console shows under *Get system log*; lazy `lazy.ec2_instance_console`
  keyed by instance id, same shape as User Data. The buffer comes back
  **base64** and is decoded + sanitised (`sanitize_console_line`: ANSI
  escapes, C0 bytes and `\r` stripped, tabs expanded) so a raw kernel line
  can't break the pane. The call asks for `latest=true` (most recent 64 KiB,
  documented as Nitro-only) and **retries without it** on
  `UnsupportedOperation` / `InvalidParameter*` rather than failing the
  section; without `latest` AWS returns the buffer posted after the last
  state transition, which is what the console shows. `Loaded(None)` (no
  `output` field) is normal for minutes after a launch/reboot — the section
  says so instead of erroring. Free, control-plane, never touches the guest,
  hence a default on-enter hook. `e` on the Console section opens the raw
  log (`.log`, one `#` header line) and on User Data the raw script (`.sh` /
  `.yaml` / `.txt` sniffed from line 1, no header — `#!` and `#cloud-config`
  must stay first) via `editor_override_content`, not the snapshot JSON.
  `GetConsoleScreenshot` (a JPEG) is deliberately not offered.
- **EBS / AMIs / Snapshots / Launch Templates** — EC2 sub-tabs (keys 3, 5–7),
  **self-owned only**. Cross-jumps: `snap-`→Snapshots, `vol-`→EBS, `i-`→instance.
- **ENIs** — EC2 sub-tab (`Ec2View::NetworkInterfaces`, key 4): paginated
  `DescribeNetworkInterfaces` (ENIs are the type most likely to exceed one
  1000-item page). The reverse index for "whose IP is this?" — every private
  IPv4 (secondaries included; EKS/Lambda/ELB ENIs carry many), IPv6, private
  DNS, SG ids/names, and the requester id are in `search_text()`. Split pane
  Overview (requester id = the what-made-this signal for managed ENIs) /
  Addresses / Attachment / Tags; VPC / subnet / SG / instance rows jump.
- **Elastic IPs** — EC2 sub-tab (key 8), `DescribeAddresses` (no paginator).
  Flat `details()`; **unassociated** EIPs render with a yellow state dot
  (`state()` → `Pending`) so billable idle spend pops. `i-`/`eni-` rows jump.
  (EC2's tab bar uses the responsive `subtab_bar::render_subtab_bar` helper.)
- **Identity Center** — five sub-tabs: Instance / Permission Sets / Users /
  Groups / **Applications**. Every type has a split pane. The instance pane is
  Overview (eager `DescribeInstance` enrichment — created date, KMS encryption;
  MFA/session/identity-source have **no public read API**, noted inline) /
  ABAC (lazy, `Ok(None)` = not configured) / Trusted Issuers (lazy, N+1
  describe) / Summary (sibling counts, zero-fetch). Users & groups carry an
  **Access** section — `ListAccountAssignmentsForPrincipal`, and for users
  *effective* access (direct + per-group, annotated "Via group X"); PS names
  resolve from the loaded sibling list, account ids enrich to `id (name)` via a
  one-shot best-effort `organizations:ListAccounts` (`ic_account_names`,
  profile-scoped). Users/groups ingest full identity-store fields (status →
  disabled reads unavailable, title, external IDs = the SCIM/identity-source
  signal, created/updated). Applications load eagerly from `ListApplications`
  (no N+1); Assignments is lazy (+ `GetApplicationAssignmentConfiguration`
  assignment-required flag). `ic_jump_target` (a specialized classifier) wires
  `Enter` across the IC graph (members↔users↔groups, assignments→principal/
  account, access→permission set/account, app assignments→principal).
- **Organizations** (`@org`, `organizations.rs`) — sub-tabs Overview / Accounts
  / **OUs** / Policies / Delegated / Trusted, streamed as sequential phases.
  The **OU pane** (`OrgUnit`) is Overview / **Children** / **Policies** /
  **Tags**, all four fed by **one** lazy bundle (`fetch_org_ou_details`,
  `lazy.org_ou_details` keyed by OU id — the Route53-Profiles pattern): child
  OUs (`ListOrganizationalUnitsForParent`) + accounts parked directly in the
  OU (`ListAccountsForParent`), the effective policy set (below), and tags
  (`ListTagsForResource` — OUs carry no eager tags, so `Resource::tags()`
  stays empty and the section reads the bundle). Each part records its own
  error string so one missing permission doesn't blank the other sections.
  `OrgUnit.parent_id` (the `ou-…`/`r-…` above it) feeds a jumpable Overview
  "Parent" row. `OrgUnit::name()` is the **full path**
  (`Root/Workloads/NonProd`, precomputed in `new()` since `name()` returns
  `&str`), not the bare OU name: sibling names are unique but names repeat
  across branches, and name-ascending `z` sort then lays the list out as the
  tree. The pane header still shows the short `ou_name`.
  **Effective policies** — `ListPoliciesForTarget` returns only what's
  attached **directly** to the one target you name, but SCPs/RCPs are
  inherited down the tree and most orgs attach them at an OU, so a bare
  per-target call showed an account no SCPs at all. The account and OU panes
  therefore share one walk (`org_roots_info` → `ancestor_chain` →
  `effective_policies`, all in organizations.rs): `ListParents` +
  `DescribeOrganizationalUnit` climb to the root, then every enabled policy
  type is listed against the target **and each ancestor**, each result
  tagged with its attachment point (`OrgAttachedPolicy.source`, `None` =
  direct). Both panes render it through `details_pane::org_policy_rows` —
  grouped by type, direct first then outward, each row annotated
  `· inherited from <OU> (ou-…)`. The one `ListRoots` call does double duty:
  the enabled policy types (the API demands a type filter and rejects
  disabled ones — probing beats eating an error per type; falls back to
  trying all seven) **and** the root id→name map, since `ListParents`
  reports a root's id but no name and `DescribeOrganizationalUnit` won't
  accept a root id. A type that simply isn't listable is skipped silently;
  only a denial is reported. **Do NOT collapse the policy rows' shape**:
  `("  {name}", "{policy_id}")` is what `org_account_scps_row_target` (the
  `e`/`v` document fetch) and `org_row_jump_target` (⏎) both key on — and
  that fetch is **section-gated to Policies**, because the OU Path section's
  ancestor rows have the identical shape while an OU id is not a policy id.
  **Jumps**: Organizations ids carry no ARN
  and a bare account id is just a number, so `org_row_jump_target` classifies
  by the *value's* shape (`ou-`/`r-` → OUs, `p-` → Policies, 12 digits →
  Accounts), gated on the service being current **and** on the row being a
  list item (`"  name"` → id) or an explicit "Parent" row — otherwise an OU's
  own "OU ID" row would link to itself. It also makes the account pane's SCP
  rows and the policy pane's attached-target rows Enter-jumpable for free.
- **Organizations member-account switch** — `s` on an `OrgAccount` row
  assumes a configured role (`org_access_roles` list, else the single
  `org_access_role`, default `OrganizationAccountAccessRole`; Control Tower
  wants `AWSControlTowerExecution`) in that account and re-points the whole
  app at it, exactly like a profile switch (`AwsClients::assume_org_role` →
  `App::switch_org_role` → the shared `reset_account_scoped_state` +
  `install_new_clients` helpers — new account-scoped lazy state MUST be
  cleared in the former, which `switch_profile` also uses). With **several**
  configured roles `s` opens the role picker (`org_role_selector.rs`,
  last-used preselected); with one it fires directly. Every session is
  scoped down with the AWS-managed `ReadOnlyAccess` **session policy**
  (`policy_arns` on AssumeRole), so assumed credentials can never mutate.
  `ReadOnlyAccess` has gaps, though — it grants no `controltower:List*`, so
  an assumed session failed every Control Tower phase with "because no
  session policy allows …" while the underlying role was admin. The fix is
  `READONLY_SUPPLEMENT_POLICY`, an **inline** session policy passed
  alongside the managed ARN (session policies union with each other, then
  intersect with the role's identity policy — so a read-verb supplement
  widens coverage without ever making the session writable). Both call sites
  must stay in step: the `AssumeRoleProvider` in `build` and the eager STS
  probe in `assume_org_role` — a probe that passes different policies than
  the real session validates the wrong thing. The
  AssumeRole is validated with a direct STS probe *before* any state is torn
  down (invited accounts lack the role — that error must leave the current
  view intact); credentials auto-refresh via `AssumeRoleProvider`. Region
  switches keep the assumed role; profile switches drop it. **Landing**: a
  successful assume lands on the config `default_service` (Organizations is
  management-account-only inside a member account); exiting lands back on
  the Organizations account list (the assume → inspect → exit → next-account
  loop) — both via `prepare_service_view` (switch_service minus the load,
  which must run on the *new* clients via `install_new_clients`). While
  assumed: `⇄ name (id)` badge in the tab bar, and the `P` selector leads
  with a synthetic `EXIT_ASSUMED_ROLE_ENTRY` row that fires
  `Event::OrgRoleExitRequested`. The `P` selector also always carries an
  `ASSUME_BY_ID_ENTRY` row → the same modal in **Manual** mode
  (`OrgRoleModalMode` — type a 12-digit account id + pick a role), which
  needs no `organizations:ListAccounts` (CI-account → deploy-target hops);
  a manual target has an empty `account_name` (badge/toast fall back to the
  id). The service-tab bar never drops the right-side badges: they degrade
  field-by-field (hint → account → long assumed form → profile) and the
  service chips window around the active one with `‹`/`›` markers
  (`service_tabs::visible_window`, same algorithm as `subtab_bar`). A config
  file that fails to parse no longer silently defaults everything —
  `Config.load_warning` surfaces it in the status bar at startup.
- **CloudTrail** (`@cloudtrail`) — sub-tabs **Events / Trails / Insights**.
  Events: JSON parsed **eagerly** (no extra API calls); `is_noise()` returns
  `read_only` so `a` hides reads (noise is **shown by default** app-wide). `f`
  (Events view; also from an event's detail pane) opens the **server-side
  event filter** (`CtEventQuery` — one `LookupEvents` attribute Event name /
  User name / Resource name / Resource type / Event source / Access key /
  Event ID + a 1h–90d range preset; the API accepts at most **one** attribute
  per call, hence the single-pick modal). On an event's detail pane `f` is a
  **pivot**: `ct_pivot_seed` classifies the row under the cursor (user / event
  name / source / access key / resource — ARNs reduced to the trailing
  segment) and opens the modal pre-filled in the value phase, so ⏎ applies
  "everything this user did" style queries; applying returns focus to the
  Events list. Applied via `apply_ct_query` (rebuild + variant-cache per
  `ct_query.variant()`); `ct_tabs.rs` shows the active query chip. List stays
  capped at 500 events per query. **Trails** stream as a best-effort phase-1
  batch (`DescribeTrails` + N+1 `GetTrailStatus`/`GetEventSelectors`, failures
  land per-row or as a load warning — never break the events view): 3-section
  eager split pane Overview (flags, destinations — `s3://` row jumps) /
  **Status** (logging on/off + per-leg delivery errors; a stopped trail reads
  red, a failing delivery leg yellow) / Selectors (basic + advanced,
  pre-flattened in `from_sdk`). `t` on a trail tails its CW Logs group — the
  group lives in the trail's **home region**, honoured via `log_tail_region`.
  **Insights** (`CtInsightEvent`, phase-2 batch, capped 200): `LookupEvents`
  with `EventCategory=insight` over the query's time range — anomaly type,
  Start/End state (Start reads yellow), baseline vs anomalous call rate,
  deviation multiple. Flat `details()`, raw JSON on `e`.
  `InsightNotEnabledException` is a normal empty state (skipped silently —
  the empty Insights tab hints the feature is off); other failures warn.
  **`state()` is overloaded on both event types** (error→red, write/read on
  events; ongoing/closed on insights), so they override
  `Resource::state_label()` — the trait hook that supplies the *word* shown
  in the wide list column, the `F` filter chips, and exports (the dot color
  still comes from `state()`). Events read `error`/`write`/`read` (matching
  the split-pane header), insights `ongoing`/`closed` — never the lifecycle
  words `running`/`available`, which mean nothing on an API event. This is
  now the rule everywhere, not a CloudTrail special case: every type whose
  `state()` mapping changes the native word overrides `state_label()` (see
  the `F` entry in CLAUDE.md's navigation keymap; Cost's compact-layout
  exemption in `resource_list.rs` predates the hook).
- **GuardDuty** (`@guardduty`, `guardduty.rs`) — sub-tabs **Findings /
  Summary / Coverage / Filters / Lists / Accounts / Malware / Detectors**,
  `t` cycles a variant-cached severity scope
  (`GdSeverityScope`, a server-side `severity >= N` criterion). The whole
  `Finding` is in memory, so **every finding section is eager** — no lazy
  store, no `*State` maps. **Archived findings are deliberately loaded** (the
  `service.archived=false` criterion was removed): a suppression rule quietly
  hiding a real finding is exactly what you need to see, so they're
  `is_noise()` instead and `a` folds them away. That makes the per-detector
  `MAX_FINDINGS=1000` cap load-bearing — a suppression rule can leave tens of
  thousands behind `ListFindings` — and hitting it warns.
  `GdFinding` sections: **Details** (+ the finding type decomposed by
  `decompose_finding_type` into ThreatPurpose / ResourceAffected /
  ThreatFamily / DetectionMechanism / Artifact — the type string is a
  structured identifier, not a name; plus `resourceRole` TARGET-vs-ACTOR,
  feature, analyst feedback, `additionalInfo`, and the threat-intel list
  matches from `evidence`) / Resource / Actor / **Sequence** / **Runtime** /
  Remediation. `extract_resource` flattens **every** shape the finding
  carries, not the first — an EKS runtime finding populates cluster *and*
  Kubernetes workload *and* container, a malware finding instance *and*
  scanned volumes; early-returning on the first match threw the rest away.
  **Sequence** renders `service.detection.sequence` (Extended Threat
  Detection): the signals timeline is the payload — a sequence stitches
  individually-unremarkable signals into one narrative — over actors,
  endpoints and indicators, each capped `MAX_SEQUENCE_ITEMS=25`. **Runtime**
  renders `service.runtimeDetails`: process, ancestry (root-first, capped
  `MAX_LINEAGE=20`) and the activity-specific context block, whose ~27 fields
  are pushed only when present because GuardDuty fills only the ones relevant
  to the finding type. Both sections render an explanatory empty state rather
  than being conditional (the descriptor is static per type). `search_text()`
  is a **precomputed** `search_blob` — every id, IP and domain from every
  extracted row — so "who touched 1.2.3.4" is a plain fuzzy search;
  precomputed because `search_text` runs per resource per keystroke.
  `trail_lookup_keys()` returns the affected resource before the finding id
  (CloudTrail indexes by resource, and knows nothing about a GuardDuty id).
  **Summary** (tab 2, `GdOverview`) is one synthetic row per detector —
  the `ShOverview` precedent — with a five-section split pane (Overview /
  Finding Types / Resources / Accounts / Coverage). Built from
  `GetFindingsStatistics` grouped four ways (SEVERITY unfiltered, plus
  FINDING_TYPE / RESOURCE / ACCOUNT ordered DESC and capped
  `MAX_TOP_STATS=10`) **rather than from the loaded finding list**, which is
  bounded by the active severity scope and by `MAX_FINDINGS` — showing the
  picture those bounds hide is the entire point of the tab, so it passes no
  `finding_criteria`. `count_by_severity` is only populated by the
  deprecated `findingStatisticTypes` form, so severity counts are summed
  from `grouped_by_severity` and bucketed with the same `severity_label`
  thresholds the rows use. Each dimension is independent and best-effort:
  a denied call records a prefixed message in `errors` and only the section
  that owns that prefix renders it. Also folds in the coverage rollup and
  `GetRemainingFreeTrialDays` (expired features dropped, not listed as
  zeroes). Phase-ordered **before** the findings pagination — five small
  non-paginated calls, so the default tab is delayed by a fraction of a
  second and the Summary is ready the moment you press 2.
  **Coverage** (tab 3, `GdCoverage`, `ListCoverage` per detector capped
  `MAX_COVERAGE=2000`) answers "is GuardDuty actually watching this host":
  one row per EC2 instance / ECS cluster / EKS cluster with status, the
  `issue` text, agent/add-on version and covered-vs-compatible node counts;
  **`is_noise()` = HEALTHY** so `a` leaves exactly the gaps. Flat `details()`
  (complete as flat). `GetCoverageStatistics` also feeds a "Runtime Coverage"
  rollup on the detector's Overview.
  **Filters** (tab 4, `GdFilter`, `ListFilters` → N+1 `GetFilter`; AWS's
  100-per-detector quota bounds it, so no cap): Overview / Criteria / Tags,
  all eager. The tab exists mostly for the `ARCHIVE` half — a **suppression
  rule** is the usual reason a finding you expect isn't in the list, and
  nothing else in the app can tell you one exists — so a suppression rule
  reads yellow (`state()` → Pending) while a `NOOP` saved filter gets no dot.
  `condition_summary` flattens each `Condition`, **joining** every populated
  operator (a range is `greaterThan` + `lessThan` on one condition, so
  first-match-wins would drop half of it) and falling back to the deprecated
  `eq`/`gte`/… aliases only when the current fields are empty — filters
  created through older API versions still come back on those.
  **Lists** (tab 5, `GdList`) unifies **four** APIs into one row type (the
  RDS-snapshot pattern): trusted IP sets, threat IP sets, and their newer
  trusted/threat **entity** set equivalents. Flat `details()` (complete as
  flat). The classic two warn on failure; the two entity-set APIs are
  **skipped silently**, because they aren't available in every region and a
  permanent "not supported here" warning on every load would be noise (the
  CloudTrail `InsightNotEnabledException` precedent). A trusted list
  suppresses findings outright, which is why it sits beside Filters rather
  than under the detector.
  **Accounts** (tab 6, `GdMember`, `ListMembers` with
  `only_associated("false")` so disabled/removed accounts still appear,
  capped `MAX_MEMBERS=1000`, then `GetMemberDetectors` in batches of
  `MEMBER_DETECTOR_CHUNK=50` — the API takes 50 ids per call, so feature
  enrichment costs one call per 50 members, not one each): split pane
  Overview / Features, sorted worst-first, `is_noise()` = Enabled with every
  feature on. A member that isn't `Enabled` isn't monitored at all, so it
  reads **red**; partially-featured reads yellow. **Every failure in this
  phase is silent** — from a standalone account these calls are *expected*
  to fail, and a permanent "you're not the GuardDuty administrator" warning
  on every load everywhere else would be noise — so the explanation lives in
  `resource_list.rs`'s per-tab empty state instead (the CloudTrail-Insights
  precedent). The account's own org posture (administrator, delegated admin,
  member auto-enable, feature auto-enable, member-limit-reached) is three
  more best-effort calls folded into the **detector's** Overview as an
  "Organization" group — the FMS `fms_admin` pattern — rather than becoming
  another resource type, since it's scalars and not a list.
  **Malware** (tab 7) is a **grouped tab** (pipe filter, the VPC-Routing
  precedent): `GdMalwareScan` (`DescribeMalwareScans` sorted
  `scanStartTime` DESC, capped `MAX_MALWARE_SCANS=200`; split pane Overview /
  Volumes, `is_noise()` = COMPLETED and not INFECTED, INFECTED reads red) and
  `GdMalwarePlan` (`ListMalwareProtectionPlans` — **no fluent paginator**,
  hand-rolled `next_page_token` loop — → `GetMalwareProtectionPlan`; flat
  `details()`) are the EC2-volume and S3-object halves of one feature and
  neither fills a tab alone. A scan's `name()` is the scanned **instance id**
  (the ARN tail), not the scan UUID, and its Overview surfaces the finding
  that triggered it as an Enter-jumpable row. Both are **silent on failure**:
  Malware Protection is off in most accounts and neither API distinguishes
  "not enabled" from a permission gap.
  Both coverage calls and the findings
  phases warn rather than erroring — a mid-stream `ResourceLoadError` clears
  `loading` and drops every batch queued behind it. No CloudWatch namespace →
  no `m`.
- **Security Hub CSPM** (`@sh`, `security_hub.rs`) — sub-tabs **Overview /
  Findings / Controls / Standards / Insights / Automations /
  Integrations / Configuration / Accounts** (nine — digits `1`–`9`, no
  tenth-tab `0`), `t` cycles a variant-cached
  severity scope (`ShSeverityScope`, a server-side `SeverityLabel` filter).
  Phases are FMS-shaped: each warns and keeps going, and only a load where
  **nothing** streamed goes fatal (with the first real error, so
  "Security Hub isn't enabled in this region" still reaches the status bar) —
  before this a `GetFindings` denial sent `ResourceLoadError` and took
  standards and insights down with it.
  **Findings** — the whole ASFF blob is in memory (`GetFindings` carries full
  detail; there is no per-finding Get), so **every section is eager** except
  History: Details / Resources / Compliance / Vulns / Context / Remediation /
  **History**. The severity-ranked query no longer filters `WorkflowStatus`
  to NEW/NOTIFIED — an automation rule quietly **suppressing** a real finding
  is exactly what you need to see, so suppressed/resolved (and passing
  control findings) load and are `is_noise()` instead, which makes the
  `MAX_FINDINGS=1000` cap load-bearing (hitting it warns). **Compliance** is
  the payoff section: `statusReasons` (reason code + description — *why* the
  control failed, the most useful field the old flat view dropped),
  `associatedStandards`, and the `securityControlParameters` the evaluation
  actually ran with. **Resources** renders the ASFF envelope per resource —
  id (key-value, so the generic classifier makes it Enter-jumpable), region,
  partition, role, application, tags, and the `Details.Other` bag; the **74
  typed `ResourceDetails` members are deliberately not flattened** (that's a
  per-type renderer each — `e` opens the JSON when you need the raw block).
  **Vulns** flattens `vulnerabilities` (CVE, EPSS, CVSS vectors, fix/exploit
  availability, vulnerable packages) — how Inspector arrives here.
  **Context** covers the shapes a *non*-control finding carries: network,
  process, threats, threat-intel indicators, action, malware, patch summary.
  `search_text()` is a **precomputed** `search_blob` (every resource id, ARN
  and CVE) — precomputed because `search_text` runs per resource per
  keystroke; `trail_lookup_keys()` returns the affected resources before the
  finding id (CloudTrail indexes by resource). **History** is the one lazy
  section (`GetFindingHistory`, `lazy.sh_finding_history` keyed by finding
  id, capped `MAX_HISTORY_RECORDS=100`): who changed the workflow status or
  note and when — i.e. "was this triaged, or did a rule silence it". It needs
  the finding's `product_arn` to build an `AwsSecurityFindingIdentifier`, so
  a finding without one renders an explanation instead of fetching.
  **Controls** (tab 3, `ShControl`) — the console's headline tab and the
  level at which "are we compliant" is actually answered. Built from
  `ListSecurityControlDefinitions` (all controls in the region, capped
  `MAX_CONTROL_DEFS=2000`) + one `ListSecurityControlDefinitions(standards_arn=…)`
  **per enabled standard** for membership + `BatchGetSecurityControls` in
  chunks of `CONTROL_BATCH=100` for enablement and customized parameters
  (~20 calls total). Membership comes from the scoped definition list
  because it speaks the canonical `SecurityControlId`;
  `DescribeStandardsControls` also lists a standard's controls but keys them
  by the *standard's* own id (`CIS.1.1`), which joins to nothing else here.
  Pass/fail comes from **one** scan of ACTIVE+FAILED, non-SUPPRESSED control
  findings (`scan_failed_controls`, capped `MAX_CONTROL_SCAN_PAGES=40`)
  counted by `securityControlId` — that scan also drives every standard's
  score, replacing the old `generator_id`-substring guess. **A truncated
  scan never claims a pass**: with the cap hit, every no-failure control
  drops from PASSED to NO DATA and each standard sets `score_estimated`
  (rendered as "score is a floor"), because a scan can prove a failure but
  never its absence. A control in **no** enabled standard is likewise
  NO DATA, not a free pass — it is never evaluated. Rows sort failures
  first, then by severity; `is_noise()` = anything but FAILED, so `a` leaves
  exactly what's broken. Split pane Overview / Standards / Parameters /
  **Failing** — Failing filters **sibling** `ShFinding` rows by control id
  (zero fetch, the Vpc-Subnets pattern) and says `· N of M loaded at this
  severity scope` when the sibling list is narrower than the scan's count,
  rather than implying it's the full set.
  **Automations** (tab 6) is a **grouped tab** (pipe filter, the VPC-Routing
  precedent): `ShAutomationRule` and `ShActionTarget` are the two ways
  something other than triage acts on a finding, and neither fills a tab
  alone. Rules come from `ListAutomationRules` (metadata only — no criteria,
  no actions) → `BatchGetAutomationRules` in chunks of
  `AUTOMATION_BATCH=100`; AWS's quota is 100 rules per account, so that's one
  extra call in practice. Sorted by **rule order**, which is the order AWS
  applies them and therefore the order that explains an outcome. Split pane
  Overview / **Criteria** / **Actions**: `flatten_criteria` walks all 40
  fields of `AutomationRulesFindingFilters` (string / number / date / map
  filters, each pushing only when populated), and a **number filter joins
  every bound** — a range is `gte` + `lte` on one filter, so first-match-wins
  would make the rule look broader than it is (the GuardDuty
  `condition_summary` lesson). The tab exists mostly for the suppression
  half: a live rule setting `WorkflowStatus=SUPPRESSED` reads **yellow**
  (`suppresses()`, the GdFilter precedent) and is the usual reason a finding
  you expect isn't in the list — nothing else in the app can tell you one
  exists. `is_noise()` = DISABLED. Custom actions (`DescribeActionTargets`)
  are flat. `ListAutomationRules` is **administrator-account only**, so a
  member account always fails it — that warns (a plain permission gap looks
  identical) *and* `resource_list.rs` carries a per-tab empty state.
  **Integrations** (tab 7, `ShProduct`, flat `details()`) joins
  `DescribeProducts` (the catalogue) with `ListEnabledProductsForImport`
  (what's actually subscribed) — the two speak **different ARN shapes**
  (`…::product/aws/guardduty` vs `…:123:product-subscription/aws/guardduty`),
  so `product_key` reduces both to the shared `aws/guardduty` tail. Enabled
  first; `is_noise()` = not subscribed, so `a` narrows the catalogue to what
  is genuinely feeding findings (the Control Tower Catalog precedent). A
  denial on the subscription list costs the enabled flag, not the catalogue.
  **Configuration** (tab 8) is a **grouped tab**: one synthetic `ShSettings`
  row (scalars, not a list — the `GdOverview` precedent) beside the
  `ShConfigPolicy` rows those settings hand control to. Settings is five
  independent best-effort calls — `DescribeHub`, `ListFindingAggregators` →
  `GetFindingAggregator`, `DescribeOrganizationConfiguration`,
  `ListOrganizationAdminAccounts`, `GetAdministratorAccount` — each failure
  recorded with a **section prefix** and rendered by `section_errors()` only
  in the section that owns it, so a member account (which cannot call
  `DescribeOrganizationConfiguration`) still reads the hub settings it can.
  Sections Overview / **Regions** / **Organization**. Regions is the one
  people need: no aggregator means the whole app is showing one region's
  findings, and the pane says so. Overview spells out what
  `controlFindingGenerator` means (`SECURITY_CONTROL` = consolidated, one
  finding per control) and Organization what `CENTRAL` vs `LOCAL`
  configuration implies. **`ShConfigPolicy`** (`ListConfigurationPolicies` →
  `GetConfigurationPolicy` per policy) is the CSPM-era answer to "why is this
  control disabled in this account": sections Overview / **Controls** /
  **Targets**. Associations are listed **once org-wide** and grouped by
  policy id rather than queried per policy — same data, one call instead of
  N. A `GetConfigurationPolicy` failure keeps a **stub row** (knowing the
  policy exists beats dropping it) with the error in `detail_error`. The
  Controls section must keep saying which list is populated and what that
  *inverts*: naming enabled controls turns every other control off including
  future ones, and naming disabled controls does the reverse — the two lists
  are mutually exclusive and read identically otherwise. `state()`: any
  FAILED target → red, service disabled → gray/Stopped, no targets →
  UNASSOCIATED; `is_noise()` = no targets. Target rows are key-value so
  `ou-`/`r-`/account ids classify as jumps into Organizations.
  **Accounts** (tab 9, `ShMember`, `ListMembers` with
  `only_associated(false)` so disabled and removed members still appear,
  capped `MAX_MEMBERS=1000`) is flat, sorted worst-first by `member_rank`;
  `is_noise()` = Enabled/Associated, so `a` leaves exactly the accounts not
  reporting. `administrator_id` falls back to the deprecated `MasterId`
  alias — members onboarded through older API versions still come back on
  it with `AdministratorId` empty. This phase is **silent on failure**,
  unlike Automations: a standalone account is *expected* to fail
  `ListMembers`, so the explanation lives in `resource_list.rs`'s per-tab
  empty state (the GuardDuty-Accounts precedent). Automation rules warn
  instead because they work fine in a standalone account, so a failure there
  really is unusual — that's the line between the two conventions.
  **Overview** (tab 1, `ShOverview`) is a real descriptor pane — score bar in
  the header, sections **Score / Findings / Controls / Coverage** — not the
  bespoke full-body renderer it used to be (which ignored the section system,
  so export, flat view and bookmark restore all skipped it). Every section is
  **zero-fetch**: Score reads the standards folded in at build time, and
  Controls / Coverage **filter sibling rows** (`ShControl`, `ShProduct`,
  `ShMember`, `ShSettings`, `ShConfigPolicy` — the Vpc-Subnets pattern), so
  the summary is assembled from phases that already ran rather than from new
  calls. CSPM has **no aggregate-count API** (GuardDuty's
  `GetFindingsStatistics` has no equivalent here), so the severity breakdown
  necessarily counts *what was loaded*: it therefore always prints the active
  severity scope and flags the fetch cap when hit, and the Score section
  flags a truncated control scan. Do not quietly drop those annotations —
  without them the numbers read as account-wide totals they are not.
  **Insights** (tab 5). **`GetInsights` with no ARNs returns custom insights
  only** — it does *not* return the ~40 AWS-managed ones the console leads
  with ("1. Resources with the most findings" …), and most accounts have zero
  custom insights, which is why this tab read as empty for so long. There is
  no list-managed-insights API, so `fetch_managed_insights` probes the stable
  contiguously-numbered ARNs `arn:{partition}:securityhub:::insight/securityhub/default/N`
  for N in `1..=MAX_MANAGED_INSIGHTS` (50), **one ARN per call**
  concurrently (`buffer_unordered(8)`): `GetInsights` rejects the *whole*
  request when any ARN in it is unknown, so batching would let one retired
  number blank the rest, and a miss here is a normal outcome rather than an
  error. `partition_for` derives the ARN partition from the region (there is
  no partition accessor on the SDK config) so GovCloud and China aren't
  silently empty. Rows sort managed-first by `managed_index` (the console's
  numbering), custom after. Split pane **Results** / **Filters** —
  **Results is section 0 deliberately**, against the Overview-first
  convention: an insight *is* its grouped results, and as section 0 the hook
  fires on drill-in so they load with the pane. The metadata an Overview
  would have held (group-by, origin, ARN) leads the Results body, and each
  group renders as a count/bar/share line plus a key-value row labelled by
  `group_label` so an ARN or resource-id group is an Enter-jump to the
  resource itself.
  Results is the lazy `GetInsightResults` (`lazy.sh_insight_results` keyed by
  ARN, largest groups first, capped `MAX_INSIGHT_RESULTS=100`) — *running*
  the insight is the entire point of one, and the tab previously showed only
  its name and group-by attribute. Rows are `(count, group value)` in that
  order deliberately: the classifier reads the **value**, so an ARN or
  resource id group lands as an Enter-jump. Filters flattens the insight's
  `AwsSecurityFindingFilters` — that type has ~90 members, so
  `flatten_insight_filters` renders the ones insights are actually built on
  rather than ninety near-empty rows, and keeps the two **deprecated**
  filters (`severity_normalized`, `keyword`) behind `#[allow(deprecated)]`
  because insights built before AWS replaced them still carry them.
  An insight is **never `is_noise()`** — deliberately unlike CloudFront's
  managed policies: most accounts have zero *custom* insights, so folding
  the ~40 AWS-managed ones away empties the tab, and since `a` is a global
  session toggle, pressing it on Controls (where it's the right move)
  would silently blank Insights. Origin (`managed` / `custom`, from the
  empty account field in a `:::insight/` ARN) is in `search_text()`
  instead — a search term can't hide everything. **The general lesson:
  `is_noise()` is only safe when the non-noise subset is normally
  non-empty**; `resource_list.rs` now checks `hide_noise &&
  hidden_noise_count > 0` **first** in the filtered-empty chain, so a
  tab emptied by `a` says so rather than falling through to a per-tab arm
  that blames a permission gap.
  **Jumps**: Security Hub rows carry no ARN and a control id (`S3.1`) looks
  like nothing the generic classifier recognizes, so `sh_row_jump_target`
  routes by **label** — "Control ID" → the Controls tab (the drill from a
  failing finding to its rule and everything else failing it), "Standard" →
  the Standards tab (trimming `standards/…/v/<version>` down to the name
  `ShStandard::name()` actually stores, so both the finding's raw
  `associatedStandards` value and the control's already-trimmed one
  resolve). Those two labels are load-bearing — keep the rows' key text in
  step with the table. No CloudWatch namespace → no `m`.
- **Inspector** (`@inspector`) — sub-tabs Findings / By Resource / Coverage.
  "By Resource" aggregates all findings into one row per scanned resource;
  `Enter` on a finding's header jumps to that finding in the Findings sub-tab.
- **WorkSpaces** (`@workspaces`) — `fetch_workspace_tags` calls `DescribeTags`
  with the **workspace id, not an ARN**; KMS-key row is jumpable.
- **Direct Connect** (`@dx`) — sub-tabs Connections / Virtual Interfaces /
  LAGs / **Gateways** (key 4). Gateways paginate via `next_token` (the other
  three describes don't) and return **no tags**. The gateway split pane is
  Overview / Associations (lazy `DescribeDirectConnectGatewayAssociations` —
  VGW/TGW ids + allowed prefixes) / Attachments (lazy
  `Describe…GatewayAttachments` — attached VIFs); both lazy maps are keyed by
  gateway id and cleared on profile switch. `dxcon-`/`dxvif-`/`dxlag-` and
  `tgw-`/`tgw-attach-`/`tgw-rtb-` id prefixes are Enter-jumpable
  (`JumpView::Dx`/`JumpView::Tgw`). Gateways have **no CloudWatch metrics** —
  traffic lives on the VIFs (`m` on a VIF) and the physical connection.

- **Budgets** (`@budgets`, `budgets.rs`) — a separate `ServiceType` from Cost,
  not a sub-tab of it: Cost's digit keys 1–7 are already claimed by its own
  group-by/period toggles, so a second resource shape can't share the tab
  strip. Global (`us-east-1`) like Cost Explorer. `DescribeBudgets` requires
  the caller's account id explicitly (budgets are designed for a payer
  account managing budgets on linked accounts), so the list load resolves it
  itself via one `sts:GetCallerIdentity` call rather than depending on
  `App.account_id` — that field resolves asynchronously elsewhere and may
  not be ready yet when this service's first load fires. Split pane Overview
  / **Filters** (`filter_expression`'s recursive AND/OR/NOT tree flattened to
  rows, falling back to the deprecated flat `cost_filters` map for older
  budgets) / **Notifications** (lazy `DescribeNotificationsForBudget` +
  per-notification `DescribeSubscribersForNotification`, `lazy.budget_notifications`
  keyed by budget name — threshold, ACTUAL/FORECASTED, ALARM state,
  email/SNS subscribers) / **Tags** (lazy `ListTagsForResource`,
  `lazy.budget_tags` — the ARN is built from the budget's own `account_id` +
  name, `arn:aws:budgets::{account}:budget/{name}`, a region-less ARN like
  IAM's; no cross-account guessing needed since budgets always live in the
  caller's own account, unlike Athena's workgroup-tags ARN construction).
  `state()` is derived locally (no extra API call): actual spend ≥ limit is
  red (over budget), forecasted spend ≥ limit is yellow (on track to
  exceed), else green — mirroring Cost's own row-dot convention. No
  CloudWatch namespace → no `m`.
- **Invoices** (`@invoices`, `invoicing.rs`) — the "payment details" half of
  this pair: AWS has no read API for stored payment methods (card on file —
  console-only, for good reason), so this is the closest thing, a browse-only
  list of `ListInvoiceSummaries` (amounts, dates, entity, PO number). Global
  (`us-east-1`). Despite the SDK typing it `Option`, the API rejects a
  request with no `selector` (`ValidationException: Value at 'selector' …
  Member must not be null`), so — like Budgets needing its own required
  `AccountId` param — the list load resolves the caller's account id via
  `sts:GetCallerIdentity` and selects `ACCOUNT_ID = <that account>`. Without
  an explicit time interval the API only covers the current billing period,
  and without `receiver_role` the interval itself is capped at one month, so
  the list load also sets `receiver_role = Buyer` (the account paying for
  AWS usage — the overwhelmingly common case) to unlock a trailing-12-month
  `DateInterval`. Deliberately flat `details()` (no split pane, like
  ElasticIp) since every field is scalar. `state()` is deliberately neutral
  (`Unknown`, no colored dot) — invoice summaries carry no paid/unpaid flag
  (most accounts auto-pay on the card on file), so an "overdue" state derived
  from `due_date` alone would be a fabrication. **`d`** (detail pane)
  downloads the invoice PDF via a fresh presigned `GetInvoicePdf` URL to
  `~/.cache/neboto/invoices/<id>.pdf` (path copied to clipboard) — the same
  download-a-presigned-URL-with-`ureq` pattern Lambda's Code section uses for
  its deployment package, right down to the `Event::InvoicePdfDownloaded` /
  `invoice_pdf_downloading` in-flight-gate shape. No CloudWatch namespace →
  no `m`.
- **Control Tower** (`@controltower`, `controltower.rs`) — sub-tabs **Landing
  Zone / Controls / Baselines / Operations / Catalog / Compliance /
  Accounts**, streamed
  as error-tolerant phases (FMS-shaped `first_error` handling: only an
  all-phases-empty failure goes fatal). "You're not in the management
  account's home region" arrives in **three** disguises —
  `AccessDeniedException`, `ResourceNotFoundException` ("you must create a
  landing zone first"), and a `ValidationException` about assuming
  `AWSControlTowerAdmin` (management-account-only, so an audit account gets
  this instead of a denial) — all classified by `wrong_env_hint` and
  collapsed by `warn_phase` into **one** hint no matter how many phases hit
  it; other failures keep their real per-phase message. **Regional, deliberately NOT
  `is_global()`** — the API only answers in the landing zone's home region
  (the Route53Resolver rationale), and only from the
  management/delegated-admin account; there is no admin-probe API like FMS's
  `GetAdminAccount`, so `resource_list.rs` renders a **static**
  wrong-region/wrong-account empty-state hint instead of a probe-driven one.
  Lazy fields use the **`tower_` prefix** (`ct_*` is CloudTrail throughout
  the codebase). Phase 0 builds three best-effort maps: an Organizations
  id→name map (`ListRoots` + local OU BFS + `ListAccounts` — `walk_ous` is
  private to organizations.rs; silent on failure) rendering targets as
  `ou-… (Security)`, the `ListBaselines` catalog (**map only**), and the
  **Control Catalog** (`controlcatalog:ListControls`, one paginated call — no
  N+1; warns on failure since it feeds a visible tab). The catalog is
  indexed by ARN + ARN tail + **every alias** so both legacy
  (`…control/AWS-GR_X`) and Control Catalog UUID enabled-control identifiers
  resolve to friendly name / description / severity / behavior /
  implementation; unresolved ones fall back to the identifier's last
  segment. `LandingZone` (≤1/account, eager `GetLandingZone`; a Get failure
  lands per-row in `detail_error`): Overview (⚠ version-behind, ✗ drift) /
  Manifest (pretty JSON as content lines; `e` opens the raw JSON via a
  section-gated editor override) / Operations (filters sibling `CtOp` rows,
  zero fetch) / Tags(lazy). `EnabledControl` (`ListEnabledControls` with
  **no target filter** = org-wide, emitted per page): Overview eager
  (catalog-enriched — severity ⚠ on HIGH/CRITICAL, wrapped description);
  Parameters lazy `GetEnabledControl` (`lazy.tower_control_details`); Tags
  lazy. The target renders as a key-value organizations-ARN row — the
  `"organizations"` arm in `arn_jump_target` (added with this service; also
  benefits FMS scope rows) makes OU/account/root targets Enter-jumpable into
  Organizations. `EnabledBaseline` mirrors it
  (`lazy.tower_baseline_details`). All three panes' Tags sections share one
  `lazy.tower_tags` map (keyed by ARN) and one `trigger_tower_tags_load`
  (tries all three downcasts). `CtOp` unifies `ListControlOperations` +
  `ListLandingZoneOperations` (the RDS-snapshot unify pattern), newest-first
  capped `MAX_CT_OPERATIONS=200`, flat `details()`; **LZ operation summaries
  carry no timestamps**, so the newest `MAX_LZ_OP_DETAILS=20` get a
  best-effort `GetLandingZoneOperation` to sort into the merged timeline.
  **Catalog** (tab 5): one `CatalogControl` row per catalog control (~500,
  zero extra calls — reuses the phase-0 fetch), flat `details()` with
  wrapped description, governed resources, and the "Enabled On" targets
  collected while the enabled-controls phase streamed; **`is_noise()` = not
  enabled**, so `a` narrows the library to what's active. **Compliance**
  (tab 6): discovers the `aws-controltower-*` Config **aggregator**
  (`DescribeConfigurationAggregators`; the real name is
  `aws-controltower-ConfigAggregatorForOrganization`, a service-linked
  org-wide aggregator) and pages
  `DescribeAggregateComplianceByConfigRules` over **every rule it carries**,
  capped `MAX_COMPLIANCE_ROWS=1000` (a hit warns), NON_COMPLIANT sorted
  first: one `CtCompliance` row per rule×account×region (id/lazy-key =
  `rule|account|region`), NON_COMPLIANT red, **`is_noise()` = COMPLIANT**;
  split pane Overview / Resources (lazy
  `GetAggregateComplianceDetailsByConfigRule`,
  `lazy.tower_compliance_resources`, non-compliant only, capped 100 —
  resource ids render key-value so `i-`/`sg-`/… classify as jumps).
  **Do NOT re-add an `AWSControlTower_*` filter** — that only matches
  *detective* guardrails, and a landing zone running the mandatory
  (preventive, SCP-backed) set enables none, so the filter emptied the tab
  in exactly the orgs it was written for; what the aggregator actually holds
  is Security Hub standards, conformance packs and custom rules across every
  enrolled account (`classify_rule` names the deployer and strips its prefix
  + generated 8-hex suffix for the row label). That makes this the app's only
  org-wide compliance view — `@config` is account-and-region-local.
  **Two accounts, one screen**: Control Tower's own APIs answer only in the
  management account while the aggregator lives only in the audit account
  (v4.0 manifests delegate Config), so the compliance phase tries locally
  first, then assumes config `controltower_audit_account` for its Config
  calls alone (`AwsClients::assume_config_for_account`); rows carry
  `source_account` so the lazy Resources drill follows the same account.
  **Accounts** (tab 7): one `CtAccount` per org account — OU path,
  enrollment (a baseline targeting it), drift, and compliant/non-compliant
  counts — built **purely from the earlier phases' by-target rollups**, zero
  extra calls beyond the phase-0 `ListAccountsForParent` parentage sweep;
  sorted worst-first, `is_noise()` = healthy+enrolled+compliant, split pane
  Overview / Controls / Compliance all sibling-filtered.
  Operations `is_noise()` = SUCCEEDED (`a` hides the enable/disable churn).
  Drift reads red and fuzzy-matches "drifted", but **only `DRIFTED` counts**
  (the `DRIFTED` const): `NOT_CHECKING` is what most preventive controls
  report and treating it as drift flagged every account in the org. No
  CloudWatch namespace → no `m`.
- **Service Catalog** (`@sc`, `servicecatalog.rs`) — sub-tabs **Portfolios /
  Products / Provisioned Products / TagOptions**, all via the **admin-view**
  APIs (`SearchProductsAsAdmin`, `SearchProvisionedProducts` with
  `AccessLevelFilter{Account, "self"}` — "self" is the only accepted value;
  with key `Account` it means the whole account) so the browser sees the
  account's full inventory, not just what's shared with the caller. Four
  independent warn-per-phase loads (the agentcore shape). **One deliberate
  silence**: `ListTagOptions` fails with `TagOptionNotMigratedException` in
  any account that never enabled the TagOptions library (most accounts), so
  that specific error is swallowed (matched via
  `is_tag_option_not_migrated_exception()` on the service error, not message
  text) and the TagOptions tab's empty state explains instead. Caps:
  `MAX_PORTFOLIOS=200`, `MAX_PRODUCTS=300`, `MAX_PROVISIONED=300`,
  `MAX_TAG_OPTIONS=300`, `MAX_PORTFOLIO_PRODUCTS=100`, `MAX_PP_RECORDS=50`
  (newest-first — a cap that dropped the tail would drop the failure),
  `MAX_ACCESS_ITEMS=100`. Page sizes are small (~20), so 300 products is
  ~15 sequential pages — fine, keep the cap.
  - **Portfolio pane** (Details / Products / Principals / Constraints /
    Shares / Tags): Products is a **forward lazy
    `SearchProductsAsAdmin(portfolio_id)`** — the sibling-filter
    (zero-fetch) pattern can't work because the global product list carries
    no portfolio membership. Principals + Constraints share one
    `lazy.sc_portfolio_access` fetch (`ListPrincipalsForPortfolio` +
    `ListConstraintsForPortfolio`, the CodeSuite one-fetch-N-sections
    precedent). Shares loops `DescribePortfolioShares` over all four share
    types (`ACCOUNT`/`ORGANIZATION`/`ORGANIZATIONAL_UNIT`/
    `ORGANIZATION_MEMBER_ACCOUNT` — the API takes one type per call);
    per-type failures are collected and the fetch errors only when **all
    four** fail, so a member account's blanket AccessDenied renders once via
    `error_rows`, not four times. Tags is lazy `DescribePortfolio` because
    `ListPortfolios` returns no tags.
  - **Product pane** (Details / Versions / Portfolios / Tags): one
    `DescribeProductAsAdmin` returns provisioning-artifact summaries + tags
    + tag options in a single call, so all three lazy sections share
    `lazy.sc_product_details` (plus a best-effort `ListPortfoliosForProduct`
    ride-along for the Portfolios section — its failure is silent, the rest
    of the payload still renders). Each version is then **enriched with a
    per-artifact `DescribeProvisioningArtifact`** (`verbose=true` +
    `include_provisioning_artifact_parameters=true`, best-effort per
    version, capped `MAX_ARTIFACT_DETAILS=25` with an annotation row past
    the cap): type, active, guidance (DEPRECATED reads `⚠`), source
    revision, the CFN parameter table (key/type/default/no-echo), an
    `Imported From` stack ARN (jumpable via the `cloudformation` arm), and
    the **template's S3 URL** from the verbose `info` map — read
    `TemplateUrl` first, then the create-time `LoadTemplateFromURL` alias
    (the deprecated-alias rule). **The URL is NOT presigned** — it's the raw
    object URL of the admin's template bucket, and an anonymous GET 403s
    (found live; the first cut assumed presigned). So `e` in the Versions
    section (`trigger_sc_template_fetch`, in the detail-pane `e` chain — the
    RDS log-file gotcha applies) resolves the version from the nearest `ID`
    row above the cursor — **falling forward to the first version below**
    when the cursor sits on the section header, so `e` works from the top of
    the section (the footer shows an `e template` hint while Versions is up
    and a template exists) — then `classify_template_url` picks the fetch
    path:
    an S3-shaped URL (virtual-hosted or path-style, legacy dashed-region
    forms included — `parse_s3_https_url`, unit-tested) goes through an
    **authenticated `GetObject`** with `s3_client_for_region` pinned to the
    URL's region (needs `s3:GetObject` on the template bucket); only a URL
    carrying its own signature query, or a non-S3 URL, takes the plain-HTTP
    `ureq` path (the Lambda code-download shape). The body's first byte
    sniffs JSON-vs-YAML and `$EDITOR` opens via `Event::ScTemplateLoaded`.
    The URL itself is never rendered; a dim `· template available — e opens
    it` annotation row advertises it. No URL (failed enrichment /
    non-template product) falls through to the default snapshot editor, per
    "`e` always opens something".
  - **Provisioned Product pane** (Details / Outputs / History / Tags): tags
    are **eager** — `SearchProvisionedProducts` returns them in the list
    output. `physical_id` is the backing CloudFormation stack ARN for
    CFN-type products; Details renders it as a `("Stack ARN", arn)` row and
    the **`"cloudformation"` arm added to `arn_jump_target`** (with this
    service) makes it Enter-jumpable to the CFN Stacks tab by name — any
    other service that surfaces a stack ARN in a key-value row gets the jump
    for free. Outputs = `GetProvisionedProductOutputs` (needs no access
    filter — it takes the pp id directly). History = `ListRecordHistory`
    (no fluent paginator — hand-rolled `next_page_token` loop) with the
    `provisionedproduct` search filter **plus** the Account access filter,
    else records for other users' provisioned products don't show; sorted
    newest-first client-side, record errors render as `⚠` content lines.
  - **TagOption is flat `details()`** (the ElasticIp rule: the list API
    already returns everything). `name()` is a precomputed `key=value`
    label. INACTIVE renders as `Unknown("INACTIVE")`, not noise — inactive
    options can be the whole library.
  - **No `console_url()`** on any type: the console splits admin
    (`/servicecatalog/home#admin-…`) vs end-user views and the deep-link
    shapes are uncertain — a wrong link is worse than none (the agentcore
    rule). No CloudWatch namespace → no `m`. No `JumpView::Sc` yet — nothing
    needs to jump *into* Service Catalog.
- **S3** (`@s3`, `s3.rs`) — architecture (Bucket|Objects toggle, Tier-1/Tier-2
  fetch split, cross-region stubs, daily storage metrics) is in CLAUDE.md;
  what's here is the *trust* fixes — places where the pane once confidently
  showed the wrong thing:
  - **Versioning is a tri-state** (`VersioningStatus`), not a bool: Suspended
    still retains (and bills for) every old version, and the earlier bool
    collapse made it read as "Disabled". `mfa_delete` rides the same call;
    the MFA Delete row only renders when versioning isn't Disabled.
  - **`public_access_block` is an `Option`**: only a
    `NoSuchPublicAccessBlockConfiguration` error means "nothing blocked" —
    any other failure (AccessDenied) is `None` and renders **⚠ Unknown**.
    The old all-false-on-error default made a locked-down bucket render as
    "✗ Public Access Allowed" to a caller who merely lacked the permission.
  - **`GetBucketPolicyStatus`** (Tier-2) is AWS's own is-public verdict —
    the Security pane's "Policy Status" row (⚠ PUBLIC / ✓ Not public). The
    call errors when there's no bucket policy; that and a denial both fold
    to `None` = no row (the Bucket Policy row above already covers it).
  - **ACL grants are listed individually** (`AclGrant`: grantee, permission,
    `public` flag for AllUsers/AuthenticatedUsers group URIs; the owner's
    canonical id renders as "Bucket owner"). The previous bare grant *count*
    hid public-read ACLs — the classic S3 exposure — entirely.
  - **Storage metrics sum every StorageType** via `GetMetricData`
    `SUM(SEARCH(…))` (`s3_storage_series`, shared by the Metadata section
    and the `m` overlay). A fixed `StorageType=StandardStorage` dimension
    charted Glacier/IA/Intelligent-Tiering-heavy buckets as near-empty.
    IAM: this is `cloudwatch:GetMetricData` now, not `GetMetricStatistics`.
  - **Notification target ARNs are rendered** (SNS/SQS/Lambda rows — each an
    Enter-jump — plus an EventBridge ✓ row); previously counts only, with
    the collected ARNs dead in the struct.
  - **Object browser versions mode** (`V`): swaps the listing to
    `ListObjectVersions` — every version **and the delete markers
    `ListObjectsV2` hides** (a "deleted" key is otherwise just gone).
    `S3Entry::Version` rows; per-key newest-first (the two response arrays
    are merged and re-sorted); `v`/`e`/`d`/`p`/`i` all target the selected
    version id. Three traps encoded in the shape: pagination needs **two**
    markers (`NextKeyMarker`+`NextVersionIdMarker`), packed into the single
    `next_token` with a `\0` separator (`VERSION_TOKEN_SEP` — a char no key
    or version id can contain); **delete markers have no content** (GET/HEAD
    on their version id fails), so `S3Entry::content_ref()` returns `None`
    for them and every content verb routes through it rather than
    pattern-matching `Object` (which silently dead-ends new variants); and
    pre-versioning objects report the literal version id `"null"` — display
    it, don't special-case it. Object-meta cache keys gain an `@version`
    suffix so a version's HEAD never shadows the current object's.
- **S3 Files** (`@s3files`, `s3files.rs`) — the 2026 S3 shared-file-system
  feature (EFS-based NFS file systems linked to a bucket or prefix, two-way
  sync). Single-list (file systems, `ListFileSystems`, cap 200, single-phase
  → a list failure is a plain `ResourceLoadError`); split pane **Details /
  Mount Targets / Access Points / Sync / Policy / Tags**, all lazy.
  - **The list output is thin**: no prefix, no KMS key, no tags — those come
    from a lazy `GetFileSystem` (`lazy.s3files_details`) shared by the
    Details and Tags sections. Consequence: the app-wide `tag:` search
    filter can't see S3 Files tags (the list rows carry an empty tag map).
  - **Details** renders the linked bucket as its `arn:aws:s3:::name` ARN row
    — jumpable via the **`"s3"` arm added to `arn_jump_target`** (with this
    service; guarded to the bare-bucket ARN form — access-point ARNs carry
    region+account and are left alone). Mount-target rows surface
    subnet-/vpc-/eni- ids, which ride the existing prefix classifiers.
  - **`"s3files"` arn arm**: `file-system/fs-…` ARNs jump into `@s3files` by
    id. This is also how the **S3 bucket pane's File Systems section**
    (added with this service: one lazy `ListFileSystems` sweep filtered
    client-side by bucket ARN, keyed by bucket name —
    `lazy.s3_bucket_filesystems`) links back: each file system renders its
    s3files ARN row. **A bare `fs-` id can NOT be a jump classifier** — EFS
    file system ids use the same prefix, so only the full ARN
    disambiguates.
  - **Policy**: a missing file system policy comes back as
    `ResourceNotFoundException` (`is_resource_not_found_exception()`) —
    mapped to `Ok(None)` → "No file system policy attached", never an
    error. The policy body is pretty-printed inline.
  - **Access points cap 100** (hard — the service allows 25k per file
    system) with an annotation row when truncated. Mount targets cap 50
    (max one per AZ, so the cap is theoretical).
  - **LifeCycleState** maps: available→Available, creating/updating→Pending,
    deleting→Deleting, deleted→Terminated, error→Unavailable.
  - No `console_url()` (deep-link shape unverified — the agentcore rule),
    no `m` (no published CloudWatch namespace found yet; revisit).
- **S3 Tables** (`@s3tables`, `s3tables.rs`) — table buckets holding Apache
  Iceberg tables with managed maintenance. Sub-tabs **Table Buckets /
  Tables** (`S3TablesView`). Two-phase streaming load: `ListTableBuckets`
  (cap 100; failure = total → `ResourceLoadError`), then per-bucket
  `ListTables` across all namespaces (cap 200/bucket, 500 total — a
  per-bucket failure is a `ResourceLoadWarning`, and hitting the total cap
  warns rather than truncating silently).
  - **Ids are the full ARN** for both types — table-bucket APIs are
    ARN-keyed, and table names are only unique per namespace. The
    `"s3tables"` `arn_jump_target` arm therefore jumps with the whole ARN
    (`bucket/<name>` and `bucket/<name>/table/<uuid>` both resolve; the
    sub-tab aligns to the resolved type via `align!`). A table's pane
    renders its Bucket ARN row as the jump back to its table bucket.
  - **Bucket pane** (Details / Namespaces / Maintenance / Policy / Tags):
    Details folds in a lazy `GetTableBucketEncryption` — a **404 means the
    SSE-S3 default**, not an error (`is_not_found_exception()` → "SSE-S3
    (default)"); tags ride the same fetch via `ListTagsForResource`
    (`lazy.s3tables_bucket_extras`, the s3files extras shape — and like
    s3files, the list rows carry no tags so the app-wide `tag:` filter
    can't see them). Maintenance = `GetTableBucketMaintenanceConfiguration`
    (unreferenced-file removal settings). Policy 404 → `Ok(None)` → "No
    table bucket policy attached".
  - **Table pane** (Details / Maintenance / Policy / Tags): Details folds in
    a lazy `GetTable` (format, metadata/warehouse locations, version token,
    created/modified-by) — `GetTable` accepts `table_arn` directly, no
    namespace plumbing. Maintenance = config
    (`GetTableMaintenanceConfiguration`: compaction + snapshot management)
    plus per-job last-run status (`GetTableMaintenanceJobStatus`,
    **best-effort** — a denial shows the config alone rather than erroring
    the section). Those two, plus `GetTablePolicy`, are keyed
    `bucket_arn+namespace+name` — the S3Table struct carries all three.
  - `type=aws` buckets/tables are owned by another AWS service
    (`managed_by_service` — rendered "managed by <principal>" in the table
    header). Don't filter them out; seeing them is the point.
  - No `console_url()` (deep-link shape unverified), no `m` (metrics
    configurations exist server-side but the CloudWatch namespace is
    unverified; revisit). `warehouse_location` is an S3-style URI whose
    bucket is internal to the service — its jump indicator may render but
    won't resolve to anything in `@s3`; harmless, left alone.
