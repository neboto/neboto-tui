use std::any::Any;
use std::collections::HashMap;

use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::aws::client::AwsClients;
use crate::aws::resource::{native_state_label, Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::aws::services::ec2::MetricsTimeRange;
use crate::error::Result;
use crate::event::{Event, LoadProgress};

type FsxClient = aws_sdk_fsx::Client;

// ═══════════════════════════════════════════════════════════════════════════════
// Service
// ═══════════════════════════════════════════════════════════════════════════════

pub struct FsxService {
    client: FsxClient,
}

impl FsxService {
    pub fn new(clients: &AwsClients) -> Self {
        Self {
            client: clients.fsx_client(),
        }
    }
}

#[async_trait]
impl AwsService for FsxService {
    fn service_type(&self) -> ServiceType {
        ServiceType::Fsx
    }

    fn name(&self) -> &str {
        "FSx"
    }

    async fn list_resources(&self) -> Result<Vec<Box<dyn Resource>>> {
        Ok(vec![])
    }

    async fn list_resources_streaming(
        &self,
        event_tx: mpsc::UnboundedSender<Event>,
        service_type: ServiceType,
    ) -> Result<()> {
        let mut total = 0usize;
        let mut paginator = self.client.describe_file_systems().into_paginator().send();

        while let Some(result) = paginator.next().await {
            match result {
                Ok(page) => {
                    let batch: Vec<Box<dyn Resource>> = page
                        .file_systems()
                        .iter()
                        .map(|fs| Box::new(FsxFileSystem::from_sdk(fs)) as Box<dyn Resource>)
                        .collect();
                    total += batch.len();
                    if !batch.is_empty() {
                        let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                            service: service_type,
                            resources: batch,
                            progress: LoadProgress {
                                loaded_count: total,
                                total_count: None,
                                status_message: None,
                            },
                        });
                    }
                }
                Err(e) => {
                    let _ = event_tx.send(Event::ResourceLoadError {
                        service: service_type,
                        error: format!(
                            "Failed to list FSx file systems: {}",
                            crate::error::sdk_error_message(&e)
                        ),
                    });
                    break;
                }
            }
        }

        // Volumes (ONTAP/OpenZFS) as first-class resources for the Volumes
        // sub-tab. Describe-only here (fast); live usage + graphs are lazy/`m`.
        let mut vol_paginator = self.client.describe_volumes().into_paginator().send();
        while let Some(result) = vol_paginator.next().await {
            match result {
                Ok(page) => {
                    let batch: Vec<Box<dyn Resource>> = page
                        .volumes()
                        .iter()
                        .map(|v| Box::new(FsxVolume::from_sdk(v)) as Box<dyn Resource>)
                        .collect();
                    total += batch.len();
                    if !batch.is_empty() {
                        let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                            service: service_type,
                            resources: batch,
                            progress: LoadProgress {
                                loaded_count: total,
                                total_count: None,
                                status_message: None,
                            },
                        });
                    }
                }
                // Volumes are best-effort — don't fail the whole FSx load.
                Err(_) => break,
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

// ═══════════════════════════════════════════════════════════════════════════════
// Resource
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct FsxFileSystem {
    pub file_system_id: String,
    pub arn: String,
    pub fs_name: String,
    pub file_system_type: String,
    pub lifecycle: String,
    pub storage_capacity_gib: i32,
    pub storage_type: String,
    pub dns_name: String,
    pub vpc_id: String,
    pub subnet_ids: Vec<String>,
    pub network_interface_ids: Vec<String>,
    pub kms_key_id: String,
    pub creation_time: String,
    pub failure_details: String,
    pub config: FsxConfig,
    pub tags: HashMap<String, String>,
}

#[derive(Debug, Clone)]
pub enum FsxConfig {
    Windows(FsxWindowsConfig),
    Lustre(FsxLustreConfig),
    Ontap(FsxOntapConfig),
    OpenZfs(FsxOpenZfsConfig),
    Unknown,
}

#[derive(Debug, Clone)]
pub struct FsxWindowsConfig {
    pub deployment_type: String,
    pub active_directory_id: String,
    pub preferred_file_server_ip: String,
    pub throughput_capacity_mbps: i32,
    pub automatic_backup_retention_days: i32,
}

#[derive(Debug, Clone)]
pub struct FsxLustreConfig {
    pub deployment_type: String,
    pub per_unit_storage_throughput: i32,
    pub mount_name: String,
    pub data_repo_count: usize,
}

#[derive(Debug, Clone)]
pub struct FsxOntapConfig {
    pub deployment_type: String,
    pub endpoint_ip_address_range: String,
    pub throughput_capacity_mbps: i32,
}

#[derive(Debug, Clone)]
pub struct FsxOpenZfsConfig {
    pub deployment_type: String,
    pub throughput_capacity_mbps: i32,
    pub root_volume_id: String,
    pub copy_tags_to_backups: bool,
}

impl FsxFileSystem {
    fn from_sdk(fs: &aws_sdk_fsx::types::FileSystem) -> Self {
        let tags: HashMap<String, String> = fs
            .tags()
            .iter()
            .filter_map(|t| Some((t.key()?.to_string(), t.value().unwrap_or_default().to_string())))
            .collect();

        let fs_name = tags
            .get("Name")
            .cloned()
            .unwrap_or_else(|| fs.file_system_id().unwrap_or_default().to_string());

        let config = if let Some(w) = fs.windows_configuration() {
            FsxConfig::Windows(FsxWindowsConfig {
                deployment_type: w
                    .deployment_type()
                    .map(|d| d.as_str().to_string())
                    .unwrap_or_default(),
                active_directory_id: w
                    .active_directory_id()
                    .unwrap_or_default()
                    .to_string(),
                preferred_file_server_ip: w
                    .preferred_file_server_ip()
                    .unwrap_or_default()
                    .to_string(),
                throughput_capacity_mbps: w.throughput_capacity().unwrap_or(0),
                automatic_backup_retention_days: w
                    .automatic_backup_retention_days()
                    .unwrap_or(0),
            })
        } else if let Some(l) = fs.lustre_configuration() {
            FsxConfig::Lustre(FsxLustreConfig {
                deployment_type: l
                    .deployment_type()
                    .map(|d| d.as_str().to_string())
                    .unwrap_or_default(),
                per_unit_storage_throughput: l.per_unit_storage_throughput().unwrap_or(0),
                mount_name: l.mount_name().unwrap_or_default().to_string(),
                data_repo_count: l.data_repository_configuration().map(|_| 1).unwrap_or(0),
            })
        } else if let Some(o) = fs.ontap_configuration() {
            FsxConfig::Ontap(FsxOntapConfig {
                deployment_type: o
                    .deployment_type()
                    .map(|d| d.as_str().to_string())
                    .unwrap_or_default(),
                endpoint_ip_address_range: o
                    .endpoint_ip_address_range()
                    .unwrap_or_default()
                    .to_string(),
                throughput_capacity_mbps: o.throughput_capacity().unwrap_or(0),
            })
        } else if let Some(z) = fs.open_zfs_configuration() {
            FsxConfig::OpenZfs(FsxOpenZfsConfig {
                deployment_type: z
                    .deployment_type()
                    .map(|d| d.as_str().to_string())
                    .unwrap_or_default(),
                throughput_capacity_mbps: z.throughput_capacity().unwrap_or(0),
                root_volume_id: z.root_volume_id().unwrap_or_default().to_string(),
                copy_tags_to_backups: z.copy_tags_to_backups().unwrap_or(false),
            })
        } else {
            FsxConfig::Unknown
        };

        Self {
            file_system_id: fs.file_system_id().unwrap_or_default().to_string(),
            arn: fs.resource_arn().unwrap_or_default().to_string(),
            fs_name,
            file_system_type: fs
                .file_system_type()
                .map(|t| t.as_str().to_string())
                .unwrap_or_default(),
            lifecycle: fs
                .lifecycle()
                .map(|l| l.as_str().to_string())
                .unwrap_or_default(),
            storage_capacity_gib: fs.storage_capacity().unwrap_or(0),
            storage_type: fs
                .storage_type()
                .map(|s| s.as_str().to_string())
                .unwrap_or_default(),
            dns_name: fs.dns_name().unwrap_or_default().to_string(),
            vpc_id: fs.vpc_id().unwrap_or_default().to_string(),
            subnet_ids: fs.subnet_ids().iter().map(|s| s.to_string()).collect(),
            network_interface_ids: fs
                .network_interface_ids()
                .iter()
                .map(|s| s.to_string())
                .collect(),
            kms_key_id: fs.kms_key_id().unwrap_or_default().to_string(),
            creation_time: fs
                .creation_time()
                .map(|d| {
                    d.fmt(aws_smithy_types::date_time::Format::DateTime)
                        .unwrap_or_default()
                })
                .unwrap_or_default(),
            failure_details: fs
                .failure_details()
                .and_then(|f| f.message())
                .unwrap_or_default()
                .to_string(),
            config,
            tags,
        }
    }

    pub fn throughput_mbps(&self) -> i32 {
        match &self.config {
            FsxConfig::Windows(w) => w.throughput_capacity_mbps,
            FsxConfig::Ontap(o) => o.throughput_capacity_mbps,
            FsxConfig::OpenZfs(z) => z.throughput_capacity_mbps,
            _ => 0,
        }
    }
}

crate::sections! {
    pub enum FsxDetailSection,
    pub static FSX_FS_SECTIONS = [
        Overview "Overview",
        Volumes "Volumes" => crate::app::App::trigger_fsx_volumes_load,
        Backups "Backups" => crate::app::App::trigger_fsx_backups_load,
        Network "Network",
        Configuration "Configuration",
        Tags "Tags",
    ]
}

impl Resource for FsxFileSystem {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&FSX_FS_SECTIONS)
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws fsx describe-file-systems --file-system-ids {}",
            crate::aws::resource::shell_quote(self.id())
        ))
    }

    fn id(&self) -> &str {
        &self.file_system_id
    }
    fn name(&self) -> &str {
        &self.fs_name
    }
    fn resource_type(&self) -> &str {
        "FSx File System"
    }
    fn state(&self) -> ResourceState {
        match self.lifecycle.as_str() {
            "AVAILABLE" => ResourceState::Available,
            "CREATING" => ResourceState::Creating,
            "UPDATING" => ResourceState::Pending,
            "DELETING" => ResourceState::Deleting,
            "FAILED" | "MISCONFIGURED" | "MISCONFIGURED_UNAVAILABLE" => {
                ResourceState::Unavailable
            }
            other => ResourceState::Unknown(other.to_string()),
        }
    }
    fn state_label(&self) -> String {
        native_state_label(&self.lifecycle, || self.state())
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        format!(
            "{} {} {} {} {} {}",
            self.file_system_id, self.fs_name, self.file_system_type,
            self.dns_name, self.vpc_id, self.storage_type
        )
    }
    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("File System ID".to_string(), self.file_system_id.clone()),
            ("Name".to_string(), self.fs_name.clone()),
            ("Type".to_string(), self.file_system_type.clone()),
            ("Lifecycle".to_string(), self.lifecycle.clone()),
            ("Storage Capacity".to_string(), format!("{} GiB", self.storage_capacity_gib)),
            ("Storage Type".to_string(), self.storage_type.clone()),
            ("DNS Name".to_string(), self.dns_name.clone()),
            ("VPC".to_string(), self.vpc_id.clone()),
            ("Created".to_string(), self.creation_time.clone()),
        ]
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/fsx/home?region={}#file-system-details/{}",
            region, region, self.file_system_id
        ))
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Volumes (ONTAP / OpenZFS) — lazy section
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct FsxVolume {
    pub volume_id: String,
    pub name: String,
    pub file_system_id: String,
    pub volume_type: String, // ONTAP | OPENZFS
    pub lifecycle: String,
    /// Configured size in bytes (ONTAP size_in_bytes / OpenZFS quota GiB).
    pub size_bytes: i64,
    /// Junction path (ONTAP) or volume path (OpenZFS).
    pub path: String,
    /// ONTAP storage virtual machine id (empty for OpenZFS).
    pub svm_id: String,
    /// Extra type-specific notes (tiering, storage efficiency, compression …).
    pub notes: Vec<String>,
    /// Live storage usage from CloudWatch (latest datapoint), if available.
    pub used_bytes: Option<i64>,
    pub utilization_pct: Option<f64>,
    pub tags: HashMap<String, String>,
}

