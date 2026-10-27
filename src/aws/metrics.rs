//! Metrics charted from a spec table (`m`): a kind lists its CloudWatch
//! metrics here — name, label, statistic, unit — and one fetch plus one
//! renderer (`ui::widgets::spec_metrics`) does the rest, instead of a
//! hand-built fetcher, state map, event and chart page per kind. Ported from
//! nebaz's `azure::metrics`; CloudWatch needs more per metric than Azure
//! Monitor does (a namespace and a dimension set, and no units come back),
//! so the spec carries them.
//!
//! One `GetMetricData` call per open charts the whole set (billed per metric
//! requested, like `GetMetricStatistics` per request), and each metric that
//! fails says so on its own row. `a` lists what `ListMetrics` knows for the
//! resource's dimensions — every namespace, so a CloudWatch agent's
//! `CWAgent` metrics show up next to the AWS ones.

use crate::aws::services::ec2::MetricsTimeRange;
use aws_sdk_cloudwatch::primitives::DateTime as CwDateTime;
use aws_sdk_cloudwatch::types::{
    Dimension, DimensionFilter, Metric, MetricDataQuery, MetricStat, ScanBy, StatusCode,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stat {
    Average,
    Sum,
    Maximum,
    Minimum,
    P99,
}

impl Stat {
    /// The order `s` cycles through.
    pub const ALL: [Stat; 5] = [Stat::Average, Stat::Maximum, Stat::Sum, Stat::Minimum, Stat::P99];

    fn api(self) -> &'static str {
        match self {
            Stat::Average => "Average",
            Stat::Sum => "Sum",
            Stat::Maximum => "Maximum",
            Stat::Minimum => "Minimum",
            Stat::P99 => "p99",
        }
    }

    pub fn short(self) -> &'static str {
        match self {
            Stat::Average => "avg",
            Stat::Sum => "sum",
            Stat::Maximum => "max",
            Stat::Minimum => "min",
            Stat::P99 => "p99",
        }
    }

    pub fn next(self) -> Stat {
        let i = Self::ALL.iter().position(|s| *s == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }
}

/// How a value reads. CloudWatch's `GetMetricData` returns no units, so the
/// spec states them; a metric picked from `a` is `Raw`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    Percent,
    Bytes,
    BytesPerSec,
    /// Published in MiB (OpenSearch's free storage); reads like `Bytes`.
    Megabytes,
    /// Decimal bits: `1.2 Gbps`, as network links are sold.
    BitsPerSec,
    Count,
    PerSec,
    Millis,
    Seconds,
    Raw,
}

/// What a series means, when its colour should say so: a block is bad
/// whatever the palette's order. Mapped to theme colours by the renderer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Good,
    Bad,
    Warn,
}

/// One curated metric of a kind.
#[derive(Debug, Clone, Copy)]
pub struct MetricSpec {
    pub metric: &'static str,
    pub label: &'static str,
    pub stat: Stat,
    pub unit: Unit,
    /// Divide each point by the period: a `Sum` of bytes becomes bytes/s.
    pub rate: bool,
    /// Metrics sharing a group draw on one chart when either is selected
    /// (in beside out), each still its own row.
    pub group: Option<&'static str>,
    /// A namespace other than the target's, for a set that spans two (an
    /// ECS service's task counts live in Container Insights).
    pub namespace: Option<&'static str>,
    /// Published only for a period that has a non-zero value (WAF's
    /// counts): a missing period is a zero, and is charted as one.
    pub sparse: bool,
    pub tone: Option<Tone>,
    /// Metric math over the set's other metrics in place of a metric of
    /// its own: `{MetricName}` stands for that metric's series.
    pub expr: Option<&'static str>,
}

const fn m(metric: &'static str, label: &'static str, stat: Stat, unit: Unit) -> MetricSpec {
    MetricSpec { metric, label, stat, unit, rate: false, group: None, namespace: None, sparse: false, tone: None, expr: None }
}

/// A row computed from the set's other rows (see [`MetricSpec::expr`]).
const fn e(label: &'static str, expr: &'static str, unit: Unit) -> MetricSpec {
    MetricSpec { expr: Some(expr), ..m("", label, Stat::Sum, unit) }
}

impl MetricSpec {
    const fn rate(self) -> Self {
        MetricSpec { rate: true, ..self }
    }

    const fn group(self, group: &'static str) -> Self {
        MetricSpec { group: Some(group), ..self }
    }

    const fn ns(self, namespace: &'static str) -> Self {
        MetricSpec { namespace: Some(namespace), ..self }
    }

    const fn sparse(self) -> Self {
        MetricSpec { sparse: true, ..self }
    }

    const fn tone(self, tone: Tone) -> Self {
        MetricSpec { tone: Some(tone), ..self }
    }
}

use Stat::{Average as Avg, Maximum as Max, Minimum as Min, Sum};

pub const EC2_INSTANCE: &[MetricSpec] = &[
    m("CPUUtilization", "CPU", Avg, Unit::Percent),
    m("NetworkIn", "Network in", Sum, Unit::BytesPerSec).rate().group("net"),
    m("NetworkOut", "Network out", Sum, Unit::BytesPerSec).rate().group("net"),
    m("StatusCheckFailed", "Status check failed", Max, Unit::Count),
];

pub const LAMBDA_FUNCTION: &[MetricSpec] = &[
    m("Invocations", "Invocations", Sum, Unit::Count).group("calls"),
    m("Errors", "Errors", Sum, Unit::Count).group("calls"),
    m("Throttles", "Throttles", Sum, Unit::Count).group("calls"),
    m("Duration", "Duration", Avg, Unit::Millis),
    m("ConcurrentExecutions", "Concurrency", Max, Unit::Count),
];

pub const RDS_DATABASE: &[MetricSpec] = &[
    m("CPUUtilization", "CPU", Avg, Unit::Percent),
    m("DatabaseConnections", "Connections", Avg, Unit::Count),
    m("ReadIOPS", "Read IOPS", Avg, Unit::PerSec).group("iops"),
    m("WriteIOPS", "Write IOPS", Avg, Unit::PerSec).group("iops"),
    m("ReadLatency", "Read latency", Avg, Unit::Seconds).group("latency"),
    m("WriteLatency", "Write latency", Avg, Unit::Seconds).group("latency"),
    m("FreeableMemory", "Freeable memory", Avg, Unit::Bytes),
    m("FreeStorageSpace", "Free storage", Avg, Unit::Bytes),
];

pub const EBS_VOLUME: &[MetricSpec] = &[
    m("VolumeReadOps", "Read IOPS", Sum, Unit::PerSec).rate().group("iops"),
    m("VolumeWriteOps", "Write IOPS", Sum, Unit::PerSec).rate().group("iops"),
    m("VolumeReadBytes", "Read throughput", Sum, Unit::BytesPerSec).rate().group("bytes"),
    m("VolumeWriteBytes", "Write throughput", Sum, Unit::BytesPerSec).rate().group("bytes"),
    m("VolumeQueueLength", "Queue length", Avg, Unit::Raw),
    m("BurstBalance", "Burst balance", Avg, Unit::Percent),
];

pub const DDB_TABLE: &[MetricSpec] = &[
    m("ConsumedReadCapacityUnits", "Read capacity", Sum, Unit::Count).group("capacity"),
    m("ConsumedWriteCapacityUnits", "Write capacity", Sum, Unit::Count).group("capacity"),
    m("ReadThrottleEvents", "Read throttles", Sum, Unit::Count).group("throttles"),
    m("WriteThrottleEvents", "Write throttles", Sum, Unit::Count).group("throttles"),
];

pub const EFS_FILE_SYSTEM: &[MetricSpec] = &[
    m("DataReadIOBytes", "Read", Sum, Unit::BytesPerSec).rate().group("io"),
    m("DataWriteIOBytes", "Write", Sum, Unit::BytesPerSec).rate().group("io"),
    m("TotalIOBytes", "Total I/O", Sum, Unit::BytesPerSec).rate(),
    m("PercentIOLimit", "I/O limit", Max, Unit::Percent),
    m("BurstCreditBalance", "Burst credits", Avg, Unit::Bytes),
    m("ClientConnections", "Client connections", Sum, Unit::Count),
];

