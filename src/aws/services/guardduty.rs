use crate::aws::client::AwsClients;
use crate::aws::resource::{native_state_label, Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::error::Result;
use crate::event::{Event, LoadProgress};
use async_trait::async_trait;
use aws_sdk_guardduty::types::{
    Condition, CoverageResource, CoverageStatisticsType, Finding, FindingCriteria, GroupByType,
    OrderBy, SortCriteria,
};
use aws_sdk_guardduty::Client as GdClient;
use std::any::Any;
use std::collections::HashMap;
use tokio::sync::mpsc;

/// Findings kept per detector. GuardDuty retains 90 days, and once archived
/// findings are in scope a suppression rule can leave tens of thousands behind
/// a `ListFindings` — far past what a list pane is useful for. Hitting the cap
/// warns rather than truncating silently.
const MAX_FINDINGS: usize = 1000;
/// Coverage rows kept per detector — one per EC2 instance in the account, so a
/// large fleet needs a ceiling.
const MAX_COVERAGE: usize = 2000;
/// Member accounts kept per detector. A large org can run to thousands.
const MAX_MEMBERS: usize = 1000;
/// `GetMemberDetectors` takes up to 50 account ids per call, so member feature
/// enrichment costs one call per this many members rather than one each.
const MEMBER_DETECTOR_CHUNK: usize = 50;
/// EBS malware scans kept per detector, newest first. A busy account scans on
/// every qualifying finding, so the history is unbounded.
const MAX_MALWARE_SCANS: usize = 200;
/// Signals / actors / endpoints / indicators rendered for an attack sequence.
const MAX_SEQUENCE_ITEMS: usize = 25;
/// Process-ancestry entries rendered for a runtime finding.
const MAX_LINEAGE: usize = 20;
/// Rows per "top N" dimension on the Summary tab. `GetFindingsStatistics`
/// caps `maxResults` at 25 for the grouped forms.
const MAX_TOP_STATS: i32 = 10;

/// Severity scope — a variant-cached server-side filter, like WAF's scope and
/// Cost's period. Cycled with `t` in the sub-tab bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GdSeverityScope {
    Critical,
    High,
    Medium,
    All,
}

impl GdSeverityScope {
    /// Minimum numeric severity for the `severity >= N` finding criterion.
    pub fn min_severity(&self) -> i64 {
        match self {
            GdSeverityScope::Critical => 9,
            GdSeverityScope::High => 7,
            GdSeverityScope::Medium => 4,
            GdSeverityScope::All => 1,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            GdSeverityScope::Critical => "Critical",
            GdSeverityScope::High => "High+",
            GdSeverityScope::Medium => "Medium+",
            GdSeverityScope::All => "All",
        }
    }
}

/// GuardDuty service — sub-tabs Findings / Detectors. Findings are the screen
/// that matters (severity-ranked, split detail pane); Detectors is a thin status
/// tab. Findings are filtered to active (unarchived) + severity ≥ the current
/// scope. The full finding lives in memory, so every detail section is eager.
pub struct GuardDutyService {
    client: GdClient,
    scope: GdSeverityScope,
}

impl GuardDutyService {
    pub fn new(aws_clients: &AwsClients, scope: GdSeverityScope) -> Self {
        Self {
            client: aws_clients.guardduty_client(),
            scope,
        }
    }
}

#[async_trait]
impl AwsService for GuardDutyService {
    fn service_type(&self) -> ServiceType {
        ServiceType::GuardDuty
    }

    fn name(&self) -> &str {
        "GuardDuty"
    }

    async fn list_resources(&self) -> Result<Vec<Box<dyn Resource>>> {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        self.list_resources_streaming(tx, ServiceType::GuardDuty)
            .await?;
        Ok(vec![])
    }

    async fn list_resources_streaming(
        &self,
        event_tx: mpsc::UnboundedSender<Event>,
        service_type: ServiceType,
    ) -> Result<()> {
        // ── Phase 0: detectors (every finding call needs a detector id) ──────
        let detector_ids = match self.client.list_detectors().send().await {
            Ok(resp) => resp.detector_ids().to_vec(),
            Err(e) => {
                let _ = event_tx.send(Event::ResourceLoadError {
                    service: service_type,
                    error: friendly_error(&crate::error::sdk_error_message(&e)),
                });
                return Ok(());
            }
        };

        if detector_ids.is_empty() {
            let _ = event_tx.send(Event::ResourceLoadError {
                service: service_type,
                error: "GuardDuty isn't enabled in this region.".to_string(),
            });
            return Ok(());
        }

        // Emit the detector(s) first (thin status tab). The coverage rollup is a
        // second best-effort call per detector — it answers "how much of the
        // fleet is the runtime agent actually on", which belongs on the
        // detector's own overview even though the per-resource rows are their
        // own sub-tab.
        let mut total = 0usize;
        let mut detectors: Vec<Box<dyn Resource>> = Vec::new();
        for id in &detector_ids {
            if let Ok(d) = self.client.get_detector().detector_id(id).send().await {
                let coverage = self
                    .client
                    .get_coverage_statistics()
                    .detector_id(id)
                    .statistics_type(CoverageStatisticsType::CountByCoverageStatus)
                    .statistics_type(CoverageStatisticsType::CountByResourceType)
                    .send()
                    .await
                    .ok()
                    .and_then(|r| r.coverage_statistics().cloned());
                let org_rows = fetch_org_rows(&self.client, id).await;
                detectors.push(Box::new(GdDetector::from_sdk(
                    id,
                    &d,
                    coverage.as_ref(),
                    org_rows,
                )) as Box<dyn Resource>);
            }
        }
        if !detectors.is_empty() {
            total += detectors.len();
            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                service: service_type,
                resources: detectors,
                progress: LoadProgress {
                    loaded_count: total,
                    total_count: None,
                    status_message: Some("Loading findings…".to_string()),
                },
            });
        }

        // ── Phase 1: summary per detector ────────────────────────────────────
        // Five small non-paginated calls, so this runs *before* the findings
        // pagination: it delays the default tab by a fraction of a second and
        // means the Summary is ready the moment you press 2. Deliberately
        // unfiltered — the whole point of the tab is the true picture, not the
        // picture through the active severity scope.
        for detector_id in &detector_ids {
            let overview = fetch_overview(&self.client, detector_id).await;
            total += 1;
            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                service: service_type,
                resources: vec![Box::new(overview) as Box<dyn Resource>],
                progress: LoadProgress {
                    loaded_count: total,
                    total_count: None,
                    status_message: Some("Loading findings…".to_string()),
                },
            });
        }

        // ── Phase 2: findings per detector ───────────────────────────────────
        //
        // Archived findings are deliberately *not* filtered out server-side:
        // suppression rules archive findings, and "why am I not seeing this"
        // is only answerable if the suppressed ones are visible. They're
        // `is_noise()` instead, so `a` hides them.
        let min_sev = self.scope.min_severity();
        for detector_id in &detector_ids {
            // Phase 1: list finding ids (server-side filter: sev ≥ N), ordered
            // most-severe first.
            let criteria = FindingCriteria::builder()
                .criterion(
                    "severity",
                    Condition::builder().greater_than_or_equal(min_sev).build(),
                )
                .build();
            let sort = SortCriteria::builder()
                .attribute_name("severity")
                .order_by(OrderBy::Desc)
                .build();

            // Each ListFindings page (≤50 ids) is hydrated and emitted immediately
            // (50 is also the GetFindings cap), so findings stream in and a slow
            // account still shows progress instead of a blank spinner.
            let mut token: Option<String> = None;
            let mut kept = 0usize;
            loop {
                let mut req = self
                    .client
                    .list_findings()
                    .detector_id(detector_id)
                    .finding_criteria(criteria.clone())
                    .sort_criteria(sort.clone())
                    .max_results(50);
                if let Some(t) = &token {
                    req = req.next_token(t);
                }
                let page = match req.send().await {
                    Ok(e) => e,
                    Err(e) => {
                        // A page failure mid-stream must NOT be fatal: a
                        // `ResourceLoadError` clears `loading`, and every batch
                        // already queued behind it is then dropped.
                        let _ = event_tx.send(Event::ResourceLoadWarning {
                            service: service_type,
                            warning: format!(
                                "findings: {}",
                                crate::error::sdk_error_message(&e)
                            ),
                        });
                        break;
                    }
                };

                let ids: Vec<String> = page.finding_ids().to_vec();
                if !ids.is_empty() {
                    match self
                        .client
                        .get_findings()
                        .detector_id(detector_id)
                        .set_finding_ids(Some(ids))
                        .send()
                        .await
                    {
                        Ok(resp) => {
                            let mut batch: Vec<GdFinding> = resp
                                .findings()
                                .iter()
                                .map(|f| GdFinding::from_sdk(f, detector_id))
                                .collect();
                            // GetFindings may reorder relative to the id list; re-sort.
                            batch.sort_by(|a, b| {
                                b.severity_score
                                    .partial_cmp(&a.severity_score)
                                    .unwrap_or(std::cmp::Ordering::Equal)
                            });
                            if !batch.is_empty() {
                                kept += batch.len();
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
                        }
                        Err(e) => {
                            let _ = event_tx.send(Event::ResourceLoadWarning {
                                service: service_type,
                                warning: format!(
                                    "finding details: {}",
                                    crate::error::sdk_error_message(&e)
                                ),
                            });
                        }
                    }
                }

                if kept >= MAX_FINDINGS {
                    let _ = event_tx.send(Event::ResourceLoadWarning {
                        service: service_type,
                        warning: format!(
                            "showing the {} most severe findings — narrow the severity scope (t) to see further",
                            MAX_FINDINGS
                        ),
                    });
                    break;
                }

                // Stop when the token is absent, empty, or non-advancing — some
                // APIs return an empty string rather than null on the last page,
                // and feeding it back restarts pagination → an infinite loop.
                token = crate::aws::pagination::next_page_token(page.next_token(), &token);
                if token.is_none() {
                    break;
                }
            }
        }

        // ── Phase 4: runtime coverage (best-effort) ──────────────────────────
        // "Is GuardDuty actually watching this host" — one paginated call per
        // detector. A permission gap warns; the findings above still stand.
        for detector_id in &detector_ids {
            // No `max_results`: GuardDuty caps ListCoverage well below the 100
            // that seemed a reasonable page size, and reports an over-range
            // value as "maxResults value should be greater than 0" — a message
            // that names the parameter but not the actual problem. The service
            // default is fine here; `MAX_COVERAGE` bounds the total either way.
            let mut pages = self
                .client
                .list_coverage()
                .detector_id(detector_id)
                .into_paginator()
                .send();
            let mut kept = 0usize;
            loop {
                let page = match pages.next().await {
                    Some(Ok(p)) => p,
                    Some(Err(e)) => {
                        let _ = event_tx.send(Event::ResourceLoadWarning {
                            service: service_type,
                            warning: format!("coverage: {}", crate::error::sdk_error_message(&e)),
                        });
                        break;
                    }
                    None => break,
                };
                let batch: Vec<Box<dyn Resource>> = page
                    .resources()
                    .iter()
                    .map(|c| Box::new(GdCoverage::from_sdk(c)) as Box<dyn Resource>)
                    .collect();
                if !batch.is_empty() {
                    kept += batch.len();
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
                if kept >= MAX_COVERAGE {
                    let _ = event_tx.send(Event::ResourceLoadWarning {
                        service: service_type,
                        warning: format!("coverage capped at {} resources", MAX_COVERAGE),
                    });
                    break;
                }
            }
        }

        // ── Phase 5: filters + lists (best-effort) ───────────────────────────
        for detector_id in &detector_ids {
            let filters = fetch_filters(&self.client, detector_id, &event_tx, service_type).await;
            if !filters.is_empty() {
                total += filters.len();
                let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                    service: service_type,
                    resources: filters,
                    progress: LoadProgress {
                        loaded_count: total,
                        total_count: None,
                        status_message: None,
                    },
                });
            }

            let lists = fetch_lists(&self.client, detector_id, &event_tx, service_type).await;
            if !lists.is_empty() {
                total += lists.len();
                let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                    service: service_type,
                    resources: lists,
                    progress: LoadProgress {
                        loaded_count: total,
                        total_count: None,
                        status_message: None,
                    },
                });
            }
        }

        // ── Phase 6: member accounts (administrator-only, silent otherwise) ──
        for detector_id in &detector_ids {
            let members = fetch_members(&self.client, detector_id).await;
            if !members.is_empty() {
                total += members.len();
                let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                    service: service_type,
                    resources: members,
                    progress: LoadProgress {
                        loaded_count: total,
                        total_count: None,
                        status_message: None,
                    },
                });
            }
        }

        // ── Phase 7: malware scans + S3 protection plans (silent) ────────────
        for detector_id in &detector_ids {
            let malware = fetch_malware(&self.client, detector_id).await;
            if !malware.is_empty() {
                total += malware.len();
                let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                    service: service_type,
                    resources: malware,
                    progress: LoadProgress {
                        loaded_count: total,
                        total_count: None,
                        status_message: None,
                    },
                });
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

/// Classify the not-enabled / access-denied errors into a friendly hint.
fn friendly_error(raw: &str) -> String {
    let low = raw.to_lowercase();
    if low.contains("accessdenied") || low.contains("not authorized") {
        "Access denied. GuardDuty read permissions are required (guardduty:ListDetectors / GetFindings).".to_string()
    } else if low.contains("badrequest") || low.contains("not enabled") {
        "GuardDuty isn't enabled in this region.".to_string()
    } else {
        format!("Failed to load GuardDuty: {}", raw)
    }
}

// ── GdDetector ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct GdDetector {
    pub id: String,
    pub status: String,
    pub finding_publishing_frequency: String,
    pub features: Vec<String>,
    pub service_role: String,
    pub created: Option<String>,
    /// Runtime-coverage rollup from `GetCoverageStatistics` — `(label, count)`
    /// by status then by resource type. Empty when the call failed or Runtime
    /// Monitoring was never enabled.
    pub coverage_rows: Vec<(String, String)>,
    /// Org posture — administrator, delegated admin, member auto-enable.
    /// Empty in a standalone account, which answers none of those calls.
    pub org_rows: Vec<(String, String)>,
    pub tags: HashMap<String, String>,
}

