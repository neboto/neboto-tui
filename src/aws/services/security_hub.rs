use crate::aws::client::AwsClients;
use crate::aws::pagination::next_page_token;
use crate::aws::resource::{native_state_label, Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::error::Result;
use crate::event::{Event, LoadProgress};
use async_trait::async_trait;
use aws_sdk_securityhub::types::{
    AutomationRulesConfig, AutomationRulesFindingFieldsUpdate, AutomationRulesFindingFilters,
    AwsSecurityFinding, AwsSecurityFindingFilters, AwsSecurityFindingIdentifier, ParameterValue,
    SecurityControl, SecurityControlDefinition, SortCriterion, SortOrder, StringFilter,
    StringFilterComparison,
};
use aws_sdk_securityhub::Client as ShClient;
use std::any::Any;
use std::collections::{HashMap, HashSet};
use tokio::sync::mpsc;

/// Findings fetched per scope are capped so a noisy account doesn't pull tens of
/// thousands of ASFF findings into memory. Suppressed and resolved findings are
/// deliberately loaded (see `is_noise`), which makes this cap load-bearing.
const MAX_FINDINGS: usize = 1000;

/// Pages of ACTIVE+FAILED control findings scanned to derive per-control and
/// per-standard pass/fail. 100 findings per page.
const MAX_CONTROL_SCAN_PAGES: usize = 40;

/// Security control definitions pulled per region (AWS ships ~500 today).
const MAX_CONTROL_DEFS: usize = 2000;

/// `BatchGetSecurityControls` takes at most 100 ids per call.
const CONTROL_BATCH: usize = 100;

/// `BatchGetAutomationRules` takes at most 100 ARNs per call.
const AUTOMATION_BATCH: usize = 100;

/// Finding-history records kept (newest first).
const MAX_HISTORY_RECORDS: usize = 100;

/// Member accounts listed for a Security Hub administrator.
const MAX_MEMBERS: usize = 1000;

/// Grouped rows kept from an insight's results (largest groups first).
const MAX_INSIGHT_RESULTS: usize = 100;

/// Highest AWS-managed insight number probed. AWS ships ~40 today; the range
/// is deliberately generous because a miss costs one cheap call and a number
/// we don't probe is an insight the console shows and we don't.
const MAX_MANAGED_INSIGHTS: u32 = 50;

/// Severity scope — a variant-cached server-side filter (like GuardDuty's).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShSeverityScope {
    Critical,
    High,
    Medium,
    All,
}

impl ShSeverityScope {
    /// Severity labels to filter on (None = no severity filter, i.e. all).
    fn labels(&self) -> Option<&'static [&'static str]> {
        match self {
            ShSeverityScope::Critical => Some(&["CRITICAL"]),
            ShSeverityScope::High => Some(&["CRITICAL", "HIGH"]),
            ShSeverityScope::Medium => Some(&["CRITICAL", "HIGH", "MEDIUM"]),
            ShSeverityScope::All => None,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            ShSeverityScope::Critical => "Critical",
            ShSeverityScope::High => "High+",
            ShSeverityScope::Medium => "Medium+",
            ShSeverityScope::All => "All",
        }
    }
}

/// Security Hub CSPM — the aggregator that normalizes GuardDuty / Inspector /
/// Config / etc. into ASFF findings. Sub-tabs Overview / Findings / Controls /
/// Standards / Insights. Findings carry full detail (no per-finding Get),
/// filtered server-side to ACTIVE, severity ≥ scope, and severity-ranked.
pub struct SecurityHubService {
    client: ShClient,
    scope: ShSeverityScope,
    /// Only used to build managed-insight ARNs, which are partition-scoped.
    partition: &'static str,
}

impl SecurityHubService {
    pub fn new(aws_clients: &AwsClients, scope: ShSeverityScope) -> Self {
        Self {
            client: aws_clients.securityhub_client(),
            scope,
            partition: partition_for(aws_clients.current_region().as_str()),
        }
    }
}

/// ARN partition for a region. There's no partition accessor on the SDK
/// config, and a managed-insight ARN needs one.
fn partition_for(region: &str) -> &'static str {
    if region.starts_with("us-gov-") {
        "aws-us-gov"
    } else if region.starts_with("cn-") {
        "aws-cn"
    } else {
        "aws"
    }
}

#[async_trait]
impl AwsService for SecurityHubService {
    fn service_type(&self) -> ServiceType {
        ServiceType::SecurityHub
    }

    fn name(&self) -> &str {
        "Security Hub"
    }

    async fn list_resources(&self) -> Result<Vec<Box<dyn Resource>>> {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        self.list_resources_streaming(tx, ServiceType::SecurityHub)
            .await?;
        Ok(vec![])
    }

