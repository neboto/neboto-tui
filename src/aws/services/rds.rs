use crate::aws::client::AwsClients;
use crate::aws::resource::sg_refs;
use crate::aws::resource::{native_state_label, Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::aws::services::ec2::MetricsTimeRange;
use crate::error::Result;
use crate::event::{Event, LoadProgress};
use async_trait::async_trait;
use aws_sdk_cloudwatch::types::Datapoint;
use aws_sdk_rds::Client as RdsClient;
use std::any::Any;
use std::collections::HashMap;
use tokio::sync::mpsc;

pub struct RdsService {
    client: RdsClient,
}

impl RdsService {
    pub fn new(aws_clients: &AwsClients) -> Self {
        Self {
            client: aws_clients.rds_client(),
        }
    }
}

#[async_trait]
impl AwsService for RdsService {
    fn service_type(&self) -> ServiceType {
        ServiceType::RDS
    }

    fn name(&self) -> &str {
        "RDS"
    }

    async fn list_resources(&self) -> Result<Vec<Box<dyn Resource>>> {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        self.list_resources_streaming(tx, ServiceType::RDS).await?;
        Ok(vec![])
    }

    async fn list_resources_streaming(
        &self,
        event_tx: mpsc::UnboundedSender<Event>,
        service_type: ServiceType,
    ) -> Result<()> {
        let mut total = 0usize;

        let mut inst_paginator = self.client.describe_db_instances().into_paginator().send();
        while let Some(result) = inst_paginator.next().await {
            match result {
                Ok(page) => {
                    let batch: Vec<Box<dyn Resource>> = page
                        .db_instances()
                        .iter()
                        .map(|db| Box::new(RdsInstance::from_sdk(db)) as Box<dyn Resource>)
                        .collect();
                    let count = batch.len();
                    if count == 0 {
                        continue;
                    }
                    total += count;
                    let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                        service: service_type,
                        resources: batch,
                        progress: LoadProgress {
                            loaded_count: total,
                            total_count: None,
                            status_message: Some("Loading instances…".to_string()),
                        },
                    });
                }
                Err(e) => {
                    let _ = event_tx.send(Event::ResourceLoadError {
                        service: service_type,
                        error: format!("Failed to list RDS instances: {}", e),
                    });
                    return Ok(());
                }
            }
        }

        let mut cluster_paginator = self.client.describe_db_clusters().into_paginator().send();
        while let Some(result) = cluster_paginator.next().await {
            match result {
                Ok(page) => {
                    let batch: Vec<Box<dyn Resource>> = page
                        .db_clusters()
                        .iter()
                        .map(|c| Box::new(RdsCluster::from_sdk(c)) as Box<dyn Resource>)
                        .collect();
                    let count = batch.len();
                    if count == 0 {
                        continue;
                    }
                    total += count;
                    let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                        service: service_type,
                        resources: batch,
                        progress: LoadProgress {
                            loaded_count: total,
                            total_count: None,
                            status_message: Some("Loading clusters…".to_string()),
                        },
                    });
                }
                Err(e) => {
                    let _ = event_tx.send(Event::ResourceLoadError {
                        service: service_type,
                        error: format!("Failed to list RDS clusters: {}", e),
                    });
                    return Ok(());
                }
            }
        }

        // Snapshots (instance + cluster). Self-owned only — the APIs default to
        // your own snapshots. Error-tolerant: a snapshot failure never aborts the
        // (already-sent) instances/clusters.
        let mut snap_paginator = self.client.describe_db_snapshots().into_paginator().send();
        while let Some(result) = snap_paginator.next().await {
            match result {
                Ok(page) => {
                    let batch: Vec<Box<dyn Resource>> = page
                        .db_snapshots()
                        .iter()
                        .map(|s| Box::new(RdsSnapshot::from_db(s)) as Box<dyn Resource>)
                        .collect();
                    if batch.is_empty() {
                        continue;
                    }
                    total += batch.len();
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
                Err(e) => {
                    let _ = event_tx.send(Event::ResourceLoadWarning {
                        service: service_type,
                        warning: format!("DB snapshots: {}", e),
                    });
                    break;
                }
            }
        }

        let mut csnap_paginator =
            self.client.describe_db_cluster_snapshots().into_paginator().send();
        while let Some(result) = csnap_paginator.next().await {
            match result {
                Ok(page) => {
                    let batch: Vec<Box<dyn Resource>> = page
                        .db_cluster_snapshots()
                        .iter()
                        .map(|s| Box::new(RdsSnapshot::from_cluster(s)) as Box<dyn Resource>)
                        .collect();
                    if batch.is_empty() {
                        continue;
                    }
                    total += batch.len();
                    let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                        service: service_type,
                        resources: batch,
                        progress: LoadProgress {
                            loaded_count: total,
                            total_count: None,
                            status_message: Some("Loading cluster snapshots…".to_string()),
                        },
                    });
                }
                Err(e) => {
                    let _ = event_tx.send(Event::ResourceLoadWarning {
                        service: service_type,
                        warning: format!("DB cluster snapshots: {}", e),
                    });
                    break;
                }
            }
        }

        // Parameter groups (instance + cluster, one phase, unified like the
        // snapshots), then option groups, then subnet groups — all best-effort.
        let mut pg_paginator = self.client.describe_db_parameter_groups().into_paginator().send();
        while let Some(result) = pg_paginator.next().await {
            match result {
                Ok(page) => {
                    let batch: Vec<Box<dyn Resource>> = page
                        .db_parameter_groups()
                        .iter()
                        .map(|g| Box::new(RdsParamGroup::from_sdk(g)) as Box<dyn Resource>)
                        .collect();
                    if batch.is_empty() {
                        continue;
                    }
                    total += batch.len();
                    let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                        service: service_type,
                        resources: batch,
                        progress: LoadProgress {
                            loaded_count: total,
                            total_count: None,
                            status_message: Some("Loading parameter groups…".to_string()),
                        },
                    });
                }
                Err(e) => {
                    let _ = event_tx.send(Event::ResourceLoadWarning {
                        service: service_type,
                        warning: format!("DB parameter groups: {}", e),
                    });
                    break;
                }
            }
        }

        let mut cpg_paginator = self
            .client
            .describe_db_cluster_parameter_groups()
            .into_paginator()
            .send();
        while let Some(result) = cpg_paginator.next().await {
            match result {
                Ok(page) => {
                    let batch: Vec<Box<dyn Resource>> = page
                        .db_cluster_parameter_groups()
                        .iter()
                        .map(|g| Box::new(RdsParamGroup::from_cluster_sdk(g)) as Box<dyn Resource>)
                        .collect();
                    if batch.is_empty() {
                        continue;
                    }
                    total += batch.len();
                    let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                        service: service_type,
                        resources: batch,
                        progress: LoadProgress {
                            loaded_count: total,
                            total_count: None,
                            status_message: Some("Loading parameter groups…".to_string()),
                        },
                    });
                }
                Err(e) => {
                    let _ = event_tx.send(Event::ResourceLoadWarning {
                        service: service_type,
                        warning: format!("Cluster parameter groups: {}", e),
                    });
                    break;
                }
            }
        }

        let mut og_paginator = self.client.describe_option_groups().into_paginator().send();
        while let Some(result) = og_paginator.next().await {
            match result {
                Ok(page) => {
                    let batch: Vec<Box<dyn Resource>> = page
                        .option_groups_list()
                        .iter()
                        .map(|g| Box::new(RdsOptionGroup::from_sdk(g)) as Box<dyn Resource>)
                        .collect();
                    if batch.is_empty() {
                        continue;
                    }
                    total += batch.len();
                    let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                        service: service_type,
                        resources: batch,
                        progress: LoadProgress {
                            loaded_count: total,
                            total_count: None,
                            status_message: Some("Loading option groups…".to_string()),
                        },
                    });
                }
                Err(e) => {
                    let _ = event_tx.send(Event::ResourceLoadWarning {
                        service: service_type,
                        warning: format!("Option groups: {}", e),
                    });
                    break;
                }
            }
        }

        let mut sg_paginator = self.client.describe_db_subnet_groups().into_paginator().send();
        while let Some(result) = sg_paginator.next().await {
            match result {
                Ok(page) => {
                    let batch: Vec<Box<dyn Resource>> = page
                        .db_subnet_groups()
                        .iter()
                        .map(|g| Box::new(RdsSubnetGroup::from_sdk(g)) as Box<dyn Resource>)
                        .collect();
                    if batch.is_empty() {
                        continue;
                    }
                    total += batch.len();
                    let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                        service: service_type,
                        resources: batch,
                        progress: LoadProgress {
                            loaded_count: total,
                            total_count: None,
                            status_message: Some("Loading subnet groups…".to_string()),
                        },
                    });
                }
                Err(e) => {
                    let _ = event_tx.send(Event::ResourceLoadWarning {
                        service: service_type,
                        warning: format!("DB subnet groups: {}", e),
                    });
                    break;
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

// ── RdsInstance ───────────────────────────────────────────────────────────────

/// Render an SDK timestamp as `YYYY-MM-DD HH:MM:SS` (drop sub-seconds/zone).
fn fmt_dt(t: &aws_sdk_rds::primitives::DateTime) -> String {
    let s = t.to_string();
    s.split('.').next().unwrap_or(&s).replace('T', " ").replace('Z', "")
}

#[derive(Debug, Clone)]
pub struct RdsInstance {
    pub db_identifier: String,
    pub arn: String,
    pub db_instance_class: String,
    pub engine: String,
    pub engine_version: String,
    pub status: String,
    pub endpoint_address: Option<String>,
    pub endpoint_port: Option<i32>,
    pub allocated_storage_gb: i32,
    /// Storage-autoscaling ceiling (None = autoscaling disabled).
    pub max_allocated_storage_gb: Option<i32>,
    /// gp3 provisioned throughput (MiB/s).
    pub storage_throughput: Option<i32>,
    pub multi_az: bool,
    pub publicly_accessible: bool,
    pub storage_type: String,
    pub iops: Option<i32>,
    pub storage_encrypted: bool,
    pub kms_key_id: Option<String>,
    pub master_username: String,
    pub db_name: Option<String>,
    pub availability_zone: String,
    pub secondary_availability_zone: Option<String>,
    pub network_type: Option<String>,
    pub license_model: Option<String>,
    pub cluster_identifier: Option<String>,
    pub subnet_group: Option<String>,
    pub vpc_id: Option<String>,
    pub subnet_ids: Vec<String>,
    pub vpc_security_groups: Vec<String>,
    pub deletion_protection: bool,
    pub iam_auth_enabled: bool,
    pub backup_retention_days: i32,
    pub preferred_backup_window: String,
    pub preferred_maintenance_window: String,
    pub auto_minor_version_upgrade: bool,
    pub copy_tags_to_snapshot: bool,
    /// "region" (default) or "outposts".
    pub backup_target: Option<String>,
    pub latest_restorable_time: Option<String>,
    pub ca_certificate_identifier: Option<String>,
    pub ca_valid_till: Option<String>,
    /// (parameter group name, apply status — e.g. in-sync/pending-reboot).
    pub parameter_groups: Vec<(String, String)>,
    /// (option group name, status).
    pub option_groups: Vec<(String, String)>,
    /// Source instance id when this is a read replica.
    pub replica_source: Option<String>,
    /// Read replicas of this instance.
    pub replica_ids: Vec<String>,
    pub replica_mode: Option<String>,
    /// Enhanced Monitoring granularity in seconds (0 = disabled).
    pub monitoring_interval: i32,
    pub monitoring_role_arn: Option<String>,
    pub pi_retention_days: Option<i32>,
    /// `PendingModifiedValues` pre-flattened to display rows; empty = none.
    pub pending_modifications: Vec<(String, String)>,
    pub create_time: String,
    /// Log types exported to CloudWatch Logs (e.g. error/general/slowquery/
    /// postgresql); each maps to `/aws/rds/instance/<id>/<type>`.
    pub enabled_log_exports: Vec<String>,
    pub performance_insights_enabled: bool,
    /// The Performance Insights resource identifier (`db-XXXX…`) — the
    /// `identifier` arg for `pi:GetResourceMetrics`, distinct from the DB id.
    pub dbi_resource_id: Option<String>,
    pub tags: HashMap<String, String>,
}

/// Flatten a DB instance's `PendingModifiedValues` into display rows. Only
/// set fields appear, so an empty Vec means "no pending modifications".
fn instance_pending_rows(p: &aws_sdk_rds::types::PendingModifiedValues) -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = Vec::new();
    let yes_no = |b: bool| if b { "Yes".to_string() } else { "No".to_string() };
    if let Some(v) = p.db_instance_class() {
        rows.push(("Instance Class".to_string(), v.to_string()));
    }
    if let Some(v) = p.engine() {
        rows.push(("Engine".to_string(), v.to_string()));
    }
    if let Some(v) = p.engine_version() {
        rows.push(("Engine Version".to_string(), v.to_string()));
    }
    if let Some(v) = p.allocated_storage() {
        rows.push(("Allocated Storage".to_string(), format!("{} GiB", v)));
    }
    if let Some(v) = p.storage_type() {
        rows.push(("Storage Type".to_string(), v.to_string()));
    }
    if let Some(v) = p.iops() {
        rows.push(("IOPS".to_string(), v.to_string()));
    }
    if let Some(v) = p.storage_throughput() {
        rows.push(("Storage Throughput".to_string(), format!("{} MiB/s", v)));
    }
    if p.master_user_password().is_some() {
        rows.push(("Master Password".to_string(), "(change pending)".to_string()));
    }
    if let Some(v) = p.port() {
        rows.push(("Port".to_string(), v.to_string()));
    }
    if let Some(v) = p.backup_retention_period() {
        rows.push(("Backup Retention".to_string(), format!("{} days", v)));
    }
    if let Some(v) = p.multi_az() {
        rows.push(("Multi-AZ".to_string(), yes_no(v)));
    }
    if let Some(v) = p.db_instance_identifier() {
        rows.push(("New Identifier".to_string(), v.to_string()));
    }
    if let Some(v) = p.license_model() {
        rows.push(("License Model".to_string(), v.to_string()));
    }
    if let Some(v) = p.ca_certificate_identifier() {
        rows.push(("CA Certificate".to_string(), v.to_string()));
    }
    if let Some(v) = p.db_subnet_group_name() {
        rows.push(("Subnet Group".to_string(), v.to_string()));
    }
    if let Some(v) = p.iam_database_authentication_enabled() {
        rows.push(("IAM Auth".to_string(), yes_no(v)));
    }
    if let Some(v) = p.dedicated_log_volume() {
        rows.push(("Dedicated Log Volume".to_string(), yes_no(v)));
    }
    if let Some(v) = p.multi_tenant() {
        rows.push(("Multi-Tenant".to_string(), yes_no(v)));
    }
    if let Some(cw) = p.pending_cloudwatch_logs_exports() {
        if !cw.log_types_to_enable().is_empty() {
            rows.push((
                "Log Exports (enable)".to_string(),
                cw.log_types_to_enable().join(", "),
            ));
        }
        if !cw.log_types_to_disable().is_empty() {
            rows.push((
                "Log Exports (disable)".to_string(),
                cw.log_types_to_disable().join(", "),
            ));
        }
    }
    rows
}

