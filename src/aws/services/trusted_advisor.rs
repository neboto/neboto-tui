use crate::aws::client::AwsClients;
use crate::aws::resource::{Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::error::Result;
use crate::event::{Event, LoadProgress};
use async_trait::async_trait;
use aws_sdk_support::Client as SupportClient;
use futures::stream::{self, StreamExt};
use std::any::Any;
use std::collections::HashMap;
use tokio::sync::mpsc;

/// Which population the list shows. The console's "organizational view" has
/// no readable API (it's a console-only report generator), so Organization
/// scope reproduces it client-side: fan out into every active member account
/// with the org access role and roll the check summaries up. Priority scope
/// is different data entirely — the account-team-curated recommendations of
/// Trusted Advisor Priority (Enterprise Support), via the newer
/// `trustedadvisor` API's org operations, which serve *only* that subset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaScope {
    Account,
    Organization,
    Priority,
}

pub struct TrustedAdvisorService {
    client: SupportClient,
    scope: TaScope,
    org_client: aws_sdk_organizations::Client,
    advisor_client: aws_sdk_trustedadvisor::Client,
    /// Base credentials, kept so Organization scope can re-point the Support
    /// API at each member account (the Control Tower audit-account pattern).
    sdk_config: aws_config::SdkConfig,
    /// Candidate member-switch role names (`org_access_roles`), tried in
    /// order per account.
    org_role_names: Vec<String>,
}

impl TrustedAdvisorService {
    pub fn new(aws_clients: &AwsClients, scope: TaScope, org_role_names: Vec<String>) -> Self {
        Self {
            client: aws_clients.support_client(),
            scope,
            org_client: aws_clients.organizations_client(),
            advisor_client: aws_clients.trustedadvisor_client(),
            sdk_config: aws_clients.sdk_config().clone(),
            org_role_names,
        }
    }
}

/// A Support client bound to `config`'s credentials but pinned to us-east-1 —
/// the Support API's only endpoint (matches `AwsClients::support_client`).
fn support_client_for(config: &aws_config::SdkConfig) -> SupportClient {
    let cfg = aws_sdk_support::config::Builder::from(config)
        .region(aws_sdk_support::config::Region::new("us-east-1"))
        .build();
    SupportClient::from_conf(cfg)
}

#[async_trait]
impl AwsService for TrustedAdvisorService {
    fn service_type(&self) -> ServiceType {
        ServiceType::TrustedAdvisor
    }

    fn name(&self) -> &str {
        "Trusted Advisor"
    }

    async fn list_resources(&self) -> Result<Vec<Box<dyn Resource>>> {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        self.list_resources_streaming(tx, ServiceType::TrustedAdvisor)
            .await?;
        Ok(vec![])
    }

