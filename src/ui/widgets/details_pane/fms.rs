use super::*;

// ── Firewall Manager split panes ────────────────────────────────────────────

pub(super) fn render_fms_policy_split(app: &App, p: &FmsPolicy, area: Rect, frame: &mut Frame) {
    let subtitle = format!("{} · {}", p.service_kind, p.resource_type);
    render_simple_split(
        app,
        area,
        frame,
        "FMS Policy",
        &p.name,
        &subtitle,
        &descriptor_tabs(app, &crate::aws::services::fms::FMS_POLICY_SECTIONS),
    );
}

pub fn fms_policy_section_lines(
    p: &FmsPolicy,
    section: FmsPolicyDetailSection,
    admin: Option<&FmsAdminInfo>,
    tags: Option<&Lazy<Vec<(String, String)>>>,
    drill: &crate::lazy::LazyMap<FmsComplianceDetail>,
) -> Vec<(String, String)> {
    match section {
        FmsPolicyDetailSection::Overview => {
            let mut rows = vec![
                ("Name".to_string(), p.name.clone()),
                ("Policy ID".to_string(), p.id.clone()),
                ("Security Service".to_string(), p.service_kind.clone()),
            ];
            if p.resource_type_list.len() > 1 {
                rows.push((
                    "Resource Types".to_string(),
                    p.resource_type_list.join(", "),
                ));
            } else {
                rows.push(("Resource Type".to_string(), p.resource_type.clone()));
            }
            if let Some(status) = &p.status {
                let shown = if status == "OUT_OF_ADMIN_SCOPE" {
                    format!("⚠ {}", status)
                } else {
                    status.clone()
                };
                rows.push(("Status".to_string(), shown));
            }
            rows.push((
                "Auto Remediation".to_string(),
                if p.remediation_enabled {
                    "✓ Enabled".to_string()
                } else {
                    "Disabled (audit only)".to_string()
                },
            ));
            rows.push((
                "Delete Unused Resources".to_string(),
                if p.delete_unused {
                    "✓ Enabled".to_string()
                } else {
                    "Disabled".to_string()
                },
            ));
            // One-line compliance verdict; the full matrix is section 4.
            if p.compliance_error.is_none() && !p.compliance.is_empty() {
                let (ok, bad) = p.compliance_counts();
                rows.push((
                    "Compliance".to_string(),
                    if bad == 0 {
                        format!("✓ all {} accounts compliant", ok + bad)
                    } else {
                        format!(
                            "✗ {} of {} accounts non-compliant ({} violators)",
                            bad,
                            ok + bad,
                            p.total_violators()
                        )
                    },
                ));
            }
            if let Some(d) = &p.description {
                if !d.is_empty() {
                    rows.push(("Description".to_string(), d.clone()));
                }
            }
            if let Some(arn) = &p.arn {
                rows.push(("ARN".to_string(), arn.clone()));
            }
            if let Some(admin) = admin {
                rows.push((String::new(), String::new()));
                rows.push(("Administration".to_string(), String::new()));
                match (&admin.admin_account, &admin.error) {
                    (Some(acct), _) => {
                        rows.push(("Admin Account".to_string(), acct.clone()));
                        if let Some(rs) = &admin.role_status {
                            rows.push(("Admin Role Status".to_string(), rs.clone()));
                        }
                        if let Some(n) = admin.member_count {
                            rows.push(("Member Accounts".to_string(), n.to_string()));
                        }
                        // Delivery topic ARN is a key-value row → jumpable to
                        // the SNS topic.
                        if let Some(topic) = &admin.sns_topic {
                            rows.push(("SNS Topic".to_string(), topic.clone()));
                        }
                        if let Some(role) = &admin.sns_role {
                            rows.push(("SNS Role".to_string(), role.clone()));
                        }
                    }
                    (None, Some(e)) => {
                        rows.push((
                            format!("  ⚠ Admin posture unknown: {}", e),
                            String::new(),
                        ));
                    }
                    (None, None) => {}
                }
            }
            if let Some(e) = &p.detail_error {
                rows.push((String::new(), String::new()));
                rows.push((
                    format!("  ⚠ GetPolicy failed (summary only): {}", e),
                    String::new(),
                ));
            }
            rows
        }
        FmsPolicyDetailSection::Scope => {
            if let Some(e) = &p.detail_error {
                return error_rows(e);
            }
            let mut rows: Vec<(String, String)> = Vec::new();
            if p.include_scope.is_empty() {
                let all = match admin.and_then(|a| a.member_count) {
                    Some(n) => format!("All accounts in the organization ({} in scope)", n),
                    None => "All accounts in the organization".to_string(),
                };
                rows.push(("Applies To".to_string(), all));
            } else {
                rows.push(("Included".to_string(), String::new()));
                for (kind, ids) in &p.include_scope {
                    rows.push((format!("  {}:", kind), String::new()));
                    for id in ids {
                        rows.push((format!("    {}", id), String::new()));
                    }
                }
            }
            if !p.exclude_scope.is_empty() {
                rows.push((String::new(), String::new()));
                rows.push(("Excluded".to_string(), String::new()));
                for (kind, ids) in &p.exclude_scope {
                    rows.push((format!("  {}:", kind), String::new()));
                    for id in ids {
                        rows.push((format!("    {}", id), String::new()));
                    }
                }
            }
            if !p.resource_tags.is_empty() {
                rows.push((String::new(), String::new()));
                rows.push(("Resource Tags".to_string(), String::new()));
                rows.push((
                    "Tag Filter".to_string(),
                    if p.exclude_resource_tags {
                        "Exclude matching resources".to_string()
                    } else {
                        "Include matching resources".to_string()
                    },
                ));
                if let Some(op) = &p.resource_tag_op {
                    rows.push(("Tag Operator".to_string(), op.clone()));
                }
                for t in &p.resource_tags {
                    rows.push((format!("  {}", t), String::new()));
                }
            }
            if !p.resource_set_ids.is_empty() {
                rows.push((String::new(), String::new()));
                rows.push(("Resource Sets".to_string(), String::new()));
                for id in &p.resource_set_ids {
                    rows.push((format!("  {}", id), String::new()));
                }
            }
            rows
        }
        FmsPolicyDetailSection::Config => {
            if let Some(e) = &p.detail_error {
                return error_rows(e);
            }
            if p.config_rows.is_empty() {
                return vec![(
                    "  (no managed service data)".to_string(),
                    String::new(),
                )];
            }
            let mut rows = p.config_rows.clone();
            rows.push((String::new(), String::new()));
            rows.push((
                "  (press e for the raw policy JSON)".to_string(),
                String::new(),
            ));
            rows
        }
        FmsPolicyDetailSection::Compliance => fms_compliance_rows(p, drill),
        FmsPolicyDetailSection::Tags => match tags {
            None | Some(Lazy::Loading) => {
                vec![("".to_string(), "Loading tags…".to_string())]
            }
            Some(Lazy::Error(e)) => error_rows(e),
            Some(Lazy::Loaded(list)) => {
                if list.is_empty() {
                    vec![("  (no tags)".to_string(), String::new())]
                } else {
                    list.clone()
                }
            }
        },
    }
}

