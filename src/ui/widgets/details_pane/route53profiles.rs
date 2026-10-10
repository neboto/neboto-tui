use super::*;

// ── Route 53 Profiles split pane ─────────────────────────────────────────────

pub(super) fn render_route53_profile_split(
    app: &App,
    p: &crate::aws::services::route53profiles::Route53Profile,
    area: Rect,
    frame: &mut Frame,
) {
    render_waf_simple_split(
        app,
        "Route53 Profile",
        p.name(),
        &p.id,
        &p.share_status,
        &descriptor_tabs(app, &crate::aws::services::route53profiles::ROUTE53_PROFILE_SECTIONS),
        area,
        frame,
    );
}

pub fn route53_profile_section_lines(
    p: &crate::aws::services::route53profiles::Route53Profile,
    section: crate::aws::services::route53profiles::Route53ProfileDetailSection,
    state: Option<
        &crate::lazy::Lazy<Box<crate::aws::services::route53profiles::Route53ProfileDetail>>,
    >,
) -> Vec<(String, String)> {
    use crate::aws::services::route53profiles::Route53ProfileDetailSection as S;
    match section {
        S::Overview => {
            let mut rows = vec![
                ("Name".to_string(), p.name.clone()),
                ("ID".to_string(), p.id.clone()),
                ("ARN".to_string(), p.arn.clone()),
                ("Share Status".to_string(), p.share_status.clone()),
            ];
            match state {
                None | Some(crate::lazy::Lazy::Loading) => {
                    rows.push(("".to_string(), "".to_string()));
                    rows.push(("  Loading…".to_string(), "".to_string()));
                }
                Some(crate::lazy::Lazy::Error(err)) => {
                    rows.push(("".to_string(), "".to_string()));
                    rows.extend(error_rows(err));
                }
                Some(crate::lazy::Lazy::Loaded(d)) => {
                    rows.push(("".to_string(), "".to_string()));
                    rows.push(("Status".to_string(), d.status.clone()));
                    if !d.status_message.is_empty() {
                        rows.push(("Status Message".to_string(), d.status_message.clone()));
                    }
                    rows.push(("Owner".to_string(), d.owner_id.clone()));
                    if let Some(ct) = &d.creation_time {
                        rows.push(("Created".to_string(), ct.clone()));
                    }
                    if let Some(mt) = &d.modification_time {
                        rows.push(("Modified".to_string(), mt.clone()));
                    }
                }
            }
            rows
        }
        S::Vpcs => {
            let mut rows = vec![("".to_string(), "".to_string())];
            match state {
                None | Some(crate::lazy::Lazy::Loading) => {
                    rows.push(("  Loading…".to_string(), "".to_string()))
                }
                Some(crate::lazy::Lazy::Error(err)) => rows.extend(error_rows(err)),
                Some(crate::lazy::Lazy::Loaded(d)) if d.vpcs.is_empty() => {
                    rows.push(("  (no VPC associations)".to_string(), "".to_string()))
                }
                Some(crate::lazy::Lazy::Loaded(d)) => {
                    rows.push((format!("VPCs ({})", d.vpcs.len()), "".to_string()));
                    rows.push(("".to_string(), "".to_string()));
                    for v in &d.vpcs {
                        // vpc_arn carries the raw vpc- token (jumpable to VPC).
                        rows.push(("VPC".to_string(), v.vpc_arn.clone()));
                        rows.push(("Status".to_string(), v.status.clone()));
                        if !v.status_message.is_empty() {
                            rows.push(("Status Message".to_string(), v.status_message.clone()));
                        }
                        rows.push(("".to_string(), "".to_string()));
                    }
                }
            }
            rows
        }
        S::Resources => {
            let mut rows = vec![("".to_string(), "".to_string())];
            match state {
                None | Some(crate::lazy::Lazy::Loading) => {
                    rows.push(("  Loading…".to_string(), "".to_string()))
                }
                Some(crate::lazy::Lazy::Error(err)) => rows.extend(error_rows(err)),
                Some(crate::lazy::Lazy::Loaded(d)) if d.resources.is_empty() => {
                    rows.push(("  (no resources bundled into this profile)".to_string(), "".to_string()))
                }
                Some(crate::lazy::Lazy::Loaded(d)) => {
                    rows.push((format!("Resources ({})", d.resources.len()), "".to_string()));
                    rows.push(("".to_string(), "".to_string()));
                    for r in &d.resources {
                        // A private-hosted-zone or Resolver-rule ARN jumps via
                        // the generic classifier; DNS Firewall rule groups
                        // aren't modeled in this app, so those just display.
                        rows.push(("Resource".to_string(), r.resource_arn.clone()));
                        rows.push(("Type".to_string(), r.resource_type.clone()));
                        rows.push(("Status".to_string(), r.status.clone()));
                        if !r.status_message.is_empty() {
                            rows.push(("Status Message".to_string(), r.status_message.clone()));
                        }
                        rows.push(("".to_string(), "".to_string()));
                    }
                }
            }
            rows
        }
        S::Tags => match state {
            None | Some(crate::lazy::Lazy::Loading) => {
                vec![("  Loading…".to_string(), "".to_string())]
            }
            Some(crate::lazy::Lazy::Error(err)) => error_rows(err),
            Some(crate::lazy::Lazy::Loaded(d)) => bundle_tag_rows(&d.tags, d.tags_error.as_deref()),
        },
    }
}
