use crate::aws::client::AwsClients;
use crate::aws::pagination::next_page_token;
use crate::aws::resource::{native_state_label, Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::error::Result;
use crate::event::{Event, LoadProgress};
use async_trait::async_trait;
use aws_sdk_inspector2::types::{
    FilterCriteria, Finding, SortCriteria, SortField, SortOrder, StringComparison, StringFilter,
};
use aws_sdk_inspector2::Client as InspClient;
use std::any::Any;
use std::collections::HashMap;
use tokio::sync::mpsc;

/// Findings fetched per scope are capped to keep memory bounded.
const MAX_FINDINGS: usize = 500;

/// Severity scope — a variant-cached server-side filter (like GuardDuty/SH).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InspSeverityScope {
    Critical,
    High,
    Medium,
    All,
}

impl InspSeverityScope {
    fn labels(&self) -> Option<&'static [&'static str]> {
        match self {
            InspSeverityScope::Critical => Some(&["CRITICAL"]),
            InspSeverityScope::High => Some(&["CRITICAL", "HIGH"]),
            InspSeverityScope::Medium => Some(&["CRITICAL", "HIGH", "MEDIUM"]),
            InspSeverityScope::All => None,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            InspSeverityScope::Critical => "Critical",
            InspSeverityScope::High => "High+",
            InspSeverityScope::Medium => "Medium+",
            InspSeverityScope::All => "All",
        }
    }
}

/// Amazon Inspector (v2) — continuous vulnerability scanning. Sub-tabs
/// Findings / Coverage. Findings are full from `list_findings` (no per-finding
/// Get), filtered to ACTIVE + severity ≥ scope, severity-ranked. Coverage lists
/// what's being scanned.
pub struct InspectorService {
    client: InspClient,
    scope: InspSeverityScope,
}

impl InspectorService {
    pub fn new(aws_clients: &AwsClients, scope: InspSeverityScope) -> Self {
        Self {
            client: aws_clients.inspector2_client(),
            scope,
        }
    }
}

#[async_trait]
impl AwsService for InspectorService {
    fn service_type(&self) -> ServiceType {
        ServiceType::Inspector
    }

    fn name(&self) -> &str {
        "Inspector"
    }

    async fn list_resources(&self) -> Result<Vec<Box<dyn Resource>>> {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        self.list_resources_streaming(tx, ServiceType::Inspector)
            .await?;
        Ok(vec![])
    }

    async fn list_resources_streaming(
        &self,
        event_tx: mpsc::UnboundedSender<Event>,
        service_type: ServiceType,
    ) -> Result<()> {
        let mut total = 0usize;
        // Accumulate every finding so we can also stream a by-resource grouping.
        let mut all_findings: Vec<InspFinding> = Vec::new();

        // ── Findings (full from list_findings, severity-ranked) ──────────────
        let filter = build_filter(self.scope);
        let sort = SortCriteria::builder()
            .field(SortField::Severity)
            .sort_order(SortOrder::Desc)
            .build()
            .ok();

        let mut token: Option<String> = None;
        loop {
            let mut req = self
                .client
                .list_findings()
                .filter_criteria(filter.clone())
                .max_results(100);
            if let Some(s) = &sort {
                req = req.sort_criteria(s.clone());
            }
            if let Some(t) = &token {
                req = req.next_token(t);
            }
            let page = match req.send().await {
                Ok(p) => p,
                Err(e) => {
                    let _ = event_tx.send(Event::ResourceLoadError {
                        service: service_type,
                        error: friendly_error(&crate::error::sdk_error_message(&e)),
                    });
                    return Ok(());
                }
            };

            let mut batch: Vec<InspFinding> =
                page.findings().iter().map(InspFinding::from_sdk).collect();
            batch.sort_by(|a, b| {
                severity_rank(&b.severity_label)
                    .cmp(&severity_rank(&a.severity_label))
                    .then(b.inspector_score.partial_cmp(&a.inspector_score).unwrap_or(std::cmp::Ordering::Equal))
            });
            if !batch.is_empty() {
                all_findings.extend(batch.iter().cloned());
                let boxed: Vec<Box<dyn Resource>> = batch
                    .into_iter()
                    .map(|f| Box::new(f) as Box<dyn Resource>)
                    .collect();
                total += boxed.len();
                let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                    service: service_type,
                    resources: boxed,
                    progress: LoadProgress {
                        loaded_count: total,
                        total_count: None,
                        status_message: None,
                    },
                });
            }

