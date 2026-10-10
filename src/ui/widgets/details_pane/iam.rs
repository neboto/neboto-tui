use super::*;

// ── Access Analyzer finding split pane ────────────────────────────────────

pub(super) fn render_access_analyzer_split(
    app: &App,
    finding: &AccessAnalyzerFinding,
    area: Rect,
    frame: &mut Frame,
) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 2, "");

    let mut block = theme::pane_block("Access Analyzer Finding", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let (state_color, state_dot) = match finding.status.as_str() {
        "ACTIVE" if finding.is_public => (theme::error(), "● "),
        "ACTIVE" => (theme::warning(), "◌ "),
        _ => (theme::success(), "● "),
    };
    let header_lines = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                finding.resource.clone(),
                Style::default().fg(crate::ui::theme::text_primary()).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(state_dot.to_string(), Style::default().fg(state_color)),
            Span::styled(
                finding.finding_type.clone(),
                Style::default().fg(state_color).add_modifier(Modifier::BOLD),
            ),
            Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
            Span::styled(finding.resource_type.clone(), Style::default().fg(theme::aws_orange())),
        ]),
        Line::raw(""),
    ];
    let header_h = header_lines.len() as u16;

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

    frame.render_widget(Paragraph::new(header_lines), chunks[0]);
    render_hr(chunks[1], frame);
    render_access_analyzer_section_tabs(app, chunks[2], frame);
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub(super) fn render_access_analyzer_section_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let tabs = descriptor_tabs(app, &crate::aws::services::iam::ACCESS_ANALYZER_SECTIONS);
    render_section_tab_bar(app, area, frame, &tabs);
}

pub fn access_analyzer_section_lines(
    finding: &AccessAnalyzerFinding,
    section: AccessAnalyzerDetailSection,
) -> Vec<(String, String)> {
    match section {
        AccessAnalyzerDetailSection::Details => {
            let mut rows: Vec<(String, String)> = Vec::new();
            rows.push(("Finding".to_string(), "".to_string()));
            rows.push(("".to_string(), "".to_string()));
            rows.push(("  Type".to_string(), finding.finding_type.clone()));
            rows.push(("  Status".to_string(), finding.status.clone()));
            rows.push((
                "  Public".to_string(),
                if finding.is_public { "✗ yes".to_string() } else { "✓ no".to_string() },
            ));
            rows.push(("".to_string(), "".to_string()));
            rows.push(("Resource".to_string(), "".to_string()));
            rows.push(("".to_string(), "".to_string()));
            // The resource ARN carries the value so `Enter` jumps to it.
            rows.push(("  Resource".to_string(), finding.resource.clone()));
            rows.push(("  Resource type".to_string(), finding.resource_type.clone()));
            rows.push(("".to_string(), "".to_string()));
            if let Some(a) = &finding.analyzed_at {
                rows.push(("  Analyzed".to_string(), a.clone()));
            }
            if let Some(u) = &finding.updated_at {
                rows.push(("  Updated".to_string(), u.clone()));
            }
            rows.push(("  Analyzer".to_string(), finding.analyzer_arn.clone()));
            rows
        }
        AccessAnalyzerDetailSection::Access => {
            let mut rows: Vec<(String, String)> = Vec::new();
            rows.push(("Granted to".to_string(), "".to_string()));
            rows.push(("".to_string(), "".to_string()));
            if finding.external_principal.is_empty() {
                rows.push(("  Principal".to_string(), "(none)".to_string()));
            } else {
                rows.push(("  Principal".to_string(), finding.external_principal.clone()));
            }

            rows.push(("".to_string(), "".to_string()));
            rows.push((
                format!("Actions ({})", finding.actions.len()),
                "".to_string(),
            ));
            rows.push(("".to_string(), "".to_string()));
            if finding.actions.is_empty() {
                rows.push(("  (none listed)".to_string(), "".to_string()));
            } else {
                for a in &finding.actions {
                    rows.push((format!("  {}", a), "".to_string()));
                }
            }

            if !finding.condition.is_empty() {
                rows.push(("".to_string(), "".to_string()));
                rows.push(("Conditions".to_string(), "".to_string()));
                rows.push(("".to_string(), "".to_string()));
                for (k, v) in &finding.condition {
                    rows.push((format!("  {}", k), v.clone()));
                }
            }
            rows
        }
    }
}

// ── IAM Role split pane ───────────────────────────────────────────────────

pub(super) fn render_iam_role_split(app: &App, role: &IamRole, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;

    let on_policy_row = focused
        && IamRoleDetailSection::from_index(app.detail_section_index()) == IamRoleDetailSection::Permissions
        && {
            let rows = app.get_detail_lines_filtered();
            app.details_selected_index
                .and_then(|i| rows.get(i))
                .map(|(k, v)| iam_permissions_row_target(k, v).is_some())
                .unwrap_or(false)
        };

    let footer = detail_footer(
        app,
        focused,
        4,
        if on_policy_row { "⏎ expand · e edit" } else { "e edit" },
    );

    let mut block = theme::pane_block("IAM Role", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let header_lines = build_iam_role_header_lines(role);
    let header_h = header_lines.len() as u16;

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

    frame.render_widget(Paragraph::new(header_lines), chunks[0]);
    render_hr(chunks[1], frame);
    render_iam_role_section_tabs(app, chunks[2], frame);
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub(super) fn build_iam_role_header_lines(role: &IamRole) -> Vec<Line<'static>> {
    let mut lines = vec![Line::raw("")];

    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            role.role_name.clone(),
            Style::default()
                .fg(crate::ui::theme::text_primary())
                .add_modifier(Modifier::BOLD),
        ),
    ]));

    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(role.arn.clone(), Style::default().fg(theme::text_dim())),
    ]));

    if let Some(created) = &role.create_date {
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(
                format!("Created {}", created),
                Style::default().fg(theme::text_dim()),
            ),
        ]));
    }

    lines.push(Line::raw(""));
    lines
}

