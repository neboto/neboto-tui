use super::*;

// ── Redshift split panes ────────────────────────────────────────────────────

pub(super) fn render_redshift_cluster_split(app: &App, c: &RedshiftCluster, area: Rect, frame: &mut Frame) {
    let subtitle = format!("{} · {}", c.size_label(), c.status);
    render_simple_split(
        app,
        area,
        frame,
        "Redshift Cluster",
        &c.id,
        &subtitle,
        &descriptor_tabs(app, &crate::aws::services::redshift::REDSHIFT_CLUSTER_SECTIONS),
    );
}

pub fn redshift_cluster_section_lines(
    c: &RedshiftCluster,
    section: RedshiftClusterDetailSection,
) -> Vec<(String, String)> {
    match section {
        RedshiftClusterDetailSection::Overview => {
            let mut rows = vec![
                ("Cluster".to_string(), c.id.clone()),
                ("Status".to_string(), c.status.clone()),
            ];
            if let Some(a) = &c.availability_status {
                rows.push(("Availability".to_string(), a.clone()));
            }
            if let Some(m) = &c.modify_status {
                rows.push(("Modify Status".to_string(), m.clone()));
            }
            rows.push(("Node Type".to_string(), c.node_type.clone()));
            rows.push(("Nodes".to_string(), c.number_of_nodes.to_string()));
            if let Some(mb) = c.total_storage_mb {
                rows.push((
                    "Total Storage".to_string(),
                    crate::aws::services::redshift::fmt_mb(mb as f64),
                ));
            }

            rows.push((String::new(), String::new()));
            rows.push(("Database".to_string(), String::new())); // group header
            if let Some(db) = &c.db_name {
                rows.push(("  Name".to_string(), db.clone()));
            }
            if let Some(u) = &c.master_username {
                rows.push(("  Master User".to_string(), u.clone()));
            }
            if let Some((addr, port)) = &c.endpoint {
                rows.push(("  Endpoint".to_string(), format!("{}:{}", addr, port)));
            }
            if let Some(v) = &c.version {
                rows.push(("  Engine Version".to_string(), v.clone()));
            }

            if !c.pending_actions.is_empty() {
                rows.push((String::new(), String::new()));
                rows.push(("Pending Actions".to_string(), String::new())); // group header
                for a in &c.pending_actions {
                    rows.push(("  ".to_string(), format!("⚠ {}", a)));
                }
            }

            rows.push((String::new(), String::new()));
            rows.push(("Other".to_string(), String::new())); // group header
            if let Some(created) = &c.created {
                rows.push(("  Created".to_string(), created.clone()));
            }
            if let Some(mz) = &c.multi_az {
                rows.push(("  Multi-AZ".to_string(), mz.clone()));
            }
            if !c.arn.is_empty() {
                rows.push(("  Namespace ARN".to_string(), c.arn.clone()));
            }
            rows
        }
        RedshiftClusterDetailSection::Network => {
            let mut rows = Vec::new();
            if let Some(vpc) = &c.vpc_id {
                rows.push(("VPC".to_string(), vpc.clone())); // jumpable
            }
            if let Some(sg) = &c.subnet_group {
                rows.push(("Subnet Group".to_string(), sg.clone()));
            }
            if let Some(az) = &c.availability_zone {
                rows.push(("Availability Zone".to_string(), az.clone()));
            }
            if let Some(rel) = &c.az_relocation {
                rows.push(("AZ Relocation".to_string(), rel.clone()));
            }
            rows.push((
                "Publicly Accessible".to_string(),
                if c.publicly_accessible {
                    "⚠ Yes".to_string()
                } else {
                    "✓ No".to_string()
                },
            ));
            rows.push((
                "Enhanced VPC Routing".to_string(),
                if c.enhanced_vpc_routing {
                    "✓ Enabled".to_string()
                } else {
                    "Disabled".to_string()
                },
            ));
            if let Some((addr, port)) = &c.endpoint {
                rows.push(("Endpoint".to_string(), addr.clone()));
                rows.push(("Port".to_string(), port.to_string()));
            }
            if let Some(eip) = &c.elastic_ip {
                rows.push(("Elastic IP".to_string(), eip.clone()));
            }
            if let Some(d) = &c.custom_domain {
                rows.push(("Custom Domain".to_string(), d.clone()));
            }

            if !c.vpc_security_groups.is_empty() {
                rows.push((String::new(), String::new()));
                rows.push(("Security Groups".to_string(), String::new())); // group header
                for (id, status) in &c.vpc_security_groups {
                    rows.push((format!("  {}", id), status.clone())); // jumpable sg-
                }
            }
            rows
        }
        RedshiftClusterDetailSection::Config => {
            let mut rows = vec![("Encryption".to_string(), String::new())]; // group header
            rows.push((
                "  At Rest".to_string(),
                if c.encrypted {
                    "✓ KMS".to_string()
                } else {
                    "✗ None".to_string()
                },
            ));
            if let Some(kms) = &c.kms_key_id {
                rows.push(("  KMS Key".to_string(), kms.clone())); // jumpable
            }

            if !c.parameter_groups.is_empty() {
                rows.push((String::new(), String::new()));
                rows.push(("Parameter Groups".to_string(), String::new())); // group header
                for (name, status) in &c.parameter_groups {
                    rows.push((format!("  {}", name), status.clone()));
                }
            }

            if !c.iam_roles.is_empty() || c.default_iam_role.is_some() {
                rows.push((String::new(), String::new()));
                rows.push(("IAM Roles".to_string(), String::new())); // group header
                for (arn, status) in &c.iam_roles {
                    let label = if Some(arn) == c.default_iam_role.as_ref() {
                        format!("{} (default)", status)
                    } else {
                        status.clone()
                    };
                    rows.push(("  Role".to_string(), arn.clone())); // jumpable
                    if !label.is_empty() {
                        rows.push(("    Status".to_string(), label));
                    }
                }
            }

            rows.push((String::new(), String::new()));
            rows.push(("Maintenance".to_string(), String::new())); // group header
            if let Some(w) = &c.maintenance_window {
                rows.push(("  Window".to_string(), w.clone()));
            }
            if let Some(n) = &c.next_maintenance {
                rows.push(("  Next Window".to_string(), n.clone()));
            }
            if let Some(t) = &c.maintenance_track {
                rows.push(("  Track".to_string(), t.clone()));
            }
            rows.push((
                "  Version Upgrade".to_string(),
                if c.allow_version_upgrade {
                    "✓ Allowed".to_string()
                } else {
                    "✗ Not allowed".to_string()
                },
            ));

            rows.push((String::new(), String::new()));
            rows.push(("Snapshots".to_string(), String::new())); // group header
            rows.push((
                "  Automated Retention".to_string(),
                if c.automated_retention_days > 0 {
                    format!("{} day(s)", c.automated_retention_days)
                } else {
                    "✗ Disabled".to_string()
                },
            ));
            if let Some(d) = c.manual_retention_days {
                rows.push((
                    "  Manual Retention".to_string(),
                    if d > 0 {
                        format!("{} day(s)", d)
                    } else {
                        "indefinite".to_string()
                    },
                ));
            }
            if let Some(s) = &c.snapshot_schedule {
                rows.push(("  Schedule".to_string(), s.clone()));
            }
            rows
        }
        RedshiftClusterDetailSection::Tags => {
            if c.tags.is_empty() {
                return vec![("".to_string(), "No tags".to_string())];
            }
            let mut rows: Vec<(String, String)> =
                c.tags.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            rows.sort_by(|a, b| a.0.cmp(&b.0));
            rows
        }
    }
}

