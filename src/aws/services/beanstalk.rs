use crate::aws::cli_actions::{CliAction, CliTier};
use crate::aws::client::AwsClients;
use crate::aws::resource::{native_state_label, shell_quote, Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::error::Result;
use crate::event::{Event, LoadProgress};
use async_trait::async_trait;
use aws_sdk_elasticbeanstalk::Client as EbClient;
use futures::stream::{self, StreamExt};
use std::any::Any;
use std::collections::HashMap;
use tokio::sync::mpsc;

/// AWS Elastic Beanstalk (`@eb`) — three sub-tabs: **Environments** (the
/// primary view), **Applications** and **Versions**. One regional client.
///
/// Two environment types share every list: **Standard** (EC2 instances in an
/// Auto Scaling group, the classic shape) and **Cluster** (Cluster Mode, GA
/// Sep 2026 — containers on an EKS cluster Beanstalk creates and operates,
/// shared by every Cluster environment on the same subnet set). The API marks
/// the type on `EnvironmentDescription.Tier`: `Cluster` / `EKS` for Cluster
/// Mode, `WebServer` / `Standard` or `Worker` / `SQS/HTTP` for Standard.
///
/// Load order: environments, applications, versions (every page — the API
/// promises no order — then newest-first and capped), then per-ARN tags for
/// environments + applications, then `DescribeEnvironmentResources` for the
/// **Cluster** environments only, so their EKS cluster ARN is on the row at
/// list time (the `U` lens on that cluster runs off warm caches and can't
/// trigger a lazy fetch).
///
/// Lazy environment sections: Health (`DescribeEnvironmentHealth`), Events
/// (`DescribeEvents`), Configuration (`DescribeConfigurationSettings`) and
/// Resources (`DescribeEnvironmentResources`). An application's
/// Environments / Versions sections filter sibling rows — no fetch.
pub struct BeanstalkService {
    client: EbClient,
}

impl BeanstalkService {
    pub fn new(aws_clients: &AwsClients) -> Self {
        Self {
            client: aws_clients.beanstalk_client(),
        }
    }
}

fn fmt_dt(dt: &aws_smithy_types::DateTime) -> String {
    crate::aws::services::cloudwatch::fmt_epoch_secs(dt.secs())
}

/// Most application versions kept on the Versions tab, newest first. The
/// per-application hard quota is 1000 and a lifecycle policy usually keeps
/// far fewer, but an account with many apps could still list thousands.
pub const MAX_VERSIONS: usize = 1000;
/// Events shown in an environment's Events section (one `DescribeEvents`
/// page, newest first — the API returns them in that order).
pub const MAX_EVENTS: i32 = 100;
/// Concurrent per-resource calls (tags, cluster resources) during the load.
const FANOUT: usize = 8;

#[async_trait]
impl AwsService for BeanstalkService {
    fn service_type(&self) -> ServiceType {
        ServiceType::Beanstalk
    }

    fn name(&self) -> &str {
        "Elastic Beanstalk"
    }

    async fn list_resources(&self) -> Result<Vec<Box<dyn Resource>>> {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        self.list_resources_streaming(tx, ServiceType::Beanstalk).await?;
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
        let progress = |msg: &str| {
            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                service: service_type,
                resources: Vec::new(),
                progress: LoadProgress {
                    loaded_count: 0,
                    total_count: None,
                    status_message: Some(msg.to_string()),
                },
            });
        };

        // ── Environments ──────────────────────────────────────────────────────
        // `IncludeDeleted(false)`: recently terminated environments would
        // otherwise list beside their live replacement under the same name,
        // and a name-keyed jump would land on whichever came first.
        let mut envs: Vec<EbEnvironment> = Vec::new();
        let mut envs_failed = None;
        let mut token: Option<String> = None;
        loop {
            match self
                .client
                .describe_environments()
                .include_deleted(false)
                .set_next_token(token.clone())
                .send()
                .await
            {
                Ok(resp) => {
                    envs.extend(resp.environments().iter().map(EbEnvironment::from_sdk));
                    token = crate::aws::pagination::next_page_token(resp.next_token(), &token);
                    if token.is_none() {
                        break;
                    }
                }
                Err(e) => {
                    envs_failed = Some(crate::error::sdk_error_message(&e));
                    break;
                }
            }
        }

        // ── Applications ──────────────────────────────────────────────────────
        let mut apps: Vec<EbApplication> = Vec::new();
        let mut apps_failed = None;
        match self.client.describe_applications().send().await {
            Ok(resp) => apps.extend(resp.applications().iter().map(EbApplication::from_sdk)),
            Err(e) => apps_failed = Some(crate::error::sdk_error_message(&e)),
        }

        if let (Some(e), Some(_)) = (&envs_failed, &apps_failed) {
            let _ = event_tx.send(Event::ResourceLoadError {
                service: service_type,
                error: format!("Elastic Beanstalk unavailable: {e}"),
            });
            return Ok(());
        }
        for (what, err) in [("environments", &envs_failed), ("applications", &apps_failed)] {
            if let Some(e) = err {
                warn(format!("Elastic Beanstalk {what}: {e}"));
            }
        }

        progress("Loading application versions…");

        // ── Application versions (every page, then newest-first + cap) ────────
        let mut versions: Vec<EbVersion> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            match self
                .client
                .describe_application_versions()
                .max_records(1000)
                .set_next_token(token.clone())
                .send()
                .await
            {
                Ok(resp) => {
                    versions.extend(resp.application_versions().iter().map(EbVersion::from_sdk));
                    token = crate::aws::pagination::next_page_token(resp.next_token(), &token);
                    if token.is_none() {
                        break;
                    }
                }
                Err(e) => {
                    warn(format!(
                        "Elastic Beanstalk application versions: {}",
                        crate::error::sdk_error_message(&e)
                    ));
                    break;
                }
            }
        }
        versions.sort_by_key(|v| std::cmp::Reverse(v.created_secs));
        if versions.len() > MAX_VERSIONS {
            warn(format!(
                "Elastic Beanstalk: {} application versions — showing the newest {MAX_VERSIONS}",
                versions.len()
            ));
            versions.truncate(MAX_VERSIONS);
        }

        // ── Tags (environments + applications, one call per ARN) ──────────────
        let arns: Vec<String> = envs
            .iter()
            .map(|e| e.arn.clone())
            .chain(apps.iter().map(|a| a.arn.clone()))
            .filter(|a| !a.is_empty())
            .collect();
        if !arns.is_empty() {
            progress("Loading tags…");
        }
        let client = &self.client;
        let tag_results: Vec<(String, std::result::Result<HashMap<String, String>, String>)> =
            stream::iter(arns)
                .map(|arn| async move {
                    let r = client
                        .list_tags_for_resource()
                        .resource_arn(&arn)
                        .send()
                        .await
                        .map(|out| {
                            out.resource_tags()
                                .iter()
                                .filter_map(|t| {
                                    Some((t.key()?.to_string(), t.value().unwrap_or_default().to_string()))
                                })
                                .collect::<HashMap<String, String>>()
                        })
                        .map_err(|e| crate::error::sdk_error_message(&e));
                    (arn, r)
                })
                .buffer_unordered(FANOUT)
                .collect()
                .await;
        let mut tags: HashMap<String, HashMap<String, String>> = HashMap::new();
        let mut tag_err = None;
        for (arn, r) in tag_results {
            match r {
                Ok(t) => {
                    tags.insert(arn, t);
                }
                Err(e) => tag_err = Some(e),
            }
        }
        if let Some(e) = tag_err {
            warn(format!("Elastic Beanstalk tags: {e}"));
        }
        for e in &mut envs {
            e.tags = tags.remove(&e.arn).unwrap_or_default();
        }
        for a in &mut apps {
            a.tags = tags.remove(&a.arn).unwrap_or_default();
        }

        // ── Cluster environments: their EKS cluster, eagerly ──────────────────
        let cluster_envs: Vec<String> = envs
            .iter()
            .filter(|e| e.deployment == EbDeploymentType::Cluster)
            .map(|e| e.id.clone())
            .collect();
        if !cluster_envs.is_empty() {
            progress("Resolving Cluster environments' EKS clusters…");
        }
        let cluster_results: Vec<(String, std::result::Result<Option<String>, String>)> =
            stream::iter(cluster_envs)
                .map(|id| async move {
                    let r = client
                        .describe_environment_resources()
                        .environment_id(&id)
                        .send()
                        .await
                        .map(|out| {
                            out.environment_resources()
                                .and_then(|r| r.cluster())
                                .and_then(|c| c.cluster_arn())
                                .map(str::to_string)
                        })
                        .map_err(|e| crate::error::sdk_error_message(&e));
                    (id, r)
                })
                .buffer_unordered(FANOUT)
                .collect()
                .await;
        let mut cluster_err = None;
        let mut clusters: HashMap<String, String> = HashMap::new();
        for (id, r) in cluster_results {
            match r {
                Ok(Some(arn)) => {
                    clusters.insert(id, arn);
                }
                Ok(None) => {}
                Err(e) => cluster_err = Some(e),
            }
        }
        if let Some(e) = cluster_err {
            warn(format!("Elastic Beanstalk cluster resources: {e}"));
        }
        for e in &mut envs {
            if let Some(arn) = clusters.remove(&e.id) {
                e.cluster_arn = Some(arn);
            }
        }

        // Environments that need attention lead their tab.
        envs.sort_by(|a, b| b.attention_rank().cmp(&a.attention_rank()).then(a.name.cmp(&b.name)));
        apps.sort_by(|a, b| a.name.cmp(&b.name));

        let mut batch: Vec<Box<dyn Resource>> = Vec::new();
        batch.extend(envs.into_iter().map(|r| Box::new(r) as Box<dyn Resource>));
        batch.extend(apps.into_iter().map(|r| Box::new(r) as Box<dyn Resource>));
        batch.extend(versions.into_iter().map(|r| Box::new(r) as Box<dyn Resource>));
        let total = batch.len();
        if total > 0 {
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

// ── Environment ───────────────────────────────────────────────────────────────

/// Standard (EC2 + Auto Scaling) vs Cluster (Cluster Mode, EKS-backed).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EbDeploymentType {
    Standard,
    Cluster,
}

