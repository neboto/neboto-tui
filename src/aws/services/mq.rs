use crate::aws::client::AwsClients;
use crate::aws::resource::{native_state_label, shell_quote, Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::error::Result;
use crate::event::{Event, LoadProgress};
use async_trait::async_trait;
use aws_sdk_mq::Client as MqClient;
use futures::stream::StreamExt;
use std::any::Any;
use std::collections::HashMap;
use tokio::sync::mpsc;

/// Amazon MQ — two sub-tabs: **Brokers** (ActiveMQ and RabbitMQ) and
/// **Configurations**. One region-scoped client.
///
/// Load order: configurations first (`ListConfigurations` carries everything
/// a row shows, tags included), then brokers (`ListBrokers` +
/// `DescribeBroker` each, streamed in chunks — the summary has no version,
/// endpoints, network or tags).
///
/// Lazy per-row sections, both on a configuration: **Document**
/// (`DescribeConfigurationRevision` for the latest revision) and
/// **Revisions** (`ListConfigurationRevisions`). A configuration's "Used By"
/// filters sibling broker rows — no fetch.
///
/// Credentials: the API never returns a user's password, and neboto calls
/// neither `DescribeUser` nor `ListUsers` — a broker's Users section is the
/// user names `DescribeBroker` already carries.
pub struct MqService {
    client: MqClient,
}

impl MqService {
    pub fn new(aws_clients: &AwsClients) -> Self {
        Self {
            client: aws_clients.mq_client(),
        }
    }
}

fn fmt_dt(dt: &aws_smithy_types::DateTime) -> String {
    crate::aws::services::cloudwatch::fmt_epoch_secs(dt.secs())
}

/// `DescribeBroker` fan-out.
const DESCRIBE_FANOUT: usize = 5;

/// Brokers are streamed in chunks of this many as their describes finish.
const BROKER_CHUNK: usize = 25;

/// Most revisions the Revisions section lists (newest first).
pub const MAX_REVISIONS: usize = 100;

/// `ACTIVEMQ` → `ActiveMQ`, `RABBITMQ` → `RabbitMQ`.
pub fn engine_label(engine: &str) -> &str {
    match engine {
        "ACTIVEMQ" => "ActiveMQ",
        "RABBITMQ" => "RabbitMQ",
        other => other,
    }
}

/// `ACTIVE_STANDBY_MULTI_AZ` → `active/standby multi-AZ`.
pub fn deployment_label(mode: &str) -> String {
    match mode {
        "SINGLE_INSTANCE" => "single instance".to_string(),
        "ACTIVE_STANDBY_MULTI_AZ" => "active/standby multi-AZ".to_string(),
        "CLUSTER_MULTI_AZ" => "cluster multi-AZ".to_string(),
        other => other.to_lowercase().replace('_', " "),
    }
}

#[async_trait]
impl AwsService for MqService {
    fn service_type(&self) -> ServiceType {
        ServiceType::Mq
    }

    fn name(&self) -> &str {
        "Amazon MQ"
    }

    async fn list_resources(&self) -> Result<Vec<Box<dyn Resource>>> {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        self.list_resources_streaming(tx, ServiceType::Mq).await?;
        Ok(vec![])
    }

    async fn list_resources_streaming(
        &self,
        event_tx: mpsc::UnboundedSender<Event>,
        service_type: ServiceType,
    ) -> Result<()> {
        let warn = |msg: String| {
            let _ = event_tx.send(Event::ResourceLoadWarning {
                service: service_type,
                warning: msg,
            });
        };
        let mut loaded = 0usize;
        let mut send = |batch: Vec<Box<dyn Resource>>, status: Option<&str>| {
            loaded += batch.len();
            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                service: service_type,
                resources: batch,
                progress: LoadProgress {
                    loaded_count: loaded,
                    total_count: None,
                    status_message: status.map(str::to_string),
                },
            });
        };

        // ── Configurations (no fluent paginator) ──────────────────────────────
        let mut configs: Vec<MqConfiguration> = Vec::new();
        let mut configs_failed = None;
        let mut token: Option<String> = None;
        loop {
            match self
                .client
                .list_configurations()
                .max_results(100)
                .set_next_token(token.clone())
                .send()
                .await
            {
                Ok(p) => {
                    configs.extend(p.configurations().iter().map(MqConfiguration::from_sdk));
                    token = crate::aws::pagination::next_page_token(p.next_token(), &token);
                    if token.is_none() {
                        break;
                    }
                }
                Err(e) => {
                    configs_failed = Some(crate::error::sdk_error_message(&e));
                    break;
                }
            }
        }

        // ── Broker list ───────────────────────────────────────────────────────
        let mut summaries = Vec::new();
        let mut brokers_failed = None;
        let mut pages = self.client.list_brokers().into_paginator().send();
        while let Some(page) = pages.next().await {
            match page {
                Ok(p) => summaries.extend(p.broker_summaries().iter().cloned()),
                Err(e) => {
                    brokers_failed = Some(crate::error::sdk_error_message(&e));
                    break;
                }
            }
        }

        // Both lists failed: nothing can stream (a missing mq:List*
        // permission, or a region without Amazon MQ).
        if let (Some(b), Some(_)) = (&brokers_failed, &configs_failed) {
            let _ = event_tx.send(Event::ResourceLoadError {
                service: service_type,
                error: format!("Amazon MQ unavailable: {b}"),
            });
            return Ok(());
        }
        for (what, err) in [("brokers", &brokers_failed), ("configurations", &configs_failed)] {
            if let Some(e) = err {
                warn(format!("Amazon MQ {what}: {e}"));
            }
        }

        configs.sort_by(|a, b| a.name.cmp(&b.name));
        let first: Vec<Box<dyn Resource>> = configs.into_iter().map(|c| Box::new(c) as Box<dyn Resource>).collect();
        if !first.is_empty() || !summaries.is_empty() {
            send(first, (!summaries.is_empty()).then_some("Loading brokers…"));
        }

        // ── Broker details, streamed ──────────────────────────────────────────
        let client = &self.client;
        let total = summaries.len();
        let mut errors: Vec<String> = Vec::new();
        let mut described = futures::stream::iter(summaries)
            .map(|s| async move {
                let id = s.broker_id().unwrap_or_default().to_string();
                match client.describe_broker().broker_id(&id).send().await {
                    Ok(out) => (MqBroker::from_describe(&out), None),
                    Err(e) => (MqBroker::from_summary(&s), Some(crate::error::sdk_error_message(&e))),
                }
            })
            .buffered(DESCRIBE_FANOUT)
            .chunks(BROKER_CHUNK);
        while let Some(chunk) = described.next().await {
            let mut batch: Vec<Box<dyn Resource>> = Vec::with_capacity(chunk.len());
            for (broker, err) in chunk {
                errors.extend(err);
                batch.push(Box::new(broker));
            }
            send(batch, None);
        }
        if let Some(first) = errors.first() {
            warn(format!("Amazon MQ broker details: {} of {total} failed ({first})", errors.len()));
        }

        let _ = event_tx.send(Event::ResourcesFullyLoaded {
            service: service_type,
            total_count: loaded,
        });
        Ok(())
    }

    async fn get_resource_details(&self, _id: &str) -> Result<Box<dyn Resource>> {
        Err(crate::error::Error::NotImplemented)
    }
}