pub(super) fn render_iam_role_section_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let tabs = descriptor_tabs(app, &crate::aws::services::iam::IAM_ROLE_SECTIONS);
    render_section_tab_bar(app, area, frame, &tabs);
}

pub fn iam_role_section_lines(
    role: &IamRole,
    section: IamRoleDetailSection,
    details_state: Option<&crate::lazy::Lazy<crate::aws::services::iam::IamRoleDetails>>,
    docs_state: Option<&crate::lazy::Lazy<crate::aws::services::iam::IamPolicyDocs>>,
) -> Vec<(String, String)> {
    match section {
        IamRoleDetailSection::Overview => iam_role_overview_lines(role, details_state),
        IamRoleDetailSection::TrustPolicy => iam_role_trust_policy_lines(role),
        IamRoleDetailSection::Permissions => iam_role_permissions_lines(details_state),
        IamRoleDetailSection::Policies => iam_policy_docs_lines(docs_state),
        IamRoleDetailSection::Tags => lazy_tag_lines(details_state, |d| &d.tags),
    }
}

/// Tags rendered off a lazy detail bundle — the IAM `List*` calls omit Tags
/// (documented API behavior), so the section waits on the per-resource Get.
pub(super) fn lazy_tag_lines<T>(
    state: Option<&crate::lazy::Lazy<T>>,
    tags_of: impl Fn(&T) -> &std::collections::HashMap<String, String>,
) -> Vec<(String, String)> {
    let mut rows = vec![("".to_string(), "".to_string())];
    match state {
        None | Some(crate::lazy::Lazy::Loading) => {
            rows.push(("".to_string(), "Loading…".to_string()));
        }
        Some(crate::lazy::Lazy::Error(e)) => {
            rows.extend(error_rows(e));
        }
        Some(crate::lazy::Lazy::Loaded(d)) => {
            let tags = tags_of(d);
            if tags.is_empty() {
                rows.push(("  No tags".to_string(), "".to_string()));
            } else {
                let mut sorted: Vec<(&String, &String)> = tags.iter().collect();
                sorted.sort_by_key(|(k, _)| k.as_str());
                for (k, v) in sorted {
                    rows.push((format!("  {}", k), v.clone()));
                }
            }
        }
    }
    rows.push(("".to_string(), "".to_string()));
    rows
}

/// Identity + configuration at a glance. The ARN row is `y`-copyable, which is
/// the whole point — it's the section users land on.
pub(super) fn iam_role_overview_lines(
    role: &IamRole,
    details_state: Option<&crate::lazy::Lazy<crate::aws::services::iam::IamRoleDetails>>,
) -> Vec<(String, String)> {
    let mut rows = vec![
        ("Identity".to_string(), "".to_string()),
        ("".to_string(), "".to_string()),
        ("  Role Name".to_string(), role.role_name.clone()),
        ("  ARN".to_string(), role.arn.clone()),
        ("  Role ID".to_string(), role.role_id.clone()),
        ("  Path".to_string(), role.path.clone()),
    ];
    if let Some(desc) = role.description.as_deref().filter(|d| !d.is_empty()) {
        rows.push(("  Description".to_string(), desc.to_string()));
    }
    rows.push(("".to_string(), "".to_string()));
    rows.push(("Configuration".to_string(), "".to_string()));
    rows.push(("".to_string(), "".to_string()));
    rows.push((
        "  Created".to_string(),
        role.create_date.clone().unwrap_or_else(|| "—".to_string()),
    ));
    if let Some(d) = role.max_session_duration {
        rows.push((
            "  Max Session".to_string(),
            format!("{}h ({}s)", d / 3600, d),
        ));
    }
    // Last-used rides the lazy GetRole (ListRoles omits RoleLastUsed).
    match details_state {
        None | Some(crate::lazy::Lazy::Loading) => {
            rows.push(("  Last Activity".to_string(), "Loading…".to_string()));
        }
        Some(crate::lazy::Lazy::Error(_)) => {
            // The Permissions section renders the fetch error in full.
            rows.push(("  Last Activity".to_string(), "—".to_string()));
        }
        Some(crate::lazy::Lazy::Loaded(d)) => {
            let activity = match (&d.last_used_date, &d.last_used_region) {
                (Some(date), Some(region)) => format!("{} ({})", date, region),
                (Some(date), None) => date.clone(),
                _ => "Not tracked (or never used)".to_string(),
            };
            rows.push(("  Last Activity".to_string(), activity));
        }
    }
    rows.push(("".to_string(), "".to_string()));
    rows
}

pub(super) fn iam_role_trust_policy_lines(role: &IamRole) -> Vec<(String, String)> {
    let mut rows = vec![("".to_string(), "".to_string())];
    let doc = role
        .trust_policy_raw
        .as_deref()
        .map(crate::aws::services::iam::pretty_policy_document)
        .unwrap_or_else(|| "No trust policy".to_string());
    for line in doc.lines() {
        rows.push((format!("  {}", line), "".to_string()));
    }
    rows.push(("".to_string(), "".to_string()));
    rows
}

/// What a selected row in the Permissions section refers to, so the caller
/// can fetch that specific policy's document on demand (`⏎`/`e`).
pub enum IamPermissionsRowTarget {
    Managed { name: String, arn: String },
    Inline { name: String },
}

