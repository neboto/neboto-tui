use super::*;

// ── Service Catalog split panes ──────────────────────────────────────────────────

pub(super) fn render_sc_split(
    app: &App,
    title: &str,
    header: Vec<Line<'static>>,
    tabs: &[(char, &str, bool)],
    extras: &str,
    area: Rect,
    frame: &mut Frame,
) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, tabs.len(), extras);
    let mut block = theme::pane_block(title, focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);
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

pub(super) fn sc_header(name: &str, subtitle: Vec<Span<'static>>) -> Vec<Line<'static>> {
    let mut second = vec![Span::raw("  ")];
    second.extend(subtitle);
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
        Line::from(second),
        Line::raw(""),
    ]
}

pub(super) fn render_sc_portfolio_split(app: &App, portfolio: &ScPortfolio, area: Rect, frame: &mut Frame) {
    let mut sub = vec![Span::styled(
        portfolio.provider.clone(),
        Style::default().fg(theme::text_dim()),
    )];
    if let Some(c) = &portfolio.created {
        sub.push(Span::styled("  ·  ", Style::default().fg(theme::text_dim())));
        sub.push(Span::styled(c.clone(), Style::default().fg(theme::text_dim())));
    }
    let header = sc_header(&portfolio.name, sub);
    render_sc_split(
        app,
        "SC Portfolio",
        header,
        &descriptor_tabs(app, &crate::aws::services::servicecatalog::SC_PORTFOLIO_SECTIONS),
        "",
        area,
        frame,
    );
}

pub(super) fn render_sc_product_split(app: &App, product: &ScProduct, area: Rect, frame: &mut Frame) {
    let mut sub = vec![Span::styled(
        product.owner.clone(),
        Style::default().fg(theme::text_dim()),
    )];
    if !product.product_type.is_empty() {
        sub.push(Span::styled("  ·  ", Style::default().fg(theme::text_dim())));
        sub.push(Span::styled(
            product.product_type.clone(),
            Style::default().fg(theme::text_dim()),
        ));
    }
    let header = sc_header(&product.name, sub);
    // Advertise the template download while the Versions section is up and
    // at least one version actually has a template URL to open.
    let has_template = matches!(
        app.lazy.sc_product_details.get(&product.id),
        Some(crate::lazy::Lazy::Loaded(d)) if d.artifacts.iter().any(|a| !a.template_url.is_empty())
    );
    let extras = if has_template
        && crate::aws::services::servicecatalog::ScProductDetailSection::from_index(
            app.detail_section_idx,
        ) == crate::aws::services::servicecatalog::ScProductDetailSection::Versions
    {
        "e template"
    } else {
        ""
    };
    render_sc_split(
        app,
        "SC Product",
        header,
        &descriptor_tabs(app, &crate::aws::services::servicecatalog::SC_PRODUCT_SECTIONS),
        extras,
        area,
        frame,
    );
}

pub(super) fn render_sc_pp_split(app: &App, pp: &ScProvisionedProduct, area: Rect, frame: &mut Frame) {
    let status_color = match pp.status.as_str() {
        "AVAILABLE" => theme::success(),
        "UNDER_CHANGE" | "PLAN_IN_PROGRESS" => theme::warning(),
        "ERROR" | "TAINTED" => theme::error(),
        _ => theme::text_dim(),
    };
    let mut sub = vec![Span::styled(
        pp.status.clone(),
        Style::default().fg(status_color).add_modifier(Modifier::BOLD),
    )];
    if !pp.product_name.is_empty() {
        sub.push(Span::styled("  ·  ", Style::default().fg(theme::text_dim())));
        sub.push(Span::styled(
            pp.product_name.clone(),
            Style::default().fg(theme::text_dim()),
        ));
    }
    if !pp.artifact_name.is_empty() {
        sub.push(Span::styled("  ·  ", Style::default().fg(theme::text_dim())));
        sub.push(Span::styled(
            pp.artifact_name.clone(),
            Style::default().fg(theme::text_dim()),
        ));
    }
    let header = sc_header(&pp.name, sub);
    render_sc_split(
        app,
        "SC Provisioned Product",
        header,
        &descriptor_tabs(app, &crate::aws::services::servicecatalog::SC_PP_SECTIONS),
        "",
        area,
        frame,
    );
}

