use crate::aws::client::AwsClients;
use crate::aws::resource::{native_state_label, Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::error::Result;
use crate::event::{Event, LoadProgress};
use async_trait::async_trait;
use aws_sdk_fms::Client as FmsClient;
use futures::stream::{self, StreamExt};
use std::any::Any;
use std::collections::HashMap;
use tokio::sync::mpsc;

/// Firewall Manager (`@fms`) — org-level security-policy manager, four
/// sub-tabs (Policies / Apps Lists / Protocols Lists / Resource Sets), one
/// `ServiceType`, browse-only. Each type streams as its own error-tolerant
/// batch.
///
/// **Admin gotcha**: nearly every FMS API only works from the Firewall
/// Manager administrator (or delegated admin) account — anywhere else the
/// calls throw `AccessDeniedException`/`InvalidOperationException`. Phase 0
/// probes posture via `GetAdminAccount` (delivered on
/// [`Event::FmsAdminLoaded`], folded into every policy's Overview and the
/// non-admin empty state); the load itself warns per phase and only errors
/// fatally when nothing at all streamed.
///
/// - **Policies** — `ListPolicies` + a per-policy `GetPolicy` (scope maps +
///   the `managed_service_data` JSON, pre-flattened into `config_rows`) and
///   `ListComplianceStatus` (the per-account compliance rollup — feeds
///   `state()`, so a policy with violations reads red like an AWS Config
///   rule). Split pane Overview / Scope / Config / **Compliance** (account
///   matrix + lazy `GetComplianceDetail` violator drill, capped
///   `MAX_DRILL_ACCOUNTS`) / Tags(lazy `ListTagsForResource`).
/// - **Apps Lists / Protocols Lists** — the `List*` summaries already carry
///   the full app/protocol entries; flat `details()`.
/// - **Resource Sets** — `ListResourceSets` (no fluent paginator —
///   hand-rolled via `next_page_token`). Split pane Overview / Members
///   (lazy `GetResourceSet` + `ListResourceSetResources`).
pub struct FmsService {
    client: FmsClient,
}

/// AWS caps `MaxResults` at 100 on the FMS `List*` calls (and the apps /
/// protocols lists require it).
const PAGE: i32 = 100;

/// The Compliance section drills violator details (`GetComplianceDetail`)
/// for at most this many non-compliant accounts per policy — a huge org
/// shouldn't turn one section view into hundreds of calls. The overflow is
/// reported in the pane.
pub const MAX_DRILL_ACCOUNTS: usize = 10;

impl FmsService {
    pub fn new(aws_clients: &AwsClients) -> Self {
        Self {
            client: aws_clients.fms_client(),
        }
    }
}

#[async_trait]
impl AwsService for FmsService {
    fn service_type(&self) -> ServiceType {
        ServiceType::Fms
    }

    fn name(&self) -> &str {
        "Firewall Manager"
    }

    async fn list_resources(&self) -> Result<Vec<Box<dyn Resource>>> {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        self.list_resources_streaming(tx, ServiceType::Fms).await?;
        Ok(vec![])
    }

    async fn list_resources_streaming(
        &self,
        event_tx: mpsc::UnboundedSender<Event>,
        service_type: ServiceType,
    ) -> Result<()> {
        let mut total = 0usize;
        // The first phase failure, kept for the fatal-error path (a non-admin
        // account fails every phase — one clear message beats four warnings).
        let mut first_error: Option<String> = None;

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

        // ── Phase 0: admin posture ────────────────────────────────────────────
        // Best-effort; the error (usually AccessDenied from a member account)
        // is part of the posture, not a load failure.
        let mut admin = match self.client.get_admin_account().send().await {
            Ok(resp) => FmsAdminInfo {
                admin_account: resp.admin_account().map(|s| s.to_string()),
                role_status: resp.role_status().map(|s| s.as_str().to_string()),
                error: None,
                sns_topic: None,
                sns_role: None,
                member_count: None,
            },
            Err(e) => FmsAdminInfo {
                admin_account: None,
                role_status: None,
                error: Some(crate::error::sdk_error_message(&e)),
                sns_topic: None,
                sns_role: None,
                member_count: None,
            },
        };
        if admin.error.is_none() {
            // Both are admin-only context; failures (no channel configured,
            // permission gap) just leave the rows blank.
            if let Ok(resp) = self.client.get_notification_channel().send().await {
                admin.sns_topic = resp.sns_topic_arn().map(|s| s.to_string());
                admin.sns_role = resp.sns_role_name().map(|s| s.to_string());
            }
            let mut count = 0usize;
            let mut ok = true;
            let mut paginator = self
                .client
                .list_member_accounts()
                .max_results(PAGE)
                .into_paginator()
                .send();
            while let Some(page) = paginator.next().await {
                match page {
                    Ok(p) => count += p.member_accounts().len(),
                    Err(_) => {
                        ok = false;
                        break;
                    }
                }
            }
            if ok {
                admin.member_count = Some(count);
            }
        }
        let _ = event_tx.send(Event::FmsAdminLoaded {
            info: admin.clone(),
        });

        // ── Phase 1: Policies (ListPolicies → N+1 GetPolicy) ──────────────────
        let mut policies: Vec<FmsPolicy> = Vec::new();
        let mut paginator = self
            .client
            .list_policies()
            .max_results(PAGE)
            .into_paginator()
            .send();
        while let Some(page) = paginator.next().await {
            match page {
                Ok(p) => {
                    for s in p.policy_list() {
                        policies.push(FmsPolicy::from_summary(s));
                    }
                }
                Err(e) => {
                    let msg = crate::error::sdk_error_message(&e);
                    let _ = event_tx.send(Event::ResourceLoadWarning {
                        service: service_type,
                        warning: format!("FMS policies: {}", msg),
                    });
                    first_error.get_or_insert(msg);
                    break;
                }
            }
        }
        if !policies.is_empty() {
            // Enrich with the full Policy (scope maps + managed service data)
            // and the per-account compliance rollup; either failing keeps the
            // summary row and records why.
            let client = self.client.clone();
            policies = stream::iter(policies)
                .map(|mut p| {
                    let client = client.clone();
                    async move {
                        match client.get_policy().policy_id(&p.id).send().await {
                            Ok(resp) => {
                                if let Some(full) = resp.policy() {
                                    p.enrich(full);
                                }
                                if p.arn.is_none() {
                                    p.arn = resp.policy_arn().map(|s| s.to_string());
                                }
                            }
                            Err(e) => {
                                p.detail_error = Some(crate::error::sdk_error_message(&e));
                            }
                        }
                        match fetch_policy_compliance(&client, &p.id).await {
                            Ok((accounts, issues)) => {
                                p.compliance = accounts;
                                p.issue_infos = issues;
                            }
                            Err(e) => p.compliance_error = Some(e.to_string()),
                        }
                        p
                    }
                })
                .buffer_unordered(8)
                .collect()
                .await;
            policies.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

            total += policies.len();
            let batch: Vec<Box<dyn Resource>> = policies
                .into_iter()
                .map(|p| Box::new(p) as Box<dyn Resource>)
                .collect();
            emit(batch, total, Some("Loading apps lists…"), false);
        }

        // ── Phase 2: Apps Lists ───────────────────────────────────────────────
        let mut apps_lists: Vec<FmsAppsList> = Vec::new();
        let mut paginator = self
            .client
            .list_apps_lists()
            .max_results(PAGE)
            .into_paginator()
            .send();
        while let Some(page) = paginator.next().await {
            match page {
                Ok(p) => {
                    for s in p.apps_lists() {
                        apps_lists.push(FmsAppsList::from_summary(s));
                    }
                }
                Err(e) => {
                    let msg = crate::error::sdk_error_message(&e);
                    let _ = event_tx.send(Event::ResourceLoadWarning {
                        service: service_type,
                        warning: format!("FMS apps lists: {}", msg),
                    });
                    first_error.get_or_insert(msg);
                    break;
                }
            }
        }
        if !apps_lists.is_empty() {
            total += apps_lists.len();
            let batch: Vec<Box<dyn Resource>> = apps_lists
                .into_iter()
                .map(|a| Box::new(a) as Box<dyn Resource>)
                .collect();
            emit(batch, total, Some("Loading protocols lists…"), false);
        }

        // ── Phase 3: Protocols Lists ──────────────────────────────────────────
        let mut protocols_lists: Vec<FmsProtocolsList> = Vec::new();
        let mut paginator = self
            .client
            .list_protocols_lists()
            .max_results(PAGE)
            .into_paginator()
            .send();
        while let Some(page) = paginator.next().await {
            match page {
                Ok(p) => {
                    for s in p.protocols_lists() {
                        protocols_lists.push(FmsProtocolsList::from_summary(s));
                    }
                }
                Err(e) => {
                    let msg = crate::error::sdk_error_message(&e);
                    let _ = event_tx.send(Event::ResourceLoadWarning {
                        service: service_type,
                        warning: format!("FMS protocols lists: {}", msg),
                    });
                    first_error.get_or_insert(msg);
                    break;
                }
            }
        }
        if !protocols_lists.is_empty() {
            total += protocols_lists.len();
            let batch: Vec<Box<dyn Resource>> = protocols_lists
                .into_iter()
                .map(|p| Box::new(p) as Box<dyn Resource>)
                .collect();
            emit(batch, total, Some("Loading resource sets…"), false);
        }

        // ── Phase 4: Resource Sets (no fluent paginator) ──────────────────────
        let mut resource_sets: Vec<FmsResourceSet> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let resp = self
                .client
                .list_resource_sets()
                .max_results(PAGE)
                .set_next_token(token.clone())
                .send()
                .await;
            match resp {
                Ok(p) => {
                    for s in p.resource_sets() {
                        resource_sets.push(FmsResourceSet::from_summary(s));
                    }
                    token = crate::aws::pagination::next_page_token(p.next_token(), &token);
                    if token.is_none() {
                        break;
                    }
                }
                Err(e) => {
                    let msg = crate::error::sdk_error_message(&e);
                    let _ = event_tx.send(Event::ResourceLoadWarning {
                        service: service_type,
                        warning: format!("FMS resource sets: {}", msg),
                    });
                    first_error.get_or_insert(msg);
                    break;
                }
            }
        }
        if !resource_sets.is_empty() {
            total += resource_sets.len();
            let batch: Vec<Box<dyn Resource>> = resource_sets
                .into_iter()
                .map(|r| Box::new(r) as Box<dyn Resource>)
                .collect();
            emit(batch, total, None, true);
        }

        // Everything failed and nothing streamed → fatal with the first (and
        // most informative) error; the empty state adds the admin-posture hint.
        if total == 0 {
            if let Some(msg) = first_error {
                let _ = event_tx.send(Event::ResourceLoadError {
                    service: service_type,
                    error: msg,
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

/// Account posture from the phase-0 `GetAdminAccount` probe. An `error`
/// (usually AccessDenied) means this account isn't the FMS administrator —
/// expected from member accounts, surfaced as a hint rather than a failure.
/// When the probe succeeds, two more best-effort context calls fill in the
/// org-wide delivery channel (`GetNotificationChannel`) and the member-account
/// count (`ListMemberAccounts`).
#[derive(Debug, Clone)]
pub struct FmsAdminInfo {
    pub admin_account: Option<String>,
    pub role_status: Option<String>, // READY / CREATING / …
    pub error: Option<String>,
    /// SNS topic FMS publishes notifications to — `None` when unconfigured.
    pub sns_topic: Option<String>,
    pub sns_role: Option<String>,
    /// Accounts in FMS scope. `None` when the count couldn't be listed.
    pub member_count: Option<usize>,
}

// ── Policies ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct FmsPolicy {
    pub id: String,
    pub arn: Option<String>,
    pub name: String,
    /// Security service kind — WAFV2 / SHIELD_ADVANCED / SECURITY_GROUPS_* /
    /// NETWORK_FIREWALL / DNS_FIREWALL / THIRD_PARTY_FIREWALL / …
    pub service_kind: String,
    pub resource_type: String,
    pub resource_type_list: Vec<String>,
    pub remediation_enabled: bool,
    pub delete_unused: bool,
    pub status: Option<String>, // ACTIVE / OUT_OF_ADMIN_SCOPE
    // GetPolicy enrichment (summary-only rows when the describe failed).
    pub detail_error: Option<String>,
    pub description: Option<String>,
    /// Scope maps flattened to (kind label, ids) — e.g. ("Accounts", […]).
    pub include_scope: Vec<(String, Vec<String>)>,
    pub exclude_scope: Vec<(String, Vec<String>)>,
    /// Resource-tag scoping rendered as `key=value` strings.
    pub resource_tags: Vec<String>,
    pub exclude_resource_tags: bool,
    pub resource_tag_op: Option<String>, // AND / OR
    pub resource_set_ids: Vec<String>,
    /// The raw `managed_service_data` JSON (the per-type policy config) —
    /// opened by `e` on the Config section.
    pub managed_service_data: Option<String>,
    /// `managed_service_data` pre-flattened into detail rows.
    pub config_rows: Vec<(String, String)>,
    /// Per-member-account compliance rollup (`ListComplianceStatus`),
    /// non-compliant first (by violator count). Feeds `state()`.
    pub compliance: Vec<FmsAccountCompliance>,
    pub compliance_error: Option<String>,
    /// Dependent-service problems (e.g. Config recorder off in an account),
    /// deduped across accounts: (service, message).
    pub issue_infos: Vec<(String, String)>,
    pub tags: HashMap<String, String>,
}

impl FmsPolicy {
    pub fn from_summary(s: &aws_sdk_fms::types::PolicySummary) -> Self {
        Self {
            id: s.policy_id().unwrap_or_default().to_string(),
            arn: s.policy_arn().map(|a| a.to_string()),
            name: s.policy_name().unwrap_or_default().to_string(),
            service_kind: s
                .security_service_type()
                .map(|t| t.as_str().to_string())
                .unwrap_or_else(|| "—".to_string()),
            resource_type: s.resource_type().unwrap_or_default().to_string(),
            resource_type_list: Vec::new(),
            remediation_enabled: s.remediation_enabled(),
            delete_unused: s.delete_unused_fm_managed_resources(),
            status: s.policy_status().map(|st| st.as_str().to_string()),
            detail_error: None,
            description: None,
            include_scope: Vec::new(),
            exclude_scope: Vec::new(),
            resource_tags: Vec::new(),
            exclude_resource_tags: false,
            resource_tag_op: None,
            resource_set_ids: Vec::new(),
            managed_service_data: None,
            config_rows: Vec::new(),
            compliance: Vec::new(),
            compliance_error: None,
            issue_infos: Vec::new(),
            tags: HashMap::new(),
        }
    }

    /// (compliant, non-compliant) account counts.
    pub fn compliance_counts(&self) -> (usize, usize) {
        let bad = self.compliance.iter().filter(|c| !c.compliant).count();
        (self.compliance.len() - bad, bad)
    }

    pub fn total_violators(&self) -> i64 {
        self.compliance.iter().map(|c| c.violators).sum()
    }

    /// Fold the full `GetPolicy` document into the summary row.
    pub fn enrich(&mut self, p: &aws_sdk_fms::types::Policy) {
        self.description = p.policy_description().map(|s| s.to_string());
        self.resource_type_list = p.resource_type_list().to_vec();
        self.exclude_resource_tags = p.exclude_resource_tags();
        self.resource_tag_op = p
            .resource_tag_logical_operator()
            .map(|o| o.as_str().to_string());
        self.resource_tags = p
            .resource_tags()
            .iter()
            .map(|t| match t.value() {
                Some(v) if !v.is_empty() => format!("{}={}", t.key(), v),
                _ => t.key().to_string(),
            })
            .collect();
        self.include_scope = flatten_scope_map(p.include_map());
        self.exclude_scope = flatten_scope_map(p.exclude_map());
        self.resource_set_ids = p.resource_set_ids().to_vec();
        if let Some(sspd) = p.security_service_policy_data() {
            self.managed_service_data = sspd.managed_service_data().map(|s| s.to_string());
            if let Some(msd) = &self.managed_service_data {
                self.config_rows = msd_rows(msd);
            }
        }
    }
}

/// Include/exclude scope map → (kind label, ids) rows, accounts first.
fn flatten_scope_map(
    map: Option<
        &HashMap<aws_sdk_fms::types::CustomerPolicyScopeIdType, Vec<String>>,
    >,
) -> Vec<(String, Vec<String>)> {
    let Some(map) = map else {
        return Vec::new();
    };
    let mut out: Vec<(String, Vec<String>)> = map
        .iter()
        .map(|(k, v)| {
            let label = match k.as_str() {
                "ACCOUNT" => "Accounts".to_string(),
                "ORG_UNIT" => "Org units".to_string(),
                other => other.to_string(),
            };
            (label, v.clone())
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Rows a `managed_service_data` policy document flattens to. Generic (works
/// for every security-service type, WAF to network ACLs): top-level scalars
/// are key-value rows, top-level objects group headers, deeper levels
/// indented content lines. Scalar arrays join inline. Capped so a giant
/// policy can't swamp the pane (the full JSON stays on `e`).
pub fn msd_rows(json: &str) -> Vec<(String, String)> {
    const MAX_ROWS: usize = 300;
    let parsed: serde_json::Value = match serde_json::from_str(json) {
        Ok(v) => v,
        // Not JSON (never seen, but the API type is just a string) — show raw.
        Err(_) => return vec![(format!("  {}", json), String::new())],
    };
    let mut rows: Vec<(String, String)> = Vec::new();
    match parsed {
        serde_json::Value::Object(map) => {
            for (k, v) in map {
                push_msd_value(&mut rows, &k, &v, 0);
            }
        }
        other => rows.push((format!("  {}", other), String::new())),
    }
    if rows.len() > MAX_ROWS {
        let dropped = rows.len() - MAX_ROWS;
        rows.truncate(MAX_ROWS);
        rows.push((String::new(), String::new()));
        rows.push((
            format!("  … +{} more lines — press e for the full JSON", dropped),
            String::new(),
        ));
    }
    rows
}

fn msd_scalar(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::Null => Some("null".to_string()),
        serde_json::Value::Bool(b) => Some(b.to_string()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::String(s) => Some(s.clone()),
        _ => None,
    }
}

/// Values the generic ARN/id jump classifier can route (WAF rule groups,
/// Network Firewall references, security groups, buckets…). Nested config
/// fields carrying one render as key-value rows instead of content lines so
/// they pick up the `→` jump indicator.
fn msd_jumpable(v: &str) -> bool {
    v.starts_with("arn:")
        || v.starts_with("sg-")
        || v.starts_with("subnet-")
        || v.starts_with("vpc-")
        || v.starts_with("s3://")
}

fn push_msd_value(rows: &mut Vec<(String, String)>, key: &str, v: &serde_json::Value, depth: usize) {
    let pad = "  ".repeat(depth.saturating_sub(1));
    match v {
        serde_json::Value::Array(items) => {
            let scalars: Vec<String> = items.iter().filter_map(msd_scalar).collect();
            if scalars.len() == items.len() {
                // All-scalar array — join inline (a lone jumpable element
                // keeps key-value form so the classifier sees it).
                let joined = if scalars.is_empty() {
                    "(none)".to_string()
                } else {
                    scalars.join(", ")
                };
                if depth == 0 {
                    rows.push((key.to_string(), joined));
                } else if scalars.len() == 1 && msd_jumpable(&joined) {
                    rows.push((format!("  {}{}", pad, key), joined));
                } else {
                    rows.push((format!("  {}{}: {}", pad, key, joined), String::new()));
                }
            } else {
                if depth == 0 {
                    rows.push((format!("{} ({})", key, items.len()), String::new()));
                } else {
                    rows.push((format!("  {}{} ({})", pad, key, items.len()), String::new()));
                }
                for (i, item) in items.iter().enumerate() {
                    push_msd_value(rows, &format!("[{}]", i), item, depth + 1);
                }
            }
        }
        serde_json::Value::Object(map) => {
            if depth == 0 {
                // Group header (magenta bold via style_detail_row).
                rows.push((key.to_string(), String::new()));
            } else {
                rows.push((format!("  {}{}", pad, key), String::new()));
            }
            for (k, inner) in map {
                push_msd_value(rows, k, inner, depth + 1);
            }
        }
        scalar => {
            let val = msd_scalar(scalar).unwrap_or_default();
            if depth == 0 {
                rows.push((key.to_string(), val));
            } else if msd_jumpable(&val) {
                // Key-value form → the jump classifier marks it with `→`.
                rows.push((format!("  {}{}", pad, key), val));
            } else {
                rows.push((format!("  {}{}: {}", pad, key, val), String::new()));
            }
        }
    }
}

crate::sections! {
    pub enum FmsPolicyDetailSection,
    pub static FMS_POLICY_SECTIONS = [
        Overview "Overview",
        Scope "Scope",
        Config "Config",
        Compliance "Compliance" => crate::app::App::trigger_fms_compliance_load,
        Tags "Tags" => crate::app::App::trigger_fms_policy_tags_load,
    ]
}

impl Resource for FmsPolicy {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&FMS_POLICY_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.id
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn resource_type(&self) -> &str {
        "FMS Policy"
    }
    fn state(&self) -> ResourceState {
        // NON_COMPLIANT reads red, matching AWS Config's rule mapping.
        if self.status.as_deref() == Some("OUT_OF_ADMIN_SCOPE") {
            return ResourceState::Unavailable;
        }
        let (_, bad) = self.compliance_counts();
        if bad > 0 {
            return ResourceState::Unavailable;
        }
        ResourceState::Available
    }

    fn state_label(&self) -> String {
        // Same two-level logic as state(): admin scope first, then compliance.
        if self.status.as_deref() == Some("OUT_OF_ADMIN_SCOPE") {
            return "out of admin scope".to_string();
        }
        let (_, bad) = self.compliance_counts();
        if bad > 0 {
            "non-compliant".to_string()
        } else {
            "compliant".to_string()
        }
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        let (_, bad) = self.compliance_counts();
        format!(
            "{} {} {} {} fms policy firewall manager{}",
            self.name,
            self.service_kind,
            self.resource_type,
            self.id,
            if bad > 0 { " noncompliant" } else { "" }
        )
    }
    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Name".to_string(), self.name.clone()),
            ("Type".to_string(), self.service_kind.clone()),
            ("Resource Type".to_string(), self.resource_type.clone()),
        ]
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/wafv2/fmsv2/home?region={}#/policies",
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


/// `ListTagsForResource` on a policy ARN (lazy Tags section).
pub async fn fetch_fms_policy_tags(
    client: FmsClient,
    resource_arn: String,
) -> Result<Vec<(String, String)>> {
    let resp = client
        .list_tags_for_resource()
        .resource_arn(&resource_arn)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
    let mut out: Vec<(String, String)> = resp
        .tag_list()
        .iter()
        .map(|t| (t.key().to_string(), t.value().to_string()))
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

// ── Compliance ──────────────────────────────────────────────────────────────

/// One member account's rollup from `ListComplianceStatus` — overall status
/// across the policy's evaluation results plus the summed violator count.
#[derive(Debug, Clone)]
pub struct FmsAccountCompliance {
    pub account: String,
    pub compliant: bool,
    pub violators: i64,
    /// AWS stopped counting (huge account) — the real number is higher.
    pub limit_exceeded: bool,
}

/// Paginated `ListComplianceStatus` for one policy → per-account rollups
/// (non-compliant first, most violators first) + deduped dependent-service
/// issues.
async fn fetch_policy_compliance(
    client: &FmsClient,
    policy_id: &str,
) -> Result<(Vec<FmsAccountCompliance>, Vec<(String, String)>)> {
    let mut accounts: Vec<FmsAccountCompliance> = Vec::new();
    let mut issues: Vec<(String, String)> = Vec::new();
    let mut paginator = client
        .list_compliance_status()
        .policy_id(policy_id)
        .max_results(PAGE)
        .into_paginator()
        .send();
    while let Some(page) = paginator.next().await {
        let page = page
            .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
        for status in page.policy_compliance_status_list() {
            accounts.push(summarize_compliance_status(status));
            if let Some(map) = status.issue_info_map() {
                for (svc, msg) in map {
                    let svc = svc.as_str().to_string();
                    if !issues.iter().any(|(s, m)| *s == svc && m == msg) {
                        issues.push((svc, msg.clone()));
                    }
                }
            }
        }
    }
    accounts.sort_by(|a, b| {
        a.compliant
            .cmp(&b.compliant)
            .then(b.violators.cmp(&a.violators))
            .then(a.account.cmp(&b.account))
    });
    issues.sort();
    Ok((accounts, issues))
}

/// Collapse one account's evaluation results (one per resource type the
/// policy covers) into a single row.
fn summarize_compliance_status(
    status: &aws_sdk_fms::types::PolicyComplianceStatus,
) -> FmsAccountCompliance {
    let mut compliant = true;
    let mut violators = 0i64;
    let mut limit_exceeded = false;
    for r in status.evaluation_results() {
        if r.compliance_status().map(|s| s.as_str()) == Some("NON_COMPLIANT") {
            compliant = false;
        }
        violators += r.violator_count();
        limit_exceeded |= r.evaluation_limit_exceeded();
    }
    FmsAccountCompliance {
        account: status.member_account().unwrap_or_default().to_string(),
        compliant,
        violators,
        limit_exceeded,
    }
}

/// One violating resource from `GetComplianceDetail`.
#[derive(Debug, Clone)]
pub struct FmsViolator {
    pub resource_id: String,
    /// Trailing segment of the CFN-style type (`AWS::EC2::SecurityGroup` →
    /// `SecurityGroup`).
    pub resource_type: String,
    pub reason: String,
}

/// The lazy per-account violator drill (Compliance section), keyed
/// `"{policy_id}/{account}"` in `App.fms_compliance_details`.
#[derive(Debug, Clone)]
pub struct FmsComplianceDetail {
    pub violators: Vec<FmsViolator>,
    pub limit_exceeded: bool,
}


pub async fn fetch_fms_compliance_detail(
    client: FmsClient,
    policy_id: String,
    member_account: String,
) -> Result<FmsComplianceDetail> {
    let resp = client
        .get_compliance_detail()
        .policy_id(&policy_id)
        .member_account(&member_account)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
    let Some(detail) = resp.policy_compliance_detail() else {
        return Ok(FmsComplianceDetail {
            violators: Vec::new(),
            limit_exceeded: false,
        });
    };
    let mut violators: Vec<FmsViolator> = detail
        .violators()
        .iter()
        .map(|v| FmsViolator {
            resource_id: v.resource_id().unwrap_or_default().to_string(),
            resource_type: v
                .resource_type()
                .map(|t| t.rsplit("::").next().unwrap_or(t).to_string())
                .unwrap_or_default(),
            reason: v
                .violation_reason()
                .map(|r| pretty_reason(r.as_str()))
                .unwrap_or_default(),
        })
        .collect();
    violators.sort_by(|a, b| a.reason.cmp(&b.reason).then(a.resource_id.cmp(&b.resource_id)));
    Ok(FmsComplianceDetail {
        violators,
        limit_exceeded: detail.evaluation_limit_exceeded(),
    })
}

/// `WEB_ACL_MISSING_RULE_GROUP` → `Web acl missing rule group`.
pub fn pretty_reason(reason: &str) -> String {
    let lower = reason.replace('_', " ").to_lowercase();
    let mut chars = lower.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
        None => lower,
    }
}

// ── Apps Lists ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct FmsAppsList {
    pub id: String,
    pub arn: Option<String>,
    pub name: String,
    /// (app name, protocol, port)
    pub apps: Vec<(String, String, i64)>,
}

impl FmsAppsList {
    pub fn from_summary(s: &aws_sdk_fms::types::AppsListDataSummary) -> Self {
        Self {
            id: s.list_id().unwrap_or_default().to_string(),
            arn: s.list_arn().map(|a| a.to_string()),
            name: s.list_name().unwrap_or_default().to_string(),
            apps: s
                .apps_list()
                .iter()
                .map(|a| (a.app_name().to_string(), a.protocol().to_string(), a.port()))
                .collect(),
        }
    }
}

impl Resource for FmsAppsList {
    fn id(&self) -> &str {
        &self.id
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn resource_type(&self) -> &str {
        "FMS Apps List"
    }
    fn state(&self) -> ResourceState {
        ResourceState::stateless()
    }
    fn tags(&self) -> &HashMap<String, String> {
        static EMPTY: std::sync::OnceLock<HashMap<String, String>> = std::sync::OnceLock::new();
        EMPTY.get_or_init(HashMap::new)
    }
    fn search_text(&self) -> String {
        let apps: Vec<String> = self.apps.iter().map(|(n, _, _)| n.clone()).collect();
        format!("{} {} fms apps list applications", self.name, apps.join(" "))
    }
    fn details(&self) -> Vec<(String, String)> {
        let mut rows = vec![
            ("Name".to_string(), self.name.clone()),
            ("List ID".to_string(), self.id.clone()),
            ("Applications".to_string(), self.apps.len().to_string()),
        ];
        if let Some(arn) = &self.arn {
            rows.push(("ARN".to_string(), arn.clone()));
        }
        if !self.apps.is_empty() {
            rows.push((String::new(), String::new()));
            rows.push(("Applications".to_string(), String::new()));
            for (name, protocol, port) in &self.apps {
                rows.push((
                    format!("  {:<24} {}/{}", name, protocol, port),
                    String::new(),
                ));
            }
        }
        rows
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/wafv2/fmsv2/home?region={}#/application-lists",
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

// ── Protocols Lists ─────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct FmsProtocolsList {
    pub id: String,
    pub arn: Option<String>,
    pub name: String,
    pub protocols: Vec<String>,
}

impl FmsProtocolsList {
    pub fn from_summary(s: &aws_sdk_fms::types::ProtocolsListDataSummary) -> Self {
        Self {
            id: s.list_id().unwrap_or_default().to_string(),
            arn: s.list_arn().map(|a| a.to_string()),
            name: s.list_name().unwrap_or_default().to_string(),
            protocols: s.protocols_list().to_vec(),
        }
    }
}

impl Resource for FmsProtocolsList {
    fn id(&self) -> &str {
        &self.id
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn resource_type(&self) -> &str {
        "FMS Protocols List"
    }
    fn state(&self) -> ResourceState {
        ResourceState::stateless()
    }
    fn tags(&self) -> &HashMap<String, String> {
        static EMPTY: std::sync::OnceLock<HashMap<String, String>> = std::sync::OnceLock::new();
        EMPTY.get_or_init(HashMap::new)
    }
    fn search_text(&self) -> String {
        format!(
            "{} {} fms protocols list",
            self.name,
            self.protocols.join(" ")
        )
    }
    fn details(&self) -> Vec<(String, String)> {
        let mut rows = vec![
            ("Name".to_string(), self.name.clone()),
            ("List ID".to_string(), self.id.clone()),
            (
                "Protocols".to_string(),
                if self.protocols.is_empty() {
                    "(none)".to_string()
                } else {
                    self.protocols.join(", ")
                },
            ),
        ];
        if let Some(arn) = &self.arn {
            rows.push(("ARN".to_string(), arn.clone()));
        }
        rows
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/wafv2/fmsv2/home?region={}#/protocol-lists",
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

// ── Resource Sets ───────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct FmsResourceSet {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub status: Option<String>, // ACTIVE / OUT_OF_ADMIN_SCOPE
    pub last_update: Option<String>,
}

impl FmsResourceSet {
    pub fn from_summary(s: &aws_sdk_fms::types::ResourceSetSummary) -> Self {
        Self {
            id: s.id().unwrap_or_default().to_string(),
            name: s.name().unwrap_or_default().to_string(),
            description: s.description().map(|d| d.to_string()),
            status: s.resource_set_status().map(|st| st.as_str().to_string()),
            last_update: s
                .last_update_time()
                .map(|t| crate::aws::services::cloudwatch::fmt_epoch_secs(t.secs())),
        }
    }
}

crate::sections! {
    pub enum FmsResourceSetDetailSection,
    pub static FMS_RESOURCE_SET_SECTIONS = [
        Overview "Overview",
        Members "Members" => crate::app::App::trigger_fms_resource_set_detail_load,
    ]
}

impl Resource for FmsResourceSet {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&FMS_RESOURCE_SET_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.id
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn resource_type(&self) -> &str {
        "FMS Resource Set"
    }
    fn state(&self) -> ResourceState {
        match self.status.as_deref() {
            Some("OUT_OF_ADMIN_SCOPE") => ResourceState::Unavailable,
            _ => ResourceState::Available,
        }
    }

    fn state_label(&self) -> String {
        native_state_label(self.status.as_deref().unwrap_or(""), || self.state())
    }
    fn tags(&self) -> &HashMap<String, String> {
        static EMPTY: std::sync::OnceLock<HashMap<String, String>> = std::sync::OnceLock::new();
        EMPTY.get_or_init(HashMap::new)
    }
    fn search_text(&self) -> String {
        format!(
            "{} {} {} fms resource set",
            self.name,
            self.id,
            self.description.as_deref().unwrap_or("")
        )
    }
    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Name".to_string(), self.name.clone()),
            ("ID".to_string(), self.id.clone()),
        ]
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/wafv2/fmsv2/home?region={}#/resource-sets",
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

/// The lazy Members section payload — resource types from `GetResourceSet`
/// plus the member resources from `ListResourceSetResources`.
#[derive(Debug, Clone)]
pub struct FmsResourceSetDetail {
    pub resource_types: Vec<String>,
    /// (resource URI, owning account id)
    pub members: Vec<(String, String)>,
}


pub async fn fetch_fms_resource_set_detail(
    client: FmsClient,
    set_id: String,
) -> Result<FmsResourceSetDetail> {
    let sdk_err = |e: String| crate::error::Error::AwsSdk(e);

    let set = client
        .get_resource_set()
        .identifier(&set_id)
        .send()
        .await
        .map_err(|e| sdk_err(crate::error::sdk_error_message(&e)))?;
    let resource_types = set
        .resource_set()
        .map(|r| r.resource_type_list().to_vec())
        .unwrap_or_default();

    let mut members: Vec<(String, String)> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let resp = client
            .list_resource_set_resources()
            .identifier(&set_id)
            .max_results(PAGE)
            .set_next_token(token.clone())
            .send()
            .await
            .map_err(|e| sdk_err(crate::error::sdk_error_message(&e)))?;
        for r in resp.items() {
            members.push((
                r.uri().to_string(),
                r.account_id().unwrap_or_default().to_string(),
            ));
        }
        token = crate::aws::pagination::next_page_token(resp.next_token(), &token);
        if token.is_none() {
            break;
        }
    }
    members.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(FmsResourceSetDetail {
        resource_types,
        members,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn msd_flattens_waf_style_document() {
        let json = r#"{
            "type": "WAFV2",
            "defaultAction": {"type": "ALLOW"},
            "overrideCustomerWebACLAssociation": false,
            "preProcessRuleGroups": [
                {
                    "ruleGroupArn": null,
                    "overrideAction": {"type": "NONE"},
                    "managedRuleGroupIdentifier": {
                        "vendorName": "AWS",
                        "managedRuleGroupName": "AWSManagedRulesCommonRuleSet"
                    }
                }
            ]
        }"#;
        let rows = msd_rows(json);
        // Top-level scalar → key-value row.
        assert!(rows
            .iter()
            .any(|(k, v)| k == "type" && v == "WAFV2"));
        assert!(rows
            .iter()
            .any(|(k, v)| k == "overrideCustomerWebACLAssociation" && v == "false"));
        // Top-level object → group header (non-empty key, empty value).
        assert!(rows
            .iter()
            .any(|(k, v)| k == "defaultAction" && v.is_empty()));
        // Nested scalar → indented content line.
        assert!(rows.iter().any(|(k, _)| k.contains("type: ALLOW")));
        // Nested managed rule group name shows up.
        assert!(rows
            .iter()
            .any(|(k, _)| k.contains("AWSManagedRulesCommonRuleSet")));
    }

    #[test]
    fn msd_joins_scalar_arrays_inline() {
        let json = r#"{"statelessDefaultActions": ["aws:forward_to_sfe", "aws:pass"]}"#;
        let rows = msd_rows(json);
        assert!(rows.iter().any(|(k, v)| k == "statelessDefaultActions"
            && v == "aws:forward_to_sfe, aws:pass"));
    }

    #[test]
    fn msd_nested_jumpable_values_become_key_value_rows() {
        let json = r#"{
            "networkFirewallStatelessRuleGroupReferences": [
                {"resourceARN": "arn:aws:network-firewall:us-east-1:111122223333:stateless-rulegroup/base", "priority": 1}
            ],
            "securityGroups": [{"id": "sg-0abc1234"}]
        }"#;
        let rows = msd_rows(json);
        // A nested ARN keeps key-value form (value non-empty) so the jump
        // classifier sees it…
        assert!(rows.iter().any(|(k, v)| k.trim_start() == "resourceARN"
            && v.starts_with("arn:aws:network-firewall:")));
        assert!(rows
            .iter()
            .any(|(k, v)| k.trim_start() == "id" && v == "sg-0abc1234"));
        // …while a plain nested scalar stays a content line (value empty).
        assert!(rows
            .iter()
            .any(|(k, v)| k.contains("priority: 1") && v.is_empty()));
    }

    #[test]
    fn msd_survives_non_json_input() {
        let rows = msd_rows("not json at all");
        assert_eq!(rows.len(), 1);
        assert!(rows[0].0.contains("not json at all"));
    }

    #[test]
    fn compliance_status_summarizes_across_evaluation_results() {
        use aws_sdk_fms::types::{
            EvaluationResult, PolicyComplianceStatus, PolicyComplianceStatusType,
        };
        let status = PolicyComplianceStatus::builder()
            .member_account("111122223333")
            .evaluation_results(
                EvaluationResult::builder()
                    .compliance_status(PolicyComplianceStatusType::Compliant)
                    .violator_count(0)
                    .build(),
            )
            .evaluation_results(
                EvaluationResult::builder()
                    .compliance_status(PolicyComplianceStatusType::NonCompliant)
                    .violator_count(4)
                    .evaluation_limit_exceeded(true)
                    .build(),
            )
            .build();
        let row = summarize_compliance_status(&status);
        assert_eq!(row.account, "111122223333");
        assert!(!row.compliant);
        assert_eq!(row.violators, 4);
        assert!(row.limit_exceeded);
    }

    #[test]
    fn noncompliant_policy_reads_unavailable() {
        use crate::aws::resource::{Resource, ResourceState};
        let mut p = FmsPolicy::from_summary(
            &aws_sdk_fms::types::PolicySummary::builder()
                .policy_id("p-1")
                .policy_name("waf-baseline")
                .build(),
        );
        assert_eq!(p.state(), ResourceState::Available);
        p.compliance.push(FmsAccountCompliance {
            account: "111122223333".to_string(),
            compliant: false,
            violators: 2,
            limit_exceeded: false,
        });
        assert_eq!(p.state(), ResourceState::Unavailable);
        assert!(p.search_text().contains("noncompliant"));
        assert_eq!(p.compliance_counts(), (0, 1));
        assert_eq!(p.total_violators(), 2);
    }

    #[test]
    fn violation_reasons_prettify() {
        assert_eq!(
            pretty_reason("WEB_ACL_MISSING_RULE_GROUP"),
            "Web acl missing rule group"
        );
        assert_eq!(pretty_reason(""), "");
    }

    #[test]
    fn scope_map_flattens_sorted_with_labels() {
        use aws_sdk_fms::types::CustomerPolicyScopeIdType;
        let mut map = HashMap::new();
        map.insert(
            CustomerPolicyScopeIdType::OrgUnit,
            vec!["ou-1".to_string(), "ou-2".to_string()],
        );
        map.insert(CustomerPolicyScopeIdType::Account, vec!["111122223333".to_string()]);
        let out = flatten_scope_map(Some(&map));
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].0, "Accounts");
        assert_eq!(out[0].1, vec!["111122223333".to_string()]);
        assert_eq!(out[1].0, "Org units");
        assert_eq!(out[1].1.len(), 2);
    }
}