pub const ELASTICACHE_NODE: &[MetricSpec] = &[
    m("CPUUtilization", "CPU", Avg, Unit::Percent).group("cpu"),
    m("EngineCPUUtilization", "Engine CPU", Avg, Unit::Percent).group("cpu"),
    m("DatabaseMemoryUsagePercentage", "Memory used", Avg, Unit::Percent),
    m("CurrConnections", "Connections", Avg, Unit::Count),
    m("CacheHits", "Cache hits", Sum, Unit::Count),
    m("Evictions", "Evictions", Sum, Unit::Count),
];

pub const KINESIS_STREAM: &[MetricSpec] = &[
    m("IncomingRecords", "Incoming records", Sum, Unit::Count),
    m("IncomingBytes", "Incoming", Sum, Unit::BytesPerSec).rate().group("bytes"),
    m("GetRecords.Bytes", "Read", Sum, Unit::BytesPerSec).rate().group("bytes"),
    m("GetRecords.IteratorAgeMilliseconds", "Iterator age", Max, Unit::Millis),
    m("ReadProvisionedThroughputExceeded", "Read throttled", Sum, Unit::Count).group("throttled"),
    m("WriteProvisionedThroughputExceeded", "Write throttled", Sum, Unit::Count).group("throttled"),
];

pub const FIREHOSE_STREAM: &[MetricSpec] = &[
    m("IncomingRecords", "Incoming records", Sum, Unit::Count).group("records"),
    m("DeliveryToS3.Records", "Delivered to S3", Sum, Unit::Count).group("records"),
    m("IncomingBytes", "Incoming", Sum, Unit::BytesPerSec).rate(),
    m("DeliveryToS3.Success", "S3 success (ratio)", Avg, Unit::Raw),
    m("DeliveryToS3.DataFreshness", "Data freshness", Max, Unit::Seconds),
    m("ThrottledRecords", "Throttled", Sum, Unit::Count),
];

pub const REDSHIFT_CLUSTER: &[MetricSpec] = &[
    m("CPUUtilization", "CPU", Avg, Unit::Percent),
    m("PercentageDiskSpaceUsed", "Disk used", Avg, Unit::Percent),
    m("DatabaseConnections", "Connections", Avg, Unit::Count),
    m("HealthStatus", "Health (1 = healthy)", Min, Unit::Raw),
    m("ReadIOPS", "Read IOPS", Avg, Unit::PerSec).group("iops"),
    m("WriteIOPS", "Write IOPS", Avg, Unit::PerSec).group("iops"),
];

pub const REDSHIFT_WORKGROUP: &[MetricSpec] = &[
    m("ComputeCapacity", "Compute (RPU)", Avg, Unit::Raw),
    m("ComputeSeconds", "Compute (RPU-seconds)", Sum, Unit::Raw),
    m("DatabaseConnections", "Connections", Avg, Unit::Count),
    m("QueriesCompletedPerSecond", "Queries completed", Avg, Unit::PerSec),
    m("QueriesRunning", "Queries running", Avg, Unit::Count).group("queries"),
    m("QueriesQueued", "Queries queued", Avg, Unit::Count).group("queries"),
];

pub const SQS_QUEUE: &[MetricSpec] = &[
    m("NumberOfMessagesSent", "Sent", Sum, Unit::Count).group("messages"),
    m("NumberOfMessagesReceived", "Received", Sum, Unit::Count).group("messages"),
    m("NumberOfMessagesDeleted", "Deleted", Sum, Unit::Count).group("messages"),
    m("ApproximateNumberOfMessagesVisible", "Visible", Avg, Unit::Count),
    m("ApproximateAgeOfOldestMessage", "Oldest message", Max, Unit::Seconds),
    m("NumberOfEmptyReceives", "Empty receives", Sum, Unit::Count),
];

pub const SNS_TOPIC: &[MetricSpec] = &[
    m("NumberOfMessagesPublished", "Published", Sum, Unit::Count),
    m("NumberOfNotificationsDelivered", "Delivered", Sum, Unit::Count).group("notifications"),
    m("NumberOfNotificationsFailed", "Failed", Sum, Unit::Count).group("notifications"),
    m("NumberOfNotificationsFilteredOut", "Filtered out", Sum, Unit::Count),
    m("NumberOfNotificationsRedrivenToDlq", "Redriven to DLQ", Sum, Unit::Count),
    m("PublishSize", "Publish size", Avg, Unit::Bytes),
];

pub const SFN_STATE_MACHINE: &[MetricSpec] = &[
    m("ExecutionsStarted", "Started", Sum, Unit::Count).group("executions"),
    m("ExecutionsSucceeded", "Succeeded", Sum, Unit::Count).group("executions"),
    m("ExecutionsFailed", "Failed", Sum, Unit::Count).group("executions"),
    m("ExecutionsAborted", "Aborted", Sum, Unit::Count).group("executions"),
    m("ExecutionsTimedOut", "Timed out", Sum, Unit::Count).group("executions"),
    m("ExecutionTime", "Execution time", Avg, Unit::Millis),
];

pub const BEDROCK_MODEL: &[MetricSpec] = &[
    m("Invocations", "Invocations", Sum, Unit::Count),
    m("InvocationLatency", "Latency", Avg, Unit::Millis),
    m("InputTokenCount", "Input tokens", Sum, Unit::Count).group("tokens"),
    m("OutputTokenCount", "Output tokens", Sum, Unit::Count).group("tokens"),
    m("InvocationClientErrors", "Client errors", Sum, Unit::Count).group("errors"),
    m("InvocationServerErrors", "Server errors", Sum, Unit::Count).group("errors"),
    m("InvocationThrottles", "Throttles", Sum, Unit::Count),
];

pub const TRANSFER_SERVER: &[MetricSpec] = &[
    m("BytesIn", "Bytes in", Sum, Unit::BytesPerSec).rate().group("bytes"),
    m("BytesOut", "Bytes out", Sum, Unit::BytesPerSec).rate().group("bytes"),
    m("FilesIn", "Files in", Sum, Unit::Count).group("files"),
    m("FilesOut", "Files out", Sum, Unit::Count).group("files"),
    m("OnUploadExecutionsSuccess", "Upload workflows ok", Sum, Unit::Count).group("workflows"),
    m("OnUploadExecutionsFailed", "Upload workflows failed", Sum, Unit::Count).group("workflows"),
];

pub const NAT_GATEWAY: &[MetricSpec] = &[
    m("BytesOutToDestination", "Out to destination", Sum, Unit::BytesPerSec).rate().group("bytes"),
    m("BytesInFromDestination", "In from destination", Sum, Unit::BytesPerSec).rate().group("bytes"),
    m("ActiveConnectionCount", "Active connections", Max, Unit::Count),
    m("ConnectionEstablishedCount", "New connections", Sum, Unit::Count),
    m("ErrorPortAllocation", "Port allocation errors", Sum, Unit::Count),
    m("PacketsDropCount", "Packets dropped", Sum, Unit::Count),
];

/// `VpnId` alone aggregates both tunnels.
pub const VPN_CONNECTION: &[MetricSpec] = &[
    m("TunnelState", "Tunnels up (1 = both)", Avg, Unit::Raw),
    m("TunnelDataIn", "Data in", Sum, Unit::BytesPerSec).rate().group("data"),
    m("TunnelDataOut", "Data out", Sum, Unit::BytesPerSec).rate().group("data"),
];

