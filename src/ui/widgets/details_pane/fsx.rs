use super::*;

// ── FSx File System split pane ─────────────────────────────────────────────────

pub fn fsx_section_lines(
    fs: &crate::aws::services::fsx::FsxFileSystem,
    section: FsxDetailSection,
    volumes: Option<&crate::lazy::Lazy<Vec<crate::aws::services::fsx::FsxVolume>>>,
    backups: Option<&crate::lazy::Lazy<Vec<crate::aws::services::fsx::FsxBackup>>>,
) -> Vec<(String, String)> {
    use crate::aws::services::fsx::FsxConfig;
    match section {
        FsxDetailSection::Volumes => fsx_volumes_lines(volumes),
        FsxDetailSection::Backups => fsx_backups_lines(backups),
        FsxDetailSection::Details => {
            let mut rows = vec![
                ("File System ID".to_string(), fs.file_system_id.clone()),
                ("ARN".to_string(), fs.arn.clone()),
                ("Type".to_string(), fs.file_system_type.clone()),
                ("Lifecycle".to_string(), fs.lifecycle.clone()),
            ];
            if !fs.failure_details.is_empty() {
                rows.push(("Failure Reason".to_string(), fs.failure_details.clone()));
            }
            rows.push(("Storage Capacity".to_string(), format!("{} GiB", fs.storage_capacity_gib)));
            rows.push(("Storage Type".to_string(), fs.storage_type.clone()));
            let tp = fs.throughput_mbps();
            if tp > 0 {
                rows.push(("Throughput".to_string(), format!("{} MB/s", tp)));
            }
            if !fs.dns_name.is_empty() {
                rows.push(("DNS Name".to_string(), fs.dns_name.clone()));
            }
            if !fs.kms_key_id.is_empty() {
                rows.push(("KMS Key".to_string(), fs.kms_key_id.clone()));
            }
            rows.push(("Created".to_string(), fs.creation_time.clone()));
            rows
        }
        FsxDetailSection::Network => {
            let mut rows = vec![];
            if !fs.vpc_id.is_empty() {
                rows.push(("VPC".to_string(), fs.vpc_id.clone()));
            }
            if !fs.subnet_ids.is_empty() {
                rows.push(("".to_string(), "".to_string()));
                rows.push(("Subnets".to_string(), "".to_string()));
                for s in &fs.subnet_ids {
                    rows.push(("  ".to_string(), s.clone()));
                }
            }
            if !fs.network_interface_ids.is_empty() {
                rows.push(("".to_string(), "".to_string()));
                rows.push(("Network Interfaces".to_string(), "".to_string()));
                for eni in &fs.network_interface_ids {
                    rows.push(("  ".to_string(), eni.clone()));
                }
            }
            // Type-specific network info
            match &fs.config {
                FsxConfig::Windows(w) => {
                    if !w.preferred_file_server_ip.is_empty() {
                        rows.push(("".to_string(), "".to_string()));
                        rows.push(("Preferred File Server IP".to_string(), w.preferred_file_server_ip.clone()));
                    }
                }
                FsxConfig::Ontap(o)
                    if !o.endpoint_ip_address_range.is_empty() => {
                        rows.push(("".to_string(), "".to_string()));
                        rows.push(("Endpoint IP Range".to_string(), o.endpoint_ip_address_range.clone()));
                    }
                _ => {}
            }
            rows
        }
        FsxDetailSection::Configuration => {
            match &fs.config {
                FsxConfig::Windows(w) => vec![
                    ("Windows Configuration".to_string(), "".to_string()),
                    ("".to_string(), "".to_string()),
                    ("Deployment Type".to_string(), w.deployment_type.clone()),
                    ("Throughput".to_string(), format!("{} MB/s", w.throughput_capacity_mbps)),
                    ("Active Directory".to_string(), w.active_directory_id.clone()),
                    ("Preferred Server IP".to_string(), w.preferred_file_server_ip.clone()),
                    ("Backup Retention".to_string(), format!("{} days", w.automatic_backup_retention_days)),
                ],
                FsxConfig::Lustre(l) => {
                    let mut rows = vec![
                        ("Lustre Configuration".to_string(), "".to_string()),
                        ("".to_string(), "".to_string()),
                        ("Deployment Type".to_string(), l.deployment_type.clone()),
                    ];
                    if l.per_unit_storage_throughput > 0 {
                        rows.push(("Per-Unit Throughput".to_string(), format!("{} MB/s/TiB", l.per_unit_storage_throughput)));
                    }
                    if !l.mount_name.is_empty() {
                        rows.push(("Mount Name".to_string(), l.mount_name.clone()));
                    }
                    if l.data_repo_count > 0 {
                        rows.push(("Data Repo Associations".to_string(), l.data_repo_count.to_string()));
                    }
                    rows
                }
                FsxConfig::Ontap(o) => vec![
                    ("ONTAP Configuration".to_string(), "".to_string()),
                    ("".to_string(), "".to_string()),
                    ("Deployment Type".to_string(), o.deployment_type.clone()),
                    ("Throughput".to_string(), format!("{} MB/s", o.throughput_capacity_mbps)),
                    ("Endpoint IP Range".to_string(), o.endpoint_ip_address_range.clone()),
                    ("".to_string(), "".to_string()),
                    ("  See the Volumes section (2) for this file system's volumes".to_string(), "".to_string()),
                ],
                FsxConfig::OpenZfs(z) => vec![
                    ("OpenZFS Configuration".to_string(), "".to_string()),
                    ("".to_string(), "".to_string()),
                    ("Deployment Type".to_string(), z.deployment_type.clone()),
                    ("Throughput".to_string(), format!("{} MB/s", z.throughput_capacity_mbps)),
                    ("Root Volume ID".to_string(), z.root_volume_id.clone()),
                    ("Copy Tags to Backups".to_string(), if z.copy_tags_to_backups { "Yes" } else { "No" }.to_string()),
                ],
                FsxConfig::Unknown => vec![
                    ("".to_string(), "Unknown file system type".to_string()),
                ],
            }
        }
        FsxDetailSection::Tags => tag_rows(&fs.tags),
    }
}

