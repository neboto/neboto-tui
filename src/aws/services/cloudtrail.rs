use crate::aws::client::AwsClients;
use crate::aws::resource::{Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::error::Result;
use crate::event::{Event, LoadProgress};
use async_trait::async_trait;
use aws_sdk_cloudtrail::types::{LookupAttribute, LookupAttributeKey};
use aws_sdk_cloudtrail::Client as CloudTrailClient;
use std::any::Any;
use std::collections::HashMap;
use tokio::sync::mpsc;

/// Lookback window for the CloudTrail lens (`W`). CloudTrail's `LookupEvents`
/// window is 90 days; default 7d, `[`/`]` widen/narrow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TrailRange {
    #[default]
    D7,
    D30,
    D90,
}

impl TrailRange {
    pub fn label(self) -> &'static str {
        match self {
            TrailRange::D7 => "7d",
            TrailRange::D30 => "30d",
            TrailRange::D90 => "90d",
        }
    }

    pub fn lookback_secs(self) -> i64 {
        let days = match self {
            TrailRange::D7 => 7,
            TrailRange::D30 => 30,
            TrailRange::D90 => 90,
        };
        days * 24 * 60 * 60
    }

    pub fn wider(self) -> Self {
        match self {
            TrailRange::D7 => TrailRange::D30,
            TrailRange::D30 | TrailRange::D90 => TrailRange::D90,
        }
    }

    pub fn narrower(self) -> Self {
        match self {
            TrailRange::D90 => TrailRange::D30,
            TrailRange::D30 | TrailRange::D7 => TrailRange::D7,
        }
    }
}

/// Server-side lookup attribute for the CloudTrail event-list filter (`f`).
/// `LookupEvents` accepts exactly **one** attribute per call (API limit — the
/// console's single-filter dropdown mirrors this, it's not a console choice).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CtLookupAttr {
    EventName,
    Username,
    ResourceName,
    ResourceType,
    EventSource,
    AccessKeyId,
    EventId,
}

impl CtLookupAttr {
    pub const ALL: [CtLookupAttr; 7] = [
        CtLookupAttr::EventName,
        CtLookupAttr::Username,
        CtLookupAttr::ResourceName,
        CtLookupAttr::ResourceType,
        CtLookupAttr::EventSource,
        CtLookupAttr::AccessKeyId,
        CtLookupAttr::EventId,
    ];

    pub fn label(self) -> &'static str {
        match self {
            CtLookupAttr::EventName => "Event name",
            CtLookupAttr::Username => "User name",
            CtLookupAttr::ResourceName => "Resource name",
            CtLookupAttr::ResourceType => "Resource type",
            CtLookupAttr::EventSource => "Event source",
            CtLookupAttr::AccessKeyId => "Access key ID",
            CtLookupAttr::EventId => "Event ID",
        }
    }

    /// Example value shown as a placeholder in the filter modal's input line.
    pub fn hint(self) -> &'static str {
        match self {
            CtLookupAttr::EventName => "e.g. ConsoleLogin, DeleteBucket",
            CtLookupAttr::Username => "e.g. alice, my-role-session",
            CtLookupAttr::ResourceName => "e.g. my-bucket, i-0abc123",
            CtLookupAttr::ResourceType => "e.g. AWS::S3::Bucket",
            CtLookupAttr::EventSource => "e.g. s3.amazonaws.com",
            CtLookupAttr::AccessKeyId => "e.g. AKIA…",
            CtLookupAttr::EventId => "the CloudTrail event GUID",
        }
    }

    /// Short tag for the cache-variant key and the filter chip.
    pub fn tag(self) -> &'static str {
        match self {
            CtLookupAttr::EventName => "event",
            CtLookupAttr::Username => "user",
            CtLookupAttr::ResourceName => "resource",
            CtLookupAttr::ResourceType => "type",
            CtLookupAttr::EventSource => "source",
            CtLookupAttr::AccessKeyId => "key",
            CtLookupAttr::EventId => "id",
        }
    }

    fn key(self) -> LookupAttributeKey {
        match self {
            CtLookupAttr::EventName => LookupAttributeKey::EventName,
            CtLookupAttr::Username => LookupAttributeKey::Username,
            CtLookupAttr::ResourceName => LookupAttributeKey::ResourceName,
            CtLookupAttr::ResourceType => LookupAttributeKey::ResourceType,
            CtLookupAttr::EventSource => LookupAttributeKey::EventSource,
            CtLookupAttr::AccessKeyId => LookupAttributeKey::AccessKeyId,
            CtLookupAttr::EventId => LookupAttributeKey::EventId,
        }
    }
}

/// Time-range presets for the event list. `LookupEvents` retains 90 days.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CtEventRange {
    H1,
    H24,
    D7,
    D30,
    #[default]
    D90,
}

impl CtEventRange {
    pub const ALL: [CtEventRange; 5] = [
        CtEventRange::H1,
        CtEventRange::H24,
        CtEventRange::D7,
        CtEventRange::D30,
        CtEventRange::D90,
    ];

    pub fn label(self) -> &'static str {
        match self {
            CtEventRange::H1 => "1h",
            CtEventRange::H24 => "24h",
            CtEventRange::D7 => "7d",
            CtEventRange::D30 => "30d",
            CtEventRange::D90 => "90d",
        }
    }

    fn lookback_secs(self) -> i64 {
        match self {
            CtEventRange::H1 => 60 * 60,
            CtEventRange::H24 => 24 * 60 * 60,
            CtEventRange::D7 => 7 * 24 * 60 * 60,
            CtEventRange::D30 => 30 * 24 * 60 * 60,
            CtEventRange::D90 => 90 * 24 * 60 * 60,
        }
    }

    pub fn next(self) -> Self {
        match self {
            CtEventRange::H1 => CtEventRange::H24,
            CtEventRange::H24 => CtEventRange::D7,
            CtEventRange::D7 => CtEventRange::D30,
            CtEventRange::D30 => CtEventRange::D90,
            CtEventRange::D90 => CtEventRange::H1,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            CtEventRange::H1 => CtEventRange::D90,
            CtEventRange::H24 => CtEventRange::H1,
            CtEventRange::D7 => CtEventRange::H24,
            CtEventRange::D30 => CtEventRange::D7,
            CtEventRange::D90 => CtEventRange::D30,
        }
    }
}

