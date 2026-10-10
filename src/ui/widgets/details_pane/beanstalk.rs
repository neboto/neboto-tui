use super::*;

// ── Elastic Beanstalk split panes ─────────────────────────────────────────────

pub(super) fn render_eb_environment_split(
    app: &App,
    e: &crate::aws::services::beanstalk::EbEnvironment,
    area: Rect,
    frame: &mut Frame,
) {
    let subtitle = format!(
        "{} · {} · {} · {}",
        e.application,
        e.deployment.label(),
        if e.tier_name.is_empty() { "—" } else { e.tier_name.as_str() },
        e.state_label()
    );
    render_simple_split(
        app,
        area,
        frame,
        "Beanstalk Environment",
        &e.name,
        &subtitle,
        &descriptor_tabs(app, &crate::aws::services::beanstalk::EB_ENVIRONMENT_SECTIONS),
    );
}

pub(super) fn render_eb_application_split(
    app: &App,
    a: &crate::aws::services::beanstalk::EbApplication,
    area: Rect,
    frame: &mut Frame,
) {
    let subtitle = a.description.clone().unwrap_or_else(|| "Elastic Beanstalk application".to_string());
    render_simple_split(
        app,
        area,
        frame,
        "Beanstalk Application",
        &a.name,
        &subtitle,
        &descriptor_tabs(app, &crate::aws::services::beanstalk::EB_APPLICATION_SECTIONS),
    );
}

pub(super) fn render_eb_version_split(
    app: &App,
    v: &crate::aws::services::beanstalk::EbVersion,
    area: Rect,
    frame: &mut Frame,
) {
    let subtitle = format!("{} · {}", v.application, v.state_label());
    render_simple_split(
        app,
        area,
        frame,
        "Beanstalk Application Version",
        &v.label,
        &subtitle,
        &descriptor_tabs(app, &crate::aws::services::beanstalk::EB_VERSION_SECTIONS),
    );
}

/// The four lazy sections of an environment pane, bundled so the
/// dispatcher in `section_detail_lines` stays one call.
pub struct EbEnvLazy<'a> {
    pub health: Option<&'a crate::lazy::Lazy<crate::aws::services::beanstalk::EbHealth>>,
    pub events: Option<&'a crate::lazy::Lazy<Vec<crate::aws::services::beanstalk::EbEvent>>>,
    pub config: Option<&'a crate::lazy::Lazy<crate::aws::services::beanstalk::EbConfig>>,
    pub resources: Option<&'a crate::lazy::Lazy<crate::aws::services::beanstalk::EbResources>>,
}

pub(super) fn eb_loading() -> Vec<(String, String)> {
    vec![("".to_string(), "Loading…".to_string())]
}

/// `✓`/`⚠`/`✗` prefix for a Beanstalk health colour (or enhanced status).
pub(super) fn eb_health_mark(word: &str) -> String {
    match word.to_ascii_lowercase().as_str() {
        "green" | "ok" | "info" => format!("✓ {word}"),
        "yellow" | "warning" | "degraded" | "pending" => format!("⚠ {word}"),
        "red" | "severe" => format!("✗ {word}"),
        _ => word.to_string(),
    }
}

/// Labels the Beanstalk row classifier (`App::eb_row_jump_target`) keys on —
/// renaming a row here without the classifier silently kills its jump.
pub const EB_ROW_APPLICATION: &str = "Application";
pub const EB_ROW_VERSION: &str = "Version Label";
pub const EB_ROW_ENVIRONMENT: &str = "Environment";
pub const EB_ROW_EKS_CLUSTER: &str = "EKS Cluster";
pub const EB_ROW_ASG: &str = "Auto Scaling Group";
pub const EB_ROW_LOAD_BALANCER: &str = "Load Balancer";
pub const EB_ROW_LAUNCH_TEMPLATE: &str = "Launch Template";
pub const EB_ROW_QUEUE: &str = "Queue";

