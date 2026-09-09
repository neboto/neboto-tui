//! Row model for the change timeline (`W`) — the merged "what changed
//! around this resource?" view rendered by `trail_lens.rs`.
//!
//! Four sources feed one time-sorted list, each with a colored badge:
//! CloudTrail mutations (`CT`), alarm state changes (`ALM`), the owning
//! CloudFormation stack's events (`CFN`, via the ownership resolver), and
//! ECS deployments + service events (`DEP`, already on the struct — zero
//! fetch). This module owns the row shape and the per-source constructors;
//! the fan-out lives in `App::seed_trail_lens`, the state/rendering in
//! `trail_lens.rs`. Lives outside `ui/` because `event.rs` carries rows in
//! `Event::TrailLensAux`.

use crate::aws::services::cloudformation::CfnStackEvent;
use crate::aws::services::cloudtrail::CloudTrailEvent;
use crate::aws::services::cloudwatch::{fmt_epoch_secs, CwAlarmHistoryItem};
use crate::aws::services::ecs::{EcsDeployment, EcsServiceEvent};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimelineSource {
    Trail,
    Alarm,
    Cfn,
    Deploy,
}

impl TimelineSource {
    /// Filter-cycle order (`f`).
    pub const ALL: [TimelineSource; 4] = [
        TimelineSource::Trail,
        TimelineSource::Alarm,
        TimelineSource::Cfn,
        TimelineSource::Deploy,
    ];

    /// Fixed-width row badge (3 chars).
    pub fn badge(self) -> &'static str {
        match self {
            TimelineSource::Trail => " CT",
            TimelineSource::Alarm => "ALM",
            TimelineSource::Cfn => "CFN",
            TimelineSource::Deploy => "DEP",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            TimelineSource::Trail => "CloudTrail",
            TimelineSource::Alarm => "Alarms",
            TimelineSource::Cfn => "CloudFormation",
            TimelineSource::Deploy => "Deployments",
        }
    }
}

#[derive(Debug, Clone)]
pub struct TimelineRow {
    /// Epoch seconds, for the merge sort (0 when the source had no
    /// parseable time — such rows sink to the bottom).
    pub ts_secs: i64,
    /// Display time, one shared format across sources.
    pub time: String,
    pub source: TimelineSource,
    /// What happened: event name / alarm transition / CFN status /
    /// deployment status.
    pub what: String,
    /// Who or where: actor, alarm name, logical id, task definition.
    pub who: String,
    /// Trailing context: source IP, status reason, counts, message.
    pub detail: String,
    /// Renders red after `detail` (CT error codes).
    pub error: String,
    /// Marks `what` red (failed statuses, ALARM transitions).
    pub failed: bool,
    /// Full raw JSON for `⏎`/`y` when the source has one (CloudTrail only);
    /// other sources fall back to [`TimelineRow::fallback_text`].
    pub json: String,
}

impl TimelineRow {
    pub fn from_trail(ev: &CloudTrailEvent) -> Self {
        TimelineRow {
            ts_secs: ev.event_time_secs,
            time: fmt_ts(ev.event_time_secs, &ev.event_time),
            source: TimelineSource::Trail,
            what: ev.event_name.clone(),
            who: display_user(ev),
            detail: ev.source_ip.clone(),
            error: ev.error_code.clone(),
            failed: !ev.error_code.is_empty(),
            json: ev.cloud_trail_event.clone(),
        }
    }

    pub fn from_alarm(alarm_name: &str, item: &CwAlarmHistoryItem) -> Self {
        TimelineRow {
            ts_secs: item.ts_secs,
            time: fmt_ts(item.ts_secs, &item.timestamp),
            source: TimelineSource::Alarm,
            what: item.summary.clone(),
            who: alarm_name.to_string(),
            detail: String::new(),
            error: String::new(),
            failed: item.summary.contains("to ALARM"),
            json: String::new(),
        }
    }

    pub fn from_cfn(ev: &CfnStackEvent) -> Self {
        let ts = ev.ts_secs.unwrap_or(0);
        TimelineRow {
            ts_secs: ts,
            time: fmt_ts(ts, &ev.timestamp),
            source: TimelineSource::Cfn,
            what: ev.status.clone(),
            who: ev.logical_id.clone(),
            detail: ev.status_reason.clone().unwrap_or_default(),
            error: String::new(),
            failed: ev.status.contains("FAILED") || ev.status.contains("ROLLBACK"),
            json: String::new(),
        }
    }

    pub fn from_deploy(d: &EcsDeployment) -> Self {
        let rollout = d.rollout_state.as_deref().unwrap_or("");
        let failed = rollout == "FAILED" || d.failed > 0;
        let mut detail = format!("{}/{} running", d.running, d.desired);
        if d.pending > 0 {
            detail.push_str(&format!(", {} pending", d.pending));
        }
        if d.failed > 0 {
            detail.push_str(&format!(", {} failed", d.failed));
        }
        if let Some(reason) = d.rollout_state_reason.as_deref() {
            if !reason.is_empty() {
                detail.push_str(" — ");
                detail.push_str(reason);
            }
        }
        TimelineRow {
            ts_secs: d.updated_at_secs,
            time: fmt_ts(d.updated_at_secs, &d.updated_at),
            source: TimelineSource::Deploy,
            what: match rollout {
                "" => format!("{} deployment", d.status),
                r => format!("{} deployment {}", d.status, r),
            },
            who: d.task_definition.clone(),
            detail,
            error: String::new(),
            failed,
            json: String::new(),
        }
    }

    pub fn from_ecs_event(e: &EcsServiceEvent) -> Self {
        let lower = e.message.to_lowercase();
        TimelineRow {
            ts_secs: e.created_at_secs,
            time: fmt_ts(e.created_at_secs, &e.created_at),
            source: TimelineSource::Deploy,
            what: "service event".to_string(),
            who: String::new(),
            detail: e.message.clone(),
            error: String::new(),
            failed: lower.contains("unable") || lower.contains("failed") || lower.contains("unhealthy"),
            json: String::new(),
        }
    }

    /// `⏎`/`y` content for rows without raw JSON.
    pub fn fallback_text(&self) -> String {
        let mut out = format!("{}  [{}]  {}", self.time, self.source.label(), self.what);
        if !self.who.is_empty() {
            out.push_str(&format!("\n{}", self.who));
        }
        if !self.detail.is_empty() {
            out.push_str(&format!("\n{}", self.detail));
        }
        if !self.error.is_empty() {
            out.push_str(&format!("\n{}", self.error));
        }
        out
    }
}

/// One shared time format across sources; falls back to the source's own
/// display string when the epoch is missing.
fn fmt_ts(secs: i64, fallback: &str) -> String {
    if secs > 0 {
        fmt_epoch_secs(secs)
    } else {
        fallback.to_string()
    }
}

/// The most identifying actor string for a CloudTrail row: prefer the human
/// username, fall back to the identity ARN's tail, then the identity type.
pub fn display_user(ev: &CloudTrailEvent) -> String {
    if !ev.username.is_empty() && ev.username != "unknown" {
        return ev.username.clone();
    }
    if !ev.identity_user_name.is_empty() {
        return ev.identity_user_name.clone();
    }
    if !ev.identity_arn.is_empty() {
        return ev
            .identity_arn
            .rsplit('/')
            .next()
            .unwrap_or(&ev.identity_arn)
            .to_string();
    }
    ev.identity_type.clone()
}
