use crate::aws::client::AwsClients;
use crate::aws::resource::{Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::aws::services::cloudtrail::CloudTrailEvent;
use crate::aws::services::cloudwatch::fmt_epoch_secs;
use crate::error::Result;
use crate::event::{Event, LoadProgress};
use async_trait::async_trait;
use futures::StreamExt;
use std::any::Any;
use std::collections::HashMap;
use tokio::sync::mpsc;

/// `@changes` (#241, ADR 0007) — "what changed in this account/region just before
/// this started?". The account-wide version of the `W` timeline: one list,
/// newest first, merged from sources that each answer part of it:
///
/// - **Config** (`SelectResourceConfig` on `configurationItemCaptureTime`):
///   every *recorded* resource whose configuration changed in the window —
///   one query, no sifting, but only where Config recording is on.
/// - **CloudFormation** stack events, **CodePipeline** runs, **CodeBuild**
///   builds: the deploys.
/// - **Auto Scaling** activities and **RDS** events: what AWS itself did,
///   which CloudTrail never records.
/// - **IAM and Organizations** (policies, roles, users, SCPs): global
///   services, so CloudTrail records them **only in us-east-1** — from any
///   other region they'd never appear, and even in us-east-1 a busy hour of
///   other writes would push them past the cap. So they get their own
///   us-east-1 lookups filtered server-side by `EventSource` (reads dropped
///   client-side, since `ReadOnly` can't combine with it), their own cap
///   and tab, and the policy document a change wrote in the detail pane.
///   Never noise: a policy change made by a service still changes access.
/// - **CloudTrail** `LookupEvents` with `ReadOnly=false`: *who* did it, and
///   console changes nothing else sees. This is the expensive one —
///   `LookupEvents` is 2 TPS × 50 events, shared with every other caller —
///   so it's capped at [`CT_PAGES`] pages, newest first, and a cap hit says
///   how far back it got (a load warning) rather than looking complete.
///   Events an AWS service made on your behalf, and session churn
///   (`AssumeRole`, `ConsoleLogin`, …), are `is_noise()` — `a` hides them.
///
/// The window (`[`/`]`: 15m / 1h / 6h / 24h / 7d) is variant-cached like
/// X-Ray's. Sub-tabs narrow by source via resource types. Each source
/// streams its batch when done, so a slow CloudTrail scan never holds up
/// the deploys. A failing source is a warning, never the whole load.
pub struct ChangesService {
    cloudtrail: aws_sdk_cloudtrail::Client,
    /// us-east-1, where IAM / Organizations events are recorded.
    global_trail: aws_sdk_cloudtrail::Client,
    config: aws_sdk_config::Client,
    cloudformation: aws_sdk_cloudformation::Client,
    codepipeline: aws_sdk_codepipeline::Client,
    codebuild: aws_sdk_codebuild::Client,
    autoscaling: aws_sdk_autoscaling::Client,
    rds: aws_sdk_rds::Client,
    region: String,
    window: ChangesWindow,
}

impl ChangesService {
    pub fn new(c: &AwsClients, window: ChangesWindow) -> Self {
        Self {
            cloudtrail: c.cloudtrail_client(),
            global_trail: c.for_region(crate::aws::region::Region::UsEast1).cloudtrail_client(),
            config: c.awsconfig_client(),
            cloudformation: c.cloudformation_client(),
            codepipeline: c.codepipeline_client(),
            codebuild: c.codebuild_client(),
            autoscaling: c.asg_client(),
            rds: c.rds_client(),
            region: c.current_region().as_str().to_string(),
            window,
        }
    }
}

/// CloudTrail pages per load (50 events each).
pub const CT_PAGES: usize = 10;
/// Pages per global lookup (IAM, then Organizations). Reads count against
/// it too, since they can only be dropped client-side.
pub const GLOBAL_PAGES: usize = 10;
/// The global services with their own lookup, and whose events the
/// regional scan therefore skips (no duplicates in us-east-1).
const GLOBAL_SOURCES: [(&str, ChangeSource); 2] = [
    ("iam.amazonaws.com", ChangeSource::Iam),
    ("organizations.amazonaws.com", ChangeSource::Org),
];
/// Stacks whose events are read (most recently touched first).
const MAX_STACKS: usize = 15;
const MAX_PIPELINES: usize = 200;
/// `ListBuilds` pages (100 build ids each, account-wide, newest first) —
/// each costs a `BatchGetBuilds` too.
const BUILD_PAGES: usize = 3;
const MAX_ROWS_PER_SOURCE: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChangesWindow {
    FifteenMinutes,
    #[default]
    OneHour,
    SixHours,
    OneDay,
    SevenDays,
}

impl ChangesWindow {
    pub const ALL: [ChangesWindow; 5] = [
        ChangesWindow::FifteenMinutes,
        ChangesWindow::OneHour,
        ChangesWindow::SixHours,
        ChangesWindow::OneDay,
        ChangesWindow::SevenDays,
    ];

    pub fn secs(self) -> i64 {
        match self {
            ChangesWindow::FifteenMinutes => 900,
            ChangesWindow::OneHour => 3600,
            ChangesWindow::SixHours => 6 * 3600,
            ChangesWindow::OneDay => 86400,
            ChangesWindow::SevenDays => 7 * 86400,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ChangesWindow::FifteenMinutes => "15m",
            ChangesWindow::OneHour => "1h",
            ChangesWindow::SixHours => "6h",
            ChangesWindow::OneDay => "24h",
            ChangesWindow::SevenDays => "7d",
        }
    }

    pub fn wider(self) -> Self {
        let i = Self::ALL.iter().position(|w| *w == self).unwrap_or(0);
        Self::ALL[(i + 1).min(Self::ALL.len() - 1)]
    }

    pub fn narrower(self) -> Self {
        let i = Self::ALL.iter().position(|w| *w == self).unwrap_or(0);
        Self::ALL[i.saturating_sub(1)]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeSource {
    Iam,
    Org,
    Trail,
    Config,
    Stack,
    Pipeline,
    Build,
    Scaling,
    Rds,
}

impl ChangeSource {
    /// Fixed-width badge, the `W` timeline's where they overlap.
    pub fn badge(self) -> &'static str {
        match self {
            ChangeSource::Iam => "IAM",
            ChangeSource::Org => "ORG",
            ChangeSource::Trail => " CT",
            ChangeSource::Config => "CFG",
            ChangeSource::Stack => "CFN",
            ChangeSource::Pipeline => "CP ",
            ChangeSource::Build => "CB ",
            ChangeSource::Scaling => "ASG",
            ChangeSource::Rds => "RDS",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ChangeSource::Iam => "IAM",
            ChangeSource::Org => "Organizations",
            ChangeSource::Trail => "CloudTrail",
            ChangeSource::Config => "AWS Config",
            ChangeSource::Stack => "CloudFormation",
            ChangeSource::Pipeline => "CodePipeline",
            ChangeSource::Build => "CodeBuild",
            ChangeSource::Scaling => "Auto Scaling",
            ChangeSource::Rds => "RDS events",
        }
    }

    /// The row's resource type — what the sub-tabs filter on.
    pub fn resource_type(self) -> &'static str {
        match self {
            ChangeSource::Iam | ChangeSource::Org => "IAM Change",
            ChangeSource::Trail => "Trail Change",
            ChangeSource::Config => "Config Change",
            ChangeSource::Stack => "Stack Change",
            ChangeSource::Pipeline => "Pipeline Run",
            ChangeSource::Build => "Build Run",
            ChangeSource::Scaling => "Scaling Activity",
            ChangeSource::Rds => "RDS Event",
        }
    }
}

/// Session and credential churn: write events by CloudTrail's definition,
/// not changes to anything.
const SESSION_CHURN: &[&str] = &[
    "AssumeRole",
    "AssumeRoleWithSAML",
    "AssumeRoleWithWebIdentity",
    "GetSessionToken",
    "GetFederationToken",
    "ConsoleLogin",
    "CreateLogStream",
    "StartSession",
    "ResumeSession",
    "TerminateSession",
];

#[derive(Debug, Clone)]
pub struct ChangeEvent {
    pub id: String,
    pub source: ChangeSource,
    pub ts_secs: i64,
    /// List title: `10-21 14:02  UpdateService  orders-api`.
    pub title: String,
    pub what: String,
    pub who: String,
    pub target_type: String,
    /// ARN or id — the detail pane's jumpable `Resource` row.
    pub target_ref: String,
    pub region: String,
    pub detail: String,
    pub error: String,
    pub failed: bool,
    /// An AWS service acting for you, or session churn (`is_noise`).
    pub machine: bool,
    /// CloudTrail's raw event JSON (`e`).
    pub raw: Option<String>,
    /// IAM / Organizations: the request's scalar parameters
    /// (`roleName`, `policyArn`, …) — what was attached to what.
    pub request: Vec<(String, String)>,
    /// IAM / Organizations: the policy document the change wrote, pretty.
    pub policy: Option<String>,
    tags: HashMap<String, String>,
}

impl ChangeEvent {
    #[allow(clippy::too_many_arguments)]
    fn new(
        source: ChangeSource,
        id: String,
        ts_secs: i64,
        what: String,
        who: String,
        target: String,
        target_type: String,
        target_ref: String,
        region: String,
    ) -> Self {
        let stamp = fmt_epoch_secs(ts_secs);
        let short = stamp.get(5..).unwrap_or(&stamp).to_string();
        let title = if target.is_empty() {
            format!("{short}  {what}")
        } else {
            format!("{short}  {what}  {target}")
        };
        Self {
            id,
            source,
            ts_secs,
            title,
            what,
            who,
            target_type,
            target_ref,
            region,
            detail: String::new(),
            error: String::new(),
            failed: false,
            machine: false,
            raw: None,
            request: Vec::new(),
            policy: None,
            tags: HashMap::new(),
        }
    }

    pub fn from_trail(ev: &CloudTrailEvent) -> Self {
        let source = GLOBAL_SOURCES
            .iter()
            .find(|(s, _)| *s == ev.event_source)
            .map(|(_, src)| *src)
            .unwrap_or(ChangeSource::Trail);
        let json = serde_json::from_str::<serde_json::Value>(&ev.cloud_trail_event).ok();
        let invoked_by = json
            .as_ref()
            .and_then(|v| v.pointer("/userIdentity/invokedBy").and_then(|s| s.as_str()).map(str::to_string))
            .unwrap_or_default();
        // Prefer an ARN for the jump; show its readable tail.
        let (rtype, rref) = ev
            .resources
            .iter()
            .find(|(_, r)| r.starts_with("arn:"))
            .or_else(|| ev.resources.first())
            .cloned()
            .unwrap_or_else(|| (ev.resource_type.clone(), ev.resource_name.clone()));
        let target = readable_tail(&rref);
        let mut who = crate::timeline::display_user(ev);
        if !invoked_by.is_empty() {
            who = format!("{who} via {invoked_by}");
        }
        let mut c = Self::new(
            source,
            ev.event_id.clone(),
            ev.event_time_secs,
            ev.event_name.clone(),
            who,
            target,
            rtype,
            rref,
            ev.aws_region.clone(),
        );
        c.detail = [ev.event_source.as_str(), ev.source_ip.as_str()]
            .iter()
            .filter(|s| !s.is_empty())
            .cloned()
            .collect::<Vec<_>>()
            .join(" · ");
        c.error = match (ev.error_code.is_empty(), ev.error_message.is_empty()) {
            (true, _) => String::new(),
            (false, true) => ev.error_code.clone(),
            (false, false) => format!("{}: {}", ev.error_code, ev.error_message),
        };
        c.failed = !ev.error_code.is_empty();
        c.machine = match source {
            // A role or policy changed by a service still changes access.
            ChangeSource::Iam | ChangeSource::Org => SESSION_CHURN.contains(&ev.event_name.as_str()),
            _ => trail_is_machine(ev, &invoked_by),
        };
        if matches!(source, ChangeSource::Iam | ChangeSource::Org) {
            if let Some(params) = json.as_ref().and_then(|v| v.get("requestParameters")).and_then(|p| p.as_object()) {
                c.request = params
                    .iter()
                    .filter(|(k, _)| !POLICY_KEYS.contains(&k.as_str()))
                    .filter_map(|(k, v)| match v {
                        serde_json::Value::String(s) => Some((k.clone(), s.clone())),
                        serde_json::Value::Number(_) | serde_json::Value::Bool(_) => Some((k.clone(), v.to_string())),
                        _ => None,
                    })
                    .collect();
                c.policy = POLICY_KEYS
                    .iter()
                    .find_map(|k| params.get(*k).and_then(|v| v.as_str()))
                    .map(pretty_policy);
            }
        }
        c.raw = Some(ev.cloud_trail_event.clone());
        c
    }

    /// One CodeBuild build: `build SUCCEEDED  orders-worker-image #88`,
    /// jumping to its project.
    pub fn from_build(b: &aws_sdk_codebuild::types::Build, region: &str) -> Option<Self> {
        let id = b.id()?.to_string();
        let project = b.project_name().unwrap_or_default().to_string();
        let status = b.build_status().map(|s| s.as_str().to_string()).unwrap_or_default();
        let ts = b.end_time().or(b.start_time()).map(|t| t.secs()).unwrap_or(0);
        // `arn:…:build/<project>:<uuid>` → `arn:…:project/<project>`, the
        // id the Build Projects tab keys on.
        let project_arn = b
            .arn()
            .and_then(|a| a.split_once(":build/"))
            .map(|(head, _)| format!("{head}:project/{project}"))
            .unwrap_or_else(|| project.clone());
        let target = match b.build_number() {
            Some(n) => format!("{project} #{n}"),
            None => project.clone(),
        };
        let mut c = Self::new(
            ChangeSource::Build,
            id,
            ts,
            format!("build {status}"),
            b.initiator().unwrap_or_default().to_string(),
            target,
            "AWS::CodeBuild::Project".to_string(),
            project_arn,
            region.to_string(),
        );
        let version = b.resolved_source_version().or(b.source_version()).unwrap_or_default();
        let short = version.get(..12).unwrap_or(version);
        c.detail = [b.current_phase().unwrap_or_default(), short]
            .iter()
            .filter(|s| !s.is_empty() && **s != "COMPLETED")
            .cloned()
            .collect::<Vec<_>>()
            .join(" · ");
        c.failed = matches!(status.as_str(), "FAILED" | "FAULT" | "TIMED_OUT");
        Some(c)
    }

    /// One `SelectResourceConfig` result row (a JSON object string).
    pub fn from_config_row(row: &str, fallback_region: &str) -> Option<Self> {
        let v: serde_json::Value = serde_json::from_str(row).ok()?;
        let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        let rtype = s("resourceType");
        // Compliance evaluations are recorded as configuration items too;
        // they are Config's own bookkeeping, not changes.
        if rtype == "AWS::Config::ResourceCompliance" {
            return None;
        }
        let ts = parse_iso_secs(&s("configurationItemCaptureTime"))?;
        let id = s("resourceId");
        let name = if s("resourceName").is_empty() { id.clone() } else { s("resourceName") };
        let what = match s("configurationItemStatus").as_str() {
            "ResourceDiscovered" => "created",
            "ResourceDeleted" | "ResourceDeletedNotRecorded" => "deleted",
            _ => "configuration changed",
        };
        let arn = s("arn");
        let region = if s("awsRegion").is_empty() { fallback_region.to_string() } else { s("awsRegion") };
        let mut c = Self::new(
            ChangeSource::Config,
            format!("config:{rtype}:{id}:{ts}"),
            ts,
            format!("{} {what}", short_cfn_type(&rtype)),
            String::new(),
            name,
            rtype,
            if arn.is_empty() { id } else { arn },
            region,
        );
        c.detail = "Config records what changed, not who — see the CloudTrail tab for the call".to_string();
        Some(c)
    }
}

/// The readable end of an ARN: its resource part after the last `/`
/// (`task-definition/orders-worker:15` → `orders-worker:15`), or after the
/// type prefix (`function:orders-api` → `orders-api`); a non-ARN
/// is returned as is.
fn readable_tail(r: &str) -> String {
    if !r.starts_with("arn:") {
        return r.to_string();
    }
    let resource = r.splitn(6, ':').nth(5).unwrap_or(r);
    match (resource.rsplit_once('/'), resource.split_once(':')) {
        (Some((_, tail)), _) => tail.to_string(),
        // `function:orders-api`, `db:orders`
        (None, Some((_, rest))) => rest.to_string(),
        (None, None) => resource.to_string(),
    }
}

/// Request parameters that carry a policy document: IAM's inline and
/// managed policies + trust policies, Organizations' SCP `content`.
const POLICY_KEYS: [&str; 3] = ["policyDocument", "assumeRolePolicyDocument", "content"];

/// A policy document as CloudTrail recorded it (a JSON string, sometimes
/// URL-encoded), pretty-printed; left as is when it isn't JSON.
fn pretty_policy(doc: &str) -> String {
    let decoded = if doc.trim_start().starts_with('%') { percent_decode(doc) } else { doc.to_string() };
    serde_json::from_str::<serde_json::Value>(&decoded)
        .ok()
        .and_then(|v| serde_json::to_string_pretty(&v).ok())
        .unwrap_or(decoded)
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = (bytes[i] == b'%' && i + 2 < bytes.len())
            .then(|| u8::from_str_radix(&s[i + 1..i + 3], 16).ok())
            .flatten();
        match (hex, bytes[i]) {
            (Some(b), _) => {
                out.push(b);
                i += 3;
            }
            (None, b'+') => {
                out.push(b' ');
                i += 1;
            }
            (None, b) => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `AWS::EC2::SecurityGroup` → `SecurityGroup`.
fn short_cfn_type(t: &str) -> &str {
    t.rsplit("::").next().unwrap_or(t)
}

fn trail_is_machine(ev: &CloudTrailEvent, invoked_by: &str) -> bool {
    ev.identity_type == "AWSService"
        || !invoked_by.is_empty()
        || ev.session_issuer_arn.contains(":role/aws-service-role/")
        || SESSION_CHURN.contains(&ev.event_name.as_str())
}

fn parse_iso_secs(s: &str) -> Option<i64> {
    aws_smithy_types::DateTime::from_str(s, aws_smithy_types::date_time::Format::DateTime)
        .ok()
        .map(|d| d.secs())
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

impl Resource for ChangeEvent {
    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        &self.title
    }

    fn resource_type(&self) -> &str {
        self.source.resource_type()
    }

    fn state(&self) -> ResourceState {
        if self.failed {
            ResourceState::Unavailable
        } else {
            ResourceState::stateless()
        }
    }

    fn state_label(&self) -> String {
        if self.failed { "failed".to_string() } else { String::new() }
    }

    fn list_summary(&self) -> Option<String> {
        let who = if self.who.is_empty() { "—" } else { &self.who };
        Some(format!("{} · {}", self.source.badge().trim(), who))
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!(
            "{} {} {} {} {} {} {} {}",
            self.request.iter().map(|(_, v)| v.as_str()).collect::<Vec<_>>().join(" "),
            self.title,
            self.who,
            self.target_type,
            self.target_ref,
            self.detail,
            self.error,
            self.source.label()
        )
    }

    fn is_noise(&self) -> bool {
        self.machine
    }

    fn details(&self) -> Vec<(String, String)> {
        let mut d = vec![
            ("Time".to_string(), format!("{} UTC", fmt_epoch_secs(self.ts_secs))),
            ("Source".to_string(), self.source.label().to_string()),
            ("Change".to_string(), self.what.clone()),
        ];
        if !self.who.is_empty() {
            let who = if self.machine && self.source == ChangeSource::Trail {
                format!("{} (AWS service / session — `a` hides these)", self.who)
            } else {
                self.who.clone()
            };
            d.push(("Actor".to_string(), who));
        }
        if !self.target_type.is_empty() {
            d.push(("Resource Type".to_string(), self.target_type.clone()));
        }
        if !self.target_ref.is_empty() {
            d.push(("Resource".to_string(), self.target_ref.clone()));
        }
        if !self.region.is_empty() {
            d.push(("Region".to_string(), self.region.clone()));
        }
        if !self.detail.is_empty() {
            d.push(("Detail".to_string(), self.detail.clone()));
        }
        if !self.error.is_empty() {
            d.push(("Error".to_string(), format!("✗ {}", self.error)));
        }
        if !self.request.is_empty() {
            d.push(("".to_string(), "".to_string()));
            d.push(("Request".to_string(), "".to_string()));
            d.extend(self.request.iter().cloned());
        }
        if let Some(policy) = &self.policy {
            d.push(("".to_string(), "".to_string()));
            d.push(("Policy document".to_string(), "".to_string()));
            d.extend(policy.lines().map(|l| (format!("  {l}"), String::new())));
        }
        if self.raw.is_some() {
            d.push(("".to_string(), "".to_string()));
            d.push(("".to_string(), "· e opens the raw CloudTrail event".to_string()));
        }
        d
    }

    fn raw_content(&self) -> Option<String> {
        let raw = self.raw.as_ref()?;
        Some(
            serde_json::from_str::<serde_json::Value>(raw)
                .ok()
                .and_then(|v| serde_json::to_string_pretty(&v).ok())
                .unwrap_or_else(|| raw.clone()),
        )
    }

    fn console_url(&self, region: &str) -> Option<String> {
        // Global events live in us-east-1's event history.
        let region = if matches!(self.source, ChangeSource::Iam | ChangeSource::Org) { "us-east-1" } else { region };
        self.raw.is_some().then(|| {
            format!(
                "https://{region}.console.aws.amazon.com/cloudtrailv2/home?region={region}#/events/{}",
                self.id
            )
        })
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[async_trait]
impl AwsService for ChangesService {
    fn service_type(&self) -> ServiceType {
        ServiceType::Changes
    }

    fn name(&self) -> &str {
        "Changes"
    }

    async fn list_resources(&self) -> Result<Vec<Box<dyn Resource>>> {
        let (tx, _rx) = mpsc::unbounded_channel();
        self.list_resources_streaming(tx, ServiceType::Changes).await?;
        Ok(vec![])
    }

    async fn list_resources_streaming(
        &self,
        event_tx: mpsc::UnboundedSender<Event>,
        service_type: ServiceType,
    ) -> Result<()> {
        let cutoff = now_secs() - self.window.secs();
        let region = self.region.clone();
        let total = std::sync::atomic::AtomicUsize::new(0);
        let send = |source: ChangeSource, rows: Vec<ChangeEvent>| {
            if rows.is_empty() {
                return;
            }
            let loaded = total.fetch_add(rows.len(), std::sync::atomic::Ordering::Relaxed) + rows.len();
            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                service: service_type,
                resources: rows.into_iter().map(|r| Box::new(r) as Box<dyn Resource>).collect(),
                progress: LoadProgress {
                    loaded_count: loaded,
                    total_count: None,
                    status_message: Some(format!("{} loaded", source.label())),
                },
            });
        };
        let warn = |source: ChangeSource, msg: String| {
            let _ = event_tx.send(Event::ResourceLoadWarning {
                service: service_type,
                warning: format!("{}: {msg}", source.label()),
            });
        };

        let back_to = |t: i64| fmt_epoch_secs(t).get(5..).unwrap_or_default().to_string();
        let trail = async {
            let regional = |ev: &CloudTrailEvent| !GLOBAL_SOURCES.iter().any(|(s, _)| *s == ev.event_source);
            match trail_changes(self.cloudtrail.clone(), TrailScan::WritesOnly, CT_PAGES, cutoff, &regional, &|rows| {
                send(ChangeSource::Trail, rows)
            })
            .await
            {
                Ok(Some(oldest)) => warn(
                    ChangeSource::Trail,
                    format!(
                        "newest {} write events only (back to {} UTC) — [ narrows the window",
                        CT_PAGES * 50,
                        back_to(oldest)
                    ),
                ),
                Ok(None) => {}
                Err(e) => warn(ChangeSource::Trail, e),
            }
        };
        // IAM, then Organizations, one after the other: both spend
        // us-east-1's 2 TPS.
        let global = async {
            for (event_source, source) in GLOBAL_SOURCES {
                let scan = TrailScan::Source(event_source);
                let writes = |ev: &CloudTrailEvent| !ev.read_only && ev.event_source == event_source;
                match trail_changes(self.global_trail.clone(), scan, GLOBAL_PAGES, cutoff, &writes, &|rows| {
                    send(source, rows)
                })
                .await
                {
                    Ok(Some(oldest)) => warn(
                        source,
                        format!(
                            "scanned the newest {} {} events, reads included (back to {} UTC) — [ narrows the window",
                            GLOBAL_PAGES * 50,
                            source.label(),
                            back_to(oldest)
                        ),
                    ),
                    Ok(None) => {}
                    Err(e) => warn(source, e),
                }
            }
        };
        let config = async {
            match config_changes(self.config.clone(), cutoff, &region).await {
                Ok(rows) => send(ChangeSource::Config, rows),
                // Not recording here: an expected gap, not news.
                Err(e) if e.contains("NoSuchConfigurationRecorder") => {}
                Err(e) => warn(ChangeSource::Config, e),
            }
        };
        let stacks = async {
            match stack_changes(self.cloudformation.clone(), cutoff, &region).await {
                Ok(rows) => send(ChangeSource::Stack, rows),
                Err(e) => warn(ChangeSource::Stack, e),
            }
        };
        let pipelines = async {
            match pipeline_changes(self.codepipeline.clone(), cutoff, &region).await {
                Ok(rows) => send(ChangeSource::Pipeline, rows),
                Err(e) => warn(ChangeSource::Pipeline, e),
            }
        };
        let builds = async {
            match build_changes(self.codebuild.clone(), cutoff, &region).await {
                Ok((rows, capped)) => {
                    send(ChangeSource::Build, rows);
                    if let Some(oldest) = capped {
                        warn(
                            ChangeSource::Build,
                            format!(
                                "newest {} builds only (back to {} UTC) — [ narrows the window",
                                BUILD_PAGES * 100,
                                back_to(oldest)
                            ),
                        );
                    }
                }
                Err(e) => warn(ChangeSource::Build, e),
            }
        };
        let scaling = async {
            match scaling_changes(self.autoscaling.clone(), cutoff, &region).await {
                Ok(rows) => send(ChangeSource::Scaling, rows),
                Err(e) => warn(ChangeSource::Scaling, e),
            }
        };
        let rds = async {
            match rds_changes(self.rds.clone(), self.window.secs(), &region).await {
                Ok(rows) => send(ChangeSource::Rds, rows),
                Err(e) => warn(ChangeSource::Rds, e),
            }
        };
        tokio::join!(trail, global, config, stacks, pipelines, builds, scaling, rds);

        let _ = event_tx.send(Event::ResourcesFullyLoaded {
            service: service_type,
            total_count: total.load(std::sync::atomic::Ordering::Relaxed),
        });
        Ok(())
    }

    async fn get_resource_details(&self, _id: &str) -> Result<Box<dyn Resource>> {
        Err(crate::error::Error::NotImplemented)
    }
}

/// The one server-side `LookupEvents` filter a scan spends.
#[derive(Clone, Copy)]
enum TrailScan {
    WritesOnly,
    Source(&'static str),
}

/// Events newest first, streamed a page at a time, `keep` deciding which
/// become rows. `Ok(Some(t))` means the page cap stopped the scan with
/// events older than `t` (scanned, kept or not) still unread — the caller
/// says so.
async fn trail_changes(
    client: aws_sdk_cloudtrail::Client,
    scan: TrailScan,
    max_pages: usize,
    cutoff: i64,
    keep: &(dyn Fn(&CloudTrailEvent) -> bool + Sync),
    send: &(dyn Fn(Vec<ChangeEvent>) + Sync),
) -> std::result::Result<Option<i64>, String> {
    use aws_sdk_cloudtrail::types::{LookupAttribute, LookupAttributeKey};
    let (key, value) = match scan {
        TrailScan::WritesOnly => (LookupAttributeKey::ReadOnly, "false"),
        TrailScan::Source(s) => (LookupAttributeKey::EventSource, s),
    };
    let attr = LookupAttribute::builder()
        .attribute_key(key)
        .attribute_value(value)
        .build()
        .map_err(|e| e.to_string())?;
    let mut pages = client
        .lookup_events()
        .lookup_attributes(attr)
        .start_time(aws_smithy_types::DateTime::from_secs(cutoff))
        .end_time(aws_smithy_types::DateTime::from_secs(now_secs()))
        .max_results(50)
        .into_paginator()
        .send();
    let mut read = 0usize;
    let mut oldest = None;
    while let Some(page) = pages.next().await {
        let page = page.map_err(|e| crate::error::sdk_error_message(&e))?;
        let events: Vec<CloudTrailEvent> = page
            .events()
            .iter()
            .map(CloudTrailEvent::from_sdk)
            .filter(|e| e.event_time_secs >= cutoff)
            .collect();
        oldest = events.iter().map(|e| e.event_time_secs).min().or(oldest);
        send(events.iter().filter(|e| keep(e)).map(ChangeEvent::from_trail).collect());
        read += 1;
        if read >= max_pages {
            return Ok(page.next_token().is_some().then_some(oldest.unwrap_or(cutoff)));
        }
    }
    Ok(None)
}

async fn config_changes(
    client: aws_sdk_config::Client,
    cutoff: i64,
    region: &str,
) -> std::result::Result<Vec<ChangeEvent>, String> {
    let since = aws_smithy_types::DateTime::from_secs(cutoff)
        .fmt(aws_smithy_types::date_time::Format::DateTime)
        .map_err(|e| e.to_string())?;
    let expression = format!(
        "SELECT resourceId, resourceName, resourceType, arn, awsRegion, \
         configurationItemCaptureTime, configurationItemStatus \
         WHERE configurationItemCaptureTime >= '{since}'"
    );
    let mut rows = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let resp = client
            .select_resource_config()
            .expression(&expression)
            .limit(100)
            .set_next_token(token.clone())
            .send()
            .await
            .map_err(|e| crate::error::sdk_error_message(&e))?;
        rows.extend(resp.results().iter().filter_map(|r| ChangeEvent::from_config_row(r, region)));
        token = crate::aws::pagination::next_page_token(resp.next_token(), &token);
        if token.is_none() || rows.len() >= MAX_ROWS_PER_SOURCE {
            break;
        }
    }
    Ok(rows)
}

/// Stacks touched in the window (created, updated or deleted — `ListStacks`
/// keeps deleted ones 90 days), then each one's stack-level events plus
/// any failed resource events. A stack update writes a row per resource;
/// the stack's own status rows and the failures are the signal.
async fn stack_changes(
    client: aws_sdk_cloudformation::Client,
    cutoff: i64,
    region: &str,
) -> std::result::Result<Vec<ChangeEvent>, String> {
    let mut touched: Vec<(i64, String, String)> = Vec::new();
    let mut pages = client.list_stacks().into_paginator().send();
    while let Some(page) = pages.next().await {
        let page = page.map_err(|e| crate::error::sdk_error_message(&e))?;
        for s in page.stack_summaries() {
            let last = [s.creation_time(), s.last_updated_time(), s.deletion_time()]
                .into_iter()
                .flatten()
                .map(|t| t.secs())
                .max()
                .unwrap_or(0);
            if last >= cutoff {
                touched.push((
                    last,
                    s.stack_id().unwrap_or_default().to_string(),
                    s.stack_name().unwrap_or_default().to_string(),
                ));
            }
        }
    }
    touched.sort_by_key(|t| std::cmp::Reverse(t.0));
    touched.truncate(MAX_STACKS);

    let results: Vec<_> = futures::stream::iter(touched)
        .map(|(_, stack_id, stack_name)| {
            let client = client.clone();
            async move {
                let resp = client.describe_stack_events().stack_name(&stack_id).send().await;
                (stack_id, stack_name, resp)
            }
        })
        .buffered(3)
        .collect()
        .await;

    let mut rows = Vec::new();
    for (stack_id, stack_name, resp) in results {
        let Ok(resp) = resp else { continue };
        for e in resp.stack_events() {
            let ts = e.timestamp().map(|t| t.secs()).unwrap_or(0);
            if ts < cutoff {
                continue;
            }
            let logical = e.logical_resource_id().unwrap_or_default();
            let status = e.resource_status().map(|s| s.as_str().to_string()).unwrap_or_default();
            let failed = status.contains("FAILED") || status.contains("ROLLBACK");
            let stack_level = logical == stack_name;
            if !stack_level && !failed {
                continue;
            }
            let (target, rtype, rref) = if stack_level {
                (stack_name.clone(), "AWS::CloudFormation::Stack".to_string(), stack_id.clone())
            } else {
                (
                    format!("{stack_name}/{logical}"),
                    e.resource_type().unwrap_or_default().to_string(),
                    e.physical_resource_id().unwrap_or_default().to_string(),
                )
            };
            let mut c = ChangeEvent::new(
                ChangeSource::Stack,
                e.event_id().unwrap_or_default().to_string(),
                ts,
                status,
                String::new(),
                target,
                rtype,
                rref,
                region.to_string(),
            );
            c.detail = e.resource_status_reason().unwrap_or_default().to_string();
            c.failed = failed;
            rows.push(c);
        }
    }
    Ok(rows)
}

/// Each pipeline's recent runs that started or finished in the window.
async fn pipeline_changes(
    client: aws_sdk_codepipeline::Client,
    cutoff: i64,
    region: &str,
) -> std::result::Result<Vec<ChangeEvent>, String> {
    let mut names = Vec::new();
    let mut pages = client.list_pipelines().into_paginator().send();
    while let Some(page) = pages.next().await {
        let page = page.map_err(|e| crate::error::sdk_error_message(&e))?;
        names.extend(page.pipelines().iter().filter_map(|p| p.name().map(str::to_string)));
        if names.len() >= MAX_PIPELINES {
            break;
        }
    }
    names.truncate(MAX_PIPELINES);

    let results: Vec<_> = futures::stream::iter(names)
        .map(|name| {
            let client = client.clone();
            async move {
                let resp = client.list_pipeline_executions().pipeline_name(&name).max_results(5).send().await;
                (name, resp)
            }
        })
        .buffered(4)
        .collect()
        .await;

    let mut rows = Vec::new();
    for (name, resp) in results {
        let Ok(resp) = resp else { continue };
        for e in resp.pipeline_execution_summaries() {
            let ts = e
                .last_update_time()
                .or(e.start_time())
                .map(|t| t.secs())
                .unwrap_or(0);
            if ts < cutoff {
                continue;
            }
            let status = e.status().map(|s| s.as_str().to_string()).unwrap_or_default();
            let trigger = e
                .trigger()
                .map(|t| {
                    let kind = t.trigger_type().map(|k| k.as_str().to_string()).unwrap_or_default();
                    match t.trigger_detail() {
                        Some(d) if !d.is_empty() => format!("{kind} {}", d.rsplit('/').next().unwrap_or(d)),
                        _ => kind,
                    }
                })
                .unwrap_or_default();
            let exec_id = e.pipeline_execution_id().unwrap_or_default().to_string();
            let mut c = ChangeEvent::new(
                ChangeSource::Pipeline,
                format!("pipeline:{name}:{exec_id}"),
                ts,
                format!("run {status}"),
                trigger,
                name.clone(),
                "AWS::CodePipeline::Pipeline".to_string(),
                name.clone(),
                region.to_string(),
            );
            c.detail = e
                .source_revisions()
                .iter()
                .filter_map(|r| r.revision_summary())
                .next()
                .unwrap_or_default()
                .lines()
                .next()
                .unwrap_or_default()
                .to_string();
            c.failed = status == "Failed";
            rows.push(c);
        }
    }
    Ok(rows)
}

/// Builds that started in the window, from every project: `ListBuilds` is
/// account-wide and newest first, so it's paged until a build starts before
/// the cutoff. `Some(t)` in the result means the page cap stopped it first,
/// with builds older than `t` unread.
async fn build_changes(
    client: aws_sdk_codebuild::Client,
    cutoff: i64,
    region: &str,
) -> std::result::Result<(Vec<ChangeEvent>, Option<i64>), String> {
    use aws_sdk_codebuild::types::SortOrderType;
    let mut rows = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..BUILD_PAGES {
        let page = client
            .list_builds()
            .sort_order(SortOrderType::Descending)
            .set_next_token(token.clone())
            .send()
            .await
            .map_err(|e| crate::error::sdk_error_message(&e))?;
        if page.ids().is_empty() {
            return Ok((rows, None));
        }
        let builds = client
            .batch_get_builds()
            .set_ids(Some(page.ids().to_vec()))
            .send()
            .await
            .map_err(|e| crate::error::sdk_error_message(&e))?;
        let mut builds: Vec<_> = builds.builds().iter().collect();
        builds.sort_by_key(|b| std::cmp::Reverse(b.start_time().map(|t| t.secs()).unwrap_or(0)));
        for b in builds {
            let started = b.start_time().map(|t| t.secs()).unwrap_or(0);
            if started < cutoff {
                return Ok((rows, None));
            }
            if let Some(c) = ChangeEvent::from_build(b, region) {
                rows.push(c);
            }
        }
        token = crate::aws::pagination::next_page_token(page.next_token(), &token);
        if token.is_none() {
            return Ok((rows, None));
        }
    }
    let oldest = rows.iter().map(|r| r.ts_secs).min();
    Ok((rows, oldest))
}

/// Every group's activities, newest first (no group filter = account-wide),
/// paged until older than the window.
async fn scaling_changes(
    client: aws_sdk_autoscaling::Client,
    cutoff: i64,
    region: &str,
) -> std::result::Result<Vec<ChangeEvent>, String> {
    let mut rows = Vec::new();
    let mut token: Option<String> = None;
    'pages: loop {
        let resp = client
            .describe_scaling_activities()
            .max_records(100)
            .set_next_token(token.clone())
            .send()
            .await
            .map_err(|e| crate::error::sdk_error_message(&e))?;
        for a in resp.activities() {
            let ts = a.start_time().map(|t| t.secs()).unwrap_or(0);
            if ts < cutoff {
                break 'pages;
            }
            let status = a.status_code().map(|s| s.as_str().to_string()).unwrap_or_default();
            let group = a.auto_scaling_group_name().unwrap_or_default().to_string();
            let mut c = ChangeEvent::new(
                ChangeSource::Scaling,
                a.activity_id().unwrap_or_default().to_string(),
                ts,
                a.description().unwrap_or_default().to_string(),
                status.clone(),
                group.clone(),
                "AWS::AutoScaling::AutoScalingGroup".to_string(),
                a.auto_scaling_group_arn().unwrap_or(&group).to_string(),
                region.to_string(),
            );
            c.detail = a.cause().unwrap_or_default().to_string();
            c.failed = matches!(status.as_str(), "Failed" | "Cancelled");
            rows.push(c);
        }
        token = crate::aws::pagination::next_page_token(resp.next_token(), &token);
        if token.is_none() || rows.len() >= MAX_ROWS_PER_SOURCE {
            break;
        }
    }
    Ok(rows)
}

/// Account-wide RDS events over the window (RDS keeps 14 days).
async fn rds_changes(
    client: aws_sdk_rds::Client,
    window_secs: i64,
    region: &str,
) -> std::result::Result<Vec<ChangeEvent>, String> {
    let minutes = (window_secs / 60).clamp(1, 14 * 24 * 60) as i32;
    let mut rows = Vec::new();
    let mut pages = client.describe_events().duration(minutes).max_records(100).into_paginator().send();
    while let Some(page) = pages.next().await {
        let page = page.map_err(|e| crate::error::sdk_error_message(&e))?;
        for e in page.events() {
            let ts = e.date().map(|t| t.secs()).unwrap_or(0);
            let source_id = e.source_identifier().unwrap_or_default().to_string();
            let categories: Vec<String> = e.event_categories().iter().map(|c| c.to_string()).collect();
            let mut c = ChangeEvent::new(
                ChangeSource::Rds,
                format!("rds:{source_id}:{ts}:{}", rows.len()),
                ts,
                e.message().unwrap_or_default().to_string(),
                categories.join(", "),
                source_id.clone(),
                e.source_type().map(|t| t.as_str().to_string()).unwrap_or_default(),
                e.source_arn().unwrap_or(&source_id).to_string(),
                region.to_string(),
            );
            c.failed = categories.iter().any(|c| matches!(c.as_str(), "failure" | "low storage" | "failover"));
            rows.push(c);
        }
        if rows.len() >= MAX_ROWS_PER_SOURCE {
            break;
        }
    }
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_jump_to_their_project_and_fail_on_fault_or_timeout() {
        use aws_sdk_codebuild::types::{Build, StatusType};
        let build = |status: StatusType| {
            Build::builder()
                .id("img:abc")
                .arn("arn:aws:codebuild:us-east-1:1:build/img:abc")
                .project_name("img")
                .build_number(7)
                .build_status(status)
                .initiator("deploy-role/GitHubActions")
                .resolved_source_version("9f3c2a1b7e4d5c6b8a9f")
                .current_phase("COMPLETED")
                .start_time(aws_smithy_types::DateTime::from_secs(1_790_000_000))
                .build()
        };
        let c = ChangeEvent::from_build(&build(StatusType::Succeeded), "us-east-1").unwrap();
        assert_eq!(c.target_ref, "arn:aws:codebuild:us-east-1:1:project/img");
        assert_eq!(c.what, "build SUCCEEDED");
        assert_eq!(c.who, "deploy-role/GitHubActions");
        assert_eq!(c.detail, "9f3c2a1b7e4d", "short sha; COMPLETED says nothing");
        assert!(c.title.ends_with("img #7"));
        assert!(!c.failed);
        for s in [StatusType::Failed, StatusType::Fault, StatusType::TimedOut] {
            assert!(ChangeEvent::from_build(&build(s), "us-east-1").unwrap().failed);
        }
        assert!(!ChangeEvent::from_build(&build(StatusType::Stopped), "us-east-1").unwrap().failed);
    }

    #[test]
    fn config_rows_parse_and_skip_compliance_bookkeeping() {
        let row = r#"{"resourceId":"sg-1","resourceName":"web-sg","resourceType":"AWS::EC2::SecurityGroup",
            "arn":"arn:aws:ec2:us-east-1:111122223333:security-group/sg-1","awsRegion":"us-east-1",
            "configurationItemCaptureTime":"2026-10-21T13:02:11.000Z","configurationItemStatus":"OK"}"#;
        let c = ChangeEvent::from_config_row(row, "eu-west-1").unwrap();
        assert_eq!(c.what, "SecurityGroup configuration changed");
        assert!(c.target_ref.starts_with("arn:aws:ec2:"));
        assert_eq!(c.region, "us-east-1");
        assert!(c.title.starts_with("10-21 13:02  SecurityGroup configuration changed  web-sg"));
        assert!(!c.is_noise());

        let deleted = row.replace("\"OK\"", "\"ResourceDeleted\"");
        assert_eq!(ChangeEvent::from_config_row(&deleted, "x").unwrap().what, "SecurityGroup deleted");
        let compliance = row.replace("AWS::EC2::SecurityGroup", "AWS::Config::ResourceCompliance");
        assert!(ChangeEvent::from_config_row(&compliance, "x").is_none());
    }

    fn trail(name: &str, identity_type: &str, issuer: &str, raw: &str) -> CloudTrailEvent {
        let sdk = aws_sdk_cloudtrail::types::Event::builder()
            .event_id("e-1")
            .event_name(name)
            .event_time(aws_smithy_types::DateTime::from_secs(1_790_000_000))
            .cloud_trail_event(raw)
            .build();
        let mut ev = CloudTrailEvent::from_sdk(&sdk);
        ev.identity_type = identity_type.to_string();
        ev.session_issuer_arn = issuer.to_string();
        ev
    }

    #[test]
    fn machine_and_session_events_are_noise_people_are_not() {
        let person = ChangeEvent::from_trail(&trail("AuthorizeSecurityGroupIngress", "AssumedRole", "arn:aws:iam::1:role/Admin", "{}"));
        assert!(!person.is_noise());
        let service = ChangeEvent::from_trail(&trail("CreateNetworkInterface", "AWSService", "", "{}"));
        assert!(service.is_noise());
        let linked = ChangeEvent::from_trail(&trail(
            "UpdateService",
            "AssumedRole",
            "arn:aws:iam::1:role/aws-service-role/ecs.amazonaws.com/AWSServiceRoleForECS",
            "{}",
        ));
        assert!(linked.is_noise());
        let invoked = ChangeEvent::from_trail(&trail(
            "PutObject",
            "AssumedRole",
            "",
            r#"{"userIdentity":{"invokedBy":"cloudformation.amazonaws.com"}}"#,
        ));
        assert!(invoked.is_noise());
        assert!(invoked.who.ends_with("via cloudformation.amazonaws.com"));
        assert!(ChangeEvent::from_trail(&trail("AssumeRole", "AssumedRole", "", "{}")).is_noise());
    }

    #[test]
    fn window_steps_saturate() {
        assert_eq!(ChangesWindow::default().label(), "1h");
        assert_eq!(ChangesWindow::FifteenMinutes.narrower(), ChangesWindow::FifteenMinutes);
        assert_eq!(ChangesWindow::SevenDays.wider(), ChangesWindow::SevenDays);
        assert_eq!(ChangesWindow::OneHour.wider(), ChangesWindow::SixHours);
    }

    #[test]
    fn arn_tails_keep_revisions_and_names() {
        assert_eq!(readable_tail("arn:aws:ecs:us-east-1:1:task-definition/orders-worker:15"), "orders-worker:15");
        assert_eq!(readable_tail("arn:aws:ecs:us-east-1:1:service/acme-prod/orders-worker"), "orders-worker");
        assert_eq!(readable_tail("arn:aws:iam::1:role/github-actions-deploy"), "github-actions-deploy");
        assert_eq!(readable_tail("arn:aws:lambda:us-east-1:1:function:orders-api"), "orders-api");
        assert_eq!(readable_tail("my-bucket"), "my-bucket");
    }

    #[test]
    fn iam_events_get_their_own_source_request_and_decoded_policy() {
        let raw = r#"{"userIdentity":{"type":"AssumedRole","invokedBy":"sso.amazonaws.com"},
            "requestParameters":{"roleName":"deploy","policyName":"extras",
            "policyDocument":"%7B%22Version%22%3A%222012-10-17%22%2C%22Statement%22%3A%5B%5D%7D"}}"#;
        let mut ev = trail("PutRolePolicy", "AssumedRole", "", raw);
        ev.event_source = "iam.amazonaws.com".to_string();
        let c = ChangeEvent::from_trail(&ev);
        assert_eq!(c.source, ChangeSource::Iam);
        assert_eq!(c.resource_type(), "IAM Change");
        // Made through a service, but it still changes access.
        assert!(!c.is_noise());
        assert_eq!(c.request, vec![("roleName".to_string(), "deploy".to_string()), ("policyName".to_string(), "extras".to_string())]);
        let policy = c.policy.as_deref().unwrap();
        assert!(policy.contains("\"Version\": \"2012-10-17\""), "{policy}");
        let rows = c.details();
        assert!(rows.iter().any(|(k, v)| k == "Policy document" && v.is_empty()));
        assert!(rows.iter().any(|(k, _)| k == "    \"Statement\": []"));
        // Global events open in us-east-1's event history, wherever you are.
        assert!(c.console_url("ap-southeast-2").unwrap().contains("us-east-1.console"));

        let mut org = trail("UpdatePolicy", "AssumedRole", "", r#"{"requestParameters":{"policyId":"p-1","content":"{\"Version\":\"2012-10-17\"}"}}"#);
        org.event_source = "organizations.amazonaws.com".to_string();
        let o = ChangeEvent::from_trail(&org);
        assert_eq!(o.source, ChangeSource::Org);
        assert!(o.policy.unwrap().contains("2012-10-17"));
    }

    #[test]
    fn percent_decoding_leaves_bad_escapes_alone() {
        assert_eq!(percent_decode("%7B%22a%22%7D"), "{\"a\"}");
        assert_eq!(percent_decode("a+b%2"), "a b%2");
        assert_eq!(pretty_policy("not json"), "not json");
    }
}
