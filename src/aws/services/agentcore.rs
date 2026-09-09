//! Amazon Bedrock **AgentCore** (`@agentcore`) — the agent *hosting* platform:
//! runtimes, gateways (MCP tool fronts), memory stores, workload identity /
//! credential providers, and the built-in browser + code-interpreter tools.
//!
//! **Not** `@bedrock` (`bedrock` + `bedrock-agent`), which is models,
//! guardrails, knowledge bases and the older Bedrock Agents. The two services
//! both say "agent" and mean different things — see the naming gotcha in
//! CLAUDE.md. Two clients here too: `bedrock-agentcore-control` (every
//! resource definition) and `bedrock-agentcore` (the data plane, used only for
//! read-only session/actor listings).
//!
//! Almost every `List*` here returns a thin summary, so the panes are lazy:
//! the list load is cheap and breadth-first, and each split-pane section pulls
//! its own `Get*` on first focus.

use std::any::Any;
use std::collections::HashMap;

use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::aws::client::AwsClients;
use crate::aws::resource::{native_state_label, shell_quote, Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::error::Result;
use crate::event::{Event, LoadProgress};

type CtlClient = aws_sdk_bedrockagentcorecontrol::Client;
type DpClient = aws_sdk_bedrockagentcore::Client;

/// Gateway targets are fetched one `GetGatewayTarget` at a time for their
/// configuration; a gateway can front many tools, so cap the deep pass.
pub const MAX_GATEWAY_TARGETS: usize = 25;
/// `ListSessions` is per-actor, so the Sessions section walks actors. Cap both
/// the actor fan-out and the rows we keep.
pub const MAX_MEMORY_ACTORS: usize = 25;
pub const MAX_MEMORY_SESSIONS: usize = 100;
/// Tool session listings are unbounded over time; keep the newest page-ish.
pub const MAX_TOOL_SESSIONS: usize = 50;
/// A payment manager's connectors need a `GetPaymentConnector` each for their
/// credential wiring; cap that second pass the way gateway targets are capped.
pub const MAX_PAYMENT_CONNECTORS: usize = 25;

/// The AgentCore console, regionalized.
///
/// **Service home only, deliberately.** AWS documents exactly one console URL
/// for this service — `https://console.aws.amazon.com/bedrock-agentcore/home#`
/// — and every console walkthrough then says "select Gateways from the left
/// navigation pane". No per-section or per-resource fragment is published
/// anywhere, and the SPA's routes are not something to reverse-engineer into
/// a shipped link.
///
/// So every AgentCore type returns the same URL. That is worth doing anyway:
/// it lands you in the right service in the right region, which beats the
/// "No console URL for this resource type" error `o` gives today. A guessed
/// `#/runtimes` that silently lands on the wrong family — or on nothing —
/// would be worse than both. Confirm a real fragment against a live console
/// and this becomes a one-line upgrade per family.
fn console_home(region: &str) -> String {
    format!(
        "https://{r}.console.aws.amazon.com/bedrock-agentcore/home?region={r}#",
        r = region
    )
}

/// Trim an `aws_smithy_types::DateTime` to a readable `YYYY-MM-DD HH:MM:SS`.
fn fmt_dt(dt: Option<&aws_smithy_types::DateTime>) -> String {
    dt.map(|t| {
        let s = t.to_string();
        s.split('.')
            .next()
            .unwrap_or(&s)
            .replace('T', " ")
            .replace('Z', "")
    })
    .unwrap_or_default()
}

/// Every AgentCore family reports a `*_STATUS` string with the same rough
/// vocabulary. `*_FAILED` must land on `Unavailable` (red) rather than
/// `Unknown` (dim) — a failed runtime is the thing you opened the list to find.
fn map_status(s: &str) -> ResourceState {
    match s.to_ascii_uppercase().as_str() {
        "READY" | "ACTIVE" | "AVAILABLE" => ResourceState::Available,
        "CREATING" => ResourceState::Creating,
        "UPDATING" | "SYNCHRONIZING" | "PENDING" => ResourceState::Pending,
        "DELETING" => ResourceState::Deleting,
        s if s.ends_with("FAILED") => ResourceState::Unavailable,
        other => ResourceState::Unknown(other.to_lowercase()),
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Service
// ═══════════════════════════════════════════════════════════════════════════════

/// Only the control plane is needed for the list load; the data-plane client
/// is built per-fetch by the lazy session triggers off `App.aws_clients`.
pub struct AgentCoreService {
    ctl: CtlClient,
}

impl AgentCoreService {
    pub fn new(clients: &AwsClients) -> Self {
        Self {
            ctl: clients.agentcore_control_client(),
        }
    }
}

#[async_trait]
impl AwsService for AgentCoreService {
    fn service_type(&self) -> ServiceType {
        ServiceType::AgentCore
    }

    fn name(&self) -> &str {
        "Bedrock AgentCore"
    }

    async fn list_resources(&self) -> Result<Vec<Box<dyn Resource>>> {
        Ok(vec![])
    }

    /// Five phases, one per sub-tab. Every phase is independent: a family that
    /// fails sends `ResourceLoadWarning` and the rest still stream (the
    /// mid-stream `ResourceLoadError` convention would clear `loading` and
    /// drop every later batch).
    async fn list_resources_streaming(
        &self,
        event_tx: mpsc::UnboundedSender<Event>,
        service_type: ServiceType,
    ) -> Result<()> {
        let mut total = 0usize;

        let emit = |batch: Vec<Box<dyn Resource>>, total: &mut usize| {
            if batch.is_empty() {
                return;
            }
            *total += batch.len();
            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                service: service_type,
                resources: batch,
                progress: LoadProgress {
                    loaded_count: *total,
                    total_count: None,
                    status_message: None,
                },
            });
        };
        let warn = |phase: &str, e: String| {
            let _ = event_tx.send(Event::ResourceLoadWarning {
                service: service_type,
                warning: format!("{}: {}", phase, e),
            });
        };

        // ── Runtimes ─────────────────────────────────────────────────────────
        let mut pager = self.ctl.list_agent_runtimes().into_paginator().send();
        loop {
            match pager.next().await {
                Some(Ok(page)) => {
                    let batch: Vec<Box<dyn Resource>> = page
                        .agent_runtimes()
                        .iter()
                        .map(|r| Box::new(AgentCoreRuntime::from_sdk(r)) as Box<dyn Resource>)
                        .collect();
                    emit(batch, &mut total);
                }
                Some(Err(e)) => {
                    warn("Runtimes", crate::error::sdk_error_message(&e));
                    break;
                }
                None => break,
            }
        }

        // ── Gateways ─────────────────────────────────────────────────────────
        let mut pager = self.ctl.list_gateways().into_paginator().send();
        loop {
            match pager.next().await {
                Some(Ok(page)) => {
                    let batch: Vec<Box<dyn Resource>> = page
                        .items()
                        .iter()
                        .map(|g| Box::new(AgentCoreGateway::from_sdk(g)) as Box<dyn Resource>)
                        .collect();
                    emit(batch, &mut total);
                }
                Some(Err(e)) => {
                    warn("Gateways", crate::error::sdk_error_message(&e));
                    break;
                }
                None => break,
            }
        }

        // ── Memory ───────────────────────────────────────────────────────────
        let mut pager = self.ctl.list_memories().into_paginator().send();
        loop {
            match pager.next().await {
                Some(Ok(page)) => {
                    let batch: Vec<Box<dyn Resource>> = page
                        .memories()
                        .iter()
                        .map(|m| Box::new(AgentCoreMemory::from_sdk(m)) as Box<dyn Resource>)
                        .collect();
                    emit(batch, &mut total);
                }
                Some(Err(e)) => {
                    warn("Memory", crate::error::sdk_error_message(&e));
                    break;
                }
                None => break,
            }
        }

        // ── Identity: both credential-provider kinds, then workload identities ─
        // Order matters here, and it is the only place in this service where
        // it does. The console's Identity page shows credential providers
        // only; we also list workload identities, because AgentCore mints one
        // per runtime / gateway / payment manager and those panes link
        // straight to it by ARN. Emitting the providers first means the tab
        // still opens on the handful of things you actually configured, with
        // the auto-provisioned identities below rather than buried under them.
        let mut pager = self
            .ctl
            .list_oauth2_credential_providers()
            .into_paginator()
            .send();
        loop {
            match pager.next().await {
                Some(Ok(page)) => {
                    let batch: Vec<Box<dyn Resource>> = page
                        .credential_providers()
                        .iter()
                        .map(|p| Box::new(AgentCoreOAuth2Provider::from_sdk(p)) as Box<dyn Resource>)
                        .collect();
                    emit(batch, &mut total);
                }
                Some(Err(e)) => {
                    warn("OAuth2 Providers", crate::error::sdk_error_message(&e));
                    break;
                }
                None => break,
            }
        }

        let mut pager = self
            .ctl
            .list_api_key_credential_providers()
            .into_paginator()
            .send();
        loop {
            match pager.next().await {
                Some(Ok(page)) => {
                    let batch: Vec<Box<dyn Resource>> = page
                        .credential_providers()
                        .iter()
                        .map(|p| Box::new(AgentCoreApiKeyProvider::from_sdk(p)) as Box<dyn Resource>)
                        .collect();
                    emit(batch, &mut total);
                }
                Some(Err(e)) => {
                    warn("API Key Providers", crate::error::sdk_error_message(&e));
                    break;
                }
                None => break,
            }
        }

        let mut pager = self.ctl.list_workload_identities().into_paginator().send();
        loop {
            match pager.next().await {
                Some(Ok(page)) => {
                    let batch: Vec<Box<dyn Resource>> = page
                        .workload_identities()
                        .iter()
                        .map(|w| {
                            Box::new(AgentCoreWorkloadIdentity::from_sdk(w)) as Box<dyn Resource>
                        })
                        .collect();
                    emit(batch, &mut total);
                }
                Some(Err(e)) => {
                    warn("Workload Identities", crate::error::sdk_error_message(&e));
                    break;
                }
                None => break,
            }
        }

        // ── Tools: browsers + browser profiles + code interpreters ───────────
        // The `*_summaries` here include the AWS-managed defaults
        // (`aws.browser.v1` / `aws.codeinterpreter.v1`) alongside custom ones;
        // both are real and both are shown.
        let mut pager = self.ctl.list_browsers().into_paginator().send();
        loop {
            match pager.next().await {
                Some(Ok(page)) => {
                    let batch: Vec<Box<dyn Resource>> = page
                        .browser_summaries()
                        .iter()
                        .map(|b| Box::new(AgentCoreBrowser::from_sdk(b)) as Box<dyn Resource>)
                        .collect();
                    emit(batch, &mut total);
                }
                Some(Err(e)) => {
                    warn("Browsers", crate::error::sdk_error_message(&e));
                    break;
                }
                None => break,
            }
        }

        let mut pager = self.ctl.list_browser_profiles().into_paginator().send();
        loop {
            match pager.next().await {
                Some(Ok(page)) => {
                    let batch: Vec<Box<dyn Resource>> = page
                        .profile_summaries()
                        .iter()
                        .map(|p| Box::new(AgentCoreBrowserProfile::from_sdk(p)) as Box<dyn Resource>)
                        .collect();
                    emit(batch, &mut total);
                }
                Some(Err(e)) => {
                    warn("Browser Profiles", crate::error::sdk_error_message(&e));
                    break;
                }
                None => break,
            }
        }

        let mut pager = self.ctl.list_code_interpreters().into_paginator().send();
        loop {
            match pager.next().await {
                Some(Ok(page)) => {
                    let batch: Vec<Box<dyn Resource>> = page
                        .code_interpreter_summaries()
                        .iter()
                        .map(|c| {
                            Box::new(AgentCoreCodeInterpreter::from_sdk(c)) as Box<dyn Resource>
                        })
                        .collect();
                    emit(batch, &mut total);
                }
                Some(Err(e)) => {
                    warn("Code Interpreters", crate::error::sdk_error_message(&e));
                    break;
                }
                None => break,
            }
        }

        // ── Policy / Evaluation / Registry (preview) ────────────────────────
        // These stay **silent** on failure — no `warn`. They are preview
        // surfaces most accounts have never enabled, so AccessDenied or a
        // missing endpoint is the norm rather than something unusual, and a
        // warning on every load everywhere would be pure noise. The per-tab
        // empty state in resource_list.rs carries the explanation instead.
        // (This is the deliberate other side of the warn-vs-silent rule from
        // CLAUDE.md; the five families above take the warn side.)
        let mut pager = self.ctl.list_policy_engines().into_paginator().send();
        while let Some(Ok(page)) = pager.next().await {
            let batch: Vec<Box<dyn Resource>> = page
                .policy_engines()
                .iter()
                .map(|e| Box::new(AgentCorePolicyEngine::from_sdk(e)) as Box<dyn Resource>)
                .collect();
            emit(batch, &mut total);
        }

        let mut pager = self.ctl.list_policies().into_paginator().send();
        while let Some(Ok(page)) = pager.next().await {
            let batch: Vec<Box<dyn Resource>> = page
                .policies()
                .iter()
                .map(|p| Box::new(AgentCorePolicy::from_sdk(p)) as Box<dyn Resource>)
                .collect();
            emit(batch, &mut total);
        }

        let mut pager = self.ctl.list_evaluators().into_paginator().send();
        while let Some(Ok(page)) = pager.next().await {
            let batch: Vec<Box<dyn Resource>> = page
                .evaluators()
                .iter()
                .map(|e| Box::new(AgentCoreEvaluator::from_sdk(e)) as Box<dyn Resource>)
                .collect();
            emit(batch, &mut total);
        }

        let mut pager = self
            .ctl
            .list_online_evaluation_configs()
            .into_paginator()
            .send();
        while let Some(Ok(page)) = pager.next().await {
            let batch: Vec<Box<dyn Resource>> = page
                .online_evaluation_configs()
                .iter()
                .map(|c| Box::new(AgentCoreOnlineEval::from_sdk(c)) as Box<dyn Resource>)
                .collect();
            emit(batch, &mut total);
        }

        let mut pager = self.ctl.list_datasets().into_paginator().send();
        while let Some(Ok(page)) = pager.next().await {
            let batch: Vec<Box<dyn Resource>> = page
                .datasets()
                .iter()
                .map(|d| Box::new(AgentCoreDataset::from_sdk(d)) as Box<dyn Resource>)
                .collect();
            emit(batch, &mut total);
        }

        let mut pager = self.ctl.list_registries().into_paginator().send();
        while let Some(Ok(page)) = pager.next().await {
            let batch: Vec<Box<dyn Resource>> = page
                .registries()
                .iter()
                .map(|r| Box::new(AgentCoreRegistry::from_sdk(r)) as Box<dyn Resource>)
                .collect();
            emit(batch, &mut total);
        }

        // ── Harness / Configuration Bundles / Payments (preview) ────────────
        // Same silent treatment as the block above, and for the same reason:
        // these are newer than the five warned families and absent from most
        // accounts and regions, so a per-load warning would fire everywhere and
        // mean nothing. The empty-state hints carry the explanation.
        let mut pager = self.ctl.list_harnesses().into_paginator().send();
        while let Some(Ok(page)) = pager.next().await {
            let batch: Vec<Box<dyn Resource>> = page
                .harnesses()
                .iter()
                .map(|h| Box::new(AgentCoreHarness::from_sdk(h)) as Box<dyn Resource>)
                .collect();
            emit(batch, &mut total);
        }

        let mut pager = self.ctl.list_configuration_bundles().into_paginator().send();
        while let Some(Ok(page)) = pager.next().await {
            let batch: Vec<Box<dyn Resource>> = page
                .bundles()
                .iter()
                .map(|b| Box::new(AgentCoreConfigBundle::from_sdk(b)) as Box<dyn Resource>)
                .collect();
            emit(batch, &mut total);
        }

        let mut pager = self.ctl.list_payment_managers().into_paginator().send();
        while let Some(Ok(page)) = pager.next().await {
            let batch: Vec<Box<dyn Resource>> = page
                .payment_managers()
                .iter()
                .map(|m| Box::new(AgentCorePaymentManager::from_sdk(m)) as Box<dyn Resource>)
                .collect();
            emit(batch, &mut total);
        }

        let mut pager = self
            .ctl
            .list_payment_credential_providers()
            .into_paginator()
            .send();
        while let Some(Ok(page)) = pager.next().await {
            let batch: Vec<Box<dyn Resource>> = page
                .credential_providers()
                .iter()
                .map(|p| Box::new(AgentCorePaymentCredProvider::from_sdk(p)) as Box<dyn Resource>)
                .collect();
            emit(batch, &mut total);
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

// ═══════════════════════════════════════════════════════════════════════════════
// Runtime
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct AgentCoreRuntime {
    pub runtime_id: String,
    pub arn: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub status: String,
    pub last_updated: String,
    pub tags: HashMap<String, String>,
}

impl AgentCoreRuntime {
    pub fn from_sdk(r: &aws_sdk_bedrockagentcorecontrol::types::AgentRuntime) -> Self {
        Self {
            runtime_id: r.agent_runtime_id().to_string(),
            arn: r.agent_runtime_arn().to_string(),
            name: r.agent_runtime_name().to_string(),
            version: r.agent_runtime_version().to_string(),
            description: r.description().to_string(),
            status: r.status().as_str().to_string(),
            last_updated: fmt_dt(Some(r.last_updated_at())),
            tags: HashMap::new(),
        }
    }

    /// The CloudWatch Logs group AgentCore writes an endpoint's application
    /// logs and spans to. Deterministic — no resolve round-trip needed, unlike
    /// ECS/NFW tails.
    pub fn log_group_for(&self, endpoint: &str) -> String {
        format!(
            "/aws/bedrock-agentcore/runtimes/{}-{}",
            self.runtime_id, endpoint
        )
    }
}

crate::sections! {
    pub enum AgentCoreRuntimeDetailSection,
    pub static AGENTCORE_RUNTIME_SECTIONS = [
        Overview "Overview" => crate::app::App::trigger_agentcore_runtime_detail_load,
        Artifact "Artifact" => crate::app::App::trigger_agentcore_runtime_detail_load,
        Network "Network" => crate::app::App::trigger_agentcore_runtime_detail_load,
        Auth "Auth" => crate::app::App::trigger_agentcore_runtime_detail_load,
        ResourcePolicy "Resource Policy" => crate::app::App::trigger_agentcore_resource_policy_load,
        Env "Env" => crate::app::App::trigger_agentcore_runtime_detail_load,
        Endpoints "Endpoints" => crate::app::App::trigger_agentcore_runtime_endpoints_load,
        Versions "Versions" => crate::app::App::trigger_agentcore_runtime_versions_load,
        // Deliberately hookless — see `fetch_agent_card`. This is the only
        // section in the app that does not fetch on entry, because the fetch
        // reaches the running agent container.
        AgentCard "Agent Card",
        Tags "Tags" => crate::app::App::trigger_agentcore_tags_load,
    ]
}

impl Resource for AgentCoreRuntime {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&AGENTCORE_RUNTIME_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.runtime_id
    }
    fn name(&self) -> &str {
        if self.name.is_empty() {
            &self.runtime_id
        } else {
            &self.name
        }
    }
    fn resource_type(&self) -> &str {
        "AgentCore Runtime"
    }
    fn state(&self) -> ResourceState {
        map_status(&self.status)
    }

    fn state_label(&self) -> String {
        native_state_label(&self.status, || self.state())
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        format!(
            "{} {} {} {} {}",
            self.runtime_id, self.name, self.status, self.version, self.description
        )
    }
    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Name".to_string(), self.name.clone()),
            ("ID".to_string(), self.runtime_id.clone()),
            ("ARN".to_string(), self.arn.clone()),
            ("Version".to_string(), self.version.clone()),
            ("Status".to_string(), self.status.clone()),
            ("Description".to_string(), self.description.clone()),
            ("Last Updated".to_string(), self.last_updated.clone()),
        ]
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws bedrock-agentcore-control get-agent-runtime --agent-runtime-id {}",
            shell_quote(&self.runtime_id)
        ))
    }
    /// `W` lookup keys. CloudTrail's `ResourceName` for AgentCore may be
    /// either the id or the ARN depending on the event, and keys are tried in
    /// order until one returns events — so carry both (the IAM precedent).
    fn trail_lookup_keys(&self) -> Vec<String> {
        vec![self.runtime_id.clone(), self.arn.clone(), self.name.clone()]
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(console_home(region))
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// `GetAgentRuntime` — everything `ListAgentRuntimes` leaves out. One fetch
/// feeds the Overview / Artifact / Network / Auth / Env sections.
#[derive(Debug, Clone, Default)]
pub struct AgentCoreRuntimeDetail {
    pub role_arn: String,
    pub created: String,
    pub failure_reason: String,
    pub workload_identity_arn: String,
    /// "Container" / "Code", plus its one identifying field (image URI …).
    pub artifact_kind: String,
    pub artifact_value: String,
    pub server_protocol: String,
    pub idle_session_timeout: Option<i32>,
    pub max_lifetime: Option<i32>,
    pub network_mode: String,
    pub subnets: Vec<String>,
    pub security_groups: Vec<String>,
    pub require_s3_endpoint: Option<bool>,
    /// Flattened `CustomJwtAuthorizerConfiguration`, when that's the authorizer.
    pub authorizer_kind: String,
    pub jwt_discovery_url: String,
    pub jwt_allowed_audience: Vec<String>,
    pub jwt_allowed_clients: Vec<String>,
    pub jwt_allowed_scopes: Vec<String>,
    pub request_header_allowlist: Vec<String>,
    /// Sorted so the pane is stable across fetches (the API returns a map).
    pub environment_variables: Vec<(String, String)>,
    pub filesystem_kinds: Vec<String>,
}

pub async fn fetch_runtime_detail(
    ctl: CtlClient,
    runtime_id: String,
) -> std::result::Result<AgentCoreRuntimeDetail, String> {
    use aws_sdk_bedrockagentcorecontrol::types::{
        AgentRuntimeArtifact, AuthorizerConfiguration, FilesystemConfiguration,
        RequestHeaderConfiguration,
    };

    let r = ctl
        .get_agent_runtime()
        .agent_runtime_id(&runtime_id)
        .send()
        .await
        .map_err(|e| crate::error::sdk_error_message(&e))?;

    let mut d = AgentCoreRuntimeDetail {
        role_arn: r.role_arn().to_string(),
        created: fmt_dt(Some(r.created_at())),
        failure_reason: r.failure_reason().unwrap_or_default().to_string(),
        workload_identity_arn: r
            .workload_identity_details()
            .map(|w| w.workload_identity_arn().to_string())
            .unwrap_or_default(),
        ..Default::default()
    };

    match r.agent_runtime_artifact() {
        Some(AgentRuntimeArtifact::ContainerConfiguration(c)) => {
            d.artifact_kind = "Container".to_string();
            d.artifact_value = c.container_uri().to_string();
        }
        Some(AgentRuntimeArtifact::CodeConfiguration(_)) => {
            d.artifact_kind = "Code".to_string();
        }
        _ => {}
    }

    if let Some(p) = r.protocol_configuration() {
        d.server_protocol = p.server_protocol().as_str().to_string();
    }
    if let Some(l) = r.lifecycle_configuration() {
        d.idle_session_timeout = l.idle_runtime_session_timeout();
        d.max_lifetime = l.max_lifetime();
    }
    if let Some(n) = r.network_configuration() {
        d.network_mode = n.network_mode().as_str().to_string();
        if let Some(v) = n.network_mode_config() {
            d.subnets = v.subnets().to_vec();
            d.security_groups = v.security_groups().to_vec();
            d.require_s3_endpoint = v.require_service_s3_endpoint();
        }
    }
    if let Some(AuthorizerConfiguration::CustomJwtAuthorizer(j)) = r.authorizer_configuration() {
        d.authorizer_kind = "Custom JWT".to_string();
        d.jwt_discovery_url = j.discovery_url().to_string();
        d.jwt_allowed_audience = j.allowed_audience().to_vec();
        d.jwt_allowed_clients = j.allowed_clients().to_vec();
        d.jwt_allowed_scopes = j.allowed_scopes().to_vec();
    }
    if let Some(RequestHeaderConfiguration::RequestHeaderAllowlist(h)) =
        r.request_header_configuration()
    {
        d.request_header_allowlist = h.clone();
    }
    if let Some(envs) = r.environment_variables() {
        let mut v: Vec<(String, String)> =
            envs.iter().map(|(k, val)| (k.clone(), val.clone())).collect();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        d.environment_variables = v;
    }
    for f in r.filesystem_configurations() {
        d.filesystem_kinds.push(
            match f {
                FilesystemConfiguration::EfsAccessPoint(_) => "EFS access point",
                FilesystemConfiguration::S3FilesAccessPoint(_) => "S3 files access point",
                FilesystemConfiguration::SessionStorage(_) => "Session storage",
                _ => "unknown",
            }
            .to_string(),
        );
    }

    Ok(d)
}

#[derive(Debug, Clone)]
pub struct AgentCoreEndpoint {
    pub name: String,
    pub id: String,
    pub arn: String,
    pub status: String,
    pub live_version: String,
    pub target_version: String,
    pub description: String,
    pub last_updated: String,
}

pub async fn fetch_runtime_endpoints(
    ctl: CtlClient,
    runtime_id: String,
) -> std::result::Result<Vec<AgentCoreEndpoint>, String> {
    let mut out = Vec::new();
    let mut pager = ctl
        .list_agent_runtime_endpoints()
        .agent_runtime_id(&runtime_id)
        .into_paginator()
        .send();
    while let Some(page) = pager.next().await {
        let page = page.map_err(|e| crate::error::sdk_error_message(&e))?;
        for e in page.runtime_endpoints() {
            out.push(AgentCoreEndpoint {
                name: e.name().to_string(),
                id: e.id().to_string(),
                arn: e.agent_runtime_endpoint_arn().to_string(),
                status: e.status().as_str().to_string(),
                live_version: e.live_version().unwrap_or_default().to_string(),
                target_version: e.target_version().unwrap_or_default().to_string(),
                description: e.description().unwrap_or_default().to_string(),
                last_updated: fmt_dt(Some(e.last_updated_at())),
            });
        }
    }
    Ok(out)
}

#[derive(Debug, Clone)]
pub struct AgentCoreRuntimeVersion {
    pub version: String,
    pub status: String,
    pub description: String,
    pub last_updated: String,
}

pub async fn fetch_runtime_versions(
    ctl: CtlClient,
    runtime_id: String,
) -> std::result::Result<Vec<AgentCoreRuntimeVersion>, String> {
    let mut out = Vec::new();
    let mut pager = ctl
        .list_agent_runtime_versions()
        .agent_runtime_id(&runtime_id)
        .into_paginator()
        .send();
    while let Some(page) = pager.next().await {
        let page = page.map_err(|e| crate::error::sdk_error_message(&e))?;
        for v in page.agent_runtimes() {
            out.push(AgentCoreRuntimeVersion {
                version: v.agent_runtime_version().to_string(),
                status: v.status().as_str().to_string(),
                description: v.description().to_string(),
                last_updated: fmt_dt(Some(v.last_updated_at())),
            });
        }
    }
    // Newest first — versions are monotonic integers as strings, so compare
    // numerically and fall back to lexical for anything unexpected.
    out.sort_by(|a, b| match (a.version.parse::<u64>(), b.version.parse::<u64>()) {
        (Ok(x), Ok(y)) => y.cmp(&x),
        _ => b.version.cmp(&a.version),
    });
    Ok(out)
}

// ═══════════════════════════════════════════════════════════════════════════════
// Gateway
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct AgentCoreGateway {
    pub gateway_id: String,
    pub name: String,
    pub status: String,
    pub description: String,
    pub protocol_type: String,
    pub authorizer_type: String,
    pub created: String,
    pub updated: String,
    pub tags: HashMap<String, String>,
}

impl AgentCoreGateway {
    pub fn from_sdk(g: &aws_sdk_bedrockagentcorecontrol::types::GatewaySummary) -> Self {
        Self {
            gateway_id: g.gateway_id().to_string(),
            name: g.name().to_string(),
            status: g.status().as_str().to_string(),
            description: g.description().unwrap_or_default().to_string(),
            protocol_type: g.protocol_type().as_str().to_string(),
            authorizer_type: g.authorizer_type().as_str().to_string(),
            created: fmt_dt(Some(g.created_at())),
            updated: fmt_dt(Some(g.updated_at())),
            tags: HashMap::new(),
        }
    }

    /// Gateway application logs land in a vended-logs group keyed by id — only
    /// present once observability is enabled on the gateway.
    pub fn log_group(&self) -> String {
        format!(
            "/aws/vendedlogs/bedrock-agentcore/gateway/APPLICATION_LOGS/{}",
            self.gateway_id
        )
    }
}

crate::sections! {
    pub enum AgentCoreGatewayDetailSection,
    pub static AGENTCORE_GATEWAY_SECTIONS = [
        Overview "Overview" => crate::app::App::trigger_agentcore_gateway_detail_load,
        Auth "Auth" => crate::app::App::trigger_agentcore_gateway_detail_load,
        ResourcePolicy "Resource Policy" => crate::app::App::trigger_agentcore_resource_policy_load,
        Targets "Targets" => crate::app::App::trigger_agentcore_gateway_targets_load,
        Rules "Rules" => crate::app::App::trigger_agentcore_gateway_rules_load,
        Security "Security" => crate::app::App::trigger_agentcore_gateway_detail_load,
        Interceptors "Interceptors" => crate::app::App::trigger_agentcore_gateway_detail_load,
        Tags "Tags" => crate::app::App::trigger_agentcore_tags_load,
    ]
}

impl Resource for AgentCoreGateway {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&AGENTCORE_GATEWAY_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.gateway_id
    }
    fn name(&self) -> &str {
        if self.name.is_empty() {
            &self.gateway_id
        } else {
            &self.name
        }
    }
    fn resource_type(&self) -> &str {
        "AgentCore Gateway"
    }
    fn state(&self) -> ResourceState {
        map_status(&self.status)
    }

    fn state_label(&self) -> String {
        native_state_label(&self.status, || self.state())
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        format!(
            "{} {} {} {} {} {}",
            self.gateway_id,
            self.name,
            self.status,
            self.protocol_type,
            self.authorizer_type,
            self.description
        )
    }
    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Name".to_string(), self.name.clone()),
            ("ID".to_string(), self.gateway_id.clone()),
            ("Status".to_string(), self.status.clone()),
            ("Protocol".to_string(), self.protocol_type.clone()),
            ("Authorizer".to_string(), self.authorizer_type.clone()),
            ("Description".to_string(), self.description.clone()),
            ("Created".to_string(), self.created.clone()),
            ("Updated".to_string(), self.updated.clone()),
        ]
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws bedrock-agentcore-control get-gateway --gateway-identifier {}",
            shell_quote(&self.gateway_id)
        ))
    }
    /// `W` lookup keys. CloudTrail's `ResourceName` for AgentCore may be
    /// either the id or the ARN depending on the event, and keys are tried in
    /// order until one returns events — so carry both (the IAM precedent).
    fn trail_lookup_keys(&self) -> Vec<String> {
        vec![self.gateway_id.clone(), self.name.clone()]
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(console_home(region))
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Debug, Clone, Default)]
pub struct AgentCoreGatewayDetail {
    pub arn: String,
    pub url: String,
    pub role_arn: String,
    pub status_reasons: Vec<String>,
    pub workload_identity_arn: String,
    pub exception_level: String,
    pub kms_key_arn: String,
    pub web_acl_arn: String,
    pub waf_failure_mode: String,
    pub policy_engine_arn: String,
    pub policy_engine_mode: String,
    pub authorizer_kind: String,
    pub jwt_discovery_url: String,
    pub jwt_allowed_audience: Vec<String>,
    pub jwt_allowed_clients: Vec<String>,
    pub jwt_allowed_scopes: Vec<String>,
    /// Flattened `McpGatewayConfiguration`.
    pub mcp_supported_versions: Vec<String>,
    pub mcp_instructions: String,
    pub mcp_search_type: String,
    pub custom_transform: bool,
    /// One row per interceptor: (interception points, description).
    pub interceptors: Vec<(String, String)>,
}

