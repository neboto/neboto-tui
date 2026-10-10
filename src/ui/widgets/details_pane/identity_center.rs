use super::*;

// ── Identity Center permission set split pane ────────────────────────────────

pub(super) fn render_permission_set_split(
    app: &App,
    ps: &crate::aws::services::identity_center::PermissionSet,
    area: Rect,
    frame: &mut Frame,
) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 4, "");
    let mut block = theme::pane_block("Permission Set", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let header = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                ps.name.clone(),
                Style::default().fg(crate::ui::theme::text_primary()).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                if ps.description.is_empty() {
                    "(no description)".to_string()
                } else {
                    ps.description.clone()
                },
                Style::default().fg(theme::text_dim()),
            ),
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
        &descriptor_tabs(app, &crate::aws::services::identity_center::PERMISSION_SET_SECTIONS),
    );
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

/// `123456789012 (prod)` when the Organizations name map has the account,
/// else the bare id.
pub(super) fn ic_account_label(id: &str, names: &std::collections::HashMap<String, String>) -> String {
    match names.get(id) {
        Some(n) => format!("{} ({})", id, n),
        None => id.to_string(),
    }
}

pub fn permission_set_section_lines(
    ps: &crate::aws::services::identity_center::PermissionSet,
    section: PermissionSetDetailSection,
    access: Option<&Lazy<Box<crate::aws::services::identity_center::PsAccess>>>,
    assignments: Option<&Lazy<Vec<crate::aws::services::identity_center::PsAssignment>>>,
    account_names: &std::collections::HashMap<String, String>,
) -> Vec<(String, String)> {
    match section {
        PermissionSetDetailSection::Details => {
            let mut rows = vec![("Name".to_string(), ps.name.clone())];
            if !ps.description.is_empty() {
                rows.push(("Description".to_string(), ps.description.clone()));
            }
            if let Some(d) = &ps.session_duration {
                rows.push(("Session Duration".to_string(), d.clone()));
            }
            if let Some(r) = &ps.relay_state {
                rows.push(("Relay State".to_string(), r.clone()));
            }
            rows.push((String::new(), String::new()));
            rows.push(("ARN".to_string(), ps.arn.clone()));
            rows
        }
        PermissionSetDetailSection::Policies => {
            let mut rows = vec![("".to_string(), "".to_string())];
            match access {
                None | Some(Lazy::Loading) => {
                    rows.push(("  Loading…".to_string(), "".to_string()));
                }
                Some(Lazy::Error(e)) => {
                    rows.extend(error_rows(e));
                }
                Some(Lazy::Loaded(a)) => {
                    rows.push((format!("AWS Managed ({})", a.managed.len()), "".to_string()));
                    if a.managed.is_empty() {
                        rows.push(("  none".to_string(), "".to_string()));
                    }
                    for (name, arn) in &a.managed {
                        // ARN value → jump to the IAM policy via resource_jump_target.
                        rows.push((format!("  {}", name), arn.clone()));
                    }

                    rows.push((String::new(), String::new()));
                    rows.push((
                        format!("Customer Managed ({})", a.customer_managed.len()),
                        "".to_string(),
                    ));
                    if a.customer_managed.is_empty() {
                        rows.push(("  none".to_string(), "".to_string()));
                    }
                    for (name, path) in &a.customer_managed {
                        rows.push((format!("  {}", name), path.clone()));
                    }

                    rows.push((String::new(), String::new()));
                    rows.push((
                        "Permissions Boundary".to_string(),
                        a.boundary.clone().unwrap_or_else(|| "none".to_string()),
                    ));

                    rows.push((String::new(), String::new()));
                    rows.push(("Inline Policy".to_string(), "".to_string()));
                    match &a.inline_policy {
                        Some(json) => {
                            for line in json.lines() {
                                rows.push((format!("  {}", line), "".to_string()));
                            }
                        }
                        None => rows.push(("  none".to_string(), "".to_string())),
                    }
                }
            }
            rows
        }
        PermissionSetDetailSection::Assignments => {
            let mut rows = vec![("".to_string(), "".to_string())];
            match assignments {
                None | Some(Lazy::Loading) => {
                    rows.push(("  Loading…".to_string(), "".to_string()));
                }
                Some(Lazy::Error(e)) => {
                    rows.extend(error_rows(e));
                }
                Some(Lazy::Loaded(list)) if list.is_empty() => {
                    rows.push((
                        "  Not assigned to any account.".to_string(),
                        "".to_string(),
                    ));
                }
                Some(Lazy::Loaded(list)) => {
                    // Group by account.
                    let mut current = String::new();
                    for a in list {
                        if a.account_id != current {
                            if !current.is_empty() {
                                rows.push((String::new(), String::new()));
                            }
                            // Account id value → jump to the account (Organizations).
                            rows.push((
                                "Account".to_string(),
                                ic_account_label(&a.account_id, account_names),
                            ));
                            current = a.account_id.clone();
                        }
                        rows.push((
                            format!("  {}", a.principal_type),
                            a.principal_name.clone(),
                        ));
                    }
                }
            }
            rows
        }
        PermissionSetDetailSection::Tags => match access {
            None | Some(Lazy::Loading) => {
                vec![("  Loading…".to_string(), "".to_string())]
            }
            Some(Lazy::Error(e)) => error_rows(e),
            Some(Lazy::Loaded(a)) => bundle_tag_rows(&a.tags, a.tags_error.as_deref()),
        },
    }
}