/// Classify a Permissions-section row by its key/value shape:
/// managed policy rows are `("  {name}", "{arn}")`, inline rows are
/// `("  {name}", "inline")` (`iam_policy_attachment_rows` — change the two
/// together). Group headers and blank spacers don't start with two spaces,
/// so they're naturally excluded.
pub fn iam_permissions_row_target(key: &str, value: &str) -> Option<IamPermissionsRowTarget> {
    if !key.starts_with("  ") {
        return None;
    }
    let name = key.trim().to_string();
    if name.is_empty() || name == "(none)" {
        return None;
    }
    if value.starts_with("arn:") {
        Some(IamPermissionsRowTarget::Managed {
            name,
            arn: value.to_string(),
        })
    } else if value == "inline" {
        Some(IamPermissionsRowTarget::Inline { name })
    } else {
        None
    }
}

pub(super) fn iam_role_permissions_lines(
    details_state: Option<&crate::lazy::Lazy<crate::aws::services::iam::IamRoleDetails>>,
) -> Vec<(String, String)> {
    match details_state {
        None | Some(crate::lazy::Lazy::Loading) => {
            vec![("".to_string(), "Loading permissions…".to_string())]
        }
        Some(crate::lazy::Lazy::Error(e)) => error_rows(e),
        Some(crate::lazy::Lazy::Loaded(d)) => {
            let mut rows = iam_policy_attachment_rows(&d.managed_policies, &d.inline_policy_names);
            rows.push(("".to_string(), "".to_string()));
            if let Some(boundary) = &d.permissions_boundary {
                rows.push(("Permissions Boundary".to_string(), "".to_string()));
                rows.push(("".to_string(), "".to_string()));
                rows.push(("  Boundary".to_string(), boundary.clone()));
                rows.push(("".to_string(), "".to_string()));
            }
            rows
        }
    }
}

// ── IAM Policy split pane ─────────────────────────────────────────────────

pub(super) fn render_iam_policy_split(app: &App, policy: &IamPolicy, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;

    let footer = detail_footer(app, focused, 4, "");

    let mut block = theme::pane_block("IAM Policy", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let header_lines = build_iam_policy_header_lines(policy);
    let header_h = header_lines.len() as u16;

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

    frame.render_widget(Paragraph::new(header_lines), chunks[0]);
    render_hr(chunks[1], frame);
    render_iam_policy_section_tabs(app, chunks[2], frame);
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub(super) fn build_iam_policy_header_lines(policy: &IamPolicy) -> Vec<Line<'static>> {
    let mut lines = vec![Line::raw("")];

    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            policy.policy_name.clone(),
            Style::default()
                .fg(crate::ui::theme::text_primary())
                .add_modifier(Modifier::BOLD),
        ),
    ]));

    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(policy.policy_arn.clone(), Style::default().fg(theme::text_dim())),
    ]));

    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            format!("{} attachments", policy.attachment_count),
            Style::default().fg(theme::text_dim()),
        ),
    ]));

    if let Some(created) = &policy.create_date {
        let updated_suffix = policy
            .update_date
            .as_deref()
            .map(|u| format!("  ·  Updated {}", u))
            .unwrap_or_default();
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(
                format!("Created {}{}", created, updated_suffix),
                Style::default().fg(theme::text_dim()),
            ),
        ]));
    }

    lines.push(Line::raw(""));
    lines
}

pub(super) fn render_iam_policy_section_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let tabs = descriptor_tabs(app, &crate::aws::services::iam::IAM_POLICY_SECTIONS);
    render_section_tab_bar(app, area, frame, &tabs);
}

pub fn iam_policy_section_lines(
    policy: &IamPolicy,
    section: IamPolicyDetailSection,
    details_state: Option<&crate::lazy::Lazy<crate::aws::services::iam::IamPolicyDetails>>,
) -> Vec<(String, String)> {
    match section {
        IamPolicyDetailSection::Overview => iam_policy_overview_lines(policy),
        IamPolicyDetailSection::Document => iam_policy_document_lines(details_state),
        IamPolicyDetailSection::AttachedTo => iam_policy_attached_to_lines(details_state),
        IamPolicyDetailSection::Tags => lazy_tag_lines(details_state, |d| &d.tags),
    }
}

/// Identity + attachment summary at a glance, with the ARN row `y`-copyable.
pub(super) fn iam_policy_overview_lines(policy: &IamPolicy) -> Vec<(String, String)> {
    let mut rows = vec![
        ("Identity".to_string(), "".to_string()),
        ("".to_string(), "".to_string()),
        ("  Policy Name".to_string(), policy.policy_name.clone()),
        ("  ARN".to_string(), policy.policy_arn.clone()),
        ("  Policy ID".to_string(), policy.policy_id.clone()),
        ("  Path".to_string(), policy.path.clone()),
    ];
    if let Some(desc) = policy.description.as_deref().filter(|d| !d.is_empty()) {
        rows.push(("  Description".to_string(), desc.to_string()));
    }
    rows.push(("".to_string(), "".to_string()));
    rows.push(("Configuration".to_string(), "".to_string()));
    rows.push(("".to_string(), "".to_string()));
    rows.push((
        "  Attachments".to_string(),
        policy.attachment_count.to_string(),
    ));
    rows.push((
        "  Default Version".to_string(),
        policy.default_version_id.clone(),
    ));
    rows.push((
        "  Created".to_string(),
        policy.create_date.clone().unwrap_or_else(|| "—".to_string()),
    ));
    if let Some(updated) = &policy.update_date {
        rows.push(("  Updated".to_string(), updated.clone()));
    }
    rows.push(("".to_string(), "".to_string()));
    rows
}

