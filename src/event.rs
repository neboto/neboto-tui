use crate::aws::region::Region;
use crate::aws::resource::Resource;
use crate::aws::service::ServiceType;
use crate::aws::services::cloudwatch::CwAlarmMetricsData;
use crate::aws::services::asg::AsgMetricsData;
use crate::aws::services::ec2::{Ec2MetricsData, EbsMetricsData, SsmInstanceStatus};
use std::collections::HashMap;
use crate::aws::services::ecs::{EcsMetricsData, EcsTaskMetricsData};
use crate::aws::services::lambda::LambdaMetricsData;
use crate::aws::services::rds::RdsMetricsData;
use crossterm::event::{KeyEvent, MouseEvent};
use std::time::Duration;
use tokio::sync::mpsc;

/// Progress information for incremental resource loading
#[derive(Debug, Clone)]
pub struct LoadProgress {
    pub loaded_count: usize,
    pub total_count: Option<usize>,     // None if total unknown
    pub status_message: Option<String>, // e.g., "Filtering buckets..."
}

#[derive(Debug)]
pub enum Event {
    // User input events
    Key(KeyEvent),
    Mouse(MouseEvent),
    Resize,

    // AWS async events
    ResourcesLoaded {
        service: ServiceType,
        resources: Vec<Box<dyn Resource>>,
    },
    ResourcesPartiallyLoaded {
        service: ServiceType,
        resources: Vec<Box<dyn Resource>>, // Incremental batch
        progress: LoadProgress,
    },
    ResourcesFullyLoaded {
        service: ServiceType,
        total_count: usize,
    },
    /// A single resource was re-fetched in place (detail-pane `r`, or a
    /// watch-mode tick); replace the matching resource (by `id`) without
    /// reloading the whole service. `quiet` (watch mode) suppresses the
    /// "Refreshed …" toast.
    ResourceRefreshed {
        id: String,
        resource: Box<dyn Resource>,
        quiet: bool,
    },
    /// A targeted single-resource refresh failed (e.g. the resource is gone).
    ResourceRefreshFailed {
        error: String,
    },
    ResourceLoadError {
        service: ServiceType,
        error: String,
    },
    /// Non-fatal: one phase of a multi-phase load failed but the load keeps
    /// streaming. Unlike `ResourceLoadError` this must NOT clear the loading
    /// state (that would make `handle_resources_partially_loaded` drop every
    /// later batch). Warnings accumulate and surface when the load completes.
    ResourceLoadWarning {
        service: ServiceType,
        warning: String,
    },
    /// A list-load stream event (`ResourcesLoaded` / `PartiallyLoaded` /
    /// `FullyLoaded` / `LoadError` / `LoadWarning`) tagged by the forwarder in
    /// `App::load_resources_async` with the `load_generation` it belongs to.
    /// `handle_event` re-checks the generation *when the event is handled*,
    /// not just when it was forwarded: while `handle_event` is blocked in a
    /// profile/region/role switch (an awaited client build), a superseded
    /// stream's events pile up in the channel past the forwarder's check and
    /// would otherwise land on the new credentials' empty list — the "old
    /// account's instances under the new profile, and `r` can't shake them"
    /// bug.
    LoadStream {
        generation: u64,
        event: Box<Event>,
    },

    RegionSwitchRequested {
        region: Region,
    },

    ProfileSwitchRequested {
        profile: String,
    },

    /// Assume `role_name` in a member account (the `s` action on an
    /// Organizations account row — direct with one configured role, via the
    /// role picker with several). Sessions are scoped to ReadOnlyAccess; see
    /// `AwsClients::assume_org_role`.
    OrgRoleSwitchRequested {
        account_id: String,
        account_name: String,
        role_name: String,
    },

    /// Drop the assumed member-account role and return to base credentials.
    OrgRoleExitRequested,

    Ec2MetricsLoaded {
        instance_id: String,
        data: Ec2MetricsData,
    },

    EbsMetricsLoaded {
        volume_id: String,
        data: EbsMetricsData,
    },

    AsgMetricsLoaded {
        group_name: String,
        data: AsgMetricsData,
    },

    LambdaMetricsLoaded {
        function_name: String,
        data: LambdaMetricsData,
    },