impl EbDeploymentType {
    /// From `EnvironmentTier` — `Name = Cluster` / `Type = EKS` is Cluster
    /// Mode; everything else (`WebServer`/`Standard`, `Worker`/`SQS/HTTP`)
    /// is the EC2-based Standard type.
    pub fn from_tier(name: Option<&str>, ty: Option<&str>) -> Self {
        let cluster = name.is_some_and(|n| n.eq_ignore_ascii_case("Cluster"))
            || ty.is_some_and(|t| t.eq_ignore_ascii_case("EKS"));
        if cluster {
            EbDeploymentType::Cluster
        } else {
            EbDeploymentType::Standard
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            EbDeploymentType::Standard => "Standard",
            EbDeploymentType::Cluster => "Cluster",
        }
    }
}

/// The load balancer `DescribeEnvironments` inlines for a load-balanced
/// Standard environment.
#[derive(Debug, Clone)]
pub struct EbLoadBalancer {
    pub name: String,
    pub domain: Option<String>,
    pub listeners: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct EbEnvironment {
    pub id: String,
    pub name: String,
    pub arn: String,
    pub application: String,
    pub version_label: Option<String>,
    pub solution_stack: Option<String>,
    pub platform_arn: Option<String>,
    pub template_name: Option<String>,
    pub description: Option<String>,
    pub endpoint_url: Option<String>,
    pub cname: Option<String>,
    pub created: Option<String>,
    pub updated: Option<String>,
    /// `Ready`, `Updating`, `Launching`, `Terminating`, … (native case).
    pub status: String,
    /// Basic health colour: `Green` / `Yellow` / `Red` / `Grey`.
    pub health: String,
    /// Enhanced health status (`Ok`, `Warning`, `Degraded`, `Severe`, …) —
    /// only with enhanced health reporting.
    pub health_status: Option<String>,
    pub abortable_operation: bool,
    /// `EnvironmentTier.Name` / `.Type` as returned.
    pub tier_name: String,
    pub tier_type: String,
    pub deployment: EbDeploymentType,
    /// `(link name, environment name)` — worker ↔ web-tier links.
    pub links: Vec<(String, String)>,
    pub operations_role: Option<String>,
    pub load_balancer: Option<EbLoadBalancer>,
    /// The backing EKS cluster ARN — Cluster environments only, resolved at
    /// load time (`DescribeEnvironmentResources`).
    pub cluster_arn: Option<String>,
    pub tags: HashMap<String, String>,
}

impl EbEnvironment {
    pub fn from_sdk(e: &aws_sdk_elasticbeanstalk::types::EnvironmentDescription) -> Self {
        let s = |v: Option<&str>| v.filter(|s| !s.is_empty()).map(str::to_string);
        let tier = e.tier();
        let tier_name = tier.and_then(|t| t.name()).unwrap_or_default().to_string();
        let tier_type = tier.and_then(|t| t.r#type()).unwrap_or_default().to_string();
        let deployment = EbDeploymentType::from_tier(Some(tier_name.as_str()), Some(tier_type.as_str()));
        let load_balancer = e.resources().and_then(|r| r.load_balancer()).and_then(|lb| {
            Some(EbLoadBalancer {
                name: lb.load_balancer_name().filter(|n| !n.is_empty())?.to_string(),
                domain: s(lb.domain()),
                listeners: lb
                    .listeners()
                    .iter()
                    .map(|l| format!("{}:{}", l.protocol().unwrap_or("?"), l.port()))
                    .collect(),
            })
        });
        Self {
            id: e.environment_id().unwrap_or_default().to_string(),
            name: e.environment_name().unwrap_or_default().to_string(),
            arn: e.environment_arn().unwrap_or_default().to_string(),
            application: e.application_name().unwrap_or_default().to_string(),
            version_label: s(e.version_label()),
            solution_stack: s(e.solution_stack_name()),
            platform_arn: s(e.platform_arn()),
            template_name: s(e.template_name()),
            description: s(e.description()),
            endpoint_url: s(e.endpoint_url()),
            cname: s(e.cname()),
            created: e.date_created().map(fmt_dt),
            updated: e.date_updated().map(fmt_dt),
            status: e.status().map(|s| s.as_str().to_string()).unwrap_or_default(),
            health: e.health().map(|h| h.as_str().to_string()).unwrap_or_default(),
            health_status: e.health_status().map(|h| h.as_str().to_string()),
            abortable_operation: e.abortable_operation_in_progress().unwrap_or(false),
            tier_name,
            tier_type,
            deployment,
            links: e
                .environment_links()
                .iter()
                .map(|l| {
                    (
                        l.link_name().unwrap_or_default().to_string(),
                        l.environment_name().unwrap_or_default().to_string(),
                    )
                })
                .collect(),
            operations_role: s(e.operations_role()),
            load_balancer,
            cluster_arn: None,
            tags: HashMap::new(),
        }
    }

    pub fn is_ready(&self) -> bool {
        self.status.is_empty() || self.status.eq_ignore_ascii_case("Ready")
    }

    /// Sort key: red first, then yellow, then anything mid-operation.
    fn attention_rank(&self) -> u8 {
        match (self.is_ready(), self.health.as_str()) {
            (_, "Red") => 3,
            (_, "Yellow") => 2,
            (false, _) => 1,
            _ => 0,
        }
    }

    /// The platform line: the solution stack for Standard, or the platform
    /// ARN's tail when that's all there is (Cluster has no solution stack —
    /// the runtime is the container image).
    pub fn platform_label(&self) -> Option<String> {
        self.solution_stack.clone().or_else(|| {
            self.platform_arn
                .as_deref()
                .and_then(|a| a.split_once(":platform/").map(|(_, p)| p.to_string()))
        })
    }

    /// The EKS cluster name from [`Self::cluster_arn`].
    pub fn cluster_name(&self) -> Option<&str> {
        self.cluster_arn
            .as_deref()
            .and_then(|a| a.split_once(":cluster/").map(|(_, n)| n))
    }
}

crate::sections! {
    pub enum EbEnvironmentDetailSection,
    pub static EB_ENVIRONMENT_SECTIONS = [
        Overview "Overview",
        Health "Health" => crate::app::App::trigger_eb_health_load,
        Events "Events" => crate::app::App::trigger_eb_events_load,
        Configuration "Configuration" => crate::app::App::trigger_eb_config_load,
        Resources "Resources" => crate::app::App::trigger_eb_resources_load,
        Tags "Tags",
    ]
}

impl Resource for EbEnvironment {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&EB_ENVIRONMENT_SECTIONS)
    }

