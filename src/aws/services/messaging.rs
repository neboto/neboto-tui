use crate::aws::client::AwsClients;
use crate::aws::resource::{Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::error::Result;
use crate::event::{Event, LoadProgress};
use async_trait::async_trait;
use aws_sdk_sns::Client as SnsClient;
use aws_sdk_sqs::Client as SqsClient;
use aws_sdk_sqs::types::QueueAttributeName;
use std::any::Any;
use std::collections::HashMap;
use tokio::sync::mpsc;

pub struct MessagingService {
    sqs: SqsClient,
    sns: SnsClient,
}

impl MessagingService {
    pub fn new(aws_clients: &AwsClients) -> Self {
        Self {
            sqs: aws_clients.sqs_client(),
            sns: aws_clients.sns_client(),
        }
    }
}

#[async_trait]
impl AwsService for MessagingService {
    fn service_type(&self) -> ServiceType {
        ServiceType::Messaging
    }

    fn name(&self) -> &str {
        "Messaging"
    }

    async fn list_resources(&self) -> Result<Vec<Box<dyn Resource>>> {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        self.list_resources_streaming(tx, ServiceType::Messaging).await?;
        Ok(vec![])
    }

    async fn list_resources_streaming(
        &self,
        event_tx: mpsc::UnboundedSender<Event>,
        service_type: ServiceType,
    ) -> Result<()> {
        let mut total = 0usize;

        // ── SQS queues ──────────────────────────────────────────────────
        let mut q_paginator = self.sqs.list_queues().into_paginator().send();
        while let Some(result) = q_paginator.next().await {
            match result {
                Ok(page) => {
                    let urls: Vec<String> = page.queue_urls().to_vec();
                    if urls.is_empty() {
                        continue;
                    }
                    // Fetch attributes + tags for each queue concurrently.
                    let futs = urls.into_iter().map(|url| {
                        let sqs = self.sqs.clone();
                        async move { fetch_queue(&sqs, url).await }
                    });
                    let queues: Vec<SqsQueue> = futures::future::join_all(futs)
                        .await
                        .into_iter()
                        .flatten()
                        .collect();
                    if queues.is_empty() {
                        continue;
                    }
                    let batch: Vec<Box<dyn Resource>> =
                        queues.into_iter().map(|q| Box::new(q) as Box<dyn Resource>).collect();
                    total += batch.len();
                    let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                        service: service_type,
                        resources: batch,
                        progress: LoadProgress {
                            loaded_count: total,
                            total_count: None,
                            status_message: Some("Loading SQS queues…".to_string()),
                        },
                    });
                }
                Err(e) => {
                    let _ = event_tx.send(Event::ResourceLoadError {
                        service: service_type,
                        error: format!("Failed to list SQS queues: {}", e),
                    });
                    return Ok(());
                }
            }
        }

        // ── SNS topics ──────────────────────────────────────────────────
        let mut t_paginator = self.sns.list_topics().into_paginator().send();
        while let Some(result) = t_paginator.next().await {
            match result {
                Ok(page) => {
                    let arns: Vec<String> = page
                        .topics()
                        .iter()
                        .filter_map(|t| t.topic_arn().map(|s| s.to_string()))
                        .collect();
                    if arns.is_empty() {
                        continue;
                    }
                    let futs = arns.into_iter().map(|arn| {
                        let sns = self.sns.clone();
                        async move { fetch_topic(&sns, arn).await }
                    });
                    let topics: Vec<SnsTopic> = futures::future::join_all(futs)
                        .await
                        .into_iter()
                        .flatten()
                        .collect();
                    if topics.is_empty() {
                        continue;
                    }
                    let batch: Vec<Box<dyn Resource>> =
                        topics.into_iter().map(|t| Box::new(t) as Box<dyn Resource>).collect();
                    total += batch.len();
                    let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                        service: service_type,
                        resources: batch,
                        progress: LoadProgress {
                            loaded_count: total,
                            total_count: None,
                            status_message: Some("Loading SNS topics…".to_string()),
                        },
                    });
                }
                Err(e) => {
                    let _ = event_tx.send(Event::ResourceLoadError {
                        service: service_type,
                        error: format!("Failed to list SNS topics: {}", e),
                    });
                    return Ok(());
                }
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

// ── SqsQueue ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct SqsQueue {
    pub url: String,
    pub name: String,
    pub arn: String,
    pub is_fifo: bool,
    pub messages_available: i64,
    pub messages_in_flight: i64,
    pub messages_delayed: i64,
    pub visibility_timeout: Option<i64>,
    pub message_retention_secs: Option<i64>,
    pub max_message_size: Option<i64>,
    pub delay_seconds: Option<i64>,
    pub receive_wait_secs: Option<i64>,
    pub content_based_dedup: Option<bool>,
    pub kms_key_id: Option<String>,
    pub sse_sqs: bool,
    pub policy: Option<String>,
    pub created: String,
    pub last_modified: String,
    // Redrive (this queue → its DLQ)
    pub dlq_target_arn: Option<String>,
    pub max_receive_count: Option<i64>,
    // Redrive allow (which sources may use this queue as a DLQ)
    pub redrive_permission: Option<String>,
    pub source_queue_arns: Vec<String>,
    pub tags: HashMap<String, String>,
}