pub(super) fn iam_policy_document_lines(
    details_state: Option<&crate::lazy::Lazy<crate::aws::services::iam::IamPolicyDetails>>,
) -> Vec<(String, String)> {
    match details_state {
        None | Some(crate::lazy::Lazy::Loading) => {
            vec![("".to_string(), "Loading policy document…".to_string())]
        }
        Some(crate::lazy::Lazy::Error(e)) => error_rows(e),
        Some(crate::lazy::Lazy::Loaded(d)) => {
            let mut rows = vec![("".to_string(), "".to_string())];
            for line in d.document_json.lines() {
                rows.push((format!("  {}", line), "".to_string()));
            }
            rows.push(("".to_string(), "".to_string()));
            rows
        }
    }
}

pub(super) fn iam_policy_attached_to_lines(
    details_state: Option<&crate::lazy::Lazy<crate::aws::services::iam::IamPolicyDetails>>,
) -> Vec<(String, String)> {
    match details_state {
        None | Some(crate::lazy::Lazy::Loading) => {
            vec![("".to_string(), "Loading attachments…".to_string())]
        }
        Some(crate::lazy::Lazy::Error(e)) => error_rows(e),
        Some(crate::lazy::Lazy::Loaded(d)) => {
            let mut rows = vec![];

            if d.attached_roles.is_empty() {
                rows.push(("Roles".to_string(), "None".to_string()));
            } else {
                rows.push(("Roles".to_string(), format!("({})", d.attached_roles.len())));
                for name in &d.attached_roles {
                    rows.push((format!("  {}", name), "".to_string()));
                }
            }
            rows.push(("".to_string(), "".to_string()));

            if d.attached_users.is_empty() {
                rows.push(("Users".to_string(), "None".to_string()));
            } else {
                rows.push(("Users".to_string(), format!("({})", d.attached_users.len())));
                for name in &d.attached_users {
                    rows.push((format!("  {}", name), "".to_string()));
                }
            }
            rows.push(("".to_string(), "".to_string()));

            if d.attached_groups.is_empty() {
                rows.push(("Groups".to_string(), "None".to_string()));
            } else {
                rows.push(("Groups".to_string(), format!("({})", d.attached_groups.len())));
                for name in &d.attached_groups {
                    rows.push((format!("  {}", name), "".to_string()));
                }
            }
            rows.push(("".to_string(), "".to_string()));
            rows
        }
    }
}

// ── IAM User split pane ───────────────────────────────────────────────────

pub(super) fn render_iam_user_split(app: &App, user: &IamUser, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 4, "");
    let mut block = theme::pane_block("IAM User", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut header_lines: Vec<Line<'static>> = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                user.user_name.clone(),
                Style::default().fg(crate::ui::theme::text_primary()).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::raw(""),
    ];
    if let Some(c) = &user.create_date {
        header_lines.push(header_kv("Created", c));
    }
    header_lines.push(header_kv("Path", &user.path));
    header_lines.push(Line::raw(""));
    let header_h = header_lines.len() as u16;

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
    frame.render_widget(Paragraph::new(header_lines), chunks[0]);
    render_hr(chunks[1], frame);
    render_iam_user_section_tabs(app, chunks[2], frame);
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub(super) fn render_iam_user_section_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let tabs = descriptor_tabs(app, &crate::aws::services::iam::IAM_USER_SECTIONS);
    render_section_tab_bar(app, area, frame, &tabs);
}

/// Render managed (name → ARN, a jump target) + inline policy names — shared by
/// the IAM user and group Permissions sections.
pub(super) fn iam_policy_attachment_rows(
    managed: &[(String, String)],
    inline: &[String],
) -> Vec<(String, String)> {
    // Group headers without a count in the label, so an export's keys don't
    // change with the number of policies (`"Managed Policies": {…}`); a
    // principal with none gets a plain `Managed Policies: None` row instead.
    // Inline rows say `inline` — `iam_permissions_row_target` keys on it.
    let mut rows = vec![];
    if managed.is_empty() {
        rows.push(("Managed Policies".to_string(), "None".to_string()));
    } else {
        rows.push(("Managed Policies".to_string(), "".to_string()));
        for (name, arn) in managed {
            rows.push((format!("  {}", name), arn.clone()));
        }
    }
    rows.push(("".to_string(), "".to_string()));
    if inline.is_empty() {
        rows.push(("Inline Policies".to_string(), "None".to_string()));
    } else {
        rows.push(("Inline Policies".to_string(), "".to_string()));
        for name in inline {
            rows.push((format!("  {}", name), "inline".to_string()));
        }
    }
    rows
}

/// The Policies section: each attached and inline policy under its own
/// header, with its type, ARN and the document itself — what the principal
/// can actually do, which Permissions only names.
pub(super) fn iam_policy_docs_lines(
    state: Option<&crate::lazy::Lazy<crate::aws::services::iam::IamPolicyDocs>>,
) -> Vec<(String, String)> {
    let mut rows = vec![("".to_string(), "".to_string())];
    let docs = match state {
        None | Some(crate::lazy::Lazy::Loading) => {
            rows.push(("".to_string(), "Loading policy documents…".to_string()));
            return rows;
        }
        Some(crate::lazy::Lazy::Error(e)) => {
            rows.extend(error_rows(e));
            return rows;
        }
        Some(crate::lazy::Lazy::Loaded(d)) => d,
    };
    if docs.docs.is_empty() {
        rows.push(("".to_string(), "No policies attached".to_string()));
        return rows;
    }
    for doc in &docs.docs {
        rows.push((doc.name.clone(), "".to_string()));
        match &doc.arn {
            Some(arn) => {
                let kind = if arn.contains(":aws:policy/") { "AWS managed" } else { "Customer managed" };
                rows.push(("  Type".to_string(), kind.to_string()));
                rows.push(("  ARN".to_string(), arn.clone()));
            }
            None => rows.push(("  Type".to_string(), "Inline".to_string())),
        }
        match &doc.document {
            Ok(body) => {
                rows.push(("".to_string(), "".to_string()));
                for line in body.lines() {
                    rows.push((format!("  {}", line), "".to_string()));
                }
            }
            Err(e) => rows.extend(error_rows(e)),
        }
        rows.push(("".to_string(), "".to_string()));
    }
    if docs.omitted > 0 {
        rows.push((
            "".to_string(),
            format!(
                "· {} more not fetched (first {} shown) — see Permissions for the full list",
                docs.omitted,
                crate::aws::services::iam::MAX_POLICY_DOCS
            ),
        ));
    }
    rows
}