    /// `d` on the Code section: deployment package downloaded + extracted.
    LambdaCodeDownloaded {
        function_name: String,
        result: std::result::Result<(String, usize), String>, // (path, files)
    },

    /// `d` on an invoice's detail pane: PDF downloaded via a fresh presigned URL.
    InvoicePdfDownloaded {
        invoice_id: String,
        result: std::result::Result<(String, u64), String>, // (path, bytes)
    },

    EcsMetricsLoaded {
        service_key: String,
        data: EcsMetricsData,
    },

    EcsTaskMetricsLoaded {
        task_key: String,
        data: EcsTaskMetricsData,
    },

    RdsMetricsLoaded {
        db_identifier: String,
        data: RdsMetricsData,
    },

    AccountInfoLoaded {
        account_id: String,
        alias: Option<String>,
        /// `App.account_info_generation` at spawn time — an event stamped with
        /// anything older belongs to superseded credentials and is dropped.
        generation: u64,
    },

    /// Fired when a single managed/inline policy document has been fetched
    /// (per-row "view document" action in the IAM Role Permissions section).
    IamPolicyDocumentLoaded {
        title: String,
        content: String,
        open_editor: bool,
    },

    /// Fired when a single SCP's policy document has been fetched
    /// (per-row "view document" action in the OrgAccount SCPs section);
    /// opens in `$EDITOR`.
    OrgScpDocumentLoaded {
        content: String,
    },

    /// Fired when a CodeCommit PR's unified patch has been assembled (blob
    /// fetches + local diff, `e` on the Changes section); opens in `$EDITOR`.
    CcPrPatchLoaded {
        content: String,
    },

    /// Fired when recent logs for an ECS task's containers have been fetched
    /// and are ready to open in `$EDITOR`.
    CwLogInsightsLoaded {
        content: String,
    },

    /// Fired when a Service Catalog provisioning artifact's template body has
    /// been downloaded (`e` on a version in the product's Versions section);
    /// opens in `$EDITOR` with a JSON/YAML suffix sniffed from the body.
    ScTemplateLoaded {
        result: Result<String, String>,
    },

    /// Fired when a CloudWatch alarm's own metric time series has been fetched
    /// for the metric-chart overlay (`m` on a CwAlarm).
    CwAlarmMetricsLoaded {
        alarm_arn: String,
        data: CwAlarmMetricsData,
    },

    /// Fired when the metric explorer's metric list has been fetched (lazily, on
    /// first switch to the Metrics sub-tab). `capped` = the result was truncated.
    CwMetricsLoaded {
        metrics: Vec<crate::aws::services::cloudwatch::CwMetric>,
        capped: bool,
    },

    /// Fired when an opt-in secret/parameter value reveal (`x`) completes.
    /// The content is opened transiently in `$EDITOR` and never cached.
    SecretValueRevealed {
        content: String,
    },

    /// Fired when an opt-in secret/parameter value copy (`Y`) completes.
    /// The content is written straight to the clipboard and never echoed.
    SecretValueCopied {
        name: String,
        content: String,
    },

    /// Fired when SSM Session Manager connectability for the region's EC2
    /// instances has been fetched via `ssm:DescribeInstanceInformation`.
    /// Keyed by instance id; instances absent from the map aren't SSM-managed.
    SsmInstanceInfoLoaded {
        statuses: HashMap<String, SsmInstanceStatus>,
    },

    /// Fired when a WAF Web ACL's traffic metrics have been fetched.
    WafMetricsLoaded {
        web_acl_name: String,
        result: Result<Box<crate::aws::services::waf::WafMetricsData>, String>,
    },

    /// Fired when a Network Firewall's traffic metrics have been fetched.
    /// Keyed by firewall name.
    NfwMetricsLoaded {
        firewall_name: String,
        data: crate::aws::services::network_firewall::NfwMetricsData,
    },


    /// Fired when the Organizations account-name map for Identity Center
    /// assignment rows is fetched (best-effort; empty on a permission gap).
    IcAccountNamesLoaded {
        names: std::collections::HashMap<String, String>,
    },

    /// Fired when WAF sampled requests have been fetched (`e` on a Web ACL,
    /// either pane); opens in `$EDITOR`.
    WafSampledRequestsLoaded {
        content: String,
    },

