use crate::aws::client::AwsClients;
use crate::aws::resource::{native_state_label, Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::aws::services::ec2::MetricsTimeRange;
use crate::error::Result;
use crate::event::{Event, LoadProgress};
use async_trait::async_trait;
use aws_sdk_firehose::Client as FirehoseClient;
use aws_sdk_kinesis::Client as KinesisClient;
use futures::stream::{self, StreamExt};
use std::any::Any;
use std::collections::HashMap;
use tokio::sync::mpsc;

/// Kinesis service — two sub-tabs (Streams / Firehose). **Data Streams** are the
/// primary list: an N+1 streaming load (`ListStreams` then a bounded-concurrency
/// `DescribeStreamSummary` per stream — summary, not the full shard list, so it's
/// cheap even for high-shard streams) with a Details / Consumers / Tags split
/// pane. **Firehose delivery streams** stream as a second batch
/// (`ListDeliveryStreams` + `DescribeDeliveryStream` per stream — source,
/// destination, buffering, compression, error-output prefix) with an Overview /
/// Destination / Tags split pane. Both support `m` metrics (`AWS/Kinesis` /
/// `AWS/Firehose`) and error-tolerant loads (one tab failing never blanks the
/// other). Enhanced fan-out consumers and tags are the lazy sections.
pub struct KinesisService {
    client: KinesisClient,
    firehose: FirehoseClient,
}

impl KinesisService {
    pub fn new(aws_clients: &AwsClients) -> Self {
        Self {
            client: aws_clients.kinesis_client(),
            firehose: aws_clients.firehose_client(),
        }
    }
}

#[async_trait]
impl AwsService for KinesisService {
    fn service_type(&self) -> ServiceType {
        ServiceType::Kinesis
    }

    fn name(&self) -> &str {
        "Kinesis"
    }

    async fn list_resources(&self) -> Result<Vec<Box<dyn Resource>>> {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        self.list_resources_streaming(tx, ServiceType::Kinesis).await?;
        Ok(vec![])
    }

    async fn list_resources_streaming(
        &self,
        event_tx: mpsc::UnboundedSender<Event>,
        service_type: ServiceType,
    ) -> Result<()> {
        let mut total = 0usize;

        // ── Kinesis Data Streams ──────────────────────────────────────────────
        // ListStreams (paginated) → stream names. Error-tolerant: a failure here
        // records the error but still lets Firehose load.
        let mut names: Vec<String> = Vec::new();
        let mut paginator = self.client.list_streams().into_paginator().send();
        while let Some(page) = paginator.next().await {
            match page {
                Ok(p) => names.extend(p.stream_names().iter().cloned()),
                Err(e) => {
                    let _ = event_tx.send(Event::ResourceLoadWarning {
                        service: service_type,
                        warning: format!(
                            "Kinesis streams: {}",
                            crate::error::sdk_error_message(&e)
                        ),
                    });
                    break;
                }
            }
        }

        if !names.is_empty() {
            // DescribeStreamSummary per stream, bounded concurrency.
            let streams: Vec<KinesisStream> = stream::iter(names)
                .map(|name| {
                    let client = self.client.clone();
                    async move {
                        client
                            .describe_stream_summary()
                            .stream_name(&name)
                            .send()
                            .await
                            .ok()
                            .and_then(|r| r.stream_description_summary().cloned())
                            .map(|s| KinesisStream::from_sdk(&s))
                    }
                })
                .buffer_unordered(8)
                .filter_map(|s| async move { s })
                .collect()
                .await;

            total += streams.len();
            let batch: Vec<Box<dyn Resource>> = streams
                .into_iter()
                .map(|s| Box::new(s) as Box<dyn Resource>)
                .collect();
            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                service: service_type,
                resources: batch,
                progress: LoadProgress {
                    loaded_count: total,
                    total_count: None,
                    status_message: Some("Loading Firehose…".to_string()),
                },
            });
        }

        // ── Firehose delivery streams (second batch) ──────────────────────────
        // ListDeliveryStreams has no fluent paginator — hand-rolled loop via
        // `has_more_delivery_streams` + `exclusive_start_delivery_stream_name`.
        let mut fh_names: Vec<String> = Vec::new();
        let mut start: Option<String> = None;
        let mut fh_list_ok = true;
        loop {
            let mut req = self.firehose.list_delivery_streams().limit(100);
            if let Some(s) = &start {
                req = req.exclusive_start_delivery_stream_name(s);
            }
            match req.send().await {
                Ok(resp) => {
                    let page: Vec<String> = resp.delivery_stream_names().to_vec();
                    let more = resp.has_more_delivery_streams();
                    start = page.last().cloned();
                    fh_names.extend(page);
                    if !more || start.is_none() {
                        break;
                    }
                }
                Err(e) => {
                    let _ = event_tx.send(Event::ResourceLoadWarning {
                        service: service_type,
                        warning: format!(
                            "Firehose delivery streams: {}",
                            crate::error::sdk_error_message(&e)
                        ),
                    });
                    fh_list_ok = false;
                    break;
                }
            }
        }

        if fh_list_ok && !fh_names.is_empty() {
            // DescribeDeliveryStream per stream, bounded concurrency.
            let fh_streams: Vec<FirehoseStream> = stream::iter(fh_names)
                .map(|name| {
                    let client = self.firehose.clone();
                    async move {
                        client
                            .describe_delivery_stream()
                            .delivery_stream_name(&name)
                            .send()
                            .await
                            .ok()
                            .and_then(|r| r.delivery_stream_description().cloned())
                            .map(|d| FirehoseStream::from_sdk(&d))
                    }
                })
                .buffer_unordered(8)
                .filter_map(|s| async move { s })
                .collect()
                .await;

            total += fh_streams.len();
            let batch: Vec<Box<dyn Resource>> = fh_streams
                .into_iter()
                .map(|s| Box::new(s) as Box<dyn Resource>)
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

// ── Resource ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct KinesisStream {
    pub name: String,
    pub arn: String,
    pub status: String,
    pub mode: String, // PROVISIONED / ON_DEMAND
    pub retention_hours: i32,
    pub encryption_type: Option<String>,
    pub kms_key_id: Option<String>,
    pub open_shard_count: i32,
    pub consumer_count: Option<i32>,
    pub enhanced_metrics: Vec<String>,
    pub created: Option<String>,
    pub tags: HashMap<String, String>,
}

impl KinesisStream {
    pub fn from_sdk(s: &aws_sdk_kinesis::types::StreamDescriptionSummary) -> Self {
        let enhanced_metrics: Vec<String> = s
            .enhanced_monitoring()
            .iter()
            .flat_map(|em| em.shard_level_metrics().iter())
            .map(|m| m.as_str().to_string())
            .filter(|m| m != "ALL")
            .collect();

        Self {
            name: s.stream_name().to_string(),
            arn: s.stream_arn().to_string(),
            status: s.stream_status().as_str().to_string(),
            mode: s
                .stream_mode_details()
                .map(|d| d.stream_mode().as_str().to_string())
                .unwrap_or_else(|| "PROVISIONED".to_string()),
            retention_hours: s.retention_period_hours(),
            encryption_type: s.encryption_type().map(|e| e.as_str().to_string()),
            kms_key_id: s.key_id().map(|s| s.to_string()),
            open_shard_count: s.open_shard_count(),
            consumer_count: s.consumer_count(),
            enhanced_metrics,
            created: Some(crate::aws::services::cloudwatch::fmt_epoch_secs(
                s.stream_creation_timestamp().secs(),
            )),
            tags: HashMap::new(),
        }
    }

    /// "168 h (7 d)" — retention with a friendly day count when it's a round day.
    pub fn retention_label(&self) -> String {
        let h = self.retention_hours;
        if h % 24 == 0 {
            format!("{} h ({} d)", h, h / 24)
        } else {
            format!("{} h", h)
        }
    }

    pub fn is_encrypted(&self) -> bool {
        matches!(self.encryption_type.as_deref(), Some("KMS"))
    }
}

crate::sections! {
    pub enum KinesisDetailSection,
    pub static KINESIS_SECTIONS = [
        Details "Details",
        Consumers "Consumers" => crate::app::App::trigger_kinesis_consumers_load,
        Tags "Tags" => crate::app::App::trigger_kinesis_tags_load,
    ]
}

impl Resource for KinesisStream {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&KINESIS_SECTIONS)
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws kinesis describe-stream-summary --stream-name {}",
            crate::aws::resource::shell_quote(self.id())
        ))
    }

    fn id(&self) -> &str {
        &self.name
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn resource_type(&self) -> &str {
        "Kinesis Stream"
    }

    fn state(&self) -> ResourceState {
        match self.status.as_str() {
            "ACTIVE" => ResourceState::Available,
            "CREATING" => ResourceState::Creating,
            "UPDATING" => ResourceState::Pending,
            "DELETING" => ResourceState::Deleting,
            other => ResourceState::Unknown(other.to_string()),
        }
    }

    fn state_label(&self) -> String {
        native_state_label(&self.status, || self.state())
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!("{} {} {} {}", self.name, self.mode, self.status, self.arn)
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Name".to_string(), self.name.clone()),
            ("Status".to_string(), self.status.clone()),
            ("Capacity Mode".to_string(), self.mode.clone()),
            ("Open Shards".to_string(), self.open_shard_count.to_string()),
            ("Retention".to_string(), self.retention_label()),
        ]
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/kinesis/home?region={}#/streams/details/{}/monitoring",
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

// ── Consumers (lazy section) ──────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct KinesisConsumer {
    pub name: String,
    pub arn: String,
    pub status: String,
    pub created: Option<String>,
}

