use super::*;

// ── ECR repository split pane ────────────────────────────────────────────────

pub(super) fn render_ecr_repo_split(app: &App, repo: &EcrRepository, area: Rect, frame: &mut Frame) {
    
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 4, "");
    let mut block = theme::pane_block("ECR Repository", focused);
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
                repo.name.clone(),
                Style::default().fg(crate::ui::theme::text_primary()).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(repo.uri.clone(), Style::default().fg(theme::text_dim())),
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
        &descriptor_tabs(app, &crate::aws::services::ecr::ECR_REPO_SECTIONS),
    );
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub fn ecr_repo_section_lines(
    repo: &EcrRepository,
    section: crate::aws::services::ecr::EcrRepoDetailSection,
    images: &[&crate::aws::services::ecr::EcrImage],
    list_loading: bool,
    lifecycle: Option<&crate::lazy::Lazy<String>>,
) -> Vec<(String, String)> {
    
    match section {
        EcrRepoDetailSection::Details => {
            let mut rows = vec![
                ("Repository".to_string(), repo.name.clone()),
                ("URI".to_string(), repo.uri.clone()),
                ("Tag Mutability".to_string(), repo.image_tag_mutability.clone()),
                ("Scan on Push".to_string(), if repo.scan_on_push { "✓ enabled" } else { "✗ disabled" }.to_string()),
                ("Encryption".to_string(), repo.encryption_type.clone()),
            ];
            if let Some(k) = &repo.kms_key {
                rows.push(("KMS Key".to_string(), k.clone()));
            }
            if let Some(c) = &repo.created {
                rows.push(("Created".to_string(), c.clone()));
            }
            rows.push((String::new(), String::new()));
            rows.push(("ARN".to_string(), repo.arn.clone()));
            rows
        }
        EcrRepoDetailSection::Images => {
            // The Images tab's rows for this repo (the list load fetched
            // them) — filtered, not fetched again.
            if images.is_empty() {
                return if list_loading {
                    vec![("".to_string(), "Loading…".to_string())]
                } else {
                    vec![(" No images in this repository".to_string(), String::new())]
                };
            }
            let seen = images[0].repo_images_seen;
            let complete = images[0].repo_images_complete;
            let heading = if seen > images.len() || !complete {
                format!(
                    "Images (newest {} of {}{})",
                    images.len(),
                    seen,
                    if complete { "" } else { "+" }
                )
            } else {
                format!("Images ({}, newest first)", images.len())
            };
            let mut rows = vec![(heading, String::new())];
            rows.push((String::new(), String::new()));
            for img in images {
                // Group header: the image tag(s), or <untagged>.
                let tag_display = if img.tags.is_empty() {
                    "<untagged>".to_string()
                } else {
                    img.tags.join(", ")
                };
                rows.push((tag_display, String::new()));

                rows.push(("  URI".to_string(), img.image_ref()));
                rows.push(("  Digest".to_string(), img.digest.clone()));
                if let Some(p) = &img.pushed_at {
                    rows.push(("  Pushed".to_string(), p.clone()));
                }
                rows.push((
                    "  Last Pulled".to_string(),
                    img.last_pulled_at.clone().unwrap_or_else(|| "Never".to_string()),
                ));
                rows.push(("  Size".to_string(), img.size_display()));
                if let Some(a) = &img.artifact_media_type {
                    rows.push(("  Artifact Type".to_string(), a.clone()));
                }
                if let Some(m) = &img.manifest_media_type {
                    rows.push(("  Manifest Type".to_string(), m.clone()));
                }

                // Scan status + findings.
                let scan = if img.scan_status.is_empty() {
                    "Not scanned".to_string()
                } else {
                    img.scan_status.clone()
                };
                rows.push(("  Scan Status".to_string(), scan));
                let breakdown = img.vuln_breakdown();
                if !breakdown.is_empty() {
                    rows.push(("  Vulnerabilities".to_string(), breakdown));
                } else if img.scan_status == "COMPLETE" {
                    rows.push(("  Vulnerabilities".to_string(), "✓ No findings".to_string()));
                } else if let Some(d) = &img.scan_status_description {
                    if !d.is_empty() {
                        rows.push(("  Scan Detail".to_string(), d.clone()));
                    }
                }

                rows.push((String::new(), String::new()));
            }
            rows
        }
        EcrRepoDetailSection::LifecyclePolicy => match lifecycle {
            None | Some(crate::lazy::Lazy::Loading) => {
                vec![("".to_string(), "Loading lifecycle policy…".to_string())]
            }
            Some(crate::lazy::Lazy::Error(e)) => error_rows(e),
            Some(crate::lazy::Lazy::Loaded(text)) => {
                if text.is_empty() {
                    return vec![(" No lifecycle policy configured".to_string(), String::new())];
                }
                // Pretty-print the JSON
                let pretty = serde_json::from_str::<serde_json::Value>(text)
                    .ok()
                    .and_then(|v| serde_json::to_string_pretty(&v).ok())
                    .unwrap_or_else(|| text.clone());
                let mut rows = vec![("Lifecycle Policy".to_string(), String::new())];
                rows.push((String::new(), String::new()));
                for line in pretty.lines() {
                    rows.push((format!("  {}", line), String::new()));
                }
                rows
            }
        },
        EcrRepoDetailSection::Tags => tag_rows(&repo.tags),
    }
}