/// The Compliance section body: summary counts, dependent-service issues,
/// the per-account matrix, then a violator group per non-compliant account
/// (fed by the lazy `GetComplianceDetail` drill, worst accounts first,
/// capped at `MAX_DRILL_ACCOUNTS`).
pub(super) fn fms_compliance_rows(
    p: &FmsPolicy,
    drill: &crate::lazy::LazyMap<FmsComplianceDetail>,
) -> Vec<(String, String)> {
    use crate::aws::services::fms::MAX_DRILL_ACCOUNTS;
    const MAX_VIOLATORS_SHOWN: usize = 25;

    if let Some(e) = &p.compliance_error {
        return error_rows(e);
    }
    if p.compliance.is_empty() {
        return vec![(
            "  No member-account evaluations yet (new policy, or no accounts in scope)"
                .to_string(),
            String::new(),
        )];
    }

    let (ok, bad) = p.compliance_counts();
    let mut rows = vec![
        ("Accounts Evaluated".to_string(), (ok + bad).to_string()),
        ("Compliant".to_string(), format!("✓ {}", ok)),
        (
            "Non-compliant".to_string(),
            if bad > 0 {
                format!("✗ {}", bad)
            } else {
                "0".to_string()
            },
        ),
    ];
    if p.total_violators() > 0 {
        rows.push(("Total Violators".to_string(), p.total_violators().to_string()));
    }
    if !p.issue_infos.is_empty() {
        rows.push((String::new(), String::new()));
        rows.push(("Service Issues".to_string(), String::new()));
        for (svc, msg) in &p.issue_infos {
            rows.push((format!("  ⚠ {}: {}", svc, msg), String::new()));
        }
    }

    rows.push((String::new(), String::new()));
    rows.push(("Accounts".to_string(), String::new()));
    for c in &p.compliance {
        let value = if c.compliant {
            "✓ compliant".to_string()
        } else {
            let mut v = format!("✗ {} violators", c.violators);
            if c.limit_exceeded {
                v.push_str(" (limit exceeded — more exist)");
            }
            v
        };
        rows.push((c.account.clone(), value));
    }

    // Violator drill, one group per non-compliant account (same order and
    // cap as the trigger, so every group has a fetch behind it).
    for c in p
        .compliance
        .iter()
        .filter(|c| !c.compliant)
        .take(MAX_DRILL_ACCOUNTS)
    {
        rows.push((String::new(), String::new()));
        rows.push((format!("Violators — {}", c.account), String::new()));
        match drill.get(&format!("{}/{}", p.id, c.account)) {
            None | Some(Lazy::Loading) => {
                rows.push(("  Loading violators…".to_string(), String::new()));
            }
            Some(Lazy::Error(e)) => rows.extend(error_rows(e)),
            Some(Lazy::Loaded(d)) => {
                if d.violators.is_empty() {
                    rows.push((
                        "  (no violator details returned)".to_string(),
                        String::new(),
                    ));
                }
                for v in d.violators.iter().take(MAX_VIOLATORS_SHOWN) {
                    // Key-value row so the resource id gets jump-classified
                    // (resolves when the resource lives in this account).
                    rows.push((
                        if v.resource_type.is_empty() {
                            "Resource".to_string()
                        } else {
                            v.resource_type.clone()
                        },
                        v.resource_id.clone(),
                    ));
                    rows.push((format!("    {}", v.reason), String::new()));
                }
                if d.violators.len() > MAX_VIOLATORS_SHOWN {
                    rows.push((
                        format!(
                            "  … +{} more violators in this account",
                            d.violators.len() - MAX_VIOLATORS_SHOWN
                        ),
                        String::new(),
                    ));
                }
                if d.limit_exceeded {
                    rows.push((
                        "  ⚠ evaluation limit exceeded — AWS truncated this list".to_string(),
                        String::new(),
                    ));
                }
            }
        }
    }
    if bad > MAX_DRILL_ACCOUNTS {
        rows.push((String::new(), String::new()));
        rows.push((
            format!(
                "  (violator details fetched for the {} worst accounts of {})",
                MAX_DRILL_ACCOUNTS, bad
            ),
            String::new(),
        ));
    }
    rows
}

