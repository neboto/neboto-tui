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
        Details "Details",
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
/// read/write/metadata latency (ms = `*OperationTime`µs ÷ `*Operations`).
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

    let lat = |time_us: Option<f64>, ops: Option<f64>| match (time_us, ops) {
        (Some(t), Some(o)) if o > 0.0 => Some((t / o) / 1000.0),
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
// CloudWatch storage metrics (`m` overlay)
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct FsxMetricsData {
    pub time_range: MetricsTimeRange,
    pub storage_utilization: Vec<(f64, f64)>, // percent
    pub storage_bytes: Vec<(f64, f64)>,       // free or used bytes (see label)
    pub storage_label: String,                // chart title for `storage_bytes`
    pub read_bytes: Vec<(f64, f64)>,          // bytes/period
    pub write_bytes: Vec<(f64, f64)>,         // bytes/period
    pub read_latency: Vec<(f64, f64)>,        // ms/op
    pub write_latency: Vec<(f64, f64)>,       // ms/op
    pub metadata_latency: Vec<(f64, f64)>,    // ms/op
    /// Per-volume StorageUsed composition (the console's pie equivalent),
    /// `(label, bytes)` largest-first. Empty for file-system / non-ONTAP.
    pub tiers: Vec<(String, f64)>,
    pub data_types: Vec<(String, f64)>,
    pub x_max: f64,
}

#[derive(Debug, Clone)]
pub enum FsxMetricsState {
    Loading,
    Loaded(FsxMetricsData),
}

/// Storage-focused `AWS/FSx` metrics for one file system — the console's
/// "storage" graphs. Metric names + dimensions differ per file-system type
/// (Windows `FreeStorageCapacity`; Lustre `FreeDataStorageCapacity`; ONTAP
/// `StorageUsed`/`StorageCapacityUtilization` with extra `StorageTier`/`DataType`
/// dims; OpenZFS `UsedStorageCapacity`), so instead of guessing we **discover**
/// what's actually published for this file system via `ListMetrics` and query
/// those exact metrics with their real dimensions. Utilization falls back to a
/// value computed from used/free bytes + the configured capacity when the
/// percentage metric isn't published.
pub async fn fetch_fsx_metrics(
    cw: aws_sdk_cloudwatch::Client,
    file_system_id: String,
    volume_id: Option<String>,
    total_capacity_bytes: i64,
    time_range: MetricsTimeRange,
) -> Result<FsxMetricsData> {
    use aws_sdk_cloudwatch::types::Statistic;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let start = now - time_range.duration_secs();
    let period = time_range.period_secs();
    let start_dt = aws_sdk_cloudwatch::primitives::DateTime::from_secs(start);
    let end_dt = aws_sdk_cloudwatch::primitives::DateTime::from_secs(now);

    // Discover metrics for the file system as a whole, or one volume.
    let (dim_name, dim_value) = match &volume_id {
        Some(v) => ("VolumeId", v.as_str()),
        None => ("FileSystemId", file_system_id.as_str()),
    };
    let variants = discover_fsx_metrics(&cw, dim_name, dim_value).await;
    let published = fewest_by_name(&variants);

    // Resolve a metric series by trying candidate names in priority order.
    let pick = |candidates: &[&str]| -> Option<(String, Vec<aws_sdk_cloudwatch::types::Dimension>)> {
        candidates
            .iter()
            .find_map(|n| published.get(*n).map(|d| (n.to_string(), d.clone())))
    };

    // Used vs free: ONTAP `StorageUsed`, OpenZFS `UsedStorageCapacity`, then
    // free variants (Windows `FreeStorageCapacity`, Lustre `FreeDataStorageCapacity`).
    let util_m = pick(&["StorageCapacityUtilization"]);
    let used_m = pick(&["StorageUsed", "UsedStorageCapacity"]);
    let free_m = pick(&["FreeStorageCapacity", "FreeDataStorageCapacity"]);
    let read_m = pick(&["DataReadBytes"]);
    let write_m = pick(&["DataWriteBytes"]);

    fn avg(dp: &aws_sdk_cloudwatch::types::Datapoint) -> Option<f64> {
        dp.average()
    }
    fn sum(dp: &aws_sdk_cloudwatch::types::Datapoint) -> Option<f64> {
        dp.sum()
    }

    let (util, used, free, read_bytes, write_bytes) = tokio::join!(
        get_fsx_series(&cw, util_m, Statistic::Average, start_dt, end_dt, period, start, avg),
        get_fsx_series(&cw, used_m, Statistic::Average, start_dt, end_dt, period, start, avg),
        get_fsx_series(&cw, free_m, Statistic::Average, start_dt, end_dt, period, start, avg),
        get_fsx_series(&cw, read_m, Statistic::Sum, start_dt, end_dt, period, start, sum),
        get_fsx_series(&cw, write_m, Statistic::Sum, start_dt, end_dt, period, start, sum),
    );

    // Latency = total operation time ÷ operation count, per period. ONTAP
    // publishes the *Operations counts and *OperationTime (microseconds) totals.
    let rops_m = pick(&["DataReadOperations"]);
    let wops_m = pick(&["DataWriteOperations"]);
    let mops_m = pick(&["MetadataOperations"]);
    let rtime_m = pick(&["DataReadOperationTime"]);
    let wtime_m = pick(&["DataWriteOperationTime"]);
    let mtime_m = pick(&["MetadataOperationTime"]);
    let (rops, wops, mops, rtime, wtime, mtime) = tokio::join!(
        get_fsx_series(&cw, rops_m, Statistic::Sum, start_dt, end_dt, period, start, sum),
        get_fsx_series(&cw, wops_m, Statistic::Sum, start_dt, end_dt, period, start, sum),
        get_fsx_series(&cw, mops_m, Statistic::Sum, start_dt, end_dt, period, start, sum),
        get_fsx_series(&cw, rtime_m, Statistic::Sum, start_dt, end_dt, period, start, sum),
        get_fsx_series(&cw, wtime_m, Statistic::Sum, start_dt, end_dt, period, start, sum),
        get_fsx_series(&cw, mtime_m, Statistic::Sum, start_dt, end_dt, period, start, sum),
    );
    let read_latency = compute_latency_ms(&rtime, &rops);
    let write_latency = compute_latency_ms(&wtime, &wops);
    let metadata_latency = compute_latency_ms(&mtime, &mops);

    // Prefer used bytes for the second chart; else free bytes.
    let (storage_bytes, storage_label) = if !used.is_empty() {
        (used.clone(), "Used Storage (bytes)".to_string())
    } else if !free.is_empty() {
        (free.clone(), "Free Storage (bytes)".to_string())
    } else {
        (Vec::new(), "Storage (bytes)".to_string())
    };

    // Utilization: native metric if present, else computed from capacity.
    let mut storage_utilization = util;
    if storage_utilization.is_empty() && total_capacity_bytes > 0 {
        let total = total_capacity_bytes as f64;
        if !used.is_empty() {
            storage_utilization = used.iter().map(|(t, b)| (*t, (b / total) * 100.0)).collect();
        } else if !free.is_empty() {
            storage_utilization = free
                .iter()
                .map(|(t, b)| (*t, ((total - b) / total) * 100.0))
                .collect();
        }
    }

    // Storage composition (the console's pie): StorageUsed broken down by
    // StorageTier and by DataType — for the volume, or for the file system as a
    // whole. Computed for both; whichever level publishes the data shows bars.
    let (tiers, data_types) =
        fetch_storage_breakdown(&cw, &variants, volume_id.is_some(), start_dt, end_dt).await;

    Ok(FsxMetricsData {
        time_range,
        storage_utilization,
        storage_bytes,
        storage_label,
        read_bytes,
        write_bytes,
        read_latency,
        write_latency,
        metadata_latency,
        tiers,
        data_types,
        x_max: time_range.duration_secs() as f64,
    })
}

/// Average op latency in **milliseconds** per period from total operation time
/// (microseconds) ÷ operation count, aligned by timestamp.
fn compute_latency_ms(time_us: &[(f64, f64)], ops: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let op_at: HashMap<u64, f64> = ops.iter().map(|(t, v)| (t.to_bits(), *v)).collect();
    time_us
        .iter()
        .filter_map(|(t, total_us)| {
            let count = *op_at.get(&t.to_bits())?;
            if count > 0.0 {
                Some((*t, (total_us / count) / 1000.0))
            } else {
                None
            }
        })
        .collect()
}

/// StorageUsed composition by `StorageTier` and by `DataType`. Scoped to the
/// file system (no `VolumeId` dim) or to one volume (`is_volume`, variants are
/// already `VolumeId`-filtered). Returns `(by_tier, by_data_type)`, each
/// `(label, bytes)` largest-first; empty when the level doesn't publish them.
async fn fetch_storage_breakdown(
    cw: &aws_sdk_cloudwatch::Client,
    variants: &[(String, Vec<aws_sdk_cloudwatch::types::Dimension>)],
    is_volume: bool,
    start_dt: aws_sdk_cloudwatch::primitives::DateTime,
    end_dt: aws_sdk_cloudwatch::primitives::DateTime,
) -> (Vec<(String, f64)>, Vec<(String, f64)>) {
    let has_dim = |dims: &[aws_sdk_cloudwatch::types::Dimension], name: &str| {
        dims.iter().any(|d| d.name() == Some(name))
    };
    // StorageUsed variants, scoped: file-system level excludes per-volume cells.
    let su: Vec<&Vec<aws_sdk_cloudwatch::types::Dimension>> = variants
        .iter()
        .filter(|(n, _)| n == "StorageUsed")
        .filter(|(_, d)| is_volume || !has_dim(d, "VolumeId"))
        .map(|(_, d)| d)
        .collect();
    if su.is_empty() {
        return (Vec::new(), Vec::new());
    }

    let by_tier = group_storage_used(cw, &su, "StorageTier", "DataType", start_dt, end_dt).await;
    let by_dtype = group_storage_used(cw, &su, "DataType", "StorageTier", start_dt, end_dt).await;
    (by_tier, by_dtype)
}

/// Sum `StorageUsed` (latest datapoint) by `group_dim`, avoiding double counting
/// over `other_dim`: prefer the `other_dim`-rollup ("All" or absent) per group;
/// otherwise sum the specific `other_dim` cells. Tier labels are prettified.
async fn group_storage_used(
    cw: &aws_sdk_cloudwatch::Client,
    su: &[&Vec<aws_sdk_cloudwatch::types::Dimension>],
    group_dim: &str,
    other_dim: &str,
    start_dt: aws_sdk_cloudwatch::primitives::DateTime,
    end_dt: aws_sdk_cloudwatch::primitives::DateTime,
) -> Vec<(String, f64)> {
    use aws_sdk_cloudwatch::types::Statistic;
    let dval = |dims: &[aws_sdk_cloudwatch::types::Dimension], name: &str| -> Option<String> {
        dims.iter()
            .find(|d| d.name() == Some(name))
            .and_then(|d| d.value())
            .map(|s| s.to_string())
    };
    let is_rollup = |dims: &[aws_sdk_cloudwatch::types::Dimension]| match dval(dims, other_dim) {
        None => true,
        Some(v) => v.eq_ignore_ascii_case("All"),
    };

    // Variants that carry a specific (non-"All") group value.
    let with_group: Vec<&Vec<aws_sdk_cloudwatch::types::Dimension>> = su
        .iter()
        .copied()
        .filter(|d| {
            dval(d, group_dim)
                .map(|v| !v.is_empty() && !v.eq_ignore_ascii_case("All"))
                .unwrap_or(false)
        })
        .collect();
    if with_group.is_empty() {
        return Vec::new();
    }
    // Prefer rollup cells (one per group); else sum the specific other-dim cells.
    let any_rollup = with_group.iter().any(|d| is_rollup(d));
    let chosen: Vec<&Vec<aws_sdk_cloudwatch::types::Dimension>> = with_group
        .into_iter()
        .filter(|d| if any_rollup { is_rollup(d) } else { !is_rollup(d) })
        .collect();

    let vals = futures::future::join_all(chosen.iter().map(|dims| {
        latest_value(
            cw.clone(),
            Some(("StorageUsed".to_string(), (*dims).clone())),
            Statistic::Average,
            start_dt,
            end_dt,
        )
    }))
    .await;

    let mut map: std::collections::BTreeMap<String, f64> = std::collections::BTreeMap::new();
    for (dims, v) in chosen.iter().zip(vals) {
        if let Some(v) = v {
            let key = dval(dims, group_dim).unwrap_or_default();
            let key = if group_dim == "StorageTier" {
                pretty_tier(&key)
            } else {
                key
            };
            *map.entry(key).or_default() += v;
        }
    }
    let mut out: Vec<(String, f64)> = map.into_iter().collect();
    out.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    out
}

fn pretty_tier(t: &str) -> String {
    match t {
        "StandardCapacityPool" => "Capacity Pool".to_string(),
        "SSD" => "SSD".to_string(),
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

fn parse_resp(
    resp: aws_sdk_cloudwatch::operation::get_metric_statistics::GetMetricStatisticsOutput,
    start: i64,
    pick: impl Fn(&aws_sdk_cloudwatch::types::Datapoint) -> Option<f64>,
) -> Vec<(f64, f64)> {
    let mut pts: Vec<(f64, f64)> = resp
        .datapoints()
        .iter()
        .filter_map(|dp| Some((dp.timestamp()?.secs() as f64 - start as f64, pick(dp)?)))
        .collect();
    pts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    pts
}

/// Fetch one discovered metric (name + its real dimensions) as a parsed series.
/// `None` metric or any error → empty series.
#[allow(clippy::too_many_arguments)]
async fn get_fsx_series(
    cw: &aws_sdk_cloudwatch::Client,
    metric: Option<(String, Vec<aws_sdk_cloudwatch::types::Dimension>)>,
    stat: aws_sdk_cloudwatch::types::Statistic,
    start_dt: aws_sdk_cloudwatch::primitives::DateTime,
    end_dt: aws_sdk_cloudwatch::primitives::DateTime,
    period: i32,
    start: i64,
    pick: fn(&aws_sdk_cloudwatch::types::Datapoint) -> Option<f64>,
) -> Vec<(f64, f64)> {
    let (name, dims) = match metric {
        Some(m) => m,
        None => return Vec::new(),
    };
    match cw
        .get_metric_statistics()
        .namespace("AWS/FSx")
        .metric_name(&name)
        .set_dimensions(Some(dims))
        .start_time(start_dt)
        .end_time(end_dt)
        .period(period)
        .set_statistics(Some(vec![stat]))
        .send()
        .await
    {
        Ok(r) => parse_resp(r, start, pick),
        Err(_) => Vec::new(),
    }
}