    /// Compute Optimizer enrollment status (one-shot, cached on `App`).
    CoEnrollmentLoaded {
        enrollment: crate::aws::services::computeoptimizer::CoEnrollment,
    },

    /// Fired when an EKS cluster's Container Insights metrics are fetched.
    EksMetricsLoaded {
        cluster: String,
        data: crate::aws::services::eks::EksMetricsData,
    },

    /// EFS file system CloudWatch metrics (`m` overlay).
    EfsMetricsLoaded {
        file_system_id: String,
        data: crate::aws::services::efs::EfsMetricsData,
    },

    /// ElastiCache cluster CloudWatch metrics (`m` overlay), keyed by cache id.
    ElastiCacheMetricsLoaded {
        cache_id: String,
        data: crate::aws::services::elasticache::ElastiCacheMetricsData,
    },

    /// OpenSearch domain CloudWatch metrics (`m` overlay), keyed by domain name.
    OpenSearchMetricsLoaded {
        domain: String,
        data: crate::aws::services::opensearch::OpenSearchMetricsData,
    },

    /// Kinesis stream CloudWatch metrics (`m` overlay), keyed by stream name.
    KinesisMetricsLoaded {
        stream_name: String,
        data: crate::aws::services::kinesis::KinesisMetricsData,
    },

    /// Firehose delivery-stream CloudWatch metrics (`m` overlay), keyed by name.
    FirehoseMetricsLoaded {
        stream_name: String,
        data: crate::aws::services::kinesis::FirehoseMetricsData,
    },

    /// Redshift cluster CloudWatch metrics (`m` overlay), keyed by cluster id.
    RedshiftMetricsLoaded {
        cluster_id: String,
        data: crate::aws::services::redshift::RedshiftMetricsData,
    },

    /// Redshift Serverless workgroup CloudWatch metrics (`m` overlay), keyed
    /// by workgroup name.
    RedshiftSlMetricsLoaded {
        workgroup: String,
        data: crate::aws::services::redshift::RedshiftSlMetricsData,
    },

    /// CloudFront distribution CloudWatch metrics (`m` overlay), keyed by id.
    CloudFrontMetricsLoaded {
        distribution_id: String,
        data: crate::aws::services::cloudfront::CfMetricsData,
    },

    /// Bedrock model / inference-profile CloudWatch metrics (`m` overlay),
    /// keyed by resource id (the `ModelId` dimension).
    BedrockMetricsLoaded {
        model_id: String,
        data: crate::aws::services::bedrock::BedrockMetricsData,
    },

    /// S3 bucket daily storage metrics (`m` overlay), keyed by bucket name.
    S3MetricsLoaded {
        bucket: String,
        data: crate::aws::services::s3::S3MetricsData,
    },

    /// Transfer Family server CloudWatch metrics (`m` overlay), keyed by server id.
    TransferMetricsLoaded {
        server_id: String,
        data: crate::aws::services::transfer::TransferMetricsData,
    },

    /// NAT gateway CloudWatch metrics (`m` overlay), keyed by nat gateway id.
    NatMetricsLoaded {
        nat_id: String,
        data: crate::aws::services::vpc::NatMetricsData,
    },

    /// VPN connection CloudWatch metrics (`m` overlay), keyed by vpn id.
    VpnMetricsLoaded {
        vpn_id: String,
        data: crate::aws::services::vpc::VpnMetricsData,
    },

    /// Direct Connect connection CloudWatch metrics (`m` overlay), keyed by connection id.
    DxMetricsLoaded {
        connection_id: String,
        data: crate::aws::services::direct_connect::DxMetricsData,
    },

    /// Direct Connect per-VIF traffic metrics (`m` overlay), keyed by VIF id.
    DxVifMetricsLoaded {
        vif_id: String,
        data: crate::aws::services::direct_connect::DxVifMetricsData,
    },

    /// VPC endpoint (PrivateLink) CloudWatch metrics (`m` overlay), keyed by endpoint id.
    VpceMetricsLoaded {
        endpoint_id: String,
        data: crate::aws::services::vpc::VpceMetricsData,
    },

    /// Transit gateway / attachment CloudWatch metrics (`m` overlay), keyed by
    /// the selected resource's id (tgw- or tgw-attach-).
    TgwMetricsLoaded {
        id: String,
        data: crate::aws::services::transit_gateway::TgwMetricsData,
    },

