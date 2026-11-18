use super::*;
use crate::aws::services::mq::{
    deployment_label, MqBroker, MqBrokerDetailSection, MqConfigDetailSection, MqConfigDocument, MqConfiguration,
    MqRevision, MAX_REVISIONS, MQ_BROKER_SECTIONS, MQ_CONFIG_SECTIONS,
};

// ── Amazon MQ split panes ───────────────────────────────────────────────────

pub(super) fn render_mq_broker_split(app: &App, b: &MqBroker, area: Rect, frame: &mut Frame) {
    let mut subtitle = format!("{} · {}", b.state_label(), b.engine_text());
    if !b.deployment_mode.is_empty() {
        subtitle = format!("{subtitle} · {}", deployment_label(&b.deployment_mode));
    }
    if b.publicly_accessible == Some(true) {
        subtitle = format!("{subtitle} · public");
    }
    render_simple_split(app, area, frame, "MQ Broker", b.name(), &subtitle, &descriptor_tabs(app, &MQ_BROKER_SECTIONS));
}

pub(super) fn render_mq_config_split(app: &App, c: &MqConfiguration, area: Rect, frame: &mut Frame) {
    let subtitle = format!("{} · revision {}", c.engine_text(), c.latest_revision);
    render_simple_split(
        app,
        area,
        frame,
        "MQ Configuration",
        c.name(),
        &subtitle,
        &descriptor_tabs(app, &MQ_CONFIG_SECTIONS),
    );
}

/// Most rows a growing Overview block shows before pointing at its tab.
const MQ_PREVIEW_CAP: usize = 8;

fn mq_status(b: &MqBroker) -> String {
    let word = b.status.to_lowercase().replace('_', " ");
    match b.status.as_str() {
        "RUNNING" => format!("✓ {word}"),
        "CREATION_FAILED" | "CRITICAL_ACTION_REQUIRED" => format!("✗ {word}"),
        "" => "—".to_string(),
        _ => word,
    }
}

/// Every endpoint as `(instance number, endpoint)`, in the API's order.
fn mq_endpoints(b: &MqBroker) -> Vec<(usize, &str)> {
    b.instances
        .iter()
        .enumerate()
        .flat_map(|(i, inst)| inst.endpoints.iter().map(move |e| (i + 1, e.as_str())))
        .collect()
}

/// The encryption row: which key protects the broker's data at rest.
fn mq_encryption(b: &MqBroker) -> String {
    match (&b.kms_key_id, b.aws_owned_key) {
        (Some(k), _) => k.clone(),
        (None, Some(true)) => "AWS owned key".to_string(),
        (None, Some(false)) => "AWS managed key (aws/mq)".to_string(),
        (None, None) => "—".to_string(),
    }
}