/// Flatten a cluster's `ClusterPendingModifiedValues` the same way.
fn cluster_pending_rows(
    p: &aws_sdk_rds::types::ClusterPendingModifiedValues,
) -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = Vec::new();
    let yes_no = |b: bool| if b { "Yes".to_string() } else { "No".to_string() };
    if let Some(v) = p.db_cluster_identifier() {
        rows.push(("New Identifier".to_string(), v.to_string()));
    }
    if let Some(v) = p.engine_version() {
        rows.push(("Engine Version".to_string(), v.to_string()));
    }
    if p.master_user_password().is_some() {
        rows.push(("Master Password".to_string(), "(change pending)".to_string()));
    }
    if let Some(v) = p.backup_retention_period() {
        rows.push(("Backup Retention".to_string(), format!("{} days", v)));
    }
    if let Some(v) = p.allocated_storage() {
        rows.push(("Allocated Storage".to_string(), format!("{} GiB", v)));
    }
    if let Some(v) = p.storage_type() {
        rows.push(("Storage Type".to_string(), v.to_string()));
    }
    if let Some(v) = p.iops() {
        rows.push(("IOPS".to_string(), v.to_string()));
    }
    if let Some(v) = p.iam_database_authentication_enabled() {
        rows.push(("IAM Auth".to_string(), yes_no(v)));
    }
    if let Some(cw) = p.pending_cloudwatch_logs_exports() {
        if !cw.log_types_to_enable().is_empty() {
            rows.push((
                "Log Exports (enable)".to_string(),
                cw.log_types_to_enable().join(", "),
            ));
        }
        if !cw.log_types_to_disable().is_empty() {
            rows.push((
                "Log Exports (disable)".to_string(),
                cw.log_types_to_disable().join(", "),
            ));
        }
    }
    rows
}

