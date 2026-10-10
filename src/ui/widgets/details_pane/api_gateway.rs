use super::*;

// ── API Gateway split panes ────────────────────────────────────────────────────

pub(super) fn api_header_lines(name: &str, id: &str, badge: &str) -> Vec<Line<'static>> {
    vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                name.to_string(),
                Style::default()
                    .fg(crate::ui::theme::text_primary())
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(id.to_string(), Style::default().fg(theme::text_dim())),
            Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
            Span::styled(badge.to_string(), Style::default().fg(theme::accent())),
        ]),
        Line::raw(""),
    ]
}

pub(super) fn render_rest_api_split(app: &App, api: &RestApi, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 6, "");
    let mut block = theme::pane_block("REST API", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let badge = if api.endpoint_type.is_empty() {
        "REST".to_string()
    } else {
        api.endpoint_type.clone()
    };
    let header = api_header_lines(api.name(), &api.id, &badge);
    let header_h = header.len() as u16;
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(header_h),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(inner);
    frame.render_widget(Paragraph::new(header), chunks[0]);
    render_hr(chunks[1], frame);
    render_section_tab_bar(app,
        chunks[2],
        frame,
        &descriptor_tabs(app, &crate::aws::services::api_gateway::REST_API_SECTIONS),
    );
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub(super) fn render_http_api_split(app: &App, api: &HttpApi, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 5, "");
    let mut block = theme::pane_block("HTTP API", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let badge = if api.protocol.is_empty() {
        "HTTP".to_string()
    } else {
        api.protocol.clone()
    };
    let header = api_header_lines(api.name(), &api.id, &badge);
    let header_h = header.len() as u16;
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(header_h),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(inner);
    frame.render_widget(Paragraph::new(header), chunks[0]);
    render_hr(chunks[1], frame);
    render_section_tab_bar(app,
        chunks[2],
        frame,
        &descriptor_tabs(app, &crate::aws::services::api_gateway::HTTP_API_SECTIONS),
    );
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

/// Render stage rows. `base_url` is the API's invoke-URL base (REST:
/// `https://{id}.execute-api.{region}.amazonaws.com`; HTTP: the api endpoint);
/// `default_stage_is_root` is true for HTTP APIs, whose `$default` stage serves
/// from the bare endpoint with no `/stage` suffix.
pub(super) fn api_stage_rows(
    stages: &[crate::aws::services::api_gateway::ApiStage],
    base_url: &str,
    default_stage_is_root: bool,
) -> Vec<(String, String)> {
    let mut rows = vec![(format!("Stages ({})", stages.len()), String::new())];
    rows.push((String::new(), String::new()));
    if stages.is_empty() {
        rows.push(("  No stages".to_string(), String::new()));
        return rows;
    }
    for s in stages {
        rows.push((s.name.clone(), String::new())); // group header
        if !base_url.is_empty() {
            let invoke = if default_stage_is_root && s.name == "$default" {
                base_url.to_string()
            } else {
                format!("{}/{}", base_url, s.name)
            };
            rows.push(("  Invoke URL".to_string(), invoke));
        }
        if !s.deployment.is_empty() {
            rows.push(("  Deployment".to_string(), s.deployment.clone()));
        }
        if !s.info.is_empty() {
            rows.push(("  Config".to_string(), s.info.clone()));
        }
        if !s.throttle.is_empty() {
            rows.push(("  Throttle".to_string(), s.throttle.clone()));
        }
        if !s.canary.is_empty() {
            // An active canary split is easy to forget — surface it loudly.
            rows.push(("  ⚠ Canary".to_string(), s.canary.clone()));
        }
        if !s.access_log_destination.is_empty() {
            // ARN → jumpable (CloudWatch log group); also drives `t` tail.
            rows.push(("  Access Log".to_string(), s.access_log_destination.clone()));
            if !s.access_log_format.is_empty() {
                rows.push(("  Log Format".to_string(), s.access_log_format.clone()));
            }
        }
        if !s.waf_web_acl_arn.is_empty() {
            rows.push(("  WAF Web ACL".to_string(), s.waf_web_acl_arn.clone()));
        }
        if !s.variables.is_empty() {
            rows.push(("  Stage Variables".to_string(), String::new())); // sub-header
            for (k, v) in &s.variables {
                rows.push((format!("    {}", k), v.clone()));
            }
        }
        rows.push((String::new(), String::new()));
    }
    rows
}

pub(super) fn api_authorizer_rows(
    auths: &[crate::aws::services::api_gateway::ApiAuthorizer],
) -> Vec<(String, String)> {
    let mut rows = vec![(format!("Authorizers ({})", auths.len()), String::new())];
    rows.push((String::new(), String::new()));
    if auths.is_empty() {
        rows.push(("  No authorizers".to_string(), String::new()));
        return rows;
    }
    for a in auths {
        rows.push((a.name.clone(), String::new())); // group header
        rows.push(("Type".to_string(), a.kind.clone()));
        if !a.identity_source.is_empty() {
            rows.push(("Identity Source".to_string(), a.identity_source.clone()));
        }
        rows.push((String::new(), String::new()));
    }
    rows
}

pub fn rest_api_section_lines(
    api: &RestApi,
    section: RestApiDetailSection,
    details: Option<&crate::lazy::Lazy<crate::aws::services::api_gateway::RestApiDetails>>,
    region: &str,
) -> Vec<(String, String)> {
    if section == RestApiDetailSection::Tags {
        return tag_rows(&api.tags);
    }
    if section == RestApiDetailSection::Overview {
        return rest_api_overview_rows(api);
    }
    let loaded = match details {
        None | Some(crate::lazy::Lazy::Loading) => {
            return vec![("".to_string(), "Loading…".to_string())];
        }
        Some(crate::lazy::Lazy::Error(e)) => return error_rows(e),
        Some(crate::lazy::Lazy::Loaded(d)) => d,
    };
    match section {
        RestApiDetailSection::Resources => {
            let mut rows = vec![(format!("Resources ({})", loaded.resources.len()), String::new())];
            rows.push((String::new(), String::new()));
            if loaded.resources.is_empty() {
                rows.push(("  No resources".to_string(), String::new()));
            } else {
                for r in &loaded.resources {
                    if r.methods.is_empty() {
                        // A structural resource with no methods of its own.
                        rows.push((r.path.clone(), String::new())); // group header
                        rows.push(("  (no methods)".to_string(), String::new()));
                        rows.push((String::new(), String::new()));
                        continue;
                    }
                    rows.push((r.path.clone(), String::new())); // group header
                    for m in &r.methods {
                        // method → backend; backend value is jump-ready (Lambda
                        // ARN jumps to Lambda). Falls back to "—" when unresolved.
                        let backend = if m.backend.is_empty() {
                            "—".to_string()
                        } else {
                            m.backend.clone()
                        };
                        rows.push((format!("  {}", m.http_method), backend));
                    }
                    rows.push((String::new(), String::new()));
                }
            }
            rows
        }
        RestApiDetailSection::Stages => {
            let base = format!("https://{}.execute-api.{}.amazonaws.com", api.id, region);
            api_stage_rows(&loaded.stages, &base, false)
        }
        RestApiDetailSection::Authorizers => api_authorizer_rows(&loaded.authorizers),
        RestApiDetailSection::Deployments => {
            let mut rows = vec![(
                if loaded.deployments_total > loaded.deployments.len() {
                    format!(
                        "Deployments (latest {} of {})",
                        loaded.deployments.len(),
                        loaded.deployments_total
                    )
                } else {
                    format!("Deployments ({})", loaded.deployments.len())
                },
                String::new(),
            )];
            rows.push((String::new(), String::new()));
            if loaded.deployments.is_empty() {
                rows.push(("  No deployments".to_string(), String::new()));
            } else {
                for d in &loaded.deployments {
                    let label = if d.description.is_empty() {
                        d.id.clone()
                    } else {
                        format!("{} — {}", d.id, d.description)
                    };
                    rows.push((format!("  {}", d.created), label));
                }
            }
            rows.push((String::new(), String::new()));
            rows
        }
        RestApiDetailSection::Overview | RestApiDetailSection::Tags => unreachable!(),
    }
}

/// Eager API-level config — everything GetRestApis returns inline.
pub(super) fn rest_api_overview_rows(api: &RestApi) -> Vec<(String, String)> {
    let mut rows = vec![
        ("ID".to_string(), api.id.clone()),
        ("Name".to_string(), api.name.clone()),
    ];
    if !api.description.is_empty() {
        rows.push(("Description".to_string(), api.description.clone()));
    }
    rows.push(("Endpoint Type".to_string(), api.endpoint_type.clone()));
    if let Some(created) = &api.created {
        rows.push(("Created".to_string(), created.clone()));
    }

    rows.push((String::new(), String::new()));
    // Security posture: a disabled default endpoint means callers must come
    // through the custom domain (and its WAF/policy), not the raw execute-api
    // hostname.
    rows.push((
        "Default Endpoint".to_string(),
        if api.default_endpoint_disabled {
            "✓ disabled (custom domain only)".to_string()
        } else {
            "enabled (execute-api reachable)".to_string()
        },
    ));
    rows.push((
        "Resource Policy".to_string(),
        if api.policy.is_some() {
            "✓ set (e to view)".to_string()
        } else {
            "none".to_string()
        },
    ));
    if !api.api_key_source.is_empty() {
        rows.push(("API Key Source".to_string(), api.api_key_source.clone()));
    }

    rows.push((String::new(), String::new()));
    rows.push((
        "Compression".to_string(),
        match api.minimum_compression_size {
            Some(min) => format!("✓ ≥ {} bytes", min),
            None => "disabled".to_string(),
        },
    ));
    if !api.binary_media_types.is_empty() {
        rows.push((
            "Binary Media Types".to_string(),
            api.binary_media_types.join(", "),
        ));
    }
    rows
}

pub fn http_api_section_lines(
    api: &HttpApi,
    section: HttpApiDetailSection,
    details: Option<&crate::lazy::Lazy<crate::aws::services::api_gateway::HttpApiDetails>>,
) -> Vec<(String, String)> {
    if section == HttpApiDetailSection::Tags {
        return tag_rows(&api.tags);
    }
    if section == HttpApiDetailSection::Overview {
        return http_api_overview_rows(api);
    }
    let loaded = match details {
        None | Some(crate::lazy::Lazy::Loading) => {
            return vec![("".to_string(), "Loading…".to_string())];
        }
        Some(crate::lazy::Lazy::Error(e)) => return error_rows(e),
        Some(crate::lazy::Lazy::Loaded(d)) => d,
    };
    match section {
        HttpApiDetailSection::Routes => {
            let mut rows = vec![(format!("Routes ({})", loaded.routes.len()), String::new())];
            rows.push((String::new(), String::new()));
            if loaded.routes.is_empty() {
                rows.push(("  No routes".to_string(), String::new()));
            } else {
                for r in &loaded.routes {
                    rows.push((r.route_key.clone(), String::new())); // group header
                    // Backend value is jump-ready (a Lambda ARN jumps to Lambda).
                    let backend = if r.backend.is_empty() {
                        r.target.rsplit('/').next().unwrap_or(&r.target).to_string()
                    } else {
                        r.backend.clone()
                    };
                    rows.push(("  Backend".to_string(), backend));
                    let auth = if r.authorization.is_empty() {
                        "NONE"
                    } else {
                        r.authorization.as_str()
                    };
                    if auth != "NONE" {
                        rows.push(("  Auth".to_string(), auth.to_string()));
                    }
                    rows.push((String::new(), String::new()));
                }
            }
            rows
        }
        HttpApiDetailSection::Stages => api_stage_rows(&loaded.stages, &api.endpoint, true),
        HttpApiDetailSection::Authorizers => api_authorizer_rows(&loaded.authorizers),
        HttpApiDetailSection::Overview | HttpApiDetailSection::Tags => unreachable!(),
    }
}

/// Eager API-level config — everything GetApis returns inline (incl. CORS).
pub(super) fn http_api_overview_rows(api: &HttpApi) -> Vec<(String, String)> {
    let mut rows = vec![
        ("ID".to_string(), api.id.clone()),
        ("Name".to_string(), api.name.clone()),
        ("Protocol".to_string(), api.protocol.clone()),
    ];
    if !api.description.is_empty() {
        rows.push(("Description".to_string(), api.description.clone()));
    }
    if !api.endpoint.is_empty() {
        rows.push(("Endpoint".to_string(), api.endpoint.clone()));
    }
    if let Some(created) = &api.created {
        rows.push(("Created".to_string(), created.clone()));
    }
    if !api.route_selection_expression.is_empty() && api.protocol == "WEBSOCKET" {
        rows.push((
            "Route Selection".to_string(),
            api.route_selection_expression.clone(),
        ));
    }

    rows.push((String::new(), String::new()));
    rows.push((
        "Default Endpoint".to_string(),
        if api.default_endpoint_disabled {
            "✓ disabled (custom domain only)".to_string()
        } else {
            "enabled (execute-api reachable)".to_string()
        },
    ));

    rows.push((String::new(), String::new()));
    match &api.cors {
        None => rows.push(("CORS".to_string(), "not configured".to_string())),
        Some(c) => {
            rows.push(("CORS".to_string(), String::new()));
            if !c.allow_origins.is_empty() {
                let wildcard = c.allow_origins.iter().any(|o| o == "*");
                rows.push((
                    "  Allow Origins".to_string(),
                    if wildcard && c.allow_origins.len() == 1 {
                        "⚠ * (any origin)".to_string()
                    } else {
                        c.allow_origins.join(", ")
                    },
                ));
            }
            if !c.allow_methods.is_empty() {
                rows.push(("  Allow Methods".to_string(), c.allow_methods.join(", ")));
            }
            if !c.allow_headers.is_empty() {
                rows.push(("  Allow Headers".to_string(), c.allow_headers.join(", ")));
            }
            if !c.expose_headers.is_empty() {
                rows.push(("  Expose Headers".to_string(), c.expose_headers.join(", ")));
            }
            rows.push((
                "  Credentials".to_string(),
                if c.allow_credentials { "allowed" } else { "not allowed" }.to_string(),
            ));
            if let Some(age) = c.max_age {
                rows.push(("  Max Age".to_string(), format!("{}s", age)));
            }
        }
    }
    rows
}

pub(super) fn render_api_domain_split(app: &App, domain: &ApiCustomDomain, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 3, "");
    let mut block = theme::pane_block("API Custom Domain", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let status_color = match domain.status.as_str() {
        "AVAILABLE" => theme::success(),
        "UPDATING" | "PENDING" => theme::warning(),
        _ => theme::text_dim(),
    };
    let header = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                domain.domain_name.clone(),
                Style::default()
                    .fg(crate::ui::theme::text_primary())
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(domain.serves.clone(), Style::default().fg(theme::accent())),
            Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
            Span::styled(domain.status.clone(), Style::default().fg(status_color)),
        ]),
        Line::raw(""),
    ];
    let header_h = header.len() as u16;
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(header_h),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(inner);
    frame.render_widget(Paragraph::new(header), chunks[0]);
    render_hr(chunks[1], frame);
    render_section_tab_bar(app, 
        chunks[2],
        frame,
        &descriptor_tabs(app, &crate::aws::services::api_gateway::API_DOMAIN_SECTIONS),
    );
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub fn api_domain_section_lines(
    domain: &ApiCustomDomain,
    section: ApiDomainDetailSection,
    details: Option<&crate::lazy::Lazy<Vec<crate::aws::services::api_gateway::DomainMapping>>>,
) -> Vec<(String, String)> {
    match section {
        ApiDomainDetailSection::Details => {
            let mut rows = vec![
                ("Domain".to_string(), domain.domain_name.clone()),
                ("Endpoint Type".to_string(), domain.endpoint_type.clone()),
                ("Status".to_string(), domain.status.clone()),
                ("Serves".to_string(), domain.serves.clone()),
                ("Security Policy".to_string(), domain.security_policy.clone()),
                (String::new(), String::new()),
                ("Target".to_string(), domain.target.clone()),
            ];
            if !domain.certificate_arn.is_empty() {
                rows.push(("Certificate".to_string(), domain.certificate_arn.clone()));
            }
            if let Some(truststore) = &domain.mtls_truststore {
                rows.push((String::new(), String::new()));
                // s3:// value → jumpable to the truststore's bucket.
                rows.push(("Mutual TLS".to_string(), truststore.clone()));
            }
            rows
        }
        ApiDomainDetailSection::Mappings => match details {
            None | Some(crate::lazy::Lazy::Loading) => {
                vec![("".to_string(), "Loading mappings…".to_string())]
            }
            Some(crate::lazy::Lazy::Error(e)) => error_rows(e),
            Some(crate::lazy::Lazy::Loaded(maps)) => {
                let mut rows = vec![(format!("Mappings ({})", maps.len()), String::new())];
                rows.push((String::new(), String::new()));
                if maps.is_empty() {
                    rows.push(("  No mappings".to_string(), String::new()));
                } else {
                    for m in maps {
                        rows.push((m.path.clone(), String::new())); // group header
                        // "API (REST|HTTP)" keys drive apigw_row_jump_target —
                        // Enter on the row jumps to that API's sub-tab.
                        rows.push((format!("  API ({})", m.kind), m.api_id.clone()));
                        rows.push(("  Stage".to_string(), m.stage.clone()));
                        rows.push((String::new(), String::new()));
                    }
                }
                rows
            }
        },
        ApiDomainDetailSection::Tags => tag_rows(&domain.tags),
    }
}

