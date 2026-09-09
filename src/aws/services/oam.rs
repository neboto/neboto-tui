//! CloudWatch Observability Access Manager (OAM) — the cross-account
//! observability wiring. Not a standalone `ServiceType`: sinks and links load
//! as a best-effort phase of the CloudWatch service and render on its
//! Cross-Account sub-tab. A **sink** lives on the monitoring account (who may
//! link, which telemetry is accepted); a **link** lives on each source account
//! (which sink it feeds, what it shares). An account is normally one side or
//! the other, so one grouped tab shows whichever exists.

use crate::aws::resource::{Resource, ResourceState};
use crate::error::sdk_error_message;
use aws_sdk_oam::Client as OamClient;
use std::any::Any;
use std::collections::HashMap;

// ── OamSink ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct OamSink {
    pub name: String,
    pub arn: String,
    pub sink_id: String,
    pub tags: HashMap<String, String>,
}

impl OamSink {
    pub fn from_sdk(s: &aws_sdk_oam::types::ListSinksItem) -> Self {
        Self {
            name: s.name().unwrap_or("").to_string(),
            arn: s.arn().unwrap_or("").to_string(),
            sink_id: s.id().unwrap_or("").to_string(),
            tags: HashMap::new(),
        }
    }
}

crate::sections! {
    pub enum OamSinkDetailSection,
    pub static OAM_SINK_SECTIONS = [
        Details "Details",
        Policy "Policy" => crate::app::App::trigger_oam_sink_policy_load,
        AttachedLinks "Attached Links" => crate::app::App::trigger_oam_attached_links_load,
    ]
}

impl Resource for OamSink {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&OAM_SINK_SECTIONS)
    }

    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws oam get-sink --identifier {}",
            crate::aws::resource::shell_quote(&self.arn)
        ))
    }

    fn id(&self) -> &str {
        &self.arn
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn resource_type(&self) -> &str {
        "OAM Sink"
    }

    fn state(&self) -> ResourceState {
        ResourceState::stateless()
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!("{} {} sink monitoring cross-account", self.name, self.arn)
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Sink".to_string(), self.name.clone()),
            ("Role".to_string(), "Monitoring account".to_string()),
            ("ARN".to_string(), self.arn.clone()),
        ]
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/cloudwatch/home?region={}#settings",
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

// ── OamLink ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct OamLink {
    pub label: String,
    pub arn: String,
    pub link_id: String,
    pub sink_arn: String,
    pub resource_types: Vec<String>,
    pub tags: HashMap<String, String>,
}

impl OamLink {
    pub fn from_sdk(l: &aws_sdk_oam::types::ListLinksItem) -> Self {
        Self {
            label: l.label().unwrap_or("").to_string(),
            arn: l.arn().unwrap_or("").to_string(),
            link_id: l.id().unwrap_or("").to_string(),
            sink_arn: l.sink_arn().unwrap_or("").to_string(),
            resource_types: l.resource_types().iter().map(|s| s.to_string()).collect(),
            tags: HashMap::new(),
        }
    }
}

crate::sections! {
    pub enum OamLinkDetailSection,
    pub static OAM_LINK_SECTIONS = [
        Details "Details",
        Configuration "Configuration" => crate::app::App::trigger_oam_link_detail_load,
    ]
}