/// Detail body for a selected FSx **volume** resource (Volumes sub-tab):
/// describe info + a live snapshot (used/utilization/latency) fetched lazily.
pub fn fsx_volume_section_lines(
    vol: &crate::aws::services::fsx::FsxVolume,
    section: FsxVolumeDetailSection,
    detail: Option<&crate::lazy::Lazy<crate::aws::services::fsx::FsxVolumeSnapshot>>,
) -> Vec<(String, String)> {
    use crate::aws::services::efs::fmt_bytes;

    match section {
        FsxVolumeDetailSection::Overview => {
            let mut rows = vec![
                (String::new(), String::new()),
                ("Volume ID".to_string(), vol.volume_id.clone()),
                ("Name".to_string(), vol.name.clone()),
                ("File System".to_string(), vol.file_system_id.clone()),
                ("Type".to_string(), vol.volume_type.clone()),
                ("Lifecycle".to_string(), vol.lifecycle.clone()),
            ];
            if vol.size_bytes > 0 {
                rows.push(("Size".to_string(), fmt_bytes(vol.size_bytes)));
            }
            if !vol.path.is_empty() {
                rows.push(("Path".to_string(), vol.path.clone()));
            }
            if !vol.svm_id.is_empty() {
                rows.push(("SVM".to_string(), vol.svm_id.clone()));
            }
            for note in &vol.notes {
                rows.push(("  ".to_string(), note.clone()));
            }
            rows
        }
        FsxVolumeDetailSection::Live => {
            let mut rows = vec![(String::new(), String::new())];
            match detail {
                None | Some(crate::lazy::Lazy::Loading) => {
                    rows.push(("  Loading…".to_string(), "".to_string()));
                }
                Some(crate::lazy::Lazy::Loaded(s)) => {
                    match s.used_bytes {
                        Some(u) => {
                            let of = if vol.size_bytes > 0 {
                                format!(" / {}", fmt_bytes(vol.size_bytes))
                            } else {
                                String::new()
                            };
                            rows.push(("Used".to_string(), format!("{}{}", fmt_bytes(u), of)));
                        }
                        None => rows.push(("Used".to_string(), "—".to_string())),
                    }
                    if let Some(pct) = s.utilization_pct {
                        let marker = if pct >= 90.0 {
                            "✗ "
                        } else if pct >= 75.0 {
                            "⚠ "
                        } else {
                            ""
                        };
                        rows.push((
                            "Utilization".to_string(),
                            format!("{}{:.1}%", marker, pct),
                        ));
                    }
                    let lat =
                        |label: &str, v: Option<f64>, rows: &mut Vec<(String, String)>| {
                            if let Some(ms) = v {
                                rows.push((label.to_string(), format!("{:.2} ms", ms)));
                            }
                        };
                    lat("Read Latency", s.read_latency_ms, &mut rows);
                    lat("Write Latency", s.write_latency_ms, &mut rows);
                    lat("Metadata Latency", s.metadata_latency_ms, &mut rows);
                }
                Some(crate::lazy::Lazy::Error(e)) => {
                    rows.extend(error_rows(e));
                }
            }
            rows.push((String::new(), String::new()));
            rows.push((
                "  press m for usage / latency / tier graphs".to_string(),
                String::new(),
            ));
            rows
        }
        FsxVolumeDetailSection::Tags => tag_rows(&vol.tags),
    }
}