impl FsxVolume {
    fn from_sdk(v: &aws_sdk_fsx::types::Volume) -> Self {
        let tags: HashMap<String, String> = v
            .tags()
            .iter()
            .filter_map(|t| Some((t.key()?.to_string(), t.value().unwrap_or_default().to_string())))
            .collect();
        let volume_type = v
            .volume_type()
            .map(|t| t.as_str().to_string())
            .unwrap_or_default();
        let lifecycle = v
            .lifecycle()
            .map(|l| l.as_str().to_string())
            .unwrap_or_default();

        let mut size_bytes = 0i64;
        let mut path = String::new();
        let mut svm_id = String::new();
        let mut notes: Vec<String> = Vec::new();

        if let Some(o) = v.ontap_configuration() {
            size_bytes = o
                .size_in_bytes()
                .or_else(|| o.size_in_megabytes().map(|m| m as i64 * 1024 * 1024))
                .unwrap_or(0);
            path = o.junction_path().unwrap_or_default().to_string();
            svm_id = o.storage_virtual_machine_id().unwrap_or_default().to_string();
            if let Some(t) = o.ontap_volume_type() {
                notes.push(format!("volume type {}", t.as_str()));
            }
            if o.storage_efficiency_enabled() == Some(true) {
                notes.push("storage efficiency on".to_string());
            }
            if let Some(t) = o.tiering_policy().and_then(|t| t.name()) {
                notes.push(format!("tiering {}", t.as_str()));
            }
            if let Some(p) = o.snapshot_policy() {
                if !p.is_empty() {
                    notes.push(format!("snapshot policy {}", p));
                }
            }
            if let Some(s) = o.security_style() {
                notes.push(format!("security {}", s.as_str()));
            }
        } else if let Some(z) = v.open_zfs_configuration() {
            size_bytes = z
                .storage_capacity_quota_gib()
                .map(|g| g as i64 * 1024 * 1024 * 1024)
                .unwrap_or(0);
            path = z.volume_path().unwrap_or_default().to_string();
            if let Some(r) = z.storage_capacity_reservation_gib() {
                if r > 0 {
                    notes.push(format!("reservation {} GiB", r));
                }
            }
            if let Some(c) = z.data_compression_type() {
                notes.push(format!("compression {}", c.as_str()));
            }
            if z.read_only() == Some(true) {
                notes.push("read-only".to_string());
            }
        }

        Self {
            volume_id: v.volume_id().unwrap_or_default().to_string(),
            name: v.name().unwrap_or_default().to_string(),
            file_system_id: v.file_system_id().unwrap_or_default().to_string(),
            volume_type,
            lifecycle,
            size_bytes,
            path,
            svm_id,
            notes,
            used_bytes: None,
            utilization_pct: None,
            tags,
        }
    }
}