pub const DX_CONNECTION: &[MetricSpec] = &[
    m("ConnectionState", "State (1 = up)", Min, Unit::Raw),
    m("ConnectionBpsEgress", "Egress", Avg, Unit::BitsPerSec).group("bps"),
    m("ConnectionBpsIngress", "Ingress", Avg, Unit::BitsPerSec).group("bps"),
    m("ConnectionPpsEgress", "Egress packets", Avg, Unit::PerSec).group("pps"),
    m("ConnectionPpsIngress", "Ingress packets", Avg, Unit::PerSec).group("pps"),
    m("ConnectionErrorCount", "Errors", Sum, Unit::Count),
    m("ConnectionLightLevelTx", "Light Tx (dBm)", Avg, Unit::Raw).group("light"),
    m("ConnectionLightLevelRx", "Light Rx (dBm)", Avg, Unit::Raw).group("light"),
];

/// Per-VIF traffic: the per-workload signal on a shared connection.
pub const DX_VIRTUAL_INTERFACE: &[MetricSpec] = &[
    m("VirtualInterfaceBpsEgress", "Egress", Avg, Unit::BitsPerSec).group("bps"),
    m("VirtualInterfaceBpsIngress", "Ingress", Avg, Unit::BitsPerSec).group("bps"),
    m("VirtualInterfacePpsEgress", "Egress packets", Avg, Unit::PerSec).group("pps"),
    m("VirtualInterfacePpsIngress", "Ingress packets", Avg, Unit::PerSec).group("pps"),
];

/// The namespace's only metric.
pub const ECR_REPOSITORY: &[MetricSpec] = &[m("RepositoryPullCount", "Pulls", Sum, Unit::Count)];

pub const WORKSPACE: &[MetricSpec] = &[
    m("ConnectionSuccess", "Connections ok", Sum, Unit::Count).group("connections"),
    m("ConnectionFailure", "Connections failed", Sum, Unit::Count).group("connections"),
    m("UserConnected", "User connected", Max, Unit::Count),
    m("InSessionLatency", "In-session latency", Avg, Unit::Millis),
    m("SessionLaunchTime", "Session launch", Avg, Unit::Seconds),
    m("Unhealthy", "Unhealthy", Max, Unit::Count),
];

pub const ATHENA_WORKGROUP: &[MetricSpec] = &[
    m("ProcessedBytes", "Data scanned", Sum, Unit::Bytes),
    m("TotalExecutionTime", "Total time", Avg, Unit::Millis),
    m("EngineExecutionTime", "Engine", Avg, Unit::Millis).group("phases"),
    m("QueryQueueTime", "Queued", Avg, Unit::Millis).group("phases"),
    m("QueryPlanningTime", "Planning", Avg, Unit::Millis).group("phases"),
    m("DPUConsumed", "DPU consumed", Sum, Unit::Raw),
];

/// Driver aggregates; Maximum, since each is a running total per run.
pub const GLUE_JOB: &[MetricSpec] = &[
    m("glue.driver.aggregate.bytesRead", "Bytes read", Max, Unit::Bytes),
    m("glue.driver.aggregate.recordsRead", "Records read", Max, Unit::Count),
    m("glue.driver.aggregate.elapsedTime", "Elapsed", Max, Unit::Millis),
    m("glue.driver.aggregate.numCompletedTasks", "Completed tasks", Max, Unit::Count).group("tasks"),
    m("glue.driver.aggregate.numFailedTasks", "Failed tasks", Max, Unit::Count).group("tasks"),
    m("glue.driver.aggregate.numCompletedStages", "Completed stages", Max, Unit::Count),
];

pub const TRANSIT_GATEWAY: &[MetricSpec] = &[
    m("BytesIn", "Bytes in", Sum, Unit::BytesPerSec).rate().group("bytes"),
    m("BytesOut", "Bytes out", Sum, Unit::BytesPerSec).rate().group("bytes"),
    m("PacketsIn", "Packets in", Sum, Unit::Count).group("packets"),
    m("PacketsOut", "Packets out", Sum, Unit::Count).group("packets"),
    m("PacketDropCountBlackhole", "Dropped (blackhole)", Sum, Unit::Count).group("drops"),
    m("PacketDropCountNoRoute", "Dropped (no route)", Sum, Unit::Count).group("drops"),
];

/// "Is this group receiving anything?"
pub const LOG_GROUP: &[MetricSpec] = &[
    m("IncomingLogEvents", "Incoming events", Sum, Unit::Count),
    m("IncomingBytes", "Incoming", Sum, Unit::BytesPerSec).rate(),
];

pub const EB_RULE: &[MetricSpec] = &[
    m("TriggeredRules", "Matched", Sum, Unit::Count).group("calls"),
    m("Invocations", "Invoked", Sum, Unit::Count).group("calls"),
    m("FailedInvocations", "Failed", Sum, Unit::Count).group("calls"),
    m("DeadLetterInvocations", "To DLQ", Sum, Unit::Count),
    m("ThrottledRules", "Throttled", Sum, Unit::Count),
];

/// Both directions are fetched; the endpoint's own is the one with data.
pub const RESOLVER_ENDPOINT: &[MetricSpec] = &[
    m("InboundQueryVolume", "Inbound queries", Sum, Unit::Count).group("queries"),
    m("OutboundQueryVolume", "Outbound queries", Sum, Unit::Count).group("queries"),
];

pub const CODEBUILD_PROJECT: &[MetricSpec] = &[
    m("Builds", "Builds", Sum, Unit::Count).group("builds"),
    m("SucceededBuilds", "Succeeded", Sum, Unit::Count).group("builds"),
    m("FailedBuilds", "Failed", Sum, Unit::Count).group("builds"),
    m("Duration", "Duration", Avg, Unit::Seconds),
];

/// Cache hit rate and origin latency need additional metrics switched on.
pub const CLOUDFRONT_DISTRIBUTION: &[MetricSpec] = &[
    m("Requests", "Requests", Sum, Unit::Count),
    m("BytesDownloaded", "Downloaded", Sum, Unit::BytesPerSec).rate().group("bytes"),
    m("BytesUploaded", "Uploaded", Sum, Unit::BytesPerSec).rate().group("bytes"),
    m("TotalErrorRate", "Error rate", Avg, Unit::Percent).group("errors"),
    m("4xxErrorRate", "4xx rate", Avg, Unit::Percent).group("errors"),
    m("5xxErrorRate", "5xx rate", Avg, Unit::Percent).group("errors"),
    m("CacheHitRate", "Cache hit rate", Avg, Unit::Percent),
    m("OriginLatency", "Origin latency", Avg, Unit::Millis),
];

/// `WebACL` + `Rule=ALL` (+ `Region` for REGIONAL ACLs): every action on
/// one chart, so the split between them reads at a glance, then the share
/// blocked. WAF publishes a minute only when it had requests, hence
/// `sparse` and the `FILL`s. Count doesn't end a request, so it isn't part
/// of the rate's denominator (`WafTraffic::requests` says the same).
pub const WAF_WEB_ACL: &[MetricSpec] = &[
    m("AllowedRequests", "Allowed", Sum, Unit::Count).group("actions").sparse().tone(Tone::Good),
    m("BlockedRequests", "Blocked", Sum, Unit::Count).group("actions").sparse().tone(Tone::Bad),
    m("CountedRequests", "Counted", Sum, Unit::Count).group("actions").sparse().tone(Tone::Warn),
    m("CaptchaRequests", "CAPTCHA", Sum, Unit::Count).group("actions").sparse(),
    m("ChallengeRequests", "Challenge", Sum, Unit::Count).group("actions").sparse(),
    e(
        "Block rate",
        "IF(FILL({AllowedRequests},0)+FILL({BlockedRequests},0)+FILL({CaptchaRequests},0)+FILL({ChallengeRequests},0) > 0, \
         100*FILL({BlockedRequests},0)/(FILL({AllowedRequests},0)+FILL({BlockedRequests},0)+FILL({CaptchaRequests},0)+FILL({ChallengeRequests},0)), 0)",
        Unit::Percent,
    )
    .tone(Tone::Bad),
];

/// One rule's or label's actions over time (`⏎` on a breakdown row): the
/// web ACL's actions, without the rate (a rule's own block rate is 100% or
/// nothing).
pub const WAF_RULE: &[MetricSpec] = WAF_WEB_ACL.split_at(5).0;

