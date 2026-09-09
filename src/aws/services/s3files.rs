use crate::aws::client::AwsClients;
use crate::aws::resource::{native_state_label, shell_quote, Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::error::Result;
use crate::event::{Event, LoadProgress};
use async_trait::async_trait;
use aws_sdk_s3files::types::LifeCycleState;
use aws_sdk_s3files::Client as S3FilesClient;
use std::any::Any;
use std::collections::HashMap;
use tokio::sync::mpsc;

const MAX_FILE_SYSTEMS: usize = 200;
const MAX_MOUNT_TARGETS: usize = 50;
const MAX_ACCESS_POINTS: usize = 100;

/// Amazon S3 Files — shared file systems (built on EFS) linked to an S3
/// bucket or prefix, mountable over NFS from EC2/Lambda/EKS/ECS with two-way
/// synchronization. Single-list (file systems); mount targets, access
/// points, the file system policy and the sync configuration are lazy
/// sections on the split pane. Read-only: no create/delete/put APIs called.
pub struct S3FilesService {
    client: S3FilesClient,
}

impl S3FilesService {
    pub fn new(aws_clients: &AwsClients) -> Self {
        Self {
            client: aws_clients.s3files_client(),
        }
    }
}

#[async_trait]
impl AwsService for S3FilesService {
    fn service_type(&self) -> ServiceType {
        ServiceType::S3Files
    }

    fn name(&self) -> &str {
        "S3 Files"
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
        let mut pager = self.client.list_file_systems().into_paginator().send();
        loop {
            match pager.next().await {
                Some(Ok(page)) => {
                    let batch: Vec<Box<dyn Resource>> = page
                        .file_systems()
                        .iter()
                        .map(|f| Box::new(S3FileSystem::from_sdk(f)) as Box<dyn Resource>)
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
                            status_message: None,
                        },
                    });
                    if total >= MAX_FILE_SYSTEMS {
                        break;
                    }
                }
                Some(Err(e)) => {
                    // Single-phase load — a list failure is total.
                    let _ = event_tx.send(Event::ResourceLoadError {
                        service: service_type,
                        error: crate::error::sdk_error_message(&e),
                    });
                    return Ok(());
                }
                None => break,
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

fn lifecycle_str(s: &LifeCycleState) -> String {
    s.as_str().to_string()
}

fn lifecycle_state(s: &LifeCycleState) -> ResourceState {
    match s {
        LifeCycleState::Available => ResourceState::Available,
        LifeCycleState::Creating | LifeCycleState::Updating => ResourceState::Pending,
        LifeCycleState::Deleting => ResourceState::Deleting,
        LifeCycleState::Deleted => ResourceState::Terminated,
        LifeCycleState::Error => ResourceState::Unavailable,
        other => ResourceState::Unknown(other.as_str().to_string()),
    }
}

/// Reduce an `arn:aws:s3:::bucket` bucket ARN to the bare bucket name.
fn bucket_name_from_arn(arn: &str) -> String {
    arn.rsplit(':').next().unwrap_or(arn).to_string()
}

// ── File system ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct S3FileSystem {
    pub id: String,
    pub arn: String,
    pub name: String,
    /// `arn:aws:s3:::bucket` of the linked bucket.
    pub bucket_arn: String,
    pub bucket_name: String,
    pub status: String,
    pub status_message: String,
    pub role_arn: String,
    pub owner_id: String,
    pub created: Option<String>,
    // Tags aren't in the list output — the Tags section fetches them via
    // GetFileSystem (`lazy.s3files_details`), so this stays empty (the
    // app-wide `tag:` filter won't see S3 Files tags; documented).
    pub tags: HashMap<String, String>,
    state: ResourceState,
    search_blob: String,
}

impl S3FileSystem {
    pub fn from_sdk(f: &aws_sdk_s3files::types::ListFileSystemsDescription) -> Self {
        let id = f.file_system_id().to_string();
        let name = match f.name() {
            Some(n) if !n.is_empty() => n.to_string(),
            _ => id.clone(),
        };
        let bucket_arn = f.bucket().to_string();
        let bucket_name = bucket_name_from_arn(&bucket_arn);
        let search_blob = format!("{} {} {} {}", name, id, bucket_name, f.status().as_str());
        Self {
            name,
            arn: f.file_system_arn().to_string(),
            bucket_name,
            bucket_arn,
            status: lifecycle_str(f.status()),
            status_message: f.status_message().unwrap_or_default().to_string(),
            role_arn: f.role_arn().to_string(),
            owner_id: f.owner_id().to_string(),
            created: Some(fmt_epoch_secs(f.creation_time().secs())),
            tags: HashMap::new(),
            state: lifecycle_state(f.status()),
            search_blob,
            id,
        }
    }
}

crate::sections! {
    pub enum S3FsDetailSection,
    pub static S3FS_SECTIONS = [
        Details "Details" => crate::app::App::trigger_s3files_details_load,
        MountTargets "Mount Targets" => crate::app::App::trigger_s3files_mount_targets_load,
        AccessPoints "Access Points" => crate::app::App::trigger_s3files_access_points_load,
        Sync "Sync" => crate::app::App::trigger_s3files_sync_load,
        Policy "Policy" => crate::app::App::trigger_s3files_policy_load,
        Tags "Tags" => crate::app::App::trigger_s3files_details_load,
    ]
}

impl Resource for S3FileSystem {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&S3FS_SECTIONS)
    }

    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn resource_type(&self) -> &str {
        "S3 File System"
    }

    fn state(&self) -> ResourceState {
        self.state.clone()
    }

    fn state_label(&self) -> String {
        native_state_label(&self.status, || self.state())
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        self.search_blob.clone()
    }

    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws s3files get-file-system --file-system-id {}",
            shell_quote(&self.id)
        ))
    }

    fn details(&self) -> Vec<(String, String)> {
        let mut d = vec![
            ("Name".to_string(), self.name.clone()),
            ("ID".to_string(), self.id.clone()),
            ("Bucket".to_string(), self.bucket_name.clone()),
            ("Status".to_string(), self.status.clone()),
        ];
        if let Some(c) = &self.created {
            d.push(("Created".to_string(), c.clone()));
        }
        d.push(("ARN".to_string(), self.arn.clone()));
        d
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── Lazy: GetFileSystem enrichment (prefix, KMS, tags) ───────────────────────

#[derive(Debug, Clone)]
pub struct S3FsExtras {
    /// The bucket prefix the file system exposes ("" = whole bucket).
    pub prefix: String,
    pub kms_key_id: String,
    pub tags: Vec<(String, String)>,
}

pub async fn fetch_s3files_details(client: S3FilesClient, id: String) -> Result<S3FsExtras> {
    let resp = client
        .get_file_system()
        .file_system_id(&id)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
    let mut tags: Vec<(String, String)> = resp
        .tags()
        .iter()
        .map(|t| (t.key().to_string(), t.value().to_string()))
        .collect();
    tags.sort();
    Ok(S3FsExtras {
        prefix: resp.prefix().to_string(),
        kms_key_id: resp.kms_key_id().unwrap_or_default().to_string(),
        tags,
    })
}

// ── Lazy: mount targets ──────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct S3FsMountTarget {
    pub id: String,
    pub status: String,
    pub status_message: String,
    pub subnet_id: String,
    pub vpc_id: String,
    pub az_id: String,
    pub ipv4: String,
    pub ipv6: String,
    pub eni_id: String,
}