/// The event list's server-side query: at most one lookup attribute (API
/// limit) plus a time-range preset. The default (no filter, 90d) matches the
/// pre-filter behaviour: the most recent events, newest first.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CtEventQuery {
    pub filter: Option<(CtLookupAttr, String)>,
    pub range: CtEventRange,
}

impl CtEventQuery {
    /// Cache-variant key — each attribute/value/range combination caches
    /// independently so flipping back to a previous filter is instant.
    pub fn variant(&self) -> String {
        let filter = match &self.filter {
            Some((attr, value)) => format!("{}={}", attr.tag(), value),
            None => "all".to_string(),
        };
        format!("{}-{}", filter, self.range.label())
    }

    /// Short human-readable summary for the scope bar; `None` when the query
    /// is the default (nothing worth calling out).
    pub fn chip(&self) -> Option<String> {
        match (&self.filter, self.range) {
            (None, CtEventRange::D90) => None,
            (None, range) => Some(format!("last {}", range.label())),
            (Some((attr, value)), range) => {
                Some(format!("{}={} · {}", attr.tag(), value, range.label()))
            }
        }
    }
}

/// One-shot `LookupEvents` for the CloudTrail lens (`W` on any resource). Tries
/// each lookup key in order (id first, then any type-specific names — see
/// `Resource::trail_lookup_keys`) and returns the first non-empty result.
///
/// `LookupEvents` is throttled at 2 TPS, so this is a single paginated call, no
/// polling. `ReadOnly` can't combine with a `ResourceName` attribute, so
/// mutations-only (`include_reads=false`) is filtered client-side using the same
/// `read_only` flag `is_noise()` uses. Capped at ~50 matched events, with a hard
/// scan cap so an all-reads resource can't paginate unbounded.
pub async fn fetch_trail_events(
    client: CloudTrailClient,
    keys: Vec<String>,
    range: TrailRange,
    include_reads: bool,
) -> Result<Vec<CloudTrailEvent>> {
    const MAX_EVENTS: usize = 50;
    const MAX_SCAN: usize = 2000; // bound pagination when filtering reads out

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let start_dt = aws_sdk_cloudtrail::primitives::DateTime::from_secs(now - range.lookback_secs());
    let end_dt = aws_sdk_cloudtrail::primitives::DateTime::from_secs(now);

    for key in keys {
        if key.trim().is_empty() {
            continue;
        }
        let attr = LookupAttribute::builder()
            .attribute_key(LookupAttributeKey::ResourceName)
            .attribute_value(key)
            .build()
            .expect("attribute_key and attribute_value are always set");

        let mut events: Vec<CloudTrailEvent> = Vec::new();
        let mut scanned = 0usize;
        let mut paginator = client
            .lookup_events()
            .lookup_attributes(attr)
            .start_time(start_dt)
            .end_time(end_dt)
            .into_paginator()
            .items()
            .send();

        while let Some(result) = paginator.next().await {
            match result {
                Ok(event) => {
                    scanned += 1;
                    let parsed = CloudTrailEvent::from_sdk(&event);
                    if include_reads || !parsed.read_only {
                        events.push(parsed);
                    }
                    if events.len() >= MAX_EVENTS || scanned >= MAX_SCAN {
                        break;
                    }
                }
                Err(e) => return Err(e.into()),
            }
        }

        if !events.is_empty() {
            return Ok(events);
        }
    }

    Ok(Vec::new())
}

/// A trail configuration (Trails sub-tab): `DescribeTrails` enriched with
/// `GetTrailStatus` (logging on/off + delivery errors — the "did my audit log
/// silently stop" signal) and `GetEventSelectors` (what it records), all
/// fetched eagerly at load. Status/selector failures are stored per row, never
/// fatal.
#[derive(Clone, Debug)]
pub struct CloudTrailTrail {
    pub name: String,
    pub arn: String,
    pub home_region: String,
    pub is_multi_region: bool,
    pub is_org_trail: bool,
    pub s3_bucket: String,
    pub s3_prefix: String,
    pub sns_topic_arn: String,
    pub log_group_arn: String,
    pub cw_role_arn: String,
    pub kms_key_id: String,
    pub include_global_events: bool,
    pub log_file_validation: bool,
    pub has_insight_selectors: bool,

    // GetTrailStatus — `None` when the status fetch failed (see status_error).
    pub is_logging: Option<bool>,
    pub status_error: Option<String>,
    pub latest_delivery_time: String,
    pub latest_delivery_error: String,
    pub latest_cw_delivery_time: String,
    pub latest_cw_delivery_error: String,
    pub latest_digest_time: String,
    pub latest_digest_error: String,
    pub latest_notification_error: String,
    pub start_logging_time: String,
    pub stop_logging_time: String,

    // GetEventSelectors, pre-flattened to display rows (Glue-crawler pattern).
    pub selector_rows: Vec<(String, String)>,
    pub selectors_error: Option<String>,

    pub tags: HashMap<String, String>,
}