fn name_from_queue_url(url: &str) -> String {
    url.rsplit('/').next().unwrap_or(url).to_string()
}

fn parse_i64(attrs: &HashMap<QueueAttributeName, String>, key: QueueAttributeName) -> Option<i64> {
    attrs.get(&key).and_then(|v| v.parse::<i64>().ok())
}

async fn fetch_queue(sqs: &SqsClient, url: String) -> Option<SqsQueue> {
    let (attrs_res, tags_res) = tokio::join!(
        sqs.get_queue_attributes()
            .queue_url(&url)
            .attribute_names(QueueAttributeName::All)
            .send(),
        sqs.list_queue_tags().queue_url(&url).send(),
    );

    let attrs = match attrs_res {
        Ok(r) => r.attributes().cloned().unwrap_or_default(),
        Err(_) => HashMap::new(),
    };
    let tags = tags_res
        .ok()
        .and_then(|r| r.tags().cloned())
        .unwrap_or_default();

    let arn = attrs
        .get(&QueueAttributeName::QueueArn)
        .cloned()
        .unwrap_or_default();
    let name = if !arn.is_empty() {
        arn.rsplit(':').next().unwrap_or(&arn).to_string()
    } else {
        name_from_queue_url(&url)
    };

    // Parse redrive policies (stored as JSON strings).
    let (dlq_target_arn, max_receive_count) = attrs
        .get(&QueueAttributeName::RedrivePolicy)
        .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
        .map(|v| {
            (
                v.get("deadLetterTargetArn")
                    .and_then(|x| x.as_str())
                    .map(|s| s.to_string()),
                v.get("maxReceiveCount").and_then(|x| match x {
                    serde_json::Value::Number(n) => n.as_i64(),
                    serde_json::Value::String(s) => s.parse::<i64>().ok(),
                    _ => None,
                }),
            )
        })
        .unwrap_or((None, None));

    let (redrive_permission, source_queue_arns) = attrs
        .get(&QueueAttributeName::RedriveAllowPolicy)
        .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
        .map(|v| {
            let perm = v
                .get("redrivePermission")
                .and_then(|x| x.as_str())
                .map(|s| s.to_string());
            let sources = v
                .get("sourceQueueArns")
                .and_then(|x| x.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|x| x.as_str().map(|s| s.to_string()))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            (perm, sources)
        })
        .unwrap_or((None, vec![]));

    let fmt_epoch = |key: QueueAttributeName| -> String {
        parse_i64(&attrs, key)
            .map(crate::aws::services::cloudwatch::fmt_epoch_secs)
            .unwrap_or_default()
    };

    Some(SqsQueue {
        name,
        is_fifo: attrs
            .get(&QueueAttributeName::FifoQueue)
            .map(|v| v == "true")
            .unwrap_or(false),
        messages_available: parse_i64(&attrs, QueueAttributeName::ApproximateNumberOfMessages)
            .unwrap_or(0),
        messages_in_flight: parse_i64(
            &attrs,
            QueueAttributeName::ApproximateNumberOfMessagesNotVisible,
        )
        .unwrap_or(0),
        messages_delayed: parse_i64(
            &attrs,
            QueueAttributeName::ApproximateNumberOfMessagesDelayed,
        )
        .unwrap_or(0),
        visibility_timeout: parse_i64(&attrs, QueueAttributeName::VisibilityTimeout),
        message_retention_secs: parse_i64(&attrs, QueueAttributeName::MessageRetentionPeriod),
        max_message_size: parse_i64(&attrs, QueueAttributeName::MaximumMessageSize),
        delay_seconds: parse_i64(&attrs, QueueAttributeName::DelaySeconds),
        receive_wait_secs: parse_i64(&attrs, QueueAttributeName::ReceiveMessageWaitTimeSeconds),
        content_based_dedup: attrs
            .get(&QueueAttributeName::ContentBasedDeduplication)
            .map(|v| v == "true"),
        kms_key_id: attrs
            .get(&QueueAttributeName::KmsMasterKeyId)
            .filter(|s| !s.is_empty())
            .cloned(),
        sse_sqs: attrs
            .get(&QueueAttributeName::SqsManagedSseEnabled)
            .map(|v| v == "true")
            .unwrap_or(false),
        policy: attrs
            .get(&QueueAttributeName::Policy)
            .filter(|s| !s.is_empty())
            .cloned(),
        created: fmt_epoch(QueueAttributeName::CreatedTimestamp),
        last_modified: fmt_epoch(QueueAttributeName::LastModifiedTimestamp),
        dlq_target_arn,
        max_receive_count,
        redrive_permission,
        source_queue_arns,
        arn,
        url,
        tags,
    })
}

