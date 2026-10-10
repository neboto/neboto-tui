use super::*;

// ── Route 53 Resolver ─────────────────────────────────────────────────────────

pub(super) fn render_resolver_endpoint_split(
    app: &App,
    e: &crate::aws::services::route53resolver::ResolverEndpoint,
    area: Rect,
    frame: &mut Frame,
) {
    render_waf_simple_split(
        app,
        "Resolver Endpoint",
        e.name(),
        &e.id,
        &e.direction,
        &descriptor_tabs(app, &crate::aws::services::route53resolver::RESOLVER_ENDPOINT_SECTIONS),
        area,
        frame,
    );
}

pub(super) fn render_resolver_rule_split(
    app: &App,
    r: &crate::aws::services::route53resolver::ResolverRule,
    area: Rect,
    frame: &mut Frame,
) {
    render_waf_simple_split(
        app,
        "Resolver Rule",
        r.name(),
        &r.id,
        &r.rule_type,
        &descriptor_tabs(app, &crate::aws::services::route53resolver::RESOLVER_RULE_SECTIONS),
        area,
        frame,
    );
}

/// `rules` are the sibling `ResolverRule` rows already in the list that
/// route through this endpoint (`resolver_endpoint_id == e.id`) — no fetch,
/// the VPC-pane pattern.
pub fn resolver_endpoint_section_lines(
    e: &crate::aws::services::route53resolver::ResolverEndpoint,
    section: crate::aws::services::route53resolver::ResolverEndpointDetailSection,
    state: Option<&crate::lazy::Lazy<Box<crate::aws::services::route53resolver::ResolverEndpointDetail>>>,
    rules: &[&crate::aws::services::route53resolver::ResolverRule],
) -> Vec<(String, String)> {
    use crate::aws::services::route53resolver::ResolverEndpointDetailSection as S;
    let flag = |b: Option<bool>| match b {
        Some(true) => "✓ enabled",
        Some(false) => "✗ disabled",
        None => "—",
    };
    match section {
        S::Details => {
            let mut rows = vec![
                ("Name".to_string(), e.name.clone()),
                ("ID".to_string(), e.id.clone()),
                ("Direction".to_string(), e.direction.clone()),
                ("Type".to_string(), e.endpoint_type.clone()),
                (
                    "Protocols".to_string(),
                    if e.protocols.is_empty() {
                        "Do53 (default)".to_string()
                    } else {
                        e.protocols.join(", ")
                    },
                ),
                ("Status".to_string(), e.status.clone()),
            ];
            if !e.status_message.is_empty() {
                rows.push(("Status Message".to_string(), e.status_message.clone()));
            }
            rows.push((
                "IP Address Count".to_string(),
                e.ip_address_count.to_string(),
            ));
            // host VPC id carries the raw vpc- token so Enter jumps to the VPC.
            rows.push(("Host VPC".to_string(), e.host_vpc_id.clone()));
            if !e.outpost_arn.is_empty() {
                rows.push(("Outpost".to_string(), e.outpost_arn.clone()));
                rows.push(("Instance Type".to_string(), e.preferred_instance_type.clone()));
            }
            // Feature flags: shown only when the API reported them, so an
            // endpoint from before these existed doesn't read as "disabled".
            let flags = [
                ("DNS64", e.dns64_enabled),
                ("IPv6 Internet Access", e.ipv6_internet_access_enabled),
                ("RNI Enhanced Metrics", e.rni_enhanced_metrics_enabled),
                ("Target Name Server Metrics", e.target_name_server_metrics_enabled),
            ];
            if flags.iter().any(|(_, v)| v.is_some()) {
                rows.push(("".to_string(), "".to_string()));
                rows.push(("Features".to_string(), "".to_string()));
                for (label, v) in flags {
                    if v.is_some() {
                        rows.push((label.to_string(), flag(v).to_string()));
                    }
                }
            }
            if !e.security_group_ids.is_empty() {
                rows.push(("".to_string(), "".to_string()));
                rows.push(("Security Groups".to_string(), "".to_string()));
                // each sg- id is its own row so Enter jumps to that group.
                for sg in &e.security_group_ids {
                    rows.push(("Security Group".to_string(), sg.clone()));
                }
            }
            if e.creation_time.is_some() || e.modification_time.is_some() {
                rows.push(("".to_string(), "".to_string()));
            }
            if let Some(ct) = &e.creation_time {
                rows.push(("Created".to_string(), ct.clone()));
            }
            if let Some(mt) = &e.modification_time {
                rows.push(("Modified".to_string(), mt.clone()));
            }
            rows
        }
        S::Rules => {
            let mut rows = vec![("".to_string(), "".to_string())];
            if !e.is_outbound() {
                rows.push((
                    "  — (only outbound endpoints carry rules)".to_string(),
                    "".to_string(),
                ));
                return rows;
            }
            if rules.is_empty() {
                rows.push((
                    "  (no loaded rules use this endpoint)".to_string(),
                    "".to_string(),
                ));
                return rows;
            }
            rows.push((format!("Rules ({})", rules.len()), "".to_string()));
            rows.push(("".to_string(), "".to_string()));
            for r in rules {
                // rule id carries the raw rslvr-rr- token (jumpable to the
                // Rules sub-tab via resource_jump_target's rslvr-rr- arm).
                rows.push((format!("{} {}", r.rule_type, r.domain_name), r.id.clone()));
            }
            rows
        }
        S::IpAddresses => {
            let mut rows = vec![("".to_string(), "".to_string())];
            match state {
                None | Some(crate::lazy::Lazy::Loading) => {
                    rows.push(("  Loading…".to_string(), "".to_string()))
                }
                Some(crate::lazy::Lazy::Error(err)) => rows.extend(error_rows(err)),
                Some(crate::lazy::Lazy::Loaded(d)) if d.ip_addresses.is_empty() => {
                    rows.push(("  (no IP addresses)".to_string(), "".to_string()))
                }
                Some(crate::lazy::Lazy::Loaded(d)) => {
                    rows.push((
                        format!("IP Addresses ({})", d.ip_addresses.len()),
                        "".to_string(),
                    ));
                    rows.push(("".to_string(), "".to_string()));
                    for ip in &d.ip_addresses {
                        let addr = if !ip.ip.is_empty() && !ip.ipv6.is_empty() {
                            format!("{} / {}", ip.ip, ip.ipv6)
                        } else if !ip.ipv6.is_empty() {
                            ip.ipv6.clone()
                        } else {
                            ip.ip.clone()
                        };
                        rows.push(("IP".to_string(), addr));
                        // subnet id carries the raw subnet- token (jumpable).
                        rows.push(("Subnet".to_string(), ip.subnet_id.clone()));
                        rows.push(("Status".to_string(), ip.status.clone()));
                        if !ip.status_message.is_empty() {
                            rows.push(("Status Message".to_string(), ip.status_message.clone()));
                        }
                        rows.push(("".to_string(), "".to_string()));
                    }
                }
            }
            rows
        }
        S::Tags => match state {
            None | Some(crate::lazy::Lazy::Loading) => vec![("  Loading…".to_string(), "".to_string())],
            Some(crate::lazy::Lazy::Error(err)) => error_rows(err),
            Some(crate::lazy::Lazy::Loaded(d)) => bundle_tag_rows(&d.tags, d.tags_error.as_deref()),
        },
    }
}