pub async fn fetch_gateway_detail(
    ctl: CtlClient,
    gateway_id: String,
) -> std::result::Result<AgentCoreGatewayDetail, String> {
    use aws_sdk_bedrockagentcorecontrol::types::{
        AuthorizerConfiguration, GatewayProtocolConfiguration,
    };

    let g = ctl
        .get_gateway()
        .gateway_identifier(&gateway_id)
        .send()
        .await
        .map_err(|e| crate::error::sdk_error_message(&e))?;

    let mut d = AgentCoreGatewayDetail {
        arn: g.gateway_arn().to_string(),
        url: g.gateway_url().unwrap_or_default().to_string(),
        role_arn: g.role_arn().unwrap_or_default().to_string(),
        status_reasons: g.status_reasons().to_vec(),
        workload_identity_arn: g
            .workload_identity_details()
            .map(|w| w.workload_identity_arn().to_string())
            .unwrap_or_default(),
        exception_level: g
            .exception_level()
            .map(|e| e.as_str().to_string())
            .unwrap_or_default(),
        kms_key_arn: g.kms_key_arn().unwrap_or_default().to_string(),
        web_acl_arn: g.web_acl_arn().unwrap_or_default().to_string(),
        custom_transform: g.custom_transform_configuration().is_some(),
        ..Default::default()
    };

    if let Some(w) = g.waf_configuration() {
        d.waf_failure_mode = w
            .failure_mode()
            .map(|f| f.as_str().to_string())
            .unwrap_or_default();
    }
    if let Some(p) = g.policy_engine_configuration() {
        d.policy_engine_arn = p.arn().to_string();
        d.policy_engine_mode = p.mode().as_str().to_string();
    }
    if let Some(AuthorizerConfiguration::CustomJwtAuthorizer(j)) = g.authorizer_configuration() {
        d.authorizer_kind = "Custom JWT".to_string();
        d.jwt_discovery_url = j.discovery_url().to_string();
        d.jwt_allowed_audience = j.allowed_audience().to_vec();
        d.jwt_allowed_clients = j.allowed_clients().to_vec();
        d.jwt_allowed_scopes = j.allowed_scopes().to_vec();
    }
    if let Some(GatewayProtocolConfiguration::Mcp(m)) = g.protocol_configuration() {
        d.mcp_supported_versions = m.supported_versions().to_vec();
        d.mcp_instructions = m.instructions().unwrap_or_default().to_string();
        d.mcp_search_type = m
            .search_type()
            .map(|s| s.as_str().to_string())
            .unwrap_or_default();
    }
    for i in g.interceptor_configurations() {
        let points = i
            .interception_points()
            .iter()
            .map(|p| p.as_str().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        d.interceptors.push((
            points,
            if i.interceptor().is_some() {
                "configured".to_string()
            } else {
                String::new()
            },
        ));
    }

    Ok(d)
}

#[derive(Debug, Clone)]
pub struct AgentCoreTarget {
    pub target_id: String,
    pub name: String,
    pub status: String,
    pub description: String,
    pub target_type: String,
    pub listing_mode: String,
    pub priority: Option<i32>,
    pub last_synchronized: String,
    pub updated: String,
    /// Which concrete backend the target fronts, flattened from the nested
    /// `TargetConfiguration` union — "Lambda", "OpenAPI schema", "MCP server"…
    pub backend_kind: String,
    /// The backend's identifying value where the union carries one (a Lambda
    /// ARN, an API Gateway rest-api id) — jumpable via the generic classifier.
    pub backend_value: String,
    pub credential_provider_types: Vec<String>,
    pub status_reasons: Vec<String>,
}

/// `ListGatewayTargets` gives shape but not backend config, so each target is
/// deepened with `GetGatewayTarget`. Capped at [`MAX_GATEWAY_TARGETS`] — a
/// gateway can front hundreds of tools and this is a detail section, not the
/// list.
pub async fn fetch_gateway_targets(
    ctl: CtlClient,
    gateway_id: String,
) -> std::result::Result<Vec<AgentCoreTarget>, String> {
    use aws_sdk_bedrockagentcorecontrol::types::{McpTargetConfiguration, TargetConfiguration};

    let mut summaries = Vec::new();
    let mut pager = ctl
        .list_gateway_targets()
        .gateway_identifier(&gateway_id)
        .into_paginator()
        .send();
    while let Some(page) = pager.next().await {
        let page = page.map_err(|e| crate::error::sdk_error_message(&e))?;
        for t in page.items() {
            summaries.push(AgentCoreTarget {
                target_id: t.target_id().to_string(),
                name: t.name().to_string(),
                status: t.status().as_str().to_string(),
                description: t.description().unwrap_or_default().to_string(),
                target_type: t
                    .target_type()
                    .map(|v| v.as_str().to_string())
                    .unwrap_or_default(),
                listing_mode: t
                    .listing_mode()
                    .map(|v| v.as_str().to_string())
                    .unwrap_or_default(),
                priority: t.resource_priority(),
                last_synchronized: fmt_dt(t.last_synchronized_at()),
                updated: fmt_dt(Some(t.updated_at())),
                backend_kind: String::new(),
                backend_value: String::new(),
                credential_provider_types: Vec::new(),
                status_reasons: Vec::new(),
            });
            if summaries.len() >= MAX_GATEWAY_TARGETS {
                break;
            }
        }
        if summaries.len() >= MAX_GATEWAY_TARGETS {
            break;
        }
    }

    for t in summaries.iter_mut() {
        let Ok(full) = ctl
            .get_gateway_target()
            .gateway_identifier(&gateway_id)
            .target_id(&t.target_id)
            .send()
            .await
        else {
            // A single target that won't describe shouldn't blank the section.
            continue;
        };
        t.status_reasons = full.status_reasons().to_vec();
        t.credential_provider_types = full
            .credential_provider_configurations()
            .iter()
            .map(|c| c.credential_provider_type().as_str().to_string())
            .collect();
        match full.target_configuration() {
            Some(TargetConfiguration::Mcp(m)) => match m {
                McpTargetConfiguration::Lambda(l) => {
                    t.backend_kind = "Lambda".to_string();
                    t.backend_value = l.lambda_arn().to_string();
                }
                McpTargetConfiguration::ApiGateway(a) => {
                    t.backend_kind = "API Gateway".to_string();
                    t.backend_value = format!("{} (stage {})", a.rest_api_id(), a.stage());
                }
                McpTargetConfiguration::OpenApiSchema(_) => {
                    t.backend_kind = "OpenAPI schema".to_string();
                }
                McpTargetConfiguration::SmithyModel(_) => {
                    t.backend_kind = "Smithy model".to_string();
                }
                McpTargetConfiguration::McpServer(_) => {
                    t.backend_kind = "MCP server".to_string();
                }
                McpTargetConfiguration::Connector(_) => {
                    t.backend_kind = "Connector".to_string();
                }
                _ => t.backend_kind = "MCP".to_string(),
            },
            Some(TargetConfiguration::Http(_)) => t.backend_kind = "HTTP".to_string(),
            Some(TargetConfiguration::Inference(_)) => t.backend_kind = "Inference".to_string(),
            _ => {}
        }
    }

    Ok(summaries)
}

#[derive(Debug, Clone)]
pub struct AgentCoreGatewayRule {
    pub rule_id: String,
    pub priority: i32,
    pub status: String,
    pub description: String,
    pub condition_count: usize,
    pub action_count: usize,
    pub system_managed: bool,
    pub created: String,
}

pub async fn fetch_gateway_rules(
    ctl: CtlClient,
    gateway_id: String,
) -> std::result::Result<Vec<AgentCoreGatewayRule>, String> {
    let mut out = Vec::new();
    let mut pager = ctl
        .list_gateway_rules()
        .gateway_identifier(&gateway_id)
        .into_paginator()
        .send();
    while let Some(page) = pager.next().await {
        let page = page.map_err(|e| crate::error::sdk_error_message(&e))?;
        for r in page.gateway_rules() {
            out.push(AgentCoreGatewayRule {
                rule_id: r.rule_id().to_string(),
                priority: r.priority(),
                status: r.status().as_str().to_string(),
                description: r.description().unwrap_or_default().to_string(),
                condition_count: r.conditions().len(),
                action_count: r.actions().len(),
                system_managed: r.system().is_some(),
                created: fmt_dt(Some(r.created_at())),
            });
        }
    }
    out.sort_by_key(|r| r.priority);
    Ok(out)
}

// ═══════════════════════════════════════════════════════════════════════════════
// Memory
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct AgentCoreMemory {
    pub memory_id: String,
    pub arn: String,
    pub status: String,
    pub created: String,
    pub updated: String,
    /// Set when the store was created *for* another resource (a runtime), not
    /// standalone — jumpable back to its owner.
    pub managed_by: String,
    pub tags: HashMap<String, String>,
}

impl AgentCoreMemory {
    pub fn from_sdk(m: &aws_sdk_bedrockagentcorecontrol::types::MemorySummary) -> Self {
        Self {
            memory_id: m.id().unwrap_or_default().to_string(),
            arn: m.arn().unwrap_or_default().to_string(),
            status: m
                .status()
                .map(|s| s.as_str().to_string())
                .unwrap_or_default(),
            created: fmt_dt(Some(m.created_at())),
            updated: fmt_dt(Some(m.updated_at())),
            managed_by: m.managed_by_resource_arn().unwrap_or_default().to_string(),
            tags: HashMap::new(),
        }
    }

    pub fn log_group(&self) -> String {
        format!(
            "/aws/vendedlogs/bedrock-agentcore/memory/APPLICATION_LOGS/{}",
            self.memory_id
        )
    }
}

crate::sections! {
    pub enum AgentCoreMemoryDetailSection,
    pub static AGENTCORE_MEMORY_SECTIONS = [
        Overview "Overview" => crate::app::App::trigger_agentcore_memory_detail_load,
        Strategies "Strategies" => crate::app::App::trigger_agentcore_memory_detail_load,
        Indexing "Indexing" => crate::app::App::trigger_agentcore_memory_detail_load,
        Actors "Actors" => crate::app::App::trigger_agentcore_memory_actors_load,
        Sessions "Sessions" => crate::app::App::trigger_agentcore_memory_sessions_load,
        Tags "Tags" => crate::app::App::trigger_agentcore_tags_load,
    ]
}

impl Resource for AgentCoreMemory {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&AGENTCORE_MEMORY_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.memory_id
    }
    fn name(&self) -> &str {
        &self.memory_id
    }
    fn resource_type(&self) -> &str {
        "AgentCore Memory"
    }
    fn state(&self) -> ResourceState {
        map_status(&self.status)
    }

    fn state_label(&self) -> String {
        native_state_label(&self.status, || self.state())
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        format!("{} {} {}", self.memory_id, self.status, self.managed_by)
    }
    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("ID".to_string(), self.memory_id.clone()),
            ("ARN".to_string(), self.arn.clone()),
            ("Status".to_string(), self.status.clone()),
            ("Created".to_string(), self.created.clone()),
            ("Updated".to_string(), self.updated.clone()),
            ("Managed By".to_string(), self.managed_by.clone()),
        ]
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws bedrock-agentcore-control get-memory --memory-id {}",
            shell_quote(&self.memory_id)
        ))
    }
    /// `W` lookup keys. CloudTrail's `ResourceName` for AgentCore may be
    /// either the id or the ARN depending on the event, and keys are tried in
    /// order until one returns events — so carry both (the IAM precedent).
    fn trail_lookup_keys(&self) -> Vec<String> {
        vec![self.memory_id.clone(), self.arn.clone()]
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(console_home(region))
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Debug, Clone)]
pub struct AgentCoreMemoryStrategy {
    pub strategy_id: String,
    pub name: String,
    pub description: String,
    pub status: String,
    pub namespaces: Vec<String>,
    pub namespace_templates: Vec<String>,
    /// Which of the four `StrategyConfiguration` arms are populated. Flatten
    /// **all** of them — a strategy commonly sets extraction *and*
    /// consolidation, and first-match-win would under-report it.
    pub configured: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct AgentCoreMemoryDetail {
    pub name: String,
    pub description: String,
    pub encryption_key_arn: String,
    pub execution_role_arn: String,
    pub event_expiry_days: i32,
    pub failure_reason: String,
    pub strategies: Vec<AgentCoreMemoryStrategy>,
    pub indexed_keys: Vec<String>,
    pub stream_delivery_count: usize,
}

pub async fn fetch_memory_detail(
    ctl: CtlClient,
    memory_id: String,
) -> std::result::Result<AgentCoreMemoryDetail, String> {
    let resp = ctl
        .get_memory()
        .memory_id(&memory_id)
        .send()
        .await
        .map_err(|e| crate::error::sdk_error_message(&e))?;
    let Some(m) = resp.memory() else {
        return Ok(AgentCoreMemoryDetail::default());
    };

    let strategies = m
        .strategies()
        .iter()
        .map(|s| {
            let mut configured = Vec::new();
            if let Some(c) = s.configuration() {
                if c.extraction().is_some() {
                    configured.push("extraction".to_string());
                }
                if c.consolidation().is_some() {
                    configured.push("consolidation".to_string());
                }
                if c.reflection().is_some() {
                    configured.push("reflection".to_string());
                }
                if c.self_managed_configuration().is_some() {
                    configured.push("self-managed".to_string());
                }
            }
            // `namespaces` is the retired name for `namespace_templates`;
            // strategies created through the older API still come back only on
            // it, so read the current field first and fall back.
            #[allow(deprecated)]
            let namespaces = s.namespaces().to_vec();
            AgentCoreMemoryStrategy {
                strategy_id: s.strategy_id().to_string(),
                name: s.name().to_string(),
                description: s.description().unwrap_or_default().to_string(),
                status: s
                    .status()
                    .map(|v| v.as_str().to_string())
                    .unwrap_or_default(),
                namespaces,
                namespace_templates: s.namespace_templates().to_vec(),
                configured,
            }
        })
        .collect();

    Ok(AgentCoreMemoryDetail {
        name: m.name().to_string(),
        description: m.description().unwrap_or_default().to_string(),
        encryption_key_arn: m.encryption_key_arn().unwrap_or_default().to_string(),
        execution_role_arn: m.memory_execution_role_arn().unwrap_or_default().to_string(),
        event_expiry_days: m.event_expiry_duration(),
        failure_reason: m.failure_reason().unwrap_or_default().to_string(),
        strategies,
        indexed_keys: m
            .indexed_keys()
            .iter()
            .map(|k| k.key().to_string())
            .collect(),
        stream_delivery_count: m
            .stream_delivery_resources()
            .map(|s| s.resources().len())
            .unwrap_or(0),
    })
}

pub async fn fetch_memory_actors(
    dp: DpClient,
    memory_id: String,
) -> std::result::Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut pager = dp
        .list_actors()
        .memory_id(&memory_id)
        .into_paginator()
        .send();
    while let Some(page) = pager.next().await {
        let page = page.map_err(|e| crate::error::sdk_error_message(&e))?;
        for a in page.actor_summaries() {
            out.push(a.actor_id().to_string());
            if out.len() >= MAX_MEMORY_ACTORS {
                return Ok(out);
            }
        }
    }
    Ok(out)
}