pub fn iam_user_section_lines(
    user: &IamUser,
    section: IamUserDetailSection,
    details_state: Option<&crate::lazy::Lazy<crate::aws::services::iam::IamUserDetails>>,
    docs_state: Option<&crate::lazy::Lazy<crate::aws::services::iam::IamPolicyDocs>>,
) -> Vec<(String, String)> {
    if section == IamUserDetailSection::Policies {
        return iam_policy_docs_lines(docs_state);
    }
    let mut rows = vec![("".to_string(), "".to_string())];
    match section {
        IamUserDetailSection::Access => {
            rows.push(("Identity".to_string(), "".to_string()));
            rows.push(("".to_string(), "".to_string()));
            rows.push(("  User name".to_string(), user.user_name.clone()));
            rows.push(("  ARN".to_string(), user.arn.clone()));
            rows.push(("  User ID".to_string(), user.user_id.clone()));
            if let Some(c) = &user.create_date {
                rows.push(("  Created".to_string(), c.clone()));
            }
            rows.push(("".to_string(), "".to_string()));
            match details_state {
                None | Some(crate::lazy::Lazy::Loading) => {
                    rows.push(("".to_string(), "Loading…".to_string()));
                }
                Some(crate::lazy::Lazy::Error(e)) => {
                    rows.extend(error_rows(e));
                }
                Some(crate::lazy::Lazy::Loaded(d)) => {
                    rows.push(("Console & MFA".to_string(), "".to_string()));
                    rows.push(("".to_string(), "".to_string()));
                    rows.push((
                        "  Console login".to_string(),
                        if d.has_console_login { "✓ Enabled" } else { "✗ None" }.to_string(),
                    ));
                    let pw = user.password_last_used.as_deref().unwrap_or("never");
                    rows.push(("  Password last used".to_string(), pw.to_string()));
                    rows.push((
                        "  MFA".to_string(),
                        if d.mfa_devices.is_empty() {
                            "✗ Not enabled".to_string()
                        } else {
                            format!("✓ {} device(s)", d.mfa_devices.len())
                        },
                    ));
                    for m in &d.mfa_devices {
                        rows.push((format!("    {}", m), "".to_string()));
                    }

                    rows.push(("".to_string(), "".to_string()));
                    rows.push((format!("Access keys ({})", d.access_keys.len()), "".to_string()));
                    rows.push(("".to_string(), "".to_string()));
                    if d.access_keys.is_empty() {
                        rows.push(("  (none)".to_string(), "".to_string()));
                    } else {
                        rows.push((
                            format!(
                                "  {:<20}  {:<9}  {:<16}  {}",
                                "Key ID", "Status", "Created", "Last used"
                            ),
                            "".to_string(),
                        ));
                        rows.push((
                            format!(
                                "  {:<20}  {:<9}  {:<16}  {}",
                                "────────────────────", "─────────", "────────────────", "────────────────"
                            ),
                            "".to_string(),
                        ));
                        for k in &d.access_keys {
                            let created = k.created.as_deref().unwrap_or("?");
                            let last = k.last_used.as_deref().unwrap_or("never");
                            rows.push((
                                format!("  {:<20}  {:<9}  {:<16}  {}", k.id, k.status, created, last),
                                "".to_string(),
                            ));
                        }
                    }
                }
            }
        }
        IamUserDetailSection::Permissions => match details_state {
            None | Some(crate::lazy::Lazy::Loading) => {
                rows.push(("".to_string(), "Loading…".to_string()));
            }
            Some(crate::lazy::Lazy::Error(e)) => {
                rows.extend(error_rows(e));
            }
            Some(crate::lazy::Lazy::Loaded(d)) => {
                rows.extend(iam_policy_attachment_rows(&d.managed_policies, &d.inline_policy_names));
                if let Some(boundary) = &d.permissions_boundary {
                    rows.push(("".to_string(), "".to_string()));
                    rows.push(("Permissions Boundary".to_string(), "".to_string()));
                    rows.push(("".to_string(), "".to_string()));
                    rows.push(("  Boundary".to_string(), boundary.clone()));
                }
            }
        },
        // Handled by the early return above.
        IamUserDetailSection::Policies => {}
        IamUserDetailSection::Groups => match details_state {
            None | Some(crate::lazy::Lazy::Loading) => {
                rows.push(("".to_string(), "Loading…".to_string()));
            }
            Some(crate::lazy::Lazy::Error(e)) => {
                rows.extend(error_rows(e));
            }
            Some(crate::lazy::Lazy::Loaded(d)) => {
                rows.push((format!("Group memberships ({})", d.groups.len()), "".to_string()));
                rows.push(("".to_string(), "".to_string()));
                if d.groups.is_empty() {
                    rows.push(("  (none)".to_string(), "".to_string()));
                } else {
                    for g in &d.groups {
                        rows.push((format!("  {}", g), "".to_string()));
                    }
                }
            }
        },
        IamUserDetailSection::Tags => {
            // ListUsers omits Tags — they arrive on the lazy GetUser.
            return lazy_tag_lines(details_state, |d| &d.tags);
        }
    }
    rows.push(("".to_string(), "".to_string()));
    rows
}