pub fn eb_environment_section_lines(
    e: &crate::aws::services::beanstalk::EbEnvironment,
    section: crate::aws::services::beanstalk::EbEnvironmentDetailSection,
    lazy: EbEnvLazy<'_>,
) -> Vec<(String, String)> {
    use crate::aws::services::beanstalk::{EbDeploymentType, EbEnvironmentDetailSection as S};
    use crate::lazy::Lazy;
    match section {
        S::Overview => {
            let mut rows = vec![
                kv("Name", e.name.clone()),
                kv("Environment ID", e.id.clone()),
                kv(EB_ROW_APPLICATION, e.application.clone()),
                kv("Deployment Type", e.deployment.label()),
                kv(
                    "Tier",
                    match (e.tier_name.as_str(), e.tier_type.as_str()) {
                        ("", "") => "—".to_string(),
                        (n, "") => n.to_string(),
                        (n, t) => format!("{n} ({t})"),
                    },
                ),
                kv("Status", e.status.clone()),
                kv("Health", eb_health_mark(&e.health)),
            ];
            if let Some(h) = &e.health_status {
                rows.push(kv("Health Status", eb_health_mark(h)));
            }
            if e.abortable_operation {
                rows.push(kv("Operation", "⚠ an update is in progress (abortable)"));
            }
            rows.push((String::new(), String::new()));
            rows.push(("Deployment".to_string(), String::new()));
            rows.push(kv(
                &format!("  {EB_ROW_VERSION}"),
                e.version_label.clone().unwrap_or_else(|| "—".to_string()),
            ));
            match e.deployment {
                EbDeploymentType::Standard => {
                    rows.push(kv("  Platform", e.platform_label().unwrap_or_else(|| "—".to_string())));
                }
                EbDeploymentType::Cluster => {
                    rows.push(kv("  Platform", "· none — the runtime is the container image"));
                }
            }
            if let Some(p) = &e.platform_arn {
                rows.push(kv("  Platform ARN", p.clone()));
            }
            if let Some(t) = &e.template_name {
                rows.push(kv("  Saved Configuration", t.clone()));
            }
            rows.push((String::new(), String::new()));
            rows.push(("Endpoint".to_string(), String::new()));
            rows.push(kv("  URL", e.endpoint_url.clone().unwrap_or_else(|| "—".to_string())));
            if let Some(c) = &e.cname {
                rows.push(kv("  CNAME", c.clone()));
            }
            if let Some(lb) = &e.load_balancer {
                rows.push(kv(&format!("  {EB_ROW_LOAD_BALANCER}"), lb.name.clone()));
                if let Some(d) = &lb.domain {
                    rows.push(kv("  Load Balancer DNS", d.clone()));
                }
                if !lb.listeners.is_empty() {
                    rows.push(kv("  Listeners", lb.listeners.join(", ")));
                }
            }
            if e.deployment == EbDeploymentType::Cluster {
                rows.push((String::new(), String::new()));
                rows.push(("Cluster".to_string(), String::new()));
                match &e.cluster_arn {
                    Some(arn) => {
                        rows.push(kv(&format!("  {EB_ROW_EKS_CLUSTER}"), e.cluster_name().unwrap_or(arn.as_str()).to_string()));
                        rows.push(kv("  Cluster ARN", arn.clone()));
                        rows.push(kv(
                            "",
                            "· shared by every Cluster environment on the same subnets — U on the EKS cluster lists them",
                        ));
                    }
                    None => rows.push(kv(
                        &format!("  {EB_ROW_EKS_CLUSTER}"),
                        "· not resolved — see the Resources section",
                    )),
                }
                rows.push(kv("", "· cluster / node roles: Configuration › aws:elasticbeanstalk:eks"));
            }
            if !e.links.is_empty() {
                rows.push((String::new(), String::new()));
                rows.push(("Links".to_string(), String::new()));
                for (name, env) in &e.links {
                    rows.push(kv(&format!("  {EB_ROW_ENVIRONMENT}"), env.clone()));
                    rows.push(kv("    Link Name", name.clone()));
                }
            }
            rows.push((String::new(), String::new()));
            rows.push(("Other".to_string(), String::new()));
            if let Some(d) = &e.description {
                rows.push(kv("  Description", d.clone()));
            }
            if let Some(r) = &e.operations_role {
                rows.push(kv("  Operations Role", r.clone()));
            }
            if let Some(c) = &e.created {
                rows.push(kv("  Created", c.clone()));
            }
            if let Some(u) = &e.updated {
                rows.push(kv("  Updated", u.clone()));
            }
            if !e.arn.is_empty() {
                rows.push(kv("  ARN", e.arn.clone()));
            }
            rows
        }
        S::Health => match lazy.health {
            None | Some(Lazy::Loading) => eb_loading(),
            Some(Lazy::Error(err)) => {
                let mut rows = vec![kv("Health", eb_health_mark(&e.health))];
                rows.extend(error_rows(err));
                rows
            }
            Some(Lazy::Loaded(h)) => {
                let mut rows = Vec::new();
                if let Some(c) = &h.color {
                    rows.push(kv("Color", eb_health_mark(c)));
                }
                if let Some(s) = &h.health_status {
                    rows.push(kv("Health Status", eb_health_mark(s)));
                }
                if let Some(s) = &h.status {
                    rows.push(kv("Status", s.clone()));
                }
                if let Some(r) = &h.refreshed {
                    rows.push(kv("Refreshed", r.clone()));
                }
                rows.push((String::new(), String::new()));
                rows.push(("Causes".to_string(), String::new()));
                if h.causes.is_empty() {
                    rows.push(kv("", "✓ none reported"));
                } else {
                    for c in &h.causes {
                        rows.push((format!("  ⚠ {c}"), String::new()));
                    }
                }
                if !h.instances.is_empty() {
                    rows.push((String::new(), String::new()));
                    rows.push(("Instances".to_string(), String::new()));
                    for (k, n) in &h.instances {
                        let v = match *k {
                            "Degraded" | "Severe" => format!("✗ {n}"),
                            "Warning" => format!("⚠ {n}"),
                            _ => n.to_string(),
                        };
                        rows.push(kv(&format!("  {k}"), v));
                    }
                } else if e.deployment == EbDeploymentType::Cluster {
                    rows.push((String::new(), String::new()));
                    rows.push(kv("", "· Cluster environments don't report per-instance health"));
                }
                if let Some(m) = &h.requests {
                    rows.push((String::new(), String::new()));
                    let window = m.duration.map(|d| format!(" (last {d}s)")).unwrap_or_default();
                    rows.push((format!("Requests{window}"), String::new()));
                    rows.push(kv("  Count", m.request_count.to_string()));
                    let code = |n: Option<i32>| n.map(|n| n.to_string()).unwrap_or_else(|| "—".to_string());
                    let codes = format!(
                        "2xx {} · 3xx {} · 4xx {} · 5xx {}",
                        code(m.status_2xx),
                        code(m.status_3xx),
                        code(m.status_4xx),
                        code(m.status_5xx)
                    );
                    rows.push(kv(
                        "  Status Codes",
                        if m.status_5xx.unwrap_or(0) > 0 { format!("✗ {codes}") } else { codes },
                    ));
                    let lat = |v: Option<f64>| v.map(|s| format!("{:.0} ms", s * 1000.0)).unwrap_or_else(|| "—".to_string());
                    rows.push(kv(
                        "  Latency",
                        format!("p50 {} · p90 {} · p99 {}", lat(m.p50), lat(m.p90), lat(m.p99)),
                    ));
                }
                rows
            }
        },
        S::Events => match lazy.events {
            None | Some(Lazy::Loading) => eb_loading(),
            Some(Lazy::Error(err)) => error_rows(err),
            Some(Lazy::Loaded(evs)) if evs.is_empty() => {
                vec![kv("", "No events for this environment")]
            }
            Some(Lazy::Loaded(evs)) => {
                let mut rows = vec![kv(
                    "",
                    format!(
                        "· newest {} events{}",
                        evs.len(),
                        if evs.len() as i32 >= crate::aws::services::beanstalk::MAX_EVENTS { " (cap)" } else { "" }
                    ),
                )];
                rows.push((String::new(), String::new()));
                for ev in evs {
                    let mark = match ev.severity.as_str() {
                        "ERROR" | "FATAL" => "✗ ",
                        "WARN" => "⚠ ",
                        // Pad unmarked rows to the marker's width so the
                        // time column lines up across severities.
                        _ => "  ",
                    };
                    rows.push((
                        format!(
                            "  {mark}{:<20} {:<6} {}",
                            ev.time.as_deref().unwrap_or("—"),
                            ev.severity,
                            ev.message
                        ),
                        String::new(),
                    ));
                }
                rows
            }
        },
        S::Configuration => match lazy.config {
            None | Some(Lazy::Loading) => eb_loading(),
            Some(Lazy::Error(err)) => error_rows(err),
            Some(Lazy::Loaded(cfg)) if cfg.settings.is_empty() => {
                vec![kv("", "No configuration settings returned")]
            }
            Some(Lazy::Loaded(cfg)) => eb_config_rows(cfg),
        },
        S::Resources => match lazy.resources {
            None | Some(Lazy::Loading) => eb_loading(),
            Some(Lazy::Error(err)) => error_rows(err),
            Some(Lazy::Loaded(r)) if r.is_empty() => {
                vec![kv("", "No resources reported — the environment may still be launching")]
            }
            Some(Lazy::Loaded(r)) => eb_resource_rows(r),
        },
        S::Tags => tag_rows(&e.tags),
    }
}