#[derive(Debug, Clone)]
pub struct AgentCoreMemorySession {
    pub actor_id: String,
    pub session_id: String,
    pub created: String,
}

/// `ListSessions` is scoped to one actor, so the Sessions section walks the
/// actor list first (same shape as the knowledge-base Ingestion section, which
/// aggregates per data source). Both fan-outs are capped; a per-actor failure
/// is skipped rather than failing the section.
pub async fn fetch_memory_sessions(
    dp: DpClient,
    memory_id: String,
) -> std::result::Result<Vec<AgentCoreMemorySession>, String> {
    let actors = fetch_memory_actors(dp.clone(), memory_id.clone()).await?;
    let mut out = Vec::new();
    for actor in actors {
        let mut pager = dp
            .list_sessions()
            .memory_id(&memory_id)
            .actor_id(&actor)
            .into_paginator()
            .send();
        while let Some(page) = pager.next().await {
            let Ok(page) = page else { break };
            for s in page.session_summaries() {
                out.push(AgentCoreMemorySession {
                    actor_id: s.actor_id().to_string(),
                    session_id: s.session_id().to_string(),
                    created: fmt_dt(Some(s.created_at())),
                });
                if out.len() >= MAX_MEMORY_SESSIONS {
                    return Ok(out);
                }
            }
        }
    }
    Ok(out)
}

// ═══════════════════════════════════════════════════════════════════════════════
// Identity — workload identities + the two credential-provider kinds
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct AgentCoreWorkloadIdentity {
    pub name: String,
    pub arn: String,
    pub tags: HashMap<String, String>,
}

impl AgentCoreWorkloadIdentity {
    pub fn from_sdk(w: &aws_sdk_bedrockagentcorecontrol::types::WorkloadIdentityType) -> Self {
        Self {
            name: w.name().to_string(),
            arn: w.workload_identity_arn().to_string(),
            tags: HashMap::new(),
        }
    }
}

crate::sections! {
    pub enum AgentCoreWorkloadIdentityDetailSection,
    pub static AGENTCORE_WORKLOAD_IDENTITY_SECTIONS = [
        Overview "Overview" => crate::app::App::trigger_agentcore_identity_detail_load,
        ReturnUrls "Return URLs" => crate::app::App::trigger_agentcore_identity_detail_load,
        Tags "Tags" => crate::app::App::trigger_agentcore_tags_load,
    ]
}

impl Resource for AgentCoreWorkloadIdentity {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&AGENTCORE_WORKLOAD_IDENTITY_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.name
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn resource_type(&self) -> &str {
        "AgentCore Workload Identity"
    }
    fn state(&self) -> ResourceState {
        // `ListWorkloadIdentities` carries no status at all — a bare "exists".
        ResourceState::stateless()
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        format!("{} {}", self.name, self.arn)
    }
    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Name".to_string(), self.name.clone()),
            ("ARN".to_string(), self.arn.clone()),
        ]
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws bedrock-agentcore-control get-workload-identity --name {}",
            shell_quote(&self.name)
        ))
    }
    /// `W` lookup keys. CloudTrail's `ResourceName` for AgentCore may be
    /// either the id or the ARN depending on the event, and keys are tried in
    /// order until one returns events — so carry both (the IAM precedent).
    fn trail_lookup_keys(&self) -> Vec<String> {
        vec![self.name.clone(), self.arn.clone()]
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(console_home(region))
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Debug, Clone)]
pub struct AgentCoreOAuth2Provider {
    pub name: String,
    pub arn: String,
    pub vendor: String,
    pub created: String,
    pub updated: String,
    pub tags: HashMap<String, String>,
}

impl AgentCoreOAuth2Provider {
    pub fn from_sdk(
        p: &aws_sdk_bedrockagentcorecontrol::types::Oauth2CredentialProviderItem,
    ) -> Self {
        Self {
            name: p.name().to_string(),
            arn: p.credential_provider_arn().to_string(),
            vendor: p.credential_provider_vendor().as_str().to_string(),
            created: fmt_dt(Some(p.created_time())),
            updated: fmt_dt(Some(p.last_updated_time())),
            tags: HashMap::new(),
        }
    }
}

crate::sections! {
    pub enum AgentCoreOAuth2ProviderDetailSection,
    pub static AGENTCORE_OAUTH2_PROVIDER_SECTIONS = [
        Overview "Overview" => crate::app::App::trigger_agentcore_identity_detail_load,
        Provider "Provider" => crate::app::App::trigger_agentcore_identity_detail_load,
        Secret "Secret" => crate::app::App::trigger_agentcore_identity_secret_load,
        Tags "Tags" => crate::app::App::trigger_agentcore_tags_load,
    ]
}

impl Resource for AgentCoreOAuth2Provider {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&AGENTCORE_OAUTH2_PROVIDER_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.name
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn resource_type(&self) -> &str {
        "AgentCore OAuth2 Provider"
    }
    fn state(&self) -> ResourceState {
        ResourceState::stateless()
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        format!("{} {} {}", self.name, self.vendor, self.arn)
    }
    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Name".to_string(), self.name.clone()),
            ("Vendor".to_string(), self.vendor.clone()),
            ("ARN".to_string(), self.arn.clone()),
            ("Created".to_string(), self.created.clone()),
            ("Updated".to_string(), self.updated.clone()),
        ]
    }
    fn cli_command(&self) -> Option<String> {
        // `get-oauth2-credential-provider` returns config, never the client
        // secret — the secret lives in the token vault and has no read API.
        Some(format!(
            "aws bedrock-agentcore-control get-oauth2-credential-provider --name {}",
            shell_quote(&self.name)
        ))
    }
    /// `W` lookup keys. CloudTrail's `ResourceName` for AgentCore may be
    /// either the id or the ARN depending on the event, and keys are tried in
    /// order until one returns events — so carry both (the IAM precedent).
    fn trail_lookup_keys(&self) -> Vec<String> {
        vec![self.name.clone(), self.arn.clone()]
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(console_home(region))
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Debug, Clone)]
pub struct AgentCoreApiKeyProvider {
    pub name: String,
    pub arn: String,
    pub created: String,
    pub updated: String,
    pub tags: HashMap<String, String>,
}

impl AgentCoreApiKeyProvider {
    pub fn from_sdk(
        p: &aws_sdk_bedrockagentcorecontrol::types::ApiKeyCredentialProviderItem,
    ) -> Self {
        Self {
            name: p.name().to_string(),
            arn: p.credential_provider_arn().to_string(),
            created: fmt_dt(Some(p.created_time())),
            updated: fmt_dt(Some(p.last_updated_time())),
            tags: HashMap::new(),
        }
    }
}

crate::sections! {
    pub enum AgentCoreApiKeyProviderDetailSection,
    pub static AGENTCORE_API_KEY_PROVIDER_SECTIONS = [
        Overview "Overview" => crate::app::App::trigger_agentcore_identity_detail_load,
        Secret "Secret" => crate::app::App::trigger_agentcore_identity_secret_load,
        Tags "Tags" => crate::app::App::trigger_agentcore_tags_load,
    ]
}

impl Resource for AgentCoreApiKeyProvider {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&AGENTCORE_API_KEY_PROVIDER_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.name
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn resource_type(&self) -> &str {
        "AgentCore API Key Provider"
    }
    fn state(&self) -> ResourceState {
        ResourceState::stateless()
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        format!("{} {}", self.name, self.arn)
    }
    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Name".to_string(), self.name.clone()),
            ("ARN".to_string(), self.arn.clone()),
            ("Created".to_string(), self.created.clone()),
            ("Updated".to_string(), self.updated.clone()),
        ]
    }
    fn cli_command(&self) -> Option<String> {
        // Metadata only — the key itself is never returned by any read API.
        Some(format!(
            "aws bedrock-agentcore-control get-api-key-credential-provider --name {}",
            shell_quote(&self.name)
        ))
    }
    /// `W` lookup keys. CloudTrail's `ResourceName` for AgentCore may be
    /// either the id or the ARN depending on the event, and keys are tried in
    /// order until one returns events — so carry both (the IAM precedent).
    fn trail_lookup_keys(&self) -> Vec<String> {
        vec![self.name.clone(), self.arn.clone()]
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(console_home(region))
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Built-in tools — browser, browser profile, code interpreter
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct AgentCoreBrowser {
    pub browser_id: String,
    pub arn: String,
    pub name: String,
    pub description: String,
    pub status: String,
    pub created: String,
    pub tags: HashMap<String, String>,
}

impl AgentCoreBrowser {
    pub fn from_sdk(b: &aws_sdk_bedrockagentcorecontrol::types::BrowserSummary) -> Self {
        Self {
            browser_id: b.browser_id().to_string(),
            arn: b.browser_arn().to_string(),
            name: b.name().unwrap_or_default().to_string(),
            description: b.description().unwrap_or_default().to_string(),
            status: b.status().as_str().to_string(),
            created: fmt_dt(Some(b.created_at())),
            tags: HashMap::new(),
        }
    }
}

crate::sections! {
    pub enum AgentCoreBrowserDetailSection,
    pub static AGENTCORE_BROWSER_SECTIONS = [
        Overview "Overview" => crate::app::App::trigger_agentcore_tool_detail_load,
        Config "Config" => crate::app::App::trigger_agentcore_tool_detail_load,
        Sessions "Sessions" => crate::app::App::trigger_agentcore_tool_sessions_load,
        Tags "Tags" => crate::app::App::trigger_agentcore_tags_load,
    ]
}

impl Resource for AgentCoreBrowser {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&AGENTCORE_BROWSER_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.browser_id
    }
    fn name(&self) -> &str {
        if self.name.is_empty() {
            &self.browser_id
        } else {
            &self.name
        }
    }
    fn resource_type(&self) -> &str {
        "AgentCore Browser"
    }
    fn state(&self) -> ResourceState {
        map_status(&self.status)
    }

    fn state_label(&self) -> String {
        native_state_label(&self.status, || self.state())
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        format!(
            "{} {} {} {}",
            self.browser_id, self.name, self.status, self.description
        )
    }
    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Name".to_string(), self.name.clone()),
            ("ID".to_string(), self.browser_id.clone()),
            ("ARN".to_string(), self.arn.clone()),
            ("Status".to_string(), self.status.clone()),
            ("Description".to_string(), self.description.clone()),
            ("Created".to_string(), self.created.clone()),
        ]
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws bedrock-agentcore-control get-browser --browser-id {}",
            shell_quote(&self.browser_id)
        ))
    }
    /// `W` lookup keys. CloudTrail's `ResourceName` for AgentCore may be
    /// either the id or the ARN depending on the event, and keys are tried in
    /// order until one returns events — so carry both (the IAM precedent).
    fn trail_lookup_keys(&self) -> Vec<String> {
        vec![self.browser_id.clone(), self.arn.clone()]
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(console_home(region))
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Debug, Clone)]
pub struct AgentCoreCodeInterpreter {
    pub interpreter_id: String,
    pub arn: String,
    pub name: String,
    pub description: String,
    pub status: String,
    pub created: String,
    pub tags: HashMap<String, String>,
}

impl AgentCoreCodeInterpreter {
    pub fn from_sdk(c: &aws_sdk_bedrockagentcorecontrol::types::CodeInterpreterSummary) -> Self {
        Self {
            interpreter_id: c.code_interpreter_id().to_string(),
            arn: c.code_interpreter_arn().to_string(),
            name: c.name().unwrap_or_default().to_string(),
            description: c.description().unwrap_or_default().to_string(),
            status: c.status().as_str().to_string(),
            created: fmt_dt(Some(c.created_at())),
            tags: HashMap::new(),
        }
    }
}

crate::sections! {
    pub enum AgentCoreCodeInterpreterDetailSection,
    pub static AGENTCORE_CODE_INTERPRETER_SECTIONS = [
        Overview "Overview" => crate::app::App::trigger_agentcore_tool_detail_load,
        Config "Config" => crate::app::App::trigger_agentcore_tool_detail_load,
        Sessions "Sessions" => crate::app::App::trigger_agentcore_tool_sessions_load,
        Tags "Tags" => crate::app::App::trigger_agentcore_tags_load,
    ]
}

impl Resource for AgentCoreCodeInterpreter {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&AGENTCORE_CODE_INTERPRETER_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.interpreter_id
    }
    fn name(&self) -> &str {
        if self.name.is_empty() {
            &self.interpreter_id
        } else {
            &self.name
        }
    }
    fn resource_type(&self) -> &str {
        "AgentCore Code Interpreter"
    }
    fn state(&self) -> ResourceState {
        map_status(&self.status)
    }

    fn state_label(&self) -> String {
        native_state_label(&self.status, || self.state())
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        format!(
            "{} {} {} {}",
            self.interpreter_id, self.name, self.status, self.description
        )
    }
    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Name".to_string(), self.name.clone()),
            ("ID".to_string(), self.interpreter_id.clone()),
            ("ARN".to_string(), self.arn.clone()),
            ("Status".to_string(), self.status.clone()),
            ("Description".to_string(), self.description.clone()),
            ("Created".to_string(), self.created.clone()),
        ]
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws bedrock-agentcore-control get-code-interpreter --code-interpreter-id {}",
            shell_quote(&self.interpreter_id)
        ))
    }
    /// `W` lookup keys. CloudTrail's `ResourceName` for AgentCore may be
    /// either the id or the ARN depending on the event, and keys are tried in
    /// order until one returns events — so carry both (the IAM precedent).
    fn trail_lookup_keys(&self) -> Vec<String> {
        vec![self.interpreter_id.clone(), self.arn.clone()]
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(console_home(region))
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Debug, Clone)]
pub struct AgentCoreBrowserProfile {
    pub profile_id: String,
    pub arn: String,
    pub name: String,
    pub description: String,
    pub status: String,
    pub created: String,
    pub last_saved: String,
    pub last_saved_browser_id: String,
    pub last_saved_session_id: String,
    pub tags: HashMap<String, String>,
}

impl AgentCoreBrowserProfile {
    pub fn from_sdk(p: &aws_sdk_bedrockagentcorecontrol::types::BrowserProfileSummary) -> Self {
        Self {
            profile_id: p.profile_id().to_string(),
            arn: p.profile_arn().to_string(),
            name: p.name().to_string(),
            description: p.description().unwrap_or_default().to_string(),
            status: p.status().as_str().to_string(),
            created: fmt_dt(Some(p.created_at())),
            last_saved: fmt_dt(p.last_saved_at()),
            last_saved_browser_id: p.last_saved_browser_id().unwrap_or_default().to_string(),
            last_saved_session_id: p
                .last_saved_browser_session_id()
                .unwrap_or_default()
                .to_string(),
            tags: HashMap::new(),
        }
    }
}

impl Resource for AgentCoreBrowserProfile {
    fn id(&self) -> &str {
        &self.profile_id
    }
    fn name(&self) -> &str {
        if self.name.is_empty() {
            &self.profile_id
        } else {
            &self.name
        }
    }
    fn resource_type(&self) -> &str {
        "AgentCore Browser Profile"
    }
    fn state(&self) -> ResourceState {
        map_status(&self.status)
    }

    fn state_label(&self) -> String {
        native_state_label(&self.status, || self.state())
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        format!(
            "{} {} {} {}",
            self.profile_id, self.name, self.status, self.description
        )
    }
    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Name".to_string(), self.name.clone()),
            ("ID".to_string(), self.profile_id.clone()),
            ("ARN".to_string(), self.arn.clone()),
            ("Status".to_string(), self.status.clone()),
            ("Description".to_string(), self.description.clone()),
            ("Created".to_string(), self.created.clone()),
            ("Last Saved".to_string(), self.last_saved.clone()),
            (
                "Saved From Browser".to_string(),
                self.last_saved_browser_id.clone(),
            ),
            (
                "Saved From Session".to_string(),
                self.last_saved_session_id.clone(),
            ),
        ]
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws bedrock-agentcore-control get-browser-profile --profile-id {}",
            shell_quote(&self.profile_id)
        ))
    }
    /// `W` lookup keys. CloudTrail's `ResourceName` for AgentCore may be
    /// either the id or the ARN depending on the event, and keys are tried in
    /// order until one returns events — so carry both (the IAM precedent).
    fn trail_lookup_keys(&self) -> Vec<String> {
        vec![self.profile_id.clone(), self.arn.clone()]
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(console_home(region))
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Shared by the browser and code-interpreter panes — the two `Get*` responses
/// differ only in which extras they carry, so one struct with empty fields for
/// the absent ones beats two near-identical bundles and two lazy maps.
#[derive(Debug, Clone, Default)]
pub struct AgentCoreToolDetail {
    pub execution_role_arn: String,
    pub failure_reason: String,
    pub network_mode: String,
    pub subnets: Vec<String>,
    pub security_groups: Vec<String>,
    pub certificate_count: usize,
    /// Browser only.
    pub recording_enabled: Option<bool>,
    pub recording_s3_bucket: String,
    pub recording_s3_prefix: String,
    pub enterprise_policy_count: usize,
    pub browser_signing: bool,
}

pub async fn fetch_browser_detail(
    ctl: CtlClient,
    browser_id: String,
) -> std::result::Result<AgentCoreToolDetail, String> {
    let b = ctl
        .get_browser()
        .browser_id(&browser_id)
        .send()
        .await
        .map_err(|e| crate::error::sdk_error_message(&e))?;
    let mut d = AgentCoreToolDetail {
        execution_role_arn: b.execution_role_arn().unwrap_or_default().to_string(),
        failure_reason: b.failure_reason().unwrap_or_default().to_string(),
        certificate_count: b.certificates().len(),
        enterprise_policy_count: b.enterprise_policies().len(),
        browser_signing: b.browser_signing().is_some(),
        ..Default::default()
    };
    if let Some(n) = b.network_configuration() {
        d.network_mode = n.network_mode().as_str().to_string();
        if let Some(v) = n.vpc_config() {
            d.subnets = v.subnets().to_vec();
            d.security_groups = v.security_groups().to_vec();
        }
    }
    if let Some(r) = b.recording() {
        d.recording_enabled = Some(r.enabled());
        if let Some(s3) = r.s3_location() {
            d.recording_s3_bucket = s3.bucket().to_string();
            d.recording_s3_prefix = s3.prefix().to_string();
        }
    }
    Ok(d)
}

pub async fn fetch_code_interpreter_detail(
    ctl: CtlClient,
    interpreter_id: String,
) -> std::result::Result<AgentCoreToolDetail, String> {
    let c = ctl
        .get_code_interpreter()
        .code_interpreter_id(&interpreter_id)
        .send()
        .await
        .map_err(|e| crate::error::sdk_error_message(&e))?;
    let mut d = AgentCoreToolDetail {
        execution_role_arn: c.execution_role_arn().unwrap_or_default().to_string(),
        failure_reason: c.failure_reason().unwrap_or_default().to_string(),
        certificate_count: c.certificates().len(),
        ..Default::default()
    };
    if let Some(n) = c.network_configuration() {
        d.network_mode = n.network_mode().as_str().to_string();
        if let Some(v) = n.vpc_config() {
            d.subnets = v.subnets().to_vec();
            d.security_groups = v.security_groups().to_vec();
        }
    }
    Ok(d)
}

#[derive(Debug, Clone)]
pub struct AgentCoreToolSession {
    pub session_id: String,
    pub name: String,
    pub status: String,
    pub created: String,
    pub last_updated: String,
}

pub async fn fetch_browser_sessions(
    dp: DpClient,
    browser_id: String,
) -> std::result::Result<Vec<AgentCoreToolSession>, String> {
    // No fluent paginator on this op — advance the token through the shared
    // helper so an absent / empty / non-advancing token stops the loop.
    let mut out = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut req = dp.list_browser_sessions().browser_identifier(&browser_id);
        if let Some(t) = &token {
            req = req.next_token(t);
        }
        let page = req
            .send()
            .await
            .map_err(|e| crate::error::sdk_error_message(&e))?;
        for s in page.items() {
            out.push(AgentCoreToolSession {
                session_id: s.session_id().to_string(),
                name: s.name().unwrap_or_default().to_string(),
                status: s.status().as_str().to_string(),
                created: fmt_dt(Some(s.created_at())),
                last_updated: fmt_dt(s.last_updated_at()),
            });
            if out.len() >= MAX_TOOL_SESSIONS {
                return Ok(out);
            }
        }
        token = crate::aws::pagination::next_page_token(page.next_token(), &token);
        if token.is_none() {
            break;
        }
    }
    Ok(out)
}

