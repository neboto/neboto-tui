use super::*;
use crate::aws::services::apiportal::{
    enum_word, source_label, ApiDiscovery, ApiDiscoveryDetailSection, ApiInventoryItem, ApiItemDetailSection, ApiPortal,
    ApiPortalDetailSection, ApiProduct, ApiProductDetailSection, ManagedApi, ManagedApiDetailSection,
    ManagedApiEvent, ManagedApiEvents, ManagedApiFinding, API_DISCOVERY_SECTIONS, API_ITEM_SECTIONS,
    API_PORTAL_SECTIONS, API_PRODUCT_SECTIONS, MANAGED_API_SECTIONS, MAX_EVENTS,
};

// ── API Portal split panes ──────────────────────────────────────────────────

/// Most rows a growing Overview block shows before pointing at its tab.
const APIP_PREVIEW_CAP: usize = 8;

pub(super) fn render_apiportal_item_split(app: &App, i: &ApiInventoryItem, area: Rect, frame: &mut Frame) {
    let mut subtitle = i.state_label();
    if i.is_public() {
        subtitle.push_str(" · public");
    }
    if i.waf_off() {
        subtitle.push_str(" · no WAF");
    }
    render_simple_split(app, area, frame, "API Inventory Item", i.name(), &subtitle, &descriptor_tabs(app, &API_ITEM_SECTIONS));
}

pub(super) fn render_apiportal_managed_split(app: &App, m: &ManagedApi, area: Rect, frame: &mut Frame) {
    let subtitle = m.state_label();
    render_simple_split(app, area, frame, "Managed API", m.name(), &subtitle, &descriptor_tabs(app, &MANAGED_API_SECTIONS));
}

pub(super) fn render_apiportal_portal_split(app: &App, p: &ApiPortal, area: Rect, frame: &mut Frame) {
    let subtitle = match p.url() {
        Some(u) => format!("{} · {u}", p.state_label()),
        None => p.state_label(),
    };
    render_simple_split(app, area, frame, "API Portal", p.name(), &subtitle, &descriptor_tabs(app, &API_PORTAL_SECTIONS));
}

pub(super) fn render_apiportal_product_split(app: &App, p: &ApiProduct, area: Rect, frame: &mut Frame) {
    render_simple_split(
        app,
        area,
        frame,
        "API Product",
        p.name(),
        &p.state_label(),
        &descriptor_tabs(app, &API_PRODUCT_SECTIONS),
    );
}

pub(super) fn render_apiportal_discovery_split(app: &App, d: &ApiDiscovery, area: Rect, frame: &mut Frame) {
    render_simple_split(
        app,
        area,
        frame,
        "API Discovery",
        d.name(),
        &d.state_label(),
        &descriptor_tabs(app, &API_DISCOVERY_SECTIONS),
    );
}

fn dash(v: &Option<String>) -> String {
    v.clone().unwrap_or_else(|| "—".to_string())
}

/// What a classification means for the API, in a line.
fn classification_text(c: Option<&str>) -> String {
    match c {
        Some("SHADOW") => "⚠ shadow — no account in the organization manages this API".to_string(),
        Some("MANAGED") => "✓ managed".to_string(),
        Some("MANAGED_ZOMBIE") => "⚠ managed zombie — no traffic for the no-traffic window, still reachable".to_string(),
        Some("MANAGED_ORPHANED") => {
            "⚠ managed orphaned — its account left the organization; a last-known snapshot".to_string()
        }
        Some(other) => enum_word(other),
        None => "—".to_string(),
    }
}