pub const R53_HEALTH_CHECK: &[MetricSpec] = &[
    m("HealthCheckStatus", "Status (1 = healthy)", Min, Unit::Raw),
    m("HealthCheckPercentageHealthy", "Checkers healthy", Avg, Unit::Percent),
    m("ConnectionTime", "Connect", Avg, Unit::Millis).group("timing"),
    m("TimeToFirstByte", "First byte", Avg, Unit::Millis).group("timing"),
];

/// Public zones only: private zones publish nothing.
pub const R53_HOSTED_ZONE: &[MetricSpec] = &[m("DNSQueries", "DNS queries", Sum, Unit::Count)];

pub const GA_ACCELERATOR: &[MetricSpec] = &[
    m("NewFlowCount", "New flows", Sum, Unit::Count),
    m("ProcessedBytesIn", "Bytes in", Sum, Unit::BytesPerSec).rate().group("bytes"),
    m("ProcessedBytesOut", "Bytes out", Sum, Unit::BytesPerSec).rate().group("bytes"),
];

/// CPU and memory from `AWS/ECS` (no Container Insights needed); the task
/// counts only exist in Container Insights.
pub const ECS_SERVICE: &[MetricSpec] = &[
    m("CPUUtilization", "CPU", Avg, Unit::Percent),
    m("MemoryUtilization", "Memory", Avg, Unit::Percent),
    m("RunningTaskCount", "Running tasks", Avg, Unit::Count).group("tasks").ns("ECS/ContainerInsights"),
    m("PendingTaskCount", "Pending tasks", Avg, Unit::Count).group("tasks").ns("ECS/ContainerInsights"),
];

/// Task level; each container's CPU and memory follow as runtime rows.
pub const ECS_TASK: &[MetricSpec] = &[
    m("TaskCpuUtilization", "CPU", Avg, Unit::Percent),
    m("TaskMemoryUtilization", "Memory", Avg, Unit::Percent),
    m("NetworkRxBytes", "Network in", Avg, Unit::BytesPerSec).group("net"),
    m("NetworkTxBytes", "Network out", Avg, Unit::BytesPerSec).group("net"),
    m("StorageReadBytes", "Storage read", Avg, Unit::Bytes).group("storage"),
    m("StorageWriteBytes", "Storage write", Avg, Unit::Bytes).group("storage"),
];

pub const ASG_GROUP: &[MetricSpec] = &[
    m("GroupDesiredCapacity", "Desired", Avg, Unit::Count).group("capacity"),
    m("GroupInServiceInstances", "In service", Avg, Unit::Count).group("capacity"),
    m("GroupTotalInstances", "Total", Avg, Unit::Count).group("capacity"),
    m("GroupPendingInstances", "Pending", Avg, Unit::Count).group("capacity"),
];

pub const EKS_CLUSTER: &[MetricSpec] = &[
    m("cluster_node_count", "Nodes", Avg, Unit::Count).group("nodes"),
    m("cluster_failed_node_count", "Failed nodes", Max, Unit::Count).group("nodes"),
    m("cluster_running_pod_count", "Running pods", Avg, Unit::Count),
    m("node_cpu_utilization", "Node CPU", Avg, Unit::Percent).group("usage"),
    m("node_memory_utilization", "Node memory", Avg, Unit::Percent).group("usage"),
];

/// REST APIs key on the API's *name*; HTTP and WebSocket on its id.
pub const API_REST: &[MetricSpec] = &[
    m("Count", "Requests", Sum, Unit::Count),
    m("4XXError", "4xx", Sum, Unit::Count).group("errors"),
    m("5XXError", "5xx", Sum, Unit::Count).group("errors"),
    m("Latency", "Latency", Avg, Unit::Millis).group("latency"),
    m("IntegrationLatency", "Integration latency", Avg, Unit::Millis).group("latency"),
    m("CacheHitCount", "Cache hits", Sum, Unit::Count).group("cache"),
    m("CacheMissCount", "Cache misses", Sum, Unit::Count).group("cache"),
];

/// HTTP APIs use lowercase `4xx` / `5xx`.
pub const API_HTTP: &[MetricSpec] = &[
    m("Count", "Requests", Sum, Unit::Count),
    m("4xx", "4xx", Sum, Unit::Count).group("errors"),
    m("5xx", "5xx", Sum, Unit::Count).group("errors"),
    m("Latency", "Latency", Avg, Unit::Millis).group("latency"),
    m("IntegrationLatency", "Integration latency", Avg, Unit::Millis).group("latency"),
    m("DataProcessed", "Data processed", Sum, Unit::Bytes),
];

/// WebSocket APIs publish no `Latency`.
pub const API_WEBSOCKET: &[MetricSpec] = &[
    m("ConnectCount", "Connects", Sum, Unit::Count),
    m("MessageCount", "Messages", Sum, Unit::Count),
    m("ClientError", "Client errors", Sum, Unit::Count).group("errors"),
    m("ExecutionError", "Execution errors", Sum, Unit::Count).group("errors"),
    m("IntegrationError", "Integration errors", Sum, Unit::Count).group("errors"),
    m("IntegrationLatency", "Integration latency", Avg, Unit::Millis),
];

pub const ALB: &[MetricSpec] = &[
    m("RequestCount", "Requests", Sum, Unit::Count),
    m("TargetResponseTime", "Target response time", Avg, Unit::Seconds),
    m("HTTPCode_Target_5XX_Count", "Target 5xx", Sum, Unit::Count).group("5xx"),
    m("HTTPCode_ELB_5XX_Count", "ELB 5xx", Sum, Unit::Count).group("5xx"),
    m("HTTPCode_Target_4XX_Count", "Target 4xx", Sum, Unit::Count),
    m("ActiveConnectionCount", "Active connections", Sum, Unit::Count),
];

pub const NLB: &[MetricSpec] = &[
    m("ActiveFlowCount", "Active flows", Avg, Unit::Count),
    m("NewFlowCount", "New flows", Sum, Unit::Count),
    m("ProcessedBytes", "Processed", Sum, Unit::BytesPerSec).rate(),
    m("TCP_Target_Reset_Count", "Target resets", Sum, Unit::Count),
];

/// A gateway load balancer publishes no TCP resets.
pub const GWLB: &[MetricSpec] = &[
    m("ActiveFlowCount", "Active flows", Avg, Unit::Count),
    m("NewFlowCount", "New flows", Sum, Unit::Count),
    m("ProcessedBytes", "Processed", Sum, Unit::BytesPerSec).rate(),
];

pub const ALB_TARGET_GROUP: &[MetricSpec] = &[
    m("HealthyHostCount", "Healthy hosts", Avg, Unit::Count).group("hosts"),
    m("UnHealthyHostCount", "Unhealthy hosts", Max, Unit::Count).group("hosts"),
    m("RequestCountPerTarget", "Requests per target", Sum, Unit::Count),
    m("TargetResponseTime", "Target response time", Avg, Unit::Seconds),
];

pub const TARGET_GROUP: &[MetricSpec] = &[
    m("HealthyHostCount", "Healthy hosts", Avg, Unit::Count).group("hosts"),
    m("UnHealthyHostCount", "Unhealthy hosts", Max, Unit::Count).group("hosts"),
];

/// Free storage is published in MiB, and its Minimum is the node closest to full.
pub const OPENSEARCH_DOMAIN: &[MetricSpec] = &[
    m("CPUUtilization", "CPU", Avg, Unit::Percent),
    m("JVMMemoryPressure", "JVM memory pressure", Max, Unit::Percent),
    m("FreeStorageSpace", "Free storage", Min, Unit::Megabytes),
    m("ClusterStatus.red", "Status red (1 = red)", Max, Unit::Raw),
    m("SearchRate", "Search rate (/min)", Avg, Unit::Raw).group("rates"),
    m("IndexingRate", "Indexing rate (/min)", Avg, Unit::Raw).group("rates"),
];