pub async fn fetch_code_interpreter_sessions(
    dp: DpClient,
    interpreter_id: String,
) -> std::result::Result<Vec<AgentCoreToolSession>, String> {
    let mut out = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut req = dp
            .list_code_interpreter_sessions()
            .code_interpreter_identifier(&interpreter_id);
        if let Some(t) = &token {
            req = req.next_token(t);
        }
        let page = req
            .send()
            .await
            .map_err(|e| crate::error::sdk_error_message(&e))?;
        for s in page.items() {
            out.push(AgentCoreToolSession {
                session_id: s.session_id().to_string(),
                name: s.name().unwrap_or_default().to_string(),
                status: s.status().as_str().to_string(),
                created: fmt_dt(Some(s.created_at())),
                last_updated: fmt_dt(s.last_updated_at()),
            });
            if out.len() >= MAX_TOOL_SESSIONS {
                return Ok(out);
            }
        }
        token = crate::aws::pagination::next_page_token(page.next_token(), &token);
        if token.is_none() {
            break;
        }
    }
    Ok(out)
}

// ═══════════════════════════════════════════════════════════════════════════════
// CloudWatch metrics (`m` overlay) — namespace `AWS/Bedrock-AgentCore`
// ═══════════════════════════════════════════════════════════════════════════════

pub use crate::aws::services::ec2::MetricsTimeRange;

/// Which chart set the pane draws. The five families share one namespace and
/// the same invocation-metric names, so one `MetricsKind` covers them all and
/// the flavor picks the extras (gateway's Duration / TargetExecutionTime,
/// runtime's vCPU / GB-hour usage) — the same shape as `ApiMetricsFlavor`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentCoreMetricsFlavor {
    Runtime,
    Gateway,
    Memory,
    Browser,
    CodeInterpreter,
}

impl AgentCoreMetricsFlavor {
    pub fn label(&self) -> &'static str {
        match self {
            AgentCoreMetricsFlavor::Runtime => "Runtime",
            AgentCoreMetricsFlavor::Gateway => "Gateway",
            AgentCoreMetricsFlavor::Memory => "Memory",
            AgentCoreMetricsFlavor::Browser => "Browser",
            AgentCoreMetricsFlavor::CodeInterpreter => "Code Interpreter",
        }
    }
}

#[derive(Debug, Clone)]
pub struct AgentCoreMetricsData {
    pub time_range: MetricsTimeRange,
    pub flavor: AgentCoreMetricsFlavor,
    pub invocations: Vec<(f64, f64)>,
    pub latency: Vec<(f64, f64)>,
    pub system_errors: Vec<(f64, f64)>,
    pub user_errors: Vec<(f64, f64)>,
    pub throttles: Vec<(f64, f64)>,
    /// Runtime only.
    pub sessions: Vec<(f64, f64)>,
    pub cpu_hours: Vec<(f64, f64)>,
    pub memory_gb_hours: Vec<(f64, f64)>,
    /// Gateway only.
    pub duration: Vec<(f64, f64)>,
    pub target_exec_time: Vec<(f64, f64)>,
    pub x_max: f64,
}

#[derive(Debug, Clone)]
pub enum AgentCoreMetricsState {
    Loading,
    Loaded(Box<AgentCoreMetricsData>),
}

/// Pull `AWS/Bedrock-AgentCore` metrics for one resource.
///
/// **Why SEARCH and not `GetMetricStatistics`**: AWS documents a different
/// dimension set per family (gateway publishes `Operation`/`Protocol`/
/// `Method`/`Resource`/`Name`; runtime's usage metrics publish `Service`,
/// `Service,Resource` and `Service,Resource,Name`; the runtime *invocation*
/// metrics' dimensions aren't documented at all). A fixed-dimension query
/// against a guess matches nothing silently. The schema-free `SEARCH` form
/// matches the ARN as a token against dimension values whatever the schema
/// is, and aggregating collapses the per-operation/per-method series into one
/// line — same reasoning as Network Firewall's per-AZ aggregation.
///
/// The usage metrics are the exception: they publish at three nesting levels,
/// so a token match would sum a resource's hours together with the
/// `Service,Resource,Name` breakdown of the same hours. Those are pinned to
/// the two-dimension schema instead.
pub async fn fetch_agentcore_metrics(
    cw: aws_sdk_cloudwatch::Client,
    arn: String,
    flavor: AgentCoreMetricsFlavor,
    time_range: MetricsTimeRange,
) -> crate::error::Result<AgentCoreMetricsData> {
    use aws_sdk_cloudwatch::types::MetricDataQuery;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let start = now - time_range.duration_secs();
    let period = time_range.period_secs();

    // (query id, outer aggregate, inner stat, metric name)
    let mut specs: Vec<(&str, &str, &str, &str)> = vec![
        ("m0", "SUM", "Sum", "Invocations"),
        ("m1", "AVG", "Average", "Latency"),
        ("m2", "SUM", "Sum", "SystemErrors"),
        ("m3", "SUM", "Sum", "UserErrors"),
        ("m4", "SUM", "Sum", "Throttles"),
    ];
    if flavor == AgentCoreMetricsFlavor::Runtime {
        specs.push(("m5", "SUM", "Sum", "SessionCount"));
    }
    if flavor == AgentCoreMetricsFlavor::Gateway {
        specs.push(("m8", "AVG", "Average", "Duration"));
        specs.push(("m9", "AVG", "Average", "TargetExecutionTime"));
    }

    let mut queries: Vec<MetricDataQuery> = specs
        .iter()
        .map(|(id, agg, stat, metric)| {
            let expr = format!(
                "{}(SEARCH('Namespace=\"AWS/Bedrock-AgentCore\" MetricName=\"{}\" \"{}\"', '{}', {}))",
                agg, metric, arn, stat, period
            );
            MetricDataQuery::builder().id(*id).expression(expr).build()
        })
        .collect();

    if flavor == AgentCoreMetricsFlavor::Runtime {
        for (id, metric) in [("m6", "CPUUsed-vCPUHours"), ("m7", "MemoryUsed-GBHours")] {
            let expr = format!(
                "SUM(SEARCH('{{AWS/Bedrock-AgentCore,Resource,Service}} MetricName=\"{}\" Resource=\"{}\"', 'Sum', {}))",
                metric, arn, period
            );
            queries.push(MetricDataQuery::builder().id(id).expression(expr).build());
        }
    }

    let resp = cw
        .get_metric_data()
        .set_metric_data_queries(Some(queries))
        .start_time(aws_sdk_cloudwatch::primitives::DateTime::from_secs(start))
        .end_time(aws_sdk_cloudwatch::primitives::DateTime::from_secs(now))
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;

    let mut series: HashMap<String, Vec<(f64, f64)>> = HashMap::new();
    for r in resp.metric_data_results() {
        let id = r.id().unwrap_or_default().to_string();
        let mut pts: Vec<(f64, f64)> = r
            .timestamps()
            .iter()
            .zip(r.values().iter())
            .map(|(t, v)| (t.secs() as f64 - start as f64, *v))
            .collect();
        pts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        series.insert(id, pts);
    }
    let take = |id: &str| series.get(id).cloned().unwrap_or_default();

    Ok(AgentCoreMetricsData {
        time_range,
        flavor,
        invocations: take("m0"),
        latency: take("m1"),
        system_errors: take("m2"),
        user_errors: take("m3"),
        throttles: take("m4"),
        sessions: take("m5"),
        cpu_hours: take("m6"),
        memory_gb_hours: take("m7"),
        duration: take("m8"),
        target_exec_time: take("m9"),
        x_max: time_range.duration_secs() as f64,
    })
}

// ═══════════════════════════════════════════════════════════════════════════════
// Memory session browser (`i`) — data-plane reads
//
// The Memory pane's Actors/Sessions sections are flat lists; this is the drill
// path underneath them. `ListEvents` is the only way to read what an agent
// actually remembered, so it's the AgentCore analogue of the S3 object
// browser — the one surface with no other read path.
// ═══════════════════════════════════════════════════════════════════════════════

/// One page cap per level. Events carry full conversation payloads, so this
/// is a page size rather than a hard ceiling — `n` fetches the next page.
pub const MEMORY_BROWSER_PAGE: i32 = 100;

/// A row at whatever level the browser is showing. `detail` is the full text
/// `e` opens and `y` copies; `primary`/`secondary` are the list columns.
#[derive(Debug, Clone)]
pub struct MemoryBrowserRow {
    /// The id this row drills into (actor id / session id), or the record /
    /// event id at a leaf level.
    pub id: String,
    pub primary: String,
    pub secondary: String,
    pub detail: String,
}

#[derive(Debug, Clone)]
pub struct MemoryBrowserPage {
    pub rows: Vec<MemoryBrowserRow>,
    pub next_token: Option<String>,
}

pub async fn browse_actors(
    dp: DpClient,
    memory_id: String,
    token: Option<String>,
) -> std::result::Result<MemoryBrowserPage, String> {
    let mut req = dp
        .list_actors()
        .memory_id(&memory_id)
        .max_results(MEMORY_BROWSER_PAGE);
    if let Some(t) = &token {
        req = req.next_token(t);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| crate::error::sdk_error_message(&e))?;
    let rows = resp
        .actor_summaries()
        .iter()
        .map(|a| MemoryBrowserRow {
            id: a.actor_id().to_string(),
            primary: a.actor_id().to_string(),
            secondary: String::new(),
            detail: a.actor_id().to_string(),
        })
        .collect();
    Ok(MemoryBrowserPage {
        rows,
        next_token: crate::aws::pagination::next_page_token(resp.next_token(), &token),
    })
}

pub async fn browse_sessions(
    dp: DpClient,
    memory_id: String,
    actor_id: String,
    token: Option<String>,
) -> std::result::Result<MemoryBrowserPage, String> {
    let mut req = dp
        .list_sessions()
        .memory_id(&memory_id)
        .actor_id(&actor_id)
        .max_results(MEMORY_BROWSER_PAGE);
    if let Some(t) = &token {
        req = req.next_token(t);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| crate::error::sdk_error_message(&e))?;
    let rows = resp
        .session_summaries()
        .iter()
        .map(|s| MemoryBrowserRow {
            id: s.session_id().to_string(),
            primary: s.session_id().to_string(),
            secondary: fmt_dt(Some(s.created_at())),
            detail: format!(
                "session {}\nactor {}\ncreated {}",
                s.session_id(),
                s.actor_id(),
                fmt_dt(Some(s.created_at()))
            ),
        })
        .collect();
    Ok(MemoryBrowserPage {
        rows,
        next_token: crate::aws::pagination::next_page_token(resp.next_token(), &token),
    })
}

/// The leaf level: the conversation itself. `include_payloads(true)` is what
/// makes this worth opening — without it the rows are bare ids.
pub async fn browse_events(
    dp: DpClient,
    memory_id: String,
    actor_id: String,
    session_id: String,
    token: Option<String>,
) -> std::result::Result<MemoryBrowserPage, String> {
    use aws_sdk_bedrockagentcore::types::{Content, PayloadType};

    let mut req = dp
        .list_events()
        .memory_id(&memory_id)
        .actor_id(&actor_id)
        .session_id(&session_id)
        .include_payloads(true)
        .max_results(MEMORY_BROWSER_PAGE);
    if let Some(t) = &token {
        req = req.next_token(t);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| crate::error::sdk_error_message(&e))?;

    let rows = resp
        .events()
        .iter()
        .map(|e| {
            // An event's payload is a *list* of parts, each either a
            // conversational turn or an opaque blob. Render every part — a
            // tool-call turn commonly pairs a message with a blob.
            let mut roles: Vec<String> = Vec::new();
            let mut body = String::new();
            for p in e.payload() {
                match p {
                    PayloadType::Conversational(c) => {
                        let role = c.role().as_str().to_string();
                        let text = match c.content() {
                            Some(Content::Text(t)) => t.clone(),
                            _ => String::new(),
                        };
                        if !roles.contains(&role) {
                            roles.push(role.clone());
                        }
                        body.push_str(&format!("[{}] {}\n", role, text));
                    }
                    PayloadType::Blob(doc) => {
                        if !roles.iter().any(|r| r == "blob") {
                            roles.push("blob".to_string());
                        }
                        body.push_str(&format!(
                            "[blob]\n{}\n",
                            crate::aws::document::document_pretty(doc)
                        ));
                    }
                    _ => {}
                }
            }
            if let Some(b) = e.branch() {
                body.push_str(&format!("\nbranch: {}\n", b.name()));
            }
            // The first line of the body, trimmed — the list column.
            let preview = body
                .lines()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("(no payload)")
                .chars()
                .take(200)
                .collect::<String>();
            MemoryBrowserRow {
                id: e.event_id().to_string(),
                primary: preview,
                secondary: format!(
                    "{}  {}",
                    fmt_dt(Some(e.event_timestamp())),
                    roles.join("/")
                ),
                detail: format!(
                    "event {}\ntime {}\nactor {}\nsession {}\n\n{}",
                    e.event_id(),
                    fmt_dt(Some(e.event_timestamp())),
                    e.actor_id(),
                    e.session_id(),
                    body
                ),
            }
        })
        .collect();
    Ok(MemoryBrowserPage {
        rows,
        next_token: crate::aws::pagination::next_page_token(resp.next_token(), &token),
    })
}

/// Long-term memory records for one strategy. `ListMemoryRecords` is scoped
/// by namespace *or* strategy id; strategy id is used here because namespaces
/// are templates (`/strategies/{id}/actors/{actorId}`) that would need
/// placeholder substitution against a specific actor to be usable.
pub async fn browse_records(
    dp: DpClient,
    memory_id: String,
    strategy_id: String,
    token: Option<String>,
) -> std::result::Result<MemoryBrowserPage, String> {
    use aws_sdk_bedrockagentcore::types::MemoryContent;

    let mut req = dp
        .list_memory_records()
        .memory_id(&memory_id)
        .memory_strategy_id(&strategy_id)
        .max_results(MEMORY_BROWSER_PAGE);
    if let Some(t) = &token {
        req = req.next_token(t);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| crate::error::sdk_error_message(&e))?;

    let rows = resp
        .memory_record_summaries()
        .iter()
        .map(|r| {
            let text = match r.content() {
                Some(MemoryContent::Text(t)) => t.clone(),
                _ => String::new(),
            };
            MemoryBrowserRow {
                id: r.memory_record_id().to_string(),
                primary: text.chars().take(200).collect::<String>(),
                secondary: fmt_dt(Some(r.created_at())),
                detail: format!(
                    "record {}\nstrategy {}\ncreated {}\nnamespaces {}\n\n{}",
                    r.memory_record_id(),
                    r.memory_strategy_id(),
                    fmt_dt(Some(r.created_at())),
                    r.namespaces().join(", "),
                    text
                ),
            }
        })
        .collect();
    Ok(MemoryBrowserPage {
        rows,
        next_token: crate::aws::pagination::next_page_token(resp.next_token(), &token),
    })
}

// ═══════════════════════════════════════════════════════════════════════════════
// A2A agent card (`x` on a runtime) — data plane, opt-in
// ═══════════════════════════════════════════════════════════════════════════════

/// Fetch a runtime's A2A agent card (the skill manifest an A2A client reads to
/// discover what the agent can do). Returns pretty JSON — the card is an
/// untyped `Document` whose schema is the A2A spec's, not AWS's, so there is
/// nothing stable to model into rows.
///
/// **Why this is `x`-gated and has no on-enter hook**, unlike every other
/// section in this service: `GetAgentCard` is a *data-plane* call on the same
/// client as `InvokeAgentRuntime`. An A2A card is served by the agent itself,
/// so asking for one reaches the container — which on an idle runtime means a
/// cold start, and billable compute. An on-enter hook would fire that from a
/// `Tab` press, and the flat detail view's trigger sweep (`\`) would fire it
/// for every runtime you looked at. Same reasoning as Secrets Manager's `x`:
/// metadata is free, the thing behind it is not.
pub async fn fetch_agent_card(
    dp: DpClient,
    runtime_arn: String,
    qualifier: Option<String>,
) -> std::result::Result<String, String> {
    let mut req = dp.get_agent_card().agent_runtime_arn(&runtime_arn);
    if let Some(q) = qualifier {
        req = req.qualifier(q);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| crate::error::sdk_error_message(&e))?;
    Ok(crate::aws::document::document_pretty(resp.agent_card()))
}

// ═══════════════════════════════════════════════════════════════════════════════
// Policy — engines + policies
//
// Preview surface. The load phases for this and the two families below stay
// **silent** on failure (no `ResourceLoadWarning`): most accounts have never
// enabled them, so a permanent warning on every load would be noise. The
// per-tab empty state in resource_list.rs carries the explanation instead —
// the same line GuardDuty's ListMembers sits on.
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct AgentCorePolicyEngine {
    pub engine_id: String,
    pub arn: String,
    pub name: String,
    pub description: String,
    pub status: String,
    pub encryption_key_arn: String,
    pub status_reasons: Vec<String>,
    pub created: String,
    pub updated: String,
    pub tags: HashMap<String, String>,
}

impl AgentCorePolicyEngine {
    pub fn from_sdk(e: &aws_sdk_bedrockagentcorecontrol::types::PolicyEngine) -> Self {
        Self {
            engine_id: e.policy_engine_id().to_string(),
            arn: e.policy_engine_arn().to_string(),
            name: e.name().to_string(),
            description: e.description().unwrap_or_default().to_string(),
            status: e.status().as_str().to_string(),
            encryption_key_arn: e.encryption_key_arn().unwrap_or_default().to_string(),
            status_reasons: e.status_reasons().to_vec(),
            created: fmt_dt(Some(e.created_at())),
            updated: fmt_dt(Some(e.updated_at())),
            tags: HashMap::new(),
        }
    }
}

crate::sections! {
    pub enum AgentCorePolicyEngineDetailSection,
    pub static AGENTCORE_POLICY_ENGINE_SECTIONS = [
        Overview "Overview",
        // No hook: the Policies section is filled by filtering the policies
        // already in `App.resources` from the same load (the Vpc/DxLag
        // precedent), so there is nothing to fetch.
        Policies "Policies",
        Generations "Generations" => crate::app::App::trigger_agentcore_policy_generations_load,
        Tags "Tags" => crate::app::App::trigger_agentcore_tags_load,
    ]
}

impl Resource for AgentCorePolicyEngine {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&AGENTCORE_POLICY_ENGINE_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.engine_id
    }
    fn name(&self) -> &str {
        if self.name.is_empty() {
            &self.engine_id
        } else {
            &self.name
        }
    }
    fn resource_type(&self) -> &str {
        "AgentCore Policy Engine"
    }
    fn state(&self) -> ResourceState {
        map_status(&self.status)
    }

    fn state_label(&self) -> String {
        native_state_label(&self.status, || self.state())
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        format!(
            "{} {} {} {}",
            self.engine_id, self.name, self.status, self.description
        )
    }
    fn details(&self) -> Vec<(String, String)> {
        let mut rows = vec![
            ("Name".to_string(), self.name.clone()),
            ("ID".to_string(), self.engine_id.clone()),
            ("ARN".to_string(), self.arn.clone()),
            ("Status".to_string(), self.status.clone()),
            ("Description".to_string(), self.description.clone()),
            ("Created".to_string(), self.created.clone()),
            ("Updated".to_string(), self.updated.clone()),
        ];
        if !self.encryption_key_arn.is_empty() {
            rows.push(("KMS Key".to_string(), self.encryption_key_arn.clone()));
        }
        for r in &self.status_reasons {
            rows.push((format!("  ⚠ {}", r), String::new()));
        }
        rows
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws bedrock-agentcore-control get-policy-engine --policy-engine-id {}",
            shell_quote(&self.engine_id)
        ))
    }
    /// `W` lookup keys. CloudTrail's `ResourceName` for AgentCore may be
    /// either the id or the ARN depending on the event, and keys are tried in
    /// order until one returns events — so carry both (the IAM precedent).
    fn trail_lookup_keys(&self) -> Vec<String> {
        vec![self.engine_id.clone(), self.arn.clone()]
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(console_home(region))
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Debug, Clone)]
pub struct AgentCorePolicy {
    pub policy_id: String,
    pub arn: String,
    pub name: String,
    pub engine_id: String,
    pub description: String,
    pub status: String,
    /// ACTIVE or LOG_ONLY — the field that decides whether this policy is
    /// actually blocking anything, so it's what `state()` reflects.
    pub enforcement_mode: String,
    /// Flattened `PolicyDefinition` union: ("Cedar" | "Policy" |
    /// "Policy generation", the statement or generation id).
    pub definition_kind: String,
    pub definition: String,
    pub status_reasons: Vec<String>,
    pub created: String,
    pub updated: String,
    pub tags: HashMap<String, String>,
}

