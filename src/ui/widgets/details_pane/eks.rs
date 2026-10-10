use super::*;

// ── EKS cluster split pane ─────────────────────────────────────────────────────

pub(super) fn render_eks_cluster_split(app: &App, cluster: &EksCluster, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 8, "");
    let mut block = theme::pane_block("EKS Cluster", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let state_color = match cluster.status.as_str() {
        "ACTIVE" => theme::success(),
        "CREATING" | "UPDATING" | "PENDING" => theme::warning(),
        "FAILED" => theme::error(),
        _ => theme::text_dim(),
    };
    let header = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                cluster.name.clone(),
                Style::default().fg(crate::ui::theme::text_primary()).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(format!("k8s {}", cluster.version), Style::default().fg(theme::accent())),
            Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
            Span::styled(cluster.status.clone(), Style::default().fg(state_color)),
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
        &descriptor_tabs(app, &crate::aws::services::eks::EKS_CLUSTER_SECTIONS),
    );
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub fn eks_cluster_section_lines(
    cluster: &EksCluster,
    section: EksClusterDetailSection,
    nodegroups: Option<&Lazy<Vec<crate::aws::services::eks::EksNodeGroup>>>,
    fargate: Option<&Lazy<Vec<crate::aws::services::eks::EksFargateProfile>>>,
    addons: Option<&Lazy<Vec<crate::aws::services::eks::EksAddon>>>,
    access: Option<&Lazy<crate::aws::services::eks::EksAccessData>>,
    insights: Option<&Lazy<Vec<crate::aws::services::eks::EksInsight>>>,
) -> Vec<(String, String)> {
    match section {
        EksClusterDetailSection::Details => {
            let mut rows = vec![
                ("Name".to_string(), cluster.name.clone()),
                ("Status".to_string(), cluster.status.clone()),
                ("K8s Version".to_string(), cluster.version.clone()),
                ("Platform".to_string(), cluster.platform_version.clone()),
                ("Auth Mode".to_string(), cluster.auth_mode.clone()),
            ];
            if !cluster.upgrade_policy.is_empty() {
                rows.push(("Support".to_string(), cluster.upgrade_policy.clone()));
            }
            // Health issues — force red via the ✗ marker.
            if !cluster.health_issues.is_empty() {
                rows.push((String::new(), String::new()));
                rows.push(("Health".to_string(), String::new())); // group header
                for issue in &cluster.health_issues {
                    rows.push(("  ✗ Issue".to_string(), issue.clone()));
                }
            }
            rows.push((String::new(), String::new()));
            rows.push(("Endpoint".to_string(), String::new())); // group header
            rows.push(("  Access".to_string(), cluster.endpoint_access()));
            rows.push(("  URL".to_string(), cluster.endpoint.clone()));
            rows.push((String::new(), String::new()));
            rows.push(("Identity & Encryption".to_string(), String::new())); // group header
            if !cluster.role_arn.is_empty() {
                rows.push(("  Cluster Role".to_string(), cluster.role_arn.clone()));
            }
            if !cluster.oidc_issuer.is_empty() {
                rows.push(("  OIDC Issuer".to_string(), cluster.oidc_issuer.clone()));
            }
            rows.push((
                "  Secrets KMS Key".to_string(),
                if cluster.secrets_kms_key.is_empty() {
                    "(none)".to_string()
                } else {
                    cluster.secrets_kms_key.clone()
                },
            ));
            rows.push((String::new(), String::new()));
            let logging = if cluster.logging_enabled.is_empty() {
                "(none enabled)".to_string()
            } else {
                cluster.logging_enabled.join(", ")
            };
            rows.push(("Control Plane Logs".to_string(), logging));
            if cluster.logging_enabled.is_empty() {
                rows.push((
                    "  enable control-plane logging to use t (tail)".to_string(),
                    String::new(),
                ));
            } else {
                rows.push(("  press t to tail · m for metrics".to_string(), String::new()));
            }
            if let Some(c) = &cluster.created {
                rows.push((String::new(), String::new()));
                rows.push(("Created".to_string(), c.clone()));
            }
            rows.push(("ARN".to_string(), cluster.arn.clone()));
            rows
        }
        EksClusterDetailSection::Networking => {
            let mut rows = vec![("VPC".to_string(), String::new())]; // group header
            rows.push(("  VPC".to_string(), cluster.vpc_id.clone()));
            if cluster.subnet_ids.is_empty() {
                rows.push(("  Subnets".to_string(), "(none)".to_string()));
            } else {
                rows.push(("  Subnets".to_string(), String::new()));
                for s in &cluster.subnet_ids {
                    rows.push(("  Subnet".to_string(), s.clone()));
                }
            }
            rows.push((String::new(), String::new()));
            rows.push(("Security Groups".to_string(), String::new())); // group header
            if !cluster.cluster_security_group_id.is_empty() {
                rows.push((
                    "  Cluster SG".to_string(),
                    cluster.cluster_security_group_id.clone(),
                ));
            }
            for sg in &cluster.security_group_ids {
                rows.push(("  Control Plane SG".to_string(), sg.clone()));
            }
            if cluster.cluster_security_group_id.is_empty()
                && cluster.security_group_ids.is_empty()
            {
                rows.push(("  (none)".to_string(), String::new()));
            }
            rows.push((String::new(), String::new()));
            rows.push(("Service Networking".to_string(), String::new())); // group header
            if !cluster.ip_family.is_empty() {
                rows.push(("  IP Family".to_string(), cluster.ip_family.clone()));
            }
            if !cluster.service_ipv4_cidr.is_empty() {
                rows.push(("  Service IPv4 CIDR".to_string(), cluster.service_ipv4_cidr.clone()));
            }
            if !cluster.service_ipv6_cidr.is_empty() {
                rows.push(("  Service IPv6 CIDR".to_string(), cluster.service_ipv6_cidr.clone()));
            }
            rows.push((String::new(), String::new()));
            rows.push(("Public Access CIDRs".to_string(), String::new())); // group header
            if cluster.public_access_cidrs.is_empty() {
                rows.push(("  (none / disabled)".to_string(), String::new()));
            } else {
                for c in &cluster.public_access_cidrs {
                    rows.push((format!("  {}", c), String::new()));
                }
            }
            rows
        }
        EksClusterDetailSection::Compute => match nodegroups {
            None | Some(Lazy::Loading) => {
                vec![("".to_string(), "Loading node groups…".to_string())]
            }
            Some(Lazy::Error(e)) => error_rows(e),
            Some(Lazy::Loaded(ngs)) => {
                if ngs.is_empty() {
                    return vec![("".to_string(), "No managed node groups".to_string())];
                }
                let mut rows = Vec::new();
                for ng in ngs {
                    rows.push((ng.name.clone(), String::new())); // group header
                    rows.push(("  Status".to_string(), ng.status.clone()));
                    for issue in &ng.health_issues {
                        rows.push(("  ✗ Health".to_string(), issue.clone()));
                    }
                    rows.push((
                        "  Version".to_string(),
                        version_drift(&ng.version, &cluster.version),
                    ));
                    if !ng.release_version.is_empty() {
                        rows.push(("  Release".to_string(), ng.release_version.clone()));
                    }
                    rows.push(("  Capacity".to_string(), ng.capacity_type.clone()));
                    rows.push(("  AMI Type".to_string(), ng.ami_type.clone()));
                    if !ng.instance_types.is_empty() {
                        rows.push(("  Instance Types".to_string(), ng.instance_types.join(", ")));
                    }
                    rows.push((
                        "  Scaling".to_string(),
                        format!("desired {} (min {} / max {})", ng.desired, ng.min, ng.max),
                    ));
                    if let Some(d) = ng.disk_size {
                        rows.push(("  Disk Size".to_string(), format!("{} GiB", d)));
                    }
                    if !ng.max_unavailable.is_empty() {
                        rows.push(("  Max Unavailable".to_string(), ng.max_unavailable.clone()));
                    }
                    if !ng.launch_template.is_empty() {
                        rows.push(("  Launch Template".to_string(), ng.launch_template.clone()));
                    }
                    if !ng.node_role.is_empty() {
                        rows.push(("  Node Role".to_string(), ng.node_role.clone()));
                    }
                    if !ng.labels.is_empty() {
                        rows.push(("  Labels".to_string(), String::new()));
                        for (k, v) in &ng.labels {
                            rows.push((format!("    {}={}", k, v), String::new()));
                        }
                    }
                    if !ng.taints.is_empty() {
                        rows.push(("  Taints".to_string(), String::new()));
                        for t in &ng.taints {
                            rows.push((format!("    {}", t), String::new()));
                        }
                    }
                    if !ng.subnets.is_empty() {
                        rows.push(("  Subnets".to_string(), String::new()));
                        for s in &ng.subnets {
                            rows.push(("    Subnet".to_string(), s.clone()));
                        }
                    }
                    if let Some(c) = &ng.created {
                        rows.push(("  Created".to_string(), c.clone()));
                    }
                    rows.push((String::new(), String::new()));
                }
                rows
            }
        },
        EksClusterDetailSection::Fargate => match fargate {
            None | Some(Lazy::Loading) => {
                vec![("".to_string(), "Loading Fargate profiles…".to_string())]
            }
            Some(Lazy::Error(e)) => error_rows(e),
            Some(Lazy::Loaded(fps)) => {
                if fps.is_empty() {
                    return vec![("".to_string(), "No Fargate profiles".to_string())];
                }
                let mut rows = Vec::new();
                for fp in fps {
                    rows.push((fp.name.clone(), String::new())); // group header
                    rows.push(("  Status".to_string(), fp.status.clone()));
                    if !fp.pod_execution_role.is_empty() {
                        rows.push((
                            "  Pod Exec Role".to_string(),
                            fp.pod_execution_role.clone(),
                        ));
                    }
                    rows.push(("  Selectors".to_string(), String::new()));
                    for sel in &fp.selectors {
                        rows.push((format!("    {}", sel), String::new()));
                    }
                    if !fp.subnets.is_empty() {
                        rows.push(("  Subnets".to_string(), String::new()));
                        for s in &fp.subnets {
                            rows.push(("    Subnet".to_string(), s.clone()));
                        }
                    }
                    if let Some(c) = &fp.created {
                        rows.push(("  Created".to_string(), c.clone()));
                    }
                    rows.push((String::new(), String::new()));
                }
                rows
            }
        },
        EksClusterDetailSection::Addons => match addons {
            None | Some(Lazy::Loading) => {
                vec![("".to_string(), "Loading add-ons…".to_string())]
            }
            Some(Lazy::Error(e)) => error_rows(e),
            Some(Lazy::Loaded(addons)) => {
                if addons.is_empty() {
                    return vec![("".to_string(), "No add-ons".to_string())];
                }
                let mut rows = Vec::new();
                for a in addons {
                    let status = if a.status == "DEGRADED" {
                        format!("✗ {}", a.status)
                    } else {
                        a.status.clone()
                    };
                    rows.push((a.name.clone(), String::new())); // group header
                    rows.push(("  Status".to_string(), status));
                    let version = if a.update_available && !a.latest_version.is_empty() {
                        format!("⚠ {}  (latest {})", a.version, a.latest_version)
                    } else {
                        a.version.clone()
                    };
                    rows.push(("  Version".to_string(), version));
                    for issue in &a.health_issues {
                        rows.push(("  ✗ Health".to_string(), issue.clone()));
                    }
                    if a.configured {
                        rows.push(("  Configured".to_string(), "yes".to_string()));
                    }
                    if !a.service_account_role.is_empty() {
                        rows.push(("  SA Role".to_string(), a.service_account_role.clone()));
                    }
                    rows.push((String::new(), String::new()));
                }
                rows
            }
        },
        EksClusterDetailSection::Access => match access {
            None | Some(Lazy::Loading) => {
                vec![("".to_string(), "Loading access entries…".to_string())]
            }
            Some(Lazy::Error(e)) => error_rows(e),
            Some(Lazy::Loaded(data)) => {
                let mut rows = vec![("Auth Mode".to_string(), cluster.auth_mode.clone())];
                if cluster.auth_mode == "CONFIG_MAP" {
                    rows.push((
                        "  CONFIG_MAP clusters keep mappings in the in-cluster aws-auth ConfigMap"
                            .to_string(),
                        String::new(),
                    ));
                }
                rows.push((String::new(), String::new()));
                rows.push(("Access Entries".to_string(), String::new())); // group header
                if data.entries.is_empty() {
                    rows.push(("  (none)".to_string(), String::new()));
                } else {
                    for e in &data.entries {
                        rows.push(("  Principal".to_string(), e.principal_arn.clone()));
                        if !e.entry_type.is_empty() {
                            rows.push(("    Type".to_string(), e.entry_type.clone()));
                        }
                        if !e.username.is_empty() {
                            rows.push(("    Username".to_string(), e.username.clone()));
                        }
                        if !e.kubernetes_groups.is_empty() {
                            rows.push((
                                "    K8s Groups".to_string(),
                                e.kubernetes_groups.join(", "),
                            ));
                        }
                        for p in &e.access_policies {
                            rows.push((format!("    {}", p), String::new()));
                        }
                        rows.push((String::new(), String::new()));
                    }
                }
                rows.push(("Pod Identity".to_string(), String::new())); // group header
                if data.pod_identities.is_empty() {
                    rows.push(("  (none)".to_string(), String::new()));
                } else {
                    for pi in &data.pod_identities {
                        rows.push((
                            "  Service Account".to_string(),
                            format!("{}/{}", pi.namespace, pi.service_account),
                        ));
                        rows.push(("    Role".to_string(), pi.role_arn.clone()));
                        rows.push((String::new(), String::new()));
                    }
                }
                rows
            }
        },
        EksClusterDetailSection::Insights => match insights {
            None | Some(Lazy::Loading) => {
                vec![("".to_string(), "Loading upgrade insights…".to_string())]
            }
            Some(Lazy::Error(e)) => error_rows(e),
            Some(Lazy::Loaded(list)) => {
                if list.is_empty() {
                    return vec![(
                        "".to_string(),
                        "No upgrade-readiness insights".to_string(),
                    )];
                }
                let mut rows = Vec::new();
                for ins in list {
                    rows.push((ins.name.clone(), String::new())); // group header
                    let status = match ins.status.as_str() {
                        "PASSING" => format!("✓ {}", ins.status),
                        "WARNING" => format!("⚠ {}", ins.status),
                        "ERROR" => format!("✗ {}", ins.status),
                        _ => ins.status.clone(),
                    };
                    rows.push(("  Status".to_string(), status));
                    if !ins.category.is_empty() {
                        rows.push(("  Category".to_string(), ins.category.clone()));
                    }
                    if !ins.kubernetes_version.is_empty() {
                        rows.push(("  Target Version".to_string(), ins.kubernetes_version.clone()));
                    }
                    if !ins.description.is_empty() {
                        rows.push(("  Description".to_string(), ins.description.clone()));
                    }
                    if !ins.recommendation.is_empty() {
                        rows.push(("  Recommendation".to_string(), ins.recommendation.clone()));
                    }
                    if !ins.deprecation_details.is_empty() {
                        rows.push(("  Deprecated APIs".to_string(), String::new()));
                        for d in &ins.deprecation_details {
                            rows.push((format!("    {}", d), String::new()));
                        }
                    }
                    if let Some(r) = &ins.last_refresh {
                        rows.push(("  Last Refresh".to_string(), r.clone()));
                    }
                    rows.push((String::new(), String::new()));
                }
                rows
            }
        },
        EksClusterDetailSection::Tags => tag_rows(&cluster.tags),
    }
}

/// Annotate a child's k8s version with a drift marker when it trails the
/// cluster's control-plane version (the EKS upgrade signal).
pub(super) fn version_drift(child: &str, cluster: &str) -> String {
    if !child.is_empty() && !cluster.is_empty() && child != cluster {
        format!("{}  ⚠ behind cluster ({})", child, cluster)
    } else {
        child.to_string()
    }
}
