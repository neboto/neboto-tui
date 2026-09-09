use crate::aws::client::AwsClients;
use crate::aws::resource::{native_state_label, Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::error::Result;
use crate::event::{Event, LoadProgress};
use async_trait::async_trait;
use aws_sdk_controltower::Client as CtClient;
use std::any::Any;
use std::collections::HashMap;
use tokio::sync::mpsc;

/// AWS Control Tower — the landing zone (version / drift / manifest), the
/// guardrail controls and baselines enabled across the org, and the
/// enable/disable operation history. Regional, deliberately NOT `is_global()`:
/// the API only answers in the landing zone's home region, so a region switch
/// must refetch (the Route53Resolver rationale). Nearly every call also only
/// answers from the management (or delegated-admin) account — every phase is
/// best-effort, and an all-phases-empty failure goes fatal with a friendly
/// hint; `resource_list.rs` renders a bespoke empty state for the
/// wrong-region / wrong-account cases.
///
/// Enabled controls are enriched from the **Control Catalog** (one paginated
/// `controlcatalog:ListControls` in phase 0, indexed by ARN + every alias —
/// no N+1): friendly name, description, behavior, severity, implementation.
/// A control the catalog can't resolve falls back to the identifier ARN's
/// last segment (legacy `AWS-GR_*` slugs read well). Target OU/account ids
/// are enriched to names via a best-effort phase-0 Organizations sweep (the
/// KMS-alias pattern), silent on failure since member accounts routinely
/// lack org read. The **Compliance** sub-tab reads the
/// `aws-controltower-*` Config **aggregator** (discovered via
/// `DescribeConfigurationAggregators`) for per-account detective-control
/// compliance — only visible from the management/audit account.
pub struct ControlTowerService {
    client: CtClient,
    org_client: aws_sdk_organizations::Client,
    catalog_client: aws_sdk_controlcatalog::Client,
    config_client: aws_sdk_config::Client,
    /// Base credentials, kept so the compliance phase can re-point AWS Config
    /// at the audit account (see `audit_target`).
    sdk_config: aws_config::SdkConfig,
    /// `(account id, role name)` of the account holding the Config aggregator,
    /// from `controltower_audit_account`. `None` (the default) keeps the
    /// compliance phase local.
    audit_target: Option<(String, String)>,
}

impl ControlTowerService {
    pub fn new(aws_clients: &AwsClients, audit_target: Option<(String, String)>) -> Self {
        Self {
            client: aws_clients.controltower_client(),
            org_client: aws_clients.organizations_client(),
            catalog_client: aws_clients.controlcatalog_client(),
            config_client: aws_clients.awsconfig_client(),
            sdk_config: aws_clients.sdk_config().clone(),
            // Already browsing that account? Then the local client is the
            // audit client and a second assume would be pointless.
            audit_target: audit_target
                .filter(|(id, _)| aws_clients.assumed_account_id() != Some(id.as_str())),
        }
    }
}

/// The one `DriftStatus` value that means something is actually wrong. The
/// others (`IN_SYNC`, `NOT_CHECKING`, `UNKNOWN`) are all normal — notably
/// `NOT_CHECKING`, which most preventive controls report because they have no
/// drift detection to do.
pub const DRIFTED: &str = "DRIFTED";

/// Operations kept after the merged newest-first sort.
pub const MAX_CT_OPERATIONS: usize = 200;
/// `ListLandingZoneOperations` summaries carry no timestamps (only the
/// detail does) — enrich the most recent few via N+1 `GetLandingZoneOperation`
/// so they sort into the merged timeline. LZ operations are rare (a handful
/// per year), so the cap is generous in practice.
const MAX_LZ_OP_DETAILS: usize = 20;
/// Aggregate compliance rows kept (rule × account × region combinations).
const MAX_COMPLIANCE_ROWS: usize = 1000;
/// Non-compliant resources fetched per compliance row (lazy Resources
/// section).
const MAX_COMPLIANCE_RESOURCES: usize = 100;

#[async_trait]
impl AwsService for ControlTowerService {
    fn service_type(&self) -> ServiceType {
        ServiceType::ControlTower
    }

    fn name(&self) -> &str {
        "Control Tower"
    }

    async fn list_resources(&self) -> Result<Vec<Box<dyn Resource>>> {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        self.list_resources_streaming(tx, ServiceType::ControlTower)
            .await?;
        Ok(vec![])
    }

    async fn list_resources_streaming(
        &self,
        event_tx: mpsc::UnboundedSender<Event>,
        service_type: ServiceType,
    ) -> Result<()> {
        let mut total = 0usize;
        // The first phase failure, kept for the fatal-error path (a member
        // account / wrong region fails every phase — one clear message beats
        // four warnings).
        let mut first_error: Option<String> = None;
        // Set once the wrong-account/region hint has been emitted, so the
        // remaining phases don't repeat it (see `warn_phase`).
        let mut env_warned = false;

        let emit = |resources: Vec<Box<dyn Resource>>,
                    loaded: usize,
                    status: Option<&str>,
                    done: bool| {
            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                service: service_type,
                resources,
                progress: LoadProgress {
                    loaded_count: loaded,
                    total_count: if done { Some(loaded) } else { None },
                    status_message: status.map(|s| s.to_string()),
                },
            });
        };

        // ── Phase 0: enrichment maps (best-effort, silent) ────────────────────
        let org = self.fetch_org_map().await;
        let target_names = &org.names;
        // By-target rollups the Accounts phase folds together at the end. Every
        // phase contributes as it streams, so the sub-tab costs no extra calls.
        let mut controls_by_target: HashMap<String, (usize, usize)> = HashMap::new();
        let mut baselines_by_target: HashMap<String, (Option<String>, Option<String>)> =
            HashMap::new();
        let mut compliance_by_account: HashMap<String, (usize, usize)> = HashMap::new();
        let baseline_names = self.fetch_baseline_catalog().await;
        // The control catalog also feeds a visible sub-tab, so its failure
        // warns instead of staying silent.
        let (catalog, catalog_err) = self.fetch_control_catalog().await;
        if let Some(msg) = catalog_err {
            let _ = event_tx.send(Event::ResourceLoadWarning {
                service: service_type,
                warning: format!("Control catalog: {}", msg),
            });
        }

        // ── Phase 1: Landing Zone (list + eager per-LZ Get; ≤1 per account) ──
        let mut lz_arns: Vec<String> = Vec::new();
        let mut paginator = self.client.list_landing_zones().into_paginator().send();
        while let Some(page) = paginator.next().await {
            match page {
                Ok(p) => {
                    for s in p.landing_zones() {
                        if let Some(arn) = s.arn() {
                            lz_arns.push(arn.to_string());
                        }
                    }
                }
                Err(e) => {
                    let msg = crate::error::sdk_error_message(&e);
                    warn_phase(
                        &event_tx,
                        service_type,
                        &mut env_warned,
                        "Control Tower landing zone",
                        &msg,
                    );
                    first_error.get_or_insert(msg);
                    break;
                }
            }
        }
        let mut landing_zones: Vec<LandingZone> = Vec::new();
        for arn in lz_arns {
            match self
                .client
                .get_landing_zone()
                .landing_zone_identifier(&arn)
                .send()
                .await
            {
                Ok(resp) => landing_zones.push(LandingZone::from_sdk(&arn, resp.landing_zone())),
                Err(e) => {
                    // The row survives with the failure inline (FMS-style).
                    let mut lz = LandingZone::from_sdk(&arn, None);
                    lz.detail_error = Some(crate::error::sdk_error_message(&e));
                    landing_zones.push(lz);
                }
            }
        }
        if !landing_zones.is_empty() {
            total += landing_zones.len();
            let batch: Vec<Box<dyn Resource>> = landing_zones
                .into_iter()
                .map(|z| Box::new(z) as Box<dyn Resource>)
                .collect();
            emit(batch, total, Some("Loading enabled controls…"), false);
        }

        // ── Phase 2: Enabled Controls (org-wide — no target filter) ──────────
        // Emitted per page: a large org easily exceeds one page of controls.
        // While streaming, collect which catalog controls are enabled where —
        // the Catalog sub-tab's "Enabled on" marks.
        let mut enabled_by_catalog: HashMap<String, Vec<String>> = HashMap::new();
        let mut paginator = self
            .client
            .list_enabled_controls()
            .into_paginator()
            .send();
        while let Some(page) = paginator.next().await {
            match page {
                Ok(p) => {
                    let batch: Vec<Box<dyn Resource>> = p
                        .enabled_controls()
                        .iter()
                        .map(|s| {
                            let c = EnabledControl::from_sdk(s, target_names, &catalog);
                            let entry = controls_by_target
                                .entry(arn_tail(&c.target_identifier).to_string())
                                .or_insert((0, 0));
                            entry.0 += 1;
                            // Only DRIFTED is drift: NOT_CHECKING means the
                            // control doesn't do drift detection at all (most
                            // preventive controls), and counting it would
                            // flag every account in the org.
                            if c.drift_status.as_deref() == Some(DRIFTED) {
                                entry.1 += 1;
                            }
                            if let Some(meta) = catalog.lookup(&c.control_identifier) {
                                enabled_by_catalog
                                    .entry(meta.arn.clone())
                                    .or_default()
                                    .push(
                                        c.target_name
                                            .clone()
                                            .unwrap_or_else(|| {
                                                arn_tail(&c.target_identifier).to_string()
                                            }),
                                    );
                            }
                            Box::new(c) as Box<dyn Resource>
                        })
                        .collect();
                    if !batch.is_empty() {
                        total += batch.len();
                        emit(batch, total, Some("Loading enabled controls…"), false);
                    }
                }
                Err(e) => {
                    let msg = crate::error::sdk_error_message(&e);
                    warn_phase(
                        &event_tx,
                        service_type,
                        &mut env_warned,
                        "Control Tower enabled controls",
                        &msg,
                    );
                    first_error.get_or_insert(msg);
                    break;
                }
            }
        }

        // ── Phase 3: Enabled Baselines ───────────────────────────────────────
        let mut baselines: Vec<EnabledBaseline> = Vec::new();
        let mut paginator = self
            .client
            .list_enabled_baselines()
            .include_children(true)
            .into_paginator()
            .send();
        while let Some(page) = paginator.next().await {
            match page {
                Ok(p) => {
                    for s in p.enabled_baselines() {
                        let b = EnabledBaseline::from_sdk(s, &baseline_names, target_names);
                        baselines_by_target.insert(
                            arn_tail(&b.target_identifier).to_string(),
                            (b.status.clone(), b.drift_status.clone()),
                        );
                        baselines.push(b);
                    }
                }
                Err(e) => {
                    let msg = crate::error::sdk_error_message(&e);
                    warn_phase(
                        &event_tx,
                        service_type,
                        &mut env_warned,
                        "Control Tower enabled baselines",
                        &msg,
                    );
                    first_error.get_or_insert(msg);
                    break;
                }
            }
        }
        if !baselines.is_empty() {
            total += baselines.len();
            let batch: Vec<Box<dyn Resource>> = baselines
                .into_iter()
                .map(|b| Box::new(b) as Box<dyn Resource>)
                .collect();
            emit(batch, total, Some("Loading operations…"), false);
        }

        // ── Phase 4: Operations (control + landing-zone legs, merged) ────────
        let mut ops: Vec<CtOp> = Vec::new();
        let mut paginator = self
            .client
            .list_control_operations()
            .into_paginator()
            .send();
        while let Some(page) = paginator.next().await {
            match page {
                Ok(p) => {
                    for s in p.control_operations() {
                        ops.push(CtOp::from_control_op(s, target_names));
                    }
                }
                Err(e) => {
                    warn_phase(
                        &event_tx,
                        service_type,
                        &mut env_warned,
                        "Control Tower control operations",
                        &crate::error::sdk_error_message(&e),
                    );
                    break;
                }
            }
        }
        let mut lz_ops: Vec<CtOp> = Vec::new();
        let mut paginator = self
            .client
            .list_landing_zone_operations()
            .into_paginator()
            .send();
        while let Some(page) = paginator.next().await {
            match page {
                Ok(p) => {
                    for s in p.landing_zone_operations() {
                        lz_ops.push(CtOp::from_lz_op(s));
                    }
                }
                Err(e) => {
                    warn_phase(
                        &event_tx,
                        service_type,
                        &mut env_warned,
                        "Control Tower landing zone operations",
                        &crate::error::sdk_error_message(&e),
                    );
                    break;
                }
            }
        }
        // LZ operation summaries carry no timestamps — enrich the first few
        // (the list is API-ordered newest-first) so they sort into the merged
        // timeline. Best-effort: a failed Get just leaves the times blank.
        for op in lz_ops.iter_mut().take(MAX_LZ_OP_DETAILS) {
            if let Ok(resp) = self
                .client
                .get_landing_zone_operation()
                .operation_identifier(&op.operation_identifier)
                .send()
                .await
            {
                if let Some(d) = resp.operation_details() {
                    op.start_secs = d.start_time().map(|t| t.secs());
                    op.start_time = d.start_time().map(fmt_time);
                    op.end_time = d.end_time().map(fmt_time);
                    op.status_message = d.status_message().map(|s| s.to_string());
                }
            }
        }
        ops.append(&mut lz_ops);
        // Newest first; ops without a resolved start time sink to the end.
        ops.sort_by_key(|o| std::cmp::Reverse(o.start_secs.unwrap_or(i64::MIN)));
        ops.truncate(MAX_CT_OPERATIONS);
        if !ops.is_empty() {
            total += ops.len();
            let batch: Vec<Box<dyn Resource>> = ops
                .into_iter()
                .map(|o| Box::new(o) as Box<dyn Resource>)
                .collect();
            emit(batch, total, Some("Loading control catalog…"), false);
        }

        // ── Phase 5: Catalog rows (from the phase-0 fetch, zero extra calls) ──
        if !catalog.metas.is_empty() {
            let batch: Vec<Box<dyn Resource>> = catalog
                .metas
                .iter()
                .map(|meta| {
                    Box::new(CatalogControl::new(
                        meta.clone(),
                        enabled_by_catalog.remove(&meta.arn).unwrap_or_default(),
                    )) as Box<dyn Resource>
                })
                .collect();
            total += batch.len();
            emit(batch, total, Some("Loading compliance…"), false);
        }

        // ── Phase 6: Compliance (Control Tower's Config aggregator) ──────────
        match self.fetch_compliance_rows(target_names, &catalog).await {
            Ok((rows, _)) if rows.is_empty() => {}
            Ok((rows, capped)) => {
                if capped {
                    let _ = event_tx.send(Event::ResourceLoadWarning {
                        service: service_type,
                        warning: format!(
                            "Control Tower compliance: showing the first {} rule results \
                             (violations first)",
                            MAX_COMPLIANCE_ROWS
                        ),
                    });
                }
                total += rows.len();
                let batch: Vec<Box<dyn Resource>> = rows
                    .into_iter()
                    .map(|r| {
                        let entry = compliance_by_account
                            .entry(r.account_id.clone())
                            .or_insert((0, 0));
                        match r.compliance.as_deref() {
                            Some("NON_COMPLIANT") => entry.1 += 1,
                            Some("COMPLIANT") => entry.0 += 1,
                            _ => {}
                        }
                        Box::new(r) as Box<dyn Resource>
                    })
                    .collect();
                emit(batch, total, Some("Loading accounts…"), false);
            }
            Err(msg) => {
                let _ = event_tx.send(Event::ResourceLoadWarning {
                    service: service_type,
                    warning: format!("Control Tower compliance: {}", msg),
                });
            }
        }

        // ── Phase 7: Accounts (pure rollup of the phases above — no calls) ───
        // Last on purpose: it needs every earlier phase's by-target counts.
        if !org.accounts.is_empty() {
            let mut accounts: Vec<CtAccount> = org
                .accounts
                .iter()
                .map(|info| {
                    CtAccount::build(
                        info,
                        &org.parents,
                        &baselines_by_target,
                        &controls_by_target,
                        &compliance_by_account,
                    )
                })
                .collect();
            // Accounts wanting attention first, then by OU so the list reads
            // like the console's tree rather than Organizations' join order.
            accounts.sort_by(|a, b| {
                b.noncompliant
                    .cmp(&a.noncompliant)
                    .then_with(|| a.ou_path.cmp(&b.ou_path))
                    .then_with(|| a.account_name.cmp(&b.account_name))
            });
            total += accounts.len();
            let batch: Vec<Box<dyn Resource>> = accounts
                .into_iter()
                .map(|a| Box::new(a) as Box<dyn Resource>)
                .collect();
            emit(batch, total, None, true);
        }

        // Everything failed and nothing streamed → fatal with the first (and
        // most informative) error; the bespoke empty state covers the
        // no-error-but-empty case (wrong region).
        if total == 0 {
            if let Some(msg) = first_error {
                let _ = event_tx.send(Event::ResourceLoadError {
                    service: service_type,
                    error: friendly_error(&msg),
                });
                return Ok(());
            }
        }

        let _ = event_tx.send(Event::ResourcesFullyLoaded {
            service: service_type,
            total_count: total,
        });
        Ok(())
    }

    async fn get_resource_details(&self, _id: &str) -> Result<Box<dyn Resource>> {
        Err(crate::error::Error::NotImplemented)
    }
}

