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

/// Findings fetched per scope are capped to keep memory bounded. Hitting the
/// cap is never silent: the load warns, and every By Resource row says its
/// tally is partial.
const MAX_FINDINGS: usize = 1000;

/// Coverage rows fetched are capped the same way (and warn the same way).
const MAX_COVERAGE: usize = 1000;

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
        let mut truncated = false;
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
            batch.sort_by(finding_order);
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
            if token.is_none() {
                break;
            }
            if total >= MAX_FINDINGS {
                truncated = true;
                let _ = event_tx.send(Event::ResourceLoadWarning {
                    service: service_type,
                    warning: format!(
                        "findings: showing the first {} ({} scope) — more exist; `t` narrows the severity scope",
                        total,
                        self.scope.label()
                    ),
                });
                break;
            }
        }

        // ── By-resource grouping (console-style: by instance / image / fn) ───
        let groups = InspResourceGroup::group(&all_findings, self.scope, truncated);
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
                    if cov_token.is_none() {
                        break;
                    }
                    if coverage.len() >= MAX_COVERAGE {
                        let _ = event_tx.send(Event::ResourceLoadWarning {
                            service: service_type,
                            warning: format!(
                                "coverage: showing the first {} resources — more exist",
                                coverage.len()
                            ),
                        });
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

/// Reference URLs / network-path steps shown before a `· N more` note.
const MAX_LIST_ROWS: usize = 10;

#[derive(Debug, Clone)]
pub struct InspFinding {
    pub arn: String,
    pub title: String,
    pub description: String,
    pub finding_type: String,
    pub severity_label: String,
    pub status: String,
    /// `None` when Inspector hasn't scored the finding — typically a CVE too
    /// new for NVD to have a CVSS. The severity then stands on its own (the
    /// vendor advisory's), so never render this as `0.0`.
    pub inspector_score: Option<f64>,
    /// The advisory vendor's own rating (`packageVulnerabilityDetails.vendorSeverity`).
    pub vendor_severity: String,
    /// EPSS: probability (0–1) the CVE is exploited in the next 30 days.
    pub epss: Option<f64>,
    pub last_known_exploit: Option<String>,
    pub aws_account_id: String,
    pub resource_region: String,
    pub resource_type: String,
    pub resource_id: String,
    /// ECR container-image findings only: the repository + image tags, so the
    /// resource reads as `repository:tag` instead of a bare image digest.
    pub repository: String,
    pub image_tags: Vec<String>,
    pub cve: String,
    pub related_vulnerabilities: Vec<String>,
    /// Every vulnerable package as `name@version → fixed X`.
    pub packages: Vec<String>,
    pub fix_available: String,
    pub exploit_available: String,
    /// The Vulnerability section (scoring, advisory, packages, references),
    /// built once at ingestion.
    pub vuln_rows: Vec<(String, String)>,
    /// Type-specific resource detail (ECR image / EC2 instance / Lambda).
    pub resource_rows: Vec<(String, String)>,
    pub remediation_text: String,
    pub remediation_url: String,
    pub first_observed: Option<String>,
    pub last_observed: Option<String>,
    pub updated_at: Option<String>,
    /// The affected resource's tags — so `tag:` filters and the ownership
    /// ribbon work on findings.
    pub tags: HashMap<String, String>,
    /// Everything searchable, precomputed (`search_text` runs per keystroke).
    pub search_blob: String,
    pub raw_json: String,
}

fn fmt_dt(d: &aws_smithy_types::DateTime) -> String {
    fmt_epoch_secs(d.secs())
}

/// `0.0423` → `0.0423 (4.2%)`.
fn fmt_epss(p: f64) -> String {
    format!("{:.4} ({:.1}%)", p, p * 100.0)
}

/// Push `(key, value)` when the value is non-empty.
fn push_some(rows: &mut Vec<(String, String)>, key: &str, value: Option<&str>) {
    if let Some(v) = value.filter(|v| !v.is_empty()) {
        rows.push((key.to_string(), v.to_string()));
    }
}

/// A capped list of rows under one key, with the `· N more` note.
fn push_capped(rows: &mut Vec<(String, String)>, key: &str, values: &[String]) {
    for v in values.iter().take(MAX_LIST_ROWS) {
        rows.push((key.to_string(), v.clone()));
    }
    if values.len() > MAX_LIST_ROWS {
        rows.push((String::new(), format!("· {} more", values.len() - MAX_LIST_ROWS)));
    }
}

impl InspFinding {
    pub(crate) fn from_sdk(f: &Finding) -> Self {
        let finding_type = f.r#type().as_str().to_string();
        let severity_label = f.severity().as_str().to_string();
        let status = f.status().as_str().to_string();
        let inspector_score = f.inspector_score();
        let epss = f.epss().map(|e| e.score());
        let last_known_exploit = f
            .exploitability_details()
            .and_then(|e| e.last_known_exploit_at())
            .map(fmt_dt);

        let first_resource = f.resources().first();
        let (resource_type, resource_id) = first_resource
            .map(|r| (r.r#type().as_str().to_string(), r.id().to_string()))
            .unwrap_or_default();
        let resource_region = first_resource
            .and_then(|r| r.region())
            .unwrap_or_default()
            .to_string();
        let tags = first_resource
            .and_then(|r| r.tags())
            .cloned()
            .unwrap_or_default();

        // Container images carry a repository + tags in their resource details;
        // capture them so the resource reads as `repo:tag`, not an image digest.
        let (mut repository, mut image_tags) = (String::new(), Vec::new());
        let mut resource_rows: Vec<(String, String)> = Vec::new();
        if let Some(d) = first_resource.and_then(|r| r.details()) {
            if let Some(img) = d.aws_ecr_container_image() {
                repository = img.repository_name().to_string();
                image_tags = img.image_tags().to_vec();
                push_some(&mut resource_rows, "Registry", Some(img.registry()));
                push_some(&mut resource_rows, "Image Hash", Some(img.image_hash()));
                push_some(&mut resource_rows, "Pushed", img.pushed_at().map(fmt_dt).as_deref());
                push_some(&mut resource_rows, "Author", img.author());
                push_some(&mut resource_rows, "Architecture", img.architecture());
                push_some(&mut resource_rows, "Platform", img.platform());
                if let Some(n) = img.in_use_count() {
                    resource_rows.push(("In Use Count".to_string(), n.to_string()));
                }
                push_some(&mut resource_rows, "Last In Use", img.last_in_use_at().map(fmt_dt).as_deref());
            }
            if let Some(ec2) = d.aws_ec2_instance() {
                push_some(&mut resource_rows, "AMI", ec2.image_id());
                push_some(&mut resource_rows, "Platform", ec2.platform());
                push_some(&mut resource_rows, "VPC", ec2.vpc_id());
                push_some(&mut resource_rows, "Subnet", ec2.subnet_id());
                push_some(&mut resource_rows, "Key Pair", ec2.key_name());
                push_some(&mut resource_rows, "Instance Profile", ec2.iam_instance_profile_arn());
                push_some(&mut resource_rows, "Launched", ec2.launched_at().map(fmt_dt).as_deref());
            }
            if let Some(fun) = d.aws_lambda_function() {
                push_some(&mut resource_rows, "Function", Some(fun.function_name()));
                push_some(&mut resource_rows, "Runtime", Some(fun.runtime().as_str()));
                push_some(&mut resource_rows, "Version", Some(fun.version()));
                push_some(&mut resource_rows, "Package Type", fun.package_type().map(|p| p.as_str()));
                let archs: Vec<&str> = fun.architectures().iter().map(|a| a.as_str()).collect();
                if !archs.is_empty() {
                    resource_rows.push(("Architectures".to_string(), archs.join(", ")));
                }
                push_some(&mut resource_rows, "Execution Role", Some(fun.execution_role_arn()));
                push_some(&mut resource_rows, "Last Modified", fun.last_modified_at().map(fmt_dt).as_deref());
                push_capped(&mut resource_rows, "Layer", fun.layers());
                if let Some(vpc) = fun.vpc_config() {
                    push_some(&mut resource_rows, "VPC", vpc.vpc_id());
                    push_capped(&mut resource_rows, "Subnet", vpc.subnet_ids());
                    push_capped(&mut resource_rows, "Security Group", vpc.security_group_ids());
                }
            }
        }

        let mut cve = String::new();
        let mut vendor_severity = String::new();
        let mut related_vulnerabilities: Vec<String> = Vec::new();
        let mut packages: Vec<String> = Vec::new();
        // Package paths / managers etc. — searchable, not shown in the list.
        let mut package_search: Vec<String> = Vec::new();
        let mut code_search: Vec<String> = Vec::new();

        // ── Scoring (every finding type) ─────────────────────────────────────
        let mut vuln_rows: Vec<(String, String)> = vec![("Scoring".to_string(), String::new())];
        vuln_rows.push((
            "Inspector Score".to_string(),
            match inspector_score {
                Some(s) => format!("{:.1}", s),
                None => "— not scored by Inspector (no CVSS yet)".to_string(),
            },
        ));
        if let Some(pv) = f.package_vulnerability_details() {
            vendor_severity = pv.vendor_severity().unwrap_or_default().to_string();
        }
        push_some(&mut vuln_rows, "Vendor Severity", Some(vendor_severity.as_str()));
        if let Some(p) = epss {
            vuln_rows.push(("EPSS".to_string(), fmt_epss(p)));
        }
        push_some(&mut vuln_rows, "Last Known Exploit", last_known_exploit.as_deref());
        if let Some(adj) = f.inspector_score_details().and_then(|d| d.adjusted_cvss()) {
            let src = match adj.cvss_source() {
                Some(c) if !c.is_empty() => format!("{} via {}", adj.score_source(), c),
                _ => adj.score_source().to_string(),
            };
            vuln_rows.push((
                "Adjusted CVSS".to_string(),
                format!("{} (v{}, {}) {}", adj.score(), adj.version(), src, adj.scoring_vector()),
            ));
            for a in adj.adjustments() {
                vuln_rows.push((format!("    {}", a.metric()), a.reason().to_string()));
            }
        }

        if let Some(pv) = f.package_vulnerability_details() {
            for score in pv.cvss() {
                let key = if score.source().is_empty() {
                    "CVSS".to_string()
                } else {
                    format!("CVSS · {}", score.source())
                };
                vuln_rows.push((
                    key,
                    format!("{} (v{}) {}", score.base_score(), score.version(), score.scoring_vector()),
                ));
            }

            cve = pv.vulnerability_id().to_string();
            related_vulnerabilities = pv.related_vulnerabilities().to_vec();
            vuln_rows.push((String::new(), String::new()));
            vuln_rows.push(("Advisory".to_string(), String::new()));
            vuln_rows.push(("CVE".to_string(), cve.clone()));
            if !related_vulnerabilities.is_empty() {
                vuln_rows.push(("Related".to_string(), related_vulnerabilities.join(", ")));
            }
            push_some(&mut vuln_rows, "Source", Some(pv.source()));
            push_some(&mut vuln_rows, "Source URL", pv.source_url());
            push_some(&mut vuln_rows, "Vendor Published", pv.vendor_created_at().map(fmt_dt).as_deref());
            push_some(&mut vuln_rows, "Vendor Updated", pv.vendor_updated_at().map(fmt_dt).as_deref());

            if !pv.vulnerable_packages().is_empty() {
                vuln_rows.push((String::new(), String::new()));
                vuln_rows.push(("Vulnerable Packages".to_string(), String::new())); // group header
                for p in pv.vulnerable_packages() {
                    let fixed = p.fixed_in_version().unwrap_or("—");
                    let line = format!("{}@{} → fixed {}", p.name(), p.version(), fixed);
                    vuln_rows.push((format!("  {}", line), String::new()));
                    let manager = p.package_manager().map(|m| m.as_str());
                    push_some(&mut vuln_rows, "    Manager", manager);
                    push_some(&mut vuln_rows, "    Arch", p.arch());
                    push_some(&mut vuln_rows, "    Release", p.release());
                    if p.epoch() != 0 {
                        vuln_rows.push(("    Epoch".to_string(), p.epoch().to_string()));
                    }
                    push_some(&mut vuln_rows, "    File", p.file_path());
                    push_some(&mut vuln_rows, "    Remediation", p.remediation());
                    push_some(&mut vuln_rows, "    Image Layer", p.source_layer_hash());
                    push_some(&mut vuln_rows, "    Lambda Layer", p.source_lambda_layer_arn());
                    package_search.extend(
                        [manager, p.file_path(), p.source_lambda_layer_arn(), p.remediation()]
                            .into_iter()
                            .flatten()
                            .map(str::to_string),
                    );
                    packages.push(line);
                }
            }
            if !pv.reference_urls().is_empty() {
                vuln_rows.push((String::new(), String::new()));
                vuln_rows.push(("References".to_string(), String::new()));
                push_capped(&mut vuln_rows, "Reference", pv.reference_urls());
            }
        } else if let Some(nr) = f.network_reachability_details() {
            vuln_rows.push((String::new(), String::new()));
            vuln_rows.push(("Network Reachability".to_string(), String::new()));
            vuln_rows.push(("Protocol".to_string(), nr.protocol().as_str().to_string()));
            if let Some(pr) = nr.open_port_range() {
                vuln_rows.push(("Open Ports".to_string(), format!("{}–{}", pr.begin(), pr.end())));
            }
            if let Some(path) = nr.network_path() {
                let steps = path.steps();
                if !steps.is_empty() {
                    vuln_rows.push((String::new(), String::new()));
                    vuln_rows.push(("Network Path".to_string(), String::new()));
                    for s in steps.iter().take(MAX_LIST_ROWS * 2) {
                        // Value is the component id (sg-/eni-/igw-…), so it's jumpable.
                        vuln_rows.push((format!("  {}", s.component_type()), s.component_id().to_string()));
                        code_search.push(s.component_id().to_string());
                    }
                    if steps.len() > MAX_LIST_ROWS * 2 {
                        vuln_rows.push((String::new(), format!("· {} more", steps.len() - MAX_LIST_ROWS * 2)));
                    }
                }
            }
        } else if let Some(cv) = f.code_vulnerability_details() {
            vuln_rows.push((String::new(), String::new()));
            vuln_rows.push(("Code Vulnerability".to_string(), String::new()));
            vuln_rows.push(("Detector".to_string(), cv.detector_name().to_string()));
            push_some(&mut vuln_rows, "Detector ID", Some(cv.detector_id()));
            push_some(&mut vuln_rows, "Rule ID", cv.rule_id());
            if !cv.cwes().is_empty() {
                vuln_rows.push(("CWEs".to_string(), cv.cwes().join(", ")));
            }
            if !cv.detector_tags().is_empty() {
                vuln_rows.push(("Detector Tags".to_string(), cv.detector_tags().join(", ")));
            }
            if let Some(fp) = cv.file_path() {
                vuln_rows.push((
                    "File".to_string(),
                    format!("{}:{}–{}", fp.file_name(), fp.start_line(), fp.end_line()),
                ));
                push_some(&mut vuln_rows, "Path", Some(fp.file_path()));
                code_search.push(fp.file_path().to_string());
            }
            push_some(&mut vuln_rows, "Lambda Layer", cv.source_lambda_layer_arn());
            if !cv.reference_urls().is_empty() {
                vuln_rows.push((String::new(), String::new()));
                vuln_rows.push(("References".to_string(), String::new()));
                push_capped(&mut vuln_rows, "Reference", cv.reference_urls());
            }
            code_search.push(cv.detector_name().to_string());
            code_search.extend(cv.rule_id().map(str::to_string));
            code_search.extend(cv.cwes().iter().cloned());
            code_search.extend(cv.detector_tags().iter().cloned());
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
        let first_observed = Some(fmt_dt(f.first_observed_at()));
        let last_observed = Some(fmt_dt(f.last_observed_at()));
        let updated_at = f.updated_at().map(fmt_dt);

        let mut search_parts: Vec<String> = vec![
            title.clone(),
            cve.clone(),
            related_vulnerabilities.join(" "),
            severity_label.clone(),
            vendor_severity.clone(),
            finding_type.clone(),
            status.clone(),
            resource_type.clone(),
            resource_id.clone(),
            repository.clone(),
            image_tags.join(" "),
            packages.join(" "),
            package_search.join(" "),
            code_search.join(" "),
            f.aws_account_id().to_string(),
            resource_region.clone(),
            f.finding_arn().to_string(),
        ];
        // Words a triage search reaches for, only when they hold.
        if fix_available == "YES" {
            search_parts.push("fixable fix-available".to_string());
        }
        if exploit_available == "YES" {
            search_parts.push("exploitable exploit-available".to_string());
        }
        if inspector_score.is_none() {
            search_parts.push("unscored".to_string());
        }
        for (k, v) in &tags {
            search_parts.push(format!("{}={}", k, v));
        }
        for (_, v) in &resource_rows {
            search_parts.push(v.clone());
        }
        search_parts.push(description.clone());
        let search_blob = search_parts
            .into_iter()
            .filter(|p| !p.is_empty())
            .collect::<Vec<_>>()
            .join(" ");

        Self {
            arn: f.finding_arn().to_string(),
            title,
            description,
            finding_type,
            severity_label,
            status,
            inspector_score,
            vendor_severity,
            epss,
            last_known_exploit,
            aws_account_id: f.aws_account_id().to_string(),
            resource_region,
            resource_type,
            resource_id,
            repository,
            image_tags,
            cve,
            related_vulnerabilities,
            packages,
            fix_available,
            exploit_available,
            vuln_rows,
            resource_rows,
            remediation_text,
            remediation_url,
            first_observed,
            last_observed,
            updated_at,
            tags,
            search_blob,
            raw_json: raw_json(f),
        }
    }

    /// Exploitable + a fix available = act now.
    pub fn is_actionable(&self) -> bool {
        self.exploit_available == "YES" && self.fix_available == "YES"
    }

    /// `CRITICAL (9.8)`, or `CRITICAL (no score)` when Inspector hasn't scored
    /// it — never `(0.0)`, which reads as a real score of zero.
    pub fn severity_display(&self) -> String {
        match self.inspector_score {
            Some(s) => format!("{} ({:.1})", self.severity_label, s),
            None => format!("{} (no score)", self.severity_label),
        }
    }

    /// The score for sorting; unscored sorts below any scored finding of the
    /// same severity.
    fn sort_score(&self) -> f64 {
        self.inspector_score.unwrap_or(-1.0)
    }

    pub fn epss_display(&self) -> Option<String> {
        self.epss.map(fmt_epss)
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

/// Severity first, then Inspector score (unscored last), then EPSS.
fn finding_order(a: &InspFinding, b: &InspFinding) -> std::cmp::Ordering {
    severity_rank(&b.severity_label)
        .cmp(&severity_rank(&a.severity_label))
        .then(b.sort_score().partial_cmp(&a.sort_score()).unwrap_or(std::cmp::Ordering::Equal))
        .then(
            b.epss
                .unwrap_or(-1.0)
                .partial_cmp(&a.epss.unwrap_or(-1.0))
                .unwrap_or(std::cmp::Ordering::Equal),
        )
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
        Overview "Overview",
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
    fn list_summary(&self) -> Option<String> {
        Some(self.resource_display()).filter(|s| !s.is_empty())
    }
    fn state(&self) -> ResourceState {
        severity_to_state(&self.severity_label)
    }

    fn state_label(&self) -> String {
        native_state_label(&self.severity_label, || self.state())
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        // Includes the ARN, so a by-resource group can jump to one finding by
        // setting the search query to it (see insp_resource_finding_jump).
        self.search_blob.clone()
    }
    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Title".to_string(), self.title.clone()),
            ("Severity".to_string(), self.severity_display()),
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
    pub display_type: String,  // friendly, e.g. "EC2 Instance"
    pub display_id: String,    // friendly, e.g. "my-repo:latest" (else = resource_id)
    pub findings: Vec<InspFinding>,
    /// One group-header line per finding (parallel to `findings`), made
    /// unique within the group — the Enter-jump maps a header back to its
    /// finding, so two findings sharing a title must not share a header.
    headers: Vec<String>,
    pub critical: usize,
    pub high: usize,
    pub medium: usize,
    pub low: usize,
    pub other: usize,
    pub highest_severity: String,
    /// The severity scope the findings were loaded under — the tally only
    /// counts what that scope fetched.
    scope: InspSeverityScope,
    /// The finding load hit `MAX_FINDINGS`, so this tally may be short.
    truncated: bool,
    tags: HashMap<String, String>,
    search_blob: String,
}

impl InspResourceGroup {
    /// Build the per-resource groups from a flat finding list, severity-sorted
    /// within each group and ordered worst-resource-first.
    fn group(findings: &[InspFinding], scope: InspSeverityScope, truncated: bool) -> Vec<InspResourceGroup> {
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
                fs.sort_by(finding_order);
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
                let tags = fs.first().map(|f| f.tags.clone()).unwrap_or_default();
                let display_type = display_resource_type(&resource_type);
                let mut blob = vec![
                    resource_id.clone(),
                    display_id.clone(),
                    display_type.clone(),
                    resource_type.clone(),
                ];
                // Every finding's own blob, so a group is findable by any CVE,
                // package, title, file path… that any of its findings carries.
                blob.extend(fs.iter().map(|f| f.search_blob.clone()));
                InspResourceGroup {
                    display_type,
                    display_id,
                    resource_id,
                    headers: unique_headers(&fs),
                    findings: fs,
                    critical,
                    high,
                    medium,
                    low,
                    other,
                    highest_severity,
                    scope,
                    truncated,
                    tags,
                    search_blob: blob.join(" "),
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

    /// Find the finding ARN whose detail header equals `header` (for the Enter
    /// jump from a by-resource group row to the individual finding).
    pub fn arn_for_header(&self, header: &str) -> Option<String> {
        self.headers
            .iter()
            .position(|h| h == header)
            .map(|i| self.findings[i].arn.clone())
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

/// The magenta group-header line per finding — `SEVERITY  title` — with a
/// ` #N` suffix on repeats so each maps back to exactly one finding.
fn unique_headers(findings: &[InspFinding]) -> Vec<String> {
    let mut seen: HashMap<String, usize> = HashMap::new();
    findings
        .iter()
        .map(|f| {
            let base = if f.title.is_empty() {
                format!("{}  {}", f.severity_label, f.cve)
            } else {
                format!("{}  {}", f.severity_label, f.title)
            };
            let n = seen.entry(base.clone()).or_insert(0);
            *n += 1;
            if *n == 1 {
                base
            } else {
                format!("{} #{}", base, n)
            }
        })
        .collect()
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
        self.search_blob.clone()
    }
    fn details(&self) -> Vec<(String, String)> {
        // The generic detail header already shows Resource/Type/State; lead with
        // the finding tally, then a full per-finding report.
        let mut rows: Vec<(String, String)> = vec![(
            "Findings".to_string(),
            format!("{} total · {}", self.total(), self.severity_summary()),
        )];
        if self.scope != InspSeverityScope::All {
            rows.push((
                String::new(),
                format!("· {} scope — lower severities aren't loaded; `t` widens", self.scope.label()),
            ));
        }
        if self.truncated {
            rows.push((
                String::new(),
                format!("⚠ Finding load capped at {} — this tally may be incomplete", MAX_FINDINGS),
            ));
        }
        if let Some(first) = self.findings.first() {
            if !first.aws_account_id.is_empty() {
                rows.push(("Account".to_string(), first.aws_account_id.clone()));
            }
            if !first.resource_region.is_empty() {
                rows.push(("Region".to_string(), first.resource_region.clone()));
            }
            rows.extend(first.resource_rows.iter().cloned());
        }
        rows.push((String::new(), String::new()));

        // Every finding for this resource, with full vulnerability detail — so
        // the pane reads as a per-resource report and `X` export captures it all.
        for (i, f) in self.findings.iter().enumerate() {
            if i > 0 {
                rows.push((String::new(), String::new()));
            }
            // Group header (magenta bold): severity + title.
            rows.push((self.headers[i].clone(), String::new()));
            if !f.cve.is_empty() {
                rows.push(("CVE".to_string(), f.cve.clone()));
            }
            rows.push((
                "Score".to_string(),
                match f.inspector_score {
                    Some(s) => format!("{:.1}", s),
                    None => "— not scored by Inspector (no CVSS yet)".to_string(),
                },
            ));
            if !f.vendor_severity.is_empty() {
                rows.push(("Vendor Severity".to_string(), f.vendor_severity.clone()));
            }
            if let Some(e) = f.epss_display() {
                rows.push(("EPSS".to_string(), e));
            }
            rows.push(("Fix Available".to_string(), f.fix_available.clone()));
            rows.push(("Exploit".to_string(), f.exploit_available.clone()));
            if let Some(x) = &f.last_known_exploit {
                rows.push(("Last Known Exploit".to_string(), x.clone()));
            }
            for p in &f.packages {
                rows.push(("Package".to_string(), p.clone()));
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

/// The whole finding as API-shaped JSON (camelCase, as `aws inspector2
/// list-findings` prints it) — every field, so `e` hides nothing.
fn raw_json(f: &Finding) -> String {
    use serde_json::{json, Value};
    fn dt(d: Option<&aws_smithy_types::DateTime>) -> Value {
        d.and_then(|d| d.fmt(aws_smithy_types::date_time::Format::DateTime).ok())
            .map(Value::String)
            .unwrap_or(Value::Null)
    }
    fn s(v: Option<&str>) -> Value {
        v.map(|v| Value::String(v.to_string())).unwrap_or(Value::Null)
    }

    let resources: Vec<Value> = f
        .resources()
        .iter()
        .map(|r| {
            let details = r.details().map(|d| {
                json!({
                    "awsEcrContainerImage": d.aws_ecr_container_image().map(|i| json!({
                        "repositoryName": i.repository_name(),
                        "imageTags": i.image_tags(),
                        "pushedAt": dt(i.pushed_at()),
                        "author": s(i.author()),
                        "architecture": s(i.architecture()),
                        "imageHash": i.image_hash(),
                        "registry": i.registry(),
                        "platform": s(i.platform()),
                        "lastInUseAt": dt(i.last_in_use_at()),
                        "inUseCount": i.in_use_count(),
                    })),
                    "awsEc2Instance": d.aws_ec2_instance().map(|e| json!({
                        "type": s(e.r#type()),
                        "imageId": s(e.image_id()),
                        "ipV4Addresses": e.ipv4_addresses(),
                        "ipV6Addresses": e.ipv6_addresses(),
                        "keyName": s(e.key_name()),
                        "iamInstanceProfileArn": s(e.iam_instance_profile_arn()),
                        "vpcId": s(e.vpc_id()),
                        "subnetId": s(e.subnet_id()),
                        "launchedAt": dt(e.launched_at()),
                        "platform": s(e.platform()),
                    })),
                    "awsLambdaFunction": d.aws_lambda_function().map(|l| json!({
                        "functionName": l.function_name(),
                        "runtime": l.runtime().as_str(),
                        "codeSha256": l.code_sha256(),
                        "version": l.version(),
                        "executionRoleArn": l.execution_role_arn(),
                        "layers": l.layers(),
                        "vpcConfig": l.vpc_config().map(|v| json!({
                            "vpcId": s(v.vpc_id()),
                            "subnetIds": v.subnet_ids(),
                            "securityGroupIds": v.security_group_ids(),
                        })),
                        "packageType": l.package_type().map(|p| p.as_str()),
                        "architectures": l.architectures().iter().map(|a| a.as_str()).collect::<Vec<_>>(),
                        "lastModifiedAt": dt(l.last_modified_at()),
                    })),
                })
            });
            json!({
                "type": r.r#type().as_str(),
                "id": r.id(),
                "partition": s(r.partition()),
                "region": s(r.region()),
                "tags": r.tags(),
                "details": details,
            })
        })
        .collect();

    let package = f.package_vulnerability_details().map(|pv| {
        json!({
            "vulnerabilityId": pv.vulnerability_id(),
            "source": pv.source(),
            "sourceUrl": s(pv.source_url()),
            "vendorSeverity": s(pv.vendor_severity()),
            "vendorCreatedAt": dt(pv.vendor_created_at()),
            "vendorUpdatedAt": dt(pv.vendor_updated_at()),
            "relatedVulnerabilities": pv.related_vulnerabilities(),
            "referenceUrls": pv.reference_urls(),
            "cvss": pv.cvss().iter().map(|c| json!({
                "baseScore": c.base_score(),
                "scoringVector": c.scoring_vector(),
                "version": c.version(),
                "source": c.source(),
            })).collect::<Vec<_>>(),
            "vulnerablePackages": pv.vulnerable_packages().iter().map(|p| json!({
                "name": p.name(),
                "version": p.version(),
                "epoch": p.epoch(),
                "release": s(p.release()),
                "arch": s(p.arch()),
                "packageManager": p.package_manager().map(|m| m.as_str()),
                "filePath": s(p.file_path()),
                "fixedInVersion": s(p.fixed_in_version()),
                "remediation": s(p.remediation()),
                "sourceLayerHash": s(p.source_layer_hash()),
                "sourceLambdaLayerArn": s(p.source_lambda_layer_arn()),
            })).collect::<Vec<_>>(),
        })
    });

    let network = f.network_reachability_details().map(|nr| {
        json!({
            "protocol": nr.protocol().as_str(),
            "openPortRange": nr.open_port_range().map(|p| json!({ "begin": p.begin(), "end": p.end() })),
            "networkPath": nr.network_path().map(|p| p.steps().iter().map(|st| json!({
                "componentId": st.component_id(),
                "componentType": st.component_type(),
                "componentArn": s(st.component_arn()),
            })).collect::<Vec<_>>()),
        })
    });

    let code = f.code_vulnerability_details().map(|cv| {
        json!({
            "detectorId": cv.detector_id(),
            "detectorName": cv.detector_name(),
            "detectorTags": cv.detector_tags(),
            "ruleId": s(cv.rule_id()),
            "cwes": cv.cwes(),
            "referenceUrls": cv.reference_urls(),
            "sourceLambdaLayerArn": s(cv.source_lambda_layer_arn()),
            "filePath": cv.file_path().map(|fp| json!({
                "fileName": fp.file_name(),
                "filePath": fp.file_path(),
                "startLine": fp.start_line(),
                "endLine": fp.end_line(),
            })),
        })
    });

    let adjusted = f.inspector_score_details().and_then(|d| d.adjusted_cvss()).map(|a| {
        json!({
            "scoreSource": a.score_source(),
            "cvssSource": s(a.cvss_source()),
            "version": a.version(),
            "score": a.score(),
            "scoringVector": a.scoring_vector(),
            "adjustments": a.adjustments().iter().map(|x| json!({
                "metric": x.metric(),
                "reason": x.reason(),
            })).collect::<Vec<_>>(),
        })
    });

    let v = json!({
        "findingArn": f.finding_arn(),
        "awsAccountId": f.aws_account_id(),
        "type": f.r#type().as_str(),
        "title": s(f.title()),
        "description": f.description(),
        "severity": f.severity().as_str(),
        "status": f.status().as_str(),
        "inspectorScore": f.inspector_score(),
        "inspectorScoreDetails": adjusted.map(|a| json!({ "adjustedCvss": a })),
        "epss": f.epss().map(|e| json!({ "score": e.score() })),
        "fixAvailable": f.fix_available().map(|x| x.as_str()),
        "exploitAvailable": f.exploit_available().map(|x| x.as_str()),
        "exploitabilityDetails": f.exploitability_details().map(|e| json!({
            "lastKnownExploitAt": dt(e.last_known_exploit_at()),
        })),
        "firstObservedAt": dt(Some(f.first_observed_at())),
        "lastObservedAt": dt(Some(f.last_observed_at())),
        "updatedAt": dt(f.updated_at()),
        "remediation": f.remediation().and_then(|r| r.recommendation()).map(|r| json!({
            "text": s(r.text()),
            "url": s(r.url()),
        })),
        "resources": resources,
        "packageVulnerabilityDetails": package,
        "networkReachabilityDetails": network,
        "codeVulnerabilityDetails": code,
    });
    serde_json::to_string_pretty(&strip_nulls(v)).unwrap_or_else(|_| "{}".to_string())
}

/// Drop null / absent fields so the JSON reads like the API's own output.
fn strip_nulls(v: serde_json::Value) -> serde_json::Value {
    use serde_json::Value;
    match v {
        Value::Object(m) => Value::Object(
            m.into_iter()
                .filter(|(_, v)| !v.is_null())
                .map(|(k, v)| (k, strip_nulls(v)))
                .collect(),
        ),
        Value::Array(a) => Value::Array(a.into_iter().map(strip_nulls).collect()),
        other => other,
    }
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
        let mut f = InspFinding::from_sdk(&sdk_finding("CVE-2024-0001", None, None));
        f.resource_type = resource_type.to_string();
        f.resource_id = resource_id.to_string();
        f.repository = repo.to_string();
        f.image_tags = tags.iter().map(|t| t.to_string()).collect();
        f
    }

    /// A CRITICAL package finding on an EC2 instance with two packages, the
    /// second of which is the one a search has to reach.
    fn sdk_finding(cve: &str, score: Option<f64>, vendor: Option<&str>) -> Finding {
        use aws_sdk_inspector2::types::{
            FindingStatus, FindingType, PackageVulnerabilityDetails, Resource as SdkResource,
            ResourceType, Severity, VulnerablePackage,
        };
        let pkg = |name: &str| {
            VulnerablePackage::builder()
                .name(name)
                .version("1.0.0")
                .fixed_in_version("1.0.1")
                .file_path(format!("/usr/lib/{name}"))
                .build()
                .unwrap()
        };
        let mut pv = PackageVulnerabilityDetails::builder()
            .vulnerability_id(cve)
            .source("UBUNTU_CVE")
            .vulnerable_packages(pkg("openssl"))
            .vulnerable_packages(pkg("libxml2"))
            .related_vulnerabilities("GHSA-xxxx-yyyy");
        if let Some(v) = vendor {
            pv = pv.vendor_severity(v);
        }
        let mut b = Finding::builder()
            .finding_arn(format!("arn:aws:inspector2:us-east-1:123456789012:finding/{cve}"))
            .aws_account_id("123456789012")
            .r#type(FindingType::PackageVulnerability)
            .title(format!("{cve} - openssl, libxml2"))
            .description("a description")
            .severity(Severity::Critical)
            .status(FindingStatus::Active)
            .first_observed_at(aws_smithy_types::DateTime::from_secs(1_704_067_200))
            .last_observed_at(aws_smithy_types::DateTime::from_secs(1_704_067_200))
            .resources(
                SdkResource::builder()
                    .r#type(ResourceType::AwsEc2Instance)
                    .id("i-0abc123")
                    .tags("team", "payments")
                    .build()
                    .unwrap(),
            )
            .package_vulnerability_details(pv.build().unwrap());
        if let Some(s) = score {
            b = b.inspector_score(s);
        }
        b.build().unwrap()
    }

    #[test]
    fn unscored_finding_never_reads_as_zero() {
        let f = InspFinding::from_sdk(&sdk_finding("CVE-2026-11147", None, Some("CRITICAL")));
        assert_eq!(f.inspector_score, None);
        assert_eq!(f.severity_display(), "CRITICAL (no score)");
        assert_eq!(f.vendor_severity, "CRITICAL");
        assert!(!f.severity_display().contains("0.0"));
        let grp = &InspResourceGroup::group(&[f], InspSeverityScope::All, false)[0];
        let score = grp.details().into_iter().find(|(k, _)| k == "Score").unwrap();
        assert!(score.1.contains("not scored"), "{score:?}");
    }

    #[test]
    fn scored_finding_shows_its_score() {
        let f = InspFinding::from_sdk(&sdk_finding("CVE-2024-0001", Some(9.8), None));
        assert_eq!(f.severity_display(), "CRITICAL (9.8)");
    }

    #[test]
    fn search_reaches_every_package_tag_and_related_id() {
        let f = InspFinding::from_sdk(&sdk_finding("CVE-2024-0001", None, None));
        let text = f.search_text();
        for needle in ["libxml2", "/usr/lib/libxml2", "GHSA-xxxx-yyyy", "team=payments", "123456789012", "unscored"] {
            assert!(text.contains(needle), "missing {needle:?} in {text}");
        }
        let grp = &InspResourceGroup::group(&[f], InspSeverityScope::All, false)[0];
        assert!(grp.search_text().contains("libxml2"));
        assert_eq!(grp.tags().get("team").map(String::as_str), Some("payments"));
    }

    #[test]
    fn every_package_is_listed_not_just_the_first() {
        let f = InspFinding::from_sdk(&sdk_finding("CVE-2024-0001", None, None));
        assert_eq!(f.packages.len(), 2);
        assert!(f.vuln_rows.iter().any(|(k, v)| k.trim() == "File" && v == "/usr/lib/libxml2"));
    }

    #[test]
    fn duplicate_titles_jump_to_distinct_findings() {
        let mut a = InspFinding::from_sdk(&sdk_finding("CVE-2024-0001", Some(9.8), None));
        let mut b = a.clone();
        a.arn = "arn-a".to_string();
        b.arn = "arn-b".to_string();
        let grp = &InspResourceGroup::group(&[a, b], InspSeverityScope::All, false)[0];
        let headers: Vec<String> = grp
            .details()
            .into_iter()
            .filter(|(k, v)| v.is_empty() && !k.is_empty() && !k.starts_with(' '))
            .map(|(k, _)| k)
            .collect();
        assert_eq!(headers.len(), 2);
        let arns: Vec<_> = headers.iter().map(|h| grp.arn_for_header(h).unwrap()).collect();
        assert_ne!(arns[0], arns[1]);
    }

    #[test]
    fn partial_tally_says_so() {
        let f = InspFinding::from_sdk(&sdk_finding("CVE-2024-0001", None, None));
        let grp = &InspResourceGroup::group(&[f], InspSeverityScope::High, true)[0];
        let notes: Vec<String> = grp.details().into_iter().map(|(_, v)| v).collect();
        assert!(notes.iter().any(|v| v.contains("High+ scope")));
        assert!(notes.iter().any(|v| v.contains("capped")));
    }

    #[test]
    fn raw_json_keeps_vendor_severity_and_omits_missing_score() {
        let f = InspFinding::from_sdk(&sdk_finding("CVE-2026-11147", None, Some("CRITICAL")));
        let v: serde_json::Value = serde_json::from_str(&f.raw_json).unwrap();
        assert_eq!(v["packageVulnerabilityDetails"]["vendorSeverity"], "CRITICAL");
        assert!(v.get("inspectorScore").is_none());
        assert_eq!(v["packageVulnerabilityDetails"]["vulnerablePackages"].as_array().unwrap().len(), 2);
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