pub fn mq_broker_section_lines(b: &MqBroker, section: MqBrokerDetailSection) -> Vec<(String, String)> {
    use MqBrokerDetailSection as S;
    let blank = || (String::new(), String::new());
    let not_described = || vec![kv("", "· DescribeBroker failed for this broker — r to retry")];
    match section {
        S::Overview => {
            let mut rows = vec![
                kv("Broker", b.name().to_string()),
                kv("Broker ID", b.id.clone()),
                kv("Status", mq_status(b)),
                kv("Engine", b.engine_text()),
                kv("Deployment", deployment_label(&b.deployment_mode)),
                kv("Instance Type", b.instance_type.clone()),
            ];
            if let Some(s) = &b.storage_type {
                rows.push(kv("Storage", s.to_lowercase()));
            }
            if let Some(c) = &b.created {
                rows.push(kv("Created", c.clone()));
            }
            rows.push(kv("ARN", b.arn.clone()));
            if !b.described {
                rows.push(blank());
                rows.extend(not_described());
                return rows;
            }

            if !b.actions_required.is_empty() {
                rows.push(blank());
                rows.push(("Action Required".to_string(), String::new()));
                for (code, info) in &b.actions_required {
                    rows.push(kv(&format!("  {code}"), format!("⚠ {info}")));
                }
            }

            rows.push(blank());
            rows.push(("Exposure".to_string(), String::new()));
            rows.push(kv(
                "  Publicly Accessible",
                match b.publicly_accessible {
                    Some(true) => "⚠ yes — endpoints resolve to public addresses",
                    Some(false) => "✓ no — reachable only inside the VPC",
                    None => "—",
                },
            ));
            rows.push(kv(
                "  Authentication",
                b.auth_strategy.as_deref().map(str::to_lowercase).unwrap_or_else(|| "—".to_string()),
            ));
            rows.push(kv("  Encryption Key", mq_encryption(b)));

            rows.push(blank());
            rows.push(("Configuration".to_string(), String::new()));
            match &b.config_current {
                Some((id, rev)) => {
                    rows.push(kv("  Configuration", id.clone()));
                    rows.push(kv("  Revision", rev.to_string()));
                }
                None => rows.push(kv("  Configuration", "—")),
            }
            let pending = b.pending_changes();
            if !pending.is_empty() {
                rows.push(kv(
                    "  Pending",
                    format!(
                        "⚠ {} change{} waiting for a reboot or the maintenance window — 5 Maintenance",
                        pending.len(),
                        if pending.len() == 1 { "" } else { "s" }
                    ),
                ));
            }

            let endpoints = mq_endpoints(b);
            if !endpoints.is_empty() {
                rows.push(blank());
                rows.push(("Endpoints".to_string(), String::new()));
                for (_, e) in endpoints.iter().take(MQ_PREVIEW_CAP) {
                    rows.push((format!("  {e}"), String::new()));
                }
                if endpoints.len() > MQ_PREVIEW_CAP {
                    rows.push(kv("", format!("· {} more — 2 Endpoints", endpoints.len() - MQ_PREVIEW_CAP)));
                }
            }

            rows.push(blank());
            rows.push(("Logs".to_string(), String::new()));
            match b.log_group() {
                Some(g) => rows.push(kv("  Log Group", format!("{g} (t to tail)"))),
                None => rows.push(kv("  Log Group", "· CloudWatch logging is off")),
            }
            rows
        }
        S::Endpoints => {
            if !b.described {
                return not_described();
            }
            if b.instances.is_empty() {
                return vec![kv("", "No endpoints yet")];
            }
            let mut rows = Vec::new();
            for (i, inst) in b.instances.iter().enumerate() {
                if i > 0 {
                    rows.push(blank());
                }
                rows.push((format!("Instance {}", i + 1), String::new()));
                if let Some(ip) = &inst.ip_address {
                    rows.push(kv("  IP Address", ip.clone()));
                }
                if let Some(u) = &inst.console_url {
                    rows.push(kv("  Web Console", u.clone()));
                }
                for e in &inst.endpoints {
                    rows.push(kv("  Endpoint", e.clone()));
                }
            }
            rows
        }
        S::Security => {
            if !b.described {
                return not_described();
            }
            let mut rows = vec![
                kv(
                    "Publicly Accessible",
                    match b.publicly_accessible {
                        Some(true) => "⚠ yes",
                        Some(false) => "✓ no",
                        None => "—",
                    },
                ),
                kv(
                    "Authentication",
                    b.auth_strategy.as_deref().map(str::to_lowercase).unwrap_or_else(|| "—".to_string()),
                ),
                kv("Encryption Key", mq_encryption(b)),
            ];
            rows.push(blank());
            rows.push(("Network".to_string(), String::new()));
            for s in &b.subnets {
                rows.push(kv("  Subnet", s.clone()));
            }
            for s in &b.security_groups {
                rows.push(kv("  Security Group", s.clone()));
            }
            if b.subnets.is_empty() && b.security_groups.is_empty() {
                rows.push(kv("", "· no subnets or security groups returned"));
            }
            if let Some(l) = &b.ldap {
                rows.push(blank());
                rows.push(("LDAP".to_string(), String::new()));
                for h in &l.hosts {
                    rows.push(kv("  Host", h.clone()));
                }
                for (k, v) in [
                    ("  Service Account", &l.service_account),
                    ("  User Base", &l.user_base),
                    ("  User Search", &l.user_search),
                    ("  Role Base", &l.role_base),
                    ("  Role Search", &l.role_search),
                ] {
                    if let Some(v) = v {
                        rows.push(kv(k, v.clone()));
                    }
                }
            }
            rows
        }
        S::Users => {
            if !b.described {
                return not_described();
            }
            if b.users.is_empty() {
                return vec![kv(
                    "",
                    if b.is_rabbit() {
                        "· RabbitMQ users live in the broker itself — this API doesn't list them"
                    } else if b.auth_strategy.as_deref() == Some("LDAP") {
                        "· users come from the LDAP directory — 3 Security"
                    } else {
                        "No users"
                    },
                )];
            }
            let mut rows = vec![("Users".to_string(), String::new())];
            for (name, pending) in &b.users {
                rows.push(kv(
                    &format!("  {name}"),
                    match pending {
                        Some(p) => format!("⚠ pending {p}"),
                        None => "✓ applied".to_string(),
                    },
                ));
            }
            rows.push(blank());
            rows.push(kv("", "· user names only — passwords are never read"));
            rows
        }
        S::Maintenance => {
            if !b.described {
                return not_described();
            }
            let onoff = |v: Option<bool>| match v {
                Some(true) => "✓ on",
                Some(false) => "off",
                None => "—",
            };
            let mut rows = vec![
                kv("Maintenance Window", b.maintenance_window.clone().unwrap_or_else(|| "—".to_string())),
                kv("Auto Minor Upgrade", onoff(b.auto_minor_version_upgrade)),
            ];
            rows.push(blank());
            rows.push(("Pending Changes".to_string(), String::new()));
            let pending = b.pending_changes();
            if pending.is_empty() {
                rows.push(kv("", "None"));
            }
            for (what, to) in pending {
                rows.push(kv(&format!("  {what}"), format!("⚠ → {to}")));
            }
            rows.push(blank());
            rows.push(("Logs".to_string(), String::new()));
            rows.push(kv("  General", onoff(b.logs_general)));
            if let Some(g) = &b.general_log_group {
                rows.push(kv("  General Log Group", g.clone()));
            }
            if !b.is_rabbit() {
                rows.push(kv("  Audit", onoff(b.logs_audit)));
                if let Some(g) = &b.audit_log_group {
                    rows.push(kv("  Audit Log Group", g.clone()));
                }
            }
            if b.replication_mode.as_deref().is_some_and(|m| m != "NONE") {
                rows.push(blank());
                rows.push(("Data Replication".to_string(), String::new()));
                rows.push(kv("  Mode", b.replication_mode.clone().unwrap_or_default().to_lowercase()));
                if let Some(r) = &b.replication_role {
                    rows.push(kv("  Role", r.to_lowercase()));
                }
                if let Some((id, region)) = &b.replication_counterpart {
                    rows.push(kv("  Counterpart", format!("{id} in {region}")));
                }
            }
            rows
        }
        S::Tags => tag_rows(&b.tags),
    }
}