impl RdsInstance {
    pub fn from_sdk(db: &aws_sdk_rds::types::DbInstance) -> Self {
        let tags: HashMap<String, String> = db
            .tag_list()
            .iter()
            .filter_map(|t| Some((t.key()?.to_string(), t.value()?.to_string())))
            .collect();

        let vpc_security_groups: Vec<String> = db
            .vpc_security_groups()
            .iter()
            .filter_map(|sg| sg.vpc_security_group_id().map(|s| s.to_string()))
            .collect();

        Self {
            db_identifier: db.db_instance_identifier().unwrap_or("").to_string(),
            arn: db.db_instance_arn().unwrap_or("").to_string(),
            db_instance_class: db.db_instance_class().unwrap_or("").to_string(),
            engine: db.engine().unwrap_or("").to_string(),
            engine_version: db.engine_version().unwrap_or("").to_string(),
            status: db.db_instance_status().unwrap_or("").to_string(),
            endpoint_address: db.endpoint().and_then(|e| e.address()).map(|s| s.to_string()),
            endpoint_port: db.endpoint().and_then(|e| e.port()),
            allocated_storage_gb: db.allocated_storage().unwrap_or(0),
            max_allocated_storage_gb: db.max_allocated_storage(),
            storage_throughput: db.storage_throughput(),
            multi_az: db.multi_az().unwrap_or(false),
            publicly_accessible: db.publicly_accessible().unwrap_or(false),
            storage_type: db.storage_type().unwrap_or("").to_string(),
            iops: db.iops(),
            storage_encrypted: db.storage_encrypted().unwrap_or(false),
            kms_key_id: db.kms_key_id().map(|s| s.to_string()),
            master_username: db.master_username().unwrap_or("").to_string(),
            db_name: db.db_name().map(|s| s.to_string()),
            availability_zone: db.availability_zone().unwrap_or("").to_string(),
            secondary_availability_zone: db
                .secondary_availability_zone()
                .map(|s| s.to_string()),
            network_type: db.network_type().map(|s| s.to_string()),
            license_model: db.license_model().map(|s| s.to_string()),
            cluster_identifier: db.db_cluster_identifier().map(|s| s.to_string()),
            subnet_group: db
                .db_subnet_group()
                .and_then(|sg| sg.db_subnet_group_name())
                .map(|s| s.to_string()),
            vpc_id: db
                .db_subnet_group()
                .and_then(|sg| sg.vpc_id())
                .map(|s| s.to_string()),
            subnet_ids: db
                .db_subnet_group()
                .map(|sg| {
                    sg.subnets()
                        .iter()
                        .filter_map(|s| s.subnet_identifier().map(|i| i.to_string()))
                        .collect()
                })
                .unwrap_or_default(),
            vpc_security_groups,
            deletion_protection: db.deletion_protection().unwrap_or(false),
            iam_auth_enabled: db.iam_database_authentication_enabled().unwrap_or(false),
            backup_retention_days: db.backup_retention_period().unwrap_or(0),
            preferred_backup_window: db.preferred_backup_window().unwrap_or("").to_string(),
            preferred_maintenance_window: db
                .preferred_maintenance_window()
                .unwrap_or("")
                .to_string(),
            auto_minor_version_upgrade: db.auto_minor_version_upgrade().unwrap_or(false),
            copy_tags_to_snapshot: db.copy_tags_to_snapshot().unwrap_or(false),
            backup_target: db.backup_target().map(|s| s.to_string()),
            latest_restorable_time: db.latest_restorable_time().map(fmt_dt),
            ca_certificate_identifier: db.ca_certificate_identifier().map(|s| s.to_string()),
            ca_valid_till: db
                .certificate_details()
                .and_then(|c| c.valid_till())
                .map(fmt_dt),
            parameter_groups: db
                .db_parameter_groups()
                .iter()
                .map(|g| {
                    (
                        g.db_parameter_group_name().unwrap_or("").to_string(),
                        g.parameter_apply_status().unwrap_or("").to_string(),
                    )
                })
                .collect(),
            option_groups: db
                .option_group_memberships()
                .iter()
                .map(|g| {
                    (
                        g.option_group_name().unwrap_or("").to_string(),
                        g.status().unwrap_or("").to_string(),
                    )
                })
                .collect(),
            replica_source: db
                .read_replica_source_db_instance_identifier()
                .map(|s| s.to_string()),
            replica_ids: db
                .read_replica_db_instance_identifiers()
                .iter()
                .map(|s| s.to_string())
                .collect(),
            replica_mode: db.replica_mode().map(|m| m.to_string()),
            monitoring_interval: db.monitoring_interval().unwrap_or(0),
            monitoring_role_arn: db.monitoring_role_arn().map(|s| s.to_string()),
            pi_retention_days: db.performance_insights_retention_period(),
            pending_modifications: db
                .pending_modified_values()
                .map(instance_pending_rows)
                .unwrap_or_default(),
            enabled_log_exports: db
                .enabled_cloudwatch_logs_exports()
                .iter()
                .map(|s| s.to_string())
                .collect(),
            performance_insights_enabled: db.performance_insights_enabled().unwrap_or(false),
            dbi_resource_id: db.dbi_resource_id().map(|s| s.to_string()),
            create_time: db.instance_create_time().map(fmt_dt).unwrap_or_default(),
            tags,
        }
    }

    /// The CloudWatch Logs group + log-type label for the most useful enabled
    /// export, if any (instances publish to `/aws/rds/instance/<id>/<type>`).
    pub fn preferred_log_group(&self) -> Option<(String, String)> {
        let t = pick_log_export(&self.enabled_log_exports)?;
        Some((
            format!("/aws/rds/instance/{}/{}", self.db_identifier, t),
            t.clone(),
        ))
    }

    pub fn endpoint_display(&self) -> String {
        match (&self.endpoint_address, self.endpoint_port) {
            (Some(addr), Some(port)) => format!("{}:{}", addr, port),
            (Some(addr), None) => addr.clone(),
            _ => "-".to_string(),
        }
    }
}

crate::sections! {
    pub enum RdsInstanceDetailSection,
    pub static RDS_INSTANCE_SECTIONS = [
        Config "Config",
        Storage "Storage",
        Network "Network",
        PerfInsights "Perf Insights" => crate::app::App::trigger_rds_pi_load,
        Backups "Backups",
        Maintenance "Maintenance" => crate::app::App::trigger_rds_maintenance_load,
        Events "Events" => crate::app::App::trigger_rds_events_load,
        Logs "Logs" => crate::app::App::trigger_rds_log_files_load,
        Tags "Tags",
    ]
}

impl Resource for RdsInstance {
    fn references(&self) -> Vec<(String, String)> {
        let mut v = sg_refs(self.security_group_ids());
        let mut r = |l: &str, x: &str| v.push((l.to_string(), x.to_string()));
        for x in &self.subnet_ids { r("Subnet", x); }
        if let Some(x) = &self.vpc_id { r("VPC", x); }
        if let Some(x) = &self.subnet_group { r("Subnet Group", x); }
        if let Some(x) = &self.kms_key_id { r("KMS Key", x); }
        if let Some(x) = &self.cluster_identifier { r("Cluster", x); }
        for (pg, _) in &self.parameter_groups { r("Parameter Group", pg); }
        for (og, _) in &self.option_groups { r("Option Group", og); }
        if let Some(x) = &self.replica_source { r("Replica Source", x); }
        for x in &self.replica_ids { r("Read Replica", x); }
        if let Some(x) = &self.monitoring_role_arn { r("Monitoring Role", x); }
        v
    }

    fn security_group_ids(&self) -> Vec<String> {
        self.vpc_security_groups.clone()
    }

    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&RDS_INSTANCE_SECTIONS)
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws rds describe-db-instances --db-instance-identifier {}",
            crate::aws::resource::shell_quote(self.id())
        ))
    }

    fn id(&self) -> &str {
        &self.db_identifier
    }

    fn name(&self) -> &str {
        &self.db_identifier
    }

    fn resource_type(&self) -> &str {
        "RDS Instance"
    }

    fn state(&self) -> ResourceState {
        match self.status.as_str() {
            "available" => ResourceState::Running,
            "stopped" | "stopping" => ResourceState::Stopped,
            _ => ResourceState::Pending,
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
            self.db_identifier, self.engine, self.engine_version, self.db_instance_class,
            self.status,
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Identifier".to_string(), self.db_identifier.clone()),
            (
                "Engine".to_string(),
                format!("{} {}", self.engine, self.engine_version),
            ),
            ("Class".to_string(), self.db_instance_class.clone()),
            ("Status".to_string(), self.status.clone()),
            ("Endpoint".to_string(), self.endpoint_display()),
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
            "https://{}.console.aws.amazon.com/rds/home?region={}#database:id={};is-cluster=false",
            region, region, self.db_identifier
        ))
    }
}

