//! The deep module behind the "lazy section" pattern (vocabulary in
//! CONTEXT.md, the event design in docs/adr/0001-apply-closure-lazy-event.md).
//!
//! A lazy section's data lives in a [`LazyMap`] inside the app's single
//! [`LazyStore`]. Fetches are spawned through `App::trigger_lazy` and resolve
//! to a [`Lazy`] outcome delivered by the one `Event::Lazy` apply-closure
//! event. The store's epoch stamp lets a stale in-flight fetch — one spawned
//! before an account or region switch — be dropped instead of poisoning the
//! fresh store, and replacing the store wholesale on a switch makes the old
//! "clear it in `reset_account_scoped_state`" convention unnecessary: a new
//! map field resets by construction.

use crate::aws::services::ssm::{
    SsmAssocDetail, SsmAutomationStep, SsmBaselineDetail, SsmCmdInvocation, SsmDocContent,
    SsmInstanceAssoc, SsmInstancePatches, SsmInventoryData, SsmMaintWindowDetail,
    SsmOpsItemDetail, SsmParamDetail,
};
use std::collections::HashMap;

/// The three-state outcome of a lazy fetch. "Nothing found" is a normal
/// `Loaded` payload (e.g. `Loaded(None)`), never an `Error`.
#[derive(Debug, Clone)]
pub enum Lazy<T> {
    Loading,
    Loaded(T),
    Error(String),
}

/// A keyed collection of [`Lazy`] outcomes for one detail concern (e.g. flow
/// logs by VPC id). Triggering a key that is already present is a no-op, so a
/// section can be triggered from digit keys, `Tab`-cycling, and drill-in
/// alike without duplicate fetches.
#[derive(Debug, Clone)]
pub struct LazyMap<T> {
    map: HashMap<String, Lazy<T>>,
}

impl<T> Default for LazyMap<T> {
    fn default() -> Self {
        Self {
            map: HashMap::new(),
        }
    }
}

impl<T> LazyMap<T> {
    pub fn get(&self, key: &str) -> Option<&Lazy<T>> {
        self.map.get(key)
    }

    /// The trigger idempotence guard: an entry in any state (including
    /// `Error` — failures don't auto-retry) blocks a re-fetch.
    pub fn contains(&self, key: &str) -> bool {
        self.map.contains_key(key)
    }

    /// Mark a fetch as in flight. `App::trigger_lazy` calls this before
    /// spawning; renderers show `Loading` until the apply lands.
    pub fn insert_loading(&mut self, key: String) {
        self.map.insert(key, Lazy::Loading);
    }

    /// Deliver a fetch result (the apply-closure's body).
    pub fn apply(&mut self, key: String, result: Result<T, String>) {
        let state = match result {
            Ok(v) => Lazy::Loaded(v),
            Err(e) => Lazy::Error(e),
        };
        self.map.insert(key, state);
    }

    /// Forget one key so the next trigger refetches it (manual refresh).
    #[allow(dead_code)]
    pub fn invalidate(&mut self, key: &str) {
        self.map.remove(key);
    }

    /// Every entry, in no particular order — for consumers that aggregate
    /// across keys (the Route 53 Records tab flattens every loaded zone).
    pub fn iter(&self) -> impl Iterator<Item = (&String, &Lazy<T>)> {
        self.map.iter()
    }

    /// How many keys are still in flight.
    pub fn loading_count(&self) -> usize {
        self.map.values().filter(|v| matches!(v, Lazy::Loading)).count()
    }
}

/// The single owner of every [`LazyMap`]. Any profile, org-role, or region
/// switch replaces the whole store with `LazyStore::new(epoch + 1)` — every
/// map clears and every in-flight fetch is invalidated in one move.
///
/// Adding a lazy section = add a field here + a `trigger_*` fn calling
/// `App::trigger_lazy`. No `Event` variant, no `*State` enum, no reset
/// wiring.
#[derive(Default)]
pub struct LazyStore {
    epoch: u64,

    // ── VPC ──────────────────────────────────────────────────────────────
    /// Managed prefix-list entries (cidr, description), keyed by `pl-` id.
    pub pl_entries: LazyMap<Vec<(String, String)>>,
    /// VPC-level flow logs, keyed by VPC id.
    pub vpc_flow_logs: LazyMap<Vec<crate::aws::services::vpc::FlowLogInfo>>,
    /// (enableDnsSupport, enableDnsHostnames), keyed by VPC id.
    pub vpc_dns_attrs: LazyMap<(bool, bool)>,

    // ── SSM ──────────────────────────────────────────────────────────────
    /// Parameter history + tags (one fetch, two sections), keyed by name.
    pub ssm_param_details: LazyMap<SsmParamDetail>,
    /// Document (content, format), keyed by document name.
    pub ssm_doc_contents: LazyMap<SsmDocContent>,
    /// Association executions + tags, keyed by association id.
    pub ssm_assoc_details: LazyMap<SsmAssocDetail>,
    /// Per-association status on a managed instance, keyed by instance id.
    pub ssm_instance_assocs: LazyMap<Vec<SsmInstanceAssoc>>,
    /// Software inventory of a managed instance, keyed by instance id.
    pub ssm_instance_inventory: LazyMap<Box<SsmInventoryData>>,
    /// Patch rollup + actionable patches, keyed by instance id.
    pub ssm_instance_patches: LazyMap<SsmInstancePatches>,
    /// Per-instance invocations of a Run Command, keyed by command id.
    pub ssm_cmd_invocations: LazyMap<Vec<SsmCmdInvocation>>,
    /// An automation execution's steps, keyed by execution id.
    pub ssm_automation_steps: LazyMap<Vec<SsmAutomationStep>>,
    /// Maintenance-window targets + tasks + history, keyed by window id.
    pub ssm_mw_details: LazyMap<SsmMaintWindowDetail>,
    /// A patch baseline's full ruleset, keyed by baseline id/ARN.
    pub ssm_baseline_details: LazyMap<Box<SsmBaselineDetail>>,
    /// An OpsItem's description + operational data, keyed by OpsItem id.
    pub ssm_ops_item_details: LazyMap<Box<SsmOpsItemDetail>>,

    // ── CloudFront ───────────────────────────────────────────────────────
    /// A CloudFront function's source code, keyed by function name.
    pub cf_function_code: LazyMap<String>,
    /// A distribution's invalidation history, keyed by distribution id.
    pub cf_invalidations: LazyMap<crate::aws::services::cloudfront::CfInvalidationsData>,

