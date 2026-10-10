use super::*;

pub(super) fn render_gd_detector_split(app: &App, det: &GdDetector, area: Rect, frame: &mut Frame) {
    render_simple_split(
        app,
        area,
        frame,
        "GuardDuty Detector",
        &det.id,
        &det.status,
        &descriptor_tabs(app, &crate::aws::services::guardduty::GD_DETECTOR_SECTIONS),
    );
}

pub fn gd_detector_section_lines(
    det: &GdDetector,
    section: GdDetectorDetailSection,
) -> Vec<(String, String)> {
    match section {
        GdDetectorDetailSection::Overview => {
            let mut rows = vec![
                (String::new(), String::new()),
                ("Detector ID".to_string(), det.id.clone()),
                ("Status".to_string(), det.status.clone()),
                (
                    "Publish Frequency".to_string(),
                    det.finding_publishing_frequency.clone(),
                ),
            ];
            if let Some(c) = &det.created {
                rows.push(("Created".to_string(), c.clone()));
            }
            if !det.service_role.is_empty() {
                rows.push(("Service Role".to_string(), det.service_role.clone()));
            }
            if !det.org_rows.is_empty() {
                // Where this account sits in the org — the FMS admin-probe
                // pattern, folded in rather than given its own resource.
                rows.push((String::new(), String::new()));
                rows.push(("Organization".to_string(), String::new()));
                rows.extend(det.org_rows.iter().cloned());
            }
            if !det.coverage_rows.is_empty() {
                // The per-resource rows live in the Coverage sub-tab; this is
                // the "how much of the fleet" headline.
                rows.push((String::new(), String::new()));
                rows.push(("Runtime Coverage".to_string(), String::new()));
                rows.extend(det.coverage_rows.iter().cloned());
            }
            rows
        }
        GdDetectorDetailSection::Features => {
            let mut rows = vec![(String::new(), String::new())];
            if det.features.is_empty() {
                rows.push(("".to_string(), "No features reported".to_string()));
            }
            for f in &det.features {
                rows.push((format!("  {}", f), String::new()));
            }
            rows
        }
        GdDetectorDetailSection::Tags => tag_rows(&det.tags),
    }
}

// ── GuardDuty summary split pane ─────────────────────────────────────────────

pub(super) fn render_gd_overview_split(app: &App, ov: &GdOverview, area: Rect, frame: &mut Frame) {
    render_simple_split(
        app,
        area,
        frame,
        "GuardDuty Summary",
        "Findings Summary",
        &format!("{} findings  ·  {}", ov.total_findings, ov.detector_id),
        &descriptor_tabs(app, &crate::aws::services::guardduty::GD_OVERVIEW_SECTIONS),
    );
}