impl AgentCorePolicy {
    pub fn from_sdk(p: &aws_sdk_bedrockagentcorecontrol::types::Policy) -> Self {
        use aws_sdk_bedrockagentcorecontrol::types::PolicyDefinition;
        let (definition_kind, definition) = match p.definition() {
            Some(PolicyDefinition::Cedar(c)) => ("Cedar".to_string(), c.statement().to_string()),
            Some(PolicyDefinition::Policy(s)) => {
                ("Statement".to_string(), s.statement().to_string())
            }
            Some(PolicyDefinition::PolicyGeneration(g)) => (
                "Generated".to_string(),
                format!(
                    "generation {} · asset {}",
                    g.policy_generation_id(),
                    g.policy_generation_asset_id()
                ),
            ),
            _ => (String::new(), String::new()),
        };
        Self {
            policy_id: p.policy_id().to_string(),
            arn: p.policy_arn().to_string(),
            name: p.name().to_string(),
            engine_id: p.policy_engine_id().to_string(),
            description: p.description().unwrap_or_default().to_string(),
            status: p.status().as_str().to_string(),
            enforcement_mode: p.enforcement_mode().as_str().to_string(),
            definition_kind,
            definition,
            status_reasons: p.status_reasons().to_vec(),
            created: fmt_dt(Some(p.created_at())),
            updated: fmt_dt(Some(p.updated_at())),
            tags: HashMap::new(),
        }
    }
}

crate::sections! {
    pub enum AgentCorePolicyDetailSection,
    pub static AGENTCORE_POLICY_SECTIONS = [
        Overview "Overview",
        Definition "Definition",
        Tags "Tags" => crate::app::App::trigger_agentcore_tags_load,
    ]
}

impl Resource for AgentCorePolicy {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&AGENTCORE_POLICY_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.policy_id
    }
    fn name(&self) -> &str {
        if self.name.is_empty() {
            &self.policy_id
        } else {
            &self.name
        }
    }
    fn resource_type(&self) -> &str {
        "AgentCore Policy"
    }
    fn state(&self) -> ResourceState {
        // A LOG_ONLY policy is deliberately not enforcing — surface that as a
        // distinct state rather than folding it into the lifecycle status,
        // because "healthy but not blocking" is the interesting case here.
        if self.enforcement_mode.eq_ignore_ascii_case("LOG_ONLY") {
            return ResourceState::Unknown("log-only".to_string());
        }
        map_status(&self.status)
    }

    fn state_label(&self) -> String {
        if self.enforcement_mode.eq_ignore_ascii_case("LOG_ONLY") {
            "log-only".to_string()
        } else {
            native_state_label(&self.status, || self.state())
        }
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        format!(
            "{} {} {} {} {} {}",
            self.policy_id,
            self.name,
            self.status,
            self.enforcement_mode,
            self.engine_id,
            self.description
        )
    }
    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Name".to_string(), self.name.clone()),
            ("ID".to_string(), self.policy_id.clone()),
            ("Engine".to_string(), self.engine_id.clone()),
            ("Status".to_string(), self.status.clone()),
            ("Enforcement".to_string(), self.enforcement_mode.clone()),
        ]
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws bedrock-agentcore-control get-policy --policy-id {}",
            shell_quote(&self.policy_id)
        ))
    }
    /// `W` lookup keys. CloudTrail's `ResourceName` for AgentCore may be
    /// either the id or the ARN depending on the event, and keys are tried in
    /// order until one returns events — so carry both (the IAM precedent).
    fn trail_lookup_keys(&self) -> Vec<String> {
        vec![self.policy_id.clone(), self.arn.clone()]
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(console_home(region))
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Evaluation — evaluators, online evaluation configs, datasets
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct AgentCoreEvaluator {
    pub evaluator_id: String,
    pub arn: String,
    pub name: String,
    pub description: String,
    pub evaluator_type: String,
    pub level: String,
    pub status: String,
    pub locked: bool,
    pub kms_key_arn: String,
    pub created: String,
    pub updated: String,
    pub tags: HashMap<String, String>,
}

impl AgentCoreEvaluator {
    pub fn from_sdk(e: &aws_sdk_bedrockagentcorecontrol::types::EvaluatorSummary) -> Self {
        Self {
            evaluator_id: e.evaluator_id().to_string(),
            arn: e.evaluator_arn().to_string(),
            name: e.evaluator_name().to_string(),
            description: e.description().unwrap_or_default().to_string(),
            evaluator_type: e.evaluator_type().as_str().to_string(),
            level: e
                .level()
                .map(|l| l.as_str().to_string())
                .unwrap_or_default(),
            status: e.status().as_str().to_string(),
            locked: e.locked_for_modification().unwrap_or(false),
            kms_key_arn: e.kms_key_arn().unwrap_or_default().to_string(),
            created: fmt_dt(Some(e.created_at())),
            updated: fmt_dt(Some(e.updated_at())),
            tags: HashMap::new(),
        }
    }
}

impl Resource for AgentCoreEvaluator {
    fn id(&self) -> &str {
        &self.evaluator_id
    }
    fn name(&self) -> &str {
        if self.name.is_empty() {
            &self.evaluator_id
        } else {
            &self.name
        }
    }
    fn resource_type(&self) -> &str {
        "AgentCore Evaluator"
    }
    fn state(&self) -> ResourceState {
        map_status(&self.status)
    }

    fn state_label(&self) -> String {
        native_state_label(&self.status, || self.state())
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        format!(
            "{} {} {} {} {} {}",
            self.evaluator_id,
            self.name,
            self.status,
            self.evaluator_type,
            self.level,
            self.description
        )
    }
    fn details(&self) -> Vec<(String, String)> {
        let mut rows = vec![
            ("Name".to_string(), self.name.clone()),
            ("ID".to_string(), self.evaluator_id.clone()),
            ("ARN".to_string(), self.arn.clone()),
            ("Type".to_string(), self.evaluator_type.clone()),
            ("Level".to_string(), self.level.clone()),
            ("Status".to_string(), self.status.clone()),
            ("Description".to_string(), self.description.clone()),
            (
                "Locked".to_string(),
                if self.locked { "✓ yes" } else { "✗ no" }.to_string(),
            ),
            ("Created".to_string(), self.created.clone()),
            ("Updated".to_string(), self.updated.clone()),
        ];
        if !self.kms_key_arn.is_empty() {
            rows.push(("KMS Key".to_string(), self.kms_key_arn.clone()));
        }
        rows
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws bedrock-agentcore-control get-evaluator --evaluator-id {}",
            shell_quote(&self.evaluator_id)
        ))
    }
    /// `W` lookup keys. CloudTrail's `ResourceName` for AgentCore may be
    /// either the id or the ARN depending on the event, and keys are tried in
    /// order until one returns events — so carry both (the IAM precedent).
    fn trail_lookup_keys(&self) -> Vec<String> {
        vec![self.evaluator_id.clone(), self.arn.clone()]
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(console_home(region))
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Debug, Clone)]
pub struct AgentCoreOnlineEval {
    pub config_id: String,
    pub arn: String,
    pub name: String,
    pub description: String,
    pub status: String,
    /// ENABLED / DISABLED — an on/off switch, **not** a lifecycle status
    /// (that's `status`). A config can be ACTIVE and DISABLED at once, which
    /// is the case worth spotting: configured, but not evaluating anything.
    pub execution_status: String,
    pub failure_reason: String,
    pub insight_count: usize,
    pub created: String,
    pub updated: String,
    pub tags: HashMap<String, String>,
}

impl AgentCoreOnlineEval {
    pub fn from_sdk(
        c: &aws_sdk_bedrockagentcorecontrol::types::OnlineEvaluationConfigSummary,
    ) -> Self {
        Self {
            config_id: c.online_evaluation_config_id().to_string(),
            arn: c.online_evaluation_config_arn().to_string(),
            name: c.online_evaluation_config_name().to_string(),
            description: c.description().unwrap_or_default().to_string(),
            status: c.status().as_str().to_string(),
            execution_status: c.execution_status().as_str().to_string(),
            failure_reason: c.failure_reason().unwrap_or_default().to_string(),
            insight_count: c.insights().len(),
            created: fmt_dt(Some(c.created_at())),
            updated: fmt_dt(Some(c.updated_at())),
            tags: HashMap::new(),
        }
    }
}

impl Resource for AgentCoreOnlineEval {
    fn id(&self) -> &str {
        &self.config_id
    }
    fn name(&self) -> &str {
        if self.name.is_empty() {
            &self.config_id
        } else {
            &self.name
        }
    }
    fn resource_type(&self) -> &str {
        "AgentCore Online Evaluation"
    }
    fn state(&self) -> ResourceState {
        // A DISABLED config is healthy but idle — surface that rather than the
        // ACTIVE lifecycle status, which would read as "running".
        if self.execution_status.eq_ignore_ascii_case("DISABLED") {
            return ResourceState::Unknown("disabled".to_string());
        }
        map_status(&self.status)
    }

    fn state_label(&self) -> String {
        if self.execution_status.eq_ignore_ascii_case("DISABLED") {
            "disabled".to_string()
        } else {
            native_state_label(&self.status, || self.state())
        }
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        format!(
            "{} {} {} {} {}",
            self.config_id, self.name, self.status, self.execution_status, self.description
        )
    }
    fn details(&self) -> Vec<(String, String)> {
        let mut rows = vec![
            ("Name".to_string(), self.name.clone()),
            ("ID".to_string(), self.config_id.clone()),
            ("ARN".to_string(), self.arn.clone()),
            ("Status".to_string(), self.status.clone()),
            ("Execution".to_string(), self.execution_status.clone()),
            ("Insights".to_string(), self.insight_count.to_string()),
            ("Description".to_string(), self.description.clone()),
            ("Created".to_string(), self.created.clone()),
            ("Updated".to_string(), self.updated.clone()),
        ];
        if !self.failure_reason.is_empty() {
            rows.push((format!("  ⚠ {}", self.failure_reason), String::new()));
        }
        rows
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws bedrock-agentcore-control get-online-evaluation-config --online-evaluation-config-id {}",
            shell_quote(&self.config_id)
        ))
    }
    /// `W` lookup keys. CloudTrail's `ResourceName` for AgentCore may be
    /// either the id or the ARN depending on the event, and keys are tried in
    /// order until one returns events — so carry both (the IAM precedent).
    fn trail_lookup_keys(&self) -> Vec<String> {
        vec![self.config_id.clone(), self.arn.clone()]
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(console_home(region))
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Debug, Clone)]
pub struct AgentCoreDataset {
    pub dataset_id: String,
    pub arn: String,
    pub name: String,
    pub description: String,
    pub status: String,
    pub draft_status: String,
    pub schema_type: String,
    pub example_count: i64,
    pub created: String,
    pub updated: String,
    pub tags: HashMap<String, String>,
}

impl AgentCoreDataset {
    pub fn from_sdk(d: &aws_sdk_bedrockagentcorecontrol::types::DatasetSummary) -> Self {
        Self {
            dataset_id: d.dataset_id().to_string(),
            arn: d.dataset_arn().to_string(),
            name: d.dataset_name().to_string(),
            description: d.description().unwrap_or_default().to_string(),
            status: d.status().as_str().to_string(),
            draft_status: d
                .draft_status()
                .map(|s| s.as_str().to_string())
                .unwrap_or_default(),
            schema_type: d.schema_type().as_str().to_string(),
            example_count: d.example_count(),
            created: fmt_dt(Some(d.created_at())),
            updated: fmt_dt(Some(d.updated_at())),
            tags: HashMap::new(),
        }
    }
}

impl Resource for AgentCoreDataset {
    fn id(&self) -> &str {
        &self.dataset_id
    }
    fn name(&self) -> &str {
        if self.name.is_empty() {
            &self.dataset_id
        } else {
            &self.name
        }
    }
    fn resource_type(&self) -> &str {
        "AgentCore Dataset"
    }
    fn state(&self) -> ResourceState {
        map_status(&self.status)
    }

    fn state_label(&self) -> String {
        native_state_label(&self.status, || self.state())
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        format!(
            "{} {} {} {} {}",
            self.dataset_id, self.name, self.status, self.schema_type, self.description
        )
    }
    fn details(&self) -> Vec<(String, String)> {
        let mut rows = vec![
            ("Name".to_string(), self.name.clone()),
            ("ID".to_string(), self.dataset_id.clone()),
            ("ARN".to_string(), self.arn.clone()),
            ("Status".to_string(), self.status.clone()),
            ("Schema".to_string(), self.schema_type.clone()),
            ("Examples".to_string(), self.example_count.to_string()),
            ("Description".to_string(), self.description.clone()),
            ("Created".to_string(), self.created.clone()),
            ("Updated".to_string(), self.updated.clone()),
        ];
        if !self.draft_status.is_empty() {
            rows.push(("Draft".to_string(), self.draft_status.clone()));
        }
        rows
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws bedrock-agentcore-control get-dataset --dataset-id {}",
            shell_quote(&self.dataset_id)
        ))
    }
    /// `W` lookup keys. CloudTrail's `ResourceName` for AgentCore may be
    /// either the id or the ARN depending on the event, and keys are tried in
    /// order until one returns events — so carry both (the IAM precedent).
    fn trail_lookup_keys(&self) -> Vec<String> {
        vec![self.dataset_id.clone(), self.arn.clone()]
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(console_home(region))
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Registry — the AI resource catalog
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct AgentCoreRegistry {
    pub registry_id: String,
    pub arn: String,
    pub name: String,
    pub description: String,
    pub status: String,
    pub status_reason: String,
    pub authorizer_type: String,
    pub created: String,
    pub updated: String,
    pub tags: HashMap<String, String>,
}

impl AgentCoreRegistry {
    pub fn from_sdk(r: &aws_sdk_bedrockagentcorecontrol::types::RegistrySummary) -> Self {
        Self {
            registry_id: r.registry_id().to_string(),
            arn: r.registry_arn().to_string(),
            name: r.name().to_string(),
            description: r.description().unwrap_or_default().to_string(),
            status: r.status().as_str().to_string(),
            status_reason: r.status_reason().unwrap_or_default().to_string(),
            authorizer_type: r
                .authorizer_type()
                .map(|a| a.as_str().to_string())
                .unwrap_or_default(),
            created: fmt_dt(Some(r.created_at())),
            updated: fmt_dt(Some(r.updated_at())),
            tags: HashMap::new(),
        }
    }
}

crate::sections! {
    pub enum AgentCoreRegistryDetailSection,
    pub static AGENTCORE_REGISTRY_SECTIONS = [
        Overview "Overview",
        Records "Records" => crate::app::App::trigger_agentcore_registry_records_load,
        Tags "Tags" => crate::app::App::trigger_agentcore_tags_load,
    ]
}

impl Resource for AgentCoreRegistry {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&AGENTCORE_REGISTRY_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.registry_id
    }
    fn name(&self) -> &str {
        if self.name.is_empty() {
            &self.registry_id
        } else {
            &self.name
        }
    }
    fn resource_type(&self) -> &str {
        "AgentCore Registry"
    }
    fn state(&self) -> ResourceState {
        map_status(&self.status)
    }

    fn state_label(&self) -> String {
        native_state_label(&self.status, || self.state())
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        format!(
            "{} {} {} {}",
            self.registry_id, self.name, self.status, self.description
        )
    }
    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Name".to_string(), self.name.clone()),
            ("ID".to_string(), self.registry_id.clone()),
            ("ARN".to_string(), self.arn.clone()),
            ("Status".to_string(), self.status.clone()),
            ("Authorizer".to_string(), self.authorizer_type.clone()),
        ]
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws bedrock-agentcore-control get-registry --registry-id {}",
            shell_quote(&self.registry_id)
        ))
    }
    /// `W` lookup keys. CloudTrail's `ResourceName` for AgentCore may be
    /// either the id or the ARN depending on the event, and keys are tried in
    /// order until one returns events — so carry both (the IAM precedent).
    fn trail_lookup_keys(&self) -> Vec<String> {
        vec![self.registry_id.clone(), self.arn.clone()]
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(console_home(region))
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Debug, Clone)]
pub struct AgentCoreRegistryRecord {
    pub record_id: String,
    pub arn: String,
    pub name: String,
    pub description: String,
    /// A2A / MCP / AgentSkills / Custom — what kind of thing is catalogued.
    pub descriptor_type: String,
    pub version: String,
    pub status: String,
    pub created: String,
    pub updated: String,
}

pub async fn fetch_registry_records(
    ctl: CtlClient,
    registry_id: String,
) -> std::result::Result<Vec<AgentCoreRegistryRecord>, String> {
    let mut out = Vec::new();
    let mut pager = ctl
        .list_registry_records()
        .registry_id(&registry_id)
        .into_paginator()
        .send();
    while let Some(page) = pager.next().await {
        let page = page.map_err(|e| crate::error::sdk_error_message(&e))?;
        for r in page.registry_records() {
            out.push(AgentCoreRegistryRecord {
                record_id: r.record_id().to_string(),
                arn: r.record_arn().to_string(),
                name: r.name().to_string(),
                description: r.description().unwrap_or_default().to_string(),
                descriptor_type: r.descriptor_type().as_str().to_string(),
                version: r.record_version().to_string(),
                status: r.status().as_str().to_string(),
                created: fmt_dt(Some(r.created_at())),
                updated: fmt_dt(Some(r.updated_at())),
            });
        }
    }
    Ok(out)
}

// ═══════════════════════════════════════════════════════════════════════════════
// Harness — the managed agent loop
// ═══════════════════════════════════════════════════════════════════════════════
//
// A **harness** is AgentCore's declarative agent: you hand it a model, a system
// prompt, a tool list and a memory binding, and it runs the loop for you —
// where a **runtime** is your own container and the loop is your problem. The
// two share the versions + endpoints shape (`ListHarnessVersions` /
// `ListHarnessEndpoints` mirror the runtime ops), so the pane below is
// deliberately laid out like the runtime's, with the config sections in
// between.

#[derive(Debug, Clone)]
pub struct AgentCoreHarness {
    pub harness_id: String,
    pub arn: String,
    pub name: String,
    pub version: String,
    pub status: String,
    pub created: String,
    pub updated: String,
    pub tags: HashMap<String, String>,
}

impl AgentCoreHarness {
    pub fn from_sdk(h: &aws_sdk_bedrockagentcorecontrol::types::HarnessSummary) -> Self {
        Self {
            harness_id: h.harness_id().to_string(),
            arn: h.arn().to_string(),
            name: h.harness_name().to_string(),
            version: h.harness_version().unwrap_or_default().to_string(),
            status: h.status().as_str().to_string(),
            created: fmt_dt(Some(h.created_at())),
            updated: fmt_dt(Some(h.updated_at())),
            tags: HashMap::new(),
        }
    }
}

crate::sections! {
    pub enum AgentCoreHarnessDetailSection,
    pub static AGENTCORE_HARNESS_SECTIONS = [
        Overview "Overview" => crate::app::App::trigger_agentcore_harness_detail_load,
        Model "Model" => crate::app::App::trigger_agentcore_harness_detail_load,
        Prompt "Prompt" => crate::app::App::trigger_agentcore_harness_detail_load,
        Tools "Tools" => crate::app::App::trigger_agentcore_harness_detail_load,
        Skills "Skills" => crate::app::App::trigger_agentcore_harness_detail_load,
        Memory "Memory" => crate::app::App::trigger_agentcore_harness_detail_load,
        Environment "Environment" => crate::app::App::trigger_agentcore_harness_detail_load,
        Endpoints "Endpoints" => crate::app::App::trigger_agentcore_harness_endpoints_load,
        Versions "Versions" => crate::app::App::trigger_agentcore_harness_versions_load,
        Tags "Tags" => crate::app::App::trigger_agentcore_tags_load,
    ]
}