            token = next_page_token(page.next_token(), &token);
            if token.is_none() || total >= MAX_FINDINGS {
                break;
            }
        }

        // ── By-resource grouping (console-style: by instance / image / fn) ───
        let groups = InspResourceGroup::group(&all_findings);
        if !groups.is_empty() {
            let boxed: Vec<Box<dyn Resource>> = groups
                .into_iter()
                .map(|g| Box::new(g) as Box<dyn Resource>)
                .collect();
            total += boxed.len();
            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                service: service_type,
                resources: boxed,
                progress: LoadProgress {
                    loaded_count: total,
                    total_count: None,
                    status_message: None,
                },
            });
        }

        // ── Coverage (what's being scanned) ──────────────────────────────────
        let mut cov_token: Option<String> = None;
        let mut coverage: Vec<Box<dyn Resource>> = Vec::new();
        loop {
            let mut req = self.client.list_coverage().max_results(200);
            if let Some(t) = &cov_token {
                req = req.next_token(t);
            }
            match req.send().await {
                Ok(page) => {
                    for c in page.covered_resources() {
                        coverage.push(Box::new(InspCoverage::from_sdk(c)) as Box<dyn Resource>);
                    }
                    cov_token = next_page_token(page.next_token(), &cov_token);
                    if cov_token.is_none() || coverage.len() >= MAX_FINDINGS {
                        break;
                    }
                }
                Err(e) => {
                    let _ = event_tx.send(Event::ResourceLoadWarning {
                        service: service_type,
                        warning: format!(
                            "coverage: {}",
                            crate::error::sdk_error_message(&e)
                        ),
                    });
                    break;
                }
            }
        }
        if !coverage.is_empty() {
            total += coverage.len();
            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                service: service_type,
                resources: coverage,
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

fn eq(value: &str) -> StringFilter {
    StringFilter::builder()
        .comparison(StringComparison::Equals)
        .value(value)
        .build()
        .expect("StringFilter requires comparison + value")
}

fn build_filter(scope: InspSeverityScope) -> FilterCriteria {
    let mut f = FilterCriteria::builder().finding_status(eq("ACTIVE"));
    if let Some(labels) = scope.labels() {
        for l in labels {
            f = f.severity(eq(l));
        }
    }
    f.build()
}

fn friendly_error(raw: &str) -> String {
    let low = raw.to_lowercase();
    if low.contains("accessdenied") || low.contains("not enabled") || low.contains("not authorized") {
        "Amazon Inspector isn't enabled in this region (or read access is missing).".to_string()
    } else {
        format!("Failed to load Inspector: {}", raw)
    }
}

// ── Severity helpers ─────────────────────────────────────────────────────────

fn severity_rank(label: &str) -> u8 {
    match label {
        "CRITICAL" => 5,
        "HIGH" => 4,
        "MEDIUM" => 3,
        "LOW" => 2,
        "INFORMATIONAL" => 1,
        _ => 0, // UNTRIAGED / unknown
    }
}

pub fn severity_to_state(label: &str) -> ResourceState {
    match label {
        "CRITICAL" | "HIGH" => ResourceState::Unavailable,
        "MEDIUM" => ResourceState::Pending,
        _ => ResourceState::Unknown(String::new()),
    }
}

// ── InspFinding ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct InspFinding {
    pub arn: String,
    pub title: String,
    pub description: String,
    pub finding_type: String,
    pub severity_label: String,
    pub status: String,
    pub inspector_score: f64,
    pub resource_type: String,
    pub resource_id: String,
    /// ECR container-image findings only: the repository + image tags, so the
    /// resource reads as `repository:tag` instead of a bare image digest.
    pub repository: String,
    pub image_tags: Vec<String>,
    pub cve: String,
    pub package_summary: String,
    pub fix_available: String,
    pub exploit_available: String,
    pub vuln_rows: Vec<(String, String)>,
    pub remediation_text: String,
    pub remediation_url: String,
    pub first_observed: Option<String>,
    pub last_observed: Option<String>,
    pub raw_json: String,
}

