use super::*;

// ── X-Ray split panes ───────────────────────────────────────────────────────

pub(super) fn render_xray_node_split(
    app: &App,
    n: &crate::aws::services::xray::XRayNode,
    area: Rect,
    frame: &mut Frame,
) {
    let label = n.state_label();
    let subtitle = if label.is_empty() {
        format!("{} · last {}", n.node_type, n.window)
    } else {
        format!("{} · {} · last {}", n.node_type, label, n.window)
    };
    render_simple_split(
        app,
        area,
        frame,
        "X-Ray Service",
        &n.name,
        &subtitle,
        &descriptor_tabs(app, &crate::aws::services::xray::XRAY_NODE_SECTIONS),
    );
}

pub(super) fn render_xray_trace_split(
    app: &App,
    t: &crate::aws::services::xray::XRayTrace,
    area: Rect,
    frame: &mut Frame,
) {
    let mut subtitle = t.state_label();
    if let Some(d) = t.duration {
        subtitle.push_str(&format!(" · {}", crate::aws::services::xray::fmt_secs(d)));
    }
    if let Some(s) = t.status {
        subtitle.push_str(&format!(" · HTTP {s}"));
    }
    render_simple_split(
        app,
        area,
        frame,
        "X-Ray Trace",
        &t.label,
        &subtitle,
        &descriptor_tabs(app, &crate::aws::services::xray::XRAY_TRACE_SECTIONS),
    );
}

/// Request-count block shared by node Overview and edge rows.
pub(super) fn xray_stats_value(s: &crate::aws::services::xray::XRayStats, n: i64) -> String {
    format!("{n} ({:.1}%)", s.pct(n))
}

pub(super) fn xray_edge_rows(
    edges: &[crate::aws::services::xray::XRayEdge],
    empty: &str,
) -> Vec<(String, String)> {
    use crate::aws::services::xray::fmt_secs;
    if edges.is_empty() {
        return vec![("".to_string(), empty.to_string())];
    }
    let mut edges: Vec<&crate::aws::services::xray::XRayEdge> = edges.iter().collect();
    edges.sort_by(|a, b| {
        (b.stats.faults, b.stats.errors, b.stats.total).cmp(&(a.stats.faults, a.stats.errors, a.stats.total))
    });
    let mut rows = Vec::new();
    for (i, e) in edges.iter().enumerate() {
        if i > 0 {
            rows.push((String::new(), String::new()));
        }
        rows.push((e.name.clone(), String::new()));
        rows.push(("  Type".to_string(), e.node_type.clone()));
        rows.push(("  Requests".to_string(), e.stats.total.to_string()));
        if e.stats.faults > 0 {
            rows.push(("  Faults (5xx)".to_string(), format!("✗ {}", xray_stats_value(&e.stats, e.stats.faults))));
        }
        if e.stats.errors > 0 {
            rows.push(("  Errors (4xx)".to_string(), format!("⚠ {}", xray_stats_value(&e.stats, e.stats.errors))));
        }
        if let Some(m) = e.stats.mean() {
            rows.push(("  Avg Latency".to_string(), fmt_secs(m)));
        }
    }
    rows
}

