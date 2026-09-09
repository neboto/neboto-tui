use crate::aws::client::AwsClients;
use crate::aws::pagination::next_page_token;
use crate::aws::resource::{Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::error::Result;
use crate::event::{Event, LoadProgress};
use async_trait::async_trait;
use aws_sdk_ecr::Client as EcrClient;
use std::any::Any;
use std::collections::HashMap;
use tokio::sync::mpsc;

pub struct EcrService {
    client: EcrClient,
}

impl EcrService {
    pub fn new(aws_clients: &AwsClients) -> Self {
        Self {
            client: aws_clients.ecr_client(),
        }
    }
}

#[async_trait]
impl AwsService for EcrService {
    fn service_type(&self) -> ServiceType {
        ServiceType::Ecr
    }

    fn name(&self) -> &str {
        "ECR"
    }

    async fn list_resources(&self) -> Result<Vec<Box<dyn Resource>>> {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        self.list_resources_streaming(tx, ServiceType::Ecr).await?;
        Ok(vec![])
    }

    async fn list_resources_streaming(
        &self,
        event_tx: mpsc::UnboundedSender<Event>,
        service_type: ServiceType,
    ) -> Result<()> {
        let mut total = 0usize;

        let repos = self.fetch_repositories().await;

        match repos {
            Ok(repos) if !repos.is_empty() => {
                let batch: Vec<Box<dyn Resource>> = repos
                    .into_iter()
                    .map(|r| Box::new(r) as Box<dyn Resource>)
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
                    error: format!("Failed to list repositories: {}", e),
                });
                return Ok(());
            }
            _ => {}
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

impl EcrService {
    async fn fetch_repositories(&self) -> Result<Vec<EcrRepository>> {
        let mut repos = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut req = self.client.describe_repositories().max_results(100);
            if let Some(t) = &token {
                req = req.next_token(t);
            }
            let page = req
                .send()
                .await
                .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
            for r in page.repositories() {
                repos.push(EcrRepository::from_sdk(r));
            }
            token = next_page_token(page.next_token(), &token);
            if token.is_none() {
                break;
            }
        }

        // Fetch tags concurrently
        let futs: Vec<_> = repos
            .iter()
            .map(|r| {
                let client = self.client.clone();
                let arn = r.arn.clone();
                async move {
                    client
                        .list_tags_for_resource()
                        .resource_arn(&arn)
                        .send()
                        .await
                        .ok()
                        .map(|resp| {
                            resp.tags()
                                .iter()
                                .map(|t| {
                                    (
                                        t.key().to_string(),
                                        t.value().to_string(),
                                    )
                                })
                                .collect::<HashMap<String, String>>()
                        })
                        .unwrap_or_default()
                }
            })
            .collect();
        let tags_list = futures::future::join_all(futs).await;
        for (repo, tags) in repos.iter_mut().zip(tags_list) {
            repo.tags = tags;
        }

        Ok(repos)
    }
}

// ── Lazy fetches for the split pane ──────────────────────────────────────────

pub async fn fetch_ecr_repo_images(
    client: EcrClient,
    repo_name: String,
    repo_uri: String,
) -> Result<Vec<EcrImage>> {
    let mut images = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut req = client
            .describe_images()
            .repository_name(&repo_name)
            .max_results(100);
        if let Some(t) = &token {
            req = req.next_token(t);
        }
        let page = req
            .send()
            .await
            .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
        for detail in page.image_details() {
            images.push(EcrImage::from_sdk(detail, &repo_name, &repo_uri));
        }
        token = next_page_token(page.next_token(), &token);
        if token.is_none() || images.len() >= 100 {
            break;
        }
    }
    images.sort_by(|a, b| b.pushed_at.cmp(&a.pushed_at));
    Ok(images)
}

pub async fn fetch_ecr_lifecycle(client: EcrClient, repo_name: String) -> Result<String> {
    match client
        .get_lifecycle_policy()
        .repository_name(&repo_name)
        .send()
        .await
    {
        Ok(resp) => Ok(resp.lifecycle_policy_text().unwrap_or_default().to_string()),
        // No lifecycle policy on the repo is a normal "empty" state, not an error.
        Err(e)
            if e.as_service_error()
                .map(|se| se.is_lifecycle_policy_not_found_exception())
                .unwrap_or(false) =>
        {
            Ok(String::new())
        }
        Err(e) => Err(crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e))),
    }
}