impl CloudTrailTrail {
    pub fn from_sdk(
        trail: &aws_sdk_cloudtrail::types::Trail,
        status: std::result::Result<
            aws_sdk_cloudtrail::operation::get_trail_status::GetTrailStatusOutput,
            String,
        >,
        selectors: std::result::Result<
            aws_sdk_cloudtrail::operation::get_event_selectors::GetEventSelectorsOutput,
            String,
        >,
    ) -> Self {
        let fmt_dt = |dt: Option<&aws_sdk_cloudtrail::primitives::DateTime>| {
            dt.map(|d| d.to_string()).unwrap_or_default()
        };

        let (is_logging, status_error, status_out) = match status {
            Ok(s) => (s.is_logging(), None, Some(s)),
            Err(e) => (None, Some(e), None),
        };
        let s = status_out.as_ref();

        let (selector_rows, selectors_error) = match selectors {
            Ok(out) => (Self::flatten_selectors(&out), None),
            Err(e) => (Vec::new(), Some(e)),
        };

        Self {
            name: trail.name().unwrap_or("unknown").to_string(),
            arn: trail.trail_arn().unwrap_or("").to_string(),
            home_region: trail.home_region().unwrap_or("").to_string(),
            is_multi_region: trail.is_multi_region_trail().unwrap_or(false),
            is_org_trail: trail.is_organization_trail().unwrap_or(false),
            s3_bucket: trail.s3_bucket_name().unwrap_or("").to_string(),
            s3_prefix: trail.s3_key_prefix().unwrap_or("").to_string(),
            sns_topic_arn: trail.sns_topic_arn().unwrap_or("").to_string(),
            log_group_arn: trail
                .cloud_watch_logs_log_group_arn()
                .unwrap_or("")
                .to_string(),
            cw_role_arn: trail.cloud_watch_logs_role_arn().unwrap_or("").to_string(),
            kms_key_id: trail.kms_key_id().unwrap_or("").to_string(),
            include_global_events: trail.include_global_service_events().unwrap_or(false),
            log_file_validation: trail.log_file_validation_enabled().unwrap_or(false),
            has_insight_selectors: trail.has_insight_selectors().unwrap_or(false),
            is_logging,
            status_error,
            latest_delivery_time: fmt_dt(s.and_then(|s| s.latest_delivery_time())),
            latest_delivery_error: s
                .and_then(|s| s.latest_delivery_error())
                .unwrap_or("")
                .to_string(),
            latest_cw_delivery_time: fmt_dt(
                s.and_then(|s| s.latest_cloud_watch_logs_delivery_time()),
            ),
            latest_cw_delivery_error: s
                .and_then(|s| s.latest_cloud_watch_logs_delivery_error())
                .unwrap_or("")
                .to_string(),
            latest_digest_time: fmt_dt(s.and_then(|s| s.latest_digest_delivery_time())),
            latest_digest_error: s
                .and_then(|s| s.latest_digest_delivery_error())
                .unwrap_or("")
                .to_string(),
            latest_notification_error: s
                .and_then(|s| s.latest_notification_error())
                .unwrap_or("")
                .to_string(),
            start_logging_time: fmt_dt(s.and_then(|s| s.start_logging_time())),
            stop_logging_time: fmt_dt(s.and_then(|s| s.stop_logging_time())),
            selector_rows,
            selectors_error,
            tags: HashMap::new(),
        }
    }

    /// Flatten basic + advanced event selectors into detail-pane rows
    /// (group header per selector, key-value per field).
    fn flatten_selectors(
        out: &aws_sdk_cloudtrail::operation::get_event_selectors::GetEventSelectorsOutput,
    ) -> Vec<(String, String)> {
        let mut rows = Vec::new();
        for (i, sel) in out.event_selectors().iter().enumerate() {
            rows.push((format!("Selector {}", i + 1), String::new()));
            if let Some(rw) = sel.read_write_type() {
                rows.push(("Read/write type".to_string(), rw.as_str().to_string()));
            }
            rows.push((
                "Management events".to_string(),
                if sel.include_management_events().unwrap_or(false) {
                    "✓ included".to_string()
                } else {
                    "✗ excluded".to_string()
                },
            ));
            let excluded = sel.exclude_management_event_sources();
            if !excluded.is_empty() {
                rows.push(("Excluded sources".to_string(), excluded.join(", ")));
            }
            for dr in sel.data_resources() {
                let values = dr.values().join(", ");
                rows.push((
                    dr.r#type().unwrap_or("data resource").to_string(),
                    if values.is_empty() {
                        "(all)".to_string()
                    } else {
                        values
                    },
                ));
            }
            rows.push((String::new(), String::new()));
        }
        for adv in out.advanced_event_selectors() {
            rows.push((
                adv.name().unwrap_or("Advanced selector").to_string(),
                String::new(),
            ));
            for f in adv.field_selectors() {
                let mut parts = Vec::new();
                let mut op = |label: &str, vals: &[String]| {
                    if !vals.is_empty() {
                        parts.push(format!("{} {}", label, vals.join(", ")));
                    }
                };
                op("=", f.equals());
                op("≠", f.not_equals());
                op("starts with", f.starts_with());
                op("ends with", f.ends_with());
                op("not starts with", f.not_starts_with());
                op("not ends with", f.not_ends_with());
                rows.push((f.field().to_string(), parts.join(" · ")));
            }
            rows.push((String::new(), String::new()));
        }
        while rows.last().is_some_and(|(k, v)| k.is_empty() && v.is_empty()) {
            rows.pop();
        }
        rows
    }

    /// Whether any delivery leg (S3, CloudWatch Logs, digest, SNS) reported an
    /// error on its last attempt.
    pub fn has_delivery_error(&self) -> bool {
        !self.latest_delivery_error.is_empty()
            || !self.latest_cw_delivery_error.is_empty()
            || !self.latest_digest_error.is_empty()
            || !self.latest_notification_error.is_empty()
    }