    async fn list_resources_streaming(
        &self,
        event_tx: mpsc::UnboundedSender<Event>,
        service_type: ServiceType,
    ) -> Result<()> {
        // Priority scope lists a different resource entirely (curated
        // recommendations, not checks) and never touches the Support API.
        if self.scope == TaScope::Priority {
            return self.load_priority(event_tx, service_type).await;
        }

        // Phase 1 — check metadata (id, name, category, description, column
        // headers). Status/counts are unknown at this point.
        let mut checks: Vec<TaCheck> = match self
            .client
            .describe_trusted_advisor_checks()
            .language("en")
            .send()
            .await
        {
            Ok(resp) => resp.checks().iter().map(TaCheck::from_description).collect(),
            Err(e) => {
                let _ = event_tx.send(Event::ResourceLoadError {
                    service: service_type,
                    error: friendly_error(&crate::error::sdk_error_message(&e)),
                });
                return Ok(());
            }
        };

        match self.scope {
            TaScope::Account => {
                // Phase 2 — one batched summaries call gives the status +
                // resource counts for every check. Merge into the metadata.
                let ids: Vec<String> = checks.iter().map(|c| c.id.clone()).collect();
                match fetch_account_summaries(self.client.clone(), &ids).await {
                    Ok(summaries) => {
                        for c in &mut checks {
                            if let Some(s) = summaries.get(&c.id) {
                                c.status = s.status.clone();
                                c.resources_flagged = s.flagged;
                                c.resources_processed = s.processed;
                                c.resources_suppressed = s.suppressed;
                            }
                        }
                    }
                    Err(e) => {
                        let _ = event_tx.send(Event::ResourceLoadError {
                            service: service_type,
                            error: friendly_error(&e),
                        });
                        return Ok(());
                    }
                }
            }
            TaScope::Organization => {
                if let Err(fatal) = self.aggregate_org(&mut checks, &event_tx, service_type).await
                {
                    let _ = event_tx.send(Event::ResourceLoadError {
                        service: service_type,
                        error: fatal,
                    });
                    return Ok(());
                }
            }
            // Returned to load_priority before the checks phase.
            TaScope::Priority => unreachable!("Priority scope early-returns above"),
        }

        // Sort error → warning → ok → n/a so the list leads with what needs
        // attention; tie-break by name for stability.
        checks.sort_by(|a, b| {
            status_rank(&a.status)
                .cmp(&status_rank(&b.status))
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });

        let total = checks.len();
        if total > 0 {
            let batch: Vec<Box<dyn Resource>> = checks
                .into_iter()
                .map(|c| Box::new(c) as Box<dyn Resource>)
                .collect();
            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                service: service_type,
                resources: batch,
                progress: LoadProgress {
                    loaded_count: total,
                    total_count: Some(total),
                    status_message: None,
                },
            });
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

impl TrustedAdvisorService {
    /// Organization scope: enumerate active org accounts, fetch each account's
    /// check summaries (base creds for the management account, assumed
    /// `org_access_role` for members), and fold them into `checks` — worst
    /// status wins, counts sum, and the per-account breakdown rides the row
    /// for the Accounts section. Per-account failures degrade to load
    /// warnings; only "no account answered at all" is fatal (`Err`).
    async fn aggregate_org(
        &self,
        checks: &mut [TaCheck],
        event_tx: &mpsc::UnboundedSender<Event>,
        service_type: ServiceType,
    ) -> std::result::Result<(), String> {
        // The management account is fanned out with base creds — it can't
        // assume the member role into itself.
        let mgmt_id = self
            .org_client
            .describe_organization()
            .send()
            .await
            .ok()
            .and_then(|r| r.organization().and_then(|o| o.master_account_id().map(String::from)));

        let mut accounts: Vec<(String, String)> = Vec::new();
        let mut pages = self.org_client.list_accounts().into_paginator().send();
        while let Some(page) = pages.next().await {
            match page {
                Ok(p) => {
                    for a in p.accounts() {
                        if a.status() == Some(&aws_sdk_organizations::types::AccountStatus::Active)
                        {
                            accounts.push((
                                a.id().unwrap_or_default().to_string(),
                                a.name().unwrap_or_default().to_string(),
                            ));
                        }
                    }
                }
                Err(e) => {
                    return Err(format!(
                        "Organization scope needs the org management account (organizations:ListAccounts failed: {}). Press t to switch back to account scope.",
                        crate::error::sdk_error_message(&e)
                    ));
                }
            }
        }
        if accounts.is_empty() {
            return Err("the organization has no active accounts".to_string());
        }

        let ids: Vec<String> = checks.iter().map(|c| c.id.clone()).collect();
        let total = accounts.len();
        let mut done = 0usize;
        let mut per_check: HashMap<String, Vec<TaAccountStatus>> = HashMap::new();
        let mut no_plan: Vec<String> = Vec::new();
        let mut failed: Vec<(String, String)> = Vec::new();

        // Concurrency kept low — the Support API has small TPS limits and
        // every account issues chunked summaries calls.
        let mut results = stream::iter(accounts.into_iter().map(|(id, name)| {
            let ids = ids.clone();
            let mgmt_id = mgmt_id.clone();
            async move {
                let summaries = if mgmt_id.as_deref() == Some(id.as_str()) {
                    fetch_account_summaries(self.client.clone(), &ids).await
                } else {
                    self.fetch_member_summaries(&id, &ids).await
                };
                (id, name, summaries)
            }
        }))
        .buffer_unordered(4);

        while let Some((id, name, result)) = results.next().await {
            done += 1;
            // Progress-only tick (empty batch): the row set isn't stable until
            // every account is folded in, but a 50-account fan-out shouldn't
            // look hung.
            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                service: service_type,
                resources: vec![],
                progress: LoadProgress {
                    loaded_count: done,
                    total_count: Some(total),
                    status_message: Some(format!(
                        "aggregating Trusted Advisor — {}/{} accounts",
                        done, total
                    )),
                },
            });
            match result {
                Ok(map) => {
                    for (check_id, s) in map {
                        per_check.entry(check_id).or_default().push(TaAccountStatus {
                            account_id: id.clone(),
                            account_name: name.clone(),
                            status: s.status,
                            flagged: s.flagged,
                            processed: s.processed,
                            suppressed: s.suppressed,
                        });
                    }
                }
                Err(e) if is_subscription_error(&e) => no_plan.push(name.clone()),
                Err(e) => failed.push((name.clone(), e)),
            }
        }

        if no_plan.len() + failed.len() >= total {
            return Err(format!(
                "no account in the organization returned Trusted Advisor data ({})",
                failed
                    .first()
                    .map(|(n, e)| format!("{}: {}", n, e))
                    .unwrap_or_else(|| "no member account has a Business/Enterprise support plan"
                        .to_string())
            ));
        }
        // Every skipped account is named, grouped by cause — the status line
        // truncates visually but `M` holds (and `y` copies) the full text,
        // and "which 20 accounts and why" is exactly what the user needs to
        // fix coverage. Don't sample here.
        if !no_plan.is_empty() {
            let _ = event_tx.send(Event::ResourceLoadWarning {
                service: service_type,
                warning: format!(
                    "{} account(s) have no Business/Enterprise support plan (no Trusted Advisor data exists for them): {}",
                    no_plan.len(),
                    no_plan.join(", ")
                ),
            });
        }
        if !failed.is_empty() {
            let (role_fail, other): (Vec<_>, Vec<_>) =
                failed.iter().partition(|(_, e)| is_assume_error(e));
            if !role_fail.is_empty() {
                let names: Vec<&str> = role_fail.iter().map(|(n, _)| n.as_str()).collect();
                let _ = event_tx.send(Event::ResourceLoadWarning {
                    service: service_type,
                    warning: format!(
                        "{} account(s) unreachable — role hop failed, add their switch role to org_access_roles: {} — first error: {}",
                        role_fail.len(),
                        names.join(", "),
                        role_fail[0].1
                    ),
                });
            }
            if !other.is_empty() {
                let names: Vec<&str> = other.iter().map(|(n, _)| n.as_str()).collect();
                let _ = event_tx.send(Event::ResourceLoadWarning {
                    service: service_type,
                    warning: format!(
                        "{} account(s) failed: {} — first error: {}",
                        other.len(),
                        names.join(", "),
                        other[0].1
                    ),
                });
            }
        }