    async fn list_resources_streaming(
        &self,
        event_tx: mpsc::UnboundedSender<Event>,
        service_type: ServiceType,
    ) -> Result<()> {
        let mut total = 0usize;
        // FMS-shaped error tolerance: every phase warns and keeps going; only a
        // load where *nothing* streamed is fatal, and then we surface the first
        // real error rather than a generic "empty".
        let mut first_error: Option<String> = None;
        let warn = |msg: String| {
            let _ = event_tx.send(Event::ResourceLoadWarning {
                service: service_type,
                warning: msg,
            });
        };

        // ── Phase 1: findings (one server-filtered, severity-ranked query) ───
        let filters = build_filters(self.scope);
        let sort = SortCriterion::builder()
            .field("SeverityNormalized")
            .sort_order(SortOrder::Descending)
            .build();

        let mut all_findings: Vec<ShFinding> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut req = self
                .client
                .get_findings()
                .filters(filters.clone())
                .sort_criteria(sort.clone())
                .max_results(100);
            if let Some(t) = &token {
                req = req.next_token(t);
            }
            let page = match req.send().await {
                Ok(p) => p,
                Err(e) => {
                    // A page failure mid-stream must NOT be fatal: a
                    // `ResourceLoadError` clears `loading`, and every batch
                    // already queued behind it is then dropped.
                    let msg = friendly_error(&crate::error::sdk_error_message(&e));
                    first_error.get_or_insert(msg.clone());
                    warn(format!("findings: {}", msg));
                    break;
                }
            };

            let findings: Vec<ShFinding> =
                page.findings().iter().map(ShFinding::from_asff).collect();
            if !findings.is_empty() {
                let batch: Vec<Box<dyn Resource>> = findings
                    .iter()
                    .map(|f| Box::new(f.clone()) as Box<dyn Resource>)
                    .collect();
                all_findings.extend(findings);
                total += batch.len();
                let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                    service: service_type,
                    resources: batch,
                    progress: LoadProgress {
                        loaded_count: total,
                        total_count: None,
                        status_message: None,
                    },
                });
            }

            token = next_page_token(page.next_token(), &token);
            if token.is_none() {
                break;
            }
            if all_findings.len() >= MAX_FINDINGS {
                warn(format!(
                    "findings: showing the first {} at this severity scope — narrow with `t` or search",
                    MAX_FINDINGS
                ));
                break;
            }
        }

        // ── Phase 2: enabled standards (subscriptions + control counts) ──────
        let mut standards: Vec<ShStandard> = Vec::new();
        match self.client.get_enabled_standards().send().await {
            Ok(resp) => {
                for sub in resp.standards_subscriptions() {
                    let mut std = ShStandard::from_subscription(sub);
                    let sub_arn = sub.standards_subscription_arn().unwrap_or_default();
                    self.fill_control_counts(sub_arn, &mut std).await;
                    standards.push(std);
                }
            }
            Err(e) => {
                let msg = friendly_error(&crate::error::sdk_error_message(&e));
                first_error.get_or_insert(msg.clone());
                warn(format!("standards: {}", msg));
            }
        }

        // ── Phase 3: controls (the console's headline tab) ───────────────────
        // One scan of ACTIVE+FAILED control findings drives both a control's
        // pass/fail and every standard's score — replacing the old
        // generator-id-substring guess with the control id AWS itself stamps
        // on the finding. The scan and the control catalogue are independent,
        // and each is dozens of sequential round-trips, so they run
        // concurrently on this task (the SSM `futures::join!` pattern — no
        // extra spawns).
        let ((failed_by_control, scan_complete), inputs) = futures::join!(
            self.scan_failed_controls(),
            self.fetch_control_inputs(&standards, &warn),
        );
        let mut controls = build_controls(&inputs, &failed_by_control, scan_complete);

        if !controls.is_empty() {
            // Failures first: the tab's whole purpose is "what is broken".
            controls.sort_by(|a, b| {
                compliance_rank(&a.compliance)
                    .cmp(&compliance_rank(&b.compliance))
                    .then_with(|| severity_rank(&b.severity).cmp(&severity_rank(&a.severity)))
                    .then_with(|| a.id.cmp(&b.id))
            });
            let batch: Vec<Box<dyn Resource>> = controls
                .iter()
                .cloned()
                .map(|c| Box::new(c) as Box<dyn Resource>)
                .collect();
            total += batch.len();
            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                service: service_type,
                resources: batch,
                progress: LoadProgress {
                    loaded_count: total,
                    total_count: None,
                    status_message: None,
                },
            });
        }

        // Standard scores, now derived from control membership rather than a
        // generator-id substring match.
        score_standards(&mut standards, &controls, scan_complete);
        if !standards.is_empty() {
            let batch: Vec<Box<dyn Resource>> = standards
                .iter()
                .cloned()
                .map(|s| Box::new(s) as Box<dyn Resource>)
                .collect();
            total += batch.len();
            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                service: service_type,
                resources: batch,
                progress: LoadProgress {
                    loaded_count: total,
                    total_count: None,
                    status_message: Some("Loading insights…".to_string()),
                },
            });
        }

        // ── Phase 4: overview (dashboard summary) ────────────────────────────
        {
            let mut critical = 0usize;
            let mut high = 0usize;
            let mut medium = 0usize;
            let mut low = 0usize;
            let mut informational = 0usize;
            for f in &all_findings {
                if f.is_noise() {
                    continue;
                }
                match f.severity_label.as_str() {
                    "CRITICAL" => critical += 1,
                    "HIGH" => high += 1,
                    "MEDIUM" => medium += 1,
                    "LOW" => low += 1,
                    "INFORMATIONAL" => informational += 1,
                    _ => {}
                }
            }
            let overview = ShOverview {
                standards: standards
                    .iter()
                    .map(|s| ShStandardScore {
                        name: s.name.clone(),
                        controls_passed: s.controls_passed,
                        controls_enabled: s.controls_enabled,
                    })
                    .collect(),
                critical_count: critical,
                high_count: high,
                medium_count: medium,
                low_count: low,
                informational_count: informational,
                total_findings: all_findings.len(),
                controls_failed: controls.iter().filter(|c| c.compliance == FAILED).count(),
                controls_total: controls.len(),
                suppressed_count: all_findings
                    .iter()
                    .filter(|f| f.workflow_status == "SUPPRESSED")
                    .count(),
                severity_scope: self.scope.label().to_string(),
                findings_capped: all_findings.len() >= MAX_FINDINGS,
                scan_complete,
            };
            total += 1;
            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                service: service_type,
                resources: vec![Box::new(overview) as Box<dyn Resource>],
                progress: LoadProgress {
                    loaded_count: total,
                    total_count: None,
                    status_message: None,
                },
            });
        }

        // ── Phase 5: insights (saved groupings) ──────────────────────────────
        // `GetInsights` with no ARNs returns **custom insights only** — it
        // does not return the ~40 AWS-managed ones the console leads with.
        // Most accounts have zero custom insights, which is why this tab read
        // as empty. There is no "list managed insights" API, so they have to
        // be named: their ARNs are the stable, contiguously-numbered
        // `…:::insight/securityhub/default/N`.
        let mut ins_token: Option<String> = None;
        let mut insights: Vec<ShInsight> = Vec::new();
        loop {
            let mut req = self.client.get_insights().max_results(100);
            if let Some(t) = &ins_token {
                req = req.next_token(t);
            }
            match req.send().await {
                Ok(page) => {
                    insights.extend(page.insights().iter().map(ShInsight::from_sdk));
                    ins_token = next_page_token(page.next_token(), &ins_token);
                    if ins_token.is_none() {
                        break;
                    }
                }
                Err(e) => {
                    warn(format!(
                        "insights: {}",
                        crate::error::sdk_error_message(&e)
                    ));
                    break;
                }
            }
        }
        insights.extend(self.fetch_managed_insights().await);
        // Managed first (numbered, and what the console shows), custom after.
        insights.sort_by(|a, b| {
            b.managed
                .cmp(&a.managed)
                .then_with(|| managed_index(&a.arn).cmp(&managed_index(&b.arn)))
                .then_with(|| a.name.cmp(&b.name))
        });
        let insights: Vec<Box<dyn Resource>> = insights
            .into_iter()
            .map(|i| Box::new(i) as Box<dyn Resource>)
            .collect();
        if !insights.is_empty() {
            total += insights.len();
            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                service: service_type,
                resources: insights,
                progress: LoadProgress {
                    loaded_count: total,
                    total_count: None,
                    status_message: None,
                },
            });
        }

        // ── Phase 6: automations (rules + custom actions) ────────────────────
        // Grouped tab: an automation rule and a custom action are the two ways
        // something other than a human acts on a finding, and neither fills a
        // tab alone.
        let mut automations: Vec<Box<dyn Resource>> = Vec::new();
        match self.fetch_automation_rules().await {
            Ok(rules) => automations.extend(
                rules
                    .into_iter()
                    .map(|r| Box::new(r) as Box<dyn Resource>),
            ),
            // Only the Security Hub administrator can list automation rules,
            // so a member account always fails here — named, not swallowed,
            // because a plain permission gap looks identical.
            Err(e) => warn(format!("automation rules: {}", e)),
        }
        match self.fetch_action_targets().await {
            Ok(targets) => automations.extend(
                targets
                    .into_iter()
                    .map(|t| Box::new(t) as Box<dyn Resource>),
            ),
            Err(e) => warn(format!("custom actions: {}", e)),
        }
        if !automations.is_empty() {
            total += automations.len();
            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                service: service_type,
                resources: automations,
                progress: LoadProgress {
                    loaded_count: total,
                    total_count: None,
                    status_message: None,
                },
            });
        }

        // ── Phase 7: integrations (which products actually feed findings) ────
        match self.fetch_products().await {
            Ok(products) if !products.is_empty() => {
                let batch: Vec<Box<dyn Resource>> = products
                    .into_iter()
                    .map(|p| Box::new(p) as Box<dyn Resource>)
                    .collect();
                total += batch.len();
                let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                    service: service_type,
                    resources: batch,
                    progress: LoadProgress {
                        loaded_count: total,
                        total_count: None,
                        status_message: None,
                    },
                });
            }
            Ok(_) => {}
            Err(e) => warn(format!("integrations: {}", e)),
        }

        // ── Phase 8: configuration (settings + central config policies) ──────
        // Grouped tab: one synthetic settings row (the GdOverview precedent —
        // it's scalars, not a list) alongside the configuration policies that
        // those settings hand control to.
        let mut config_rows: Vec<Box<dyn Resource>> =
            vec![Box::new(self.fetch_settings().await) as Box<dyn Resource>];
        match self.fetch_config_policies().await {
            Ok(policies) => config_rows.extend(
                policies
                    .into_iter()
                    .map(|p| Box::new(p) as Box<dyn Resource>),
            ),
            Err(e) => warn(format!("configuration policies: {}", e)),
        }
        total += config_rows.len();
        let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
            service: service_type,
            resources: config_rows,
            progress: LoadProgress {
                loaded_count: total,
                total_count: None,
                status_message: None,
            },
        });

        // ── Phase 9: member accounts ─────────────────────────────────────────
        // Silent on failure, unlike the automation-rules phase: a standalone
        // account is *expected* to fail this, so a warning would fire on every
        // load nearly everywhere. The explanation lives in `resource_list.rs`'s
        // per-tab empty state instead (the GuardDuty-Accounts precedent).
        let members = self.fetch_members().await;
        if !members.is_empty() {
            let batch: Vec<Box<dyn Resource>> = members
                .into_iter()
                .map(|m| Box::new(m) as Box<dyn Resource>)
                .collect();
            total += batch.len();
            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                service: service_type,
                resources: batch,
                progress: LoadProgress {
                    loaded_count: total,
                    total_count: None,
                    status_message: None,
                },
            });
        }

        // Nothing at all streamed and something failed → that failure is the
        // load's real result, so report it rather than an empty list.
        if total == 0 {
            if let Some(err) = first_error {
                let _ = event_tx.send(Event::ResourceLoadError {
                    service: service_type,
                    error: err,
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

impl SecurityHubService {
    /// Count ENABLED / DISABLED controls for a standard subscription.
    async fn fill_control_counts(&self, sub_arn: &str, std: &mut ShStandard) {
        if sub_arn.is_empty() {
            return;
        }
        let mut token: Option<String> = None;
        loop {
            let mut req = self
                .client
                .describe_standards_controls()
                .standards_subscription_arn(sub_arn)
                .max_results(100);
            if let Some(t) = &token {
                req = req.next_token(t);
            }
            match req.send().await {
                Ok(page) => {
                    for c in page.controls() {
                        match c.control_status().map(|s| s.as_str()) {
                            Some("ENABLED") => std.controls_enabled += 1,
                            Some("DISABLED") => std.controls_disabled += 1,
                            _ => {}
                        }
                    }
                    token = next_page_token(page.next_token(), &token);
                    if token.is_none() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    }

    /// Scan ACTIVE + FAILED control findings and count failing resources per
    /// security control id. Suppressed findings are excluded — the console
    /// leaves them out of the score too, and the Findings tab is where you go
    /// to see what a suppression is hiding.
    ///
    /// Returns `(counts, complete)`; `complete` is false when the page cap cut
    /// the scan short, which downgrades every no-failure control from PASSED
    /// to NO DATA rather than claiming a pass we didn't verify.
    async fn scan_failed_controls(&self) -> (HashMap<String, usize>, bool) {
        let filters = AwsSecurityFindingFilters::builder()
            .record_state(eq("ACTIVE"))
            .compliance_status(eq("FAILED"))
            .workflow_status(ne("SUPPRESSED"))
            .build();

        let mut counts: HashMap<String, usize> = HashMap::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut req = self
                .client
                .get_findings()
                .filters(filters.clone())
                .max_results(100);
            if let Some(t) = &token {
                req = req.next_token(t);
            }
            match req.send().await {
                Ok(page) => {
                    for f in page.findings() {
                        if let Some(id) = f.compliance().and_then(|c| c.security_control_id()) {
                            if !id.is_empty() {
                                *counts.entry(id.to_string()).or_insert(0) += 1;
                            }
                        }
                    }
                    token = next_page_token(page.next_token(), &token);
                    pages += 1;
                    if token.is_none() {
                        return (counts, true);
                    }
                    if pages >= MAX_CONTROL_SCAN_PAGES {
                        return (counts, false);
                    }
                }
                Err(_) => return (counts, false),
            }
        }
    }

    /// Fetch the control catalogue: every security control available in the
    /// region, which enabled standards it belongs to, and its enablement +
    /// customized parameters.
    ///
    /// Membership comes from `ListSecurityControlDefinitions(standards_arn=…)`
    /// — one paginated call per standard, and it speaks the canonical
    /// `SecurityControlId`. `DescribeStandardsControls` would also list a
    /// standard's controls but keys them by the *standard's* own control id
    /// (`CIS.1.1`), which doesn't join to anything else here.
    async fn fetch_control_inputs(
        &self,
        standards: &[ShStandard],
        warn: &impl Fn(String),
    ) -> ControlInputs {
        let defs = match self.list_control_definitions(None).await {
            Ok(d) => d,
            Err(e) => {
                warn(format!("controls: {}", e));
                return ControlInputs::default();
            }
        };
        if defs.is_empty() {
            return ControlInputs::default();
        }

        // Standard membership, one paginated list per enabled standard.
        let mut membership: HashMap<String, Vec<String>> = HashMap::new();
        for std in standards {
            match self.list_control_definitions(Some(&std.arn)).await {
                Ok(in_std) => {
                    for d in &in_std {
                        let id = d.security_control_id().unwrap_or_default();
                        if !id.is_empty() {
                            membership
                                .entry(id.to_string())
                                .or_default()
                                .push(std.name.clone());
                        }
                    }
                }
                Err(e) => warn(format!("controls ({}): {}", std.name, e)),
            }
        }

        // Enablement + customized parameters, 100 control ids per call.
        let ids: Vec<String> = defs
            .iter()
            .filter_map(|d| d.security_control_id().map(|s| s.to_string()))
            .collect();
        let mut enriched: HashMap<String, SecurityControl> = HashMap::new();
        for chunk in ids.chunks(CONTROL_BATCH) {
            match self
                .client
                .batch_get_security_controls()
                .set_security_control_ids(Some(chunk.to_vec()))
                .send()
                .await
            {
                Ok(resp) => {
                    for c in resp.security_controls() {
                        if let Some(id) = c.security_control_id() {
                            enriched.insert(id.to_string(), c.clone());
                        }
                    }
                }
                Err(e) => {
                    warn(format!(
                        "control status: {}",
                        crate::error::sdk_error_message(&e)
                    ));
                    break;
                }
            }
        }

        ControlInputs {
            defs,
            membership,
            enriched,
        }
    }

    /// `ListAutomationRules` returns metadata only; the criteria and actions —
    /// the entire point of the tab — need `BatchGetAutomationRules`, which
    /// takes 100 ARNs per call. AWS's per-account quota is 100 rules, so this
    /// is one extra call in practice.
    async fn fetch_automation_rules(&self) -> std::result::Result<Vec<ShAutomationRule>, String> {
        let mut arns: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut req = self.client.list_automation_rules().max_results(100);
            if let Some(t) = &token {
                req = req.next_token(t);
            }
            let page = req
                .send()
                .await
                .map_err(|e| crate::error::sdk_error_message(&e))?;
            arns.extend(
                page.automation_rules_metadata()
                    .iter()
                    .filter_map(|m| m.rule_arn().map(|a| a.to_string())),
            );
            token = next_page_token(page.next_token(), &token);
            if token.is_none() {
                break;
            }
        }
        if arns.is_empty() {
            return Ok(Vec::new());
        }

        let mut rules: Vec<ShAutomationRule> = Vec::new();
        for chunk in arns.chunks(AUTOMATION_BATCH) {
            let resp = self
                .client
                .batch_get_automation_rules()
                .set_automation_rules_arns(Some(chunk.to_vec()))
                .send()
                .await
                .map_err(|e| crate::error::sdk_error_message(&e))?;
            rules.extend(resp.rules().iter().map(ShAutomationRule::from_sdk));
        }
        // Rule order is the order AWS applies them in, so it's the order that
        // explains an outcome.
        rules.sort_by_key(|r| (r.order, r.name.clone()));
        Ok(rules)
    }

    /// Fetch the AWS-managed insights by ARN, since `GetInsights` won't list
    /// them. They're numbered from 1 with no gaps, but AWS has retired some
    /// over time and adds more, so probe a fixed range concurrently and keep
    /// whatever answers — a miss is a normal outcome, not an error. One ARN
    /// per call: `GetInsights` rejects the **whole** request if any ARN in it
    /// is unknown, so batching would let one retired number blank the rest.
    async fn fetch_managed_insights(&self) -> Vec<ShInsight> {
        use futures::stream::StreamExt;
        let client = &self.client;
        let partition = self.partition;
        futures::stream::iter(1..=MAX_MANAGED_INSIGHTS)
            .map(|n| async move {
                let arn = format!("arn:{}:securityhub:::insight/securityhub/default/{}", partition, n);
                client
                    .get_insights()
                    .insight_arns(arn)
                    .send()
                    .await
                    .ok()
                    .map(|r| {
                        r.insights()
                            .iter()
                            .map(ShInsight::from_sdk)
                            .collect::<Vec<_>>()
                    })
            })
            .buffer_unordered(8)
            .filter_map(|r| async move { r })
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .flatten()
            .collect()
    }

    /// Custom actions — the EventBridge hooks a human can fire from a finding.
    async fn fetch_action_targets(&self) -> std::result::Result<Vec<ShActionTarget>, String> {
        let mut out: Vec<ShActionTarget> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut req = self.client.describe_action_targets().max_results(100);
            if let Some(t) = &token {
                req = req.next_token(t);
            }
            let page = req
                .send()
                .await
                .map_err(|e| crate::error::sdk_error_message(&e))?;
            out.extend(page.action_targets().iter().map(ShActionTarget::from_sdk));
            token = next_page_token(page.next_token(), &token);
            if token.is_none() {
                break;
            }
        }
        Ok(out)
    }

    /// The integration catalogue, joined with what's actually subscribed.
    /// `ListEnabledProductsForImport` returns *subscription* ARNs
    /// (`…:product-subscription/aws/guardduty`) while `DescribeProducts`
    /// returns *product* ARNs (`…::product/aws/guardduty`), so the join is on
    /// the shared vendor/product tail rather than the ARN itself.
    async fn fetch_products(&self) -> std::result::Result<Vec<ShProduct>, String> {
        let mut enabled: HashSet<String> = HashSet::new();
        let mut token: Option<String> = None;
        loop {
            let mut req = self
                .client
                .list_enabled_products_for_import()
                .max_results(100);
            if let Some(t) = &token {
                req = req.next_token(t);
            }
            match req.send().await {
                Ok(page) => {
                    for arn in page.product_subscriptions() {
                        enabled.insert(product_key(arn));
                    }
                    token = next_page_token(page.next_token(), &token);
                    if token.is_none() {
                        break;
                    }
                }
                // A denial here costs the enabled flag, not the catalogue.
                Err(_) => break,
            }
        }

        let mut out: Vec<ShProduct> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut req = self.client.describe_products().max_results(100);
            if let Some(t) = &token {
                req = req.next_token(t);
            }
            let page = req
                .send()
                .await
                .map_err(|e| crate::error::sdk_error_message(&e))?;
            for p in page.products() {
                let arn = p.product_arn().unwrap_or_default();
                out.push(ShProduct::from_sdk(p, enabled.contains(&product_key(arn))));
            }
            token = next_page_token(page.next_token(), &token);
            if token.is_none() {
                break;
            }
        }
        // Subscribed integrations first — the rest is a catalogue.
        out.sort_by(|a, b| {
            b.enabled
                .cmp(&a.enabled)
                .then_with(|| a.company.cmp(&b.company))
                .then_with(|| a.name.cmp(&b.name))
        });
        Ok(out)
    }

    /// Assemble the account's Security Hub posture from five independent
    /// best-effort calls. Each failure is recorded with a section prefix and
    /// surfaces only in the section that owns it (the `GdOverview` pattern) —
    /// a member account can't call `DescribeOrganizationConfiguration`, and
    /// that shouldn't blank the hub settings it *can* read.
    async fn fetch_settings(&self) -> ShSettings {
        let mut s = ShSettings::default();

        match self.client.describe_hub().send().await {
            Ok(h) => {
                s.hub_arn = h.hub_arn().unwrap_or_default().to_string();
                s.subscribed_at = h.subscribed_at().unwrap_or_default().to_string();
                s.auto_enable_controls = h.auto_enable_controls();
                s.control_finding_generator = h
                    .control_finding_generator()
                    .map(|g| g.as_str().to_string())
                    .unwrap_or_default();
            }
            Err(e) => s.errors.push(format!(
                "hub: {}",
                friendly_error(&crate::error::sdk_error_message(&e))
            )),
        }

        // Cross-region aggregation: which region collects findings, and from
        // where. Explains why findings from another region do (or don't) show.
        match self.client.list_finding_aggregators().send().await {
            Ok(list) => {
                let arn = list
                    .finding_aggregators()
                    .first()
                    .and_then(|a| a.finding_aggregator_arn())
                    .map(|a| a.to_string());
                match arn {
                    Some(arn) => match self
                        .client
                        .get_finding_aggregator()
                        .finding_aggregator_arn(&arn)
                        .send()
                        .await
                    {
                        Ok(agg) => {
                            s.aggregation_region =
                                agg.finding_aggregation_region().unwrap_or_default().to_string();
                            s.region_linking_mode =
                                agg.region_linking_mode().unwrap_or_default().to_string();
                            s.linked_regions = agg.regions().to_vec();
                        }
                        Err(e) => s.errors.push(format!(
                            "regions: {}",
                            crate::error::sdk_error_message(&e)
                        )),
                    },
                    // No aggregator is a normal state, not an error.
                    None => s.aggregation_configured = false,
                }
                s.aggregation_configured = !s.aggregation_region.is_empty();
            }
            Err(e) => s.errors.push(format!(
                "regions: {}",
                crate::error::sdk_error_message(&e)
            )),
        }

        match self
            .client
            .describe_organization_configuration()
            .send()
            .await
        {
            Ok(o) => {
                s.org_readable = true;
                s.auto_enable_members = o.auto_enable();
                s.member_limit_reached = o.member_account_limit_reached();
                s.auto_enable_standards = o
                    .auto_enable_standards()
                    .map(|a| a.as_str().to_string())
                    .unwrap_or_default();
                if let Some(oc) = o.organization_configuration() {
                    s.org_config_type = oc
                        .configuration_type()
                        .map(|t| t.as_str().to_string())
                        .unwrap_or_default();
                    s.org_config_status = oc
                        .status()
                        .map(|t| t.as_str().to_string())
                        .unwrap_or_default();
                    s.org_config_status_message =
                        oc.status_message().unwrap_or_default().to_string();
                }
            }
            Err(e) => s.errors.push(format!(
                "organization: {}",
                crate::error::sdk_error_message(&e)
            )),
        }

        if let Ok(list) = self.client.list_organization_admin_accounts().send().await {
            s.delegated_admins = list
                .admin_accounts()
                .iter()
                .map(|a| {
                    format!(
                        "{} ({})",
                        a.account_id().unwrap_or_default(),
                        a.status().map(|st| st.as_str()).unwrap_or("")
                    )
                    .trim()
                    .to_string()
                })
                .collect();
        }

        // Only populated when *this* account is a member of another's hub.
        if let Ok(admin) = self.client.get_administrator_account().send().await {
            if let Some(inv) = admin.administrator() {
                s.administrator_account = inv.account_id().unwrap_or_default().to_string();
                s.administrator_status = inv.member_status().unwrap_or_default().to_string();
            }
        }

        s
    }

    /// Central configuration policies + their targets. Associations are
    /// listed **once** org-wide and grouped by policy id rather than queried
    /// per policy — same data, one call instead of N.
    async fn fetch_config_policies(&self) -> std::result::Result<Vec<ShConfigPolicy>, String> {
        let mut summaries: Vec<(String, String)> = Vec::new(); // (id, name)
        let mut token: Option<String> = None;
        loop {
            let mut req = self.client.list_configuration_policies().max_results(100);
            if let Some(t) = &token {
                req = req.next_token(t);
            }
            let page = req
                .send()
                .await
                .map_err(|e| crate::error::sdk_error_message(&e))?;
            for p in page.configuration_policy_summaries() {
                summaries.push((
                    p.id().unwrap_or_default().to_string(),
                    p.name().unwrap_or_default().to_string(),
                ));
            }
            token = next_page_token(page.next_token(), &token);
            if token.is_none() {
                break;
            }
        }
        if summaries.is_empty() {
            return Ok(Vec::new());
        }

        let mut targets_by_policy: HashMap<String, Vec<ShPolicyTarget>> = HashMap::new();
        let mut token: Option<String> = None;
        loop {
            let mut req = self
                .client
                .list_configuration_policy_associations()
                .max_results(100);
            if let Some(t) = &token {
                req = req.next_token(t);
            }
            match req.send().await {
                Ok(page) => {
                    for a in page.configuration_policy_association_summaries() {
                        let policy_id = a.configuration_policy_id().unwrap_or_default().to_string();
                        targets_by_policy
                            .entry(policy_id)
                            .or_default()
                            .push(ShPolicyTarget::from_sdk(a));
                    }
                    token = next_page_token(page.next_token(), &token);
                    if token.is_none() {
                        break;
                    }
                }
                // Targets are the nice-to-have; the policies themselves are not.
                Err(_) => break,
            }
        }

        let mut out: Vec<ShConfigPolicy> = Vec::new();
        for (id, name) in summaries {
            match self
                .client
                .get_configuration_policy()
                .identifier(&id)
                .send()
                .await
            {
                Ok(p) => {
                    let targets = targets_by_policy.remove(&id).unwrap_or_default();
                    out.push(ShConfigPolicy::from_sdk(&p, targets));
                }
                Err(e) => {
                    // Keep the row: knowing the policy exists beats dropping it.
                    let mut stub = ShConfigPolicy::stub(&id, &name);
                    stub.detail_error = crate::error::sdk_error_message(&e);
                    stub.targets = targets_by_policy.remove(&id).unwrap_or_default();
                    out.push(stub);
                }
            }
        }
        Ok(out)
    }

    /// Member accounts. `only_associated(false)` so disabled and removed
    /// members still appear — a member that stopped reporting is exactly what
    /// this tab is for (the GuardDuty-Accounts precedent).
    async fn fetch_members(&self) -> Vec<ShMember> {
        let mut out: Vec<ShMember> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut req = self
                .client
                .list_members()
                .only_associated(false)
                .max_results(50);
            if let Some(t) = &token {
                req = req.next_token(t);
            }
            match req.send().await {
                Ok(page) => {
                    out.extend(page.members().iter().map(ShMember::from_sdk));
                    token = next_page_token(page.next_token(), &token);
                    if token.is_none() || out.len() >= MAX_MEMBERS {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        out.truncate(MAX_MEMBERS);
        // Worst first: an account that isn't reporting is the whole point.
        out.sort_by(|a, b| {
            member_rank(&a.status)
                .cmp(&member_rank(&b.status))
                .then_with(|| a.account_id.cmp(&b.account_id))
        });
        out
    }

    /// Paginate `ListSecurityControlDefinitions`, optionally scoped to one
    /// standard (which is how control→standard membership is resolved).
    async fn list_control_definitions(
        &self,
        standards_arn: Option<&str>,
    ) -> std::result::Result<Vec<SecurityControlDefinition>, String> {
        let mut out: Vec<SecurityControlDefinition> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut req = self
                .client
                .list_security_control_definitions()
                .max_results(100);
            if let Some(arn) = standards_arn {
                req = req.standards_arn(arn);
            }
            if let Some(t) = &token {
                req = req.next_token(t);
            }
            let page = req
                .send()
                .await
                .map_err(|e| crate::error::sdk_error_message(&e))?;
            out.extend(page.security_control_definitions().iter().cloned());
            token = next_page_token(page.next_token(), &token);
            if token.is_none() || out.len() >= MAX_CONTROL_DEFS {
                break;
            }
        }
        Ok(out)
    }
}

/// The control catalogue, fetched independently of the failed-control scan so
/// the two can run concurrently.
#[derive(Default)]
struct ControlInputs {
    defs: Vec<SecurityControlDefinition>,
    /// control id → enabled standards containing it
    membership: HashMap<String, Vec<String>>,
    /// control id → enablement + customized parameters
    enriched: HashMap<String, SecurityControl>,
}

/// Join the catalogue with the failed-control scan into displayable rows.
fn build_controls(
    inputs: &ControlInputs,
    failed_by_control: &HashMap<String, usize>,
    scan_complete: bool,
) -> Vec<ShControl> {
    inputs
        .defs
        .iter()
        .map(|d| {
            let id = d.security_control_id().unwrap_or_default().to_string();
            let failed = failed_by_control.get(&id).copied().unwrap_or(0);
            let in_standards = inputs.membership.get(&id).cloned().unwrap_or_default();
            ShControl::build(
                d,
                inputs.enriched.get(&id),
                in_standards,
                failed,
                scan_complete,
            )
        })
        .collect()
}

/// Distribute control outcomes over the standards that contain them, which is
/// what the console's per-standard security score actually measures.
fn score_standards(standards: &mut [ShStandard], controls: &[ShControl], scan_complete: bool) {
    for std in standards.iter_mut() {
        let mut enabled = 0i32;
        let mut failed = 0i32;
        for c in controls {
            if !c.standards.iter().any(|s| s == &std.name) {
                continue;
            }
            if c.compliance == DISABLED {
                continue;
            }
            enabled += 1;
            if c.compliance == FAILED {
                failed += 1;
            }
        }
        // Only overwrite the DescribeStandardsControls counters when the
        // control phase actually produced membership for this standard.
        if enabled > 0 {
            std.controls_enabled = enabled;
            std.controls_failed = failed;
            std.controls_passed = (enabled - failed).max(0);
        }
        std.score_estimated = !scan_complete;
    }
}

// ── shared vocabulary ───────────────────────────────────────────────────────

pub const FAILED: &str = "FAILED";
pub const PASSED: &str = "PASSED";
pub const DISABLED: &str = "DISABLED";
pub const NO_DATA: &str = "NO DATA";

fn compliance_rank(c: &str) -> u8 {
    match c {
        FAILED => 0,
        NO_DATA => 1,
        PASSED => 2,
        _ => 3,
    }
}

fn severity_rank(s: &str) -> u8 {
    match s {
        "CRITICAL" => 4,
        "HIGH" => 3,
        "MEDIUM" => 2,
        "LOW" => 1,
        _ => 0,
    }
}

fn eq(value: &str) -> StringFilter {
    StringFilter::builder()
        .value(value)
        .comparison(StringFilterComparison::Equals)
        .build()
}

fn ne(value: &str) -> StringFilter {
    StringFilter::builder()
        .value(value)
        .comparison(StringFilterComparison::NotEquals)
        .build()
}

/// ACTIVE record state, severity ≥ scope. Deliberately **not** filtered to
/// NEW/NOTIFIED: an automation rule quietly suppressing a real finding is
/// exactly what you need to see, so suppressed and resolved findings load and
/// are `is_noise()` instead (`a` folds them away).
fn build_filters(scope: ShSeverityScope) -> AwsSecurityFindingFilters {
    let mut f = AwsSecurityFindingFilters::builder().record_state(eq("ACTIVE"));
    if let Some(labels) = scope.labels() {
        for l in labels {
            f = f.severity_label(eq(l));
        }
    }
    f.build()
}

/// A not-enabled / not-subscribed error → a friendly hint.
fn friendly_error(raw: &str) -> String {
    let low = raw.to_lowercase();
    if low.contains("invalidaccess") || low.contains("not subscribed") {
        "Security Hub isn't enabled in this region.".to_string()
    } else if low.contains("accessdenied") || low.contains("not authorized") {
        "Access denied. Security Hub read permissions are required (securityhub:GetFindings)."
            .to_string()
    } else {
        format!("Failed to load Security Hub: {}", raw)
    }
}

/// Wrap prose to a column so a long description reads as content lines.
fn wrap_words(text: &str, width: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > width {
            out.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        out.push(line);
    }
    out
}

/// Group header row (non-empty key, empty value, no leading space).
fn group(rows: &mut Vec<(String, String)>, label: &str) {
    if !rows.is_empty() {
        rows.push((String::new(), String::new()));
    }
    rows.push((label.to_string(), String::new()));
}

/// Decompose an ASFF finding type into its taxonomy parts. The format is
/// `Namespace/Category/Classifier/Type`, only the namespace being mandatory —
/// it's a structured identifier, not a sentence.
pub fn decompose_finding_type(t: &str) -> Vec<(String, String)> {
    const LABELS: [&str; 4] = ["Namespace", "Category", "Classifier", "Type"];
    t.split('/')
        .filter(|p| !p.is_empty())
        .zip(LABELS.iter())
        .map(|(part, label)| (format!("  {}", label), part.to_string()))
        .collect()
}

// ── ShFinding ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ShFinding {
    pub id: String,
    pub product: String,
    pub product_arn: String,
    pub company: String,
    pub generator_id: String,
    pub title: String,
    pub description: String,
    pub severity_label: String,
    pub severity_normalized: Option<i32>,
    pub severity_original: String,
    pub criticality: Option<i32>,
    pub confidence: Option<i32>,
    pub workflow_status: String,
    pub record_state: String,
    pub compliance_status: String,
    pub security_control_id: String,
    pub related_requirements: Vec<String>,
    /// `(reason code, description)` — *why* the control failed. The single
    /// most useful field the old flat view discarded.
    pub status_reasons: Vec<(String, String)>,
    /// Standards this control finding rolls up into (jump anchors).
    pub associated_standards: Vec<String>,
    /// Customized control parameters this evaluation ran with.
    pub control_parameters: Vec<(String, String)>,
    pub types: Vec<String>,
    pub account_id: String,
    pub account_name: String,
    pub region: String,
    pub sample: bool,
    pub source_url: String,
    /// (resource type, resource id/ARN) — ARN rows are cross-service jump anchors.
    pub resources: Vec<(String, String)>,
    /// Pre-flattened per-resource rows (region/partition/role/tags/`Other`).
    pub resource_rows: Vec<(String, String)>,
    /// Pre-flattened CVE rows (Inspector findings arrive here).
    pub vuln_rows: Vec<(String, String)>,
    /// Pre-flattened network / process / threat / action / patch rows.
    pub context_rows: Vec<(String, String)>,
    pub note_text: String,
    pub note_updated_by: String,
    pub note_updated_at: String,
    pub product_fields: Vec<(String, String)>,
    pub user_defined_fields: Vec<(String, String)>,
    pub related_findings: Vec<String>,
    pub remediation_text: String,
    pub remediation_url: String,
    pub first_observed: Option<String>,
    pub last_observed: Option<String>,
    pub created: Option<String>,
    pub updated: Option<String>,
    pub processed_at: Option<String>,
    /// Every searchable identifier, precomputed — `search_text` runs per
    /// resource per keystroke, so it must not re-walk the flattened rows.
    pub search_blob: String,
    pub raw_json: String,
}

impl ShFinding {
    fn from_asff(f: &AwsSecurityFinding) -> Self {
        let severity = f.severity();
        let severity_label = severity
            .and_then(|s| s.label())
            .map(|l| l.as_str().to_string())
            .unwrap_or_default();
        let workflow_status = f
            .workflow()
            .and_then(|w| w.status())
            .map(|s| s.as_str().to_string())
            .unwrap_or_default();
        let record_state = f
            .record_state()
            .map(|s| s.as_str().to_string())
            .unwrap_or_default();

        let (
            compliance_status,
            security_control_id,
            related_requirements,
            status_reasons,
            associated_standards,
            control_parameters,
        ) = match f.compliance() {
            Some(c) => (
                c.status()
                    .map(|s| s.as_str().to_string())
                    .unwrap_or_default(),
                c.security_control_id().unwrap_or_default().to_string(),
                c.related_requirements().to_vec(),
                c.status_reasons()
                    .iter()
                    .map(|r| {
                        (
                            r.reason_code().unwrap_or_default().to_string(),
                            r.description().unwrap_or_default().to_string(),
                        )
                    })
                    .collect(),
                c.associated_standards()
                    .iter()
                    .filter_map(|s| s.standards_id().map(|v| v.to_string()))
                    .collect(),
                c.security_control_parameters()
                    .iter()
                    .map(|p| {
                        (
                            p.name().unwrap_or_default().to_string(),
                            p.value().join(", "),
                        )
                    })
                    .collect(),
            ),
            None => (
                String::new(),
                String::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
            ),
        };

        let resources: Vec<(String, String)> = f
            .resources()
            .iter()
            .map(|r| {
                (
                    r.r#type().unwrap_or_default().to_string(),
                    r.id().unwrap_or_default().to_string(),
                )
            })
            .collect();
        let resource_rows = extract_resources(f);
        let vuln_rows = extract_vulnerabilities(f);
        let context_rows = extract_context(f);

        let (remediation_text, remediation_url) =
            match f.remediation().and_then(|r| r.recommendation()) {
                Some(rec) => (
                    rec.text().unwrap_or_default().to_string(),
                    rec.url().unwrap_or_default().to_string(),
                ),
                None => (String::new(), String::new()),
            };

        let (note_text, note_updated_by, note_updated_at) = match f.note() {
            Some(n) => (
                n.text().unwrap_or_default().to_string(),
                n.updated_by().unwrap_or_default().to_string(),
                n.updated_at().unwrap_or_default().to_string(),
            ),
            None => (String::new(), String::new(), String::new()),
        };

        let mut product_fields: Vec<(String, String)> = f
            .product_fields()
            .map(|m| {
                m.iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        product_fields.sort();
        let mut user_defined_fields: Vec<(String, String)> = f
            .user_defined_fields()
            .map(|m| {
                m.iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        user_defined_fields.sort();

        let mut out = Self {
            id: f.id().unwrap_or_default().to_string(),
            product: f.product_name().unwrap_or_default().to_string(),
            product_arn: f.product_arn().unwrap_or_default().to_string(),
            company: f.company_name().unwrap_or_default().to_string(),
            generator_id: f.generator_id().unwrap_or_default().to_string(),
            title: f.title().unwrap_or_default().to_string(),
            description: f.description().unwrap_or_default().to_string(),
            severity_label,
            severity_normalized: severity.and_then(|s| s.normalized()),
            severity_original: severity
                .and_then(|s| s.original())
                .unwrap_or_default()
                .to_string(),
            criticality: f.criticality(),
            confidence: f.confidence(),
            workflow_status,
            record_state,
            compliance_status,
            security_control_id,
            related_requirements,
            status_reasons,
            associated_standards,
            control_parameters,
            types: f.types().to_vec(),
            account_id: f.aws_account_id().unwrap_or_default().to_string(),
            account_name: f.aws_account_name().unwrap_or_default().to_string(),
            region: f.region().unwrap_or_default().to_string(),
            sample: f.sample().unwrap_or(false),
            source_url: f.source_url().unwrap_or_default().to_string(),
            resources,
            resource_rows,
            vuln_rows,
            context_rows,
            note_text,
            note_updated_by,
            note_updated_at,
            product_fields,
            user_defined_fields,
            related_findings: f
                .related_findings()
                .iter()
                .filter_map(|r| r.id().map(|v| v.to_string()))
                .collect(),
            remediation_text,
            remediation_url,
            first_observed: f.first_observed_at().map(|s| s.to_string()),
            last_observed: f.last_observed_at().map(|s| s.to_string()),
            created: f.created_at().map(|s| s.to_string()),
            updated: f.updated_at().map(|s| s.to_string()),
            processed_at: f.processed_at().map(|s| s.to_string()),
            search_blob: String::new(),
            raw_json: String::new(),
        };
        out.search_blob = build_search_blob(&out);
        out.raw_json = build_raw_json(f, &out);
        out
    }

    /// The identifier pair `GetFindingHistory` needs.
    pub fn history_key(&self) -> (String, String) {
        (self.id.clone(), self.product_arn.clone())
    }
}

/// Flatten every resource the finding names: the generic ASFF envelope plus
/// the `Details.Other` bag. The 74 *typed* `ResourceDetails` members (one per
/// AWS resource shape) are deliberately not flattened — that's a per-type
/// renderer each, and `e` opens the finding JSON when you need the raw block.
fn extract_resources(f: &AwsSecurityFinding) -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = Vec::new();
    for r in f.resources() {
        let rtype = r.r#type().unwrap_or_default();
        group(&mut rows, if rtype.is_empty() { "Resource" } else { rtype });
        // Key-value so the generic classifier makes ARNs/ids Enter-jumpable.
        rows.push(("  ARN/Id".to_string(), r.id().unwrap_or_default().to_string()));
        if let Some(region) = r.region() {
            rows.push(("  Region".to_string(), region.to_string()));
        }
        if let Some(p) = r.partition() {
            rows.push(("  Partition".to_string(), p.as_str().to_string()));
        }
        if let Some(role) = r.resource_role() {
            rows.push(("  Role".to_string(), role.to_string()));
        }
        if let Some(app) = r.application_name() {
            rows.push(("  Application".to_string(), app.to_string()));
        }
        if let Some(arn) = r.application_arn() {
            rows.push(("  Application ARN".to_string(), arn.to_string()));
        }
        if let Some(tags) = r.tags() {
            if !tags.is_empty() {
                let mut kv: Vec<(&String, &String)> = tags.iter().collect();
                kv.sort();
                rows.push(("  Tags".to_string(), String::new()));
                for (k, v) in kv {
                    rows.push((format!("    {}", k), v.clone()));
                }
            }
        }
        if let Some(other) = r.details().and_then(|d| d.other()) {
            if !other.is_empty() {
                let mut kv: Vec<(&String, &String)> = other.iter().collect();
                kv.sort();
                rows.push(("  Details".to_string(), String::new()));
                for (k, v) in kv {
                    rows.push((format!("    {}", k), v.clone()));
                }
            }
        }
    }
    rows
}

/// CVE rows — how Inspector vulnerabilities arrive in Security Hub.
fn extract_vulnerabilities(f: &AwsSecurityFinding) -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = Vec::new();
    for v in f.vulnerabilities() {
        group(&mut rows, v.id().unwrap_or("Vulnerability"));
        if let Some(score) = v.epss_score() {
            rows.push(("  EPSS Score".to_string(), format!("{:.4}", score)));
        }
        if let Some(fix) = v.fix_available() {
            rows.push(("  Fix Available".to_string(), fix.as_str().to_string()));
        }
        if let Some(x) = v.exploit_available() {
            rows.push(("  Exploit Available".to_string(), x.as_str().to_string()));
        }
        if let Some(t) = v.last_known_exploit_at() {
            rows.push(("  Last Known Exploit".to_string(), t.to_string()));
        }
        for c in v.cvss() {
            let version = c.version().unwrap_or("CVSS");
            let score = c
                .base_score()
                .map(|s| format!("{:.1}", s))
                .unwrap_or_default();
            let vector = c.base_vector().unwrap_or_default();
            rows.push((
                format!("  CVSS {}", version),
                format!("{} {}", score, vector).trim().to_string(),
            ));
        }
        if let Some(vendor) = v.vendor() {
            rows.push((
                "  Vendor".to_string(),
                vendor.name().unwrap_or_default().to_string(),
            ));
            if let Some(sev) = vendor.vendor_severity() {
                rows.push(("  Vendor Severity".to_string(), sev.to_string()));
            }
        }
        if !v.related_vulnerabilities().is_empty() {
            rows.push((
                "  Related".to_string(),
                v.related_vulnerabilities().join(", "),
            ));
        }
        for p in v.vulnerable_packages() {
            let name = p.name().unwrap_or_default();
            let version = p.version().unwrap_or_default();
            let fixed = p.fixed_in_version().unwrap_or_default();
            let label = if fixed.is_empty() {
                format!("{}@{}", name, version)
            } else {
                format!("{}@{} → {}", name, version, fixed)
            };
            rows.push(("  Package".to_string(), label));
            if let Some(path) = p.file_path() {
                rows.push(("    Path".to_string(), path.to_string()));
            }
        }
        for url in v.reference_urls().iter().take(5) {
            rows.push(("  Reference".to_string(), url.to_string()));
        }
    }
    rows
}

/// Network / process / threat / action / patch context — the shapes a
/// GuardDuty- or SSM-sourced finding carries and a control finding does not.
fn extract_context(f: &AwsSecurityFinding) -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = Vec::new();

    if let Some(n) = f.network() {
        group(&mut rows, "Network");
        if let Some(d) = n.direction() {
            rows.push(("  Direction".to_string(), d.as_str().to_string()));
        }
        if let Some(p) = n.protocol() {
            rows.push(("  Protocol".to_string(), p.to_string()));
        }
        if let Some(ip) = n.source_ipv4().or_else(|| n.source_ipv6()) {
            rows.push(("  Source IP".to_string(), ip.to_string()));
        }
        if let Some(port) = n.source_port() {
            rows.push(("  Source Port".to_string(), port.to_string()));
        }
        if let Some(d) = n.source_domain() {
            rows.push(("  Source Domain".to_string(), d.to_string()));
        }
        if let Some(ip) = n.destination_ipv4().or_else(|| n.destination_ipv6()) {
            rows.push(("  Destination IP".to_string(), ip.to_string()));
        }
        if let Some(port) = n.destination_port() {
            rows.push(("  Destination Port".to_string(), port.to_string()));
        }
        if let Some(d) = n.destination_domain() {
            rows.push(("  Destination Domain".to_string(), d.to_string()));
        }
    }

    if let Some(p) = f.process() {
        group(&mut rows, "Process");
        if let Some(n) = p.name() {
            rows.push(("  Name".to_string(), n.to_string()));
        }
        if let Some(path) = p.path() {
            rows.push(("  Path".to_string(), path.to_string()));
        }
        if let Some(pid) = p.pid() {
            rows.push(("  PID".to_string(), pid.to_string()));
        }
        if let Some(ppid) = p.parent_pid() {
            rows.push(("  Parent PID".to_string(), ppid.to_string()));
        }
        if let Some(t) = p.launched_at() {
            rows.push(("  Launched".to_string(), t.to_string()));
        }
    }

    if !f.threats().is_empty() {
        group(&mut rows, "Threats");
        for t in f.threats() {
            rows.push((
                format!("  {}", t.name().unwrap_or("(unnamed)")),
                t.severity().unwrap_or_default().to_string(),
            ));
            if let Some(count) = t.item_count() {
                rows.push(("    Items".to_string(), count.to_string()));
            }
        }
    }

    if !f.threat_intel_indicators().is_empty() {
        group(&mut rows, "Threat Intel");
        for i in f.threat_intel_indicators() {
            let kind = i.r#type().map(|t| t.as_str()).unwrap_or("Indicator");
            rows.push((
                format!("  {}", kind),
                i.value().unwrap_or_default().to_string(),
            ));
            if let Some(source) = i.source() {
                rows.push(("    Source".to_string(), source.to_string()));
            }
        }
    }

    if let Some(a) = f.action() {
        group(&mut rows, "Action");
        if let Some(t) = a.action_type() {
            rows.push(("  Type".to_string(), t.to_string()));
        }
        if let Some(api) = a.aws_api_call_action() {
            if let Some(name) = api.api() {
                rows.push(("  API".to_string(), name.to_string()));
            }
            if let Some(svc) = api.service_name() {
                rows.push(("  Service".to_string(), svc.to_string()));
            }
            if let Some(c) = api.caller_type() {
                rows.push(("  Caller".to_string(), c.to_string()));
            }
        }
        if let Some(dns) = a.dns_request_action() {
            if let Some(d) = dns.domain() {
                rows.push(("  DNS Domain".to_string(), d.to_string()));
            }
        }
        if let Some(port) = a.port_probe_action() {
            if let Some(blocked) = port.blocked() {
                rows.push(("  Port Probe Blocked".to_string(), blocked.to_string()));
            }
        }
    }

    if !f.malware().is_empty() {
        group(&mut rows, "Malware");
        for m in f.malware() {
            rows.push((
                format!("  {}", m.name().unwrap_or("(unnamed)")),
                m.state().map(|s| s.as_str().to_string()).unwrap_or_default(),
            ));
            if let Some(path) = m.path() {
                rows.push(("    Path".to_string(), path.to_string()));
            }
        }
    }

    if let Some(p) = f.patch_summary() {
        group(&mut rows, "Patch Summary");
        if let Some(id) = p.id() {
            rows.push(("  Baseline".to_string(), id.to_string()));
        }
        if let Some(c) = p.installed_count() {
            rows.push(("  Installed".to_string(), c.to_string()));
        }
        if let Some(c) = p.missing_count() {
            rows.push(("  Missing".to_string(), c.to_string()));
        }
        if let Some(c) = p.failed_count() {
            rows.push(("  Failed".to_string(), c.to_string()));
        }
        if let Some(c) = p.installed_pending_reboot() {
            rows.push(("  Pending Reboot".to_string(), c.to_string()));
        }
    }

    rows
}

/// Every id, ARN, CVE and hostname in the finding, so "who touched this
/// resource" is a plain fuzzy search.
fn build_search_blob(f: &ShFinding) -> String {
    let mut parts: Vec<&str> = vec![
        &f.title,
        &f.product,
        &f.company,
        &f.severity_label,
        &f.security_control_id,
        &f.compliance_status,
        &f.workflow_status,
        &f.generator_id,
        &f.account_id,
        &f.account_name,
    ];
    for (_, id) in &f.resources {
        parts.push(id);
    }
    for t in &f.types {
        parts.push(t);
    }
    for s in &f.associated_standards {
        parts.push(s);
    }
    for (_, v) in f
        .resource_rows
        .iter()
        .chain(f.vuln_rows.iter())
        .chain(f.context_rows.iter())
    {
        if !v.is_empty() {
            parts.push(v);
        }
    }
    parts.retain(|p| !p.is_empty());
    parts.join(" ")
}

crate::sections! {
    pub enum ShFindingDetailSection,
    pub static SH_FINDING_SECTIONS = [
        Details "Details",
        Resources "Resources",
        Compliance "Compliance",
        Vulnerabilities "Vulns",
        Context "Context",
        Remediation "Remediation",
        History "History" => crate::app::App::trigger_sh_finding_history_load,
    ]
}

impl Resource for ShFinding {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&SH_FINDING_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        if self.title.is_empty() {
            &self.generator_id
        } else {
            &self.title
        }
    }

    fn resource_type(&self) -> &str {
        "Security Hub Finding"
    }

    fn state(&self) -> ResourceState {
        severity_to_state(&self.severity_label)
    }
    fn state_label(&self) -> String {
        native_state_label(&self.severity_label, || self.state())
    }

    /// Suppressed and resolved findings load (a suppression hiding a real
    /// finding is the thing you most need to see) but fold away under `a`, as
    /// do passing control findings.
    fn is_noise(&self) -> bool {
        matches!(self.workflow_status.as_str(), "SUPPRESSED" | "RESOLVED")
            || self.compliance_status == PASSED
    }

    fn tags(&self) -> &HashMap<String, String> {
        static EMPTY: std::sync::OnceLock<HashMap<String, String>> = std::sync::OnceLock::new();
        EMPTY.get_or_init(HashMap::new)
    }

    fn search_text(&self) -> String {
        self.search_blob.clone()
    }

    /// CloudTrail indexes by resource, and knows nothing about an ASFF finding
    /// id — so look the affected resource up first.
    fn trail_lookup_keys(&self) -> Vec<String> {
        let mut keys: Vec<String> = self
            .resources
            .iter()
            .map(|(_, id)| id.clone())
            .filter(|id| !id.is_empty())
            .collect();
        keys.push(self.id.clone());
        keys
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Title".to_string(), self.title.clone()),
            ("Product".to_string(), self.product.clone()),
            ("Severity".to_string(), self.severity_label.clone()),
        ]
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/securityhub/home?region={}#/findings",
            region, region
        ))
    }

    fn raw_content(&self) -> Option<String> {
        Some(self.raw_json.clone())
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── ShFinding history (lazy) ────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ShHistoryRecord {
    pub time: String,
    pub source: String,
    pub identity: String,
    pub created: bool,
    /// `(field, old, new)`
    pub updates: Vec<(String, String, String)>,
}

/// The ASFF audit trail: who changed a finding's workflow status or note, and
/// when. Answers "was this triaged, or did a rule silence it?".
pub async fn fetch_finding_history(
    client: ShClient,
    id: String,
    product_arn: String,
) -> std::result::Result<Vec<ShHistoryRecord>, String> {
    let identifier = AwsSecurityFindingIdentifier::builder()
        .id(id)
        .product_arn(product_arn)
        .build();

    let mut out: Vec<ShHistoryRecord> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut req = client
            .get_finding_history()
            .finding_identifier(identifier.clone())
            .max_results(100);
        if let Some(t) = &token {
            req = req.next_token(t);
        }
        let page = req
            .send()
            .await
            .map_err(|e| crate::error::sdk_error_message(&e))?;
        for r in page.records() {
            out.push(ShHistoryRecord {
                time: r
                    .update_time()
                    .map(|t| t.to_string())
                    .unwrap_or_default(),
                source: r
                    .update_source()
                    .and_then(|s| s.r#type())
                    .map(|t| t.as_str().to_string())
                    .unwrap_or_default(),
                identity: r
                    .update_source()
                    .and_then(|s| s.identity())
                    .unwrap_or_default()
                    .to_string(),
                created: r.finding_created().unwrap_or(false),
                updates: r
                    .updates()
                    .iter()
                    .map(|u| {
                        (
                            u.updated_field().unwrap_or_default().to_string(),
                            u.old_value().unwrap_or_default().to_string(),
                            u.new_value().unwrap_or_default().to_string(),
                        )
                    })
                    .collect(),
            });
        }
        token = next_page_token(page.next_token(), &token);
        if token.is_none() || out.len() >= MAX_HISTORY_RECORDS {
            break;
        }
    }
    out.truncate(MAX_HISTORY_RECORDS);
    Ok(out)
}

// ── ShControl ───────────────────────────────────────────────────────────────

/// One security control — the console's Controls tab. A control is the unit a
/// standard is built from, and the level at which "are we compliant" is
/// actually answered.
#[derive(Debug, Clone)]
pub struct ShControl {
    pub id: String,
    pub arn: String,
    pub title: String,
    pub description: String,
    pub remediation_url: String,
    pub severity: String,
    /// ENABLED / DISABLED (account-wide, from `BatchGetSecurityControls`).
    pub control_status: String,
    pub update_status: String,
    pub last_update_reason: String,
    pub region_availability: String,
    /// Customized parameter values (`(name, value)`).
    pub parameters: Vec<(String, String)>,
    /// Properties this control allows customizing at all.
    pub customizable: Vec<String>,
    /// Enabled standards containing this control.
    pub standards: Vec<String>,
    pub failed_resources: usize,
    /// FAILED / PASSED / DISABLED / NO DATA.
    pub compliance: String,
    pub tags: HashMap<String, String>,
}

impl ShControl {
    fn build(
        def: &SecurityControlDefinition,
        enriched: Option<&SecurityControl>,
        standards: Vec<String>,
        failed_resources: usize,
        scan_complete: bool,
    ) -> Self {
        let control_status = enriched
            .and_then(|c| c.security_control_status())
            .map(|s| s.as_str().to_string())
            .unwrap_or_default();

        // A control not in any enabled standard is never evaluated, so it has
        // no verdict — "NO DATA", not a free pass. Likewise when the failed
        // scan hit its page cap: we can prove a failure, never its absence.
        let compliance = if control_status == DISABLED {
            DISABLED
        } else if failed_resources > 0 {
            FAILED
        } else if !standards.is_empty() && scan_complete {
            PASSED
        } else {
            NO_DATA
        }
        .to_string();

        let parameters = enriched
            .and_then(|c| c.parameters())
            .map(|m| {
                let mut kv: Vec<(String, String)> = m
                    .iter()
                    .map(|(k, v)| (k.clone(), parameter_value_text(v.value())))
                    .collect();
                kv.sort();
                kv
            })
            .unwrap_or_default();

        Self {
            id: def.security_control_id().unwrap_or_default().to_string(),
            arn: enriched
                .and_then(|c| c.security_control_arn())
                .unwrap_or_default()
                .to_string(),
            title: def.title().unwrap_or_default().to_string(),
            description: def.description().unwrap_or_default().to_string(),
            remediation_url: def.remediation_url().unwrap_or_default().to_string(),
            severity: def
                .severity_rating()
                .map(|s| s.as_str().to_string())
                .unwrap_or_default(),
            control_status,
            update_status: enriched
                .and_then(|c| c.update_status())
                .map(|s| s.as_str().to_string())
                .unwrap_or_default(),
            last_update_reason: enriched
                .and_then(|c| c.last_update_reason())
                .unwrap_or_default()
                .to_string(),
            region_availability: def
                .current_region_availability()
                .map(|s| s.as_str().to_string())
                .unwrap_or_default(),
            parameters,
            customizable: def
                .customizable_properties()
                .iter()
                .map(|p| p.as_str().to_string())
                .collect(),
            standards,
            failed_resources,
            compliance,
            tags: HashMap::new(),
        }
    }
}

/// Flatten the `ParameterValue` union to display text.
fn parameter_value_text(v: Option<&ParameterValue>) -> String {
    match v {
        Some(ParameterValue::Boolean(b)) => b.to_string(),
        Some(ParameterValue::Double(d)) => d.to_string(),
        Some(ParameterValue::Enum(s)) | Some(ParameterValue::String(s)) => s.clone(),
        Some(ParameterValue::EnumList(l)) | Some(ParameterValue::StringList(l)) => l.join(", "),
        Some(ParameterValue::Integer(i)) => i.to_string(),
        Some(ParameterValue::IntegerList(l)) => l
            .iter()
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join(", "),
        _ => String::new(),
    }
}

crate::sections! {
    pub enum ShControlDetailSection,
    pub static SH_CONTROL_SECTIONS = [
        Overview "Overview",
        Standards "Standards",
        Parameters "Parameters",
        Failing "Failing",
    ]
}

impl Resource for ShControl {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&SH_CONTROL_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.id
    }
    fn name(&self) -> &str {
        &self.id
    }
    fn resource_type(&self) -> &str {
        "Security Control"
    }
    fn state(&self) -> ResourceState {
        match self.compliance.as_str() {
            FAILED => ResourceState::Unavailable,
            PASSED => ResourceState::Available,
            other => ResourceState::Unknown(other.to_string()),
        }
    }
    fn state_label(&self) -> String {
        native_state_label(&self.compliance, || self.state())
    }
    /// `a` leaves exactly the controls something is failing.
    fn is_noise(&self) -> bool {
        self.compliance != FAILED
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        format!(
            "{} {} {} {} {} {}",
            self.id,
            self.title,
            self.severity,
            self.compliance,
            self.control_status,
            self.standards.join(" "),
        )
    }
    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Control".to_string(), self.id.clone()),
            ("Title".to_string(), self.title.clone()),
            ("Compliance".to_string(), self.compliance.clone()),
        ]
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/securityhub/home?region={}#/controls/{}",
            region, region, self.id
        ))
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── ShAutomationRule ────────────────────────────────────────────────────────