// ── RdsCluster ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct RdsCluster {
    pub cluster_identifier: String,
    pub arn: String,
    pub engine: String,
    pub engine_version: String,
    pub status: String,
    pub endpoint: Option<String>,
    pub reader_endpoint: Option<String>,
    pub port: Option<i32>,
    pub multi_az: bool,
    pub engine_mode: String,
    pub master_username: String,
    pub db_name: Option<String>,
    pub deletion_protection: bool,
    pub storage_encrypted: bool,
    pub kms_key_id: Option<String>,
    pub iam_auth_enabled: bool,
    pub backup_retention_days: i32,
    pub preferred_backup_window: String,
    pub preferred_maintenance_window: String,
    pub auto_minor_version_upgrade: bool,
    pub copy_tags_to_snapshot: bool,
    pub earliest_restorable_time: Option<String>,
    pub latest_restorable_time: Option<String>,
    /// Aurora MySQL backtrack window (seconds; None/0 = disabled).
    pub backtrack_window_secs: Option<i64>,
    /// Aurora Serverless v2 capacity range (min, max ACU).
    pub serverless_v2_acu: Option<(f64, f64)>,
    pub cluster_parameter_group: Option<String>,
    /// "aurora" / "aurora-iopt1" / gp3+io1 for Multi-AZ DB clusters.
    pub storage_type: Option<String>,
    /// Only set for Multi-AZ DB clusters (Aurora reports storage dynamically).
    pub allocated_storage_gb: Option<i32>,
    pub iops: Option<i32>,
    pub network_type: Option<String>,
    pub subnet_group: Option<String>,
    pub vpc_security_groups: Vec<String>,
    /// Source cluster/instance when this cluster is a replica.
    pub replication_source: Option<String>,
    pub read_replica_identifiers: Vec<String>,
    pub global_write_forwarding_status: Option<String>,
    pub availability_zones: Vec<String>,
    /// (instance id, is writer).
    pub members: Vec<(String, bool)>,
    /// `ClusterPendingModifiedValues` pre-flattened; empty = none.
    pub pending_modifications: Vec<(String, String)>,
    pub create_time: String,
    /// Log types exported to CloudWatch Logs; each maps to
    /// `/aws/rds/cluster/<id>/<type>`.
    pub enabled_log_exports: Vec<String>,
    pub tags: HashMap<String, String>,
}

impl RdsCluster {
    pub fn from_sdk(c: &aws_sdk_rds::types::DbCluster) -> Self {
        let tags: HashMap<String, String> = c
            .tag_list()
            .iter()
            .filter_map(|t| Some((t.key()?.to_string(), t.value()?.to_string())))
            .collect();

        let members: Vec<(String, bool)> = c
            .db_cluster_members()
            .iter()
            .filter_map(|m| {
                m.db_instance_identifier()
                    .map(|s| (s.to_string(), m.is_cluster_writer().unwrap_or(false)))
            })
            .collect();

        let availability_zones: Vec<String> =
            c.availability_zones().iter().map(|az| az.to_string()).collect();

        Self {
            cluster_identifier: c.db_cluster_identifier().unwrap_or("").to_string(),
            arn: c.db_cluster_arn().unwrap_or("").to_string(),
            engine: c.engine().unwrap_or("").to_string(),
            engine_version: c.engine_version().unwrap_or("").to_string(),
            status: c.status().unwrap_or("").to_string(),
            endpoint: c.endpoint().map(|s| s.to_string()),
            reader_endpoint: c.reader_endpoint().map(|s| s.to_string()),
            port: c.port(),
            multi_az: c.multi_az().unwrap_or(false),
            engine_mode: c
                .engine_mode()
                .map(|m| m.to_string())
                .unwrap_or_default(),
            master_username: c.master_username().unwrap_or("").to_string(),
            db_name: c.database_name().map(|s| s.to_string()),
            deletion_protection: c.deletion_protection().unwrap_or(false),
            storage_encrypted: c.storage_encrypted().unwrap_or(false),
            kms_key_id: c.kms_key_id().map(|s| s.to_string()),
            iam_auth_enabled: c.iam_database_authentication_enabled().unwrap_or(false),
            backup_retention_days: c.backup_retention_period().unwrap_or(0),
            preferred_backup_window: c.preferred_backup_window().unwrap_or("").to_string(),
            preferred_maintenance_window: c
                .preferred_maintenance_window()
                .unwrap_or("")
                .to_string(),
            auto_minor_version_upgrade: c.auto_minor_version_upgrade().unwrap_or(false),
            copy_tags_to_snapshot: c.copy_tags_to_snapshot().unwrap_or(false),
            earliest_restorable_time: c.earliest_restorable_time().map(fmt_dt),
            latest_restorable_time: c.latest_restorable_time().map(fmt_dt),
            backtrack_window_secs: c.backtrack_window(),
            serverless_v2_acu: c.serverless_v2_scaling_configuration().and_then(|s| {
                Some((s.min_capacity()?, s.max_capacity()?))
            }),
            cluster_parameter_group: c.db_cluster_parameter_group().map(|s| s.to_string()),
            storage_type: c.storage_type().map(|s| s.to_string()),
            allocated_storage_gb: c.allocated_storage(),
            iops: c.iops(),
            network_type: c.network_type().map(|s| s.to_string()),
            subnet_group: c.db_subnet_group().map(|s| s.to_string()),
            vpc_security_groups: c
                .vpc_security_groups()
                .iter()
                .filter_map(|sg| sg.vpc_security_group_id().map(|s| s.to_string()))
                .collect(),
            replication_source: c.replication_source_identifier().map(|s| s.to_string()),
            read_replica_identifiers: c
                .read_replica_identifiers()
                .iter()
                .map(|s| s.to_string())
                .collect(),
            global_write_forwarding_status: c
                .global_write_forwarding_status()
                .map(|s| s.to_string()),
            availability_zones,
            members,
            pending_modifications: c
                .pending_modified_values()
                .map(cluster_pending_rows)
                .unwrap_or_default(),
            create_time: c.cluster_create_time().map(fmt_dt).unwrap_or_default(),
            enabled_log_exports: c
                .enabled_cloudwatch_logs_exports()
                .iter()
                .map(|s| s.to_string())
                .collect(),
            tags,
        }
    }
}

impl RdsCluster {
    /// The CloudWatch Logs group + log-type label for the most useful enabled
    /// export, if any (clusters publish to `/aws/rds/cluster/<id>/<type>`).
    pub fn preferred_log_group(&self) -> Option<(String, String)> {
        let t = pick_log_export(&self.enabled_log_exports)?;
        Some((
            format!("/aws/rds/cluster/{}/{}", self.cluster_identifier, t),
            t.clone(),
        ))
    }
}

/// Pick the most useful log export to tail (errors first), else the first one.
fn pick_log_export(exports: &[String]) -> Option<&String> {
    const PREFERRED: &[&str] = &[
        "error",
        "postgresql",
        "general",
        "slowquery",
        "audit",
        "upgrade",
    ];
    PREFERRED
        .iter()
        .find_map(|p| exports.iter().find(|e| e.as_str() == *p))
        .or_else(|| exports.first())
}

crate::sections! {
    pub enum RdsClusterDetailSection,
    pub static RDS_CLUSTER_SECTIONS = [
        Config "Config",
        Endpoints "Endpoints",
        Members "Members",
        Backups "Backups",
        Maintenance "Maintenance" => crate::app::App::trigger_rds_maintenance_load,
        Events "Events" => crate::app::App::trigger_rds_events_load,
        Tags "Tags",
    ]
}

impl Resource for RdsCluster {
    fn references(&self) -> Vec<(String, String)> {
        let mut v = sg_refs(self.security_group_ids());
        let mut r = |l: &str, x: &str| v.push((l.to_string(), x.to_string()));
        if let Some(x) = &self.kms_key_id { r("KMS Key", x); }
        if let Some(x) = &self.subnet_group { r("Subnet Group", x); }
        if let Some(x) = &self.cluster_parameter_group { r("Parameter Group", x); }
        for (m, _) in &self.members { r("Member", m); }
        if let Some(x) = &self.replication_source { r("Replication Source", x); }
        for x in &self.read_replica_identifiers { r("Read Replica", x); }
        v
    }

    fn security_group_ids(&self) -> Vec<String> {
        self.vpc_security_groups.clone()
    }

    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&RDS_CLUSTER_SECTIONS)
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws rds describe-db-clusters --db-cluster-identifier {}",
            crate::aws::resource::shell_quote(self.id())
        ))
    }

    fn id(&self) -> &str {
        &self.cluster_identifier
    }

    fn name(&self) -> &str {
        &self.cluster_identifier
    }

    fn resource_type(&self) -> &str {
        "RDS Cluster"
    }

    fn state(&self) -> ResourceState {
        match self.status.as_str() {
            "available" => ResourceState::Running,
            "stopped" | "stopping" => ResourceState::Stopped,
            _ => ResourceState::Pending,
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
            self.cluster_identifier, self.engine, self.engine_version, self.engine_mode,
            self.status,
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Cluster".to_string(), self.cluster_identifier.clone()),
            (
                "Engine".to_string(),
                format!("{} {}", self.engine, self.engine_version),
            ),
            ("Mode".to_string(), self.engine_mode.clone()),
            ("Status".to_string(), self.status.clone()),
            ("Members".to_string(), self.members.len().to_string()),
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
            "https://{}.console.aws.amazon.com/rds/home?region={}#database:id={};is-cluster=true",
            region, region, self.cluster_identifier
        ))
    }
}

// ── RdsSnapshot ───────────────────────────────────────────────────────────────