    fn references(&self) -> Vec<(String, String)> {
        let mut out = vec![("Application".to_string(), self.application.clone())];
        if let Some(c) = &self.cluster_arn {
            out.push(("EKS Cluster".to_string(), c.clone()));
        }
        if let Some(lb) = &self.load_balancer {
            out.push(("Load Balancer".to_string(), lb.name.clone()));
        }
        if let Some(v) = &self.version_label {
            out.push(("Application Version".to_string(), format!("{}@{v}", self.application)));
        }
        if let Some(p) = &self.platform_arn {
            out.push(("Platform".to_string(), p.clone()));
        }
        if let Some(r) = &self.operations_role {
            out.push(("Operations Role".to_string(), r.clone()));
        }
        for (_, env) in &self.links {
            out.push(("Linked Environment".to_string(), env.clone()));
        }
        out.retain(|(_, v)| !v.is_empty());
        out
    }

    fn trail_lookup_keys(&self) -> Vec<String> {
        let mut keys = vec![self.name.clone(), self.id.clone()];
        if !self.arn.is_empty() {
            keys.push(self.arn.clone());
        }
        keys
    }

    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws elasticbeanstalk describe-environments --environment-ids {}",
            shell_quote(&self.id)
        ))
    }

    fn cli_actions(&self) -> Vec<CliAction> {
        let id = shell_quote(&self.id);
        vec![
            CliAction::new(
                CliTier::Inspect,
                "describe-environment-health",
                format!(
                    "aws elasticbeanstalk describe-environment-health --environment-id {id} --attribute-names All"
                ),
            ),
            CliAction::new(
                CliTier::Inspect,
                "describe-events",
                format!("aws elasticbeanstalk describe-events --environment-id {id} --max-items 50"),
            ),
            CliAction::new(
                CliTier::Inspect,
                "describe-environment-resources",
                format!("aws elasticbeanstalk describe-environment-resources --environment-id {id}"),
            ),
            CliAction::new(
                CliTier::Inspect,
                "describe-configuration-settings",
                format!(
                    "aws elasticbeanstalk describe-configuration-settings --application-name {} --environment-name {}",
                    shell_quote(&self.application),
                    shell_quote(&self.name)
                ),
            ),
            CliAction::new(
                CliTier::Change,
                "restart-app-server",
                format!("aws elasticbeanstalk restart-app-server --environment-id {id}"),
            )
            .with_note("restarts the application server on every instance — no redeploy"),
        ]
    }

    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn resource_type(&self) -> &str {
        "Beanstalk Environment"
    }

    fn state(&self) -> ResourceState {
        match self.status.as_str() {
            "Terminated" => return ResourceState::Terminated,
            "Terminating" => return ResourceState::Deleting,
            "Launching" => return ResourceState::Creating,
            "Updating" | "Aborting" | "LinkingFrom" | "LinkingTo" => return ResourceState::Pending,
            _ => {}
        }
        match self.health.as_str() {
            "Green" => ResourceState::Running,
            "Yellow" => ResourceState::Pending,
            "Red" => ResourceState::Unavailable,
            other => ResourceState::Unknown(other.to_ascii_lowercase()),
        }
    }

    /// The health colour while the environment is `Ready` (the console's own
    /// vocabulary: green / yellow / red / grey), the lifecycle status while
    /// it's mid-operation.
    fn state_label(&self) -> String {
        let word = if self.is_ready() { &self.health } else { &self.status };
        native_state_label(word, || self.state())
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!(
            "{} {} {} {} {} {} {} {} {} {} {} beanstalk environment",
            self.name,
            self.id,
            self.application,
            self.status,
            self.health,
            self.deployment.label(),
            self.tier_name,
            self.cname.as_deref().unwrap_or(""),
            self.version_label.as_deref().unwrap_or(""),
            self.platform_label().unwrap_or_default(),
            self.cluster_arn.as_deref().unwrap_or(""),
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Environment".to_string(), self.name.clone()),
            ("Application".to_string(), self.application.clone()),
            ("Type".to_string(), self.deployment.label().to_string()),
            ("Health".to_string(), self.health.clone()),
            ("Status".to_string(), self.status.clone()),
        ]
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{region}.console.aws.amazon.com/elasticbeanstalk/home?region={region}#/environment/dashboard?environmentId={}",
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

// ── Application ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct EbApplication {
    pub name: String,
    pub arn: String,
    pub description: Option<String>,
    pub created: Option<String>,
    pub updated: Option<String>,
    pub configuration_templates: Vec<String>,
    /// Version lifecycle policy, one line per enabled rule.
    pub lifecycle_rules: Vec<String>,
    pub lifecycle_role: Option<String>,
    pub tags: HashMap<String, String>,
}

