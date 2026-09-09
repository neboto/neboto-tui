use crate::aws::client::AwsClients;
use crate::aws::resource::{native_state_label, Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::error::Result;
use crate::event::{Event, LoadProgress};
use crate::aws::services::ec2::MetricsTimeRange;
use async_trait::async_trait;
use aws_sdk_efs::Client as EfsClient;
use std::any::Any;
use std::collections::HashMap;
use tokio::sync::mpsc;

/// EFS service — single-list of file systems with a split detail pane. Mount
/// targets and access points are *per-file-system*, so they're fetched lazily
/// on first view. The performance picture (throughput mode, IOPS saturation,
/// burst credit) is the operational signal here, surfaced both in Details and
/// the `m` CloudWatch metrics overlay.
pub struct EfsService {
    client: EfsClient,
}

impl EfsService {
    pub fn new(aws_clients: &AwsClients) -> Self {
        Self {
            client: aws_clients.efs_client(),
        }
    }
}

#[async_trait]
impl AwsService for EfsService {
    fn service_type(&self) -> ServiceType {
        ServiceType::Efs
    }

    fn name(&self) -> &str {
        "EFS"
    }

    async fn list_resources(&self) -> Result<Vec<Box<dyn Resource>>> {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        self.list_resources_streaming(tx, ServiceType::Efs).await?;
        Ok(vec![])
    }

    async fn list_resources_streaming(
        &self,
        event_tx: mpsc::UnboundedSender<Event>,
        service_type: ServiceType,
    ) -> Result<()> {
        // `describe_file_systems` returns everything except the lifecycle policy
        // and backup config; lifecycle is cheap and few file systems exist per
        // region, so we fetch it concurrently per fs to keep Details eager.
        let mut total = 0usize;
        let mut paginator = self.client.describe_file_systems().into_paginator().send();

        while let Some(result) = paginator.next().await {
            match result {
                Ok(page) => {
                    let descriptions = page.file_systems().to_vec();
                    if descriptions.is_empty() {
                        continue;
                    }

                    let futs: Vec<_> = descriptions
                        .iter()
                        .map(|fs| {
                            let client = self.client.clone();
                            let id = fs.file_system_id().to_string();
                            async move {
                                client
                                    .describe_lifecycle_configuration()
                                    .file_system_id(&id)
                                    .send()
                                    .await
                                    .ok()
                                    .map(|r| EfsLifecycle::from_policies(r.lifecycle_policies()))
                                    .unwrap_or_default()
                            }
                        })
                        .collect();
                    let lifecycles = futures::future::join_all(futs).await;

                    let batch: Vec<Box<dyn Resource>> = descriptions
                        .iter()
                        .zip(lifecycles)
                        .map(|(fs, lc)| {
                            Box::new(EfsFileSystem::from_sdk(fs, lc)) as Box<dyn Resource>
                        })
                        .collect();

                    total += batch.len();
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
                Err(e) => {
                    let _ = event_tx.send(Event::ResourceLoadError {
                        service: service_type,
                        error: format!(
                            "Failed to list EFS file systems: {}",
                            crate::error::sdk_error_message(&e)
                        ),
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

// ── Lifecycle policy (transition rules) ───────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct EfsLifecycle {
    pub to_ia: Option<String>,
    pub to_archive: Option<String>,
    pub to_primary: Option<String>,
}

impl EfsLifecycle {
    fn from_policies(policies: &[aws_sdk_efs::types::LifecyclePolicy]) -> Self {
        let mut out = Self::default();
        for p in policies {
            if let Some(v) = p.transition_to_ia() {
                out.to_ia = Some(v.as_str().to_string());
            }
            if let Some(v) = p.transition_to_archive() {
                out.to_archive = Some(v.as_str().to_string());
            }
            if let Some(v) = p.transition_to_primary_storage_class() {
                out.to_primary = Some(v.as_str().to_string());
            }
        }
        out
    }

    pub fn is_empty(&self) -> bool {
        self.to_ia.is_none() && self.to_archive.is_none() && self.to_primary.is_none()
    }
}

// ── EfsFileSystem ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct EfsFileSystem {
    pub file_system_id: String,
    pub fs_name: String,
    pub arn: String,
    pub lifecycle_state: String,
    pub performance_mode: String, // generalPurpose / maxIO
    pub throughput_mode: String,  // bursting / elastic / provisioned
    pub provisioned_mibps: Option<f64>,
    pub size_total: i64,
    pub size_standard: i64,
    pub size_ia: i64,
    pub size_archive: i64,
    pub number_of_mount_targets: i32,
    pub encrypted: bool,
    pub kms_key_id: Option<String>,
    pub availability_zone_name: Option<String>, // Some => One Zone
    pub created: Option<String>,
    pub lifecycle: EfsLifecycle,
    pub tags: HashMap<String, String>,
}

impl EfsFileSystem {
    pub fn from_sdk(fs: &aws_sdk_efs::types::FileSystemDescription, lifecycle: EfsLifecycle) -> Self {
        let size = fs.size_in_bytes();
        let (size_total, size_standard, size_ia, size_archive) = match size {
            Some(s) => (
                s.value(),
                s.value_in_standard().unwrap_or(0),
                s.value_in_ia().unwrap_or(0),
                s.value_in_archive().unwrap_or(0),
            ),
            None => (0, 0, 0, 0),
        };

        let mut tags: HashMap<String, String> = HashMap::new();
        for t in fs.tags() {
            tags.insert(t.key().to_string(), t.value().to_string());
        }

        let fs_name = fs
            .name()
            .map(|n| n.to_string())
            .filter(|n| !n.is_empty())
            .or_else(|| tags.get("Name").cloned())
            .unwrap_or_else(|| fs.file_system_id().to_string());

        Self {
            file_system_id: fs.file_system_id().to_string(),
            fs_name,
            arn: fs.file_system_arn().unwrap_or_default().to_string(),
            lifecycle_state: fs.life_cycle_state().as_str().to_string(),
            performance_mode: fs.performance_mode().as_str().to_string(),
            throughput_mode: fs
                .throughput_mode()
                .map(|t| t.as_str().to_string())
                .unwrap_or_default(),
            provisioned_mibps: fs.provisioned_throughput_in_mibps(),
            size_total,
            size_standard,
            size_ia,
            size_archive,
            number_of_mount_targets: fs.number_of_mount_targets(),
            encrypted: fs.encrypted().unwrap_or(false),
            kms_key_id: fs.kms_key_id().map(|s| s.to_string()),
            availability_zone_name: fs.availability_zone_name().map(|s| s.to_string()),
            created: Some(fmt_epoch_secs(fs.creation_time().secs())),
            lifecycle,
            tags,
        }
    }

    /// "One Zone" vs "Regional" — One Zone systems store in a single AZ.
    pub fn storage_class(&self) -> &str {
        if self.availability_zone_name.is_some() {
            "One Zone"
        } else {
            "Regional"
        }
    }

    /// Human-readable throughput summary including the provisioned rate.
    pub fn throughput_summary(&self) -> String {
        match (self.throughput_mode.as_str(), self.provisioned_mibps) {
            ("provisioned", Some(m)) => format!("provisioned ({} MiB/s)", trim_float(m)),
            (mode, _) if !mode.is_empty() => mode.to_string(),
            _ => "—".to_string(),
        }
    }
}

crate::sections! {
    pub enum EfsFileSystemDetailSection,
    pub static EFS_FS_SECTIONS = [
        Details "Details",
        MountTargets "Mount Targets" => crate::app::App::trigger_efs_mount_targets_load,
        AccessPoints "Access Points" => crate::app::App::trigger_efs_access_points_load,
        Tags "Tags",
    ]
}

impl Resource for EfsFileSystem {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&EFS_FS_SECTIONS)
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws efs describe-file-systems --file-system-id {}",
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
        "EFS File System"
    }

    fn state(&self) -> ResourceState {
        match self.lifecycle_state.as_str() {
            "available" => ResourceState::Available,
            "creating" => ResourceState::Creating,
            "updating" => ResourceState::Pending,
            "deleting" | "deleted" => ResourceState::Deleting,
            "error" => ResourceState::Unavailable,
            other => ResourceState::Unknown(other.to_string()),
        }
    }

    fn state_label(&self) -> String {
        native_state_label(&self.lifecycle_state, || self.state())
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!(
            "{} {} {} {} {}",
            self.fs_name,
            self.file_system_id,
            self.performance_mode,
            self.throughput_mode,
            self.arn,
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Name".to_string(), self.fs_name.clone()),
            ("File System ID".to_string(), self.file_system_id.clone()),
            ("State".to_string(), self.lifecycle_state.clone()),
            ("Performance".to_string(), self.performance_mode.clone()),
            ("Throughput".to_string(), self.throughput_summary()),
            ("Size".to_string(), fmt_bytes(self.size_total)),
            ("Mount Targets".to_string(), self.number_of_mount_targets.to_string()),
        ]
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/efs/home?region={}#/file-systems/{}",
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

// ── Children (rendered lazily in the split pane) ──────────────────────────────

#[derive(Debug, Clone)]
pub struct EfsMountTarget {
    pub mount_target_id: String,
    pub availability_zone: String,
    pub subnet_id: String,
    pub ip_address: String,
    pub lifecycle_state: String,
    pub network_interface_id: String,
}

#[derive(Debug, Clone)]
pub struct EfsAccessPoint {
    pub access_point_id: String,
    pub name: String,
    pub lifecycle_state: String,
    pub posix_user: Option<String>, // "uid:gid"
    pub root_directory: String,
}

// ── Lazy fetch functions (per file system) ────────────────────────────────────

/// `describe_mount_targets` for one file system.
pub async fn fetch_efs_mount_targets(
    client: EfsClient,
    file_system_id: String,
) -> Result<Vec<EfsMountTarget>> {
    let resp = client
        .describe_mount_targets()
        .file_system_id(&file_system_id)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;

    let mut out = Vec::new();
    for mt in resp.mount_targets() {
        out.push(EfsMountTarget {
            mount_target_id: mt.mount_target_id().to_string(),
            availability_zone: mt.availability_zone_name().unwrap_or_default().to_string(),
            subnet_id: mt.subnet_id().to_string(),
            ip_address: mt.ip_address().unwrap_or_default().to_string(),
            lifecycle_state: mt.life_cycle_state().as_str().to_string(),
            network_interface_id: mt.network_interface_id().unwrap_or_default().to_string(),
        });
    }
    Ok(out)
}

/// `describe_access_points` for one file system.
pub async fn fetch_efs_access_points(
    client: EfsClient,
    file_system_id: String,
) -> Result<Vec<EfsAccessPoint>> {
    let resp = client
        .describe_access_points()
        .file_system_id(&file_system_id)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;

    let mut out = Vec::new();
    for ap in resp.access_points() {
        let posix_user = ap
            .posix_user()
            .map(|u| format!("{}:{}", u.uid(), u.gid()));
        let root_directory = ap
            .root_directory()
            .and_then(|r| r.path())
            .unwrap_or("/")
            .to_string();
        out.push(EfsAccessPoint {
            access_point_id: ap.access_point_id().unwrap_or_default().to_string(),
            name: ap.name().unwrap_or_default().to_string(),
            lifecycle_state: ap
                .life_cycle_state()
                .map(|s| s.as_str().to_string())
                .unwrap_or_default(),
            posix_user,
            root_directory,
        });
    }
    Ok(out)
}

// ── CloudWatch metrics (`m` overlay) ──────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct EfsMetricsData {
    pub time_range: MetricsTimeRange,
    pub total_io: Vec<(f64, f64)>,
    pub read_io: Vec<(f64, f64)>,
    pub write_io: Vec<(f64, f64)>,
    pub percent_io_limit: Vec<(f64, f64)>,
    pub burst_credit: Vec<(f64, f64)>,
    pub client_connections: Vec<(f64, f64)>,
    pub x_max: f64,
}

#[derive(Debug, Clone)]
pub enum EfsMetricsState {
    Loading,
    Loaded(EfsMetricsData),
}

/// Pull the EFS performance metrics from `AWS/EFS` for one file system.
/// Throughput series are summed bytes/period; `PercentIOLimit` is the max
/// saturation against the generalPurpose IOPS ceiling; `BurstCreditBalance`
/// is the average remaining burst credit.
pub async fn fetch_efs_metrics(
    cw: aws_sdk_cloudwatch::Client,
    file_system_id: String,
    time_range: MetricsTimeRange,
) -> Result<EfsMetricsData> {
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
            .name("FileSystemId")
            .value(&file_system_id)
            .build()
    };
    let metric = |name: &'static str, stat: Statistic| {
        cw.get_metric_statistics()
            .namespace("AWS/EFS")
            .metric_name(name)
            .dimensions(dim())
            .start_time(start_dt)
            .end_time(end_dt)
            .period(period)
            .set_statistics(Some(vec![stat]))
            .send()
    };

    let (total, read, write, pct, burst, conns) = tokio::join!(
        metric("TotalIOBytes", Statistic::Sum),
        metric("DataReadIOBytes", Statistic::Sum),
        metric("DataWriteIOBytes", Statistic::Sum),
        metric("PercentIOLimit", Statistic::Maximum),
        metric("BurstCreditBalance", Statistic::Average),
        metric("ClientConnections", Statistic::Sum),
    );

    Ok(EfsMetricsData {
        time_range,
        total_io: parse_points(total, start, |dp| dp.sum()),
        read_io: parse_points(read, start, |dp| dp.sum()),
        write_io: parse_points(write, start, |dp| dp.sum()),
        percent_io_limit: parse_points(pct, start, |dp| dp.maximum()),
        burst_credit: parse_points(burst, start, |dp| dp.average()),
        client_connections: parse_points(conns, start, |dp| dp.sum()),
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

// ── Formatting helpers ────────────────────────────────────────────────────────

/// Human-readable byte size (binary units).
pub fn fmt_bytes(bytes: i64) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    if bytes <= 0 {
        return "0 B".to_string();
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", bytes, UNITS[unit])
    } else {
        format!("{:.2} {}", value, UNITS[unit])
    }
}

fn trim_float(v: f64) -> String {
    if (v.fract()).abs() < f64::EPSILON {
        format!("{}", v as i64)
    } else {
        format!("{:.2}", v)
    }
}

fn fmt_epoch_secs(secs: i64) -> String {
    let days = secs / 86400;
    let rem = secs % 86400;
    let (h, mi) = (rem / 3600, (rem % 3600) / 60);
    let (y, mo, d) = epoch_days_to_ymd(days);
    format!("{:04}-{:02}-{:02} {:02}:{:02} UTC", y, mo, d, h, mi)
}

fn epoch_days_to_ymd(mut days: i64) -> (i32, u8, u8) {
    let mut year = 1970i32;
    loop {
        let dy = if is_leap(year) { 366 } else { 365 };
        if days < dy {
            break;
        }
        days -= dy;
        year += 1;
    }
    let dm = [
        31u8,
        if is_leap(year) { 29 } else { 28 },
        31, 30, 31, 30, 31, 31, 30, 31, 30, 31,
    ];
    let mut month = 1u8;
    for &len in &dm {
        if days < len as i64 {
            break;
        }
        days -= len as i64;
        month += 1;
    }
    (year, month, (days + 1) as u8)
}

fn is_leap(y: i32) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}