/// A DB snapshot — instance (`DescribeDBSnapshots`) or cluster
/// (`DescribeDBClusterSnapshots`). Both APIs default to self-owned snapshots
/// (manual + automated), which is what we want; shared/public are excluded.
#[derive(Debug, Clone)]
pub struct RdsSnapshot {
    pub id: String,
    /// Source DB instance or cluster identifier.
    pub source: String,
    pub is_cluster: bool,
    pub engine: String,
    pub engine_version: String,
    /// "manual" / "automated" / "awsbackup" / …
    pub snapshot_type: String,
    pub allocated_storage: i32,
    pub status: String,
    /// 0–100; only meaningful while `status` is "creating".
    pub percent_progress: i32,
    pub encrypted: bool,
    pub created: Option<String>,
    pub kms_key_id: Option<String>,
    pub port: Option<i32>,
    pub vpc_id: Option<String>,
    /// Instance snapshots only (cluster snapshots span AZs).
    pub availability_zone: Option<String>,
    pub license_model: Option<String>,
    pub iam_auth_enabled: bool,
    pub storage_type: Option<String>,
    pub storage_throughput: Option<i32>,
    /// When the source DB/cluster was created (restore-vintage signal).
    pub source_created: Option<String>,
    /// The snapshot this one was copied from, when it's a copy.
    pub copy_source: Option<String>,
    pub tags: HashMap<String, String>,
}

impl RdsSnapshot {
    pub fn from_db(s: &aws_sdk_rds::types::DbSnapshot) -> Self {
        Self {
            id: s.db_snapshot_identifier().unwrap_or("").to_string(),
            source: s.db_instance_identifier().unwrap_or("").to_string(),
            is_cluster: false,
            engine: s.engine().unwrap_or("").to_string(),
            engine_version: s.engine_version().unwrap_or("").to_string(),
            snapshot_type: s.snapshot_type().unwrap_or("").to_string(),
            allocated_storage: s.allocated_storage().unwrap_or(0),
            status: s.status().unwrap_or("").to_string(),
            percent_progress: s.percent_progress().unwrap_or(0),
            encrypted: s.encrypted().unwrap_or(false),
            created: s.snapshot_create_time().map(fmt_dt),
            kms_key_id: s.kms_key_id().map(|s| s.to_string()),
            port: s.port(),
            vpc_id: s.vpc_id().map(|s| s.to_string()),
            availability_zone: s.availability_zone().map(|s| s.to_string()),
            license_model: s.license_model().map(|s| s.to_string()),
            iam_auth_enabled: s.iam_database_authentication_enabled().unwrap_or(false),
            storage_type: s.storage_type().map(|s| s.to_string()),
            storage_throughput: s.storage_throughput(),
            source_created: s.instance_create_time().map(fmt_dt),
            copy_source: s.source_db_snapshot_identifier().map(|s| s.to_string()),
            tags: s
                .tag_list()
                .iter()
                .filter_map(|t| Some((t.key()?.to_string(), t.value()?.to_string())))
                .collect(),
        }
    }

    pub fn from_cluster(s: &aws_sdk_rds::types::DbClusterSnapshot) -> Self {
        Self {
            id: s.db_cluster_snapshot_identifier().unwrap_or("").to_string(),
            source: s.db_cluster_identifier().unwrap_or("").to_string(),
            is_cluster: true,
            engine: s.engine().unwrap_or("").to_string(),
            engine_version: s.engine_version().unwrap_or("").to_string(),
            snapshot_type: s.snapshot_type().unwrap_or("").to_string(),
            allocated_storage: s.allocated_storage().unwrap_or(0),
            status: s.status().unwrap_or("").to_string(),
            percent_progress: s.percent_progress().unwrap_or(0),
            encrypted: s.storage_encrypted().unwrap_or(false),
            created: s.snapshot_create_time().map(fmt_dt),
            kms_key_id: s.kms_key_id().map(|s| s.to_string()),
            port: s.port(),
            vpc_id: s.vpc_id().map(|s| s.to_string()),
            availability_zone: None,
            license_model: s.license_model().map(|s| s.to_string()),
            iam_auth_enabled: s.iam_database_authentication_enabled().unwrap_or(false),
            storage_type: s.storage_type().map(|s| s.to_string()),
            storage_throughput: None,
            source_created: s.cluster_create_time().map(fmt_dt),
            copy_source: s.source_db_cluster_snapshot_arn().map(|s| s.to_string()),
            tags: s
                .tag_list()
                .iter()
                .filter_map(|t| Some((t.key()?.to_string(), t.value()?.to_string())))
                .collect(),
        }
    }
}

crate::sections! {
    pub enum RdsSnapshotDetailSection,
    pub static RDS_SNAPSHOT_SECTIONS = [
        Overview "Overview",
        Encryption "Encryption",
        Tags "Tags",
    ]
}