// ── IAM Group split pane ──────────────────────────────────────────────────

pub(super) fn render_iam_group_split(app: &App, group: &IamGroup, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 3, "");
    let mut block = theme::pane_block("IAM Group", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut header_lines: Vec<Line<'static>> = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                group.group_name.clone(),
                Style::default().fg(crate::ui::theme::text_primary()).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::raw(""),
    ];
    if let Some(c) = &group.create_date {
        header_lines.push(header_kv("Created", c));
    }
    header_lines.push(header_kv("Path", &group.path));
    header_lines.push(Line::raw(""));
    let header_h = header_lines.len() as u16;

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
    frame.render_widget(Paragraph::new(header_lines), chunks[0]);
    render_hr(chunks[1], frame);
    render_iam_group_section_tabs(app, chunks[2], frame);
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub(super) fn render_iam_group_section_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let tabs = descriptor_tabs(app, &crate::aws::services::iam::IAM_GROUP_SECTIONS);
    render_section_tab_bar(app, area, frame, &tabs);
}

pub fn iam_group_section_lines(
    group: &IamGroup,
    section: IamGroupDetailSection,
    details_state: Option<&crate::lazy::Lazy<crate::aws::services::iam::IamGroupDetails>>,
    docs_state: Option<&crate::lazy::Lazy<crate::aws::services::iam::IamPolicyDocs>>,
) -> Vec<(String, String)> {
    // Overview is eager (no fetch) — handle it before the load-state arms so it
    // never shows a spurious "Loading…". Policies has its own fetch.
    if section == IamGroupDetailSection::Overview {
        return iam_group_overview_lines(group);
    }
    if section == IamGroupDetailSection::Policies {
        return iam_policy_docs_lines(docs_state);
    }
    let mut rows = vec![("".to_string(), "".to_string())];
    match (section, details_state) {
        (_, None) | (_, Some(crate::lazy::Lazy::Loading)) => {
            rows.push(("".to_string(), "Loading…".to_string()));
        }
        (_, Some(crate::lazy::Lazy::Error(e))) => {
            rows.extend(error_rows(e));
        }
        (IamGroupDetailSection::Members, Some(crate::lazy::Lazy::Loaded(d))) => {
            rows.push((format!("Members ({})", d.members.len()), "".to_string()));
            rows.push(("".to_string(), "".to_string()));
            if d.members.is_empty() {
                rows.push(("  (none)".to_string(), "".to_string()));
            } else {
                for m in &d.members {
                    rows.push((format!("  {}", m), "".to_string()));
                }
            }
        }
        (IamGroupDetailSection::Permissions, Some(crate::lazy::Lazy::Loaded(d))) => {
            rows.extend(iam_policy_attachment_rows(&d.managed_policies, &d.inline_policy_names));
        }
        // Overview and Policies are handled by the early returns above.
        (IamGroupDetailSection::Overview | IamGroupDetailSection::Policies, _) => {}
    }
    rows.push(("".to_string(), "".to_string()));
    rows
}

/// Identity at a glance, with the ARN row `y`-copyable. IAM groups don't carry
/// tags, so this is the group's identity home.
pub(super) fn iam_group_overview_lines(group: &IamGroup) -> Vec<(String, String)> {
    vec![
        ("Identity".to_string(), "".to_string()),
        ("".to_string(), "".to_string()),
        ("  Group Name".to_string(), group.group_name.clone()),
        ("  ARN".to_string(), group.arn.clone()),
        ("  Group ID".to_string(), group.group_id.clone()),
        ("  Path".to_string(), group.path.clone()),
        (
            "  Created".to_string(),
            group.create_date.clone().unwrap_or_else(|| "—".to_string()),
        ),
        ("".to_string(), "".to_string()),
    ]
}

// ── IAM Identity Provider split pane ──────────────────────────────────────