pub fn apiportal_item_section_lines(i: &ApiInventoryItem, section: ApiItemDetailSection) -> Vec<(String, String)> {
    // One section today; the match keeps a second from being missed.
    match section {
        ApiItemDetailSection::Overview => {}
    }
    let blank = || (String::new(), String::new());
    let mut rows = vec![
        kv("API", i.signature()),
        kv("Method", i.method.clone()),
        kv("Host", i.host.clone()),
        kv("Port", i.port.to_string()),
        kv("Path", i.path.clone()),
        kv("Classification", classification_text(i.classification.as_deref())),
    ];
    if let Some(m) = &i.managed_by {
        rows.push(kv(
            "Managed By",
            match m.as_str() {
                "SELF" => "this account".to_string(),
                "ORGANIZATION" => "another account in the organization".to_string(),
                other => enum_word(other),
            },
        ));
    }
    if let Some(id) = &i.managed_api_id {
        rows.push(kv("Managed API", id.clone()));
    }

    rows.push(blank());
    rows.push(("Exposure".to_string(), String::new()));
    rows.push(kv(
        "  Visibility",
        match i.visibility.as_deref() {
            Some("PUBLIC") => "⚠ public — reachable from the internet".to_string(),
            Some("PRIVATE") => "✓ private — reachable only inside a private network".to_string(),
            Some(other) => enum_word(other),
            None => "—".to_string(),
        },
    ));
    rows.push(kv(
        "  WAF",
        match i.waf_status.as_deref() {
            Some("ENABLED") => "✓ a web ACL is attached to the front door".to_string(),
            Some("DISABLED") => "⚠ no web ACL on the front door".to_string(),
            Some(other) => enum_word(other),
            None => "—".to_string(),
        },
    ));
    rows.push(kv(
        "  AI Workloads",
        if i.workloads.is_empty() {
            "· none reported (needs AI discovery on)".to_string()
        } else {
            i.workloads.join(", ")
        },
    ));
    if i.is_public() && i.is_shadow() && i.waf_off() {
        rows.push(kv(
            "",
            if i.is_ai() {
                "✗ public, unmanaged and without WAF — and it serves AI traffic"
            } else {
                "⚠ public, unmanaged and without WAF"
            },
        ));
    }

    rows.push(blank());
    rows.push(("Discovery".to_string(), String::new()));
    rows.push(kv("  Source", source_label(&i.source)));
    rows.push(kv("  Resource", i.resource_arn.clone()));
    rows.push(kv("  Account", i.account_id.clone()));
    rows.push(kv("  Region", i.region.clone()));
    rows.push(kv("  First Seen", dash(&i.first_seen)));
    rows.push(kv("  Last Seen", dash(&i.last_seen)));
    rows
}

/// `✓ satisfied` / `✗ <reason>` for one posture check.
fn finding_text(f: &ManagedApiFinding) -> String {
    let reason = f.reason.as_deref().map(enum_word);
    match f.status.as_str() {
        "TRUE" => "✓ satisfied".to_string(),
        "FALSE" => format!("✗ {}", reason.unwrap_or_else(|| "not satisfied".to_string())),
        "INSUFFICIENT_DATA" => match reason {
            Some(r) => format!("· insufficient data — {r}"),
            None => "· insufficient data".to_string(),
        },
        "NOT_APPLICABLE" => "· not applicable".to_string(),
        other => enum_word(other),
    }
}

fn finding_rows(rows: &mut Vec<(String, String)>, findings: &[&ManagedApiFinding]) {
    for f in findings {
        let key = if f.posture_id.is_empty() { "  Posture".to_string() } else { format!("  {}", f.posture_id) };
        rows.push(kv(&key, finding_text(f)));
    }
}

fn event_rows(rows: &mut Vec<(String, String)>, e: &ManagedApiEvent) {
    let title = e.label.clone().or_else(|| e.event_type.as_deref().map(enum_word)).unwrap_or_else(|| e.id.clone());
    rows.push((title, String::new()));
    rows.push(kv("  Status", e.status.as_deref().map(|s| enum_word(s.trim_start_matches("EVENT_"))).unwrap_or_else(|| "—".to_string())));
    if let Some(c) = &e.category {
        rows.push(kv("  Category", c.clone()));
    }
    if let Some(c) = &e.confidence {
        rows.push(kv("  Confidence", enum_word(c)));
    }
    rows.push(kv("  Started", dash(&e.started)));
    if let Some(t) = &e.ended {
        rows.push(kv("  Ended", t.clone()));
    }
    if let Some(w) = &e.web_acl_arn {
        rows.push(kv("  Web ACL", w.clone()));
    }
    rows.push(kv("  Event ID", e.id.clone()));
}