    /// Log-group ingestion metrics (`m` overlay), keyed by the group ARN
    /// (`CwLogGroup::id()`).
    LogGroupMetricsLoaded {
        arn: String,
        data: crate::aws::services::cloudwatch::LogGroupMetricsData,
    },

    /// A charted CloudWatch dashboard (`m` overlay), keyed by dashboard name.
    /// Boxed — the payload carries every widget and every series.
    CwDashboardMetricsLoaded {
        name: String,
        data: Box<crate::aws::services::cloudwatch::CwDashboardMetricsData>,
    },

    /// The dashboard fetch failed (`GetDashboard` / `GetMetricData`). Unlike the
    /// other `m` overlays this renders inline rather than dropping to a spinner
    /// forever — a denied `GetDashboard` is the common case.
    CwDashboardMetricsFailed {
        name: String,
        error: String,
    },

    /// EventBridge rule CloudWatch metrics (`m` overlay), keyed by the rule ARN.
    EbRuleMetricsLoaded {
        arn: String,
        data: crate::aws::services::eventbridge::EbRuleMetricsData,
    },

    /// Hosted-zone DNS query metrics (`m` overlay), keyed by the zone's full
    /// path id (`/hostedzone/Z…`).
    R53ZoneMetricsLoaded {
        id: String,
        data: crate::aws::services::route53::R53ZoneMetricsData,
    },

    /// Resolver endpoint query-volume metrics (`m` overlay), keyed by endpoint id.
    ResolverEpMetricsLoaded {
        id: String,
        data: crate::aws::services::route53resolver::ResolverEpMetricsData,
    },

    /// CodeBuild project CloudWatch metrics (`m` overlay), keyed by the project ARN.
    CodeBuildMetricsLoaded {
        arn: String,
        data: crate::aws::services::code::CodeBuildMetricsData,
    },

    /// Bedrock AgentCore CloudWatch metrics (`m` overlay), keyed by the
    /// resource id (one map covers all five families).
    AgentCoreMetricsLoaded {
        key: String,
        data: Box<crate::aws::services::agentcore::AgentCoreMetricsData>,
    },

    /// ECR repository CloudWatch metrics (`m` overlay), keyed by repo name.
    EcrMetricsLoaded {
        repo: String,
        data: crate::aws::services::ecr::EcrMetricsData,
    },

    /// Global Accelerator CloudWatch metrics (`m` overlay), keyed by accelerator ARN.
    GaMetricsLoaded {
        arn: String,
        data: crate::aws::services::global_accelerator::GaMetricsData,
    },

    /// WorkSpaces CloudWatch metrics (`m` overlay), keyed by workspace id.
    WorkspaceMetricsLoaded {
        workspace_id: String,
        data: crate::aws::services::workspaces::WsMetricsData,
    },

    /// Cognito user-pool CloudWatch metrics (`m` overlay), keyed by pool id.
    CognitoMetricsLoaded {
        pool_id: String,
        data: crate::aws::services::cognito::CognitoMetricsData,
    },

    /// Athena per-workgroup CloudWatch metrics (`m` overlay), keyed by
    /// workgroup name.
    AthenaWgMetricsLoaded {
        workgroup: String,
        data: crate::aws::services::athena::AthenaWgMetricsData,
    },

    /// Glue per-job CloudWatch metrics (`m` overlay), keyed by job name.
    GlueJobMetricsLoaded {
        job_name: String,
        data: crate::aws::services::glue::GlueJobMetricsData,
    },


    /// Route 53 health-check CloudWatch metrics (`m` overlay), keyed by check id.
    R53HealthCheckMetricsLoaded {
        id: String,
        data: crate::aws::services::route53::R53HealthMetricsData,
    },

    /// SQS queue CloudWatch metrics (`m` overlay), keyed by queue url.
    SqsMetricsLoaded {
        queue_url: String,
        data: crate::aws::services::messaging::SqsMetricsData,
    },

    /// SNS topic CloudWatch metrics (`m` overlay), keyed by topic ARN.
    SnsMetricsLoaded {
        topic_arn: String,
        data: crate::aws::services::messaging::SnsMetricsData,
    },