/// An automation rule — Security Hub's answer to a GuardDuty suppression
/// filter. A rule that forces `WorkflowStatus=SUPPRESSED` is the usual reason
/// a finding you expect isn't in the list, and nothing else in the app can
/// tell you one exists.
#[derive(Debug, Clone)]
pub struct ShAutomationRule {
    pub arn: String,
    pub name: String,
    pub description: String,
    pub status: String,
    pub order: i32,
    pub is_terminal: bool,
    pub created_at: String,
    pub updated_at: String,
    pub created_by: String,
    /// Pre-flattened match criteria (40 possible filter fields).
    pub criteria_rows: Vec<(String, String)>,
    /// Pre-flattened field updates the rule applies.
    pub action_rows: Vec<(String, String)>,
    /// The workflow status this rule forces, when it forces one.
    pub sets_workflow: String,
    pub tags: HashMap<String, String>,
}

impl ShAutomationRule {
    fn from_sdk(r: &AutomationRulesConfig) -> Self {
        let criteria_rows = r.criteria().map(flatten_criteria).unwrap_or_default();
        let mut action_rows: Vec<(String, String)> = Vec::new();
        let mut sets_workflow = String::new();
        for a in r.actions() {
            if let Some(u) = a.finding_fields_update() {
                if let Some(w) = u.workflow().and_then(|w| w.status()) {
                    sets_workflow = w.as_str().to_string();
                }
                action_rows.extend(flatten_field_update(u));
            }
        }
        Self {
            arn: r.rule_arn().unwrap_or_default().to_string(),
            name: r.rule_name().unwrap_or_default().to_string(),
            description: r.description().unwrap_or_default().to_string(),
            status: r
                .rule_status()
                .map(|s| s.as_str().to_string())
                .unwrap_or_default(),
            order: r.rule_order().unwrap_or(0),
            is_terminal: r.is_terminal().unwrap_or(false),
            created_at: r.created_at().map(|t| t.to_string()).unwrap_or_default(),
            updated_at: r.updated_at().map(|t| t.to_string()).unwrap_or_default(),
            created_by: r.created_by().unwrap_or_default().to_string(),
            criteria_rows,
            action_rows,
            sets_workflow,
            tags: HashMap::new(),
        }
    }