// ── EcrRepository ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct EcrRepository {
    pub name: String,
    pub arn: String,
    pub registry_id: String,
    pub uri: String,
    pub created: Option<String>,
    pub image_tag_mutability: String,
    pub scan_on_push: bool,
    pub encryption_type: String,
    pub kms_key: Option<String>,
    pub tags: HashMap<String, String>,
}

impl EcrRepository {
    fn from_sdk(r: &aws_sdk_ecr::types::Repository) -> Self {
        Self {
            name: r.repository_name().unwrap_or_default().to_string(),
            arn: r.repository_arn().unwrap_or_default().to_string(),
            registry_id: r.registry_id().unwrap_or_default().to_string(),
            uri: r.repository_uri().unwrap_or_default().to_string(),
            created: r.created_at().map(|d| fmt_epoch(d.secs())),
            image_tag_mutability: r
                .image_tag_mutability()
                .map(|m| m.as_str().to_string())
                .unwrap_or_default(),
            scan_on_push: r
                .image_scanning_configuration()
                .map(|s| s.scan_on_push())
                .unwrap_or(false),
            encryption_type: r
                .encryption_configuration()
                .map(|e| e.encryption_type().as_str().to_string())
                .unwrap_or_else(|| "AES256".to_string()),
            kms_key: r
                .encryption_configuration()
                .and_then(|e| e.kms_key())
                .map(|s| s.to_string()),
            tags: HashMap::new(),
        }
    }
}

crate::sections! {
    pub enum EcrRepoDetailSection,
    pub static ECR_REPO_SECTIONS = [
        Details "Details",
        Images "Images" => crate::app::App::trigger_ecr_images_load,
        LifecyclePolicy "Lifecycle" => crate::app::App::trigger_ecr_lifecycle_load,
        Tags "Tags",
    ]
}

impl Resource for EcrRepository {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&ECR_REPO_SECTIONS)
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws ecr describe-images --repository-name {}",
            crate::aws::resource::shell_quote(self.id())
        ))
    }

    fn id(&self) -> &str {
        &self.name
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn resource_type(&self) -> &str {
        "ECR Repository"
    }
    fn state(&self) -> ResourceState {
        ResourceState::stateless()
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        format!(
            "{} {} {} {}",
            self.name,
            self.uri,
            self.encryption_type,
            self.tags.values().cloned().collect::<Vec<_>>().join(" "),
        )
    }
    fn details(&self) -> Vec<(String, String)> {
        let mut rows = vec![
            ("Repository".to_string(), self.name.clone()),
            ("URI".to_string(), self.uri.clone()),
            ("Tag Mutability".to_string(), self.image_tag_mutability.clone()),
            ("Scan on Push".to_string(), if self.scan_on_push { "✓" } else { "✗" }.to_string()),
            ("Encryption".to_string(), self.encryption_type.clone()),
        ];
        if let Some(k) = &self.kms_key {
            rows.push(("KMS Key".to_string(), k.clone()));
        }
        if let Some(c) = &self.created {
            rows.push(("Created".to_string(), c.clone()));
        }
        rows.push(("ARN".to_string(), self.arn.clone()));
        rows
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/ecr/repositories/private/{}/{}?region={}",
            region, self.registry_id, self.name, region,
        ))
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── EcrImage ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct EcrImage {
    pub repo_name: String,
    pub repo_uri: String,
    pub registry_id: String,
    pub digest: String,
    pub tags: Vec<String>,
    pub pushed_at: Option<String>,
    pub last_pulled_at: Option<String>,
    pub size_bytes: Option<i64>,
    pub artifact_media_type: Option<String>,
    pub manifest_media_type: Option<String>,
    pub scan_status: String,
    pub scan_status_description: Option<String>,
    pub finding_counts: HashMap<String, i32>,
}