/// Option settings grouped under a header per namespace. Values that look
/// like secrets are not special-cased: Beanstalk returns environment
/// properties verbatim, exactly as the console's Configuration page shows
/// them, and `elasticbeanstalk:DescribeConfigurationSettings` is the read
/// permission that already grants them.
pub fn eb_config_rows(cfg: &crate::aws::services::beanstalk::EbConfig) -> Vec<(String, String)> {
    let mut rows = Vec::new();
    if let Some(s) = &cfg.deployment_status {
        rows.push(kv("Deployment Status", s.clone()));
        rows.push((String::new(), String::new()));
    }
    let mut current: Option<(&str, Option<&str>)> = None;
    for s in &cfg.settings {
        let group = (s.namespace.as_str(), s.resource.as_deref());
        if current != Some(group) {
            if current.is_some() {
                rows.push((String::new(), String::new()));
            }
            let header = match s.resource.as_deref() {
                Some(r) => format!("{} [{r}]", s.namespace),
                None => s.namespace.clone(),
            };
            rows.push((header, String::new()));
            current = Some(group);
        }
        rows.push(kv(&format!("  {}", s.option), s.value.clone().unwrap_or_else(|| "—".to_string())));
    }
    rows
}

pub fn eb_resource_rows(r: &crate::aws::services::beanstalk::EbResources) -> Vec<(String, String)> {
    let mut rows = Vec::new();
    if let Some(arn) = &r.cluster_arn {
        rows.push(("Cluster".to_string(), String::new()));
        let name = arn.split_once(":cluster/").map(|(_, n)| n).unwrap_or(arn.as_str());
        rows.push(kv(&format!("  {EB_ROW_EKS_CLUSTER}"), name.to_string()));
        rows.push(kv("  Cluster ARN", arn.clone()));
    }
    fn group(rows: &mut Vec<(String, String)>, title: &str, label: &str, items: &[String]) {
        if items.is_empty() {
            return;
        }
        if !rows.is_empty() {
            rows.push((String::new(), String::new()));
        }
        rows.push((title.to_string(), String::new()));
        for i in items {
            rows.push(kv(&format!("  {label}"), i.clone()));
        }
    }
    group(&mut rows, "Auto Scaling Groups", EB_ROW_ASG, &r.auto_scaling_groups);
    group(&mut rows, "Instances", "Instance", &r.instances);
    group(&mut rows, "Load Balancers", EB_ROW_LOAD_BALANCER, &r.load_balancers);
    group(&mut rows, "Launch Templates", EB_ROW_LAUNCH_TEMPLATE, &r.launch_templates);
    group(&mut rows, "Launch Configurations", "Launch Configuration", &r.launch_configurations);
    group(&mut rows, "Scaling Triggers", "Trigger", &r.triggers);
    if !r.queues.is_empty() {
        if !rows.is_empty() {
            rows.push((String::new(), String::new()));
        }
        rows.push(("Queues".to_string(), String::new()));
        for (name, url) in &r.queues {
            rows.push(kv(&format!("  {EB_ROW_QUEUE}"), url.clone()));
            rows.push(kv("    Name", name.clone()));
        }
    }
    rows
}