    /// A live rule that suppresses findings is worth a warning colour; one
    /// that only re-labels them is routine.
    pub fn suppresses(&self) -> bool {
        self.status != DISABLED && self.sets_workflow == "SUPPRESSED"
    }
}

crate::sections! {
    pub enum ShAutomationRuleDetailSection,
    pub static SH_AUTOMATION_SECTIONS = [
        Overview "Overview",
        Criteria "Criteria",
        Actions "Actions",
    ]
}

impl Resource for ShAutomationRule {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&SH_AUTOMATION_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.arn
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn resource_type(&self) -> &str {
        "Security Hub Automation Rule"
    }
    fn state(&self) -> ResourceState {
        if self.status == DISABLED {
            ResourceState::Stopped
        } else if self.suppresses() {
            ResourceState::Pending
        } else {
            ResourceState::Available
        }
    }
    fn state_label(&self) -> String {
        if self.status == DISABLED {
            "disabled"
        } else if self.suppresses() {
            "suppressing"
        } else {
            "enabled"
        }
        .to_string()
    }
    /// A disabled rule doesn't do anything; `a` leaves the live ones.
    fn is_noise(&self) -> bool {
        self.status == DISABLED
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        format!(
            "{} {} {} {} {}",
            self.name,
            self.description,
            self.status,
            self.sets_workflow,
            if self.suppresses() { "suppress" } else { "" },
        )
    }
    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Rule".to_string(), self.name.clone()),
            ("Status".to_string(), self.status.clone()),
            ("Order".to_string(), self.order.to_string()),
        ]
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Flatten the 40-field `AutomationRulesFindingFilters` into rows. Every field
/// is a list of filters, and only the populated ones are pushed.
fn flatten_criteria(c: &AutomationRulesFindingFilters) -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = Vec::new();
    let s = |rows: &mut Vec<(String, String)>, label: &str, f: &[StringFilter]| {
        push_string_filters(rows, label, f)
    };

    s(&mut rows, "Product ARN", c.product_arn());
    s(&mut rows, "Product Name", c.product_name());
    s(&mut rows, "Company", c.company_name());
    s(&mut rows, "Account", c.aws_account_id());
    s(&mut rows, "Account Name", c.aws_account_name());
    s(&mut rows, "Finding ID", c.id());
    s(&mut rows, "Generator", c.generator_id());
    s(&mut rows, "Title", c.title());
    s(&mut rows, "Description", c.description());
    s(&mut rows, "Source URL", c.source_url());
    s(&mut rows, "Severity", c.severity_label());
    s(&mut rows, "Resource Type", c.resource_type());
    s(&mut rows, "Resource ID", c.resource_id());
    s(&mut rows, "Resource Partition", c.resource_partition());
    s(&mut rows, "Resource Region", c.resource_region());
    s(&mut rows, "Application ARN", c.resource_application_arn());
    s(&mut rows, "Application Name", c.resource_application_name());
    s(&mut rows, "Compliance Status", c.compliance_status());
    s(&mut rows, "Control ID", c.compliance_security_control_id());
    s(&mut rows, "Standard", c.compliance_associated_standards_id());
    s(&mut rows, "Verification State", c.verification_state());
    s(&mut rows, "Workflow Status", c.workflow_status());
    s(&mut rows, "Record State", c.record_state());
    s(&mut rows, "Related Product ARN", c.related_findings_product_arn());
    s(&mut rows, "Related Finding ID", c.related_findings_id());
    s(&mut rows, "Note Text", c.note_text());
    s(&mut rows, "Note Updated By", c.note_updated_by());

    push_number_filters(&mut rows, "Confidence", c.confidence());
    push_number_filters(&mut rows, "Criticality", c.criticality());

    push_date_filters(&mut rows, "First Observed", c.first_observed_at());
    push_date_filters(&mut rows, "Last Observed", c.last_observed_at());
    push_date_filters(&mut rows, "Created", c.created_at());
    push_date_filters(&mut rows, "Updated", c.updated_at());
    push_date_filters(&mut rows, "Note Updated", c.note_updated_at());

    push_map_filters(&mut rows, "Resource Tag", c.resource_tags());
    push_map_filters(&mut rows, "Resource Detail", c.resource_details_other());
    push_map_filters(&mut rows, "User-Defined Field", c.user_defined_fields());

    rows
}