pub fn apiportal_managed_section_lines(
    m: &ManagedApi,
    section: ManagedApiDetailSection,
    events: Option<&Lazy<ManagedApiEvents>>,
) -> Vec<(String, String)> {
    use ManagedApiDetailSection as S;
    let blank = || (String::new(), String::new());
    let not_described = || vec![kv("", "· GetManagedApi failed for this API — r to retry")];
    match section {
        S::Overview => {
            let mut rows = vec![
                kv("API", m.name().to_string()),
                kv("Managed API ID", m.id.clone()),
                kv("Resource", m.resource_arn.clone()),
            ];
            if let Some(i) = &m.inventory_item {
                rows.push(kv("Inventory Item", i.clone()));
            }
            rows.push(kv("Created", dash(&m.created)));
            rows.push(kv("Updated", dash(&m.updated)));
            rows.push(kv("ARN", m.arn.clone()));
            if !m.described {
                rows.push(blank());
                rows.extend(not_described());
                return rows;
            }
            rows.push(blank());
            rows.push(("Posture".to_string(), String::new()));
            if m.findings.is_empty() {
                rows.push(kv("", "No posture checks reported"));
            } else {
                let failing: Vec<&ManagedApiFinding> = m.findings.iter().filter(|f| f.failing()).collect();
                if failing.is_empty() {
                    rows.push(kv("  Checks", format!("✓ all {} satisfied", m.findings.len())));
                } else {
                    rows.push(kv("  Checks", format!("✗ {} of {} failing", failing.len(), m.findings.len())));
                    finding_rows(&mut rows, &failing[..failing.len().min(APIP_PREVIEW_CAP)]);
                    if failing.len() > APIP_PREVIEW_CAP {
                        rows.push(kv("", format!("· {} more — 2 Findings", failing.len() - APIP_PREVIEW_CAP)));
                    }
                }
            }
            rows
        }
        S::Findings => {
            if !m.described {
                return not_described();
            }
            if m.findings.is_empty() {
                return vec![kv("", "No posture checks reported")];
            }
            // Failing first, then the rest in the API's order.
            let mut sorted: Vec<&ManagedApiFinding> = m.findings.iter().collect();
            sorted.sort_by_key(|f| !f.failing());
            let mut rows = Vec::new();
            for f in sorted {
                let key = if f.posture_id.is_empty() { "Posture".to_string() } else { f.posture_id.clone() };
                rows.push((key, String::new()));
                rows.push(kv("  Result", finding_text(f)));
                if let Some(t) = &f.updated {
                    rows.push(kv("  Updated", t.clone()));
                }
            }
            rows
        }
        S::Events => match events {
            None | Some(Lazy::Loading) => vec![kv("", "Loading…")],
            Some(Lazy::Error(e)) => error_rows(e),
            Some(Lazy::Loaded(ev)) if ev.items.is_empty() => vec![kv("", "No security events in the last 30 days")],
            Some(Lazy::Loaded(ev)) => {
                let mut rows = Vec::new();
                for e in &ev.items {
                    rows.push(blank());
                    event_rows(&mut rows, e);
                }
                if ev.truncated {
                    rows.push(blank());
                    rows.push(kv("", format!("· the newest {MAX_EVENTS} events")));
                }
                rows
            }
        },
        S::Tags => tag_rows(&m.tags),
    }
}

