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

/// Repos whose images are listed concurrently during the list load. ECR's
/// DescribeImages rate limit is per account, so stay well under it.
const IMAGE_LIST_CONCURRENCY: usize = 6;

/// Image rows kept per repo (the newest by push time).
pub const MAX_IMAGES_PER_REPO: usize = 100;

/// Images read per repo before giving up on the walk. DescribeImages has no
/// server-side sort, so finding the newest means reading them all; this is
/// the bound on that (10 pages of 1000).
const MAX_IMAGES_SCANNED_PER_REPO: usize = 10_000;

/// Findings kept per image for the Findings section — the worst ones, since
/// the list is sorted by severity before the cut.
const MAX_IMAGE_FINDINGS: usize = 300;

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
        use futures::stream::StreamExt;
        let mut total = 0usize;

        // Phase 1: repositories (+ tags). A failure here is fatal — nothing
        // else can stream without the repo list.
        let repos = match self.fetch_repositories().await {
            Ok(repos) => repos,
            Err(e) => {
                let _ = event_tx.send(Event::ResourceLoadError {
                    service: service_type,
                    error: format!("Failed to list repositories: {}", e),
                });
                return Ok(());
            }
        };
        let targets: Vec<(String, String)> =
            repos.iter().map(|r| (r.name.clone(), r.uri.clone())).collect();
        if !repos.is_empty() {
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
                    status_message: Some("Loaded repositories. Loading images...".to_string()),
                },
            });
        }

        // Phase 2: every repo's images, so they are list rows of their own
        // (the Images sub-tab). One DescribeImages walk per repo, cut to the
        // newest `MAX_IMAGES_PER_REPO` by `fetch_ecr_repo_images` (a cut repo
        // is named in a load warning). A repo that fails is a warning,
        // never a load error — the repos and other images already streamed.
        let client = self.client.clone();
        let mut stream = futures::stream::iter(targets.into_iter().map(|(name, uri)| {
            let client = client.clone();
            async move {
                let res = fetch_ecr_repo_images(client, name.clone(), uri).await;
                (name, res)
            }
        }))
        .buffer_unordered(IMAGE_LIST_CONCURRENCY);
        let mut failed = 0usize;
        let mut first_err: Option<String> = None;
        let mut cut: Vec<String> = Vec::new();
        while let Some((name, res)) = stream.next().await {
            match res {
                Ok(page) => {
                    if page.truncated() {
                        cut.push(name);
                    }
                    if page.images.is_empty() {
                        continue;
                    }
                    total += page.images.len();
                    let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                        service: service_type,
                        resources: page
                            .images
                            .into_iter()
                            .map(|i| Box::new(i) as Box<dyn Resource>)
                            .collect(),
                        progress: LoadProgress {
                            loaded_count: total,
                            total_count: None,
                            status_message: None,
                        },
                    });
                }
                Err(e) => {
                    failed += 1;
                    if first_err.is_none() {
                        first_err = Some(e.to_string());
                    }
                }
            }
        }
        if failed > 0 {
            let _ = event_tx.send(Event::ResourceLoadWarning {
                service: service_type,
                warning: format!(
                    "images unavailable for {} repositor{}: {}",
                    failed,
                    if failed == 1 { "y" } else { "ies" },
                    first_err.unwrap_or_default()
                ),
            });
        }
        if !cut.is_empty() {
            let _ = event_tx.send(Event::ResourceLoadWarning {
                service: service_type,
                warning: truncation_warning(&cut),
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

/// One repo's images as list rows: the newest `MAX_IMAGES_PER_REPO` by push
/// time, plus how many the walk saw. `DescribeImages` returns images in no
/// particular order, so the walk reads every page (up to
/// `MAX_IMAGES_SCANNED_PER_REPO`) *before* sorting and cutting — stopping at
/// the first page would keep an arbitrary 100, not the newest.
pub struct EcrRepoImages {
    pub images: Vec<EcrImage>,
    /// Images the walk read (before the cut).
    pub seen: usize,
    /// False when the scan ceiling stopped the walk with pages left, so the
    /// kept rows are the newest *of those read*, not of the whole repo.
    pub complete: bool,
}

impl EcrRepoImages {
    pub fn truncated(&self) -> bool {
        !self.complete || self.seen > self.images.len()
    }
}

pub async fn fetch_ecr_repo_images(
    client: EcrClient,
    repo_name: String,
    repo_uri: String,
) -> Result<EcrRepoImages> {
    let mut images = Vec::new();
    let mut token: Option<String> = None;
    let complete = loop {
        let mut req = client
            .describe_images()
            .repository_name(&repo_name)
            .max_results(1000);
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
        if token.is_none() {
            break true;
        }
        if images.len() >= MAX_IMAGES_SCANNED_PER_REPO {
            break false;
        }
    };
    Ok(newest_images(images, complete))
}

/// Sort newest push first, then keep `MAX_IMAGES_PER_REPO`. Every kept row
/// carries the repo's totals so the repo pane can say what was cut.
fn newest_images(mut images: Vec<EcrImage>, complete: bool) -> EcrRepoImages {
    images.sort_by_key(|i| std::cmp::Reverse(i.pushed_secs));
    let seen = images.len();
    images.truncate(MAX_IMAGES_PER_REPO);
    for img in &mut images {
        img.repo_images_seen = seen;
        img.repo_images_complete = complete;
    }
    EcrRepoImages { images, seen, complete }
}

/// The load warning for repos whose image list was cut to the newest
/// `MAX_IMAGES_PER_REPO`. Names up to three repos.
fn truncation_warning(repos: &[String]) -> String {
    let mut names: Vec<&str> = repos.iter().map(String::as_str).collect();
    names.sort_unstable();
    let shown = names.iter().take(3).copied().collect::<Vec<_>>().join(", ");
    let more = names.len().saturating_sub(3);
    format!(
        "images: showing the newest {} per repository for {}{}",
        MAX_IMAGES_PER_REPO,
        shown,
        if more > 0 { format!(" (+{} more)", more) } else { String::new() }
    )
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

/// One vulnerability from `DescribeImageScanFindings`, normalized across the
/// two scanners: basic scanning returns `findings` (CVE name + attributes),
/// enhanced (Inspector) scanning returns `enhancedFindings` (package details,
/// score, remediation). An image only ever has one or the other.
#[derive(Debug, Clone, Default)]
pub struct EcrFinding {
    /// CVE id (basic: `name`; enhanced: `vulnerabilityId`, else `title`).
    pub id: String,
    pub severity: String,
    pub score: Option<f64>,
    pub package: Option<String>,
    pub installed: Option<String>,
    pub fixed_in: Option<String>,
    /// Enhanced only: ACTIVE / SUPPRESSED / CLOSED.
    pub status: Option<String>,
    pub fix_available: Option<String>,
    pub exploit_available: Option<String>,
    pub url: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct EcrScanFindings {
    pub scan_status: String,
    pub status_description: Option<String>,
    pub completed_at: Option<String>,
    pub vuln_db_updated_at: Option<String>,
    /// "Basic" / "Enhanced" — which scanner produced the findings (empty
    /// when there are none, since the response doesn't say otherwise).
    pub scanner: String,
    pub findings: Vec<EcrFinding>,
    /// Findings returned beyond `MAX_IMAGE_FINDINGS` (dropped, least severe).
    pub truncated: usize,
}

/// An ECS task or task definition that references an image (Used By).
#[derive(Debug, Clone)]
pub struct EcrImageUser {
    /// "Task" / "Task Definition" — also the row label the jump keys on.
    pub kind: &'static str,
    pub name: String,
    /// Task ARN / task-definition ARN — the jump target.
    pub id: String,
    pub container: String,
    pub status: String,
}

/// What became of the image a running container pulled (#156).
#[derive(Debug, Clone, PartialEq)]
pub enum DigestStatus {
    Tagged,
    /// Still in the repo, but no tag points at it any more (a re-pushed
    /// mutable tag moved on) — an untagged-expiry lifecycle rule can delete
    /// it while tasks still run it.
    Untagged,
    /// Not among the repo's loaded images. `cut` is how many newest images
    /// were loaded when the repo's list was cut: the digest may just be
    /// older than the cut, so callers must not say it was deleted.
    Missing { cut: Option<usize> },
}

/// Where a running container's pulled digest stands in its repository.
/// `None` when it isn't ours to judge: not an ECR ref, the repo isn't
/// loaded (another account / region, or ECR not opened yet), or the repo
/// has no image rows — which is also what a repo whose image fetch failed
/// looks like, and "not found" must never be a guess.
pub fn digest_status(
    repos: &[&EcrRepository],
    images: &[&EcrImage],
    image_ref: &str,
    digest: &str,
) -> Option<DigestStatus> {
    let repo = repos.iter().find(|r| r.holds_ref(image_ref))?;
    let own: Vec<&&EcrImage> = images
        .iter()
        .filter(|i| i.repo_name == repo.name && i.repo_uri.eq_ignore_ascii_case(&repo.uri))
        .collect();
    let first = own.first()?;
    if let Some(img) = own.iter().find(|i| i.digest == digest) {
        return Some(if img.tags.is_empty() { DigestStatus::Untagged } else { DigestStatus::Tagged });
    }
    let cut = (first.repo_images_seen > own.len() || !first.repo_images_complete).then_some(own.len());
    Some(DigestStatus::Missing { cut })
}

/// A running container whose image digest is untagged or missing from its
/// repo — the drift behind `CannotPullContainerError` on the next placement.
#[derive(Debug, Clone)]
pub struct DigestDrift {
    /// The service the task belongs to, else the task's display name.
    pub owner: String,
    pub container: String,
    pub repo_name: String,
    pub repo_uri: String,
    pub digest: String,
    pub status: DigestStatus,
    /// Running tasks with this same owner/container/digest — one line, not
    /// one per replica.
    pub tasks: usize,
}

impl DigestDrift {
    /// The warning line, e.g. `⚠ orders-worker/app runs orders-worker@sha256:3c4d…
    /// — untagged: an untagged-expiry lifecycle rule can delete it`.
    pub fn warning(&self) -> String {
        let short = self.digest.get(..19).unwrap_or(&self.digest);
        let count = if self.tasks > 1 { format!(" ({} tasks)", self.tasks) } else { String::new() };
        let what = format!("⚠ {}/{}{count} runs {}@{}…", self.owner, self.container, self.repo_name, short);
        match &self.status {
            DigestStatus::Untagged => format!("{what} — untagged; an untagged-expiry rule can delete it"),
            DigestStatus::Missing { cut: None } => {
                format!("{what} — not in {}: new tasks can't pull it", self.repo_name)
            }
            DigestStatus::Missing { cut: Some(n) } => {
                format!("{what} — not among the newest {n} images loaded for {}", self.repo_name)
            }
            DigestStatus::Tagged => what,
        }
    }
}

/// Split an ECR image reference into `(registry host, repo name)`;
/// `None` for anything that isn't `<acct>.dkr.ecr.<region>.amazonaws.com/<repo>…`.
pub fn ecr_ref_repo(image_ref: &str) -> Option<(&str, &str)> {
    let (host, path) = image_ref.trim().split_once(".amazonaws.com/")?;
    if !host.contains(".dkr.ecr.") {
        return None;
    }
    let repo_tag = path.split_once('@').map_or(path, |(rt, _)| rt);
    let repo = repo_tag.rsplit_once(':').map_or(repo_tag, |(r, _)| r);
    Some((host, repo))
}

/// Severity rank for sorting, worst first.
pub fn severity_rank(sev: &str) -> u8 {
    match sev.to_ascii_uppercase().as_str() {
        "CRITICAL" => 0,
        "HIGH" => 1,
        "MEDIUM" => 2,
        "LOW" => 3,
        "INFORMATIONAL" => 4,
        _ => 5,
    }
}

/// The image's scan findings. `ScanNotFoundException` (never scanned) is an
/// empty result, not an error.
pub async fn fetch_ecr_image_findings(
    client: EcrClient,
    repo_name: String,
    registry_id: String,
    digest: String,
) -> Result<EcrScanFindings> {
    use aws_sdk_ecr::types::ImageIdentifier;
    let mut out = EcrScanFindings::default();
    let mut token: Option<String> = None;
    loop {
        let mut req = client
            .describe_image_scan_findings()
            .repository_name(&repo_name)
            .image_id(ImageIdentifier::builder().image_digest(&digest).build())
            .max_results(1000);
        if !registry_id.is_empty() {
            req = req.registry_id(&registry_id);
        }
        if let Some(t) = &token {
            req = req.next_token(t);
        }
        let page = match req.send().await {
            Ok(p) => p,
            Err(e)
                if e.as_service_error()
                    .map(|se| se.is_scan_not_found_exception())
                    .unwrap_or(false) =>
            {
                out.scan_status = "NOT SCANNED".to_string();
                return Ok(out);
            }
            Err(e) => {
                return Err(crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))
            }
        };
        if let Some(st) = page.image_scan_status() {
            out.scan_status = st.status().map(|s| s.as_str().to_string()).unwrap_or_default();
            out.status_description = st.description().map(|s| s.to_string());
        }
        if let Some(f) = page.image_scan_findings() {
            out.completed_at = f.image_scan_completed_at().map(|t| fmt_epoch(t.secs()));
            out.vuln_db_updated_at =
                f.vulnerability_source_updated_at().map(|t| fmt_epoch(t.secs()));
            for b in f.findings() {
                out.scanner = "Basic".to_string();
                let attr = |k: &str| {
                    b.attributes()
                        .iter()
                        .find(|a| a.key() == k)
                        .and_then(|a| a.value())
                        .map(|s| s.to_string())
                };
                out.findings.push(EcrFinding {
                    id: b.name().unwrap_or_default().to_string(),
                    severity: b.severity().map(|s| s.as_str().to_string()).unwrap_or_default(),
                    score: attr("CVSS3_SCORE")
                        .or_else(|| attr("CVSS2_SCORE"))
                        .and_then(|s| s.parse().ok()),
                    package: attr("package_name"),
                    installed: attr("package_version"),
                    url: b.uri().map(|s| s.to_string()),
                    description: b.description().map(|s| s.to_string()),
                    ..Default::default()
                });
            }
            for e in f.enhanced_findings() {
                out.scanner = "Enhanced".to_string();
                let details = e.package_vulnerability_details();
                let pkg = details.and_then(|d| d.vulnerable_packages().first());
                out.findings.push(EcrFinding {
                    id: details
                        .and_then(|d| d.vulnerability_id())
                        .or(e.title())
                        .unwrap_or_default()
                        .to_string(),
                    severity: e.severity().unwrap_or_default().to_string(),
                    score: (e.score() > 0.0).then_some(e.score()),
                    package: pkg.and_then(|p| p.name()).map(|s| s.to_string()),
                    installed: pkg.and_then(|p| p.version()).map(|s| s.to_string()),
                    fixed_in: pkg.and_then(|p| p.fixed_in_version()).map(|s| s.to_string()),
                    status: e.status().map(|s| s.to_string()),
                    fix_available: e.fix_available().map(|s| s.to_string()),
                    exploit_available: e.exploit_available().map(|s| s.to_string()),
                    url: details
                        .and_then(|d| d.source_url())
                        .or_else(|| {
                            e.remediation()
                                .and_then(|r| r.recommendation())
                                .and_then(|r| r.url())
                        })
                        .map(|s| s.to_string()),
                    description: e.description().map(|s| s.to_string()),
                });
            }
        }
        token = next_page_token(page.next_token(), &token);
        if token.is_none() {
            break;
        }
    }
    sort_findings(&mut out.findings);
    if out.findings.len() > MAX_IMAGE_FINDINGS {
        out.truncated = out.findings.len() - MAX_IMAGE_FINDINGS;
        out.findings.truncate(MAX_IMAGE_FINDINGS);
    }
    Ok(out)
}

/// Worst first: severity, then score (highest first), then id for a stable
/// order between equal findings.
pub fn sort_findings(findings: &mut [EcrFinding]) {
    findings.sort_by(|a, b| {
        severity_rank(&a.severity)
            .cmp(&severity_rank(&b.severity))
            .then_with(|| {
                b.score
                    .unwrap_or(0.0)
                    .partial_cmp(&a.score.unwrap_or(0.0))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| a.id.cmp(&b.id))
    });
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
    /// Whether `image_ref` names this repository: same registry host
    /// (account *and* region — a same-named repo elsewhere never matches)
    /// and same repo name.
    pub fn holds_ref(&self, image_ref: &str) -> bool {
        let (Some((host, repo)), Some((own_host, _))) =
            (ecr_ref_repo(image_ref), self.uri.split_once(".amazonaws.com/"))
        else {
            return false;
        };
        repo == self.name && host.eq_ignore_ascii_case(own_host)
    }

    /// Tags can be re-pushed onto a new image (`MUTABLE`, or
    /// `MUTABLE_WITH_EXCLUSION` for all but the excluded tags).
    pub fn tags_mutable(&self) -> bool {
        self.image_tag_mutability.starts_with("MUTABLE")
    }

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
        // Filtered from the Images tab's rows already loaded — no fetch.
        Images "Images",
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
    /// `repo@sha256:…` — the row id. A digest alone isn't unique: the same
    /// image pushed to two repos has the same digest.
    pub key: String,
    /// `repo:first-tag`, or `repo@sha256:<12 hex>` when untagged — the list
    /// label, which has to say which repo since the Images tab spans them all.
    pub label: String,
    pub repo_name: String,
    pub repo_uri: String,
    pub registry_id: String,
    pub digest: String,
    pub tags: Vec<String>,
    pub pushed_at: Option<String>,
    /// Push time in epoch seconds — the Images tab's newest-first order.
    pub pushed_secs: Option<i64>,
    pub last_pulled_at: Option<String>,
    pub size_bytes: Option<i64>,
    pub artifact_media_type: Option<String>,
    pub manifest_media_type: Option<String>,
    pub scan_status: String,
    pub scan_status_description: Option<String>,
    pub finding_counts: HashMap<String, i32>,
    /// Images the list load read for this repo — more than the repo's
    /// loaded rows when the newest-N cut dropped some.
    pub repo_images_seen: usize,
    /// False when the load stopped reading the repo's images early.
    pub repo_images_complete: bool,
}

impl EcrImage {
    pub fn from_sdk(d: &aws_sdk_ecr::types::ImageDetail, repo_name: &str, repo_uri: &str) -> Self {
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

        let digest = d.image_digest().unwrap_or_default().to_string();
        let label = match d.image_tags().first() {
            Some(t) => format!("{}:{}", repo_name, t),
            None => format!("{}@{}", repo_name, short_digest_of(&digest)),
        };
        Self {
            key: format!("{}@{}", repo_name, digest),
            label,
            repo_name: repo_name.to_string(),
            repo_uri: repo_uri.to_string(),
            registry_id: d.registry_id().unwrap_or_default().to_string(),
            digest,
            tags: d.image_tags().to_vec(),
            pushed_at: d.image_pushed_at().map(|t| fmt_epoch(t.secs())),
            pushed_secs: d.image_pushed_at().map(|t| t.secs()),
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
            repo_images_seen: 0,
            repo_images_complete: true,
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
        short_digest_of(&self.digest)
    }

    /// Whether a container image reference (`<acct>.dkr.ecr.<region>.
    /// amazonaws.com/<repo>[:tag|@digest]`, as ECS task definitions and
    /// tasks carry it) names this image. A digest reference must match the
    /// digest; a tag reference (no tag = `latest`) must be one of this
    /// image's *current* tags — so a task started from `:latest` before the
    /// tag moved is matched by its resolved digest instead, where the caller
    /// has one. The registry host (account + region) must be this repo's.
    pub fn matches_image_ref(&self, image_ref: &str, resolved_digest: Option<&str>) -> bool {
        let r = image_ref.trim();
        let Some((host, path)) = r.split_once(".amazonaws.com/") else {
            return false;
        };
        if !host.contains(".dkr.ecr.") {
            return false;
        }
        // Same registry: compare the whole host (account *and* region) with
        // this image's repo URI, so `web` in eu-west-1 isn't matched by a
        // us-east-1 `web`. Without a URI, fall back to the account alone.
        let own_host = self
            .repo_uri
            .split_once(".amazonaws.com/")
            .map(|(h, _)| h)
            .unwrap_or_default();
        if !own_host.is_empty() {
            if !host.eq_ignore_ascii_case(own_host) {
                return false;
            }
        } else {
            let account = host.split('.').next().unwrap_or_default();
            if !self.registry_id.is_empty() && !account.is_empty() && account != self.registry_id {
                return false;
            }
        }
        let (repo_tag, digest) = match path.split_once('@') {
            Some((rt, d)) => (rt, Some(d)),
            None => (path, None),
        };
        let (repo, tag) = match repo_tag.rsplit_once(':') {
            Some((repo, tag)) => (repo, Some(tag)),
            None => (repo_tag, None),
        };
        if repo != self.repo_name {
            return false;
        }
        if let Some(d) = digest.or(resolved_digest).filter(|d| !d.is_empty()) {
            return d == self.digest;
        }
        let tag = tag.unwrap_or("latest");
        self.tags.iter().any(|t| t == tag)
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

crate::sections! {
    pub enum EcrImageDetailSection,
    pub static ECR_IMAGE_SECTIONS = [
        Overview "Overview",
        Findings "Findings" => crate::app::App::trigger_ecr_image_findings_load,
        // Computed from the warm ECS cache at render time — no fetch, no hook.
        UsedBy "Used By",
    ]
}

impl Resource for EcrImage {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&ECR_IMAGE_SECTIONS)
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws ecr describe-images --repository-name {} --image-ids imageDigest={}",
            crate::aws::resource::shell_quote(&self.repo_name),
            crate::aws::resource::shell_quote(&self.digest)
        ))
    }
    fn cli_actions(&self) -> Vec<crate::aws::cli_actions::CliAction> {
        use crate::aws::cli_actions::{CliAction, CliTier};
        use crate::aws::resource::shell_quote;
        vec![CliAction::new(
            CliTier::Inspect,
            "describe-image-scan-findings",
            format!(
                "aws ecr describe-image-scan-findings --repository-name {} --image-id imageDigest={}",
                shell_quote(&self.repo_name),
                shell_quote(&self.digest)
            ),
        )]
    }
    fn references(&self) -> Vec<(String, String)> {
        vec![("Repository".to_string(), self.repo_name.clone())]
    }
    fn id(&self) -> &str {
        &self.key
    }
    fn name(&self) -> &str {
        &self.label
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
            "{} {} {} {} {} {}",
            self.key,
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

/// `sha256:<64 hex>` → the first 12 hex chars, as the console abbreviates.
fn short_digest_of(digest: &str) -> &str {
    match digest.strip_prefix("sha256:") {
        Some(hex) if hex.len() > 12 => &hex[..12],
        Some(hex) => hex,
        None => digest,
    }
}

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

#[cfg(test)]
mod image_tests {
    use super::*;

    const D1: &str = "sha256:aaaaaaaaaaaa0000000000000000000000000000000000000000000000000000";
    const D2: &str = "sha256:bbbbbbbbbbbb0000000000000000000000000000000000000000000000000000";
    const HOST: &str = "123456789012.dkr.ecr.us-east-1.amazonaws.com";

    fn image(repo: &str, digest: &str, tags: &[&str]) -> EcrImage {
        let mut b = aws_sdk_ecr::types::ImageDetail::builder()
            .registry_id("123456789012")
            .repository_name(repo)
            .image_digest(digest);
        for t in tags {
            b = b.image_tags(*t);
        }
        EcrImage::from_sdk(&b.build(), repo, &format!("{HOST}/{repo}"))
    }

    #[test]
    fn id_and_label_name_the_repo() {
        let tagged = image("team/app", D1, &["v1", "latest"]);
        assert_eq!(tagged.id(), format!("team/app@{D1}"));
        assert_eq!(tagged.name(), "team/app:v1");
        let untagged = image("team/app", D1, &[]);
        assert_eq!(untagged.name(), "team/app@aaaaaaaaaaaa");
        // Same digest in another repo is a different row.
        assert_ne!(image("other", D1, &[]).id(), tagged.id());
    }

    #[test]
    fn image_ref_matching() {
        let img = image("web", D1, &["v2", "latest"]);
        // Tag refs match current tags; no tag means latest.
        assert!(img.matches_image_ref(&format!("{HOST}/web:v2"), None));
        assert!(img.matches_image_ref(&format!("{HOST}/web"), None));
        assert!(!img.matches_image_ref(&format!("{HOST}/web:v1"), None));
        // Digest refs must match the digest.
        assert!(img.matches_image_ref(&format!("{HOST}/web@{D1}"), None));
        assert!(!img.matches_image_ref(&format!("{HOST}/web@{D2}"), None));
        // A resolved digest wins over a tag that has since moved.
        assert!(!img.matches_image_ref(&format!("{HOST}/web:latest"), Some(D2)));
        assert!(img.matches_image_ref(&format!("{HOST}/web:v1"), Some(D1)));
        // Wrong repo / account / non-ECR.
        assert!(!img.matches_image_ref(&format!("{HOST}/webapp:v2"), None));
        assert!(!img.matches_image_ref(
            "999999999999.dkr.ecr.us-east-1.amazonaws.com/web:v2",
            None
        ));
        assert!(!img.matches_image_ref("docker.io/library/web:v2", None));
        // Same account and repo name, other region's registry.
        assert!(!img.matches_image_ref(
            "123456789012.dkr.ecr.eu-west-1.amazonaws.com/web:v2",
            None
        ));
    }

    fn pushed(repo: &str, digest: &str, secs: i64) -> EcrImage {
        let d = aws_sdk_ecr::types::ImageDetail::builder()
            .registry_id("123456789012")
            .repository_name(repo)
            .image_digest(digest)
            .image_pushed_at(aws_smithy_types::DateTime::from_secs(secs))
            .build();
        EcrImage::from_sdk(&d, repo, &format!("{HOST}/{repo}"))
    }

    #[test]
    fn newest_images_sorts_before_cutting() {
        // Arrive oldest-first (DescribeImages has no order) — the cut must
        // keep the newest, not the first page.
        let n = MAX_IMAGES_PER_REPO + 20;
        let images: Vec<EcrImage> =
            (0..n).map(|i| pushed("web", &format!("sha256:{i:064}"), i as i64)).collect();
        let out = newest_images(images, true);
        assert_eq!(out.images.len(), MAX_IMAGES_PER_REPO);
        assert_eq!(out.seen, n);
        assert!(out.truncated());
        assert_eq!(out.images[0].pushed_secs, Some(n as i64 - 1));
        assert_eq!(out.images.last().unwrap().pushed_secs, Some(20));
        assert!(out.images.iter().all(|i| i.repo_images_seen == n));

        let small = newest_images(vec![pushed("web", D1, 1)], true);
        assert!(!small.truncated());
        assert!(newest_images(vec![pushed("web", D1, 1)], false).truncated());
    }

    #[test]
    fn truncation_warning_names_a_few_repos() {
        let one = truncation_warning(&["web".to_string()]);
        assert!(one.contains("newest 100") && one.ends_with("for web"), "{one}");
        let many: Vec<String> = ["e", "d", "c", "b", "a"].iter().map(|s| s.to_string()).collect();
        assert!(truncation_warning(&many).ends_with("for a, b, c (+2 more)"));
    }

    #[test]
    fn findings_sort_worst_first() {
        let f = |id: &str, sev: &str, score: Option<f64>| EcrFinding {
            id: id.into(),
            severity: sev.into(),
            score,
            ..Default::default()
        };
        let mut v = vec![
            f("low", "LOW", Some(9.0)),
            f("high-5", "HIGH", Some(5.0)),
            f("crit", "CRITICAL", None),
            f("high-8", "HIGH", Some(8.0)),
            f("odd", "", None),
        ];
        sort_findings(&mut v);
        let ids: Vec<&str> = v.iter().map(|x| x.id.as_str()).collect();
        assert_eq!(ids, ["crit", "high-8", "high-5", "low", "odd"]);
    }
}

#[cfg(test)]
mod drift_tests {
    use super::*;

    const HOST: &str = "123456789012.dkr.ecr.us-east-1.amazonaws.com";
    const LIVE: &str = "sha256:1111111111110000000000000000000000000000000000000000000000000000";
    const GONE: &str = "sha256:2222222222220000000000000000000000000000000000000000000000000000";

    fn repo(name: &str, mutability: &str) -> EcrRepository {
        EcrRepository::from_sdk(
            &aws_sdk_ecr::types::Repository::builder()
                .repository_name(name)
                .repository_uri(format!("{HOST}/{name}"))
                .image_tag_mutability(aws_sdk_ecr::types::ImageTagMutability::from(mutability))
                .build(),
        )
    }

    fn image(name: &str, digest: &str, tags: &[&str]) -> EcrImage {
        let mut b = aws_sdk_ecr::types::ImageDetail::builder()
            .registry_id("123456789012")
            .repository_name(name)
            .image_digest(digest);
        for t in tags {
            b = b.image_tags(*t);
        }
        EcrImage::from_sdk(&b.build(), name, &format!("{HOST}/{name}"))
    }

    #[test]
    fn classifies_a_running_digest_against_its_repo() {
        let web = repo("web", "MUTABLE");
        let tagged = image("web", LIVE, &["v2"]);
        let untagged = image("web", LIVE, &[]);
        let r = format!("{HOST}/web:v2");
        assert_eq!(digest_status(&[&web], &[&tagged], &r, LIVE), Some(DigestStatus::Tagged));
        // The tag moved on: the running digest is still there, untagged.
        assert_eq!(digest_status(&[&web], &[&untagged], &r, LIVE), Some(DigestStatus::Untagged));
        // The digest is gone from a fully read repo.
        assert_eq!(
            digest_status(&[&web], &[&tagged], &r, GONE),
            Some(DigestStatus::Missing { cut: None })
        );
    }

    #[test]
    fn a_cut_list_never_claims_the_digest_is_gone() {
        let web = repo("web", "MUTABLE");
        let mut newest = image("web", LIVE, &["v2"]);
        newest.repo_images_seen = 250;
        let status = digest_status(&[&web], &[&newest], &format!("{HOST}/web:v2"), GONE);
        assert_eq!(status, Some(DigestStatus::Missing { cut: Some(1) }));
        let drift = DigestDrift {
            owner: "api".into(),
            container: "app".into(),
            repo_name: "web".into(),
            repo_uri: format!("{HOST}/web"),
            digest: GONE.into(),
            status: status.unwrap(),
            tasks: 2,
        };
        let w = drift.warning();
        assert!(w.starts_with("⚠ api/app (2 tasks) runs web@sha256:222222222222…"), "{w}");
        assert!(w.contains("not among the newest 1"), "{w}");
        assert!(!w.contains("not in web"), "{w}");
    }

    #[test]
    fn says_nothing_when_it_cannot_know() {
        let web = repo("web", "MUTABLE");
        let img = image("web", LIVE, &["v2"]);
        // Repo not loaded (another region's same-named repo, or none at all).
        let other_region = "123456789012.dkr.ecr.eu-west-1.amazonaws.com/web:v2";
        assert_eq!(digest_status(&[&web], &[&img], other_region, GONE), None);
        assert_eq!(digest_status(&[], &[&img], &format!("{HOST}/web:v2"), GONE), None);
        // Not ECR at all.
        assert_eq!(digest_status(&[&web], &[&img], "docker.io/library/nginx:1", GONE), None);
        // No image rows for the repo — an empty repo looks like a failed fetch.
        assert_eq!(digest_status(&[&web], &[], &format!("{HOST}/web:v2"), GONE), None);
    }

    #[test]
    fn repo_holds_refs_by_host_and_exact_name() {
        let web = repo("team/web", "IMMUTABLE");
        assert!(web.holds_ref(&format!("{HOST}/team/web:v1")));
        assert!(web.holds_ref(&format!("{HOST}/team/web@{LIVE}")));
        assert!(web.holds_ref(&format!("{HOST}/team/web")));
        assert!(!web.holds_ref(&format!("{HOST}/team/webapp:v1")));
        assert!(!web.tags_mutable());
        assert!(repo("x", "MUTABLE_WITH_EXCLUSION").tags_mutable());
    }
}