/// Body rows for an ECR image's split pane. `users` is `Some((rows,
/// ecs_cache_warm))` for the Used By section (computed by the caller from
/// in-memory ECS data), `None` otherwise.
pub fn ecr_image_section_lines(
    img: &crate::aws::services::ecr::EcrImage,
    section: crate::aws::services::ecr::EcrImageDetailSection,
    findings: Option<&crate::lazy::Lazy<crate::aws::services::ecr::EcrScanFindings>>,
    users: Option<(&[crate::aws::services::ecr::EcrImageUser], bool)>,
) -> Vec<(String, String)> {
    use crate::aws::services::ecr::EcrImageDetailSection as S;
    use crate::lazy::Lazy;
    match section {
        S::Overview => {
            let mut rows = vec![
                ("Repository".to_string(), img.repo_name.clone()),
                (
                    "Tags".to_string(),
                    if img.tags.is_empty() { "<untagged>".to_string() } else { img.tags.join(", ") },
                ),
                ("Digest".to_string(), img.digest.clone()),
                ("URI".to_string(), img.image_ref()),
                ("Size".to_string(), img.size_display()),
            ];
            if let Some(p) = &img.pushed_at {
                rows.push(("Pushed".to_string(), p.clone()));
            }
            rows.push((
                "Last Pulled".to_string(),
                img.last_pulled_at.clone().unwrap_or_else(|| "Never".to_string()),
            ));
            if let Some(a) = &img.artifact_media_type {
                rows.push(("Artifact Type".to_string(), a.clone()));
            }
            if let Some(m) = &img.manifest_media_type {
                rows.push(("Manifest Type".to_string(), m.clone()));
            }
            rows.push((String::new(), String::new()));
            rows.push(("Scan".to_string(), String::new()));
            rows.push((
                "  Status".to_string(),
                if img.scan_status.is_empty() { "Not scanned".to_string() } else { img.scan_status.clone() },
            ));
            let breakdown = img.vuln_breakdown();
            if !breakdown.is_empty() {
                rows.push(("  Vulnerabilities".to_string(), breakdown));
            } else if img.scan_status == "COMPLETE" {
                rows.push(("  Vulnerabilities".to_string(), "✓ No findings".to_string()));
            }
            if let Some(d) = img.scan_status_description.as_ref().filter(|d| !d.is_empty()) {
                rows.push(("  Detail".to_string(), d.clone()));
            }
            rows.push((String::new(), String::new()));
            rows.push((
                " · Last Pulled is ECR's own record (refreshed at most daily)".to_string(),
                String::new(),
            ));
            rows
        }
        S::Findings => match findings {
            None | Some(Lazy::Loading) => vec![("".to_string(), "Loading…".to_string())],
            Some(Lazy::Error(e)) => error_rows(e),
            Some(Lazy::Loaded(f)) => ecr_findings_rows(f),
        },
        S::UsedBy => {
            let (list, warm) = users.unwrap_or((&[][..], false));
            let mut rows = Vec::new();
            if list.is_empty() {
                rows.push((" No ECS task or task definition found using this image".to_string(), String::new()));
            } else {
                rows.push((format!("ECS ({})", list.len()), String::new()));
                rows.push((String::new(), String::new()));
                for u in list {
                    rows.push((u.name.clone(), String::new()));
                    rows.push((format!("  {}", u.kind), u.id.clone()));
                    rows.push(("  Container".to_string(), u.container.clone()));
                    if !u.status.is_empty() {
                        rows.push(("  Status".to_string(), u.status.clone()));
                    }
                    rows.push((String::new(), String::new()));
                }
            }
            rows.push((String::new(), String::new()));
            // Coverage, so an empty list never reads as "nothing uses it".
            if warm {
                rows.push((" · searched the loaded ECS tasks (running + recently stopped)".to_string(), String::new()));
            } else {
                rows.push((" · ECS not loaded — open @ecs, then come back to search its tasks".to_string(), String::new()));
            }
            rows.push((" · task definitions count once their details have been opened".to_string(), String::new()));
            rows
        }
    }
}