impl EbApplication {
    pub fn from_sdk(a: &aws_sdk_elasticbeanstalk::types::ApplicationDescription) -> Self {
        let lifecycle = a.resource_lifecycle_config();
        let mut rules = Vec::new();
        if let Some(v) = lifecycle.and_then(|l| l.version_lifecycle_config()) {
            if let Some(r) = v.max_count_rule().filter(|r| r.enabled()) {
                rules.push(format!(
                    "keep the newest {} versions{}",
                    r.max_count().map(|n| n.to_string()).unwrap_or_else(|| "?".into()),
                    if r.delete_source_from_s3() == Some(true) { " (deletes S3 source)" } else { "" }
                ));
            }
            if let Some(r) = v.max_age_rule().filter(|r| r.enabled()) {
                rules.push(format!(
                    "delete versions older than {} days{}",
                    r.max_age_in_days().map(|n| n.to_string()).unwrap_or_else(|| "?".into()),
                    if r.delete_source_from_s3() == Some(true) { " (deletes S3 source)" } else { "" }
                ));
            }
        }
        Self {
            name: a.application_name().unwrap_or_default().to_string(),
            arn: a.application_arn().unwrap_or_default().to_string(),
            description: a.description().filter(|d| !d.is_empty()).map(str::to_string),
            created: a.date_created().map(fmt_dt),
            updated: a.date_updated().map(fmt_dt),
            configuration_templates: a.configuration_templates().to_vec(),
            lifecycle_rules: rules,
            lifecycle_role: lifecycle.and_then(|l| l.service_role()).map(str::to_string),
            tags: HashMap::new(),
        }
    }
}