impl ControlTowerService {
    /// Best-effort org sweep: the id→name map every target row renders through
    /// (root/OU/account ids keyed by their bare trailing id) plus the account
    /// inventory + OU parentage the Accounts sub-tab is built from. Any failure
    /// returns what was gathered so far — member accounts routinely lack
    /// `organizations:*` read, and a bare `ou-…` row is still usable.
    async fn fetch_org_map(&self) -> OrgMap {
        let mut org = OrgMap::default();

        // Containers to walk, each carrying the display path built so far, so
        // an account's OU path reads "Root / Security" without a second pass.
        let mut queue: std::collections::VecDeque<(String, String)> =
            std::collections::VecDeque::new();
        let mut paginator = self.org_client.list_roots().into_paginator().send();
        while let Some(page) = paginator.next().await {
            let Ok(p) = page else { return org };
            for root in p.roots() {
                if let Some(id) = root.id() {
                    let name = root.name().unwrap_or("Root").to_string();
                    org.names.insert(id.to_string(), name.clone());
                    queue.push_back((id.to_string(), name));
                }
            }
        }

        // BFS over the OU tree (async can't recurse without boxing). Accounts
        // are read per container on the way down — `ListAccountsForParent` is
        // the only API that reports parentage without an N+1 over accounts.
        let mut containers: Vec<(String, String)> = Vec::new();
        while let Some((parent_id, parent_path)) = queue.pop_front() {
            containers.push((parent_id.clone(), parent_path.clone()));
            let mut paginator = self
                .org_client
                .list_organizational_units_for_parent()
                .parent_id(&parent_id)
                .into_paginator()
                .send();
            while let Some(page) = paginator.next().await {
                let Ok(p) = page else { break };
                for ou in p.organizational_units() {
                    if let (Some(id), Some(name)) = (ou.id(), ou.name()) {
                        org.names.insert(id.to_string(), name.to_string());
                        queue.push_back((id.to_string(), format!("{} / {}", parent_path, name)));
                    }
                }
            }
        }

        for (container_id, path) in &containers {
            let mut paginator = self
                .org_client
                .list_accounts_for_parent()
                .parent_id(container_id)
                .into_paginator()
                .send();
            while let Some(page) = paginator.next().await {
                let Ok(p) = page else { break };
                for a in p.accounts() {
                    if let Some(id) = a.id() {
                        org.parents
                            .insert(id.to_string(), (container_id.clone(), path.clone()));
                    }
                }
            }
        }

        // The authoritative account list last: parentage is best-effort, but a
        // missing account row isn't (the sub-tab would silently lose accounts).
        let mut paginator = self.org_client.list_accounts().into_paginator().send();
        while let Some(page) = paginator.next().await {
            let Ok(p) = page else { return org };
            for a in p.accounts() {
                let Some(id) = a.id() else { continue };
                let name = a.name().unwrap_or(id).to_string();
                org.names.insert(id.to_string(), name.clone());
                org.accounts.push(OrgAccountInfo {
                    id: id.to_string(),
                    name,
                    email: a.email().map(|e| e.to_string()),
                    status: a.status().map(|s| s.as_str().to_string()),
                    joined_method: a.joined_method().map(|m| m.as_str().to_string()),
                    joined_at: a.joined_timestamp().map(fmt_time),
                });
            }
        }
        org
    }

