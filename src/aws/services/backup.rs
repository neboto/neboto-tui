use crate::aws::client::AwsClients;
use crate::aws::pagination::next_page_token;
use crate::aws::resource::{native_state_label, Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::error::Result;
use crate::event::{Event, LoadProgress};
use async_trait::async_trait;
use aws_sdk_backup::Client as BackupClient;
use std::any::Any;
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;

/// Jobs and recovery points are high-volume; cap them.
const JOB_WINDOW_DAYS: i64 = 7;
const MAX_JOBS: usize = 200;
const MAX_RECOVERY_POINTS: usize = 300;

/// AWS Backup — sub-tabs Vaults / Plans / Protected Resources / Jobs. Vaults and
/// Plans get split panes (recovery points / rules+selections lazy); Protected
/// Resources and Jobs are flat. Jobs are windowed to the last 7 days.
pub struct BackupService {
    client: BackupClient,
}

impl BackupService {
    pub fn new(aws_clients: &AwsClients) -> Self {
        Self {
            client: aws_clients.backup_client(),
        }
    }
}

#[async_trait]
impl AwsService for BackupService {
    fn service_type(&self) -> ServiceType {
        ServiceType::Backup
    }

    fn name(&self) -> &str {
        "AWS Backup"
    }

    async fn list_resources(&self) -> Result<Vec<Box<dyn Resource>>> {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        self.list_resources_streaming(tx, ServiceType::Backup).await?;
        Ok(vec![])
    }

    async fn list_resources_streaming(
        &self,
        event_tx: mpsc::UnboundedSender<Event>,
        service_type: ServiceType,
    ) -> Result<()> {
        let mut total = 0usize;

        // ── Vaults ───────────────────────────────────────────────────────────
        let mut vaults: Vec<BackupVault> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut req = self.client.list_backup_vaults();
            if let Some(t) = &token {
                req = req.next_token(t);
            }
            match req.send().await {
                Ok(page) => {
                    for v in page.backup_vault_list() {
                        vaults.push(BackupVault::from_sdk(v));
                    }
                    token = next_page_token(page.next_token(), &token);
                    if token.is_none() {
                        break;
                    }
                }
                Err(e) => {
                    let _ = event_tx.send(Event::ResourceLoadError {
                        service: service_type,
                        error: format!("Failed to list backup vaults: {}", e),
                    });
                    return Ok(());
                }
            }
        }
        // Tags per vault, concurrently.
        let vault_tags = futures::future::join_all(
            vaults.iter().map(|v| fetch_tags(&self.client, v.arn.clone())),
        )
        .await;
        for (v, tags) in vaults.iter_mut().zip(vault_tags) {
            v.tags = tags;
        }
        if !vaults.is_empty() {
            let batch: Vec<Box<dyn Resource>> = vaults
                .into_iter()
                .map(|v| Box::new(v) as Box<dyn Resource>)
                .collect();
            total += batch.len();
            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                service: service_type,
                resources: batch,
                progress: LoadProgress {
                    loaded_count: total,
                    total_count: None,
                    status_message: Some("Loading plans…".to_string()),
                },
            });
        }

        // ── Plans ────────────────────────────────────────────────────────────
        let mut plans: Vec<BackupPlan> = Vec::new();
        token = None;
        loop {
            let mut req = self.client.list_backup_plans();
            if let Some(t) = &token {
                req = req.next_token(t);
            }
            match req.send().await {
                Ok(page) => {
                    for p in page.backup_plans_list() {
                        plans.push(BackupPlan::from_sdk(p));
                    }
                    token = next_page_token(page.next_token(), &token);
                    if token.is_none() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        let plan_tags = futures::future::join_all(
            plans.iter().map(|p| fetch_tags(&self.client, p.arn.clone())),
        )
        .await;
        for (p, tags) in plans.iter_mut().zip(plan_tags) {
            p.tags = tags;
        }
        if !plans.is_empty() {
            let batch: Vec<Box<dyn Resource>> = plans
                .into_iter()
                .map(|p| Box::new(p) as Box<dyn Resource>)
                .collect();
            total += batch.len();
            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                service: service_type,
                resources: batch,
                progress: LoadProgress {
                    loaded_count: total,
                    total_count: None,
                    status_message: Some("Loading protected resources…".to_string()),
                },
            });
        }

        // ── Protected resources ──────────────────────────────────────────────
        let mut protected: Vec<Box<dyn Resource>> = Vec::new();
        token = None;
        loop {
            let mut req = self.client.list_protected_resources();
            if let Some(t) = &token {
                req = req.next_token(t);
            }
            match req.send().await {
                Ok(page) => {
                    for r in page.results() {
                        protected.push(Box::new(ProtectedResource::from_sdk(r)) as Box<dyn Resource>);
                    }
                    token = next_page_token(page.next_token(), &token);
                    if token.is_none() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        if !protected.is_empty() {
            total += protected.len();
            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                service: service_type,
                resources: protected,
                progress: LoadProgress {
                    loaded_count: total,
                    total_count: None,
                    status_message: Some("Loading jobs…".to_string()),
                },
            });
        }

        // ── Jobs (windowed to the last 7 days) ───────────────────────────────
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let since = aws_sdk_backup::primitives::DateTime::from_secs(now - JOB_WINDOW_DAYS * 86400);
        let mut jobs: Vec<Box<dyn Resource>> = Vec::new();
        token = None;
        loop {
            let mut req = self.client.list_backup_jobs().by_created_after(since).max_results(100);
            if let Some(t) = &token {
                req = req.next_token(t);
            }
            match req.send().await {
                Ok(page) => {
                    for j in page.backup_jobs() {
                        jobs.push(Box::new(BackupJob::from_sdk(j)) as Box<dyn Resource>);
                    }
                    token = next_page_token(page.next_token(), &token);
                    if token.is_none() || jobs.len() >= MAX_JOBS {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        if !jobs.is_empty() {
            total += jobs.len();
            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                service: service_type,
                resources: jobs,
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

async fn fetch_tags(client: &BackupClient, arn: String) -> HashMap<String, String> {
    if arn.is_empty() {
        return HashMap::new();
    }
    match client.list_tags().resource_arn(&arn).send().await {
        Ok(resp) => resp
            .tags()
            .map(|t| t.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default(),
        Err(_) => HashMap::new(),
    }
}

// ── Lazy: plan details (rules + selections) ──────────────────────────────────

#[derive(Debug, Clone)]
pub struct BackupRule {
    pub name: String,
    pub target_vault: String,
    pub schedule: String,
    pub lifecycle: String,
    pub start_window: String,
}

#[derive(Debug, Clone)]
pub struct BackupPlanDetails {
    pub rules: Vec<BackupRule>,
    pub selections: Vec<String>,
}

pub async fn fetch_backup_plan_details(
    client: BackupClient,
    plan_id: String,
) -> Result<BackupPlanDetails> {
    let rules = match client.get_backup_plan().backup_plan_id(&plan_id).send().await {
        Ok(resp) => resp
            .backup_plan()
            .map(|p| p.rules().iter().map(BackupRule::from_sdk).collect())
            .unwrap_or_default(),
        Err(e) => return Err(crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e))),
    };

    let mut selections: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut req = client.list_backup_selections().backup_plan_id(&plan_id);
        if let Some(t) = &token {
            req = req.next_token(t);
        }
        match req.send().await {
            Ok(page) => {
                for s in page.backup_selections_list() {
                    selections.push(s.selection_name().unwrap_or_default().to_string());
                }
                token = next_page_token(page.next_token(), &token);
                if token.is_none() {
                    break;
                }
            }
            Err(_) => break,
        }
    }

    Ok(BackupPlanDetails { rules, selections })
}

impl BackupRule {
    fn from_sdk(r: &aws_sdk_backup::types::BackupRule) -> Self {
        let lifecycle = r
            .lifecycle()
            .map(|lc| {
                let mut parts = Vec::new();
                if let Some(cold) = lc.move_to_cold_storage_after_days() {
                    parts.push(format!("cold@{}d", cold));
                }
                if let Some(del) = lc.delete_after_days() {
                    parts.push(format!("delete@{}d", del));
                }
                if parts.is_empty() {
                    "—".to_string()
                } else {
                    parts.join(", ")
                }
            })
            .unwrap_or_else(|| "—".to_string());
        Self {
            name: r.rule_name().to_string(),
            target_vault: r.target_backup_vault_name().to_string(),
            schedule: r.schedule_expression().unwrap_or("—").to_string(),
            lifecycle,
            start_window: r
                .start_window_minutes()
                .map(|m| format!("{} min", m))
                .unwrap_or_else(|| "—".to_string()),
        }
    }
}

// ── Lazy: recovery points per vault ──────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct RecoveryPoint {
    pub arn: String,
    pub resource_type: String,
    pub status: String,
    pub created: Option<String>,
    pub size: Option<i64>,
}

pub async fn fetch_recovery_points(
    client: BackupClient,
    vault_name: String,
) -> Result<Vec<RecoveryPoint>> {
    let mut points: Vec<RecoveryPoint> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut req = client
            .list_recovery_points_by_backup_vault()
            .backup_vault_name(&vault_name)
            .max_results(100);
        if let Some(t) = &token {
            req = req.next_token(t);
        }
        match req.send().await {
            Ok(page) => {
                for rp in page.recovery_points() {
                    points.push(RecoveryPoint {
                        arn: rp.recovery_point_arn().unwrap_or_default().to_string(),
                        resource_type: rp.resource_type().unwrap_or_default().to_string(),
                        status: rp
                            .status()
                            .map(|s| s.as_str().to_string())
                            .unwrap_or_default(),
                        created: rp.creation_date().map(|d| fmt_epoch_secs(d.secs())),
                        size: rp.backup_size_in_bytes(),
                    });
                }
                token = next_page_token(page.next_token(), &token);
                if token.is_none() || points.len() >= MAX_RECOVERY_POINTS {
                    break;
                }
            }
            Err(e) => return Err(crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e))),
        }
    }
    Ok(points)
}

// ── BackupVault ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct BackupVault {
    pub name: String,
    pub arn: String,
    pub recovery_points: i64,
    pub encryption_key_arn: String,
    pub locked: bool,
    pub created: Option<String>,
    pub tags: HashMap<String, String>,
}