pub(super) fn render_fms_resource_set_split(app: &App, s: &FmsResourceSet, area: Rect, frame: &mut Frame) {
    let subtitle = s.description.as_deref().unwrap_or("Resource set");
    render_simple_split(
        app,
        area,
        frame,
        "FMS Resource Set",
        &s.name,
        subtitle,
        &descriptor_tabs(app, &crate::aws::services::fms::FMS_RESOURCE_SET_SECTIONS),
    );
}

pub fn fms_resource_set_section_lines(
    s: &FmsResourceSet,
    section: FmsResourceSetDetailSection,
    detail: Option<&Lazy<FmsResourceSetDetail>>,
) -> Vec<(String, String)> {
    match section {
        FmsResourceSetDetailSection::Overview => {
            let mut rows = vec![
                ("Name".to_string(), s.name.clone()),
                ("ID".to_string(), s.id.clone()),
            ];
            if let Some(status) = &s.status {
                let shown = if status == "OUT_OF_ADMIN_SCOPE" {
                    format!("⚠ {}", status)
                } else {
                    status.clone()
                };
                rows.push(("Status".to_string(), shown));
            }
            if let Some(t) = &s.last_update {
                rows.push(("Last Updated".to_string(), t.clone()));
            }
            if let Some(d) = &s.description {
                if !d.is_empty() {
                    rows.push(("Description".to_string(), d.clone()));
                }
            }
            rows
        }
        FmsResourceSetDetailSection::Members => match detail {
            None | Some(Lazy::Loading) => {
                vec![("".to_string(), "Loading members…".to_string())]
            }
            Some(Lazy::Error(e)) => error_rows(e),
            Some(Lazy::Loaded(d)) => {
                let mut rows = vec![(
                    "Resource Types".to_string(),
                    if d.resource_types.is_empty() {
                        "—".to_string()
                    } else {
                        d.resource_types.join(", ")
                    },
                )];
                rows.push((String::new(), String::new()));
                rows.push((format!("Members ({})", d.members.len()), String::new()));
                if d.members.is_empty() {
                    rows.push(("  (no resources in this set)".to_string(), String::new()));
                } else {
                    // Key-value rows (account → URI) so member ARNs pick up
                    // the jump classifier; same-account ones resolve.
                    for (uri, account) in &d.members {
                        let key = if account.is_empty() {
                            "  Member".to_string()
                        } else {
                            format!("  {}", account)
                        };
                        rows.push((key, uri.clone()));
                    }
                }
                rows
            }
        },
    }
}
