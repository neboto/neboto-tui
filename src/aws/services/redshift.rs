use crate::aws::client::AwsClients;
use crate::aws::resource::{native_state_label, Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::aws::services::ec2::MetricsTimeRange;
use crate::error::Result;
use crate::event::{Event, LoadProgress};
use async_trait::async_trait;
use aws_sdk_redshift::Client as RedshiftClient;
use aws_sdk_redshiftserverless::Client as ServerlessClient;
use std::any::Any;
use std::collections::HashMap;
use tokio::sync::mpsc;

/// Redshift service — three sub-tabs (Clusters / Serverless / Snapshots), two
/// clients (`redshift` + `redshift-serverless`), error-tolerant per type.
/// **Clusters**: one paginated `DescribeClusters` — the response already
/// carries the full config (nodes, endpoint, VPC, encryption, IAM roles,
/// maintenance, tags), so no N+1 and no lazy sections. **Serverless**:
/// `ListWorkgroups` joined with `ListNamespaces` by namespace name — the
/// workgroup carries compute/network, its namespace carries the data plane
/// (db, admin, KMS, IAM roles, log exports); both render in one split pane.
/// **Snapshots**: `DescribeClusterSnapshots` + serverless `ListSnapshots`
/// unified into one `RedshiftSnapshot` list (newest first); automated
/// snapshots are `is_noise()` so `a` hides the daily churn. Clusters and
/// workgroups support `m` metrics (`AWS/Redshift` / `AWS/Redshift-Serverless`).
pub struct RedshiftService {
    client: RedshiftClient,
    serverless: ServerlessClient,
}

impl RedshiftService {
    pub fn new(aws_clients: &AwsClients) -> Self {
        Self {
            client: aws_clients.redshift_client(),
            serverless: aws_clients.redshift_serverless_client(),
        }
    }
}

#[async_trait]
impl AwsService for RedshiftService {
    fn service_type(&self) -> ServiceType {
        ServiceType::Redshift
    }

    fn name(&self) -> &str {
        "Redshift"
    }

    async fn list_resources(&self) -> Result<Vec<Box<dyn Resource>>> {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        self.list_resources_streaming(tx, ServiceType::Redshift).await?;
        Ok(vec![])
    }

    async fn list_resources_streaming(
        &self,
        event_tx: mpsc::UnboundedSender<Event>,
        service_type: ServiceType,
    ) -> Result<()> {
        let mut total = 0usize;
        let warn = |msg: String| {
            let _ = event_tx.send(Event::ResourceLoadWarning {
                service: service_type,
                warning: msg,
            });
        };

        // ── Provisioned clusters ──────────────────────────────────────────────
        let mut clusters: Vec<RedshiftCluster> = Vec::new();
        let mut paginator = self.client.describe_clusters().into_paginator().send();
        while let Some(page) = paginator.next().await {
            match page {
                Ok(p) => clusters.extend(p.clusters().iter().map(RedshiftCluster::from_sdk)),
                Err(e) => {
                    warn(format!(
                        "Redshift clusters: {}",
                        crate::error::sdk_error_message(&e)
                    ));
                    break;
                }
            }
        }
        if !clusters.is_empty() {
            total += clusters.len();
            let batch: Vec<Box<dyn Resource>> = clusters
                .into_iter()
                .map(|c| Box::new(c) as Box<dyn Resource>)
                .collect();
            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                service: service_type,
                resources: batch,
                progress: LoadProgress {
                    loaded_count: total,
                    total_count: None,
                    status_message: Some("Loading serverless workgroups…".to_string()),
                },
            });
        }

        // ── Serverless workgroups (namespace-joined) ──────────────────────────
        // Namespaces first: they carry the data plane (db/admin/KMS/IAM/log
        // exports) that the workgroup pane's Namespace section renders. A
        // namespaces failure degrades to workgroups-without-namespace-detail.
        let mut namespaces: HashMap<String, aws_sdk_redshiftserverless::types::Namespace> =
            HashMap::new();
        let mut paginator = self.serverless.list_namespaces().into_paginator().send();
        while let Some(page) = paginator.next().await {
            match page {
                Ok(p) => {
                    for ns in p.namespaces() {
                        if let Some(name) = ns.namespace_name() {
                            namespaces.insert(name.to_string(), ns.clone());
                        }
                    }
                }
                Err(e) => {
                    warn(format!(
                        "Serverless namespaces: {}",
                        crate::error::sdk_error_message(&e)
                    ));
                    break;
                }
            }
        }

        let mut workgroups: Vec<RedshiftWorkgroup> = Vec::new();
        let mut paginator = self.serverless.list_workgroups().into_paginator().send();
        while let Some(page) = paginator.next().await {
            match page {
                Ok(p) => {
                    for wg in p.workgroups() {
                        let ns = wg.namespace_name().and_then(|n| namespaces.get(n));
                        workgroups.push(RedshiftWorkgroup::from_sdk(wg, ns));
                    }
                }
                Err(e) => {
                    warn(format!(
                        "Serverless workgroups: {}",
                        crate::error::sdk_error_message(&e)
                    ));
                    break;
                }
            }
        }
        if !workgroups.is_empty() {
            total += workgroups.len();
            let batch: Vec<Box<dyn Resource>> = workgroups
                .into_iter()
                .map(|w| Box::new(w) as Box<dyn Resource>)
                .collect();
            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                service: service_type,
                resources: batch,
                progress: LoadProgress {
                    loaded_count: total,
                    total_count: None,
                    status_message: Some("Loading snapshots…".to_string()),
                },
            });
        }

        // ── Snapshots (provisioned + serverless, unified) ─────────────────────
        let mut snapshots: Vec<RedshiftSnapshot> = Vec::new();
        let mut paginator = self
            .client
            .describe_cluster_snapshots()
            .into_paginator()
            .send();
        while let Some(page) = paginator.next().await {
            match page {
                Ok(p) => snapshots.extend(p.snapshots().iter().map(RedshiftSnapshot::from_cluster_sdk)),
                Err(e) => {
                    warn(format!(
                        "Cluster snapshots: {}",
                        crate::error::sdk_error_message(&e)
                    ));
                    break;
                }
            }
        }
        let mut paginator = self.serverless.list_snapshots().into_paginator().send();
        while let Some(page) = paginator.next().await {
            match page {
                Ok(p) => {
                    snapshots.extend(p.snapshots().iter().map(RedshiftSnapshot::from_serverless_sdk))
                }
                Err(e) => {
                    warn(format!(
                        "Serverless snapshots: {}",
                        crate::error::sdk_error_message(&e)
                    ));
                    break;
                }
            }
        }
        // Newest first — the recent restore point is what you're looking for.
        snapshots.sort_by(|a, b| b.created_epoch.cmp(&a.created_epoch));
        if !snapshots.is_empty() {
            total += snapshots.len();
            let batch: Vec<Box<dyn Resource>> = snapshots
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

fn fmt_dt(dt: &aws_smithy_types::DateTime) -> String {
    crate::aws::services::cloudwatch::fmt_epoch_secs(dt.secs())
}

/// "12.3 GB" / "512 MB" — snapshot / storage sizes come back in megabytes.
pub fn fmt_mb(mb: f64) -> String {
    if mb >= 1024.0 * 1024.0 {
        format!("{:.1} TB", mb / (1024.0 * 1024.0))
    } else if mb >= 1024.0 {
        format!("{:.1} GB", mb / 1024.0)
    } else {
        format!("{:.0} MB", mb)
    }
}

// ── Provisioned cluster ───────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct RedshiftCluster {
    pub id: String,
    pub arn: String, // cluster namespace ARN (the only ARN the API returns)
    pub status: String,
    pub availability_status: Option<String>,
    pub modify_status: Option<String>,
    pub node_type: String,
    pub number_of_nodes: i32,
    pub db_name: Option<String>,
    pub master_username: Option<String>,
    pub endpoint: Option<(String, i32)>,
    pub created: Option<String>,
    pub version: Option<String>,
    pub allow_version_upgrade: bool,
    pub maintenance_window: Option<String>,
    pub next_maintenance: Option<String>,
    pub maintenance_track: Option<String>,
    pub pending_actions: Vec<String>,
    pub automated_retention_days: i32,
    pub manual_retention_days: Option<i32>,
    pub snapshot_schedule: Option<String>,
    pub vpc_id: Option<String>,
    pub subnet_group: Option<String>,
    pub availability_zone: Option<String>,
    pub az_relocation: Option<String>,
    pub multi_az: Option<String>,
    pub publicly_accessible: bool,
    pub enhanced_vpc_routing: bool,
    pub encrypted: bool,
    pub kms_key_id: Option<String>,
    pub parameter_groups: Vec<(String, String)>, // (name, apply status)
    pub vpc_security_groups: Vec<(String, String)>, // (sg-id, status)
    pub iam_roles: Vec<(String, String)>,        // (role ARN, apply status)
    pub default_iam_role: Option<String>,
    pub total_storage_mb: Option<i64>,
    pub elastic_ip: Option<String>,
    pub custom_domain: Option<String>,
    pub tags: HashMap<String, String>,
}