crate::sections! {
    pub enum FsxVolumeDetailSection,
    pub static FSX_VOLUME_SECTIONS = [
        // The live snapshot loads on drill-in (Overview is the default
        // section) and again on Live, which renders it — both idempotent.
        Overview "Overview" => crate::app::App::trigger_fsx_volume_detail_load,
        Live "Live" => crate::app::App::trigger_fsx_volume_detail_load,
        Tags "Tags",
    ]
}

impl Resource for FsxVolume {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&FSX_VOLUME_SECTIONS)
    }

    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws fsx describe-volumes --volume-ids {}",
            crate::aws::resource::shell_quote(self.id())
        ))
    }

    fn id(&self) -> &str {
        &self.volume_id
    }
    fn name(&self) -> &str {
        if self.name.is_empty() {
            &self.volume_id
        } else {
            &self.name
        }
    }
    fn resource_type(&self) -> &str {
        "FSx Volume"
    }
    fn state(&self) -> ResourceState {
        match self.lifecycle.as_str() {
            "CREATED" | "AVAILABLE" => ResourceState::Available,
            "CREATING" | "PENDING" | "RESTORING" => ResourceState::Creating,
            "DELETING" => ResourceState::Deleting,
            "FAILED" | "MISCONFIGURED" => ResourceState::Unavailable,
            other => ResourceState::Unknown(other.to_string()),
        }
    }
    fn state_label(&self) -> String {
        native_state_label(&self.lifecycle, || self.state())
    }
    fn search_text(&self) -> String {
        format!(
            "{} {} {} {} {} {}",
            self.name, self.volume_id, self.file_system_id, self.volume_type, self.path, self.svm_id
        )
    }
    fn details(&self) -> Vec<(String, String)> {
        // Rendered via the split pane (fsx_volume_section_lines); this is the
        // flat fallback.
        let mut rows = vec![
            ("Volume ID".to_string(), self.volume_id.clone()),
            ("File System".to_string(), self.file_system_id.clone()),
            ("Type".to_string(), self.volume_type.clone()),
            ("Lifecycle".to_string(), self.lifecycle.clone()),
        ];
        if self.size_bytes > 0 {
            rows.push((
                "Size".to_string(),
                crate::aws::services::efs::fmt_bytes(self.size_bytes),
            ));
        }
        if !self.path.is_empty() {
            rows.push(("Path".to_string(), self.path.clone()));
        }
        if !self.svm_id.is_empty() {
            rows.push(("SVM".to_string(), self.svm_id.clone()));
        }
        if let Some(used) = self.used_bytes {
            rows.push((
                "Used".to_string(),
                crate::aws::services::efs::fmt_bytes(used),
            ));
        }
        if let Some(pct) = self.utilization_pct {
            rows.push(("Utilization".to_string(), format!("{:.1}%", pct)));
        }
        for note in &self.notes {
            rows.push(("  ".to_string(), note.clone()));
        }
        rows.push((String::new(), String::new()));
        rows.push((
            "  press m for usage / latency / tier graphs".to_string(),
            String::new(),
        ));
        rows
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/fsx/home?region={}#volume-details/{}",
            region, region, self.volume_id
        ))
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Live snapshot for a volume's detail pane (latest CloudWatch datapoints).
#[derive(Debug, Clone)]
pub struct FsxVolumeSnapshot {
    pub used_bytes: Option<i64>,
    pub utilization_pct: Option<f64>,
    pub read_latency_ms: Option<f64>,
    pub write_latency_ms: Option<f64>,
    pub metadata_latency_ms: Option<f64>,
}

/// One-shot live snapshot of a volume: latest used/utilization and latest
/// read/write/metadata latency (ms = 1000 × `*OperationTime` seconds ÷
/// `*Operations`).
pub async fn fetch_fsx_volume_detail(
    cw: aws_sdk_cloudwatch::Client,
    volume_id: String,
    capacity_bytes: i64,
) -> FsxVolumeSnapshot {
    use aws_sdk_cloudwatch::types::Statistic;
    let published = fewest_by_name(&discover_fsx_metrics(&cw, "VolumeId", &volume_id).await);
    let pick = |candidates: &[&str]| -> Option<(String, Vec<aws_sdk_cloudwatch::types::Dimension>)> {
        candidates
            .iter()
            .find_map(|n| published.get(*n).map(|d| (n.to_string(), d.clone())))
    };

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let start_dt = aws_sdk_cloudwatch::primitives::DateTime::from_secs(now - 3 * 3600);
    let end_dt = aws_sdk_cloudwatch::primitives::DateTime::from_secs(now);

    let (used, util, rt, ro, wt, wo, mt, mo) = tokio::join!(
        latest_value(cw.clone(), pick(&["StorageUsed", "UsedStorageCapacity"]), Statistic::Average, start_dt, end_dt),
        latest_value(cw.clone(), pick(&["StorageCapacityUtilization"]), Statistic::Average, start_dt, end_dt),
        latest_value(cw.clone(), pick(&["DataReadOperationTime"]), Statistic::Sum, start_dt, end_dt),
        latest_value(cw.clone(), pick(&["DataReadOperations"]), Statistic::Sum, start_dt, end_dt),
        latest_value(cw.clone(), pick(&["DataWriteOperationTime"]), Statistic::Sum, start_dt, end_dt),
        latest_value(cw.clone(), pick(&["DataWriteOperations"]), Statistic::Sum, start_dt, end_dt),
        latest_value(cw.clone(), pick(&["MetadataOperationTime"]), Statistic::Sum, start_dt, end_dt),
        latest_value(cw.clone(), pick(&["MetadataOperations"]), Statistic::Sum, start_dt, end_dt),
    );

    // `*OperationTime` is in seconds (the ONTAP guide), not microseconds.
    let lat = |time_s: Option<f64>, ops: Option<f64>| match (time_s, ops) {
        (Some(t), Some(o)) if o > 0.0 => Some(t / o * 1000.0),
        _ => None,
    };
    let used_bytes = used.map(|v| v as i64);
    let utilization_pct = util.or_else(|| match (used, capacity_bytes) {
        (Some(u), c) if c > 0 => Some((u / c as f64) * 100.0),
        _ => None,
    });
    FsxVolumeSnapshot {
        used_bytes,
        utilization_pct,
        read_latency_ms: lat(rt, ro),
        write_latency_ms: lat(wt, wo),
        metadata_latency_ms: lat(mt, mo),
    }
}

