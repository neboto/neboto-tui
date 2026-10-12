use super::*;

// ── ACM Certificate Split Pane ────────────────────────────────────────────────

pub(super) fn render_acm_cert_split(app: &App, cert: &AcmCertificate, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 4, "");

    let mut block = theme::pane_block("ACM Certificate", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let header_lines = build_acm_cert_header_lines(cert);
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
    render_acm_section_tabs(app, chunks[2], frame);
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub(super) fn build_acm_cert_header_lines(cert: &AcmCertificate) -> Vec<Line<'static>> {
    let mut lines = vec![Line::raw("")];

    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            cert.domain_name.clone(),
            Style::default()
                .fg(crate::ui::theme::text_primary())
                .add_modifier(Modifier::BOLD),
        ),
    ]));

    let status_color = match cert.status.as_str() {
        "ISSUED" => match cert.days_until_expiry {
            Some(d) if d < 30 => theme::error(),
            Some(d) if d < 90 => theme::warning(),
            _ => theme::success(),
        },
        "EXPIRED" | "FAILED" => theme::error(),
        "PENDING_VALIDATION" => theme::warning(),
        _ => theme::text_dim(),
    };

    let cert_type_display = match cert.cert_type.as_str() {
        "AMAZON_ISSUED" => "Amazon Issued",
        "IMPORTED" => "Imported",
        "PRIVATE" => "Private CA",
        other => other,
    };

    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(cert.status.clone(), Style::default().fg(status_color)),
        Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
        Span::styled(
            cert_type_display.to_string(),
            Style::default().fg(theme::text_dim()),
        ),
    ]));

    if let Some(days) = cert.days_until_expiry {
        let expiry_text = if days < 0 {
            format!(
                "Expired {}  ({} days ago)",
                cert.not_after.as_deref().unwrap_or("—"),
                days.abs()
            )
        } else {
            format!(
                "Expires {}  ({} days remaining)",
                cert.not_after.as_deref().unwrap_or("—"),
                days
            )
        };
        let expiry_color = if days < 0 || days < 30 {
            theme::error()
        } else if days < 90 {
            theme::warning()
        } else {
            theme::text_dim()
        };
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(expiry_text, Style::default().fg(expiry_color)),
        ]));
    } else if cert.not_after.is_some() {
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(
                format!("Expires {}", cert.not_after.as_deref().unwrap_or("—")),
                Style::default().fg(theme::text_dim()),
            ),
        ]));
    }

    lines.push(Line::raw(""));
    lines
}

pub(super) fn render_acm_section_tabs(app: &App, area: Rect, frame: &mut Frame) {
    render_section_tab_bar(
        app,
        area,
        frame,
        &descriptor_tabs(app, &crate::aws::services::acm::ACM_CERT_SECTIONS),
    );
}

pub fn acm_cert_section_lines(
    cert: &AcmCertificate,
    section: AcmCertDetailSection,
    details_state: Option<&crate::lazy::Lazy<crate::aws::services::acm::AcmCertDetails>>,
) -> Vec<(String, String)> {
    match section {
        AcmCertDetailSection::Details => acm_details_lines(cert, details_state),
        AcmCertDetailSection::Domains => acm_domains_lines(cert, details_state),
        AcmCertDetailSection::Validation => acm_validation_lines(details_state),
        AcmCertDetailSection::Tags => acm_tags_lines(details_state),
    }
}