    /// Best-effort `ListBaselines` catalog: baseline ARN → friendly name.
    /// Map-only — the handful of AWS-provided baselines are static reference
    /// data, not worth rows of their own.
    async fn fetch_baseline_catalog(&self) -> HashMap<String, String> {
        let mut names = HashMap::new();
        let mut paginator = self.client.list_baselines().into_paginator().send();
        while let Some(page) = paginator.next().await {
            let Ok(p) = page else { return names };
            for b in p.baselines() {
                names.insert(b.arn().to_string(), b.name().to_string());
            }
        }
        names
    }

    /// The full Control Catalog (one paginated `ListControls` — no N+1),
    /// indexed by ARN, ARN tail, and every alias so both legacy
    /// (`…controltower…control/AWS-GR_X`) and Control Catalog UUID enabled-
    /// control identifiers resolve. Returns what was gathered plus the error
    /// that stopped it, if any.
    async fn fetch_control_catalog(&self) -> (ControlCatalog, Option<String>) {
        let mut catalog = ControlCatalog::default();
        let mut paginator = self.catalog_client.list_controls().into_paginator().send();
        while let Some(page) = paginator.next().await {
            let p = match page {
                Ok(p) => p,
                Err(e) => return (catalog, Some(crate::error::sdk_error_message(&e))),
            };
            for c in p.controls() {
                let meta = CatalogMeta {
                    arn: c.arn().to_string(),
                    name: c.name().to_string(),
                    description: c.description().to_string(),
                    behavior: c.behavior().map(|b| b.as_str().to_string()),
                    severity: c.severity().map(|s| s.as_str().to_string()),
                    implementation_type: c
                        .implementation()
                        .map(|i| i.r#type().to_string()),
                    implementation_identifier: c
                        .implementation()
                        .and_then(|i| i.identifier())
                        .map(|s| s.to_string()),
                    aliases: c.aliases().to_vec(),
                    governed_resources: c.governed_resources().to_vec(),
                };
                catalog.push(meta);
            }
        }
        (catalog, None)
    }

    /// Find Control Tower's Config aggregator (`aws-controltower-…`, typically
    /// `…GuardrailsComplianceAggregator`) in whatever account the client is
    /// pointed at. `Ok(None)` = the account has none, which is a normal
    /// answer, not a failure; `Err` = the call itself failed.
    async fn discover_aggregator(
        client: &aws_sdk_config::Client,
    ) -> std::result::Result<Option<String>, String> {
        let mut token: Option<String> = None;
        loop {
            let resp = client
                .describe_configuration_aggregators()
                .set_next_token(token.clone())
                .send()
                .await
                .map_err(|e| crate::error::sdk_error_message(&e))?;
            for a in resp.configuration_aggregators() {
                if let Some(name) = a.configuration_aggregator_name() {
                    if name.to_lowercase().starts_with("aws-controltower") {
                        return Ok(Some(name.to_string()));
                    }
                }
            }
            token = crate::aws::pagination::next_page_token(resp.next_token(), &token);
            if token.is_none() {
                return Ok(None);
            }
        }
    }

    /// The Compliance sub-tab: discover Control Tower's org-wide Config
    /// aggregator, then one paginated `DescribeAggregateComplianceByConfigRules`
    /// over **every** rule it carries — one row per rule × account × region.
    ///
    /// Deliberately not filtered to `AWSControlTower_*`: that's only populated
    /// by *detective* guardrails, and a landing zone running the mandatory
    /// (preventive, SCP-backed) set enables none, so the filter emptied the tab
    /// in exactly the orgs it was written for. What the aggregator actually
    /// holds is every Config rule across every enrolled account — Security Hub
    /// standards, conformance packs, custom rules — which is the org-wide
    /// compliance view nothing else in the app offers (`@config` is
    /// account-and-region-local). Returns the rows plus whether the cap
    /// truncated them.
    async fn fetch_compliance_rows(
        &self,
        target_names: &HashMap<String, String>,
        catalog: &ControlCatalog,
    ) -> std::result::Result<(Vec<CtCompliance>, bool), String> {
        // Local account first: in an audit-account session the aggregator is
        // right here, and an assume would be pure overhead.
        let mut client = self.config_client.clone();
        let mut aggregator = Self::discover_aggregator(&client).await?;
        let mut via_audit = false;
        let mut source_account: Option<String> = None;

        // Landing zones that delegate AWS Config (v4.0 manifests do) keep the
        // aggregator in the audit account, where Control Tower's own APIs
        // don't answer — so one session can only show both tabs by reaching
        // across. Only configuring the account opts into that.
        if aggregator.is_none() {
            if let Some((account_id, role_name)) = &self.audit_target {
                let config =
                    AwsClients::assume_config_for_account(&self.sdk_config, account_id, role_name)
                        .await;
                let audit_client = aws_sdk_config::Client::new(&config);
                aggregator = Self::discover_aggregator(&audit_client)
                    .await
                    .map_err(|e| format!("audit account {}: {}", account_id, e))?;
                client = audit_client;
                via_audit = true;
                source_account = Some(account_id.clone());
            }
        }

        let Some(aggregator) = aggregator else {
            return Err(if via_audit {
                let (account_id, _) = self.audit_target.clone().unwrap_or_default();
                format!(
                    "no aws-controltower Config aggregator in the configured audit account ({})",
                    account_id
                )
            } else {
                "no aws-controltower Config aggregator visible — set \
                 controltower_audit_account in the config file to read it from the audit \
                 account, or browse that account directly"
                    .to_string()
            });
        };

        let mut rows = Vec::new();
        let mut paginator = client
            .describe_aggregate_compliance_by_config_rules()
            .configuration_aggregator_name(&aggregator)
            .into_paginator()
            .send();
        while let Some(page) = paginator.next().await {
            let p = page.map_err(|e| crate::error::sdk_error_message(&e))?;
            for r in p.aggregate_compliance_by_config_rules() {
                let mut row = CtCompliance::from_sdk(&aggregator, r, target_names, catalog);
                row.source_account = source_account.clone();
                rows.push(row);
                if rows.len() >= MAX_COMPLIANCE_ROWS {
                    return Ok((rows, true));
                }
            }
        }
        // Violations first: the tab spans every rule × account × region, so raw
        // API order buries the handful that matter under the compliant bulk.
        rows.sort_by_key(|r| r.compliance.as_deref() != Some("NON_COMPLIANT"));
        Ok((rows, false))
    }
}

/// The three ways Control Tower says "this isn't the landing zone's account
/// or home region": an IAM denial, no landing zone registered here
/// (`ResourceNotFoundException` — "you must create a landing zone first"), or
/// `AWSControlTowerAdmin` not being assumable (that role exists only in the
/// management account, so an audit/member account gets a `ValidationException`
/// instead of a denial). All three mean the same thing to the user.
fn wrong_env_hint(raw: &str) -> Option<&'static str> {
    let low = raw.to_lowercase();
    (low.contains("accessdenied")
        || low.contains("access denied")
        || low.contains("not authorized")
        || low.contains("create a landing zone first")
        || low.contains("awscontroltoweradmin"))
    .then_some(
        "no landing zone visible here — Control Tower answers only from the management \
         (or delegated-admin) account, in the landing zone's home region",
    )
}

/// Map raw Control Tower SDK errors to actionable hints.
fn friendly_error(raw: &str) -> String {
    match wrong_env_hint(raw) {
        Some(hint) => format!("Control Tower: {}.", hint),
        None => format!("Failed to load Control Tower: {}", raw),
    }
}

/// Emit a phase-failure warning. Every phase hits the same wall from a
/// non-management account, so the wrong-account/region class collapses to a
/// single hint instead of five near-identical SDK errors concatenated into one
/// status line; anything else warns per phase with the real message.
fn warn_phase(
    event_tx: &mpsc::UnboundedSender<Event>,
    service_type: ServiceType,
    env_warned: &mut bool,
    label: &str,
    raw: &str,
) {
    let warning = match wrong_env_hint(raw) {
        Some(hint) => {
            if *env_warned {
                return;
            }
            *env_warned = true;
            format!("Control Tower: {}", hint)
        }
        None => format!("{}: {}", label, raw),
    };
    let _ = event_tx.send(Event::ResourceLoadWarning {
        service: service_type,
        warning,
    });
}

fn fmt_time(t: &aws_smithy_types::DateTime) -> String {
    crate::aws::services::cloudwatch::fmt_epoch_secs(t.secs())
}

/// Trailing segment of an organizations ARN / slash path — the bare
/// `ou-…` / `r-…` / 12-digit account id.
pub(crate) fn arn_tail(s: &str) -> &str {
    s.rsplit('/').next().unwrap_or(s)
}

/// Resolve a target identifier (an organizations ARN) to `id (name)` when the
/// phase-0 map knows the name, else the bare trailing id.
fn resolve_target(target_arn: &str, names: &HashMap<String, String>) -> Option<String> {
    let tail = arn_tail(target_arn);
    names.get(tail).map(|n| format!("{} ({})", tail, n))
}

use crate::aws::document::{document_display, document_to_json};

// ── Control Catalog (phase-0 fetch: enrichment map + Catalog sub-tab) ───────