pub fn xray_node_section_lines(
    n: &crate::aws::services::xray::XRayNode,
    section: crate::aws::services::xray::XRayNodeDetailSection,
) -> Vec<(String, String)> {
    use crate::aws::services::xray::{fmt_secs, XRayNodeDetailSection as S};
    match section {
        S::Overview => {
            let mut rows = vec![
                ("Service".to_string(), n.name.clone()),
                ("Type".to_string(), n.node_type.clone()),
                ("Window".to_string(), format!("last {} ([ ] in the list to change)", n.window)),
            ];
            if let Some(target) = n.neboto_target() {
                rows.push(("Jump To".to_string(), target));
            }
            if let Some(a) = &n.account {
                rows.push(("Account".to_string(), a.clone()));
            }
            if let Some(st) = &n.state {
                rows.push(("State".to_string(), st.clone()));
            }
            if n.root {
                rows.push(("Entry Point".to_string(), "✓ receives client requests".to_string()));
            }
            if !n.aliases.is_empty() {
                rows.push(("Also Known As".to_string(), n.aliases.join(", ")));
            }
            match n.stats {
                None => rows.push((
                    "".to_string(),
                    "· no request statistics (a client or an inferred node)".to_string(),
                )),
                Some(s) => {
                    rows.push((String::new(), String::new()));
                    rows.push(("Requests".to_string(), String::new()));
                    rows.push(("  Total".to_string(), s.total.to_string()));
                    rows.push(("  OK".to_string(), xray_stats_value(&s, s.ok)));
                    rows.push((
                        "  Faults (5xx)".to_string(),
                        if s.faults > 0 {
                            format!("✗ {}", xray_stats_value(&s, s.faults))
                        } else {
                            "✓ 0".to_string()
                        },
                    ));
                    rows.push((
                        "  Errors (4xx)".to_string(),
                        if s.errors > 0 {
                            format!("⚠ {}", xray_stats_value(&s, s.errors))
                        } else {
                            "0".to_string()
                        },
                    ));
                    if s.throttles > 0 {
                        rows.push(("  Throttles (429)".to_string(), format!("⚠ {}", xray_stats_value(&s, s.throttles))));
                    }
                    rows.push((String::new(), String::new()));
                    rows.push(("Response Time".to_string(), String::new()));
                    if let Some(m) = s.mean() {
                        rows.push(("  Average".to_string(), fmt_secs(m)));
                    }
                    for (k, v) in [("  p50", n.p50), ("  p90", n.p90), ("  p99", n.p99)] {
                        if let Some(v) = v {
                            rows.push((k.to_string(), fmt_secs(v)));
                        }
                    }
                }
            }
            rows.push((String::new(), String::new()));
            rows.push((
                "Edges".to_string(),
                format!("{} downstream · {} upstream", n.downstream.len(), n.upstream.len()),
            ));
            rows
        }
        S::Downstream => xray_edge_rows(&n.downstream, "No downstream calls recorded in this window"),
        S::Upstream => xray_edge_rows(&n.upstream, "No callers recorded in this window"),
    }
}