pub fn sc_portfolio_section_lines(
    portfolio: &ScPortfolio,
    section: ScPortfolioDetailSection,
    products: Option<&crate::lazy::Lazy<Vec<crate::aws::services::servicecatalog::ScPortfolioProduct>>>,
    access: Option<&crate::lazy::Lazy<crate::aws::services::servicecatalog::ScPortfolioAccess>>,
    shares: Option<&crate::lazy::Lazy<Vec<crate::aws::services::servicecatalog::ScShare>>>,
    extras: Option<&crate::lazy::Lazy<crate::aws::services::servicecatalog::ScPortfolioExtras>>,
) -> Vec<(String, String)> {
    match section {
        ScPortfolioDetailSection::Details => {
            let mut rows = vec![
                ("Portfolio".to_string(), portfolio.name.clone()),
                ("ID".to_string(), portfolio.id.clone()),
                ("Provider".to_string(), portfolio.provider.clone()),
            ];
            if !portfolio.description.is_empty() {
                rows.push(("Description".to_string(), portfolio.description.clone()));
            }
            if let Some(c) = &portfolio.created {
                rows.push(("Created".to_string(), c.clone()));
            }
            rows.push((String::new(), String::new()));
            rows.push(("ARN".to_string(), portfolio.arn.clone()));
            rows
        }
        ScPortfolioDetailSection::Products => match products {
            None | Some(crate::lazy::Lazy::Loading) => {
                vec![("".to_string(), "Loading products…".to_string())]
            }
            Some(crate::lazy::Lazy::Error(e)) => error_rows(e),
            Some(crate::lazy::Lazy::Loaded(list)) => {
                if list.is_empty() {
                    return vec![("".to_string(), "No products in this portfolio".to_string())];
                }
                let mut rows = vec![(format!("Products ({})", list.len()), String::new())];
                rows.push((String::new(), String::new()));
                for p in list {
                    rows.push((p.name.clone(), String::new()));
                    rows.push(("  Product ID".to_string(), p.id.clone()));
                    if !p.owner.is_empty() {
                        rows.push(("  Owner".to_string(), p.owner.clone()));
                    }
                    if !p.product_type.is_empty() {
                        rows.push(("  Type".to_string(), p.product_type.clone()));
                    }
                    if !p.short_description.is_empty() {
                        rows.push(("  Description".to_string(), p.short_description.clone()));
                    }
                    rows.push((String::new(), String::new()));
                }
                rows
            }
        },
        ScPortfolioDetailSection::Principals => match access {
            None | Some(crate::lazy::Lazy::Loading) => {
                vec![("".to_string(), "Loading principals…".to_string())]
            }
            Some(crate::lazy::Lazy::Error(e)) => error_rows(e),
            Some(crate::lazy::Lazy::Loaded(a)) => {
                if a.principals.is_empty() {
                    return vec![("".to_string(), "No principals associated".to_string())];
                }
                let mut rows = vec![(format!("Principals ({})", a.principals.len()), String::new())];
                rows.push((String::new(), String::new()));
                for (arn, ptype) in &a.principals {
                    rows.push((ptype.clone(), arn.clone()));
                }
                rows
            }
        },
        ScPortfolioDetailSection::Constraints => match access {
            None | Some(crate::lazy::Lazy::Loading) => {
                vec![("".to_string(), "Loading constraints…".to_string())]
            }
            Some(crate::lazy::Lazy::Error(e)) => error_rows(e),
            Some(crate::lazy::Lazy::Loaded(a)) => {
                if a.constraints.is_empty() {
                    return vec![("".to_string(), "No constraints".to_string())];
                }
                let mut rows = vec![(format!("Constraints ({})", a.constraints.len()), String::new())];
                rows.push((String::new(), String::new()));
                for c in &a.constraints {
                    rows.push((c.ctype.clone(), String::new()));
                    rows.push(("  ID".to_string(), c.id.clone()));
                    if !c.product_id.is_empty() {
                        rows.push(("  Product ID".to_string(), c.product_id.clone()));
                    }
                    if !c.description.is_empty() {
                        rows.push(("  Description".to_string(), c.description.clone()));
                    }
                    rows.push((String::new(), String::new()));
                }
                rows
            }
        },
        ScPortfolioDetailSection::Shares => match shares {
            None | Some(crate::lazy::Lazy::Loading) => {
                vec![("".to_string(), "Loading shares…".to_string())]
            }
            Some(crate::lazy::Lazy::Error(e)) => error_rows(e),
            Some(crate::lazy::Lazy::Loaded(list)) => {
                if list.is_empty() {
                    return vec![("".to_string(), "Not shared".to_string())];
                }
                let mut rows = vec![(format!("Shares ({})", list.len()), String::new())];
                rows.push((String::new(), String::new()));
                for s in list {
                    rows.push((s.share_type.clone(), s.principal_id.clone()));
                    rows.push((
                        "  Accepted".to_string(),
                        if s.accepted { "✓".to_string() } else { "✗".to_string() },
                    ));
                    let mut extras = Vec::new();
                    if s.share_tag_options {
                        extras.push("tag options");
                    }
                    if s.share_principals {
                        extras.push("principals");
                    }
                    if !extras.is_empty() {
                        rows.push(("  Shares".to_string(), extras.join(", ")));
                    }
                    rows.push((String::new(), String::new()));
                }
                rows
            }
        },
        ScPortfolioDetailSection::Tags => match extras {
            None | Some(crate::lazy::Lazy::Loading) => {
                vec![("".to_string(), "Loading tags…".to_string())]
            }
            Some(crate::lazy::Lazy::Error(e)) => error_rows(e),
            Some(crate::lazy::Lazy::Loaded(x)) => {
                let mut rows = Vec::new();
                if x.tags.is_empty() {
                    rows.push(("".to_string(), "No tags".to_string()));
                } else {
                    rows.push((format!("Tags ({})", x.tags.len()), String::new()));
                    rows.push((String::new(), String::new()));
                    for (k, v) in &x.tags {
                        rows.push((k.clone(), v.clone()));
                    }
                }
                if !x.tag_options.is_empty() {
                    rows.push((String::new(), String::new()));
                    rows.push((format!("TagOptions ({})", x.tag_options.len()), String::new()));
                    rows.push((String::new(), String::new()));
                    for (label, active) in &x.tag_options {
                        rows.push((
                            format!("  {}", label),
                            if *active { String::new() } else { "inactive".to_string() },
                        ));
                    }
                }
                rows
            }
        },
    }
}