crate::sections! {
    pub enum EbApplicationDetailSection,
    pub static EB_APPLICATION_SECTIONS = [
        Overview "Overview",
        Environments "Environments",
        Versions "Versions",
        Tags "Tags",
    ]
}

impl Resource for EbApplication {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&EB_APPLICATION_SECTIONS)
    }

    fn references(&self) -> Vec<(String, String)> {
        self.lifecycle_role
            .iter()
            .map(|r| ("Lifecycle Role".to_string(), r.clone()))
            .collect()
    }

    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws elasticbeanstalk describe-applications --application-names {}",
            shell_quote(&self.name)
        ))
    }

    fn id(&self) -> &str {
        &self.name
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn resource_type(&self) -> &str {
        "Beanstalk Application"
    }

    fn state(&self) -> ResourceState {
        ResourceState::stateless()
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!(
            "{} {} {} beanstalk application",
            self.name,
            self.arn,
            self.description.as_deref().unwrap_or("")
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![("Application".to_string(), self.name.clone())]
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{region}.console.aws.amazon.com/elasticbeanstalk/home?region={region}#/application/overview?applicationName={}",
            self.name
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── Application version ───────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct EbVersion {
    /// `application@label` — a label is only unique within its application,
    /// and the Versions tab spans all of them.
    pub key: String,
    pub application: String,
    pub label: String,
    pub arn: String,
    pub description: Option<String>,
    /// `Processed`, `Unprocessed`, `Processing`, `Building`, `Failed`.
    pub status: String,
    pub created: Option<String>,
    pub created_secs: i64,
    /// `s3://bucket/key` of the source bundle (Standard; Cluster source builds).
    pub source_bundle: Option<String>,
    /// Container image URI (Cluster versions supplied as an image).
    pub image_uri: Option<String>,
    /// CodeBuild build behind the version (source builds).
    pub build_arn: Option<String>,
    pub build_role: Option<String>,
    /// `(type, repository, location)` for versions built from a repo.
    pub source_build: Option<(String, String, String)>,
}

impl EbVersion {
    pub fn from_sdk(v: &aws_sdk_elasticbeanstalk::types::ApplicationVersionDescription) -> Self {
        let application = v.application_name().unwrap_or_default().to_string();
        let label = v.version_label().unwrap_or_default().to_string();
        Self {
            key: format!("{application}@{label}"),
            arn: v.application_version_arn().unwrap_or_default().to_string(),
            description: v.description().filter(|d| !d.is_empty()).map(str::to_string),
            status: v.status().map(|s| s.as_str().to_string()).unwrap_or_default(),
            created: v.date_created().map(fmt_dt),
            created_secs: v.date_created().map(|d| d.secs()).unwrap_or(0),
            source_bundle: v.source_bundle().and_then(|b| {
                Some(format!("s3://{}/{}", b.s3_bucket()?, b.s3_key().unwrap_or_default()))
            }),
            image_uri: v.image_source().and_then(|i| i.uri()).map(str::to_string),
            build_arn: v.build_arn().filter(|a| !a.is_empty()).map(str::to_string),
            build_role: v
                .image_build_configuration()
                .and_then(|c| c.code_build_service_role())
                .map(str::to_string),
            source_build: v.source_build_information().map(|s| {
                (
                    s.source_type().as_str().to_string(),
                    s.source_repository().as_str().to_string(),
                    s.source_location().to_string(),
                )
            }),
            application,
            label,
        }
    }
}

crate::sections! {
    pub enum EbVersionDetailSection,
    pub static EB_VERSION_SECTIONS = [
        Overview "Overview",
        Source "Source",
    ]
}

impl Resource for EbVersion {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&EB_VERSION_SECTIONS)
    }

    fn references(&self) -> Vec<(String, String)> {
        let mut out = vec![("Application".to_string(), self.application.clone())];
        out.extend(
            [
                ("Source Bundle", &self.source_bundle),
                ("Image", &self.image_uri),
                ("Build", &self.build_arn),
                ("Build Role", &self.build_role),
            ]
            .into_iter()
            .filter_map(|(label, v)| Some((label.to_string(), v.clone()?))),
        );
        out
    }

    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws elasticbeanstalk describe-application-versions --application-name {} --version-labels {}",
            shell_quote(&self.application),
            shell_quote(&self.label)
        ))
    }

    fn id(&self) -> &str {
        &self.key
    }

    fn name(&self) -> &str {
        &self.label
    }

    fn resource_type(&self) -> &str {
        "Beanstalk Application Version"
    }

    fn state(&self) -> ResourceState {
        match self.status.as_str() {
            "Processed" | "Unprocessed" => ResourceState::Available,
            "Processing" | "Building" => ResourceState::Pending,
            "Failed" => ResourceState::Unavailable,
            other => ResourceState::Unknown(other.to_ascii_lowercase()),
        }
    }

    fn state_label(&self) -> String {
        native_state_label(&self.status, || self.state())
    }

    fn tags(&self) -> &HashMap<String, String> {
        static EMPTY: std::sync::OnceLock<HashMap<String, String>> = std::sync::OnceLock::new();
        EMPTY.get_or_init(HashMap::new)
    }

    fn search_text(&self) -> String {
        format!(
            "{} {} {} {} {} beanstalk application version",
            self.key,
            self.status,
            self.description.as_deref().unwrap_or(""),
            self.source_bundle.as_deref().unwrap_or(""),
            self.image_uri.as_deref().unwrap_or(""),
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Application".to_string(), self.application.clone()),
            ("Version Label".to_string(), self.label.clone()),
            ("Status".to_string(), self.status.clone()),
        ]
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{region}.console.aws.amazon.com/elasticbeanstalk/home?region={region}#/application/versions?applicationName={}",
            self.application
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── Lazy section payloads + fetchers ──────────────────────────────────────────

/// `DescribeEnvironmentHealth` (attribute `All`). Needs enhanced health
/// reporting; on basic health the API errors and the pane says so.
#[derive(Debug, Clone, Default)]
pub struct EbHealth {
    pub color: Option<String>,
    pub status: Option<String>,
    pub health_status: Option<String>,
    pub causes: Vec<String>,
    pub refreshed: Option<String>,
    /// `(label, count)` per instance-health bucket, non-zero only.
    pub instances: Vec<(&'static str, i32)>,
    pub requests: Option<EbRequestMetrics>,
}

#[derive(Debug, Clone, Default)]
pub struct EbRequestMetrics {
    /// Window the counts cover, seconds.
    pub duration: Option<i32>,
    pub request_count: i32,
    pub status_2xx: Option<i32>,
    pub status_3xx: Option<i32>,
    pub status_4xx: Option<i32>,
    pub status_5xx: Option<i32>,
    pub p50: Option<f64>,
    pub p90: Option<f64>,
    pub p99: Option<f64>,
}

pub async fn fetch_health(client: EbClient, env_id: String) -> std::result::Result<EbHealth, String> {
    let out = client
        .describe_environment_health()
        .environment_id(env_id)
        .attribute_names(aws_sdk_elasticbeanstalk::types::EnvironmentHealthAttribute::All)
        .send()
        .await
        .map_err(|e| crate::error::sdk_error_message(&e))?;
    let instances = out
        .instances_health()
        .map(|h| {
            [
                ("Ok", h.ok()),
                ("Info", h.info()),
                ("Pending", h.pending()),
                ("Warning", h.warning()),
                ("Degraded", h.degraded()),
                ("Severe", h.severe()),
                ("Unknown", h.unknown()),
                ("No data", h.no_data()),
            ]
            .into_iter()
            .filter_map(|(k, v)| v.filter(|n| *n > 0).map(|n| (k, n)))
            .collect()
        })
        .unwrap_or_default();
    let requests = out.application_metrics().map(|m| {
        let codes = m.status_codes();
        let lat = m.latency();
        EbRequestMetrics {
            duration: m.duration(),
            request_count: m.request_count(),
            status_2xx: codes.and_then(|c| c.status2xx()),
            status_3xx: codes.and_then(|c| c.status3xx()),
            status_4xx: codes.and_then(|c| c.status4xx()),
            status_5xx: codes.and_then(|c| c.status5xx()),
            p50: lat.and_then(|l| l.p50()),
            p90: lat.and_then(|l| l.p90()),
            p99: lat.and_then(|l| l.p99()),
        }
    });
    Ok(EbHealth {
        color: out.color().map(str::to_string),
        status: out.status().map(|s| s.as_str().to_string()),
        health_status: out.health_status().map(str::to_string),
        causes: out.causes().to_vec(),
        refreshed: out.refreshed_at().map(fmt_dt),
        instances,
        requests,
    })
}

#[derive(Debug, Clone)]
pub struct EbEvent {
    pub time: Option<String>,
    /// `TRACE` / `DEBUG` / `INFO` / `WARN` / `ERROR` / `FATAL`.
    pub severity: String,
    pub message: String,
}

pub async fn fetch_events(client: EbClient, env_id: String) -> std::result::Result<Vec<EbEvent>, String> {
    let out = client
        .describe_events()
        .environment_id(env_id)
        .max_records(MAX_EVENTS)
        .send()
        .await
        .map_err(|e| crate::error::sdk_error_message(&e))?;
    Ok(out
        .events()
        .iter()
        .map(|e| EbEvent {
            time: e.event_date().map(fmt_dt),
            severity: e.severity().map(|s| s.as_str().to_string()).unwrap_or_default(),
            message: e.message().unwrap_or_default().to_string(),
        })
        .collect())
}

#[derive(Debug, Clone)]
pub struct EbSetting {
    pub namespace: String,
    pub option: String,
    pub value: Option<String>,
    /// Set for options scoped to one resource (an ASG trigger, a listener).
    pub resource: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct EbConfig {
    pub deployment_status: Option<String>,
    /// Sorted by namespace, then option.
    pub settings: Vec<EbSetting>,
}

pub async fn fetch_config(
    client: EbClient,
    application: String,
    env_name: String,
) -> std::result::Result<EbConfig, String> {
    let out = client
        .describe_configuration_settings()
        .application_name(application)
        .environment_name(env_name)
        .send()
        .await
        .map_err(|e| crate::error::sdk_error_message(&e))?;
    let Some(desc) = out.configuration_settings().first() else {
        return Ok(EbConfig::default());
    };
    let mut settings: Vec<EbSetting> = desc
        .option_settings()
        .iter()
        .map(|o| EbSetting {
            namespace: o.namespace().unwrap_or_default().to_string(),
            option: o.option_name().unwrap_or_default().to_string(),
            value: o.value().map(str::to_string),
            resource: o.resource_name().filter(|r| !r.is_empty()).map(str::to_string),
        })
        .collect();
    settings.sort_by(|a, b| (&a.namespace, &a.resource, &a.option).cmp(&(&b.namespace, &b.resource, &b.option)));
    Ok(EbConfig {
        deployment_status: desc.deployment_status().map(|s| s.as_str().to_string()),
        settings,
    })
}

/// `DescribeEnvironmentResources` — the live AWS resources an environment
/// runs on. Standard: ASG / instances / launch templates / load balancers /
/// queues. Cluster: the EKS cluster (and nothing EC2-shaped).
#[derive(Debug, Clone, Default)]
pub struct EbResources {
    pub cluster_arn: Option<String>,
    pub auto_scaling_groups: Vec<String>,
    pub instances: Vec<String>,
    pub launch_templates: Vec<String>,
    pub launch_configurations: Vec<String>,
    pub load_balancers: Vec<String>,
    pub triggers: Vec<String>,
    /// `(name, url)`.
    pub queues: Vec<(String, String)>,
}

impl EbResources {
    pub fn is_empty(&self) -> bool {
        self.cluster_arn.is_none()
            && self.auto_scaling_groups.is_empty()
            && self.instances.is_empty()
            && self.launch_templates.is_empty()
            && self.launch_configurations.is_empty()
            && self.load_balancers.is_empty()
            && self.triggers.is_empty()
            && self.queues.is_empty()
    }
}

pub async fn fetch_resources(client: EbClient, env_id: String) -> std::result::Result<EbResources, String> {
    let out = client
        .describe_environment_resources()
        .environment_id(env_id)
        .send()
        .await
        .map_err(|e| crate::error::sdk_error_message(&e))?;
    let Some(r) = out.environment_resources() else {
        return Ok(EbResources::default());
    };
    fn names<'a>(v: impl Iterator<Item = Option<&'a str>>) -> Vec<String> {
        v.flatten().filter(|s| !s.is_empty()).map(str::to_string).collect()
    }
    Ok(EbResources {
        cluster_arn: r.cluster().and_then(|c| c.cluster_arn()).map(str::to_string),
        auto_scaling_groups: names(r.auto_scaling_groups().iter().map(|g| g.name())),
        instances: names(r.instances().iter().map(|i| i.id())),
        launch_templates: names(r.launch_templates().iter().map(|t| t.id())),
        launch_configurations: names(r.launch_configurations().iter().map(|c| c.name())),
        load_balancers: names(r.load_balancers().iter().map(|l| l.name())),
        triggers: names(r.triggers().iter().map(|t| t.name())),
        queues: r
            .queues()
            .iter()
            .filter_map(|q| Some((q.name()?.to_string(), q.url().unwrap_or_default().to_string())))
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_sdk_elasticbeanstalk::types::{
        EnvironmentDescription, EnvironmentHealth, EnvironmentStatus, EnvironmentTier,
    };

    fn env(tier: (&str, &str), status: EnvironmentStatus, health: EnvironmentHealth) -> EbEnvironment {
        EbEnvironment::from_sdk(
            &EnvironmentDescription::builder()
                .environment_id("e-abc123")
                .environment_name("web-prod")
                .application_name("storefront")
                .tier(EnvironmentTier::builder().name(tier.0).r#type(tier.1).build())
                .status(status)
                .health(health)
                .build(),
        )
    }

    #[test]
    fn tier_decides_the_deployment_type() {
        let c = env(("Cluster", "EKS"), EnvironmentStatus::Ready, EnvironmentHealth::Green);
        assert_eq!(c.deployment, EbDeploymentType::Cluster);
        let w = env(("WebServer", "Standard"), EnvironmentStatus::Ready, EnvironmentHealth::Green);
        assert_eq!(w.deployment, EbDeploymentType::Standard);
        let q = env(("Worker", "SQS/HTTP"), EnvironmentStatus::Ready, EnvironmentHealth::Green);
        assert_eq!(q.deployment, EbDeploymentType::Standard);
    }

    #[test]
    fn state_label_is_the_health_colour_when_ready_else_the_status() {
        let red = env(("WebServer", "Standard"), EnvironmentStatus::Ready, EnvironmentHealth::Red);
        assert_eq!(red.state(), ResourceState::Unavailable);
        assert_eq!(red.state_label(), "red");
        let upd = env(("WebServer", "Standard"), EnvironmentStatus::Updating, EnvironmentHealth::Grey);
        assert_eq!(upd.state(), ResourceState::Pending);
        assert_eq!(upd.state_label(), "updating");
    }

    #[test]
    fn cluster_environment_references_its_eks_cluster() {
        let mut c = env(("Cluster", "EKS"), EnvironmentStatus::Ready, EnvironmentHealth::Green);
        c.cluster_arn = Some("arn:aws:eks:us-east-1:123456789012:cluster/eb-shared".to_string());
        assert_eq!(c.cluster_name(), Some("eb-shared"));
        assert!(c
            .references()
            .iter()
            .any(|(l, v)| l == "EKS Cluster" && crate::references::value_mentions(v, "eb-shared")));
    }

    #[test]
    fn version_ids_carry_the_application() {
        let v = EbVersion::from_sdk(
            &aws_sdk_elasticbeanstalk::types::ApplicationVersionDescription::builder()
                .application_name("storefront")
                .version_label("v42")
                .build(),
        );
        assert_eq!(v.id(), "storefront@v42");
        assert_eq!(v.name(), "v42");
    }
}