    // ── Direct Connect ───────────────────────────────────────────────────
    /// A DX gateway's VGW/TGW associations, keyed by gateway id.
    pub dx_gateway_associations:
        LazyMap<Vec<crate::aws::services::direct_connect::DxGwAssociation>>,
    /// A DX gateway's VIF attachments, keyed by gateway id.
    pub dx_gateway_attachments:
        LazyMap<Vec<crate::aws::services::direct_connect::DxGwAttachment>>,

    // ── API Gateway ──────────────────────────────────────────────────────
    /// A usage plan's API keys (id/name only, never values), keyed by plan id.
    pub api_usage_plan_keys: LazyMap<Vec<crate::aws::services::api_gateway::UsagePlanKeyInfo>>,

    // ── WAF ──────────────────────────────────────────────────────────────
    /// A Web ACL's aggregated sampled-request insights, keyed by ACL name.
    pub waf_insights: LazyMap<Box<crate::aws::services::waf::WafInsights>>,
    /// A Web ACL's rules / associated resources / logging, keyed by ACL id.
    pub waf_web_acl_details: LazyMap<Box<crate::aws::services::waf::WafWebAclDetail>>,
    /// An IP set's addresses + tags, keyed by IP set id.
    pub waf_ip_set_details: LazyMap<Box<crate::aws::services::waf::WafIpSetDetail>>,
    /// A rule group's rules + tags, keyed by rule-group id.
    pub waf_rule_group_details: LazyMap<Box<crate::aws::services::waf::WafRuleGroupDetail>>,

    // ── API Gateway (details) ────────────────────────────────────────────
    /// A REST API's resources/stages/authorizers/deployments, keyed by API id.
    pub rest_api_details: LazyMap<crate::aws::services::api_gateway::RestApiDetails>,
    /// An HTTP/WS API's routes/stages/integrations, keyed by API id.
    pub http_api_details: LazyMap<crate::aws::services::api_gateway::HttpApiDetails>,
    /// A custom domain's API mappings, keyed by domain name.
    pub api_domain_details: LazyMap<Vec<crate::aws::services::api_gateway::DomainMapping>>,

    // ── Lambda / RDS ─────────────────────────────────────────────────────
    /// A function's GetFunction code metadata, keyed by function ARN.
    pub lambda_code: LazyMap<Box<crate::aws::services::lambda::LambdaCodeInfo>>,
    /// A function's event source mappings, keyed by function ARN.
    pub lambda_esms: LazyMap<Vec<crate::aws::services::lambda::LambdaEsmInfo>>,
    /// EventBridge rules that target a function (the push-trigger side of
    /// Triggers), keyed by function ARN.
    pub lambda_eb_rules: LazyMap<Vec<crate::aws::services::eventbridge::EbRule>>,
    /// Reserved + provisioned concurrency and async-invoke destinations
    /// (Config section), keyed by function ARN.
    pub lambda_concurrency: LazyMap<crate::aws::services::lambda::LambdaConcurrency>,
    /// An instance's Performance Insights wait-event profile, keyed by DB id.
    pub rds_pi: LazyMap<crate::aws::services::rds::RdsPiData>,
    /// Pending maintenance actions (instance or cluster), keyed by ARN.
    pub rds_pending_maintenance: LazyMap<Vec<crate::aws::services::rds::RdsPendingAction>>,
    /// Last-14-days events (instance or cluster), keyed by `kind:id` so an
    /// instance and cluster sharing a name can't collide.
    pub rds_events: LazyMap<Vec<crate::aws::services::rds::RdsEvent>>,
    /// An instance's native DB log files `(files, total)`, keyed by DB id.
    pub rds_log_files: LazyMap<(Vec<crate::aws::services::rds::RdsLogFile>, usize)>,
    /// A parameter group's parameters `(params, total)`, keyed by
    /// `RdsParamGroup::params_key`.
    pub rds_parameters: LazyMap<(Vec<crate::aws::services::rds::RdsParameter>, usize)>,

    // ── Identity Center ──────────────────────────────────────────────────
    /// Permission-set policies/boundary/tags, keyed by PS ARN.
    pub ic_ps_access: LazyMap<Box<crate::aws::services::identity_center::PsAccess>>,
    /// Permission-set account assignments, keyed by PS ARN.
    pub ic_ps_assignments: LazyMap<Vec<crate::aws::services::identity_center::PsAssignment>>,
    /// Group members, keyed by group id.
    pub ic_group_members: LazyMap<Vec<crate::aws::services::identity_center::IcMember>>,
    /// A user's group memberships, keyed by user id.
    pub ic_user_groups: LazyMap<Vec<crate::aws::services::identity_center::IcGroupRef>>,
    /// Principal effective access (direct + via groups), keyed by user/group id.
    pub ic_principal_access: LazyMap<Vec<crate::aws::services::identity_center::IcAccessEntry>>,
    /// Instance ABAC configuration (None = not configured), keyed by instance ARN.
    pub ic_abac: LazyMap<Option<crate::aws::services::identity_center::IcAbac>>,
    /// Instance trusted token issuers, keyed by instance ARN.
    pub ic_tti: LazyMap<Vec<crate::aws::services::identity_center::IcTti>>,
    /// Application assignments, keyed by application ARN.
    pub ic_app_assignments:
        LazyMap<Box<crate::aws::services::identity_center::IcAppAssignments>>,

    // ── Compute Optimizer ────────────────────────────────────────────────
    /// Per-resource recommendation, keyed by ARN. `Loaded(None)` = the call
    /// succeeded but Optimizer has no recommendation (normal for new /
    /// unsupported resources).
    pub optimizer_recs: LazyMap<Option<crate::aws::services::computeoptimizer::OptimizerRec>>,

    // ── Firewall Manager ─────────────────────────────────────────────────
    /// An FMS policy's tags, keyed by policy id.
    pub fms_policy_tags: LazyMap<Vec<(String, String)>>,
    /// Per-account violator drill, keyed by `policy_id/account`.
    pub fms_compliance_details: LazyMap<crate::aws::services::fms::FmsComplianceDetail>,
    /// An FMS resource set's config + members, keyed by set id.
    pub fms_resource_set_details: LazyMap<crate::aws::services::fms::FmsResourceSetDetail>,