pub fn gd_overview_section_lines(
    ov: &GdOverview,
    section: GdOverviewDetailSection,
) -> Vec<(String, String)> {
    // Every dimension is fetched independently, so each section reports only
    // the failures that actually blanked it.
    let dimension_errors = |prefix: &str| -> Vec<(String, String)> {
        ov.errors
            .iter()
            .filter(|e| e.starts_with(prefix))
            .flat_map(|e| error_rows(e))
            .collect()
    };

    match section {
        GdOverviewDetailSection::Overview => {
            let mut rows = vec![
                (String::new(), String::new()),
                ("Total Findings".to_string(), ov.total_findings.to_string()),
                ("Detector".to_string(), ov.detector_id.clone()),
                (String::new(), String::new()),
                ("By Severity".to_string(), String::new()),
            ];
            for (label, count) in &ov.by_severity {
                // A non-zero Critical/High count is the headline — mark it so
                // it reads as a warning rather than as another statistic.
                let value = if *count > 0 && (label == "Critical" || label == "High") {
                    format!("⚠ {}", count)
                } else {
                    count.to_string()
                };
                rows.push((format!("  {}", label), value));
            }
            if !ov.free_trial.is_empty() {
                rows.push((String::new(), String::new()));
                rows.push(("Free Trial Remaining".to_string(), String::new()));
                for (feature, days) in &ov.free_trial {
                    rows.push((format!("  {}", feature), format!("{} days", days)));
                }
            }
            rows.extend(dimension_errors("severity:"));
            rows
        }
        GdOverviewDetailSection::Types => {
            let mut rows = vec![(String::new(), String::new())];
            if ov.top_types.is_empty() {
                rows.push((" No finding types reported".to_string(), String::new()));
            } else {
                rows.push((
                    format!("Top {} Finding Types", ov.top_types.len()),
                    String::new(),
                ));
                for (name, count) in &ov.top_types {
                    rows.push((format!("  {}", name), count.to_string()));
                }
            }
            rows.extend(dimension_errors("finding types:"));
            rows
        }
        GdOverviewDetailSection::Resources => {
            let mut rows = vec![(String::new(), String::new())];
            if ov.top_resources.is_empty() {
                rows.push((" No resources reported".to_string(), String::new()));
            } else {
                rows.push((
                    format!("Top {} Affected Resources", ov.top_resources.len()),
                    String::new(),
                ));
                for (id, kind, count) in &ov.top_resources {
                    // Key-value shape so the generic classifier makes `i-…`,
                    // `vol-…`, bucket names and the rest Enter-jumpable.
                    rows.push((format!("  {}", id), format!("{} findings", count)));
                    if !kind.is_empty() {
                        rows.push((format!("    · {}", kind), String::new()));
                    }
                }
            }
            rows.extend(dimension_errors("resources:"));
            rows
        }
        GdOverviewDetailSection::Accounts => {
            let mut rows = vec![(String::new(), String::new())];
            if ov.top_accounts.is_empty() {
                rows.push((" No accounts reported".to_string(), String::new()));
            } else {
                rows.push((
                    format!("Top {} Accounts", ov.top_accounts.len()),
                    String::new(),
                ));
                for (account, count) in &ov.top_accounts {
                    rows.push((format!("  {}", account), format!("{} findings", count)));
                }
            }
            rows.extend(dimension_errors("accounts:"));
            rows
        }
        GdOverviewDetailSection::Coverage => {
            let mut rows = vec![(String::new(), String::new())];
            if ov.coverage_rows.is_empty() {
                rows.push((
                    " No runtime coverage reported — Runtime Monitoring may not be enabled."
                        .to_string(),
                    String::new(),
                ));
            } else {
                rows.push(("Runtime Coverage".to_string(), String::new()));
                rows.extend(ov.coverage_rows.iter().cloned());
                rows.push((String::new(), String::new()));
                rows.push((
                    " Per-resource rows are on the Coverage sub-tab (3).".to_string(),
                    String::new(),
                ));
            }
            rows
        }
    }
}

// ── GuardDuty malware scan split pane ────────────────────────────────────────

pub(super) fn render_gd_malware_scan_split(app: &App, s: &GdMalwareScan, area: Rect, frame: &mut Frame) {
    render_simple_split(
        app,
        area,
        frame,
        "GuardDuty Malware Scan",
        &s.name,
        &if s.result.is_empty() {
            s.status.clone()
        } else {
            format!("{}  ·  {}", s.status, s.result)
        },
        &descriptor_tabs(
            app,
            &crate::aws::services::guardduty::GD_MALWARE_SCAN_SECTIONS,
        ),
    );
}

pub fn gd_malware_scan_section_lines(
    s: &GdMalwareScan,
    section: GdMalwareScanDetailSection,
) -> Vec<(String, String)> {
    match section {
        GdMalwareScanDetailSection::Overview => {
            let mut rows = vec![
                (String::new(), String::new()),
                ("Status".to_string(), s.status.clone()),
            ];
            if !s.result.is_empty() {
                rows.push((
                    "Result".to_string(),
                    if s.is_infected() {
                        format!("⚠ {}", s.result)
                    } else {
                        s.result.clone()
                    },
                ));
            }
            if !s.failure_reason.is_empty() {
                rows.push(("⚠ Failure".to_string(), s.failure_reason.clone()));
            }
            rows.push(("Scan Type".to_string(), s.scan_type.clone()));
            if let Some(t) = &s.started {
                rows.push(("Started".to_string(), t.clone()));
            }
            if let Some(t) = &s.ended {
                rows.push(("Ended".to_string(), t.clone()));
            }
            if s.file_count > 0 || s.total_bytes > 0 {
                rows.push(("Files Scanned".to_string(), s.file_count.to_string()));
                rows.push(("Bytes Scanned".to_string(), fmt_bytes(s.total_bytes)));
            }
            if !s.instance_arn.is_empty() {
                // Key-value so the generic ARN classifier makes it jumpable
                // back to the instance that was scanned.
                rows.push(("Instance".to_string(), s.instance_arn.clone()));
            }
            rows.push(("Account".to_string(), s.account_id.clone()));
            rows.push(("Scan ID".to_string(), s.scan_id.clone()));

            if !s.trigger_finding_id.is_empty()
                || !s.trigger_type.is_empty()
                || !s.trigger_description.is_empty()
            {
                rows.push((String::new(), String::new()));
                rows.push(("Trigger".to_string(), String::new()));
                if !s.trigger_type.is_empty() {
                    rows.push(("Type".to_string(), s.trigger_type.clone()));
                }
                if !s.trigger_finding_id.is_empty() {
                    // The finding that caused the scan — Enter jumps to it,
                    // since a GuardDuty finding id is its own resource id.
                    rows.push(("Finding".to_string(), s.trigger_finding_id.clone()));
                }
                if !s.trigger_description.is_empty() {
                    for line in wrap_plain(&s.trigger_description, 64) {
                        rows.push((format!("  {}", line), String::new()));
                    }
                }
            }
            rows
        }
        GdMalwareScanDetailSection::Volumes => {
            let mut rows = vec![(String::new(), String::new())];
            if s.volume_rows.is_empty() {
                rows.push((
                    " No attached-volume detail on this scan".to_string(),
                    String::new(),
                ));
            } else {
                rows.extend(s.volume_rows.iter().cloned());
            }
            rows
        }
    }
}