/// One catalog control's metadata — enriches the matching enabled control
/// and backs one Catalog sub-tab row.
#[derive(Debug, Clone, Default)]
pub struct CatalogMeta {
    pub arn: String,
    pub name: String,
    pub description: String,
    pub behavior: Option<String>,
    pub severity: Option<String>,
    pub implementation_type: Option<String>,
    pub implementation_identifier: Option<String>,
    pub aliases: Vec<String>,
    pub governed_resources: Vec<String>,
}

/// One account as Organizations reports it — the raw material for the
/// Accounts sub-tab, before Control Tower's own enrollment/compliance data is
/// folded in.
#[derive(Debug, Clone)]
pub struct OrgAccountInfo {
    pub id: String,
    pub name: String,
    pub email: Option<String>,
    pub status: Option<String>,
    pub joined_method: Option<String>,
    pub joined_at: Option<String>,
}

/// The phase-0 organizations sweep (see `fetch_org_map`).
#[derive(Debug, Default)]
pub struct OrgMap {
    /// Bare id → display name, for every root / OU / account. Target rows
    /// across all the Control Tower panes render through this.
    pub names: HashMap<String, String>,
    pub accounts: Vec<OrgAccountInfo>,
    /// Account id → (parent container id, display path). Best-effort: an org
    /// read failure leaves accounts unparented rather than dropping them.
    pub parents: HashMap<String, (String, String)>,
}

/// The fetched catalog plus a lookup index over ARN / ARN tail / aliases —
/// enabled-control identifiers come in both the legacy
/// `…:controltower:…:control/AWS-GR_X` and the Control Catalog UUID form,
/// and the aliases cover the legacy names.
#[derive(Debug, Default)]
pub struct ControlCatalog {
    pub metas: Vec<CatalogMeta>,
    index: HashMap<String, usize>,
}

impl ControlCatalog {
    fn push(&mut self, meta: CatalogMeta) {
        let idx = self.metas.len();
        self.index.insert(meta.arn.clone(), idx);
        self.index.insert(arn_tail(&meta.arn).to_string(), idx);
        for alias in &meta.aliases {
            self.index.insert(alias.clone(), idx);
        }
        self.metas.push(meta);
    }

    pub fn lookup(&self, control_identifier: &str) -> Option<&CatalogMeta> {
        self.index
            .get(control_identifier)
            .or_else(|| self.index.get(arn_tail(control_identifier)))
            .map(|&i| &self.metas[i])
    }

    /// Resolve the Config rule name a guardrail publishes compliance under
    /// (`AWSControlTower_AWS-GR_ENCRYPTED_VOLUMES`) back to its catalog entry.
    /// The rule name is the control's legacy alias behind a fixed prefix —
    /// except some landing-zone versions append a per-deployment suffix
    /// (`…-a1b2c3d4`), so an exact miss retries without the trailing segment.
    /// A wrong guess simply misses (the fallback string is still the slug).
    pub fn lookup_config_rule(&self, rule_name: &str) -> Option<&CatalogMeta> {
        let alias = rule_name
            .strip_prefix("AWSControlTower_")
            .unwrap_or(rule_name);
        self.lookup(alias).or_else(|| {
            alias
                .rsplit_once('-')
                .and_then(|(head, _suffix)| self.lookup(head))
        })
    }
}

// ── LandingZone ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct LandingZone {
    pub arn: String,
    pub version: String,
    pub latest_available_version: Option<String>,
    pub status: Option<String>,
    pub drift_status: Option<String>,
    pub remediation_types: Vec<String>,
    pub manifest_pretty: Option<String>,
    pub manifest_raw: Option<String>,
    /// A failed `GetLandingZone` lands here — the row survives on the
    /// `ListLandingZones` summary alone.
    pub detail_error: Option<String>,
    tags: HashMap<String, String>,
}

impl LandingZone {
    pub fn from_sdk(
        arn: &str,
        detail: Option<&aws_sdk_controltower::types::LandingZoneDetail>,
    ) -> Self {
        let manifest_json = detail.map(|d| document_to_json(d.manifest()));
        Self {
            arn: arn.to_string(),
            version: detail.map(|d| d.version().to_string()).unwrap_or_default(),
            latest_available_version: detail
                .and_then(|d| d.latest_available_version())
                .map(|s| s.to_string()),
            status: detail
                .and_then(|d| d.status())
                .map(|s| s.as_str().to_string()),
            drift_status: detail
                .and_then(|d| d.drift_status())
                .and_then(|d| d.status())
                .map(|s| s.as_str().to_string()),
            remediation_types: detail
                .map(|d| {
                    d.remediation_types()
                        .iter()
                        .map(|r| r.as_str().to_string())
                        .collect()
                })
                .unwrap_or_default(),
            manifest_pretty: manifest_json
                .as_ref()
                .and_then(|j| serde_json::to_string_pretty(j).ok()),
            manifest_raw: manifest_json
                .as_ref()
                .and_then(|j| serde_json::to_string_pretty(j).ok()),
            detail_error: None,
            tags: HashMap::new(),
        }
    }

    /// The landing zone runs a version behind the latest available.
    pub fn version_outdated(&self) -> bool {
        match &self.latest_available_version {
            Some(latest) => !self.version.is_empty() && self.version != *latest,
            None => false,
        }
    }
}

crate::sections! {
    pub enum LandingZoneDetailSection,
    pub static LANDING_ZONE_SECTIONS = [
        Overview "Overview",
        Manifest "Manifest",
        Operations "Operations",
        Tags "Tags" => crate::app::App::trigger_tower_tags_load,
    ]
}