impl BackupVault {
    fn from_sdk(v: &aws_sdk_backup::types::BackupVaultListMember) -> Self {
        Self {
            name: v.backup_vault_name().unwrap_or_default().to_string(),
            arn: v.backup_vault_arn().unwrap_or_default().to_string(),
            recovery_points: v.number_of_recovery_points(),
            encryption_key_arn: v.encryption_key_arn().unwrap_or_default().to_string(),
            locked: v.locked().unwrap_or(false),
            created: v.creation_date().map(|d| fmt_epoch_secs(d.secs())),
            tags: HashMap::new(),
        }
    }
}

crate::sections! {
    pub enum BackupVaultDetailSection,
    pub static BACKUP_VAULT_SECTIONS = [
        Details "Details",
        RecoveryPoints "Recovery Points" => crate::app::App::trigger_backup_recovery_points_load,
        Tags "Tags",
    ]
}

impl Resource for BackupVault {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&BACKUP_VAULT_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.arn
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn resource_type(&self) -> &str {
        "Backup Vault"
    }
    fn state(&self) -> ResourceState {
        ResourceState::stateless()
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        format!("{} {}", self.name, self.arn)
    }
    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Vault".to_string(), self.name.clone()),
            ("Recovery Points".to_string(), self.recovery_points.to_string()),
            ("Locked".to_string(), self.locked.to_string()),
        ]
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/backup/home?region={}#/backupvaults/details/{}",
            region, region, self.name
        ))
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── BackupPlan ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct BackupPlan {
    pub id: String,
    pub name: String,
    pub arn: String,
    pub last_execution: Option<String>,
    pub tags: HashMap<String, String>,
}