impl Resource for RdsSnapshot {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&RDS_SNAPSHOT_SECTIONS)
    }

    fn cli_command(&self) -> Option<String> {
        Some(if self.is_cluster {
            format!(
                "aws rds describe-db-cluster-snapshots --db-cluster-snapshot-identifier {}",
                crate::aws::resource::shell_quote(&self.id)
            )
        } else {
            format!(
                "aws rds describe-db-snapshots --db-snapshot-identifier {}",
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
        "RDS Snapshot"
    }

    fn state(&self) -> ResourceState {
        match self.status.as_str() {
            "available" => ResourceState::Available,
            "creating" => ResourceState::Creating,
            "deleting" | "deleted" => ResourceState::Deleting,
            "failed" | "incompatible-restore" => ResourceState::Unavailable,
            other => ResourceState::Unknown(other.to_string()),
        }
    }

    fn state_label(&self) -> String {
        native_state_label(&self.status, || self.state())
    }

    /// System-generated snapshots (automated daily / AWS Backup) are the bulk
    /// of most accounts' lists — `a` hides them, like Redshift's automated
    /// snapshots. Manual snapshots always show.
    fn is_noise(&self) -> bool {
        self.snapshot_type != "manual"
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        let mut parts = vec![
            self.id.clone(),
            self.source.clone(),
            self.engine.clone(),
            self.snapshot_type.clone(),
        ];
        for (k, v) in &self.tags {
            parts.push(format!("{}:{}", k, v));
        }
        parts.join(" ")
    }

    fn details(&self) -> Vec<(String, String)> {
        let mut rows = vec![
            ("Snapshot ID".to_string(), self.id.clone()),
            (
                if self.is_cluster { "Source Cluster".to_string() } else { "Source DB".to_string() },
                self.source.clone(),
            ),
            ("Type".to_string(), self.snapshot_type.clone()),
            ("Engine".to_string(), format!("{} {}", self.engine, self.engine_version)),
            ("Status".to_string(), self.status.clone()),
            ("Allocated Storage".to_string(), format!("{} GiB", self.allocated_storage)),
            (
                "Encrypted".to_string(),
                if self.encrypted { "✓ yes".to_string() } else { "✗ no".to_string() },
            ),
        ];
        // Surface the encrypting KMS key (jumpable to KMS via the generic classifier).
        if let Some(kms) = &self.kms_key_id {
            rows.push(("KMS Key".to_string(), kms.clone()));
        }
        rows.push((
            "Created".to_string(),
            self.created.clone().unwrap_or_else(|| "—".to_string()),
        ));
        rows
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/rds/home?region={}#db-snapshot:id={}",
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

// ── Performance Insights (lazy, `pi:GetResourceMetrics`) ──────────────────────
//
// `db.load.avg` (average active sessions) grouped by `db.wait_event` is the
// single best "what is my DB doing / waiting on" signal. We aggregate each wait
// event's datapoints to a mean over the window and rank them; the total load is
// the ungrouped series. Only meaningful when Performance Insights is enabled on
// the instance (`RdsInstance::performance_insights_enabled`).

#[derive(Debug, Clone)]
pub struct RdsPiData {
    /// (wait event name, average active sessions), highest first.
    pub waits: Vec<(String, f64)>,
    /// Average total DB load (active sessions) over the window.
    pub total_load: f64,
}

/// Fetch the last hour of `db.load.avg` grouped by wait event for one instance.
pub async fn fetch_rds_pi(
    pi: aws_sdk_pi::Client,
    dbi_resource_id: String,
) -> Result<RdsPiData> {
    use aws_sdk_pi::types::{DimensionGroup, MetricQuery, ServiceType as PiServiceType};

    let now = std::time::SystemTime::now();
    let start = now - std::time::Duration::from_secs(3600);
    let start_dt = aws_sdk_pi::primitives::DateTime::from(start);
    let end_dt = aws_sdk_pi::primitives::DateTime::from(now);

    let group = DimensionGroup::builder()
        .group("db.wait_event")
        .limit(10)
        .build()
        .map_err(|e| crate::error::Error::AwsSdk(e.to_string()))?;
    let query = MetricQuery::builder()
        .metric("db.load.avg")
        .group_by(group)
        .build()
        .map_err(|e| crate::error::Error::AwsSdk(e.to_string()))?;

    let resp = pi
        .get_resource_metrics()
        .service_type(PiServiceType::Rds)
        .identifier(&dbi_resource_id)
        .metric_queries(query)
        .start_time(start_dt)
        .end_time(end_dt)
        .period_in_seconds(60)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;

    let mean = |dps: &[aws_sdk_pi::types::DataPoint]| -> f64 {
        // DataPoint::value() is f64; PI uses NaN for gaps, so filter those out.
        let vals: Vec<f64> = dps.iter().map(|d| d.value()).filter(|v| !v.is_nan()).collect();
        if vals.is_empty() {
            0.0
        } else {
            vals.iter().sum::<f64>() / vals.len() as f64
        }
    };

    let mut waits: Vec<(String, f64)> = Vec::new();
    let mut total_load = 0.0;
    for series in resp.metric_list() {
        let avg = mean(series.data_points());
        let dims = series.key().and_then(|k| k.dimensions());
        match dims {
            // Grouped series carry the wait-event name in the dimension map.
            Some(m) if !m.is_empty() => {
                let name = m
                    .get("db.wait_event.name")
                    .or_else(|| m.values().next())
                    .cloned()
                    .unwrap_or_else(|| "unknown".to_string());
                waits.push((name, avg));
            }
            // The ungrouped series (no dimensions) is the total DB load.
            _ => total_load = avg,
        }
    }
    waits.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    Ok(RdsPiData { waits, total_load })
}

// ── RdsParamGroup / RdsOptionGroup / RdsSubnetGroup ──────────────────────────
//
// The three supporting resource types the console gives dedicated pages:
// parameter groups (instance + cluster unified, like snapshots), option
// groups, and subnet groups. All stream as best-effort phases after the
// snapshots — a permission gap warns and never breaks the core load.

#[derive(Debug, Clone)]
pub struct RdsParamGroup {
    pub name: String,
    pub arn: String,
    pub family: String,
    pub description: String,
    /// Cluster parameter group (`DescribeDBClusterParameterGroups`) vs
    /// instance (`DescribeDBParameterGroups`).
    pub is_cluster: bool,
    pub tags: HashMap<String, String>,
}

impl RdsParamGroup {
    pub fn from_sdk(g: &aws_sdk_rds::types::DbParameterGroup) -> Self {
        Self {
            name: g.db_parameter_group_name().unwrap_or("").to_string(),
            arn: g.db_parameter_group_arn().unwrap_or("").to_string(),
            family: g.db_parameter_group_family().unwrap_or("").to_string(),
            description: g.description().unwrap_or("").to_string(),
            is_cluster: false,
            tags: HashMap::new(),
        }
    }

    pub fn from_cluster_sdk(g: &aws_sdk_rds::types::DbClusterParameterGroup) -> Self {
        Self {
            name: g.db_cluster_parameter_group_name().unwrap_or("").to_string(),
            arn: g.db_cluster_parameter_group_arn().unwrap_or("").to_string(),
            family: g.db_parameter_group_family().unwrap_or("").to_string(),
            description: g.description().unwrap_or("").to_string(),
            is_cluster: true,
            tags: HashMap::new(),
        }
    }

    /// The `lazy.rds_parameters` key (name alone could collide across kinds).
    pub fn params_key(&self) -> String {
        format!(
            "{}:{}",
            if self.is_cluster { "cluster" } else { "db" },
            self.name
        )
    }
}

crate::sections! {
    pub enum RdsParamGroupDetailSection,
    pub static RDS_PARAM_GROUP_SECTIONS = [
        Overview "Overview",
        Parameters "Parameters" => crate::app::App::trigger_rds_parameters_load,
    ]
}

impl Resource for RdsParamGroup {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&RDS_PARAM_GROUP_SECTIONS)
    }

    fn cli_command(&self) -> Option<String> {
        Some(if self.is_cluster {
            format!(
                "aws rds describe-db-cluster-parameter-groups --db-cluster-parameter-group-name {}",
                crate::aws::resource::shell_quote(&self.name)
            )
        } else {
            format!(
                "aws rds describe-db-parameter-groups --db-parameter-group-name {}",
                crate::aws::resource::shell_quote(&self.name)
            )
        })
    }

    fn id(&self) -> &str {
        &self.name
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn resource_type(&self) -> &str {
        "RDS Parameter Group"
    }

    fn state(&self) -> ResourceState {
        ResourceState::stateless()
    }

    /// The engine-default `default.*` groups exist in every account and can't
    /// be modified — `a` hides them.
    fn is_noise(&self) -> bool {
        self.name.starts_with("default.")
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!(
            "{} {} {} {}",
            self.name,
            self.family,
            if self.is_cluster { "cluster" } else { "instance" },
            self.description,
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Name".to_string(), self.name.clone()),
            (
                "Kind".to_string(),
                if self.is_cluster { "cluster".to_string() } else { "instance".to_string() },
            ),
            ("Family".to_string(), self.family.clone()),
            ("Description".to_string(), self.description.clone()),
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
            "https://{}.console.aws.amazon.com/rds/home?region={}#parameter-groups:",
            region, region
        ))
    }
}

/// One engine parameter (Parameters section rows). Only the fields the pane
/// shows — the full description is available via the CLI command.
#[derive(Debug, Clone)]
pub struct RdsParameter {
    pub name: String,
    pub value: Option<String>,
    /// "user" = explicitly overridden — the signal rows, sorted first.
    pub source: String,
    pub apply_type: String,
    pub is_modifiable: bool,
}

/// Parameter list cap — engine families carry 500+ mostly-default params.
pub const MAX_RDS_PARAMETERS: usize = 500;

/// Fetch a parameter group's parameters, user-overridden first, capped at
/// [`MAX_RDS_PARAMETERS`]. Returns `(params, total_count)` so the pane can
/// report what the cap hid.
pub async fn fetch_rds_parameters(
    client: RdsClient,
    name: String,
    is_cluster: bool,
) -> Result<(Vec<RdsParameter>, usize)> {
    let mut params: Vec<RdsParameter> = Vec::new();

    let from_sdk = |p: &aws_sdk_rds::types::Parameter| RdsParameter {
        name: p.parameter_name().unwrap_or("").to_string(),
        value: p.parameter_value().map(|s| s.to_string()),
        source: p.source().unwrap_or("").to_string(),
        apply_type: p.apply_type().unwrap_or("").to_string(),
        is_modifiable: p.is_modifiable().unwrap_or(false),
    };

    if is_cluster {
        let mut pg = client
            .describe_db_cluster_parameters()
            .db_cluster_parameter_group_name(&name)
            .into_paginator()
            .send();
        while let Some(page) = pg.next().await {
            let page = page
                .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
            params.extend(page.parameters().iter().map(from_sdk));
        }
    } else {
        let mut pg = client
            .describe_db_parameters()
            .db_parameter_group_name(&name)
            .into_paginator()
            .send();
        while let Some(page) = pg.next().await {
            let page = page
                .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
            params.extend(page.parameters().iter().map(from_sdk));
        }
    }

    let total = params.len();
    // User-overridden first (the reason anyone opens this pane), then by name.
    params.sort_by(|a, b| {
        let a_user = a.source == "user";
        let b_user = b.source == "user";
        b_user.cmp(&a_user).then_with(|| a.name.cmp(&b.name))
    });
    params.truncate(MAX_RDS_PARAMETERS);
    Ok((params, total))
}

/// One option inside an option group, pre-flattened for display.
#[derive(Debug, Clone)]
pub struct RdsOption {
    pub name: String,
    pub version: Option<String>,
    pub port: Option<i32>,
    /// (setting name, value) — modified settings only when the group has any.
    pub settings: Vec<(String, String)>,
    pub security_groups: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct RdsOptionGroup {
    pub name: String,
    pub arn: String,
    pub description: String,
    pub engine: String,
    pub major_engine_version: String,
    pub vpc_id: Option<String>,
    pub options: Vec<RdsOption>,
    pub tags: HashMap<String, String>,
}

impl RdsOptionGroup {
    pub fn from_sdk(g: &aws_sdk_rds::types::OptionGroup) -> Self {
        let options: Vec<RdsOption> = g
            .options()
            .iter()
            .map(|o| RdsOption {
                name: o.option_name().unwrap_or("").to_string(),
                version: o.option_version().map(|s| s.to_string()),
                port: o.port(),
                settings: o
                    .option_settings()
                    .iter()
                    .filter_map(|s| {
                        Some((s.name()?.to_string(), s.value().unwrap_or("").to_string()))
                    })
                    .collect(),
                security_groups: o
                    .vpc_security_group_memberships()
                    .iter()
                    .filter_map(|m| m.vpc_security_group_id().map(|s| s.to_string()))
                    .collect(),
            })
            .collect();
        Self {
            name: g.option_group_name().unwrap_or("").to_string(),
            arn: g.option_group_arn().unwrap_or("").to_string(),
            description: g.option_group_description().unwrap_or("").to_string(),
            engine: g.engine_name().unwrap_or("").to_string(),
            major_engine_version: g.major_engine_version().unwrap_or("").to_string(),
            vpc_id: g.vpc_id().map(|s| s.to_string()),
            options,
            tags: HashMap::new(),
        }
    }
}

impl Resource for RdsOptionGroup {
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws rds describe-option-groups --option-group-name {}",
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
        "RDS Option Group"
    }

    fn state(&self) -> ResourceState {
        ResourceState::stateless()
    }

    /// The engine-default `default:*` groups exist for every engine — `a`
    /// hides them.
    fn is_noise(&self) -> bool {
        self.name.starts_with("default:")
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        let opts: Vec<&str> = self.options.iter().map(|o| o.name.as_str()).collect();
        format!(
            "{} {} {} {}",
            self.name,
            self.engine,
            self.major_engine_version,
            opts.join(" "),
        )
    }

    // Complete as flat: overview rows + one block per option.
    fn details(&self) -> Vec<(String, String)> {
        let mut rows = vec![
            ("Name".to_string(), self.name.clone()),
            (
                "Engine".to_string(),
                format!("{} {}", self.engine, self.major_engine_version),
            ),
            ("Description".to_string(), self.description.clone()),
        ];
        if let Some(vpc) = &self.vpc_id {
            rows.push(("VPC".to_string(), vpc.clone()));
        }
        if !self.arn.is_empty() {
            rows.push(("ARN".to_string(), self.arn.clone()));
        }
        rows.push((String::new(), String::new()));
        rows.push((format!("Options ({})", self.options.len()), String::new()));
        if self.options.is_empty() {
            rows.push(("  · none configured".to_string(), String::new()));
        }
        for o in &self.options {
            rows.push((String::new(), String::new()));
            let title = match &o.version {
                Some(v) => format!("{} ({})", o.name, v),
                None => o.name.clone(),
            };
            rows.push((format!("  Option: {}", title), String::new()));
            if let Some(p) = o.port {
                rows.push(("  Port".to_string(), p.to_string()));
            }
            for sg in &o.security_groups {
                rows.push(("  Security Group".to_string(), sg.clone()));
            }
            for (k, v) in &o.settings {
                rows.push((format!("    {}", k), v.clone()));
            }
        }
        rows
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/rds/home?region={}#option-groups:",
            region, region
        ))
    }
}