// ── Identity Center group split pane ─────────────────────────────────────────

pub(super) fn render_ic_group_split(
    app: &App,
    g: &crate::aws::services::identity_center::IcGroup,
    area: Rect,
    frame: &mut Frame,
) {
    render_ic_principal_split(
        app,
        "IC Group",
        &g.display_name,
        &g.description,
        &descriptor_tabs(app, &crate::aws::services::identity_center::IC_GROUP_SECTIONS),
        area,
        frame,
    );
}

pub(super) fn render_ic_user_split(
    app: &App,
    u: &crate::aws::services::identity_center::IcUser,
    area: Rect,
    frame: &mut Frame,
) {
    let subtitle = u.email.clone().unwrap_or_else(|| u.username.clone());
    render_ic_principal_split(
        app,
        "IC User",
        u.name(),
        &subtitle,
        &descriptor_tabs(app, &crate::aws::services::identity_center::IC_USER_SECTIONS),
        area,
        frame,
    );
}

/// Shared split-pane chrome for IC users & groups (and the instance/app panes).
#[allow(clippy::too_many_arguments)]
pub(super) fn render_ic_principal_split(
    app: &App,
    title: &str,
    name: &str,
    subtitle: &str,
    tabs: &[(char, &str, bool)],
    area: Rect,
    frame: &mut Frame,
) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, tabs.len(), "");
    let mut block = theme::pane_block(title, focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let header = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                name.to_string(),
                Style::default().fg(crate::ui::theme::text_primary()).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(subtitle.to_string(), Style::default().fg(theme::text_dim())),
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
    render_section_tab_bar(app, chunks[2], frame, tabs);
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub(super) fn render_ic_instance_split(
    app: &App,
    inst: &crate::aws::services::identity_center::IcInstance,
    area: Rect,
    frame: &mut Frame,
) {
    let subtitle = format!("{} · {}", inst.identity_store_id, inst.status);
    render_ic_principal_split(
        app,
        "IC Instance",
        inst.name(),
        &subtitle,
        &descriptor_tabs(app, &crate::aws::services::identity_center::IC_INSTANCE_SECTIONS),
        area,
        frame,
    );
}

pub(super) fn render_ic_application_split(
    app: &App,
    a: &crate::aws::services::identity_center::IcApplication,
    area: Rect,
    frame: &mut Frame,
) {
    let subtitle = format!("{} · {}", a.provider_name(), a.status);
    render_ic_principal_split(
        app,
        "IC Application",
        &a.name,
        &subtitle,
        &descriptor_tabs(app, &crate::aws::services::identity_center::IC_APPLICATION_SECTIONS),
        area,
        frame,
    );
}

/// Provisioning rows shared by IC users and groups: external IDs reveal SCIM
/// provisioning from an external IdP — the closest public-API signal for the
/// configured identity source.
pub(super) fn ic_provisioning_rows(
    rows: &mut Vec<(String, String)>,
    external_ids: &[(String, String)],
    created_at: Option<&str>,
    created_by: Option<&str>,
    updated_at: Option<&str>,
    updated_by: Option<&str>,
) {
    rows.push((String::new(), String::new()));
    rows.push(("Provisioning".to_string(), String::new()));
    if external_ids.is_empty() {
        rows.push((
            "Source".to_string(),
            "Identity Center directory (no external IDs)".to_string(),
        ));
    } else {
        rows.push(("Source".to_string(), "External IdP (SCIM)".to_string()));
        for (issuer, id) in external_ids {
            rows.push((format!("  {}", issuer), id.clone()));
        }
    }
    if created_at.is_some() || updated_at.is_some() {
        rows.push((String::new(), String::new()));
        if let Some(c) = created_at {
            rows.push(("Created".to_string(), c.to_string()));
        }
        if let Some(c) = created_by {
            rows.push(("Created By".to_string(), c.to_string()));
        }
        if let Some(u) = updated_at {
            rows.push(("Updated".to_string(), u.to_string()));
        }
        if let Some(u) = updated_by {
            rows.push(("Updated By".to_string(), u.to_string()));
        }
    }
}

/// Shared Access section body for IC users and groups: every (account,
/// permission set) the principal can reach — direct assignments first, then
/// per contributing group. Permission-set rows carry the PS ARN so `Enter`
/// jumps to the permission set; Account rows jump to Organizations.
pub(super) fn ic_access_lines(
    state: Option<&Lazy<Vec<crate::aws::services::identity_center::IcAccessEntry>>>,
    account_names: &std::collections::HashMap<String, String>,
    is_group: bool,
) -> Vec<(String, String)> {
    let mut rows = vec![("".to_string(), "".to_string())];
    match state {
        None | Some(Lazy::Loading) => {
            rows.push(("  Loading…".to_string(), "".to_string()));
        }
        Some(Lazy::Error(e)) => {
            rows.extend(error_rows(e));
        }
        Some(Lazy::Loaded(list)) if list.is_empty() => {
            rows.push((
                "  No account access assigned.".to_string(),
                "".to_string(),
            ));
        }
        Some(Lazy::Loaded(list)) => {
            let mut current_via: Option<&Option<String>> = None;
            let mut current_account = String::new();
            for e in list {
                if current_via != Some(&e.via_group) {
                    if current_via.is_some() {
                        rows.push((String::new(), String::new()));
                    }
                    let count = list.iter().filter(|x| x.via_group == e.via_group).count();
                    let header = match &e.via_group {
                        None if is_group => format!("Assignments ({})", count),
                        None => format!("Direct assignments ({})", count),
                        Some(g) => format!("Via group {} ({})", g, count),
                    };
                    rows.push((header, String::new()));
                    current_via = Some(&e.via_group);
                    current_account.clear();
                }
                if e.account_id != current_account {
                    rows.push((
                        "Account".to_string(),
                        ic_account_label(&e.account_id, account_names),
                    ));
                    current_account = e.account_id.clone();
                }
                // Value is the PS ARN → Enter jumps to the permission set.
                rows.push((format!("  {}", e.ps_name), e.ps_arn.clone()));
            }
        }
    }
    rows
}

pub fn ic_group_section_lines(
    g: &crate::aws::services::identity_center::IcGroup,
    section: IcGroupDetailSection,
    members: Option<&Lazy<Vec<crate::aws::services::identity_center::IcMember>>>,
    access: Option<&Lazy<Vec<crate::aws::services::identity_center::IcAccessEntry>>>,
    account_names: &std::collections::HashMap<String, String>,
) -> Vec<(String, String)> {
    match section {
        IcGroupDetailSection::Details => {
            let mut rows = vec![("Display Name".to_string(), g.display_name.clone())];
            if !g.description.is_empty() {
                rows.push(("Description".to_string(), g.description.clone()));
            }
            rows.push(("Group ID".to_string(), g.group_id.clone()));
            ic_provisioning_rows(
                &mut rows,
                &g.external_ids,
                g.created_at.as_deref(),
                g.created_by.as_deref(),
                g.updated_at.as_deref(),
                g.updated_by.as_deref(),
            );
            rows
        }
        IcGroupDetailSection::Members => {
            let mut rows = vec![("".to_string(), "".to_string())];
            match members {
                None | Some(Lazy::Loading) => {
                    rows.push(("  Loading…".to_string(), "".to_string()));
                }
                Some(Lazy::Error(e)) => {
                    rows.extend(error_rows(e));
                }
                Some(Lazy::Loaded(list)) if list.is_empty() => {
                    rows.push(("  (no members)".to_string(), "".to_string()));
                }
                Some(Lazy::Loaded(list)) => {
                    rows.push((format!("Members ({})", list.len()), "".to_string()));
                    rows.push(("".to_string(), "".to_string()));
                    for m in list {
                        rows.push((format!("  {}", m.name), m.user_id.clone()));
                    }
                }
            }
            rows
        }
        IcGroupDetailSection::Access => ic_access_lines(access, account_names, true),
    }
}

pub fn ic_user_section_lines(
    u: &crate::aws::services::identity_center::IcUser,
    section: IcUserDetailSection,
    groups: Option<&Lazy<Vec<crate::aws::services::identity_center::IcGroupRef>>>,
    access: Option<&Lazy<Vec<crate::aws::services::identity_center::IcAccessEntry>>>,
    account_names: &std::collections::HashMap<String, String>,
) -> Vec<(String, String)> {
    match section {
        IcUserDetailSection::Details => {
            let mut rows = vec![("Username".to_string(), u.username.clone())];
            if !u.display_name.is_empty() {
                rows.push(("Display Name".to_string(), u.display_name.clone()));
            }
            if let Some(email) = &u.email {
                rows.push(("Email".to_string(), email.clone()));
            }
            if let Some(status) = &u.status {
                let v = if status == "DISABLED" {
                    "✗ DISABLED".to_string()
                } else {
                    status.clone()
                };
                rows.push(("Status".to_string(), v));
            }
            match (&u.given_name, &u.family_name) {
                (Some(g), Some(f)) => rows.push(("Name".to_string(), format!("{} {}", g, f))),
                (Some(g), None) => rows.push(("Name".to_string(), g.clone())),
                (None, Some(f)) => rows.push(("Name".to_string(), f.clone())),
                (None, None) => {}
            }
            if let Some(title) = &u.title {
                rows.push(("Title".to_string(), title.clone()));
            }
            if let Some(t) = &u.user_type {
                rows.push(("User Type".to_string(), t.clone()));
            }
            rows.push(("User ID".to_string(), u.user_id.clone()));
            ic_provisioning_rows(
                &mut rows,
                &u.external_ids,
                u.created_at.as_deref(),
                u.created_by.as_deref(),
                u.updated_at.as_deref(),
                u.updated_by.as_deref(),
            );
            rows
        }
        IcUserDetailSection::Groups => {
            let mut rows = vec![("".to_string(), "".to_string())];
            match groups {
                None | Some(Lazy::Loading) => {
                    rows.push(("  Loading…".to_string(), "".to_string()));
                }
                Some(Lazy::Error(e)) => {
                    rows.extend(error_rows(e));
                }
                Some(Lazy::Loaded(list)) if list.is_empty() => {
                    rows.push(("  (no group memberships)".to_string(), "".to_string()));
                }
                Some(Lazy::Loaded(list)) => {
                    rows.push((format!("Groups ({})", list.len()), "".to_string()));
                    rows.push(("".to_string(), "".to_string()));
                    for grp in list {
                        rows.push((format!("  {}", grp.name), grp.group_id.clone()));
                    }
                }
            }
            rows
        }
        IcUserDetailSection::Access => ic_access_lines(access, account_names, false),
    }
}

pub fn ic_instance_section_lines(
    inst: &crate::aws::services::identity_center::IcInstance,
    section: IcInstanceDetailSection,
    abac: Option<&Lazy<Option<crate::aws::services::identity_center::IcAbac>>>,
    tti: Option<&Lazy<Vec<crate::aws::services::identity_center::IcTti>>>,
    counts: (usize, usize, usize, usize),
) -> Vec<(String, String)> {
    match section {
        IcInstanceDetailSection::Overview => {
            let mut rows = Vec::new();
            if !inst.name.is_empty() {
                rows.push(("Name".to_string(), inst.name.clone()));
            }
            rows.push(("Status".to_string(), inst.status.clone()));
            if let Some(r) = &inst.status_reason {
                rows.push(("Status Reason".to_string(), r.clone()));
            }
            rows.push(("Instance ARN".to_string(), inst.instance_arn.clone()));
            rows.push((
                "Identity Store ID".to_string(),
                inst.identity_store_id.clone(),
            ));
            if !inst.owner_account_id.is_empty() {
                rows.push(("Owner Account".to_string(), inst.owner_account_id.clone()));
            }
            if let Some(c) = &inst.created_date {
                rows.push(("Created".to_string(), c.clone()));
            }
            rows.push((String::new(), String::new()));
            rows.push(("Encryption".to_string(), String::new()));
            match &inst.encryption_key_type {
                Some(t) => {
                    rows.push(("Key Type".to_string(), t.clone()));
                    if let Some(arn) = &inst.encryption_kms_key_arn {
                        // KMS key ARN → jumpable.
                        rows.push(("KMS Key".to_string(), arn.clone()));
                    }
                    if let Some(s) = &inst.encryption_status {
                        rows.push(("Encryption Status".to_string(), s.clone()));
                    }
                    if let Some(r) = &inst.encryption_status_reason {
                        rows.push(("Reason".to_string(), r.clone()));
                    }
                }
                None => rows.push((
                    "Key Type".to_string(),
                    "AWS owned key (default)".to_string(),
                )),
            }
            rows.push((String::new(), String::new()));
            rows.push((
                "  MFA policy, session duration, and the identity source have no public read API — view them in the console."
                    .to_string(),
                String::new(),
            ));
            rows
        }
        IcInstanceDetailSection::Abac => {
            let mut rows = vec![("".to_string(), "".to_string())];
            match abac {
                None | Some(Lazy::Loading) => {
                    rows.push(("  Loading…".to_string(), "".to_string()));
                }
                Some(Lazy::Error(e)) => {
                    rows.extend(error_rows(e));
                }
                Some(Lazy::Loaded(None)) => {
                    rows.push((
                        "  Attribute-based access control is not configured.".to_string(),
                        "".to_string(),
                    ));
                    rows.push((String::new(), String::new()));
                    rows.push((
                        "  ABAC maps identity-source attributes onto sessions so permission-set policies can filter on ${aws:PrincipalTag/...}."
                            .to_string(),
                        String::new(),
                    ));
                }
                Some(Lazy::Loaded(Some(a))) => {
                    rows.push(("Status".to_string(), a.status.clone()));
                    if let Some(r) = &a.status_reason {
                        rows.push(("Status Reason".to_string(), r.clone()));
                    }
                    rows.push((String::new(), String::new()));
                    rows.push((
                        format!("Access Control Attributes ({})", a.attributes.len()),
                        String::new(),
                    ));
                    if a.attributes.is_empty() {
                        rows.push(("  none".to_string(), "".to_string()));
                    }
                    for (key, sources) in &a.attributes {
                        rows.push((format!("  {}", key), sources.clone()));
                    }
                }
            }
            rows
        }
        IcInstanceDetailSection::TrustedIssuers => {
            let mut rows = vec![("".to_string(), "".to_string())];
            match tti {
                None | Some(Lazy::Loading) => {
                    rows.push(("  Loading…".to_string(), "".to_string()));
                }
                Some(Lazy::Error(e)) => {
                    rows.extend(error_rows(e));
                }
                Some(Lazy::Loaded(list)) if list.is_empty() => {
                    rows.push((
                        "  No trusted token issuers configured.".to_string(),
                        "".to_string(),
                    ));
                    rows.push((String::new(), String::new()));
                    rows.push((
                        "  Trusted token issuers let external IdP tokens propagate identity into AWS services (trusted identity propagation)."
                            .to_string(),
                        String::new(),
                    ));
                }
                Some(Lazy::Loaded(list)) => {
                    for (i, t) in list.iter().enumerate() {
                        if i > 0 {
                            rows.push((String::new(), String::new()));
                        }
                        rows.push((t.name.clone(), String::new()));
                        rows.push(("Type".to_string(), t.issuer_type.clone()));
                        if let Some(u) = &t.issuer_url {
                            rows.push(("Issuer URL".to_string(), u.clone()));
                        }
                        if let Some(c) = &t.claim_attribute_path {
                            rows.push(("Claim Attribute".to_string(), c.clone()));
                        }
                        if let Some(p) = &t.identity_store_attribute_path {
                            rows.push(("Store Attribute".to_string(), p.clone()));
                        }
                        if let Some(j) = &t.jwks_retrieval {
                            rows.push(("JWKS Retrieval".to_string(), j.clone()));
                        }
                        rows.push(("ARN".to_string(), t.arn.clone()));
                    }
                }
            }
            rows
        }
        IcInstanceDetailSection::Summary => {
            let (ps, users, groups, apps) = counts;
            vec![
                ("Permission Sets".to_string(), ps.to_string()),
                ("Users".to_string(), users.to_string()),
                ("Groups".to_string(), groups.to_string()),
                ("Applications".to_string(), apps.to_string()),
                (String::new(), String::new()),
                (
                    "  Counts reflect the loaded list — sub-tabs 2–5 browse each type."
                        .to_string(),
                    String::new(),
                ),
            ]
        }
    }
}

pub fn ic_application_section_lines(
    a: &crate::aws::services::identity_center::IcApplication,
    section: IcApplicationDetailSection,
    assignments: Option<&Lazy<Box<crate::aws::services::identity_center::IcAppAssignments>>>,
) -> Vec<(String, String)> {
    match section {
        IcApplicationDetailSection::Overview => {
            let mut rows = vec![
                ("Name".to_string(), a.name.clone()),
                ("Provider".to_string(), a.provider_name().to_string()),
                ("Status".to_string(), a.status.clone()),
            ];
            if !a.description.is_empty() {
                rows.push(("Description".to_string(), a.description.clone()));
            }
            if !a.application_account.is_empty() {
                rows.push(("Account".to_string(), a.application_account.clone()));
            }
            if let Some(c) = &a.created_date {
                rows.push(("Created".to_string(), c.clone()));
            }
            rows.push((String::new(), String::new()));
            rows.push(("Portal".to_string(), String::new()));
            rows.push((
                "Visibility".to_string(),
                a.visibility.clone().unwrap_or_else(|| "—".to_string()),
            ));
            rows.push((
                "Sign-in Origin".to_string(),
                a.sign_in_origin.clone().unwrap_or_else(|| "—".to_string()),
            ));
            if let Some(u) = &a.application_url {
                rows.push(("Application URL".to_string(), u.clone()));
            }
            rows.push((String::new(), String::new()));
            rows.push(("ARN".to_string(), a.arn.clone()));
            rows
        }
        IcApplicationDetailSection::Assignments => {
            let mut rows = vec![("".to_string(), "".to_string())];
            match assignments {
                None | Some(Lazy::Loading) => {
                    rows.push(("  Loading…".to_string(), "".to_string()));
                }
                Some(Lazy::Error(e)) => {
                    rows.extend(error_rows(e));
                }
                Some(Lazy::Loaded(a)) => {
                    if let Some(req) = a.assignment_required {
                        rows.push((
                            "Assignment Required".to_string(),
                            if req { "✓ yes".to_string() } else { "no".to_string() },
                        ));
                        rows.push((String::new(), String::new()));
                    }
                    if a.assignments.is_empty() {
                        let hint = if a.assignment_required == Some(false) {
                            "  No explicit assignments — all users can access (assignment not required)."
                        } else {
                            "  No principals assigned."
                        };
                        rows.push((hint.to_string(), "".to_string()));
                    } else {
                        rows.push((
                            format!("Assigned Principals ({})", a.assignments.len()),
                            String::new(),
                        ));
                        rows.push((String::new(), String::new()));
                        for p in &a.assignments {
                            rows.push((
                                format!("  {}", p.principal_type),
                                p.principal_name.clone(),
                            ));
                        }
                    }
                }
            }
            rows
        }
    }
}