/// `DescribeVolumes` filtered to one file system (ONTAP/OpenZFS only; other
/// types return an empty list), then enriches each volume with its live storage
/// usage (latest CloudWatch datapoint) under bounded concurrency.
pub async fn fetch_fsx_volumes(
    client: FsxClient,
    cw: aws_sdk_cloudwatch::Client,
    file_system_id: String,
) -> Result<Vec<FsxVolume>> {
    use aws_sdk_fsx::types::{VolumeFilter, VolumeFilterName};
    use futures::stream::StreamExt;

    let filter = VolumeFilter::builder()
        .name(VolumeFilterName::FileSystemId)
        .values(file_system_id)
        .build();

    let mut volumes = Vec::new();
    let mut paginator = client
        .describe_volumes()
        .filters(filter)
        .into_paginator()
        .send();
    while let Some(page) = paginator.next().await {
        let page = page.map_err(|e| {
            crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e))
        })?;
        for v in page.volumes() {
            volumes.push(FsxVolume::from_sdk(v));
        }
    }
    volumes.sort_by(|a, b| a.name.cmp(&b.name));

    // Live usage per volume (best-effort; missing metrics just leave None).
    // Collect owned (id, capacity) first so the futures don't borrow `volumes`.
    let keys: Vec<(String, i64)> =
        volumes.iter().map(|v| (v.volume_id.clone(), v.size_bytes)).collect();
    let usages = futures::stream::iter(keys.into_iter().map(|(id, cap)| {
        let cw = cw.clone();
        async move { fetch_fsx_volume_usage(cw, id, cap).await }
    }))
    .buffer_unordered(8)
    .collect::<Vec<_>>()
    .await;
    for (vol, (used, util)) in volumes.iter_mut().zip(usages) {
        vol.used_bytes = used;
        vol.utilization_pct = util;
    }

    Ok(volumes)
}

/// Latest (used_bytes, utilization_pct) for one volume from `AWS/FSx`
/// (`VolumeId` dimension), discovering the real metric names/dims. Utilization
/// falls back to used/capacity when the percentage metric isn't published.
async fn fetch_fsx_volume_usage(
    cw: aws_sdk_cloudwatch::Client,
    volume_id: String,
    capacity_bytes: i64,
) -> (Option<i64>, Option<f64>) {
    use aws_sdk_cloudwatch::types::Statistic;

    let published = fewest_by_name(&discover_fsx_metrics(&cw, "VolumeId", &volume_id).await);
    let pick = |candidates: &[&str]| -> Option<(String, Vec<aws_sdk_cloudwatch::types::Dimension>)> {
        candidates
            .iter()
            .find_map(|n| published.get(*n).map(|d| (n.to_string(), d.clone())))
    };
    let used_m = pick(&["StorageUsed", "UsedStorageCapacity"]);
    let util_m = pick(&["StorageCapacityUtilization"]);

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    // Last ~3h, 5-min period — take the most recent datapoint.
    let start_dt = aws_sdk_cloudwatch::primitives::DateTime::from_secs(now - 3 * 3600);
    let end_dt = aws_sdk_cloudwatch::primitives::DateTime::from_secs(now);

    let (used, util) = tokio::join!(
        latest_value(cw.clone(), used_m, Statistic::Average, start_dt, end_dt),
        latest_value(cw.clone(), util_m, Statistic::Average, start_dt, end_dt),
    );

    let used_bytes = used.map(|v| v as i64);
    let utilization_pct = util.or_else(|| match (used, capacity_bytes) {
        (Some(u), c) if c > 0 => Some((u / c as f64) * 100.0),
        _ => None,
    });
    (used_bytes, utilization_pct)
}

/// Most-recent datapoint value for a discovered metric, or `None`.
async fn latest_value(
    cw: aws_sdk_cloudwatch::Client,
    metric: Option<(String, Vec<aws_sdk_cloudwatch::types::Dimension>)>,
    stat: aws_sdk_cloudwatch::types::Statistic,
    start_dt: aws_sdk_cloudwatch::primitives::DateTime,
    end_dt: aws_sdk_cloudwatch::primitives::DateTime,
) -> Option<f64> {
    let (name, dims) = metric?;
    let resp = cw
        .get_metric_statistics()
        .namespace("AWS/FSx")
        .metric_name(&name)
        .set_dimensions(Some(dims))
        .start_time(start_dt)
        .end_time(end_dt)
        .period(300)
        .set_statistics(Some(vec![stat]))
        .send()
        .await
        .ok()?;
    // Pick the datapoint with the latest timestamp.
    resp.datapoints()
        .iter()
        .filter_map(|dp| Some((dp.timestamp()?.secs(), dp.average().or_else(|| dp.sum())?)))
        .max_by_key(|(ts, _)| *ts)
        .map(|(_, v)| v)
}

// ═══════════════════════════════════════════════════════════════════════════════
// Backups — lazy section
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct FsxBackup {
    pub backup_id: String,
    pub backup_type: String, // AUTOMATIC | USER_INITIATED | AWS_BACKUP
    pub lifecycle: String,
    pub progress_percent: i32,
    pub creation_time: String,
    pub volume_id: String, // for ONTAP/OpenZFS volume backups
}

impl FsxBackup {
    fn from_sdk(b: &aws_sdk_fsx::types::Backup) -> Self {
        Self {
            backup_id: b.backup_id().unwrap_or_default().to_string(),
            backup_type: b.r#type().map(|t| t.as_str().to_string()).unwrap_or_default(),
            lifecycle: b.lifecycle().map(|l| l.as_str().to_string()).unwrap_or_default(),
            progress_percent: b.progress_percent().unwrap_or(0),
            creation_time: b
                .creation_time()
                .map(|t| t.to_string())
                .unwrap_or_default(),
            volume_id: b
                .volume()
                .and_then(|v| v.volume_id())
                .unwrap_or_default()
                .to_string(),
        }
    }
}

/// `DescribeBackups` filtered server-side to one file system (`file-system-id`),
/// so it doesn't scan every backup in the account. Page count is capped
/// defensively. Returns newest-first.
pub async fn fetch_fsx_backups(client: FsxClient, file_system_id: String) -> Result<Vec<FsxBackup>> {
    use aws_sdk_fsx::types::{Filter, FilterName};
    let filter = Filter::builder()
        .name(FilterName::FileSystemId)
        .values(file_system_id)
        .build();

    let mut backups = Vec::new();
    let mut paginator = client
        .describe_backups()
        .filters(filter)
        .into_paginator()
        .send();
    let mut pages = 0;
    while let Some(page) = paginator.next().await {
        let page = page.map_err(|e| {
            crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e))
        })?;
        for b in page.backups() {
            backups.push(FsxBackup::from_sdk(b));
        }
        pages += 1;
        if pages >= 20 {
            break;
        }
    }
    // Newest first.
    backups.sort_by(|a, b| b.creation_time.cmp(&a.creation_time));
    Ok(backups)
}

// ═══════════════════════════════════════════════════════════════════════════════
// CloudWatch metrics (`m`, the spec view — `aws::metrics::FSX_*`)
// ═══════════════════════════════════════════════════════════════════════════════