    // ── KMS ──────────────────────────────────────────────────────────────
    /// Key-rotation status (`None` = n/a: asymmetric or AWS-managed), keyed
    /// by key id.
    pub kms_rotation: LazyMap<Option<bool>>,
    /// The key policy document, keyed by key id.
    pub kms_policy: LazyMap<String>,
    /// Grants on a key, keyed by key id.
    pub kms_grants: LazyMap<Vec<crate::aws::services::kms::KmsGrant>>,
    /// Key tags, keyed by key id.
    pub kms_tags: LazyMap<HashMap<String, String>>,

    // ── Bedrock AgentCore ────────────────────────────────────────────────
    // (`@agentcore` — the agent hosting platform, not `@bedrock`.)
    /// `GetAgentRuntime` — feeds the Overview/Artifact/Network/Auth/Env
    /// sections from one fetch. Keyed by runtime id.
    pub agentcore_runtime_detail:
        LazyMap<Box<crate::aws::services::agentcore::AgentCoreRuntimeDetail>>,
    /// A runtime's endpoints, keyed by runtime id.
    pub agentcore_runtime_endpoints:
        LazyMap<Vec<crate::aws::services::agentcore::AgentCoreEndpoint>>,
    /// A runtime's A2A agent card as pretty JSON, keyed by runtime id.
    /// Populated only by the explicit `x` reveal — never by a section hook.
    pub agentcore_agent_card: LazyMap<String>,
    /// A runtime's versions (newest first), keyed by runtime id.
    pub agentcore_runtime_versions:
        LazyMap<Vec<crate::aws::services::agentcore::AgentCoreRuntimeVersion>>,
    /// `GetGateway` — Overview/Auth/Security/Interceptors, keyed by gateway id.
    pub agentcore_gateway_detail:
        LazyMap<Box<crate::aws::services::agentcore::AgentCoreGatewayDetail>>,
    /// A gateway's targets, deepened per target, keyed by gateway id.
    pub agentcore_gateway_targets: LazyMap<Vec<crate::aws::services::agentcore::AgentCoreTarget>>,
    /// A gateway's routing rules, keyed by gateway id.
    pub agentcore_gateway_rules: LazyMap<Vec<crate::aws::services::agentcore::AgentCoreGatewayRule>>,
    /// `GetMemory` — Overview/Strategies/Indexing, keyed by memory id.
    pub agentcore_memory_detail:
        LazyMap<Box<crate::aws::services::agentcore::AgentCoreMemoryDetail>>,
    /// A memory store's actors (data plane), keyed by memory id.
    pub agentcore_memory_actors: LazyMap<Vec<String>>,
    /// A memory store's sessions, aggregated across actors, keyed by memory id.
    pub agentcore_memory_sessions:
        LazyMap<Vec<crate::aws::services::agentcore::AgentCoreMemorySession>>,
    /// `GetBrowser` / `GetCodeInterpreter` — one shared bundle, keyed by the
    /// tool's id (the two id spaces don't collide).
    pub agentcore_tool_detail: LazyMap<Box<crate::aws::services::agentcore::AgentCoreToolDetail>>,
    /// A registry's catalogued records, keyed by registry id.
    pub agentcore_registry_records:
        LazyMap<Vec<crate::aws::services::agentcore::AgentCoreRegistryRecord>>,
    /// A tool's sessions (data plane), keyed by the tool's id.
    pub agentcore_tool_sessions: LazyMap<Vec<crate::aws::services::agentcore::AgentCoreToolSession>>,
    /// `GetHarness` — the whole agent definition (model / prompt / tools /
    /// skills / memory / environment) from one fetch, keyed by harness id.
    pub agentcore_harness_detail:
        LazyMap<Box<crate::aws::services::agentcore::AgentCoreHarnessDetail>>,
    /// A harness's endpoints, keyed by harness id.
    pub agentcore_harness_endpoints:
        LazyMap<Vec<crate::aws::services::agentcore::AgentCoreHarnessEndpoint>>,
    /// A harness's versions (newest first), keyed by harness id.
    pub agentcore_harness_versions:
        LazyMap<Vec<crate::aws::services::agentcore::AgentCoreHarnessVersion>>,
    /// `GetConfigurationBundle` — live version + component blobs, keyed by
    /// bundle id.
    pub agentcore_bundle_detail:
        LazyMap<Box<crate::aws::services::agentcore::AgentCoreConfigBundleDetail>>,
    /// A bundle's version lineage, keyed by bundle id.
    pub agentcore_bundle_versions:
        LazyMap<Vec<crate::aws::services::agentcore::AgentCoreConfigBundleVersion>>,
    /// `GetPaymentManager` / `GetPaymentCredentialProvider` — one shared
    /// bundle (the tool-detail pattern), keyed by manager id or provider ARN.
    pub agentcore_payment_detail:
        LazyMap<Box<crate::aws::services::agentcore::AgentCorePaymentDetail>>,
    /// A payment manager's connectors, keyed by manager id.
    pub agentcore_payment_connectors:
        LazyMap<Vec<crate::aws::services::agentcore::AgentCorePaymentConnector>>,
    /// `GetWorkloadIdentity` / `GetOauth2CredentialProvider` /
    /// `GetApiKeyCredentialProvider` — one shared bundle for the three
    /// Identity families, keyed by **ARN** (the three `Get*` calls key off
    /// name, which isn't guaranteed unique across the families).
    pub agentcore_identity_detail:
        LazyMap<Box<crate::aws::services::agentcore::AgentCoreIdentityDetail>>,
    /// The region's token vault (KMS config protecting every credential
    /// provider's secret), keyed by vault id — always `default`.
    pub agentcore_token_vault: LazyMap<crate::aws::services::agentcore::AgentCoreTokenVault>,
    /// `ListTagsForResource` for any AgentCore resource, keyed by ARN. Lazy
    /// and per-pane by design — see the Tags note in `agentcore.rs` for why
    /// `tag:` search deliberately does not cover this service.
    pub agentcore_tags: LazyMap<Vec<(String, String)>>,
    /// A policy engine's generations, each deepened with its generated Cedar
    /// assets, keyed by engine id.
    pub agentcore_policy_generations:
        LazyMap<Vec<crate::aws::services::agentcore::AgentCorePolicyGeneration>>,
    /// `GetResourcePolicy` — the resource-based policy on a runtime or gateway
    /// (the only two types the API supports), keyed by resource id. An empty
    /// `raw` means no policy is attached, which is a normal state.
    pub agentcore_resource_policy:
        LazyMap<crate::aws::services::agentcore::AgentCoreResourcePolicy>,