/// `ListStreamConsumers` — registered enhanced fan-out consumers (keyed by ARN).
pub async fn fetch_kinesis_consumers(
    client: KinesisClient,
    stream_arn: String,
) -> Result<Vec<KinesisConsumer>> {
    let mut out = Vec::new();
    let mut paginator = client
        .list_stream_consumers()
        .stream_arn(&stream_arn)
        .into_paginator()
        .send();
    while let Some(page) = paginator.next().await {
        let page = page.map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
        for c in page.consumers() {
            out.push(KinesisConsumer {
                name: c.consumer_name().to_string(),
                arn: c.consumer_arn().to_string(),
                status: c.consumer_status().as_str().to_string(),
                created: Some(crate::aws::services::cloudwatch::fmt_epoch_secs(
                    c.consumer_creation_timestamp().secs(),
                )),
            });
        }
    }
    Ok(out)
}

/// `ListTagsForStream` — paginated via `has_more_tags`, capped at a few pages.
pub async fn fetch_kinesis_tags(
    client: KinesisClient,
    stream_name: String,
) -> Result<Vec<(String, String)>> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut start: Option<String> = None;
    for _ in 0..10 {
        let mut req = client.list_tags_for_stream().stream_name(&stream_name);
        if let Some(s) = &start {
            req = req.exclusive_start_tag_key(s);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
        for t in resp.tags() {
            out.push((
                t.key().to_string(),
                t.value().unwrap_or_default().to_string(),
            ));
        }
        if resp.has_more_tags() && !out.is_empty() {
            start = out.last().map(|(k, _)| k.clone());
        } else {
            break;
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

// ── Firehose delivery stream (Firehose sub-tab) ───────────────────────────────

/// One record-processing step (a "processor") on the delivery stream — most
/// commonly a Lambda data-transformation, but also metadata extraction (for
/// dynamic partitioning), decompression, delimiter appending, etc.
#[derive(Debug, Clone)]
pub struct FirehoseProcessor {
    pub kind: String,               // Lambda / MetadataExtraction / Decompression / …
    pub lambda_arn: Option<String>, // jumpable when kind == Lambda
    pub params: Vec<(String, String)>, // buffer hints, retries, query, …
}

/// The resolved destination of a Firehose delivery stream, flattened from the
/// SDK's per-destination-type union into rows the split pane can render. `arn`
/// is the jumpable primary target (S3 bucket / OpenSearch domain); `extra` holds
/// destination-specific rows (HTTP url, Redshift JDBC, index name, …).
#[derive(Debug, Clone, Default)]
pub struct FirehoseDestination {
    pub kind: String,
    pub arn: Option<String>,
    pub buffering: Option<(i32, i32)>, // (size MB, interval sec)
    pub compression: Option<String>,
    pub prefix: Option<String>,
    pub error_prefix: Option<String>,
    pub extra: Vec<(String, String)>,
    // Enrichment (Extended-S3 mostly, plus the S3-backed destinations):
    pub format_conversion: Option<String>, // "Parquet" / "ORC" / "Enabled"
    pub dynamic_partitioning: bool,
    pub s3_backup: Option<String>, // backup mode when != Disabled
    pub processors: Vec<FirehoseProcessor>,
    pub log_group: Option<String>, // CloudWatch error-logging group (enabled)
    pub log_stream: Option<String>,
}

#[derive(Debug, Clone)]
pub struct FirehoseStream {
    pub name: String,
    pub arn: String,
    pub status: String,
    pub stream_type: String, // DirectPut / KinesisStreamAsSource / MSKAsSource / …
    pub version_id: String,
    pub source: Option<String>, // resolved source label (Kinesis ARN / MSK cluster / Direct PUT)
    pub dest: FirehoseDestination,
    pub encryption: Option<String>, // key type or "AWS owned CMK"
    pub kms_key_arn: Option<String>,
    pub failure: Option<(String, String)>, // (type, details) when *_FAILED
    pub created: Option<String>,
    pub tags: HashMap<String, String>,
}

impl FirehoseStream {
    pub fn from_sdk(d: &aws_sdk_firehose::types::DeliveryStreamDescription) -> Self {
        let source = d.source().and_then(|s| {
            if let Some(k) = s.kinesis_stream_source_description() {
                k.kinesis_stream_arn().map(|a| a.to_string())
            } else if let Some(m) = s.msk_source_description() {
                match (m.msk_cluster_arn(), m.topic_name()) {
                    (Some(c), Some(t)) => Some(format!("{} (topic {})", c, t)),
                    (Some(c), None) => Some(c.to_string()),
                    _ => None,
                }
            } else if s.direct_put_source_description().is_some() {
                Some("Direct PUT".to_string())
            } else {
                None
            }
        });

        // Firehose has exactly one active destination; take the first.
        let dest = d
            .destinations()
            .first()
            .map(parse_destination)
            .unwrap_or_default();

        let (encryption, kms_key_arn) = match d.delivery_stream_encryption_configuration() {
            Some(enc) => {
                let key_type = enc.key_type().map(|k| k.as_str().to_string());
                (key_type, enc.key_arn().map(|a| a.to_string()))
            }
            None => (None, None),
        };

        let failure = d
            .failure_description()
            .map(|f| (f.r#type().as_str().to_string(), f.details().to_string()));

        Self {
            name: d.delivery_stream_name().to_string(),
            arn: d.delivery_stream_arn().to_string(),
            status: d.delivery_stream_status().as_str().to_string(),
            stream_type: d.delivery_stream_type().as_str().to_string(),
            version_id: d.version_id().to_string(),
            source,
            dest,
            encryption,
            kms_key_arn,
            failure,
            created: d.create_timestamp().map(|t| {
                crate::aws::services::cloudwatch::fmt_epoch_secs(t.secs())
            }),
            tags: HashMap::new(),
        }
    }

    pub fn buffering_label(&self) -> Option<String> {
        self.dest
            .buffering
            .map(|(sz, iv)| format!("{} MB / {} s", sz, iv))
    }

    /// The CloudWatch error-logging group, when error logging is enabled — the
    /// tail target for `t` (delivery/transform failures land here).
    pub fn error_log_group(&self) -> Option<&str> {
        self.dest.log_group.as_deref()
    }
}

/// Buffering / compression / prefix / error-prefix pulled from an S3
/// destination — shared by the direct-S3 destinations and the intermediate S3
/// backup used by Redshift/OpenSearch/Splunk/HTTP.
struct S3Common {
    buffering: Option<(i32, i32)>,
    compression: Option<String>,
    prefix: Option<String>,
    error_prefix: Option<String>,
}

fn s3_common(s3: &aws_sdk_firehose::types::S3DestinationDescription) -> S3Common {
    let buffering = s3.buffering_hints().and_then(|b| {
        match (b.size_in_mbs(), b.interval_in_seconds()) {
            (Some(sz), Some(iv)) => Some((sz, iv)),
            _ => None,
        }
    });
    S3Common {
        buffering,
        compression: Some(s3.compression_format().as_str().to_string()),
        prefix: s3.prefix().map(|p| p.to_string()),
        error_prefix: s3.error_output_prefix().map(|p| p.to_string()),
    }
}

/// Flatten a `ProcessingConfiguration` into displayable processors. Skips the
/// whole thing when processing is disabled or empty. Each processor's Lambda ARN
/// (if any) is split out so the pane can make it jumpable; the rest of the
/// parameters (buffer hints, retries, extraction query, …) become rows.
fn parse_processors(
    pc: Option<&aws_sdk_firehose::types::ProcessingConfiguration>,
) -> Vec<FirehoseProcessor> {
    let Some(pc) = pc else { return Vec::new() };
    if !pc.enabled().unwrap_or(false) {
        return Vec::new();
    }
    pc.processors()
        .iter()
        .map(|p| {
            let mut lambda_arn = None;
            let mut params = Vec::new();
            for pp in p.parameters() {
                let name = pp.parameter_name().as_str().to_string();
                let value = pp.parameter_value().to_string();
                if pp.parameter_name()
                    == &aws_sdk_firehose::types::ProcessorParameterName::LambdaArn
                {
                    lambda_arn = Some(value);
                } else {
                    params.push((name, value));
                }
            }
            FirehoseProcessor {
                kind: p.r#type().as_str().to_string(),
                lambda_arn,
                params,
            }
        })
        .collect()
}

/// CloudWatch error-logging group/stream, when logging is enabled.
fn logging_group(
    cw: Option<&aws_sdk_firehose::types::CloudWatchLoggingOptions>,
) -> (Option<String>, Option<String>) {
    match cw {
        Some(o) if o.enabled().unwrap_or(false) => (
            o.log_group_name().map(|s| s.to_string()),
            o.log_stream_name().map(|s| s.to_string()),
        ),
        _ => (None, None),
    }
}

fn parse_destination(d: &aws_sdk_firehose::types::DestinationDescription) -> FirehoseDestination {
    let mut out = FirehoseDestination::default();

    if let Some(s3) = d.extended_s3_destination_description() {
        out.kind = "Amazon S3".to_string();
        out.arn = Some(s3.bucket_arn().to_string());
        out.buffering = s3.buffering_hints().and_then(|b| {
            match (b.size_in_mbs(), b.interval_in_seconds()) {
                (Some(sz), Some(iv)) => Some((sz, iv)),
                _ => None,
            }
        });
        out.compression = Some(s3.compression_format().as_str().to_string());
        out.prefix = s3.prefix().map(|p| p.to_string());
        out.error_prefix = s3.error_output_prefix().map(|p| p.to_string());
        // Extended-S3-only enrichment.
        out.format_conversion = s3.data_format_conversion_configuration().and_then(|c| {
            if !c.enabled().unwrap_or(false) {
                return None;
            }
            let ser = c
                .output_format_configuration()
                .and_then(|o| o.serializer());
            Some(match ser {
                Some(s) if s.parquet_ser_de().is_some() => "Parquet".to_string(),
                Some(s) if s.orc_ser_de().is_some() => "ORC".to_string(),
                _ => "Enabled".to_string(),
            })
        });
        out.dynamic_partitioning = s3
            .dynamic_partitioning_configuration()
            .map(|c| c.enabled().unwrap_or(false))
            .unwrap_or(false);
        out.s3_backup = s3
            .s3_backup_mode()
            .map(|m| m.as_str().to_string())
            .filter(|m| m != "Disabled");
        out.processors = parse_processors(s3.processing_configuration());
        let (g, s) = logging_group(s3.cloud_watch_logging_options());
        out.log_group = g;
        out.log_stream = s;
    } else if let Some(s3) = d.s3_destination_description() {
        out.kind = "Amazon S3".to_string();
        out.arn = Some(s3.bucket_arn().to_string());
        let c = s3_common(s3);
        out.buffering = c.buffering;
        out.compression = c.compression;
        out.prefix = c.prefix;
        out.error_prefix = c.error_prefix;
        let (g, st) = logging_group(s3.cloud_watch_logging_options());
        out.log_group = g;
        out.log_stream = st;
    } else if let Some(rs) = d.redshift_destination_description() {
        out.kind = "Amazon Redshift".to_string();
        out.extra
            .push(("Cluster JDBC".to_string(), rs.cluster_jdbcurl().to_string()));
        if let Some(u) = rs.username() {
            out.extra.push(("Username".to_string(), u.to_string()));
        }
        if let Some(s3) = rs.s3_destination_description() {
            let c = s3_common(s3);
            out.buffering = c.buffering;
            out.compression = c.compression;
            out.prefix = c.prefix;
            out.error_prefix = c.error_prefix;
            out.arn = Some(s3.bucket_arn().to_string());
        }
        out.processors = parse_processors(rs.processing_configuration());
        let (g, s) = logging_group(rs.cloud_watch_logging_options());
        out.log_group = g;
        out.log_stream = s;
    } else if let Some(os) = d.amazonopensearchservice_destination_description() {
        out.kind = "Amazon OpenSearch".to_string();
        out.arn = os.domain_arn().map(|a| a.to_string());
        if let Some(ep) = os.cluster_endpoint() {
            out.extra.push(("Cluster Endpoint".to_string(), ep.to_string()));
        }
        if let Some(idx) = os.index_name() {
            out.extra.push(("Index".to_string(), idx.to_string()));
        }
        if let Some(s3) = os.s3_destination_description() {
            let c = s3_common(s3);
            out.buffering = c.buffering;
            out.compression = c.compression;
            out.error_prefix = c.error_prefix;
        }
        out.processors = parse_processors(os.processing_configuration());
        let (g, s) = logging_group(os.cloud_watch_logging_options());
        out.log_group = g;
        out.log_stream = s;
    } else if let Some(http) = d.http_endpoint_destination_description() {
        out.kind = "HTTP Endpoint".to_string();
        if let Some(ep) = http.endpoint_configuration() {
            if let Some(url) = ep.url() {
                out.extra.push(("URL".to_string(), url.to_string()));
            }
            if let Some(n) = ep.name() {
                out.extra.push(("Name".to_string(), n.to_string()));
            }
        }
        if let Some(s3) = http.s3_destination_description() {
            let c = s3_common(s3);
            out.buffering = c.buffering;
            out.compression = c.compression;
            out.error_prefix = c.error_prefix;
            out.arn = Some(s3.bucket_arn().to_string());
        }
        out.processors = parse_processors(http.processing_configuration());
        let (g, s) = logging_group(http.cloud_watch_logging_options());
        out.log_group = g;
        out.log_stream = s;
    } else if let Some(sp) = d.splunk_destination_description() {
        out.kind = "Splunk".to_string();
        if let Some(ep) = sp.hec_endpoint() {
            out.extra.push(("HEC Endpoint".to_string(), ep.to_string()));
        }
        if let Some(t) = sp.hec_endpoint_type() {
            out.extra
                .push(("HEC Endpoint Type".to_string(), t.as_str().to_string()));
        }
        if let Some(s3) = sp.s3_destination_description() {
            let c = s3_common(s3);
            out.buffering = c.buffering;
            out.compression = c.compression;
            out.error_prefix = c.error_prefix;
            out.arn = Some(s3.bucket_arn().to_string());
        }
        out.processors = parse_processors(sp.processing_configuration());
        let (g, s) = logging_group(sp.cloud_watch_logging_options());
        out.log_group = g;
        out.log_stream = s;
    } else if d.snowflake_destination_description().is_some() {
        out.kind = "Snowflake".to_string();
    } else if d.iceberg_destination_description().is_some() {
        out.kind = "Apache Iceberg".to_string();
    } else if d.elasticsearch_destination_description().is_some() {
        out.kind = "Elasticsearch".to_string();
    } else if d
        .amazon_open_search_serverless_destination_description()
        .is_some()
    {
        out.kind = "OpenSearch Serverless".to_string();
    } else {
        out.kind = "Unknown".to_string();
    }

    out
}

crate::sections! {
    pub enum FirehoseDetailSection,
    pub static FIREHOSE_SECTIONS = [
        Overview "Overview",
        Destination "Destination",
        Processing "Processing",
        Tags "Tags" => crate::app::App::trigger_firehose_tags_load,
    ]
}

impl Resource for FirehoseStream {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&FIREHOSE_SECTIONS)
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws firehose describe-delivery-stream --delivery-stream-name {}",
            crate::aws::resource::shell_quote(self.id())
        ))
    }

    fn id(&self) -> &str {
        &self.name
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn resource_type(&self) -> &str {
        "Firehose Delivery Stream"
    }

    fn state(&self) -> ResourceState {
        match self.status.as_str() {
            "ACTIVE" => ResourceState::Available,
            "CREATING" => ResourceState::Creating,
            "DELETING" => ResourceState::Deleting,
            s if s.ends_with("_FAILED") => ResourceState::Unavailable,
            other => ResourceState::Unknown(other.to_string()),
        }
    }

    fn state_label(&self) -> String {
        native_state_label(&self.status, || self.state())
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!(
            "{} {} {} {} firehose",
            self.name, self.stream_type, self.dest.kind, self.arn
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Name".to_string(), self.name.clone()),
            ("Status".to_string(), self.status.clone()),
            ("Type".to_string(), self.stream_type.clone()),
            ("Destination".to_string(), self.dest.kind.clone()),
        ]
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/firehose/home?region={}#/details/{}/monitoring",
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

/// `ListTagsForDeliveryStream` — paginated via `has_more_tags`, capped at a few
/// pages (keyed by delivery-stream name).
pub async fn fetch_firehose_tags(
    client: FirehoseClient,
    stream_name: String,
) -> Result<Vec<(String, String)>> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut start: Option<String> = None;
    for _ in 0..10 {
        let mut req = client
            .list_tags_for_delivery_stream()
            .delivery_stream_name(&stream_name);
        if let Some(s) = &start {
            req = req.exclusive_start_tag_key(s);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
        for t in resp.tags() {
            out.push((t.key().to_string(), t.value().unwrap_or_default().to_string()));
        }
        if resp.has_more_tags() && !out.is_empty() {
            start = out.last().map(|(k, _)| k.clone());
        } else {
            break;
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

// ── Firehose CloudWatch metrics (`m` overlay) ─────────────────────────────────

#[derive(Debug, Clone)]
pub struct FirehoseMetricsData {
    pub time_range: MetricsTimeRange,
    pub incoming_records: Vec<(f64, f64)>,
    pub incoming_bytes: Vec<(f64, f64)>,
    pub delivery_records: Vec<(f64, f64)>, // DeliveryToS3.Records (Sum)
    pub delivery_success: Vec<(f64, f64)>, // DeliveryToS3.Success (Average, 0..1)
    pub data_freshness: Vec<(f64, f64)>,   // DeliveryToS3.DataFreshness (Max, sec)
    pub throttled_records: Vec<(f64, f64)>,
    pub x_max: f64,
}

#[derive(Debug, Clone)]
pub enum FirehoseMetricsState {
    Loading,
    Loaded(FirehoseMetricsData),
}

/// Pull `AWS/Firehose` metrics for one delivery stream. `IncomingBytes/Records`
/// are the ingest volume; `DeliveryToS3.Success` (a 0..1 ratio) flags delivery
/// failures; `ThrottledRecords` flags a throughput-limited stream.
pub async fn fetch_firehose_metrics(
    cw: aws_sdk_cloudwatch::Client,
    stream_name: String,
    time_range: MetricsTimeRange,
) -> Result<FirehoseMetricsData> {
    use aws_sdk_cloudwatch::types::{Dimension, Statistic};

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let start = now - time_range.duration_secs();
    let period = time_range.period_secs();
    let start_dt = aws_sdk_cloudwatch::primitives::DateTime::from_secs(start);
    let end_dt = aws_sdk_cloudwatch::primitives::DateTime::from_secs(now);

    let dim = || {
        Dimension::builder()
            .name("DeliveryStreamName")
            .value(&stream_name)
            .build()
    };
    let metric = |name: &'static str, stat: Statistic| {
        cw.get_metric_statistics()
            .namespace("AWS/Firehose")
            .metric_name(name)
            .dimensions(dim())
            .start_time(start_dt)
            .end_time(end_dt)
            .period(period)
            .set_statistics(Some(vec![stat]))
            .send()
    };

    let (records, bytes, del_records, success, freshness, throttled) = tokio::join!(
        metric("IncomingRecords", Statistic::Sum),
        metric("IncomingBytes", Statistic::Sum),
        metric("DeliveryToS3.Records", Statistic::Sum),
        metric("DeliveryToS3.Success", Statistic::Average),
        metric("DeliveryToS3.DataFreshness", Statistic::Maximum),
        metric("ThrottledRecords", Statistic::Sum),
    );

    Ok(FirehoseMetricsData {
        time_range,
        incoming_records: parse_points(records, start, |dp| dp.sum()),
        incoming_bytes: parse_points(bytes, start, |dp| dp.sum()),
        delivery_records: parse_points(del_records, start, |dp| dp.sum()),
        delivery_success: parse_points(success, start, |dp| dp.average()),
        data_freshness: parse_points(freshness, start, |dp| dp.maximum()),
        throttled_records: parse_points(throttled, start, |dp| dp.sum()),
        x_max: time_range.duration_secs() as f64,
    })
}

// ── CloudWatch metrics (`m` overlay) ──────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct KinesisMetricsData {
    pub time_range: MetricsTimeRange,
    pub incoming_records: Vec<(f64, f64)>,
    pub incoming_bytes: Vec<(f64, f64)>,
    pub iterator_age: Vec<(f64, f64)>,
    pub read_throttle: Vec<(f64, f64)>,
    pub write_throttle: Vec<(f64, f64)>,
    pub get_records_bytes: Vec<(f64, f64)>,
    pub x_max: f64,
}

#[derive(Debug, Clone)]
pub enum KinesisMetricsState {
    Loading,
    Loaded(KinesisMetricsData),
}

/// Pull `AWS/Kinesis` metrics for one stream. `GetRecords.IteratorAgeMilliseconds`
/// is the consumer-lag signal (rising = consumers falling behind); the throttle
/// metrics flag a provisioned stream that needs more shards.
pub async fn fetch_kinesis_metrics(
    cw: aws_sdk_cloudwatch::Client,
    stream_name: String,
    time_range: MetricsTimeRange,
) -> Result<KinesisMetricsData> {
    use aws_sdk_cloudwatch::types::{Dimension, Statistic};

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let start = now - time_range.duration_secs();
    let period = time_range.period_secs();
    let start_dt = aws_sdk_cloudwatch::primitives::DateTime::from_secs(start);
    let end_dt = aws_sdk_cloudwatch::primitives::DateTime::from_secs(now);

    let dim = || {
        Dimension::builder()
            .name("StreamName")
            .value(&stream_name)
            .build()
    };
    let metric = |name: &'static str, stat: Statistic| {
        cw.get_metric_statistics()
            .namespace("AWS/Kinesis")
            .metric_name(name)
            .dimensions(dim())
            .start_time(start_dt)
            .end_time(end_dt)
            .period(period)
            .set_statistics(Some(vec![stat]))
            .send()
    };

    let (records, bytes, age, read_t, write_t, get_bytes) = tokio::join!(
        metric("IncomingRecords", Statistic::Sum),
        metric("IncomingBytes", Statistic::Sum),
        metric("GetRecords.IteratorAgeMilliseconds", Statistic::Maximum),
        metric("ReadProvisionedThroughputExceeded", Statistic::Sum),
        metric("WriteProvisionedThroughputExceeded", Statistic::Sum),
        metric("GetRecords.Bytes", Statistic::Sum),
    );

    Ok(KinesisMetricsData {
        time_range,
        incoming_records: parse_points(records, start, |dp| dp.sum()),
        incoming_bytes: parse_points(bytes, start, |dp| dp.sum()),
        iterator_age: parse_points(age, start, |dp| dp.maximum()),
        read_throttle: parse_points(read_t, start, |dp| dp.sum()),
        write_throttle: parse_points(write_t, start, |dp| dp.sum()),
        get_records_bytes: parse_points(get_bytes, start, |dp| dp.sum()),
        x_max: time_range.duration_secs() as f64,
    })
}

fn parse_points<E>(
    resp: std::result::Result<
        aws_sdk_cloudwatch::operation::get_metric_statistics::GetMetricStatisticsOutput,
        E,
    >,
    start: i64,
    pick: impl Fn(&aws_sdk_cloudwatch::types::Datapoint) -> Option<f64>,
) -> Vec<(f64, f64)> {
    let dps = match resp {
        Ok(r) => r.datapoints().to_vec(),
        Err(_) => vec![],
    };
    let mut pts: Vec<(f64, f64)> = dps
        .iter()
        .filter_map(|dp| Some((dp.timestamp()?.secs() as f64 - start as f64, pick(dp)?)))
        .collect();
    pts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    pts
}
