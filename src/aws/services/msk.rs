use crate::aws::client::AwsClients;
use crate::aws::resource::{native_state_label, Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::aws::services::ec2::MetricsTimeRange;
use crate::error::Result;
use crate::event::{Event, LoadProgress};
use async_trait::async_trait;
use aws_sdk_cloudwatch::primitives::DateTime as CwDateTime;
use aws_sdk_kafka::Client as KafkaClient;
use std::any::Any;
use std::collections::HashMap;
use tokio::sync::mpsc;

/// Amazon MSK (Managed Streaming for Kafka) — single-list of clusters
/// (provisioned + serverless) with a split detail pane
/// (Overview / Networking / Monitoring / Config / Tags). The load is a single
/// paginated `ListClustersV2`, which already returns the full nested
/// provisioned/serverless config — no N+1 describe. The **Config** section is
/// the one lazy fetch: `DescribeConfigurationRevision` for the cluster's applied
/// Kafka configuration file (keyed by cluster ARN). `m` charts `AWS/Kafka`
/// health metrics — topic/partition counts, cluster-wide bytes in/out
/// (aggregated across per-broker series with the SEARCH/SUM pattern), and the
/// worst-broker data-logs disk usage.
pub struct MskService {
    client: KafkaClient,
}

impl MskService {
    pub fn new(aws_clients: &AwsClients) -> Self {
        Self {
            client: aws_clients.kafka_client(),
        }
    }
}

#[async_trait]
impl AwsService for MskService {
    fn service_type(&self) -> ServiceType {
        ServiceType::Msk
    }

    fn name(&self) -> &str {
        "MSK"
    }

    async fn list_resources(&self) -> Result<Vec<Box<dyn Resource>>> {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        self.list_resources_streaming(tx, ServiceType::Msk).await?;
        Ok(vec![])
    }

    async fn list_resources_streaming(
        &self,
        event_tx: mpsc::UnboundedSender<Event>,
        service_type: ServiceType,
    ) -> Result<()> {
        let mut pager = self
            .client
            .list_clusters_v2()
            .into_paginator()
            .items()
            .send();

        let mut clusters: Vec<MskCluster> = Vec::new();
        while let Some(item) = pager.next().await {
            match item {
                Ok(c) => clusters.push(MskCluster::from_sdk(&c)),
                Err(e) => {
                    let _ = event_tx.send(Event::ResourceLoadError {
                        service: service_type,
                        error: format!(
                            "Failed to list MSK clusters: {}",
                            crate::error::sdk_error_message(&e)
                        ),
                    });
                    return Ok(());
                }
            }
        }

        let total = clusters.len();
        if total > 0 {
            let batch: Vec<Box<dyn Resource>> = clusters
                .into_iter()
                .map(|c| Box::new(c) as Box<dyn Resource>)
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
pub struct MskCluster {
    pub arn: String,
    pub name: String,
    pub cluster_type: String, // PROVISIONED / SERVERLESS
    pub state: String,
    pub state_message: Option<String>,
    pub kafka_version: Option<String>,
    pub created: Option<String>,
    pub tags: HashMap<String, String>,

    // Provisioned specifics (None for serverless).
    pub broker_count: Option<i32>,
    pub instance_type: Option<String>,
    pub volume_size: Option<i32>,
    pub storage_mode: Option<String>,
    pub enhanced_monitoring: Option<String>,
    pub zookeeper: Option<String>,
    pub az_distribution: Option<String>,
    pub public_access: Option<String>,
    pub prometheus_jmx: bool,
    pub prometheus_node: bool,
    /// Applied configuration for the lazy Config section.
    pub config_arn: Option<String>,
    pub config_revision: Option<i64>,

    // Networking (both cluster types).
    pub subnets: Vec<String>,
    pub security_groups: Vec<String>,
    pub zone_ids: Vec<String>,

    // Authentication.
    pub auth_iam: bool,
    pub auth_scram: bool,
    pub auth_tls: bool,
    pub auth_unauthenticated: bool,

    // Encryption.
    pub encryption_at_rest_kms: Option<String>,
    pub encryption_in_transit_client: Option<String>, // TLS / PLAINTEXT / TLS_PLAINTEXT
    pub encryption_in_cluster: Option<bool>,
}

impl MskCluster {
    pub fn from_sdk(c: &aws_sdk_kafka::types::Cluster) -> Self {
        let arn = c.cluster_arn().unwrap_or_default().to_string();
        let name = c.cluster_name().unwrap_or_default().to_string();
        let cluster_type = c
            .cluster_type()
            .map(|t| t.as_str().to_string())
            .unwrap_or_default();
        let state = c
            .state()
            .map(|s| s.as_str().to_string())
            .unwrap_or_default();
        let state_message = c
            .state_info()
            .and_then(|s| s.message())
            .filter(|m| !m.is_empty())
            .map(|m| m.to_string());
        let created = c
            .creation_time()
            .map(|t| crate::aws::services::cloudwatch::fmt_epoch_secs(t.secs()));
        let tags = c.tags().cloned().unwrap_or_default();

        let mut out = Self {
            arn,
            name,
            cluster_type,
            state,
            state_message,
            kafka_version: None,
            created,
            tags,
            broker_count: None,
            instance_type: None,
            volume_size: None,
            storage_mode: None,
            enhanced_monitoring: None,
            zookeeper: None,
            az_distribution: None,
            public_access: None,
            prometheus_jmx: false,
            prometheus_node: false,
            config_arn: None,
            config_revision: None,
            subnets: Vec::new(),
            security_groups: Vec::new(),
            zone_ids: Vec::new(),
            auth_iam: false,
            auth_scram: false,
            auth_tls: false,
            auth_unauthenticated: false,
            encryption_at_rest_kms: None,
            encryption_in_transit_client: None,
            encryption_in_cluster: None,
        };

        if let Some(p) = c.provisioned() {
            out.broker_count = p.number_of_broker_nodes();
            out.zookeeper = p
                .zookeeper_connect_string()
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string());
            out.storage_mode = p.storage_mode().map(|s| s.as_str().to_string());
            out.enhanced_monitoring = p.enhanced_monitoring().map(|m| m.as_str().to_string());
            out.kafka_version = p
                .current_broker_software_info()
                .and_then(|b| b.kafka_version())
                .map(|s| s.to_string());
            if let Some(b) = p.current_broker_software_info() {
                out.config_arn = b
                    .configuration_arn()
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string());
                out.config_revision = b.configuration_revision();
            }

            if let Some(bng) = p.broker_node_group_info() {
                out.instance_type = bng.instance_type().map(|s| s.to_string());
                out.subnets = bng.client_subnets().to_vec();
                out.security_groups = bng.security_groups().to_vec();
                out.zone_ids = bng.zone_ids().to_vec();
                out.az_distribution = bng
                    .broker_az_distribution()
                    .map(|d| d.as_str().to_string());
                out.volume_size = bng
                    .storage_info()
                    .and_then(|s| s.ebs_storage_info())
                    .and_then(|e| e.volume_size());
                out.public_access = bng
                    .connectivity_info()
                    .and_then(|ci| ci.public_access())
                    .and_then(|pa| pa.r#type())
                    .map(|s| s.to_string());
            }

            if let Some(auth) = p.client_authentication() {
                out.auth_iam = auth.sasl().and_then(|s| s.iam()).and_then(|i| i.enabled()).unwrap_or(false);
                out.auth_scram = auth.sasl().and_then(|s| s.scram()).and_then(|s| s.enabled()).unwrap_or(false);
                out.auth_tls = auth.tls().and_then(|t| t.enabled()).unwrap_or(false);
                out.auth_unauthenticated = auth
                    .unauthenticated()
                    .and_then(|u| u.enabled())
                    .unwrap_or(false);
            }

            if let Some(enc) = p.encryption_info() {
                out.encryption_at_rest_kms = enc
                    .encryption_at_rest()
                    .and_then(|e| e.data_volume_kms_key_id())
                    .map(|s| s.to_string());
                if let Some(it) = enc.encryption_in_transit() {
                    out.encryption_in_transit_client =
                        it.client_broker().map(|c| c.as_str().to_string());
                    out.encryption_in_cluster = it.in_cluster();
                }
            }

            if let Some(prom) = p.open_monitoring().and_then(|o| o.prometheus()) {
                out.prometheus_jmx = prom
                    .jmx_exporter()
                    .and_then(|j| j.enabled_in_broker())
                    .unwrap_or(false);
                out.prometheus_node = prom
                    .node_exporter()
                    .and_then(|n| n.enabled_in_broker())
                    .unwrap_or(false);
            }
        }

        if let Some(s) = c.serverless() {
            // Serverless clusters expose only VPC configs + SASL/IAM auth.
            for vc in s.vpc_configs() {
                out.subnets.extend(vc.subnet_ids().iter().cloned());
                out.security_groups
                    .extend(vc.security_group_ids().iter().cloned());
            }
            out.auth_iam = s
                .client_authentication()
                .and_then(|a| a.sasl())
                .and_then(|sasl| sasl.iam())
                .and_then(|i| i.enabled())
                .unwrap_or(false);
        }

        out
    }
}

/// Map an MSK `ClusterState` to the shared resource-state palette.
pub fn msk_state(state: &str) -> ResourceState {
    match state {
        "ACTIVE" => ResourceState::Available,
        "CREATING" => ResourceState::Creating,
        "DELETING" => ResourceState::Deleting,
        "FAILED" => ResourceState::Unavailable,
        "HEALING" | "MAINTENANCE" | "REBOOTING_BROKER" | "UPDATING" => ResourceState::Pending,
        "" => ResourceState::Unknown("Unknown".to_string()),
        other => ResourceState::Unknown(other.to_string()),
    }
}

crate::sections! {
    pub enum MskDetailSection,
    pub static MSK_SECTIONS = [
        Overview "Overview",
        Networking "Networking",
        Monitoring "Monitoring",
        Config "Config" => crate::app::App::trigger_msk_config_load,
        Tags "Tags",
    ]
}

impl Resource for MskCluster {
    fn security_group_ids(&self) -> Vec<String> {
        self.security_groups.clone()
    }

    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&MSK_SECTIONS)
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws kafka describe-cluster-v2 --cluster-arn {}",
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
        "MSK Cluster"
    }

    fn state(&self) -> ResourceState {
        msk_state(&self.state)
    }

    fn state_label(&self) -> String {
        native_state_label(&self.state, || self.state())
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!(
            "{} {} {} {}",
            self.name,
            self.cluster_type,
            self.kafka_version.as_deref().unwrap_or(""),
            self.arn,
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Name".to_string(), self.name.clone()),
            ("Type".to_string(), self.cluster_type.clone()),
            ("State".to_string(), self.state.clone()),
            (
                "Kafka".to_string(),
                self.kafka_version.clone().unwrap_or_else(|| "—".to_string()),
            ),
            (
                "Brokers".to_string(),
                self.broker_count
                    .map(|b| b.to_string())
                    .unwrap_or_else(|| "—".to_string()),
            ),
        ]
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/msk/home?region={}#/cluster/{}/view",
            region, region, self.arn
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── Config (lazy section — DescribeConfigurationRevision) ─────────────────────

#[derive(Debug, Clone)]
pub struct MskConfigRevision {
    pub revision: i64,
    pub description: Option<String>,
    pub created: Option<String>,
    /// The Kafka `server.properties` file, split into lines for rendering.
    pub properties: Vec<String>,
}

/// Fetch the applied Kafka configuration file for one cluster's config revision.
pub async fn fetch_msk_config(
    client: KafkaClient,
    config_arn: String,
    revision: i64,
) -> Result<MskConfigRevision> {
    let resp = client
        .describe_configuration_revision()
        .arn(&config_arn)
        .revision(revision)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;

    let properties = resp
        .server_properties()
        .map(|b| String::from_utf8_lossy(b.as_ref()).to_string())
        .map(|s| {
            s.lines()
                .filter(|l| !l.trim().is_empty())
                .map(|l| l.to_string())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    Ok(MskConfigRevision {
        revision: resp.revision().unwrap_or(revision),
        description: resp
            .description()
            .filter(|d| !d.is_empty())
            .map(|d| d.to_string()),
        created: resp
            .creation_time()
            .map(|t| crate::aws::services::cloudwatch::fmt_epoch_secs(t.secs())),
        properties,
    })
}

// ── CloudWatch metrics (`m` overlay — AWS/Kafka) ──────────────────────────────

#[derive(Debug, Clone)]
pub struct MskMetricsData {
    pub time_range: MetricsTimeRange,
    pub topics: Vec<(f64, f64)>,
    pub partitions: Vec<(f64, f64)>,
    pub bytes_in: Vec<(f64, f64)>,
    pub bytes_out: Vec<(f64, f64)>,
    pub disk_used: Vec<(f64, f64)>,
    pub x_max: f64,
}

#[derive(Debug, Clone)]
pub enum MskMetricsState {
    Loading,
    Loaded(MskMetricsData),
}

/// Pull `AWS/Kafka` metrics for one cluster. `GlobalTopicCount` /
/// `GlobalPartitionCount` are cluster-level (dim `Cluster Name` only), but
/// `BytesInPerSec` / `BytesOutPerSec` / `KafkaDataLogsDiskUsed` are published
/// **per broker** (dim `Broker ID`), so — like Network Firewall's per-AZ series —
/// we aggregate every broker's series with `GetMetricData` SEARCH: SUM for the
/// throughput totals, MAX for the worst-broker disk usage. The schema form
/// (`{AWS/Kafka,"Cluster Name","Broker ID"}`) restricts the SEARCH to broker-level
/// series so per-topic series don't double-count.
pub async fn fetch_msk_metrics(
    cw: aws_sdk_cloudwatch::Client,
    cluster_name: String,
    time_range: MetricsTimeRange,
) -> Result<MskMetricsData> {
    use aws_sdk_cloudwatch::types::MetricDataQuery;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let start = now - time_range.duration_secs();
    let period = time_range.period_secs();

    // (id, aggregate, schema, metric)
    let specs = [
        ("m0", "MAX", "\"Cluster Name\"", "GlobalTopicCount"),
        ("m1", "MAX", "\"Cluster Name\"", "GlobalPartitionCount"),
        ("m2", "SUM", "\"Cluster Name\",\"Broker ID\"", "BytesInPerSec"),
        ("m3", "SUM", "\"Cluster Name\",\"Broker ID\"", "BytesOutPerSec"),
        ("m4", "MAX", "\"Cluster Name\",\"Broker ID\"", "KafkaDataLogsDiskUsed"),
    ];

    let queries: Vec<MetricDataQuery> = specs
        .iter()
        .map(|(id, agg, schema, metric)| {
            let expr = format!(
                "{}(SEARCH('{{AWS/Kafka,{}}} MetricName=\"{}\" \"Cluster Name\"=\"{}\"', 'Average', {}))",
                agg, schema, metric, cluster_name, period
            );
            MetricDataQuery::builder().id(*id).expression(expr).build()
        })
        .collect();

    let resp = cw
        .get_metric_data()
        .set_metric_data_queries(Some(queries))
        .start_time(CwDateTime::from_secs(start))
        .end_time(CwDateTime::from_secs(now))
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
    Ok(MskMetricsData {
        time_range,
        topics: take("m0"),
        partitions: take("m1"),
        bytes_in: take("m2"),
        bytes_out: take("m3"),
        disk_used: take("m4"),
        x_max: time_range.duration_secs() as f64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cluster_state_maps() {
        assert!(matches!(msk_state("ACTIVE"), ResourceState::Available));
        assert!(matches!(msk_state("FAILED"), ResourceState::Unavailable));
        assert!(matches!(msk_state("UPDATING"), ResourceState::Pending));
        assert!(matches!(msk_state("CREATING"), ResourceState::Creating));
        assert!(matches!(msk_state("ZZZ"), ResourceState::Unknown(_)));
    }
}
