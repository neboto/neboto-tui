use super::*;

// ── S3 Tables split panes ────────────────────────────────────────────────────

pub(super) fn render_s3tables_bucket_split(
    app: &App,
    tb: &crate::aws::services::s3tables::S3TableBucket,
    area: Rect,
    frame: &mut Frame,
) {
    let mut sub = vec![Span::styled(
        format!("{} bucket", tb.bucket_type),
        Style::default().fg(theme::accent()).add_modifier(Modifier::BOLD),
    )];
    sub.push(Span::styled("  ·  ", Style::default().fg(theme::text_dim())));
    sub.push(Span::styled(
        tb.created.clone(),
        Style::default().fg(theme::text_dim()),
    ));
    let header = sc_header(&tb.name, sub);
    render_sc_split(
        app,
        "S3 Table Bucket",
        header,
        &descriptor_tabs(app, &crate::aws::services::s3tables::S3TABLE_BUCKET_SECTIONS),
        "",
        area,
        frame,
    );
}

pub(super) fn render_s3tables_table_split(
    app: &App,
    t: &crate::aws::services::s3tables::S3Table,
    area: Rect,
    frame: &mut Frame,
) {
    let mut sub = vec![Span::styled(
        t.bucket_name.clone(),
        Style::default().fg(theme::accent()).add_modifier(Modifier::BOLD),
    )];
    sub.push(Span::styled("  ·  ", Style::default().fg(theme::text_dim())));
    sub.push(Span::styled(
        if t.managed_by.is_empty() {
            format!("{} table", t.table_type)
        } else {
            format!("managed by {}", t.managed_by)
        },
        Style::default().fg(theme::text_dim()),
    ));
    let header = sc_header(&t.full_name, sub);
    render_sc_split(
        app,
        "S3 Table",
        header,
        &descriptor_tabs(app, &crate::aws::services::s3tables::S3TABLE_SECTIONS),
        "",
        area,
        frame,
    );
}

/// Pretty-print a resource policy JSON as indented content rows.
pub(super) fn policy_doc_rows(title: &str, doc: &str) -> Vec<(String, String)> {
    let pretty = serde_json::from_str::<serde_json::Value>(doc)
        .and_then(|v| serde_json::to_string_pretty(&v))
        .unwrap_or_else(|_| doc.to_string());
    let mut rows = vec![(title.to_string(), String::new())];
    rows.push((String::new(), String::new()));
    for line in pretty.lines() {
        rows.push((format!("  {}", line), String::new()));
    }
    rows
}