pub(super) fn acm_details_lines(
    cert: &AcmCertificate,
    details_state: Option<&crate::lazy::Lazy<crate::aws::services::acm::AcmCertDetails>>,
) -> Vec<(String, String)> {
    let mut rows = vec![
        ("ARN".to_string(), cert.certificate_arn.clone()),
        ("Domain".to_string(), cert.domain_name.clone()),
        ("Status".to_string(), cert.status.clone()),
        (
            "Type".to_string(),
            match cert.cert_type.as_str() {
                "AMAZON_ISSUED" => "Amazon Issued".to_string(),
                "IMPORTED" => "Imported".to_string(),
                "PRIVATE" => "Private CA".to_string(),
                other => other.to_string(),
            },
        ),
        (
            "Expires".to_string(),
            cert.not_after.clone().unwrap_or_else(|| "—".to_string()),
        ),
    ];
    if let Some(days) = cert.days_until_expiry {
        rows.push((
            "Days Remaining".to_string(),
            if days < 0 {
                format!("Expired {} days ago", days.abs())
            } else {
                format!("{}", days)
            },
        ));
    }
    rows.push(("".to_string(), "".to_string()));

    match details_state {
        None | Some(crate::lazy::Lazy::Loading) => {
            rows.push(("".to_string(), "Loading certificate details…".to_string()));
        }
        Some(crate::lazy::Lazy::Error(e)) => {
            rows.extend(error_rows(e));
        }
        Some(crate::lazy::Lazy::Loaded(d)) => {
            rows.push(("Key Algorithm".to_string(), d.key_algorithm.clone()));
            if let Some(issuer) = &d.issuer {
                rows.push(("Issuer".to_string(), issuer.clone()));
            }
            if let Some(ca) = &d.ca_arn {
                // A full ACM PCA ARN → the generic ARN jump.
                rows.push(("Private CA".to_string(), ca.clone()));
            }
            rows.push((
                "Renewal Eligibility".to_string(),
                d.renewal_eligibility.clone(),
            ));
            if let Some(r) = &d.renewal {
                let status = match (r.status.as_str(), &r.reason) {
                    ("FAILED", Some(reason)) => format!("✗ FAILED — {reason}"),
                    ("PENDING_VALIDATION", _) => "⚠ PENDING_VALIDATION".to_string(),
                    ("SUCCESS", _) => "✓ SUCCESS".to_string(),
                    (s, _) => s.to_string(),
                };
                // The domains holding it up sit indented under it, so an
                // export nests them in the Renewal object.
                rows.push(("Renewal".to_string(), status));
                for (domain, status) in &r.pending_domains {
                    rows.push((format!("  {domain}"), format!("⚠ {status}")));
                }
                rows.push(("Renewal Updated".to_string(), r.updated_at.clone()));
            }
            if let Some(issued) = &d.issued_at {
                rows.push(("Issued At".to_string(), issued.clone()));
            }
            if let Some(nb) = &d.not_before {
                rows.push(("Not Before".to_string(), nb.clone()));
            }
            rows.push(("".to_string(), "".to_string()));
            if d.in_use_by.is_empty() {
                rows.push(("In Use By".to_string(), "Not in use".to_string()));
            } else {
                rows.push(("In Use By".to_string(), "".to_string()));
                for arn in &d.in_use_by {
                    rows.push((format!("  {}", arn), "".to_string()));
                }
            }
        }
    }

    rows.push(("".to_string(), "".to_string()));
    rows
}

pub(super) fn acm_domains_lines(
    cert: &AcmCertificate,
    details_state: Option<&crate::lazy::Lazy<crate::aws::services::acm::AcmCertDetails>>,
) -> Vec<(String, String)> {
    let mut rows = vec![
        ("Primary Domain".to_string(), cert.domain_name.clone()),
        ("".to_string(), "".to_string()),
    ];

    match details_state {
        None | Some(crate::lazy::Lazy::Loading) => {
            rows.push(("".to_string(), "Loading SANs…".to_string()));
        }
        Some(crate::lazy::Lazy::Error(e)) => {
            rows.extend(error_rows(e));
        }
        Some(crate::lazy::Lazy::Loaded(d)) => {
            if d.subject_alternative_names.is_empty() {
                rows.push(("SANs".to_string(), "None".to_string()));
            } else {
                rows.push((
                    "SANs".to_string(),
                    format!("({} domains)", d.subject_alternative_names.len()),
                ));
                for san in &d.subject_alternative_names {
                    rows.push((format!("  {}", san), "".to_string()));
                }
            }
        }
    }

    rows.push(("".to_string(), "".to_string()));
    rows
}