impl BackupPlan {
    fn from_sdk(p: &aws_sdk_backup::types::BackupPlansListMember) -> Self {
        Self {
            id: p.backup_plan_id().unwrap_or_default().to_string(),
            name: p.backup_plan_name().unwrap_or_default().to_string(),
            arn: p.backup_plan_arn().unwrap_or_default().to_string(),
            last_execution: p.last_execution_date().map(|d| fmt_epoch_secs(d.secs())),
            tags: HashMap::new(),
        }
    }
}

crate::sections! {
    pub enum BackupPlanDetailSection,
    pub static BACKUP_PLAN_SECTIONS = [
        Rules "Rules" => crate::app::App::trigger_backup_plan_details_load,
        Selections "Selections" => crate::app::App::trigger_backup_plan_details_load,
        Tags "Tags",
    ]
}

impl Resource for BackupPlan {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&BACKUP_PLAN_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.id
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn resource_type(&self) -> &str {
        "Backup Plan"
    }
    fn state(&self) -> ResourceState {
        ResourceState::stateless()
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        format!("{} {} {}", self.name, self.id, self.arn)
    }
    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Plan".to_string(), self.name.clone()),
            ("ID".to_string(), self.id.clone()),
            (
                "Last Run".to_string(),
                self.last_execution.clone().unwrap_or_else(|| "—".to_string()),
            ),
        ]
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/backup/home?region={}#/backupplans/details/{}",
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

// ── ProtectedResource ────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ProtectedResource {
    pub arn: String,
    pub resource_type: String,
    pub last_backup: Option<String>,
    pub tags: HashMap<String, String>,
}