impl Resource for AgentCoreHarness {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&AGENTCORE_HARNESS_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.harness_id
    }
    fn name(&self) -> &str {
        if self.name.is_empty() {
            &self.harness_id
        } else {
            &self.name
        }
    }
    fn resource_type(&self) -> &str {
        "AgentCore Harness"
    }
    fn state(&self) -> ResourceState {
        map_status(&self.status)
    }

    fn state_label(&self) -> String {
        native_state_label(&self.status, || self.state())
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        format!(
            "{} {} {} {}",
            self.harness_id, self.name, self.status, self.version
        )
    }
    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Name".to_string(), self.name.clone()),
            ("ID".to_string(), self.harness_id.clone()),
            ("ARN".to_string(), self.arn.clone()),
            ("Version".to_string(), self.version.clone()),
            ("Status".to_string(), self.status.clone()),
            ("Created".to_string(), self.created.clone()),
            ("Updated".to_string(), self.updated.clone()),
        ]
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws bedrock-agentcore-control get-harness --harness-id {}",
            shell_quote(&self.harness_id)
        ))
    }
    /// `W` lookup keys. CloudTrail's `ResourceName` for AgentCore may be
    /// either the id or the ARN depending on the event, and keys are tried in
    /// order until one returns events — so carry both (the IAM precedent).
    fn trail_lookup_keys(&self) -> Vec<String> {
        vec![self.harness_id.clone(), self.arn.clone()]
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(console_home(region))
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// One entry of the harness's tool list, flattened out of the
/// `HarnessToolConfiguration` union. `detail` is the one identifier worth a
/// row on its own (an ARN or URL — jumpable); `extra` is everything else.
#[derive(Debug, Clone, Default)]
pub struct AgentCoreHarnessTool {
    pub kind: String,
    pub name: String,
    pub detail_label: String,
    pub detail: String,
    pub extra: Vec<(String, String)>,
}

/// One skill source, flattened out of the `HarnessSkill` union.
#[derive(Debug, Clone, Default)]
pub struct AgentCoreHarnessSkill {
    pub kind: String,
    pub value: String,
    pub extra: Vec<(String, String)>,
}

/// `GetHarness` — the whole agent definition. One fetch feeds seven sections;
/// only Endpoints and Versions have their own calls.
#[derive(Debug, Clone, Default)]
pub struct AgentCoreHarnessDetail {
    pub role_arn: String,
    pub failure_reason: String,

    // ── Model ────────────────────────────────────────────────────────────────
    /// "Bedrock" / "OpenAI" / "Gemini" / "LiteLLM" — which arm of
    /// `HarnessModelConfiguration` was populated.
    pub model_provider: String,
    pub model_id: String,
    pub model_api_format: String,
    pub model_api_base: String,
    /// A Secrets Manager **ARN**, never a key. Third-party providers keep the
    /// key in Secrets Manager and hand AgentCore the pointer; the pointer is
    /// what the API returns and all this pane ever shows.
    pub model_api_key_arn: String,
    pub model_max_tokens: Option<i32>,
    pub model_temperature: Option<f32>,
    pub model_top_p: Option<f32>,
    pub model_top_k: Option<i32>,
    /// Pretty-printed `additional_params` Document, when present.
    pub model_additional_params: String,

    // ── Prompt ───────────────────────────────────────────────────────────────
    pub system_prompt: Vec<String>,

    // ── Tools ────────────────────────────────────────────────────────────────
    pub tools: Vec<AgentCoreHarnessTool>,
    pub allowed_tools: Vec<String>,

    // ── Skills ───────────────────────────────────────────────────────────────
    pub skills: Vec<AgentCoreHarnessSkill>,

    // ── Memory ───────────────────────────────────────────────────────────────
    /// "AgentCore Memory" (an existing store) / "Managed" (one AgentCore
    /// creates and owns) / "Disabled".
    pub memory_kind: String,
    pub memory_arn: String,
    pub memory_actor_id: String,
    pub memory_messages_count: Option<i32>,
    pub memory_strategies: Vec<String>,
    pub memory_event_expiry: Option<i32>,
    pub memory_encryption_key_arn: String,
    /// namespace → the retrieval knobs for it, already formatted.
    pub memory_retrieval: Vec<(String, String)>,

    // ── Environment ──────────────────────────────────────────────────────────
    pub env_runtime_arn: String,
    pub env_runtime_name: String,
    pub env_runtime_id: String,
    pub env_network_mode: String,
    pub env_subnets: Vec<String>,
    pub env_security_groups: Vec<String>,
    pub env_idle_timeout: Option<i32>,
    pub env_max_lifetime: Option<i32>,
    pub env_filesystem_kinds: Vec<String>,
    pub env_artifact_kind: String,
    pub env_artifact_value: String,
    /// Sorted so the pane is stable across fetches (the API returns a map).
    pub environment_variables: Vec<(String, String)>,

    // ── Overview: auth + loop limits ─────────────────────────────────────────
    pub authorizer_kind: String,
    pub jwt_discovery_url: String,
    pub jwt_allowed_audience: Vec<String>,
    pub jwt_allowed_clients: Vec<String>,
    pub jwt_allowed_scopes: Vec<String>,
    pub max_iterations: Option<i32>,
    pub max_tokens: Option<i32>,
    pub timeout_seconds: Option<i32>,
    pub truncation_strategy: String,
    pub truncation_detail: Vec<(String, String)>,
}

pub async fn fetch_harness_detail(
    ctl: CtlClient,
    harness_id: String,
) -> std::result::Result<AgentCoreHarnessDetail, String> {
    use aws_sdk_bedrockagentcorecontrol::types::{
        AuthorizerConfiguration, FilesystemConfiguration, HarnessEnvironmentArtifact,
        HarnessEnvironmentProvider, HarnessGatewayOutboundAuth, HarnessMemoryConfiguration,
        HarnessModelConfiguration, HarnessSkill, HarnessSystemContentBlock,
        HarnessToolConfiguration, HarnessTruncationStrategyConfiguration,
    };

    let out = ctl
        .get_harness()
        .harness_id(&harness_id)
        .send()
        .await
        .map_err(|e| crate::error::sdk_error_message(&e))?;
    let h = out
        .harness()
        .ok_or_else(|| "GetHarness returned no harness".to_string())?;

    let mut d = AgentCoreHarnessDetail {
        role_arn: h.execution_role_arn().to_string(),
        failure_reason: h.failure_reason().unwrap_or_default().to_string(),
        max_iterations: h.max_iterations(),
        max_tokens: h.max_tokens(),
        timeout_seconds: h.timeout_seconds(),
        allowed_tools: h.allowed_tools().to_vec(),
        ..Default::default()
    };

    // ── Model ────────────────────────────────────────────────────────────────
    match h.model() {
        Some(HarnessModelConfiguration::BedrockModelConfig(m)) => {
            d.model_provider = "Bedrock".to_string();
            d.model_id = m.model_id().to_string();
            d.model_api_format = m
                .api_format()
                .map(|f| f.as_str().to_string())
                .unwrap_or_default();
            d.model_max_tokens = m.max_tokens();
            d.model_temperature = m.temperature();
            d.model_top_p = m.top_p();
            if let Some(p) = m.additional_params() {
                d.model_additional_params = crate::aws::document::document_pretty(p);
            }
        }
        Some(HarnessModelConfiguration::OpenAiModelConfig(m)) => {
            d.model_provider = "OpenAI".to_string();
            d.model_id = m.model_id().to_string();
            d.model_api_key_arn = m.api_key_arn().to_string();
            d.model_api_format = m
                .api_format()
                .map(|f| f.as_str().to_string())
                .unwrap_or_default();
            d.model_max_tokens = m.max_tokens();
            d.model_temperature = m.temperature();
            d.model_top_p = m.top_p();
            if let Some(p) = m.additional_params() {
                d.model_additional_params = crate::aws::document::document_pretty(p);
            }
        }
        Some(HarnessModelConfiguration::GeminiModelConfig(m)) => {
            d.model_provider = "Gemini".to_string();
            d.model_id = m.model_id().to_string();
            d.model_api_key_arn = m.api_key_arn().to_string();
            d.model_max_tokens = m.max_tokens();
            d.model_temperature = m.temperature();
            d.model_top_p = m.top_p();
            d.model_top_k = m.top_k();
        }
        Some(HarnessModelConfiguration::LiteLlmModelConfig(m)) => {
            d.model_provider = "LiteLLM".to_string();
            d.model_id = m.model_id().to_string();
            d.model_api_key_arn = m.api_key_arn().unwrap_or_default().to_string();
            d.model_api_base = m.api_base().unwrap_or_default().to_string();
            d.model_max_tokens = m.max_tokens();
            d.model_temperature = m.temperature();
            d.model_top_p = m.top_p();
            if let Some(p) = m.additional_params() {
                d.model_additional_params = crate::aws::document::document_pretty(p);
            }
        }
        _ => {}
    }

    // ── Prompt ───────────────────────────────────────────────────────────────
    for block in h.system_prompt() {
        if let HarnessSystemContentBlock::Text(t) = block {
            d.system_prompt.push(t.clone());
        }
    }

    // ── Tools ────────────────────────────────────────────────────────────────
    for t in h.tools() {
        let mut tool = AgentCoreHarnessTool {
            kind: t.r#type().as_str().to_string(),
            name: t.name().unwrap_or_default().to_string(),
            ..Default::default()
        };
        match t.config() {
            Some(HarnessToolConfiguration::AgentCoreGateway(g)) => {
                tool.detail_label = "Gateway".to_string();
                tool.detail = g.gateway_arn().to_string();
                match g.outbound_auth() {
                    Some(HarnessGatewayOutboundAuth::AwsIam) => {
                        tool.extra.push(("Outbound Auth".to_string(), "IAM".to_string()))
                    }
                    Some(HarnessGatewayOutboundAuth::None) => tool
                        .extra
                        .push(("Outbound Auth".to_string(), "none".to_string())),
                    Some(HarnessGatewayOutboundAuth::Oauth(o)) => {
                        tool.extra.push((
                            "Outbound Auth".to_string(),
                            format!("OAuth ({})", o.grant_type().as_str()),
                        ));
                        tool.extra
                            .push(("Credential Provider".to_string(), o.provider_arn().to_string()));
                        if !o.scopes().is_empty() {
                            tool.extra
                                .push(("Scopes".to_string(), o.scopes().join(", ")));
                        }
                    }
                    _ => {}
                }
            }
            Some(HarnessToolConfiguration::AgentCoreBrowser(b)) => {
                tool.detail_label = "Browser".to_string();
                tool.detail = b.browser_arn().unwrap_or_default().to_string();
            }
            Some(HarnessToolConfiguration::AgentCoreCodeInterpreter(c)) => {
                tool.detail_label = "Code Interpreter".to_string();
                tool.detail = c.code_interpreter_arn().unwrap_or_default().to_string();
            }
            Some(HarnessToolConfiguration::RemoteMcp(m)) => {
                tool.detail_label = "URL".to_string();
                tool.detail = m.url().to_string();
                // Header *names* only. The values are bearer tokens as often
                // as not, and this pane is not the place to print them.
                if let Some(hdrs) = m.headers() {
                    let mut names: Vec<&str> = hdrs.keys().map(|k| k.as_str()).collect();
                    names.sort_unstable();
                    tool.extra
                        .push(("Headers".to_string(), names.join(", ")));
                }
            }
            Some(HarnessToolConfiguration::InlineFunction(f)) => {
                tool.detail_label = "Description".to_string();
                tool.detail = f.description().to_string();
                tool.extra.push((
                    "Input Schema".to_string(),
                    crate::aws::document::document_display(f.input_schema()),
                ));
            }
            _ => {}
        }
        d.tools.push(tool);
    }

    // ── Skills ───────────────────────────────────────────────────────────────
    for s in h.skills() {
        let mut skill = AgentCoreHarnessSkill::default();
        match s {
            HarnessSkill::AwsSkills(a) => {
                skill.kind = "AWS skills".to_string();
                skill.value = a.paths().join(", ");
            }
            HarnessSkill::Git(g) => {
                skill.kind = "Git".to_string();
                skill.value = g.url().to_string();
                if let Some(p) = g.path() {
                    skill.extra.push(("Path".to_string(), p.to_string()));
                }
                if let Some(auth) = g.auth() {
                    skill
                        .extra
                        .push(("Credential".to_string(), auth.credential_arn().to_string()));
                    if let Some(u) = auth.username() {
                        skill.extra.push(("Username".to_string(), u.to_string()));
                    }
                }
            }
            HarnessSkill::S3(s3) => {
                skill.kind = "S3".to_string();
                skill.value = s3.uri().to_string();
            }
            HarnessSkill::Path(p) => {
                skill.kind = "Path".to_string();
                skill.value = p.clone();
            }
            _ => skill.kind = "unknown".to_string(),
        }
        d.skills.push(skill);
    }

    // ── Memory ───────────────────────────────────────────────────────────────
    match h.memory() {
        Some(HarnessMemoryConfiguration::AgentCoreMemoryConfiguration(m)) => {
            d.memory_kind = "AgentCore Memory".to_string();
            d.memory_arn = m.arn().to_string();
            d.memory_actor_id = m.actor_id().unwrap_or_default().to_string();
            d.memory_messages_count = m.messages_count();
            if let Some(rc) = m.retrieval_config() {
                let mut v: Vec<(String, String)> = rc
                    .iter()
                    .map(|(ns, cfg)| {
                        let mut parts = Vec::new();
                        if let Some(k) = cfg.top_k() {
                            parts.push(format!("top {}", k));
                        }
                        if let Some(s) = cfg.relevance_score() {
                            parts.push(format!("score ≥ {}", s));
                        }
                        if let Some(sid) = cfg.strategy_id() {
                            parts.push(format!("strategy {}", sid));
                        }
                        (ns.clone(), parts.join(" · "))
                    })
                    .collect();
                v.sort_by(|a, b| a.0.cmp(&b.0));
                d.memory_retrieval = v;
            }
        }
        Some(HarnessMemoryConfiguration::ManagedMemoryConfiguration(m)) => {
            d.memory_kind = "Managed".to_string();
            d.memory_arn = m.arn().unwrap_or_default().to_string();
            d.memory_strategies = m
                .strategies()
                .iter()
                .map(|s| s.as_str().to_string())
                .collect();
            d.memory_event_expiry = m.event_expiry_duration();
            d.memory_encryption_key_arn = m.encryption_key_arn().unwrap_or_default().to_string();
        }
        Some(HarnessMemoryConfiguration::Disabled(_)) => {
            d.memory_kind = "Disabled".to_string();
        }
        _ => {}
    }

    // ── Environment ──────────────────────────────────────────────────────────
    if let Some(HarnessEnvironmentProvider::AgentCoreRuntimeEnvironment(e)) = h.environment() {
        d.env_runtime_arn = e.agent_runtime_arn().to_string();
        d.env_runtime_name = e.agent_runtime_name().to_string();
        d.env_runtime_id = e.agent_runtime_id().to_string();
        if let Some(l) = e.lifecycle_configuration() {
            d.env_idle_timeout = l.idle_runtime_session_timeout();
            d.env_max_lifetime = l.max_lifetime();
        }
        if let Some(n) = e.network_configuration() {
            d.env_network_mode = n.network_mode().as_str().to_string();
            if let Some(v) = n.network_mode_config() {
                d.env_subnets = v.subnets().to_vec();
                d.env_security_groups = v.security_groups().to_vec();
            }
        }
        for f in e.filesystem_configurations() {
            d.env_filesystem_kinds.push(
                match f {
                    FilesystemConfiguration::EfsAccessPoint(_) => "EFS access point",
                    FilesystemConfiguration::S3FilesAccessPoint(_) => "S3 files access point",
                    FilesystemConfiguration::SessionStorage(_) => "Session storage",
                    _ => "unknown",
                }
                .to_string(),
            );
        }
    }
    if let Some(HarnessEnvironmentArtifact::ContainerConfiguration(c)) = h.environment_artifact() {
        d.env_artifact_kind = "Container".to_string();
        d.env_artifact_value = c.container_uri().to_string();
    }
    if let Some(envs) = h.environment_variables() {
        let mut v: Vec<(String, String)> = envs
            .iter()
            .map(|(k, val)| (k.clone(), val.clone()))
            .collect();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        d.environment_variables = v;
    }

    // ── Auth + truncation ────────────────────────────────────────────────────
    if let Some(AuthorizerConfiguration::CustomJwtAuthorizer(j)) = h.authorizer_configuration() {
        d.authorizer_kind = "Custom JWT".to_string();
        d.jwt_discovery_url = j.discovery_url().to_string();
        d.jwt_allowed_audience = j.allowed_audience().to_vec();
        d.jwt_allowed_clients = j.allowed_clients().to_vec();
        d.jwt_allowed_scopes = j.allowed_scopes().to_vec();
    }
    if let Some(t) = h.truncation() {
        d.truncation_strategy = t.strategy().as_str().to_string();
        match t.config() {
            Some(HarnessTruncationStrategyConfiguration::SlidingWindow(w)) => {
                if let Some(c) = w.messages_count() {
                    d.truncation_detail
                        .push(("Window".to_string(), format!("{} messages", c)));
                }
            }
            Some(HarnessTruncationStrategyConfiguration::Summarization(s)) => {
                if let Some(r) = s.summary_ratio() {
                    d.truncation_detail
                        .push(("Summary Ratio".to_string(), format!("{}", r)));
                }
                if let Some(p) = s.preserve_recent_messages() {
                    d.truncation_detail
                        .push(("Preserve Recent".to_string(), format!("{} messages", p)));
                }
                if let Some(p) = s.summarization_system_prompt() {
                    d.truncation_detail
                        .push(("Summarization Prompt".to_string(), p.to_string()));
                }
            }
            _ => {}
        }
    }

    Ok(d)
}

#[derive(Debug, Clone)]
pub struct AgentCoreHarnessEndpoint {
    pub name: String,
    pub arn: String,
    pub status: String,
    pub live_version: String,
    pub target_version: String,
    pub description: String,
    pub failure_reason: String,
    pub updated: String,
}

pub async fn fetch_harness_endpoints(
    ctl: CtlClient,
    harness_id: String,
) -> std::result::Result<Vec<AgentCoreHarnessEndpoint>, String> {
    let mut out = Vec::new();
    let mut pager = ctl
        .list_harness_endpoints()
        .harness_id(&harness_id)
        .into_paginator()
        .send();
    while let Some(page) = pager.next().await {
        let page = page.map_err(|e| crate::error::sdk_error_message(&e))?;
        for e in page.endpoints() {
            out.push(AgentCoreHarnessEndpoint {
                name: e.endpoint_name().to_string(),
                arn: e.arn().to_string(),
                status: e.status().as_str().to_string(),
                live_version: e.live_version().unwrap_or_default().to_string(),
                target_version: e.target_version().unwrap_or_default().to_string(),
                description: e.description().unwrap_or_default().to_string(),
                failure_reason: e.failure_reason().unwrap_or_default().to_string(),
                updated: fmt_dt(Some(e.updated_at())),
            });
        }
    }
    Ok(out)
}

#[derive(Debug, Clone)]
pub struct AgentCoreHarnessVersion {
    pub version: String,
    pub status: String,
    pub failure_reason: String,
    pub updated: String,
}

pub async fn fetch_harness_versions(
    ctl: CtlClient,
    harness_id: String,
) -> std::result::Result<Vec<AgentCoreHarnessVersion>, String> {
    let mut out = Vec::new();
    let mut pager = ctl
        .list_harness_versions()
        .harness_id(&harness_id)
        .into_paginator()
        .send();
    while let Some(page) = pager.next().await {
        let page = page.map_err(|e| crate::error::sdk_error_message(&e))?;
        for v in page.harness_versions() {
            out.push(AgentCoreHarnessVersion {
                version: v.harness_version().to_string(),
                status: v.status().as_str().to_string(),
                failure_reason: v.failure_reason().unwrap_or_default().to_string(),
                updated: fmt_dt(Some(v.updated_at())),
            });
        }
    }
    // Newest first, numerically — same shape as runtime versions.
    out.sort_by(|a, b| match (a.version.parse::<u64>(), b.version.parse::<u64>()) {
        (Ok(x), Ok(y)) => y.cmp(&x),
        _ => b.version.cmp(&a.version),
    });
    Ok(out)
}

// ═══════════════════════════════════════════════════════════════════════════════
// Configuration bundles
// ═══════════════════════════════════════════════════════════════════════════════
//
// A versioned blob of component configuration that a runtime endpoint's
// traffic split points at (`TrafficSplitEntry.configurationBundle`), which is
// why these live in the **Runtimes** tab rather than earning a digit key of
// their own: a bundle only means anything next to the endpoint routing it.
// The components themselves are untyped `Document`s, so the pane pretty-prints
// them rather than pretending to know the schema.

#[derive(Debug, Clone)]
pub struct AgentCoreConfigBundle {
    pub bundle_id: String,
    pub arn: String,
    pub name: String,
    pub description: String,
    pub created: String,
    pub tags: HashMap<String, String>,
}

impl AgentCoreConfigBundle {
    pub fn from_sdk(b: &aws_sdk_bedrockagentcorecontrol::types::ConfigurationBundleSummary) -> Self {
        Self {
            bundle_id: b.bundle_id().to_string(),
            arn: b.bundle_arn().to_string(),
            name: b.bundle_name().to_string(),
            description: b.description().unwrap_or_default().to_string(),
            created: fmt_dt(b.created_at()),
            tags: HashMap::new(),
        }
    }
}

crate::sections! {
    pub enum AgentCoreConfigBundleDetailSection,
    pub static AGENTCORE_BUNDLE_SECTIONS = [
        Overview "Overview" => crate::app::App::trigger_agentcore_bundle_detail_load,
        Components "Components" => crate::app::App::trigger_agentcore_bundle_detail_load,
        Versions "Versions" => crate::app::App::trigger_agentcore_bundle_versions_load,
        Tags "Tags" => crate::app::App::trigger_agentcore_tags_load,
    ]
}

impl Resource for AgentCoreConfigBundle {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&AGENTCORE_BUNDLE_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.bundle_id
    }
    fn name(&self) -> &str {
        if self.name.is_empty() {
            &self.bundle_id
        } else {
            &self.name
        }
    }
    fn resource_type(&self) -> &str {
        "AgentCore Config Bundle"
    }
    /// `ListConfigurationBundles` reports no status — a bundle is a versioned
    /// blob, not a lifecycle resource — so there is nothing for the state
    /// column to say.
    fn state(&self) -> ResourceState {
        ResourceState::Unknown(String::new())
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        format!(
            "{} {} {}",
            self.bundle_id, self.name, self.description
        )
    }
    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Name".to_string(), self.name.clone()),
            ("ID".to_string(), self.bundle_id.clone()),
            ("ARN".to_string(), self.arn.clone()),
            ("Description".to_string(), self.description.clone()),
            ("Created".to_string(), self.created.clone()),
        ]
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws bedrock-agentcore-control get-configuration-bundle --bundle-id {}",
            shell_quote(&self.bundle_id)
        ))
    }
    /// `W` lookup keys. CloudTrail's `ResourceName` for AgentCore may be
    /// either the id or the ARN depending on the event, and keys are tried in
    /// order until one returns events — so carry both (the IAM precedent).
    fn trail_lookup_keys(&self) -> Vec<String> {
        vec![self.bundle_id.clone(), self.arn.clone()]
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(console_home(region))
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// `GetConfigurationBundle` — the live version plus its component blobs.
#[derive(Debug, Clone, Default)]
pub struct AgentCoreConfigBundleDetail {
    pub version_id: String,
    pub kms_key_arn: String,
    pub updated: String,
    pub branch_name: String,
    pub commit_message: String,
    pub created_by: String,
    pub parent_versions: Vec<String>,
    /// component name → pretty-printed configuration Document, sorted by name.
    pub components: Vec<(String, String)>,
}

pub async fn fetch_bundle_detail(
    ctl: CtlClient,
    bundle_id: String,
) -> std::result::Result<AgentCoreConfigBundleDetail, String> {
    let b = ctl
        .get_configuration_bundle()
        .bundle_id(&bundle_id)
        .send()
        .await
        .map_err(|e| crate::error::sdk_error_message(&e))?;

    let mut d = AgentCoreConfigBundleDetail {
        version_id: b.version_id().to_string(),
        kms_key_arn: b.kms_key_arn().unwrap_or_default().to_string(),
        updated: fmt_dt(Some(b.updated_at())),
        ..Default::default()
    };
    if let Some(m) = b.lineage_metadata() {
        d.branch_name = m.branch_name().unwrap_or_default().to_string();
        d.commit_message = m.commit_message().unwrap_or_default().to_string();
        d.created_by = m.created_by().map(|c| c.name().to_string()).unwrap_or_default();
        d.parent_versions = m.parent_version_ids().to_vec();
    }
    let mut comps: Vec<(String, String)> = b
        .components()
        .iter()
        .map(|(k, v)| (k.clone(), crate::aws::document::document_pretty(v.configuration())))
        .collect();
    comps.sort_by(|a, b| a.0.cmp(&b.0));
    d.components = comps;
    Ok(d)
}

#[derive(Debug, Clone)]
pub struct AgentCoreConfigBundleVersion {
    pub version_id: String,
    pub created: String,
    pub branch_name: String,
    pub commit_message: String,
    pub created_by: String,
    pub parent_versions: Vec<String>,
}

pub async fn fetch_bundle_versions(
    ctl: CtlClient,
    bundle_id: String,
) -> std::result::Result<Vec<AgentCoreConfigBundleVersion>, String> {
    let mut out = Vec::new();
    let mut pager = ctl
        .list_configuration_bundle_versions()
        .bundle_id(&bundle_id)
        .into_paginator()
        .send();
    while let Some(page) = pager.next().await {
        let page = page.map_err(|e| crate::error::sdk_error_message(&e))?;
        for v in page.versions() {
            let (branch_name, commit_message, created_by, parent_versions) = match v
                .lineage_metadata()
            {
                Some(m) => (
                    m.branch_name().unwrap_or_default().to_string(),
                    m.commit_message().unwrap_or_default().to_string(),
                    m.created_by().map(|c| c.name().to_string()).unwrap_or_default(),
                    m.parent_version_ids().to_vec(),
                ),
                None => (String::new(), String::new(), String::new(), Vec::new()),
            };
            out.push(AgentCoreConfigBundleVersion {
                version_id: v.version_id().to_string(),
                created: fmt_dt(Some(v.version_created_at())),
                branch_name,
                commit_message,
                created_by,
                parent_versions,
            });
        }
    }
    Ok(out)
}

// ═══════════════════════════════════════════════════════════════════════════════
// Payments — agentic payments
// ═══════════════════════════════════════════════════════════════════════════════
//
// **Metadata only, deliberately.** The data plane
// (`bedrock-agentcore`) carries `GetPaymentInstrument`,
// `GetPaymentInstrumentBalance` and `GetResourcePaymentToken`; none of them is
// called here and none should be. They return spendable material — the same
// line neboto draws around Secrets Manager values, which are fetched only on an
// explicit `x`/`Y` and never stored. There is no `x` gate for payments because
// there is no read of a payment instrument this browser needs at all.
//
// What is listed is the *configuration*: managers (the authorizer + role a
// payment flow runs under), their connectors (which vendor, scoped per
// manager, so they hang off the manager's pane), and the account's credential
// providers. Vendor credentials come back as **Secrets Manager ARNs** —
// pointers, not keys — and those are what the pane shows.

#[derive(Debug, Clone)]
pub struct AgentCorePaymentManager {
    pub manager_id: String,
    pub arn: String,
    pub name: String,
    pub description: String,
    pub authorizer_type: String,
    pub role_arn: String,
    pub status: String,
    pub created: String,
    pub updated: String,
    pub tags: HashMap<String, String>,
}

impl AgentCorePaymentManager {
    pub fn from_sdk(m: &aws_sdk_bedrockagentcorecontrol::types::PaymentManagerSummary) -> Self {
        Self {
            manager_id: m.payment_manager_id().to_string(),
            arn: m.payment_manager_arn().to_string(),
            name: m.name().to_string(),
            description: m.description().unwrap_or_default().to_string(),
            authorizer_type: m.authorizer_type().as_str().to_string(),
            role_arn: m.role_arn().to_string(),
            status: m.status().as_str().to_string(),
            created: fmt_dt(m.created_at()),
            updated: fmt_dt(Some(m.last_updated_at())),
            tags: HashMap::new(),
        }
    }
}

crate::sections! {
    pub enum AgentCorePaymentManagerDetailSection,
    pub static AGENTCORE_PAYMENT_MANAGER_SECTIONS = [
        Overview "Overview" => crate::app::App::trigger_agentcore_payment_detail_load,
        Auth "Auth" => crate::app::App::trigger_agentcore_payment_detail_load,
        Connectors "Connectors" => crate::app::App::trigger_agentcore_payment_connectors_load,
        Tags "Tags" => crate::app::App::trigger_agentcore_tags_load,
    ]
}

impl Resource for AgentCorePaymentManager {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&AGENTCORE_PAYMENT_MANAGER_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.manager_id
    }
    fn name(&self) -> &str {
        if self.name.is_empty() {
            &self.manager_id
        } else {
            &self.name
        }
    }
    fn resource_type(&self) -> &str {
        "AgentCore Payment Manager"
    }
    fn state(&self) -> ResourceState {
        map_status(&self.status)
    }

    fn state_label(&self) -> String {
        native_state_label(&self.status, || self.state())
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        format!(
            "{} {} {} {} {}",
            self.manager_id, self.name, self.status, self.authorizer_type, self.description
        )
    }
    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Name".to_string(), self.name.clone()),
            ("ID".to_string(), self.manager_id.clone()),
            ("ARN".to_string(), self.arn.clone()),
            ("Authorizer".to_string(), self.authorizer_type.clone()),
            ("Role".to_string(), self.role_arn.clone()),
            ("Status".to_string(), self.status.clone()),
        ]
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws bedrock-agentcore-control get-payment-manager --payment-manager-id {}",
            shell_quote(&self.manager_id)
        ))
    }
    /// `W` lookup keys. CloudTrail's `ResourceName` for AgentCore may be
    /// either the id or the ARN depending on the event, and keys are tried in
    /// order until one returns events — so carry both (the IAM precedent).
    fn trail_lookup_keys(&self) -> Vec<String> {
        vec![self.manager_id.clone(), self.arn.clone()]
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(console_home(region))
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Debug, Clone)]
pub struct AgentCorePaymentCredProvider {
    pub arn: String,
    pub name: String,
    pub vendor: String,
    pub created: String,
    pub updated: String,
    pub tags: HashMap<String, String>,
}

