use super::*;

// ── EFS file system split pane ─────────────────────────────────────────────────

pub(super) fn render_efs_fs_split(app: &App, fs: &EfsFileSystem, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 4, "");
    let mut block = theme::pane_block("EFS File System", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let state_color = match fs.lifecycle_state.as_str() {
        "available" => theme::success(),
        "creating" | "updating" => theme::warning(),
        "error" => theme::error(),
        _ => theme::text_dim(),
    };
    let header = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                fs.fs_name.clone(),
                Style::default().fg(crate::ui::theme::text_primary()).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(fs.file_system_id.clone(), Style::default().fg(theme::text_dim())),
            Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
            Span::styled(fs.lifecycle_state.clone(), Style::default().fg(state_color)),
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
    render_section_tab_bar(
        app,
        chunks[2],
        frame,
        &descriptor_tabs(app, &crate::aws::services::efs::EFS_FS_SECTIONS),
    );
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub fn efs_fs_section_lines(
    fs: &EfsFileSystem,
    section: EfsFileSystemDetailSection,
    mount_targets: Option<&Lazy<Vec<crate::aws::services::efs::EfsMountTarget>>>,
    access_points: Option<&Lazy<Vec<crate::aws::services::efs::EfsAccessPoint>>>,
) -> Vec<(String, String)> {
    use crate::aws::services::efs::fmt_bytes;
    match section {
        EfsFileSystemDetailSection::Details => {
            let mut rows = vec![
                ("Name".to_string(), fs.fs_name.clone()),
                ("File System ID".to_string(), fs.file_system_id.clone()),
                ("State".to_string(), fs.lifecycle_state.clone()),
                ("Storage Class".to_string(), fs.storage_class().to_string()),
            ];
            if let Some(az) = &fs.availability_zone_name {
                rows.push(("  Availability Zone".to_string(), az.clone()));
            }
            rows.push((String::new(), String::new()));
            rows.push(("Performance".to_string(), String::new())); // group header
            rows.push(("  Performance Mode".to_string(), fs.performance_mode.clone()));
            rows.push(("  Throughput Mode".to_string(), fs.throughput_summary()));
            rows.push((String::new(), String::new()));
            rows.push(("Storage".to_string(), String::new())); // group header
            rows.push(("  Total Size".to_string(), fmt_bytes(fs.size_total)));
            rows.push(("  Standard".to_string(), fmt_bytes(fs.size_standard)));
            rows.push(("  Infrequent Access".to_string(), fmt_bytes(fs.size_ia)));
            if fs.size_archive > 0 {
                rows.push(("  Archive".to_string(), fmt_bytes(fs.size_archive)));
            }
            rows.push((String::new(), String::new()));
            rows.push(("Lifecycle".to_string(), String::new())); // group header
            if fs.lifecycle.is_empty() {
                rows.push(("  Policy".to_string(), "None".to_string()));
            } else {
                if let Some(v) = &fs.lifecycle.to_ia {
                    rows.push(("  → IA".to_string(), v.clone()));
                }
                if let Some(v) = &fs.lifecycle.to_archive {
                    rows.push(("  → Archive".to_string(), v.clone()));
                }
                if let Some(v) = &fs.lifecycle.to_primary {
                    rows.push(("  → Primary".to_string(), v.clone()));
                }
            }
            rows.push((String::new(), String::new()));
            rows.push(("Other".to_string(), String::new())); // group header
            rows.push((
                "  Encryption".to_string(),
                if fs.encrypted {
                    "✓ Encrypted".to_string()
                } else {
                    "✗ Not encrypted".to_string()
                },
            ));
            if let Some(kms) = &fs.kms_key_id {
                rows.push(("  KMS Key".to_string(), kms.clone()));
            }
            rows.push((
                "  Mount Targets".to_string(),
                fs.number_of_mount_targets.to_string(),
            ));
            if let Some(c) = &fs.created {
                rows.push(("  Created".to_string(), c.clone()));
            }
            rows.push(("  ARN".to_string(), fs.arn.clone()));
            rows
        }
        EfsFileSystemDetailSection::MountTargets => match mount_targets {
            None | Some(Lazy::Loading) => {
                vec![("".to_string(), "Loading mount targets…".to_string())]
            }
            Some(Lazy::Error(e)) => error_rows(e),
            Some(Lazy::Loaded(mts)) => {
                if mts.is_empty() {
                    return vec![(
                        "".to_string(),
                        "No mount targets — the file system is unreachable until one exists"
                            .to_string(),
                    )];
                }
                let mut rows = Vec::new();
                for mt in mts {
                    rows.push((mt.availability_zone.clone(), String::new())); // group header
                    rows.push(("  Mount Target".to_string(), mt.mount_target_id.clone()));
                    rows.push(("  State".to_string(), mt.lifecycle_state.clone()));
                    rows.push(("  IP Address".to_string(), mt.ip_address.clone()));
                    rows.push(("  Subnet".to_string(), mt.subnet_id.clone()));
                    if !mt.network_interface_id.is_empty() {
                        rows.push(("  ENI".to_string(), mt.network_interface_id.clone()));
                    }
                    rows.push((String::new(), String::new()));
                }
                rows
            }
        },
        EfsFileSystemDetailSection::AccessPoints => match access_points {
            None | Some(Lazy::Loading) => {
                vec![("".to_string(), "Loading access points…".to_string())]
            }
            Some(Lazy::Error(e)) => error_rows(e),
            Some(Lazy::Loaded(aps)) => {
                if aps.is_empty() {
                    return vec![("".to_string(), "No access points".to_string())];
                }
                let mut rows = Vec::new();
                for ap in aps {
                    let label = if ap.name.is_empty() {
                        ap.access_point_id.clone()
                    } else {
                        ap.name.clone()
                    };
                    rows.push((label, String::new())); // group header
                    rows.push(("  Access Point".to_string(), ap.access_point_id.clone()));
                    rows.push(("  State".to_string(), ap.lifecycle_state.clone()));
                    rows.push(("  Root Directory".to_string(), ap.root_directory.clone()));
                    if let Some(u) = &ap.posix_user {
                        rows.push(("  POSIX User (uid:gid)".to_string(), u.clone()));
                    }
                    rows.push((String::new(), String::new()));
                }
                rows
            }
        },
        EfsFileSystemDetailSection::Tags => tag_rows(&fs.tags),
    }
}