/// What `m` charts for a file system. ONTAP and OpenZFS add a breakdown:
/// read/write per volume (`⏎` charts one) and, for ONTAP, storage by tier.
pub fn file_system_metric_target(fs: &FsxFileSystem, volumes: &[(String, String)]) -> crate::aws::metrics::MetricTarget {
    use crate::aws::metrics as m;
    let (kind, specs, volume_specs): (&str, &'static [m::MetricSpec], &'static [m::MetricSpec]) =
        match fs.file_system_type.as_str() {
            "LUSTRE" => ("FSx Lustre", m::FSX_LUSTRE, &[]),
            "ONTAP" => ("FSx ONTAP", m::FSX_ONTAP, m::FSX_ONTAP_VOLUME),
            "OPENZFS" => ("FSx OpenZFS", m::FSX_OPENZFS, m::FSX_OPENZFS_VOLUME),
            _ => ("FSx Windows", m::FSX_WINDOWS, &[]),
        };
    let dimensions = vec![("FileSystemId".to_string(), fs.file_system_id.clone())];
    let breakdown = (!volume_specs.is_empty()).then(|| {
        m::Breakdown::Fsx(FsxBreakdown {
            file_system_id: fs.file_system_id.clone(),
            volume_id: None,
            names: volumes.to_vec(),
            volume_specs,
            tiers: fs.file_system_type == "ONTAP",
        })
    });
    m::MetricTarget {
        title: format!("{kind} · {}", fs.name()),
        namespace: "AWS/FSx".to_string(),
        dimensions: dimensions.clone(),
        specs,
        home: None,
        hint: None,
        extra: Vec::new(),
        breakdown,
        resolve: Some(m::Resolve::Fsx { dimensions, capacity_bytes: fs.storage_capacity_gib as i64 * (1 << 30) }),
    }
}

/// What `m` charts for one volume: its own storage and traffic, and an
/// ONTAP volume's storage by tier and data type.
pub fn volume_metric_target(vol: &FsxVolume) -> crate::aws::metrics::MetricTarget {
    use crate::aws::metrics as m;
    let ontap = vol.volume_type == "ONTAP";
    let specs = if ontap { m::FSX_ONTAP_VOLUME } else { m::FSX_OPENZFS_VOLUME };
    let dimensions = vec![
        ("FileSystemId".to_string(), vol.file_system_id.clone()),
        ("VolumeId".to_string(), vol.volume_id.clone()),
    ];
    let name = if vol.name.is_empty() { &vol.volume_id } else { &vol.name };
    m::MetricTarget {
        title: format!("FSx volume · {name}"),
        namespace: "AWS/FSx".to_string(),
        dimensions: dimensions.clone(),
        specs,
        home: None,
        hint: None,
        extra: Vec::new(),
        breakdown: ontap.then(|| {
            m::Breakdown::Fsx(FsxBreakdown {
                file_system_id: vol.file_system_id.clone(),
                volume_id: Some(vol.volume_id.clone()),
                names: Vec::new(),
                volume_specs: &[],
                tiers: true,
            })
        }),
        resolve: Some(m::Resolve::Fsx { dimensions, capacity_bytes: vol.size_bytes }),
    }
}

/// How one of a set's metrics is published for this resource.
#[derive(Debug, PartialEq)]
enum Published {
    /// Under exactly the target's dimensions.
    Exact,
    /// Under one other set: re-key the row to it (ONTAP's SSD utilisation
    /// carries `StorageTier` + `DataType`).
    At(Vec<(String, String)>),
    /// Once per storage target or server (Lustre): one series over all of
    /// them, the schema's dimension names.
    Each(Vec<String>),
}

/// The variant of `metric` a row should chart, given every variant
/// `ListMetrics` returned for the target. A file system's own rows never
/// take a volume's variant.
fn published(
    metric: &str,
    variants: &[(String, Vec<(String, String)>)],
    target: &[(String, String)],
) -> Option<Published> {
    let is_volume = target.iter().any(|(n, _)| n == "VolumeId");
    let mut sorted_target = target.to_vec();
    sorted_target.sort();
    let candidates: Vec<Vec<(String, String)>> = variants
        .iter()
        .filter(|(n, d)| n == metric && (is_volume || !d.iter().any(|(k, _)| k == "VolumeId")))
        .map(|(_, d)| {
            let mut d = d.clone();
            d.sort();
            d
        })
        .collect();
    if candidates.is_empty() {
        return None;
    }
    if candidates.contains(&sorted_target) {
        return Some(Published::Exact);
    }
    let extra = |d: &[(String, String)]| d.iter().filter(|kv| !sorted_target.contains(kv)).cloned().collect::<Vec<_>>();
    // Storage tier / data type: pick the whole-of-tier cell (`DataType=All`,
    // the SSD tier), not a sum that would count it twice.
    let tiered = candidates.iter().all(|d| extra(d).iter().all(|(k, _)| k == "StorageTier" || k == "DataType"));
    if tiered || candidates.len() == 1 {
        let score = |d: &Vec<(String, String)>| {
            extra(d)
                .iter()
                .map(|(k, v)| match (k.as_str(), v.as_str()) {
                    ("DataType", "All") | ("StorageTier", "SSD") | ("StorageTier", "All") => 1,
                    _ => 0,
                })
                .sum::<i32>()
        };
        return candidates.iter().max_by_key(|d| score(d)).cloned().map(Published::At);
    }
    // One series per target or server: the schema most of them share.
    let mut schemas: HashMap<Vec<String>, usize> = HashMap::new();
    for d in &candidates {
        *schemas.entry(d.iter().map(|(k, _)| k.clone()).collect()).or_default() += 1;
    }
    schemas.into_iter().max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(&a.0))).map(|(s, _)| Published::Each(s))
}

/// The `{Name}` placeholders of a math row.
fn expression_metrics(expr: &str) -> Vec<&str> {
    expr.split('{').skip(1).filter_map(|s| s.split_once('}').map(|(name, _)| name)).collect()
}

/// `Resolve::Fsx`: keep the rows this file system publishes, each under the
/// dimensions it's published with, and fold a per-target metric into one
/// series — the busiest server for a utilisation, the sum otherwise. A math
/// row stays when everything it reads does. When `ListMetrics` fails or
/// finds nothing, the set is left as it is: the fetch then says what's
/// wrong row by row, instead of the view going quiet.
pub async fn resolve_metrics(
    cw: &aws_sdk_cloudwatch::Client,
    target: &[(String, String)],
    capacity_bytes: i64,
    queries: &[crate::aws::metrics::MetricQuery],
) -> crate::aws::metrics::TargetFix {
    use crate::aws::metrics::TargetFix;
    match discover_target_metrics(cw, target).await {
        Ok(variants) if !variants.is_empty() => {
            TargetFix { curated: Some(curate(&variants, target, capacity_bytes, queries)), ..Default::default() }
        }
        _ => TargetFix::default(),
    }
}