pub fn xray_trace_section_lines(
    t: &crate::aws::services::xray::XRayTrace,
    section: crate::aws::services::xray::XRayTraceDetailSection,
    detail: Option<&crate::lazy::Lazy<crate::aws::services::xray::XRayTraceDetail>>,
) -> Vec<(String, String)> {
    use crate::aws::services::xray::{fmt_secs, XRayTraceDetailSection as S};
    use crate::lazy::Lazy;
    match section {
        S::Overview => {
            let mut rows = vec![
                ("Trace ID".to_string(), t.trace_id.clone()),
                (
                    "Outcome".to_string(),
                    match t.state_label().as_str() {
                        "fault" => "✗ fault (5xx)".to_string(),
                        "error" => "⚠ error (4xx)".to_string(),
                        "throttled" => "⚠ throttled (429)".to_string(),
                        other => format!("✓ {other}"),
                    },
                ),
            ];
            if let Some(s) = &t.start {
                rows.push(("Started".to_string(), s.clone()));
            }
            if let Some(d) = t.duration {
                rows.push(("Duration".to_string(), fmt_secs(d)));
            }
            if let Some(r) = t.response_time {
                rows.push(("Response Time".to_string(), fmt_secs(r)));
            }
            if t.partial {
                rows.push(("".to_string(), "⚠ partial trace — some segments haven't arrived".to_string()));
            }
            rows.push((String::new(), String::new()));
            rows.push(("Request".to_string(), String::new()));
            if let Some(m) = &t.method {
                rows.push(("  Method".to_string(), m.clone()));
            }
            if let Some(u) = &t.url {
                rows.push(("  URL".to_string(), u.clone()));
            }
            if let Some(s) = t.status {
                let v = if s >= 500 {
                    format!("✗ {s}")
                } else if s >= 400 {
                    format!("⚠ {s}")
                } else {
                    s.to_string()
                };
                rows.push(("  Status".to_string(), v));
            }
            if let Some(ip) = &t.client_ip {
                rows.push(("  Client IP".to_string(), ip.clone()));
            }
            if let Some(ua) = &t.user_agent {
                rows.push(("  User Agent".to_string(), ua.clone()));
            }
            if let Some(e) = &t.entry_point {
                rows.push(("  Entry Point".to_string(), e.clone()));
            }
            if !t.services.is_empty() {
                rows.push((String::new(), String::new()));
                rows.push(("Services".to_string(), String::new()));
                for s in &t.services {
                    rows.push((format!("  {s}"), String::new()));
                }
            }
            if !t.resource_arns.is_empty() {
                rows.push((String::new(), String::new()));
                rows.push(("Resources".to_string(), String::new()));
                for a in &t.resource_arns {
                    rows.push(("  Resource".to_string(), a.clone()));
                }
            }
            rows
        }
        S::RootCause => {
            if t.root_causes.is_empty() {
                return vec![(
                    "".to_string(),
                    if t.fault || t.error {
                        "X-Ray didn't attribute a root cause — see Segments".to_string()
                    } else {
                        "No faults, errors or latency outliers in this trace".to_string()
                    },
                )];
            }
            let mut rows = Vec::new();
            for (i, rc) in t.root_causes.iter().enumerate() {
                if i > 0 {
                    rows.push((String::new(), String::new()));
                }
                let mark = match rc.kind {
                    "fault" => "✗",
                    "error" => "⚠",
                    _ => "⏱",
                };
                rows.push((format!("{} {}", rc.kind, rc.service), String::new()));
                rows.push(("  Kind".to_string(), format!("{mark} {}", rc.kind)));
                if !rc.path.is_empty() {
                    rows.push(("  Path".to_string(), rc.path.clone()));
                }
                if let Some(x) = &rc.exception {
                    rows.push(("  Exception".to_string(), format!("{mark} {x}")));
                }
            }
            rows
        }
        S::Segments => match detail {
            None | Some(Lazy::Loading) => vec![("".to_string(), "Loading…".to_string())],
            Some(Lazy::Error(e)) => error_rows(e),
            Some(Lazy::Loaded(d)) if d.rows.is_empty() => {
                vec![("".to_string(), "No segments returned (the trace may have aged out — X-Ray keeps 30 days)".to_string())]
            }
            Some(Lazy::Loaded(d)) => {
                let mut rows = vec![("Segments".to_string(), format!("{} · e opens the raw trace JSON", d.rows.len()))];
                if d.limit_exceeded {
                    rows.push(("".to_string(), "⚠ trace exceeded X-Ray's size limit — segments are incomplete".to_string()));
                }
                rows.push((String::new(), String::new()));
                rows.push((
                    format!("    {:<48} {:>10} {:>10}  {}", "SEGMENT", "START", "DURATION", "STATUS"),
                    String::new(),
                ));
                for r in &d.rows {
                    let mark = if r.fault {
                        "✗ "
                    } else if r.error || r.throttle {
                        "⚠ "
                    } else {
                        "  "
                    };
                    let name = format!(
                        "{}{}{}",
                        "  ".repeat(r.depth),
                        r.name,
                        r.origin.as_deref().map(|o| format!(" ({o})")).unwrap_or_default()
                    );
                    let mut status = r.http_status.map(|s| s.to_string()).unwrap_or_default();
                    if let Some(x) = &r.exception {
                        if !status.is_empty() {
                            status.push_str("  ");
                        }
                        status.push_str(x);
                    }
                    rows.push((
                        format!(
                            "  {mark}{:<48} {:>10} {:>10}  {}",
                            truncate_chars_x(&name, 48),
                            format!("+{}", fmt_secs(r.offset)),
                            r.duration.map(fmt_secs).unwrap_or_else(|| "…".to_string()),
                            status
                        ),
                        String::new(),
                    ));
                }
                rows
            }
        },
    }
}

pub(super) fn truncate_chars_x(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}