pub const DMS_INSTANCE: &[MetricSpec] = &[
    m("CPUUtilization", "CPU", Avg, Unit::Percent),
    m("FreeableMemory", "Freeable memory", Avg, Unit::Bytes),
    m("FreeStorageSpace", "Free storage", Avg, Unit::Bytes),
    m("SwapUsage", "Swap", Avg, Unit::Bytes),
];

pub const DMS_TASK: &[MetricSpec] = &[
    m("CDCLatencySource", "Source latency", Avg, Unit::Seconds).group("latency"),
    m("CDCLatencyTarget", "Target latency", Avg, Unit::Seconds).group("latency"),
    m("CDCIncomingChanges", "Incoming changes", Max, Unit::Count),
    m("FullLoadThroughputRowsTarget", "Full-load rows", Avg, Unit::PerSec),
];

/// What `m` charts for one resource: where its metrics live and its curated set.
#[derive(Debug, Clone)]
pub struct MetricTarget {
    /// `EC2 instance · web-1`, the view's title.
    pub title: String,
    pub namespace: String,
    pub dimensions: Vec<(String, String)>,
    pub specs: &'static [MetricSpec],
    /// The region CloudWatch publishes these in when it isn't the current
    /// one: `us-east-1` for global services (CloudFront, Route 53),
    /// `us-west-2` for Global Accelerator.
    pub home: Option<&'static str>,
    /// Said in place of the rows when every metric came back empty: what
    /// has to be switched on for them to exist (Glue job metrics, …).
    pub hint: Option<&'static str>,
    /// Queries only known at runtime, after the spec rows: one per container
    /// of an ECS task, or the whole set for a CloudWatch metric row.
    pub extra: Vec<MetricQuery>,
    /// Range totals per dimension value, drawn beside the chart.
    pub breakdown: Option<Breakdown>,
}

/// A kind's per-dimension totals, fetched with the series. Only WAF has one
/// so far: per-rule and per-label SEARCH totals, whose `WebACL` dimension
/// (the ACL's metric name) is looked up once, before the first fetch.
#[derive(Debug, Clone)]
pub enum Breakdown {
    Waf {
        client: aws_sdk_wafv2::Client,
        scope: crate::aws::services::waf::WafScope,
        name: String,
        id: String,
        /// `None` for CLOUDFRONT-scope ACLs, which publish without one.
        region: Option<String>,
        /// The `WebACL` value once looked up.
        metric_name: Option<String>,
    },
}

/// What a breakdown fetch brings back besides its tables.
#[derive(Debug, Clone, Default)]
pub struct BreakdownFix {
    /// Dimensions to set on the target and its queries: `(name, value)`.
    pub dimensions: Vec<(String, String)>,
    /// Said in place of the rows when nothing can publish (metrics off).
    pub hint: Option<&'static str>,
}

impl Breakdown {
    /// Look up what the series need before they can be fetched, and patch
    /// `queries` with it. Done once: the result is kept on the target.
    pub async fn prepare(&self, cw: &aws_sdk_cloudwatch::Client, queries: &mut [MetricQuery]) -> Option<BreakdownFix> {
        match self {
            Breakdown::Waf { metric_name: Some(_), .. } => None,
            Breakdown::Waf { client, scope, name, id, region, .. } => {
                let (metric, enabled) =
                    crate::aws::services::waf::resolve_metric_name(client, cw, *scope, name, id, region.as_deref())
                        .await;
                let fix = BreakdownFix {
                    dimensions: vec![("WebACL".to_string(), metric)],
                    hint: (!enabled).then_some("CloudWatch metrics are off in this web ACL's visibility config"),
                };
                for q in queries.iter_mut() {
                    fix.apply_to(&mut q.dimensions);
                }
                Some(fix)
            }
        }
    }

    /// Record a `prepare` result so the next fetch skips it.
    pub fn settle(&mut self, fix: &BreakdownFix) {
        match self {
            Breakdown::Waf { metric_name, .. } => {
                *metric_name = fix.dimensions.iter().find(|(n, _)| n == "WebACL").map(|(_, v)| v.clone());
            }
        }
    }

    pub async fn fetch(
        &self,
        cw: &aws_sdk_cloudwatch::Client,
        dimensions: &[(String, String)],
        range: MetricsTimeRange,
        end: i64,
    ) -> Result<BreakdownData, String> {
        match self {
            Breakdown::Waf { region, .. } => {
                let web_acl = dimensions.iter().find(|(n, _)| n == "WebACL").map_or("", |(_, v)| v.as_str());
                let start = end - range.duration_secs();
                let traffic = crate::aws::services::waf::fetch_traffic(
                    cw,
                    web_acl,
                    region.as_deref(),
                    start,
                    end,
                    range.period_secs(),
                )
                .await?;
                // A row charts against the ACL's own WebACL and Region.
                let base: Vec<(String, String)> =
                    dimensions.iter().filter(|(n, _)| n == "WebACL" || n == "Region").cloned().collect();
                Ok(BreakdownData { summary: traffic.summary(), tables: traffic.tables(&base) })
            }
        }
    }
}

impl BreakdownFix {
    pub fn apply_to(&self, dims: &mut [(String, String)]) {
        for (name, value) in &self.dimensions {
            if let Some(d) = dims.iter_mut().find(|(n, _)| n == name) {
                d.1 = value.clone();
            }
        }
    }
}

/// A breakdown's answer: a one-line summary of the whole (`None` when there
/// is nothing to sum), then its tables.
#[derive(Debug, Clone, Default)]
pub struct BreakdownData {
    pub summary: Option<String>,
    pub tables: Vec<BreakdownTable>,
}

/// Range totals per dimension value: one row per value, one column per
/// metric, rows sorted by their total.
#[derive(Debug, Clone)]
pub struct BreakdownTable {
    pub title: String,
    pub columns: &'static [&'static str],
    pub rows: Vec<(String, Vec<f64>)>,
    /// How `⏎` charts a row over time, when it can.
    pub drill: Option<Drill>,
}

/// A breakdown row's own series: `specs` against that row's dimensions.
#[derive(Debug, Clone)]
pub struct Drill {
    pub namespace: &'static str,
    pub specs: &'static [MetricSpec],
    /// One set per row, in `rows` order.
    pub dimensions: Vec<Vec<(String, String)>>,
}

impl Drill {
    pub fn queries(&self, row: usize) -> Option<Vec<MetricQuery>> {
        let dims = self.dimensions.get(row)?;
        Some(self.specs.iter().map(|s| MetricQuery::from_spec(s, self.namespace, dims.clone())).collect())
    }
}

impl BreakdownTable {
    /// The columns with any traffic, as indexes: an ACL with no CAPTCHA
    /// rules shows no CAPTCHA column.
    fn live_columns(&self) -> Vec<usize> {
        (0..self.columns.len()).filter(|&c| self.rows.iter().any(|(_, v)| v.get(c).is_some_and(|x| *x > 0.0))).collect()
    }

    /// The header and up to `max_rows` rows as text in `width` columns:
    /// name, then `Total`, then each live column. Columns go from the right
    /// before the name gets narrower than 16.
    pub fn text(&self, width: usize, max_rows: usize) -> (String, Vec<String>) {
        self.text_window(width, 0, max_rows)
    }

    /// As [`text`](Self::text), from row `skip` on: the panel scrolled to
    /// its cursor. The layout is the whole table's, so it holds still.
    pub fn text_window(&self, width: usize, skip: usize, max_rows: usize) -> (String, Vec<String>) {
        self.text_capped(width, skip, max_rows, 40)
    }

    /// Every row with every live column and each name in full: the `e`
    /// view, where names are copied out to the console or Terraform.
    pub fn full_text(&self) -> (String, Vec<String>) {
        self.text_capped(usize::MAX, 0, usize::MAX, usize::MAX)
    }