pub fn eb_application_section_lines(
    a: &crate::aws::services::beanstalk::EbApplication,
    section: crate::aws::services::beanstalk::EbApplicationDetailSection,
    envs: &[&crate::aws::services::beanstalk::EbEnvironment],
    versions: &[&crate::aws::services::beanstalk::EbVersion],
) -> Vec<(String, String)> {
    use crate::aws::services::beanstalk::{EbApplicationDetailSection as S, EbDeploymentType};
    match section {
        S::Overview => {
            let standard = envs.iter().filter(|e| e.deployment == EbDeploymentType::Standard).count();
            let cluster = envs.len() - standard;
            let mut rows = vec![
                kv("Name", a.name.clone()),
                kv("Environments", format!("{} ({standard} Standard · {cluster} Cluster)", envs.len())),
                kv("Versions", versions.len().to_string()),
            ];
            if let Some(d) = &a.description {
                rows.push(kv("Description", d.clone()));
            }
            if let Some(c) = &a.created {
                rows.push(kv("Created", c.clone()));
            }
            if let Some(u) = &a.updated {
                rows.push(kv("Updated", u.clone()));
            }
            rows.push((String::new(), String::new()));
            rows.push(("Version Lifecycle".to_string(), String::new()));
            if a.lifecycle_rules.is_empty() {
                rows.push(kv("", "· no lifecycle policy — versions accumulate until the quota"));
            } else {
                for r in &a.lifecycle_rules {
                    rows.push(kv("  Rule", r.clone()));
                }
                if let Some(role) = &a.lifecycle_role {
                    rows.push(kv("  Service Role", role.clone()));
                }
            }
            if !a.configuration_templates.is_empty() {
                rows.push((String::new(), String::new()));
                rows.push(("Saved Configurations".to_string(), String::new()));
                for t in &a.configuration_templates {
                    rows.push((format!("  {t}"), String::new()));
                }
            }
            if !a.arn.is_empty() {
                rows.push((String::new(), String::new()));
                rows.push(kv("ARN", a.arn.clone()));
            }
            rows
        }
        S::Environments => {
            if envs.is_empty() {
                return vec![kv("", "No environments in this application")];
            }
            let mut rows = Vec::new();
            for (i, e) in envs.iter().enumerate() {
                if i > 0 {
                    rows.push((String::new(), String::new()));
                }
                rows.push(kv(EB_ROW_ENVIRONMENT, e.name.clone()));
                rows.push(kv("  Type", e.deployment.label()));
                rows.push(kv("  Health", eb_health_mark(&e.health)));
                rows.push(kv("  Status", e.status.clone()));
                if let Some(v) = &e.version_label {
                    rows.push(kv(&format!("  {EB_ROW_VERSION}"), v.clone()));
                }
                if let Some(c) = e.cluster_name() {
                    rows.push(kv(&format!("  {EB_ROW_EKS_CLUSTER}"), c.to_string()));
                }
            }
            rows
        }
        S::Versions => {
            if versions.is_empty() {
                return vec![kv("", "No application versions loaded for this application")];
            }
            let mut rows = vec![(
                format!("    {:<32} {:<12} {:<20} {}", "LABEL", "STATUS", "CREATED", "DEPLOYED TO"),
                String::new(),
            )];
            for v in versions {
                let deployed: Vec<&str> = envs
                    .iter()
                    .filter(|e| e.version_label.as_deref() == Some(v.label.as_str()))
                    .map(|e| e.name.as_str())
                    .collect();
                let mark = if v.status == "Failed" { "✗ " } else { "  " };
                rows.push((
                    format!(
                        "  {mark}{:<32} {:<12} {:<20} {}",
                        truncate_chars(&v.label, 32),
                        v.status,
                        v.created.as_deref().unwrap_or("—"),
                        deployed.join(", ")
                    ),
                    String::new(),
                ));
            }
            rows
        }
        S::Tags => tag_rows(&a.tags),
    }
}