impl ProtectedResource {
    fn from_sdk(r: &aws_sdk_backup::types::ProtectedResource) -> Self {
        Self {
            arn: r.resource_arn().unwrap_or_default().to_string(),
            resource_type: r.resource_type().unwrap_or_default().to_string(),
            last_backup: r.last_backup_time().map(|d| fmt_epoch_secs(d.secs())),
            tags: HashMap::new(),
        }
    }
}

impl Resource for ProtectedResource {
    fn id(&self) -> &str {
        &self.arn
    }
    fn name(&self) -> &str {
        // Last path/colon segment is the most readable label.
        self.arn
            .rsplit(['/', ':'])
            .next()
            .filter(|s| !s.is_empty())
            .unwrap_or(&self.arn)
    }
    fn resource_type(&self) -> &str {
        "Protected Resource"
    }
    fn state(&self) -> ResourceState {
        ResourceState::stateless()
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        format!("{} {}", self.arn, self.resource_type)
    }
    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Resource".to_string(), self.arn.clone()),
            ("Type".to_string(), self.resource_type.clone()),
            (
                "Last Backup".to_string(),
                self.last_backup.clone().unwrap_or_else(|| "never".to_string()),
            ),
        ]
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── BackupJob ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct BackupJob {
    pub id: String,
    pub resource_arn: String,
    pub resource_type: String,
    pub state: String,
    pub created: Option<String>,
    pub completed: Option<String>,
    pub vault: String,
    pub bytes: Option<i64>,
    pub tags: HashMap<String, String>,
}

impl BackupJob {
    fn from_sdk(j: &aws_sdk_backup::types::BackupJob) -> Self {
        Self {
            id: j.backup_job_id().unwrap_or_default().to_string(),
            resource_arn: j.resource_arn().unwrap_or_default().to_string(),
            resource_type: j.resource_type().unwrap_or_default().to_string(),
            state: j.state().map(|s| s.as_str().to_string()).unwrap_or_default(),
            created: j.creation_date().map(|d| fmt_epoch_secs(d.secs())),
            completed: j.completion_date().map(|d| fmt_epoch_secs(d.secs())),
            vault: j.backup_vault_name().unwrap_or_default().to_string(),
            bytes: j.backup_size_in_bytes(),
            tags: HashMap::new(),
        }
    }
}

impl Resource for BackupJob {
    fn id(&self) -> &str {
        &self.id
    }
    fn name(&self) -> &str {
        let res = self
            .resource_arn
            .rsplit(['/', ':'])
            .next()
            .filter(|s| !s.is_empty())
            .unwrap_or(&self.resource_arn);
        // Pair the resource with the job state for a readable list row.
        res
    }
    fn resource_type(&self) -> &str {
        "Backup Job"
    }
    fn state(&self) -> ResourceState {
        match self.state.as_str() {
            "COMPLETED" => ResourceState::Available,
            "FAILED" | "ABORTED" | "EXPIRED" => ResourceState::Unavailable,
            "RUNNING" | "CREATED" | "PENDING" => ResourceState::Pending,
            "ABORTING" => ResourceState::Deleting,
            other => ResourceState::Unknown(other.to_string()),
        }
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
            self.resource_arn, self.resource_type, self.state, self.vault
        )
    }
    fn details(&self) -> Vec<(String, String)> {
        let mut rows = vec![
            ("Resource".to_string(), self.resource_arn.clone()),
            ("Type".to_string(), self.resource_type.clone()),
            ("State".to_string(), self.state.clone()),
            ("Vault".to_string(), self.vault.clone()),
        ];
        if let Some(c) = &self.created {
            rows.push(("Created".to_string(), c.clone()));
        }
        if let Some(c) = &self.completed {
            rows.push(("Completed".to_string(), c.clone()));
        }
        if let Some(b) = self.bytes {
            rows.push(("Size".to_string(), fmt_bytes(b)));
        }
        rows
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

pub fn fmt_bytes(bytes: i64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut b = bytes as f64;
    let mut i = 0;
    while b >= 1024.0 && i < UNITS.len() - 1 {
        b /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{} {}", bytes, UNITS[0])
    } else {
        format!("{:.1} {}", b, UNITS[i])
    }
}

fn fmt_epoch_secs(secs: i64) -> String {
    let days = secs / 86400;
    let time_rem = secs % 86400;
    let h = time_rem / 3600;
    let m = (time_rem % 3600) / 60;
    let (y, mo, d) = epoch_days_to_ymd(days);
    format!("{:04}-{:02}-{:02} {:02}:{:02} UTC", y, mo, d, h, m)
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
    for &d in &dm {
        if days < d as i64 {
            break;
        }
        days -= d as i64;
        month += 1;
    }
    (year, month, (days + 1) as u8)
}

fn is_leap(y: i32) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}