pub fn resolver_rule_section_lines(
    r: &crate::aws::services::route53resolver::ResolverRule,
    section: crate::aws::services::route53resolver::ResolverRuleDetailSection,
    state: Option<&crate::lazy::Lazy<Box<crate::aws::services::route53resolver::ResolverRuleDetail>>>,
) -> Vec<(String, String)> {
    use crate::aws::services::route53resolver::ResolverRuleDetailSection as S;
    match section {
        S::Details => {
            let mut rows = vec![
                ("Name".to_string(), r.name.clone()),
                ("ID".to_string(), r.id.clone()),
                ("Domain Name".to_string(), r.domain_name.clone()),
                ("Rule Type".to_string(), r.rule_type.clone()),
                ("Status".to_string(), r.status.clone()),
            ];
            if !r.status_message.is_empty() {
                rows.push(("Status Message".to_string(), r.status_message.clone()));
            }
            // FORWARD and DELEGATE rules both route through an outbound
            // endpoint. resolver_endpoint_id carries the raw rslvr- token
            // (jumpable to the Endpoints sub-tab via resource_jump_target's
            // rslvr- arm). Keying this on FORWARD alone once hid the endpoint
            // of every DELEGATE rule.
            if r.uses_endpoint() {
                if let Some(ep) = &r.resolver_endpoint_id {
                    rows.push(("Resolver Endpoint".to_string(), ep.clone()));
                }
            }
            if let Some(dr) = &r.delegation_record {
                rows.push(("Delegation Record".to_string(), dr.clone()));
            }
            if !r.share_status.is_empty() {
                rows.push(("Share Status".to_string(), r.share_status.clone()));
            }
            if !r.owner_id.is_empty() {
                rows.push(("Owner".to_string(), r.owner_id.clone()));
            }
            if r.creation_time.is_some() || r.modification_time.is_some() {
                rows.push(("".to_string(), "".to_string()));
            }
            if let Some(ct) = &r.creation_time {
                rows.push(("Created".to_string(), ct.clone()));
            }
            if let Some(mt) = &r.modification_time {
                rows.push(("Modified".to_string(), mt.clone()));
            }
            rows
        }
        S::Targets => {
            let mut rows = vec![("".to_string(), "".to_string())];
            // Match on the raw string so an unknown future rule type still
            // renders (as "no targets") instead of being mislabelled.
            match r.rule_type.as_str() {
                "DELEGATE" => {
                    rows.push((
                        "  — (delegation rule, no targets — see Delegation Record)".to_string(),
                        "".to_string(),
                    ));
                    return rows;
                }
                "SYSTEM" | "RECURSIVE" => {
                    rows.push((
                        format!("  — ({} rule, no targets)", r.rule_type.to_lowercase()),
                        "".to_string(),
                    ));
                    return rows;
                }
                _ => {}
            }
            if r.targets.is_empty() {
                rows.push(("  (no targets)".to_string(), "".to_string()));
                return rows;
            }
            rows.push((format!("Targets ({})", r.targets.len()), "".to_string()));
            rows.push(("".to_string(), "".to_string()));
            for t in &r.targets {
                rows.push((format!("  {}", t), "".to_string()));
            }
            rows
        }
        S::Associations => {
            let mut rows = vec![("".to_string(), "".to_string())];
            match state {
                None | Some(crate::lazy::Lazy::Loading) => {
                    rows.push(("  Loading…".to_string(), "".to_string()))
                }
                Some(crate::lazy::Lazy::Error(err)) => rows.extend(error_rows(err)),
                Some(crate::lazy::Lazy::Loaded(d)) if d.associations.is_empty() => {
                    rows.push(("  (no associations)".to_string(), "".to_string()))
                }
                Some(crate::lazy::Lazy::Loaded(d)) => {
                    rows.push((
                        format!("VPC Associations ({})", d.associations.len()),
                        "".to_string(),
                    ));
                    rows.push(("".to_string(), "".to_string()));
                    for a in &d.associations {
                        // vpc id carries the raw vpc- token (jumpable to VPC).
                        rows.push(("VPC".to_string(), a.vpc_id.clone()));
                        rows.push(("Status".to_string(), a.status.clone()));
                        rows.push(("".to_string(), "".to_string()));
                    }
                }
            }
            rows
        }
        S::Tags => match state {
            None | Some(crate::lazy::Lazy::Loading) => vec![("  Loading…".to_string(), "".to_string())],
            Some(crate::lazy::Lazy::Error(err)) => error_rows(err),
            Some(crate::lazy::Lazy::Loaded(d)) => bundle_tag_rows(&d.tags, d.tags_error.as_deref()),
        },
    }
}