impl Resource for OamLink {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&OAM_LINK_SECTIONS)
    }

    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws oam get-link --identifier {}",
            crate::aws::resource::shell_quote(&self.arn)
        ))
    }

    fn id(&self) -> &str {
        &self.arn
    }

    fn name(&self) -> &str {
        &self.label
    }

    fn resource_type(&self) -> &str {
        "OAM Link"
    }

    fn state(&self) -> ResourceState {
        ResourceState::stateless()
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!(
            "{} {} {} link source cross-account {}",
            self.label,
            self.arn,
            self.sink_arn,
            self.resource_types.join(" ")
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Link".to_string(), self.label.clone()),
            ("Role".to_string(), "Source account".to_string()),
            ("Sink".to_string(), self.sink_arn.clone()),
            ("Shares".to_string(), self.resource_types.join(", ")),
        ]
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/cloudwatch/home?region={}#settings",
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

// ── Lazy detail payloads ─────────────────────────────────────────────────────

/// The sink policy plus a parsed who-may-link summary. The raw JSON is kept
/// (pretty-printed) for the body and `e`; the parsed fields drive the summary
/// rows so the answer to "who can link here" doesn't require reading IAM JSON.
#[derive(Debug, Clone)]
pub struct OamSinkPolicy {
    pub policy_json: String,
    pub accounts: Vec<String>,
    pub org_ids: Vec<String>,
    pub org_paths: Vec<String>,
    pub telemetry_types: Vec<String>,
    /// `Principal: "*"` with no org condition — anyone may link.
    pub any_principal: bool,
}

#[derive(Debug, Clone)]
pub struct OamAttachedLink {
    pub label: String,
    pub link_arn: String,
    pub resource_types: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct OamLinkDetail {
    pub label_template: String,
    pub resource_types: Vec<String>,
    pub sink_arn: String,
    /// Metric-namespace filter narrowing what's shared (empty = everything).
    pub metric_filter: String,
    /// Log-group name filter narrowing what's shared (empty = everything).
    pub log_filter: String,
    pub tags: Vec<(String, String)>,
}

/// Account id segment of an OAM ARN (`arn:aws:oam:region:ACCOUNT:…`).
pub fn oam_arn_account(arn: &str) -> Option<&str> {
    arn.split(':').nth(4).filter(|s| !s.is_empty())
}

/// Friendly name for an OAM telemetry resource type; unknown types (the enum
/// grows — Application Signals arrived after launch) fall back to the raw
/// `AWS::…` string rather than hiding.
pub fn telemetry_display(raw: &str) -> &str {
    match raw {
        "AWS::CloudWatch::Metric" => "Metrics",
        "AWS::Logs::LogGroup" => "Log groups",
        "AWS::XRay::Trace" => "X-Ray traces",
        "AWS::ApplicationInsights::Application" => "Application Insights",
        "AWS::InternetMonitor::Monitor" => "Internet Monitor",
        "AWS::ApplicationSignals::Service" => "Application Signals services",
        "AWS::ApplicationSignals::ServiceLevelObjective" => "Application Signals SLOs",
        other => other,
    }
}

/// Compact comma list of telemetry types for one-line row values.
pub fn telemetry_summary(types: &[String]) -> String {
    if types.is_empty() {
        return "—".to_string();
    }
    types
        .iter()
        .map(|t| telemetry_display(t))
        .collect::<Vec<_>>()
        .join(", ")
}

// ── Fetches ──────────────────────────────────────────────────────────────────

pub async fn fetch_sinks(client: OamClient) -> Result<Vec<OamSink>, String> {
    let mut out = Vec::new();
    let mut paginator = client.list_sinks().into_paginator().send();
    while let Some(page) = paginator.next().await {
        let page = page.map_err(|e| sdk_error_message(&e))?;
        out.extend(page.items().iter().map(OamSink::from_sdk));
    }
    Ok(out)
}

pub async fn fetch_links(client: OamClient) -> Result<Vec<OamLink>, String> {
    let mut out = Vec::new();
    let mut paginator = client.list_links().into_paginator().send();
    while let Some(page) = paginator.next().await {
        let page = page.map_err(|e| sdk_error_message(&e))?;
        out.extend(page.items().iter().map(OamLink::from_sdk));
    }
    Ok(out)
}

pub async fn fetch_sink_policy(client: OamClient, sink_arn: String) -> Result<OamSinkPolicy, String> {
    let resp = client
        .get_sink_policy()
        .sink_identifier(&sink_arn)
        .send()
        .await
        .map_err(|e| sdk_error_message(&e))?;
    let raw = resp.policy().unwrap_or("").to_string();
    Ok(parse_sink_policy(&raw))
}

pub async fn fetch_attached_links(
    client: OamClient,
    sink_arn: String,
) -> Result<Vec<OamAttachedLink>, String> {
    let mut out = Vec::new();
    let mut paginator = client
        .list_attached_links()
        .sink_identifier(&sink_arn)
        .into_paginator()
        .send();
    while let Some(page) = paginator.next().await {
        let page = page.map_err(|e| sdk_error_message(&e))?;
        out.extend(page.items().iter().map(|l| OamAttachedLink {
            label: l.label().unwrap_or("").to_string(),
            link_arn: l.link_arn().unwrap_or("").to_string(),
            resource_types: l.resource_types().iter().map(|s| s.to_string()).collect(),
        }));
    }
    // Group a fleet of source accounts alphabetically rather than in API order.
    out.sort_by(|a, b| a.label.cmp(&b.label));
    Ok(out)
}

pub async fn fetch_link_detail(client: OamClient, link_arn: String) -> Result<OamLinkDetail, String> {
    let resp = client
        .get_link()
        .identifier(&link_arn)
        .send()
        .await
        .map_err(|e| sdk_error_message(&e))?;
    let cfg = resp.link_configuration();
    let mut tags: Vec<(String, String)> = resp
        .tags()
        .map(|t| t.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default();
    tags.sort();
    Ok(OamLinkDetail {
        label_template: resp.label_template().unwrap_or("").to_string(),
        resource_types: resp.resource_types().iter().map(|s| s.to_string()).collect(),
        sink_arn: resp.sink_arn().unwrap_or("").to_string(),
        metric_filter: cfg
            .and_then(|c| c.metric_configuration())
            .map(|m| m.filter().to_string())
            .unwrap_or_default(),
        log_filter: cfg
            .and_then(|c| c.log_group_configuration())
            .map(|l| l.filter().to_string())
            .unwrap_or_default(),
        tags,
    })
}

/// Parse the sink policy into a who-may-link summary. Best-effort: fields the
/// parse can't find just stay empty and the raw JSON is still shown in full.
pub fn parse_sink_policy(raw: &str) -> OamSinkPolicy {
    let parsed: Option<serde_json::Value> = serde_json::from_str(raw).ok();
    let policy_json = parsed
        .as_ref()
        .and_then(|v| serde_json::to_string_pretty(v).ok())
        .unwrap_or_else(|| raw.to_string());

    let mut accounts: Vec<String> = Vec::new();
    let mut org_ids: Vec<String> = Vec::new();
    let mut org_paths: Vec<String> = Vec::new();
    let mut telemetry_types: Vec<String> = Vec::new();
    let mut wildcard = false;

    // A value that may be a string or an array of strings.
    fn strings(v: &serde_json::Value) -> Vec<String> {
        match v {
            serde_json::Value::String(s) => vec![s.clone()],
            serde_json::Value::Array(a) => a
                .iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .collect(),
            _ => Vec::new(),
        }
    }

    if let Some(root) = parsed.as_ref() {
        let statements = match root.get("Statement") {
            Some(serde_json::Value::Array(a)) => a.iter().collect::<Vec<_>>(),
            Some(one) => vec![one],
            None => Vec::new(),
        };
        for st in statements {
            if st.get("Effect").and_then(|e| e.as_str()) != Some("Allow") {
                continue;
            }
            match st.get("Principal") {
                Some(serde_json::Value::String(s)) if s == "*" => wildcard = true,
                Some(p) => {
                    if let Some(aws) = p.get("AWS") {
                        for s in strings(aws) {
                            if s == "*" {
                                wildcard = true;
                            } else if !accounts.contains(&s) {
                                accounts.push(s);
                            }
                        }
                    }
                }
                None => {}
            }
            if let Some(cond) = st.get("Condition").and_then(|c| c.as_object()) {
                for op in cond.values() {
                    let Some(map) = op.as_object() else { continue };
                    for (key, val) in map {
                        let vals = strings(val);
                        // Condition keys are case-insensitive in IAM.
                        let target = match key.to_ascii_lowercase().as_str() {
                            "aws:principalorgid" => &mut org_ids,
                            "aws:principalorgpaths" => &mut org_paths,
                            "oam:resourcetypes" => &mut telemetry_types,
                            _ => continue,
                        };
                        for v in vals {
                            if !target.contains(&v) {
                                target.push(v);
                            }
                        }
                    }
                }
            }
        }
    }

    let any_principal = wildcard && org_ids.is_empty() && org_paths.is_empty();
    OamSinkPolicy {
        policy_json,
        accounts,
        org_ids,
        org_paths,
        telemetry_types,
        any_principal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_org_scoped_sink_policy() {
        let raw = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":"*","Action":["oam:CreateLink","oam:UpdateLink"],"Resource":"*","Condition":{"ForAllValues:StringEquals":{"oam:ResourceTypes":["AWS::CloudWatch::Metric","AWS::Logs::LogGroup"]},"ForAnyValue:StringEquals":{"aws:PrincipalOrgID":"o-abc123"}}}]}"#;
        let p = parse_sink_policy(raw);
        assert_eq!(p.org_ids, vec!["o-abc123"]);
        assert_eq!(
            p.telemetry_types,
            vec!["AWS::CloudWatch::Metric", "AWS::Logs::LogGroup"]
        );
        assert!(!p.any_principal, "org condition scopes the wildcard");
        assert!(p.accounts.is_empty());
    }

    #[test]
    fn parses_account_list_sink_policy() {
        let raw = r#"{"Statement":{"Effect":"Allow","Principal":{"AWS":["111111111111","222222222222"]},"Action":"oam:CreateLink","Resource":"*"}}"#;
        let p = parse_sink_policy(raw);
        assert_eq!(p.accounts, vec!["111111111111", "222222222222"]);
        assert!(!p.any_principal);
    }

    #[test]
    fn wildcard_without_condition_is_any_principal() {
        let raw = r#"{"Statement":[{"Effect":"Allow","Principal":"*","Action":"oam:CreateLink","Resource":"*"}]}"#;
        assert!(parse_sink_policy(raw).any_principal);
    }

    #[test]
    fn unparseable_policy_keeps_raw_text() {
        let p = parse_sink_policy("not json");
        assert_eq!(p.policy_json, "not json");
        assert!(!p.any_principal);
    }

    #[test]
    fn arn_account_segment() {
        assert_eq!(
            oam_arn_account("arn:aws:oam:eu-west-1:123456789012:link/uuid"),
            Some("123456789012")
        );
        assert_eq!(oam_arn_account("nonsense"), None);
    }
}