/// The brokers running (or about to run) a configuration, as
/// `("  <name>", "<broker id>")` rows — the id is what `⏎` follows.
fn mq_config_broker_rows(rows: &mut Vec<(String, String)>, c: &MqConfiguration, brokers: &[&MqBroker]) {
    for b in brokers {
        rows.push(kv(&format!("  {}", b.name()), b.id.clone()));
        let rev = |r: i32| {
            if r == c.latest_revision {
                format!("revision {r} (latest)")
            } else {
                format!("⚠ revision {r} — latest is {}", c.latest_revision)
            }
        };
        match (&b.config_current, &b.config_pending) {
            (Some((id, r)), _) if *id == c.id => rows.push(kv("    Running", rev(*r))),
            (_, Some((_, r))) => rows.push(kv("    Pending", rev(*r))),
            _ => {}
        }
    }
}

pub fn mq_config_section_lines(
    c: &MqConfiguration,
    section: MqConfigDetailSection,
    document: Option<&Lazy<MqConfigDocument>>,
    revisions: Option<&Lazy<Vec<MqRevision>>>,
    brokers: &[&MqBroker],
) -> Vec<(String, String)> {
    use MqConfigDetailSection as S;
    let blank = || (String::new(), String::new());
    match section {
        S::Overview => {
            let mut rows = vec![
                kv("Configuration", c.name().to_string()),
                kv("Configuration ID", c.id.clone()),
                kv("Engine", c.engine_text()),
            ];
            if let Some(a) = &c.auth_strategy {
                rows.push(kv("Authentication", a.to_lowercase()));
            }
            if let Some(d) = &c.description {
                rows.push(kv("Description", d.clone()));
            }
            if let Some(t) = &c.created {
                rows.push(kv("Created", t.clone()));
            }
            rows.push(kv("ARN", c.arn.clone()));

            rows.push(blank());
            rows.push(("Latest Revision".to_string(), String::new()));
            rows.push(kv("  Revision", c.latest_revision.to_string()));
            if let Some(t) = &c.latest_created {
                rows.push(kv("  Created", t.clone()));
            }
            if let Some(d) = &c.latest_description {
                rows.push(kv("  Description", d.clone()));
            }

            rows.push(blank());
            rows.push(("Used By".to_string(), String::new()));
            if brokers.is_empty() {
                rows.push(kv("", "⚠ no broker uses this configuration"));
            } else {
                let shown = &brokers[..brokers.len().min(MQ_PREVIEW_CAP)];
                mq_config_broker_rows(&mut rows, c, shown);
                if brokers.len() > MQ_PREVIEW_CAP {
                    rows.push(kv("", format!("· {} more brokers", brokers.len() - MQ_PREVIEW_CAP)));
                }
            }
            rows
        }
        S::Document => match document {
            None | Some(Lazy::Loading) => vec![kv("", "Loading…")],
            Some(Lazy::Error(e)) => error_rows(e),
            Some(Lazy::Loaded(d)) => {
                let mut rows = vec![kv("Revision", d.revision.to_string())];
                if let Some(t) = &d.created {
                    rows.push(kv("Created", t.clone()));
                }
                if let Some(t) = &d.description {
                    rows.push(kv("Description", t.clone()));
                }
                rows.push(blank());
                rows.push(("Document".to_string(), String::new()));
                if d.lines.iter().all(|l| l.trim().is_empty()) {
                    rows.push(("  (empty)".to_string(), String::new()));
                }
                for line in &d.lines {
                    // A blank line would read as a spacer and be collapsed.
                    rows.push((format!("  {line}"), String::new()));
                }
                rows
            }
        },
        S::Revisions => match revisions {
            None | Some(Lazy::Loading) => vec![kv("", "Loading…")],
            Some(Lazy::Error(e)) => error_rows(e),
            Some(Lazy::Loaded(revs)) if revs.is_empty() => vec![kv("", "No revisions")],
            Some(Lazy::Loaded(revs)) => {
                let mut rows = Vec::new();
                for r in revs {
                    let running: Vec<&str> = brokers
                        .iter()
                        .filter(|b| b.config_current.as_ref() == Some(&(c.id.clone(), r.revision)))
                        .map(|b| b.name())
                        .collect();
                    rows.push((format!("Revision {}", r.revision), String::new()));
                    rows.push(kv("  Created", r.created.clone().unwrap_or_else(|| "—".to_string())));
                    if let Some(d) = &r.description {
                        rows.push(kv("  Description", d.clone()));
                    }
                    if !running.is_empty() {
                        rows.push(kv("  Running On", running.join(", ")));
                    }
                }
                if revs.len() >= MAX_REVISIONS {
                    rows.push(blank());
                    rows.push(kv("", format!("· the newest {MAX_REVISIONS} revisions")));
                }
                rows
            }
        },
        S::Tags => tag_rows(&c.tags),
    }
}