impl SqsQueue {
    /// Total messages across all states — the headline "backlog" number.
    pub fn total_messages(&self) -> i64 {
        self.messages_available + self.messages_in_flight + self.messages_delayed
    }

    /// Whether this queue can be used as a dead-letter target by other queues.
    pub fn is_dlq(&self) -> bool {
        self.redrive_permission.is_some() || !self.source_queue_arns.is_empty()
    }

    /// The access policy pretty-printed, when it parses as JSON.
    pub fn pretty_policy(&self) -> Option<String> {
        let raw = self.policy.as_ref()?;
        Some(
            serde_json::from_str::<serde_json::Value>(raw)
                .ok()
                .and_then(|v| serde_json::to_string_pretty(&v).ok())
                .unwrap_or_else(|| raw.clone()),
        )
    }
}

crate::sections! {
    pub enum SqsQueueDetailSection,
    pub static SQS_QUEUE_SECTIONS = [
        Attributes "Attributes",
        Redrive "Redrive",
        Permissions "Permissions",
        Tags "Tags",
    ]
}

impl Resource for SqsQueue {
    fn references(&self) -> Vec<(String, String)> {
        let mut v = Vec::new();
        let mut r = |l: &str, x: &str| v.push((l.to_string(), x.to_string()));
        if let Some(x) = &self.kms_key_id { r("KMS Key", x); }
        if let Some(x) = &self.dlq_target_arn { r("Dead Letter Queue", x); }
        for x in &self.source_queue_arns { r("Source Queue", x); }
        v
    }

    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&SQS_QUEUE_SECTIONS)
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws sqs get-queue-attributes --queue-url {} --attribute-names All",
            crate::aws::resource::shell_quote(&self.url)
        ))
    }

    fn id(&self) -> &str {
        &self.url
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn resource_type(&self) -> &str {
        "SQS Queue"
    }

    fn state(&self) -> ResourceState {
        ResourceState::stateless()
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!(
            "{} {} {}",
            self.name,
            if self.is_fifo { "fifo" } else { "standard" },
            if self.is_dlq() { "dlq dead-letter" } else { "" }
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Name".to_string(), self.name.clone()),
            (
                "Type".to_string(),
                if self.is_fifo { "FIFO".to_string() } else { "Standard".to_string() },
            ),
            ("Messages".to_string(), self.total_messages().to_string()),
        ]
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/sqs/v3/home?region={}#/queues/{}",
            region,
            region,
            urlencoding_minimal(&self.url)
        ))
    }
}

/// Minimal percent-encoding for the few characters that appear in a queue URL.
fn urlencoding_minimal(s: &str) -> String {
    s.replace(':', "%3A").replace('/', "%2F")
}

// ── SnsTopic ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct SnsTopic {
    pub arn: String,
    pub name: String,
    pub is_fifo: bool,
    pub display_name: Option<String>,
    pub subscriptions_confirmed: i64,
    pub subscriptions_pending: i64,
    pub subscriptions_deleted: i64,
    pub owner: Option<String>,
    pub kms_key_id: Option<String>,
    pub policy: Option<String>,
    pub effective_delivery_policy: Option<String>,
    pub tags: HashMap<String, String>,
}

impl SnsTopic {
    /// The access policy pretty-printed, when it parses as JSON.
    pub fn pretty_policy(&self) -> Option<String> {
        let raw = self.policy.as_ref()?;
        Some(
            serde_json::from_str::<serde_json::Value>(raw)
                .ok()
                .and_then(|v| serde_json::to_string_pretty(&v).ok())
                .unwrap_or_else(|| raw.clone()),
        )
    }
}