    /// The CloudWatch Logs destination as `(group name, region)`, parsed from
    /// the log-group ARN (`arn:…:logs:REGION:acct:log-group:NAME[:*]`) — the
    /// group lives in the trail's home region, which `t` must honour.
    pub fn cw_log_group(&self) -> Option<(String, String)> {
        let arn = &self.log_group_arn;
        let region = arn.split(':').nth(3)?.to_string();
        let group = arn
            .split(":log-group:")
            .nth(1)?
            .trim_end_matches(":*")
            .to_string();
        if group.is_empty() || region.is_empty() {
            None
        } else {
            Some((group, region))
        }
    }
}

crate::sections! {
    pub enum CtTrailDetailSection,
    pub static CT_TRAIL_SECTIONS = [
        Overview "Overview",
        Status "Status",
        Selectors "Selectors",
    ]
}

impl Resource for CloudTrailTrail {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&CT_TRAIL_SECTIONS)
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws cloudtrail get-trail-status --name {}",
            crate::aws::resource::shell_quote(&self.name)
        ))
    }

    fn id(&self) -> &str {
        // The ARN, not the name — multi-region trails repeat names across
        // regions and jump/selection resolution needs a stable unique id.
        &self.arn
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn resource_type(&self) -> &str {
        "CloudTrail Trail"
    }

    fn state(&self) -> ResourceState {
        // A trail that stopped logging is the classic silent failure — red.
        // Logging but with a failing delivery leg reads as a warning.
        match self.is_logging {
            Some(false) => ResourceState::Unavailable,
            Some(true) if self.has_delivery_error() => ResourceState::Pending,
            Some(true) => ResourceState::Running,
            None => ResourceState::Unknown("status unavailable".to_string()),
        }
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn search_text(&self) -> String {
        format!(
            "{} {} {} {}",
            self.name, self.arn, self.home_region, self.s3_bucket
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        // Flat fallback (feeds export); the rich view is the split pane.
        let mut details = vec![
            ("Name".to_string(), self.name.clone()),
            ("Home region".to_string(), self.home_region.clone()),
            (
                "Multi-region".to_string(),
                if self.is_multi_region { "Yes" } else { "No" }.to_string(),
            ),
            ("S3 bucket".to_string(), self.s3_bucket.clone()),
        ];
        if let Some(logging) = self.is_logging {
            details.push((
                "Logging".to_string(),
                if logging { "On" } else { "Off" }.to_string(),
            ));
        }
        details.push(("ARN".to_string(), self.arn.clone()));
        details
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{region}.console.aws.amazon.com/cloudtrail/home?region={region}#/trails/{}",
            self.arn
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn trail_lookup_keys(&self) -> Vec<String> {
        // The default (id) is the ARN; CloudTrail records the friendly name
        // as the event ResourceName, so try that first.
        vec![self.name.clone(), self.arn.clone()]
    }
}

/// A CloudTrail Insights event (Insights sub-tab): an API call-rate or
/// error-rate anomaly. Parsed eagerly from the insight event JSON; flat
/// `details()` (no split pane), raw JSON on `e`.
#[derive(Clone, Debug)]
pub struct CtInsightEvent {
    pub event_id: String,
    pub event_time: String,
    pub aws_region: String,
    /// ApiCallRateInsight / ApiErrorRateInsight.
    pub insight_type: String,
    /// Start (anomaly ongoing when recorded) / End.
    pub state: String,
    /// The API whose rate went anomalous.
    pub event_source: String,
    pub event_name: String,
    pub baseline_avg: f64,
    pub insight_avg: f64,
    /// Minutes.
    pub insight_duration: i64,
    pub baseline_duration: i64,
    pub raw: String,
    pub tags: HashMap<String, String>,
}

impl CtInsightEvent {
    pub fn from_sdk(event: &aws_sdk_cloudtrail::types::Event) -> Self {
        let raw = event.cloud_trail_event().unwrap_or("").to_string();
        let parsed: serde_json::Value =
            serde_json::from_str(&raw).unwrap_or(serde_json::Value::Null);
        let details = parsed.get("insightDetails");
        let s = |path: &[&str]| -> String {
            let mut cur = match details {
                Some(d) => d,
                None => return String::new(),
            };
            for k in path {
                match cur.get(k) {
                    Some(v) => cur = v,
                    None => return String::new(),
                }
            }
            cur.as_str().unwrap_or("").to_string()
        };
        let stats = details
            .and_then(|d| d.get("insightContext"))
            .and_then(|c| c.get("statistics"));
        let avg = |key: &str| -> f64 {
            stats
                .and_then(|st| st.get(key))
                .and_then(|b| b.get("average"))
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0)
        };
        let dur = |key: &str| -> i64 {
            stats
                .and_then(|st| st.get(key))
                .and_then(|v| v.as_i64())
                .unwrap_or(0)
        };
        Self {
            event_id: event.event_id().unwrap_or("unknown").to_string(),
            event_time: event
                .event_time()
                .map(|dt| dt.to_string())
                .unwrap_or_else(|| "unknown".to_string()),
            aws_region: parsed
                .get("awsRegion")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            insight_type: s(&["insightType"]),
            state: s(&["state"]),
            event_source: s(&["eventSource"]),
            event_name: s(&["eventName"]),
            baseline_avg: avg("baseline"),
            insight_avg: avg("insight"),
            insight_duration: dur("insightDuration"),
            baseline_duration: dur("baselineDuration"),
            raw,
            tags: HashMap::new(),
        }
    }
}

impl Resource for CtInsightEvent {
    fn id(&self) -> &str {
        &self.event_id
    }

    fn name(&self) -> &str {
        &self.event_name
    }

    fn resource_type(&self) -> &str {
        "CloudTrail Insight"
    }

    fn state(&self) -> ResourceState {
        // An anomaly still marked Start was ongoing when recorded — yellow;
        // a closed one reads neutral.
        if self.state.eq_ignore_ascii_case("start") {
            ResourceState::Pending
        } else {
            ResourceState::Available
        }
    }

    fn state_label(&self) -> String {
        if self.state.eq_ignore_ascii_case("start") {
            "ongoing".to_string()
        } else {
            "closed".to_string()
        }
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn search_text(&self) -> String {
        format!(
            "{} {} {} {}",
            self.event_name, self.event_source, self.insight_type, self.event_time
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        let fmt_rate = |v: f64| {
            if v >= 1.0 {
                format!("{:.2} calls/min", v)
            } else {
                format!("{:.4} calls/min", v)
            }
        };
        let mut rows = vec![
            ("Insight type".to_string(), self.insight_type.clone()),
            ("State".to_string(), self.state.clone()),
            ("API".to_string(), self.event_name.clone()),
            ("Source".to_string(), self.event_source.clone()),
            ("Time".to_string(), self.event_time.clone()),
        ];
        if self.insight_avg > 0.0 || self.baseline_avg > 0.0 {
            rows.push((
                "Anomalous rate".to_string(),
                fmt_rate(self.insight_avg),
            ));
            rows.push(("Baseline rate".to_string(), fmt_rate(self.baseline_avg)));
            if self.baseline_avg > 0.0 {
                rows.push((
                    "Deviation".to_string(),
                    format!("{:.0}× baseline", self.insight_avg / self.baseline_avg),
                ));
            }
        }
        if self.insight_duration > 0 {
            rows.push((
                "Anomaly duration".to_string(),
                format!("{} min", self.insight_duration),
            ));
        }
        if self.baseline_duration > 0 {
            rows.push((
                "Baseline window".to_string(),
                format!("{} min", self.baseline_duration),
            ));
        }
        if !self.aws_region.is_empty() {
            rows.push(("Region".to_string(), self.aws_region.clone()));
        }
        rows.push(("Event ID".to_string(), self.event_id.clone()));
        rows
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{region}.console.aws.amazon.com/cloudtrail/home?region={region}#/insights"
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn raw_content(&self) -> Option<String> {
        if !self.raw.is_empty() {
            Some(self.raw.clone())
        } else {
            None
        }
    }
}

pub struct CloudTrailService {
    client: CloudTrailClient,
    query: CtEventQuery,
}

impl CloudTrailService {
    pub fn new(aws_clients: &AwsClients) -> Self {
        Self::with_query(aws_clients, CtEventQuery::default())
    }

    /// Build the service scoped to a server-side event query (`f` filter).
    /// Variant-cached per query, so revisiting a filter is instant.
    pub fn with_query(aws_clients: &AwsClients, query: CtEventQuery) -> Self {
        Self {
            client: aws_clients.cloudtrail_client(),
            query,
        }
    }

    /// The base `LookupEvents` request with the query's attribute filter and
    /// time window applied.
    fn lookup_request(&self) -> aws_sdk_cloudtrail::operation::lookup_events::builders::LookupEventsFluentBuilder {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let mut req = self
            .client
            .lookup_events()
            .start_time(aws_sdk_cloudtrail::primitives::DateTime::from_secs(
                now - self.query.range.lookback_secs(),
            ))
            .end_time(aws_sdk_cloudtrail::primitives::DateTime::from_secs(now));
        if let Some((attr, value)) = &self.query.filter {
            req = req.lookup_attributes(
                LookupAttribute::builder()
                    .attribute_key(attr.key())
                    .attribute_value(value.clone())
                    .build()
                    .expect("attribute_key and attribute_value are always set"),
            );
        }
        req
    }

    /// Phase-1 trail fetch: `DescribeTrails` + per-trail `GetTrailStatus` /
    /// `GetEventSelectors` (each best-effort — a failure lands in the row).
    /// Errors with the AWS message so the caller can degrade to a warning.
    async fn fetch_trails(&self) -> std::result::Result<Vec<Box<dyn Resource>>, String> {
        let resp = self
            .client
            .describe_trails()
            .send()
            .await
            .map_err(|e| crate::error::sdk_error_message(&e))?;
        let mut trails: Vec<Box<dyn Resource>> = Vec::new();
        for t in resp.trail_list() {
            let arn = t.trail_arn().unwrap_or_default().to_string();
            let status = self
                .client
                .get_trail_status()
                .name(&arn)
                .send()
                .await
                .map_err(|e| crate::error::sdk_error_message(&e));
            let selectors = self
                .client
                .get_event_selectors()
                .trail_name(&arn)
                .send()
                .await
                .map_err(|e| crate::error::sdk_error_message(&e));
            trails.push(Box::new(CloudTrailTrail::from_sdk(t, status, selectors)));
        }
        Ok(trails)
    }

    /// Insights-events fetch (`EventCategory=insight` over the query's time
    /// window; the attribute filter doesn't apply — insights are their own
    /// category). Errors with the AWS message; the caller treats
    /// InsightNotEnabled as a normal empty state, not a warning.
    async fn fetch_insights(&self) -> std::result::Result<Vec<Box<dyn Resource>>, String> {
        const MAX_INSIGHTS: usize = 200;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let mut paginator = self
            .client
            .lookup_events()
            .event_category(aws_sdk_cloudtrail::types::EventCategory::Insight)
            .start_time(aws_sdk_cloudtrail::primitives::DateTime::from_secs(
                now - self.query.range.lookback_secs(),
            ))
            .end_time(aws_sdk_cloudtrail::primitives::DateTime::from_secs(now))
            .into_paginator()
            .items()
            .send();
        let mut insights: Vec<Box<dyn Resource>> = Vec::new();
        while let Some(result) = paginator.next().await {
            match result {
                Ok(event) => {
                    insights.push(Box::new(CtInsightEvent::from_sdk(&event)));
                    if insights.len() >= MAX_INSIGHTS {
                        break;
                    }
                }
                Err(e) => return Err(crate::error::sdk_error_message(&e)),
            }
        }
        Ok(insights)
    }
}

#[async_trait]
impl AwsService for CloudTrailService {
    fn service_type(&self) -> ServiceType {
        ServiceType::CloudTrail
    }

    fn name(&self) -> &str {
        "CloudTrail Events"
    }

    async fn list_resources(&self) -> Result<Vec<Box<dyn Resource>>> {
        // Trails + insights are best-effort here too (mirrors the streaming path).
        let mut resources: Vec<Box<dyn Resource>> = self.fetch_trails().await.unwrap_or_default();
        resources.extend(self.fetch_insights().await.unwrap_or_default());

        const MAX_EVENTS: usize = 500; // Cap at 500 events for TUI performance

        // Lookup recent events with pagination (removed the 50 event hard limit)
        let mut paginator = self.lookup_request().into_paginator().items().send();

        while let Some(result) = paginator.next().await {
            match result {
                Ok(event) => {
                    resources
                        .push(Box::new(CloudTrailEvent::from_sdk(&event)) as Box<dyn Resource>);

                    // Stop if we've reached the cap
                    if resources.len() >= MAX_EVENTS {
                        return Ok(resources);
                    }
                }
                Err(e) => return Err(e.into()),
            }
        }

        Ok(resources)
    }

    async fn list_resources_streaming(
        &self,
        event_tx: mpsc::UnboundedSender<Event>,
        service_type: ServiceType,
    ) -> Result<()> {
        const MAX_EVENTS: usize = 500; // Cap at 500 events for TUI performance

        // Phase 1: trails (small, fast) — best-effort so a permission gap on
        // DescribeTrails/GetTrailStatus never breaks the events view. Per the
        // multi-phase convention this must be a warning, not a mid-stream
        // ResourceLoadError (which would drop every later batch).
        let mut trail_count = 0usize;
        match self.fetch_trails().await {
            Ok(trails) if !trails.is_empty() => {
                trail_count = trails.len();
                let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                    service: service_type,
                    resources: trails,
                    progress: LoadProgress {
                        loaded_count: trail_count,
                        total_count: None,
                        status_message: Some(format!("Loaded {} trails...", trail_count)),
                    },
                });
            }
            Ok(_) => {}
            Err(e) => {
                let _ = event_tx.send(Event::ResourceLoadWarning {
                    service: service_type,
                    warning: format!("Trails unavailable: {}", e),
                });
            }
        }

        // Phase 2: Insights events (one quick call) — best-effort.
        // InsightNotEnabledException just means the feature is off: a normal
        // empty state, not a warning worth a "Partial load" banner.
        let mut insight_count = 0usize;
        match self.fetch_insights().await {
            Ok(insights) if !insights.is_empty() => {
                insight_count = insights.len();
                let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                    service: service_type,
                    resources: insights,
                    progress: LoadProgress {
                        loaded_count: trail_count + insight_count,
                        total_count: None,
                        status_message: Some(format!(
                            "Loaded {} Insights events...",
                            insight_count
                        )),
                    },
                });
            }
            Ok(_) => {}
            Err(e) if e.contains("InsightNotEnabled") => {}
            Err(e) => {
                let _ = event_tx.send(Event::ResourceLoadWarning {
                    service: service_type,
                    warning: format!("Insights unavailable: {}", e),
                });
            }
        }

        // Phase 3: events. A failure here is fatal only when nothing at all
        // streamed (no trails/insights either); otherwise it degrades to a warning.
        let mut paginator = self.lookup_request().into_paginator().items().send();

        let mut total_loaded = 0;
        let mut batch_resources: Vec<Box<dyn Resource>> = Vec::new();

        while let Some(result) = paginator.next().await {
            match result {
                Ok(event) => {
                    batch_resources
                        .push(Box::new(CloudTrailEvent::from_sdk(&event)) as Box<dyn Resource>);
                    total_loaded += 1;

                    let reached_cap = total_loaded >= MAX_EVENTS;

                    // Send partial updates in batches of 50
                    if batch_resources.len() >= 50 || reached_cap {
                        let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                            service: service_type,
                            resources: batch_resources.clone(),
                            progress: LoadProgress {
                                loaded_count: total_loaded,
                                total_count: Some(MAX_EVENTS),
                                status_message: if reached_cap {
                                    Some(format!("Loaded {} events (max limit)", total_loaded))
                                } else {
                                    Some(format!("Loaded {} events...", total_loaded))
                                },
                            },
                        });
                        batch_resources.clear();
                    }

                    // Stop if we've reached the cap
                    if reached_cap {
                        break;
                    }
                }
                Err(e) => {
                    let msg = crate::error::sdk_error_message(&e);
                    if trail_count == 0 && insight_count == 0 && total_loaded == 0 {
                        let _ = event_tx.send(Event::ResourceLoadError {
                            service: service_type,
                            error: msg,
                        });
                        return Err(e.into());
                    }
                    let _ = event_tx.send(Event::ResourceLoadWarning {
                        service: service_type,
                        warning: format!("Events unavailable: {}", msg),
                    });
                    break;
                }
            }
        }

        // Send any remaining events
        if !batch_resources.is_empty() {
            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                service: service_type,
                resources: batch_resources,
                progress: LoadProgress {
                    loaded_count: total_loaded,
                    total_count: Some(MAX_EVENTS),
                    status_message: Some(format!("Loaded {} events...", total_loaded)),
                },
            });
        }

        // Send completion event
        let _ = event_tx.send(Event::ResourcesFullyLoaded {
            service: service_type,
            total_count: trail_count + insight_count + total_loaded,
        });

        Ok(())
    }

    async fn get_resource_details(&self, id: &str) -> Result<Box<dyn Resource>> {
        // For CloudTrail, we can't fetch a single event by ID easily
        // So we'll return an error
        Err(crate::error::Error::ResourceNotFound(id.to_string()))
    }
}