fn push_string_filters(rows: &mut Vec<(String, String)>, label: &str, filters: &[StringFilter]) {
    for f in filters {
        let comparison = f
            .comparison()
            .map(|c| c.as_str().to_string())
            .unwrap_or_default();
        let value = f.value().unwrap_or_default();
        rows.push((
            label.to_string(),
            format!("{} {}", comparison, value).trim().to_string(),
        ));
    }
}

fn push_number_filters(
    rows: &mut Vec<(String, String)>,
    label: &str,
    filters: &[aws_sdk_securityhub::types::NumberFilter],
) {
    for f in filters {
        // A range is gte + lte on one filter, so joining beats first-match.
        let mut parts: Vec<String> = Vec::new();
        if let Some(v) = f.eq() {
            parts.push(format!("= {}", v));
        }
        if let Some(v) = f.gt() {
            parts.push(format!("> {}", v));
        }
        if let Some(v) = f.gte() {
            parts.push(format!("≥ {}", v));
        }
        if let Some(v) = f.lt() {
            parts.push(format!("< {}", v));
        }
        if let Some(v) = f.lte() {
            parts.push(format!("≤ {}", v));
        }
        rows.push((label.to_string(), parts.join("  ")));
    }
}

fn push_date_filters(
    rows: &mut Vec<(String, String)>,
    label: &str,
    filters: &[aws_sdk_securityhub::types::DateFilter],
) {
    for f in filters {
        let text = if let Some(r) = f.date_range() {
            format!(
                "last {} {}",
                r.value().unwrap_or(0),
                r.unit().map(|u| u.as_str()).unwrap_or("")
            )
            .trim()
            .to_string()
        } else {
            format!(
                "{} → {}",
                f.start().unwrap_or("(any)"),
                f.end().unwrap_or("(any)")
            )
        };
        rows.push((label.to_string(), text));
    }
}

fn push_map_filters(
    rows: &mut Vec<(String, String)>,
    label: &str,
    filters: &[aws_sdk_securityhub::types::MapFilter],
) {
    for f in filters {
        let comparison = f
            .comparison()
            .map(|c| c.as_str().to_string())
            .unwrap_or_default();
        rows.push((
            format!("{} {}", label, f.key().unwrap_or_default())
                .trim()
                .to_string(),
            format!("{} {}", comparison, f.value().unwrap_or_default())
                .trim()
                .to_string(),
        ));
    }
}

/// Flatten the field updates a rule applies to a matching finding.
fn flatten_field_update(u: &AutomationRulesFindingFieldsUpdate) -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = Vec::new();
    if let Some(w) = u.workflow().and_then(|w| w.status()) {
        rows.push(("Workflow Status".to_string(), w.as_str().to_string()));
    }
    if let Some(s) = u.severity() {
        if let Some(l) = s.label() {
            rows.push(("Severity".to_string(), l.as_str().to_string()));
        }
        if let Some(n) = s.normalized() {
            rows.push(("Severity (0–100)".to_string(), n.to_string()));
        }
    }
    if let Some(v) = u.verification_state() {
        rows.push(("Verification State".to_string(), v.as_str().to_string()));
    }
    if let Some(c) = u.confidence() {
        rows.push(("Confidence".to_string(), c.to_string()));
    }
    if let Some(c) = u.criticality() {
        rows.push(("Criticality".to_string(), c.to_string()));
    }
    if !u.types().is_empty() {
        rows.push(("Types".to_string(), u.types().join(", ")));
    }
    if let Some(n) = u.note() {
        rows.push((
            "Note".to_string(),
            n.text().unwrap_or_default().to_string(),
        ));
        if let Some(by) = n.updated_by() {
            rows.push(("Note By".to_string(), by.to_string()));
        }
    }
    if let Some(m) = u.user_defined_fields() {
        let mut kv: Vec<(&String, &String)> = m.iter().collect();
        kv.sort();
        for (k, v) in kv {
            rows.push((format!("Field {}", k), v.clone()));
        }
    }
    for r in u.related_findings() {
        rows.push((
            "Related Finding".to_string(),
            r.id().unwrap_or_default().to_string(),
        ));
    }
    rows
}

// ── ShActionTarget ──────────────────────────────────────────────────────────

/// A custom action — the EventBridge hook a human fires from a finding.
/// Shares the Automations tab with rules (the VPC-Routing grouped-tab
/// precedent): both are "something other than triage acts on this finding",
/// and neither fills a tab alone.
#[derive(Debug, Clone)]
pub struct ShActionTarget {
    pub arn: String,
    pub name: String,
    pub description: String,
    pub tags: HashMap<String, String>,
}

impl ShActionTarget {
    fn from_sdk(t: &aws_sdk_securityhub::types::ActionTarget) -> Self {
        Self {
            arn: t.action_target_arn().unwrap_or_default().to_string(),
            name: t.name().unwrap_or_default().to_string(),
            description: t.description().unwrap_or_default().to_string(),
            tags: HashMap::new(),
        }
    }
}