impl AgentCorePaymentCredProvider {
    pub fn from_sdk(
        p: &aws_sdk_bedrockagentcorecontrol::types::PaymentCredentialProviderItem,
    ) -> Self {
        Self {
            arn: p.credential_provider_arn().to_string(),
            name: p.name().to_string(),
            vendor: p.credential_provider_vendor().as_str().to_string(),
            created: fmt_dt(Some(p.created_time())),
            updated: fmt_dt(Some(p.last_updated_time())),
            tags: HashMap::new(),
        }
    }
}

crate::sections! {
    pub enum AgentCorePaymentCredProviderDetailSection,
    pub static AGENTCORE_PAYMENT_CRED_SECTIONS = [
        Overview "Overview" => crate::app::App::trigger_agentcore_payment_detail_load,
        Vendor "Vendor" => crate::app::App::trigger_agentcore_payment_detail_load,
        Tags "Tags" => crate::app::App::trigger_agentcore_tags_load,
    ]
}

impl Resource for AgentCorePaymentCredProvider {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&AGENTCORE_PAYMENT_CRED_SECTIONS)
    }
    /// The list op returns no separate id, so the ARN is the identity — which
    /// is also the key `GetPaymentCredentialProvider` wants.
    fn id(&self) -> &str {
        &self.arn
    }
    fn name(&self) -> &str {
        if self.name.is_empty() {
            &self.arn
        } else {
            &self.name
        }
    }
    fn resource_type(&self) -> &str {
        "AgentCore Payment Credentials"
    }
    /// The listing carries no status; the vendor is the useful discriminator,
    /// so that is what the state column shows.
    fn state(&self) -> ResourceState {
        ResourceState::Unknown(self.vendor.to_lowercase())
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        format!("{} {} {}", self.name, self.vendor, self.arn)
    }
    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Name".to_string(), self.name.clone()),
            ("Vendor".to_string(), self.vendor.clone()),
            ("ARN".to_string(), self.arn.clone()),
            ("Created".to_string(), self.created.clone()),
            ("Updated".to_string(), self.updated.clone()),
        ]
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws bedrock-agentcore-control get-payment-credential-provider --name {}",
            shell_quote(&self.name)
        ))
    }
    /// `W` lookup keys. CloudTrail's `ResourceName` for AgentCore may be
    /// either the id or the ARN depending on the event, and keys are tried in
    /// order until one returns events — so carry both (the IAM precedent).
    fn trail_lookup_keys(&self) -> Vec<String> {
        vec![self.arn.clone(), self.name.clone()]
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(console_home(region))
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// One bundle for both payment panes, the way `AgentCoreToolDetail` serves the
/// browser and the code interpreter: the two `Get*` shapes barely overlap, but
/// they are never both wanted at once and one `LazyMap` keyed by id/ARN keeps
/// the store flat.
#[derive(Debug, Clone, Default)]
pub struct AgentCorePaymentDetail {
    // Manager
    pub workload_identity_arn: String,
    pub authorizer_kind: String,
    pub jwt_discovery_url: String,
    pub jwt_allowed_audience: Vec<String>,
    pub jwt_allowed_clients: Vec<String>,
    pub jwt_allowed_scopes: Vec<String>,
    // Credential provider
    pub vendor_kind: String,
    /// The vendor's non-secret identifiers (app id, API key id, …).
    pub vendor_fields: Vec<(String, String)>,
    /// Every secret this provider points at, as `(label, Secrets Manager ARN)`
    /// — the API returns pointers only, and so does this pane.
    pub vendor_secret_refs: Vec<(String, String)>,
}

pub async fn fetch_payment_manager_detail(
    ctl: CtlClient,
    manager_id: String,
) -> std::result::Result<AgentCorePaymentDetail, String> {
    use aws_sdk_bedrockagentcorecontrol::types::AuthorizerConfiguration;

    let m = ctl
        .get_payment_manager()
        .payment_manager_id(&manager_id)
        .send()
        .await
        .map_err(|e| crate::error::sdk_error_message(&e))?;

    let mut d = AgentCorePaymentDetail {
        workload_identity_arn: m
            .workload_identity_details()
            .map(|w| w.workload_identity_arn().to_string())
            .unwrap_or_default(),
        ..Default::default()
    };
    if let Some(AuthorizerConfiguration::CustomJwtAuthorizer(j)) = m.authorizer_configuration() {
        d.authorizer_kind = "Custom JWT".to_string();
        d.jwt_discovery_url = j.discovery_url().to_string();
        d.jwt_allowed_audience = j.allowed_audience().to_vec();
        d.jwt_allowed_clients = j.allowed_clients().to_vec();
        d.jwt_allowed_scopes = j.allowed_scopes().to_vec();
    }
    Ok(d)
}

pub async fn fetch_payment_cred_provider_detail(
    ctl: CtlClient,
    name: String,
) -> std::result::Result<AgentCorePaymentDetail, String> {
    use aws_sdk_bedrockagentcorecontrol::types::PaymentProviderConfigurationOutput as Cfg;

    let p = ctl
        .get_payment_credential_provider()
        .name(&name)
        .send()
        .await
        .map_err(|e| crate::error::sdk_error_message(&e))?;

    let mut d = AgentCorePaymentDetail::default();
    // Every `*_secret_arn` below is an `aws_smithy_types`-free `Secret { secret_arn }`
    // — a Secrets Manager reference. Nothing here is the credential itself.
    match p.provider_configuration_output() {
        Some(Cfg::CoinbaseCdpConfiguration(c)) => {
            d.vendor_kind = "Coinbase CDP".to_string();
            d.vendor_fields
                .push(("API Key ID".to_string(), c.api_key_id().to_string()));
            if let Some(s) = c.api_key_secret_arn() {
                d.vendor_secret_refs
                    .push(("API Key Secret".to_string(), s.secret_arn().to_string()));
            }
            if let Some(k) = c.api_key_secret_json_key() {
                d.vendor_fields
                    .push(("API Key JSON Key".to_string(), k.to_string()));
            }
            if let Some(src) = c.api_key_secret_source() {
                d.vendor_fields
                    .push(("API Key Source".to_string(), src.as_str().to_string()));
            }
            if let Some(s) = c.wallet_secret_arn() {
                d.vendor_secret_refs
                    .push(("Wallet Secret".to_string(), s.secret_arn().to_string()));
            }
            if let Some(k) = c.wallet_secret_json_key() {
                d.vendor_fields
                    .push(("Wallet JSON Key".to_string(), k.to_string()));
            }
            if let Some(src) = c.wallet_secret_source() {
                d.vendor_fields
                    .push(("Wallet Source".to_string(), src.as_str().to_string()));
            }
        }
        Some(Cfg::StripePrivyConfiguration(s)) => {
            d.vendor_kind = "Stripe / Privy".to_string();
            d.vendor_fields
                .push(("App ID".to_string(), s.app_id().to_string()));
            d.vendor_fields.push((
                "Authorization ID".to_string(),
                s.authorization_id().to_string(),
            ));
            if let Some(a) = s.app_secret_arn() {
                d.vendor_secret_refs
                    .push(("App Secret".to_string(), a.secret_arn().to_string()));
            }
            if let Some(k) = s.app_secret_json_key() {
                d.vendor_fields
                    .push(("App Secret JSON Key".to_string(), k.to_string()));
            }
            if let Some(src) = s.app_secret_source() {
                d.vendor_fields
                    .push(("App Secret Source".to_string(), src.as_str().to_string()));
            }
            if let Some(a) = s.authorization_private_key_arn() {
                d.vendor_secret_refs.push((
                    "Authorization Private Key".to_string(),
                    a.secret_arn().to_string(),
                ));
            }
            if let Some(k) = s.authorization_private_key_json_key() {
                d.vendor_fields
                    .push(("Auth Key JSON Key".to_string(), k.to_string()));
            }
            if let Some(src) = s.authorization_private_key_source() {
                d.vendor_fields
                    .push(("Auth Key Source".to_string(), src.as_str().to_string()));
            }
        }
        _ => {}
    }
    Ok(d)
}

/// A manager's connectors. `ListPaymentConnectors` is scoped by
/// `paymentManagerId`, so this is a section on the manager's pane rather than
/// a list of its own — there is no account-wide connector listing to build a
/// tab from.
#[derive(Debug, Clone)]
pub struct AgentCorePaymentConnector {
    pub connector_id: String,
    pub name: String,
    pub kind: String,
    pub status: String,
    pub updated: String,
    /// `GetPaymentConnector` — which credential providers it draws on, by ARN.
    pub credential_provider_arns: Vec<String>,
    pub description: String,
}

pub async fn fetch_payment_connectors(
    ctl: CtlClient,
    manager_id: String,
) -> std::result::Result<Vec<AgentCorePaymentConnector>, String> {
    use aws_sdk_bedrockagentcorecontrol::types::CredentialsProviderConfiguration as Cpc;

    let mut out = Vec::new();
    let mut pager = ctl
        .list_payment_connectors()
        .payment_manager_id(&manager_id)
        .into_paginator()
        .send();
    while let Some(page) = pager.next().await {
        let page = page.map_err(|e| crate::error::sdk_error_message(&e))?;
        for c in page.payment_connectors() {
            out.push(AgentCorePaymentConnector {
                connector_id: c.payment_connector_id().to_string(),
                name: c.name().to_string(),
                kind: c.r#type().as_str().to_string(),
                status: c.status().as_str().to_string(),
                updated: fmt_dt(Some(c.last_updated_at())),
                credential_provider_arns: Vec::new(),
                description: String::new(),
            });
        }
    }

    // Second pass for the bits only `GetPaymentConnector` carries. Capped like
    // the gateway targets — a manager can front many connectors and this is a
    // call per row.
    for c in out.iter_mut().take(MAX_PAYMENT_CONNECTORS) {
        if let Ok(full) = ctl
            .get_payment_connector()
            .payment_connector_id(&c.connector_id)
            .send()
            .await
        {
            c.description = full.description().unwrap_or_default().to_string();
            for cfg in full.credential_provider_configurations() {
                let arn = match cfg {
                    Cpc::CoinbaseCdp(p) | Cpc::StripePrivy(p) => {
                        p.credential_provider_arn().to_string()
                    }
                    _ => continue,
                };
                c.credential_provider_arns.push(arn);
            }
        }
    }
    Ok(out)
}

// ═══════════════════════════════════════════════════════════════════════════════
// Identity detail — workload identities + both credential-provider kinds
// ═══════════════════════════════════════════════════════════════════════════════
//
// The three Identity list ops are the thinnest in the service:
// `WorkloadIdentityType` is **two fields** (name + ARN), and the two provider
// items add only a vendor and timestamps. Everything that makes an identity
// intelligible — the OAuth2 authorization server, the client id, which secret
// backs it, the allowed return URLs — lives behind `Get*`, so all three panes
// are lazy over one shared bundle (the `AgentCoreToolDetail` precedent).
//
// All three `Get*` calls key off **name**, while the map keys off **ARN**: the
// three families share one `LazyMap` and a workload identity could in
// principle share a name with a credential provider.
//
// Nothing here reads a credential. The provider responses carry a
// `Secret { secretArn }` — a Secrets Manager pointer — and the pane shows the
// pointer. There is no API that returns an OAuth2 client secret or an API key,
// and this app calls none of the token-vault issue operations either.

#[derive(Debug, Clone, Default)]
pub struct AgentCoreIdentityDetail {
    pub created: String,
    pub updated: String,

    // ── Workload identity ────────────────────────────────────────────────────
    pub return_urls: Vec<String>,

    // ── OAuth2 provider ──────────────────────────────────────────────────────
    pub vendor: String,
    pub status: String,
    pub failure_reason: String,
    pub callback_url: String,
    pub client_id: String,
    /// The two `Oauth2Discovery` arms: either a discovery URL, or the server
    /// metadata spelled out. Both are populated when present — a provider
    /// configured with inline metadata has no URL and vice versa.
    pub discovery_url: String,
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub response_types: Vec<String>,
    pub token_endpoint_auth_methods: Vec<String>,
    /// Custom providers only — the other eight arms carry discovery + client id
    /// and nothing else.
    pub client_auth_method: String,
    pub on_behalf_of_grant_type: String,
    pub on_behalf_of_token_content: String,
    pub on_behalf_of_scopes: Vec<String>,
    pub private_endpoint: String,
    pub private_endpoint_detail: Vec<(String, String)>,
    pub private_endpoint_overrides: Vec<(String, String)>,

    // ── Either provider kind ─────────────────────────────────────────────────
    /// A Secrets Manager ARN. A pointer, never the credential.
    pub secret_arn: String,
    pub secret_json_key: String,
    /// MANAGED (AgentCore created and owns the secret) vs EXTERNAL (you
    /// brought your own) — the field that says who can rotate it.
    pub secret_source: String,
}

/// Flatten one `PrivateEndpoint` union arm into (kind, detail rows).
fn private_endpoint_summary(
    ep: &aws_sdk_bedrockagentcorecontrol::types::PrivateEndpoint,
) -> (String, Vec<(String, String)>) {
    use aws_sdk_bedrockagentcorecontrol::types::PrivateEndpoint;
    match ep {
        PrivateEndpoint::ManagedVpcResource(v) => {
            let mut rows = vec![("VPC".to_string(), v.vpc_identifier().to_string())];
            if !v.subnet_ids().is_empty() {
                rows.push(("Subnets".to_string(), v.subnet_ids().join(", ")));
            }
            if !v.security_group_ids().is_empty() {
                rows.push((
                    "Security Groups".to_string(),
                    v.security_group_ids().join(", "),
                ));
            }
            rows.push((
                "IP Address Type".to_string(),
                v.endpoint_ip_address_type().as_str().to_string(),
            ));
            if let Some(d) = v.routing_domain() {
                rows.push(("Routing Domain".to_string(), d.to_string()));
            }
            ("Managed VPC".to_string(), rows)
        }
        PrivateEndpoint::SelfManagedLatticeResource(l) => {
            let rows = l
                .as_resource_configuration_identifier()
                .ok()
                .map(|id| vec![("Resource Configuration".to_string(), id.clone())])
                .unwrap_or_default();
            ("Self-managed (VPC Lattice)".to_string(), rows)
        }
        _ => ("unknown".to_string(), Vec::new()),
    }
}

pub async fn fetch_workload_identity_detail(
    ctl: CtlClient,
    name: String,
) -> std::result::Result<AgentCoreIdentityDetail, String> {
    let w = ctl
        .get_workload_identity()
        .name(&name)
        .send()
        .await
        .map_err(|e| crate::error::sdk_error_message(&e))?;
    Ok(AgentCoreIdentityDetail {
        created: fmt_dt(Some(w.created_time())),
        updated: fmt_dt(Some(w.last_updated_time())),
        return_urls: w.allowed_resource_oauth2_return_urls().to_vec(),
        ..Default::default()
    })
}

pub async fn fetch_oauth2_provider_detail(
    ctl: CtlClient,
    name: String,
) -> std::result::Result<AgentCoreIdentityDetail, String> {
    use aws_sdk_bedrockagentcorecontrol::types::{Oauth2Discovery, Oauth2ProviderConfigOutput};

    let p = ctl
        .get_oauth2_credential_provider()
        .name(&name)
        .send()
        .await
        .map_err(|e| crate::error::sdk_error_message(&e))?;

    let mut d = AgentCoreIdentityDetail {
        created: fmt_dt(Some(p.created_time())),
        updated: fmt_dt(Some(p.last_updated_time())),
        vendor: p.credential_provider_vendor().as_str().to_string(),
        status: p
            .status()
            .map(|s| s.as_str().to_string())
            .unwrap_or_default(),
        failure_reason: p.failure_reason().unwrap_or_default().to_string(),
        callback_url: p.callback_url().unwrap_or_default().to_string(),
        secret_arn: p
            .client_secret_arn()
            .map(|s| s.secret_arn().to_string())
            .unwrap_or_default(),
        secret_json_key: p.client_secret_json_key().unwrap_or_default().to_string(),
        secret_source: p
            .client_secret_source()
            .map(|s| s.as_str().to_string())
            .unwrap_or_default(),
        ..Default::default()
    };

    // Eight of the nine vendor arms are the same shape (discovery + client
    // id); only Custom carries the private-endpoint / token-exchange extras.
    // Bind the common pair once instead of nine near-identical branches.
    let (discovery, client_id) = match p.oauth2_provider_config_output() {
        Some(Oauth2ProviderConfigOutput::CustomOauth2ProviderConfig(c)) => {
            if let Some(m) = c.client_authentication_method() {
                d.client_auth_method = m.as_str().to_string();
            }
            if let Some(o) = c.on_behalf_of_token_exchange_config() {
                d.on_behalf_of_grant_type = o.grant_type().as_str().to_string();
                if let Some(cfg) = o.token_exchange_grant_type_config() {
                    d.on_behalf_of_token_content = cfg.actor_token_content().as_str().to_string();
                    d.on_behalf_of_scopes = cfg.actor_token_scopes().to_vec();
                }
            }
            if let Some(ep) = c.private_endpoint() {
                let (kind, rows) = private_endpoint_summary(ep);
                d.private_endpoint = kind;
                d.private_endpoint_detail = rows;
            }
            for o in c.private_endpoint_overrides() {
                let kind = o
                    .private_endpoint()
                    .map(|ep| private_endpoint_summary(ep).0)
                    .unwrap_or_default();
                d.private_endpoint_overrides
                    .push((o.domain().to_string(), kind));
            }
            (c.oauth_discovery(), c.client_id())
        }
        Some(Oauth2ProviderConfigOutput::GithubOauth2ProviderConfig(c)) => {
            (c.oauth_discovery(), c.client_id())
        }
        Some(Oauth2ProviderConfigOutput::GoogleOauth2ProviderConfig(c)) => {
            (c.oauth_discovery(), c.client_id())
        }
        Some(Oauth2ProviderConfigOutput::MicrosoftOauth2ProviderConfig(c)) => {
            (c.oauth_discovery(), c.client_id())
        }
        Some(Oauth2ProviderConfigOutput::SlackOauth2ProviderConfig(c)) => {
            (c.oauth_discovery(), c.client_id())
        }
        Some(Oauth2ProviderConfigOutput::SalesforceOauth2ProviderConfig(c)) => {
            (c.oauth_discovery(), c.client_id())
        }
        Some(Oauth2ProviderConfigOutput::AtlassianOauth2ProviderConfig(c)) => {
            (c.oauth_discovery(), c.client_id())
        }
        Some(Oauth2ProviderConfigOutput::LinkedinOauth2ProviderConfig(c)) => {
            (c.oauth_discovery(), c.client_id())
        }
        Some(Oauth2ProviderConfigOutput::IncludedOauth2ProviderConfig(c)) => {
            (c.oauth_discovery(), c.client_id())
        }
        _ => (None, None),
    };
    d.client_id = client_id.unwrap_or_default().to_string();
    match discovery {
        Some(Oauth2Discovery::DiscoveryUrl(u)) => d.discovery_url = u.clone(),
        Some(Oauth2Discovery::AuthorizationServerMetadata(m)) => {
            d.issuer = m.issuer().to_string();
            d.authorization_endpoint = m.authorization_endpoint().to_string();
            d.token_endpoint = m.token_endpoint().to_string();
            d.response_types = m.response_types().to_vec();
            d.token_endpoint_auth_methods = m.token_endpoint_auth_methods().to_vec();
        }
        _ => {}
    }

    Ok(d)
}

pub async fn fetch_api_key_provider_detail(
    ctl: CtlClient,
    name: String,
) -> std::result::Result<AgentCoreIdentityDetail, String> {
    let p = ctl
        .get_api_key_credential_provider()
        .name(&name)
        .send()
        .await
        .map_err(|e| crate::error::sdk_error_message(&e))?;
    Ok(AgentCoreIdentityDetail {
        created: fmt_dt(Some(p.created_time())),
        updated: fmt_dt(Some(p.last_updated_time())),
        secret_arn: p
            .api_key_secret_arn()
            .map(|s| s.secret_arn().to_string())
            .unwrap_or_default(),
        secret_json_key: p.api_key_secret_json_key().unwrap_or_default().to_string(),
        secret_source: p
            .api_key_secret_source()
            .map(|s| s.as_str().to_string())
            .unwrap_or_default(),
        ..Default::default()
    })
}

/// The account's token vault — the KMS configuration protecting every
/// credential-provider secret in the region. There is one per region and its
/// id is literally `default`; `GetTokenVault` takes no other handle.
pub const DEFAULT_TOKEN_VAULT_ID: &str = "default";

#[derive(Debug, Clone, Default)]
pub struct AgentCoreTokenVault {
    pub vault_id: String,
    pub key_type: String,
    pub kms_key_arn: String,
    pub last_modified: String,
}

pub async fn fetch_token_vault(
    ctl: CtlClient,
    vault_id: String,
) -> std::result::Result<AgentCoreTokenVault, String> {
    let v = ctl
        .get_token_vault()
        .token_vault_id(&vault_id)
        .send()
        .await
        .map_err(|e| crate::error::sdk_error_message(&e))?;
    let mut out = AgentCoreTokenVault {
        vault_id: v.token_vault_id().to_string(),
        last_modified: fmt_dt(Some(v.last_modified_date())),
        ..Default::default()
    };
    if let Some(k) = v.kms_configuration() {
        out.key_type = k.key_type().as_str().to_string();
        out.kms_key_arn = k.kms_key_arn().unwrap_or_default().to_string();
    }
    Ok(out)
}

// ═══════════════════════════════════════════════════════════════════════════════
// Resource policies
// ═══════════════════════════════════════════════════════════════════════════════
//
// The resource-based policy on a runtime or gateway — i.e. **who outside this
// account can invoke it**. `GetResourcePolicy` is documented as available "only
// for AgentCore Runtime and Gateway", so the section exists on exactly those
// two panes; there is no need to probe other types and render an error.
//
// The raw document is what `e` opens. The parsed statements are what the pane
// shows, because a resource policy's whole point is the principal list and
// scrolling raw JSON to find it is the thing this app exists to avoid.

/// One statement of a resource policy, flattened for display.
#[derive(Debug, Clone, Default)]
pub struct AgentCorePolicyStatement {
    pub sid: String,
    pub effect: String,
    /// Already-qualified principal strings: a bare ARN for `AWS`, otherwise
    /// prefixed with its kind (`Service: …`, `Federated: …`).
    pub principals: Vec<String>,
    pub actions: Vec<String>,
    /// `"<op> <key> = <value>"`, one per condition entry.
    pub conditions: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct AgentCoreResourcePolicy {
    /// Exactly what the API returned — the `e` payload. Empty when no policy
    /// is attached.
    pub raw: String,
    pub statements: Vec<AgentCorePolicyStatement>,
    /// True when `raw` is non-empty but didn't parse as the expected shape;
    /// the pane then falls back to showing the document.
    pub unparsed: bool,
}

/// `"a"` or `["a","b"]` → a Vec. IAM policy JSON uses both forms
/// interchangeably for Action, Resource and each condition value.
fn json_str_or_list(v: &serde_json::Value) -> Vec<String> {
    match v {
        serde_json::Value::String(s) => vec![s.clone()],
        serde_json::Value::Array(a) => a
            .iter()
            .filter_map(|x| x.as_str().map(|s| s.to_string()))
            .collect(),
        _ => Vec::new(),
    }
}

/// Flatten a `Principal` value. Three shapes occur: the bare string `"*"`, a
/// map of kind → one-or-many, and (rarely) a list.
fn json_principals(v: &serde_json::Value) -> Vec<String> {
    match v {
        serde_json::Value::String(s) => vec![s.clone()],
        serde_json::Value::Object(map) => {
            let mut out = Vec::new();
            for (kind, val) in map {
                for p in json_str_or_list(val) {
                    // `AWS` is the common case and its values are already
                    // ARNs or account ids; the others need their kind said.
                    if kind == "AWS" {
                        out.push(p);
                    } else {
                        out.push(format!("{}: {}", kind, p));
                    }
                }
            }
            out
        }
        serde_json::Value::Array(_) => json_str_or_list(v),
        _ => Vec::new(),
    }
}

fn parse_resource_policy(raw: &str) -> AgentCoreResourcePolicy {
    let mut out = AgentCoreResourcePolicy {
        raw: raw.to_string(),
        ..Default::default()
    };
    if raw.trim().is_empty() {
        return out;
    }
    let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) else {
        out.unparsed = true;
        return out;
    };
    // `Statement` is a single object as often as it is a list.
    let stmts: Vec<&serde_json::Value> = match v.get("Statement") {
        Some(serde_json::Value::Array(a)) => a.iter().collect(),
        Some(obj @ serde_json::Value::Object(_)) => vec![obj],
        _ => Vec::new(),
    };
    if stmts.is_empty() {
        out.unparsed = true;
        return out;
    }
    for s in stmts {
        let mut st = AgentCorePolicyStatement {
            sid: s.get("Sid").and_then(|x| x.as_str()).unwrap_or("").to_string(),
            effect: s
                .get("Effect")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string(),
            ..Default::default()
        };
        if let Some(p) = s.get("Principal") {
            st.principals = json_principals(p);
        }
        // A NotPrincipal statement grants to everyone *except* the listed
        // principals — the opposite reading, so label it rather than let it
        // render as if it were a Principal list.
        if let Some(p) = s.get("NotPrincipal") {
            for x in json_principals(p) {
                st.principals.push(format!("NOT {}", x));
            }
        }
        if let Some(a) = s.get("Action") {
            st.actions = json_str_or_list(a);
        }
        if let Some(a) = s.get("NotAction") {
            for x in json_str_or_list(a) {
                st.actions.push(format!("NOT {}", x));
            }
        }
        if let Some(serde_json::Value::Object(conds)) = s.get("Condition") {
            for (op, kv) in conds {
                if let serde_json::Value::Object(pairs) = kv {
                    for (key, val) in pairs {
                        st.conditions
                            .push(format!("{} {} = {}", op, key, json_str_or_list(val).join(", ")));
                    }
                }
            }
        }
        out.statements.push(st);
    }
    out
}

/// A principal that is not this account. `account` empty ⇒ unknown, so nothing
/// is flagged rather than everything.
pub fn principal_is_external(principal: &str, account: &str) -> bool {
    if principal == "*" {
        return true;
    }
    // Service principals are AWS itself, not a third party.
    if principal.starts_with("Service: ") {
        return false;
    }
    if account.is_empty() {
        return false;
    }
    !principal.contains(account)
}

pub async fn fetch_resource_policy(
    ctl: CtlClient,
    arn: String,
) -> std::result::Result<AgentCoreResourcePolicy, String> {
    match ctl.get_resource_policy().resource_arn(&arn).send().await {
        Ok(r) => Ok(parse_resource_policy(r.policy().unwrap_or_default())),
        Err(e) => {
            // "No policy attached" comes back as ResourceNotFound, which is a
            // normal state, not an error to show — the pane says so instead.
            // Matched on the wire code rather than `into_service_error()`,
            // which would consume the `SdkError` that `sdk_error_message`
            // needs for the real-failure path.
            use aws_sdk_bedrockagentcorecontrol::error::ProvideErrorMetadata;
            if e.code() == Some("ResourceNotFoundException") {
                Ok(AgentCoreResourcePolicy::default())
            } else {
                Err(crate::error::sdk_error_message(&e))
            }
        }
    }
}

#[cfg(test)]
mod resource_policy_tests {
    use super::*;

    #[test]
    fn parses_list_and_scalar_statement_forms() {
        let one = parse_resource_policy(
            r#"{"Version":"2012-10-17","Statement":{"Sid":"a","Effect":"Allow",
                "Principal":{"AWS":"arn:aws:iam::999:root"},
                "Action":"bedrock-agentcore:InvokeAgentRuntime"}}"#,
        );
        assert_eq!(one.statements.len(), 1);
        assert!(!one.unparsed);
        assert_eq!(one.statements[0].principals, vec!["arn:aws:iam::999:root"]);
        assert_eq!(
            one.statements[0].actions,
            vec!["bedrock-agentcore:InvokeAgentRuntime"]
        );

        let many = parse_resource_policy(
            r#"{"Statement":[{"Effect":"Allow","Principal":"*","Action":["a","b"]},
                {"Effect":"Deny","Principal":{"Service":"lambda.amazonaws.com"},"Action":"c",
                 "Condition":{"StringEquals":{"aws:PrincipalOrgID":"o-123"}}}]}"#,
        );
        assert_eq!(many.statements.len(), 2);
        assert_eq!(many.statements[0].actions.len(), 2);
        assert_eq!(
            many.statements[1].principals,
            vec!["Service: lambda.amazonaws.com"]
        );
        assert_eq!(
            many.statements[1].conditions,
            vec!["StringEquals aws:PrincipalOrgID = o-123"]
        );
    }

    #[test]
    fn empty_is_no_policy_not_a_parse_failure() {
        let p = parse_resource_policy("");
        assert!(p.statements.is_empty() && !p.unparsed && p.raw.is_empty());
    }

    #[test]
    fn garbage_and_shapeless_json_fall_back_to_the_raw_document() {
        assert!(parse_resource_policy("not json").unparsed);
        assert!(parse_resource_policy(r#"{"Version":"2012-10-17"}"#).unparsed);
    }

    #[test]
    fn external_principals() {
        assert!(principal_is_external("*", "111122223333"));
        assert!(principal_is_external("arn:aws:iam::999999999999:root", "111122223333"));
        assert!(!principal_is_external(
            "arn:aws:iam::111122223333:role/app",
            "111122223333"
        ));
        // Service principals are AWS, not a third party.
        assert!(!principal_is_external("Service: lambda.amazonaws.com", "111122223333"));
        // Unknown account ⇒ flag nothing rather than everything.
        assert!(!principal_is_external("arn:aws:iam::999999999999:root", ""));
    }

    #[test]
    fn not_principal_is_labelled_as_the_inverse() {
        let p = parse_resource_policy(
            r#"{"Statement":[{"Effect":"Deny","NotPrincipal":{"AWS":"arn:aws:iam::111:root"},
                "Action":"*"}]}"#,
        );
        assert_eq!(p.statements[0].principals, vec!["NOT arn:aws:iam::111:root"]);
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Policy generations
// ═══════════════════════════════════════════════════════════════════════════════
//
// AgentCore's policy *generator*: you describe what an agent should be allowed
// to do, and it emits Cedar policy **assets** — one per fragment of the source
// description. A `Policy` whose definition is
// `PolicyDefinition::PolicyGeneration` points back at one of these by
// (generation id, asset id), which until now had nowhere to lead.
//
// The load-bearing part is `Finding`. A generated asset carries findings whose
// type says how the translation went, and four of the seven mean the output
// does not do what the author asked:
//   • `AllowAll`        — the generated allow permits everything
//   • `DenyNone`        — the generated deny blocks nothing
//   • `NotTranslatable` — the fragment produced no policy at all, silently
//   • `Invalid`         — it produced something that isn't valid policy
// `AllowNone` and `DenyAll` are deliberately **not** flagged. They are also a
// mismatch with intent, but they fail *closed* — an over-restrictive policy
// announces itself the first time something is denied, whereas the four above
// leave you believing a control exists when it does not. Flagging all six
// would put a ⚠ on most generations and train people to ignore it.