// ── GuardDuty member split pane ──────────────────────────────────────────────

pub(super) fn render_gd_member_split(app: &App, m: &GdMember, area: Rect, frame: &mut Frame) {
    render_simple_split(
        app,
        area,
        frame,
        "GuardDuty Member",
        &m.account_id,
        &m.relationship_status,
        &descriptor_tabs(app, &crate::aws::services::guardduty::GD_MEMBER_SECTIONS),
    );
}

pub fn gd_member_section_lines(
    m: &GdMember,
    section: GdMemberDetailSection,
) -> Vec<(String, String)> {
    match section {
        GdMemberDetailSection::Overview => {
            let mut rows = vec![
                (String::new(), String::new()),
                ("Account".to_string(), m.account_id.clone()),
                (
                    "Status".to_string(),
                    if m.is_enabled() {
                        m.relationship_status.clone()
                    } else {
                        // Not enabled means the account isn't being monitored
                        // at all — the one thing this tab exists to surface.
                        format!("⚠ {} (not monitored)", m.relationship_status)
                    },
                ),
            ];
            if !m.email.is_empty() {
                rows.push(("Email".to_string(), m.email.clone()));
            }
            if !m.detector_id.is_empty() {
                rows.push(("Detector".to_string(), m.detector_id.clone()));
            }
            if !m.invited_at.is_empty() {
                rows.push(("Invited".to_string(), m.invited_at.clone()));
            }
            if !m.updated_at.is_empty() {
                rows.push(("Updated".to_string(), m.updated_at.clone()));
            }
            if m.disabled_features > 0 {
                rows.push((
                    "Features Off".to_string(),
                    format!("⚠ {}", m.disabled_features),
                ));
            }
            rows
        }
        GdMemberDetailSection::Features => {
            let mut rows = vec![(String::new(), String::new())];
            if m.feature_rows.is_empty() {
                rows.push((
                    " No per-feature detail — GetMemberDetectors returned nothing for this account."
                        .to_string(),
                    String::new(),
                ));
            } else {
                for (name, status) in &m.feature_rows {
                    rows.push((
                        name.clone(),
                        if status == "ENABLED" {
                            status.clone()
                        } else {
                            format!("⚠ {}", status)
                        },
                    ));
                }
            }
            rows
        }
    }
}

// ── GuardDuty filter split pane ──────────────────────────────────────────────

pub(super) fn render_gd_filter_split(app: &App, f: &GdFilter, area: Rect, frame: &mut Frame) {
    render_simple_split(
        app,
        area,
        frame,
        "GuardDuty Filter",
        &f.name,
        if f.is_suppression() {
            "suppression rule"
        } else {
            "saved filter"
        },
        &descriptor_tabs(app, &crate::aws::services::guardduty::GD_FILTER_SECTIONS),
    );
}

