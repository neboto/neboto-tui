use super::*;

// ── S3 Files split pane ──────────────────────────────────────────────────────

pub(super) fn render_s3files_fs_split(
    app: &App,
    fs: &crate::aws::services::s3files::S3FileSystem,
    area: Rect,
    frame: &mut Frame,
) {
    let status_color = match fs.status.as_str() {
        "available" => theme::success(),
        "creating" | "updating" => theme::warning(),
        "error" | "deleting" => theme::error(),
        _ => theme::text_dim(),
    };
    let mut sub = vec![Span::styled(
        fs.status.clone(),
        Style::default().fg(status_color).add_modifier(Modifier::BOLD),
    )];
    sub.push(Span::styled("  ·  ", Style::default().fg(theme::text_dim())));
    sub.push(Span::styled(
        format!("s3://{}", fs.bucket_name),
        Style::default().fg(theme::text_dim()),
    ));
    let header = sc_header(&fs.name, sub);
    render_sc_split(
        app,
        "S3 File System",
        header,
        &descriptor_tabs(app, &crate::aws::services::s3files::S3FS_SECTIONS),
        "",
        area,
        frame,
    );
}

pub fn s3files_fs_section_lines(
    fs: &crate::aws::services::s3files::S3FileSystem,
    section: crate::aws::services::s3files::S3FsDetailSection,
    extras: Option<&crate::lazy::Lazy<crate::aws::services::s3files::S3FsExtras>>,
    mount_targets: Option<&crate::lazy::Lazy<Vec<crate::aws::services::s3files::S3FsMountTarget>>>,
    access_points: Option<&crate::lazy::Lazy<Vec<crate::aws::services::s3files::S3FsAccessPoint>>>,
    policy: Option<&crate::lazy::Lazy<Option<String>>>,
    sync: Option<&crate::lazy::Lazy<crate::aws::services::s3files::S3FsSyncConfig>>,
) -> Vec<(String, String)> {
    use crate::aws::services::s3files::S3FsDetailSection as S;
    match section {
        S::Details => {
            let mut rows = vec![
                ("Name".to_string(), fs.name.clone()),
                ("ID".to_string(), fs.id.clone()),
                ("Status".to_string(), fs.status.clone()),
            ];
            if !fs.status_message.is_empty() {
                rows.push(("Status Message".to_string(), fs.status_message.clone()));
            }
            if let Some(c) = &fs.created {
                rows.push(("Created".to_string(), c.clone()));
            }
            rows.push((String::new(), String::new()));
            rows.push(("Linked Bucket".to_string(), String::new()));
            // The bucket ARN row Enter-jumps to the S3 bucket.
            rows.push(("  Bucket".to_string(), fs.bucket_arn.clone()));
            match extras {
                None | Some(crate::lazy::Lazy::Loading) => {
                    rows.push(("  Prefix".to_string(), "Loading…".to_string()));
                }
                Some(crate::lazy::Lazy::Error(e)) => rows.extend(error_rows(e)),
                Some(crate::lazy::Lazy::Loaded(x)) => {
                    rows.push((
                        "  Prefix".to_string(),
                        if x.prefix.is_empty() {
                            "(whole bucket)".to_string()
                        } else {
                            x.prefix.clone()
                        },
                    ));
                    if !x.kms_key_id.is_empty() {
                        rows.push(("  KMS Key".to_string(), x.kms_key_id.clone()));
                    }
                }
            }
            rows.push((String::new(), String::new()));
            if !fs.role_arn.is_empty() {
                rows.push(("S3 Access Role".to_string(), fs.role_arn.clone()));
            }
            if !fs.owner_id.is_empty() {
                rows.push(("Owner Account".to_string(), fs.owner_id.clone()));
            }
            rows.push(("ARN".to_string(), fs.arn.clone()));
            rows
        }
        S::MountTargets => match mount_targets {
            None | Some(crate::lazy::Lazy::Loading) => {
                vec![("".to_string(), "Loading mount targets…".to_string())]
            }
            Some(crate::lazy::Lazy::Error(e)) => error_rows(e),
            Some(crate::lazy::Lazy::Loaded(list)) => {
                if list.is_empty() {
                    return vec![
                        ("".to_string(), "No mount targets".to_string()),
                        (String::new(), String::new()),
                        (
                            "  · a mount target per AZ gives compute a local NFS path to this file system"
                                .to_string(),
                            String::new(),
                        ),
                    ];
                }
                let mut rows = vec![(format!("Mount Targets ({})", list.len()), String::new())];
                rows.push((String::new(), String::new()));
                for m in list {
                    rows.push((m.id.clone(), String::new()));
                    rows.push(("  Status".to_string(), m.status.clone()));
                    if !m.status_message.is_empty() {
                        rows.push(("  Status Message".to_string(), m.status_message.clone()));
                    }
                    if !m.az_id.is_empty() {
                        rows.push(("  AZ".to_string(), m.az_id.clone()));
                    }
                    // subnet-/vpc-/eni- ids all Enter-jump via the prefix
                    // classifiers.
                    rows.push(("  Subnet".to_string(), m.subnet_id.clone()));
                    if !m.vpc_id.is_empty() {
                        rows.push(("  VPC".to_string(), m.vpc_id.clone()));
                    }
                    if !m.ipv4.is_empty() {
                        rows.push(("  IPv4".to_string(), m.ipv4.clone()));
                    }
                    if !m.ipv6.is_empty() {
                        rows.push(("  IPv6".to_string(), m.ipv6.clone()));
                    }
                    if !m.eni_id.is_empty() {
                        rows.push(("  ENI".to_string(), m.eni_id.clone()));
                    }
                    rows.push((String::new(), String::new()));
                }
                rows
            }
        },
        S::AccessPoints => match access_points {
            None | Some(crate::lazy::Lazy::Loading) => {
                vec![("".to_string(), "Loading access points…".to_string())]
            }
            Some(crate::lazy::Lazy::Error(e)) => error_rows(e),
            Some(crate::lazy::Lazy::Loaded(list)) => {
                if list.is_empty() {
                    return vec![("".to_string(), "No access points".to_string())];
                }
                let capped = list.last().is_some_and(|a| a.capped);
                let mut rows = vec![(format!("Access Points ({})", list.len()), String::new())];
                rows.push((String::new(), String::new()));
                for a in list {
                    let label = if a.name.is_empty() { a.id.clone() } else { a.name.clone() };
                    rows.push((label, String::new()));
                    if !a.name.is_empty() {
                        rows.push(("  ID".to_string(), a.id.clone()));
                    }
                    rows.push(("  Status".to_string(), a.status.clone()));
                    if !a.posix_user.is_empty() {
                        rows.push(("  POSIX User".to_string(), a.posix_user.clone()));
                    }
                    if !a.root_path.is_empty() {
                        rows.push(("  Root Directory".to_string(), a.root_path.clone()));
                    }
                    rows.push((String::new(), String::new()));
                }
                if capped {
                    rows.push((
                        "    · list truncated — a file system can hold up to 25k access points"
                            .to_string(),
                        String::new(),
                    ));
                }
                rows
            }
        },
        S::Sync => match sync {
            None | Some(crate::lazy::Lazy::Loading) => {
                vec![("".to_string(), "Loading sync configuration…".to_string())]
            }
            Some(crate::lazy::Lazy::Error(e)) => error_rows(e),
            Some(crate::lazy::Lazy::Loaded(cfg)) => {
                let mut rows = Vec::new();
                if let Some(v) = cfg.version {
                    rows.push(("Configuration Version".to_string(), v.to_string()));
                    rows.push((String::new(), String::new()));
                }
                rows.push(("Import Rules".to_string(), String::new()));
                if cfg.import_rules.is_empty() {
                    rows.push(("  (none — defaults apply)".to_string(), String::new()));
                } else {
                    for (prefix, trigger, size) in &cfg.import_rules {
                        rows.push((
                            format!("  {}", if prefix.is_empty() { "(all)" } else { prefix }),
                            format!("{} · files < {}", trigger, fmt_bytes(*size)),
                        ));
                    }
                }
                rows.push((String::new(), String::new()));
                rows.push(("Expiration Rules".to_string(), String::new()));
                if cfg.expiration_rules.is_empty() {
                    rows.push(("  (none — defaults apply)".to_string(), String::new()));
                } else {
                    for days in &cfg.expiration_rules {
                        rows.push((
                            "  Expire After".to_string(),
                            format!("{} days without access", days),
                        ));
                    }
                }
                rows
            }
        },
        S::Policy => match policy {
            None | Some(crate::lazy::Lazy::Loading) => {
                vec![("".to_string(), "Loading policy…".to_string())]
            }
            Some(crate::lazy::Lazy::Error(e)) => error_rows(e),
            Some(crate::lazy::Lazy::Loaded(None)) => {
                vec![("".to_string(), "No file system policy attached".to_string())]
            }
            Some(crate::lazy::Lazy::Loaded(Some(doc))) => {
                let pretty = serde_json::from_str::<serde_json::Value>(doc)
                    .and_then(|v| serde_json::to_string_pretty(&v))
                    .unwrap_or_else(|_| doc.clone());
                let mut rows = vec![("File System Policy".to_string(), String::new())];
                rows.push((String::new(), String::new()));
                for line in pretty.lines() {
                    rows.push((format!("  {}", line), String::new()));
                }
                rows
            }
        },
        S::Tags => match extras {
            None | Some(crate::lazy::Lazy::Loading) => {
                vec![("".to_string(), "Loading tags…".to_string())]
            }
            Some(crate::lazy::Lazy::Error(e)) => error_rows(e),
            Some(crate::lazy::Lazy::Loaded(x)) => {
                if x.tags.is_empty() {
                    return vec![("  (no tags)".to_string(), String::new())];
                }
                x.tags.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
            }
        },
    }
}