// ── Broker ────────────────────────────────────────────────────────────────────

/// One broker node: a single-instance broker has one, active/standby two,
/// a RabbitMQ cluster one (the cluster's shared endpoint).
#[derive(Debug, Clone, Default)]
pub struct MqInstance {
    pub console_url: Option<String>,
    pub endpoints: Vec<String>,
    pub ip_address: Option<String>,
}

/// LDAP authentication settings. The service account's password is never
/// returned by the API.
#[derive(Debug, Clone, Default)]
pub struct MqLdap {
    pub hosts: Vec<String>,
    pub service_account: Option<String>,
    pub user_base: Option<String>,
    pub user_search: Option<String>,
    pub role_base: Option<String>,
    pub role_search: Option<String>,
}

impl MqLdap {
    fn from_sdk(l: &aws_sdk_mq::types::LdapServerMetadataOutput) -> Self {
        Self {
            hosts: l.hosts().to_vec(),
            service_account: l.service_account_username().map(str::to_string),
            user_base: l.user_base().map(str::to_string),
            user_search: l.user_search_matching().map(str::to_string),
            role_base: l.role_base().map(str::to_string),
            role_search: l.role_search_matching().map(str::to_string),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct MqBroker {
    pub arn: String,
    pub id: String,
    pub name: String,
    pub status: String,
    /// `ACTIVEMQ` / `RABBITMQ`.
    pub engine: String,
    pub engine_version: Option<String>,
    pub pending_engine_version: Option<String>,
    pub deployment_mode: String,
    pub instance_type: String,
    pub pending_instance_type: Option<String>,
    pub created: Option<String>,
    pub publicly_accessible: Option<bool>,
    pub auto_minor_version_upgrade: Option<bool>,
    pub auth_strategy: Option<String>,
    pub pending_auth_strategy: Option<String>,
    pub storage_type: Option<String>,
    pub subnets: Vec<String>,
    pub security_groups: Vec<String>,
    pub pending_security_groups: Vec<String>,
    pub instances: Vec<MqInstance>,
    /// `(configuration id, revision)`.
    pub config_current: Option<(String, i32)>,
    pub config_pending: Option<(String, i32)>,
    pub kms_key_id: Option<String>,
    pub aws_owned_key: Option<bool>,
    pub logs_general: Option<bool>,
    pub logs_audit: Option<bool>,
    pub general_log_group: Option<String>,
    pub audit_log_group: Option<String>,
    pub pending_logs_general: Option<bool>,
    pub pending_logs_audit: Option<bool>,
    /// `Sunday 02:00 UTC`.
    pub maintenance_window: Option<String>,
    /// `(user name, pending change)`. ActiveMQ only — RabbitMQ users live in
    /// the broker itself.
    pub users: Vec<(String, Option<String>)>,
    pub ldap: Option<MqLdap>,
    /// `(code, info)`.
    pub actions_required: Vec<(String, String)>,
    pub replication_mode: Option<String>,
    pub replication_role: Option<String>,
    /// `(broker id, region)` of the cross-region replica pair.
    pub replication_counterpart: Option<(String, String)>,
    pub tags: HashMap<String, String>,
    /// False when `DescribeBroker` failed and only the summary is known.
    pub described: bool,
}

impl MqBroker {
    pub fn from_summary(s: &aws_sdk_mq::types::BrokerSummary) -> Self {
        Self {
            arn: s.broker_arn().unwrap_or_default().to_string(),
            id: s.broker_id().unwrap_or_default().to_string(),
            name: s.broker_name().unwrap_or_default().to_string(),
            status: s.broker_state().map(|v| v.as_str().to_string()).unwrap_or_default(),
            engine: s.engine_type().map(|v| v.as_str().to_string()).unwrap_or_default(),
            deployment_mode: s.deployment_mode().map(|v| v.as_str().to_string()).unwrap_or_default(),
            instance_type: s.host_instance_type().unwrap_or_default().to_string(),
            created: s.created().map(fmt_dt),
            ..Default::default()
        }
    }

    pub fn from_describe(b: &aws_sdk_mq::operation::describe_broker::DescribeBrokerOutput) -> Self {
        let cfg_id = |c: Option<&aws_sdk_mq::types::ConfigurationId>| {
            c.and_then(|c| Some((c.id()?.to_string(), c.revision().unwrap_or_default())))
        };
        let cfgs = b.configurations();
        let logs = b.logs();
        let repl = b.data_replication_metadata();
        Self {
            arn: b.broker_arn().unwrap_or_default().to_string(),
            id: b.broker_id().unwrap_or_default().to_string(),
            name: b.broker_name().unwrap_or_default().to_string(),
            status: b.broker_state().map(|v| v.as_str().to_string()).unwrap_or_default(),
            engine: b.engine_type().map(|v| v.as_str().to_string()).unwrap_or_default(),
            engine_version: b.engine_version().map(str::to_string),
            pending_engine_version: b.pending_engine_version().map(str::to_string),
            deployment_mode: b.deployment_mode().map(|v| v.as_str().to_string()).unwrap_or_default(),
            instance_type: b.host_instance_type().unwrap_or_default().to_string(),
            pending_instance_type: b.pending_host_instance_type().map(str::to_string),
            created: b.created().map(fmt_dt),
            publicly_accessible: b.publicly_accessible(),
            auto_minor_version_upgrade: b.auto_minor_version_upgrade(),
            auth_strategy: b.authentication_strategy().map(|v| v.as_str().to_string()),
            pending_auth_strategy: b.pending_authentication_strategy().map(|v| v.as_str().to_string()),
            storage_type: b.storage_type().map(|v| v.as_str().to_string()),
            subnets: b.subnet_ids().to_vec(),
            security_groups: b.security_groups().to_vec(),
            pending_security_groups: b.pending_security_groups().to_vec(),
            instances: b
                .broker_instances()
                .iter()
                .map(|i| MqInstance {
                    console_url: i.console_url().map(str::to_string),
                    endpoints: i.endpoints().to_vec(),
                    ip_address: i.ip_address().map(str::to_string),
                })
                .collect(),
            config_current: cfg_id(cfgs.and_then(|c| c.current())),
            config_pending: cfg_id(cfgs.and_then(|c| c.pending())),
            kms_key_id: b.encryption_options().and_then(|e| e.kms_key_id()).map(str::to_string),
            aws_owned_key: b.encryption_options().and_then(|e| e.use_aws_owned_key()),
            logs_general: logs.and_then(|l| l.general()),
            logs_audit: logs.and_then(|l| l.audit()),
            general_log_group: logs.and_then(|l| l.general_log_group()).map(str::to_string),
            audit_log_group: logs.and_then(|l| l.audit_log_group()).map(str::to_string),
            pending_logs_general: logs.and_then(|l| l.pending()).and_then(|p| p.general()),
            pending_logs_audit: logs.and_then(|l| l.pending()).and_then(|p| p.audit()),
            maintenance_window: b.maintenance_window_start_time().map(|w| {
                let day = w.day_of_week().map(|d| d.as_str().to_string()).unwrap_or_default();
                let day = day.chars().take(1).chain(day.chars().skip(1).flat_map(char::to_lowercase)).collect::<String>();
                format!("{day} {} {}", w.time_of_day().unwrap_or_default(), w.time_zone().unwrap_or("UTC"))
            }),
            users: b
                .users()
                .iter()
                .map(|u| {
                    (
                        u.username().unwrap_or_default().to_string(),
                        u.pending_change().map(|c| c.as_str().to_lowercase()),
                    )
                })
                .collect(),
            ldap: b.ldap_server_metadata().map(MqLdap::from_sdk),
            actions_required: b
                .actions_required()
                .iter()
                .map(|a| {
                    (
                        a.action_required_code().unwrap_or_default().to_string(),
                        a.action_required_info().unwrap_or_default().to_string(),
                    )
                })
                .collect(),
            replication_mode: b.data_replication_mode().map(|v| v.as_str().to_string()),
            replication_role: repl.and_then(|r| r.data_replication_role()).map(str::to_string),
            replication_counterpart: repl.and_then(|r| r.data_replication_counterpart()).and_then(|c| {
                Some((c.broker_id()?.to_string(), c.region().unwrap_or_default().to_string()))
            }),
            tags: b.tags().cloned().unwrap_or_default(),
            described: true,
        }
    }

    pub fn is_rabbit(&self) -> bool {
        self.engine == "RABBITMQ"
    }

    /// `ActiveMQ 5.18`.
    pub fn engine_text(&self) -> String {
        match &self.engine_version {
            Some(v) => format!("{} {v}", engine_label(&self.engine)),
            None => engine_label(&self.engine).to_string(),
        }
    }

    /// The `Broker` dimension CloudWatch publishes under. RabbitMQ uses the
    /// broker name; ActiveMQ publishes per instance as `<name>-1` (and
    /// `<name>-2` for the second node of an active/standby pair).
    pub fn metric_broker(&self) -> String {
        if self.is_rabbit() {
            self.name.clone()
        } else {
            format!("{}-1", self.name)
        }
    }

    /// The log group `t` tails: general when it's on, else audit.
    pub fn log_group(&self) -> Option<String> {
        let on = |flag: Option<bool>, group: &Option<String>| {
            group.clone().filter(|g| flag == Some(true) && !g.is_empty())
        };
        on(self.logs_general, &self.general_log_group).or_else(|| on(self.logs_audit, &self.audit_log_group))
    }

    /// Changes waiting for the next maintenance window or reboot, as
    /// `(what, from → to)` rows.
    pub fn pending_changes(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        if let Some(v) = &self.pending_engine_version {
            out.push(("Engine Version".to_string(), v.clone()));
        }
        if let Some(v) = &self.pending_instance_type {
            out.push(("Instance Type".to_string(), v.clone()));
        }
        if let Some(v) = &self.pending_auth_strategy {
            out.push(("Authentication".to_string(), v.to_lowercase()));
        }
        if !self.pending_security_groups.is_empty() {
            out.push(("Security Groups".to_string(), self.pending_security_groups.join(", ")));
        }
        if let Some((id, rev)) = &self.config_pending {
            if self.config_current.as_ref() != Some(&(id.clone(), *rev)) {
                out.push(("Configuration".to_string(), format!("{id} revision {rev}")));
            }
        }
        let onoff = |b: bool| if b { "on" } else { "off" };
        if let Some(g) = self.pending_logs_general.filter(|g| Some(*g) != self.logs_general) {
            out.push(("General Logs".to_string(), onoff(g).to_string()));
        }
        if let Some(a) = self.pending_logs_audit.filter(|a| Some(*a) != self.logs_audit) {
            out.push(("Audit Logs".to_string(), onoff(a).to_string()));
        }
        out
    }
}

crate::sections! {
    pub enum MqBrokerDetailSection,
    pub static MQ_BROKER_SECTIONS = [
        Overview "Overview",
        Endpoints "Endpoints",
        Security "Security",
        Users "Users",
        Maintenance "Maintenance",
        Tags "Tags",
    ]
}

impl Resource for MqBroker {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&MQ_BROKER_SECTIONS)
    }

    fn security_group_ids(&self) -> Vec<String> {
        self.security_groups.clone()
    }

    fn references(&self) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> =
            self.security_groups.iter().map(|id| ("Security Group".to_string(), id.clone())).collect();
        out.extend(self.subnets.iter().map(|s| ("Subnet".to_string(), s.clone())));
        if let Some(k) = &self.kms_key_id {
            out.push(("KMS Key".to_string(), k.clone()));
        }
        out
    }