impl RedshiftCluster {
    pub fn from_sdk(c: &aws_sdk_redshift::types::Cluster) -> Self {
        let tags: HashMap<String, String> = c
            .tags()
            .iter()
            .filter_map(|t| Some((t.key()?.to_string(), t.value()?.to_string())))
            .collect();
        Self {
            id: c.cluster_identifier().unwrap_or_default().to_string(),
            arn: c.cluster_namespace_arn().unwrap_or_default().to_string(),
            status: c.cluster_status().unwrap_or("unknown").to_string(),
            availability_status: c.cluster_availability_status().map(|s| s.to_string()),
            modify_status: c.modify_status().map(|s| s.to_string()),
            node_type: c.node_type().unwrap_or_default().to_string(),
            number_of_nodes: c.number_of_nodes().unwrap_or(0),
            db_name: c.db_name().map(|s| s.to_string()),
            master_username: c.master_username().map(|s| s.to_string()),
            endpoint: c
                .endpoint()
                .and_then(|e| Some((e.address()?.to_string(), e.port().unwrap_or(5439)))),
            created: c.cluster_create_time().map(fmt_dt),
            version: c.cluster_version().map(|s| s.to_string()),
            allow_version_upgrade: c.allow_version_upgrade().unwrap_or(false),
            maintenance_window: c.preferred_maintenance_window().map(|s| s.to_string()),
            next_maintenance: c.next_maintenance_window_start_time().map(fmt_dt),
            maintenance_track: c.maintenance_track_name().map(|s| s.to_string()),
            pending_actions: c.pending_actions().to_vec(),
            automated_retention_days: c.automated_snapshot_retention_period().unwrap_or(0),
            manual_retention_days: c.manual_snapshot_retention_period(),
            snapshot_schedule: c.snapshot_schedule_identifier().map(|s| s.to_string()),
            vpc_id: c.vpc_id().map(|s| s.to_string()),
            subnet_group: c.cluster_subnet_group_name().map(|s| s.to_string()),
            availability_zone: c.availability_zone().map(|s| s.to_string()),
            az_relocation: c.availability_zone_relocation_status().map(|s| s.to_string()),
            multi_az: c.multi_az().map(|s| s.to_string()),
            publicly_accessible: c.publicly_accessible().unwrap_or(false),
            enhanced_vpc_routing: c.enhanced_vpc_routing().unwrap_or(false),
            encrypted: c.encrypted().unwrap_or(false),
            kms_key_id: c.kms_key_id().map(|s| s.to_string()),
            parameter_groups: c
                .cluster_parameter_groups()
                .iter()
                .filter_map(|g| {
                    Some((
                        g.parameter_group_name()?.to_string(),
                        g.parameter_apply_status().unwrap_or("").to_string(),
                    ))
                })
                .collect(),
            vpc_security_groups: c
                .vpc_security_groups()
                .iter()
                .filter_map(|g| {
                    Some((
                        g.vpc_security_group_id()?.to_string(),
                        g.status().unwrap_or("").to_string(),
                    ))
                })
                .collect(),
            iam_roles: c
                .iam_roles()
                .iter()
                .filter_map(|r| {
                    Some((
                        r.iam_role_arn()?.to_string(),
                        r.apply_status().unwrap_or("").to_string(),
                    ))
                })
                .collect(),
            default_iam_role: c.default_iam_role_arn().map(|s| s.to_string()),
            total_storage_mb: c.total_storage_capacity_in_mega_bytes(),
            elastic_ip: c
                .elastic_ip_status()
                .and_then(|e| e.elastic_ip())
                .map(|s| s.to_string()),
            custom_domain: c.custom_domain_name().map(|s| s.to_string()),
            tags,
        }
    }