/// Generations are deepened one `ListPolicyGenerationAssets` at a time; cap
/// the fan-out the way gateway targets are capped. Newest first, because a
/// generation history is append-only and the recent end is the live one.
pub const MAX_POLICY_GENERATIONS: usize = 15;

#[derive(Debug, Clone, Default)]
pub struct AgentCorePolicyGenerationAsset {
    pub asset_id: String,
    /// The Cedar (or statement) text this fragment produced. Empty when the
    /// fragment wasn't translatable.
    pub definition: String,
    /// The source fragment the asset was generated from — what the author
    /// actually wrote.
    pub source_text: String,
    /// `(type, description)` per finding.
    pub findings: Vec<(String, String)>,
}

impl AgentCorePolicyGenerationAsset {
    /// True when any finding says the generated policy fails *open* relative
    /// to the fragment that produced it — see the section note above for why
    /// fail-closed findings are excluded.
    pub fn has_problem_finding(&self) -> bool {
        self.findings
            .iter()
            .any(|(t, _)| Self::finding_is_problem(t))
    }

    pub fn finding_is_problem(finding_type: &str) -> bool {
        matches!(
            finding_type.to_ascii_uppercase().as_str(),
            "ALLOW_ALL" | "ALLOWALL" | "DENY_NONE" | "DENYNONE" | "INVALID" | "NOT_TRANSLATABLE"
                | "NOTTRANSLATABLE"
        )
    }
}

#[derive(Debug, Clone, Default)]
pub struct AgentCorePolicyGeneration {
    pub generation_id: String,
    pub arn: String,
    pub name: String,
    pub status: String,
    /// The `Resource` union's only arm — the ARN this generation targets, so
    /// the row is jumpable through the generic classifier.
    pub target_arn: String,
    pub findings: String,
    pub status_reasons: Vec<String>,
    pub created: String,
    pub updated: String,
    pub assets: Vec<AgentCorePolicyGenerationAsset>,
    /// False when the asset listing wasn't attempted (past the cap) or failed
    /// — distinct from "attempted and there were none", which the pane must
    /// not report as an empty generation.
    pub assets_loaded: bool,
}

pub async fn fetch_policy_generations(
    ctl: CtlClient,
    engine_id: String,
) -> std::result::Result<Vec<AgentCorePolicyGeneration>, String> {
    use aws_sdk_bedrockagentcorecontrol::types::{PolicyDefinition, Resource as AcResource};

    let mut out: Vec<AgentCorePolicyGeneration> = Vec::new();
    let mut pager = ctl
        .list_policy_generations()
        .policy_engine_id(&engine_id)
        .into_paginator()
        .send();
    while let Some(page) = pager.next().await {
        let page = page.map_err(|e| crate::error::sdk_error_message(&e))?;
        for g in page.policy_generations() {
            out.push(AgentCorePolicyGeneration {
                generation_id: g.policy_generation_id().to_string(),
                arn: g.policy_generation_arn().to_string(),
                name: g.name().to_string(),
                status: g.status().as_str().to_string(),
                target_arn: match g.resource() {
                    Some(AcResource::Arn(a)) => a.clone(),
                    _ => String::new(),
                },
                findings: g.findings().unwrap_or_default().to_string(),
                status_reasons: g.status_reasons().to_vec(),
                created: fmt_dt(Some(g.created_at())),
                updated: fmt_dt(Some(g.updated_at())),
                ..Default::default()
            });
        }
    }

    // Newest first. `fmt_dt` emits `YYYY-MM-DD HH:MM:SS`, which sorts
    // lexically in chronological order, so a plain reverse compare is right.
    out.sort_by(|a, b| b.created.cmp(&a.created));

    // Second pass for the assets — one call per generation, so capped.
    for gen in out.iter_mut().take(MAX_POLICY_GENERATIONS) {
        let mut assets = Vec::new();
        let mut pager = ctl
            .list_policy_generation_assets()
            .policy_engine_id(&engine_id)
            .policy_generation_id(&gen.generation_id)
            .into_paginator()
            .send();
        let mut ok = true;
        while let Some(page) = pager.next().await {
            // One generation that won't list its assets is skipped, not fatal
            // — the rest of the section is still worth drawing.
            let Ok(page) = page else {
                ok = false;
                break;
            };
            for a in page.policy_generation_assets() {
                let definition = match a.definition() {
                    Some(PolicyDefinition::Cedar(c)) => c.statement().to_string(),
                    Some(PolicyDefinition::Policy(p)) => p.statement().to_string(),
                    // A generation asset pointing at another generation would
                    // be circular; nothing to show.
                    _ => String::new(),
                };
                assets.push(AgentCorePolicyGenerationAsset {
                    asset_id: a.policy_generation_asset_id().to_string(),
                    definition,
                    source_text: a.raw_text_fragment().to_string(),
                    findings: a
                        .findings()
                        .iter()
                        .map(|f| {
                            (
                                f.r#type()
                                    .map(|t| t.as_str().to_string())
                                    .unwrap_or_default(),
                                f.description().unwrap_or_default().to_string(),
                            )
                        })
                        .collect(),
                });
            }
        }
        if ok {
            gen.assets = assets;
            gen.assets_loaded = true;
        }
    }

    Ok(out)
}

#[cfg(test)]
mod policy_generation_tests {
    use super::AgentCorePolicyGenerationAsset as A;

    #[test]
    fn problem_findings_are_the_four_that_fail_open() {
        // The generator "succeeded" in all of these — that is exactly why they
        // need flagging rather than being trusted as a green status.
        for t in [
            "ALLOW_ALL",
            "AllowAll",
            "DENY_NONE",
            "INVALID",
            "NOT_TRANSLATABLE",
            "NotTranslatable",
        ] {
            assert!(A::finding_is_problem(t), "{t} should be flagged");
        }
        // ALLOW_NONE / DENY_ALL are a mismatch too, but they fail closed —
        // flagging them would put a warning on most generations.
        for t in ["VALID", "Valid", "ALLOW_NONE", "DENY_ALL", ""] {
            assert!(!A::finding_is_problem(t), "{t} should not be flagged");
        }
    }

    #[test]
    fn an_asset_is_problematic_if_any_finding_is() {
        let a = A {
            findings: vec![
                ("VALID".into(), String::new()),
                ("ALLOW_ALL".into(), "permits everything".into()),
            ],
            ..Default::default()
        };
        assert!(a.has_problem_finding());
        let clean = A {
            findings: vec![("VALID".into(), String::new())],
            ..Default::default()
        };
        assert!(!clean.has_problem_finding());
        // No findings at all is not a problem — it is just no signal.
        assert!(!A::default().has_problem_finding());
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Tags
// ═══════════════════════════════════════════════════════════════════════════════
//
// `ListTagsForResource` takes any AgentCore ARN, so one fetcher and one
// `LazyMap` serve every pane. It is **lazy, per-pane** — the same shape the
// other fourteen tag maps in `LazyStore` use — and that is a deliberate
// ceiling, not an oversight:
//
// `tag:` search matches `Resource::tags()`, a *synchronous* field populated
// when the list loads. No AgentCore `List*` returns tags (checked: only
// `GetDataset`, `GetPaymentManager` and `GetPaymentCredentialProvider` do),
// so filling it would mean one `ListTagsForResource` per resource across
// every family on every load — turning a ~15-call list into N. That trade is
// what the other fourteen services already declined. So AgentCore rows carry
// a Tags *section*, and `tag:` does not filter them.

pub async fn fetch_agentcore_tags(
    ctl: CtlClient,
    arn: String,
) -> std::result::Result<Vec<(String, String)>, String> {
    let r = ctl
        .list_tags_for_resource()
        .resource_arn(&arn)
        .send()
        .await
        .map_err(|e| crate::error::sdk_error_message(&e))?;
    let mut out: Vec<(String, String)> = r
        .tags()
        .map(|t| t.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default();
    // The API returns a map; sort so the pane is stable across refetches.
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}