    // ── Bedrock ──────────────────────────────────────────────────────────
    /// A guardrail's full policy detail, keyed by guardrail id.
    pub guardrail_detail: LazyMap<Box<crate::aws::services::bedrock::GuardrailFull>>,
    /// A knowledge base's full config (Overview / Configuration / Vector
    /// Store sections), keyed by KB id.
    pub kb_detail: LazyMap<Box<crate::aws::services::bedrock::KnowledgeBaseFull>>,
    /// A KB's data sources, keyed by KB id.
    pub kb_data_sources: LazyMap<Vec<crate::aws::services::bedrock::KbDataSource>>,
    /// A KB's recent ingestion jobs (aggregated across sources), keyed by
    /// KB id.
    pub kb_ingestion: LazyMap<Vec<crate::aws::services::bedrock::KbIngestionJob>>,
    /// An agent's full config (Overview section), keyed by agent id.
    pub agent_detail: LazyMap<Box<crate::aws::services::bedrock::AgentFull>>,
    /// An agent's action groups, keyed by agent id.
    pub agent_action_groups: LazyMap<Vec<crate::aws::services::bedrock::AgentActionGroup>>,
    /// An agent's aliases, keyed by agent id.
    pub agent_aliases: LazyMap<Vec<crate::aws::services::bedrock::AgentAlias>>,
    /// An agent's associated knowledge bases, keyed by agent id.
    pub agent_knowledge_bases: LazyMap<Vec<crate::aws::services::bedrock::AgentKb>>,

    // ── RAM ──────────────────────────────────────────────────────────────
    /// A resource share's shared resources, keyed by share ARN.
    pub ram_resources: LazyMap<Vec<crate::aws::services::ram::RamSharedResource>>,
    /// A resource share's principals, keyed by share ARN.
    pub ram_principals: LazyMap<Vec<crate::aws::services::ram::RamPrincipal>>,

    // ── WorkSpaces ───────────────────────────────────────────────────────
    /// A WorkSpace's connection status, keyed by workspace id.
    pub workspace_connections: LazyMap<crate::aws::services::workspaces::WorkspaceConnection>,
    /// A WorkSpace's tags, keyed by workspace id.
    pub workspace_tags: LazyMap<HashMap<String, String>>,

    // ── Resource Groups ──────────────────────────────────────────────────
    /// A group's tag/CFN query, keyed by group name.
    pub resource_group_query: LazyMap<crate::aws::services::resource_groups::ResourceGroupQuery>,
    /// A group's member resources, keyed by group name.
    pub resource_group_resources:
        LazyMap<Vec<crate::aws::services::resource_groups::ResourceGroupMember>>,
    /// A group's tags, keyed by group name.
    pub resource_group_tags: LazyMap<HashMap<String, String>>,

    // ── Trusted Advisor ──────────────────────────────────────────────────
    /// A check's flagged resources, keyed by check id.
    pub ta_results: LazyMap<Vec<crate::aws::services::trusted_advisor::TaFlaggedResource>>,
    /// A Priority recommendation's full detail (description etc.), keyed by ARN.
    pub ta_rec_detail: LazyMap<crate::aws::services::trusted_advisor::TaRecDetail>,
    /// A Priority recommendation's affected accounts, keyed by ARN.
    pub ta_rec_accounts: LazyMap<Vec<crate::aws::services::trusted_advisor::TaRecAccount>>,
    /// A Priority recommendation's affected resources (capped) + truncation
    /// flag, keyed by ARN.
    pub ta_rec_resources:
        LazyMap<(Vec<crate::aws::services::trusted_advisor::TaRecResource>, bool)>,

    // ── Global Accelerator ───────────────────────────────────────────────
    /// An accelerator's listeners, keyed by accelerator ARN.
    pub ga_listeners: LazyMap<Vec<crate::aws::services::global_accelerator::GaListener>>,
    /// An accelerator's endpoint groups, keyed by accelerator ARN.
    pub ga_endpoint_groups:
        LazyMap<Vec<crate::aws::services::global_accelerator::GaEndpointGroup>>,
    /// An accelerator's tags, keyed by accelerator ARN.
    pub ga_tags: LazyMap<HashMap<String, String>>,

    // ── EKS ──────────────────────────────────────────────────────────────
    /// A cluster's node groups, keyed by cluster name.
    pub eks_nodegroups: LazyMap<Vec<crate::aws::services::eks::EksNodeGroup>>,
    /// A cluster's Fargate profiles, keyed by cluster name.
    pub eks_fargate: LazyMap<Vec<crate::aws::services::eks::EksFargateProfile>>,
    /// A cluster's add-ons (with update-available checks), keyed by cluster
    /// name.
    pub eks_addons: LazyMap<Vec<crate::aws::services::eks::EksAddon>>,
    /// A cluster's access entries + auth mode, keyed by cluster name.
    pub eks_access: LazyMap<crate::aws::services::eks::EksAccessData>,
    /// A cluster's upgrade-readiness insights, keyed by cluster name.
    pub eks_insights: LazyMap<Vec<crate::aws::services::eks::EksInsight>>,

    // ── EFS ──────────────────────────────────────────────────────────────
    /// A file system's mount targets, keyed by file-system id.
    pub efs_mount_targets: LazyMap<Vec<crate::aws::services::efs::EfsMountTarget>>,
    /// A file system's access points, keyed by file-system id.
    pub efs_access_points: LazyMap<Vec<crate::aws::services::efs::EfsAccessPoint>>,

    // ── ElastiCache / OpenSearch ─────────────────────────────────────────
    /// A cache cluster's tags, keyed by ARN.
    pub elasticache_tags: LazyMap<Vec<(String, String)>>,
    /// A domain's tags, keyed by ARN.
    pub opensearch_tags: LazyMap<Vec<(String, String)>>,

    // ── MSK ──────────────────────────────────────────────────────────────
    /// A cluster's applied Kafka configuration revision, keyed by cluster
    /// ARN. Clusters with no custom configuration never fetch.
    pub msk_config: LazyMap<crate::aws::services::msk::MskConfigRevision>,

    // ── Kinesis / Firehose ───────────────────────────────────────────────
    /// A data stream's enhanced fan-out consumers, keyed by stream ARN.
    pub kinesis_consumers: LazyMap<Vec<crate::aws::services::kinesis::KinesisConsumer>>,
    /// A data stream's tags, keyed by stream name.
    pub kinesis_tags: LazyMap<Vec<(String, String)>>,
    /// A delivery stream's tags, keyed by stream name.
    pub firehose_tags: LazyMap<Vec<(String, String)>>,