pub fn gd_filter_section_lines(
    f: &GdFilter,
    section: GdFilterDetailSection,
) -> Vec<(String, String)> {
    match section {
        GdFilterDetailSection::Overview => {
            let mut rows = vec![
                (String::new(), String::new()),
                ("Name".to_string(), f.name.clone()),
                (
                    "Action".to_string(),
                    if f.is_suppression() {
                        "⚠ ARCHIVE (suppresses matching findings)".to_string()
                    } else {
                        "NOOP (saved filter — changes nothing)".to_string()
                    },
                ),
                ("Rank".to_string(), f.rank.to_string()),
                ("Conditions".to_string(), f.criteria_rows.len().to_string()),
            ];
            if !f.description.is_empty() {
                rows.push((String::new(), String::new()));
                rows.push(("Description".to_string(), String::new()));
                for line in wrap_plain(&f.description, 64) {
                    rows.push((format!("  {}", line), String::new()));
                }
            }
            if f.is_suppression() {
                rows.push((String::new(), String::new()));
                rows.push((
                    " Findings matching every condition below are archived on arrival."
                        .to_string(),
                    String::new(),
                ));
                rows.push((
                    " They still load here — press a to fold archived findings away."
                        .to_string(),
                    String::new(),
                ));
            }
            rows
        }
        GdFilterDetailSection::Criteria => {
            let mut rows = vec![(String::new(), String::new())];
            if f.criteria_rows.is_empty() {
                rows.push((" No conditions on this filter".to_string(), String::new()));
            } else {
                // All conditions are ANDed by GuardDuty; saying so beats
                // leaving the reader to guess from a flat list.
                rows.push((
                    format!("All {} must match", f.criteria_rows.len()),
                    String::new(),
                ));
                for (field, cond) in &f.criteria_rows {
                    rows.push((format!("  {}", field), cond.clone()));
                }
            }
            rows
        }
        GdFilterDetailSection::Tags => tag_rows(&f.tags),
    }
}

// ── GuardDuty finding split pane ────────────────────────────────────────────────

/// Colour for a GuardDuty severity label.
pub(super) fn gd_severity_color(label: &str) -> Color {
    match label {
        "Critical" | "High" => theme::error(),
        "Medium" => theme::warning(),
        _ => theme::text_dim(),
    }
}

pub(super) fn render_gd_finding_split(app: &App, finding: &GdFinding, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 6, "");
    let mut block = theme::pane_block("GuardDuty Finding", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let sev_color = gd_severity_color(&finding.severity_label);
    let header = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                finding.name().to_string(),
                Style::default().fg(crate::ui::theme::text_primary()).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("● ", Style::default().fg(sev_color)),
            Span::styled(
                format!("{} ({:.1})", finding.severity_label, finding.severity_score),
                Style::default().fg(sev_color).add_modifier(Modifier::BOLD),
            ),
            Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
            Span::styled(finding.finding_type.clone(), Style::default().fg(theme::text_dim())),
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
        &descriptor_tabs(app, &crate::aws::services::guardduty::GD_FINDING_SECTIONS),
    );
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub fn gd_finding_section_lines(
    finding: &GdFinding,
    section: GdFindingDetailSection,
) -> Vec<(String, String)> {
    match section {
        GdFindingDetailSection::Details => {
            let mut rows = vec![
                ("Title".to_string(), finding.title.clone()),
                ("Type".to_string(), finding.finding_type.clone()),
                (
                    "Severity".to_string(),
                    format!("{} ({:.1})", finding.severity_label, finding.severity_score),
                ),
                ("Count".to_string(), finding.count.to_string()),
            ];
            if finding.archived {
                // Suppression rules archive findings; saying so here is the
                // only way to tell a suppressed finding from a live one.
                rows.push(("Archived".to_string(), "⚠ yes (suppressed)".to_string()));
            }
            if let Some(f) = &finding.first_seen {
                rows.push(("First Seen".to_string(), f.clone()));
            }
            if let Some(l) = &finding.last_seen {
                rows.push(("Last Seen".to_string(), l.clone()));
            }
            if let Some(u) = &finding.updated_at {
                rows.push(("Updated".to_string(), u.clone()));
            }
            rows.push(("Account".to_string(), finding.account_id.clone()));
            rows.push(("Region".to_string(), finding.region.clone()));
            rows.push(("Finding ID".to_string(), finding.id.clone()));

            if !finding.description.is_empty() {
                rows.push((String::new(), String::new()));
                rows.push(("Description".to_string(), String::new())); // group header
                for line in wrap_plain(&finding.description, 64) {
                    rows.push((format!("  {}", line), String::new()));
                }
            }

            // The type string is a structured identifier, not a name — spelling
            // out its parts is how you learn to read the next one.
            let parts = crate::aws::services::guardduty::decompose_finding_type(
                &finding.finding_type,
            );
            if !parts.is_empty() {
                rows.push((String::new(), String::new()));
                rows.push(("Finding Type".to_string(), String::new()));
                rows.extend(parts);
            }

            let mut classification: Vec<(String, String)> = Vec::new();
            if !finding.resource_role.is_empty() {
                // TARGET vs ACTOR decides whether the affected resource was
                // attacked or did the attacking.
                classification.push(("Resource Role".to_string(), finding.resource_role.clone()));
            }
            if !finding.feature_name.is_empty() {
                classification.push(("Detected By".to_string(), finding.feature_name.clone()));
            }
            if !finding.user_feedback.is_empty() {
                classification.push(("Analyst Feedback".to_string(), finding.user_feedback.clone()));
            }
            classification.extend(finding.additional_info.iter().cloned());
            if !classification.is_empty() {
                rows.push((String::new(), String::new()));
                rows.push(("Classification".to_string(), String::new()));
                rows.extend(classification);
            }

            if !finding.threat_intel.is_empty() {
                rows.push((String::new(), String::new()));
                rows.push(("Threat Intelligence".to_string(), String::new()));
                for (list, names) in &finding.threat_intel {
                    rows.push((list.clone(), names.clone()));
                }
            }
            rows
        }
        GdFindingDetailSection::Resource => {
            let mut rows = vec![("Resource Type".to_string(), finding.resource_type.clone())];
            if finding.resource_rows.is_empty() {
                rows.push((String::new(), String::new()));
                rows.push((" No resource detail on this finding".to_string(), String::new()));
            } else {
                rows.push((String::new(), String::new()));
                rows.extend(finding.resource_rows.iter().cloned());
            }
            rows
        }
        GdFindingDetailSection::Actor => {
            if finding.actor_rows.is_empty() {
                vec![(" No actor / connection detail on this finding".to_string(), String::new())]
            } else {
                // Lead with the one-line "who" (remote IP, domain or API call),
                // mirroring how Resource leads with its type.
                let mut rows = Vec::new();
                if !finding.actor_label.is_empty() {
                    rows.push(("Actor".to_string(), finding.actor_label.clone()));
                    rows.push((String::new(), String::new()));
                }
                rows.extend(finding.actor_rows.iter().cloned());
                rows
            }
        }
        GdFindingDetailSection::Sequence => {
            if finding.sequence_rows.is_empty() {
                vec![(
                    " Not an attack-sequence finding — Extended Threat Detection correlates \
                     several signals into one sequence when it spots a multi-step attack."
                        .to_string(),
                    String::new(),
                )]
            } else {
                finding.sequence_rows.clone()
            }
        }
        GdFindingDetailSection::Runtime => {
            if finding.runtime_rows.is_empty() {
                vec![(
                    " No runtime process detail — this finding didn't come from Runtime \
                     Monitoring (see the Coverage tab for which hosts run the agent)."
                        .to_string(),
                    String::new(),
                )]
            } else {
                finding.runtime_rows.clone()
            }
        }
        GdFindingDetailSection::Remediation => gd_remediation_lines(finding),
    }
}