/// [`resolve_metrics`] once `ListMetrics` has answered: the set to chart.
fn curate(
    variants: &[(String, Vec<(String, String)>)],
    target: &[(String, String)],
    capacity_bytes: i64,
    queries: &[crate::aws::metrics::MetricQuery],
) -> Vec<crate::aws::metrics::MetricQuery> {
    use crate::aws::metrics::{MetricQuery, Unit};
    let filters: String =
        target.iter().map(|(k, v)| format!("{k}=\"{v}\"")).collect::<Vec<_>>().join(" ");
    let mut kept: Vec<MetricQuery> = Vec::new();
    for q in queries.iter().filter(|q| q.expression.is_none()) {
        let mut q = q.clone();
        match published(&q.metric, variants, target) {
            None => continue,
            Some(Published::Exact) => {}
            Some(Published::At(dims)) => q.dimensions = dims,
            Some(Published::Each(schema)) => {
                let (agg, note) = if q.unit == Unit::Percent { ("MAX", " (max)") } else { ("SUM", "") };
                q.expression = Some(format!(
                    "{agg}(SEARCH('{{AWS/FSx,{}}} MetricName=\"{}\" {filters}', '{}', $PERIOD))",
                    schema.join(","),
                    q.metric,
                    q.stat.api()
                ));
                q.label.push_str(note);
            }
        }
        kept.push(q);
    }
    let has = |kept: &[MetricQuery], name: &str| kept.iter().any(|k| k.metric == name);
    // Back in the set's order, with the math rows whose inputs survived.
    let mut set: Vec<MetricQuery> = Vec::new();
    for q in queries {
        match (&q.expression, q.metric.is_empty()) {
            (Some(expr), true) => {
                if expression_metrics(expr).iter().all(|n| has(&kept, n)) {
                    set.push(q.clone());
                }
            }
            _ => {
                if let Some(k) = kept.iter().find(|k| k.metric == q.metric) {
                    set.push(k.clone());
                }
            }
        }
    }
    // Windows publishes free bytes always, the percentage only above
    // 32 MBps: work it out from the configured size.
    if !set.iter().any(|q| q.label == "Storage used" || q.metric == "StorageCapacityUtilization")
        && has(&set, "FreeStorageCapacity")
        && capacity_bytes > 0
    {
        let free = set.iter().find(|q| q.metric == "FreeStorageCapacity").cloned().unwrap();
        set.insert(
            0,
            MetricQuery {
                metric: String::new(),
                label: "Storage used".to_string(),
                unit: Unit::Percent,
                rate: false,
                group: None,
                expression: Some(format!("100*(1-{{FreeStorageCapacity}}/{capacity_bytes})")),
                ..free
            },
        );
    }
    set
}

/// `ListMetrics(AWS/FSx)` for every variant carrying all of the target's
/// dimensions (Lustre: one per storage target, so several pages).
async fn discover_target_metrics(
    cw: &aws_sdk_cloudwatch::Client,
    target: &[(String, String)],
) -> std::result::Result<Vec<(String, Vec<(String, String)>)>, String> {
    use aws_sdk_cloudwatch::types::DimensionFilter;
    let filters: Vec<DimensionFilter> =
        target.iter().map(|(n, v)| DimensionFilter::builder().name(n).value(v).build()).collect();
    let mut out = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..10 {
        let resp = cw
            .list_metrics()
            .namespace("AWS/FSx")
            .set_dimensions(Some(filters.clone()))
            .set_next_token(token.clone())
            .send()
            .await
            .map_err(|e| crate::error::sdk_error_message(&e))?;
        for metric in resp.metrics() {
            let dims = metric
                .dimensions()
                .iter()
                .map(|d| (d.name().unwrap_or_default().to_string(), d.value().unwrap_or_default().to_string()))
                .collect();
            out.push((metric.metric_name().unwrap_or_default().to_string(), dims));
        }
        token = crate::aws::pagination::next_page_token(resp.next_token(), &token);
        if token.is_none() {
            break;
        }
    }
    Ok(out)
}

/// An ONTAP / OpenZFS file system's tables beside the chart: read and
/// write per volume over the range, and (ONTAP) used storage by tier and by
/// data type, for the file system or one volume.
#[derive(Debug, Clone)]
pub struct FsxBreakdown {
    pub file_system_id: String,
    /// The volume a volume's own view breaks down; `None` for a file system.
    pub volume_id: Option<String>,
    /// Volume id → name, for the rows (ids the SEARCH finds stay ids).
    pub names: Vec<(String, String)>,
    /// What `⏎` charts for a volume row; empty, no Volumes table.
    pub volume_specs: &'static [crate::aws::metrics::MetricSpec],
    /// ONTAP: StorageUsed by `StorageTier` / `DataType`.
    pub tiers: bool,
}

impl FsxBreakdown {
    pub async fn fetch(
        &self,
        cw: &aws_sdk_cloudwatch::Client,
        range: MetricsTimeRange,
        end: i64,
    ) -> std::result::Result<crate::aws::metrics::BreakdownData, String> {
        use aws_sdk_cloudwatch::types::MetricDataQuery;
        let period = range.period_secs();
        let fs = &self.file_system_id;
        let mut queries = Vec::new();
        if self.volume_id.is_none() && !self.volume_specs.is_empty() {
            for (id, metric) in [("r", "DataReadBytes"), ("w", "DataWriteBytes")] {
                queries.push(
                    MetricDataQuery::builder()
                        .id(id)
                        .expression(format!(
                            "SEARCH('{{AWS/FSx,FileSystemId,VolumeId}} MetricName=\"{metric}\" FileSystemId=\"{fs}\"', 'Sum', {period})"
                        ))
                        .label("${PROP('Dim.VolumeId')}")
                        .build(),
                );
            }
        }
        if self.tiers {
            let (schema, filter) = match &self.volume_id {
                Some(v) => (",VolumeId", format!(" VolumeId=\"{v}\"")),
                None => ("", String::new()),
            };
            queries.push(
                MetricDataQuery::builder()
                    .id("t")
                    .expression(format!(
                        "SEARCH('{{AWS/FSx,DataType,FileSystemId,StorageTier{schema}}} MetricName=\"StorageUsed\" FileSystemId=\"{fs}\"{filter}', 'Average', {period})"
                    ))
                    .label("${PROP('Dim.StorageTier')}|${PROP('Dim.DataType')}")
                    .build(),
            );
        }
        if queries.is_empty() {
            return Ok(Default::default());
        }

        // Range totals for the volumes; the latest value for storage.
        let mut read: HashMap<String, f64> = HashMap::new();
        let mut write: HashMap<String, f64> = HashMap::new();
        let mut stored: HashMap<String, (i64, f64)> = HashMap::new();
        let mut token: Option<String> = None;
        loop {
            let resp = cw
                .get_metric_data()
                .set_metric_data_queries(Some(queries.clone()))
                .start_time(aws_sdk_cloudwatch::primitives::DateTime::from_secs(end - range.duration_secs()))
                .end_time(aws_sdk_cloudwatch::primitives::DateTime::from_secs(end))
                .set_next_token(token.clone())
                .send()
                .await
                .map_err(|e| crate::error::sdk_error_message(&e))?;
            for r in resp.metric_data_results() {
                let label = r.label().unwrap_or_default().to_string();
                match r.id().unwrap_or_default() {
                    "r" => *read.entry(label).or_default() += r.values().iter().sum::<f64>(),
                    "w" => *write.entry(label).or_default() += r.values().iter().sum::<f64>(),
                    _ => {
                        let latest = r.timestamps().iter().zip(r.values()).max_by_key(|(t, _)| t.secs());
                        if let Some((t, v)) = latest {
                            let slot = stored.entry(label).or_insert((t.secs(), *v));
                            if t.secs() >= slot.0 {
                                *slot = (t.secs(), *v);
                            }
                        }
                    }
                }
            }
            token = crate::aws::pagination::next_page_token(resp.next_token(), &token);
            if token.is_none() {
                break;
            }
        }

        let mut tables = Vec::new();
        if self.volume_id.is_none() && !self.volume_specs.is_empty() {
            tables.push(self.volumes_table(&read, &write));
        }
        let cells: Vec<(String, String, f64)> = stored
            .into_iter()
            .filter_map(|(label, (_, v))| {
                let (tier, data) = label.split_once('|')?;
                Some((tier.to_string(), data.to_string(), v))
            })
            .collect();
        let (by_tier, by_type) = fold_storage(&cells);
        for (title, rows) in [("Used by tier · now", by_tier), ("Used by data type · now", by_type)] {
            if !rows.is_empty() {
                tables.push(crate::aws::metrics::BreakdownTable {
                    title: title.to_string(),
                    columns: &["Used"],
                    rows: rows.into_iter().map(|(n, v)| (n, vec![v])).collect(),
                    unit: crate::aws::metrics::Unit::Bytes,
                    drill: None,
                });
            }
        }
        Ok(crate::aws::metrics::BreakdownData { summary: None, tables })
    }

