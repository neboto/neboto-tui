use crate::aws::client::AwsClients;
use crate::aws::resource::{Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::aws::services::ec2::MetricsTimeRange;
use crate::error::Result;
use crate::event::{Event, LoadProgress};
use async_trait::async_trait;
use aws_sdk_opensearch::Client as OpenSearchClient;
use futures::stream::{self, StreamExt};
use std::any::Any;
use std::collections::HashMap;
use tokio::sync::mpsc;

/// OpenSearch service — single-list of domains with a split detail pane
/// (Details / Cluster / Network / Security / Tags). The streaming load is N+1:
/// `ListDomainNames` then a bounded-concurrency `DescribeDomain` per domain for
/// the full cluster/EBS/VPC/encryption picture. `m` charts the `AWS/ES` health
/// metrics (CPU, JVM memory pressure, free storage, cluster status, search +
/// indexing rate). Works for both OpenSearch and legacy Elasticsearch engines.
pub struct OpenSearchService {
    client: OpenSearchClient,
}

impl OpenSearchService {
    pub fn new(aws_clients: &AwsClients) -> Self {
        Self {
            client: aws_clients.opensearch_client(),
        }
    }
}

#[async_trait]
impl AwsService for OpenSearchService {
    fn service_type(&self) -> ServiceType {
        ServiceType::OpenSearch
    }

    fn name(&self) -> &str {
        "OpenSearch"
    }

    async fn list_resources(&self) -> Result<Vec<Box<dyn Resource>>> {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        self.list_resources_streaming(tx, ServiceType::OpenSearch)
            .await?;
        Ok(vec![])
    }

    async fn list_resources_streaming(
        &self,
        event_tx: mpsc::UnboundedSender<Event>,
        service_type: ServiceType,
    ) -> Result<()> {
        let names = match self.client.list_domain_names().send().await {
            Ok(r) => r
                .domain_names()
                .iter()
                .filter_map(|d| d.domain_name().map(|s| s.to_string()))
                .collect::<Vec<_>>(),
            Err(e) => {
                let _ = event_tx.send(Event::ResourceLoadError {
                    service: service_type,
                    error: format!(
                        "Failed to list OpenSearch domains: {}",
                        crate::error::sdk_error_message(&e)
                    ),
                });
                return Ok(());
            }
        };

        if names.is_empty() {
            let _ = event_tx.send(Event::ResourcesFullyLoaded {
                service: service_type,
                total_count: 0,
            });
            return Ok(());
        }

        // DescribeDomain per name, bounded concurrency.
        let domains: Vec<OpenSearchDomain> = stream::iter(names)
            .map(|name| {
                let client = self.client.clone();
                async move {
                    client
                        .describe_domain()
                        .domain_name(&name)
                        .send()
                        .await
                        .ok()
                        .and_then(|r| r.domain_status().cloned())
                        .map(|s| OpenSearchDomain::from_sdk(&s))
                }
            })
            .buffer_unordered(8)
            .filter_map(|d| async move { d })
            .collect()
            .await;

        let total = domains.len();
        let batch: Vec<Box<dyn Resource>> = domains
            .into_iter()
            .map(|d| Box::new(d) as Box<dyn Resource>)
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
pub struct OpenSearchDomain {
    pub domain_name: String,
    pub domain_id: String,
    pub arn: String,
    /// Account id, parsed from the ARN — the `ClientId` CloudWatch dimension.
    pub client_id: String,
    pub engine_version: String,
    pub processing_status: Option<String>,
    pub created: bool,
    pub deleted: bool,
    pub processing: bool,
    pub endpoint: Option<String>,
    pub vpc_endpoints: Vec<String>,
    pub auto_tune: Option<String>,
    pub snapshot_hour: Option<i32>,

    // Cluster
    pub instance_type: String,
    pub instance_count: i32,
    pub dedicated_master_enabled: bool,
    pub dedicated_master_type: Option<String>,
    pub dedicated_master_count: Option<i32>,
    pub zone_awareness_enabled: bool,
    pub availability_zone_count: Option<i32>,
    pub warm_enabled: bool,
    pub warm_type: Option<String>,
    pub warm_count: Option<i32>,
    pub cold_storage_enabled: bool,
    pub multi_az_standby: bool,
    pub ebs_enabled: bool,
    pub volume_type: Option<String>,
    pub volume_size: Option<i32>,
    pub volume_iops: Option<i32>,
    pub volume_throughput: Option<i32>,

    // Network
    pub vpc_id: Option<String>,
    pub subnet_ids: Vec<String>,
    pub security_group_ids: Vec<String>,
    pub availability_zones: Vec<String>,
    pub enforce_https: Option<bool>,
    pub tls_policy: Option<String>,
    pub custom_endpoint: Option<String>,

    // Security
    pub encryption_at_rest: bool,
    pub kms_key_id: Option<String>,
    pub node_to_node_encryption: bool,
    pub fine_grained_access: bool,
    pub internal_user_db: bool,
    pub cognito_enabled: bool,
    pub cognito_user_pool_id: Option<String>,
    pub access_policies: Option<String>,

    pub tags: HashMap<String, String>,
}

impl OpenSearchDomain {
    pub fn from_sdk(s: &aws_sdk_opensearch::types::DomainStatus) -> Self {
        let arn = s.arn().to_string();
        let client_id = arn.split(':').nth(4).unwrap_or_default().to_string();

        let cc = s.cluster_config();
        let ebs = s.ebs_options();
        let vpc = s.vpc_options();
        let dep = s.domain_endpoint_options();
        let ear = s.encryption_at_rest_options();
        let n2n = s.node_to_node_encryption_options();
        let adv = s.advanced_security_options();
        let cog = s.cognito_options();

        let vpc_endpoints = s
            .endpoints()
            .map(|m| {
                let mut v: Vec<String> = m.iter().map(|(k, val)| format!("{}: {}", k, val)).collect();
                v.sort();
                v
            })
            .unwrap_or_default();

        Self {
            domain_name: s.domain_name().to_string(),
            domain_id: s.domain_id().to_string(),
            client_id,
            engine_version: s.engine_version().unwrap_or_default().to_string(),
            processing_status: s.domain_processing_status().map(|p| p.as_str().to_string()),
            created: s.created().unwrap_or(false),
            deleted: s.deleted().unwrap_or(false),
            processing: s.processing().unwrap_or(false),
            endpoint: s.endpoint().map(|e| e.to_string()),
            vpc_endpoints,
            auto_tune: s
                .auto_tune_options()
                .and_then(|a| a.state())
                .map(|st| st.as_str().to_string()),
            snapshot_hour: s.snapshot_options().and_then(|o| o.automated_snapshot_start_hour()),

            instance_type: cc
                .and_then(|c| c.instance_type())
                .map(|t| t.as_str().to_string())
                .unwrap_or_default(),
            instance_count: cc.and_then(|c| c.instance_count()).unwrap_or(0),
            dedicated_master_enabled: cc
                .and_then(|c| c.dedicated_master_enabled())
                .unwrap_or(false),
            dedicated_master_type: cc
                .and_then(|c| c.dedicated_master_type())
                .map(|t| t.as_str().to_string()),
            dedicated_master_count: cc.and_then(|c| c.dedicated_master_count()),
            zone_awareness_enabled: cc.and_then(|c| c.zone_awareness_enabled()).unwrap_or(false),
            availability_zone_count: cc
                .and_then(|c| c.zone_awareness_config())
                .and_then(|z| z.availability_zone_count()),
            warm_enabled: cc.and_then(|c| c.warm_enabled()).unwrap_or(false),
            warm_type: cc
                .and_then(|c| c.warm_type())
                .map(|t| t.as_str().to_string()),
            warm_count: cc.and_then(|c| c.warm_count()),
            cold_storage_enabled: cc
                .and_then(|c| c.cold_storage_options())
                .map(|cs| cs.enabled())
                .unwrap_or(false),
            multi_az_standby: cc
                .and_then(|c| c.multi_az_with_standby_enabled())
                .unwrap_or(false),
            ebs_enabled: ebs.and_then(|e| e.ebs_enabled()).unwrap_or(false),
            volume_type: ebs
                .and_then(|e| e.volume_type())
                .map(|t| t.as_str().to_string()),
            volume_size: ebs.and_then(|e| e.volume_size()),
            volume_iops: ebs.and_then(|e| e.iops()),
            volume_throughput: ebs.and_then(|e| e.throughput()),

            vpc_id: vpc.and_then(|v| v.vpc_id()).map(|s| s.to_string()),
            subnet_ids: vpc
                .map(|v| v.subnet_ids().to_vec())
                .unwrap_or_default(),
            security_group_ids: vpc
                .map(|v| v.security_group_ids().to_vec())
                .unwrap_or_default(),
            availability_zones: vpc
                .map(|v| v.availability_zones().to_vec())
                .unwrap_or_default(),
            enforce_https: dep.and_then(|d| d.enforce_https()),
            tls_policy: dep
                .and_then(|d| d.tls_security_policy())
                .map(|t| t.as_str().to_string()),
            custom_endpoint: dep
                .filter(|d| d.custom_endpoint_enabled().unwrap_or(false))
                .and_then(|d| d.custom_endpoint())
                .map(|s| s.to_string()),

            encryption_at_rest: ear.and_then(|e| e.enabled()).unwrap_or(false),
            kms_key_id: ear.and_then(|e| e.kms_key_id()).map(|s| s.to_string()),
            node_to_node_encryption: n2n.and_then(|n| n.enabled()).unwrap_or(false),
            fine_grained_access: adv.and_then(|a| a.enabled()).unwrap_or(false),
            internal_user_db: adv
                .and_then(|a| a.internal_user_database_enabled())
                .unwrap_or(false),
            cognito_enabled: cog.and_then(|c| c.enabled()).unwrap_or(false),
            cognito_user_pool_id: cog.and_then(|c| c.user_pool_id()).map(|s| s.to_string()),
            access_policies: s
                .access_policies()
                .filter(|p| !p.is_empty())
                .map(|s| s.to_string()),

            arn,
            tags: HashMap::new(),
        }
    }

    /// Console-style domain health summary.
    pub fn status_label(&self) -> String {
        if self.deleted {
            return "Deleting".to_string();
        }
        if let Some(s) = &self.processing_status {
            return s.clone();
        }
        if self.processing {
            return "Processing".to_string();
        }
        if self.created {
            "Active".to_string()
        } else {
            "Creating".to_string()
        }
    }
}

crate::sections! {
    pub enum OpenSearchDetailSection,
    pub static OPENSEARCH_SECTIONS = [
        Details "Details",
        Cluster "Cluster",
        Network "Network",
        Security "Security",
        Tags "Tags" => crate::app::App::trigger_opensearch_tags_load,
    ]
}

impl Resource for OpenSearchDomain {
    fn security_group_ids(&self) -> Vec<String> {
        self.security_group_ids.clone()
    }

    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&OPENSEARCH_SECTIONS)
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws opensearch describe-domain --domain-name {}",
            crate::aws::resource::shell_quote(self.id())
        ))
    }

    fn id(&self) -> &str {
        &self.domain_name
    }

    fn name(&self) -> &str {
        &self.domain_name
    }

    fn resource_type(&self) -> &str {
        "OpenSearch Domain"
    }

    fn state(&self) -> ResourceState {
        if self.deleted {
            return ResourceState::Deleting;
        }
        match self.processing_status.as_deref() {
            Some("Active") => ResourceState::Available,
            Some("Creating") => ResourceState::Creating,
            Some("Deleting") => ResourceState::Deleting,
            Some("Isolated") => ResourceState::Unavailable,
            Some(other) if !other.is_empty() => ResourceState::Pending, // Modifying/Upgrading/Updating
            _ => {
                if self.processing {
                    ResourceState::Pending
                } else if self.created {
                    ResourceState::Available
                } else {
                    ResourceState::Creating
                }
            }
        }
    }

    fn state_label(&self) -> String {
        self.status_label().to_lowercase()
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!(
            "{} {} {} {} {}",
            self.domain_name, self.domain_id, self.engine_version, self.instance_type, self.arn,
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Name".to_string(), self.domain_name.clone()),
            ("Engine".to_string(), self.engine_version.clone()),
            ("Status".to_string(), self.status_label()),
            ("Instance Type".to_string(), self.instance_type.clone()),
            ("Instances".to_string(), self.instance_count.to_string()),
            (
                "Endpoint".to_string(),
                self.endpoint.clone().unwrap_or_else(|| "—".to_string()),
            ),
        ]
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/aos/home?region={}#opensearch/domains/{}",
            region, region, self.domain_name
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── Tags (lazy section) ───────────────────────────────────────────────────────

/// `ListTags` for one domain (keyed by ARN).
pub async fn fetch_opensearch_tags(
    client: OpenSearchClient,
    arn: String,
) -> Result<Vec<(String, String)>> {
    let resp = client
        .list_tags()
        .arn(&arn)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
    let mut out: Vec<(String, String)> = resp
        .tag_list()
        .iter()
        .map(|t| (t.key().to_string(), t.value().to_string()))
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

// ── CloudWatch metrics (`m` overlay) ──────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct OpenSearchMetricsData {
    pub time_range: MetricsTimeRange,
    pub cpu: Vec<(f64, f64)>,
    pub jvm_memory_pressure: Vec<(f64, f64)>,
    pub free_storage: Vec<(f64, f64)>,
    pub cluster_status_red: Vec<(f64, f64)>,
    pub search_rate: Vec<(f64, f64)>,
    pub indexing_rate: Vec<(f64, f64)>,
    pub x_max: f64,
}

#[derive(Debug, Clone)]
pub enum OpenSearchMetricsState {
    Loading,
    Loaded(OpenSearchMetricsData),
}

/// Pull `AWS/ES` metrics for one domain. The namespace is still `AWS/ES` for
/// OpenSearch, and the metrics are dimensioned by BOTH `ClientId` (account) and
/// `DomainName`. JVM memory pressure and free storage are the cluster-health
/// early-warning signals; `ClusterStatus.red` flags unavailable primaries.
pub async fn fetch_opensearch_metrics(
    cw: aws_sdk_cloudwatch::Client,
    domain_name: String,
    client_id: String,
    time_range: MetricsTimeRange,
) -> Result<OpenSearchMetricsData> {
    use aws_sdk_cloudwatch::types::{Dimension, Statistic};

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let start = now - time_range.duration_secs();
    let period = time_range.period_secs();
    let start_dt = aws_sdk_cloudwatch::primitives::DateTime::from_secs(start);
    let end_dt = aws_sdk_cloudwatch::primitives::DateTime::from_secs(now);

    let dims = || {
        vec![
            Dimension::builder()
                .name("ClientId")
                .value(&client_id)
                .build(),
            Dimension::builder()
                .name("DomainName")
                .value(&domain_name)
                .build(),
        ]
    };
    let metric = |name: &'static str, stat: Statistic| {
        cw.get_metric_statistics()
            .namespace("AWS/ES")
            .metric_name(name)
            .set_dimensions(Some(dims()))
            .start_time(start_dt)
            .end_time(end_dt)
            .period(period)
            .set_statistics(Some(vec![stat]))
            .send()
    };

    let (cpu, jvm, storage, red, search, index) = tokio::join!(
        metric("CPUUtilization", Statistic::Average),
        metric("JVMMemoryPressure", Statistic::Maximum),
        metric("FreeStorageSpace", Statistic::Minimum),
        metric("ClusterStatus.red", Statistic::Maximum),
        metric("SearchRate", Statistic::Average),
        metric("IndexingRate", Statistic::Average),
    );

    Ok(OpenSearchMetricsData {
        time_range,
        cpu: parse_points(cpu, start, |dp| dp.average()),
        jvm_memory_pressure: parse_points(jvm, start, |dp| dp.maximum()),
        free_storage: parse_points(storage, start, |dp| dp.minimum()),
        cluster_status_red: parse_points(red, start, |dp| dp.maximum()),
        search_rate: parse_points(search, start, |dp| dp.average()),
        indexing_rate: parse_points(index, start, |dp| dp.average()),
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