        for c in checks.iter_mut() {
            let mut accts = per_check.remove(&c.id).unwrap_or_default();
            // Worst first, then most flagged, then name — the order the
            // Accounts section renders in.
            accts.sort_by(|a, b| {
                status_rank(&a.status)
                    .cmp(&status_rank(&b.status))
                    .then_with(|| b.flagged.cmp(&a.flagged))
                    .then_with(|| a.account_name.to_lowercase().cmp(&b.account_name.to_lowercase()))
            });
            c.status = worst_status(&accts);
            c.resources_flagged = accts.iter().map(|a| a.flagged).sum();
            c.resources_processed = accts.iter().map(|a| a.processed).sum();
            c.resources_suppressed = accts.iter().map(|a| a.suppressed).sum();
            c.org_scope = true;
            c.org_accounts = accts;
        }
        Ok(())
    }

    /// Fetch one member account's summaries via the org access role(s), tried
    /// in order. A missing-support-plan error is an account property, so it
    /// short-circuits — another role can't fix it.
    async fn fetch_member_summaries(
        &self,
        account_id: &str,
        check_ids: &[String],
    ) -> std::result::Result<HashMap<String, TaSummary>, String> {
        let mut last_err = String::from("no org access role configured");
        for role in &self.org_role_names {
            let cfg =
                AwsClients::assume_config_for_account(&self.sdk_config, account_id, role).await;
            match fetch_account_summaries(support_client_for(&cfg), check_ids).await {
                Ok(map) => return Ok(map),
                Err(e) => {
                    let subscription = is_subscription_error(&e);
                    last_err = e;
                    if subscription {
                        return Err(last_err);
                    }
                }
            }
        }
        Err(last_err)
    }
}