    /// Every volume of the file system, busiest first: the ones the SEARCH
    /// found traffic for, then the known ones it didn't, at zero.
    fn volumes_table(&self, read: &HashMap<String, f64>, write: &HashMap<String, f64>) -> crate::aws::metrics::BreakdownTable {
        use crate::aws::metrics::{BreakdownTable, Drill, Unit};
        let mut ids: Vec<String> = read.keys().chain(write.keys()).cloned().collect();
        ids.extend(self.names.iter().map(|(id, _)| id.clone()));
        ids.sort();
        ids.dedup();
        let total = |id: &String| read.get(id).copied().unwrap_or(0.0) + write.get(id).copied().unwrap_or(0.0);
        ids.sort_by(|a, b| total(b).partial_cmp(&total(a)).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.cmp(b)));
        let name = |id: &String| {
            self.names.iter().find(|(i, n)| i == id && !n.is_empty()).map_or_else(|| id.clone(), |(_, n)| n.clone())
        };
        BreakdownTable {
            title: format!("Volumes ({})", ids.len()),
            columns: &["Read", "Write"],
            rows: ids
                .iter()
                .map(|id| (name(id), vec![read.get(id).copied().unwrap_or(0.0), write.get(id).copied().unwrap_or(0.0)]))
                .collect(),
            unit: Unit::Bytes,
            drill: Some(Drill {
                namespace: "AWS/FSx",
                specs: self.volume_specs,
                dimensions: ids
                    .iter()
                    .map(|id| {
                        vec![("FileSystemId".to_string(), self.file_system_id.clone()), ("VolumeId".to_string(), id.clone())]
                    })
                    .collect(),
            }),
        }
    }
}

/// `StorageUsed` cells `(tier, data type, bytes)` as two tables, largest
/// first. By tier reads the `DataType=All` cells; by data type reads the
/// `StorageTier=All` cells when there are any, else sums each type across
/// tiers — either way nothing is counted twice.
fn fold_storage(cells: &[(String, String, f64)]) -> (Vec<(String, f64)>, Vec<(String, f64)>) {
    let all = |s: &str| s.eq_ignore_ascii_case("All");
    let mut by_tier: HashMap<String, f64> = HashMap::new();
    let tier_rollups = cells.iter().any(|(t, d, _)| all(d) && !all(t));
    for (t, _, v) in cells.iter().filter(|(t, d, _)| !all(t) && (all(d) || !tier_rollups)) {
        *by_tier.entry(pretty_tier(t)).or_default() += v;
    }
    let mut by_type: HashMap<String, f64> = HashMap::new();
    let type_rollups = cells.iter().any(|(t, d, _)| all(t) && !all(d));
    for (_, d, v) in cells.iter().filter(|(t, d, _)| !all(d) && (all(t) || !type_rollups)) {
        *by_type.entry(d.clone()).or_default() += v;
    }
    let sorted = |m: HashMap<String, f64>| {
        let mut v: Vec<(String, f64)> = m.into_iter().collect();
        v.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.0.cmp(&b.0)));
        v
    };
    (sorted(by_tier), sorted(by_type))
}

fn pretty_tier(t: &str) -> String {
    match t {
        "StandardCapacityPool" => "Capacity Pool".to_string(),
        other => other.to_string(),
    }
}

/// `ListMetrics(AWS/FSx)` filtered to one dimension (`FileSystemId` or
/// `VolumeId`), returning every (metric name, dimension set) variant published.
async fn discover_fsx_metrics(
    cw: &aws_sdk_cloudwatch::Client,
    dim_name: &str,
    dim_value: &str,
) -> Vec<(String, Vec<aws_sdk_cloudwatch::types::Dimension>)> {
    use aws_sdk_cloudwatch::types::DimensionFilter;
    let mut out: Vec<(String, Vec<aws_sdk_cloudwatch::types::Dimension>)> = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..5 {
        let mut req = cw.list_metrics().namespace("AWS/FSx").dimensions(
            DimensionFilter::builder()
                .name(dim_name)
                .value(dim_value)
                .build(),
        );
        if let Some(t) = &token {
            req = req.next_token(t);
        }
        let resp = match req.send().await {
            Ok(r) => r,
            Err(_) => break,
        };
        for m in resp.metrics() {
            if let Some(name) = m.metric_name() {
                out.push((name.to_string(), m.dimensions().to_vec()));
            }
        }
        token = crate::aws::pagination::next_page_token(resp.next_token(), &token);
        if token.is_none() {
            break;
        }
    }
    out
}

/// Collapse discovery variants to each metric name's most-aggregate
/// (fewest-dimension) variant — the file-system-wide / volume-wide value.
fn fewest_by_name(
    variants: &[(String, Vec<aws_sdk_cloudwatch::types::Dimension>)],
) -> HashMap<String, Vec<aws_sdk_cloudwatch::types::Dimension>> {
    let mut published: HashMap<String, Vec<aws_sdk_cloudwatch::types::Dimension>> = HashMap::new();
    for (name, dims) in variants {
        match published.get(name) {
            Some(existing) if existing.len() <= dims.len() => {}
            _ => {
                published.insert(name.clone(), dims.clone());
            }
        }
    }
    published
}

#[cfg(test)]
mod metrics_tests {
    use super::*;
    use crate::aws::metrics::{self as m, MetricQuery, MetricSpec, MetricTarget};

    const FS: &str = "fs-1";