pub(super) fn render_iam_idp_split(app: &App, idp: &IamIdentityProvider, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 2, "");
    let mut block = theme::pane_block("Identity Provider", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut header_lines: Vec<Line<'static>> = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                idp.provider_name.clone(),
                Style::default().fg(crate::ui::theme::text_primary()).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(idp.arn.clone(), Style::default().fg(theme::text_dim())),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                if idp.kind == "SAML" { "SAML 2.0" } else { "OpenID Connect" }.to_string(),
                Style::default().fg(theme::text_dim()),
            ),
        ]),
        Line::raw(""),
    ];
    if header_lines.len() as u16 > inner.height {
        header_lines.truncate(inner.height as usize);
    }
    let header_h = header_lines.len() as u16;

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
    frame.render_widget(Paragraph::new(header_lines), chunks[0]);
    render_hr(chunks[1], frame);
    let tabs = descriptor_tabs(app, &crate::aws::services::iam::IAM_IDP_SECTIONS);
    render_section_tab_bar(app, chunks[2], frame, &tabs);
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub fn iam_idp_section_lines(
    idp: &IamIdentityProvider,
    section: IamIdpDetailSection,
    details_state: Option<&crate::lazy::Lazy<crate::aws::services::iam::IamIdpDetails>>,
) -> Vec<(String, String)> {
    match section {
        IamIdpDetailSection::Tags => lazy_tag_lines(details_state, |d| &d.tags),
        IamIdpDetailSection::Overview => {
            let mut rows = vec![
                ("Identity".to_string(), "".to_string()),
                ("".to_string(), "".to_string()),
                ("  Provider".to_string(), idp.provider_name.clone()),
                ("  ARN".to_string(), idp.arn.clone()),
                (
                    "  Type".to_string(),
                    if idp.kind == "SAML" { "SAML 2.0" } else { "OpenID Connect" }.to_string(),
                ),
                ("".to_string(), "".to_string()),
                ("Configuration".to_string(), "".to_string()),
                ("".to_string(), "".to_string()),
            ];
            match details_state {
                None | Some(crate::lazy::Lazy::Loading) => {
                    rows.push(("".to_string(), "Loading…".to_string()));
                }
                Some(crate::lazy::Lazy::Error(e)) => {
                    rows.extend(error_rows(e));
                }
                Some(crate::lazy::Lazy::Loaded(d)) => {
                    rows.push((
                        "  Created".to_string(),
                        d.create_date.clone().unwrap_or_else(|| "—".to_string()),
                    ));
                    if idp.kind == "SAML" {
                        rows.push((
                            "  Valid Until".to_string(),
                            d.valid_until.clone().unwrap_or_else(|| "—".to_string()),
                        ));
                        if let Some(len) = d.metadata_len {
                            rows.push((
                                "  Metadata Document".to_string(),
                                format!("{} bytes", len),
                            ));
                        }
                    } else {
                        rows.push((
                            "  Provider URL".to_string(),
                            d.url.clone().unwrap_or_else(|| "—".to_string()),
                        ));
                        rows.push(("".to_string(), "".to_string()));
                        rows.push((
                            format!("Audiences ({})", d.client_ids.len()),
                            "".to_string(),
                        ));
                        rows.push(("".to_string(), "".to_string()));
                        if d.client_ids.is_empty() {
                            rows.push(("  (none)".to_string(), "".to_string()));
                        } else {
                            for c in &d.client_ids {
                                rows.push((format!("  {}", c), "".to_string()));
                            }
                        }
                        if !d.thumbprints.is_empty() {
                            rows.push(("".to_string(), "".to_string()));
                            rows.push((
                                format!("Thumbprints ({})", d.thumbprints.len()),
                                "".to_string(),
                            ));
                            rows.push(("".to_string(), "".to_string()));
                            for t in &d.thumbprints {
                                rows.push((format!("  {}", t), "".to_string()));
                            }
                        }
                    }
                }
            }
            rows.push(("".to_string(), "".to_string()));
            rows
        }
    }
}

// ── IAM Account Settings split pane ───────────────────────────────────────

pub(super) fn render_iam_account_split(app: &App, settings: &IamAccountSettings, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 3, "");
    let mut block = theme::pane_block("IAM Account Settings", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let alias = if settings.aliases.is_empty() {
        "no account alias".to_string()
    } else {
        settings.aliases.join(", ")
    };
    let header_lines: Vec<Line<'static>> = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                "Account settings".to_string(),
                Style::default().fg(crate::ui::theme::text_primary()).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(alias, Style::default().fg(theme::text_dim())),
        ]),
        Line::raw(""),
    ];
    let header_h = header_lines.len() as u16;

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
    frame.render_widget(Paragraph::new(header_lines), chunks[0]);
    render_hr(chunks[1], frame);
    let tabs = descriptor_tabs(app, &crate::aws::services::iam::IAM_ACCOUNT_SECTIONS);
    render_section_tab_bar(app, chunks[2], frame, &tabs);
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub fn iam_account_section_lines(
    settings: &IamAccountSettings,
    section: IamAccountDetailSection,
) -> Vec<(String, String)> {
    let mut rows = vec![("".to_string(), "".to_string())];
    match section {
        IamAccountDetailSection::Overview => {
            rows.push(("Account".to_string(), "".to_string()));
            rows.push(("".to_string(), "".to_string()));
            rows.push((
                "  Alias".to_string(),
                if settings.aliases.is_empty() {
                    "— (none set)".to_string()
                } else {
                    settings.aliases.join(", ")
                },
            ));
            rows.push(("".to_string(), "".to_string()));
            rows.push(("Root User Security".to_string(), "".to_string()));
            rows.push(("".to_string(), "".to_string()));
            if let Some(e) = &settings.summary_error {
                rows.extend(error_rows(e));
            } else {
                let flag = |v: Option<i32>, on: &str, off: &str| match v {
                    Some(0) => off.to_string(),
                    Some(_) => on.to_string(),
                    None => "—".to_string(),
                };
                rows.push((
                    "  Root MFA".to_string(),
                    flag(
                        settings.summary_value("AccountMFAEnabled"),
                        "✓ Enabled",
                        "✗ Not enabled",
                    ),
                ));
                rows.push((
                    "  Root Access Keys".to_string(),
                    flag(
                        settings.summary_value("AccountAccessKeysPresent"),
                        "⚠ Present — root should have none",
                        "✓ None",
                    ),
                ));
                rows.push((
                    "  Root Signing Certs".to_string(),
                    flag(
                        settings.summary_value("AccountSigningCertificatesPresent"),
                        "⚠ Present",
                        "✓ None",
                    ),
                ));
                if let Some(v) = settings.summary_value("GlobalEndpointTokenVersion") {
                    rows.push((
                        "  STS Token Version".to_string(),
                        format!(
                            "v{}{}",
                            v,
                            if v >= 2 { " (valid in all regions)" } else { " (global endpoint only)" }
                        ),
                    ));
                }
            }
        }
        IamAccountDetailSection::PasswordPolicy => {
            if let Some(e) = &settings.pw_policy_error {
                rows.extend(error_rows(e));
            } else if let Some(p) = &settings.password_policy {
                let yn = |b: bool| if b { "✓ Yes" } else { "✗ No" }.to_string();
                rows.push(("Requirements".to_string(), "".to_string()));
                rows.push(("".to_string(), "".to_string()));
                rows.push((
                    "  Minimum Length".to_string(),
                    p.minimum_length.map(|v| v.to_string()).unwrap_or_else(|| "—".to_string()),
                ));
                rows.push(("  Require Symbols".to_string(), yn(p.require_symbols)));
                rows.push(("  Require Numbers".to_string(), yn(p.require_numbers)));
                rows.push(("  Require Uppercase".to_string(), yn(p.require_uppercase)));
                rows.push(("  Require Lowercase".to_string(), yn(p.require_lowercase)));
                rows.push(("".to_string(), "".to_string()));
                rows.push(("Rotation".to_string(), "".to_string()));
                rows.push(("".to_string(), "".to_string()));
                rows.push((
                    "  Users Can Change".to_string(),
                    yn(p.allow_users_to_change),
                ));
                rows.push(("  Passwords Expire".to_string(), yn(p.expire_passwords)));
                if let Some(age) = p.max_age {
                    rows.push(("  Max Age".to_string(), format!("{} days", age)));
                }
                if let Some(n) = p.reuse_prevention {
                    rows.push(("  Reuse Prevention".to_string(), format!("last {} remembered", n)));
                }
                rows.push((
                    "  Hard Expiry".to_string(),
                    if p.hard_expiry {
                        "✓ Admin reset required after expiry".to_string()
                    } else {
                        "✗ Users can self-renew".to_string()
                    },
                ));
            } else {
                rows.push((
                    "  · No custom policy — the AWS default applies (8+ chars, three character classes)".to_string(),
                    "".to_string(),
                ));
            }
        }
        IamAccountDetailSection::Usage => {
            if let Some(e) = &settings.summary_error {
                rows.extend(error_rows(e));
            } else if settings.summary.is_empty() {
                rows.push(("  No summary data".to_string(), "".to_string()));
            } else {
                // Pair every `K` with its `KQuota` sibling ("used of quota");
                // whatever doesn't pair renders raw at the end.
                rows.push(("Entities".to_string(), "".to_string()));
                rows.push(("".to_string(), "".to_string()));
                let mut consumed: Vec<&str> = Vec::new();
                for (key, value) in &settings.summary {
                    if key.ends_with("Quota") {
                        continue;
                    }
                    let quota_key = format!("{}Quota", key);
                    if let Some(q) = settings.summary_value(&quota_key) {
                        consumed.push(key.as_str());
                        rows.push((format!("  {}", key), format!("{} of {}", value, q)));
                    }
                }
                rows.push(("".to_string(), "".to_string()));
                rows.push(("Other".to_string(), "".to_string()));
                rows.push(("".to_string(), "".to_string()));
                for (key, value) in &settings.summary {
                    if key.ends_with("Quota")
                        && consumed.contains(&&key[..key.len() - "Quota".len()])
                    {
                        continue;
                    }
                    if consumed.contains(&key.as_str()) {
                        continue;
                    }
                    rows.push((format!("  {}", key), value.to_string()));
                }
            }
        }
    }
    rows.push(("".to_string(), "".to_string()));
    rows
}