// CloudTrail Event Resource
#[derive(Clone, Debug)]
pub struct CloudTrailEvent {
    pub event_id: String,
    pub event_name: String,
    pub event_time: String,
    /// Epoch seconds of `event_time` — the timeline lens merge-sorts on it.
    pub event_time_secs: i64,
    pub username: String,
    pub resource_type: String,
    pub resource_name: String,
    pub read_only: bool,
    pub event_source: String,
    pub access_key_id: String,
    pub cloud_trail_event: String, // Full JSON event for details

    // Parsed eagerly from the full event JSON (no extra API calls).
    pub aws_region: String,
    pub source_ip: String,
    pub user_agent: String,
    pub error_code: String,
    pub error_message: String,
    pub event_type: String,
    pub event_category: String,
    pub recipient_account_id: String,
    pub request_id: String,
    // userIdentity
    pub identity_type: String,
    pub identity_arn: String,
    pub identity_principal: String,
    pub identity_account: String,
    pub identity_user_name: String,
    pub mfa_authenticated: String,
    pub session_creation: String,
    pub session_issuer_arn: String,
    // pretty-printed JSON blocks ("" when absent/null)
    pub request_parameters: String,
    pub response_elements: String,
    // affected resources: (type, arn-or-name)
    pub resources: Vec<(String, String)>,