impl Resource for LandingZone {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&LANDING_ZONE_SECTIONS)
    }

    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws controltower get-landing-zone --landing-zone-identifier {}",
            crate::aws::resource::shell_quote(&self.arn),
        ))
    }

    fn id(&self) -> &str {
        &self.arn
    }

    fn name(&self) -> &str {
        "Landing Zone"
    }

    fn resource_type(&self) -> &str {
        "Landing Zone"
    }

    fn state(&self) -> ResourceState {
        match self.status.as_deref() {
            Some("FAILED") => ResourceState::Unavailable,
            _ if self.drift_status.as_deref() == Some("DRIFTED") => ResourceState::Unavailable,
            Some("PROCESSING") => ResourceState::Pending,
            _ if self.version_outdated() => ResourceState::Pending,
            Some("ACTIVE") => ResourceState::Available,
            _ => ResourceState::Unknown(String::new()),
        }
    }

    fn state_label(&self) -> String {
        // Same ladder as state(): drift and an outdated version outrank a
        // healthy ACTIVE status.
        match self.status.as_deref() {
            Some("FAILED") => "failed".to_string(),
            _ if self.drift_status.as_deref() == Some("DRIFTED") => "drifted".to_string(),
            Some("PROCESSING") => "processing".to_string(),
            _ if self.version_outdated() => "outdated".to_string(),
            _ => native_state_label(self.status.as_deref().unwrap_or(""), || self.state()),
        }
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        let mut text = format!(
            "landing zone {} {} {}",
            self.version,
            self.status.as_deref().unwrap_or_default(),
            self.drift_status.as_deref().unwrap_or_default(),
        );
        if self.version_outdated() {
            text.push_str(" outdated");
        }
        text
    }

    fn details(&self) -> Vec<(String, String)> {
        let mut rows = vec![
            ("ARN".to_string(), self.arn.clone()),
            ("Version".to_string(), self.version.clone()),
        ];
        if let Some(s) = &self.status {
            rows.push(("Status".to_string(), s.clone()));
        }
        if let Some(d) = &self.drift_status {
            rows.push(("Drift".to_string(), d.clone()));
        }
        rows
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/controltower/home/dashboard?region={}",
            region, region
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── EnabledControl ──────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct EnabledControl {
    /// The enabled-control ARN (this enablement, not the control itself).
    pub arn: String,
    pub control_identifier: String,
    /// Catalog friendly name when resolved, else the identifier's last
    /// segment (legacy `AWS-GR_*` slugs read well; an unresolved Control
    /// Catalog UUID stays opaque).
    pub control_name: String,
    /// Catalog enrichment (empty when the catalog fetch failed or the
    /// identifier didn't resolve).
    pub description: Option<String>,
    pub behavior: Option<String>,
    pub severity: Option<String>,
    pub implementation: Option<String>,
    pub target_identifier: String,
    /// `id (name)` when the phase-0 org map resolved the target.
    pub target_name: Option<String>,
    pub parent_identifier: Option<String>,
    pub status: Option<String>,
    pub drift_status: Option<String>,
    pub last_operation_id: Option<String>,
    tags: HashMap<String, String>,
}

impl EnabledControl {
    pub fn from_sdk(
        s: &aws_sdk_controltower::types::EnabledControlSummary,
        target_names: &HashMap<String, String>,
        catalog: &ControlCatalog,
    ) -> Self {
        let control_identifier = s.control_identifier().unwrap_or_default().to_string();
        let target_identifier = s.target_identifier().unwrap_or_default().to_string();
        let meta = catalog.lookup(&control_identifier);
        Self {
            arn: s.arn().unwrap_or_default().to_string(),
            control_name: meta
                .map(|m| m.name.clone())
                .unwrap_or_else(|| arn_tail(&control_identifier).to_string()),
            description: meta.map(|m| m.description.clone()),
            behavior: meta.and_then(|m| m.behavior.clone()),
            severity: meta.and_then(|m| m.severity.clone()),
            implementation: meta.and_then(implementation_label),
            target_name: resolve_target(&target_identifier, target_names),
            control_identifier,
            target_identifier,
            parent_identifier: s.parent_identifier().map(|p| p.to_string()),
            status: s
                .status_summary()
                .and_then(|ss| ss.status())
                .map(|v| v.as_str().to_string()),
            drift_status: s
                .drift_status_summary()
                .and_then(|ds| ds.drift_status())
                .map(|v| v.as_str().to_string()),
            last_operation_id: s
                .status_summary()
                .and_then(|ss| ss.last_operation_identifier())
                .map(|v| v.to_string()),
            tags: HashMap::new(),
        }
    }
}

/// `AWS::Config::ConfigRule (rule-id)`-style implementation row.
fn implementation_label(m: &CatalogMeta) -> Option<String> {
    match (&m.implementation_type, &m.implementation_identifier) {
        (Some(t), Some(id)) => Some(format!("{} ({})", t, id)),
        (Some(t), None) => Some(t.clone()),
        _ => None,
    }
}

crate::sections! {
    pub enum EnabledControlDetailSection,
    pub static ENABLED_CONTROL_SECTIONS = [
        Overview "Overview",
        Parameters "Parameters" => crate::app::App::trigger_tower_control_detail_load,
        Tags "Tags" => crate::app::App::trigger_tower_tags_load,
    ]
}

impl Resource for EnabledControl {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&ENABLED_CONTROL_SECTIONS)
    }

    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws controltower get-enabled-control --enabled-control-identifier {}",
            crate::aws::resource::shell_quote(&self.arn),
        ))
    }

    fn id(&self) -> &str {
        &self.arn
    }

    fn name(&self) -> &str {
        &self.control_name
    }

    fn resource_type(&self) -> &str {
        "Enabled Control"
    }

    fn state(&self) -> ResourceState {
        if self.drift_status.as_deref() == Some("DRIFTED") {
            return ResourceState::Unavailable;
        }
        match self.status.as_deref() {
            Some("FAILED") => ResourceState::Unavailable,
            Some("UNDER_CHANGE") => ResourceState::Pending,
            Some("SUCCEEDED") => ResourceState::Available,
            _ => ResourceState::Unknown(String::new()),
        }
    }

    fn state_label(&self) -> String {
        if self.drift_status.as_deref() == Some("DRIFTED") {
            return "drifted".to_string();
        }
        native_state_label(self.status.as_deref().unwrap_or(""), || self.state())
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        let mut text = format!(
            "{} {} {} {} {} {}",
            self.control_name,
            self.control_identifier,
            self.target_name.as_deref().unwrap_or_default(),
            arn_tail(&self.target_identifier),
            self.severity.as_deref().unwrap_or_default(),
            self.behavior.as_deref().unwrap_or_default(),
        );
        if self.drift_status.as_deref() == Some("DRIFTED") {
            text.push_str(" drifted");
        }
        if let Some(s) = &self.status {
            text.push(' ');
            text.push_str(s);
        }
        text
    }

    fn details(&self) -> Vec<(String, String)> {
        let mut rows = vec![
            ("Control".to_string(), self.control_name.clone()),
            ("Target".to_string(), self.target_identifier.clone()),
        ];
        if let Some(t) = &self.target_name {
            rows.push(("Target Name".to_string(), t.clone()));
        }
        if let Some(s) = &self.status {
            rows.push(("Status".to_string(), s.clone()));
        }
        if let Some(d) = &self.drift_status {
            rows.push(("Drift".to_string(), d.clone()));
        }
        rows
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/controltower/home/controls?region={}",
            region, region
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── EnabledBaseline ─────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct EnabledBaseline {
    pub arn: String,
    pub baseline_identifier: String,
    /// Friendly name resolved from the phase-0 `ListBaselines` catalog map.
    pub baseline_name: Option<String>,
    pub baseline_version: Option<String>,
    pub target_identifier: String,
    pub target_name: Option<String>,
    pub parent_identifier: Option<String>,
    pub status: Option<String>,
    /// Inheritance drift — the only drift type baselines report.
    pub drift_status: Option<String>,
    tags: HashMap<String, String>,
}

impl EnabledBaseline {
    pub fn from_sdk(
        s: &aws_sdk_controltower::types::EnabledBaselineSummary,
        baseline_names: &HashMap<String, String>,
        target_names: &HashMap<String, String>,
    ) -> Self {
        Self {
            arn: s.arn().to_string(),
            baseline_name: baseline_names.get(s.baseline_identifier()).cloned(),
            baseline_identifier: s.baseline_identifier().to_string(),
            baseline_version: s.baseline_version().map(|v| v.to_string()),
            target_name: resolve_target(s.target_identifier(), target_names),
            target_identifier: s.target_identifier().to_string(),
            parent_identifier: s.parent_identifier().map(|p| p.to_string()),
            status: s
                .status_summary()
                .and_then(|ss| ss.status())
                .map(|v| v.as_str().to_string()),
            drift_status: s
                .drift_status_summary()
                .and_then(|d| d.types())
                .and_then(|t| t.inheritance())
                .and_then(|i| i.status())
                .map(|v| v.as_str().to_string()),
            tags: HashMap::new(),
        }
    }

    /// Row label: friendly catalog name, else the identifier's trailing
    /// segment.
    pub fn display_name(&self) -> &str {
        self.baseline_name
            .as_deref()
            .unwrap_or_else(|| arn_tail(&self.baseline_identifier))
    }
}

crate::sections! {
    pub enum EnabledBaselineDetailSection,
    pub static ENABLED_BASELINE_SECTIONS = [
        Overview "Overview",
        Parameters "Parameters" => crate::app::App::trigger_tower_baseline_detail_load,
        Tags "Tags" => crate::app::App::trigger_tower_tags_load,
    ]
}

impl Resource for EnabledBaseline {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&ENABLED_BASELINE_SECTIONS)
    }

    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws controltower get-enabled-baseline --enabled-baseline-identifier {}",
            crate::aws::resource::shell_quote(&self.arn),
        ))
    }

    fn id(&self) -> &str {
        &self.arn
    }

    fn name(&self) -> &str {
        self.display_name()
    }

    fn resource_type(&self) -> &str {
        "Enabled Baseline"
    }

    fn state(&self) -> ResourceState {
        if self.drift_status.as_deref() == Some("DRIFTED") {
            return ResourceState::Unavailable;
        }
        match self.status.as_deref() {
            Some("FAILED") => ResourceState::Unavailable,
            Some("UNDER_CHANGE") => ResourceState::Pending,
            Some("SUCCEEDED") => ResourceState::Available,
            _ => ResourceState::Unknown(String::new()),
        }
    }

    fn state_label(&self) -> String {
        if self.drift_status.as_deref() == Some("DRIFTED") {
            return "drifted".to_string();
        }
        native_state_label(self.status.as_deref().unwrap_or(""), || self.state())
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!(
            "{} {} {} {} {}",
            self.display_name(),
            self.baseline_identifier,
            self.baseline_version.as_deref().unwrap_or_default(),
            self.target_name.as_deref().unwrap_or_default(),
            arn_tail(&self.target_identifier),
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        let mut rows = vec![
            ("Baseline".to_string(), self.display_name().to_string()),
            ("Target".to_string(), self.target_identifier.clone()),
        ];
        if let Some(v) = &self.baseline_version {
            rows.push(("Version".to_string(), v.clone()));
        }
        if let Some(s) = &self.status {
            rows.push(("Status".to_string(), s.clone()));
        }
        rows
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/controltower/home/dashboard?region={}",
            region, region
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── CtOp (unified control + landing-zone operations) ────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CtOpKind {
    Control,
    LandingZone,
}

#[derive(Debug, Clone)]
pub struct CtOp {
    pub kind: CtOpKind,
    pub operation_identifier: String,
    pub operation_type: String,
    pub status: Option<String>,
    pub status_message: Option<String>,
    pub start_time: Option<String>,
    pub end_time: Option<String>,
    /// Epoch seconds for the merged newest-first sort (LZ ops only get one
    /// after the capped `GetLandingZoneOperation` enrichment).
    pub start_secs: Option<i64>,
    pub control_identifier: Option<String>,
    pub control_name: Option<String>,
    pub target_identifier: Option<String>,
    pub target_name: Option<String>,
    pub enabled_control_identifier: Option<String>,
    tags: HashMap<String, String>,
}

impl CtOp {
    pub fn from_control_op(
        s: &aws_sdk_controltower::types::ControlOperationSummary,
        target_names: &HashMap<String, String>,
    ) -> Self {
        let control_identifier = s.control_identifier().map(|v| v.to_string());
        let target_identifier = s.target_identifier().map(|v| v.to_string());
        Self {
            kind: CtOpKind::Control,
            operation_identifier: s.operation_identifier().unwrap_or_default().to_string(),
            operation_type: s
                .operation_type()
                .map(|t| t.as_str().to_string())
                .unwrap_or_default(),
            status: s.status().map(|v| v.as_str().to_string()),
            status_message: s.status_message().map(|v| v.to_string()),
            start_time: s.start_time().map(fmt_time),
            end_time: s.end_time().map(fmt_time),
            start_secs: s.start_time().map(|t| t.secs()),
            control_name: control_identifier.as_deref().map(|c| arn_tail(c).to_string()),
            target_name: target_identifier
                .as_deref()
                .and_then(|t| resolve_target(t, target_names)),
            control_identifier,
            target_identifier,
            enabled_control_identifier: s.enabled_control_identifier().map(|v| v.to_string()),
            tags: HashMap::new(),
        }
    }

    pub fn from_lz_op(s: &aws_sdk_controltower::types::LandingZoneOperationSummary) -> Self {
        Self {
            kind: CtOpKind::LandingZone,
            operation_identifier: s.operation_identifier().unwrap_or_default().to_string(),
            operation_type: s
                .operation_type()
                .map(|t| t.as_str().to_string())
                .unwrap_or_default(),
            status: s.status().map(|v| v.as_str().to_string()),
            status_message: None,
            start_time: None,
            end_time: None,
            start_secs: None,
            control_identifier: None,
            control_name: None,
            target_identifier: None,
            target_name: None,
            enabled_control_identifier: None,
            tags: HashMap::new(),
        }
    }

    pub fn kind_label(&self) -> &'static str {
        match self.kind {
            CtOpKind::Control => "Control",
            CtOpKind::LandingZone => "Landing Zone",
        }
    }
}

impl Resource for CtOp {
    fn cli_command(&self) -> Option<String> {
        let verb = match self.kind {
            CtOpKind::Control => "get-control-operation",
            CtOpKind::LandingZone => "get-landing-zone-operation",
        };
        Some(format!(
            "aws controltower {} --operation-identifier {}",
            verb,
            crate::aws::resource::shell_quote(&self.operation_identifier),
        ))
    }

    fn id(&self) -> &str {
        &self.operation_identifier
    }

    fn name(&self) -> &str {
        &self.operation_type
    }

    fn resource_type(&self) -> &str {
        "Control Tower Operation"
    }

    fn state(&self) -> ResourceState {
        match self.status.as_deref() {
            Some("FAILED") => ResourceState::Unavailable,
            Some("IN_PROGRESS") => ResourceState::Pending,
            Some("SUCCEEDED") => ResourceState::Available,
            _ => ResourceState::Unknown(String::new()),
        }
    }

    fn state_label(&self) -> String {
        native_state_label(self.status.as_deref().unwrap_or(""), || self.state())
    }

    /// Succeeded operations are routine churn — `a` hides them (noise shows
    /// by default app-wide).
    fn is_noise(&self) -> bool {
        self.status.as_deref() == Some("SUCCEEDED")
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!(
            "{} {} {} {} {} {}",
            self.operation_type,
            self.status.as_deref().unwrap_or_default(),
            self.kind_label(),
            self.control_name.as_deref().unwrap_or_default(),
            self.target_name.as_deref().unwrap_or_default(),
            self.operation_identifier,
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        let mut rows = vec![
            ("Operation".to_string(), self.operation_type.clone()),
            ("Kind".to_string(), self.kind_label().to_string()),
            ("Operation ID".to_string(), self.operation_identifier.clone()),
        ];
        if let Some(s) = &self.status {
            rows.push(("Status".to_string(), s.clone()));
        }
        if let Some(m) = &self.status_message {
            rows.push(("Message".to_string(), m.clone()));
        }
        if let Some(t) = &self.start_time {
            rows.push(("Started".to_string(), t.clone()));
        }
        if let Some(t) = &self.end_time {
            rows.push(("Ended".to_string(), t.clone()));
        }
        if let Some(c) = &self.control_name {
            rows.push(("Control".to_string(), c.clone()));
        }
        if let Some(c) = &self.control_identifier {
            rows.push(("Control Identifier".to_string(), c.clone()));
        }
        if let Some(t) = &self.target_identifier {
            rows.push(("Target".to_string(), t.clone()));
        }
        if let Some(t) = &self.target_name {
            rows.push(("Target Name".to_string(), t.clone()));
        }
        if let Some(e) = &self.enabled_control_identifier {
            rows.push(("Enabled Control".to_string(), e.clone()));
        }
        rows
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── Lazy details (GetEnabledControl / GetEnabledBaseline / tags) ────────────

#[derive(Debug, Clone)]
pub struct CtEnabledControlDetail {
    pub parameters: Vec<(String, String)>,
    pub target_regions: Vec<String>,
    pub status: Option<String>,
    pub drift_status: Option<String>,
}

pub async fn fetch_enabled_control_detail(
    client: CtClient,
    arn: String,
) -> Result<CtEnabledControlDetail> {
    let resp = client
        .get_enabled_control()
        .enabled_control_identifier(&arn)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
    let d = resp.enabled_control_details();
    Ok(CtEnabledControlDetail {
        parameters: d
            .map(|d| {
                d.parameters()
                    .iter()
                    .map(|p| (p.key().to_string(), document_display(p.value())))
                    .collect()
            })
            .unwrap_or_default(),
        target_regions: d
            .map(|d| {
                d.target_regions()
                    .iter()
                    .filter_map(|r| r.name().map(|n| n.to_string()))
                    .collect()
            })
            .unwrap_or_default(),
        status: d
            .and_then(|d| d.status_summary())
            .and_then(|ss| ss.status())
            .map(|v| v.as_str().to_string()),
        drift_status: d
            .and_then(|d| d.drift_status_summary())
            .and_then(|ds| ds.drift_status())
            .map(|v| v.as_str().to_string()),
    })
}

#[derive(Debug, Clone)]
pub struct CtEnabledBaselineDetail {
    pub parameters: Vec<(String, String)>,
    pub status: Option<String>,
    pub drift_status: Option<String>,
}

pub async fn fetch_enabled_baseline_detail(
    client: CtClient,
    arn: String,
) -> Result<CtEnabledBaselineDetail> {
    let resp = client
        .get_enabled_baseline()
        .enabled_baseline_identifier(&arn)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
    let d = resp.enabled_baseline_details();
    Ok(CtEnabledBaselineDetail {
        parameters: d
            .map(|d| {
                d.parameters()
                    .iter()
                    .map(|p| (p.key().to_string(), document_display(p.value())))
                    .collect()
            })
            .unwrap_or_default(),
        status: d
            .and_then(|d| d.status_summary())
            .and_then(|ss| ss.status())
            .map(|v| v.as_str().to_string()),
        drift_status: d
            .and_then(|d| d.drift_status_summary())
            .and_then(|ds| ds.types())
            .and_then(|t| t.inheritance())
            .and_then(|i| i.status())
            .map(|v| v.as_str().to_string()),
    })
}

/// `ListTagsForResource` on any Control Tower ARN (landing zone / enabled
/// control / enabled baseline — one lazy map serves all three panes).
pub async fn fetch_tower_tags(client: CtClient, arn: String) -> Result<Vec<(String, String)>> {
    let resp = client
        .list_tags_for_resource()
        .resource_arn(&arn)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
    let mut tags: Vec<(String, String)> = resp
        .tags()
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    tags.sort();
    Ok(tags)
}

// ── CatalogControl (Catalog sub-tab: the full controls library) ─────────────

#[derive(Debug, Clone)]
pub struct CatalogControl {
    pub meta: CatalogMeta,
    /// Targets this control is enabled on (from the enabled-controls phase);
    /// empty = available but not enabled.
    pub enabled_targets: Vec<String>,
    tags: HashMap<String, String>,
}

impl CatalogControl {
    pub fn new(meta: CatalogMeta, enabled_targets: Vec<String>) -> Self {
        Self {
            meta,
            enabled_targets,
            tags: HashMap::new(),
        }
    }
}

/// Word-wrap for the catalog description content lines (the detail pane
/// doesn't wrap long plain rows).
pub(crate) fn wrap_words(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if !line.is_empty() && line.len() + 1 + word.len() > width {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

impl Resource for CatalogControl {
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws controlcatalog get-control --control-arn {}",
            crate::aws::resource::shell_quote(&self.meta.arn),
        ))
    }

    fn id(&self) -> &str {
        &self.meta.arn
    }

    fn name(&self) -> &str {
        &self.meta.name
    }

    fn resource_type(&self) -> &str {
        "Catalog Control"
    }

    fn state(&self) -> ResourceState {
        if self.enabled_targets.is_empty() {
            ResourceState::Unknown(String::new())
        } else {
            ResourceState::Available
        }
    }

    fn state_label(&self) -> String {
        if self.enabled_targets.is_empty() {
            "not enabled".to_string()
        } else {
            "enabled".to_string()
        }
    }

    /// Not-enabled catalog controls are reference data — `a` narrows the
    /// library to what's actually enabled.
    fn is_noise(&self) -> bool {
        self.enabled_targets.is_empty()
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        let mut text = format!(
            "{} {} {} {}",
            self.meta.name,
            self.meta.aliases.join(" "),
            self.meta.behavior.as_deref().unwrap_or_default(),
            self.meta.severity.as_deref().unwrap_or_default(),
        );
        if !self.enabled_targets.is_empty() {
            text.push_str(" enabled");
        }
        text
    }

    fn details(&self) -> Vec<(String, String)> {
        let mut rows = vec![("Control".to_string(), self.meta.name.clone())];
        if let Some(s) = &self.meta.severity {
            rows.push(("Severity".to_string(), s.clone()));
        }
        if let Some(b) = &self.meta.behavior {
            rows.push(("Behavior".to_string(), b.clone()));
        }
        if let Some(i) = implementation_label(&self.meta) {
            rows.push(("Implementation".to_string(), i));
        }
        rows.push(("ARN".to_string(), self.meta.arn.clone()));
        if !self.meta.aliases.is_empty() {
            rows.push(("Aliases".to_string(), self.meta.aliases.join(", ")));
        }
        if !self.meta.description.is_empty() {
            rows.push((String::new(), String::new()));
            rows.push(("Description".to_string(), String::new())); // group header
            for line in wrap_words(&self.meta.description, 90) {
                rows.push((format!(" {}", line), String::new()));
            }
        }
        if !self.meta.governed_resources.is_empty() {
            rows.push((String::new(), String::new()));
            rows.push(("Governed Resources".to_string(), String::new())); // group header
            for r in &self.meta.governed_resources {
                rows.push((format!(" {}", r), String::new()));
            }
        }
        rows.push((String::new(), String::new()));
        if self.enabled_targets.is_empty() {
            rows.push(("Enabled".to_string(), "not enabled in this landing zone".to_string()));
        } else {
            rows.push((
                format!("Enabled On ({})", self.enabled_targets.len()),
                String::new(),
            )); // group header
            for t in &self.enabled_targets {
                rows.push((format!(" {}", t), String::new()));
            }
        }
        rows
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Split an aggregated Config rule name into `(source, display label)`.
///
/// The org aggregator mixes deployers, each with its own naming scheme:
/// `AWSControlTower_AWS-GR_ENCRYPTED_VOLUMES` (detective guardrails),
/// `securityhub-s3-bucket-ssl-requests-only-d49ab884` (Security Hub standards,
/// with a generated 8-hex suffix), and anything else a team deployed. The
/// prefixes and suffixes are noise once the source is named separately, so
/// they're stripped for the row label.
pub(crate) fn classify_rule(rule_name: &str) -> (&'static str, String) {
    if let Some(rest) = rule_name.strip_prefix("AWSControlTower_") {
        return ("Control Tower", rest.to_string());
    }
    if let Some(rest) = rule_name.strip_prefix("securityhub-") {
        return ("Security Hub", strip_generated_suffix(rest).to_string());
    }
    ("Config", rule_name.to_string())
}

/// Drop a trailing `-<8 hex>` disambiguator (Security Hub appends one per
/// deployed rule). Anything else is left alone — a real name segment must not
/// be eaten.
fn strip_generated_suffix(name: &str) -> &str {
    match name.rsplit_once('-') {
        Some((head, suffix))
            if suffix.len() == 8 && suffix.chars().all(|c| c.is_ascii_hexdigit()) =>
        {
            head
        }
        _ => name,
    }
}

// ── CtAccount (Accounts sub-tab: the org's enrolled accounts) ───────────────

/// One org account as Control Tower governs it — the console's Organization
/// page in a row: where it sits in the OU tree, whether a baseline is enrolled
/// on it, how many controls apply, and how its detective controls are doing.
///
/// Derived **entirely from data the earlier phases already fetched** (the
/// phase-0 org sweep, the enabled-controls and baselines phases, and the
/// compliance rows) — the Vpc-Subnets sibling-filtering idea taken one step
/// further, so a whole sub-tab costs zero extra API calls.
#[derive(Debug, Clone)]
pub struct CtAccount {
    pub account_id: String,
    pub account_name: String,
    pub email: Option<String>,
    pub status: Option<String>,
    pub joined_method: Option<String>,
    pub joined_at: Option<String>,
    pub ou_id: Option<String>,
    pub ou_path: Option<String>,
    /// A baseline enabled directly on this account — Control Tower's own
    /// definition of "enrolled".
    pub enrolled: bool,
    pub baseline_status: Option<String>,
    pub baseline_drift: Option<String>,
    /// Controls applying to this account: enabled on it directly or on the OU
    /// that contains it.
    pub controls_applied: usize,
    pub controls_drifted: usize,
    pub compliant: usize,
    pub noncompliant: usize,
    tags: HashMap<String, String>,
}

impl CtAccount {
    /// Fold the phases' by-target rollups into one account row. `by_target` is
    /// keyed by bare id (account **or** OU), matching `arn_tail` of the
    /// organizations ARNs Control Tower reports targets as.
    pub fn build(
        info: &OrgAccountInfo,
        parents: &HashMap<String, (String, String)>,
        baselines: &HashMap<String, (Option<String>, Option<String>)>,
        controls: &HashMap<String, (usize, usize)>,
        compliance: &HashMap<String, (usize, usize)>,
    ) -> Self {
        let parent = parents.get(&info.id);
        let ou_id = parent.map(|(id, _)| id.clone());
        let baseline = baselines.get(&info.id);
        // Controls land on the OU in the common case; a control enabled
        // directly on the account counts too.
        let (mut applied, mut drifted) = controls.get(&info.id).copied().unwrap_or((0, 0));
        if let Some(ou) = &ou_id {
            let (a, d) = controls.get(ou).copied().unwrap_or((0, 0));
            applied += a;
            drifted += d;
        }
        let (compliant, noncompliant) = compliance.get(&info.id).copied().unwrap_or((0, 0));
        Self {
            account_id: info.id.clone(),
            account_name: info.name.clone(),
            email: info.email.clone(),
            status: info.status.clone(),
            joined_method: info.joined_method.clone(),
            joined_at: info.joined_at.clone(),
            ou_path: parent.map(|(_, path)| path.clone()),
            ou_id,
            enrolled: baseline.is_some(),
            baseline_status: baseline.and_then(|(s, _)| s.clone()),
            baseline_drift: baseline.and_then(|(_, d)| d.clone()),
            controls_applied: applied,
            controls_drifted: drifted,
            compliant,
            noncompliant,
            tags: HashMap::new(),
        }
    }

    /// True when anything about this account wants attention — drives both the
    /// row colour and `is_noise`.
    fn needs_attention(&self) -> bool {
        self.noncompliant > 0
            || self.controls_drifted > 0
            || !self.enrolled
            || self.baseline_drift.as_deref() == Some(DRIFTED)
            || self
                .baseline_status
                .as_deref()
                .is_some_and(|s| s.contains("FAILED"))
    }
}

crate::sections! {
    pub enum CtAccountDetailSection,
    pub static CT_ACCOUNT_SECTIONS = [
        Overview "Overview",
        Controls "Controls",
        Compliance "Compliance",
    ]
}

impl Resource for CtAccount {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&CT_ACCOUNT_SECTIONS)
    }

    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws organizations describe-account --account-id {}",
            crate::aws::resource::shell_quote(&self.account_id),
        ))
    }

    fn id(&self) -> &str {
        &self.account_id
    }

    fn name(&self) -> &str {
        &self.account_name
    }

    fn resource_type(&self) -> &str {
        "Enrolled Account"
    }

    fn state(&self) -> ResourceState {
        // A suspended account is org-level dead — that outranks any Control
        // Tower finding about it.
        if self.status.as_deref() == Some("SUSPENDED") {
            return ResourceState::Stopped;
        }
        if self.noncompliant > 0 {
            return ResourceState::Unavailable;
        }
        if self.needs_attention() {
            return ResourceState::Pending;
        }
        ResourceState::Available
    }

    fn state_label(&self) -> String {
        // Mirrors state()'s ladder, then names which needs_attention() signal
        // fired (the search_text words).
        if self.status.as_deref() == Some("SUSPENDED") {
            "suspended".to_string()
        } else if self.noncompliant > 0 {
            "noncompliant".to_string()
        } else if self.controls_drifted > 0 || self.baseline_drift.as_deref() == Some(DRIFTED) {
            "drifted".to_string()
        } else if !self.enrolled {
            "not enrolled".to_string()
        } else if self.needs_attention() {
            "failed".to_string()
        } else {
            "compliant".to_string()
        }
    }

    /// Healthy, enrolled, fully compliant accounts are the expected case —
    /// `a` narrows the org to the accounts that need looking at.
    fn is_noise(&self) -> bool {
        !self.needs_attention() && self.status.as_deref() != Some("SUSPENDED")
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        let mut text = format!(
            "{} {} {} {} {}",
            self.account_id,
            self.account_name,
            self.email.as_deref().unwrap_or_default(),
            self.ou_path.as_deref().unwrap_or_default(),
            self.status.as_deref().unwrap_or_default(),
        );
        // Searchable words for the states the row colour is showing.
        if !self.enrolled {
            text.push_str(" not-enrolled");
        }
        if self.noncompliant > 0 {
            text.push_str(" noncompliant");
        }
        if self.controls_drifted > 0 || self.baseline_drift.as_deref() == Some(DRIFTED) {
            text.push_str(" drifted");
        }
        text
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Account".to_string(), self.account_id.clone()),
            ("Name".to_string(), self.account_name.clone()),
        ]
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── CtCompliance (Compliance sub-tab: the Config-aggregator guardrail view) ─

#[derive(Debug, Clone)]
pub struct CtCompliance {
    /// The discovered aggregator name — the lazy Resources drill needs it.
    pub aggregator: String,
    /// Account the aggregator was read from, when that wasn't the account
    /// being browsed (`controltower_audit_account`). The Resources drill has
    /// to assume the same account or it would query an aggregator that isn't
    /// there.
    pub source_account: Option<String>,
    pub rule_name: String,
    /// What deployed this rule (`classify_rule`) — the aggregator mixes
    /// Control Tower guardrails, Security Hub standards and custom rules, and
    /// which one it is changes where you'd go to fix it.
    pub source: &'static str,
    /// The rule name with its deployer's prefix and generated suffix stripped
    /// (`securityhub-s3-bucket-ssl-requests-only-d49ab884` →
    /// `s3-bucket-ssl-requests-only`).
    pub control_label: String,
    /// Friendly catalog name ("Enable encryption for EBS volumes"), when the
    /// rule resolved against the Control Catalog.
    pub control_name: Option<String>,
    pub control_description: Option<String>,
    pub severity: Option<String>,
    /// `<control> · <account>` — the list row label. The sub-tab spans every
    /// account, so the control alone doesn't identify a row (the
    /// `SfnExecution::name()` precedent).
    pub display_name: String,
    pub account_id: String,
    pub account_name: Option<String>,
    pub region: String,
    pub compliance: Option<String>,
    /// (count, cap_exceeded) of non-compliant resources, when reported.
    pub noncompliant_count: Option<(i32, bool)>,
    /// `rule|account|region` — the row id and the lazy-map key.
    pub key: String,
    tags: HashMap<String, String>,
}

impl CtCompliance {
    pub fn from_sdk(
        aggregator: &str,
        r: &aws_sdk_config::types::AggregateComplianceByConfigRule,
        target_names: &HashMap<String, String>,
        catalog: &ControlCatalog,
    ) -> Self {
        let rule_name = r.config_rule_name().unwrap_or_default().to_string();
        let account_id = r.account_id().unwrap_or_default().to_string();
        let region = r.aws_region().unwrap_or_default().to_string();
        let (source, control_label) = classify_rule(&rule_name);
        let meta = catalog.lookup_config_rule(&rule_name);
        let account_name = target_names.get(&account_id).cloned();
        // Prefer the catalog's prose over the slug, and the account's name
        // over its id — a list of a dozen 12-digit numbers is unreadable.
        let display_name = format!(
            "{} · {}",
            meta.map(|m| m.name.clone())
                .unwrap_or_else(|| control_label.clone()),
            account_name.clone().unwrap_or_else(|| account_id.clone()),
        );
        Self {
            aggregator: aggregator.to_string(),
            source_account: None,
            source,
            control_name: meta.map(|m| m.name.clone()),
            control_description: meta.map(|m| m.description.clone()),
            severity: meta.and_then(|m| m.severity.clone()),
            display_name,
            control_label,
            key: format!("{}|{}|{}", rule_name, account_id, region),
            account_name,
            compliance: r
                .compliance()
                .and_then(|c| c.compliance_type())
                .map(|t| t.as_str().to_string()),
            noncompliant_count: r
                .compliance()
                .and_then(|c| c.compliance_contributor_count())
                .map(|c| (c.capped_count(), c.cap_exceeded())),
            rule_name,
            account_id,
            region,
            tags: HashMap::new(),
        }
    }
}

crate::sections! {
    pub enum CtComplianceDetailSection,
    pub static CT_COMPLIANCE_SECTIONS = [
        Overview "Overview",
        Resources "Resources" => crate::app::App::trigger_tower_compliance_resources_load,
    ]
}

impl Resource for CtCompliance {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&CT_COMPLIANCE_SECTIONS)
    }

    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws configservice get-aggregate-compliance-details-by-config-rule \
             --configuration-aggregator-name {} --config-rule-name {} --account-id {} \
             --aws-region {}",
            crate::aws::resource::shell_quote(&self.aggregator),
            crate::aws::resource::shell_quote(&self.rule_name),
            crate::aws::resource::shell_quote(&self.account_id),
            crate::aws::resource::shell_quote(&self.region),
        ))
    }

    fn id(&self) -> &str {
        &self.key
    }

    fn name(&self) -> &str {
        &self.display_name
    }

    fn resource_type(&self) -> &str {
        "Compliance"
    }

    fn state(&self) -> ResourceState {
        match self.compliance.as_deref() {
            Some("NON_COMPLIANT") => ResourceState::Unavailable,
            Some("COMPLIANT") => ResourceState::Available,
            _ => ResourceState::Unknown(String::new()),
        }
    }

    fn state_label(&self) -> String {
        native_state_label(self.compliance.as_deref().unwrap_or(""), || self.state())
    }

    /// Compliant rows are the healthy baseline — `a` narrows to violations.
    fn is_noise(&self) -> bool {
        self.compliance.as_deref() == Some("COMPLIANT")
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!(
            "{} {} {} {} {} {} {} {} {}",
            self.control_label,
            self.control_name.as_deref().unwrap_or_default(),
            self.source,
            self.rule_name,
            self.account_id,
            self.account_name.as_deref().unwrap_or_default(),
            self.region,
            self.compliance.as_deref().unwrap_or_default(),
            self.severity.as_deref().unwrap_or_default(),
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        let mut rows = vec![
            ("Control".to_string(), self.control_label.clone()),
            ("Account".to_string(), self.account_id.clone()),
            ("Region".to_string(), self.region.clone()),
        ];
        if let Some(c) = &self.compliance {
            rows.push(("Compliance".to_string(), c.clone()));
        }
        rows
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// One non-compliant resource from the lazy Resources drill.
#[derive(Debug, Clone)]
pub struct CtComplianceResource {
    pub resource_type: String,
    pub resource_id: String,
    pub annotation: Option<String>,
    pub recorded: Option<String>,
}

/// Lazy Resources section: the non-compliant resources behind one
/// rule × account × region row, capped `MAX_COMPLIANCE_RESOURCES`.
pub async fn fetch_compliance_resources(
    client: aws_sdk_config::Client,
    aggregator: String,
    rule_name: String,
    account_id: String,
    region: String,
) -> Result<Vec<CtComplianceResource>> {
    let mut out = Vec::new();
    let mut paginator = client
        .get_aggregate_compliance_details_by_config_rule()
        .configuration_aggregator_name(&aggregator)
        .config_rule_name(&rule_name)
        .account_id(&account_id)
        .aws_region(&region)
        .compliance_type(aws_sdk_config::types::ComplianceType::NonCompliant)
        .into_paginator()
        .send();
    while let Some(page) = paginator.next().await {
        let p = page
            .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
        for r in p.aggregate_evaluation_results() {
            let qualifier = r
                .evaluation_result_identifier()
                .and_then(|i| i.evaluation_result_qualifier());
            out.push(CtComplianceResource {
                resource_type: qualifier
                    .and_then(|q| q.resource_type())
                    .unwrap_or_default()
                    .to_string(),
                resource_id: qualifier
                    .and_then(|q| q.resource_id())
                    .unwrap_or_default()
                    .to_string(),
                annotation: r.annotation().map(|a| a.to_string()),
                recorded: r.result_recorded_time().map(fmt_time),
            });
            if out.len() >= MAX_COMPLIANCE_RESOURCES {
                return Ok(out);
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The three real SDK messages a non-management account returns — all
    /// observed live from an audit account (sv-audit) assumed via
    /// `AWSControlTowerExecution`.
    #[test]
    fn wrong_env_covers_denial_missing_lz_and_admin_role() {
        for raw in [
            "AccessDeniedException: User: arn:aws:sts::1234:assumed-role/X/y is not authorized \
             to perform: controltower:ListLandingZones",
            "ResourceNotFoundException: AWS Control Tower cannot complete the operation, because \
             you must create a landing zone first. To continue, create your landing zone from \
             the console, or call the CreateLandingZone API.",
            "ValidationException: AWS Control Tower could not complete the operation because it \
             could not assume the 'AWSControlTowerAdmin' role. Check the configuration for this \
             role and try again.",
        ] {
            assert!(wrong_env_hint(raw).is_some(), "unclassified: {}", raw);
        }
    }

    fn catalog_with_alias(alias: &str) -> ControlCatalog {
        let mut catalog = ControlCatalog::default();
        catalog.push(CatalogMeta {
            arn: "arn:aws:controlcatalog:::control/abcd1234".to_string(),
            name: "Enable encryption for EBS volumes".to_string(),
            description: String::new(),
            behavior: None,
            severity: Some("HIGH".to_string()),
            implementation_type: None,
            implementation_identifier: None,
            aliases: vec![alias.to_string()],
            governed_resources: vec![],
        });
        catalog
    }

    /// Compliance rows arrive keyed by Config rule name, which is the control's
    /// legacy alias behind a fixed prefix — sometimes with a per-deployment
    /// suffix. Both forms must reach the catalog entry, or every row falls back
    /// to the raw slug.
    #[test]
    fn config_rule_names_resolve_to_catalog_controls() {
        let catalog = catalog_with_alias("AWS-GR_ENCRYPTED_VOLUMES");

        for rule in [
            "AWSControlTower_AWS-GR_ENCRYPTED_VOLUMES",
            "AWSControlTower_AWS-GR_ENCRYPTED_VOLUMES-a1b2c3d4",
        ] {
            assert_eq!(
                catalog.lookup_config_rule(rule).map(|m| m.name.as_str()),
                Some("Enable encryption for EBS volumes"),
                "unresolved: {}",
                rule,
            );
        }

        assert!(catalog
            .lookup_config_rule("AWSControlTower_AWS-GR_SOMETHING_ELSE")
            .is_none());
    }

    fn account(baseline_drift: &str, drifted_controls: usize) -> CtAccount {
        CtAccount::build(
            &OrgAccountInfo {
                id: "444455556666".to_string(),
                name: "nonprod".to_string(),
                email: None,
                status: Some("ACTIVE".to_string()),
                joined_method: None,
                joined_at: None,
            },
            &HashMap::from([(
                "444455556666".to_string(),
                ("ou-test".to_string(), "Root / NonProd".to_string()),
            )]),
            &HashMap::from([(
                "444455556666".to_string(),
                (
                    Some("SUCCEEDED".to_string()),
                    Some(baseline_drift.to_string()),
                ),
            )]),
            &HashMap::from([("ou-test".to_string(), (9, drifted_controls))]),
            &HashMap::new(),
        )
    }

    /// `NOT_CHECKING` is what most preventive controls report — they have no
    /// drift detection to do. Treating it as drift made every account in the
    /// org read as needing attention, which also broke `a` (nothing was noise).
    #[test]
    fn not_checking_drift_is_not_a_problem() {
        let healthy = account("NOT_CHECKING", 0);
        assert!(healthy.is_noise(), "a healthy account should be hideable");
        assert_eq!(healthy.state(), ResourceState::Available);
        assert!(!healthy.search_text().contains("drifted"));

        let drifted = account(DRIFTED, 0);
        assert!(!drifted.is_noise());
        assert_eq!(drifted.state(), ResourceState::Pending);
        assert!(drifted.search_text().contains("drifted"));

        // A genuinely drifted control still counts, whatever the baseline says.
        assert!(!account("NOT_CHECKING", 1).is_noise());
    }

    /// Real rule names from an org aggregator. The Security Hub form dominates
    /// in practice — a landing zone with only preventive guardrails publishes
    /// no `AWSControlTower_*` rules at all.
    #[test]
    fn rules_are_classified_and_stripped_for_display() {
        assert_eq!(
            classify_rule("securityhub-s3-bucket-ssl-requests-only-d49ab884"),
            ("Security Hub", "s3-bucket-ssl-requests-only".to_string()),
        );
        assert_eq!(
            classify_rule("AWSControlTower_AWS-GR_ENCRYPTED_VOLUMES"),
            ("Control Tower", "AWS-GR_ENCRYPTED_VOLUMES".to_string()),
        );
        // Unknown deployer: keep the name exactly as AWS reports it.
        assert_eq!(
            classify_rule("my-team-custom-rule"),
            ("Config", "my-team-custom-rule".to_string()),
        );
        // A trailing segment that merely looks short must survive — only an
        // 8-char hex disambiguator is dropped.
        assert_eq!(
            classify_rule("securityhub-restricted-ssh"),
            ("Security Hub", "restricted-ssh".to_string()),
        );
    }

    /// A genuine fault in the right account must keep its real message rather
    /// than being flattened into the wrong-account hint.
    #[test]
    fn unrelated_errors_keep_their_message() {
        let raw = "ThrottlingException: Rate exceeded";
        assert!(wrong_env_hint(raw).is_none());
        assert!(friendly_error(raw).contains("Rate exceeded"));
    }
}