    fn kv(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    fn at(metric: &str, extra: &[(&str, &str)]) -> (String, Vec<(String, String)>) {
        let mut dims = kv(&[("FileSystemId", FS)]);
        dims.extend(kv(extra));
        (metric.to_string(), dims)
    }

    fn set_of(specs: &'static [MetricSpec], dims: Vec<(String, String)>) -> Vec<MetricQuery> {
        MetricTarget {
            title: String::new(),
            namespace: "AWS/FSx".into(),
            dimensions: dims,
            specs,
            home: None,
            hint: None,
            extra: Vec::new(),
            breakdown: None,
            resolve: None,
        }
        .curated_queries()
    }

    fn labels(set: &[MetricQuery]) -> Vec<&str> {
        set.iter().map(|q| q.label.as_str()).collect()
    }

    /// A Windows file system under 32 MBps publishes only the six basics:
    /// the rest go, and the utilisation it can't publish is worked out
    /// from its free bytes and size.
    #[test]
    fn a_small_windows_file_system_keeps_what_it_publishes() {
        let fs = kv(&[("FileSystemId", FS)]);
        let variants: Vec<_> = ["FreeStorageCapacity", "DataReadBytes", "DataWriteBytes", "DataReadOperations", "DataWriteOperations", "MetadataOperations"]
            .iter()
            .map(|n| at(n, &[]))
            .collect();
        let set = curate(&variants, &fs, 32 << 30, &set_of(m::FSX_WINDOWS, fs.clone()));
        assert_eq!(labels(&set), ["Storage used", "Free storage", "Read", "Write", "Read IOPS", "Write IOPS", "Metadata IOPS"]);
        assert_eq!(set[0].expression.as_deref(), Some("100*(1-{FreeStorageCapacity}/34359738368)"));
        assert!(set[1..].iter().all(|q| q.expression.is_none() && q.dimensions == fs), "published as asked");
    }

    /// Lustre publishes capacity per storage target and utilisation per
    /// server: one series each, summed or the busiest. Charting one target
    /// would show a sliver of the file system.
    #[test]
    fn lustre_folds_each_target_s_series_into_one() {
        let fs = kv(&[("FileSystemId", FS)]);
        let variants = vec![
            at("FreeDataStorageCapacity", &[("StorageTargetId", "OST0000")]),
            at("FreeDataStorageCapacity", &[("StorageTargetId", "OST0001")]),
            at("StorageCapacityUtilization", &[("StorageTargetId", "OST0000")]),
            at("StorageCapacityUtilization", &[("StorageTargetId", "OST0001")]),
            at("NetworkThroughputUtilization", &[("FileServer", "OSS0000")]),
            at("NetworkThroughputUtilization", &[("FileServer", "OSS0002")]),
            at("DataReadBytes", &[]),
        ];
        let set = curate(&variants, &fs, 1200 << 30, &set_of(m::FSX_LUSTRE, fs.clone()));
        assert_eq!(labels(&set), ["Storage used (max)", "Free storage", "Read", "Network (max)"]);
        let free = set.iter().find(|q| q.metric == "FreeDataStorageCapacity").unwrap();
        assert_eq!(
            free.expression.as_deref(),
            Some("SUM(SEARCH('{AWS/FSx,FileSystemId,StorageTargetId} MetricName=\"FreeDataStorageCapacity\" FileSystemId=\"fs-1\"', 'Average', $PERIOD))")
        );
        let net = set.iter().find(|q| q.metric == "NetworkThroughputUtilization").unwrap();
        assert!(net.expression.as_deref().unwrap().starts_with("MAX(SEARCH('{AWS/FSx,FileServer,FileSystemId}"), "{net:?}");
        assert!(set.iter().find(|q| q.metric == "DataReadBytes").unwrap().expression.is_none());
    }

    /// ONTAP's utilisation is per tier: the SSD tier's, not a sum with the
    /// capacity pool. Latency stays only where its operation time is
    /// published, and a volume's series never stand in for the file
    /// system's.
    #[test]
    fn ontap_charts_the_ssd_tier_and_the_latency_it_can() {
        let fs = kv(&[("FileSystemId", FS)]);
        let variants = vec![
            at("StorageCapacityUtilization", &[("StorageTier", "SSD"), ("DataType", "All")]),
            at("StorageCapacityUtilization", &[("StorageTier", "StandardCapacityPool"), ("DataType", "All")]),
            at("StorageUsed", &[]),
            at("StorageUsed", &[("StorageTier", "SSD"), ("DataType", "All")]),
            at("DataReadOperations", &[]),
            at("DataReadOperationTime", &[]),
            at("DataWriteOperations", &[]),
            at("DataReadBytes", &[("VolumeId", "fsvol-1")]),
        ];
        let set = curate(&variants, &fs, 1 << 40, &set_of(m::FSX_ONTAP, fs.clone()));
        assert_eq!(labels(&set), ["SSD used", "Used storage", "Read IOPS", "Write IOPS", "Read latency", "Read time"]);
        let ssd = &set[0];
        assert!(ssd.dimensions.contains(&("StorageTier".into(), "SSD".into())), "{ssd:?}");
        assert_eq!(set[1].dimensions, fs, "the file system's own StorageUsed");
        assert!(set.last().unwrap().hidden, "the latency's input, last");
    }

    #[test]
    fn a_volume_charts_its_own_series() {
        let vol = kv(&[("FileSystemId", FS), ("VolumeId", "fsvol-1")]);
        let mut used = at("StorageUsed", &[("VolumeId", "fsvol-1")]);
        used.1.reverse();
        let variants = vec![used, at("DataReadBytes", &[("VolumeId", "fsvol-1")])];
        let set = curate(&variants, &vol, 1 << 30, &set_of(m::FSX_ONTAP_VOLUME, vol.clone()));
        assert_eq!(labels(&set), ["Used storage", "Read"], "dimension order doesn't matter");
    }

    #[test]
    fn nothing_published_is_no_row() {
        let fs = kv(&[("FileSystemId", FS)]);
        assert_eq!(published("CPUUtilization", &[at("DataReadBytes", &[])], &fs), None);
    }

    /// Used storage by tier reads the `DataType=All` cells and by data type
    /// sums the tiers: nothing counted twice.
    #[test]
    fn storage_folds_by_tier_and_by_data_type() {
        let cell = |t: &str, d: &str, v: f64| (t.to_string(), d.to_string(), v);
        let cells = vec![
            cell("SSD", "All", 10.0),
            cell("StandardCapacityPool", "All", 30.0),
            cell("SSD", "User", 8.0),
            cell("SSD", "Snapshot", 2.0),
            cell("StandardCapacityPool", "User", 30.0),
        ];
        let (tier, kind) = fold_storage(&cells);
        assert_eq!(tier, [("Capacity Pool".to_string(), 30.0), ("SSD".to_string(), 10.0)]);
        assert_eq!(kind, [("User".to_string(), 38.0), ("Snapshot".to_string(), 2.0)]);
    }

    #[test]
    fn the_volumes_table_lists_every_volume_busiest_first() {
        let b = FsxBreakdown {
            file_system_id: FS.into(),
            volume_id: None,
            names: vec![("fsvol-1".into(), "home".into()), ("fsvol-2".into(), "scratch".into())],
            volume_specs: m::FSX_ONTAP_VOLUME,
            tiers: true,
        };
        let read = HashMap::from([("fsvol-2".to_string(), 5e9), ("fsvol-9".to_string(), 1e9)]);
        let t = b.volumes_table(&read, &HashMap::new());
        let names: Vec<&str> = t.rows.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["scratch", "fsvol-9", "home"], "an unnamed id stays an id; a quiet volume is still listed");
        let drill = t.drill.as_ref().unwrap();
        assert_eq!(drill.dimensions[0], kv(&[("FileSystemId", FS), ("VolumeId", "fsvol-2")]));
        let (header, rows) = t.text(80, 10);
        assert!(header.contains("Read") && rows[0].contains("GB"), "{header} / {rows:?}");
        let quiet = b.volumes_table(&HashMap::new(), &HashMap::new());
        let (header, _) = quiet.text(80, 10);
        assert!(header.contains("Read") && header.contains("Write"), "a quiet table still says what it counts: {header}");
    }
}