impl Resource for ShActionTarget {
    fn id(&self) -> &str {
        &self.arn
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn resource_type(&self) -> &str {
        "Security Hub Custom Action"
    }
    fn state(&self) -> ResourceState {
        ResourceState::stateless()
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        format!("{} {} {}", self.name, self.description, self.arn)
    }
    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Custom Action".to_string(), self.name.clone()),
            ("Description".to_string(), self.description.clone()),
            ("ARN".to_string(), self.arn.clone()),
            (String::new(), String::new()),
            (
                "  · fires an EventBridge event — wire a rule to it to act".to_string(),
                String::new(),
            ),
        ]
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── ShProduct ───────────────────────────────────────────────────────────────

/// An integration — a product that can send findings into Security Hub.
/// Answers "where do these findings actually come from", and (via `a`) "what
/// else could be feeding us and isn't".
#[derive(Debug, Clone)]
pub struct ShProduct {
    pub arn: String,
    pub name: String,
    pub company: String,
    pub description: String,
    pub categories: Vec<String>,
    pub integration_types: Vec<String>,
    pub marketplace_url: String,
    pub activation_url: String,
    pub enabled: bool,
    pub tags: HashMap<String, String>,
}

impl ShProduct {
    fn from_sdk(p: &aws_sdk_securityhub::types::Product, enabled: bool) -> Self {
        Self {
            arn: p.product_arn().unwrap_or_default().to_string(),
            name: p.product_name().unwrap_or_default().to_string(),
            company: p.company_name().unwrap_or_default().to_string(),
            description: p.description().unwrap_or_default().to_string(),
            categories: p.categories().to_vec(),
            integration_types: p
                .integration_types()
                .iter()
                .map(|t| t.as_str().to_string())
                .collect(),
            marketplace_url: p.marketplace_url().unwrap_or_default().to_string(),
            activation_url: p.activation_url().unwrap_or_default().to_string(),
            enabled,
            tags: HashMap::new(),
        }
    }
}

impl Resource for ShProduct {
    fn id(&self) -> &str {
        &self.arn
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn resource_type(&self) -> &str {
        "Security Hub Integration"
    }
    fn state(&self) -> ResourceState {
        if self.enabled {
            ResourceState::Available
        } else {
            ResourceState::Unknown(String::new())
        }
    }
    fn state_label(&self) -> String {
        if self.enabled { "subscribed" } else { "not subscribed" }.to_string()
    }
    /// `a` narrows the catalogue to what's actually feeding findings (the
    /// Control Tower Catalog precedent).
    fn is_noise(&self) -> bool {
        !self.enabled
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        format!(
            "{} {} {} {} {}",
            self.name,
            self.company,
            self.categories.join(" "),
            self.integration_types.join(" "),
            if self.enabled { "enabled" } else { "" },
        )
    }
    fn details(&self) -> Vec<(String, String)> {
        let mut rows = vec![
            ("Integration".to_string(), self.name.clone()),
            ("Company".to_string(), self.company.clone()),
            (
                "Subscribed".to_string(),
                if self.enabled { "✓ yes" } else { "✗ no" }.to_string(),
            ),
        ];
        if !self.integration_types.is_empty() {
            rows.push((
                "Integration Types".to_string(),
                self.integration_types.join(", "),
            ));
        }
        if !self.description.is_empty() {
            rows.push((String::new(), String::new()));
            rows.push(("Description".to_string(), String::new()));
            for line in wrap_words(&self.description, 64) {
                rows.push((format!("  {}", line), String::new()));
            }
        }
        if !self.categories.is_empty() {
            rows.push((String::new(), String::new()));
            rows.push(("Categories".to_string(), String::new()));
            for c in &self.categories {
                rows.push((format!("  {}", c), String::new()));
            }
        }
        rows.push((String::new(), String::new()));
        rows.push(("ARN".to_string(), self.arn.clone()));
        if !self.marketplace_url.is_empty() {
            rows.push(("Marketplace".to_string(), self.marketplace_url.clone()));
        }
        if !self.activation_url.is_empty() {
            rows.push(("Activation".to_string(), self.activation_url.clone()));
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

/// The trailing number of a managed-insight ARN, for ordering them the way
/// the console numbers them (`1.`, `2.`, …). Custom insights sort last.
fn managed_index(arn: &str) -> u32 {
    arn.rsplit('/')
        .next()
        .and_then(|n| n.parse().ok())
        .unwrap_or(u32::MAX)
}

/// The shared vendor/product tail of a product ARN and its subscription ARN —
/// `arn:…::product/aws/guardduty` and
/// `arn:…:123:product-subscription/aws/guardduty` both reduce to `aws/guardduty`.
fn product_key(arn: &str) -> String {
    for marker in ["product-subscription/", "product/"] {
        if let Some(tail) = arn.split(marker).nth(1) {
            return tail.to_string();
        }
    }
    arn.to_string()
}

// ── ShSettings ──────────────────────────────────────────────────────────────

/// The account's Security Hub posture as one synthetic row — hub settings,
/// cross-region aggregation, and org configuration. Scalars, not a list, so
/// it's a row rather than a resource type (the `GdOverview` precedent).
#[derive(Debug, Clone, Default)]
pub struct ShSettings {
    pub hub_arn: String,
    pub subscribed_at: String,
    pub auto_enable_controls: Option<bool>,
    pub control_finding_generator: String,

    pub aggregation_configured: bool,
    pub aggregation_region: String,
    pub region_linking_mode: String,
    pub linked_regions: Vec<String>,

    /// True once `DescribeOrganizationConfiguration` answered — it only does
    /// for the administrator, so it distinguishes "not an org" from "not us".
    pub org_readable: bool,
    pub auto_enable_members: Option<bool>,
    pub member_limit_reached: Option<bool>,
    pub auto_enable_standards: String,
    pub org_config_type: String,
    pub org_config_status: String,
    pub org_config_status_message: String,
    pub delegated_admins: Vec<String>,
    pub administrator_account: String,
    pub administrator_status: String,

    /// Section-prefixed failures; each section renders only its own.
    pub errors: Vec<String>,
    pub tags: HashMap<String, String>,
}

impl ShSettings {
    /// Failures whose prefix matches this section, as `⚠` content lines.
    pub fn section_errors(&self, prefix: &str) -> Vec<(String, String)> {
        self.errors
            .iter()
            .filter(|e| e.starts_with(prefix))
            .map(|e| (format!("  ⚠ {}", e), String::new()))
            .collect()
    }
}

crate::sections! {
    pub enum ShSettingsDetailSection,
    pub static SH_SETTINGS_SECTIONS = [
        Overview "Overview",
        Regions "Regions",
        Organization "Organization",
    ]
}

impl Resource for ShSettings {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&SH_SETTINGS_SECTIONS)
    }
    fn id(&self) -> &str {
        "security-hub-settings"
    }
    fn name(&self) -> &str {
        "Security Hub Settings"
    }
    fn resource_type(&self) -> &str {
        "Security Hub Settings"
    }
    fn state(&self) -> ResourceState {
        if self.hub_arn.is_empty() {
            ResourceState::Unavailable
        } else {
            ResourceState::Available
        }
    }
    fn state_label(&self) -> String {
        if self.hub_arn.is_empty() { "not enabled" } else { "enabled" }.to_string()
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        format!(
            "security hub settings configuration {} {} {}",
            self.aggregation_region, self.org_config_type, self.control_finding_generator
        )
    }
    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Settings".to_string(), "Security Hub".to_string()),
            (
                "Aggregation".to_string(),
                if self.aggregation_configured {
                    self.aggregation_region.clone()
                } else {
                    "not configured".to_string()
                },
            ),
        ]
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── ShConfigPolicy ──────────────────────────────────────────────────────────

/// One target of a configuration policy.
#[derive(Debug, Clone)]
pub struct ShPolicyTarget {
    pub target_id: String,
    pub target_type: String,
    /// APPLIED (attached here) vs INHERITED (from an ancestor OU).
    pub association_type: String,
    pub status: String,
    pub status_message: String,
    pub updated_at: String,
}

impl ShPolicyTarget {
    fn from_sdk(a: &aws_sdk_securityhub::types::ConfigurationPolicyAssociationSummary) -> Self {
        Self {
            target_id: a.target_id().unwrap_or_default().to_string(),
            target_type: a
                .target_type()
                .map(|t| t.as_str().to_string())
                .unwrap_or_default(),
            association_type: a
                .association_type()
                .map(|t| t.as_str().to_string())
                .unwrap_or_default(),
            status: a
                .association_status()
                .map(|s| s.as_str().to_string())
                .unwrap_or_default(),
            status_message: a.association_status_message().unwrap_or_default().to_string(),
            updated_at: a.updated_at().map(|t| t.to_string()).unwrap_or_default(),
        }
    }
}

/// A central-configuration policy — the CSPM-era mechanism that decides, org
/// wide, whether Security Hub is on and which standards and controls run.
/// It's the answer to "why is this control disabled in this account".
#[derive(Debug, Clone)]
pub struct ShConfigPolicy {
    pub id: String,
    pub arn: String,
    pub name: String,
    pub description: String,
    pub created_at: String,
    pub updated_at: String,
    pub service_enabled: bool,
    pub enabled_standards: Vec<String>,
    /// Exactly one of these is populated: naming enabled controls disables
    /// every other (including future ones), and vice versa.
    pub enabled_controls: Vec<String>,
    pub disabled_controls: Vec<String>,
    pub custom_parameters: Vec<(String, String)>,
    pub targets: Vec<ShPolicyTarget>,
    /// Set when `GetConfigurationPolicy` failed — the row survives with what
    /// the list call gave us.
    pub detail_error: String,
    pub tags: HashMap<String, String>,
}

impl ShConfigPolicy {
    fn stub(id: &str, name: &str) -> Self {
        Self {
            id: id.to_string(),
            arn: String::new(),
            name: name.to_string(),
            description: String::new(),
            created_at: String::new(),
            updated_at: String::new(),
            service_enabled: true,
            enabled_standards: Vec::new(),
            enabled_controls: Vec::new(),
            disabled_controls: Vec::new(),
            custom_parameters: Vec::new(),
            targets: Vec::new(),
            detail_error: String::new(),
            tags: HashMap::new(),
        }
    }

    fn from_sdk(
        p: &aws_sdk_securityhub::operation::get_configuration_policy::GetConfigurationPolicyOutput,
        targets: Vec<ShPolicyTarget>,
    ) -> Self {
        let mut out = Self::stub(
            p.id().unwrap_or_default(),
            p.name().unwrap_or_default(),
        );
        out.arn = p.arn().unwrap_or_default().to_string();
        out.description = p.description().unwrap_or_default().to_string();
        out.created_at = p.created_at().map(|t| t.to_string()).unwrap_or_default();
        out.updated_at = p.updated_at().map(|t| t.to_string()).unwrap_or_default();
        out.targets = targets;

        if let Some(sh) = p
            .configuration_policy()
            .and_then(|pol| pol.as_security_hub().ok())
        {
            out.service_enabled = sh.service_enabled().unwrap_or(false);
            out.enabled_standards = sh.enabled_standard_identifiers().to_vec();
            if let Some(c) = sh.security_controls_configuration() {
                out.enabled_controls = c.enabled_security_control_identifiers().to_vec();
                out.disabled_controls = c.disabled_security_control_identifiers().to_vec();
                for cp in c.security_control_custom_parameters() {
                    let control = cp.security_control_id().unwrap_or_default();
                    if let Some(params) = cp.parameters() {
                        let mut sorted: Vec<(&String, _)> = params.iter().collect();
                        sorted.sort_by(|a, b| a.0.cmp(b.0));
                        for (name, cfg) in sorted {
                            out.custom_parameters.push((
                                format!("{} · {}", control, name),
                                parameter_value_text(cfg.value()),
                            ));
                        }
                    }
                }
            }
        }
        out
    }

    /// A policy nothing is attached to isn't configuring anything.
    pub fn failed_targets(&self) -> usize {
        self.targets.iter().filter(|t| t.status == "FAILED").count()
    }
}

crate::sections! {
    pub enum ShConfigPolicyDetailSection,
    pub static SH_CONFIG_POLICY_SECTIONS = [
        Overview "Overview",
        Controls "Controls",
        Targets "Targets",
    ]
}

impl Resource for ShConfigPolicy {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&SH_CONFIG_POLICY_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.id
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn resource_type(&self) -> &str {
        "Security Hub Config Policy"
    }
    fn state(&self) -> ResourceState {
        if self.failed_targets() > 0 {
            ResourceState::Unavailable
        } else if !self.service_enabled {
            ResourceState::Stopped
        } else if self.targets.is_empty() {
            ResourceState::Unknown("UNASSOCIATED".to_string())
        } else {
            ResourceState::Available
        }
    }
    fn state_label(&self) -> String {
        if self.failed_targets() > 0 {
            "failed"
        } else if !self.service_enabled {
            "disabled"
        } else if self.targets.is_empty() {
            "unassociated"
        } else {
            "associated"
        }
        .to_string()
    }
    fn is_noise(&self) -> bool {
        self.targets.is_empty()
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        format!(
            "{} {} {} {}",
            self.name,
            self.description,
            self.id,
            self.targets
                .iter()
                .map(|t| t.target_id.as_str())
                .collect::<Vec<_>>()
                .join(" "),
        )
    }
    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Policy".to_string(), self.name.clone()),
            ("Targets".to_string(), self.targets.len().to_string()),
        ]
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── ShMember ────────────────────────────────────────────────────────────────

/// A member account of this Security Hub administrator.
#[derive(Debug, Clone)]
pub struct ShMember {
    pub account_id: String,
    pub email: String,
    pub status: String,
    pub administrator_id: String,
    pub invited_at: String,
    pub updated_at: String,
    pub tags: HashMap<String, String>,
}

impl ShMember {
    fn from_sdk(m: &aws_sdk_securityhub::types::Member) -> Self {
        Self {
            account_id: m.account_id().unwrap_or_default().to_string(),
            email: m.email().unwrap_or_default().to_string(),
            status: m.member_status().unwrap_or_default().to_string(),
            // Members onboarded through older API versions still come back on
            // the deprecated `MasterId` alias with `AdministratorId` empty,
            // so fall back rather than showing a blank administrator.
            administrator_id: {
                #[allow(deprecated)]
                let legacy = m.master_id();
                m.administrator_id()
                    .or(legacy)
                    .unwrap_or_default()
                    .to_string()
            },
            invited_at: m.invited_at().map(|t| t.to_string()).unwrap_or_default(),
            updated_at: m.updated_at().map(|t| t.to_string()).unwrap_or_default(),
            tags: HashMap::new(),
        }
    }
}

/// Worst-first ordering. `Enabled` / `Associated` are the healthy states;
/// everything else means the account isn't reporting findings.
fn member_rank(status: &str) -> u8 {
    match status {
        "Removed" | "Disabled" => 0,
        "Created" | "Invited" | "Resigned" => 1,
        "Enabled" | "Associated" => 3,
        _ => 2,
    }
}

impl Resource for ShMember {
    fn id(&self) -> &str {
        &self.account_id
    }
    fn name(&self) -> &str {
        &self.account_id
    }
    fn resource_type(&self) -> &str {
        "Security Hub Member"
    }
    fn state(&self) -> ResourceState {
        match member_rank(&self.status) {
            0 => ResourceState::Unavailable,
            3 => ResourceState::Available,
            _ => ResourceState::Pending,
        }
    }
    fn state_label(&self) -> String {
        native_state_label(&self.status, || self.state())
    }
    /// `a` leaves exactly the accounts that aren't reporting.
    fn is_noise(&self) -> bool {
        member_rank(&self.status) == 3
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        format!("{} {} {}", self.account_id, self.email, self.status)
    }
    fn details(&self) -> Vec<(String, String)> {
        let mut rows = vec![
            ("Account".to_string(), self.account_id.clone()),
            ("Status".to_string(), self.status.clone()),
        ];
        if !self.email.is_empty() {
            rows.push(("Email".to_string(), self.email.clone()));
        }
        if !self.administrator_id.is_empty() {
            rows.push(("Administrator".to_string(), self.administrator_id.clone()));
        }
        if !self.invited_at.is_empty() {
            rows.push(("Invited".to_string(), self.invited_at.clone()));
        }
        if !self.updated_at.is_empty() {
            rows.push(("Updated".to_string(), self.updated_at.clone()));
        }
        if member_rank(&self.status) != 3 {
            rows.push((String::new(), String::new()));
            rows.push((
                "  ⚠ not reporting findings to this administrator".to_string(),
                String::new(),
            ));
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

// ── ShOverview (dashboard summary) ───────────────────────────────────────────

/// A single-row "resource" representing the Security Hub dashboard overview.
/// Shown in the Overview sub-tab with per-standard scores and severity breakdown.
#[derive(Debug, Clone)]
pub struct ShOverview {
    pub standards: Vec<ShStandardScore>,
    pub critical_count: usize,
    pub high_count: usize,
    pub medium_count: usize,
    pub low_count: usize,
    pub informational_count: usize,
    pub total_findings: usize,
    pub controls_failed: usize,
    pub controls_total: usize,
    /// How many loaded findings a rule or an analyst has silenced.
    pub suppressed_count: usize,
    /// The severity scope the findings were fetched at, and whether the cap
    /// truncated them — without both, the severity breakdown reads as an
    /// account-wide total it isn't.
    pub severity_scope: String,
    pub findings_capped: bool,
    /// False when the failed-control scan hit its page cap, which makes every
    /// score on this page a floor.
    pub scan_complete: bool,
}

/// Per-standard compliance score.
#[derive(Debug, Clone)]
pub struct ShStandardScore {
    pub name: String,
    pub controls_passed: i32,
    pub controls_enabled: i32,
}

impl ShStandardScore {
    pub fn score_pct(&self) -> u8 {
        if self.controls_enabled == 0 {
            return 0;
        }
        ((self.controls_passed as f64 / self.controls_enabled as f64) * 100.0).round() as u8
    }
}

impl ShOverview {
    pub fn overall_score(&self) -> u8 {
        let total_enabled: i32 = self.standards.iter().map(|s| s.controls_enabled).sum();
        let total_passed: i32 = self.standards.iter().map(|s| s.controls_passed).sum();
        if total_enabled == 0 {
            return 0;
        }
        ((total_passed as f64 / total_enabled as f64) * 100.0).round() as u8
    }
}

crate::sections! {
    pub enum ShOverviewDetailSection,
    pub static SH_OVERVIEW_SECTIONS = [
        Score "Score",
        Findings "Findings",
        Controls "Controls",
        Coverage "Coverage",
    ]
}

impl Resource for ShOverview {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&SH_OVERVIEW_SECTIONS)
    }
    fn id(&self) -> &str {
        "security-hub-overview"
    }
    fn name(&self) -> &str {
        "Security Score Overview"
    }
    fn resource_type(&self) -> &str {
        "Security Hub Overview"
    }
    fn state(&self) -> ResourceState {
        let score = self.overall_score();
        if score >= 80 {
            ResourceState::Available
        } else if score >= 50 {
            ResourceState::Pending
        } else {
            ResourceState::Unavailable
        }
    }
    fn state_label(&self) -> String {
        // Same score bands as `state()`: ≥80 / ≥50 / below.
        let score = self.overall_score();
        if score >= 80 {
            "healthy"
        } else if score >= 50 {
            "degraded"
        } else {
            "failing"
        }
        .to_string()
    }
    fn tags(&self) -> &HashMap<String, String> {
        static EMPTY: std::sync::OnceLock<HashMap<String, String>> = std::sync::OnceLock::new();
        EMPTY.get_or_init(HashMap::new)
    }
    fn search_text(&self) -> String {
        "security hub overview dashboard score".to_string()
    }
    fn details(&self) -> Vec<(String, String)> {
        let mut rows = vec![
            (
                "Overall Score".to_string(),
                format!("{}%", self.overall_score()),
            ),
            ("Total Findings".to_string(), self.total_findings.to_string()),
            (
                "Failing Controls".to_string(),
                format!("{} / {}", self.controls_failed, self.controls_total),
            ),
            (String::new(), String::new()),
            ("Severity Breakdown".to_string(), String::new()),
            ("  Critical".to_string(), self.critical_count.to_string()),
            ("  High".to_string(), self.high_count.to_string()),
            ("  Medium".to_string(), self.medium_count.to_string()),
            ("  Low".to_string(), self.low_count.to_string()),
        ];
        if self.informational_count > 0 {
            rows.push((
                "  Informational".to_string(),
                self.informational_count.to_string(),
            ));
        }
        rows.push((String::new(), String::new()));
        rows.push(("Standards".to_string(), String::new()));
        for s in &self.standards {
            rows.push((
                format!("  {}", s.name),
                format!(
                    "{}%  ({}/{})",
                    s.score_pct(),
                    s.controls_passed,
                    s.controls_enabled
                ),
            ));
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

// ── ShStandard ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ShStandard {
    pub name: String,
    pub arn: String,
    pub status: String,
    pub controls_enabled: i32,
    pub controls_disabled: i32,
    pub controls_passed: i32,
    pub controls_failed: i32,
    /// True when the failed-control scan was truncated, so the score is a
    /// floor rather than a measurement.
    pub score_estimated: bool,
    pub tags: HashMap<String, String>,
}

impl ShStandard {
    fn from_subscription(sub: &aws_sdk_securityhub::types::StandardsSubscription) -> Self {
        let arn = sub.standards_arn().unwrap_or_default().to_string();
        // arn:…:standards/aws-foundational-security-best-practices/v/1.0.0 → name
        let name = arn
            .split("standards/")
            .nth(1)
            .and_then(|s| s.split("/v/").next())
            .unwrap_or(&arn)
            .to_string();
        Self {
            name,
            arn,
            status: sub
                .standards_status()
                .map(|s| s.as_str().to_string())
                .unwrap_or_default(),
            controls_enabled: 0,
            controls_disabled: 0,
            controls_passed: 0,
            controls_failed: 0,
            score_estimated: false,
            tags: HashMap::new(),
        }
    }

    pub fn score_pct(&self) -> u8 {
        if self.controls_enabled == 0 {
            return 0;
        }
        ((self.controls_passed as f64 / self.controls_enabled as f64) * 100.0).round() as u8
    }
}

impl Resource for ShStandard {
    fn id(&self) -> &str {
        &self.arn
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn resource_type(&self) -> &str {
        "Security Standard"
    }
    fn state(&self) -> ResourceState {
        match self.status.as_str() {
            "READY" | "PENDING" => ResourceState::Available,
            "DELETING" | "INCOMPLETE" => ResourceState::Pending,
            other => ResourceState::Unknown(other.to_string()),
        }
    }
    fn state_label(&self) -> String {
        native_state_label(&self.status, || self.state())
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        format!("{} {} {}", self.name, self.status, self.arn)
    }
    fn details(&self) -> Vec<(String, String)> {
        let score = if self.controls_enabled > 0 {
            format!("{}%", self.score_pct())
        } else {
            "—".to_string()
        };
        let mut rows = vec![
            ("Standard".to_string(), self.name.clone()),
            ("Status".to_string(), self.status.clone()),
            ("Security Score".to_string(), score),
            (
                "Controls Passed".to_string(),
                self.controls_passed.to_string(),
            ),
            (
                "Controls Failed".to_string(),
                self.controls_failed.to_string(),
            ),
            (
                "Controls Enabled".to_string(),
                self.controls_enabled.to_string(),
            ),
            (
                "Controls Disabled".to_string(),
                self.controls_disabled.to_string(),
            ),
            ("ARN".to_string(), self.arn.clone()),
        ];
        if self.score_estimated {
            rows.push((String::new(), String::new()));
            rows.push((
                "  · score is a floor — the failed-control scan hit its page cap".to_string(),
                String::new(),
            ));
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

// ── ShInsight ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ShInsight {
    pub name: String,
    pub arn: String,
    pub group_by: String,
    /// Pre-flattened filter rows (the common `AwsSecurityFindingFilters`
    /// fields — see `flatten_insight_filters`).
    pub filter_rows: Vec<(String, String)>,
    /// AWS-managed insights ship with the service and have no account id in
    /// their ARN; custom ones are what somebody here actually built.
    pub managed: bool,
    pub tags: HashMap<String, String>,
}

impl ShInsight {
    fn from_sdk(i: &aws_sdk_securityhub::types::Insight) -> Self {
        let arn = i.insight_arn().unwrap_or_default().to_string();
        Self {
            name: i.name().unwrap_or_default().to_string(),
            // `arn:aws:securityhub:::insight/securityhub/default/N` — the
            // empty account field is what marks a managed insight.
            managed: arn.contains(":::insight/"),
            arn,
            group_by: i.group_by_attribute().unwrap_or_default().to_string(),
            filter_rows: i.filters().map(flatten_insight_filters).unwrap_or_default(),
            tags: HashMap::new(),
        }
    }
}

/// One grouped row of an insight's results.
#[derive(Debug, Clone)]
pub struct ShInsightResults {
    pub group_by: String,
    /// `(group value, finding count)`, highest first.
    pub values: Vec<(String, i32)>,
    /// True when the result set was longer than `MAX_INSIGHT_RESULTS`.
    pub truncated: bool,
}

/// Run an insight and return its grouped counts — the actual point of an
/// insight, and the one thing the Insights tab never showed.
pub async fn fetch_insight_results(
    client: ShClient,
    arn: String,
) -> std::result::Result<ShInsightResults, String> {
    let resp = client
        .get_insight_results()
        .insight_arn(arn)
        .send()
        .await
        .map_err(|e| crate::error::sdk_error_message(&e))?;
    let results = match resp.insight_results() {
        Some(r) => r,
        None => {
            return Ok(ShInsightResults {
                group_by: String::new(),
                values: Vec::new(),
                truncated: false,
            })
        }
    };
    let mut values: Vec<(String, i32)> = results
        .result_values()
        .iter()
        .map(|v| {
            (
                v.group_by_attribute_value().unwrap_or_default().to_string(),
                v.count().unwrap_or(0),
            )
        })
        .collect();
    values.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
    let truncated = values.len() > MAX_INSIGHT_RESULTS;
    values.truncate(MAX_INSIGHT_RESULTS);
    Ok(ShInsightResults {
        group_by: results.group_by_attribute().unwrap_or_default().to_string(),
        values,
        truncated,
    })
}

/// Flatten the filters an insight groups over. `AwsSecurityFindingFilters` has
/// ~90 members; this renders the ones insights are actually built on rather
/// than generating ninety near-empty rows.
fn flatten_insight_filters(f: &AwsSecurityFindingFilters) -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = Vec::new();
    let s = |rows: &mut Vec<(String, String)>, label: &str, filters: &[StringFilter]| {
        push_string_filters(rows, label, filters)
    };
    s(&mut rows, "Product", f.product_name());
    s(&mut rows, "Product ARN", f.product_arn());
    s(&mut rows, "Company", f.company_name());
    s(&mut rows, "Generator", f.generator_id());
    s(&mut rows, "Account", f.aws_account_id());
    s(&mut rows, "Account Name", f.aws_account_name());
    s(&mut rows, "Region", f.region());
    s(&mut rows, "Finding Type", f.finding_provider_fields_types());
    s(&mut rows, "Title", f.title());
    s(&mut rows, "Description", f.description());
    s(&mut rows, "Severity", f.severity_label());
    s(&mut rows, "Workflow Status", f.workflow_status());
    s(&mut rows, "Record State", f.record_state());
    s(&mut rows, "Verification State", f.verification_state());
    s(&mut rows, "Compliance Status", f.compliance_status());
    s(&mut rows, "Control ID", f.compliance_security_control_id());
    s(&mut rows, "Standard", f.compliance_associated_standards_id());
    s(&mut rows, "Resource Type", f.resource_type());
    s(&mut rows, "Resource ID", f.resource_id());
    s(&mut rows, "Resource Region", f.resource_region());
    s(&mut rows, "Resource Partition", f.resource_partition());
    s(&mut rows, "Application Name", f.resource_application_name());
    s(&mut rows, "Exploit Available", f.vulnerabilities_exploit_available());
    s(&mut rows, "Fix Available", f.vulnerabilities_fix_available());
    s(&mut rows, "Note Text", f.note_text());

    push_number_filters(&mut rows, "Confidence", f.confidence());
    push_number_filters(&mut rows, "Criticality", f.criticality());
    // Deprecated filters, still populated on insights built before AWS
    // replaced them — rendering them beats showing a filter that isn't there.
    #[allow(deprecated)]
    push_number_filters(&mut rows, "Severity (0–100)", f.severity_normalized());
    push_date_filters(&mut rows, "First Observed", f.first_observed_at());
    push_date_filters(&mut rows, "Last Observed", f.last_observed_at());
    push_date_filters(&mut rows, "Created", f.created_at());
    push_date_filters(&mut rows, "Updated", f.updated_at());
    push_map_filters(&mut rows, "Resource Tag", f.resource_tags());
    push_map_filters(&mut rows, "User-Defined Field", f.user_defined_fields());
    push_map_filters(&mut rows, "Product Field", f.product_fields());
    #[allow(deprecated)]
    for k in f.keyword() {
        rows.push((
            "Keyword".to_string(),
            k.value().unwrap_or_default().to_string(),
        ));
    }
    rows
}

crate::sections! {
    /// **Results is section 0 deliberately** — the codebase convention is
    /// Overview-first, but the stronger principle is that the default section
    /// is what you came for, and an insight *is* its grouped results. As
    /// section 0 its hook also fires on drill-in, so the results load the
    /// moment the pane opens. The handful of metadata rows an Overview would
    /// have held lead the Results body instead.
    pub enum ShInsightDetailSection,
    pub static SH_INSIGHT_SECTIONS = [
        Results "Results" => crate::app::App::trigger_sh_insight_results_load,
        Filters "Filters",
    ]
}

impl Resource for ShInsight {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&SH_INSIGHT_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.arn
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn resource_type(&self) -> &str {
        "Security Insight"
    }
    fn state(&self) -> ResourceState {
        ResourceState::stateless()
    }
    // Deliberately NOT `is_noise() = managed`, unlike CloudFront's managed
    // policies. Most accounts have zero *custom* insights, so folding the
    // AWS-managed ones away empties the tab entirely — and `a` is a global
    // toggle, so pressing it on Controls (where it's the right move) would
    // silently blank Insights. Origin is searchable instead: `custom` or
    // `managed` narrows the list without a filter that can hide everything.
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        format!(
            "{} {} {} {}",
            self.name,
            self.group_by,
            self.arn,
            if self.managed { "managed" } else { "custom" },
        )
    }
    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Insight".to_string(), self.name.clone()),
            ("Group By".to_string(), self.group_by.clone()),
            ("ARN".to_string(), self.arn.clone()),
        ]
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── Severity helper ─────────────────────────────────────────────────────────

/// Map an ASFF severity label to a list-pane status dot (mirrors findings.md).
pub fn severity_to_state(label: &str) -> ResourceState {
    match label {
        "CRITICAL" | "HIGH" => ResourceState::Unavailable,
        "MEDIUM" => ResourceState::Pending,
        _ => ResourceState::Unknown(String::new()),
    }
}

// ── raw ASFF JSON (the SDK finding isn't Serialize) ──────────────────────────

fn build_raw_json(f: &AwsSecurityFinding, g: &ShFinding) -> String {
    use serde_json::{json, Map, Value};

    fn kv(rows: &[(String, String)]) -> Value {
        let mut m = Map::new();
        for (k, v) in rows {
            m.insert(k.trim().to_string(), Value::String(v.clone()));
        }
        Value::Object(m)
    }

    let res: Vec<Value> = g
        .resources
        .iter()
        .map(|(t, id)| json!({ "Type": t, "Id": id }))
        .collect();
    let v = json!({
        "Id": g.id,
        "ProductArn": g.product_arn,
        "Title": g.title,
        "Description": g.description,
        "ProductName": g.product,
        "CompanyName": g.company,
        "GeneratorId": g.generator_id,
        "Types": g.types,
        "Severity": {
            "Label": g.severity_label,
            "Normalized": g.severity_normalized,
            "Original": g.severity_original,
        },
        "Criticality": g.criticality,
        "Confidence": g.confidence,
        "WorkflowStatus": g.workflow_status,
        "RecordState": g.record_state,
        "AwsAccountId": g.account_id,
        "AwsAccountName": g.account_name,
        "Region": g.region,
        "Sample": g.sample,
        "SourceUrl": g.source_url,
        "FirstObservedAt": g.first_observed,
        "LastObservedAt": g.last_observed,
        "CreatedAt": g.created,
        "UpdatedAt": g.updated,
        "ProcessedAt": g.processed_at,
        "Resources": res,
        "ResourceDetails": kv(&g.resource_rows),
        "Compliance": {
            "Status": g.compliance_status,
            "SecurityControlId": g.security_control_id,
            "RelatedRequirements": g.related_requirements,
            "StatusReasons": g.status_reasons.iter()
                .map(|(c, d)| json!({ "ReasonCode": c, "Description": d }))
                .collect::<Vec<_>>(),
            "AssociatedStandards": g.associated_standards,
            "SecurityControlParameters": kv(&g.control_parameters),
        },
        "Vulnerabilities": kv(&g.vuln_rows),
        "Context": kv(&g.context_rows),
        "Note": { "Text": g.note_text, "UpdatedBy": g.note_updated_by, "UpdatedAt": g.note_updated_at },
        "ProductFields": kv(&g.product_fields),
        "UserDefinedFields": kv(&g.user_defined_fields),
        "RelatedFindings": g.related_findings,
        "Remediation": { "Text": g.remediation_text, "Url": g.remediation_url },
        "SchemaVersion": f.schema_version().unwrap_or_default(),
    });
    serde_json::to_string_pretty(&v).unwrap_or_else(|_| "{}".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn control(compliance: &str, standards: Vec<&str>) -> ShControl {
        ShControl {
            id: "S3.1".to_string(),
            arn: String::new(),
            title: "Block public access".to_string(),
            description: String::new(),
            remediation_url: String::new(),
            severity: "HIGH".to_string(),
            control_status: "ENABLED".to_string(),
            update_status: String::new(),
            last_update_reason: String::new(),
            region_availability: "AVAILABLE".to_string(),
            parameters: Vec::new(),
            customizable: Vec::new(),
            standards: standards.into_iter().map(|s| s.to_string()).collect(),
            failed_resources: if compliance == FAILED { 3 } else { 0 },
            compliance: compliance.to_string(),
            tags: HashMap::new(),
        }
    }

    #[test]
    fn decomposes_a_full_finding_type() {
        let rows = decompose_finding_type(
            "Software and Configuration Checks/Industry and Regulatory Standards/CIS AWS Foundations Benchmark",
        );
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].1, "Software and Configuration Checks");
        assert_eq!(rows[2].0.trim(), "Classifier");
    }

    #[test]
    fn a_namespace_only_type_yields_one_row() {
        let rows = decompose_finding_type("Effects");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0.trim(), "Namespace");
    }

    #[test]
    fn an_empty_type_yields_no_rows() {
        assert!(decompose_finding_type("").is_empty());
    }

    #[test]
    fn severity_maps_to_the_list_dot() {
        assert!(matches!(
            severity_to_state("CRITICAL"),
            ResourceState::Unavailable
        ));
        assert!(matches!(severity_to_state("MEDIUM"), ResourceState::Pending));
    }

    #[test]
    fn a_failing_control_is_the_only_thing_left_after_a() {
        assert!(!control(FAILED, vec!["fsbp"]).is_noise());
        assert!(control(PASSED, vec!["fsbp"]).is_noise());
        assert!(control(DISABLED, vec!["fsbp"]).is_noise());
        assert!(control(NO_DATA, vec![]).is_noise());
    }

    #[test]
    fn a_truncated_scan_never_claims_a_pass() {
        let def = SecurityControlDefinition::builder()
            .security_control_id("S3.1")
            .build();
        let complete = ShControl::build(&def, None, vec!["fsbp".to_string()], 0, true);
        assert_eq!(complete.compliance, PASSED);
        let truncated = ShControl::build(&def, None, vec!["fsbp".to_string()], 0, false);
        assert_eq!(truncated.compliance, NO_DATA);
    }

    #[test]
    fn a_control_in_no_enabled_standard_is_unevaluated_not_passing() {
        let def = SecurityControlDefinition::builder()
            .security_control_id("S3.1")
            .build();
        let orphan = ShControl::build(&def, None, Vec::new(), 0, true);
        assert_eq!(orphan.compliance, NO_DATA);
    }

    #[test]
    fn standard_scores_come_from_control_membership() {
        let mut standards = vec![ShStandard {
            name: "fsbp".to_string(),
            arn: String::new(),
            status: "READY".to_string(),
            controls_enabled: 99,
            controls_disabled: 0,
            controls_passed: 0,
            controls_failed: 0,
            score_estimated: false,
            tags: HashMap::new(),
        }];
        let controls = vec![
            control(FAILED, vec!["fsbp"]),
            control(PASSED, vec!["fsbp"]),
            control(PASSED, vec!["fsbp"]),
            // Disabled controls leave the denominator, as in the console.
            control(DISABLED, vec!["fsbp"]),
            // A control in another standard must not move this score.
            control(FAILED, vec!["cis"]),
        ];
        score_standards(&mut standards, &controls, true);
        assert_eq!(standards[0].controls_enabled, 3);
        assert_eq!(standards[0].controls_failed, 1);
        assert_eq!(standards[0].controls_passed, 2);
        assert_eq!(standards[0].score_pct(), 67);
        assert!(!standards[0].score_estimated);
    }

    #[test]
    fn a_truncated_scan_marks_every_standard_score_as_a_floor() {
        let mut standards = vec![ShStandard {
            name: "fsbp".to_string(),
            arn: String::new(),
            status: "READY".to_string(),
            controls_enabled: 0,
            controls_disabled: 0,
            controls_passed: 0,
            controls_failed: 0,
            score_estimated: false,
            tags: HashMap::new(),
        }];
        score_standards(&mut standards, &[control(FAILED, vec!["fsbp"])], false);
        assert!(standards[0].score_estimated);
    }

    #[test]
    fn failed_controls_sort_ahead_of_everything_else() {
        assert!(compliance_rank(FAILED) < compliance_rank(NO_DATA));
        assert!(compliance_rank(NO_DATA) < compliance_rank(PASSED));
        assert!(severity_rank("CRITICAL") > severity_rank("LOW"));
    }

    #[test]
    fn parameter_values_flatten_across_the_union() {
        assert_eq!(
            parameter_value_text(Some(&ParameterValue::Integer(90))),
            "90"
        );
        assert_eq!(
            parameter_value_text(Some(&ParameterValue::StringList(vec![
                "a".to_string(),
                "b".to_string()
            ]))),
            "a, b"
        );
        assert_eq!(parameter_value_text(None), "");
    }

    fn rule(status: &str, sets_workflow: &str) -> ShAutomationRule {
        ShAutomationRule {
            arn: "arn:aws:securityhub:us-east-1:1:automation-rule/x".to_string(),
            name: "r".to_string(),
            description: String::new(),
            status: status.to_string(),
            order: 1,
            is_terminal: false,
            created_at: String::new(),
            updated_at: String::new(),
            created_by: String::new(),
            criteria_rows: Vec::new(),
            action_rows: Vec::new(),
            sets_workflow: sets_workflow.to_string(),
            tags: HashMap::new(),
        }
    }

    #[test]
    fn only_a_live_suppression_rule_reads_as_a_warning() {
        assert!(rule("ENABLED", "SUPPRESSED").suppresses());
        // A disabled rule isn't suppressing anything, whatever it says.
        assert!(!rule(DISABLED, "SUPPRESSED").suppresses());
        assert!(!rule("ENABLED", "NOTIFIED").suppresses());
        assert!(rule(DISABLED, "SUPPRESSED").is_noise());
        assert!(!rule("ENABLED", "SUPPRESSED").is_noise());
    }

    #[test]
    fn product_and_subscription_arns_join_on_the_same_key() {
        assert_eq!(
            product_key("arn:aws:securityhub:us-east-1::product/aws/guardduty"),
            "aws/guardduty"
        );
        assert_eq!(
            product_key("arn:aws:securityhub:us-east-1:123:product-subscription/aws/guardduty"),
            "aws/guardduty"
        );
        // An unrecognised shape falls through rather than collapsing to "".
        assert_eq!(product_key("not-an-arn"), "not-an-arn");
    }

    #[test]
    fn an_unsubscribed_integration_is_noise() {
        let mut p = ShProduct {
            arn: "arn:aws:securityhub:us-east-1::product/aws/guardduty".to_string(),
            name: "GuardDuty".to_string(),
            company: "AWS".to_string(),
            description: String::new(),
            categories: Vec::new(),
            integration_types: Vec::new(),
            marketplace_url: String::new(),
            activation_url: String::new(),
            enabled: true,
            tags: HashMap::new(),
        };
        assert!(!p.is_noise());
        p.enabled = false;
        assert!(p.is_noise());
    }

    #[test]
    fn a_number_filter_range_keeps_both_bounds() {
        use aws_sdk_securityhub::types::NumberFilter;
        let mut rows = Vec::new();
        // A range is gte + lte on one filter — first-match-wins would drop
        // half of it and make the rule look broader than it is.
        push_number_filters(
            &mut rows,
            "Criticality",
            &[NumberFilter::builder().gte(50.0).lte(90.0).build()],
        );
        assert_eq!(rows.len(), 1);
        assert!(rows[0].1.contains("50"));
        assert!(rows[0].1.contains("90"));
    }

    #[test]
    fn a_relative_date_filter_reads_as_a_window() {
        use aws_sdk_securityhub::types::{DateFilter, DateRange, DateRangeUnit};
        let mut rows = Vec::new();
        push_date_filters(
            &mut rows,
            "Updated",
            &[DateFilter::builder()
                .date_range(
                    DateRange::builder()
                        .value(7)
                        .unit(DateRangeUnit::Days)
                        .build(),
                )
                .build()],
        );
        assert_eq!(rows[0].1, "last 7 DAYS");
    }

    #[test]
    fn managed_insights_sort_the_way_the_console_numbers_them() {
        let arn = |n: u32| format!("arn:aws:securityhub:::insight/securityhub/default/{}", n);
        assert_eq!(managed_index(&arn(1)), 1);
        assert_eq!(managed_index(&arn(14)), 14);
        // A custom insight's ARN tail isn't a number, so it sorts last.
        assert_eq!(
            managed_index("arn:aws:securityhub:us-east-1:1:insight/1/custom/abc"),
            u32::MAX
        );
    }

    #[test]
    fn managed_insight_arns_follow_the_region_partition() {
        assert_eq!(partition_for("eu-west-1"), "aws");
        assert_eq!(partition_for("us-gov-west-1"), "aws-us-gov");
        assert_eq!(partition_for("cn-north-1"), "aws-cn");
    }

    #[test]
    fn no_insight_is_ever_noise_and_origin_is_searchable() {
        let insight = |arn: &str| ShInsight {
            name: "i".to_string(),
            managed: arn.contains(":::insight/"),
            arn: arn.to_string(),
            group_by: "ResourceId".to_string(),
            filter_rows: Vec::new(),
            tags: HashMap::new(),
        };
        // AWS-managed insights carry an empty account field in the ARN.
        let managed = insight("arn:aws:securityhub:::insight/securityhub/default/1");
        let custom =
            insight("arn:aws:securityhub:us-east-1:123456789012:insight/123456789012/custom/abc");
        assert!(managed.managed);
        assert!(!custom.managed);
        // Most accounts have no custom insights, so folding the managed ones
        // away under `a` would empty the tab — origin is a search term, not a
        // filter that can hide everything.
        assert!(!managed.is_noise());
        assert!(!custom.is_noise());
        assert!(managed.search_text().contains("managed"));
        assert!(custom.search_text().contains("custom"));
    }

    #[test]
    fn member_health_drives_the_dot_and_the_sort() {
        let member = |status: &str| ShMember {
            account_id: "1".to_string(),
            email: String::new(),
            status: status.to_string(),
            administrator_id: String::new(),
            invited_at: String::new(),
            updated_at: String::new(),
            tags: HashMap::new(),
        };
        assert!(member("Enabled").is_noise());
        assert!(member("Associated").is_noise());
        // An account that stopped reporting is the whole point of the tab.
        assert!(!member("Removed").is_noise());
        assert!(!member("Invited").is_noise());
        assert!(member_rank("Removed") < member_rank("Invited"));
        assert!(member_rank("Invited") < member_rank("Enabled"));
        assert!(matches!(
            member("Removed").state(),
            ResourceState::Unavailable
        ));
        assert!(matches!(member("Invited").state(), ResourceState::Pending));
    }

    fn policy(targets: Vec<&str>, statuses: Vec<&str>) -> ShConfigPolicy {
        ShConfigPolicy {
            id: "p".to_string(),
            arn: String::new(),
            name: "Baseline".to_string(),
            description: String::new(),
            created_at: String::new(),
            updated_at: String::new(),
            service_enabled: true,
            enabled_standards: Vec::new(),
            enabled_controls: Vec::new(),
            disabled_controls: Vec::new(),
            custom_parameters: Vec::new(),
            targets: targets
                .into_iter()
                .zip(statuses)
                .map(|(id, status)| ShPolicyTarget {
                    target_id: id.to_string(),
                    target_type: "ORGANIZATIONAL_UNIT".to_string(),
                    association_type: "APPLIED".to_string(),
                    status: status.to_string(),
                    status_message: String::new(),
                    updated_at: String::new(),
                })
                .collect(),
            detail_error: String::new(),
            tags: HashMap::new(),
        }
    }

    #[test]
    fn an_unassociated_policy_configures_nothing() {
        let orphan = policy(vec![], vec![]);
        assert!(orphan.is_noise());
        assert!(matches!(orphan.state(), ResourceState::Unknown(_)));

        let applied = policy(vec!["ou-1"], vec!["SUCCESS"]);
        assert!(!applied.is_noise());
        assert!(matches!(applied.state(), ResourceState::Available));
    }

    #[test]
    fn a_failed_target_makes_the_policy_read_red() {
        let broken = policy(vec!["ou-1", "ou-2"], vec!["SUCCESS", "FAILED"]);
        assert_eq!(broken.failed_targets(), 1);
        assert!(matches!(broken.state(), ResourceState::Unavailable));
    }

    #[test]
    fn a_policy_that_turns_security_hub_off_reads_as_stopped() {
        let mut off = policy(vec!["ou-1"], vec!["SUCCESS"]);
        off.service_enabled = false;
        assert!(matches!(off.state(), ResourceState::Stopped));
    }

    #[test]
    fn settings_errors_reach_only_their_own_section() {
        let s = ShSettings {
            errors: vec![
                "organization: not the administrator".to_string(),
                "regions: denied".to_string(),
            ],
            ..Default::default()
        };
        assert_eq!(s.section_errors("organization:").len(), 1);
        assert_eq!(s.section_errors("regions:").len(), 1);
        assert!(s.section_errors("hub:").is_empty());
        // Rendered as ⚠ content lines, not key-value rows.
        assert!(s.section_errors("regions:")[0].0.contains('⚠'));
        assert!(s.section_errors("regions:")[0].1.is_empty());
    }

    #[test]
    fn wrap_words_breaks_on_word_boundaries() {
        let lines = wrap_words("the quick brown fox jumps", 10);
        assert!(lines.iter().all(|l| l.chars().count() <= 10));
        assert_eq!(lines.join(" "), "the quick brown fox jumps");
    }

    #[test]
    fn suppressed_findings_load_but_read_as_noise() {
        let mut f = ShFinding {
            id: "f".to_string(),
            product: String::new(),
            product_arn: String::new(),
            company: String::new(),
            generator_id: String::new(),
            title: "t".to_string(),
            description: String::new(),
            severity_label: "HIGH".to_string(),
            severity_normalized: None,
            severity_original: String::new(),
            criticality: None,
            confidence: None,
            workflow_status: "NEW".to_string(),
            record_state: "ACTIVE".to_string(),
            compliance_status: FAILED.to_string(),
            security_control_id: String::new(),
            related_requirements: Vec::new(),
            status_reasons: Vec::new(),
            associated_standards: Vec::new(),
            control_parameters: Vec::new(),
            types: Vec::new(),
            account_id: String::new(),
            account_name: String::new(),
            region: String::new(),
            sample: false,
            source_url: String::new(),
            resources: Vec::new(),
            resource_rows: Vec::new(),
            vuln_rows: Vec::new(),
            context_rows: Vec::new(),
            note_text: String::new(),
            note_updated_by: String::new(),
            note_updated_at: String::new(),
            product_fields: Vec::new(),
            user_defined_fields: Vec::new(),
            related_findings: Vec::new(),
            remediation_text: String::new(),
            remediation_url: String::new(),
            first_observed: None,
            last_observed: None,
            created: None,
            updated: None,
            processed_at: None,
            search_blob: String::new(),
            raw_json: String::new(),
        };
        assert!(!f.is_noise());
        f.workflow_status = "SUPPRESSED".to_string();
        assert!(f.is_noise());
        f.workflow_status = "NEW".to_string();
        f.compliance_status = PASSED.to_string();
        assert!(f.is_noise());
    }

    #[test]
    fn the_search_blob_carries_every_resource_id() {
        let f = ShFinding {
            id: "f".to_string(),
            product: "Security Hub".to_string(),
            product_arn: String::new(),
            company: String::new(),
            generator_id: String::new(),
            title: "Bucket is public".to_string(),
            description: String::new(),
            severity_label: "HIGH".to_string(),
            severity_normalized: None,
            severity_original: String::new(),
            criticality: None,
            confidence: None,
            workflow_status: "NEW".to_string(),
            record_state: "ACTIVE".to_string(),
            compliance_status: FAILED.to_string(),
            security_control_id: "S3.1".to_string(),
            related_requirements: Vec::new(),
            status_reasons: Vec::new(),
            associated_standards: Vec::new(),
            control_parameters: Vec::new(),
            types: Vec::new(),
            account_id: String::new(),
            account_name: String::new(),
            region: String::new(),
            sample: false,
            source_url: String::new(),
            resources: vec![(
                "AwsS3Bucket".to_string(),
                "arn:aws:s3:::my-bucket".to_string(),
            )],
            resource_rows: vec![("  Region".to_string(), "eu-west-1".to_string())],
            vuln_rows: Vec::new(),
            context_rows: Vec::new(),
            note_text: String::new(),
            note_updated_by: String::new(),
            note_updated_at: String::new(),
            product_fields: Vec::new(),
            user_defined_fields: Vec::new(),
            related_findings: Vec::new(),
            remediation_text: String::new(),
            remediation_url: String::new(),
            first_observed: None,
            last_observed: None,
            created: None,
            updated: None,
            processed_at: None,
            search_blob: String::new(),
            raw_json: String::new(),
        };
        let blob = build_search_blob(&f);
        assert!(blob.contains("arn:aws:s3:::my-bucket"));
        assert!(blob.contains("eu-west-1"));
        assert!(blob.contains("S3.1"));
    }
}