    /// Step Functions state-machine CloudWatch metrics (`m` overlay), keyed by ARN.
    SfnMetricsLoaded {
        state_machine_arn: String,
        data: crate::aws::services::step_functions::SfnMetricsData,
    },

    /// FSx metrics, keyed by file-system id.
    FsxMetricsLoaded {
        file_system_id: String,
        data: crate::aws::services::fsx::FsxMetricsData,
    },
    FsxVolumeMetricsLoaded {
        volume_id: String,
        data: crate::aws::services::fsx::FsxMetricsData,
    },
    /// ELB load balancer / target group CloudWatch metrics (`m` overlay), keyed by ARN.
    ElbMetricsLoaded {
        arn: String,
        data: crate::aws::services::elb::ElbMetricsData,
    },

    /// The live tail resolved its log group + streams (for async-resolved
    /// sources: ECS task / Network Firewall / CodeBuild). Stored on the pane so
    /// `[`/`]` window re-seed and `s`-to-search work on those sources too.
    LogTailResolved {
        generation: u64,
        group: String,
        streams: Vec<String>,
    },
    /// A batch of new log lines for the live tail. `generation` guards against a
    /// stale poll loop appending to a tail that was closed/reopened.
    LogTailBatch {
        generation: u64,
        lines: Vec<crate::aws::services::cloudwatch::LogTailLine>,
    },
    /// The live tail's poll loop hit an error (shown inline, tail keeps polling).
    LogTailError {
        generation: u64,
        error: String,
    },

    /// Results of a one-shot quick log search (the "Search log group" view).
    /// `generation` guards against a stale search clobbering a newer one.
    LogSearchResults {
        generation: u64,
        lines: Vec<crate::aws::services::cloudwatch::LogTailLine>,
        capped: bool,
    },

    /// Result of the CloudTrail lens (`W`) one-shot `LookupEvents`.
    /// `generation` guards against a stale lookup landing after close/reopen.
    TrailLensLoaded {
        generation: u64,
        events: Vec<crate::aws::services::cloudtrail::CloudTrailEvent>,
    },
    /// The CloudTrail lens lookup failed.
    TrailLensError {
        generation: u64,
        error: String,
    },
    /// An auxiliary timeline source (alarm history / CFN stack events)
    /// landed for the lens. Same generation guard as the CloudTrail fetch.
    TrailLensAux {
        generation: u64,
        source: crate::timeline::TimelineSource,
        rows: Result<Vec<crate::timeline::TimelineRow>, String>,
    },

    /// Network-access lens (`N`): the `DescribeSecurityGroups` result for
    /// the groups that weren't in a warm cache. Generation-guarded like the
    /// timeline sources.
    AccessLensLoaded {
        generation: u64,
        groups: Result<Vec<crate::aws::services::ec2::SecurityGroup>, String>,
    },

    /// Fired when an API Gateway API's CloudWatch metrics have been fetched.
    ApiMetricsLoaded {
        api_id: String,
        data: crate::aws::services::api_gateway::ApiMetricsData,
    },

    /// Fired when the Service Quotas service list (for the picker) is fetched.
    QuotaServicesLoaded {
        services: Vec<(String, String)>,
    },

    /// DynamoDB table CloudWatch metrics (`m` overlay).
    DdbMetricsLoaded {
        table: String,
        data: crate::aws::services::dynamodb::DdbMetricsData,
    },

    /// A page of DynamoDB items (item browser Scan/Query).
    /// One page of the AgentCore memory browser's current level.
    MemoryBrowserLoaded {
        rows: Vec<crate::aws::services::agentcore::MemoryBrowserRow>,
        next_token: Option<String>,
        append: bool,
    },

    /// The memory browser's listing failed — rendered inside the overlay.
    MemoryBrowserError {
        message: String,
    },

    DdbItemsLoaded {
        items: Vec<std::collections::HashMap<String, aws_sdk_dynamodb::types::AttributeValue>>,
        last_key:
            Option<std::collections::HashMap<String, aws_sdk_dynamodb::types::AttributeValue>>,
        scanned: i32,
        append: bool,
    },

    /// The item browser query/scan failed.
    DdbItemsError {
        message: String,
    },