    pub tags: HashMap<String, String>,
}

impl CloudTrailEvent {
    pub fn from_sdk(event: &aws_sdk_cloudtrail::types::Event) -> Self {
        let event_id = event.event_id().unwrap_or("unknown").to_string();
        let event_name = event.event_name().unwrap_or("unknown").to_string();

        let event_time = event
            .event_time()
            .map(|dt| dt.to_string())
            .unwrap_or_else(|| "unknown".to_string());

        let username = event.username().unwrap_or("unknown").to_string();

        // Extract resource information from resources list
        let (resource_type, resource_name) = {
            let resources = event.resources();
            if let Some(first_resource) = resources.first() {
                (
                    first_resource.resource_type().unwrap_or("").to_string(),
                    first_resource.resource_name().unwrap_or("").to_string(),
                )
            } else {
                (String::new(), String::new())
            }
        };

        let read_only = event
            .read_only()
            .unwrap_or_default()
            .parse::<bool>()
            .unwrap_or(false);
        let event_source = event.event_source().unwrap_or("").to_string();
        let access_key_id = event.access_key_id().unwrap_or("").to_string();
        let cloud_trail_event = event.cloud_trail_event().unwrap_or("").to_string();

        // Parse the full event JSON for the rich fields the SDK summary omits.
        let parsed: serde_json::Value =
            serde_json::from_str(&cloud_trail_event).unwrap_or(serde_json::Value::Null);
        let s = |path: &[&str]| -> String {
            let mut cur = &parsed;
            for k in path {
                match cur.get(k) {
                    Some(v) => cur = v,
                    None => return String::new(),
                }
            }
            cur.as_str().unwrap_or("").to_string()
        };
        let pretty = |key: &str| -> String {
            match parsed.get(key) {
                Some(v) if !v.is_null() => serde_json::to_string_pretty(v).unwrap_or_default(),
                _ => String::new(),
            }
        };

        let mut resources = Vec::new();
        if let Some(arr) = parsed.get("resources").and_then(|v| v.as_array()) {
            for r in arr {
                let ty = r
                    .get("type")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string();
                let arn = r
                    .get("ARN")
                    .and_then(|x| x.as_str())
                    .or_else(|| r.get("resourceName").and_then(|x| x.as_str()))
                    .unwrap_or("")
                    .to_string();
                if !arn.is_empty() || !ty.is_empty() {
                    resources.push((ty, arn));
                }
            }
        }
        // Fall back to the SDK-provided single resource when the JSON has none.
        if resources.is_empty() && !resource_name.is_empty() {
            resources.push((resource_type.clone(), resource_name.clone()));
        }

        Self {
            event_id,
            event_name,
            event_time,
            event_time_secs: event.event_time().map(|dt| dt.secs()).unwrap_or(0),
            username,
            resource_type,
            resource_name,
            read_only,
            event_source,
            access_key_id,
            cloud_trail_event,
            aws_region: s(&["awsRegion"]),
            source_ip: s(&["sourceIPAddress"]),
            user_agent: s(&["userAgent"]),
            error_code: s(&["errorCode"]),
            error_message: s(&["errorMessage"]),
            event_type: s(&["eventType"]),
            event_category: s(&["eventCategory"]),
            recipient_account_id: s(&["recipientAccountId"]),
            request_id: s(&["requestID"]),
            identity_type: s(&["userIdentity", "type"]),
            identity_arn: s(&["userIdentity", "arn"]),
            identity_principal: s(&["userIdentity", "principalId"]),
            identity_account: s(&["userIdentity", "accountId"]),
            identity_user_name: s(&["userIdentity", "userName"]),
            mfa_authenticated: s(&["userIdentity", "sessionContext", "attributes", "mfaAuthenticated"]),
            session_creation: s(&["userIdentity", "sessionContext", "attributes", "creationDate"]),
            session_issuer_arn: s(&["userIdentity", "sessionContext", "sessionIssuer", "arn"]),
            request_parameters: pretty("requestParameters"),
            response_elements: pretty("responseElements"),
            resources,
            tags: HashMap::new(),
        }
    }
}