#[derive(Debug, Clone)]
pub struct RdsSubnetGroup {
    pub name: String,
    pub arn: String,
    pub description: String,
    pub vpc_id: Option<String>,
    pub status: String,
    /// (subnet id, AZ, status).
    pub subnets: Vec<(String, String, String)>,
    pub network_types: Vec<String>,
    pub tags: HashMap<String, String>,
}

impl RdsSubnetGroup {
    pub fn from_sdk(g: &aws_sdk_rds::types::DbSubnetGroup) -> Self {
        Self {
            name: g.db_subnet_group_name().unwrap_or("").to_string(),
            arn: g.db_subnet_group_arn().unwrap_or("").to_string(),
            description: g.db_subnet_group_description().unwrap_or("").to_string(),
            vpc_id: g.vpc_id().map(|s| s.to_string()),
            status: g.subnet_group_status().unwrap_or("").to_string(),
            subnets: g
                .subnets()
                .iter()
                .map(|s| {
                    (
                        s.subnet_identifier().unwrap_or("").to_string(),
                        s.subnet_availability_zone()
                            .and_then(|z| z.name())
                            .unwrap_or("")
                            .to_string(),
                        s.subnet_status().unwrap_or("").to_string(),
                    )
                })
                .collect(),
            network_types: g
                .supported_network_types()
                .iter()
                .map(|s| s.to_string())
                .collect(),
            tags: HashMap::new(),
        }
    }
}

impl Resource for RdsSubnetGroup {
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws rds describe-db-subnet-groups --db-subnet-group-name {}",
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
        "RDS Subnet Group"
    }

    fn state(&self) -> ResourceState {
        if self.status == "Complete" {
            ResourceState::Available
        } else {
            ResourceState::Pending
        }
    }

    fn state_label(&self) -> String {
        native_state_label(&self.status, || self.state())
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        let subnets: Vec<&str> = self.subnets.iter().map(|(id, _, _)| id.as_str()).collect();
        format!(
            "{} {} {} {}",
            self.name,
            self.vpc_id.as_deref().unwrap_or(""),
            self.description,
            subnets.join(" "),
        )
    }

    // Complete as flat: subnet ids and the VPC are key-value rows, so the
    // generic classifier makes them Enter-jumpable.
    fn details(&self) -> Vec<(String, String)> {
        let mut rows = vec![
            ("Name".to_string(), self.name.clone()),
            ("Description".to_string(), self.description.clone()),
            ("Status".to_string(), self.status.clone()),
        ];
        if let Some(vpc) = &self.vpc_id {
            rows.push(("VPC".to_string(), vpc.clone()));
        }
        if !self.network_types.is_empty() {
            rows.push(("Network Types".to_string(), self.network_types.join(", ")));
        }
        if !self.arn.is_empty() {
            rows.push(("ARN".to_string(), self.arn.clone()));
        }
        rows.push((String::new(), String::new()));
        rows.push((format!("Subnets ({})", self.subnets.len()), String::new()));
        for (id, az, status) in &self.subnets {
            let label = if status == "Active" {
                az.clone()
            } else {
                format!("{} · ⚠ {}", az, status)
            };
            rows.push((format!("  {}", label), id.clone()));
        }
        rows
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/rds/home?region={}#db-subnet-groups-list:",
            region, region
        ))
    }
}

// ── DB log files (lazy `DescribeDBLogFiles` + on-demand download) ────────────

/// Newest-first cap for the Logs section.
pub const MAX_RDS_LOG_FILES: usize = 100;
/// Lines pulled when opening a log file in the editor.
const RDS_LOG_TAIL_LINES: i32 = 5000;

#[derive(Debug, Clone)]
pub struct RdsLogFile {
    pub name: String,
    pub size_bytes: i64,
    pub last_written: Option<String>,
}

/// List an instance's native DB log files (error/, postgresql.log.*, …),
/// newest first, capped at [`MAX_RDS_LOG_FILES`]. Returns `(files, total)`.
pub async fn fetch_rds_log_files(
    client: RdsClient,
    db_id: String,
) -> Result<(Vec<RdsLogFile>, usize)> {
    let mut files: Vec<(i64, RdsLogFile)> = Vec::new();
    let mut pg = client
        .describe_db_log_files()
        .db_instance_identifier(&db_id)
        .into_paginator()
        .send();
    while let Some(page) = pg.next().await {
        let page =
            page.map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
        for f in page.describe_db_log_files() {
            let written_ms = f.last_written().unwrap_or(0);
            files.push((
                written_ms,
                RdsLogFile {
                    name: f.log_file_name().unwrap_or("").to_string(),
                    size_bytes: f.size().unwrap_or(0),
                    last_written: (written_ms > 0).then(|| {
                        fmt_dt(&aws_sdk_rds::primitives::DateTime::from_millis(written_ms))
                    }),
                },
            ));
        }
    }
    let total = files.len();
    files.sort_by_key(|(written, _)| std::cmp::Reverse(*written));
    files.truncate(MAX_RDS_LOG_FILES);
    Ok((files.into_iter().map(|(_, f)| f).collect(), total))
}

/// Download the most recent lines of one log file (the `e`-on-a-log-row
/// editor path). A partial read is prefixed with a truncation note.
pub async fn fetch_rds_log_tail(
    client: RdsClient,
    db_id: String,
    file_name: String,
) -> Result<String> {
    let resp = client
        .download_db_log_file_portion()
        .db_instance_identifier(&db_id)
        .log_file_name(&file_name)
        // With no marker this returns the most recent N lines.
        .number_of_lines(RDS_LOG_TAIL_LINES)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;

    let data = resp.log_file_data().unwrap_or("").to_string();
    if data.is_empty() {
        return Ok(format!("({} is empty)\n", file_name));
    }
    Ok(if resp.additional_data_pending().unwrap_or(false) {
        format!(
            "── {} — most recent {} lines (file continues above) ──\n{}",
            file_name, RDS_LOG_TAIL_LINES, data
        )
    } else {
        format!("── {} ──\n{}", file_name, data)
    })
}

// ── Pending maintenance actions (lazy, `DescribePendingMaintenanceActions`) ──

/// One pending maintenance action on an instance or cluster (OS patch,
/// certificate rotation, engine upgrade …).
#[derive(Debug, Clone)]
pub struct RdsPendingAction {
    pub action: String,
    pub description: Option<String>,
    pub opt_in_status: Option<String>,
    /// The date the action will actually run (the effective schedule).
    pub current_apply_date: Option<String>,
    /// Applied in the next maintenance window after this date.
    pub auto_applied_after: Option<String>,
    /// Hard deadline — applied regardless of windows after this date.
    pub forced_apply_date: Option<String>,
}