pub fn apiportal_portal_section_lines(
    p: &ApiPortal,
    section: ApiPortalDetailSection,
    products: &[&ApiProduct],
) -> Vec<(String, String)> {
    use ApiPortalDetailSection as S;
    let blank = || (String::new(), String::new());
    // `(name, ARN)` of each included product; the ARN is what jumps.
    let included: Vec<(String, String)> = p
        .product_arns
        .iter()
        .map(|arn| {
            let name = products.iter().find(|x| &x.arn == arn).map(|x| x.name().to_string());
            (name.unwrap_or_else(|| arn.rsplit('/').next().unwrap_or(arn).to_string()), arn.clone())
        })
        .collect();
    let product_rows = |rows: &mut Vec<(String, String)>, list: &[(String, String)]| {
        for (name, arn) in list {
            rows.push(kv(&format!("  {name}"), arn.clone()));
        }
    };
    match section {
        S::Overview => {
            let mut rows = vec![
                kv("Portal", p.name().to_string()),
                kv("Portal ID", p.id.clone()),
                kv(
                    "Publish Status",
                    match p.publish_status.as_deref() {
                        Some("PUBLISHED") => "✓ published".to_string(),
                        Some(s @ ("PUBLISH_FAILED" | "DISABLE_FAILED")) => format!("✗ {}", enum_word(s)),
                        Some(s) => enum_word(s),
                        None => "—".to_string(),
                    },
                ),
            ];
            if let Some((ex, msg)) = &p.status_exception {
                rows.push(kv("Status Exception", format!("✗ {} — {msg}", enum_word(ex))));
            }
            if let Some(d) = &p.description {
                rows.push(kv("Description", d.clone()));
            }
            rows.push(kv("Updated", dash(&p.updated)));
            if let Some(t) = &p.published {
                rows.push(kv("Published", t.clone()));
            }
            if let Some(d) = &p.publish_description {
                rows.push(kv("Publish Note", d.clone()));
            }
            rows.push(kv("ARN", p.arn.clone()));

            rows.push(blank());
            rows.push(("Access".to_string(), String::new()));
            rows.push(kv("  URL", p.url().unwrap_or_else(|| "—".to_string())));
            if let Some(d) = &p.domain_name {
                rows.push(kv("  Custom Domain", d.clone()));
            }
            if let Some(d) = &p.default_domain {
                rows.push(kv("  Default Domain", d.clone()));
            }
            if let Some(c) = &p.certificate_arn {
                rows.push(kv("  Certificate", c.clone()));
            }
            rows.push(kv(
                "  Sign-in",
                match p.authorization.as_deref() {
                    Some("none") => "⚠ none — anyone with the URL can browse".to_string(),
                    Some(a) => a.to_string(),
                    None => "—".to_string(),
                },
            ));
            if let Some(u) = &p.cognito_user_pool_arn {
                rows.push(kv("  User Pool", u.clone()));
            }
            if let Some(r) = &p.rum_app_monitor {
                rows.push(kv("  RUM App Monitor", r.clone()));
            }
            if let Some(s) = &p.preview_status {
                rows.push(kv("  Preview", enum_word(s)));
            }
            if let Some(u) = &p.preview_url {
                rows.push(kv("  Preview URL", u.clone()));
            }

            rows.push(blank());
            rows.push(("Products".to_string(), String::new()));
            if included.is_empty() {
                rows.push(kv("", "No products"));
            } else {
                product_rows(&mut rows, &included[..included.len().min(APIP_PREVIEW_CAP)]);
                if included.len() > APIP_PREVIEW_CAP {
                    rows.push(kv("", format!("· {} more — 2 Products", included.len() - APIP_PREVIEW_CAP)));
                }
            }
            rows
        }
        S::Products => {
            if included.is_empty() {
                return vec![kv("", "No products")];
            }
            let mut rows = Vec::new();
            product_rows(&mut rows, &included);
            rows
        }
    }
}

pub fn apiportal_product_section_lines(
    p: &ApiProduct,
    section: ApiProductDetailSection,
    apis: Option<&Lazy<Vec<(String, String)>>>,
    portals: &[&ApiPortal],
) -> Vec<(String, String)> {
    use ApiProductDetailSection as S;
    let blank = || (String::new(), String::new());
    let api_rows = |rows: &mut Vec<(String, String)>, cap: usize, more: bool| match apis {
        None | Some(Lazy::Loading) => rows.push(kv("", "Loading…")),
        Some(Lazy::Error(e)) => rows.extend(error_rows(e)),
        Some(Lazy::Loaded(list)) if list.is_empty() => rows.push(kv("", "No APIs")),
        Some(Lazy::Loaded(list)) => {
            for (id, sig) in list.iter().take(cap) {
                rows.push(kv(&format!("  {sig}"), id.clone()));
            }
            if more && list.len() > cap {
                rows.push(kv("", format!("· {} more — 2 APIs", list.len() - cap)));
            }
        }
    };
    match section {
        S::Overview => {
            let mut rows = vec![
                kv("Product", p.name().to_string()),
                kv("Product ID", p.id.clone()),
                kv(
                    "Status",
                    match p.status.as_deref() {
                        Some("ACTIVE") => "✓ active".to_string(),
                        Some("FAILED") => "✗ failed".to_string(),
                        Some(s) => enum_word(s),
                        None => "—".to_string(),
                    },
                ),
            ];
            if let Some(r) = &p.failure_reason {
                rows.push(kv("Failure Reason", format!("✗ {}", enum_word(r))));
            }
            if let Some(d) = &p.description {
                rows.push(kv("Description", d.clone()));
            }
            rows.push(kv("Updated", dash(&p.updated)));
            rows.push(kv("ARN", p.arn.clone()));

            rows.push(blank());
            rows.push(("API Selection".to_string(), String::new()));
            rows.push(kv("  Mode", dash(&p.selection_mode)));
            for (key, values) in &p.rule_filters {
                rows.push(kv(&format!("  {}", enum_word(key)), values.join(", ")));
            }

            rows.push(blank());
            rows.push(("Portals".to_string(), String::new()));
            if portals.is_empty() {
                rows.push(kv("", "· no loaded portal includes this product"));
            }
            for portal in portals {
                rows.push(kv(&format!("  {}", portal.name()), portal.id.clone()));
            }

            rows.push(blank());
            rows.push(("APIs".to_string(), String::new()));
            api_rows(&mut rows, APIP_PREVIEW_CAP, true);
            rows
        }
        S::Apis => {
            let mut rows = Vec::new();
            api_rows(&mut rows, usize::MAX, false);
            rows
        }
    }
}