    // ── Transfer Family ──────────────────────────────────────────────────
    /// A server's users, keyed by server id.
    pub transfer_users: LazyMap<Vec<crate::aws::services::transfer::TransferUser>>,

    // ── CloudFormation ───────────────────────────────────────────────────
    /// Stacks importing an export (empty = not imported by any stack),
    /// keyed by export name.
    pub cfn_imports: LazyMap<Vec<String>>,
    /// A stack's resources, keyed by stack name.
    pub cfn_stack_resources:
        LazyMap<Vec<crate::aws::services::cloudformation::CfnStackResourceEntry>>,
    /// A stack's event log, keyed by stack name.
    pub cfn_stack_events: LazyMap<Vec<crate::aws::services::cloudformation::CfnStackEvent>>,
    /// A stack's template body, keyed by stack name.
    pub cfn_stack_template: LazyMap<String>,
    /// A stack's per-resource drift results, keyed by stack name.
    pub cfn_stack_drift: LazyMap<Vec<crate::aws::services::cloudformation::CfnResourceDrift>>,
    /// A stack's change sets, keyed by stack name.
    pub cfn_stack_changesets: LazyMap<Vec<crate::aws::services::cloudformation::CfnChangeSet>>,
    /// A stack set's config detail (Config + Tags sections), keyed by
    /// stack-set name.
    pub cfn_stackset_detail: LazyMap<crate::aws::services::cloudformation::CfnStackSetDetailData>,
    /// A stack set's instances, keyed by stack-set name.
    pub cfn_stackset_instances:
        LazyMap<Vec<crate::aws::services::cloudformation::CfnStackInstance>>,
    /// A stack set's operation history, keyed by stack-set name.
    pub cfn_stackset_operations:
        LazyMap<Vec<crate::aws::services::cloudformation::CfnStackSetOperation>>,

    // ── CodeSuite ────────────────────────────────────────────────────────
    /// A pipeline's stage/action definitions, keyed by pipeline name.
    pub code_pipeline_details: LazyMap<crate::aws::services::code::CodePipelineDetails>,
    /// What one pipeline run actually did, stage by stage, keyed by pipeline
    /// execution id. (The runs themselves are list rows, not a lazy fetch.)
    pub code_action_executions:
        LazyMap<Vec<crate::aws::services::code::PipelineActionExecution>>,
    /// A repo's README (`None` = no README on the default branch), keyed by
    /// repo name.
    pub code_commit_readmes: LazyMap<Option<String>>,
    /// A repo's branches with tip commit metadata, keyed by repo name.
    pub cc_branches: LazyMap<crate::aws::services::code::CcBranches>,
    /// A branch's first-parent commit walk, keyed by `repo\u{1}branch`
    /// (the Commits section re-keys per branch via `cc_commits_branch`).
    pub cc_branch_commits: LazyMap<crate::aws::services::code::CcCommitWalk>,
    /// A repo's tags + triggers (feeds the Details and Triggers sections),
    /// keyed by repo name.
    pub cc_repo_extras: LazyMap<crate::aws::services::code::CcRepoExtras>,
    /// A repo's recently-closed PRs (the repo pane's Pull Requests section —
    /// the eager list phase loads open PRs only), keyed by repo name.
    pub cc_repo_closed_prs: LazyMap<Vec<crate::aws::services::code::CodeCommitPullRequest>>,
    /// A PR's approval-rule evaluation + per-user approval states, keyed by
    /// PR id (revision-scoped inside the fetch).
    pub cc_pr_approvals: LazyMap<crate::aws::services::code::CcPrApprovals>,
    /// A PR's activity timeline, keyed by PR id.
    pub cc_pr_events: LazyMap<Vec<crate::aws::services::code::CcPrEvent>>,
    /// A PR's comment threads, keyed by PR id.
    pub cc_pr_comments: LazyMap<crate::aws::services::code::CcPrComments>,
    /// A PR's changed-file list (merge base → source tip), keyed by PR id.
    pub cc_pr_diff: LazyMap<crate::aws::services::code::CcPrDiff>,
    /// A project's recent builds, keyed by project name.
    pub code_build_builds: LazyMap<Vec<crate::aws::services::code::CodeBuildBuild>>,
    /// A project's resolved buildspec, keyed by project name.
    pub code_build_buildspecs: LazyMap<crate::aws::services::code::BuildspecResult>,
    /// A deployment group's recent deployments, keyed by group id.
    pub code_deploy_deployments:
        LazyMap<Vec<crate::aws::services::code::CodeDeployDeployment>>,
    /// A CodeArtifact repo's packages, keyed by repo ARN.
    pub code_artifact_packages:
        LazyMap<Vec<crate::aws::services::code::CodeArtifactPackage>>,

    // ── EC2 detail sections ──────────────────────────────────────────────
    /// The role(s) inside an instance's IAM instance profile, keyed by
    /// profile name (the trailing segment of the profile ARN).
    pub ec2_instance_profile_roles: LazyMap<Vec<crate::aws::services::ec2::InstanceProfileRole>>,
    /// An instance's launch user data, base64-decoded, keyed by instance id.
    /// `Loaded(None)` = no user data configured (the common case).
    pub ec2_instance_user_data: LazyMap<Option<String>>,
    /// An instance's system console output (`GetConsoleOutput`), keyed by
    /// instance id. `Loaded(None)` = AWS has posted nothing for it yet.
    pub ec2_instance_console: LazyMap<Option<crate::aws::services::ec2::ConsoleOutput>>,
    /// The ENIs a security group is attached to (the Used-By section), keyed
    /// by group id. Shared by the EC2 and VPC security-group panes.
    pub sg_network_interfaces: LazyMap<Vec<crate::aws::services::ec2::SgEni>>,
    /// An EBS volume's snapshots, keyed by volume id.
    pub ebs_snapshots: LazyMap<Vec<crate::aws::services::ec2::EbsSnapshot>>,
    /// An AMI's launch permissions (sharing), keyed by image id.
    pub ami_permissions: LazyMap<Vec<String>>,
    /// A launch template's versions (Versions + Data sections), keyed by
    /// template id.
    pub lt_versions: LazyMap<Vec<crate::aws::services::ec2::LaunchTemplateVersion>>,