pub fn eb_version_section_lines(
    v: &crate::aws::services::beanstalk::EbVersion,
    section: crate::aws::services::beanstalk::EbVersionDetailSection,
    deployed: &[&crate::aws::services::beanstalk::EbEnvironment],
) -> Vec<(String, String)> {
    use crate::aws::services::beanstalk::EbVersionDetailSection as S;
    match section {
        S::Overview => {
            let mut rows = vec![
                kv("Label", v.label.clone()),
                kv(EB_ROW_APPLICATION, v.application.clone()),
                kv(
                    "Status",
                    if v.status == "Failed" { format!("✗ {}", v.status) } else { v.status.clone() },
                ),
            ];
            if let Some(d) = &v.description {
                rows.push(kv("Description", d.clone()));
            }
            if let Some(c) = &v.created {
                rows.push(kv("Created", c.clone()));
            }
            rows.push((String::new(), String::new()));
            rows.push(("Deployed To".to_string(), String::new()));
            if deployed.is_empty() {
                rows.push(kv("", "· not running in any environment"));
            } else {
                for e in deployed {
                    rows.push(kv(&format!("  {EB_ROW_ENVIRONMENT}"), e.name.clone()));
                }
            }
            if !v.arn.is_empty() {
                rows.push((String::new(), String::new()));
                rows.push(kv("ARN", v.arn.clone()));
            }
            rows
        }
        S::Source => {
            let mut rows = Vec::new();
            if let Some(i) = &v.image_uri {
                rows.push(kv("Image", i.clone()));
            }
            if let Some(b) = &v.source_bundle {
                rows.push(kv("Source Bundle", b.clone()));
            }
            if let Some((ty, repo, loc)) = &v.source_build {
                rows.push(kv("Source Repository", format!("{repo} ({ty})")));
                rows.push(kv("Source Location", loc.clone()));
            }
            if let Some(b) = &v.build_arn {
                rows.push(kv("Build", b.clone()));
            }
            if let Some(r) = &v.build_role {
                rows.push(kv("Build Role", r.clone()));
            }
            if rows.is_empty() {
                rows.push(kv("", "No source recorded for this version"));
            }
            rows
        }
    }
}