    fn cli_command(&self) -> Option<String> {
        Some(format!("aws mq describe-broker --broker-id {}", shell_quote(&self.id)))
    }

    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        if self.name.is_empty() {
            &self.id
        } else {
            &self.name
        }
    }

    fn resource_type(&self) -> &str {
        "MQ Broker"
    }

    fn state(&self) -> ResourceState {
        match self.status.as_str() {
            "RUNNING" | "REPLICA" => ResourceState::Running,
            "CREATION_IN_PROGRESS" => ResourceState::Creating,
            "DELETION_IN_PROGRESS" => ResourceState::Deleting,
            "REBOOT_IN_PROGRESS" => ResourceState::Pending,
            "CREATION_FAILED" | "CRITICAL_ACTION_REQUIRED" => ResourceState::Unavailable,
            other => ResourceState::Unknown(other.to_lowercase()),
        }
    }

    fn state_label(&self) -> String {
        native_state_label(&self.status, || self.state())
    }

    fn list_summary(&self) -> Option<String> {
        Some(format!("{} · {}", self.engine_text(), self.instance_type))
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!(
            "{} {} {} {} {} {} {} {} amazon mq broker",
            self.name,
            self.id,
            self.status,
            self.engine_text(),
            self.deployment_mode,
            self.instance_type,
            if self.publicly_accessible == Some(true) { "public" } else { "private" },
            self.config_current.as_ref().map(|(id, _)| id.as_str()).unwrap_or(""),
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Broker".to_string(), self.name().to_string()),
            ("Broker ID".to_string(), self.id.clone()),
            ("Status".to_string(), self.state_label()),
            ("Engine".to_string(), self.engine_text()),
            ("ARN".to_string(), self.arn.clone()),
        ]
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{region}.console.aws.amazon.com/amazon-mq/home?region={region}#/brokers/details?id={}",
            self.id
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── Configuration ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct MqConfiguration {
    pub arn: String,
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub engine: String,
    pub engine_version: Option<String>,
    pub auth_strategy: Option<String>,
    pub created: Option<String>,
    pub latest_revision: i32,
    pub latest_created: Option<String>,
    pub latest_description: Option<String>,
    pub tags: HashMap<String, String>,
}

impl MqConfiguration {
    pub fn from_sdk(c: &aws_sdk_mq::types::Configuration) -> Self {
        let latest = c.latest_revision();
        Self {
            arn: c.arn().unwrap_or_default().to_string(),
            id: c.id().unwrap_or_default().to_string(),
            name: c.name().unwrap_or_default().to_string(),
            description: c.description().filter(|d| !d.is_empty()).map(str::to_string),
            engine: c.engine_type().map(|v| v.as_str().to_string()).unwrap_or_default(),
            engine_version: c.engine_version().map(str::to_string),
            auth_strategy: c.authentication_strategy().map(|v| v.as_str().to_string()),
            created: c.created().map(fmt_dt),
            latest_revision: latest.and_then(|r| r.revision()).unwrap_or_default(),
            latest_created: latest.and_then(|r| r.created()).map(fmt_dt),
            latest_description: latest.and_then(|r| r.description()).filter(|d| !d.is_empty()).map(str::to_string),
            tags: c.tags().cloned().unwrap_or_default(),
        }
    }

    pub fn engine_text(&self) -> String {
        match &self.engine_version {
            Some(v) => format!("{} {v}", engine_label(&self.engine)),
            None => engine_label(&self.engine).to_string(),
        }
    }
}

crate::sections! {
    pub enum MqConfigDetailSection,
    pub static MQ_CONFIG_SECTIONS = [
        Overview "Overview",
        Document "Document" => crate::app::App::trigger_mq_config_document_load,
        Revisions "Revisions" => crate::app::App::trigger_mq_config_revisions_load,
        Tags "Tags",
    ]
}

impl Resource for MqConfiguration {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&MQ_CONFIG_SECTIONS)
    }

    fn cli_command(&self) -> Option<String> {
        Some(format!("aws mq describe-configuration --configuration-id {}", shell_quote(&self.id)))
    }

    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        if self.name.is_empty() {
            &self.id
        } else {
            &self.name
        }
    }

    fn resource_type(&self) -> &str {
        "MQ Configuration"
    }

    // A configuration has no lifecycle state.
    fn state(&self) -> ResourceState {
        ResourceState::stateless()
    }

    fn list_summary(&self) -> Option<String> {
        Some(format!("{} · revision {}", self.engine_text(), self.latest_revision))
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!(
            "{} {} {} {} amazon mq configuration",
            self.name,
            self.id,
            self.engine_text(),
            self.description.as_deref().unwrap_or(""),
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Configuration".to_string(), self.name().to_string()),
            ("Configuration ID".to_string(), self.id.clone()),
            ("Engine".to_string(), self.engine_text()),
            ("Latest Revision".to_string(), self.latest_revision.to_string()),
            ("ARN".to_string(), self.arn.clone()),
        ]
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{region}.console.aws.amazon.com/amazon-mq/home?region={region}#/configurations/details?id={}",
            self.id
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── Lazy: configuration document and revisions ────────────────────────────────

/// One revision's document: ActiveMQ's `activemq.xml` or RabbitMQ's
/// `rabbitmq.conf`, split into lines.
#[derive(Debug, Clone, Default)]
pub struct MqConfigDocument {
    pub revision: i32,
    pub created: Option<String>,
    pub description: Option<String>,
    pub lines: Vec<String>,
}

pub async fn fetch_config_document(client: MqClient, id: String, revision: i32) -> Result<MqConfigDocument> {
    use base64::Engine;
    let out = client
        .describe_configuration_revision()
        .configuration_id(&id)
        .configuration_revision(revision.to_string())
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
    // The document arrives base64-encoded; one that isn't is shown as sent.
    let raw = out.data().unwrap_or_default();
    let text = base64::engine::general_purpose::STANDARD
        .decode(raw.trim())
        .ok()
        .and_then(|b| String::from_utf8(b).ok())
        .unwrap_or_else(|| raw.to_string());
    Ok(MqConfigDocument {
        revision,
        created: out.created().map(fmt_dt),
        description: out.description().filter(|d| !d.is_empty()).map(str::to_string),
        lines: text.lines().map(|l| l.trim_end().to_string()).collect(),
    })
}

#[derive(Debug, Clone, Default)]
pub struct MqRevision {
    pub revision: i32,
    pub created: Option<String>,
    pub description: Option<String>,
}

/// A configuration's revisions, newest first, capped at [`MAX_REVISIONS`].
pub async fn fetch_config_revisions(client: MqClient, id: String) -> Result<Vec<MqRevision>> {
    let mut out: Vec<MqRevision> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let page = client
            .list_configuration_revisions()
            .configuration_id(&id)
            .max_results(100)
            .set_next_token(token.clone())
            .send()
            .await
            .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
        out.extend(page.revisions().iter().map(|r| MqRevision {
            revision: r.revision().unwrap_or_default(),
            created: r.created().map(fmt_dt),
            description: r.description().filter(|d| !d.is_empty()).map(str::to_string),
        }));
        token = crate::aws::pagination::next_page_token(page.next_token(), &token);
        if token.is_none() {
            break;
        }
    }
    out.sort_by_key(|r| std::cmp::Reverse(r.revision));
    out.truncate(MAX_REVISIONS);
    Ok(out)
}