#[cfg(test)]
mod iam_permissions_tests {
    use super::*;
    use crate::aws::services::iam::{IamPolicyDoc, IamPolicyDocs};

    #[test]
    fn classifier_finds_exactly_the_policy_rows() {
        // The Permissions rows and the Enter-to-expand classifier are keyed
        // on each other's shape; this is the contract between them.
        let rows = iam_policy_attachment_rows(
            &[("orders-dynamodb".into(), "arn:aws:iam::1:policy/orders-dynamodb".into())],
            &["sqs-consume-orders".into()],
        );
        let hits: Vec<String> = rows
            .iter()
            .filter_map(|(k, v)| match iam_permissions_row_target(k, v)? {
                IamPermissionsRowTarget::Managed { name, .. } => Some(format!("managed {name}")),
                IamPermissionsRowTarget::Inline { name } => Some(format!("inline {name}")),
            })
            .collect();
        assert_eq!(hits, ["managed orders-dynamodb", "inline sqs-consume-orders"]);
        // Headers carry no count, so export keys are stable.
        assert!(rows.contains(&("Managed Policies".into(), "".into())));
        assert!(rows.contains(&("Inline Policies".into(), "".into())));

        let none = iam_policy_attachment_rows(&[], &[]);
        assert!(none.contains(&("Managed Policies".into(), "None".into())));
        assert!(none.iter().all(|(k, v)| iam_permissions_row_target(k, v).is_none()));
        // A group's Members rows (`("  alice", "")`) aren't inline policies.
        assert!(iam_permissions_row_target("  alice", "").is_none());
    }

    #[test]
    fn policies_section_nests_each_document_under_its_name() {
        let docs = IamPolicyDocs {
            docs: vec![
                IamPolicyDoc {
                    name: "AdministratorAccess".into(),
                    arn: Some("arn:aws:iam::aws:policy/AdministratorAccess".into()),
                    document: Ok("{\n  \"Statement\": []\n}".into()),
                },
                IamPolicyDoc { name: "broken".into(), arn: None, document: Err("AccessDenied".into()) },
            ],
            omitted: 3,
        };
        let rows = iam_policy_docs_lines(Some(&crate::lazy::Lazy::Loaded(docs)));
        let v = crate::export::detail_value_for_test(&rows);
        assert_eq!(v["AdministratorAccess"]["Type"], "AWS managed");
        assert_eq!(v["AdministratorAccess"]["content"][0], "{");
        assert_eq!(v["broken"]["Type"], "Inline");
        assert!(rows.iter().any(|(k, val)| k.contains("AccessDenied") || val.contains("AccessDenied")));
        assert!(rows.iter().any(|(_, val)| val.contains("3 more not fetched")));
    }
}