impl EcrImage {
    fn from_sdk(d: &aws_sdk_ecr::types::ImageDetail, repo_name: &str, repo_uri: &str) -> Self {
        let finding_counts = d
            .image_scan_findings_summary()
            .and_then(|s| s.finding_severity_counts())
            .map(|counts| {
                counts
                    .iter()
                    .map(|(k, v)| (k.as_str().to_string(), *v))
                    .collect()
            })
            .unwrap_or_default();

        Self {
            repo_name: repo_name.to_string(),
            repo_uri: repo_uri.to_string(),
            registry_id: d.registry_id().unwrap_or_default().to_string(),
            digest: d.image_digest().unwrap_or_default().to_string(),
            tags: d.image_tags().to_vec(),
            pushed_at: d.image_pushed_at().map(|t| fmt_epoch(t.secs())),
            last_pulled_at: d.last_recorded_pull_time().map(|t| fmt_epoch(t.secs())),
            size_bytes: d.image_size_in_bytes(),
            artifact_media_type: d.artifact_media_type().map(|s| s.to_string()),
            manifest_media_type: d.image_manifest_media_type().map(|s| s.to_string()),
            scan_status: d
                .image_scan_status()
                .and_then(|s| s.status())
                .map(|s| s.as_str().to_string())
                .unwrap_or_default(),
            scan_status_description: d
                .image_scan_status()
                .and_then(|s| s.description())
                .map(|s| s.to_string()),
            finding_counts,
        }
    }

    /// The pull-by-digest reference shown (and copyable) in the console:
    /// `<repo-uri>@sha256:…`. Falls back to just the digest if the URI is empty.
    pub fn image_ref(&self) -> String {
        if self.repo_uri.is_empty() {
            self.digest.clone()
        } else {
            format!("{}@{}", self.repo_uri, self.digest)
        }
    }

    /// Full per-severity vulnerability breakdown, e.g.
    /// "3 Critical · 5 High · 2 Medium · 1 Low". Empty if no scan findings.
    pub fn vuln_breakdown(&self) -> String {
        let mut parts = Vec::new();
        for (sev, label) in &[
            ("CRITICAL", "Critical"),
            ("HIGH", "High"),
            ("MEDIUM", "Medium"),
            ("LOW", "Low"),
            ("INFORMATIONAL", "Informational"),
            ("UNDEFINED", "Undefined"),
        ] {
            if let Some(&c) = self.finding_counts.get(*sev) {
                if c > 0 {
                    parts.push(format!("{} {}", c, label));
                }
            }
        }
        parts.join(" · ")
    }

    fn short_digest(&self) -> &str {
        if self.digest.len() > 19 {
            &self.digest[7..19] // skip "sha256:" show 12 hex chars
        } else {
            &self.digest
        }
    }

    pub fn size_display(&self) -> String {
        match self.size_bytes {
            Some(b) if b >= 1_073_741_824 => format!("{:.1} GB", b as f64 / 1_073_741_824.0),
            Some(b) if b >= 1_048_576 => format!("{:.1} MB", b as f64 / 1_048_576.0),
            Some(b) => format!("{} KB", b / 1024),
            None => "—".to_string(),
        }
    }

    pub fn vuln_summary(&self) -> String {
        if self.finding_counts.is_empty() {
            return String::new();
        }
        let mut parts = Vec::new();
        for sev in &["CRITICAL", "HIGH", "MEDIUM", "LOW"] {
            if let Some(&c) = self.finding_counts.get(*sev) {
                if c > 0 {
                    let short = &sev[..1]; // C, H, M, L
                    parts.push(format!("{}{}", c, short));
                }
            }
        }
        parts.join(" ")
    }

    fn has_critical_or_high(&self) -> bool {
        self.finding_counts.get("CRITICAL").copied().unwrap_or(0) > 0
            || self.finding_counts.get("HIGH").copied().unwrap_or(0) > 0
    }

    fn has_medium(&self) -> bool {
        self.finding_counts.get("MEDIUM").copied().unwrap_or(0) > 0
    }
}