pub fn sc_product_section_lines(
    product: &ScProduct,
    section: ScProductDetailSection,
    details: Option<&crate::lazy::Lazy<Box<crate::aws::services::servicecatalog::ScProductAdminDetails>>>,
) -> Vec<(String, String)> {
    match section {
        ScProductDetailSection::Details => {
            let mut rows = vec![
                ("Product".to_string(), product.name.clone()),
                ("ID".to_string(), product.id.clone()),
                ("Owner".to_string(), product.owner.clone()),
                ("Type".to_string(), product.product_type.clone()),
            ];
            if !product.status.is_empty() {
                rows.push(("Status".to_string(), product.status.clone()));
            }
            if !product.distributor.is_empty() {
                rows.push(("Distributor".to_string(), product.distributor.clone()));
            }
            if !product.short_description.is_empty() {
                rows.push(("Description".to_string(), product.short_description.clone()));
            }
            if !product.support_email.is_empty() {
                rows.push(("Support Email".to_string(), product.support_email.clone()));
            }
            if !product.support_url.is_empty() {
                rows.push(("Support URL".to_string(), product.support_url.clone()));
            }
            rows.push((
                "Default Launch Path".to_string(),
                if product.has_default_path { "yes".to_string() } else { "no".to_string() },
            ));
            if let Some(c) = &product.created {
                rows.push(("Created".to_string(), c.clone()));
            }
            if !product.arn.is_empty() {
                rows.push((String::new(), String::new()));
                rows.push(("ARN".to_string(), product.arn.clone()));
            }
            rows
        }
        ScProductDetailSection::Versions => match details {
            None | Some(crate::lazy::Lazy::Loading) => {
                vec![("".to_string(), "Loading versions…".to_string())]
            }
            Some(crate::lazy::Lazy::Error(e)) => error_rows(e),
            Some(crate::lazy::Lazy::Loaded(d)) => {
                if d.artifacts.is_empty() {
                    return vec![("".to_string(), "No provisioning artifacts".to_string())];
                }
                let mut rows = vec![(format!("Versions ({})", d.artifacts.len()), String::new())];
                rows.push((String::new(), String::new()));
                for a in &d.artifacts {
                    rows.push((a.name.clone(), String::new()));
                    rows.push(("  ID".to_string(), a.id.clone()));
                    if !a.artifact_type.is_empty() {
                        rows.push(("  Type".to_string(), a.artifact_type.clone()));
                    }
                    if let Some(active) = a.active {
                        rows.push((
                            "  Active".to_string(),
                            if active { "✓ yes".to_string() } else { "✗ no".to_string() },
                        ));
                    }
                    if !a.guidance.is_empty() && a.guidance != "DEFAULT" {
                        // DEPRECATED = end-users shouldn't launch this anymore.
                        rows.push(("  Guidance".to_string(), format!("⚠ {}", a.guidance)));
                    }
                    if !a.description.is_empty() {
                        rows.push(("  Description".to_string(), a.description.clone()));
                    }
                    if let Some(c) = &a.created {
                        rows.push(("  Created".to_string(), c.clone()));
                    }
                    if !a.source_revision.is_empty() {
                        rows.push(("  Source Revision".to_string(), a.source_revision.clone()));
                    }
                    if !a.imported_from.is_empty() {
                        // A stack ARN when the product was imported from a
                        // running stack — Enter-jumpable to the CFN pane.
                        rows.push(("  Imported From".to_string(), a.imported_from.clone()));
                    }
                    if !a.params.is_empty() {
                        rows.push((format!("  Parameters ({})", a.params.len()), String::new()));
                        for p in &a.params {
                            let mut meta = p.param_type.clone();
                            if !p.default.is_empty() {
                                meta = format!("{} · default {}", meta, p.default);
                            }
                            if p.no_echo {
                                meta = format!("{} · no-echo", meta);
                            }
                            if !p.description.is_empty() {
                                meta = format!("{} · {}", meta, p.description);
                            }
                            rows.push((format!("    {}", p.key), meta));
                        }
                    }
                    if !a.template_url.is_empty() {
                        rows.push(("    · template available — e opens it".to_string(), String::new()));
                    }
                    rows.push((String::new(), String::new()));
                }
                if d.artifact_details_capped {
                    rows.push((
                        format!(
                            "    · details fetched for the first {} versions only",
                            crate::aws::services::servicecatalog::MAX_ARTIFACT_DETAILS
                        ),
                        String::new(),
                    ));
                }
                rows
            }
        },
        ScProductDetailSection::Portfolios => match details {
            None | Some(crate::lazy::Lazy::Loading) => {
                vec![("".to_string(), "Loading portfolios…".to_string())]
            }
            Some(crate::lazy::Lazy::Error(e)) => error_rows(e),
            Some(crate::lazy::Lazy::Loaded(d)) => {
                if d.portfolios.is_empty() {
                    return vec![("".to_string(), "Not published to any portfolio".to_string())];
                }
                let mut rows = vec![(format!("Portfolios ({})", d.portfolios.len()), String::new())];
                rows.push((String::new(), String::new()));
                for (id, name) in &d.portfolios {
                    rows.push((name.clone(), id.clone()));
                }
                rows
            }
        },
        ScProductDetailSection::Tags => match details {
            None | Some(crate::lazy::Lazy::Loading) => {
                vec![("".to_string(), "Loading tags…".to_string())]
            }
            Some(crate::lazy::Lazy::Error(e)) => error_rows(e),
            Some(crate::lazy::Lazy::Loaded(d)) => {
                let mut rows = Vec::new();
                if d.tags.is_empty() {
                    rows.push(("".to_string(), "No tags".to_string()));
                } else {
                    rows.push((format!("Tags ({})", d.tags.len()), String::new()));
                    rows.push((String::new(), String::new()));
                    for (k, v) in &d.tags {
                        rows.push((k.clone(), v.clone()));
                    }
                }
                if !d.tag_options.is_empty() {
                    rows.push((String::new(), String::new()));
                    rows.push((format!("TagOptions ({})", d.tag_options.len()), String::new()));
                    rows.push((String::new(), String::new()));
                    for (label, active) in &d.tag_options {
                        rows.push((
                            format!("  {}", label),
                            if *active { String::new() } else { "inactive".to_string() },
                        ));
                    }
                }
                rows
            }
        },
    }
}