async fn fetch_topic(sns: &SnsClient, arn: String) -> Option<SnsTopic> {
    let (attrs_res, tags_res) = tokio::join!(
        sns.get_topic_attributes().topic_arn(&arn).send(),
        sns.list_tags_for_resource().resource_arn(&arn).send(),
    );

    let attrs = match attrs_res {
        Ok(r) => r.attributes().cloned().unwrap_or_default(),
        Err(_) => HashMap::new(),
    };
    let tags: HashMap<String, String> = tags_res
        .ok()
        .map(|r| {
            r.tags()
                .iter()
                .map(|t| (t.key().to_string(), t.value().to_string()))
                .collect()
        })
        .unwrap_or_default();

    let name = arn.rsplit(':').next().unwrap_or(&arn).to_string();
    let parse = |k: &str| attrs.get(k).and_then(|v| v.parse::<i64>().ok()).unwrap_or(0);

    Some(SnsTopic {
        is_fifo: attrs.get("FifoTopic").map(|v| v == "true").unwrap_or(false)
            || arn.ends_with(".fifo"),
        display_name: attrs.get("DisplayName").filter(|s| !s.is_empty()).cloned(),
        subscriptions_confirmed: parse("SubscriptionsConfirmed"),
        subscriptions_pending: parse("SubscriptionsPending"),
        subscriptions_deleted: parse("SubscriptionsDeleted"),
        owner: attrs.get("Owner").cloned(),
        kms_key_id: attrs.get("KmsMasterKeyId").filter(|s| !s.is_empty()).cloned(),
        policy: attrs.get("Policy").filter(|s| !s.is_empty()).cloned(),
        effective_delivery_policy: attrs
            .get("EffectiveDeliveryPolicy")
            .filter(|s| !s.is_empty())
            .cloned(),
        name,
        arn,
        tags,
    })
}

crate::sections! {
    pub enum SnsTopicDetailSection,
    pub static SNS_TOPIC_SECTIONS = [
        Config "Config",
        Subscriptions "Subscriptions" => crate::app::App::trigger_sns_subscriptions_load,
        Permissions "Permissions",
        Tags "Tags",
    ]
}

impl Resource for SnsTopic {
    fn references(&self) -> Vec<(String, String)> {
        let mut v = Vec::new();
        if let Some(x) = &self.kms_key_id { v.push(("KMS Key".to_string(), x.clone())); }
        v
    }

    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&SNS_TOPIC_SECTIONS)
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws sns get-topic-attributes --topic-arn {}",
            crate::aws::resource::shell_quote(self.id())
        ))
    }

    fn id(&self) -> &str {
        &self.arn
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn resource_type(&self) -> &str {
        "SNS Topic"
    }

    fn state(&self) -> ResourceState {
        ResourceState::stateless()
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!(
            "{} {}",
            self.name,
            if self.is_fifo { "fifo" } else { "standard" }
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Name".to_string(), self.name.clone()),
            (
                "Type".to_string(),
                if self.is_fifo { "FIFO".to_string() } else { "Standard".to_string() },
            ),
            (
                "Subscriptions".to_string(),
                self.subscriptions_confirmed.to_string(),
            ),
        ]
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/sns/v3/home?region={}#/topic/{}",
            region, region, self.arn
        ))
    }
}

// ── SNS subscriptions (lazy-loaded via list_subscriptions_by_topic) ───────────

#[derive(Debug, Clone)]
pub struct SnsSubscription {
    pub protocol: String,
    pub endpoint: String,
    pub confirmed: bool,
}

pub async fn fetch_topic_subscriptions(
    sns: SnsClient,
    topic_arn: String,
) -> Result<Vec<SnsSubscription>> {
    let mut subs = Vec::new();
    let mut paginator = sns
        .list_subscriptions_by_topic()
        .topic_arn(&topic_arn)
        .into_paginator()
        .send();
    while let Some(result) = paginator.next().await {
        let page = result.map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
        for s in page.subscriptions() {
            let sub_arn = s.subscription_arn().unwrap_or("");
            subs.push(SnsSubscription {
                protocol: s.protocol().unwrap_or("").to_string(),
                endpoint: s.endpoint().unwrap_or("").to_string(),
                confirmed: sub_arn != "PendingConfirmation" && !sub_arn.is_empty(),
            });
        }
    }
    Ok(subs)
}

// ── CloudWatch metrics (`m` overlay) ──────────────────────────────────────────

pub use crate::aws::services::ec2::MetricsTimeRange;

#[derive(Debug, Clone)]
pub struct SqsMetricsData {
    pub time_range: MetricsTimeRange,
    pub sent: Vec<(f64, f64)>,
    pub received: Vec<(f64, f64)>,
    pub deleted: Vec<(f64, f64)>,
    pub visible: Vec<(f64, f64)>,
    pub oldest_age: Vec<(f64, f64)>,
    pub empty_receives: Vec<(f64, f64)>,
    pub x_max: f64,
}