pub(super) fn fsx_volumes_lines(
    state: Option<&crate::lazy::Lazy<Vec<crate::aws::services::fsx::FsxVolume>>>,
) -> Vec<(String, String)> {
    use crate::aws::services::efs::fmt_bytes;
    let mut rows = vec![("".to_string(), "".to_string())];
    match state {
        None | Some(crate::lazy::Lazy::Loading) => {
            rows.push(("  Loading volumes…".to_string(), "".to_string()));
        }
        Some(crate::lazy::Lazy::Error(e)) => {
            rows.extend(error_rows(e));
        }
        Some(crate::lazy::Lazy::Loaded(vols)) if vols.is_empty() => {
            rows.push((
                "  No volumes (Windows/Lustre file systems have none)".to_string(),
                "".to_string(),
            ));
        }
        Some(crate::lazy::Lazy::Loaded(vols)) => {
            for (i, v) in vols.iter().enumerate() {
                if i > 0 {
                    rows.push(("".to_string(), "".to_string()));
                }
                // Group header: name + type.
                let title = if v.name.is_empty() { v.volume_id.clone() } else { v.name.clone() };
                rows.push((format!("{}  ({})", title, v.volume_type), String::new()));
                rows.push(("Volume ID".to_string(), v.volume_id.clone()));
                rows.push(("Lifecycle".to_string(), v.lifecycle.clone()));
                if v.size_bytes > 0 {
                    rows.push(("Size".to_string(), fmt_bytes(v.size_bytes)));
                }
                // Live usage (CloudWatch). Utilization gets a ⚠/✗ prefix so the
                // row colours (yellow ≥75%, red ≥90%) — spot the full volume fast.
                if let Some(used) = v.used_bytes {
                    let of = if v.size_bytes > 0 {
                        format!(" / {}", fmt_bytes(v.size_bytes))
                    } else {
                        String::new()
                    };
                    rows.push(("Used".to_string(), format!("{}{}", fmt_bytes(used), of)));
                }
                if let Some(pct) = v.utilization_pct {
                    let marker = if pct >= 90.0 {
                        "✗ "
                    } else if pct >= 75.0 {
                        "⚠ "
                    } else {
                        ""
                    };
                    rows.push(("Utilization".to_string(), format!("{}{:.1}%", marker, pct)));
                }
                if !v.path.is_empty() {
                    rows.push(("Path".to_string(), v.path.clone()));
                }
                if !v.svm_id.is_empty() {
                    rows.push(("SVM".to_string(), v.svm_id.clone()));
                }
                for note in &v.notes {
                    rows.push(("  ".to_string(), note.clone()));
                }
            }
        }
    }
    rows.push(("".to_string(), "".to_string()));
    rows
}