pub fn apiportal_discovery_section_lines(d: &ApiDiscovery, section: ApiDiscoveryDetailSection) -> Vec<(String, String)> {
    use ApiDiscoveryDetailSection as S;
    let blank = || (String::new(), String::new());
    let list = |v: &[String], f: fn(&str) -> String| if v.is_empty() { "—".to_string() } else { v.iter().map(|s| f(s)).collect::<Vec<_>>().join(", ") };
    match section {
        S::Overview => {
            let mut rows = vec![
                kv("Scope", if d.is_organization() { "organization" } else { "this account" }),
                kv(
                    "Status",
                    match d.status.as_deref() {
                        Some("ENABLED") => "✓ enabled".to_string(),
                        Some("DISABLED") => "⚠ disabled".to_string(),
                        Some(s) => enum_word(s),
                        None if d.error.is_some() => "—".to_string(),
                        None => "⚠ not configured — discovery has never been set up here".to_string(),
                    },
                ),
                kv("Sources", list(&d.sources, source_label)),
                kv("Features", list(&d.features, enum_word)),
            ];
            if let Some(t) = &d.created {
                rows.push(kv("Created", t.clone()));
            }
            if let Some(t) = &d.updated {
                rows.push(kv("Updated", t.clone()));
            }
            if let Some(a) = &d.arn {
                rows.push(kv("ARN", a.clone()));
            }
            if let Some(e) = &d.error {
                rows.extend(error_rows(e));
            }
            if !d.excluded_resources.is_empty() {
                rows.push(blank());
                rows.push(("Excluded Resources".to_string(), String::new()));
                for r in d.excluded_resources.iter().take(APIP_PREVIEW_CAP) {
                    rows.push((format!("  {r}"), String::new()));
                }
                if d.excluded_resources.len() > APIP_PREVIEW_CAP {
                    rows.push(kv("", format!("· {} more", d.excluded_resources.len() - APIP_PREVIEW_CAP)));
                }
            }
            if !d.statuses.is_empty() {
                rows.push(blank());
                rows.push(("Crawlers".to_string(), String::new()));
                for s in d.statuses.iter().take(APIP_PREVIEW_CAP) {
                    rows.push(status_row(s));
                }
                if d.statuses.len() > APIP_PREVIEW_CAP {
                    rows.push(kv("", format!("· {} more — 2 Statuses", d.statuses.len() - APIP_PREVIEW_CAP)));
                }
            }
            rows
        }
        S::Statuses => {
            if d.is_organization() {
                return vec![kv("", "· per-source statuses are on the account row")];
            }
            if d.statuses.is_empty() {
                return vec![kv("", "No discovery statuses")];
            }
            d.statuses.iter().map(status_row).collect()
        }
        S::Tags => tag_rows(&d.tags),
    }
}

fn status_row(s: &crate::aws::services::apiportal::DiscoveryStatus) -> (String, String) {
    let key = match &s.account_id {
        Some(a) => format!("  {} · {a}", source_label(&s.source)),
        None => format!("  {}", source_label(&s.source)),
    };
    let value = match s.status.as_str() {
        "RUNNING" => "✓ running".to_string(),
        "FAILED" => format!(
            "✗ failed{}",
            s.failure_reason.as_deref().map(|r| format!(" — {}", enum_word(r))).unwrap_or_default()
        ),
        other => enum_word(other),
    };
    kv(&key, value)
}
