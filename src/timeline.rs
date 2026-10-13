//! Row model for the change timeline (`W`) — the merged "what changed
//! around this resource?" view rendered by `trail_lens.rs`.
//!
//! Six sources feed one time-sorted list, each with a colored badge:
//! CloudTrail mutations (`CT`), alarm state changes (`ALM`), the owning
//! CloudFormation stack's events (`CFN`, via the ownership resolver), ECS
//! deployments + service events (`DEP`, already on the struct — zero
//! fetch), Auto Scaling activities (`ASG` — scale-ins, health replacements,
//! Spot interruptions: none of them are CloudTrail calls) and RDS events
//! (`RDS` — failovers, AWS-initiated reboots, maintenance, storage-full). This module owns the row shape and the per-source constructors;
//! the fan-out lives in `App::seed_trail_lens`, the state/rendering in
//! `trail_lens.rs`. Lives outside `ui/` because `event.rs` carries rows in
//! `Event::TrailLensAux`.

use crate::aws::services::cloudformation::CfnStackEvent;
use crate::aws::services::cloudtrail::CloudTrailEvent;
use crate::aws::services::cloudwatch::{fmt_epoch_secs, CwAlarmHistoryItem};
use crate::aws::services::asg::ScalingActivity;
use crate::aws::services::ecs::{EcsDeployment, EcsServiceEvent};
use crate::aws::services::rds::RdsEvent;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimelineSource {
    Trail,
    Alarm,
    Cfn,
    Deploy,
    Scaling,
    Rds,
}

impl TimelineSource {
    /// Filter-cycle order (`f`).
    pub const ALL: [TimelineSource; 6] = [
        TimelineSource::Trail,
        TimelineSource::Alarm,
        TimelineSource::Cfn,
        TimelineSource::Deploy,
        TimelineSource::Scaling,
        TimelineSource::Rds,
    ];

    /// Fixed-width row badge (3 chars).
    pub fn badge(self) -> &'static str {
        match self {
            TimelineSource::Trail => " CT",
            TimelineSource::Alarm => "ALM",
            TimelineSource::Cfn => "CFN",
            TimelineSource::Deploy => "DEP",
            TimelineSource::Scaling => "ASG",
            TimelineSource::Rds => "RDS",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            TimelineSource::Trail => "CloudTrail",
            TimelineSource::Alarm => "Alarms",
            TimelineSource::Cfn => "CloudFormation",
            TimelineSource::Deploy => "Deployments",
            TimelineSource::Scaling => "Auto Scaling",
            TimelineSource::Rds => "RDS events",
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

    /// An Auto Scaling activity: what happened (description), its status,
    /// and why (the cause — "an instance was taken out of service in
    /// response to an EC2 health check…").
    pub fn from_scaling(a: &ScalingActivity) -> Self {
        let mut detail = a.cause.clone();
        if let Some(m) = a.status_message.as_deref().filter(|m| !m.is_empty()) {
            detail = format!("{m} — {detail}");
        }
        TimelineRow {
            ts_secs: a.start_secs,
            time: fmt_ts(a.start_secs, &a.start_time),
            source: TimelineSource::Scaling,
            what: a.description.clone(),
            who: a.status_code.clone(),
            detail,
            error: String::new(),
            failed: matches!(a.status_code.as_str(), "Failed" | "Cancelled"),
            json: String::new(),
        }
    }

    /// An RDS event: the message, with its categories where the actor goes.
    pub fn from_rds(e: &RdsEvent) -> Self {
        TimelineRow {
            ts_secs: e.ts_secs,
            time: fmt_ts(e.ts_secs, &e.date),
            source: TimelineSource::Rds,
            what: e.message.clone(),
            who: e.categories.join(", "),
            detail: String::new(),
            error: String::new(),
            failed: e
                .categories
                .iter()
                .any(|c| matches!(c.as_str(), "failure" | "low storage" | "failover")),
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

/// Auto Scaling activities as timeline rows: inside the lens window and,
/// when the lens is on an instance rather than its group, only the
/// activities that name that instance (launch, terminate, health
/// replacement, Spot interruption all do — in the description or cause).
pub fn scaling_rows(acts: &[ScalingActivity], cutoff: i64, instance: Option<&str>) -> Vec<TimelineRow> {
    acts.iter()
        .filter(|a| a.start_secs >= cutoff)
        .filter(|a| instance.is_none_or(|id| a.description.contains(id) || a.cause.contains(id)))
        .map(TimelineRow::from_scaling)
        .collect()
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

#[cfg(test)]
mod ops_source_tests {
    use super::*;

    fn act(secs: i64, status: &str, description: &str) -> ScalingActivity {
        ScalingActivity {
            start_time: String::new(),
            start_secs: secs,
            status_code: status.into(),
            description: description.into(),
            cause: "At 2026-10-13T01:00:00Z an instance was taken out of service in response to an EC2 health check".into(),
            progress: None,
            status_message: None,
        }
    }

    /// On an instance, only the activities naming it — and only inside
    /// the window.
    #[test]
    fn instance_lens_keeps_only_its_own_activities() {
        let acts = vec![
            act(2_000, "Successful", "Terminating EC2 instance: i-aaa"),
            act(2_000, "Successful", "Launching a new EC2 instance: i-bbb"),
            act(10, "Successful", "Terminating EC2 instance: i-aaa"),
        ];
        let rows = scaling_rows(&acts, 1_000, Some("i-aaa"));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].what, "Terminating EC2 instance: i-aaa");
        assert!(rows[0].detail.contains("health check"), "the cause explains why");
        assert_eq!(scaling_rows(&acts, 1_000, None).len(), 2, "the group sees both");
    }

    #[test]
    fn failures_are_marked() {
        assert!(TimelineRow::from_scaling(&act(1, "Failed", "Launching")).failed);
        assert!(!TimelineRow::from_scaling(&act(1, "Successful", "Launching")).failed);
        let ev = |cat: &str| RdsEvent {
            date: String::new(),
            ts_secs: 1,
            message: "Multi-AZ instance failover completed".into(),
            categories: vec![cat.into()],
        };
        assert!(TimelineRow::from_rds(&ev("failover")).failed);
        assert!(!TimelineRow::from_rds(&ev("notification")).failed);
        assert_eq!(TimelineRow::from_rds(&ev("failover")).source, TimelineSource::Rds);
    }
}