    /// A page of S3 objects/folders for the object browser (keyed by bucket +
    /// prefix so a stale response for a folder we've navigated away from is
    /// dropped). `append` extends the current listing (next page).
    S3ObjectsLoaded {
        bucket: String,
        prefix: String,
        result: Result<crate::aws::services::s3::S3ObjectPage, String>,
        append: bool,
    },

    /// An object was downloaded to disk (the `d` action).
    S3ObjectDownloaded {
        key: String,
        result: Result<std::path::PathBuf, String>,
    },

    /// An object's text content was fetched (`v` previews the first 1 MiB of
    /// oversize objects, `e` refuses them); both open in `$EDITOR`.
    S3ObjectContentLoaded {
        key: String,
        result: Result<String, String>,
    },

    /// Terraform state file parsed from S3.
    TfStateLoaded {
        result: Result<crate::terraform::TfState, String>,
    },

    /// How many buckets live in regions other than the current one (filtered
    /// out of the S3 list). Drives the "N in other regions" hint.
    S3RegionSummary {
        other_regions: usize,
    },

    /// A presigned GET URL for an object (the `p` action), copied to clipboard.
    S3PresignedUrl {
        key: String,
        result: Result<String, String>,
    },

    /// SES account-level posture (send quota / sending enabled), fetched once
    /// per load via `GetAccount` and folded into each identity's Overview.
    SesAccountLoaded {
        info: crate::aws::services::ses::SesAccountInfo,
    },

    /// SES account-wide `AWS/SES` metrics (`m` overlay); a single series, so it
    /// is stored under a fixed key rather than a per-resource one.
    SesMetricsLoaded {
        data: crate::aws::services::ses::SesMetricsData,
    },

    /// Firewall Manager admin posture (`GetAdminAccount`), fetched once per
    /// load and folded into each policy's Overview + the non-admin empty state.
    FmsAdminLoaded {
        info: crate::aws::services::fms::FmsAdminInfo,
    },

    /// A lazy-section fetch result: an epoch-stamped apply closure that
    /// writes into its LazyMap — the one delivery event for every migrated
    /// lazy fetch (docs/adr/0001-apply-closure-lazy-event.md). Don't add new
    /// per-fetch `*Loaded` variants for lazy sections.
    Lazy(crate::lazy::LazyApply),

    /// Fired when an MSK cluster's `AWS/Kafka` metrics have been fetched. Keyed
    /// by cluster ARN.
    MskMetricsLoaded {
        arn: String,
        data: crate::aws::services::msk::MskMetricsData,
    },

    // Application events
    Tick,
}

pub struct EventHandler {
    tx: mpsc::UnboundedSender<Event>,
    rx: mpsc::UnboundedReceiver<Event>,
}

impl EventHandler {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self { tx, rx }
    }

    pub fn sender(&self) -> mpsc::UnboundedSender<Event> {
        self.tx.clone()
    }

    pub async fn next(&mut self) -> Option<Event> {
        self.rx.recv().await
    }
}

// Spawn a task to handle terminal events
pub async fn handle_terminal_events(tx: mpsc::UnboundedSender<Event>) {
    use crossterm::event::{poll, read, Event as CrosstermEvent};

    loop {
        // Poll for events with a timeout
        match tokio::task::spawn_blocking(|| {
            if poll(Duration::from_millis(100)).unwrap_or(false) {
                read().ok()
            } else {
                None
            }
        })
        .await
        {
            Ok(Some(event)) => match event {
                CrosstermEvent::Key(key) => {
                    if tx.send(Event::Key(key)).is_err() {
                        break;
                    }
                }
                CrosstermEvent::Mouse(mouse) => {
                    if tx.send(Event::Mouse(mouse)).is_err() {
                        break;
                    }
                }
                CrosstermEvent::Resize(_, _) => {
                    if tx.send(Event::Resize).is_err() {
                        break;
                    }
                }
                _ => {}
            },
            Ok(None) => {
                // No event available, continue polling
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Err(_) => {
                // Task join error, break the loop
                break;
            }
        }
    }
}

// Spawn a task to generate tick events
pub async fn handle_tick_events(tx: mpsc::UnboundedSender<Event>, tick_rate: Duration) {
    let mut interval = tokio::time::interval(tick_rate);

    loop {
        interval.tick().await;
        if tx.send(Event::Tick).is_err() {
            break;
        }
    }
}