// ── API Usage Plan split pane ─────────────────────────────────────────────────

pub(super) fn render_api_usage_plan_split(app: &App, plan: &ApiUsagePlan, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 3, "");
    let mut block = theme::pane_block("API Usage Plan", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Header: name + id + the throttle/quota envelope at a glance.
    let envelope = {
        let mut parts = Vec::new();
        if let Some(rate) = plan.throttle_rate {
            parts.push(format!("{:.0} req/s", rate));
        }
        if let Some((limit, period)) = &plan.quota {
            parts.push(format!("{} / {}", limit, period));
        }
        if parts.is_empty() {
            "no limits".to_string()
        } else {
            parts.join("  ·  ")
        }
    };
    let header = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                plan.name.clone(),
                Style::default()
                    .fg(crate::ui::theme::text_primary())
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(plan.id.clone(), Style::default().fg(theme::text_dim())),
            Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
            Span::styled(envelope, Style::default().fg(theme::accent())),
        ]),
        Line::raw(""),
    ];
    let header_h = header.len() as u16;
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(header_h),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(inner);
    frame.render_widget(Paragraph::new(header), chunks[0]);
    render_hr(chunks[1], frame);
    render_section_tab_bar(app,
        chunks[2],
        frame,
        &descriptor_tabs(app, &crate::aws::services::api_gateway::API_USAGE_PLAN_SECTIONS),
    );
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub fn api_usage_plan_section_lines(
    plan: &ApiUsagePlan,
    section: ApiUsagePlanDetailSection,
    keys_state: Option<&Lazy<Vec<UsagePlanKeyInfo>>>,
) -> Vec<(String, String)> {
    match section {
        ApiUsagePlanDetailSection::Overview => {
            let mut rows = vec![
                ("ID".to_string(), plan.id.clone()),
                ("Name".to_string(), plan.name.clone()),
            ];
            if !plan.description.is_empty() {
                rows.push(("Description".to_string(), plan.description.clone()));
            }
            rows.push((String::new(), String::new()));
            rows.push(("Throttle".to_string(), String::new()));
            match (plan.throttle_rate, plan.throttle_burst) {
                (None, None) => {
                    rows.push(("  Rate".to_string(), "unlimited".to_string()));
                }
                (rate, burst) => {
                    if let Some(r) = rate {
                        rows.push(("  Rate".to_string(), format!("{:.0} req/s", r)));
                    }
                    if let Some(b) = burst {
                        rows.push(("  Burst".to_string(), b.to_string()));
                    }
                }
            }
            rows.push((String::new(), String::new()));
            rows.push((
                "Quota".to_string(),
                match &plan.quota {
                    Some((limit, period)) => format!("{} requests / {}", limit, period),
                    None => "none".to_string(),
                },
            ));
            rows
        }
        ApiUsagePlanDetailSection::Stages => {
            let mut rows = vec![(
                format!("API Stages ({})", plan.api_stages.len()),
                String::new(),
            )];
            rows.push((String::new(), String::new()));
            if plan.api_stages.is_empty() {
                rows.push((
                    "  No API stages — this plan limits nothing".to_string(),
                    String::new(),
                ));
            } else {
                for (api_id, stage) in &plan.api_stages {
                    rows.push((format!("  {}", stage), api_id.clone()));
                }
            }
            rows.push((String::new(), String::new()));
            rows
        }
        ApiUsagePlanDetailSection::Keys => {
            let mut rows = vec![(String::new(), String::new())];
            match keys_state {
                None | Some(Lazy::Loading) => {
                    rows.push(("  Loading keys…".to_string(), String::new()));
                }
                Some(Lazy::Error(e)) => return error_rows(e),
                Some(Lazy::Loaded(keys)) => {
                    if keys.is_empty() {
                        rows.push((
                            "  No API keys attached to this plan".to_string(),
                            String::new(),
                        ));
                    } else {
                        rows.push((format!("Keys ({})", keys.len()), String::new()));
                        rows.push((String::new(), String::new()));
                        // Key *values* are deliberately never fetched or shown.
                        for k in keys {
                            let name = if k.name.is_empty() { &k.id } else { &k.name };
                            rows.push((format!("  {}", name), k.id.clone()));
                        }
                    }
                }
            }
            rows.push((String::new(), String::new()));
            rows
        }
    }
}