/// The Findings section body: a summary, then one group per finding,
/// worst first (already sorted by the fetch).
pub(super) fn ecr_findings_rows(f: &crate::aws::services::ecr::EcrScanFindings) -> Vec<(String, String)> {
    let mut rows = vec![(
        "Scan Status".to_string(),
        if f.scan_status.is_empty() { "—".to_string() } else { f.scan_status.clone() },
    )];
    if !f.scanner.is_empty() {
        rows.push(("Scanner".to_string(), f.scanner.clone()));
    }
    if let Some(d) = f.status_description.as_ref().filter(|d| !d.is_empty()) {
        rows.push(("Detail".to_string(), d.clone()));
    }
    if let Some(c) = &f.completed_at {
        rows.push(("Completed".to_string(), c.clone()));
    }
    if let Some(u) = &f.vuln_db_updated_at {
        rows.push(("Vuln DB Updated".to_string(), u.clone()));
    }
    rows.push((String::new(), String::new()));
    if f.findings.is_empty() {
        let note = if f.scan_status == "COMPLETE" {
            "✓ No findings"
        } else {
            "No findings to show"
        };
        rows.push((String::new(), note.to_string()));
        return rows;
    }
    let shown = f.findings.len();
    rows.push((
        if f.truncated > 0 {
            format!("Findings ({} worst of {})", shown, shown + f.truncated)
        } else {
            format!("Findings ({})", shown)
        },
        String::new(),
    ));
    rows.push((String::new(), String::new()));
    for x in &f.findings {
        let sev = if x.severity.is_empty() { "UNDEFINED" } else { x.severity.as_str() };
        rows.push((format!("{} {}", sev, x.id), String::new()));
        if let Some(p) = &x.package {
            let ver = x.installed.as_deref().map(|v| format!(" {}", v)).unwrap_or_default();
            rows.push(("  Package".to_string(), format!("{}{}", p, ver)));
        }
        if let Some(v) = &x.fixed_in {
            rows.push(("  Fixed In".to_string(), v.clone()));
        } else if let Some(fa) = &x.fix_available {
            rows.push(("  Fix Available".to_string(), fa.clone()));
        }
        if let Some(sc) = x.score {
            rows.push(("  Score".to_string(), format!("{:.1}", sc)));
        }
        if let Some(st) = &x.status {
            rows.push(("  Status".to_string(), st.clone()));
        }
        if let Some(e) = &x.exploit_available {
            rows.push(("  Exploit Available".to_string(), e.clone()));
        }
        if let Some(u) = &x.url {
            rows.push(("  URL".to_string(), u.clone()));
        }
        if let Some(d) = x.description.as_ref().filter(|d| !d.is_empty()) {
            let one_line = d.split_whitespace().collect::<Vec<_>>().join(" ");
            let short: String = one_line.chars().take(200).collect();
            let ell = if one_line.chars().count() > 200 { "…" } else { "" };
            rows.push(("  Description".to_string(), format!("{}{}", short, ell)));
        }
        rows.push((String::new(), String::new()));
    }
    rows
}