pub fn sc_pp_section_lines(
    pp: &ScProvisionedProduct,
    section: ScProvisionedProductDetailSection,
    outputs: Option<&crate::lazy::Lazy<Vec<(String, String, String)>>>,
    records: Option<&crate::lazy::Lazy<Vec<crate::aws::services::servicecatalog::ScRecord>>>,
) -> Vec<(String, String)> {
    match section {
        ScProvisionedProductDetailSection::Details => {
            let mut rows = vec![
                ("Provisioned Product".to_string(), pp.name.clone()),
                ("ID".to_string(), pp.id.clone()),
                ("Status".to_string(), pp.status.clone()),
            ];
            if !pp.status_message.is_empty() {
                rows.push(("Status Message".to_string(), pp.status_message.clone()));
            }
            if !pp.pp_type.is_empty() {
                rows.push(("Type".to_string(), pp.pp_type.clone()));
            }
            if let Some(c) = &pp.created {
                rows.push(("Created".to_string(), c.clone()));
            }
            rows.push((String::new(), String::new()));
            rows.push(("Product".to_string(), pp.product_name.clone()));
            rows.push(("Product ID".to_string(), pp.product_id.clone()));
            rows.push(("Version".to_string(), pp.artifact_name.clone()));
            rows.push(("Version ID".to_string(), pp.artifact_id.clone()));
            if !pp.physical_id.is_empty() {
                rows.push((String::new(), String::new()));
                // A CFN-backed product's physical id is the stack ARN — the
                // generic ARN classifier makes this row Enter-jumpable.
                rows.push(("Stack ARN".to_string(), pp.physical_id.clone()));
            }
            if !pp.user_arn.is_empty() {
                rows.push(("Provisioned By".to_string(), pp.user_arn.clone()));
            }
            if !pp.last_record_id.is_empty() {
                rows.push(("Last Record".to_string(), pp.last_record_id.clone()));
            }
            rows.push((String::new(), String::new()));
            rows.push(("ARN".to_string(), pp.arn.clone()));
            rows
        }
        ScProvisionedProductDetailSection::Outputs => match outputs {
            None | Some(crate::lazy::Lazy::Loading) => {
                vec![("".to_string(), "Loading outputs…".to_string())]
            }
            Some(crate::lazy::Lazy::Error(e)) => error_rows(e),
            Some(crate::lazy::Lazy::Loaded(list)) => {
                if list.is_empty() {
                    return vec![("".to_string(), "No outputs".to_string())];
                }
                let mut rows = vec![(format!("Outputs ({})", list.len()), String::new())];
                rows.push((String::new(), String::new()));
                for (k, v, desc) in list {
                    rows.push((k.clone(), v.clone()));
                    if !desc.is_empty() {
                        rows.push((format!("  · {}", desc), String::new()));
                    }
                }
                rows
            }
        },
        ScProvisionedProductDetailSection::History => match records {
            None | Some(crate::lazy::Lazy::Loading) => {
                vec![("".to_string(), "Loading record history…".to_string())]
            }
            Some(crate::lazy::Lazy::Error(e)) => error_rows(e),
            Some(crate::lazy::Lazy::Loaded(list)) => {
                if list.is_empty() {
                    return vec![("".to_string(), "No records".to_string())];
                }
                let mut rows = vec![(format!("Records ({}, newest first)", list.len()), String::new())];
                rows.push((String::new(), String::new()));
                for r in list {
                    rows.push((format!("{} · {}", r.record_type, r.status), String::new()));
                    rows.push(("  Record ID".to_string(), r.id.clone()));
                    if let Some(c) = &r.created {
                        rows.push(("  Created".to_string(), c.clone()));
                    }
                    if let Some(u) = &r.updated {
                        rows.push(("  Updated".to_string(), u.clone()));
                    }
                    if !r.artifact_id.is_empty() {
                        rows.push(("  Version ID".to_string(), r.artifact_id.clone()));
                    }
                    for e in &r.errors {
                        rows.push((format!("  ⚠ {}", e), String::new()));
                    }
                    rows.push((String::new(), String::new()));
                }
                rows
            }
        },
        ScProvisionedProductDetailSection::Tags => tag_rows(&pp.tags),
    }
}