/// Fetch the pending maintenance actions for one resource (instance or
/// cluster), addressed by ARN. Empty = nothing scheduled (the common case).
pub async fn fetch_rds_pending_maintenance(
    client: RdsClient,
    arn: String,
) -> Result<Vec<RdsPendingAction>> {
    let resp = client
        .describe_pending_maintenance_actions()
        .resource_identifier(&arn)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;

    let mut actions = Vec::new();
    for res in resp.pending_maintenance_actions() {
        for a in res.pending_maintenance_action_details() {
            actions.push(RdsPendingAction {
                action: a.action().unwrap_or("").to_string(),
                description: a.description().map(|s| s.to_string()),
                opt_in_status: a.opt_in_status().map(|s| s.to_string()),
                current_apply_date: a.current_apply_date().map(fmt_dt),
                auto_applied_after: a.auto_applied_after_date().map(fmt_dt),
                forced_apply_date: a.forced_apply_date().map(fmt_dt),
            });
        }
    }
    Ok(actions)
}

// ── Recent events (lazy, `DescribeEvents`) ───────────────────────────────────

/// The API's ceiling is 14 days; that's the window we show.
const RDS_EVENTS_WINDOW_MINUTES: i32 = 14 * 24 * 60;
/// Newest-first display cap.
const MAX_RDS_EVENTS: usize = 100;

#[derive(Debug, Clone)]
pub struct RdsEvent {
    pub date: String,
    pub message: String,
    pub categories: Vec<String>,
}

/// Fetch the last 14 days of events for one instance or cluster, newest first.
pub async fn fetch_rds_events(
    client: RdsClient,
    source_id: String,
    is_cluster: bool,
) -> Result<Vec<RdsEvent>> {
    use aws_sdk_rds::types::SourceType;

    let source_type = if is_cluster {
        SourceType::DbCluster
    } else {
        SourceType::DbInstance
    };

    let mut events: Vec<RdsEvent> = Vec::new();
    let mut paginator = client
        .describe_events()
        .source_identifier(&source_id)
        .source_type(source_type)
        .duration(RDS_EVENTS_WINDOW_MINUTES)
        .into_paginator()
        .send();
    while let Some(page) = paginator.next().await {
        let page =
            page.map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
        for e in page.events() {
            events.push(RdsEvent {
                date: e.date().map(fmt_dt).unwrap_or_default(),
                message: e.message().unwrap_or("").to_string(),
                categories: e.event_categories().iter().map(|c| c.to_string()).collect(),
            });
        }
    }
    // The API returns chronological order; show newest first, capped.
    events.reverse();
    events.truncate(MAX_RDS_EVENTS);
    Ok(events)
}

// ── RDS metrics (CloudWatch) ──────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct RdsMetricsData {
    pub time_range: MetricsTimeRange,
    pub cpu: Vec<(f64, f64)>,          // % average
    pub connections: Vec<(f64, f64)>,  // count average
    pub read_iops: Vec<(f64, f64)>,    // ops/sec average
    pub write_iops: Vec<(f64, f64)>,   // ops/sec average
    pub free_storage: Vec<(f64, f64)>, // bytes average (display as GB)
    pub free_memory: Vec<(f64, f64)>,  // bytes average (display as GB)
    pub x_max: f64,
}

#[derive(Debug, Clone)]
pub enum RdsMetricsState {
    Loading,
    Loaded(RdsMetricsData),
}

fn parse_dp_avg(datapoints: &[Datapoint], start_secs: i64) -> Vec<(f64, f64)> {
    let mut pts: Vec<(f64, f64)> = datapoints
        .iter()
        .filter_map(|dp| {
            let t = (dp.timestamp()?.secs() - start_secs) as f64;
            Some((t, dp.average().unwrap_or(0.0)))
        })
        .collect();
    pts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    pts
}

pub async fn fetch_rds_metrics(
    cw_client: aws_sdk_cloudwatch::Client,
    identifier: String,
    dimension_name: &str,
    time_range: MetricsTimeRange,
) -> Result<RdsMetricsData> {
    use aws_sdk_cloudwatch::types::{Dimension, Statistic};

    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let start_secs = now_secs - time_range.duration_secs();
    let period = time_range.period_secs();

    let start_dt = aws_sdk_cloudwatch::primitives::DateTime::from_secs(start_secs);
    let end_dt = aws_sdk_cloudwatch::primitives::DateTime::from_secs(now_secs);

    // Instances key on DBInstanceIdentifier; Aurora/Multi-AZ clusters roll up
    // under DBClusterIdentifier (some per-instance metrics may be empty there).
    let make_dim = || {
        Dimension::builder()
            .name(dimension_name)
            .value(&identifier)
            .build()
    };

    let (cpu_r, conn_r, read_r, write_r, store_r, mem_r) = tokio::join!(
        cw_client
            .get_metric_statistics()
            .namespace("AWS/RDS")
            .metric_name("CPUUtilization")
            .dimensions(make_dim())
            .start_time(start_dt.clone())
            .end_time(end_dt.clone())
            .period(period)
            .set_statistics(Some(vec![Statistic::Average]))
            .send(),
        cw_client
            .get_metric_statistics()
            .namespace("AWS/RDS")
            .metric_name("DatabaseConnections")
            .dimensions(make_dim())
            .start_time(start_dt.clone())
            .end_time(end_dt.clone())
            .period(period)
            .set_statistics(Some(vec![Statistic::Average]))
            .send(),
        cw_client
            .get_metric_statistics()
            .namespace("AWS/RDS")
            .metric_name("ReadIOPS")
            .dimensions(make_dim())
            .start_time(start_dt.clone())
            .end_time(end_dt.clone())
            .period(period)
            .set_statistics(Some(vec![Statistic::Average]))
            .send(),
        cw_client
            .get_metric_statistics()
            .namespace("AWS/RDS")
            .metric_name("WriteIOPS")
            .dimensions(make_dim())
            .start_time(start_dt.clone())
            .end_time(end_dt.clone())
            .period(period)
            .set_statistics(Some(vec![Statistic::Average]))
            .send(),
        cw_client
            .get_metric_statistics()
            .namespace("AWS/RDS")
            .metric_name("FreeStorageSpace")
            .dimensions(make_dim())
            .start_time(start_dt.clone())
            .end_time(end_dt.clone())
            .period(period)
            .set_statistics(Some(vec![Statistic::Average]))
            .send(),
        cw_client
            .get_metric_statistics()
            .namespace("AWS/RDS")
            .metric_name("FreeableMemory")
            .dimensions(make_dim())
            .start_time(start_dt)
            .end_time(end_dt)
            .period(period)
            .set_statistics(Some(vec![Statistic::Average]))
            .send(),
    );

    let cpu_dp = match cpu_r { Ok(r) => r.datapoints().to_vec(), Err(_) => vec![] };
    let conn_dp = match conn_r { Ok(r) => r.datapoints().to_vec(), Err(_) => vec![] };
    let read_dp = match read_r { Ok(r) => r.datapoints().to_vec(), Err(_) => vec![] };
    let write_dp = match write_r { Ok(r) => r.datapoints().to_vec(), Err(_) => vec![] };
    let store_dp = match store_r { Ok(r) => r.datapoints().to_vec(), Err(_) => vec![] };
    let mem_dp = match mem_r { Ok(r) => r.datapoints().to_vec(), Err(_) => vec![] };

    Ok(RdsMetricsData {
        time_range,
        cpu: parse_dp_avg(&cpu_dp, start_secs),
        connections: parse_dp_avg(&conn_dp, start_secs),
        read_iops: parse_dp_avg(&read_dp, start_secs),
        write_iops: parse_dp_avg(&write_dp, start_secs),
        free_storage: parse_dp_avg(&store_dp, start_secs),
        free_memory: parse_dp_avg(&mem_dp, start_secs),
        x_max: time_range.duration_secs() as f64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pick_log_export_prefers_error_then_first() {
        // "error" wins over other enabled types regardless of order.
        let exports = vec!["slowquery".to_string(), "error".to_string()];
        assert_eq!(pick_log_export(&exports).map(String::as_str), Some("error"));

        // No preferred type → falls back to the first listed.
        let exports = vec!["audit".to_string(), "general".to_string()];
        assert_eq!(
            pick_log_export(&exports).map(String::as_str),
            Some("general")
        );

        // Postgres-style: only "postgresql" enabled.
        let exports = vec!["postgresql".to_string()];
        assert_eq!(
            pick_log_export(&exports).map(String::as_str),
            Some("postgresql")
        );

        // Nothing enabled → None (caller shows a friendly message).
        assert_eq!(pick_log_export(&[]), None);
    }
}
