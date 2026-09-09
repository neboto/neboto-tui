use crate::aws::client::AwsClients;
use crate::aws::resource::{native_state_label, Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::aws::services::ec2::MetricsTimeRange;
use crate::error::Result;
use crate::event::{Event, LoadProgress};
use async_trait::async_trait;
use aws_sdk_elasticache::Client as ElastiCacheClient;
use std::any::Any;
use std::collections::HashMap;
use tokio::sync::mpsc;

/// ElastiCache service — single-list of *logical caches*: Redis OSS / Valkey
/// replication groups and standalone Memcached (or legacy single-node Redis)
/// cache clusters. Both shapes collapse into one `ElastiCacheCluster` row with a
/// split detail pane (Details / Nodes / Network / Tags). Tags need a separate
/// `ListTagsForResource` call, so they're the one lazy section; `m` charts the
/// `AWS/ElastiCache` engine/memory/connection metrics for the primary node.
pub struct ElastiCacheService {
    client: ElastiCacheClient,
}

impl ElastiCacheService {
    pub fn new(aws_clients: &AwsClients) -> Self {
        Self {
            client: aws_clients.elasticache_client(),
        }
    }
}

#[async_trait]
impl AwsService for ElastiCacheService {
    fn service_type(&self) -> ServiceType {
        ServiceType::ElastiCache
    }

    fn name(&self) -> &str {
        "ElastiCache"
    }

    async fn list_resources(&self) -> Result<Vec<Box<dyn Resource>>> {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        self.list_resources_streaming(tx, ServiceType::ElastiCache)
            .await?;
        Ok(vec![])
    }

    async fn list_resources_streaming(
        &self,
        event_tx: mpsc::UnboundedSender<Event>,
        service_type: ServiceType,
    ) -> Result<()> {
        // Phase 1: every cache cluster (node-level info included). This is both
        // the source of standalone Memcached rows AND the enrichment map for
        // replication-group rows (engine/version/subnet group/params/SGs live on
        // the member cache clusters, not on the replication group itself).
        let mut clusters: Vec<aws_sdk_elasticache::types::CacheCluster> = Vec::new();
        let mut paginator = self
            .client
            .describe_cache_clusters()
            .show_cache_node_info(true)
            .into_paginator()
            .items()
            .send();
        while let Some(item) = paginator.next().await {
            match item {
                Ok(cc) => clusters.push(cc),
                Err(e) => {
                    let _ = event_tx.send(Event::ResourceLoadError {
                        service: service_type,
                        error: format!(
                            "Failed to list ElastiCache clusters: {}",
                            crate::error::sdk_error_message(&e)
                        ),
                    });
                    return Ok(());
                }
            }
        }

        let cluster_map: HashMap<String, aws_sdk_elasticache::types::CacheCluster> = clusters
            .iter()
            .filter_map(|c| c.cache_cluster_id().map(|id| (id.to_string(), c.clone())))
            .collect();

        let mut total = 0usize;

        // Phase 2: replication groups (Redis OSS / Valkey caches).
        let mut rg_paginator = self
            .client
            .describe_replication_groups()
            .into_paginator()
            .items()
            .send();
        let mut rg_batch: Vec<Box<dyn Resource>> = Vec::new();
        while let Some(item) = rg_paginator.next().await {
            match item {
                Ok(rg) => {
                    rg_batch.push(Box::new(ElastiCacheCluster::from_replication_group(
                        &rg,
                        &cluster_map,
                    )) as Box<dyn Resource>);
                }
                Err(e) => {
                    // Replication groups are optional (a pure-Memcached account
                    // has none, and the permission may be missing); don't abort
                    // the whole load over it.
                    let _ = event_tx.send(Event::ResourceLoadWarning {
                        service: service_type,
                        warning: format!(
                            "replication groups: {}",
                            crate::error::sdk_error_message(&e)
                        ),
                    });
                    break;
                }
            }
        }
        if !rg_batch.is_empty() {
            total += rg_batch.len();
            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                service: service_type,
                resources: rg_batch,
                progress: LoadProgress {
                    loaded_count: total,
                    total_count: None,
                    status_message: None,
                },
            });
        }

        // Phase 3: standalone cache clusters (Memcached / legacy single-node
        // Redis) — those not owned by a replication group.
        let standalone: Vec<Box<dyn Resource>> = clusters
            .iter()
            .filter(|c| c.replication_group_id().is_none())
            .map(|c| Box::new(ElastiCacheCluster::from_cache_cluster(c)) as Box<dyn Resource>)
            .collect();
        if !standalone.is_empty() {
            total += standalone.len();
            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                service: service_type,
                resources: standalone,
                progress: LoadProgress {
                    loaded_count: total,
                    total_count: None,
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

#[derive(Debug, Clone, PartialEq)]
pub enum CacheKind {
    ReplicationGroup,
    CacheCluster,
}

#[derive(Debug, Clone)]
pub struct ElastiCacheNode {
    pub cache_cluster_id: String,
    pub node_id: String,
    pub role: Option<String>, // primary / replica (replication groups)
    pub status: String,
    pub az: Option<String>,
    pub endpoint: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ElastiCacheCluster {
    pub id: String,
    pub name: String,
    pub kind: CacheKind,
    pub engine: String,
    pub engine_version: String,
    pub status: String,
    pub node_type: String,
    pub num_nodes: i32,
    pub cluster_mode_enabled: bool,
    pub multi_az: Option<String>,
    pub automatic_failover: Option<String>,
    pub endpoint: Option<String>,
    pub reader_endpoint: Option<String>,
    pub at_rest_encryption: bool,
    pub transit_encryption: bool,
    pub auth_token_enabled: bool,
    pub kms_key_id: Option<String>,
    pub subnet_group: Option<String>,
    pub parameter_group: Option<String>,
    pub security_group_ids: Vec<String>,
    pub maintenance_window: Option<String>,
    pub snapshot_retention: Option<i32>,
    pub created: Option<String>,
    pub arn: String,
    pub description: Option<String>,
    /// The `CacheClusterId` CloudWatch dimension to chart (the primary node).
    pub metric_cache_cluster_id: Option<String>,
    pub nodes: Vec<ElastiCacheNode>,
    pub tags: HashMap<String, String>,
}

impl ElastiCacheCluster {
    pub fn from_replication_group(
        rg: &aws_sdk_elasticache::types::ReplicationGroup,
        cluster_map: &HashMap<String, aws_sdk_elasticache::types::CacheCluster>,
    ) -> Self {
        let id = rg.replication_group_id().unwrap_or_default().to_string();
        let cluster_mode_enabled = rg.cluster_enabled().unwrap_or(false);

        // Flatten node groups into a node list; remember the primary node's
        // cache-cluster id for metrics + enrichment from the member describe.
        let mut nodes = Vec::new();
        let mut primary_cc_id: Option<String> = None;
        let mut endpoint = None;
        let mut reader_endpoint = None;
        for ng in rg.node_groups() {
            if endpoint.is_none() {
                endpoint = fmt_endpoint(ng.primary_endpoint());
            }
            if reader_endpoint.is_none() {
                reader_endpoint = fmt_endpoint(ng.reader_endpoint());
            }
            for m in ng.node_group_members() {
                let role = m.current_role().map(|s| s.to_string());
                let cc_id = m.cache_cluster_id().unwrap_or_default().to_string();
                if primary_cc_id.is_none()
                    && role.as_deref().map(|r| r.eq_ignore_ascii_case("primary")) == Some(true)
                {
                    primary_cc_id = Some(cc_id.clone());
                }
                nodes.push(ElastiCacheNode {
                    cache_cluster_id: cc_id,
                    node_id: m.cache_node_id().unwrap_or_default().to_string(),
                    role,
                    status: String::new(),
                    az: m.preferred_availability_zone().map(|s| s.to_string()),
                    endpoint: fmt_endpoint(m.read_endpoint()),
                });
            }
        }
        // Cluster-mode-enabled groups expose a single configuration endpoint.
        if cluster_mode_enabled {
            if let Some(cfg) = fmt_endpoint(rg.configuration_endpoint()) {
                endpoint = Some(cfg);
            }
        }
        let metric_cache_cluster_id = primary_cc_id
            .clone()
            .or_else(|| nodes.first().map(|n| n.cache_cluster_id.clone()))
            .or_else(|| rg.member_clusters().first().map(|s| s.to_string()));

        // Enrich from the primary member cache cluster (engine, network).
        let member = primary_cc_id
            .as_deref()
            .or_else(|| rg.member_clusters().first().map(|s| s.as_str()))
            .and_then(|cc| cluster_map.get(cc));

        let engine = member
            .and_then(|m| m.engine())
            .unwrap_or("redis")
            .to_string();
        let engine_version = member
            .and_then(|m| m.engine_version())
            .unwrap_or_default()
            .to_string();
        let (subnet_group, parameter_group, security_group_ids, maintenance_window, created) =
            match member {
                Some(m) => (
                    m.cache_subnet_group_name().map(|s| s.to_string()),
                    m.cache_parameter_group()
                        .and_then(|p| p.cache_parameter_group_name())
                        .map(|s| s.to_string()),
                    m.security_groups()
                        .iter()
                        .filter_map(|sg| sg.security_group_id().map(|s| s.to_string()))
                        .collect(),
                    m.preferred_maintenance_window().map(|s| s.to_string()),
                    m.cache_cluster_create_time()
                        .map(|t| crate::aws::services::cloudwatch::fmt_epoch_secs(t.secs())),
                ),
                None => (None, None, Vec::new(), None, None),
            };

        Self {
            name: id.clone(),
            id,
            kind: CacheKind::ReplicationGroup,
            engine,
            engine_version,
            status: rg.status().unwrap_or_default().to_string(),
            node_type: rg.cache_node_type().unwrap_or_default().to_string(),
            num_nodes: rg.member_clusters().len() as i32,
            cluster_mode_enabled,
            multi_az: rg.multi_az().map(|m| m.as_str().to_string()),
            automatic_failover: rg.automatic_failover().map(|a| a.as_str().to_string()),
            endpoint,
            reader_endpoint,
            at_rest_encryption: rg.at_rest_encryption_enabled().unwrap_or(false),
            transit_encryption: rg.transit_encryption_enabled().unwrap_or(false),
            auth_token_enabled: rg.auth_token_enabled().unwrap_or(false),
            kms_key_id: rg.kms_key_id().map(|s| s.to_string()),
            subnet_group,
            parameter_group,
            security_group_ids,
            maintenance_window,
            snapshot_retention: rg.snapshot_retention_limit(),
            created,
            arn: rg.arn().unwrap_or_default().to_string(),
            description: rg
                .description()
                .filter(|d| !d.is_empty() && *d != " ")
                .map(|s| s.to_string()),
            metric_cache_cluster_id,
            nodes,
            tags: HashMap::new(),
        }
    }

    pub fn from_cache_cluster(cc: &aws_sdk_elasticache::types::CacheCluster) -> Self {
        let id = cc.cache_cluster_id().unwrap_or_default().to_string();

        let nodes: Vec<ElastiCacheNode> = cc
            .cache_nodes()
            .iter()
            .map(|n| ElastiCacheNode {
                cache_cluster_id: id.clone(),
                node_id: n.cache_node_id().unwrap_or_default().to_string(),
                role: None,
                status: n.cache_node_status().unwrap_or_default().to_string(),
                az: n.customer_availability_zone().map(|s| s.to_string()),
                endpoint: fmt_endpoint(n.endpoint()),
            })
            .collect();

        // Memcached exposes a configuration endpoint; single-node Redis exposes
        // its node endpoint.
        let endpoint = fmt_endpoint(cc.configuration_endpoint())
            .or_else(|| nodes.first().and_then(|n| n.endpoint.clone()));

        Self {
            name: id.clone(),
            id: id.clone(),
            kind: CacheKind::CacheCluster,
            engine: cc.engine().unwrap_or_default().to_string(),
            engine_version: cc.engine_version().unwrap_or_default().to_string(),
            status: cc.cache_cluster_status().unwrap_or_default().to_string(),
            node_type: cc.cache_node_type().unwrap_or_default().to_string(),
            num_nodes: cc.num_cache_nodes().unwrap_or(0),
            cluster_mode_enabled: false,
            multi_az: None,
            automatic_failover: None,
            endpoint,
            reader_endpoint: None,
            at_rest_encryption: cc.at_rest_encryption_enabled().unwrap_or(false),
            transit_encryption: cc.transit_encryption_enabled().unwrap_or(false),
            auth_token_enabled: cc.auth_token_enabled().unwrap_or(false),
            kms_key_id: None,
            subnet_group: cc.cache_subnet_group_name().map(|s| s.to_string()),
            parameter_group: cc
                .cache_parameter_group()
                .and_then(|p| p.cache_parameter_group_name())
                .map(|s| s.to_string()),
            security_group_ids: cc
                .security_groups()
                .iter()
                .filter_map(|sg| sg.security_group_id().map(|s| s.to_string()))
                .collect(),
            maintenance_window: cc.preferred_maintenance_window().map(|s| s.to_string()),
            snapshot_retention: cc.snapshot_retention_limit(),
            created: cc
                .cache_cluster_create_time()
                .map(|t| crate::aws::services::cloudwatch::fmt_epoch_secs(t.secs())),
            arn: cc.arn().unwrap_or_default().to_string(),
            description: None,
            metric_cache_cluster_id: Some(id),
            nodes,
            tags: HashMap::new(),
        }
    }

    /// "Valkey 7.2 (cluster mode enabled)" — engine + version + mode summary.
    pub fn engine_summary(&self) -> String {
        let mut s = self.engine.clone();
        if !self.engine_version.is_empty() {
            s.push(' ');
            s.push_str(&self.engine_version);
        }
        if self.kind == CacheKind::ReplicationGroup {
            s.push_str(if self.cluster_mode_enabled {
                " (cluster mode enabled)"
            } else {
                " (cluster mode disabled)"
            });
        }
        s
    }
}

crate::sections! {
    pub enum ElastiCacheDetailSection,
    pub static ELASTICACHE_SECTIONS = [
        Details "Details",
        Nodes "Nodes",
        Network "Network",
        Tags "Tags" => crate::app::App::trigger_elasticache_tags_load,
    ]
}

impl Resource for ElastiCacheCluster {
    fn security_group_ids(&self) -> Vec<String> {
        self.security_group_ids.clone()
    }

    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&ELASTICACHE_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn resource_type(&self) -> &str {
        "ElastiCache Cluster"
    }

    fn state(&self) -> ResourceState {
        match self.status.as_str() {
            "available" => ResourceState::Available,
            "creating" => ResourceState::Creating,
            "modifying" | "snapshotting" | "rebooting cache cluster nodes" => ResourceState::Pending,
            "deleting" | "deleted" => ResourceState::Deleting,
            "create-failed" | "incompatible-network" | "restore-failed" => {
                ResourceState::Unavailable
            }
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
            "{} {} {} {} {}",
            self.name, self.id, self.engine, self.node_type, self.arn,
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Name".to_string(), self.name.clone()),
            ("Engine".to_string(), self.engine_summary()),
            ("Status".to_string(), self.status.clone()),
            ("Node Type".to_string(), self.node_type.clone()),
            ("Nodes".to_string(), self.num_nodes.to_string()),
            (
                "Endpoint".to_string(),
                self.endpoint.clone().unwrap_or_else(|| "—".to_string()),
            ),
        ]
    }

    fn console_url(&self, region: &str) -> Option<String> {
        let tab = match (self.kind.clone(), self.engine.as_str()) {
            (CacheKind::CacheCluster, "memcached") => "memcached",
            _ => "redis",
        };
        Some(format!(
            "https://{}.console.aws.amazon.com/elasticache/home?region={}#/{}/{}",
            region, region, tab, self.id
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── Endpoint formatting ───────────────────────────────────────────────────────

fn fmt_endpoint(ep: Option<&aws_sdk_elasticache::types::Endpoint>) -> Option<String> {
    ep.and_then(|e| e.address().map(|a| format!("{}:{}", a, e.port().unwrap_or(0))))
}

// ── Tags (lazy section) ───────────────────────────────────────────────────────

/// `ListTagsForResource` for one cache (keyed by ARN).
pub async fn fetch_elasticache_tags(
    client: ElastiCacheClient,
    arn: String,
) -> Result<Vec<(String, String)>> {
    let resp = client
        .list_tags_for_resource()
        .resource_name(&arn)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
    let mut out: Vec<(String, String)> = resp
        .tag_list()
        .iter()
        .map(|t| {
            (
                t.key().unwrap_or_default().to_string(),
                t.value().unwrap_or_default().to_string(),
            )
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

// ── CloudWatch metrics (`m` overlay) ──────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ElastiCacheMetricsData {
    pub time_range: MetricsTimeRange,
    pub cpu: Vec<(f64, f64)>,
    pub engine_cpu: Vec<(f64, f64)>,
    pub memory_pct: Vec<(f64, f64)>,
    pub connections: Vec<(f64, f64)>,
    pub cache_hits: Vec<(f64, f64)>,
    pub evictions: Vec<(f64, f64)>,
    pub x_max: f64,
}

#[derive(Debug, Clone)]
pub enum ElastiCacheMetricsState {
    Loading,
    Loaded(ElastiCacheMetricsData),
}

/// Pull `AWS/ElastiCache` metrics for one cache cluster node. CPU and memory are
/// the operational signal; `EngineCPUUtilization` is Redis/Valkey's single-thread
/// CPU (the real bottleneck), `DatabaseMemoryUsagePercentage` warns of eviction
/// pressure, and `Evictions` confirms it.
pub async fn fetch_elasticache_metrics(
    cw: aws_sdk_cloudwatch::Client,
    cache_cluster_id: String,
    time_range: MetricsTimeRange,
) -> Result<ElastiCacheMetricsData> {
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
            .name("CacheClusterId")
            .value(&cache_cluster_id)
            .build()
    };
    let metric = |name: &'static str, stat: Statistic| {
        cw.get_metric_statistics()
            .namespace("AWS/ElastiCache")
            .metric_name(name)
            .dimensions(dim())
            .start_time(start_dt)
            .end_time(end_dt)
            .period(period)
            .set_statistics(Some(vec![stat]))
            .send()
    };

    let (cpu, engine_cpu, mem, conns, hits, evict) = tokio::join!(
        metric("CPUUtilization", Statistic::Average),
        metric("EngineCPUUtilization", Statistic::Average),
        metric("DatabaseMemoryUsagePercentage", Statistic::Average),
        metric("CurrConnections", Statistic::Average),
        metric("CacheHits", Statistic::Sum),
        metric("Evictions", Statistic::Sum),
    );

    Ok(ElastiCacheMetricsData {
        time_range,
        cpu: parse_points(cpu, start, |dp| dp.average()),
        engine_cpu: parse_points(engine_cpu, start, |dp| dp.average()),
        memory_pct: parse_points(mem, start, |dp| dp.average()),
        connections: parse_points(conns, start, |dp| dp.average()),
        cache_hits: parse_points(hits, start, |dp| dp.sum()),
        evictions: parse_points(evict, start, |dp| dp.sum()),
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