impl InspFinding {
    fn from_sdk(f: &Finding) -> Self {
        let finding_type = f.r#type().as_str().to_string();
        let severity_label = f.severity().as_str().to_string();
        let status = f.status().as_str().to_string();
        let inspector_score = f.inspector_score().unwrap_or(0.0);

        let first_resource = f.resources().first();
        let (resource_type, resource_id) = first_resource
            .map(|r| (r.r#type().as_str().to_string(), r.id().to_string()))
            .unwrap_or_default();

        // Container images carry a repository + tags in their resource details;
        // capture them so the resource reads as `repo:tag`, not an image digest.
        let (mut repository, mut image_tags) = (String::new(), Vec::new());
        if let Some(img) = first_resource
            .and_then(|r| r.details())
            .and_then(|d| d.aws_ecr_container_image())
        {
            repository = img.repository_name().to_string();
            image_tags = img.image_tags().to_vec();
        }

        let mut cve = String::new();
        let mut package_summary = String::new();
        let mut vuln_rows: Vec<(String, String)> = Vec::new();

        if let Some(pv) = f.package_vulnerability_details() {
            cve = pv.vulnerability_id().to_string();
            vuln_rows.push(("CVE".to_string(), cve.clone()));
            if !pv.source().is_empty() {
                vuln_rows.push(("Source".to_string(), pv.source().to_string()));
            }
            if let Some(score) = pv.cvss().first() {
                vuln_rows.push((
                    "CVSS".to_string(),
                    format!("{} (v{}) {}", score.base_score(), score.version(), score.scoring_vector()),
                ));
            }
            if !pv.vulnerable_packages().is_empty() {
                vuln_rows.push((String::new(), String::new()));
                vuln_rows.push(("Vulnerable Packages".to_string(), String::new())); // group header
                for p in pv.vulnerable_packages() {
                    let fixed = p.fixed_in_version().unwrap_or("—");
                    let line = format!("{}@{} → fixed {}", p.name(), p.version(), fixed);
                    vuln_rows.push((format!("  {}", line), String::new()));
                    if package_summary.is_empty() {
                        package_summary = line;
                    }
                }
            }
            for url in pv.reference_urls().iter().take(5) {
                vuln_rows.push(("Reference".to_string(), url.clone()));
            }
        } else if let Some(nr) = f.network_reachability_details() {
            vuln_rows.push(("Protocol".to_string(), nr.protocol().as_str().to_string()));
            if let Some(pr) = nr.open_port_range() {
                vuln_rows.push(("Open Ports".to_string(), format!("{}–{}", pr.begin(), pr.end())));
            }
        } else if let Some(cv) = f.code_vulnerability_details() {
            vuln_rows.push(("Detector".to_string(), cv.detector_name().to_string()));
            if !cv.cwes().is_empty() {
                vuln_rows.push(("CWEs".to_string(), cv.cwes().join(", ")));
            }
            if let Some(fp) = cv.file_path() {
                vuln_rows.push(("File".to_string(), fp.file_name().to_string()));
            }
        }

        let fix_available = f
            .fix_available()
            .map(|x| x.as_str().to_string())
            .unwrap_or_else(|| "UNKNOWN".to_string());
        let exploit_available = f
            .exploit_available()
            .map(|x| x.as_str().to_string())
            .unwrap_or_else(|| "UNKNOWN".to_string());

        let (remediation_text, remediation_url) = match f.remediation().and_then(|r| r.recommendation()) {
            Some(rec) => (
                rec.text().unwrap_or_default().to_string(),
                rec.url().unwrap_or_default().to_string(),
            ),
            None => (String::new(), String::new()),
        };

        let title = f.title().unwrap_or_default().to_string();
        let description = f.description().to_string();
        let first_observed = Some(fmt_epoch_secs(f.first_observed_at().secs()));
        let last_observed = Some(fmt_epoch_secs(f.last_observed_at().secs()));

        let raw_json = build_raw_json(
            f,
            &finding_type,
            &severity_label,
            &status,
            &resource_type,
            &resource_id,
            &vuln_rows,
            &fix_available,
            &exploit_available,
        );

        Self {
            arn: f.finding_arn().to_string(),
            title,
            description,
            finding_type,
            severity_label,
            status,
            inspector_score,
            resource_type,
            resource_id,
            repository,
            image_tags,
            cve,
            package_summary,
            fix_available,
            exploit_available,
            vuln_rows,
            remediation_text,
            remediation_url,
            first_observed,
            last_observed,
            raw_json,
        }
    }

    /// Exploitable + a fix available = act now.
    pub fn is_actionable(&self) -> bool {
        self.exploit_available == "YES" && self.fix_available == "YES"
    }

    /// A human-friendly label for the affected resource. Container images read
    /// as `repository:tag` (`+N` when multiply-tagged, `repo@sha256:abcd…` when
    /// untagged) instead of the bare image digest; other types keep their id.
    pub fn resource_display(&self) -> String {
        if self.resource_type == "AWS_ECR_CONTAINER_IMAGE" && !self.repository.is_empty() {
            if let Some(tag) = self.image_tags.first() {
                let more = match self.image_tags.len() {
                    0 | 1 => String::new(),
                    n => format!(" +{}", n - 1),
                };
                return format!("{}:{}{}", self.repository, tag, more);
            }
            return format!("{}@{}", self.repository, short_digest(&self.resource_id));
        }
        self.resource_id.clone()
    }
}

/// Truncate an image-digest reference to `sha256:` + the first 12 hex chars.
fn short_digest(id: &str) -> String {
    match id.find("sha256:") {
        Some(idx) => {
            let end = (idx + "sha256:".len() + 12).min(id.len());
            format!("{}…", &id[idx..end])
        }
        None => id.to_string(),
    }
}

crate::sections! {
    pub enum InspFindingDetailSection,
    pub static INSP_FINDING_SECTIONS = [
        Details "Details",
        Vulnerability "Vulnerability",
        Resource "Resource",
        Remediation "Remediation",
    ]
}

impl Resource for InspFinding {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&INSP_FINDING_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.arn
    }
    fn name(&self) -> &str {
        if self.title.is_empty() {
            &self.cve
        } else {
            &self.title
        }
    }
    fn resource_type(&self) -> &str {
        "Inspector Finding"
    }
    fn state(&self) -> ResourceState {
        severity_to_state(&self.severity_label)
    }

    fn state_label(&self) -> String {
        native_state_label(&self.severity_label, || self.state())
    }
    fn tags(&self) -> &HashMap<String, String> {
        static EMPTY: std::sync::OnceLock<HashMap<String, String>> = std::sync::OnceLock::new();
        EMPTY.get_or_init(HashMap::new)
    }
    fn search_text(&self) -> String {
        // `arn` is included so a by-resource group can jump to one finding by
        // setting the search query to its ARN (see insp_resource_finding_jump).
        format!(
            "{} {} {} {} {} {} {} {} {}",
            self.title,
            self.cve,
            self.severity_label,
            self.finding_type,
            self.resource_id,
            self.repository,
            self.image_tags.join(" "),
            self.package_summary,
            self.arn,
        )
    }
    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Title".to_string(), self.title.clone()),
            ("Severity".to_string(), self.severity_label.clone()),
            ("CVE".to_string(), self.cve.clone()),
        ]
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/inspector/v2/home?region={}#/findings",
            region, region
        ))
    }
    fn raw_content(&self) -> Option<String> {
        Some(self.raw_json.clone())
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── InspCoverage ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct InspCoverage {
    pub resource_id: String,
    pub resource_type: String,
    pub scan_type: String,
    pub scan_status: String,
    pub tags: HashMap<String, String>,
}

impl InspCoverage {
    fn from_sdk(c: &aws_sdk_inspector2::types::CoveredResource) -> Self {
        Self {
            resource_id: c.resource_id().to_string(),
            resource_type: c.resource_type().as_str().to_string(),
            scan_type: c.scan_type().as_str().to_string(),
            scan_status: c
                .scan_status()
                .map(|s| s.status_code().as_str().to_string())
                .unwrap_or_default(),
            tags: HashMap::new(),
        }
    }
}

impl Resource for InspCoverage {
    fn id(&self) -> &str {
        &self.resource_id
    }
    fn name(&self) -> &str {
        &self.resource_id
    }
    fn resource_type(&self) -> &str {
        "Inspector Coverage"
    }
    fn state(&self) -> ResourceState {
        match self.scan_status.as_str() {
            "ACTIVE" => ResourceState::Available,
            "INACTIVE" => ResourceState::Stopped,
            other => ResourceState::Unknown(other.to_string()),
        }
    }

    fn state_label(&self) -> String {
        native_state_label(&self.scan_status, || self.state())
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        format!(
            "{} {} {} {}",
            self.resource_id, self.resource_type, self.scan_type, self.scan_status
        )
    }
    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Resource".to_string(), self.resource_id.clone()),
            ("Type".to_string(), self.resource_type.clone()),
            ("Scan Type".to_string(), self.scan_type.clone()),
            ("Scan Status".to_string(), self.scan_status.clone()),
        ]
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── InspResourceGroup (console-style "by resource" grouping) ─────────────────

/// One row per scanned resource (EC2 instance, container image, Lambda, …),
/// aggregating all of that resource's findings — mirrors the AWS console's
/// "by instance / by container image / by Lambda function" views. Carries the
/// full finding list so the detail pane and `X` export show every vulnerability.
#[derive(Debug, Clone)]
pub struct InspResourceGroup {
    pub resource_id: String,
    pub resource_type: String, // raw, e.g. AWS_EC2_INSTANCE
    pub display_type: String,  // friendly, e.g. "EC2 Instance"
    pub display_id: String,    // friendly, e.g. "my-repo:latest" (else = resource_id)
    pub findings: Vec<InspFinding>,
    pub critical: usize,
    pub high: usize,
    pub medium: usize,
    pub low: usize,
    pub other: usize,
    pub highest_severity: String,
    tags: HashMap<String, String>,
}

impl InspResourceGroup {
    /// Build the per-resource groups from a flat finding list, severity-sorted
    /// within each group and ordered worst-resource-first.
    fn group(findings: &[InspFinding]) -> Vec<InspResourceGroup> {
        let mut map: HashMap<(String, String), Vec<InspFinding>> = HashMap::new();
        for f in findings {
            if f.resource_id.is_empty() {
                continue;
            }
            map.entry((f.resource_type.clone(), f.resource_id.clone()))
                .or_default()
                .push(f.clone());
        }

        let mut groups: Vec<InspResourceGroup> = map
            .into_iter()
            .map(|((resource_type, resource_id), mut fs)| {
                fs.sort_by(|a, b| {
                    severity_rank(&b.severity_label)
                        .cmp(&severity_rank(&a.severity_label))
                        .then(
                            b.inspector_score
                                .partial_cmp(&a.inspector_score)
                                .unwrap_or(std::cmp::Ordering::Equal),
                        )
                });
                let (mut critical, mut high, mut medium, mut low, mut other) = (0, 0, 0, 0, 0);
                for f in &fs {
                    match f.severity_label.as_str() {
                        "CRITICAL" => critical += 1,
                        "HIGH" => high += 1,
                        "MEDIUM" => medium += 1,
                        "LOW" => low += 1,
                        _ => other += 1,
                    }
                }
                let highest_severity = fs
                    .first()
                    .map(|f| f.severity_label.clone())
                    .unwrap_or_default();
                let display_id = fs
                    .first()
                    .map(|f| f.resource_display())
                    .unwrap_or_else(|| resource_id.clone());
                InspResourceGroup {
                    display_type: display_resource_type(&resource_type),
                    display_id,
                    resource_id,
                    resource_type,
                    findings: fs,
                    critical,
                    high,
                    medium,
                    low,
                    other,
                    highest_severity,
                    tags: HashMap::new(),
                }
            })
            .collect();

        groups.sort_by(|a, b| {
            severity_rank(&b.highest_severity)
                .cmp(&severity_rank(&a.highest_severity))
                .then(b.findings.len().cmp(&a.findings.len()))
                .then(a.resource_id.cmp(&b.resource_id))
        });
        groups
    }

    pub fn total(&self) -> usize {
        self.findings.len()
    }

    /// The magenta group-header line for a finding in `details()` — also the key
    /// the Enter-jump matches a row back to its finding.
    fn finding_header(f: &InspFinding) -> String {
        if f.title.is_empty() {
            format!("{}  {}", f.severity_label, f.cve)
        } else {
            format!("{}  {}", f.severity_label, f.title)
        }
    }

    /// Find the finding ARN whose detail header equals `header` (for the Enter
    /// jump from a by-resource group row to the individual finding).
    pub fn arn_for_header(&self, header: &str) -> Option<String> {
        self.findings
            .iter()
            .find(|f| Self::finding_header(f) == header)
            .map(|f| f.arn.clone())
    }

    /// Compact severity tally for the list row, e.g. "3C 5H 2M".
    pub fn severity_summary(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if self.critical > 0 {
            parts.push(format!("{}C", self.critical));
        }
        if self.high > 0 {
            parts.push(format!("{}H", self.high));
        }
        if self.medium > 0 {
            parts.push(format!("{}M", self.medium));
        }
        if self.low > 0 {
            parts.push(format!("{}L", self.low));
        }
        if self.other > 0 {
            parts.push(format!("{}·", self.other));
        }
        parts.join(" ")
    }
}

/// Map a raw Inspector resource type to a friendly label.
fn display_resource_type(raw: &str) -> String {
    match raw {
        "AWS_EC2_INSTANCE" => "EC2 Instance".to_string(),
        "AWS_ECR_CONTAINER_IMAGE" => "Container Image".to_string(),
        "AWS_ECR_REPOSITORY" => "ECR Repository".to_string(),
        "AWS_LAMBDA_FUNCTION" => "Lambda Function".to_string(),
        other => other.trim_start_matches("AWS_").replace('_', " "),
    }
}

impl Resource for InspResourceGroup {
    fn id(&self) -> &str {
        &self.resource_id
    }
    fn name(&self) -> &str {
        // Friendly label (`repo:tag` for container images); == resource_id otherwise.
        &self.display_id
    }
    fn resource_type(&self) -> &str {
        "Inspector Resource"
    }
    fn state(&self) -> ResourceState {
        severity_to_state(&self.highest_severity)
    }

    fn state_label(&self) -> String {
        native_state_label(&self.highest_severity, || self.state())
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        // Include CVEs so a resource group is findable by any of its CVEs.
        let cves: Vec<&str> = self
            .findings
            .iter()
            .map(|f| f.cve.as_str())
            .filter(|c| !c.is_empty())
            .collect();
        format!(
            "{} {} {} {} {}",
            self.resource_id,
            self.display_id,
            self.display_type,
            self.resource_type,
            cves.join(" ")
        )
    }
    fn details(&self) -> Vec<(String, String)> {
        // The generic detail header already shows Resource/Type/State; lead with
        // the finding tally, then a full per-finding report.
        let mut rows: Vec<(String, String)> = vec![
            (
                "Findings".to_string(),
                format!("{} total · {}", self.total(), self.severity_summary()),
            ),
            (String::new(), String::new()),
        ];

        // Every finding for this resource, with full vulnerability detail — so
        // the pane reads as a per-resource report and `X` export captures it all.
        for (i, f) in self.findings.iter().enumerate() {
            if i > 0 {
                rows.push((String::new(), String::new()));
            }
            // Group header (magenta bold): severity + title.
            rows.push((Self::finding_header(f), String::new()));
            if !f.cve.is_empty() {
                rows.push(("CVE".to_string(), f.cve.clone()));
            }
            if f.inspector_score > 0.0 {
                rows.push(("Score".to_string(), format!("{:.1}", f.inspector_score)));
            }
            rows.push(("Fix Available".to_string(), f.fix_available.clone()));
            rows.push(("Exploit".to_string(), f.exploit_available.clone()));
            if !f.package_summary.is_empty() {
                rows.push(("Package".to_string(), f.package_summary.clone()));
            }
            if let Some(last) = &f.last_observed {
                rows.push(("Last Observed".to_string(), last.clone()));
            }
            if !f.remediation_text.is_empty() {
                rows.push(("Remediation".to_string(), f.remediation_text.clone()));
            }
        }
        rows.push((String::new(), String::new()));
        rows
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/inspector/v2/home?region={}#/findings",
            region, region
        ))
    }
    fn raw_content(&self) -> Option<String> {
        // Concatenate each finding's raw JSON into a JSON array.
        let items: Vec<String> = self.findings.iter().map(|f| f.raw_json.clone()).collect();
        Some(format!("[\n{}\n]", items.join(",\n")))
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── raw JSON (the SDK finding isn't Serialize) ───────────────────────────────

#[allow(clippy::too_many_arguments)]
fn build_raw_json(
    f: &Finding,
    finding_type: &str,
    severity_label: &str,
    status: &str,
    resource_type: &str,
    resource_id: &str,
    vuln_rows: &[(String, String)],
    fix_available: &str,
    exploit_available: &str,
) -> String {
    use serde_json::{json, Map, Value};
    let mut vuln = Map::new();
    for (k, v) in vuln_rows {
        if k.trim().is_empty() || v.is_empty() {
            continue;
        }
        vuln.insert(k.trim().to_string(), Value::String(v.clone()));
    }
    let v = json!({
        "FindingArn": f.finding_arn(),
        "Title": f.title().unwrap_or_default(),
        "Description": f.description(),
        "Type": finding_type,
        "Severity": severity_label,
        "Status": status,
        "InspectorScore": f.inspector_score().unwrap_or(0.0),
        "Resource": { "Type": resource_type, "Id": resource_id },
        "FixAvailable": fix_available,
        "ExploitAvailable": exploit_available,
        "Vulnerability": Value::Object(vuln),
    });
    serde_json::to_string_pretty(&v).unwrap_or_else(|_| "{}".to_string())
}

// ── Timestamp helper ─────────────────────────────────────────────────────────

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

#[cfg(test)]
mod tests {
    use super::*;

    fn finding(resource_type: &str, resource_id: &str, repo: &str, tags: &[&str]) -> InspFinding {
        InspFinding {
            arn: String::new(),
            title: String::new(),
            description: String::new(),
            finding_type: String::new(),
            severity_label: "HIGH".to_string(),
            status: "ACTIVE".to_string(),
            inspector_score: 0.0,
            resource_type: resource_type.to_string(),
            resource_id: resource_id.to_string(),
            repository: repo.to_string(),
            image_tags: tags.iter().map(|t| t.to_string()).collect(),
            cve: String::new(),
            package_summary: String::new(),
            fix_available: "UNKNOWN".to_string(),
            exploit_available: "UNKNOWN".to_string(),
            vuln_rows: Vec::new(),
            remediation_text: String::new(),
            remediation_url: String::new(),
            first_observed: None,
            last_observed: None,
            raw_json: String::new(),
        }
    }

    const IMG_ID: &str =
        "arn:aws:ecr:us-east-1:123456789012:repository/my-app/sha256:abcdef0123456789ff";

    #[test]
    fn container_image_reads_as_repo_tag() {
        let f = finding("AWS_ECR_CONTAINER_IMAGE", IMG_ID, "my-app", &["latest"]);
        assert_eq!(f.resource_display(), "my-app:latest");
    }

    #[test]
    fn container_image_multi_tag_shows_overflow() {
        let f = finding("AWS_ECR_CONTAINER_IMAGE", IMG_ID, "my-app", &["latest", "v2", "prod"]);
        assert_eq!(f.resource_display(), "my-app:latest +2");
    }

    #[test]
    fn untagged_image_falls_back_to_short_digest() {
        let f = finding("AWS_ECR_CONTAINER_IMAGE", IMG_ID, "my-app", &[]);
        assert_eq!(f.resource_display(), "my-app@sha256:abcdef012345…");
    }

    #[test]
    fn non_image_keeps_resource_id() {
        let f = finding("AWS_EC2_INSTANCE", "i-0abc123", "", &[]);
        assert_eq!(f.resource_display(), "i-0abc123");
    }

    #[test]
    fn image_without_repository_keeps_resource_id() {
        // Defensive: no repo detail → don't fabricate a label.
        let f = finding("AWS_ECR_CONTAINER_IMAGE", IMG_ID, "", &["latest"]);
        assert_eq!(f.resource_display(), IMG_ID);
    }

    #[test]
    fn short_digest_without_sha_is_unchanged() {
        assert_eq!(short_digest("no-digest-here"), "no-digest-here");
    }
}