pub async fn fetch_s3files_mount_targets(
    client: S3FilesClient,
    fs_id: String,
) -> Result<Vec<S3FsMountTarget>> {
    let mut out = Vec::new();
    let mut pager = client
        .list_mount_targets()
        .file_system_id(&fs_id)
        .into_paginator()
        .send();
    loop {
        match pager.next().await {
            Some(Ok(page)) => {
                for m in page.mount_targets() {
                    out.push(S3FsMountTarget {
                        id: m.mount_target_id().to_string(),
                        status: m.status().map(lifecycle_str).unwrap_or_default(),
                        status_message: m.status_message().unwrap_or_default().to_string(),
                        subnet_id: m.subnet_id().to_string(),
                        vpc_id: m.vpc_id().unwrap_or_default().to_string(),
                        az_id: m.availability_zone_id().unwrap_or_default().to_string(),
                        ipv4: m.ipv4_address().unwrap_or_default().to_string(),
                        ipv6: m.ipv6_address().unwrap_or_default().to_string(),
                        eni_id: m.network_interface_id().unwrap_or_default().to_string(),
                    });
                }
                if out.len() >= MAX_MOUNT_TARGETS {
                    break;
                }
            }
            Some(Err(e)) => {
                return Err(crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))
            }
            None => break,
        }
    }
    Ok(out)
}

// ── Lazy: access points ──────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct S3FsAccessPoint {
    pub id: String,
    pub name: String,
    pub status: String,
    /// "uid:gid" (+ secondary gids) enforced on requests, "" when none.
    pub posix_user: String,
    pub root_path: String,
    pub capped: bool,
}