    // ── IAM / Organizations ──────────────────────────────────────────────
    /// A role's trust policy + attached/inline policies, keyed by role name.
    pub iam_role_details: LazyMap<crate::aws::services::iam::IamRoleDetails>,
    /// A managed policy's document + attachments, keyed by policy ARN.
    pub iam_policy_details: LazyMap<crate::aws::services::iam::IamPolicyDetails>,
    /// A user's access keys / MFA / groups / policies, keyed by user name.
    pub iam_user_details: LazyMap<crate::aws::services::iam::IamUserDetails>,
    /// A group's members + policies, keyed by group name.
    pub iam_group_details: LazyMap<crate::aws::services::iam::IamGroupDetails>,
    /// An identity provider's config + tags (SAML/OIDC get call), keyed by ARN.
    pub iam_idp_details: LazyMap<crate::aws::services::iam::IamIdpDetails>,
    /// An account's parents/OUs + attached SCPs, keyed by account id.
    pub org_account_details: LazyMap<crate::aws::services::organizations::OrgAccountDetails>,
    /// An SCP's document + attached targets, keyed by policy id.
    pub org_scp_details: LazyMap<crate::aws::services::organizations::OrgScpDetails>,
    /// An OU's child OUs + accounts, attached policies and tags, keyed by OU id.
    pub org_ou_details: LazyMap<crate::aws::services::organizations::OrgUnitDetails>,

    // ── Athena / Cost ────────────────────────────────────────────────────
    /// A database's table metadata, keyed by `catalog/db`.
    pub athena_tables: LazyMap<Vec<crate::aws::services::athena::AthenaTable>>,
    /// A workgroup's tags, keyed by workgroup name.
    pub athena_workgroup_tags: LazyMap<Vec<(String, String)>>,
    /// A cost row's drill-down, keyed by `App::cost_drilldown_key` (period +
    /// row key — never the bare row key).
    pub cost_drilldown: LazyMap<Box<crate::aws::services::cost::CostDrilldown>>,
    /// A budget's notifications + subscribers, keyed by budget name.
    pub budget_notifications: LazyMap<Vec<crate::aws::services::budgets::BudgetNotification>>,
    /// A budget's tags, keyed by budget name.
    pub budget_tags: LazyMap<Vec<(String, String)>>,

    // ── Control Tower ────────────────────────────────────────────────────
    // (`tower_` prefix — `ct_*` is CloudTrail throughout the codebase.)
    /// An enabled control's parameters + target regions, keyed by the
    /// enabled-control ARN.
    pub tower_control_details:
        LazyMap<crate::aws::services::controltower::CtEnabledControlDetail>,
    /// An enabled baseline's parameters + drift, keyed by its ARN.
    pub tower_baseline_details:
        LazyMap<crate::aws::services::controltower::CtEnabledBaselineDetail>,
    /// Tags for any Control Tower resource (landing zone / enabled control /
    /// enabled baseline share one map), keyed by resource ARN.
    pub tower_tags: LazyMap<Vec<(String, String)>>,
    /// A compliance row's non-compliant resources (Config aggregator drill),
    /// keyed by the row's `rule|account|region` key.
    pub tower_compliance_resources:
        LazyMap<Vec<crate::aws::services::controltower::CtComplianceResource>>,

    // ── Security Hub ─────────────────────────────────────────────────────
    /// A finding's ASFF audit trail (`GetFindingHistory`) — who changed the
    /// workflow status or note, and when. Keyed by finding id.
    pub sh_finding_history:
        LazyMap<Vec<crate::aws::services::security_hub::ShHistoryRecord>>,
    /// An insight's grouped counts (`GetInsightResults`), keyed by insight ARN.
    pub sh_insight_results: LazyMap<crate::aws::services::security_hub::ShInsightResults>,

    // ── S3 ───────────────────────────────────────────────────────────────
    /// A bucket's Tier-2 detailed configuration, keyed by bucket name.
    pub s3_bucket_details: LazyMap<Box<crate::aws::services::s3::S3BucketDetails>>,
    /// An object's HeadObject metadata (object-browser detail), keyed by
    /// `S3ObjectBrowserState::meta_key(bucket, key)`.
    pub s3_object_meta: LazyMap<Box<crate::aws::services::s3::S3ObjectMeta>>,
    /// A bucket's daily CloudWatch storage metrics (Metadata view; fixed 30d
    /// window so it never refetches), keyed by bucket name.
    pub s3_storage_metrics: LazyMap<Box<crate::aws::services::s3::S3StorageMetrics>>,

    // ── DynamoDB ─────────────────────────────────────────────────────────
    /// A table's TTL status `(enabled, attribute)`, keyed by table name.
    pub ddb_ttl: LazyMap<(bool, Option<String>)>,
    /// A table's tags, keyed by table name.
    pub ddb_tags: LazyMap<HashMap<String, String>>,

    // ── Route 53 (+ Resolver) ────────────────────────────────────────────
    /// A hosted zone's records, keyed by zone id.
    pub r53_zone_records: LazyMap<Vec<crate::aws::services::route53::R53Record>>,
    /// A hosted zone's VPC associations + pending authorizations + tags,
    /// keyed by zone id.
    pub r53_zone_detail: LazyMap<Box<crate::aws::services::route53::R53ZoneDetail>>,
    /// A health check's per-region live observations, keyed by check id.
    pub r53_health_status: LazyMap<Vec<crate::aws::services::route53::R53HealthObservation>>,
    /// A Resolver endpoint's IPs + rules + tags, keyed by endpoint id.
    pub resolver_endpoint_details:
        LazyMap<Box<crate::aws::services::route53resolver::ResolverEndpointDetail>>,
    /// A Resolver rule's VPC associations + tags, keyed by rule id.
    pub resolver_rule_details:
        LazyMap<Box<crate::aws::services::route53resolver::ResolverRuleDetail>>,
    /// A Route53 Profile's status/owner + VPC associations + bundled DNS
    /// resources + tags, keyed by profile id.
    pub r53_profile_details:
        LazyMap<Box<crate::aws::services::route53profiles::Route53ProfileDetail>>,