    fn text_capped(&self, width: usize, skip: usize, max_rows: usize, name_cap: usize) -> (String, Vec<String>) {
        const NUM_W: usize = 8;
        const NAME_MIN: usize = 16;
        let mut cols = self.live_columns();
        // One leading space, the name, then a space and a number per column
        // (Total included).
        let name_w = |n: usize| width.saturating_sub(1 + (n + 1) * (NUM_W + 1));
        while !cols.is_empty() && name_w(cols.len()) < NAME_MIN {
            cols.pop();
        }
        // When the columns all fit, the name takes what it needs, up to the cap.
        let longest = self.rows.iter().map(|(n, _)| n.chars().count()).max().unwrap_or(0).max(4);
        let nw = name_w(cols.len()).min(longest.max(NAME_MIN)).min(name_cap);
        // The end of a name is what tells rows apart (`…CommonRuleSet`,
        // `…bot:category:http_library`), so a long one keeps its end.
        let fit = |s: &str| -> String {
            let n = s.chars().count();
            if n > nw {
                format!("…{}", s.chars().skip(n + 1 - nw).collect::<String>())
            } else {
                s.to_string()
            }
        };
        let mut header = format!(" {:<nw$} {:>NUM_W$}", "", "Total");
        for &c in &cols {
            header.push_str(&format!(" {:>NUM_W$}", self.columns[c]));
        }
        let rows = self
            .rows
            .iter()
            .skip(skip)
            .take(max_rows)
            .map(|(name, vals)| {
                let total: f64 = vals.iter().sum();
                let mut line = format!(" {:<nw$} {:>NUM_W$}", fit(name), fmt_count(total));
                for &c in &cols {
                    let v = vals.get(c).copied().unwrap_or(0.0);
                    let cell = if v > 0.0 { fmt_count(v) } else { "—".to_string() };
                    line.push_str(&format!(" {cell:>NUM_W$}"));
                }
                line
            })
            .collect();
        (header, rows)
    }
}

/// `1.2K`, `3.4M`: a range total in a table cell.
pub fn fmt_count(v: f64) -> String {
    if v >= 1_000_000.0 {
        format!("{:.1}M", v / 1_000_000.0)
    } else if v >= 1_000.0 {
        format!("{:.1}K", v / 1_000.0)
    } else {
        format!("{}", v as u64)
    }
}

impl MetricTarget {
    pub fn curated_queries(&self) -> Vec<MetricQuery> {
        self.specs
            .iter()
            .map(|s| MetricQuery::from_spec(s, &self.namespace, self.dimensions.clone()))
            .chain(self.extra.iter().cloned())
            .collect()
    }
}

/// One series to fetch: a curated spec resolved against a resource, or a
/// metric picked from `a`.
#[derive(Debug, Clone, PartialEq)]
pub struct MetricQuery {
    pub namespace: String,
    pub metric: String,
    pub dimensions: Vec<(String, String)>,
    pub label: String,
    pub stat: Stat,
    pub unit: Unit,
    pub rate: bool,
    pub group: Option<String>,
    /// See [`MetricSpec::sparse`].
    pub sparse: bool,
    pub tone: Option<Tone>,
    /// See [`MetricSpec::expr`]: metric math, `{MetricName}` placeholders
    /// and all; `metric` is then empty.
    pub expression: Option<String>,
}

impl MetricQuery {
    /// `sum BlockedRequests`, or `expression`: what a row is, after its label.
    pub fn describe(&self) -> String {
        match &self.expression {
            Some(_) => "expression".to_string(),
            None => format!("{} {}", self.stat.short(), self.metric),
        }
    }

    /// One spec resolved against a set of dimensions.
    pub fn from_spec(s: &MetricSpec, namespace: &str, dimensions: Vec<(String, String)>) -> Self {
        MetricQuery {
            namespace: s.namespace.unwrap_or(namespace).to_string(),
            metric: s.metric.to_string(),
            dimensions,
            label: s.label.to_string(),
            stat: s.stat,
            unit: s.unit,
            rate: s.rate,
            group: s.group.map(str::to_string),
            sparse: s.sparse,
            tone: s.tone,
            expression: s.expr.map(str::to_string),
        }
    }
}

/// One metric's answer: (epoch seconds, value) points oldest first, or why not.
#[derive(Debug, Clone, PartialEq)]
pub struct MetricSeries {
    pub query: MetricQuery,
    pub points: Vec<(i64, f64)>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stats {
    pub last: f64,
    pub min: f64,
    pub max: f64,
    pub avg: f64,
}

impl MetricSeries {
    pub fn stats(&self) -> Option<Stats> {
        let last = self.points.last()?.1;
        let (mut min, mut max, mut sum) = (f64::MAX, f64::MIN, 0.0);
        for &(_, v) in &self.points {
            min = min.min(v);
            max = max.max(v);
            sum += v;
        }
        Some(Stats { last, min, max, avg: sum / self.points.len() as f64 })
    }

