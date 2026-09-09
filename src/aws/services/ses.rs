use crate::aws::client::AwsClients;
use crate::aws::resource::{Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::error::Result;
use crate::event::{Event, LoadProgress};
use async_trait::async_trait;
use aws_sdk_sesv2::Client as SesClient;
use std::any::Any;
use std::collections::HashMap;
use tokio::sync::mpsc;

pub use crate::aws::services::ec2::MetricsTimeRange;

/// Amazon SES (v2) — three sub-tabs (Identities / Configuration Sets /
/// Suppression List), one `ServiceType`, browse-only. Each type streams as its
/// own **error-tolerant** batch; a permission gap on any one API records a
/// status message and moves on rather than blanking the whole service.
///
/// - **Identities** — `ListEmailIdentities` (quick summaries) enriched by an N+1
///   `GetEmailIdentity` per identity (verification, DKIM, MAIL FROM, feedback
///   forwarding, tags, configuration set). Split pane Overview / DKIM /
///   MAIL FROM / Tags. The Overview also shows account-level send quota (fetched
///   once via `GetAccount`, delivered on [`Event::SesAccountLoaded`]).
/// - **Configuration Sets** — `ListConfigurationSets` + N+1 `GetConfigurationSet`
///   (sending/reputation/delivery/tracking/suppression/VDM posture + tags);
///   split pane Overview / Event Destinations (lazy
///   `GetConfigurationSetEventDestinations`) / Tags.
/// - **Suppression List** — `ListSuppressedDestinations` (address, reason,
///   last update). Flat details.
///
/// `m` charts account-wide `AWS/SES` metrics (Send / Delivery / Bounce /
/// Complaint / Reject / Rendering Failures) — no dimensions, so the same series
/// regardless of which SES row is selected.
pub struct SesService {
    client: SesClient,
}

/// Fixed cache key for the account-wide `m` metrics — SES `AWS/SES` metrics
/// carry no dimensions, so one series serves every SES row.
pub const SES_METRICS_KEY: &str = "__ses_account__";

impl SesService {
    pub fn new(aws_clients: &AwsClients) -> Self {
        Self {
            client: aws_clients.sesv2_client(),
        }
    }
}

#[async_trait]
impl AwsService for SesService {
    fn service_type(&self) -> ServiceType {
        ServiceType::Ses
    }

    fn name(&self) -> &str {
        "SES"
    }

    async fn list_resources(&self) -> Result<Vec<Box<dyn Resource>>> {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        self.list_resources_streaming(tx, ServiceType::Ses).await?;
        Ok(vec![])
    }

    async fn list_resources_streaming(
        &self,
        event_tx: mpsc::UnboundedSender<Event>,
        service_type: ServiceType,
    ) -> Result<()> {
        let mut total = 0usize;

        // Phase failures are non-fatal: warn and keep streaming the rest
        // (a fatal ResourceLoadError would make the app drop later batches).
        let err = |stage: &str, e: &dyn std::fmt::Display| Event::ResourceLoadWarning {
            service: service_type,
            warning: format!("SES {}: {}", stage, e),
        };
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

        // ── Phase 0: Account (send quota / sending enabled) ───────────────────
        // Not a resource — folded into the identity Overview via a dedicated
        // event. A permission gap here is non-fatal; the identities still load.
        if let Ok(acct) = self.client.get_account().send().await {
            let info = SesAccountInfo::from_sdk(&acct);
            let _ = event_tx.send(Event::SesAccountLoaded { info });
        }

        // ── Phase 1: Identities (N+1 GetEmailIdentity) ────────────────────────
        let mut summaries: Vec<aws_sdk_sesv2::types::IdentityInfo> = Vec::new();
        let mut paginator = self.client.list_email_identities().into_paginator().send();
        while let Some(page) = paginator.next().await {
            match page {
                Ok(p) => summaries.extend(p.email_identities().iter().cloned()),
                Err(e) => {
                    let _ =
                        event_tx.send(err("identities", &crate::error::sdk_error_message(&e)));
                    break;
                }
            }
        }
        let mut identities: Vec<SesIdentity> = Vec::new();
        for s in &summaries {
            let Some(name) = s.identity_name() else {
                continue;
            };
            match self.client.get_email_identity().email_identity(name).send().await {
                Ok(detail) => identities.push(SesIdentity::from_sdk(name, s, Some(&detail))),
                // Fall back to the list summary if the detail fetch is denied.
                Err(_) => identities.push(SesIdentity::from_sdk(name, s, None)),
            }
        }
        if !identities.is_empty() {
            total += identities.len();
            let batch: Vec<Box<dyn Resource>> = identities
                .into_iter()
                .map(|i| Box::new(i) as Box<dyn Resource>)
                .collect();
            emit(batch, total, Some("Loading configuration sets…"), false);
        }

        // ── Phase 2: Configuration Sets ───────────────────────────────────────
        // Names from ListConfigurationSets, then an N+1 GetConfigurationSet per
        // set (concurrent) for sending/reputation/delivery/tracking/suppression
        // posture + tags. A per-set describe failure degrades that set to
        // name-only rather than dropping it.
        let mut cs_names: Vec<String> = Vec::new();
        let mut cs_paginator = self.client.list_configuration_sets().into_paginator().send();
        while let Some(page) = cs_paginator.next().await {
            match page {
                Ok(p) => {
                    for name in p.configuration_sets() {
                        cs_names.push(name.to_string());
                    }
                }
                Err(e) => {
                    let _ = event_tx
                        .send(err("configuration sets", &crate::error::sdk_error_message(&e)));
                    break;
                }
            }
        }
        let cs_futs = cs_names.iter().map(|name| {
            let client = self.client.clone();
            let name = name.clone();
            async move {
                match client
                    .get_configuration_set()
                    .configuration_set_name(&name)
                    .send()
                    .await
                {
                    Ok(resp) => SesConfigSet::from_sdk(&name, Some(&resp)),
                    Err(_) => SesConfigSet::from_sdk(&name, None),
                }
            }
        });
        let config_sets: Vec<SesConfigSet> = futures::future::join_all(cs_futs).await;
        if !config_sets.is_empty() {
            total += config_sets.len();
            let batch: Vec<Box<dyn Resource>> = config_sets
                .into_iter()
                .map(|c| Box::new(c) as Box<dyn Resource>)
                .collect();
            emit(batch, total, Some("Loading suppression list…"), false);
        }

        // ── Phase 3: Suppression List ─────────────────────────────────────────
        let mut suppressed: Vec<SesSuppressedDest> = Vec::new();
        let mut sup_paginator = self
            .client
            .list_suppressed_destinations()
            .into_paginator()
            .send();
        while let Some(page) = sup_paginator.next().await {
            match page {
                Ok(p) => {
                    for s in p.suppressed_destination_summaries() {
                        suppressed.push(SesSuppressedDest::from_sdk(s));
                    }
                }
                Err(e) => {
                    let _ = event_tx
                        .send(err("suppression list", &crate::error::sdk_error_message(&e)));
                    break;
                }
            }
        }
        if !suppressed.is_empty() {
            total += suppressed.len();
            let batch: Vec<Box<dyn Resource>> = suppressed
                .into_iter()
                .map(|s| Box::new(s) as Box<dyn Resource>)
                .collect();
            emit(batch, total, None, true);
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

// ── Helpers ──────────────────────────────────────────────────────────────────

fn fmt_epoch(dt: Option<&aws_smithy_types::DateTime>) -> Option<String> {
    dt.map(|t| crate::aws::services::cloudwatch::fmt_epoch_secs(t.secs()))
}

// ── Account (folded into identity Overview) ──────────────────────────────────

/// Account-level SES posture — send quota + sending status. Fetched once per
/// load (`GetAccount`) and shown atop each identity's Overview section.
#[derive(Debug, Clone)]
pub struct SesAccountInfo {
    pub sending_enabled: bool,
    pub production_access: bool,
    pub enforcement_status: Option<String>,
    pub max_24h_send: f64,
    pub max_send_rate: f64,
    pub sent_last_24h: f64,
}

impl SesAccountInfo {
    pub fn from_sdk(
        a: &aws_sdk_sesv2::operation::get_account::GetAccountOutput,
    ) -> Self {
        let q = a.send_quota();
        Self {
            sending_enabled: a.sending_enabled(),
            production_access: a.production_access_enabled(),
            enforcement_status: a.enforcement_status().map(|s| s.to_string()),
            max_24h_send: q.map(|q| q.max24_hour_send()).unwrap_or(0.0),
            max_send_rate: q.map(|q| q.max_send_rate()).unwrap_or(0.0),
            sent_last_24h: q.map(|q| q.sent_last24_hours()).unwrap_or(0.0),
        }
    }
}

// ── Identities ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct SesIdentity {
    pub name: String,
    pub identity_type: Option<String>, // EMAIL_ADDRESS / DOMAIN / MANAGED_DOMAIN
    pub verified_for_sending: bool,
    pub verification_status: Option<String>,
    pub sending_enabled: bool,
    pub feedback_forwarding: bool,
    pub configuration_set: Option<String>,
    // DKIM
    pub dkim_signing_enabled: bool,
    pub dkim_status: Option<String>,
    pub dkim_origin: Option<String>,
    pub dkim_current_key_length: Option<String>,
    pub dkim_next_key_length: Option<String>,
    pub dkim_tokens: Vec<String>,
    // MAIL FROM
    pub mail_from_domain: Option<String>,
    pub mail_from_status: Option<String>,
    pub mail_from_behavior: Option<String>,
    pub tags: HashMap<String, String>,
}

impl SesIdentity {
    pub fn from_sdk(
        name: &str,
        summary: &aws_sdk_sesv2::types::IdentityInfo,
        detail: Option<&aws_sdk_sesv2::operation::get_email_identity::GetEmailIdentityOutput>,
    ) -> Self {
        let mut me = Self {
            name: name.to_string(),
            identity_type: summary.identity_type().map(|t| t.as_str().to_string()),
            verified_for_sending: false,
            verification_status: summary
                .verification_status()
                .map(|s| s.as_str().to_string()),
            sending_enabled: summary.sending_enabled(),
            feedback_forwarding: false,
            configuration_set: None,
            dkim_signing_enabled: false,
            dkim_status: None,
            dkim_origin: None,
            dkim_current_key_length: None,
            dkim_next_key_length: None,
            dkim_tokens: Vec::new(),
            mail_from_domain: None,
            mail_from_status: None,
            mail_from_behavior: None,
            tags: HashMap::new(),
        };
        if let Some(d) = detail {
            me.verified_for_sending = d.verified_for_sending_status();
            me.feedback_forwarding = d.feedback_forwarding_status();
            me.configuration_set = d.configuration_set_name().map(|s| s.to_string());
            if let Some(vs) = d.verification_status() {
                me.verification_status = Some(vs.as_str().to_string());
            }
            if d.identity_type().is_some() {
                me.identity_type = d.identity_type().map(|t| t.as_str().to_string());
            }
            if let Some(dk) = d.dkim_attributes() {
                me.dkim_signing_enabled = dk.signing_enabled();
                me.dkim_status = dk.status().map(|s| s.as_str().to_string());
                me.dkim_origin = dk.signing_attributes_origin().map(|o| o.as_str().to_string());
                me.dkim_current_key_length =
                    dk.current_signing_key_length().map(|k| k.as_str().to_string());
                me.dkim_next_key_length =
                    dk.next_signing_key_length().map(|k| k.as_str().to_string());
                me.dkim_tokens = dk.tokens().to_vec();
            }
            if let Some(mf) = d.mail_from_attributes() {
                me.mail_from_domain = Some(mf.mail_from_domain().to_string());
                me.mail_from_status = Some(mf.mail_from_domain_status().as_str().to_string());
                me.mail_from_behavior = Some(mf.behavior_on_mx_failure().as_str().to_string());
            }
            me.tags = d
                .tags()
                .iter()
                .map(|t| (t.key().to_string(), t.value().to_string()))
                .collect();
        }
        me
    }
}

crate::sections! {
    pub enum SesIdentityDetailSection,
    pub static SES_IDENTITY_SECTIONS = [
        Overview "Overview",
        Dkim "DKIM",
        MailFrom "MAIL FROM",
        Tags "Tags",
    ]
}

impl Resource for SesIdentity {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&SES_IDENTITY_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.name
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn resource_type(&self) -> &str {
        "SES Identity"
    }
    fn state(&self) -> ResourceState {
        if self.verified_for_sending {
            return ResourceState::Available;
        }
        match self.verification_status.as_deref() {
            Some("SUCCESS") => ResourceState::Available,
            Some("PENDING") => ResourceState::Pending,
            Some(other) => ResourceState::Unknown(other.to_string()),
            None => ResourceState::Unavailable,
        }
    }

    fn state_label(&self) -> String {
        if self.verified_for_sending {
            return "verified".to_string();
        }
        match self.verification_status.as_deref() {
            Some(s) if !s.trim().is_empty() => s.to_lowercase(),
            _ => "unverified".to_string(),
        }
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        format!(
            "{} {} {} ses email identity sender",
            self.name,
            self.identity_type.as_deref().unwrap_or(""),
            self.mail_from_domain.as_deref().unwrap_or("")
        )
    }
    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Identity".to_string(), self.name.clone()),
            (
                "Type".to_string(),
                self.identity_type.clone().unwrap_or_default(),
            ),
        ]
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/ses/home?region={}#/verified-identities/{}",
            region, region, self.name
        ))
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── Configuration Sets ───────────────────────────────────────────────────────

/// A configuration set enriched by `GetConfigurationSet`. `detail_ok` is false
/// when the describe failed (permission gap) — the set stays listed name-only.
#[derive(Debug, Clone)]
pub struct SesConfigSet {
    pub name: String,
    pub detail_ok: bool,
    pub sending_enabled: Option<bool>,
    pub reputation_enabled: Option<bool>,
    pub last_fresh_start: Option<String>,
    pub tls_policy: Option<String>,
    pub sending_pool: Option<String>,
    pub max_delivery_secs: Option<i64>,
    pub redirect_domain: Option<String>,
    pub https_policy: Option<String>,
    pub suppressed_reasons: Vec<String>,
    pub vdm_engagement: Option<String>,
    pub vdm_guardian: Option<String>,
    pub archiving_arn: Option<String>,
    pub tags: HashMap<String, String>,
}

impl SesConfigSet {
    pub fn from_sdk(
        name: &str,
        resp: Option<&aws_sdk_sesv2::operation::get_configuration_set::GetConfigurationSetOutput>,
    ) -> Self {
        let mut cs = Self {
            name: name.to_string(),
            detail_ok: resp.is_some(),
            sending_enabled: None,
            reputation_enabled: None,
            last_fresh_start: None,
            tls_policy: None,
            sending_pool: None,
            max_delivery_secs: None,
            redirect_domain: None,
            https_policy: None,
            suppressed_reasons: Vec::new(),
            vdm_engagement: None,
            vdm_guardian: None,
            archiving_arn: None,
            tags: HashMap::new(),
        };
        let Some(r) = resp else { return cs };
        cs.sending_enabled = r.sending_options().map(|s| s.sending_enabled());
        cs.reputation_enabled = r
            .reputation_options()
            .map(|o| o.reputation_metrics_enabled());
        cs.last_fresh_start =
            fmt_epoch(r.reputation_options().and_then(|o| o.last_fresh_start()));
        cs.tls_policy = r
            .delivery_options()
            .and_then(|d| d.tls_policy())
            .map(|t| t.as_str().to_string());
        cs.sending_pool = r
            .delivery_options()
            .and_then(|d| d.sending_pool_name())
            .map(|s| s.to_string());
        cs.max_delivery_secs = r.delivery_options().and_then(|d| d.max_delivery_seconds());
        cs.redirect_domain = r
            .tracking_options()
            .map(|t| t.custom_redirect_domain().to_string());
        cs.https_policy = r
            .tracking_options()
            .and_then(|t| t.https_policy())
            .map(|p| p.as_str().to_string());
        cs.suppressed_reasons = r
            .suppression_options()
            .map(|s| {
                s.suppressed_reasons()
                    .iter()
                    .map(|x| x.as_str().to_string())
                    .collect()
            })
            .unwrap_or_default();
        cs.vdm_engagement = r
            .vdm_options()
            .and_then(|v| v.dashboard_options())
            .and_then(|d| d.engagement_metrics())
            .map(|s| s.as_str().to_string());
        cs.vdm_guardian = r
            .vdm_options()
            .and_then(|v| v.guardian_options())
            .and_then(|g| g.optimized_shared_delivery())
            .map(|s| s.as_str().to_string());
        cs.archiving_arn = r
            .archiving_options()
            .and_then(|a| a.archive_arn())
            .map(|s| s.to_string());
        cs.tags = r
            .tags()
            .iter()
            .map(|t| (t.key().to_string(), t.value().to_string()))
            .collect();
        cs
    }
}

crate::sections! {
    pub enum SesConfigSetDetailSection,
    pub static SES_CONFIG_SET_SECTIONS = [
        Overview "Overview",
        EventDestinations "Event Destinations" => crate::app::App::trigger_ses_event_dests_load,
        Tags "Tags",
    ]
}

impl Resource for SesConfigSet {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&SES_CONFIG_SET_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.name
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn resource_type(&self) -> &str {
        "SES Configuration Set"
    }
    fn state(&self) -> ResourceState {
        // Sending paused on this set is the operational red flag.
        match self.sending_enabled {
            Some(false) => ResourceState::Stopped,
            _ => ResourceState::Available,
        }
    }

    fn state_label(&self) -> String {
        // Same words as the config-set pane header.
        match self.sending_enabled {
            Some(false) => "sending paused",
            _ => "sending enabled",
        }
        .to_string()
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        format!("{} ses configuration set config", self.name)
    }
    fn details(&self) -> Vec<(String, String)> {
        // Fallback only — the split pane (ses_config_set_section_lines) is the
        // real view.
        vec![("Configuration Set".to_string(), self.name.clone())]
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/ses/home?region={}#/configuration-sets/{}",
            region, region, self.name
        ))
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// One event destination on a configuration set (where send/bounce/complaint…
/// events are published).
#[derive(Debug, Clone)]
pub struct SesEventDest {
    pub name: String,
    pub enabled: bool,
    pub event_types: Vec<String>,
    pub kind: String,        // CloudWatch / Firehose / SNS / EventBridge / Pinpoint
    pub target: Option<String>, // stream/topic/bus ARN when the type has one
}

/// Lazy fetch for a configuration set's event destinations (first view of the
/// Event Destinations section).
pub async fn fetch_ses_event_destinations(
    client: SesClient,
    config_set: String,
) -> Result<Vec<SesEventDest>> {
    let resp = client
        .get_configuration_set_event_destinations()
        .configuration_set_name(&config_set)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;

    Ok(resp
        .event_destinations()
        .iter()
        .map(|d| {
            let (kind, target) = if let Some(cw) = d.cloud_watch_destination() {
                let dims = cw
                    .dimension_configurations()
                    .iter()
                    .map(|c| c.dimension_name().to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                ("CloudWatch".to_string(), (!dims.is_empty()).then(|| format!("dims: {}", dims)))
            } else if let Some(fh) = d.kinesis_firehose_destination() {
                ("Firehose".to_string(), Some(fh.delivery_stream_arn().to_string()))
            } else if let Some(sns) = d.sns_destination() {
                ("SNS".to_string(), Some(sns.topic_arn().to_string()))
            } else if let Some(eb) = d.event_bridge_destination() {
                ("EventBridge".to_string(), Some(eb.event_bus_arn().to_string()))
            } else if d.pinpoint_destination().is_some() {
                ("Pinpoint".to_string(), None)
            } else {
                ("—".to_string(), None)
            };
            SesEventDest {
                name: d.name().to_string(),
                enabled: d.enabled(),
                event_types: d
                    .matching_event_types()
                    .iter()
                    .map(|t| t.as_str().to_string())
                    .collect(),
                kind,
                target,
            }
        })
        .collect())
}

// ── Suppression List ─────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct SesSuppressedDest {
    pub email_address: String,
    pub reason: String, // BOUNCE / COMPLAINT
    pub last_update: Option<String>,
    pub tags: HashMap<String, String>,
}

impl SesSuppressedDest {
    pub fn from_sdk(s: &aws_sdk_sesv2::types::SuppressedDestinationSummary) -> Self {
        Self {
            email_address: s.email_address().to_string(),
            reason: s.reason().as_str().to_string(),
            last_update: fmt_epoch(Some(s.last_update_time())),
            tags: HashMap::new(),
        }
    }
}

impl Resource for SesSuppressedDest {
    fn id(&self) -> &str {
        &self.email_address
    }
    fn name(&self) -> &str {
        &self.email_address
    }
    fn resource_type(&self) -> &str {
        "SES Suppressed Destination"
    }
    fn state(&self) -> ResourceState {
        // A suppressed address is a warning signal, not a healthy one.
        ResourceState::Unavailable
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        format!(
            "{} {} ses suppressed suppression bounce complaint",
            self.email_address, self.reason
        )
    }
    fn details(&self) -> Vec<(String, String)> {
        let mut rows = vec![
            ("Email Address".to_string(), self.email_address.clone()),
            ("Reason".to_string(), self.reason.clone()),
        ];
        if let Some(u) = &self.last_update {
            rows.push(("Last Update".to_string(), u.clone()));
        }
        rows
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/ses/home?region={}#/suppression-list",
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

// ── Metrics (account-wide `AWS/SES`, no dimensions) ──────────────────────────

#[derive(Debug, Clone)]
pub struct SesMetricsData {
    pub time_range: MetricsTimeRange,
    pub send: Vec<(f64, f64)>,
    pub delivery: Vec<(f64, f64)>,
    pub bounce: Vec<(f64, f64)>,
    pub complaint: Vec<(f64, f64)>,
    pub reject: Vec<(f64, f64)>,
    pub rendering_failures: Vec<(f64, f64)>,
    pub x_max: f64,
}

#[derive(Debug, Clone)]
pub enum SesMetricsState {
    Loading,
    Loaded(SesMetricsData),
}

/// Pull account-wide `AWS/SES` volume metrics. These carry no dimensions — the
/// numbers are for the whole account/region, so the same chart is shown for any
/// selected SES row. Send/Delivery are the throughput headline; Bounce/Complaint
/// are the reputation-risk signals; Reject/Rendering Failures catch pre-send
/// problems.
pub async fn fetch_ses_metrics(
    cw: aws_sdk_cloudwatch::Client,
    time_range: MetricsTimeRange,
) -> Result<SesMetricsData> {
    use aws_sdk_cloudwatch::types::Statistic;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let start = now - time_range.duration_secs();
    let period = time_range.period_secs();
    let start_dt = aws_sdk_cloudwatch::primitives::DateTime::from_secs(start);
    let end_dt = aws_sdk_cloudwatch::primitives::DateTime::from_secs(now);

    let metric = |name: &'static str| {
        cw.get_metric_statistics()
            .namespace("AWS/SES")
            .metric_name(name)
            .start_time(start_dt)
            .end_time(end_dt)
            .period(period)
            .set_statistics(Some(vec![Statistic::Sum]))
            .send()
    };

    let (send, delivery, bounce, complaint, reject, rendering) = tokio::join!(
        metric("Send"),
        metric("Delivery"),
        metric("Bounce"),
        metric("Complaint"),
        metric("Reject"),
        metric("Rendering Failures"),
    );

    let parse = |resp: std::result::Result<
        aws_sdk_cloudwatch::operation::get_metric_statistics::GetMetricStatisticsOutput,
        _,
    >| -> Vec<(f64, f64)> {
        let mut pts: Vec<(f64, f64)> = match resp {
            Ok(r) => r
                .datapoints()
                .iter()
                .filter_map(|dp| Some((dp.timestamp()?.secs() as f64 - start as f64, dp.sum()?)))
                .collect(),
            Err(_) => vec![],
        };
        pts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        pts
    };

    Ok(SesMetricsData {
        time_range,
        send: parse(send),
        delivery: parse(delivery),
        bounce: parse(bounce),
        complaint: parse(complaint),
        reject: parse(reject),
        rendering_failures: parse(rendering),
        x_max: time_range.duration_secs() as f64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_state_from_verification() {
        let summary = aws_sdk_sesv2::types::IdentityInfo::builder().build();
        let mut id = SesIdentity::from_sdk("a@example.com", &summary, None);
        assert_eq!(id.state(), ResourceState::Unavailable);
        id.verified_for_sending = true;
        assert_eq!(id.state(), ResourceState::Available);
    }

    #[test]
    fn suppressed_dest_is_unhealthy() {
        let d = SesSuppressedDest {
            email_address: "x@example.com".into(),
            reason: "BOUNCE".into(),
            last_update: None,
            tags: HashMap::new(),
        };
        assert_eq!(d.state(), ResourceState::Unavailable);
    }
}