pub(super) fn render_redshift_workgroup_split(
    app: &App,
    w: &RedshiftWorkgroup,
    area: Rect,
    frame: &mut Frame,
) {
    let subtitle = format!("{} · {}", w.capacity_label(), w.status);
    render_simple_split(
        app,
        area,
        frame,
        "Redshift Serverless Workgroup",
        &w.name,
        &subtitle,
        &descriptor_tabs(app, &crate::aws::services::redshift::REDSHIFT_WORKGROUP_SECTIONS),
    );
}

pub fn redshift_workgroup_section_lines(
    w: &RedshiftWorkgroup,
    section: RedshiftWorkgroupDetailSection,
) -> Vec<(String, String)> {
    match section {
        RedshiftWorkgroupDetailSection::Overview => {
            let mut rows = vec![
                ("Workgroup".to_string(), w.name.clone()),
                ("Status".to_string(), w.status.clone()),
                ("Namespace".to_string(), w.namespace_name.clone()),
            ];

            rows.push((String::new(), String::new()));
            rows.push(("Compute".to_string(), String::new())); // group header
            if let Some(b) = w.base_capacity {
                rows.push(("  Base Capacity".to_string(), format!("{} RPU", b)));
            }
            if let Some(m) = w.max_capacity {
                rows.push(("  Max Capacity".to_string(), format!("{} RPU", m)));
            }
            if let Some((status, level)) = &w.price_performance {
                rows.push((
                    "  Price-Performance".to_string(),
                    format!("{} (level {})", status, level),
                ));
            }

            if !w.config_parameters.is_empty() {
                rows.push((String::new(), String::new()));
                rows.push(("Config Parameters".to_string(), String::new())); // group header
                for (k, v) in &w.config_parameters {
                    rows.push((format!("  {}", k), v.clone()));
                }
            }

            rows.push((String::new(), String::new()));
            rows.push(("Other".to_string(), String::new())); // group header
            if let Some(c) = &w.created {
                rows.push(("  Created".to_string(), c.clone()));
            }
            if let Some(v) = &w.workgroup_version {
                rows.push(("  Version".to_string(), v.clone()));
            }
            if let Some(p) = &w.patch_version {
                rows.push(("  Patch".to_string(), p.clone()));
            }
            if let Some(t) = &w.track_name {
                rows.push(("  Track".to_string(), t.clone()));
            }
            if !w.workgroup_id.is_empty() {
                rows.push(("  Workgroup ID".to_string(), w.workgroup_id.clone()));
            }
            if !w.arn.is_empty() {
                rows.push(("  ARN".to_string(), w.arn.clone()));
            }
            rows
        }
        RedshiftWorkgroupDetailSection::Namespace => {
            if w.ns_status.is_none() && w.ns_db_name.is_none() && w.ns_arn.is_none() {
                return vec![(
                    "".to_string(),
                    "Namespace detail unavailable (redshift-serverless:ListNamespaces failed or the namespace was not found)"
                        .to_string(),
                )];
            }
            let mut rows = vec![("Namespace".to_string(), w.namespace_name.clone())];
            if let Some(s) = &w.ns_status {
                rows.push(("Status".to_string(), s.clone()));
            }
            if let Some(db) = &w.ns_db_name {
                rows.push(("Database".to_string(), db.clone()));
            }
            if let Some(u) = &w.ns_admin_username {
                rows.push(("Admin User".to_string(), u.clone()));
            }

            rows.push((String::new(), String::new()));
            rows.push(("Encryption".to_string(), String::new())); // group header
            match &w.ns_kms_key_id {
                Some(k) if k != "AWS_OWNED_KMS_KEY" => {
                    rows.push(("  KMS Key".to_string(), k.clone())) // jumpable
                }
                _ => rows.push(("  KMS Key".to_string(), "AWS-owned key".to_string())),
            }

            if !w.ns_iam_roles.is_empty() || w.ns_default_iam_role.is_some() {
                rows.push((String::new(), String::new()));
                rows.push(("IAM Roles".to_string(), String::new())); // group header
                if let Some(d) = &w.ns_default_iam_role {
                    rows.push(("  Default Role".to_string(), d.clone())); // jumpable
                }
                for r in &w.ns_iam_roles {
                    // The API wraps roles in "IamRole(applyStatus=…, iamRoleArn=…)"
                    // sometimes; show verbatim — plain ARNs still jump.
                    rows.push(("  Role".to_string(), r.clone()));
                }
            }

            rows.push((String::new(), String::new()));
            rows.push(("Audit Logging".to_string(), String::new())); // group header
            if w.ns_log_exports.is_empty() {
                rows.push(("  Log Exports".to_string(), "✗ None".to_string()));
            } else {
                for l in &w.ns_log_exports {
                    rows.push(("  Export".to_string(), l.clone()));
                }
            }

            rows.push((String::new(), String::new()));
            rows.push(("Other".to_string(), String::new())); // group header
            if let Some(c) = &w.ns_created {
                rows.push(("  Created".to_string(), c.clone()));
            }
            if let Some(id) = &w.ns_id {
                rows.push(("  Namespace ID".to_string(), id.clone()));
            }
            if let Some(arn) = &w.ns_arn {
                rows.push(("  ARN".to_string(), arn.clone()));
            }
            rows
        }
        RedshiftWorkgroupDetailSection::Network => {
            let mut rows = vec![(
                "Publicly Accessible".to_string(),
                if w.publicly_accessible {
                    "⚠ Yes".to_string()
                } else {
                    "✓ No".to_string()
                },
            )];
            rows.push((
                "Enhanced VPC Routing".to_string(),
                if w.enhanced_vpc_routing {
                    "✓ Enabled".to_string()
                } else {
                    "Disabled".to_string()
                },
            ));
            if let Some((addr, port)) = &w.endpoint {
                rows.push(("Endpoint".to_string(), addr.clone()));
                rows.push(("Port".to_string(), port.to_string()));
            }

            if !w.subnet_ids.is_empty() {
                rows.push((String::new(), String::new()));
                rows.push(("Subnets".to_string(), String::new())); // group header
                for s in &w.subnet_ids {
                    rows.push(("  Subnet".to_string(), s.clone())); // jumpable
                }
            }

            if !w.security_group_ids.is_empty() {
                rows.push((String::new(), String::new()));
                rows.push(("Security Groups".to_string(), String::new())); // group header
                for s in &w.security_group_ids {
                    rows.push(("  Security Group".to_string(), s.clone())); // jumpable
                }
            }
            rows
        }
    }
}