    /// "ra3.4xlarge × 2" — the header/one-line size label.
    pub fn size_label(&self) -> String {
        format!("{} × {}", self.node_type, self.number_of_nodes)
    }
}

crate::sections! {
    pub enum RedshiftClusterDetailSection,
    pub static REDSHIFT_CLUSTER_SECTIONS = [
        Overview "Overview",
        Network "Network",
        Config "Config",
        Tags "Tags",
    ]
}

impl Resource for RedshiftCluster {
    fn security_group_ids(&self) -> Vec<String> {
        self.vpc_security_groups.iter().map(|(id, _)| id.clone()).collect()
    }

    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&REDSHIFT_CLUSTER_SECTIONS)
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws redshift describe-clusters --cluster-identifier {}",
            crate::aws::resource::shell_quote(&self.id)
        ))
    }

    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        &self.id
    }

    fn resource_type(&self) -> &str {
        "Redshift Cluster"
    }

    fn state(&self) -> ResourceState {
        match self.status.as_str() {
            "available" => match self.availability_status.as_deref() {
                Some("Unavailable") | Some("Failed") => ResourceState::Unavailable,
                _ => ResourceState::Available,
            },
            "paused" | "pausing" => ResourceState::Stopped,
            "creating" | "restoring" => ResourceState::Creating,
            "deleting" | "final-snapshot" => ResourceState::Deleting,
            "modifying" | "resizing" | "rebooting" | "renaming" | "rotating-keys"
            | "updating-hsm" | "resuming" | "recovering" => ResourceState::Pending,
            "unavailable" | "hardware-failure" | "storage-full"
            | "incompatible-hsm" | "incompatible-network" | "incompatible-parameters"
            | "incompatible-restore" => ResourceState::Unavailable,
            other => ResourceState::Unknown(other.to_string()),
        }
    }

    fn state_label(&self) -> String {
        // An "available" cluster whose availability status says otherwise
        // wears that word, mirroring the two-level state() above.
        if self.status == "available" {
            if let Some(a @ ("Unavailable" | "Failed")) = self.availability_status.as_deref() {
                return a.to_lowercase();
            }
        }
        native_state_label(&self.status, || self.state())
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!(
            "{} {} {} {} {} redshift cluster",
            self.id,
            self.status,
            self.node_type,
            self.vpc_id.as_deref().unwrap_or(""),
            self.endpoint.as_ref().map(|(a, _)| a.as_str()).unwrap_or(""),
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Cluster".to_string(), self.id.clone()),
            ("Status".to_string(), self.status.clone()),
            ("Nodes".to_string(), self.size_label()),
            (
                "Database".to_string(),
                self.db_name.clone().unwrap_or_default(),
            ),
        ]
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/redshiftv2/home?region={}#cluster-details?cluster={}",
            region, region, self.id
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── Serverless workgroup (namespace-joined) ───────────────────────────────────

#[derive(Debug, Clone)]
pub struct RedshiftWorkgroup {
    pub name: String,
    pub workgroup_id: String,
    pub arn: String,
    pub status: String,
    pub base_capacity: Option<i32>, // RPUs
    pub max_capacity: Option<i32>,
    pub price_performance: Option<(String, i32)>, // (status, level 1..100)
    pub enhanced_vpc_routing: bool,
    pub publicly_accessible: bool,
    pub endpoint: Option<(String, i32)>,
    pub subnet_ids: Vec<String>,
    pub security_group_ids: Vec<String>,
    pub config_parameters: Vec<(String, String)>,
    pub created: Option<String>,
    pub workgroup_version: Option<String>,
    pub patch_version: Option<String>,
    pub track_name: Option<String>,
    // Joined namespace (data plane) — None when the namespaces list failed.
    pub namespace_name: String,
    pub ns_id: Option<String>,
    pub ns_arn: Option<String>,
    pub ns_status: Option<String>,
    pub ns_db_name: Option<String>,
    pub ns_admin_username: Option<String>,
    pub ns_kms_key_id: Option<String>,
    pub ns_default_iam_role: Option<String>,
    pub ns_iam_roles: Vec<String>,
    pub ns_log_exports: Vec<String>,
    pub ns_created: Option<String>,
    pub tags: HashMap<String, String>,
}

impl RedshiftWorkgroup {
    pub fn from_sdk(
        w: &aws_sdk_redshiftserverless::types::Workgroup,
        ns: Option<&aws_sdk_redshiftserverless::types::Namespace>,
    ) -> Self {
        Self {
            name: w.workgroup_name().unwrap_or_default().to_string(),
            workgroup_id: w.workgroup_id().unwrap_or_default().to_string(),
            arn: w.workgroup_arn().unwrap_or_default().to_string(),
            status: w
                .status()
                .map(|s| s.as_str().to_string())
                .unwrap_or_else(|| "UNKNOWN".to_string()),
            base_capacity: w.base_capacity(),
            max_capacity: w.max_capacity(),
            price_performance: w.price_performance_target().map(|t| {
                (
                    t.status()
                        .map(|s| s.as_str().to_string())
                        .unwrap_or_default(),
                    t.level().unwrap_or(0),
                )
            }),
            enhanced_vpc_routing: w.enhanced_vpc_routing().unwrap_or(false),
            publicly_accessible: w.publicly_accessible().unwrap_or(false),
            endpoint: w
                .endpoint()
                .and_then(|e| Some((e.address()?.to_string(), e.port().unwrap_or(5439)))),
            subnet_ids: w.subnet_ids().to_vec(),
            security_group_ids: w.security_group_ids().to_vec(),
            config_parameters: w
                .config_parameters()
                .iter()
                .filter_map(|p| {
                    Some((
                        p.parameter_key()?.to_string(),
                        p.parameter_value().unwrap_or("").to_string(),
                    ))
                })
                .collect(),
            created: w.creation_date().map(fmt_dt),
            workgroup_version: w.workgroup_version().map(|s| s.to_string()),
            patch_version: w.patch_version().map(|s| s.to_string()),
            track_name: w.track_name().map(|s| s.to_string()),
            namespace_name: w.namespace_name().unwrap_or_default().to_string(),
            ns_id: ns.and_then(|n| n.namespace_id()).map(|s| s.to_string()),
            ns_arn: ns.and_then(|n| n.namespace_arn()).map(|s| s.to_string()),
            ns_status: ns
                .and_then(|n| n.status())
                .map(|s| s.as_str().to_string()),
            ns_db_name: ns.and_then(|n| n.db_name()).map(|s| s.to_string()),
            ns_admin_username: ns.and_then(|n| n.admin_username()).map(|s| s.to_string()),
            ns_kms_key_id: ns.and_then(|n| n.kms_key_id()).map(|s| s.to_string()),
            ns_default_iam_role: ns
                .and_then(|n| n.default_iam_role_arn())
                .map(|s| s.to_string()),
            ns_iam_roles: ns
                .map(|n| n.iam_roles().to_vec())
                .unwrap_or_default(),
            ns_log_exports: ns
                .map(|n| {
                    n.log_exports()
                        .iter()
                        .map(|l| l.as_str().to_string())
                        .collect()
                })
                .unwrap_or_default(),
            ns_created: ns.and_then(|n| n.creation_date()).map(fmt_dt),
            tags: HashMap::new(),
        }
    }

    /// "8–64 RPU" / "8 RPU" — the capacity label.
    pub fn capacity_label(&self) -> String {
        match (self.base_capacity, self.max_capacity) {
            (Some(b), Some(m)) => format!("{}–{} RPU", b, m),
            (Some(b), None) => format!("{} RPU", b),
            _ => "price-performance managed".to_string(),
        }
    }
}

crate::sections! {
    pub enum RedshiftWorkgroupDetailSection,
    pub static REDSHIFT_WORKGROUP_SECTIONS = [
        Overview "Overview",
        Namespace "Namespace",
        Network "Network",
    ]
}

impl Resource for RedshiftWorkgroup {
    fn security_group_ids(&self) -> Vec<String> {
        self.security_group_ids.clone()
    }

    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&REDSHIFT_WORKGROUP_SECTIONS)
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws redshift-serverless get-workgroup --workgroup-name {}",
            crate::aws::resource::shell_quote(&self.name)
        ))
    }

    fn id(&self) -> &str {
        &self.name
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn resource_type(&self) -> &str {
        "Redshift Workgroup"
    }

    fn state(&self) -> ResourceState {
        match self.status.as_str() {
            "AVAILABLE" => ResourceState::Available,
            "CREATING" => ResourceState::Creating,
            "MODIFYING" => ResourceState::Pending,
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
        format!(
            "{} {} {} {} redshift serverless workgroup",
            self.name,
            self.namespace_name,
            self.status,
            self.endpoint.as_ref().map(|(a, _)| a.as_str()).unwrap_or(""),
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Workgroup".to_string(), self.name.clone()),
            ("Status".to_string(), self.status.clone()),
            ("Capacity".to_string(), self.capacity_label()),
            ("Namespace".to_string(), self.namespace_name.clone()),
        ]
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/redshiftv2/home?region={}#serverless-dashboard",
            region, region
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── Snapshots (provisioned + serverless, unified) ─────────────────────────────

#[derive(Debug, Clone)]
pub struct RedshiftSnapshot {
    pub id: String,
    pub arn: Option<String>,
    /// Source cluster identifier (provisioned) or namespace name (serverless).
    pub source: String,
    pub is_serverless: bool,
    pub status: String,
    pub snapshot_type: String, // manual / automated / serverless
    pub size_mb: Option<f64>,
    pub incremental_mb: Option<f64>,
    pub created: Option<String>,
    pub created_epoch: i64,
    pub retention_days: Option<i32>, // None/-1 = retained indefinitely
    pub remaining_days: Option<i32>,
    pub node_type: Option<String>,
    pub number_of_nodes: Option<i32>,
    pub db_name: Option<String>,
    pub master_username: Option<String>,
    pub encrypted: bool,
    pub kms_key_id: Option<String>,
    pub vpc_id: Option<String>,
    pub owner_account: Option<String>,
    pub source_region: Option<String>,
    pub tags: HashMap<String, String>,
}

impl RedshiftSnapshot {
    pub fn from_cluster_sdk(s: &aws_sdk_redshift::types::Snapshot) -> Self {
        let tags: HashMap<String, String> = s
            .tags()
            .iter()
            .filter_map(|t| Some((t.key()?.to_string(), t.value()?.to_string())))
            .collect();
        Self {
            id: s.snapshot_identifier().unwrap_or_default().to_string(),
            arn: s.snapshot_arn().map(|a| a.to_string()),
            source: s.cluster_identifier().unwrap_or_default().to_string(),
            is_serverless: false,
            status: s.status().unwrap_or("unknown").to_string(),
            snapshot_type: s.snapshot_type().unwrap_or("manual").to_string(),
            size_mb: s.total_backup_size_in_mega_bytes(),
            incremental_mb: s.actual_incremental_backup_size_in_mega_bytes(),
            created: s.snapshot_create_time().map(fmt_dt),
            created_epoch: s.snapshot_create_time().map(|t| t.secs()).unwrap_or(0),
            retention_days: s.manual_snapshot_retention_period(),
            remaining_days: s.manual_snapshot_remaining_days(),
            node_type: s.node_type().map(|s| s.to_string()),
            number_of_nodes: s.number_of_nodes(),
            db_name: s.db_name().map(|s| s.to_string()),
            master_username: s.master_username().map(|s| s.to_string()),
            encrypted: s.encrypted().unwrap_or(false),
            kms_key_id: s.kms_key_id().map(|s| s.to_string()),
            vpc_id: s.vpc_id().map(|s| s.to_string()),
            owner_account: s.owner_account().map(|s| s.to_string()),
            source_region: s.source_region().map(|s| s.to_string()),
            tags,
        }
    }

    pub fn from_serverless_sdk(s: &aws_sdk_redshiftserverless::types::Snapshot) -> Self {
        Self {
            id: s.snapshot_name().unwrap_or_default().to_string(),
            arn: s.snapshot_arn().map(|a| a.to_string()),
            source: s.namespace_name().unwrap_or_default().to_string(),
            is_serverless: true,
            status: s
                .status()
                .map(|st| st.as_str().to_lowercase())
                .unwrap_or_else(|| "unknown".to_string()),
            snapshot_type: "serverless".to_string(),
            size_mb: s.total_backup_size_in_mega_bytes(),
            incremental_mb: s.actual_incremental_backup_size_in_mega_bytes(),
            created: s.snapshot_create_time().map(fmt_dt),
            created_epoch: s.snapshot_create_time().map(|t| t.secs()).unwrap_or(0),
            retention_days: s.snapshot_retention_period(),
            remaining_days: s.snapshot_remaining_days(),
            node_type: None,
            number_of_nodes: None,
            db_name: None,
            master_username: s.admin_username().map(|s| s.to_string()),
            encrypted: s.kms_key_id().is_some(),
            kms_key_id: s.kms_key_id().map(|s| s.to_string()),
            vpc_id: None,
            owner_account: s.owner_account().map(|s| s.to_string()),
            source_region: None,
            tags: HashMap::new(),
        }
    }

    fn retention_label(&self) -> String {
        match self.retention_days {
            Some(d) if d > 0 => format!("{} day(s)", d),
            _ => "indefinite".to_string(),
        }
    }
}

impl Resource for RedshiftSnapshot {
    fn cli_command(&self) -> Option<String> {
        Some(if self.is_serverless {
            format!(
                "aws redshift-serverless get-snapshot --snapshot-name {}",
                crate::aws::resource::shell_quote(&self.id)
            )
        } else {
            format!(
                "aws redshift describe-cluster-snapshots --snapshot-identifier {}",
                crate::aws::resource::shell_quote(&self.id)
            )
        })
    }

    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        &self.id
    }

    fn resource_type(&self) -> &str {
        "Redshift Snapshot"
    }

    fn state(&self) -> ResourceState {
        match self.status.as_str() {
            "available" => ResourceState::Available,
            "creating" | "copying" => ResourceState::Creating,
            "deleted" | "deleting" => ResourceState::Deleting,
            "failed" => ResourceState::Unavailable,
            other => ResourceState::Unknown(other.to_string()),
        }
    }

    fn state_label(&self) -> String {
        native_state_label(&self.status, || self.state())
    }

    /// Automated snapshots are daily churn — `a` hides them.
    fn is_noise(&self) -> bool {
        self.snapshot_type == "automated"
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!(
            "{} {} {} {} redshift snapshot",
            self.id, self.source, self.snapshot_type, self.status
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        let mut rows = vec![
            ("Snapshot".to_string(), self.id.clone()),
            ("Status".to_string(), self.status.clone()),
            ("Type".to_string(), self.snapshot_type.clone()),
            (
                if self.is_serverless {
                    "Namespace".to_string()
                } else {
                    "Cluster".to_string()
                },
                self.source.clone(),
            ),
        ];
        if let Some(sz) = self.size_mb {
            rows.push(("Size".to_string(), fmt_mb(sz)));
        }
        if let Some(inc) = self.incremental_mb {
            rows.push(("Incremental".to_string(), fmt_mb(inc)));
        }
        if let Some(c) = &self.created {
            rows.push(("Created".to_string(), c.clone()));
        }
        rows.push(("Retention".to_string(), self.retention_label()));
        if let Some(d) = self.remaining_days {
            rows.push(("Remaining".to_string(), format!("{} day(s)", d)));
        }
        if let (Some(nt), Some(n)) = (&self.node_type, self.number_of_nodes) {
            rows.push(("Nodes".to_string(), format!("{} × {}", nt, n)));
        }
        if let Some(db) = &self.db_name {
            rows.push(("Database".to_string(), db.clone()));
        }
        if let Some(u) = &self.master_username {
            rows.push(("Admin User".to_string(), u.clone()));
        }
        rows.push((
            "Encrypted".to_string(),
            if self.encrypted {
                "✓ Yes".to_string()
            } else {
                "✗ No".to_string()
            },
        ));
        if let Some(kms) = &self.kms_key_id {
            rows.push(("KMS Key".to_string(), kms.clone()));
        }
        if let Some(vpc) = &self.vpc_id {
            rows.push(("VPC".to_string(), vpc.clone()));
        }
        if let Some(acct) = &self.owner_account {
            rows.push(("Owner Account".to_string(), acct.clone()));
        }
        if let Some(r) = &self.source_region {
            rows.push(("Source Region".to_string(), r.clone()));
        }
        if let Some(arn) = &self.arn {
            rows.push(("ARN".to_string(), arn.clone()));
        }
        rows
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/redshiftv2/home?region={}#snapshots",
            region, region
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── CloudWatch metrics (`m` overlay) ──────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct RedshiftMetricsData {
    pub time_range: MetricsTimeRange,
    pub cpu: Vec<(f64, f64)>,
    pub disk_used_pct: Vec<(f64, f64)>,
    pub connections: Vec<(f64, f64)>,
    pub health: Vec<(f64, f64)>, // HealthStatus: 1 healthy, 0 unhealthy
    pub read_iops: Vec<(f64, f64)>,
    pub write_iops: Vec<(f64, f64)>,
    pub x_max: f64,
}

#[derive(Debug, Clone)]
pub enum RedshiftMetricsState {
    Loading,
    Loaded(RedshiftMetricsData),
}

/// Pull `AWS/Redshift` metrics for one provisioned cluster (dim
/// `ClusterIdentifier`). `PercentageDiskSpaceUsed` is the capacity-pressure
/// signal; `HealthStatus` (1/0) flags an unhealthy cluster.
pub async fn fetch_redshift_metrics(
    cw: aws_sdk_cloudwatch::Client,
    cluster_id: String,
    time_range: MetricsTimeRange,
) -> Result<RedshiftMetricsData> {
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
            .name("ClusterIdentifier")
            .value(&cluster_id)
            .build()
    };
    let metric = |name: &'static str, stat: Statistic| {
        cw.get_metric_statistics()
            .namespace("AWS/Redshift")
            .metric_name(name)
            .dimensions(dim())
            .start_time(start_dt)
            .end_time(end_dt)
            .period(period)
            .set_statistics(Some(vec![stat]))
            .send()
    };

    let (cpu, disk, conns, health, read_iops, write_iops) = tokio::join!(
        metric("CPUUtilization", Statistic::Average),
        metric("PercentageDiskSpaceUsed", Statistic::Average),
        metric("DatabaseConnections", Statistic::Average),
        metric("HealthStatus", Statistic::Minimum),
        metric("ReadIOPS", Statistic::Average),
        metric("WriteIOPS", Statistic::Average),
    );

    let parse = crate::aws::services::ec2::parse_metric_datapoints;
    Ok(RedshiftMetricsData {
        time_range,
        cpu: parse(cpu, start),
        disk_used_pct: parse(disk, start),
        connections: parse(conns, start),
        health: parse(health, start),
        read_iops: parse(read_iops, start),
        write_iops: parse(write_iops, start),
        x_max: time_range.duration_secs() as f64,
    })
}

#[derive(Debug, Clone)]
pub struct RedshiftSlMetricsData {
    pub time_range: MetricsTimeRange,
    pub compute_capacity: Vec<(f64, f64)>, // RPUs in use
    pub compute_seconds: Vec<(f64, f64)>,  // the billing signal
    pub connections: Vec<(f64, f64)>,
    pub queries_completed: Vec<(f64, f64)>,
    pub queries_running: Vec<(f64, f64)>,
    pub queries_queued: Vec<(f64, f64)>,
    pub x_max: f64,
}

#[derive(Debug, Clone)]
pub enum RedshiftSlMetricsState {
    Loading,
    Loaded(RedshiftSlMetricsData),
}

/// Pull `AWS/Redshift-Serverless` metrics for one workgroup (dim `Workgroup`).
/// `ComputeSeconds` is the RPU-seconds billing proxy; queued queries flag a
/// max-capacity ceiling.
pub async fn fetch_redshift_sl_metrics(
    cw: aws_sdk_cloudwatch::Client,
    workgroup: String,
    time_range: MetricsTimeRange,
) -> Result<RedshiftSlMetricsData> {
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
            .name("Workgroup")
            .value(&workgroup)
            .build()
    };
    let metric = |name: &'static str, stat: Statistic| {
        cw.get_metric_statistics()
            .namespace("AWS/Redshift-Serverless")
            .metric_name(name)
            .dimensions(dim())
            .start_time(start_dt)
            .end_time(end_dt)
            .period(period)
            .set_statistics(Some(vec![stat]))
            .send()
    };

    let (capacity, seconds, conns, completed, running, queued) = tokio::join!(
        metric("ComputeCapacity", Statistic::Average),
        metric("ComputeSeconds", Statistic::Sum),
        metric("DatabaseConnections", Statistic::Average),
        metric("QueriesCompletedPerSecond", Statistic::Average),
        metric("QueriesRunning", Statistic::Average),
        metric("QueriesQueued", Statistic::Average),
    );

    let parse = crate::aws::services::ec2::parse_metric_datapoints;
    Ok(RedshiftSlMetricsData {
        time_range,
        compute_capacity: parse(capacity, start),
        compute_seconds: parse(seconds, start),
        connections: parse(conns, start),
        queries_completed: parse(completed, start),
        queries_running: parse(running, start),
        queries_queued: parse(queued, start),
        x_max: time_range.duration_secs() as f64,
    })
}