pub async fn fetch_s3files_access_points(
    client: S3FilesClient,
    fs_id: String,
) -> Result<Vec<S3FsAccessPoint>> {
    let mut out = Vec::new();
    let mut capped = false;
    let mut pager = client
        .list_access_points()
        .file_system_id(&fs_id)
        .into_paginator()
        .send();
    'outer: loop {
        match pager.next().await {
            Some(Ok(page)) => {
                for a in page.access_points() {
                    let posix_user = a
                        .posix_user()
                        .map(|p| {
                            let mut s = format!("{}:{}", p.uid(), p.gid());
                            if !p.secondary_gids().is_empty() {
                                let gids: Vec<String> =
                                    p.secondary_gids().iter().map(|g| g.to_string()).collect();
                                s = format!("{} (+{})", s, gids.join(","));
                            }
                            s
                        })
                        .unwrap_or_default();
                    out.push(S3FsAccessPoint {
                        id: a.access_point_id().to_string(),
                        name: a.name().unwrap_or_default().to_string(),
                        status: lifecycle_str(a.status()),
                        posix_user,
                        root_path: a
                            .root_directory()
                            .and_then(|r| r.path())
                            .unwrap_or_default()
                            .to_string(),
                        capped: false,
                    });
                    // A file system can hold up to 25k access points — cap hard.
                    if out.len() >= MAX_ACCESS_POINTS {
                        capped = true;
                        break 'outer;
                    }
                }
            }
            Some(Err(e)) => {
                return Err(crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))
            }
            None => break,
        }
    }
    if capped {
        if let Some(last) = out.last_mut() {
            last.capped = true;
        }
    }
    Ok(out)
}

// ── Lazy: file system policy ─────────────────────────────────────────────────

/// `None` = no policy attached (the API 404s — an expected state, not an
/// error).
pub async fn fetch_s3files_policy(client: S3FilesClient, fs_id: String) -> Result<Option<String>> {
    match client.get_file_system_policy().file_system_id(&fs_id).send().await {
        Ok(resp) => Ok(Some(resp.policy().to_string())),
        Err(e) => {
            if e.as_service_error().is_some_and(|se| se.is_resource_not_found_exception()) {
                Ok(None)
            } else {
                Err(crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))
            }
        }
    }
}

// ── Lazy: synchronization configuration ──────────────────────────────────────

#[derive(Debug, Clone)]
pub struct S3FsSyncConfig {
    pub version: Option<i32>,
    /// (prefix, trigger, size_less_than_bytes)
    pub import_rules: Vec<(String, String, i64)>,
    /// days_after_last_access per rule
    pub expiration_rules: Vec<i32>,
}

pub async fn fetch_s3files_sync(client: S3FilesClient, fs_id: String) -> Result<S3FsSyncConfig> {
    let resp = client
        .get_synchronization_configuration()
        .file_system_id(&fs_id)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
    Ok(S3FsSyncConfig {
        version: resp.latest_version_number(),
        import_rules: resp
            .import_data_rules()
            .iter()
            .map(|r| {
                (
                    r.prefix().to_string(),
                    r.trigger().as_str().to_string(),
                    r.size_less_than(),
                )
            })
            .collect(),
        expiration_rules: resp
            .expiration_data_rules()
            .iter()
            .map(|r| r.days_after_last_access())
            .collect(),
    })
}

/// One `ListFileSystems` sweep filtered to the file systems linked to a
/// bucket — feeds the S3 bucket pane's File Systems section. The `bucket`
/// field is the bucket ARN, so match on the reduced name.
pub async fn fetch_s3files_for_bucket(
    client: S3FilesClient,
    bucket: String,
) -> Result<Vec<S3FileSystem>> {
    let mut out = Vec::new();
    let mut pager = client.list_file_systems().into_paginator().send();
    loop {
        match pager.next().await {
            Some(Ok(page)) => {
                for f in page.file_systems() {
                    if bucket_name_from_arn(f.bucket()) == bucket {
                        out.push(S3FileSystem::from_sdk(f));
                    }
                }
            }
            Some(Err(e)) => {
                return Err(crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))
            }
            None => break,
        }
    }
    Ok(out)
}

fn fmt_epoch_secs(secs: i64) -> String {
    let days = secs / 86400;
    let time_rem = secs % 86400;
    let h = time_rem / 3600;
    let m = (time_rem % 3600) / 60;
    let (y, mo, d) = epoch_days_to_ymd(days);
    format!("{:04}-{:02}-{:02} {:02}:{:02} UTC", y, mo, d, h, m)
}

fn epoch_days_to_ymd(days: i64) -> (i64, u32, u32) {
    let mut y = 1970i64;
    let mut rem = days;
    loop {
        let len = if is_leap(y) { 366 } else { 365 };
        if rem < len {
            break;
        }
        rem -= len;
        y += 1;
    }
    let month_lens = [
        31,
        if is_leap(y) { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut mo = 1u32;
    for len in month_lens {
        if rem < len {
            break;
        }
        rem -= len;
        mo += 1;
    }
    (y, mo, rem as u32 + 1)
}

fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}