    // ── Step Functions / ACM / CloudFront / SES / TGW ────────────────────
    /// A state machine's config + ASL definition + tags, keyed by ARN.
    pub sfn_details: LazyMap<Box<crate::aws::services::step_functions::SfnDetails>>,
    /// An execution's input/output/failure, keyed by execution ARN.
    pub sfn_exec_details: LazyMap<Box<crate::aws::services::step_functions::SfnExecutionDetail>>,
    /// An execution's event history + derived state spans, keyed by execution ARN.
    pub sfn_history: LazyMap<Box<crate::aws::services::step_functions::SfnHistory>>,
    /// A certificate's full details, keyed by certificate ARN.
    pub acm_cert_details: LazyMap<crate::aws::services::acm::AcmCertDetails>,
    /// A distribution's tags, keyed by distribution ARN.
    pub cf_distribution_tags: LazyMap<HashMap<String, String>>,
    /// A configuration set's event destinations, keyed by set name.
    pub ses_event_dests: LazyMap<Vec<crate::aws::services::ses::SesEventDest>>,
    /// A TGW route table's routes, keyed by route-table id.
    pub tgw_routes: LazyMap<Vec<crate::aws::services::transit_gateway::TgwRoute>>,

    // ── EventBridge ──────────────────────────────────────────────────────
    /// A rule's targets, keyed by `bus/rule`.
    pub eb_targets: LazyMap<Vec<crate::aws::services::eventbridge::EbTarget>>,
    /// A schedule's `GetSchedule` detail, keyed by schedule ARN.
    pub eb_schedule_detail: LazyMap<Box<crate::aws::services::eventbridge::EbScheduleDetail>>,
    /// A pipe's `DescribePipe` detail, keyed by pipe ARN.
    pub eb_pipe_detail: LazyMap<crate::aws::services::eventbridge::EbPipeDetail>,

    // ── ECS ──────────────────────────────────────────────────────────────
    /// A service's tasks (running + recently stopped), keyed by service ARN.
    pub ecs_service_tasks: LazyMap<Vec<crate::aws::services::ecs::EcsTask>>,
    /// A task definition's full detail, keyed by task-definition ARN.
    pub ecs_taskdef_details: LazyMap<Box<crate::aws::services::ecs::EcsTaskDefinitionDetails>>,

    // ── FSx ──────────────────────────────────────────────────────────────
    /// A file system's volumes, keyed by file-system id.
    pub fsx_volumes: LazyMap<Vec<crate::aws::services::fsx::FsxVolume>>,
    /// A volume's live used/utilization snapshot, keyed by volume id.
    pub fsx_volume_detail: LazyMap<crate::aws::services::fsx::FsxVolumeSnapshot>,
    /// A file system's backups, keyed by file-system id.
    pub fsx_backups: LazyMap<Vec<crate::aws::services::fsx::FsxBackup>>,

    // ── Health / Network Firewall ────────────────────────────────────────
    /// A Health event's description + affected entities, keyed by event ARN.
    pub health_event_details: LazyMap<crate::aws::services::health::HealthEventDetails>,
    /// A firewall's logging destinations, keyed by firewall ARN.
    pub nfw_firewall_logging:
        LazyMap<Vec<crate::aws::services::network_firewall::NfwLogDestination>>,
    /// A rule group's contents (Suricata text / 5-tuple table), keyed by ARN.
    pub nfw_rule_group_rules:
        LazyMap<crate::aws::services::network_firewall::NfwRuleGroupRules>,

    // ── CloudWatch ───────────────────────────────────────────────────────
    /// A dashboard's parsed body, keyed by dashboard name.
    pub cw_dashboards: LazyMap<crate::aws::services::cloudwatch::CwDashboardBody>,
    /// An alarm's state-transition history, keyed by alarm name.
    pub cw_alarm_history: LazyMap<Vec<crate::aws::services::cloudwatch::CwAlarmHistoryItem>>,
    /// A log group's subscription + metric filters, keyed by group name.
    pub cw_log_group_filters: LazyMap<crate::aws::services::cloudwatch::CwLogGroupFilters>,
    /// A log group's most-recently-active streams, keyed by group name.
    pub cw_log_streams: LazyMap<Vec<crate::aws::services::cloudwatch::CwLogStream>>,
    /// A metric stream's filters + statistics configs, keyed by stream name.
    pub cw_metric_stream: LazyMap<crate::aws::services::cloudwatch::CwMetricStreamDetail>,
    /// An OAM sink's policy (who may link), keyed by sink ARN.
    pub oam_sink_policy: LazyMap<crate::aws::services::oam::OamSinkPolicy>,
    /// An OAM sink's attached source-account links, keyed by sink ARN.
    pub oam_attached_links: LazyMap<Vec<crate::aws::services::oam::OamAttachedLink>>,
    /// An OAM link's full configuration (filters, tags), keyed by link ARN.
    pub oam_link_detail: LazyMap<crate::aws::services::oam::OamLinkDetail>,

    // ── Messaging / Config ───────────────────────────────────────────────
    /// An SNS topic's subscriptions, keyed by topic ARN.
    pub sns_subscriptions: LazyMap<Vec<crate::aws::services::messaging::SnsSubscription>>,
    /// A Config rule's non-compliant resources, keyed by rule name.
    pub config_eval: LazyMap<Vec<crate::aws::services::awsconfig::ConfigEvalResult>>,

    // ── ECR / Cognito / Backup ───────────────────────────────────────────
    /// A repo's images (newest first), keyed by repo name.
    pub ecr_repo_images: LazyMap<Vec<crate::aws::services::ecr::EcrImage>>,
    /// A repo's lifecycle-policy JSON (empty = none), keyed by repo name.
    pub ecr_lifecycle: LazyMap<String>,
    /// A user pool's app clients, keyed by pool id.
    pub cognito_clients: LazyMap<Vec<crate::aws::services::cognito::CognitoAppClient>>,
    /// A backup plan's rules + selections, keyed by plan id.
    pub backup_plan_details: LazyMap<crate::aws::services::backup::BackupPlanDetails>,
    /// A vault's recovery points, keyed by vault name.
    pub backup_recovery_points: LazyMap<Vec<crate::aws::services::backup::RecoveryPoint>>,