pub(super) fn acm_validation_lines(details_state: Option<&crate::lazy::Lazy<crate::aws::services::acm::AcmCertDetails>>) -> Vec<(String, String)> {
    match details_state {
        None | Some(crate::lazy::Lazy::Loading) => {
            vec![("".to_string(), "Loading validation details…".to_string())]
        }
        Some(crate::lazy::Lazy::Error(e)) => error_rows(e),
        Some(crate::lazy::Lazy::Loaded(d)) => {
            let mut rows = vec![];
            if d.validation_options.is_empty() {
                rows.push(("".to_string(), "No validation options available.".to_string()));
            }
            for opt in &d.validation_options {
                rows.push(("Domain".to_string(), opt.domain_name.clone()));
                rows.push(("Method".to_string(), opt.validation_method.clone()));
                rows.push(("Status".to_string(), opt.validation_status.clone()));
                if let Some(rr) = &opt.resource_record {
                    rows.push(("".to_string(), "".to_string()));
                    rows.push(("CNAME Name".to_string(), rr.name.clone()));
                    rows.push(("CNAME Value".to_string(), rr.value.clone()));
                    rows.push(("Record Type".to_string(), rr.record_type.clone()));
                }
                rows.push(("".to_string(), "".to_string()));
            }
            rows
        }
    }
}

pub(super) fn acm_tags_lines(details_state: Option<&crate::lazy::Lazy<crate::aws::services::acm::AcmCertDetails>>) -> Vec<(String, String)> {
    match details_state {
        None | Some(crate::lazy::Lazy::Loading) => {
            vec![("".to_string(), "Loading tags…".to_string())]
        }
        Some(crate::lazy::Lazy::Error(e)) => error_rows(e),
        Some(crate::lazy::Lazy::Loaded(d)) if d.tags_error.is_some() => {
            error_rows(d.tags_error.as_deref().unwrap_or_default())
        }
        Some(crate::lazy::Lazy::Loaded(d)) => {
            let mut rows = vec![];
            if d.tags.is_empty() {
                rows.push(("".to_string(), "No tags".to_string()));
            } else {
                let mut sorted: Vec<(&String, &String)> = d.tags.iter().collect();
                sorted.sort_by_key(|(k, _)| k.as_str());
                for (key, value) in sorted {
                    rows.push((format!("  {}", key), value.clone()));
                }
            }
            rows.push(("".to_string(), "".to_string()));
            rows
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aws::services::acm::{AcmCertDetails, AcmRenewal};

    #[test]
    fn a_stalled_renewal_names_the_domain_holding_it_up() {
        let cert = AcmCertificate::from_summary(&aws_sdk_acm::types::CertificateSummary::builder().certificate_arn("arn:aws:acm:us-east-1:1:certificate/x").domain_name("api.example.com").build());
        let d = AcmCertDetails {
            subject_alternative_names: vec![],
            key_algorithm: "RSA_2048".into(),
            renewal_eligibility: "ELIGIBLE".into(),
            renewal: Some(AcmRenewal {
                status: "PENDING_VALIDATION".into(),
                reason: None,
                updated_at: "2026-10-01 00:00".into(),
                pending_domains: vec![("api.example.com".into(), "PENDING_VALIDATION".into())],
            }),
            issuer: Some("Amazon".into()),
            ca_arn: None,
            in_use_by: vec![],
            issued_at: None,
            not_before: None,
            validation_options: vec![],
            tags: Default::default(),
            tags_error: None,
        };
        let rows = acm_details_lines(&cert, Some(&crate::lazy::Lazy::Loaded(d)));
        let at = |k: &str| rows.iter().position(|(key, _)| key == k).unwrap_or_else(|| panic!("{k}: {rows:?}"));
        assert_eq!(rows[at("Issuer")].1, "Amazon");
        assert_eq!(rows[at("Renewal")].1, "⚠ PENDING_VALIDATION");
        // The domain sits indented right under Renewal, so an export nests it there.
        assert_eq!(rows[at("Renewal") + 1], ("  api.example.com".to_string(), "⚠ PENDING_VALIDATION".to_string()));
    }
}