impl TrustedAdvisorService {
    /// Priority scope: Trusted Advisor Priority's org recommendations
    /// (Enterprise Support, management account or delegated admin). One
    /// paginated `ListOrganizationRecommendations` — active and closed both,
    /// so the closed history is browsable via the `F` state filter.
    async fn load_priority(
        &self,
        event_tx: mpsc::UnboundedSender<Event>,
        service_type: ServiceType,
    ) -> Result<()> {
        let mut recs: Vec<TaRecommendation> = Vec::new();
        let mut pages = self
            .advisor_client
            .list_organization_recommendations()
            .into_paginator()
            .send();
        while let Some(page) = pages.next().await {
            match page {
                Ok(p) => {
                    for s in p.organization_recommendation_summaries() {
                        recs.push(TaRecommendation::from_summary(s));
                    }
                }
                Err(e) => {
                    let _ = event_tx.send(Event::ResourceLoadError {
                        service: service_type,
                        error: priority_friendly_error(&crate::error::sdk_error_message(&e)),
                    });
                    return Ok(());
                }
            }
        }

        // Enrich each recommendation with its affected accounts, so `/`
        // search filters the list per account — the list API has no account
        // filter, so this is the same client-side join the console's account
        // selector does. Priority lists are curated and small; one accounts
        // call per recommendation. All best-effort: account names come from
        // `ListAccounts` (a delegated admin without Organizations access
        // gets bare ids), and a failed accounts call leaves that row
        // un-enriched rather than warning.
        let names: HashMap<String, String> = {
            let mut map = HashMap::new();
            let mut pages = self.org_client.list_accounts().into_paginator().send();
            while let Some(page) = pages.next().await {
                match page {
                    Ok(p) => {
                        for a in p.accounts() {
                            if let (Some(id), Some(name)) = (a.id(), a.name()) {
                                map.insert(id.to_string(), name.to_string());
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
            map
        };
        let names = &names;
        let mut recs: Vec<TaRecommendation> = stream::iter(recs.into_iter().map(|mut r| {
            let client = self.advisor_client.clone();
            async move {
                if let Ok(accts) = fetch_ta_rec_accounts(client, r.arn.clone()).await {
                    r.affected_accounts = accts
                        .iter()
                        .map(|a| match names.get(&a.account_id) {
                            Some(n) => format!("{} ({})", n, a.account_id),
                            None => a.account_id.clone(),
                        })
                        .collect();
                }
                r
            }
        }))
        .buffer_unordered(4)
        .collect()
        .await;

        // Active first (the console's default tab), then worst status, then
        // most recently updated.
        recs.sort_by(|a, b| {
            b.active()
                .cmp(&a.active())
                .then_with(|| status_rank(&a.status).cmp(&status_rank(&b.status)))
                .then_with(|| b.last_updated.cmp(&a.last_updated))
        });

        let total = recs.len();
        if total > 0 {
            let batch: Vec<Box<dyn Resource>> = recs
                .into_iter()
                .map(|r| Box::new(r) as Box<dyn Resource>)
                .collect();
            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                service: service_type,
                resources: batch,
                progress: LoadProgress {
                    loaded_count: total,
                    total_count: Some(total),
                    status_message: None,
                },
            });
        }
        let _ = event_tx.send(Event::ResourcesFullyLoaded {
            service: service_type,
            total_count: total,
        });
        Ok(())
    }
}

/// Map a Priority-API failure to the enablement story: the API answers only
/// on Enterprise Support with Trusted Advisor Priority enabled, from the org
/// management account or a delegated administrator.
fn priority_friendly_error(raw: &str) -> String {
    let low = raw.to_lowercase();
    if low.contains("accessdenied") || low.contains("access denied") || low.contains("not authorized")
    {
        "Trusted Advisor Priority needs Enterprise Support with Priority enabled, viewed from the org management account or a delegated admin (trustedadvisor:ListOrganizationRecommendations). Press t to cycle scope.".to_string()
    } else {
        format!("Failed to load Priority recommendations: {}", raw)
    }
}

/// One account's status + counts for every check, chunked at the API's
/// 100-id limit. Shared by both scopes.
async fn fetch_account_summaries(
    client: SupportClient,
    check_ids: &[String],
) -> std::result::Result<HashMap<String, TaSummary>, String> {
    let mut out = HashMap::new();
    for chunk in check_ids.chunks(100) {
        let arg: Vec<Option<String>> = chunk.iter().map(|s| Some(s.clone())).collect();
        let resp = client
            .describe_trusted_advisor_check_summaries()
            .set_check_ids(Some(arg))
            .send()
            .await
            .map_err(|e| crate::error::sdk_error_message(&e))?;
        for s in resp.summaries() {
            let counts = s.resources_summary();
            out.insert(
                s.check_id().to_string(),
                TaSummary {
                    status: s.status().to_string(),
                    flagged: counts.map(|c| c.resources_flagged()).unwrap_or(0),
                    processed: counts.map(|c| c.resources_processed()).unwrap_or(0),
                    suppressed: counts.map(|c| c.resources_suppressed()).unwrap_or(0),
                },
            );
        }
    }
    Ok(out)
}

fn is_subscription_error(msg: &str) -> bool {
    let low = msg.to_lowercase();
    low.contains("subscriptionrequired") || low.contains("subscription required")
}

/// Did the failure happen on the role hop (fixable via `org_access_roles`)
/// rather than in the target account's Support API?
fn is_assume_error(msg: &str) -> bool {
    let low = msg.to_lowercase();
    low.contains("assumerole")
        || low.contains("assume role")
        || low.contains("sts:")
        || low.contains("security token")
}

/// Worst status across accounts (error < warning < ok < not_available by
/// `status_rank`). Empty input → empty status (renders as "—").
fn worst_status(accts: &[TaAccountStatus]) -> String {
    accts
        .iter()
        .min_by_key(|a| status_rank(&a.status))
        .map(|a| a.status.clone())
        .unwrap_or_default()
}


/// Map the raw Support SDK error to an actionable hint — the dominant case is
/// the account lacking a paid support plan (`SubscriptionRequiredException`).
fn friendly_error(raw: &str) -> String {
    let low = raw.to_lowercase();
    if low.contains("subscriptionrequired") || low.contains("subscription required") {
        "Trusted Advisor needs a Business or Enterprise Support plan. Upgrade in the Support console to use this view.".to_string()
    } else if low.contains("accessdenied")
        || low.contains("access denied")
        || low.contains("not authorized")
    {
        "Access denied — need support:DescribeTrustedAdvisorChecks and related permissions.".to_string()
    } else {
        format!("Failed to load Trusted Advisor checks: {}", raw)
    }
}

/// error → 0, warning → 1, ok → 2, everything else (not_available) → 3.
fn status_rank(status: &str) -> u8 {
    match status {
        "error" => 0,
        "warning" => 1,
        "ok" => 2,
        _ => 3,
    }
}

/// Human-readable category/pillar label (both TA APIs use snake_case keys).
pub fn category_label(category: &str) -> String {
    match category {
        "cost_optimizing" => "Cost Optimization".to_string(),
        "security" => "Security".to_string(),
        "fault_tolerance" => "Fault Tolerance".to_string(),
        "performance" => "Performance".to_string(),
        "service_limits" => "Service Limits".to_string(),
        "operational_excellence" => "Operational Excellence".to_string(),
        other => other.replace('_', " "),
    }
}

struct TaSummary {
    status: String,
    flagged: i64,
    processed: i64,
    suppressed: i64,
}

// ── TaCheck ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct TaCheck {
    pub id: String,
    pub name: String,
    pub category: String,
    pub description: String,
    pub status: String, // ok / warning / error / not_available
    pub resources_flagged: i64,
    pub resources_processed: i64,
    pub resources_suppressed: i64,
    /// Column headers for the flagged-resources table (from the check metadata).
    pub metadata_cols: Vec<String>,
    pub tags: HashMap<String, String>,
    /// True when this row aggregates the whole organization (status = worst
    /// across accounts, counts = sums; reduced Summary/Accounts pane).
    pub org_scope: bool,
    /// Organization scope only: per-account breakdown, sorted worst-first.
    pub org_accounts: Vec<TaAccountStatus>,
}

/// One account's contribution to an org-aggregated check.
#[derive(Debug, Clone)]
pub struct TaAccountStatus {
    pub account_id: String,
    pub account_name: String,
    pub status: String,
    pub flagged: i64,
    pub processed: i64,
    pub suppressed: i64,
}

impl TaCheck {
    fn from_description(d: &aws_sdk_support::types::TrustedAdvisorCheckDescription) -> Self {
        Self {
            id: d.id().to_string(),
            name: d.name().to_string(),
            category: d.category().to_string(),
            // AWS ships these as HTML; flatten to readable plain text (links
            // preserved as "text (URL)") so the detail pane doesn't show markup.
            description: crate::html::html_to_text(d.description()),
            status: String::new(),
            resources_flagged: 0,
            resources_processed: 0,
            resources_suppressed: 0,
            metadata_cols: d
                .metadata()
                .iter()
                .map(|m| m.clone().unwrap_or_default())
                .collect(),
            tags: HashMap::new(),
            org_scope: false,
            org_accounts: Vec::new(),
        }
    }

    pub fn category_label(&self) -> String {
        category_label(&self.category)
    }
}

crate::sections! {
    pub enum TaCheckDetailSection,
    pub static TA_CHECK_SECTIONS = [
        Summary "Summary",
        Resources "Resources" => crate::app::App::trigger_ta_result_load,
    ]
}

// Organization scope: no Resources section — flagged-resource detail is
// per-account (DescribeTrustedAdvisorCheckResult under that account's
// creds), so the Accounts breakdown points at the member-account switch
// instead. No hooks: the breakdown rides the list load.
crate::sections! {
    pub enum TaOrgCheckDetailSection,
    pub static TA_ORG_CHECK_SECTIONS = [
        Summary "Summary",
        Accounts "Accounts",
    ]
}

/// The `F`-chip / list-column word for a Trusted Advisor status — the
/// console's vocabulary (`ta_status_display` in the pane renders the same
/// words), not the coarse `ResourceState` bucket it maps onto.
fn ta_state_label(status: &str) -> String {
    match status {
        "ok" => "ok".to_string(),
        "warning" => "warning".to_string(),
        "error" => "action recommended".to_string(),
        "not_available" => "not available".to_string(),
        "" => "unknown".to_string(),
        other => other.to_lowercase(),
    }
}

impl Resource for TaCheck {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(if self.org_scope {
            &TA_ORG_CHECK_SECTIONS
        } else {
            &TA_CHECK_SECTIONS
        })
    }
    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn resource_type(&self) -> &str {
        "Trusted Advisor Check"
    }

    fn is_noise(&self) -> bool {
        // Checks AWS couldn't evaluate carry no signal — hide by default.
        self.status == "not_available"
    }

    fn state(&self) -> ResourceState {
        // ResourceState has no "warning"; Pending reads as attention without
        // being red. The renderer colours the status row explicitly.
        match self.status.as_str() {
            "ok" => ResourceState::Available,
            "warning" => ResourceState::Pending,
            "error" => ResourceState::Unavailable,
            _ => ResourceState::Unknown(String::new()),
        }
    }

    fn state_label(&self) -> String {
        ta_state_label(&self.status)
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        let status_label = match self.status.as_str() {
            "error" => "error action recommended",
            "warning" => "warning",
            "ok" => "ok",
            "not_available" => "not available",
            other => other,
        };
        let mut text = format!(
            "{} {} {} {} {}",
            self.name,
            self.category_label(),
            self.category,
            status_label,
            self.description,
        );
        // Org scope: affected account names/ids are searchable ("which checks
        // flag prod?"). Only non-ok accounts — every check names every account
        // otherwise, which un-filters the search.
        for a in &self.org_accounts {
            if a.status == "error" || a.status == "warning" {
                text.push(' ');
                text.push_str(&a.account_name);
                text.push(' ');
                text.push_str(&a.account_id);
            }
        }
        text
    }

    fn details(&self) -> Vec<(String, String)> {
        let mut rows = vec![
            ("Name".to_string(), self.name.clone()),
            ("Category".to_string(), self.category_label()),
            ("Status".to_string(), self.status.clone()),
            ("Flagged".to_string(), self.resources_flagged.to_string()),
            ("Processed".to_string(), self.resources_processed.to_string()),
            ("Suppressed".to_string(), self.resources_suppressed.to_string()),
            ("Check ID".to_string(), self.id.clone()),
        ];
        if self.org_scope {
            let affected = self
                .org_accounts
                .iter()
                .filter(|a| a.status == "error" || a.status == "warning")
                .count();
            rows.push(("Accounts".to_string(), self.org_accounts.len().to_string()));
            rows.push(("Accounts Affected".to_string(), affected.to_string()));
        }
        rows
    }

    fn console_url(&self, _region: &str) -> Option<String> {
        Some(if self.org_scope {
            "https://us-east-1.console.aws.amazon.com/trustedadvisor/home#/organizational-view"
                .to_string()
        } else {
            format!(
                "https://us-east-1.console.aws.amazon.com/trustedadvisor/home#/category/{}",
                self.category
            )
        })
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── Flagged resources (lazy) ──────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct TaFlaggedResource {
    pub status: String,
    pub region: String,
    pub is_suppressed: bool,
    /// Values aligned positionally to `TaCheck::metadata_cols`.
    pub metadata: Vec<String>,
}

pub async fn fetch_ta_check_result(
    client: SupportClient,
    check_id: String,
) -> Result<Vec<TaFlaggedResource>> {
    let resp = client
        .describe_trusted_advisor_check_result()
        .check_id(&check_id)
        .language("en")
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;

    let flagged = resp
        .result()
        .map(|r| {
            r.flagged_resources()
                .iter()
                .map(|d| TaFlaggedResource {
                    status: d.status().to_string(),
                    region: d.region().unwrap_or_default().to_string(),
                    is_suppressed: d.is_suppressed(),
                    metadata: d
                        .metadata()
                        .iter()
                        .map(|m| m.clone().unwrap_or_default())
                        .collect(),
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(flagged)
}

// ── Priority recommendations ─────────────────────────────────────────────────

/// A Trusted Advisor Priority org recommendation (curated by the AWS account
/// team / promoted from checks). Only summary fields ride the list; the
/// description, affected accounts and resources are lazy sections.
#[derive(Debug, Clone)]
pub struct TaRecommendation {
    pub id: String,
    pub arn: String,
    pub name: String,
    pub status: String,    // ok / warning / error
    pub lifecycle: String, // pending_response / in_progress / dismissed / resolved
    pub rec_type: String,  // priority / standard
    pub pillars: Vec<String>,
    pub source: String,
    pub aws_services: Vec<String>,
    pub error_count: i64,
    pub warning_count: i64,
    pub ok_count: i64,
    pub created_at: String,
    pub last_updated: String,
    /// Affected accounts as "name (id)" (name best-effort from
    /// `ListAccounts`), enriched at load time so `/` search filters the list
    /// per account — the console's account selector, as a query. Empty when
    /// the per-recommendation accounts call failed (search just won't match).
    pub affected_accounts: Vec<String>,
    pub tags: HashMap<String, String>,
}

impl TaRecommendation {
    fn from_summary(s: &aws_sdk_trustedadvisor::types::OrganizationRecommendationSummary) -> Self {
        let agg = s.resources_aggregates();
        Self {
            id: s.id().to_string(),
            arn: s.arn().to_string(),
            name: s.name().to_string(),
            status: s.status().as_str().to_string(),
            lifecycle: s
                .lifecycle_stage()
                .map(|l| l.as_str().to_string())
                .unwrap_or_default(),
            rec_type: s.r#type().as_str().to_string(),
            pillars: s.pillars().iter().map(|p| p.as_str().to_string()).collect(),
            source: s.source().as_str().to_string(),
            aws_services: s.aws_services().to_vec(),
            error_count: agg.map(|a| a.error_count()).unwrap_or(0),
            warning_count: agg.map(|a| a.warning_count()).unwrap_or(0),
            ok_count: agg.map(|a| a.ok_count()).unwrap_or(0),
            created_at: s.created_at().map(|t| t.to_string()).unwrap_or_default(),
            last_updated: s.last_updated_at().map(|t| t.to_string()).unwrap_or_default(),
            affected_accounts: Vec::new(),
            tags: HashMap::new(),
        }
    }

    /// Not yet resolved or dismissed — the console's "Active" tab.
    pub fn active(&self) -> bool {
        !matches!(self.lifecycle.as_str(), "resolved" | "dismissed")
    }

    pub fn lifecycle_label(&self) -> String {
        match self.lifecycle.as_str() {
            "pending_response" => "Pending response".to_string(),
            "in_progress" => "In progress".to_string(),
            "dismissed" => "Dismissed".to_string(),
            "resolved" => "Resolved".to_string(),
            "" => "—".to_string(),
            other => other.replace('_', " "),
        }
    }
}

crate::sections! {
    pub enum TaRecDetailSection,
    pub static TA_REC_SECTIONS = [
        Overview "Overview" => crate::app::App::trigger_ta_rec_detail_load,
        Accounts "Accounts" => crate::app::App::trigger_ta_rec_accounts_load,
        Resources "Resources" => crate::app::App::trigger_ta_rec_resources_load,
    ]
}

impl Resource for TaRecommendation {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&TA_REC_SECTIONS)
    }

    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn resource_type(&self) -> &str {
        "TA Recommendation"
    }

    fn state(&self) -> ResourceState {
        // Closed recommendations read as Stopped so the `F` state filter can
        // isolate active vs closed (the console's two tabs).
        if !self.active() {
            return ResourceState::Stopped;
        }
        match self.status.as_str() {
            "ok" => ResourceState::Available,
            "warning" => ResourceState::Pending,
            "error" => ResourceState::Unavailable,
            _ => ResourceState::Unknown(String::new()),
        }
    }

    fn state_label(&self) -> String {
        // Same two-level logic as `state()`: the lifecycle word for a closed
        // recommendation, else the check-status word.
        if !self.active() {
            return self.lifecycle.to_lowercase();
        }
        ta_state_label(&self.status)
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!(
            "{} {} {} {} {} {} {} {}",
            self.name,
            self.status,
            self.lifecycle_label(),
            self.pillars.iter().map(|p| category_label(p)).collect::<Vec<_>>().join(" "),
            self.source,
            self.aws_services.join(" "),
            self.rec_type,
            // "name (id)" per affected account — typing either filters the
            // list to recommendations hitting that account.
            self.affected_accounts.join(" "),
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Name".to_string(), self.name.clone()),
            ("Status".to_string(), self.status.clone()),
            ("Lifecycle".to_string(), self.lifecycle_label()),
            ("Type".to_string(), self.rec_type.clone()),
            ("Source".to_string(), self.source.clone()),
            (
                "Pillars".to_string(),
                self.pillars.iter().map(|p| category_label(p)).collect::<Vec<_>>().join(", "),
            ),
            ("Services".to_string(), self.aws_services.join(", ")),
            (
                "Accounts Affected".to_string(),
                self.affected_accounts.len().to_string(),
            ),
            ("Resources Error".to_string(), self.error_count.to_string()),
            ("Resources Warning".to_string(), self.warning_count.to_string()),
            ("Last Updated".to_string(), self.last_updated.clone()),
        ]
    }

    fn console_url(&self, _region: &str) -> Option<String> {
        Some(
            "https://us-east-1.console.aws.amazon.com/trustedadvisor/home#/priority".to_string(),
        )
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Lazy Overview extras — everything `GetOrganizationRecommendation` adds on
/// top of the list summary.
#[derive(Debug, Clone)]
pub struct TaRecDetail {
    /// Markdown-ish free text from the account team; rendered as plain lines.
    pub description: String,
    pub created_by: String,
    pub updated_on_behalf_of: String,
    pub update_reason: String,
    pub update_reason_code: String,
    pub resolved_at: String,
}

pub async fn fetch_ta_rec_detail(
    client: aws_sdk_trustedadvisor::Client,
    arn: String,
) -> Result<TaRecDetail> {
    let resp = client
        .get_organization_recommendation()
        .organization_recommendation_identifier(&arn)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
    let r = resp.organization_recommendation();
    Ok(TaRecDetail {
        description: r.map(|r| r.description().to_string()).unwrap_or_default(),
        created_by: r.and_then(|r| r.created_by()).unwrap_or_default().to_string(),
        updated_on_behalf_of: r
            .and_then(|r| r.updated_on_behalf_of())
            .unwrap_or_default()
            .to_string(),
        update_reason: r.and_then(|r| r.update_reason()).unwrap_or_default().to_string(),
        update_reason_code: r
            .and_then(|r| r.update_reason_code())
            .map(|c| c.as_str().to_string())
            .unwrap_or_default(),
        resolved_at: r
            .and_then(|r| r.resolved_at())
            .map(|t| t.to_string())
            .unwrap_or_default(),
    })
}

/// One affected account's lifecycle for a recommendation.
#[derive(Debug, Clone)]
pub struct TaRecAccount {
    pub account_id: String,
    pub lifecycle: String,
    pub updated_on_behalf_of: String,
    pub update_reason: String,
    pub last_updated: String,
}

pub async fn fetch_ta_rec_accounts(
    client: aws_sdk_trustedadvisor::Client,
    arn: String,
) -> Result<Vec<TaRecAccount>> {
    let mut out = Vec::new();
    let mut pages = client
        .list_organization_recommendation_accounts()
        .organization_recommendation_identifier(&arn)
        .into_paginator()
        .send();
    while let Some(page) = pages.next().await {
        let p = page.map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
        for a in p.account_recommendation_lifecycle_summaries() {
            out.push(TaRecAccount {
                account_id: a.account_id().unwrap_or_default().to_string(),
                lifecycle: a
                    .lifecycle_stage()
                    .map(|l| l.as_str().to_string())
                    .unwrap_or_default(),
                updated_on_behalf_of: a
                    .updated_on_behalf_of()
                    .unwrap_or_default()
                    .to_string(),
                update_reason: a.update_reason().unwrap_or_default().to_string(),
                last_updated: a
                    .last_updated_at()
                    .map(|t| t.to_string())
                    .unwrap_or_default(),
            });
        }
    }
    Ok(out)
}

/// One affected resource row. `metadata` is the API's free-form column map —
/// rendered as-is, like the legacy check's flagged-resource metadata.
#[derive(Debug, Clone)]
pub struct TaRecResource {
    pub aws_resource_id: String,
    pub account_id: String,
    pub region: String,
    pub status: String,
    pub excluded: bool,
    pub metadata: Vec<(String, String)>,
}

pub const TA_REC_RESOURCES_CAP: usize = 200;

/// Affected resources across the org, capped (a big org's recommendation can
/// span thousands; the section says so when truncated). Returns
/// `(rows, truncated)`.
pub async fn fetch_ta_rec_resources(
    client: aws_sdk_trustedadvisor::Client,
    arn: String,
) -> Result<(Vec<TaRecResource>, bool)> {
    let mut out = Vec::new();
    let mut pages = client
        .list_organization_recommendation_resources()
        .organization_recommendation_identifier(&arn)
        .into_paginator()
        .send();
    while let Some(page) = pages.next().await {
        let p = page.map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
        for r in p.organization_recommendation_resource_summaries() {
            let mut metadata: Vec<(String, String)> = r
                .metadata()
                .iter()
                .filter(|(_, v)| !v.is_empty())
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            metadata.sort();
            out.push(TaRecResource {
                aws_resource_id: r.aws_resource_id().to_string(),
                account_id: r.account_id().unwrap_or_default().to_string(),
                region: r.region_code().to_string(),
                status: r.status().as_str().to_string(),
                excluded: matches!(
                    r.exclusion_status(),
                    aws_sdk_trustedadvisor::types::ExclusionStatus::Excluded
                ),
                metadata,
            });
            if out.len() >= TA_REC_RESOURCES_CAP {
                return Ok((out, true));
            }
        }
    }
    Ok((out, false))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_rank_orders_error_first() {
        assert!(status_rank("error") < status_rank("warning"));
        assert!(status_rank("warning") < status_rank("ok"));
        assert!(status_rank("ok") < status_rank("not_available"));
        // Unknown / empty statuses sort last with not_available.
        assert_eq!(status_rank("not_available"), status_rank(""));
    }

    #[test]
    fn state_label_uses_the_console_words_not_the_bucket() {
        // `state()` buckets error→Unavailable / ok→Available for colour and
        // sort; the `F` chip must read the way the pane does.
        assert_eq!(ta_state_label("error"), "action recommended");
        assert_eq!(ta_state_label("warning"), "warning");
        assert_eq!(ta_state_label("ok"), "ok");
        assert_eq!(ta_state_label("not_available"), "not available");
        assert_eq!(ta_state_label(""), "unknown");
    }

    #[test]
    fn checks_sort_error_warning_ok() {
        let mk = |name: &str, status: &str| TaCheck {
            id: name.to_string(),
            name: name.to_string(),
            category: "security".to_string(),
            description: String::new(),
            status: status.to_string(),
            resources_flagged: 0,
            resources_processed: 0,
            resources_suppressed: 0,
            metadata_cols: vec![],
            tags: HashMap::new(),
            org_scope: false,
            org_accounts: vec![],
        };
        let mut checks = vec![mk("c-ok", "ok"), mk("a-err", "error"), mk("b-warn", "warning")];
        checks.sort_by(|a, b| {
            status_rank(&a.status)
                .cmp(&status_rank(&b.status))
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        let order: Vec<&str> = checks.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(order, vec!["a-err", "b-warn", "c-ok"]);
    }

    #[test]
    fn category_label_humanizes_keys() {
        assert_eq!(category_label("cost_optimizing"), "Cost Optimization");
        assert_eq!(category_label("fault_tolerance"), "Fault Tolerance");
        assert_eq!(category_label("service_limits"), "Service Limits");
        // Unknown keys just de-underscore.
        assert_eq!(category_label("some_new_thing"), "some new thing");
    }

    #[test]
    fn friendly_error_flags_missing_support_plan() {
        let msg = friendly_error("SubscriptionRequiredException: ...");
        assert!(msg.contains("Business or Enterprise Support"));
        let denied = friendly_error("AccessDeniedException");
        assert!(denied.contains("Access denied"));
    }

    #[test]
    fn worst_status_picks_most_severe_across_accounts() {
        let mk = |status: &str, flagged: i64| TaAccountStatus {
            account_id: "111111111111".to_string(),
            account_name: "acct".to_string(),
            status: status.to_string(),
            flagged,
            processed: 0,
            suppressed: 0,
        };
        assert_eq!(worst_status(&[mk("ok", 0), mk("error", 3), mk("warning", 1)]), "error");
        assert_eq!(worst_status(&[mk("ok", 0), mk("not_available", 0)]), "ok");
        // All-n/a stays n/a (feeds is_noise), empty stays empty (renders "—").
        assert_eq!(worst_status(&[mk("not_available", 0)]), "not_available");
        assert_eq!(worst_status(&[]), "");
    }

    #[test]
    fn subscription_errors_detected_case_insensitively() {
        assert!(is_subscription_error("SubscriptionRequiredException: upgrade"));
        assert!(!is_subscription_error("AccessDenied"));
    }

    #[test]
    fn assume_errors_split_from_support_api_errors() {
        assert!(is_assume_error(
            "User: arn:aws:iam::111111111111:user/x is not authorized to perform: sts:AssumeRole on resource: arn:aws:iam::222222222222:role/OrganizationAccountAccessRole"
        ));
        assert!(is_assume_error("The security token included in the request is invalid"));
        // A Support-API denial inside the member account is NOT a role-hop
        // failure — it must land in the "other" bucket.
        assert!(!is_assume_error(
            "AccessDeniedException: not authorized to perform: support:DescribeTrustedAdvisorCheckSummaries"
        ));
    }
}