    // ── Service Catalog ──────────────────────────────────────────────────
    /// A portfolio's products (`SearchProductsAsAdmin(portfolio_id)`), keyed
    /// by portfolio id.
    pub sc_portfolio_products: LazyMap<Vec<crate::aws::services::servicecatalog::ScPortfolioProduct>>,
    /// A portfolio's principals + constraints (one fetch, two sections),
    /// keyed by portfolio id.
    pub sc_portfolio_access: LazyMap<crate::aws::services::servicecatalog::ScPortfolioAccess>,
    /// A portfolio's shares across all four share types, keyed by portfolio id.
    pub sc_portfolio_shares: LazyMap<Vec<crate::aws::services::servicecatalog::ScShare>>,
    /// A portfolio's tags + associated TagOptions (`DescribePortfolio` — the
    /// list API returns neither), keyed by portfolio id.
    pub sc_portfolio_extras: LazyMap<crate::aws::services::servicecatalog::ScPortfolioExtras>,
    /// A product's admin details (artifacts + tags + tag options + portfolios
    /// — one `DescribeProductAsAdmin` feeds three sections), keyed by product id.
    pub sc_product_details: LazyMap<Box<crate::aws::services::servicecatalog::ScProductAdminDetails>>,
    /// A provisioned product's outputs as `(key, value, description)`, keyed
    /// by provisioned-product id.
    pub sc_pp_outputs: LazyMap<Vec<(String, String, String)>>,
    /// A provisioned product's record history (newest first), keyed by
    /// provisioned-product id.
    pub sc_pp_records: LazyMap<Vec<crate::aws::services::servicecatalog::ScRecord>>,

    // ── S3 Files ─────────────────────────────────────────────────────────
    /// GetFileSystem enrichment (prefix, KMS key, tags — none of which the
    /// list API returns), keyed by file-system id.
    pub s3files_details: LazyMap<crate::aws::services::s3files::S3FsExtras>,
    /// A file system's mount targets, keyed by file-system id.
    pub s3files_mount_targets: LazyMap<Vec<crate::aws::services::s3files::S3FsMountTarget>>,
    /// A file system's access points (capped), keyed by file-system id.
    pub s3files_access_points: LazyMap<Vec<crate::aws::services::s3files::S3FsAccessPoint>>,
    /// A file system's resource policy (`None` = no policy attached), keyed
    /// by file-system id.
    pub s3files_policy: LazyMap<Option<String>>,
    /// A file system's synchronization configuration, keyed by file-system id.
    pub s3files_sync: LazyMap<crate::aws::services::s3files::S3FsSyncConfig>,
    /// The file systems linked to a bucket (one filtered `ListFileSystems`
    /// sweep — feeds the S3 bucket pane's File Systems section), keyed by
    /// bucket name.
    pub s3_bucket_filesystems: LazyMap<Vec<crate::aws::services::s3files::S3FileSystem>>,

    // ── S3 Tables ────────────────────────────────────────────────────────
    /// A table bucket's encryption config + tags (neither in the list
    /// output), keyed by bucket ARN.
    pub s3tables_bucket_extras: LazyMap<crate::aws::services::s3tables::S3TableBucketExtras>,
    /// A table bucket's namespaces (capped), keyed by bucket ARN.
    pub s3tables_namespaces: LazyMap<Vec<crate::aws::services::s3tables::S3TablesNamespace>>,
    /// A table bucket's maintenance configuration (unreferenced-file
    /// removal), keyed by bucket ARN.
    pub s3tables_bucket_maintenance: LazyMap<Vec<crate::aws::services::s3tables::MaintenanceRow>>,
    /// A table bucket's resource policy (`None` = none attached), keyed by
    /// bucket ARN.
    pub s3tables_bucket_policy: LazyMap<Option<String>>,
    /// GetTable enrichment (format, metadata/warehouse locations, tags),
    /// keyed by table ARN.
    pub s3tables_table_extras: LazyMap<Box<crate::aws::services::s3tables::S3TableExtras>>,
    /// A table's maintenance config + per-job last-run status, keyed by
    /// table ARN.
    pub s3tables_table_maintenance: LazyMap<crate::aws::services::s3tables::S3TableMaintenance>,
    /// A table's resource policy (`None` = none attached), keyed by table ARN.
    pub s3tables_table_policy: LazyMap<Option<String>>,

    // ── ELB / ASG ────────────────────────────────────────────────────────
    /// A target group's per-target health, keyed by target-group ARN.
    pub target_group_health: LazyMap<Vec<crate::aws::services::elb::TargetHealthEntry>>,
    /// A target group's attributes, keyed by target-group ARN.
    pub target_group_attributes: LazyMap<Vec<(String, String)>>,
    /// A load balancer's listeners + attributes, keyed by LB ARN.
    pub load_balancer_details: LazyMap<crate::aws::services::elb::LbDetails>,
    /// An Auto Scaling group's scaling activities, keyed by group name.
    pub asg_activities: LazyMap<Vec<crate::aws::services::asg::ScalingActivity>>,
}

impl LazyStore {
    /// A fresh store at the given epoch (see the type docs for when).
    pub fn new(epoch: u64) -> Self {
        Self {
            epoch,
            ..Default::default()
        }
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }
}

/// The payload of the one `Event::Lazy` variant: an epoch-stamped closure
/// that writes a fetch result into its [`LazyMap`]. The single handler arm in
/// `App::handle_event` runs it — or drops it when the epoch is stale.
pub struct LazyApply {
    pub epoch: u64,
    pub apply: Box<dyn FnOnce(&mut crate::app::App) + Send>,
}

impl std::fmt::Debug for LazyApply {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LazyApply")
            .field("epoch", &self.epoch)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_maps_ok_to_loaded_and_err_to_error() {
        let mut m: LazyMap<u32> = LazyMap::default();
        m.insert_loading("a".into());
        assert!(matches!(m.get("a"), Some(Lazy::Loading)));
        m.apply("a".into(), Ok(7));
        assert!(matches!(m.get("a"), Some(Lazy::Loaded(7))));
        m.apply("b".into(), Err("denied".into()));
        assert!(matches!(m.get("b"), Some(Lazy::Error(e)) if e == "denied"));
    }

    #[test]
    fn contains_blocks_retrigger_until_invalidated() {
        let mut m: LazyMap<u32> = LazyMap::default();
        m.apply("a".into(), Err("boom".into()));
        assert!(m.contains("a"), "an Error entry must block re-trigger");
        m.invalidate("a");
        assert!(!m.contains("a"), "invalidate must allow a refetch");
    }

    #[test]
    fn new_store_is_empty_at_the_given_epoch() {
        let mut store = LazyStore::new(3);
        store.pl_entries.insert_loading("pl-1".into());
        store = LazyStore::new(store.epoch() + 1);
        assert_eq!(store.epoch(), 4);
        assert!(store.pl_entries.get("pl-1").is_none());
    }
}