impl Resource for EcrImage {
    fn id(&self) -> &str {
        &self.digest
    }
    fn name(&self) -> &str {
        if let Some(first) = self.tags.first() {
            first
        } else {
            "<untagged>"
        }
    }
    fn resource_type(&self) -> &str {
        "ECR Image"
    }
    fn state(&self) -> ResourceState {
        if self.has_critical_or_high() {
            ResourceState::Unavailable
        } else if self.has_medium() {
            ResourceState::Pending
        } else if self.scan_status == "COMPLETE" {
            ResourceState::Available
        } else {
            ResourceState::Unknown(String::new())
        }
    }
    fn state_label(&self) -> String {
        // Severity words, matching the repo pane's Vulnerabilities row.
        if self.finding_counts.get("CRITICAL").copied().unwrap_or(0) > 0 {
            "critical".to_string()
        } else if self.has_critical_or_high() {
            "high".to_string()
        } else if self.has_medium() {
            "medium".to_string()
        } else if self.scan_status == "COMPLETE" {
            "no findings".to_string()
        } else if self.scan_status.is_empty() {
            "not scanned".to_string()
        } else {
            self.scan_status.to_lowercase()
        }
    }
    fn tags(&self) -> &HashMap<String, String> {
        static EMPTY: std::sync::OnceLock<HashMap<String, String>> = std::sync::OnceLock::new();
        EMPTY.get_or_init(HashMap::new)
    }
    fn search_text(&self) -> String {
        format!(
            "{} {} {} {} {}",
            self.repo_name,
            self.tags.join(" "),
            self.short_digest(),
            self.scan_status,
            self.vuln_summary(),
        )
    }
    fn details(&self) -> Vec<(String, String)> {
        let mut rows = vec![
            ("Repository".to_string(), self.repo_name.clone()),
            ("Tags".to_string(), if self.tags.is_empty() { "<untagged>".to_string() } else { self.tags.join(", ") }),
            ("Digest".to_string(), self.digest.clone()),
            ("Size".to_string(), self.size_display()),
            ("Scan Status".to_string(), self.scan_status.clone()),
        ];
        if !self.finding_counts.is_empty() {
            rows.push(("Vulnerabilities".to_string(), self.vuln_summary()));
        }
        if let Some(p) = &self.pushed_at {
            rows.push(("Pushed".to_string(), p.clone()));
        }
        rows
    }
    fn console_url(&self, region: &str) -> Option<String> {
        let encoded_digest = self.digest.replace(':', "%3A");
        Some(format!(
            "https://{}.console.aws.amazon.com/ecr/repositories/private/{}/{}/_/image/{}/details?region={}",
            region, self.registry_id, self.repo_name, encoded_digest, region,
        ))
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

fn fmt_epoch(secs: i64) -> String {
    let days = secs / 86400;
    let rem = secs % 86400;
    let h = rem / 3600;
    let m = (rem % 3600) / 60;
    let (y, mo, d) = epoch_days_to_ymd(days);
    format!("{:04}-{:02}-{:02} {:02}:{:02} UTC", y, mo, d, h, m)
}

fn epoch_days_to_ymd(mut days: i64) -> (i32, u8, u8) {
    let mut year = 1970i32;
    loop {
        let dy = if is_leap(year) { 366 } else { 365 };
        if days < dy { break; }
        days -= dy;
        year += 1;
    }
    let dm = [31u8, if is_leap(year) { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut month = 1u8;
    for &d in &dm {
        if days < d as i64 { break; }
        days -= d as i64;
        month += 1;
    }
    (year, month, (days + 1) as u8)
}

fn is_leap(y: i32) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

// ── CloudWatch metrics (`m` overlay) — AWS/ECR, dimension RepositoryName ──────

pub use crate::aws::services::ec2::{parse_metric_datapoints, MetricsTimeRange};

#[derive(Debug, Clone)]
pub struct EcrMetricsData {
    pub time_range: MetricsTimeRange,
    pub pull_count: Vec<(f64, f64)>,
    pub x_max: f64,
}

#[derive(Debug, Clone)]
pub enum EcrMetricsState {
    Loading,
    Loaded(EcrMetricsData),
}

/// Pull `AWS/ECR` metrics for one repository. `RepositoryPullCount` is the
/// namespace's only metric — one chart, but it answers "is anything still
/// pulling this image?" before an archive/cleanup.
pub async fn fetch_ecr_metrics(
    cw: aws_sdk_cloudwatch::Client,
    repo_name: String,
    time_range: MetricsTimeRange,
) -> crate::error::Result<EcrMetricsData> {
    use aws_sdk_cloudwatch::types::{Dimension, Statistic};

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let start = now - time_range.duration_secs();

    let resp = cw
        .get_metric_statistics()
        .namespace("AWS/ECR")
        .metric_name("RepositoryPullCount")
        .dimensions(
            Dimension::builder()
                .name("RepositoryName")
                .value(&repo_name)
                .build(),
        )
        .start_time(aws_sdk_cloudwatch::primitives::DateTime::from_secs(start))
        .end_time(aws_sdk_cloudwatch::primitives::DateTime::from_secs(now))
        .period(time_range.period_secs())
        .set_statistics(Some(vec![Statistic::Sum]))
        .send()
        .await;

    Ok(EcrMetricsData {
        time_range,
        pull_count: parse_metric_datapoints(resp, start),
        x_max: time_range.duration_secs() as f64,
    })
}