/// GuardDuty's API returns no remediation field; synthesise threat-purpose
/// guidance from the finding type plus a pointer to the docs/console.
pub(super) fn gd_remediation_lines(finding: &GdFinding) -> Vec<(String, String)> {
    // Finding type shape: ThreatPurpose:ResourceType/ThreatFamilyName...
    let purpose = finding.finding_type.split(':').next().unwrap_or("");
    let guidance = match purpose {
        "UnauthorizedAccess" => {
            "Credentials or a resource may be accessed by an unauthorized party. Rotate exposed credentials, review IAM permissions, and restrict network access to the affected resource."
        }
        "Recon" => {
            "An actor is probing your environment. Confirm the source is expected; if not, tighten security-group / NACL rules and block the remote IP."
        }
        "CryptoCurrency" => {
            "A resource may be mining cryptocurrency (often post-compromise). Isolate the instance, investigate for compromise, and rotate any credentials it held."
        }
        "Trojan" | "Backdoor" | "Persistence" => {
            "A resource may be compromised and communicating with a C2 host. Isolate it, snapshot for forensics, and rebuild from a known-good image."
        }
        "PrivilegeEscalation" | "Discovery" | "CredentialAccess" | "DefenseEvasion"
        | "Execution" | "Impact" | "Exfiltration" | "InitialAccess" | "Policy" => {
            "Suspicious API activity detected. Review CloudTrail for the principal, revoke/rotate its credentials, and confirm the actions were intended."
        }
        "PenTest" => {
            "Activity attributed to a known pen-testing tool. Confirm it's an authorized test; otherwise treat as an intrusion."
        }
        _ => {
            "Investigate the affected resource and the actor below. Confirm whether the activity was authorized."
        }
    };

    let mut rows = vec![("Suggested Action".to_string(), String::new())]; // group header
    for line in wrap_plain(guidance, 64) {
        rows.push((format!("  {}", line), String::new()));
    }
    rows.push((String::new(), String::new()));
    rows.push(("Reference".to_string(), String::new())); // group header
    rows.push((
        "  Finding types".to_string(),
        "https://docs.aws.amazon.com/guardduty/latest/ug/guardduty_finding-types-active.html"
            .to_string(),
    ));
    rows.push(("  Suppress / archive".to_string(), "via the GuardDuty console".to_string()));
    rows
}