crate::sections! {
    pub enum CtEventDetailSection,
    pub static CT_EVENT_SECTIONS = [
        Overview "Overview",
        Identity "Identity",
        Request "Request",
        Response "Response",
        Resources "Resources",
    ]
}

impl Resource for CloudTrailEvent {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&CT_EVENT_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.event_id
    }

    fn name(&self) -> &str {
        &self.event_name
    }

    fn resource_type(&self) -> &str {
        "CloudTrail Event"
    }

    fn state(&self) -> ResourceState {
        // Failed/denied calls stand out red; writes (mutations) read as Running,
        // read-only calls as a neutral Available dot.
        if !self.error_code.is_empty() {
            ResourceState::Unavailable
        } else if self.read_only {
            ResourceState::Available
        } else {
            ResourceState::Running
        }
    }

    fn state_label(&self) -> String {
        // The lifecycle words ("running", "available") mean nothing on an API
        // event — say what the state colors actually encode.
        if !self.error_code.is_empty() {
            "error".to_string()
        } else if self.read_only {
            "read".to_string()
        } else {
            "write".to_string()
        }
    }

    fn is_noise(&self) -> bool {
        // `a` hides read-only events to surface mutations (the "what changed" view).
        self.read_only
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn search_text(&self) -> String {
        // Include all searchable fields for CloudTrail events
        format!(
            "{} {} {} {} {} {} {} {} {} {} {}",
            self.event_id,
            self.event_name,
            self.username,
            self.resource_type,
            self.resource_name,
            self.event_source,
            self.event_time,
            self.identity_arn,
            self.source_ip,
            self.aws_region,
            self.error_code,
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        // Flat fallback (feeds export); the rich view is the split pane.
        let mut details = vec![
            ("Event Name".to_string(), self.event_name.clone()),
            ("Event Time".to_string(), self.event_time.clone()),
            ("Event Source".to_string(), self.event_source.clone()),
            ("Region".to_string(), self.aws_region.clone()),
            ("User".to_string(), self.username.clone()),
            (
                "Read Only".to_string(),
                if self.read_only { "Yes" } else { "No" }.to_string(),
            ),
        ];
        if !self.error_code.is_empty() {
            details.push(("Error".to_string(), self.error_code.clone()));
        }
        if !self.source_ip.is_empty() {
            details.push(("Source IP".to_string(), self.source_ip.clone()));
        }
        if !self.identity_arn.is_empty() {
            details.push(("Identity ARN".to_string(), self.identity_arn.clone()));
        }
        if !self.access_key_id.is_empty() {
            details.push(("Access Key ID".to_string(), self.access_key_id.clone()));
        }
        details.push(("Event ID".to_string(), self.event_id.clone()));
        details
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{region}.console.aws.amazon.com/cloudtrail/home?region={region}#/events?EventId={}",
            self.event_id
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn raw_content(&self) -> Option<String> {
        // Return the full CloudTrail event JSON for the editor
        if !self.cloud_trail_event.is_empty() {
            Some(self.cloud_trail_event.clone())
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_query_variant_and_chip() {
        let q = CtEventQuery::default();
        assert_eq!(q.variant(), "all-90d");
        // The default query is unremarkable — no chip in the scope bar.
        assert_eq!(q.chip(), None);
    }

    #[test]
    fn filtered_query_variant_and_chip() {
        let q = CtEventQuery {
            filter: Some((CtLookupAttr::Username, "alice".to_string())),
            range: CtEventRange::H24,
        };
        assert_eq!(q.variant(), "user=alice-24h");
        assert_eq!(q.chip().as_deref(), Some("user=alice · 24h"));
    }

    #[test]
    fn range_only_query_chips_the_range() {
        let q = CtEventQuery {
            filter: None,
            range: CtEventRange::H1,
        };
        assert_eq!(q.variant(), "all-1h");
        assert_eq!(q.chip().as_deref(), Some("last 1h"));
    }

    #[test]
    fn insight_event_parses_details() {
        let json = r#"{
            "eventVersion": "1.07",
            "eventTime": "2026-07-08T01:00:00Z",
            "awsRegion": "ap-southeast-2",
            "eventID": "abc-123",
            "eventType": "AwsCloudTrailInsight",
            "eventCategory": "Insight",
            "insightDetails": {
                "state": "Start",
                "eventSource": "autoscaling.amazonaws.com",
                "eventName": "CompleteLifecycleAction",
                "insightType": "ApiCallRateInsight",
                "insightContext": {
                    "statistics": {
                        "baseline": {"average": 0.05},
                        "insight": {"average": 2.5},
                        "insightDuration": 5,
                        "baselineDuration": 11336
                    }
                }
            }
        }"#;
        let event = aws_sdk_cloudtrail::types::Event::builder()
            .event_id("abc-123")
            .event_name("CompleteLifecycleAction")
            .cloud_trail_event(json)
            .build();
        let insight = CtInsightEvent::from_sdk(&event);
        assert_eq!(insight.insight_type, "ApiCallRateInsight");
        assert_eq!(insight.state, "Start");
        assert_eq!(insight.event_source, "autoscaling.amazonaws.com");
        assert_eq!(insight.baseline_avg, 0.05);
        assert_eq!(insight.insight_avg, 2.5);
        assert_eq!(insight.insight_duration, 5);
        assert_eq!(insight.state(), ResourceState::Pending); // ongoing → yellow
    }

    #[test]
    fn range_cycle_covers_all_presets_and_wraps() {
        let mut r = CtEventRange::default();
        for _ in 0..CtEventRange::ALL.len() {
            assert_eq!(r.prev().next(), r);
            r = r.next();
        }
        assert_eq!(r, CtEventRange::default()); // full cycle wraps
    }
}
