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
}

const fn m(metric: &'static str, label: &'static str, stat: Stat, unit: Unit) -> MetricSpec {
    MetricSpec { metric, label, stat, unit, rate: false, group: None, namespace: None }
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
}

impl MetricTarget {
    pub fn curated_queries(&self) -> Vec<MetricQuery> {
        self.specs
            .iter()
            .map(|s| MetricQuery {
                namespace: s.namespace.map_or_else(|| self.namespace.clone(), str::to_string),
                metric: s.metric.to_string(),
                dimensions: self.dimensions.clone(),
                label: s.label.to_string(),
                stat: s.stat,
                unit: s.unit,
                rate: s.rate,
                group: s.group.map(str::to_string),
            })
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
        let head = format!("{} ({} {})", q.label, q.stat.short(), q.metric);
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
    let data_queries: Vec<MetricDataQuery> = queries
        .iter()
        .enumerate()
        .map(|(i, q)| {
            let metric = Metric::builder()
                .namespace(&q.namespace)
                .metric_name(&q.metric)
                .set_dimensions(Some(dims(&q.dimensions)))
                .build();
            let stat = MetricStat::builder().metric(metric).period(period).stat(q.stat.api()).build();
            MetricDataQuery::builder().id(format!("q{i}")).metric_stat(stat).build()
        })
        .collect();

    let mut points: Vec<Vec<(i64, f64)>> = vec![Vec::new(); queries.len()];
    let mut errors: Vec<Option<String>> = vec![Some("not in the response".to_string()); queries.len()];
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
            MetricSeries { query: q.clone(), points: pts, error }
        })
        .collect()
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
        _ => return None,
    })
}

pub const RANGES: [MetricsTimeRange; 4] = [
    MetricsTimeRange::OneHour,
    MetricsTimeRange::SixHours,
    MetricsTimeRange::TwentyFourHours,
    MetricsTimeRange::SevenDays,
];

#[cfg(test)]
mod tests {
    use super::*;

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