#[derive(Debug, Clone)]
pub enum SqsMetricsState {
    Loading,
    Loaded(SqsMetricsData),
}

/// Pull `AWS/SQS` metrics for one queue (dimension `QueueName`).
/// `ApproximateAgeOfOldestMessage` (max) is the key backlog/lag signal — a
/// rising age means consumers are falling behind.
pub async fn fetch_sqs_metrics(
    cw: aws_sdk_cloudwatch::Client,
    queue_name: String,
    time_range: MetricsTimeRange,
) -> Result<SqsMetricsData> {
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
            .name("QueueName")
            .value(&queue_name)
            .build()
    };
    let metric = |name: &'static str, stat: Statistic| {
        cw.get_metric_statistics()
            .namespace("AWS/SQS")
            .metric_name(name)
            .dimensions(dim())
            .start_time(start_dt)
            .end_time(end_dt)
            .period(period)
            .set_statistics(Some(vec![stat]))
            .send()
    };

    let (sent, received, deleted, visible, oldest, empty) = tokio::join!(
        metric("NumberOfMessagesSent", Statistic::Sum),
        metric("NumberOfMessagesReceived", Statistic::Sum),
        metric("NumberOfMessagesDeleted", Statistic::Sum),
        metric("ApproximateNumberOfMessagesVisible", Statistic::Average),
        metric("ApproximateAgeOfOldestMessage", Statistic::Maximum),
        metric("NumberOfEmptyReceives", Statistic::Sum),
    );

    Ok(SqsMetricsData {
        time_range,
        sent: parse_points(sent, start, |dp| dp.sum()),
        received: parse_points(received, start, |dp| dp.sum()),
        deleted: parse_points(deleted, start, |dp| dp.sum()),
        visible: parse_points(visible, start, |dp| dp.average()),
        oldest_age: parse_points(oldest, start, |dp| dp.maximum()),
        empty_receives: parse_points(empty, start, |dp| dp.sum()),
        x_max: time_range.duration_secs() as f64,
    })
}

#[derive(Debug, Clone)]
pub struct SnsMetricsData {
    pub time_range: MetricsTimeRange,
    pub published: Vec<(f64, f64)>,
    pub delivered: Vec<(f64, f64)>,
    pub failed: Vec<(f64, f64)>,
    pub filtered_out: Vec<(f64, f64)>,
    pub publish_size: Vec<(f64, f64)>,
    pub redriven_dlq: Vec<(f64, f64)>,
    pub x_max: f64,
}

#[derive(Debug, Clone)]
pub enum SnsMetricsState {
    Loading,
    Loaded(SnsMetricsData),
}

/// Pull `AWS/SNS` metrics for one topic (dimension `TopicName`).
/// `NumberOfNotificationsFailed` rising is the headline delivery-failure signal.
pub async fn fetch_sns_metrics(
    cw: aws_sdk_cloudwatch::Client,
    topic_name: String,
    time_range: MetricsTimeRange,
) -> Result<SnsMetricsData> {
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
            .name("TopicName")
            .value(&topic_name)
            .build()
    };
    let metric = |name: &'static str, stat: Statistic| {
        cw.get_metric_statistics()
            .namespace("AWS/SNS")
            .metric_name(name)
            .dimensions(dim())
            .start_time(start_dt)
            .end_time(end_dt)
            .period(period)
            .set_statistics(Some(vec![stat]))
            .send()
    };

    let (published, delivered, failed, filtered, size, redriven) = tokio::join!(
        metric("NumberOfMessagesPublished", Statistic::Sum),
        metric("NumberOfNotificationsDelivered", Statistic::Sum),
        metric("NumberOfNotificationsFailed", Statistic::Sum),
        metric("NumberOfNotificationsFilteredOut", Statistic::Sum),
        metric("PublishSize", Statistic::Average),
        metric("NumberOfNotificationsRedrivenToDlq", Statistic::Sum),
    );

    Ok(SnsMetricsData {
        time_range,
        published: parse_points(published, start, |dp| dp.sum()),
        delivered: parse_points(delivered, start, |dp| dp.sum()),
        failed: parse_points(failed, start, |dp| dp.sum()),
        filtered_out: parse_points(filtered, start, |dp| dp.sum()),
        publish_size: parse_points(size, start, |dp| dp.average()),
        redriven_dlq: parse_points(redriven, start, |dp| dp.sum()),
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