    /// `CPU (avg CPUUtilization)  last 41%  min 3%  max 88%  avg 27%` — the
    /// `y` copy and the `e` editor view.
    pub fn summary(&self) -> String {
        let q = &self.query;
        let head = format!("{} ({})", q.label, q.describe());
        match (&self.error, self.stats()) {
            (Some(e), _) => format!("{head}  ! {e}"),
            (None, None) => format!("{head}  no data in the window"),
            (None, Some(s)) => format!(
                "{head}  last {}  min {}  max {}  avg {}",
                fmt_value(s.last, q.unit),
                fmt_value(s.min, q.unit),
                fmt_value(s.max, q.unit),
                fmt_value(s.avg, q.unit)
            ),
        }
    }
}

/// A short, row-sized form of an AWS error.
pub fn short_error(e: &str) -> String {
    if e.contains("not authorized") || e.contains("AccessDenied") {
        return "no access (cloudwatch:GetMetricData)".to_string();
    }
    let first = e.split(". ").next().unwrap_or(e);
    if first.chars().count() > 80 {
        first.chars().take(79).chain(['…']).collect()
    } else {
        first.to_string()
    }
}

fn dims(pairs: &[(String, String)]) -> Vec<Dimension> {
    pairs.iter().map(|(n, v)| Dimension::builder().name(n).value(v).build()).collect()
}

/// Every query's series over `[end − range, end]`, in the order asked, from
/// one `GetMetricData` call (paged). A failed call fails every row with its
/// message; a query the response doesn't answer is a row with an error.
pub async fn fetch_series(
    cw: &aws_sdk_cloudwatch::Client,
    queries: &[MetricQuery],
    range: MetricsTimeRange,
    end: i64,
) -> Vec<MetricSeries> {
    let period = range.period_secs();
    let start = end - range.duration_secs();
    let mut errors: Vec<Option<String>> = vec![Some("not in the response".to_string()); queries.len()];
    let mut data_queries: Vec<MetricDataQuery> = Vec::new();
    for (i, q) in queries.iter().enumerate() {
        match &q.expression {
            None => {
                let metric = Metric::builder()
                    .namespace(&q.namespace)
                    .metric_name(&q.metric)
                    .set_dimensions(Some(dims(&q.dimensions)))
                    .build();
                let stat = MetricStat::builder().metric(metric).period(period).stat(q.stat.api()).build();
                data_queries.push(MetricDataQuery::builder().id(format!("q{i}")).metric_stat(stat).build());
            }
            // An expression naming a metric that isn't in the request would
            // fail the whole call, so it's left out and says why.
            Some(expr) => match resolve_expression(expr, queries) {
                Ok(resolved) => {
                    data_queries.push(MetricDataQuery::builder().id(format!("q{i}")).expression(resolved).build())
                }
                Err(e) => errors[i] = Some(e),
            },
        }
    }

    let mut points: Vec<Vec<(i64, f64)>> = vec![Vec::new(); queries.len()];
    let mut token: Option<String> = None;
    loop {
        let resp = cw
            .get_metric_data()
            .set_metric_data_queries(Some(data_queries.clone()))
            .start_time(CwDateTime::from_secs(start))
            .end_time(CwDateTime::from_secs(end))
            .scan_by(ScanBy::TimestampAscending)
            .set_next_token(token.clone())
            .send()
            .await;
        let resp = match resp {
            Ok(r) => r,
            Err(e) => {
                let msg = short_error(&crate::error::sdk_error_message(&e));
                return queries
                    .iter()
                    .map(|q| MetricSeries { query: q.clone(), points: Vec::new(), error: Some(msg.clone()) })
                    .collect();
            }
        };
        for r in resp.metric_data_results() {
            let Some(i) = r.id().and_then(|id| id.strip_prefix('q')).and_then(|n| n.parse::<usize>().ok()) else {
                continue;
            };
            let Some(q) = queries.get(i) else { continue };
            errors[i] = match r.status_code() {
                Some(StatusCode::InternalError) | Some(StatusCode::Forbidden) => Some(
                    r.messages()
                        .first()
                        .and_then(|m| m.value())
                        .map(short_error)
                        .unwrap_or_else(|| "CloudWatch could not read it".to_string()),
                ),
                _ => None,
            };
            let divisor = if q.rate { period as f64 } else { 1.0 };
            points[i].extend(r.timestamps().iter().zip(r.values()).map(|(t, v)| (t.secs(), v / divisor)));
        }
        token = crate::aws::pagination::next_page_token(resp.next_token(), &token);
        if token.is_none() {
            break;
        }
    }
    queries
        .iter()
        .zip(points.into_iter().zip(errors))
        .map(|(q, (mut pts, error))| {
            pts.sort_by_key(|(t, _)| *t);
            if q.sparse {
                zero_fill(&mut pts, start, end, period as i64);
            }
            MetricSeries { query: q.clone(), points: pts, error }
        })
        .collect()
}

/// `{MetricName}` placeholders as the ids of the set's metrics of that
/// name (`q0`, `q1`, …), or the name of one the set doesn't have.
fn resolve_expression(expr: &str, queries: &[MetricQuery]) -> Result<String, String> {
    let mut out = String::with_capacity(expr.len());
    let mut rest = expr;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let close = after.find('}').ok_or_else(|| "unclosed { in the expression".to_string())?;
        let name = &after[..close];
        let i = queries
            .iter()
            .position(|q| q.expression.is_none() && q.metric == name)
            .ok_or_else(|| format!("needs {name}, which isn't charted"))?;
        out.push_str(&format!("q{i}"));
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// How long CloudWatch can take to publish a period: a missing point this
/// close to the window's end may be late, not zero.
const PUBLISH_LAG_SECS: i64 = 180;

/// A sparse metric's quiet periods as zeros, so min, avg and last and the
/// chart read them as nothing happening rather than skipping them. A
/// series with no points at all stays empty ("no data in the window"),
/// and the last few minutes are left alone (`PUBLISH_LAG_SECS`).
fn zero_fill(points: &mut Vec<(i64, f64)>, start: i64, end: i64, period: i64) {
    let Some(&(first, _)) = points.first() else { return };
    if period <= 0 {
        return;
    }
    // CloudWatch's buckets are aligned to the period; follow its phase.
    let phase = first.rem_euclid(period);
    let mut t = start - (start - phase).rem_euclid(period);
    if t < start {
        t += period;
    }
    let have: std::collections::HashSet<i64> = points.iter().map(|(t, _)| *t).collect();
    let cutoff = end - period - PUBLISH_LAG_SECS;
    while t <= cutoff {
        if !have.contains(&t) {
            points.push((t, 0.0));
        }
        t += period;
    }
    points.sort_by_key(|(t, _)| *t);
}

/// One entry of the `a` picker: a metric `ListMetrics` knows for the resource.
#[derive(Debug, Clone, PartialEq)]
pub struct MetricDefinition {
    pub namespace: String,
    pub metric: String,
    pub dimensions: Vec<(String, String)>,
}

impl MetricDefinition {
    /// The definition as a query: `Average`, raw values (`s` changes the stat).
    pub fn query(&self) -> MetricQuery {
        MetricQuery {
            namespace: self.namespace.clone(),
            metric: self.metric.clone(),
            dimensions: self.dimensions.clone(),
            label: self.metric.clone(),
            stat: Stat::Average,
            unit: Unit::Raw,
            rate: false,
            group: None,
            sparse: false,
            tone: None,
            expression: None,
        }
    }
}

/// The picker holds at most this many metrics (five `ListMetrics` pages).
pub const MAX_DEFINITIONS: usize = 2_500;

/// Every metric carrying the resource's dimensions, in any namespace, sorted
/// namespace → fewest extra dimensions → name.
pub async fn list_definitions(
    cw: &aws_sdk_cloudwatch::Client,
    dimensions: &[(String, String)],
) -> Result<Vec<MetricDefinition>, String> {
    let filters: Vec<DimensionFilter> =
        dimensions.iter().map(|(n, v)| DimensionFilter::builder().name(n).value(v).build()).collect();
    let mut out: Vec<MetricDefinition> = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..5 {
        let resp = cw
            .list_metrics()
            .set_dimensions(Some(filters.clone()))
            .set_next_token(token.clone())
            .send()
            .await
            .map_err(|e| crate::error::sdk_error_message(&e))?;
        for metric in resp.metrics() {
            out.push(MetricDefinition {
                namespace: metric.namespace().unwrap_or_default().to_string(),
                metric: metric.metric_name().unwrap_or_default().to_string(),
                dimensions: metric
                    .dimensions()
                    .iter()
                    .map(|d| (d.name().unwrap_or_default().to_string(), d.value().unwrap_or_default().to_string()))
                    .collect(),
            });
        }
        token = crate::aws::pagination::next_page_token(resp.next_token(), &token);
        if token.is_none() || out.len() >= MAX_DEFINITIONS {
            break;
        }
    }
    out.sort_by(|a, b| {
        (&a.namespace, a.dimensions.len(), &a.metric, &a.dimensions)
            .cmp(&(&b.namespace, b.dimensions.len(), &b.metric, &b.dimensions))
    });
    out.dedup();
    out.truncate(MAX_DEFINITIONS);
    Ok(out)
}

/// `41%`, `2.1 MB/s`, `340 ms`, `1.2k`.
pub fn fmt_value(v: f64, unit: Unit) -> String {
    fn si(v: f64) -> String {
        let a = v.abs();
        if a >= 1e9 {
            format!("{:.1}G", v / 1e9)
        } else if a >= 1e6 {
            format!("{:.1}M", v / 1e6)
        } else if a >= 1e4 {
            format!("{:.1}k", v / 1e3)
        } else if a >= 100.0 || v.fract() == 0.0 {
            format!("{v:.0}")
        } else if a >= 10.0 {
            format!("{v:.1}")
        } else {
            format!("{v:.2}")
        }
    }
    fn bytes(v: f64) -> String {
        const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
        let mut v = v;
        let mut i = 0;
        while v.abs() >= 1024.0 && i < U.len() - 1 {
            v /= 1024.0;
            i += 1;
        }
        if i == 0 { format!("{v:.0} B") } else { format!("{v:.1} {}", U[i]) }
    }
    match unit {
        Unit::Percent => format!("{v:.1}%"),
        Unit::Bytes => bytes(v),
        Unit::Megabytes => bytes(v * 1_048_576.0),
        Unit::BytesPerSec => format!("{}/s", bytes(v)),
        Unit::BitsPerSec => {
            let (d, u) = [(1e9, "Gbps"), (1e6, "Mbps"), (1e3, "kbps")]
                .into_iter()
                .find(|(d, _)| v.abs() >= *d)
                .unwrap_or((1.0, "bps"));
            format!("{:.1} {u}", v / d)
        }
        Unit::Count | Unit::Raw => si(v),
        Unit::PerSec => format!("{}/s", si(v)),
        Unit::Millis if v >= 1000.0 => format!("{:.2} s", v / 1000.0),
        Unit::Millis => format!("{v:.0} ms"),
        Unit::Seconds if v < 1.0 => format!("{:.1} ms", v * 1000.0),
        Unit::Seconds => format!("{v:.2} s"),
    }
}

/// `1`–`4` pick a range in the view.
pub fn range_from_digit(c: char) -> Option<MetricsTimeRange> {
    Some(match c {
        '1' => MetricsTimeRange::OneHour,
        '2' => MetricsTimeRange::SixHours,
        '3' => MetricsTimeRange::TwentyFourHours,
        '4' => MetricsTimeRange::SevenDays,
        '5' => MetricsTimeRange::ThirtyDays,
        _ => return None,
    })
}

pub const RANGES: [MetricsTimeRange; 5] = [
    MetricsTimeRange::OneHour,
    MetricsTimeRange::SixHours,
    MetricsTimeRange::TwentyFourHours,
    MetricsTimeRange::SevenDays,
    MetricsTimeRange::ThirtyDays,
];

/// The range `step` places along `RANGES` from `r`, wrapping.
pub fn step_range(r: MetricsTimeRange, step: isize) -> MetricsTimeRange {
    let i = RANGES.iter().position(|x| *x == r).unwrap_or(0) as isize;
    RANGES[(i + step).rem_euclid(RANGES.len() as isize) as usize]
}

#[cfg(test)]
mod tests {
    use super::*;

    impl MetricDefinition {
        fn default_query() -> MetricQuery {
            MetricDefinition { namespace: "AWS/WAFV2".into(), metric: "BlockedRequests".into(), dimensions: vec![] }.query()
        }
    }

    fn table() -> BreakdownTable {
        BreakdownTable {
            title: "Requests by rule".to_string(),
            columns: &["Allow", "Block", "Count"],
            rows: vec![
                ("AWS-AWSManagedRulesCommonRuleSet".to_string(), vec![0.0, 1_500.0, 0.0]),
                ("Default_Action".to_string(), vec![900.0, 0.0, 0.0]),
            ],
            drill: None,
        }
    }

    #[test]
    fn an_expression_names_the_set_s_metrics_by_id() {
        let target = MetricTarget {
            title: String::new(),
            namespace: "AWS/WAFV2".into(),
            dimensions: vec![("WebACL".into(), "a".into())],
            specs: WAF_WEB_ACL,
            home: None,
            hint: None,
            extra: Vec::new(),
            breakdown: None,
        };
        let qs = target.curated_queries();
        let rate = qs.iter().find(|q| q.label == "Block rate").unwrap();
        let resolved = resolve_expression(rate.expression.as_deref().unwrap(), &qs).unwrap();
        assert!(resolved.contains("FILL(q1,0)") && resolved.contains("FILL(q0,0)"), "{resolved}");
        assert!(!resolved.contains('{') && !resolved.contains("FILL(q2"), "Count isn't in the rate: {resolved}");
        let only_blocked: Vec<MetricQuery> = qs.iter().filter(|q| q.metric != "AllowedRequests").cloned().collect();
        let err = resolve_expression(rate.expression.as_deref().unwrap(), &only_blocked).unwrap_err();
        assert_eq!(err, "needs AllowedRequests, which isn't charted");
        assert_eq!(WAF_RULE.len(), 5);
        assert!(WAF_RULE.iter().all(|s| s.expr.is_none()));
    }

    #[test]
    fn a_sparse_series_reads_its_quiet_minutes_as_zero() {
        // Three blocked requests in one minute of an hour.
        let (start, end) = (6_000, 6_000 + 3_600);
        let mut pts = vec![(6_600, 3.0)];
        zero_fill(&mut pts, start, end, 60);
        let s = MetricSeries { query: MetricDefinition::default_query(), points: pts.clone(), error: None };
        let st = s.stats().unwrap();
        assert_eq!((st.min, st.max), (0.0, 3.0));
        assert!(st.avg < 0.06, "not 3: {}", st.avg);
        assert!(pts.windows(2).all(|w| w[1].0 - w[0].0 == 60), "one point a minute");
        assert!(pts.last().unwrap().0 <= end - 60 - PUBLISH_LAG_SECS, "the last minutes may still be publishing");
        let mut empty: Vec<(i64, f64)> = Vec::new();
        zero_fill(&mut empty, start, end, 60);
        assert!(empty.is_empty(), "no data stays no data");
    }

    #[test]
    fn a_breakdown_shows_only_columns_with_traffic() {
        let (header, rows) = table().text(80, 10);
        assert!(header.contains("Total") && header.contains("Allow") && header.contains("Block"), "{header}");
        assert!(!header.contains("Count"), "no rule counted anything: {header}");
        assert!(rows[0].contains("AWS-AWSManagedRulesCommonRuleSet") && rows[0].contains("1.5K"), "{}", rows[0]);
        // A zero cell is a dash, so the row's one action stands out.
        assert!(rows[1].trim_end().ends_with("—"), "{}", rows[1]);
    }

    #[test]
    fn a_narrow_breakdown_drops_columns_before_the_name_gets_short() {
        let (header, rows) = table().text(36, 10);
        assert!(header.contains("Total") && header.contains("Allow") && !header.contains("Block"), "{header}");
        assert!(rows.iter().all(|r| r.chars().count() <= 36), "{rows:?}");
        assert!(rows[0].contains("…") && rows[0].contains("CommonRuleSet"), "keeps the name's end: {}", rows[0]);
        let (_, rows) = table().text(80, 1);
        assert_eq!(rows.len(), 1, "max_rows caps the rows");
    }

    #[test]
    fn the_full_text_keeps_every_name_whole() {
        let mut t = table();
        let long = "awswaf:managed:aws:bot-control:bot:category:http_library";
        t.rows.push((long.to_string(), vec![0.0, 640.0, 0.0]));
        let (_, rows) = t.text(120, 10);
        assert!(!rows[2].contains(long), "the view truncates: {}", rows[2]);
        let (header, rows) = t.full_text();
        assert!(rows[2].contains(long) && rows.iter().all(|r| !r.contains('…')), "{rows:?}");
        assert!(header.contains("Allow") && header.contains("Block"), "{header}");
    }

    #[test]
    fn values_read_in_their_unit() {
        assert_eq!(fmt_value(41.234, Unit::Percent), "41.2%");
        assert_eq!(fmt_value(2.0 * 1024.0 * 1024.0, Unit::BytesPerSec), "2.0 MB/s");
        assert_eq!(fmt_value(340.4, Unit::Millis), "340 ms");
        assert_eq!(fmt_value(0.0042, Unit::Seconds), "4.2 ms");
        assert_eq!(fmt_value(12_345.0, Unit::Count), "12.3k");
        assert_eq!(fmt_value(3.0, Unit::Count), "3");
        assert_eq!(fmt_value(1.25e9, Unit::BitsPerSec), "1.2 Gbps");
    }

    #[test]
    fn a_target_resolves_its_specs_against_the_resource() {
        let t = MetricTarget {
            title: "EC2 instance · web".into(),
            namespace: "AWS/EC2".into(),
            dimensions: vec![("InstanceId".into(), "i-1".into())],
            specs: EC2_INSTANCE,
            home: None,
            hint: None,
            extra: Vec::new(),
            breakdown: None,
        };
        let q = t.curated_queries();
        assert_eq!(q.len(), EC2_INSTANCE.len());
        let net_in = q.iter().find(|q| q.metric == "NetworkIn").unwrap();
        assert!(net_in.rate && net_in.group.as_deref() == Some("net"));
        assert_eq!(net_in.dimensions, vec![("InstanceId".to_string(), "i-1".to_string())]);
    }

    #[test]
    fn a_denied_call_reads_as_no_access() {
        let e = "User: arn:aws:sts::1:assumed-role/r is not authorized to perform: cloudwatch:GetMetricData";
        assert_eq!(short_error(e), "no access (cloudwatch:GetMetricData)");
        assert_eq!(Stat::P99.next(), Stat::Average, "s wraps around");
    }
}