pub(super) fn fsx_backups_lines(
    state: Option<&crate::lazy::Lazy<Vec<crate::aws::services::fsx::FsxBackup>>>,
) -> Vec<(String, String)> {
    let mut rows = vec![("".to_string(), "".to_string())];
    match state {
        None | Some(crate::lazy::Lazy::Loading) => {
            rows.push(("  Loading backups…".to_string(), "".to_string()));
        }
        Some(crate::lazy::Lazy::Error(e)) => {
            rows.extend(error_rows(e));
        }
        Some(crate::lazy::Lazy::Loaded(bks)) if bks.is_empty() => {
            rows.push(("  No backups".to_string(), "".to_string()));
        }
        Some(crate::lazy::Lazy::Loaded(bks)) => {
            rows.push(("Backups".to_string(), format!("{} total", bks.len())));
            rows.push(("".to_string(), "".to_string()));
            for (i, b) in bks.iter().enumerate() {
                if i > 0 {
                    rows.push(("".to_string(), "".to_string()));
                }
                rows.push((format!("{}  ({})", b.backup_id, b.backup_type), String::new()));
                rows.push(("Lifecycle".to_string(), b.lifecycle.clone()));
                if b.lifecycle == "CREATING" || b.lifecycle == "PENDING" {
                    rows.push(("Progress".to_string(), format!("{}%", b.progress_percent)));
                }
                if !b.volume_id.is_empty() {
                    rows.push(("Volume".to_string(), b.volume_id.clone()));
                }
                rows.push(("Created".to_string(), b.creation_time.clone()));
            }
        }
    }
    rows.push(("".to_string(), "".to_string()));
    rows
}

pub(super) fn render_fsx_split(
    app: &App,
    fs: &crate::aws::services::fsx::FsxFileSystem,
    area: ratatui::layout::Rect,
    frame: &mut Frame,
) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 6, "");
    let mut block = theme::pane_block("FSx File System", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let header_lines = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                fs.fs_name.clone(),
                Style::default()
                    .fg(crate::ui::theme::text_primary())
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                format!("{} · {} · {} GiB {}", fs.file_system_id, fs.file_system_type, fs.storage_capacity_gib, fs.storage_type),
                Style::default().fg(theme::text_dim()),
            ),
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

    let sections = [
        ('1', "Details", FsxDetailSection::Details),
        ('2', "Volumes", FsxDetailSection::Volumes),
        ('3', "Backups", FsxDetailSection::Backups),
        ('4', "Network", FsxDetailSection::Network),
        ('5', "Configuration", FsxDetailSection::Configuration),
        ('6', "Tags", FsxDetailSection::Tags),
    ];
    let mut spans: Vec<Span> = vec![Span::raw("  ")];
    for (i, (key, label, section)) in sections.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("   │   ", Style::default().fg(theme::text_dim())));
        }
        let active = FsxDetailSection::from_index(app.detail_section_index()) == *section;
        if active {
            spans.push(Span::styled(
                key.to_string(),
                Style::default()
                    .fg(theme::aws_orange())
                    .add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::styled(
                format!(" {} ", label),
                Style::default()
                    .fg(Color::Black)
                    .bg(theme::aws_orange())
                    .add_modifier(Modifier::BOLD),
            ));
        } else {
            spans.push(Span::styled(
                key.to_string(),
                Style::default().fg(theme::text_dim()),
            ));
            spans.push(Span::styled(
                format!(" {} ", label),
                Style::default().fg(crate::ui::theme::text_muted()),
            ));
        }
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), chunks[2]);
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub(super) fn render_fsx_volume_split(
    app: &App,
    vol: &crate::aws::services::fsx::FsxVolume,
    area: Rect,
    frame: &mut Frame,
) {
    let name = if vol.name.is_empty() {
        &vol.volume_id
    } else {
        &vol.name
    };
    render_simple_split(
        app,
        area,
        frame,
        "FSx Volume",
        name,
        &vol.volume_type,
        &descriptor_tabs(app, &crate::aws::services::fsx::FSX_VOLUME_SECTIONS),
    );
}