impl GdDetector {
    fn from_sdk(
        id: &str,
        d: &aws_sdk_guardduty::operation::get_detector::GetDetectorOutput,
        coverage: Option<&aws_sdk_guardduty::types::CoverageStatistics>,
        org_rows: Vec<(String, String)>,
    ) -> Self {
        let features = d
            .features()
            .iter()
            .map(|f| {
                let name = f.name().map(|n| n.as_str()).unwrap_or("?");
                let status = f.status().map(|s| s.as_str()).unwrap_or("?");
                format!("{} ({})", name, status)
            })
            .collect();

        // Status first (HEALTHY/UNHEALTHY is the actionable half), then the
        // per-resource-type split.
        let coverage_rows = coverage.map(coverage_stat_rows).unwrap_or_default();

        Self {
            id: id.to_string(),
            status: d.status().map(|s| s.as_str().to_string()).unwrap_or_default(),
            finding_publishing_frequency: d
                .finding_publishing_frequency()
                .map(|f| f.as_str().to_string())
                .unwrap_or_default(),
            features,
            service_role: d.service_role().unwrap_or_default().to_string(),
            created: d.created_at().map(|s| s.to_string()),
            coverage_rows,
            org_rows,
            tags: d
                .tags()
                .map(|t| t.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                .unwrap_or_default(),
        }
    }
}

crate::sections! {
    pub enum GdDetectorDetailSection,
    pub static GD_DETECTOR_SECTIONS = [
        Overview "Overview",
        Features "Features",
        Tags "Tags",
    ]
}

impl Resource for GdDetector {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&GD_DETECTOR_SECTIONS)
    }

    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        &self.id
    }

    fn resource_type(&self) -> &str {
        "GuardDuty Detector"
    }

    fn state(&self) -> ResourceState {
        match self.status.as_str() {
            "ENABLED" => ResourceState::Available,
            "DISABLED" => ResourceState::Stopped,
            other => ResourceState::Unknown(other.to_string()),
        }
    }
    fn state_label(&self) -> String {
        native_state_label(&self.status, || self.state())
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!("{} detector {}", self.id, self.status)
    }

    fn details(&self) -> Vec<(String, String)> {
        let mut rows = vec![
            ("Detector ID".to_string(), self.id.clone()),
            ("Status".to_string(), self.status.clone()),
            (
                "Publish Frequency".to_string(),
                self.finding_publishing_frequency.clone(),
            ),
            (
                "Created".to_string(),
                self.created.clone().unwrap_or_else(|| "—".to_string()),
            ),
        ];
        if !self.service_role.is_empty() {
            rows.push(("Service Role".to_string(), self.service_role.clone()));
        }
        if !self.features.is_empty() {
            rows.push((String::new(), String::new()));
            rows.push(("Features".to_string(), String::new()));
            for f in &self.features {
                rows.push((format!("  {}", f), String::new()));
            }
        }
        rows
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/guardduty/home?region={}#/settings",
            region, region
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── GdFinding ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct GdFinding {
    pub id: String,
    #[allow(dead_code)]
    pub arn: String,
    #[allow(dead_code)]
    pub detector_id: String,
    pub title: String,
    pub finding_type: String,
    pub severity_score: f64,
    pub severity_label: String,
    pub region: String,
    pub account_id: String,
    pub resource_type: String,
    pub resource_summary: String,
    pub resource_rows: Vec<(String, String)>,
    pub actor_label: String,
    pub actor_rows: Vec<(String, String)>,
    pub count: i32,
    pub first_seen: Option<String>,
    pub last_seen: Option<String>,
    pub description: String,
    /// Suppressed (or manually archived). Archived findings are loaded — a
    /// suppression rule silently hiding a real finding is exactly what you
    /// want to be able to see — and marked `is_noise()` so `a` hides them.
    pub archived: bool,
    /// `TARGET` (the resource was acted upon) or `ACTOR` (it did the acting).
    /// Materially changes how the finding reads, and the console shows it.
    pub resource_role: String,
    pub feature_name: String,
    /// Analyst feedback submitted through the console (`USEFUL`/`NOT_USEFUL`).
    pub user_feedback: String,
    /// `service.additionalInfo` as `(type, value)` — free-form context the
    /// detector attaches, e.g. a sample-finding marker.
    pub additional_info: Vec<(String, String)>,
    /// `evidence.threatIntelligenceDetails` as `(list name, threat names)`.
    pub threat_intel: Vec<(String, String)>,
    /// `service.runtimeDetails` flattened — process, ancestry and the
    /// activity-specific context block. Empty for non-runtime findings.
    pub runtime_rows: Vec<(String, String)>,
    /// `service.detection.sequence` flattened — the Extended Threat Detection
    /// attack sequence: signals, actors, endpoints, indicators. Empty unless
    /// this is an `AttackSequence` finding.
    pub sequence_rows: Vec<(String, String)>,
    pub updated_at: Option<String>,
    /// Precomputed haystack for `search_text()` — every id, IP and domain the
    /// finding mentions, so "who touched 1.2.3.4" is a plain fuzzy search.
    /// Built once because `search_text` runs per resource per keystroke.
    pub search_blob: String,
    pub raw_json: String,
}

impl GdFinding {
    fn from_sdk(f: &Finding, detector_id: &str) -> Self {
        let severity_score = f.severity().unwrap_or(0.0);
        let severity_label = severity_label(severity_score).to_string();

        let (resource_type, resource_summary, resource_rows) = extract_resource(f.resource());
        let (actor_label, actor_rows) = extract_actor(f.service());

        let svc = f.service();
        let count = svc.and_then(|s| s.count()).unwrap_or(0);
        let first_seen = svc
            .and_then(|s| s.event_first_seen())
            .map(|s| s.to_string());
        let last_seen = svc.and_then(|s| s.event_last_seen()).map(|s| s.to_string());

        let archived = svc.and_then(|s| s.archived()).unwrap_or(false);
        let resource_role = svc.and_then(|s| s.resource_role()).unwrap_or("").to_string();
        let feature_name = svc.and_then(|s| s.feature_name()).unwrap_or("").to_string();
        let user_feedback = svc.and_then(|s| s.user_feedback()).unwrap_or("").to_string();

        let additional_info = svc
            .and_then(|s| s.additional_info())
            .map(|a| {
                let kind = a.r#type().unwrap_or("Info").to_string();
                let value = a.value().unwrap_or_default().to_string();
                if value.is_empty() {
                    Vec::new()
                } else {
                    vec![(kind, value)]
                }
            })
            .unwrap_or_default();

        let threat_intel = f
            .service()
            .and_then(|s| s.evidence())
            .map(|e| {
                e.threat_intelligence_details()
                    .iter()
                    .map(|t| {
                        (
                            t.threat_list_name().unwrap_or("Threat list").to_string(),
                            t.threat_names().join(", "),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();

        let runtime_rows = extract_runtime(svc.and_then(|s| s.runtime_details()));
        let sequence_rows =
            extract_sequence(svc.and_then(|s| s.detection()).and_then(|d| d.sequence()));

        let title = f.title().unwrap_or_default().to_string();
        let finding_type = f.r#type().unwrap_or_default().to_string();
        let region = f.region().unwrap_or_default().to_string();
        let account_id = f.account_id().unwrap_or_default().to_string();
        let description = f.description().unwrap_or_default().to_string();

        // Every value from the extracted rows goes into the haystack, so an id,
        // IP, domain, bucket or access key anywhere in the finding is
        // fuzzy-searchable without knowing which section it lives in.
        let mut search_blob = format!(
            "{} {} {} {} {} {} {}",
            title, finding_type, severity_label, resource_type, resource_summary, actor_label,
            account_id,
        );
        for (k, v) in resource_rows
            .iter()
            .chain(actor_rows.iter())
            .chain(runtime_rows.iter())
            .chain(sequence_rows.iter())
        {
            if !v.is_empty() {
                search_blob.push(' ');
                search_blob.push_str(v);
            } else if k.trim_start().starts_with(|c: char| c.is_ascii_alphanumeric()) {
                // Content lines (indented, no value) carry the signal/lineage
                // text — the key *is* the content there.
                search_blob.push(' ');
                search_blob.push_str(k.trim());
            }
        }

        let mut out = Self {
            id: f.id().unwrap_or_default().to_string(),
            arn: f.arn().unwrap_or_default().to_string(),
            detector_id: detector_id.to_string(),
            title,
            finding_type,
            severity_score,
            severity_label,
            region,
            account_id,
            resource_type,
            resource_summary,
            resource_rows,
            actor_label,
            actor_rows,
            count,
            first_seen,
            last_seen,
            description,
            archived,
            resource_role,
            feature_name,
            user_feedback,
            additional_info,
            threat_intel,
            runtime_rows,
            sequence_rows,
            updated_at: f.updated_at().map(|s| s.to_string()),
            search_blob,
            raw_json: String::new(),
        };
        // Built last so the JSON view reflects the assembled resource rather
        // than a parallel set of arguments that can drift out of step.
        out.raw_json = build_raw_json(f, &out);
        out
    }
}

/// Split a GuardDuty finding type into its documented parts:
/// `ThreatPurpose:ResourceTypeAffected/ThreatFamilyName.DetectionMechanism!Artifact`.
/// Only the parts actually present are returned, so a short type like
/// `Recon:EC2/PortProbeUnprotectedPort` yields three rows rather than five.
pub fn decompose_finding_type(t: &str) -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = Vec::new();
    let (purpose, rest) = match t.split_once(':') {
        Some((p, r)) => (p, r),
        None => return rows,
    };
    rows.push(("Threat Purpose".to_string(), purpose.to_string()));

    let (affected, rest) = match rest.split_once('/') {
        Some((a, r)) => (a, r),
        None => (rest, ""),
    };
    if !affected.is_empty() {
        rows.push(("Resource Affected".to_string(), affected.to_string()));
    }
    if rest.is_empty() {
        return rows;
    }

    let (family_part, artifact) = match rest.split_once('!') {
        Some((f, a)) => (f, a),
        None => (rest, ""),
    };
    let (family, mechanism) = match family_part.split_once('.') {
        Some((f, m)) => (f, m),
        None => (family_part, ""),
    };
    if !family.is_empty() {
        rows.push(("Threat Family".to_string(), family.to_string()));
    }
    if !mechanism.is_empty() {
        rows.push(("Detection Mechanism".to_string(), mechanism.to_string()));
    }
    if !artifact.is_empty() {
        rows.push(("Artifact".to_string(), artifact.to_string()));
    }
    rows
}

crate::sections! {
    pub enum GdFindingDetailSection,
    pub static GD_FINDING_SECTIONS = [
        Details "Details",
        Resource "Resource",
        Actor "Actor",
        Sequence "Sequence",
        Runtime "Runtime",
        Remediation "Remediation",
    ]
}

impl Resource for GdFinding {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&GD_FINDING_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        if self.title.is_empty() {
            &self.finding_type
        } else {
            &self.title
        }
    }

    fn resource_type(&self) -> &str {
        "GuardDuty Finding"
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
        self.search_blob.clone()
    }

    /// Suppressed findings stay in the list (a suppression rule quietly hiding
    /// something real is worth seeing) but `a` folds them away.
    fn is_noise(&self) -> bool {
        self.archived
    }

    /// CloudTrail indexes events by the resource they touched, not by a
    /// GuardDuty finding id — so `W` searches the affected resource first and
    /// only falls back to the finding id.
    fn trail_lookup_keys(&self) -> Vec<String> {
        let mut keys = Vec::new();
        if !self.resource_summary.is_empty() {
            keys.push(self.resource_summary.clone());
        }
        keys.push(self.id.clone());
        keys
    }

    fn details(&self) -> Vec<(String, String)> {
        // Fallback only — the split pane (gd_finding_section_lines) is the real view.
        vec![
            ("Title".to_string(), self.title.clone()),
            ("Type".to_string(), self.finding_type.clone()),
            (
                "Severity".to_string(),
                format!("{} ({:.1})", self.severity_label, self.severity_score),
            ),
        ]
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/guardduty/home?region={}#/findings?search=id%3D{}",
            region, region, self.id
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

// ── Severity helpers ────────────────────────────────────────────────────────

/// Bucket a GuardDuty numeric severity (1.0–8.9, newer schemas to ~10) to a label.
pub fn severity_label(score: f64) -> &'static str {
    if score >= 9.0 {
        "Critical"
    } else if score >= 7.0 {
        "High"
    } else if score >= 4.0 {
        "Medium"
    } else {
        "Low"
    }
}

/// Map a severity label to a list-pane status dot (mirrors findings.md).
pub fn severity_to_state(label: &str) -> ResourceState {
    match label {
        "Critical" | "High" => ResourceState::Unavailable,
        "Medium" => ResourceState::Pending,
        _ => ResourceState::Unknown(String::new()),
    }
}

// ── Resource extraction (varies by GuardDuty resource type) ──────────────────

/// Flatten every resource shape the finding carries — not just the first.
/// A single finding routinely populates several: an EKS runtime finding has
/// cluster **and** Kubernetes workload **and** container details, and a malware
/// finding has the instance **and** the scanned volumes. Early-returning on the
/// first match (as this once did) threw the rest away.
///
/// Returns `(resource type, one-line summary, rows)`; the summary is the first
/// identifier any shape supplies, and is what the list row and the CloudTrail
/// lens key off.
fn extract_resource(
    res: Option<&aws_sdk_guardduty::types::Resource>,
) -> (String, String, Vec<(String, String)>) {
    let Some(res) = res else {
        return ("Unknown".to_string(), String::new(), Vec::new());
    };
    let rtype = res.resource_type().unwrap_or("Unknown").to_string();
    let mut rows: Vec<(String, String)> = Vec::new();
    let mut summary = String::new();

    if let Some(inst) = res.instance_details() {
        set_if_empty(&mut summary, inst.instance_id());
        group(&mut rows, "EC2 Instance");
        push_row(&mut rows, "Instance ID", inst.instance_id());
        push_row(&mut rows, "Instance Type", inst.instance_type());
        push_row(&mut rows, "Image ID", inst.image_id());
        push_row(&mut rows, "Availability Zone", inst.availability_zone());
        push_row(&mut rows, "State", inst.instance_state());
        push_row(&mut rows, "Platform", inst.platform());
        if let Some(prof) = inst.iam_instance_profile().and_then(|p| p.arn()) {
            rows.push(("IAM Instance Profile".to_string(), prof.to_string()));
        }
        for ni in inst.network_interfaces() {
            push_row(&mut rows, "Private IP", ni.private_ip_address());
            push_row(&mut rows, "Public IP", ni.public_ip());
            push_row(&mut rows, "VPC", ni.vpc_id());
            push_row(&mut rows, "Subnet", ni.subnet_id());
            for sg in ni.security_groups() {
                if let Some(id) = sg.group_id() {
                    rows.push((
                        "Security Group".to_string(),
                        match sg.group_name() {
                            Some(n) if !n.is_empty() => format!("{} ({})", id, n),
                            _ => id.to_string(),
                        },
                    ));
                }
            }
        }
    }

    if let Some(ak) = res.access_key_details() {
        set_if_empty(&mut summary, ak.user_name().or(ak.access_key_id()));
        group(&mut rows, "IAM Principal");
        push_row(&mut rows, "Access Key ID", ak.access_key_id());
        push_row(&mut rows, "User Name", ak.user_name());
        push_row(&mut rows, "User Type", ak.user_type());
        push_row(&mut rows, "Principal ID", ak.principal_id());
    }

    let buckets = res.s3_bucket_details();
    if !buckets.is_empty() {
        set_if_empty(&mut summary, buckets.first().and_then(|b| b.name()));
        group(&mut rows, "S3");
        for b in buckets {
            push_row(&mut rows, "Bucket", b.name());
            push_row(&mut rows, "Owner", b.owner().and_then(|o| o.id()));
            if let Some(pa) = b.public_access() {
                if let Some(p) = pa.effective_permission() {
                    if !p.is_empty() {
                        rows.push(("Public Access".to_string(), p.to_string()));
                    }
                }
            }
            if let Some(enc) = b.default_server_side_encryption() {
                push_row(&mut rows, "Encryption", enc.encryption_type());
            }
        }
    }

    if let Some(eks) = res.eks_cluster_details() {
        set_if_empty(&mut summary, eks.name());
        group(&mut rows, "EKS Cluster");
        push_row(&mut rows, "EKS Cluster", eks.name());
        push_row(&mut rows, "ARN", eks.arn());
        push_row(&mut rows, "Status", eks.status());
        push_row(&mut rows, "VPC", eks.vpc_id());
    }

    if let Some(k8s) = res.kubernetes_details() {
        if let Some(u) = k8s.kubernetes_user_details() {
            set_if_empty(&mut summary, u.username());
            group(&mut rows, "Kubernetes User");
            push_row(&mut rows, "Username", u.username());
            push_row(&mut rows, "UID", u.uid());
            if !u.groups().is_empty() {
                rows.push(("Groups".to_string(), u.groups().join(", ")));
            }
            if let Some(imp) = u.impersonated_user() {
                push_row(&mut rows, "Impersonating", imp.username());
            }
        }
        if let Some(w) = k8s.kubernetes_workload_details() {
            set_if_empty(&mut summary, w.name());
            group(&mut rows, "Kubernetes Workload");
            push_row(&mut rows, "Workload", w.name());
            push_row(&mut rows, "Namespace", w.namespace());
            push_row(&mut rows, "Service Account", w.service_account_name());
            // Host namespace sharing is the container-escape signal — always
            // shown, including the reassuring `false`.
            if let Some(v) = w.host_network() {
                rows.push(("Host Network".to_string(), v.to_string()));
            }
            if let Some(v) = w.host_pid() {
                rows.push(("Host PID".to_string(), v.to_string()));
            }
            if let Some(v) = w.host_ipc() {
                rows.push(("Host IPC".to_string(), v.to_string()));
            }
            for c in w.containers() {
                push_container_rows(&mut rows, c);
            }
        }
    }

    if let Some(c) = res.container_details() {
        set_if_empty(&mut summary, c.name().or(c.id()));
        group(&mut rows, "Container");
        push_container_rows(&mut rows, c);
    }

    if let Some(ecs) = res.ecs_cluster_details() {
        set_if_empty(&mut summary, ecs.name());
        group(&mut rows, "ECS Cluster");
        push_row(&mut rows, "Cluster", ecs.name());
        push_row(&mut rows, "ARN", ecs.arn());
        push_row(&mut rows, "Status", ecs.status());
        if let Some(t) = ecs.task_details() {
            push_row(&mut rows, "Task", t.arn());
            push_row(&mut rows, "Task Definition", t.definition_arn());
            push_row(&mut rows, "Launch Type", t.launch_type());
            push_row(&mut rows, "Started By", t.started_by());
            for c in t.containers() {
                push_container_rows(&mut rows, c);
            }
        }
    }

    if let Some(lambda) = res.lambda_details() {
        set_if_empty(&mut summary, lambda.function_name());
        group(&mut rows, "Lambda");
        push_row(&mut rows, "Function", lambda.function_name());
        push_row(&mut rows, "ARN", lambda.function_arn());
        push_row(&mut rows, "Revision", lambda.revision_id());
        push_row(&mut rows, "Role", lambda.role());
        if let Some(v) = lambda.vpc_config() {
            for s in v.subnet_ids() {
                rows.push(("Subnet".to_string(), s.clone()));
            }
            for g in v.security_groups() {
                push_row(&mut rows, "Security Group", g.group_id());
            }
        }
    }

    if let Some(rds) = res.rds_db_instance_details() {
        set_if_empty(&mut summary, rds.db_instance_identifier());
        group(&mut rows, "RDS Instance");
        push_row(&mut rows, "DB Instance", rds.db_instance_identifier());
        push_row(&mut rows, "Engine", rds.engine());
        push_row(&mut rows, "Engine Version", rds.engine_version());
        push_row(&mut rows, "Cluster", rds.db_cluster_identifier());
    }

    if let Some(rds) = res.rds_limitless_db_details() {
        set_if_empty(&mut summary, rds.db_shard_group_identifier());
        group(&mut rows, "RDS Limitless");
        push_row(&mut rows, "Shard Group", rds.db_shard_group_identifier());
        push_row(&mut rows, "Engine", rds.engine());
        push_row(&mut rows, "Engine Version", rds.engine_version());
        push_row(&mut rows, "Cluster", rds.db_cluster_identifier());
    }

    if let Some(u) = res.rds_db_user_details() {
        group(&mut rows, "RDS Login");
        push_row(&mut rows, "DB User", u.user());
        push_row(&mut rows, "Database", u.database());
        push_row(&mut rows, "Application", u.application());
        push_row(&mut rows, "Auth Method", u.auth_method());
        push_row(&mut rows, "SSL", u.ssl());
    }

    if let Some(ebs) = res.ebs_volume_details() {
        group(&mut rows, "Scanned Volumes");
        for v in ebs.scanned_volume_details() {
            push_volume_rows(&mut rows, v, "Scanned");
        }
        for v in ebs.skipped_volume_details() {
            push_volume_rows(&mut rows, v, "Skipped");
        }
    }

    if let Some(s) = res.ebs_snapshot_details() {
        set_if_empty(&mut summary, s.snapshot_arn());
        group(&mut rows, "EBS Snapshot");
        push_row(&mut rows, "Snapshot", s.snapshot_arn());
    }

    if let Some(i) = res.ec2_image_details() {
        set_if_empty(&mut summary, i.image_arn());
        group(&mut rows, "EC2 Image");
        push_row(&mut rows, "Image", i.image_arn());
    }

    if let Some(r) = res.recovery_point_details() {
        set_if_empty(&mut summary, r.recovery_point_arn());
        group(&mut rows, "Backup Recovery Point");
        push_row(&mut rows, "Recovery Point", r.recovery_point_arn());
        push_row(&mut rows, "Backup Vault", r.backup_vault_name());
    }

    (rtype, summary, rows)
}

/// Container rows, shared by the standalone, ECS-task and Kubernetes-workload
/// container shapes.
fn push_container_rows(rows: &mut Vec<(String, String)>, c: &aws_sdk_guardduty::types::Container) {
    push_row(rows, "Container", c.name().or(c.id()));
    push_row(rows, "Image", c.image());
    push_row(rows, "Runtime", c.container_runtime());
    if let Some(sc) = c.security_context() {
        // Both are escape-relevant, so `false` is worth stating explicitly.
        if let Some(v) = sc.privileged() {
            rows.push(("Privileged".to_string(), v.to_string()));
        }
        if let Some(v) = sc.allow_privilege_escalation() {
            rows.push(("Allow Priv Escalation".to_string(), v.to_string()));
        }
    }
    for m in c.volume_mounts() {
        if let Some(path) = m.mount_path() {
            rows.push((
                "Mount".to_string(),
                match m.name() {
                    Some(n) if !n.is_empty() => format!("{} → {}", n, path),
                    _ => path.to_string(),
                },
            ));
        }
    }
}

fn push_volume_rows(
    rows: &mut Vec<(String, String)>,
    v: &aws_sdk_guardduty::types::VolumeDetail,
    disposition: &str,
) {
    let label = match (v.device_name(), v.volume_size_in_gb()) {
        (Some(d), Some(gb)) => format!("{} · {} GiB · {}", d, gb, disposition),
        (Some(d), None) => format!("{} · {}", d, disposition),
        (None, Some(gb)) => format!("{} GiB · {}", gb, disposition),
        (None, None) => disposition.to_string(),
    };
    if let Some(arn) = v.volume_arn() {
        rows.push((label, arn.to_string()));
    } else {
        // No ARN → render as a content line, not a key-value row: a non-empty
        // key with an empty value and no leading space is `style_detail_row`'s
        // *group header*, which this is not.
        rows.push((format!("  {}", label), String::new()));
    }
    push_row(rows, "Encryption", v.encryption_type());
    push_row(rows, "KMS Key", v.kms_key_arn());
}

/// Set `dst` only if it's still empty and `v` is a non-empty value — the
/// "first shape to supply an identifier wins" rule for the resource summary.
fn set_if_empty(dst: &mut String, v: Option<&str>) {
    if dst.is_empty() {
        if let Some(v) = v {
            if !v.is_empty() {
                *dst = v.to_string();
            }
        }
    }
}

/// Push a blank spacer + group header (`style_detail_row`'s magenta subsection
/// label: non-empty key, empty value, no leading space).
fn group(rows: &mut Vec<(String, String)>, label: &str) {
    if !rows.is_empty() {
        rows.push((String::new(), String::new()));
    }
    rows.push((label.to_string(), String::new()));
}

// ── Actor extraction (the "who", from service.action) ────────────────────────

fn extract_actor(
    service: Option<&aws_sdk_guardduty::types::Service>,
) -> (String, Vec<(String, String)>) {
    let Some(action) = service.and_then(|s| s.action()) else {
        return (String::new(), Vec::new());
    };
    let mut rows: Vec<(String, String)> = Vec::new();
    let action_type = action.action_type().unwrap_or_default().to_string();
    if !action_type.is_empty() {
        rows.push(("Action Type".to_string(), action_type.clone()));
    }
    let mut label = String::new();

    if let Some(api) = action.aws_api_call_action() {
        push_row(&mut rows, "API", api.api());
        push_row(&mut rows, "Service", api.service_name());
        push_row(&mut rows, "Caller Type", api.caller_type());
        push_row(&mut rows, "Error Code", api.error_code());
        label = remote_ip_rows(&mut rows, api.remote_ip_details())
            .or_else(|| api.api().map(|s| s.to_string()))
            .unwrap_or_default();
    } else if let Some(nc) = action.network_connection_action() {
        push_row(&mut rows, "Direction", nc.connection_direction());
        push_row(&mut rows, "Protocol", nc.protocol());
        if let Some(blocked) = nc.blocked() {
            rows.push(("Blocked".to_string(), blocked.to_string()));
        }
        if let Some(lp) = nc.local_port_details().and_then(|p| p.port()) {
            rows.push(("Local Port".to_string(), lp.to_string()));
        }
        if let Some(rp) = nc.remote_port_details().and_then(|p| p.port()) {
            rows.push(("Remote Port".to_string(), rp.to_string()));
        }
        label = remote_ip_rows(&mut rows, nc.remote_ip_details()).unwrap_or_default();
    } else if let Some(dns) = action.dns_request_action() {
        push_row(&mut rows, "Domain", dns.domain());
        push_row(&mut rows, "Protocol", dns.protocol());
        if let Some(blocked) = dns.blocked() {
            rows.push(("Blocked".to_string(), blocked.to_string()));
        }
        label = dns.domain().unwrap_or_default().to_string();
    } else if let Some(pp) = action.port_probe_action() {
        if let Some(blocked) = pp.blocked() {
            rows.push(("Blocked".to_string(), blocked.to_string()));
        }
        if let Some(first) = pp.port_probe_details().first() {
            if let Some(port) = first.local_port_details().and_then(|p| p.port()) {
                rows.push(("Probed Port".to_string(), port.to_string()));
            }
            label = remote_ip_rows(&mut rows, first.remote_ip_details()).unwrap_or_default();
        }
    } else if let Some(rds) = action.rds_login_attempt_action() {
        if let Some(first) = rds.login_attributes().first() {
            push_row(&mut rows, "DB User", first.user());
            if let Some(app) = first.application() {
                rows.push(("Application".to_string(), app.to_string()));
            }
        }
        label = remote_ip_rows(&mut rows, rds.remote_ip_details()).unwrap_or_default();
    } else if let Some(k) = action.kubernetes_api_call_action() {
        push_row(&mut rows, "Verb", k.verb());
        push_row(&mut rows, "Request URI", k.request_uri());
        push_row(&mut rows, "K8s Resource", k.resource());
        push_row(&mut rows, "Subresource", k.subresource());
        push_row(&mut rows, "Namespace", k.namespace());
        push_row(&mut rows, "Resource Name", k.resource_name());
        if let Some(code) = k.status_code() {
            rows.push(("Status Code".to_string(), code.to_string()));
        }
        push_row(&mut rows, "User Agent", k.user_agent());
        for ip in k.source_ips() {
            rows.push(("Source IP".to_string(), ip.clone()));
        }
        label = remote_ip_rows(&mut rows, k.remote_ip_details())
            .or_else(|| k.verb().map(|v| v.to_string()))
            .unwrap_or_default();
    }

    // These three carry no action union of their own — they annotate a
    // Kubernetes RBAC finding alongside whichever action fired.
    if let Some(p) = action.kubernetes_permission_checked_details() {
        group(&mut rows, "Permission Checked");
        push_row(&mut rows, "Verb", p.verb());
        push_row(&mut rows, "Resource", p.resource());
        push_row(&mut rows, "Namespace", p.namespace());
        if let Some(allowed) = p.allowed() {
            rows.push(("Allowed".to_string(), allowed.to_string()));
        }
    }
    if let Some(rb) = action.kubernetes_role_binding_details() {
        group(&mut rows, "Role Binding");
        push_row(&mut rows, "Name", rb.name());
        push_row(&mut rows, "Kind", rb.kind());
        push_row(&mut rows, "UID", rb.uid());
        push_row(&mut rows, "Role Ref Name", rb.role_ref_name());
        push_row(&mut rows, "Role Ref Kind", rb.role_ref_kind());
    }
    if let Some(r) = action.kubernetes_role_details() {
        group(&mut rows, "Role");
        push_row(&mut rows, "Name", r.name());
        push_row(&mut rows, "Kind", r.kind());
        push_row(&mut rows, "UID", r.uid());
    }

    if label.is_empty() {
        label = action_type;
    }
    (label, rows)
}

/// Append remote-IP rows (ip / country / org) and return the IP for the label.
fn remote_ip_rows(
    rows: &mut Vec<(String, String)>,
    ip: Option<&aws_sdk_guardduty::types::RemoteIpDetails>,
) -> Option<String> {
    let ip = ip?;
    let addr = ip.ip_address_v4().map(|s| s.to_string());
    if let Some(a) = &addr {
        rows.push(("Remote IP".to_string(), a.clone()));
    }
    if let Some(country) = ip.country().and_then(|c| c.country_name()) {
        rows.push(("Country".to_string(), country.to_string()));
    }
    if let Some(org) = ip.organization() {
        let org_str = [org.asn(), org.org(), org.isp()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · ");
        if !org_str.is_empty() {
            rows.push(("Organization".to_string(), org_str));
        }
    }
    addr
}

// ── Runtime Monitoring detail (service.runtimeDetails) ──────────────────────

/// Flatten the observed process, its ancestry, and the activity-specific
/// context block. GuardDuty populates only the context fields relevant to the
/// finding type (a `LD_PRELOAD` finding fills `ldPreloadValue`, a container
/// escape fills `mountSource`/`releaseAgentPath`, …), so every field is pushed
/// only when present rather than laid out as a fixed template.
fn extract_runtime(rt: Option<&aws_sdk_guardduty::types::RuntimeDetails>) -> Vec<(String, String)> {
    let Some(rt) = rt else {
        return Vec::new();
    };
    let mut rows: Vec<(String, String)> = Vec::new();

    if let Some(p) = rt.process() {
        rows.push(("Process".to_string(), String::new()));
        push_process_rows(&mut rows, p);

        // Ancestry reads root-first, the direction you follow to find how the
        // process got started.
        let lineage = p.lineage();
        if !lineage.is_empty() {
            rows.push((String::new(), String::new()));
            rows.push(("Ancestry".to_string(), String::new()));
            for l in lineage.iter().take(MAX_LINEAGE) {
                let pid = l.pid().map(|v| v.to_string()).unwrap_or_default();
                let name = l.name().unwrap_or("?");
                let path = l.executable_path().unwrap_or("");
                rows.push((format!("  [{}] {} {}", pid, name, path), String::new()));
            }
            if lineage.len() > MAX_LINEAGE {
                rows.push((
                    format!("  · {} more ancestors", lineage.len() - MAX_LINEAGE),
                    String::new(),
                ));
            }
        }
    }

    if let Some(c) = rt.context() {
        rows.push((String::new(), String::new()));
        rows.push(("Context".to_string(), String::new()));
        push_row(&mut rows, "Tool Name", c.tool_name());
        push_row(&mut rows, "Tool Category", c.tool_category());
        push_row(&mut rows, "Service Name", c.service_name());
        push_row(&mut rows, "Script Path", c.script_path());
        push_row(&mut rows, "Library Path", c.library_path());
        push_row(&mut rows, "LD_PRELOAD", c.ld_preload_value());
        push_row(&mut rows, "Socket Path", c.socket_path());
        push_row(&mut rows, "Runc Binary Path", c.runc_binary_path());
        push_row(&mut rows, "Release Agent Path", c.release_agent_path());
        push_row(&mut rows, "Mount Source", c.mount_source());
        push_row(&mut rows, "Mount Target", c.mount_target());
        push_row(&mut rows, "File System Type", c.file_system_type());
        if !c.flags().is_empty() {
            rows.push(("Flags".to_string(), c.flags().join(", ")));
        }
        push_row(&mut rows, "Module Name", c.module_name());
        push_row(&mut rows, "Module File Path", c.module_file_path());
        push_row(&mut rows, "Shell History", c.shell_history_file_path());
        push_row(&mut rows, "Address Family", c.address_family());
        if let Some(n) = c.iana_protocol_number() {
            rows.push(("IANA Protocol".to_string(), n.to_string()));
        }
        if !c.memory_regions().is_empty() {
            rows.push(("Memory Regions".to_string(), c.memory_regions().join(", ")));
        }
        push_row(&mut rows, "File Operation", c.file_operation());
        push_row(&mut rows, "File Path", c.file_path());
        push_row(&mut rows, "Threat File Path", c.threat_file_path());
        for p in c.related_file_paths() {
            rows.push(("Related File".to_string(), p.clone()));
        }
        push_row(&mut rows, "Command Line", c.command_line_example());
        if let Some(m) = c.modified_at() {
            rows.push(("Modified At".to_string(), m.to_string()));
        }

        if let Some(p) = c.modifying_process() {
            rows.push((String::new(), String::new()));
            rows.push(("Modifying Process".to_string(), String::new()));
            push_process_rows(&mut rows, p);
        }
        if let Some(p) = c.target_process() {
            rows.push((String::new(), String::new()));
            rows.push(("Target Process".to_string(), String::new()));
            push_process_rows(&mut rows, p);
        }
    }

    rows
}

fn push_process_rows(
    rows: &mut Vec<(String, String)>,
    p: &aws_sdk_guardduty::types::ProcessDetails,
) {
    push_row(rows, "Name", p.name());
    push_row(rows, "Executable Path", p.executable_path());
    push_row(rows, "Working Dir", p.pwd());
    if let Some(v) = p.pid() {
        rows.push(("PID".to_string(), v.to_string()));
    }
    if let Some(v) = p.namespace_pid() {
        rows.push(("Namespace PID".to_string(), v.to_string()));
    }
    push_row(rows, "User", p.user());
    if let Some(v) = p.user_id() {
        rows.push(("User ID".to_string(), v.to_string()));
    }
    if let Some(v) = p.euid() {
        // Effective uid 0 on a process that isn't meant to be root is the
        // privilege signal in most runtime findings.
        rows.push((
            "Effective UID".to_string(),
            if v == 0 {
                "0 (root)".to_string()
            } else {
                v.to_string()
            },
        ));
    }
    if let Some(t) = p.start_time() {
        rows.push(("Started".to_string(), t.to_string()));
    }
}

// ── Attack sequence (service.detection.sequence) ────────────────────────────

/// Flatten an Extended Threat Detection attack sequence. This is the finding
/// shape the console gives a whole screen to: a sequence stitches many
/// individually-unremarkable signals into one narrative, so the signals
/// timeline is the payload and everything else is supporting context.
fn extract_sequence(seq: Option<&aws_sdk_guardduty::types::Sequence>) -> Vec<(String, String)> {
    let Some(seq) = seq else {
        return Vec::new();
    };
    let mut rows: Vec<(String, String)> = Vec::new();

    push_row(&mut rows, "Sequence ID", seq.uid());
    if !seq.additional_sequence_types().is_empty() {
        rows.push((
            "Also Classified".to_string(),
            seq.additional_sequence_types().join(", "),
        ));
    }
    if let Some(d) = seq.description() {
        if !d.is_empty() {
            rows.push((String::new(), String::new()));
            rows.push(("Description".to_string(), String::new()));
            for line in wrap_words(d, 64) {
                rows.push((format!("  {}", line), String::new()));
            }
        }
    }

    let signals = seq.signals();
    if !signals.is_empty() {
        rows.push((String::new(), String::new()));
        rows.push((format!("Signals ({})", signals.len()), String::new()));
        for s in signals.iter().take(MAX_SEQUENCE_ITEMS) {
            let name = s.name().or(s.uid()).unwrap_or("(unnamed signal)");
            let kind = s.r#type().map(|t| t.as_str()).unwrap_or("");
            rows.push((format!("  {}", name), kind.to_string()));
            let mut meta: Vec<String> = Vec::new();
            if let Some(sev) = s.severity() {
                meta.push(format!("severity {:.1}", sev));
            }
            if let Some(c) = s.count() {
                meta.push(format!("×{}", c));
            }
            if let Some(t) = s.first_seen_at().or(s.created_at()) {
                meta.push(format!("first {}", t));
            }
            if let Some(t) = s.last_seen_at() {
                meta.push(format!("last {}", t));
            }
            if !meta.is_empty() {
                rows.push((format!("    · {}", meta.join("  ·  ")), String::new()));
            }
        }
        if signals.len() > MAX_SEQUENCE_ITEMS {
            rows.push((
                format!("  · {} more signals", signals.len() - MAX_SEQUENCE_ITEMS),
                String::new(),
            ));
        }
    }

    let actors = seq.actors();
    if !actors.is_empty() {
        rows.push((String::new(), String::new()));
        rows.push((format!("Actors ({})", actors.len()), String::new()));
        for a in actors.iter().take(MAX_SEQUENCE_ITEMS) {
            push_row(&mut rows, "Actor", a.id());
            if let Some(u) = a.user() {
                push_row(&mut rows, "User", u.name());
                push_row(&mut rows, "User Type", u.r#type());
                push_row(&mut rows, "Credential", u.credential_uid());
                if let Some(acct) = u.account() {
                    push_row(&mut rows, "Account", acct.uid());
                }
            }
            if let Some(s) = a.session() {
                push_row(&mut rows, "Session Issuer", s.issuer());
                if let Some(m) = s.mfa_status() {
                    rows.push(("MFA".to_string(), m.as_str().to_string()));
                }
                if let Some(t) = s.created_time() {
                    rows.push(("Session Started".to_string(), t.to_string()));
                }
            }
            if let Some(p) = a.process() {
                push_row(&mut rows, "Process", p.name());
                push_row(&mut rows, "Process Path", p.path());
            }
        }
    }

    let endpoints = seq.endpoints();
    if !endpoints.is_empty() {
        rows.push((String::new(), String::new()));
        rows.push((format!("Endpoints ({})", endpoints.len()), String::new()));
        for e in endpoints.iter().take(MAX_SEQUENCE_ITEMS) {
            let head = e.ip().or(e.domain()).or(e.id()).unwrap_or("(endpoint)");
            let port = e.port().map(|p| p.to_string()).unwrap_or_default();
            rows.push((format!("  {}", head), port));
            let mut meta: Vec<String> = Vec::new();
            if let Some(l) = e.location() {
                let place = [l.city(), l.country()]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(", ");
                if !place.is_empty() {
                    meta.push(place);
                }
            }
            if let Some(a) = e.autonomous_system() {
                match (a.name(), a.number()) {
                    (Some(n), Some(num)) => meta.push(format!("{} (AS{})", n, num)),
                    (Some(n), None) => meta.push(n.to_string()),
                    (None, Some(num)) => meta.push(format!("AS{}", num)),
                    (None, None) => {}
                }
            }
            if !meta.is_empty() {
                rows.push((format!("    · {}", meta.join("  ·  ")), String::new()));
            }
        }
    }

    let indicators = seq.sequence_indicators();
    if !indicators.is_empty() {
        rows.push((String::new(), String::new()));
        rows.push((format!("Indicators ({})", indicators.len()), String::new()));
        for i in indicators.iter().take(MAX_SEQUENCE_ITEMS) {
            let key = i
                .title()
                .map(|t| t.to_string())
                .or_else(|| i.key().map(|k| k.as_str().to_string()))
                .unwrap_or_else(|| "Indicator".to_string());
            rows.push((format!("  {}", key), i.values().join(", ")));
        }
    }

    rows
}

/// Word-wrap for descriptions built at ingestion time (the details pane has its
/// own `wrap_plain`, but these rows are precomputed in `from_sdk`).
fn wrap_words(text: &str, width: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > width {
            out.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        out.push(line);
    }
    out
}

// ── GdOverview (the Summary tab) ────────────────────────────────────────────

/// A single synthetic row summarising the whole detector — the console's
/// landing page. Built from `GetFindingsStatistics` grouped four ways rather
/// than from the loaded finding list, because that list is bounded by the
/// active severity scope and by `MAX_FINDINGS`; the point of this tab is the
/// picture those bounds hide.
#[derive(Debug, Clone)]
pub struct GdOverview {
    pub id: String,
    pub detector_id: String,
    pub total_findings: i32,
    /// Critical → Low, always all four buckets so a zero reads as a zero
    /// rather than as a missing row.
    pub by_severity: Vec<(String, i32)>,
    pub top_types: Vec<(String, i32)>,
    /// `(resource id, resource type, count)`.
    pub top_resources: Vec<(String, String, i32)>,
    pub top_accounts: Vec<(String, i32)>,
    pub coverage_rows: Vec<(String, String)>,
    /// Features still inside their 30-day free trial, `(feature, days left)`.
    pub free_trial: Vec<(String, i32)>,
    /// One entry per dimension that failed, rendered inline. A denied
    /// statistics call shouldn't blank the tab that explains it.
    pub errors: Vec<String>,
}

impl GdOverview {
    fn severity_count(&self, label: &str) -> i32 {
        self.by_severity
            .iter()
            .find(|(l, _)| l == label)
            .map(|(_, n)| *n)
            .unwrap_or(0)
    }
}

crate::sections! {
    pub enum GdOverviewDetailSection,
    pub static GD_OVERVIEW_SECTIONS = [
        Overview "Overview",
        Types "Finding Types",
        Resources "Resources",
        Accounts "Accounts",
        Coverage "Coverage",
    ]
}

impl Resource for GdOverview {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&GD_OVERVIEW_SECTIONS)
    }

    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        "Findings Summary"
    }

    fn resource_type(&self) -> &str {
        "GuardDuty Summary"
    }

    /// Reads like a finding row: anything Critical/High outstanding is red.
    fn state(&self) -> ResourceState {
        if self.severity_count("Critical") + self.severity_count("High") > 0 {
            ResourceState::Unavailable
        } else if self.severity_count("Medium") > 0 {
            ResourceState::Pending
        } else {
            ResourceState::Available
        }
    }
    fn state_label(&self) -> String {
        if self.severity_count("Critical") + self.severity_count("High") > 0 {
            "critical/high findings"
        } else if self.severity_count("Medium") > 0 {
            "medium findings"
        } else {
            "clear"
        }
        .to_string()
    }

    fn tags(&self) -> &HashMap<String, String> {
        static EMPTY: std::sync::OnceLock<HashMap<String, String>> = std::sync::OnceLock::new();
        EMPTY.get_or_init(HashMap::new)
    }

    fn search_text(&self) -> String {
        "guardduty summary overview dashboard statistics".to_string()
    }

    fn details(&self) -> Vec<(String, String)> {
        // Fallback only — the split pane is the real view.
        vec![
            ("Total Findings".to_string(), self.total_findings.to_string()),
            ("Detector".to_string(), self.detector_id.clone()),
        ]
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/guardduty/home?region={}#/summary",
            region, region
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Assemble the Summary row. Every dimension is independent and best-effort:
/// one denied call records its own message and the rest still render.
async fn fetch_overview(client: &GdClient, detector_id: &str) -> GdOverview {
    let mut errors: Vec<String> = Vec::new();

    // Severity. `count_by_severity` is only populated by the deprecated
    // `findingStatisticTypes` form, so the counts come from the grouped list
    // and are bucketed with the same thresholds the finding rows use.
    let mut by_severity: Vec<(String, i32)> = ["Critical", "High", "Medium", "Low"]
        .iter()
        .map(|l| (l.to_string(), 0))
        .collect();
    let mut total_findings = 0;
    match client
        .get_findings_statistics()
        .detector_id(detector_id)
        .group_by(GroupByType::Severity)
        .send()
        .await
    {
        Ok(resp) => {
            if let Some(stats) = resp.finding_statistics() {
                for s in stats.grouped_by_severity() {
                    let n = s.total_findings().unwrap_or(0);
                    total_findings += n;
                    let label = severity_label(s.severity().unwrap_or(0.0));
                    if let Some(e) = by_severity.iter_mut().find(|(l, _)| l == label) {
                        e.1 += n;
                    }
                }
            }
        }
        Err(e) => errors.push(format!("severity: {}", crate::error::sdk_error_message(&e))),
    }

    let mut top_types: Vec<(String, i32)> = Vec::new();
    match client
        .get_findings_statistics()
        .detector_id(detector_id)
        .group_by(GroupByType::FindingType)
        .order_by(OrderBy::Desc)
        .max_results(MAX_TOP_STATS)
        .send()
        .await
    {
        Ok(resp) => {
            if let Some(stats) = resp.finding_statistics() {
                top_types = stats
                    .grouped_by_finding_type()
                    .iter()
                    .map(|t| {
                        (
                            t.finding_type().unwrap_or("(unknown)").to_string(),
                            t.total_findings().unwrap_or(0),
                        )
                    })
                    .collect();
            }
        }
        Err(e) => errors.push(format!(
            "finding types: {}",
            crate::error::sdk_error_message(&e)
        )),
    }

    let mut top_resources: Vec<(String, String, i32)> = Vec::new();
    match client
        .get_findings_statistics()
        .detector_id(detector_id)
        .group_by(GroupByType::Resource)
        .order_by(OrderBy::Desc)
        .max_results(MAX_TOP_STATS)
        .send()
        .await
    {
        Ok(resp) => {
            if let Some(stats) = resp.finding_statistics() {
                top_resources = stats
                    .grouped_by_resource()
                    .iter()
                    .map(|r| {
                        (
                            r.resource_id().unwrap_or("(unknown)").to_string(),
                            r.resource_type().unwrap_or("").to_string(),
                            r.total_findings().unwrap_or(0),
                        )
                    })
                    .collect();
            }
        }
        Err(e) => errors.push(format!(
            "resources: {}",
            crate::error::sdk_error_message(&e)
        )),
    }

    let mut top_accounts: Vec<(String, i32)> = Vec::new();
    match client
        .get_findings_statistics()
        .detector_id(detector_id)
        .group_by(GroupByType::Account)
        .order_by(OrderBy::Desc)
        .max_results(MAX_TOP_STATS)
        .send()
        .await
    {
        Ok(resp) => {
            if let Some(stats) = resp.finding_statistics() {
                top_accounts = stats
                    .grouped_by_account()
                    .iter()
                    .map(|a| {
                        (
                            a.account_id().unwrap_or("(unknown)").to_string(),
                            a.total_findings().unwrap_or(0),
                        )
                    })
                    .collect();
            }
        }
        Err(e) => errors.push(format!("accounts: {}", crate::error::sdk_error_message(&e))),
    }

    // Coverage rollup — the same call the detector pane shows, repeated here so
    // the Summary answers "am I protected" as well as "what fired".
    let coverage_rows = client
        .get_coverage_statistics()
        .detector_id(detector_id)
        .statistics_type(CoverageStatisticsType::CountByCoverageStatus)
        .statistics_type(CoverageStatisticsType::CountByResourceType)
        .send()
        .await
        .ok()
        .and_then(|r| r.coverage_statistics().cloned())
        .map(|c| coverage_stat_rows(&c))
        .unwrap_or_default();

    // Free-trial days are only interesting while a trial is actually running,
    // so expired features (0 days) are dropped rather than listed as zeroes.
    let free_trial = client
        .get_remaining_free_trial_days()
        .detector_id(detector_id)
        .send()
        .await
        .ok()
        .map(|r| {
            r.accounts()
                .iter()
                .flat_map(|a| a.features().iter())
                .filter_map(|f| {
                    let days = f.free_trial_days_remaining().unwrap_or(0);
                    if days > 0 {
                        Some((
                            f.name().map(|n| n.as_str()).unwrap_or("?").to_string(),
                            days,
                        ))
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    GdOverview {
        id: format!("gd-summary-{}", detector_id),
        detector_id: detector_id.to_string(),
        total_findings,
        by_severity,
        top_types,
        top_resources,
        top_accounts,
        coverage_rows,
        free_trial,
        errors,
    }
}

/// `ListFilters` → `GetFilter` per name. Both AWS quotas bound this well
/// below anything worth capping (100 filters per detector).
async fn fetch_filters(
    client: &GdClient,
    detector_id: &str,
    event_tx: &mpsc::UnboundedSender<Event>,
    service_type: ServiceType,
) -> Vec<Box<dyn Resource>> {
    let mut out: Vec<Box<dyn Resource>> = Vec::new();
    let mut pages = client
        .list_filters()
        .detector_id(detector_id)
        .into_paginator()
        .send();
    loop {
        let page = match pages.next().await {
            Some(Ok(p)) => p,
            Some(Err(e)) => {
                let _ = event_tx.send(Event::ResourceLoadWarning {
                    service: service_type,
                    warning: format!("filters: {}", crate::error::sdk_error_message(&e)),
                });
                break;
            }
            None => break,
        };
        for name in page.filter_names() {
            match client
                .get_filter()
                .detector_id(detector_id)
                .filter_name(name)
                .send()
                .await
            {
                Ok(f) => out.push(Box::new(GdFilter::from_sdk(name, &f)) as Box<dyn Resource>),
                Err(e) => {
                    let _ = event_tx.send(Event::ResourceLoadWarning {
                        service: service_type,
                        warning: format!(
                            "filter {}: {}",
                            name,
                            crate::error::sdk_error_message(&e)
                        ),
                    });
                }
            }
        }
    }
    out
}

/// The four list APIs, unified. The classic IP / threat-intel sets warn on
/// failure; the newer trusted/threat **entity** sets are skipped silently,
/// because they aren't available in every region and a permanent "not
/// supported here" warning on every load would be pure noise (the
/// `InsightNotEnabledException` precedent in CloudTrail).
async fn fetch_lists(
    client: &GdClient,
    detector_id: &str,
    event_tx: &mpsc::UnboundedSender<Event>,
    service_type: ServiceType,
) -> Vec<Box<dyn Resource>> {
    let mut out: Vec<Box<dyn Resource>> = Vec::new();
    let warn = |what: &str, e: String| {
        let _ = event_tx.send(Event::ResourceLoadWarning {
            service: service_type,
            warning: format!("{}: {}", what, e),
        });
    };

    // Trusted IP sets.
    let mut pages = client
        .list_ip_sets()
        .detector_id(detector_id)
        .into_paginator()
        .send();
    loop {
        match pages.next().await {
            Some(Ok(page)) => {
                for id in page.ip_set_ids() {
                    match client
                        .get_ip_set()
                        .detector_id(detector_id)
                        .ip_set_id(id)
                        .send()
                        .await
                    {
                        Ok(s) => out.push(Box::new(GdList::new(
                            id,
                            s.name().unwrap_or(id),
                            "Trusted IP",
                            s.format().map(|f| f.as_str()).unwrap_or(""),
                            s.location().unwrap_or(""),
                            s.status().map(|s| s.as_str()).unwrap_or(""),
                            s.expected_bucket_owner().unwrap_or(""),
                            s.tags()
                                .map(|t| t.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                                .unwrap_or_default(),
                        )) as Box<dyn Resource>),
                        Err(e) => warn("trusted IP set", crate::error::sdk_error_message(&e)),
                    }
                }
            }
            Some(Err(e)) => {
                warn("trusted IP sets", crate::error::sdk_error_message(&e));
                break;
            }
            None => break,
        }
    }

    // Threat IP sets.
    let mut pages = client
        .list_threat_intel_sets()
        .detector_id(detector_id)
        .into_paginator()
        .send();
    loop {
        match pages.next().await {
            Some(Ok(page)) => {
                for id in page.threat_intel_set_ids() {
                    match client
                        .get_threat_intel_set()
                        .detector_id(detector_id)
                        .threat_intel_set_id(id)
                        .send()
                        .await
                    {
                        Ok(s) => out.push(Box::new(GdList::new(
                            id,
                            s.name().unwrap_or(id),
                            "Threat IP",
                            s.format().map(|f| f.as_str()).unwrap_or(""),
                            s.location().unwrap_or(""),
                            s.status().map(|s| s.as_str()).unwrap_or(""),
                            s.expected_bucket_owner().unwrap_or(""),
                            s.tags()
                                .map(|t| t.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                                .unwrap_or_default(),
                        )) as Box<dyn Resource>),
                        Err(e) => warn("threat IP set", crate::error::sdk_error_message(&e)),
                    }
                }
            }
            Some(Err(e)) => {
                warn("threat IP sets", crate::error::sdk_error_message(&e));
                break;
            }
            None => break,
        }
    }

    // Trusted entity sets (newer generation — silent on failure, see above).
    if let Ok(page) = client
        .list_trusted_entity_sets()
        .detector_id(detector_id)
        .send()
        .await
    {
        for id in page.trusted_entity_set_ids() {
            if let Ok(s) = client
                .get_trusted_entity_set()
                .detector_id(detector_id)
                .trusted_entity_set_id(id)
                .send()
                .await
            {
                let mut list = GdList::new(
                    id,
                    s.name().unwrap_or(id),
                    "Trusted Entity",
                    s.format().map(|f| f.as_str()).unwrap_or(""),
                    s.location().unwrap_or(""),
                    s.status().map(|s| s.as_str()).unwrap_or(""),
                    s.expected_bucket_owner().unwrap_or(""),
                    s.tags()
                        .map(|t| t.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                        .unwrap_or_default(),
                );
                list.error_details = s.error_details().unwrap_or_default().to_string();
                list.created = s.created_at().map(|t| t.to_string());
                list.updated = s.updated_at().map(|t| t.to_string());
                out.push(Box::new(list) as Box<dyn Resource>);
            }
        }
    }

    // Threat entity sets (likewise).
    if let Ok(page) = client
        .list_threat_entity_sets()
        .detector_id(detector_id)
        .send()
        .await
    {
        for id in page.threat_entity_set_ids() {
            if let Ok(s) = client
                .get_threat_entity_set()
                .detector_id(detector_id)
                .threat_entity_set_id(id)
                .send()
                .await
            {
                let mut list = GdList::new(
                    id,
                    s.name().unwrap_or(id),
                    "Threat Entity",
                    s.format().map(|f| f.as_str()).unwrap_or(""),
                    s.location().unwrap_or(""),
                    s.status().map(|s| s.as_str()).unwrap_or(""),
                    s.expected_bucket_owner().unwrap_or(""),
                    s.tags()
                        .map(|t| t.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                        .unwrap_or_default(),
                );
                list.error_details = s.error_details().unwrap_or_default().to_string();
                list.created = s.created_at().map(|t| t.to_string());
                list.updated = s.updated_at().map(|t| t.to_string());
                out.push(Box::new(list) as Box<dyn Resource>);
            }
        }
    }

    out
}

/// `ListMembers` (including unassociated members — a removed or never-enabled
/// account is exactly what this tab is for), then per-feature enrichment via
/// `GetMemberDetectors` in batches of 50.
///
/// Every failure here is silent rather than a warning: from a standalone
/// account these calls are *expected* to fail, and a permanent "you're not the
/// GuardDuty administrator" warning on every load in every non-admin account
/// would be noise. `resource_list` explains the empty tab instead.
async fn fetch_members(client: &GdClient, detector_id: &str) -> Vec<Box<dyn Resource>> {
    let mut members: Vec<GdMember> = Vec::new();
    let mut pages = client
        .list_members()
        .detector_id(detector_id)
        // "false" = don't restrict to *associated* members, so disabled and
        // removed accounts still appear.
        .only_associated("false")
        .into_paginator()
        .send();
    while let Some(Ok(page)) = pages.next().await {
        for m in page.members() {
            members.push(GdMember::from_sdk(m));
            if members.len() >= MAX_MEMBERS {
                break;
            }
        }
        if members.len() >= MAX_MEMBERS {
            break;
        }
    }
    if members.is_empty() {
        return Vec::new();
    }

    // Feature enablement, 50 accounts per call.
    let ids: Vec<String> = members.iter().map(|m| m.account_id.clone()).collect();
    let mut by_account: HashMap<String, (Vec<(String, String)>, usize)> = HashMap::new();
    for chunk in ids.chunks(MEMBER_DETECTOR_CHUNK) {
        if let Ok(resp) = client
            .get_member_detectors()
            .detector_id(detector_id)
            .set_account_ids(Some(chunk.to_vec()))
            .send()
            .await
        {
            for cfg in resp.member_data_source_configurations() {
                let account = cfg.account_id().unwrap_or_default().to_string();
                let mut rows: Vec<(String, String)> = Vec::new();
                let mut disabled = 0usize;
                for f in cfg.features() {
                    let name = f.name().map(|n| n.as_str()).unwrap_or("?").to_string();
                    let status = f.status().map(|s| s.as_str()).unwrap_or("?").to_string();
                    if status != "ENABLED" {
                        disabled += 1;
                    }
                    rows.push((name, status));
                }
                rows.sort();
                by_account.insert(account, (rows, disabled));
            }
        }
    }
    for m in &mut members {
        if let Some((rows, disabled)) = by_account.remove(&m.account_id) {
            m.feature_rows = rows;
            m.disabled_features = disabled;
        }
    }

    // Worst first: unenabled accounts, then partially-featured ones.
    members.sort_by(|a, b| {
        (a.is_enabled(), a.disabled_features == 0)
            .cmp(&(b.is_enabled(), b.disabled_features == 0))
            .then(a.account_id.cmp(&b.account_id))
    });
    members
        .into_iter()
        .map(|m| Box::new(m) as Box<dyn Resource>)
        .collect()
}

/// The account's position in the org: who administers it, whether it *is* the
/// delegated admin, and how new member accounts are auto-enabled. Folded into
/// the detector's Overview (the FMS `fms_admin` pattern) rather than becoming
/// its own resource — it's three scalars, not a list. All best-effort: a
/// standalone account answers none of these.
async fn fetch_org_rows(client: &GdClient, detector_id: &str) -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = Vec::new();

    if let Ok(resp) = client
        .get_administrator_account()
        .detector_id(detector_id)
        .send()
        .await
    {
        if let Some(a) = resp.administrator() {
            if let Some(id) = a.account_id() {
                if !id.is_empty() {
                    rows.push(("Administered By".to_string(), id.to_string()));
                    if let Some(s) = a.relationship_status() {
                        rows.push(("Relationship".to_string(), s.to_string()));
                    }
                }
            }
        }
    }

    if let Ok(resp) = client.list_organization_admin_accounts().send().await {
        for a in resp.admin_accounts() {
            if let Some(id) = a.admin_account_id() {
                rows.push((
                    "Delegated Admin".to_string(),
                    match a.admin_status() {
                        Some(s) => format!("{} ({})", id, s.as_str()),
                        None => id.to_string(),
                    },
                ));
            }
        }
    }

    if let Ok(resp) = client
        .describe_organization_configuration()
        .detector_id(detector_id)
        .send()
        .await
    {
        if let Some(a) = resp.auto_enable_organization_members() {
            rows.push(("Auto-Enable Members".to_string(), a.as_str().to_string()));
        }
        if resp.member_account_limit_reached() == Some(true) {
            rows.push((
                "Member Limit".to_string(),
                "⚠ reached — new accounts can't be enrolled".to_string(),
            ));
        }
        let mut features: Vec<(String, String)> = resp
            .features()
            .iter()
            .filter_map(|f| {
                let name = f.name()?.as_str().to_string();
                let status = f.auto_enable()?.as_str().to_string();
                Some((format!("  {}", name), status))
            })
            .collect();
        if !features.is_empty() {
            features.sort();
            rows.push(("Feature Auto-Enable".to_string(), String::new()));
            rows.extend(features);
        }
    }

    rows
}

/// EBS malware scans (newest first) and S3 Malware Protection plans — the two
/// halves of one feature, sharing a tab. Both are silent on failure: Malware
/// Protection is off in most accounts, and neither API distinguishes
/// "feature not enabled" from a permission gap in a way worth warning about
/// on every load.
async fn fetch_malware(client: &GdClient, detector_id: &str) -> Vec<Box<dyn Resource>> {
    let mut out: Vec<Box<dyn Resource>> = Vec::new();

    let sort = SortCriteria::builder()
        .attribute_name("scanStartTime")
        .order_by(OrderBy::Desc)
        .build();
    let mut pages = client
        .describe_malware_scans()
        .detector_id(detector_id)
        .sort_criteria(sort)
        .max_results(50)
        .into_paginator()
        .send();
    let mut kept = 0usize;
    while let Some(Ok(page)) = pages.next().await {
        for s in page.scans() {
            out.push(Box::new(GdMalwareScan::from_sdk(s)) as Box<dyn Resource>);
            kept += 1;
            if kept >= MAX_MALWARE_SCANS {
                break;
            }
        }
        if kept >= MAX_MALWARE_SCANS {
            break;
        }
    }

    // `ListMalwareProtectionPlans` has no fluent paginator — advance the token
    // through the shared helper, which stops on absent / empty / non-advancing
    // tokens (a bare `is_none()` check can spin forever).
    let mut token: Option<String> = None;
    loop {
        let mut req = client.list_malware_protection_plans();
        if let Some(t) = &token {
            req = req.next_token(t);
        }
        let Ok(page) = req.send().await else { break };
        for summary in page.malware_protection_plans() {
            let Some(id) = summary.malware_protection_plan_id() else {
                continue;
            };
            if let Ok(p) = client
                .get_malware_protection_plan()
                .malware_protection_plan_id(id)
                .send()
                .await
            {
                let s3 = p.protected_resource().and_then(|r| r.s3_bucket());
                let bucket = s3
                    .and_then(|b| b.bucket_name())
                    .unwrap_or_default()
                    .to_string();
                let status = p
                    .status()
                    .map(|s| s.as_str().to_string())
                    .unwrap_or_default();
                out.push(Box::new(GdMalwarePlan {
                    search_blob: format!("{} {} {} malware protection plan", bucket, id, status),
                    id: id.to_string(),
                    arn: p.arn().unwrap_or_default().to_string(),
                    role: p.role().unwrap_or_default().to_string(),
                    bucket,
                    object_prefixes: s3
                        .map(|b| b.object_prefixes().to_vec())
                        .unwrap_or_default(),
                    tagging: p
                        .actions()
                        .and_then(|a| a.tagging())
                        .and_then(|t| t.status())
                        .map(|s| s.as_str().to_string())
                        .unwrap_or_default(),
                    status,
                    status_reasons: p
                        .status_reasons()
                        .iter()
                        .map(|r| {
                            (
                                r.code().unwrap_or("Reason").to_string(),
                                r.message().unwrap_or_default().to_string(),
                            )
                        })
                        .collect(),
                    created: p.created_at().map(|t| t.to_string()),
                    tags: p
                        .tags()
                        .map(|t| t.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                        .unwrap_or_default(),
                }) as Box<dyn Resource>);
            }
        }
        token = crate::aws::pagination::next_page_token(page.next_token(), &token);
        if token.is_none() {
            break;
        }
    }

    out
}

/// Flatten a `CoverageStatistics` into stable, sorted `(label, count)` rows.
/// Shared by the detector's Overview and the Summary tab — both maps are
/// unordered, so sorting keeps the pane from reshuffling on every refresh.
fn coverage_stat_rows(c: &aws_sdk_guardduty::types::CoverageStatistics) -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = Vec::new();
    if let Some(by_status) = c.count_by_coverage_status() {
        let mut v: Vec<_> = by_status
            .iter()
            .map(|(k, n)| (k.as_str().to_string(), *n))
            .collect();
        v.sort();
        rows.extend(v.into_iter().map(|(k, n)| (k, n.to_string())));
    }
    if let Some(by_type) = c.count_by_resource_type() {
        let mut v: Vec<_> = by_type
            .iter()
            .map(|(k, n)| (k.as_str().to_string(), *n))
            .collect();
        v.sort();
        rows.extend(v.into_iter().map(|(k, n)| (k, n.to_string())));
    }
    rows
}

// ── GdMalwareScan (EBS volume scans) ────────────────────────────────────────

/// One `DescribeMalwareScans` entry — a scan of an EC2 instance's EBS volumes,
/// either triggered by a qualifying finding or run on demand. Shares the
/// Malware tab with `GdMalwarePlan` via a pipe-separated type filter: they're
/// two halves of one feature (EC2 volumes and S3 objects) and neither fills a
/// tab on its own.
#[derive(Debug, Clone)]
pub struct GdMalwareScan {
    pub scan_id: String,
    /// The scanned instance's id where the ARN gave one, else the scan id —
    /// a bare scan UUID doesn't identify anything to a reader.
    pub name: String,
    pub account_id: String,
    pub status: String,
    /// `CLEAN` / `INFECTED`, empty until the scan completes.
    pub result: String,
    pub scan_type: String,
    pub failure_reason: String,
    pub started: Option<String>,
    pub ended: Option<String>,
    pub trigger_finding_id: String,
    pub trigger_type: String,
    pub trigger_description: String,
    pub instance_arn: String,
    pub total_bytes: i64,
    pub file_count: i64,
    pub volume_rows: Vec<(String, String)>,
    pub search_blob: String,
}

impl GdMalwareScan {
    fn from_sdk(s: &aws_sdk_guardduty::types::Scan) -> Self {
        let instance_arn = s
            .resource_details()
            .and_then(|r| r.instance_arn())
            .unwrap_or_default()
            .to_string();
        let scan_id = s.scan_id().unwrap_or_default().to_string();
        // An instance ARN ends `.../i-0123…`; the tail is what people read.
        let name = instance_arn
            .rsplit('/')
            .next()
            .filter(|t| !t.is_empty())
            .unwrap_or(&scan_id)
            .to_string();

        let mut volume_rows: Vec<(String, String)> = Vec::new();
        for v in s.attached_volumes() {
            push_volume_rows(&mut volume_rows, v, "Attached");
        }

        let status = s
            .scan_status()
            .map(|v| v.as_str().to_string())
            .unwrap_or_default();
        let result = s
            .scan_result_details()
            .and_then(|d| d.scan_result())
            .map(|r| r.as_str().to_string())
            .unwrap_or_default();
        let trigger = s.trigger_details();

        Self {
            search_blob: format!(
                "{} {} {} {} malware scan {}",
                name, scan_id, status, result, instance_arn
            ),
            scan_id,
            name,
            account_id: s.account_id().unwrap_or_default().to_string(),
            status,
            result,
            scan_type: s
                .scan_type()
                .map(|v| v.as_str().to_string())
                .unwrap_or_default(),
            failure_reason: s.failure_reason().unwrap_or_default().to_string(),
            started: s.scan_start_time().map(|t| t.to_string()),
            ended: s.scan_end_time().map(|t| t.to_string()),
            trigger_finding_id: trigger
                .and_then(|t| t.guard_duty_finding_id())
                .unwrap_or_default()
                .to_string(),
            trigger_type: trigger
                .and_then(|t| t.trigger_type())
                .map(|t| t.as_str().to_string())
                .unwrap_or_default(),
            trigger_description: trigger
                .and_then(|t| t.description())
                .unwrap_or_default()
                .to_string(),
            instance_arn,
            total_bytes: s.total_bytes().unwrap_or(0),
            file_count: s.file_count().unwrap_or(0),
            volume_rows,
        }
    }

    pub fn is_infected(&self) -> bool {
        self.result == "INFECTED"
    }
}

crate::sections! {
    pub enum GdMalwareScanDetailSection,
    pub static GD_MALWARE_SCAN_SECTIONS = [
        Overview "Overview",
        Volumes "Volumes",
    ]
}

impl Resource for GdMalwareScan {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&GD_MALWARE_SCAN_SECTIONS)
    }

    fn id(&self) -> &str {
        &self.scan_id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn resource_type(&self) -> &str {
        "GuardDuty Malware Scan"
    }

    fn state(&self) -> ResourceState {
        if self.is_infected() {
            return ResourceState::Unavailable;
        }
        match self.status.as_str() {
            "COMPLETED" => ResourceState::Available,
            "FAILED" => ResourceState::Unavailable,
            "RUNNING" => ResourceState::Pending,
            "SKIPPED" => ResourceState::Stopped,
            other => ResourceState::Unknown(other.to_string()),
        }
    }
    fn state_label(&self) -> String {
        if self.is_infected() {
            "infected".to_string()
        } else {
            native_state_label(&self.status, || self.state())
        }
    }

    /// A clean completed scan is the overwhelming majority — `a` leaves the
    /// infected and failed ones.
    fn is_noise(&self) -> bool {
        self.status == "COMPLETED" && !self.is_infected()
    }

    fn tags(&self) -> &HashMap<String, String> {
        static EMPTY: std::sync::OnceLock<HashMap<String, String>> = std::sync::OnceLock::new();
        EMPTY.get_or_init(HashMap::new)
    }

    fn search_text(&self) -> String {
        self.search_blob.clone()
    }

    fn details(&self) -> Vec<(String, String)> {
        // Fallback only — the split pane is the real view.
        vec![
            ("Scan ID".to_string(), self.scan_id.clone()),
            ("Status".to_string(), self.status.clone()),
        ]
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/guardduty/home?region={}#/malware-scans",
            region, region
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── GdMalwarePlan (Malware Protection for S3) ───────────────────────────────

/// A `ListMalwareProtectionPlans` entry hydrated by `GetMalwareProtectionPlan`
/// — GuardDuty Malware Protection for **S3**, which is a separate feature from
/// the EBS scans above and configured per bucket.
#[derive(Debug, Clone)]
pub struct GdMalwarePlan {
    pub id: String,
    pub arn: String,
    pub role: String,
    pub bucket: String,
    pub object_prefixes: Vec<String>,
    pub tagging: String,
    pub status: String,
    pub status_reasons: Vec<(String, String)>,
    pub created: Option<String>,
    pub tags: HashMap<String, String>,
    pub search_blob: String,
}

impl Resource for GdMalwarePlan {
    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        if self.bucket.is_empty() {
            &self.id
        } else {
            &self.bucket
        }
    }

    fn resource_type(&self) -> &str {
        "GuardDuty Malware Plan"
    }

    fn state(&self) -> ResourceState {
        match self.status.as_str() {
            "ACTIVE" => ResourceState::Available,
            "WARNING" => ResourceState::Pending,
            "ERROR" => ResourceState::Unavailable,
            other => ResourceState::Unknown(other.to_string()),
        }
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

    fn details(&self) -> Vec<(String, String)> {
        let mut rows = vec![
            ("Protected Bucket".to_string(), self.bucket.clone()),
            ("Status".to_string(), self.status.clone()),
        ];
        for (code, message) in &self.status_reasons {
            rows.push((format!("⚠ {}", code), message.clone()));
        }
        if !self.object_prefixes.is_empty() {
            rows.push((
                "Object Prefixes".to_string(),
                self.object_prefixes.join(", "),
            ));
        } else if !self.bucket.is_empty() {
            rows.push(("Object Prefixes".to_string(), "(entire bucket)".to_string()));
        }
        if !self.tagging.is_empty() {
            rows.push(("Tag Scanned Objects".to_string(), self.tagging.clone()));
        }
        if !self.role.is_empty() {
            rows.push(("Role".to_string(), self.role.clone()));
        }
        if let Some(c) = &self.created {
            rows.push(("Created".to_string(), c.clone()));
        }
        rows.push(("Plan ID".to_string(), self.id.clone()));
        if !self.arn.is_empty() {
            rows.push(("ARN".to_string(), self.arn.clone()));
        }
        if !self.tags.is_empty() {
            rows.push((String::new(), String::new()));
            rows.push(("Tags".to_string(), String::new()));
            let mut tags: Vec<_> = self.tags.iter().collect();
            tags.sort();
            for (k, v) in tags {
                rows.push((format!("  {}", k), v.clone()));
            }
        }
        rows
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/guardduty/home?region={}#/malware-protection-s3",
            region, region
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── GdMember (org member accounts) ──────────────────────────────────────────

/// One org member account as the GuardDuty administrator sees it. Only the
/// administrator / delegated admin can list these at all — from a standalone
/// account the tab is simply empty, which `resource_list` explains rather than
/// showing a bare "no resources".
#[derive(Debug, Clone)]
pub struct GdMember {
    pub account_id: String,
    pub email: String,
    /// `Enabled`, `Disabled`, `Invited`, `Removed`, `Resigned`, …
    pub relationship_status: String,
    pub detector_id: String,
    pub invited_at: String,
    pub updated_at: String,
    /// Per-feature enablement from `GetMemberDetectors`, `(feature, status)`.
    pub feature_rows: Vec<(String, String)>,
    /// How many features are off — the number that makes a row worth reading.
    pub disabled_features: usize,
    pub search_blob: String,
}

impl GdMember {
    fn from_sdk(m: &aws_sdk_guardduty::types::Member) -> Self {
        let account_id = m.account_id().unwrap_or_default().to_string();
        let email = m.email().unwrap_or_default().to_string();
        let relationship_status = m.relationship_status().unwrap_or_default().to_string();
        Self {
            search_blob: format!(
                "{} {} {} member account",
                account_id, email, relationship_status
            ),
            account_id,
            email,
            relationship_status,
            detector_id: m.detector_id().unwrap_or_default().to_string(),
            invited_at: m.invited_at().unwrap_or_default().to_string(),
            updated_at: m.updated_at().unwrap_or_default().to_string(),
            feature_rows: Vec::new(),
            disabled_features: 0,
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.relationship_status == "Enabled"
    }
}

crate::sections! {
    pub enum GdMemberDetailSection,
    pub static GD_MEMBER_SECTIONS = [
        Overview "Overview",
        Features "Features",
    ]
}

impl Resource for GdMember {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&GD_MEMBER_SECTIONS)
    }

    fn id(&self) -> &str {
        &self.account_id
    }

    fn name(&self) -> &str {
        &self.account_id
    }

    fn resource_type(&self) -> &str {
        "GuardDuty Member"
    }

    /// A member that isn't `Enabled` is an unprotected account, not merely an
    /// inactive row — that reads red. A partially-featured member reads yellow.
    fn state(&self) -> ResourceState {
        match self.relationship_status.as_str() {
            "Enabled" => {
                if self.disabled_features > 0 {
                    ResourceState::Pending
                } else {
                    ResourceState::Available
                }
            }
            "Created" | "Invited" | "EmailVerificationInProgress" => ResourceState::Pending,
            "Disabled" | "Removed" | "Resigned" => ResourceState::Unavailable,
            other => ResourceState::Unknown(other.to_string()),
        }
    }
    fn state_label(&self) -> String {
        if self.relationship_status == "Enabled" && self.disabled_features > 0 {
            "partially enabled".to_string()
        } else {
            native_state_label(&self.relationship_status, || self.state())
        }
    }

    /// Fully-enabled members are the uninteresting majority — `a` narrows the
    /// tab to the accounts with a gap, mirroring Coverage.
    fn is_noise(&self) -> bool {
        self.is_enabled() && self.disabled_features == 0
    }

    fn tags(&self) -> &HashMap<String, String> {
        static EMPTY: std::sync::OnceLock<HashMap<String, String>> = std::sync::OnceLock::new();
        EMPTY.get_or_init(HashMap::new)
    }

    fn search_text(&self) -> String {
        self.search_blob.clone()
    }

    fn details(&self) -> Vec<(String, String)> {
        // Fallback only — the split pane is the real view.
        vec![
            ("Account".to_string(), self.account_id.clone()),
            ("Status".to_string(), self.relationship_status.clone()),
        ]
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/guardduty/home?region={}#/accounts",
            region, region
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── GdFilter (saved filters + suppression rules) ────────────────────────────

/// A `ListFilters` entry hydrated by `GetFilter`. The tab exists mostly for
/// the `ARCHIVE` half: a suppression rule is the usual reason a finding you
/// expect to see isn't in the list, and nothing else in the app can tell you
/// one exists.
#[derive(Debug, Clone)]
pub struct GdFilter {
    pub name: String,
    pub description: String,
    /// `ARCHIVE` (suppression rule) or `NOOP` (saved filter).
    pub action: String,
    pub rank: i32,
    /// The `FindingCriteria` map flattened to `(field, condition)`, sorted by
    /// field — the map is unordered, so sorting keeps the pane stable.
    pub criteria_rows: Vec<(String, String)>,
    pub tags: HashMap<String, String>,
    pub search_blob: String,
}

impl GdFilter {
    fn from_sdk(
        name: &str,
        f: &aws_sdk_guardduty::operation::get_filter::GetFilterOutput,
    ) -> Self {
        let mut criteria_rows: Vec<(String, String)> = Vec::new();
        if let Some(c) = f.finding_criteria() {
            if let Some(map) = c.criterion() {
                let mut fields: Vec<_> = map.iter().collect();
                fields.sort_by(|a, b| a.0.cmp(b.0));
                for (field, cond) in fields {
                    criteria_rows.push((field.clone(), condition_summary(cond)));
                }
            }
        }
        let action = f
            .action()
            .map(|a| a.as_str().to_string())
            .unwrap_or_default();
        let description = f.description().unwrap_or_default().to_string();
        let search_blob = format!(
            "{} {} {} filter {}",
            name,
            description,
            action,
            criteria_rows
                .iter()
                .map(|(k, v)| format!("{} {}", k, v))
                .collect::<Vec<_>>()
                .join(" ")
        );
        Self {
            name: name.to_string(),
            description,
            action,
            rank: f.rank().unwrap_or(0),
            criteria_rows,
            tags: f
                .tags()
                .map(|t| t.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                .unwrap_or_default(),
            search_blob,
        }
    }

    pub fn is_suppression(&self) -> bool {
        self.action == "ARCHIVE"
    }
}

/// Render a `Condition`'s populated operators as one line. Several can be set
/// at once (a range is `greaterThan` + `lessThan`), so they're joined rather
/// than first-match-wins. The deprecated `eq`/`gte`/… aliases are read as a
/// fallback: filters created through older API versions still come back
/// populated on those fields rather than the current ones.
fn condition_summary(c: &Condition) -> String {
    let mut parts: Vec<String> = Vec::new();
    let list = |v: &[String]| format!("[{}]", v.join(", "));

    if !c.equals().is_empty() {
        parts.push(format!("equals {}", list(c.equals())));
    }
    if !c.not_equals().is_empty() {
        parts.push(format!("not equals {}", list(c.not_equals())));
    }
    if let Some(v) = c.greater_than() {
        parts.push(format!("> {}", v));
    }
    if let Some(v) = c.greater_than_or_equal() {
        parts.push(format!(">= {}", v));
    }
    if let Some(v) = c.less_than() {
        parts.push(format!("< {}", v));
    }
    if let Some(v) = c.less_than_or_equal() {
        parts.push(format!("<= {}", v));
    }

    #[allow(deprecated)]
    {
        if parts.is_empty() {
            if !c.eq().is_empty() {
                parts.push(format!("equals {}", list(c.eq())));
            }
            if !c.neq().is_empty() {
                parts.push(format!("not equals {}", list(c.neq())));
            }
            if let Some(v) = c.gt() {
                parts.push(format!("> {}", v));
            }
            if let Some(v) = c.gte() {
                parts.push(format!(">= {}", v));
            }
            if let Some(v) = c.lt() {
                parts.push(format!("< {}", v));
            }
            if let Some(v) = c.lte() {
                parts.push(format!("<= {}", v));
            }
        }
    }

    if parts.is_empty() {
        "(no condition)".to_string()
    } else {
        parts.join("  ·  ")
    }
}

crate::sections! {
    pub enum GdFilterDetailSection,
    pub static GD_FILTER_SECTIONS = [
        Overview "Overview",
        Criteria "Criteria",
        Tags "Tags",
    ]
}

impl Resource for GdFilter {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&GD_FILTER_SECTIONS)
    }

    fn id(&self) -> &str {
        &self.name
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn resource_type(&self) -> &str {
        "GuardDuty Filter"
    }

    /// A suppression rule reads yellow — it is actively hiding findings, which
    /// is worth noticing. A saved filter changes nothing, so it gets no dot.
    fn state(&self) -> ResourceState {
        if self.is_suppression() {
            ResourceState::Pending
        } else {
            ResourceState::Unknown(String::new())
        }
    }
    fn state_label(&self) -> String {
        // ARCHIVE = suppression rule, NOOP = saved filter.
        if self.is_suppression() { "suppression" } else { "saved filter" }.to_string()
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        self.search_blob.clone()
    }

    fn details(&self) -> Vec<(String, String)> {
        // Fallback only — the split pane is the real view.
        vec![
            ("Name".to_string(), self.name.clone()),
            ("Action".to_string(), self.action.clone()),
        ]
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/guardduty/home?region={}#/findings",
            region, region
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── GdList (trusted / threat IP and entity sets) ────────────────────────────

/// The four list types GuardDuty keeps — trusted IP sets, threat IP sets, and
/// their newer trusted/threat *entity* set equivalents — unified into one row
/// (the RDS-snapshot pattern). They answer the other half of "why don't I see
/// this": a trusted IP list suppresses findings for its addresses outright.
#[derive(Debug, Clone)]
pub struct GdList {
    pub id: String,
    pub name: String,
    /// `Trusted IP` / `Threat IP` / `Trusted Entity` / `Threat Entity`.
    pub kind: String,
    pub format: String,
    /// The S3 URL the list is loaded from.
    pub location: String,
    pub status: String,
    pub expected_bucket_owner: String,
    pub error_details: String,
    pub created: Option<String>,
    pub updated: Option<String>,
    pub tags: HashMap<String, String>,
    pub search_blob: String,
}

impl GdList {
    #[allow(clippy::too_many_arguments)]
    fn new(
        id: &str,
        name: &str,
        kind: &str,
        format: &str,
        location: &str,
        status: &str,
        expected_bucket_owner: &str,
        tags: HashMap<String, String>,
    ) -> Self {
        Self {
            id: id.to_string(),
            name: name.to_string(),
            kind: kind.to_string(),
            format: format.to_string(),
            location: location.to_string(),
            status: status.to_string(),
            expected_bucket_owner: expected_bucket_owner.to_string(),
            error_details: String::new(),
            created: None,
            updated: None,
            tags,
            search_blob: format!("{} {} {} {} {}", name, id, kind, status, location),
        }
    }

    /// True for the two lists that *suppress* findings — worth distinguishing
    /// from the threat lists, which generate them.
    pub fn is_trusted(&self) -> bool {
        self.kind.starts_with("Trusted")
    }
}

impl Resource for GdList {
    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn resource_type(&self) -> &str {
        "GuardDuty List"
    }

    fn state(&self) -> ResourceState {
        match self.status.as_str() {
            "ACTIVE" => ResourceState::Available,
            "ERROR" => ResourceState::Unavailable,
            "ACTIVATING" | "DEACTIVATING" | "DELETE_PENDING" => ResourceState::Pending,
            "INACTIVE" | "DELETED" => ResourceState::Stopped,
            other => ResourceState::Unknown(other.to_string()),
        }
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

    fn details(&self) -> Vec<(String, String)> {
        let mut rows = vec![
            ("Name".to_string(), self.name.clone()),
            ("Kind".to_string(), self.kind.clone()),
            ("Status".to_string(), self.status.clone()),
            ("Format".to_string(), self.format.clone()),
            ("Location".to_string(), self.location.clone()),
        ];
        if !self.expected_bucket_owner.is_empty() {
            rows.push((
                "Expected Bucket Owner".to_string(),
                self.expected_bucket_owner.clone(),
            ));
        }
        if !self.error_details.is_empty() {
            rows.push(("⚠ Error".to_string(), self.error_details.clone()));
        }
        if let Some(c) = &self.created {
            rows.push(("Created".to_string(), c.clone()));
        }
        if let Some(u) = &self.updated {
            rows.push(("Updated".to_string(), u.clone()));
        }
        rows.push(("List ID".to_string(), self.id.clone()));
        rows.push((String::new(), String::new()));
        rows.push((
            if self.is_trusted() {
                " Addresses on a trusted list generate no findings at all.".to_string()
            } else {
                " Activity involving these entries generates findings.".to_string()
            },
            String::new(),
        ));
        if !self.tags.is_empty() {
            rows.push((String::new(), String::new()));
            rows.push(("Tags".to_string(), String::new()));
            let mut tags: Vec<_> = self.tags.iter().collect();
            tags.sort();
            for (k, v) in tags {
                rows.push((format!("  {}", k), v.clone()));
            }
        }
        rows
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/guardduty/home?region={}#/lists",
            region, region
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── GdCoverage (runtime-agent coverage per resource) ────────────────────────

/// One row per EC2 instance / ECS cluster / EKS cluster that GuardDuty Runtime
/// Monitoring assesses. Answers "is GuardDuty actually watching this host" —
/// `is_noise()` hides the healthy ones so `a` leaves exactly the gaps.
#[derive(Debug, Clone)]
pub struct GdCoverage {
    pub id: String,
    pub account_id: String,
    /// `EC2` / `ECS` / `EKS`.
    pub kind: String,
    /// Instance id or cluster name — what the row reads as.
    pub name: String,
    pub status: String,
    /// Why coverage is unhealthy, verbatim from the API.
    pub issue: String,
    pub management_type: String,
    pub agent_version: String,
    /// Covered / compatible node or container-instance counts, where the
    /// resource kind reports them.
    pub covered: Option<i64>,
    pub compatible: Option<i64>,
    pub extra_rows: Vec<(String, String)>,
    pub updated_at: Option<String>,
    pub search_blob: String,
}

impl GdCoverage {
    fn from_sdk(c: &CoverageResource) -> Self {
        let details = c.resource_details();
        let kind = details
            .and_then(|d| d.resource_type())
            .map(|t| t.as_str().to_string())
            .unwrap_or_else(|| "Unknown".to_string());

        let mut name = String::new();
        let mut management_type = String::new();
        let mut agent_version = String::new();
        let mut covered = None;
        let mut compatible = None;
        let mut extra_rows: Vec<(String, String)> = Vec::new();

        if let Some(d) = details {
            if let Some(ec2) = d.ec2_instance_details() {
                name = ec2.instance_id().unwrap_or_default().to_string();
                management_type = ec2
                    .management_type()
                    .map(|m| m.as_str().to_string())
                    .unwrap_or_default();
                agent_version = ec2
                    .agent_details()
                    .and_then(|a| a.version())
                    .unwrap_or_default()
                    .to_string();
                push_row(&mut extra_rows, "Instance ID", ec2.instance_id());
                push_row(&mut extra_rows, "Instance Type", ec2.instance_type());
                push_row(&mut extra_rows, "ECS Cluster", ec2.cluster_arn());
            }
            if let Some(eks) = d.eks_cluster_details() {
                name = eks.cluster_name().unwrap_or_default().to_string();
                management_type = eks
                    .management_type()
                    .map(|m| m.as_str().to_string())
                    .unwrap_or_default();
                covered = eks.covered_nodes();
                compatible = eks.compatible_nodes();
                push_row(&mut extra_rows, "Cluster", eks.cluster_name());
                if let Some(a) = eks.addon_details() {
                    push_row(&mut extra_rows, "Add-on Version", a.addon_version());
                    push_row(&mut extra_rows, "Add-on Status", a.addon_status());
                    agent_version = a.addon_version().unwrap_or_default().to_string();
                }
            }
            if let Some(ecs) = d.ecs_cluster_details() {
                name = ecs.cluster_name().unwrap_or_default().to_string();
                push_row(&mut extra_rows, "Cluster", ecs.cluster_name());
                if let Some(ci) = ecs.container_instance_details() {
                    covered = ci.covered_container_instances();
                    compatible = ci.compatible_container_instances();
                }
                if let Some(f) = ecs.fargate_details() {
                    management_type = f
                        .management_type()
                        .map(|m| m.as_str().to_string())
                        .unwrap_or_default();
                    if !f.issues().is_empty() {
                        extra_rows
                            .push(("Fargate Issues".to_string(), f.issues().join("; ")));
                    }
                }
            }
        }

        let id = c.resource_id().unwrap_or_default().to_string();
        if name.is_empty() {
            name = id.clone();
        }
        let status = c
            .coverage_status()
            .map(|s| s.as_str().to_string())
            .unwrap_or_default();
        let issue = c.issue().unwrap_or_default().to_string();
        let account_id = c.account_id().unwrap_or_default().to_string();

        let search_blob = format!(
            "{} {} {} {} {} coverage",
            name, id, kind, status, account_id
        );

        Self {
            id,
            account_id,
            kind,
            name,
            status,
            issue,
            management_type,
            agent_version,
            covered,
            compatible,
            extra_rows,
            updated_at: c.updated_at().map(|t| t.to_string()),
            search_blob,
        }
    }
}

impl Resource for GdCoverage {
    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn resource_type(&self) -> &str {
        "GuardDuty Coverage"
    }

    fn state(&self) -> ResourceState {
        match self.status.as_str() {
            "HEALTHY" => ResourceState::Available,
            "UNHEALTHY" => ResourceState::Unavailable,
            other => ResourceState::Unknown(other.to_string()),
        }
    }
    fn state_label(&self) -> String {
        native_state_label(&self.status, || self.state())
    }

    /// Covered resources are the uninteresting majority — `a` narrows the tab
    /// to the hosts GuardDuty can't see.
    fn is_noise(&self) -> bool {
        self.status == "HEALTHY"
    }

    fn tags(&self) -> &HashMap<String, String> {
        static EMPTY: std::sync::OnceLock<HashMap<String, String>> = std::sync::OnceLock::new();
        EMPTY.get_or_init(HashMap::new)
    }

    fn search_text(&self) -> String {
        self.search_blob.clone()
    }

    fn details(&self) -> Vec<(String, String)> {
        let mut rows = vec![
            ("Resource".to_string(), self.name.clone()),
            ("Type".to_string(), self.kind.clone()),
            ("Coverage Status".to_string(), self.status.clone()),
        ];
        if !self.issue.is_empty() {
            rows.push(("Issue".to_string(), self.issue.clone()));
        }
        if let (Some(c), Some(t)) = (self.covered, self.compatible) {
            rows.push((
                "Nodes Covered".to_string(),
                if t > 0 {
                    format!("{} / {}", c, t)
                } else {
                    c.to_string()
                },
            ));
        }
        if !self.agent_version.is_empty() {
            rows.push(("Agent Version".to_string(), self.agent_version.clone()));
        }
        if !self.management_type.is_empty() {
            rows.push(("Managed".to_string(), self.management_type.clone()));
        }
        rows.push(("Account".to_string(), self.account_id.clone()));
        if let Some(u) = &self.updated_at {
            rows.push(("Updated".to_string(), u.clone()));
        }
        if !self.extra_rows.is_empty() {
            rows.push((String::new(), String::new()));
            rows.extend(self.extra_rows.iter().cloned());
        }
        rows
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/guardduty/home?region={}#/runtime-monitoring",
            region, region
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn push_row(rows: &mut Vec<(String, String)>, label: &str, value: Option<&str>) {
    if let Some(v) = value {
        if !v.is_empty() {
            rows.push((label.to_string(), v.to_string()));
        }
    }
}

/// Build a useful pretty-printed JSON view of the finding from the extracted
/// fields (the SDK `Finding` type isn't `Serialize`). Reads the already-built
/// `GdFinding` rather than a dozen loose parameters; only `createdAt` still
/// comes off the SDK type, which the struct doesn't keep.
fn build_raw_json(f: &Finding, g: &GdFinding) -> String {
    use serde_json::{json, Map, Value};
    let rows_to_obj = |rows: &[(String, String)]| -> Value {
        let mut m = Map::new();
        for (k, v) in rows {
            if k.trim().is_empty() {
                continue;
            }
            m.insert(k.trim().to_string(), Value::String(v.clone()));
        }
        Value::Object(m)
    };
    let v = json!({
        "id": g.id,
        "arn": g.arn,
        "type": g.finding_type,
        "title": g.title,
        "description": g.description,
        "severity": g.severity_score,
        "severityLabel": g.severity_label,
        "accountId": g.account_id,
        "region": g.region,
        "createdAt": f.created_at().unwrap_or_default(),
        "updatedAt": g.updated_at.clone().unwrap_or_default(),
        "count": g.count,
        "eventFirstSeen": g.first_seen.clone().unwrap_or_default(),
        "eventLastSeen": g.last_seen.clone().unwrap_or_default(),
        "archived": g.archived,
        "resourceRole": g.resource_role,
        "featureName": g.feature_name,
        "resourceType": g.resource_type,
        "resource": rows_to_obj(&g.resource_rows),
        "actor": rows_to_obj(&g.actor_rows),
        "runtime": rows_to_obj(&g.runtime_rows),
        "sequence": rows_to_obj(&g.sequence_rows),
        "threatIntelligence": rows_to_obj(&g.threat_intel),
    });
    serde_json::to_string_pretty(&v).unwrap_or_else(|_| "{}".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(t: &str) -> Vec<(&'static str, String)> {
        decompose_finding_type(t)
            .into_iter()
            .map(|(k, v)| {
                let k: &'static str = Box::leak(k.into_boxed_str());
                (k, v)
            })
            .collect()
    }

    #[test]
    fn decomposes_a_full_finding_type() {
        assert_eq!(
            labels("CryptoCurrency:EC2/BitcoinTool.B!DNS"),
            vec![
                ("Threat Purpose", "CryptoCurrency".to_string()),
                ("Resource Affected", "EC2".to_string()),
                ("Threat Family", "BitcoinTool".to_string()),
                ("Detection Mechanism", "B".to_string()),
                ("Artifact", "DNS".to_string()),
            ]
        );
    }

    #[test]
    fn decomposes_a_type_without_mechanism_or_artifact() {
        assert_eq!(
            labels("Recon:EC2/PortProbeUnprotectedPort"),
            vec![
                ("Threat Purpose", "Recon".to_string()),
                ("Resource Affected", "EC2".to_string()),
                ("Threat Family", "PortProbeUnprotectedPort".to_string()),
            ]
        );
    }

    #[test]
    fn decomposes_a_type_with_mechanism_but_no_artifact() {
        assert_eq!(
            labels("UnauthorizedAccess:IAMUser/MaliciousIPCaller.Custom"),
            vec![
                ("Threat Purpose", "UnauthorizedAccess".to_string()),
                ("Resource Affected", "IAMUser".to_string()),
                ("Threat Family", "MaliciousIPCaller".to_string()),
                ("Detection Mechanism", "Custom".to_string()),
            ]
        );
    }

    /// A type string that doesn't match the documented shape must degrade to
    /// nothing rather than emitting a bogus breakdown — the raw type is always
    /// shown above it either way.
    #[test]
    fn unparseable_types_yield_no_rows() {
        assert!(decompose_finding_type("").is_empty());
        assert!(decompose_finding_type("NotAFindingType").is_empty());
    }

    #[test]
    fn severity_buckets_match_the_console_labels() {
        assert_eq!(severity_label(9.5), "Critical");
        assert_eq!(severity_label(7.0), "High");
        assert_eq!(severity_label(4.0), "Medium");
        assert_eq!(severity_label(3.9), "Low");
    }

    /// The summary is the first identifier any shape supplies; later shapes
    /// must not overwrite it, but must still contribute their rows.
    #[test]
    fn first_shape_wins_the_summary() {
        let mut s = String::new();
        set_if_empty(&mut s, Some("i-123"));
        set_if_empty(&mut s, Some("bucket"));
        assert_eq!(s, "i-123");

        let mut empty = String::new();
        set_if_empty(&mut empty, Some(""));
        set_if_empty(&mut empty, None);
        set_if_empty(&mut empty, Some("later"));
        assert_eq!(empty, "later");
    }

    fn overview_with(sev: &[(&str, i32)]) -> GdOverview {
        GdOverview {
            id: "gd-summary-d".to_string(),
            detector_id: "d".to_string(),
            total_findings: sev.iter().map(|(_, n)| n).sum(),
            by_severity: sev.iter().map(|(l, n)| (l.to_string(), *n)).collect(),
            top_types: vec![],
            top_resources: vec![],
            top_accounts: vec![],
            coverage_rows: vec![],
            free_trial: vec![],
            errors: vec![],
        }
    }

    /// The summary row's dot has to agree with the finding rows below it: any
    /// outstanding Critical/High reads red, Medium yellow, nothing green.
    #[test]
    fn summary_state_tracks_the_worst_severity_present() {
        assert_eq!(
            overview_with(&[("Critical", 0), ("High", 1), ("Medium", 9), ("Low", 40)]).state(),
            ResourceState::Unavailable
        );
        assert_eq!(
            overview_with(&[("Critical", 0), ("High", 0), ("Medium", 3), ("Low", 40)]).state(),
            ResourceState::Pending
        );
        assert_eq!(
            overview_with(&[("Critical", 0), ("High", 0), ("Medium", 0), ("Low", 40)]).state(),
            ResourceState::Available
        );
        // A detector with no findings at all is healthy, not unknown.
        assert_eq!(overview_with(&[]).state(), ResourceState::Available);
    }

    /// A range is expressed as two operators on one condition, so the summary
    /// has to join them rather than report the first one it finds.
    #[test]
    fn condition_summary_joins_every_operator_set() {
        let c = Condition::builder()
            .greater_than_or_equal(4)
            .less_than(7)
            .build();
        assert_eq!(condition_summary(&c), ">= 4  ·  < 7");

        let c = Condition::builder()
            .equals("a")
            .equals("b")
            .not_equals("c")
            .build();
        assert_eq!(
            condition_summary(&c),
            "equals [a, b]  ·  not equals [c]"
        );
    }

    /// Filters written through older API versions come back on the deprecated
    /// aliases; those are read only when the current fields are empty, so a
    /// condition never renders twice.
    #[test]
    fn condition_summary_falls_back_to_deprecated_aliases() {
        #[allow(deprecated)]
        let c = Condition::builder().gte(7).build();
        assert_eq!(condition_summary(&c), ">= 7");

        #[allow(deprecated)]
        let both = Condition::builder().gte(7).greater_than_or_equal(9).build();
        assert_eq!(condition_summary(&both), ">= 9");
    }

    #[test]
    fn an_empty_condition_says_so_rather_than_rendering_blank() {
        assert_eq!(condition_summary(&Condition::builder().build()), "(no condition)");
    }

    fn member(status: &str, disabled: usize) -> GdMember {
        GdMember {
            account_id: "123456789012".to_string(),
            email: String::new(),
            relationship_status: status.to_string(),
            detector_id: String::new(),
            invited_at: String::new(),
            updated_at: String::new(),
            feature_rows: vec![],
            disabled_features: disabled,
            search_blob: String::new(),
        }
    }

    /// An account that isn't Enabled isn't monitored at all, so it must read
    /// red rather than merely "inactive"; a partial feature set is a softer
    /// warning. Only a fully-covered member counts as noise.
    #[test]
    fn member_state_and_noise_track_coverage_gaps() {
        assert_eq!(member("Enabled", 0).state(), ResourceState::Available);
        assert!(member("Enabled", 0).is_noise());

        assert_eq!(member("Enabled", 2).state(), ResourceState::Pending);
        assert!(!member("Enabled", 2).is_noise());

        assert_eq!(member("Disabled", 0).state(), ResourceState::Unavailable);
        assert_eq!(member("Removed", 0).state(), ResourceState::Unavailable);
        assert!(!member("Disabled", 0).is_noise());

        assert_eq!(member("Invited", 0).state(), ResourceState::Pending);
        assert!(!member("Invited", 0).is_noise());
    }

    #[test]
    fn wrap_words_breaks_on_word_boundaries() {
        assert_eq!(
            wrap_words("the quick brown fox jumps", 10),
            vec!["the quick", "brown fox", "jumps"]
        );
        assert!(wrap_words("", 10).is_empty());
    }
}