pub fn s3tables_bucket_section_lines(
    tb: &crate::aws::services::s3tables::S3TableBucket,
    section: crate::aws::services::s3tables::S3TableBucketDetailSection,
    extras: Option<&crate::lazy::Lazy<crate::aws::services::s3tables::S3TableBucketExtras>>,
    namespaces: Option<&crate::lazy::Lazy<Vec<crate::aws::services::s3tables::S3TablesNamespace>>>,
    maintenance: Option<&crate::lazy::Lazy<Vec<crate::aws::services::s3tables::MaintenanceRow>>>,
    policy: Option<&crate::lazy::Lazy<Option<String>>>,
) -> Vec<(String, String)> {
    use crate::aws::services::s3tables::S3TableBucketDetailSection as S;
    match section {
        S::Details => {
            let mut rows = vec![
                ("Name".to_string(), tb.name.clone()),
                ("Type".to_string(), tb.bucket_type.clone()),
                ("Owner Account".to_string(), tb.owner_account_id.clone()),
                ("Created".to_string(), tb.created.clone()),
            ];
            match extras {
                None | Some(crate::lazy::Lazy::Loading) => {
                    rows.push(("Encryption".to_string(), "Loading…".to_string()));
                }
                Some(crate::lazy::Lazy::Error(e)) => rows.extend(error_rows(e)),
                Some(crate::lazy::Lazy::Loaded(x)) => match &x.encryption {
                    Some((algo, kms)) => {
                        rows.push(("Encryption".to_string(), algo.clone()));
                        if !kms.is_empty() {
                            rows.push(("  KMS Key".to_string(), kms.clone()));
                        }
                    }
                    None => {
                        rows.push(("Encryption".to_string(), "SSE-S3 (default)".to_string()));
                    }
                },
            }
            rows.push((String::new(), String::new()));
            rows.push(("ARN".to_string(), tb.arn.clone()));
            rows
        }
        S::Namespaces => match namespaces {
            None | Some(crate::lazy::Lazy::Loading) => {
                vec![("".to_string(), "Loading namespaces…".to_string())]
            }
            Some(crate::lazy::Lazy::Error(e)) => error_rows(e),
            Some(crate::lazy::Lazy::Loaded(list)) => {
                if list.is_empty() {
                    return vec![("".to_string(), "No namespaces".to_string())];
                }
                let capped = list.last().is_some_and(|n| n.capped);
                let mut rows = vec![(format!("Namespaces ({})", list.len()), String::new())];
                rows.push((String::new(), String::new()));
                for n in list {
                    rows.push((n.name.clone(), String::new()));
                    rows.push(("  Created".to_string(), n.created.clone()));
                    if !n.created_by.is_empty() {
                        rows.push(("  Created By".to_string(), n.created_by.clone()));
                    }
                    rows.push((String::new(), String::new()));
                }
                if capped {
                    rows.push(("    · list truncated".to_string(), String::new()));
                }
                rows
            }
        },
        S::Maintenance => match maintenance {
            None | Some(crate::lazy::Lazy::Loading) => {
                vec![("".to_string(), "Loading maintenance configuration…".to_string())]
            }
            Some(crate::lazy::Lazy::Error(e)) => error_rows(e),
            Some(crate::lazy::Lazy::Loaded(list)) => {
                if list.is_empty() {
                    return vec![(
                        "".to_string(),
                        "No bucket-level maintenance configuration".to_string(),
                    )];
                }
                let mut rows = vec![("Maintenance".to_string(), String::new())];
                rows.push((String::new(), String::new()));
                for (ty, status, settings) in list {
                    rows.push((ty.clone(), status.clone()));
                    if !settings.is_empty() {
                        rows.push(("  Settings".to_string(), settings.clone()));
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
                vec![("".to_string(), "No table bucket policy attached".to_string())]
            }
            Some(crate::lazy::Lazy::Loaded(Some(doc))) => {
                policy_doc_rows("Table Bucket Policy", doc)
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

pub fn s3tables_table_section_lines(
    t: &crate::aws::services::s3tables::S3Table,
    section: crate::aws::services::s3tables::S3TableDetailSection,
    extras: Option<&crate::lazy::Lazy<Box<crate::aws::services::s3tables::S3TableExtras>>>,
    maintenance: Option<&crate::lazy::Lazy<crate::aws::services::s3tables::S3TableMaintenance>>,
    policy: Option<&crate::lazy::Lazy<Option<String>>>,
) -> Vec<(String, String)> {
    use crate::aws::services::s3tables::S3TableDetailSection as S;
    match section {
        S::Details => {
            let mut rows = vec![
                ("Table".to_string(), t.table_name.clone()),
                ("Namespace".to_string(), t.namespace.clone()),
                ("Type".to_string(), t.table_type.clone()),
            ];
            if !t.managed_by.is_empty() {
                rows.push(("Managed By".to_string(), t.managed_by.clone()));
            }
            rows.push(("Created".to_string(), t.created.clone()));
            rows.push(("Modified".to_string(), t.modified.clone()));
            rows.push((String::new(), String::new()));
            match extras {
                None | Some(crate::lazy::Lazy::Loading) => {
                    rows.push(("Format".to_string(), "Loading…".to_string()));
                }
                Some(crate::lazy::Lazy::Error(e)) => rows.extend(error_rows(e)),
                Some(crate::lazy::Lazy::Loaded(x)) => {
                    rows.push(("Format".to_string(), x.format.clone()));
                    if !x.metadata_location.is_empty() {
                        rows.push(("Metadata Location".to_string(), x.metadata_location.clone()));
                    }
                    rows.push(("Warehouse Location".to_string(), x.warehouse_location.clone()));
                    rows.push(("Version Token".to_string(), x.version_token.clone()));
                    if !x.created_by.is_empty() {
                        rows.push(("Created By".to_string(), x.created_by.clone()));
                    }
                    if !x.modified_by.is_empty() {
                        rows.push(("Modified By".to_string(), x.modified_by.clone()));
                    }
                }
            }
            rows.push((String::new(), String::new()));
            // The bucket ARN row Enter-jumps to the table bucket's pane.
            rows.push(("Bucket".to_string(), t.bucket_arn.clone()));
            rows.push(("ARN".to_string(), t.arn.clone()));
            rows
        }
        S::Maintenance => match maintenance {
            None | Some(crate::lazy::Lazy::Loading) => {
                vec![("".to_string(), "Loading maintenance…".to_string())]
            }
            Some(crate::lazy::Lazy::Error(e)) => error_rows(e),
            Some(crate::lazy::Lazy::Loaded(m)) => {
                let mut rows = vec![("Configuration".to_string(), String::new())];
                if m.config.is_empty() {
                    rows.push(("  (defaults apply)".to_string(), String::new()));
                } else {
                    for (ty, status, settings) in &m.config {
                        rows.push((format!("  {}", ty), status.clone()));
                        if !settings.is_empty() {
                            rows.push(("    Settings".to_string(), settings.clone()));
                        }
                    }
                }
                rows.push((String::new(), String::new()));
                rows.push(("Last Runs".to_string(), String::new()));
                if m.jobs.is_empty() {
                    rows.push(("  (no job status available)".to_string(), String::new()));
                } else {
                    for (ty, status, last_run, failure) in &m.jobs {
                        let val = match (status.as_str(), last_run.is_empty()) {
                            ("Failed", _) => format!("✗ {}", status),
                            ("Successful", _) => format!("✓ {}", status),
                            _ => status.clone(),
                        };
                        rows.push((format!("  {}", ty), val));
                        if !last_run.is_empty() {
                            rows.push(("    Last Run".to_string(), last_run.clone()));
                        }
                        if !failure.is_empty() {
                            rows.push((format!("    ⚠ {}", failure), String::new()));
                        }
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
                vec![("".to_string(), "No table policy attached".to_string())]
            }
            Some(crate::lazy::Lazy::Loaded(Some(doc))) => policy_doc_rows("Table Policy", doc),
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
