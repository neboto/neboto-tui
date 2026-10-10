use super::*;

// ═══════════════════════════════════════════════════════════════════════════════
// Bedrock AgentCore
//
// Fifteen split panes — grep `render_agentcore_*_split` for the current set.
// Every `List*` in this service returns a thin summary, so each pane's eager
// rows are short and the substance arrives through the lazy bundles — the
// three-arm `Lazy` match below is the load-bearing part of all of them.
// ═══════════════════════════════════════════════════════════════════════════════

/// The `None | Loading` / `Error` / empty-`Loaded` preamble every AgentCore
/// list section shares. Returns `Some(rows)` when the section has nothing of
/// its own to draw yet.
pub(super) fn agentcore_list_preamble<T>(
    state: Option<&crate::lazy::Lazy<Vec<T>>>,
    empty: &str,
) -> Option<Vec<(String, String)>> {
    match state {
        None | Some(crate::lazy::Lazy::Loading) => Some(vec![
            (String::new(), String::new()),
            ("  Loading…".to_string(), String::new()),
        ]),
        Some(crate::lazy::Lazy::Error(err)) => {
            let mut rows = vec![(String::new(), String::new())];
            rows.extend(error_rows(err));
            Some(rows)
        }
        Some(crate::lazy::Lazy::Loaded(v)) if v.is_empty() => Some(vec![
            (String::new(), String::new()),
            (format!("  ({})", empty), String::new()),
        ]),
        Some(crate::lazy::Lazy::Loaded(_)) => None,
    }
}

/// `("  label", value)` when the value is non-empty — the indented
/// content-row form used inside the per-item blocks below.
pub(super) fn ac_row(label: &str, value: &str) -> Option<(String, String)> {
    (!value.is_empty()).then(|| (format!("  {}", label), value.to_string()))
}

pub(super) fn ac_list_rows(rows: &mut Vec<(String, String)>, label: &str, values: &[String]) {
    if values.is_empty() {
        return;
    }
    rows.push((label.to_string(), values[0].clone()));
    for v in &values[1..] {
        rows.push((String::new(), v.clone()));
    }
}

// ── Runtime ───────────────────────────────────────────────────────────────────

pub(super) fn render_agentcore_runtime_split(
    app: &App,
    r: &crate::aws::services::agentcore::AgentCoreRuntime,
    area: Rect,
    frame: &mut Frame,
) {
    render_waf_simple_split(
        app,
        "AgentCore Runtime",
        r.name(),
        &r.runtime_id,
        &r.status,
        &descriptor_tabs(
            app,
            &crate::aws::services::agentcore::AGENTCORE_RUNTIME_SECTIONS,
        ),
        area,
        frame,
    );
}

// Each parameter is one lazy map this pane reads; grouping them into a struct
// no other AgentCore pane uses would hide that rather than simplify it.
#[allow(clippy::too_many_arguments)]
pub fn agentcore_runtime_section_lines(
    r: &crate::aws::services::agentcore::AgentCoreRuntime,
    section: crate::aws::services::agentcore::AgentCoreRuntimeDetailSection,
    detail: Option<&crate::lazy::Lazy<Box<crate::aws::services::agentcore::AgentCoreRuntimeDetail>>>,
    endpoints: Option<&crate::lazy::Lazy<Vec<crate::aws::services::agentcore::AgentCoreEndpoint>>>,
    versions: Option<
        &crate::lazy::Lazy<Vec<crate::aws::services::agentcore::AgentCoreRuntimeVersion>>,
    >,
    agent_card: Option<&crate::lazy::Lazy<String>>,
    resource_policy: Option<
        &crate::lazy::Lazy<crate::aws::services::agentcore::AgentCoreResourcePolicy>,
    >,
    account_id: &str,
    tags: Option<&crate::lazy::Lazy<Vec<(String, String)>>>,
) -> Vec<(String, String)> {
    use crate::aws::services::agentcore::AgentCoreRuntimeDetailSection as S;
    use crate::lazy::Lazy;

    // The four detail-backed sections share one bundle; this pulls it out or
    // hands back the Loading/Error rows to render instead.
    let bundle = |rows: &mut Vec<(String, String)>| -> Option<
        &crate::aws::services::agentcore::AgentCoreRuntimeDetail,
    > {
        match detail {
            None | Some(Lazy::Loading) => {
                rows.push((String::new(), String::new()));
                rows.push(("  Loading…".to_string(), String::new()));
                None
            }
            Some(Lazy::Error(err)) => {
                rows.push((String::new(), String::new()));
                rows.extend(error_rows(err));
                None
            }
            Some(Lazy::Loaded(d)) => Some(d.as_ref()),
        }
    };

    match section {
        S::Tags => agentcore_tags_lines(tags),
        S::Overview => {
            let mut rows = vec![
                ("Name".to_string(), r.name.clone()),
                ("ID".to_string(), r.runtime_id.clone()),
                ("ARN".to_string(), r.arn.clone()),
                ("Version".to_string(), r.version.clone()),
                ("Status".to_string(), r.status.clone()),
                ("Last Updated".to_string(), r.last_updated.clone()),
            ];
            if !r.description.is_empty() {
                rows.push(("Description".to_string(), r.description.clone()));
            }
            if let Some(d) = bundle(&mut rows) {
                rows.push((String::new(), String::new()));
                rows.push(("Created".to_string(), d.created.clone()));
                rows.push(("Execution Role".to_string(), d.role_arn.clone()));
                if !d.workload_identity_arn.is_empty() {
                    rows.push((
                        "Workload Identity".to_string(),
                        d.workload_identity_arn.clone(),
                    ));
                }
                if !d.failure_reason.is_empty() {
                    rows.push((String::new(), String::new()));
                    rows.push(("Failure".to_string(), String::new()));
                    rows.push((format!("  ⚠ {}", d.failure_reason), String::new()));
                }
            }
            rows
        }
        S::Artifact => {
            let mut rows = Vec::new();
            if let Some(d) = bundle(&mut rows) {
                rows.push(("Artifact".to_string(), String::new()));
                rows.push(("".to_string(), "".to_string()));
                if d.artifact_kind.is_empty() {
                    rows.push(("  (no artifact reported)".to_string(), String::new()));
                } else {
                    rows.push(("Type".to_string(), d.artifact_kind.clone()));
                    // A container URI is an ECR image reference — the generic
                    // classifier turns it into a jump to the repo.
                    if !d.artifact_value.is_empty() {
                        rows.push(("Container Image".to_string(), d.artifact_value.clone()));
                    }
                }
                rows.push((String::new(), String::new()));
                rows.push(("Protocol".to_string(), String::new()));
                rows.push((String::new(), String::new()));
                rows.push((
                    "Server Protocol".to_string(),
                    if d.server_protocol.is_empty() {
                        "HTTP (default)".to_string()
                    } else {
                        d.server_protocol.clone()
                    },
                ));
                rows.push((String::new(), String::new()));
                rows.push(("Lifecycle".to_string(), String::new()));
                rows.push((String::new(), String::new()));
                rows.push((
                    "Idle Session Timeout".to_string(),
                    d.idle_session_timeout
                        .map(|v| format!("{}s", v))
                        .unwrap_or_else(|| "default".to_string()),
                ));
                rows.push((
                    "Max Lifetime".to_string(),
                    d.max_lifetime
                        .map(|v| format!("{}s", v))
                        .unwrap_or_else(|| "default".to_string()),
                ));
                if !d.filesystem_kinds.is_empty() {
                    rows.push((String::new(), String::new()));
                    rows.push((
                        format!("Filesystems ({})", d.filesystem_kinds.len()),
                        String::new(),
                    ));
                    rows.push((String::new(), String::new()));
                    for f in &d.filesystem_kinds {
                        rows.push((format!("  {}", f), String::new()));
                    }
                }
            }
            rows
        }
        S::Network => {
            let mut rows = Vec::new();
            if let Some(d) = bundle(&mut rows) {
                rows.push(("Network Mode".to_string(), d.network_mode.clone()));
                if d.subnets.is_empty() && d.security_groups.is_empty() {
                    rows.push((String::new(), String::new()));
                    rows.push((
                        "  (public networking — no VPC configuration)".to_string(),
                        String::new(),
                    ));
                } else {
                    rows.push((String::new(), String::new()));
                    ac_list_rows(&mut rows, "Subnets", &d.subnets);
                    ac_list_rows(&mut rows, "Security Groups", &d.security_groups);
                    if let Some(req) = d.require_s3_endpoint {
                        rows.push((
                            "Requires S3 Endpoint".to_string(),
                            if req { "✓ yes" } else { "✗ no" }.to_string(),
                        ));
                    }
                }
            }
            rows
        }
        S::Auth => {
            let mut rows = Vec::new();
            if let Some(d) = bundle(&mut rows) {
                rows.push((
                    "Authorizer".to_string(),
                    if d.authorizer_kind.is_empty() {
                        "IAM (SigV4)".to_string()
                    } else {
                        d.authorizer_kind.clone()
                    },
                ));
                if !d.jwt_discovery_url.is_empty() {
                    rows.push(("Discovery URL".to_string(), d.jwt_discovery_url.clone()));
                }
                ac_list_rows(&mut rows, "Allowed Audience", &d.jwt_allowed_audience);
                ac_list_rows(&mut rows, "Allowed Clients", &d.jwt_allowed_clients);
                ac_list_rows(&mut rows, "Allowed Scopes", &d.jwt_allowed_scopes);
                if !d.workload_identity_arn.is_empty() {
                    rows.push((String::new(), String::new()));
                    rows.push((
                        "Workload Identity".to_string(),
                        d.workload_identity_arn.clone(),
                    ));
                }
                if !d.request_header_allowlist.is_empty() {
                    rows.push((String::new(), String::new()));
                    rows.push(("Request Headers".to_string(), String::new()));
                    rows.push((String::new(), String::new()));
                    for h in &d.request_header_allowlist {
                        rows.push((format!("  {}", h), String::new()));
                    }
                }
            }
            rows
        }
        S::ResourcePolicy => {
            agentcore_resource_policy_lines(resource_policy, account_id, "runtime")
        }
        S::Env => {
            let mut rows = Vec::new();
            if let Some(d) = bundle(&mut rows) {
                if d.environment_variables.is_empty() {
                    rows.push((String::new(), String::new()));
                    rows.push(("  (no environment variables)".to_string(), String::new()));
                } else {
                    rows.push((
                        format!("Environment ({})", d.environment_variables.len()),
                        String::new(),
                    ));
                    rows.push((String::new(), String::new()));
                    for (k, v) in &d.environment_variables {
                        rows.push((k.clone(), v.clone()));
                    }
                }
            }
            rows
        }
        S::Endpoints => {
            if let Some(rows) = agentcore_list_preamble(endpoints, "no endpoints") {
                return rows;
            }
            let Some(Lazy::Loaded(eps)) = endpoints else {
                return Vec::new();
            };
            let mut rows = vec![(format!("Endpoints ({})", eps.len()), String::new())];
            rows.push((String::new(), String::new()));
            for e in eps {
                rows.push(("Endpoint".to_string(), e.name.clone()));
                rows.extend(ac_row("Status", &e.status));
                rows.extend(ac_row("Live Version", &e.live_version));
                rows.extend(ac_row("Target Version", &e.target_version));
                rows.extend(ac_row("Description", &e.description));
                rows.extend(ac_row("Updated", &e.last_updated));
                // The log group `t` tails for this endpoint — spelled out so
                // the deterministic naming is visible, not folklore.
                rows.push((
                    "  Log Group".to_string(),
                    r.log_group_for(&e.name),
                ));
                rows.push((String::new(), String::new()));
            }
            rows
        }
        S::AgentCard => {
            // The only section in the app that does not fetch on entry: the
            // card is served by the agent container, so asking for one can
            // cold-start it. `x` opts in. Keep this an explicit hint rather
            // than a `Loading…` row — nothing is loading.
            let mut rows = vec![(
                "Protocol".to_string(),
                match detail {
                    Some(Lazy::Loaded(d)) if !d.server_protocol.is_empty() => {
                        d.server_protocol.clone()
                    }
                    _ => "unknown".to_string(),
                },
            )];
            let a2a = matches!(detail, Some(Lazy::Loaded(d))
                if d.server_protocol.eq_ignore_ascii_case("A2A"));
            rows.push((String::new(), String::new()));
            match agent_card {
                None => {
                    if a2a {
                        rows.push((
                            "  · not fetched — x to request the agent card".to_string(),
                            String::new(),
                        ));
                    } else {
                        // Non-A2A runtimes serve no card; say so rather than
                        // inviting a call that will just fail.
                        rows.push((
                            "  · agent cards are served by A2A runtimes only".to_string(),
                            String::new(),
                        ));
                        rows.push((
                            "  · x requests one anyway, if you want to check".to_string(),
                            String::new(),
                        ));
                    }
                    rows.push((String::new(), String::new()));
                    rows.push((
                        "  the fetch reaches the running agent and may start it".to_string(),
                        String::new(),
                    ));
                }
                Some(Lazy::Loading) => rows.push(("  Loading…".to_string(), String::new())),
                Some(Lazy::Error(err)) => rows.extend(error_rows(err)),
                Some(Lazy::Loaded(card)) => {
                    rows.push(("Agent Card".to_string(), String::new()));
                    rows.push((String::new(), String::new()));
                    for line in card.lines() {
                        rows.push((format!("  {}", line), String::new()));
                    }
                    rows.push((String::new(), String::new()));
                    rows.push((
                        "  · e opens the raw card in $EDITOR".to_string(),
                        String::new(),
                    ));
                }
            }
            rows
        }
        S::Versions => {
            if let Some(rows) = agentcore_list_preamble(versions, "no other versions") {
                return rows;
            }
            let Some(Lazy::Loaded(vs)) = versions else {
                return Vec::new();
            };
            let mut rows = vec![(format!("Versions ({})", vs.len()), String::new())];
            rows.push((String::new(), String::new()));
            for v in vs {
                let live = if v.version == r.version { "  ← current" } else { "" };
                rows.push(("Version".to_string(), format!("{}{}", v.version, live)));
                rows.extend(ac_row("Status", &v.status));
                rows.extend(ac_row("Description", &v.description));
                rows.extend(ac_row("Updated", &v.last_updated));
                rows.push((String::new(), String::new()));
            }
            rows
        }
    }
}

// ── Gateway ───────────────────────────────────────────────────────────────────

pub(super) fn render_agentcore_gateway_split(
    app: &App,
    g: &crate::aws::services::agentcore::AgentCoreGateway,
    area: Rect,
    frame: &mut Frame,
) {
    render_waf_simple_split(
        app,
        "AgentCore Gateway",
        g.name(),
        &g.gateway_id,
        &g.status,
        &descriptor_tabs(
            app,
            &crate::aws::services::agentcore::AGENTCORE_GATEWAY_SECTIONS,
        ),
        area,
        frame,
    );
}

// Each parameter is one lazy map this pane reads; grouping them into a struct
// no other AgentCore pane uses would hide that rather than simplify it.
#[allow(clippy::too_many_arguments)]
pub fn agentcore_gateway_section_lines(
    g: &crate::aws::services::agentcore::AgentCoreGateway,
    section: crate::aws::services::agentcore::AgentCoreGatewayDetailSection,
    detail: Option<&crate::lazy::Lazy<Box<crate::aws::services::agentcore::AgentCoreGatewayDetail>>>,
    targets: Option<&crate::lazy::Lazy<Vec<crate::aws::services::agentcore::AgentCoreTarget>>>,
    rules: Option<&crate::lazy::Lazy<Vec<crate::aws::services::agentcore::AgentCoreGatewayRule>>>,
    resource_policy: Option<
        &crate::lazy::Lazy<crate::aws::services::agentcore::AgentCoreResourcePolicy>,
    >,
    account_id: &str,
    tags: Option<&crate::lazy::Lazy<Vec<(String, String)>>>,
) -> Vec<(String, String)> {
    use crate::aws::services::agentcore::AgentCoreGatewayDetailSection as S;
    use crate::lazy::Lazy;

    let bundle = |rows: &mut Vec<(String, String)>| -> Option<
        &crate::aws::services::agentcore::AgentCoreGatewayDetail,
    > {
        match detail {
            None | Some(Lazy::Loading) => {
                rows.push((String::new(), String::new()));
                rows.push(("  Loading…".to_string(), String::new()));
                None
            }
            Some(Lazy::Error(err)) => {
                rows.push((String::new(), String::new()));
                rows.extend(error_rows(err));
                None
            }
            Some(Lazy::Loaded(d)) => Some(d.as_ref()),
        }
    };

    match section {
        S::Tags => agentcore_tags_lines(tags),
        S::Overview => {
            let mut rows = vec![
                ("Name".to_string(), g.name.clone()),
                ("ID".to_string(), g.gateway_id.clone()),
                ("Status".to_string(), g.status.clone()),
                ("Protocol".to_string(), g.protocol_type.clone()),
                ("Created".to_string(), g.created.clone()),
                ("Updated".to_string(), g.updated.clone()),
            ];
            if !g.description.is_empty() {
                rows.push(("Description".to_string(), g.description.clone()));
            }
            if let Some(d) = bundle(&mut rows) {
                rows.push((String::new(), String::new()));
                rows.push(("ARN".to_string(), d.arn.clone()));
                if !d.url.is_empty() {
                    rows.push(("Endpoint URL".to_string(), d.url.clone()));
                }
                if !d.role_arn.is_empty() {
                    rows.push(("Role".to_string(), d.role_arn.clone()));
                }
                if !d.exception_level.is_empty() {
                    rows.push(("Exception Level".to_string(), d.exception_level.clone()));
                }
                if !d.mcp_supported_versions.is_empty()
                    || !d.mcp_search_type.is_empty()
                    || !d.mcp_instructions.is_empty()
                {
                    rows.push((String::new(), String::new()));
                    rows.push(("MCP".to_string(), String::new()));
                    rows.push((String::new(), String::new()));
                    ac_list_rows(&mut rows, "Supported Versions", &d.mcp_supported_versions);
                    if !d.mcp_search_type.is_empty() {
                        rows.push(("Search Type".to_string(), d.mcp_search_type.clone()));
                    }
                    if !d.mcp_instructions.is_empty() {
                        rows.push(("Instructions".to_string(), d.mcp_instructions.clone()));
                    }
                }
                if !d.status_reasons.is_empty() {
                    rows.push((String::new(), String::new()));
                    rows.push(("Status Reasons".to_string(), String::new()));
                    rows.push((String::new(), String::new()));
                    for r in &d.status_reasons {
                        rows.push((format!("  ⚠ {}", r), String::new()));
                    }
                }
            }
            rows
        }
        S::Auth => {
            let mut rows = vec![("Authorizer Type".to_string(), g.authorizer_type.clone())];
            if let Some(d) = bundle(&mut rows) {
                if !d.authorizer_kind.is_empty() {
                    rows.push(("Authorizer".to_string(), d.authorizer_kind.clone()));
                }
                if !d.jwt_discovery_url.is_empty() {
                    rows.push(("Discovery URL".to_string(), d.jwt_discovery_url.clone()));
                }
                ac_list_rows(&mut rows, "Allowed Audience", &d.jwt_allowed_audience);
                ac_list_rows(&mut rows, "Allowed Clients", &d.jwt_allowed_clients);
                ac_list_rows(&mut rows, "Allowed Scopes", &d.jwt_allowed_scopes);
                if !d.workload_identity_arn.is_empty() {
                    rows.push((String::new(), String::new()));
                    rows.push((
                        "Workload Identity".to_string(),
                        d.workload_identity_arn.clone(),
                    ));
                }
            }
            rows
        }
        S::ResourcePolicy => {
            agentcore_resource_policy_lines(resource_policy, account_id, "gateway")
        }
        S::Targets => {
            if let Some(rows) = agentcore_list_preamble(targets, "no targets") {
                return rows;
            }
            let Some(Lazy::Loaded(ts)) = targets else {
                return Vec::new();
            };
            let capped = ts.len() >= crate::aws::services::agentcore::MAX_GATEWAY_TARGETS;
            let mut rows = vec![(
                format!(
                    "Targets ({}{})",
                    ts.len(),
                    if capped { ", capped" } else { "" }
                ),
                String::new(),
            )];
            rows.push((String::new(), String::new()));
            for t in ts {
                rows.push(("Target".to_string(), t.name.clone()));
                rows.extend(ac_row("ID", &t.target_id));
                rows.extend(ac_row("Status", &t.status));
                rows.extend(ac_row("Type", &t.target_type));
                rows.extend(ac_row("Backend", &t.backend_kind));
                // Lambda ARNs / API Gateway ids here jump through the generic
                // ARN classifier.
                rows.extend(ac_row("Backend Ref", &t.backend_value));
                rows.extend(ac_row("Listing Mode", &t.listing_mode));
                if let Some(p) = t.priority {
                    rows.push(("  Priority".to_string(), p.to_string()));
                }
                if !t.credential_provider_types.is_empty() {
                    rows.push((
                        "  Credentials".to_string(),
                        t.credential_provider_types.join(", "),
                    ));
                }
                rows.extend(ac_row("Last Synchronized", &t.last_synchronized));
                rows.extend(ac_row("Description", &t.description));
                for r in &t.status_reasons {
                    rows.push((format!("  ⚠ {}", r), String::new()));
                }
                rows.push((String::new(), String::new()));
            }
            rows
        }
        S::Rules => {
            if let Some(rows) = agentcore_list_preamble(rules, "no routing rules") {
                return rows;
            }
            let Some(Lazy::Loaded(rs)) = rules else {
                return Vec::new();
            };
            let mut rows = vec![(format!("Rules ({})", rs.len()), String::new())];
            rows.push((String::new(), String::new()));
            for r in rs {
                rows.push(("Rule".to_string(), r.rule_id.clone()));
                rows.push(("  Priority".to_string(), r.priority.to_string()));
                rows.extend(ac_row("Status", &r.status));
                rows.push((
                    "  Conditions / Actions".to_string(),
                    format!("{} / {}", r.condition_count, r.action_count),
                ));
                if r.system_managed {
                    rows.push(("  Managed By".to_string(), "AWS".to_string()));
                }
                rows.extend(ac_row("Description", &r.description));
                rows.extend(ac_row("Created", &r.created));
                rows.push((String::new(), String::new()));
            }
            rows
        }
        S::Security => {
            let mut rows = Vec::new();
            if let Some(d) = bundle(&mut rows) {
                rows.push((
                    "Encryption".to_string(),
                    if d.kms_key_arn.is_empty() {
                        "AWS owned key".to_string()
                    } else {
                        String::new()
                    },
                ));
                if !d.kms_key_arn.is_empty() {
                    rows.push(("KMS Key".to_string(), d.kms_key_arn.clone()));
                }
                rows.push((String::new(), String::new()));
                rows.push(("WAF".to_string(), String::new()));
                rows.push((String::new(), String::new()));
                if d.web_acl_arn.is_empty() {
                    rows.push(("  (no web ACL associated)".to_string(), String::new()));
                } else {
                    rows.push(("Web ACL".to_string(), d.web_acl_arn.clone()));
                    if !d.waf_failure_mode.is_empty() {
                        rows.push(("Failure Mode".to_string(), d.waf_failure_mode.clone()));
                    }
                }
                rows.push((String::new(), String::new()));
                rows.push(("Policy Engine".to_string(), String::new()));
                rows.push((String::new(), String::new()));
                if d.policy_engine_arn.is_empty() {
                    rows.push(("  (no policy engine attached)".to_string(), String::new()));
                } else {
                    rows.push(("Engine".to_string(), d.policy_engine_arn.clone()));
                    rows.push(("Mode".to_string(), d.policy_engine_mode.clone()));
                }
            }
            rows
        }
        S::Interceptors => {
            let mut rows = Vec::new();
            if let Some(d) = bundle(&mut rows) {
                rows.push((
                    "Custom Transform".to_string(),
                    if d.custom_transform {
                        "✓ configured"
                    } else {
                        "✗ none"
                    }
                    .to_string(),
                ));
                rows.push((String::new(), String::new()));
                if d.interceptors.is_empty() {
                    rows.push(("  (no interceptors)".to_string(), String::new()));
                } else {
                    rows.push((
                        format!("Interceptors ({})", d.interceptors.len()),
                        String::new(),
                    ));
                    rows.push((String::new(), String::new()));
                    for (points, note) in &d.interceptors {
                        rows.push(("Interception Points".to_string(), points.clone()));
                        rows.extend(ac_row("Interceptor", note));
                        rows.push((String::new(), String::new()));
                    }
                }
            }
            rows
        }
    }
}

// ── Memory ────────────────────────────────────────────────────────────────────

pub(super) fn render_agentcore_memory_split(
    app: &App,
    m: &crate::aws::services::agentcore::AgentCoreMemory,
    area: Rect,
    frame: &mut Frame,
) {
    render_waf_simple_split(
        app,
        "AgentCore Memory",
        m.name(),
        &m.memory_id,
        &m.status,
        &descriptor_tabs(
            app,
            &crate::aws::services::agentcore::AGENTCORE_MEMORY_SECTIONS,
        ),
        area,
        frame,
    );
}

pub fn agentcore_memory_section_lines(
    m: &crate::aws::services::agentcore::AgentCoreMemory,
    section: crate::aws::services::agentcore::AgentCoreMemoryDetailSection,
    detail: Option<&crate::lazy::Lazy<Box<crate::aws::services::agentcore::AgentCoreMemoryDetail>>>,
    actors: Option<&crate::lazy::Lazy<Vec<String>>>,
    sessions: Option<&crate::lazy::Lazy<Vec<crate::aws::services::agentcore::AgentCoreMemorySession>>>,
    tags: Option<&crate::lazy::Lazy<Vec<(String, String)>>>,
) -> Vec<(String, String)> {
    use crate::aws::services::agentcore::AgentCoreMemoryDetailSection as S;
    use crate::lazy::Lazy;

    let bundle = |rows: &mut Vec<(String, String)>| -> Option<
        &crate::aws::services::agentcore::AgentCoreMemoryDetail,
    > {
        match detail {
            None | Some(Lazy::Loading) => {
                rows.push((String::new(), String::new()));
                rows.push(("  Loading…".to_string(), String::new()));
                None
            }
            Some(Lazy::Error(err)) => {
                rows.push((String::new(), String::new()));
                rows.extend(error_rows(err));
                None
            }
            Some(Lazy::Loaded(d)) => Some(d.as_ref()),
        }
    };

    match section {
        S::Tags => agentcore_tags_lines(tags),
        S::Overview => {
            let mut rows = vec![
                ("ID".to_string(), m.memory_id.clone()),
                ("ARN".to_string(), m.arn.clone()),
                ("Status".to_string(), m.status.clone()),
                ("Created".to_string(), m.created.clone()),
                ("Updated".to_string(), m.updated.clone()),
            ];
            if !m.managed_by.is_empty() {
                // The owning runtime — jumpable back through the AgentCore ARN
                // classifier.
                rows.push(("Managed By".to_string(), m.managed_by.clone()));
            }
            if let Some(d) = bundle(&mut rows) {
                rows.push((String::new(), String::new()));
                rows.push(("Name".to_string(), d.name.clone()));
                if !d.description.is_empty() {
                    rows.push(("Description".to_string(), d.description.clone()));
                }
                rows.push((
                    "Event Expiry".to_string(),
                    format!("{} days", d.event_expiry_days),
                ));
                if !d.execution_role_arn.is_empty() {
                    rows.push(("Execution Role".to_string(), d.execution_role_arn.clone()));
                }
                rows.push((
                    "Encryption".to_string(),
                    if d.encryption_key_arn.is_empty() {
                        "AWS owned key".to_string()
                    } else {
                        String::new()
                    },
                ));
                if !d.encryption_key_arn.is_empty() {
                    rows.push(("KMS Key".to_string(), d.encryption_key_arn.clone()));
                }
                if !d.failure_reason.is_empty() {
                    rows.push((String::new(), String::new()));
                    rows.push((format!("  ⚠ {}", d.failure_reason), String::new()));
                }
            }
            rows
        }
        S::Strategies => {
            let mut rows = Vec::new();
            if let Some(d) = bundle(&mut rows) {
                if d.strategies.is_empty() {
                    rows.push((String::new(), String::new()));
                    rows.push((
                        "  (short-term memory only — no long-term strategies)".to_string(),
                        String::new(),
                    ));
                } else {
                    rows.push((
                        format!("Strategies ({})", d.strategies.len()),
                        String::new(),
                    ));
                    rows.push((String::new(), String::new()));
                    for s in &d.strategies {
                        rows.push(("Strategy".to_string(), s.name.clone()));
                        rows.extend(ac_row("ID", &s.strategy_id));
                        rows.extend(ac_row("Status", &s.status));
                        rows.extend(ac_row("Description", &s.description));
                        if !s.configured.is_empty() {
                            rows.push(("  Configured".to_string(), s.configured.join(", ")));
                        }
                        for n in &s.namespace_templates {
                            rows.push(("  Namespace".to_string(), n.clone()));
                        }
                        // Older strategies only populate the retired
                        // `namespaces` field; show those too when the current
                        // one came back empty.
                        if s.namespace_templates.is_empty() {
                            for n in &s.namespaces {
                                rows.push(("  Namespace".to_string(), n.clone()));
                            }
                        }
                        rows.push((String::new(), String::new()));
                    }
                }
            }
            rows
        }
        S::Indexing => {
            let mut rows = Vec::new();
            if let Some(d) = bundle(&mut rows) {
                if d.indexed_keys.is_empty() {
                    rows.push(("  (no indexed keys)".to_string(), String::new()));
                } else {
                    rows.push((
                        format!("Indexed Keys ({})", d.indexed_keys.len()),
                        String::new(),
                    ));
                    rows.push((String::new(), String::new()));
                    for k in &d.indexed_keys {
                        rows.push((format!("  {}", k), String::new()));
                    }
                }
                rows.push((String::new(), String::new()));
                rows.push((
                    "Stream Delivery".to_string(),
                    if d.stream_delivery_count == 0 {
                        "✗ none".to_string()
                    } else {
                        format!("{} resource(s)", d.stream_delivery_count)
                    },
                ));
            }
            rows
        }
        S::Actors => {
            if let Some(rows) = agentcore_list_preamble(actors, "no actors have written to this store") {
                return rows;
            }
            let Some(Lazy::Loaded(a)) = actors else {
                return Vec::new();
            };
            let capped = a.len() >= crate::aws::services::agentcore::MAX_MEMORY_ACTORS;
            let mut rows = vec![(
                format!("Actors ({}{})", a.len(), if capped { ", capped" } else { "" }),
                String::new(),
            )];
            rows.push((String::new(), String::new()));
            for id in a {
                rows.push((format!("  {}", id), String::new()));
            }
            rows
        }
        S::Sessions => {
            if let Some(rows) = agentcore_list_preamble(sessions, "no sessions") {
                return rows;
            }
            let Some(Lazy::Loaded(s)) = sessions else {
                return Vec::new();
            };
            let capped = s.len() >= crate::aws::services::agentcore::MAX_MEMORY_SESSIONS;
            let mut rows = vec![(
                format!(
                    "Sessions ({}{})",
                    s.len(),
                    if capped { ", capped" } else { "" }
                ),
                String::new(),
            )];
            rows.push((String::new(), String::new()));
            for sess in s {
                rows.push(("Session".to_string(), sess.session_id.clone()));
                rows.extend(ac_row("Actor", &sess.actor_id));
                rows.extend(ac_row("Created", &sess.created));
                rows.push((String::new(), String::new()));
            }
            rows
        }
    }
}

// ── Built-in tools (browser + code interpreter) ───────────────────────────────

pub(super) fn render_agentcore_browser_split(
    app: &App,
    b: &crate::aws::services::agentcore::AgentCoreBrowser,
    area: Rect,
    frame: &mut Frame,
) {
    render_waf_simple_split(
        app,
        "AgentCore Browser",
        b.name(),
        &b.browser_id,
        &b.status,
        &descriptor_tabs(
            app,
            &crate::aws::services::agentcore::AGENTCORE_BROWSER_SECTIONS,
        ),
        area,
        frame,
    );
}

pub(super) fn render_agentcore_code_interpreter_split(
    app: &App,
    c: &crate::aws::services::agentcore::AgentCoreCodeInterpreter,
    area: Rect,
    frame: &mut Frame,
) {
    render_waf_simple_split(
        app,
        "AgentCore Code Interpreter",
        c.name(),
        &c.interpreter_id,
        &c.status,
        &descriptor_tabs(
            app,
            &crate::aws::services::agentcore::AGENTCORE_CODE_INTERPRETER_SECTIONS,
        ),
        area,
        frame,
    );
}

/// Shared body for the two tool panes — same three sections, same lazy bundle;
/// only the eager Overview rows and the browser-only Config extras differ.
pub(super) fn agentcore_tool_section_lines(
    is_browser: bool,
    eager: Vec<(String, String)>,
    section_idx: usize,
    detail: Option<&crate::lazy::Lazy<Box<crate::aws::services::agentcore::AgentCoreToolDetail>>>,
    sessions: Option<&crate::lazy::Lazy<Vec<crate::aws::services::agentcore::AgentCoreToolSession>>>,
) -> Vec<(String, String)> {
    use crate::lazy::Lazy;

    let bundle = |rows: &mut Vec<(String, String)>| -> Option<
        &crate::aws::services::agentcore::AgentCoreToolDetail,
    > {
        match detail {
            None | Some(Lazy::Loading) => {
                rows.push((String::new(), String::new()));
                rows.push(("  Loading…".to_string(), String::new()));
                None
            }
            Some(Lazy::Error(err)) => {
                rows.push((String::new(), String::new()));
                rows.extend(error_rows(err));
                None
            }
            Some(Lazy::Loaded(d)) => Some(d.as_ref()),
        }
    };

    match section_idx {
        0 => {
            let mut rows = eager;
            if let Some(d) = bundle(&mut rows) {
                rows.push((String::new(), String::new()));
                rows.push((
                    "Execution Role".to_string(),
                    if d.execution_role_arn.is_empty() {
                        "(AWS managed)".to_string()
                    } else {
                        d.execution_role_arn.clone()
                    },
                ));
                if !d.failure_reason.is_empty() {
                    rows.push((String::new(), String::new()));
                    rows.push((format!("  ⚠ {}", d.failure_reason), String::new()));
                }
            }
            rows
        }
        1 => {
            let mut rows = Vec::new();
            if let Some(d) = bundle(&mut rows) {
                rows.push(("Network Mode".to_string(), d.network_mode.clone()));
                if d.subnets.is_empty() && d.security_groups.is_empty() {
                    rows.push((String::new(), String::new()));
                    rows.push((
                        "  (no VPC configuration)".to_string(),
                        String::new(),
                    ));
                } else {
                    rows.push((String::new(), String::new()));
                    ac_list_rows(&mut rows, "Subnets", &d.subnets);
                    ac_list_rows(&mut rows, "Security Groups", &d.security_groups);
                }
                rows.push((String::new(), String::new()));
                rows.push((
                    "Certificates".to_string(),
                    if d.certificate_count == 0 {
                        "✗ none".to_string()
                    } else {
                        d.certificate_count.to_string()
                    },
                ));
                if is_browser {
                    rows.push((String::new(), String::new()));
                    rows.push(("Recording".to_string(), String::new()));
                    rows.push((String::new(), String::new()));
                    match d.recording_enabled {
                        Some(true) => {
                            rows.push(("Enabled".to_string(), "✓ yes".to_string()));
                            if !d.recording_s3_bucket.is_empty() {
                                // `s3://…` is the form the generic classifier
                                // turns into a bucket jump.
                                rows.push((
                                    "Destination".to_string(),
                                    format!(
                                        "s3://{}/{}",
                                        d.recording_s3_bucket, d.recording_s3_prefix
                                    ),
                                ));
                            }
                        }
                        Some(false) => rows.push(("Enabled".to_string(), "✗ no".to_string())),
                        None => rows.push(("  (not configured)".to_string(), String::new())),
                    }
                    rows.push((String::new(), String::new()));
                    rows.push((
                        "Enterprise Policies".to_string(),
                        if d.enterprise_policy_count == 0 {
                            "✗ none".to_string()
                        } else {
                            d.enterprise_policy_count.to_string()
                        },
                    ));
                    rows.push((
                        "Browser Signing".to_string(),
                        if d.browser_signing {
                            "✓ configured"
                        } else {
                            "✗ none"
                        }
                        .to_string(),
                    ));
                }
            }
            rows
        }
        _ => {
            if let Some(rows) = agentcore_list_preamble(sessions, "no sessions") {
                return rows;
            }
            let Some(Lazy::Loaded(ss)) = sessions else {
                return Vec::new();
            };
            let capped = ss.len() >= crate::aws::services::agentcore::MAX_TOOL_SESSIONS;
            let mut rows = vec![(
                format!(
                    "Sessions ({}{})",
                    ss.len(),
                    if capped { ", capped" } else { "" }
                ),
                String::new(),
            )];
            rows.push((String::new(), String::new()));
            for s in ss {
                rows.push(("Session".to_string(), s.session_id.clone()));
                rows.extend(ac_row("Name", &s.name));
                rows.extend(ac_row("Status", &s.status));
                rows.extend(ac_row("Created", &s.created));
                rows.extend(ac_row("Updated", &s.last_updated));
                rows.push((String::new(), String::new()));
            }
            rows
        }
    }
}

pub fn agentcore_browser_section_lines(
    b: &crate::aws::services::agentcore::AgentCoreBrowser,
    section: crate::aws::services::agentcore::AgentCoreBrowserDetailSection,
    detail: Option<&crate::lazy::Lazy<Box<crate::aws::services::agentcore::AgentCoreToolDetail>>>,
    sessions: Option<&crate::lazy::Lazy<Vec<crate::aws::services::agentcore::AgentCoreToolSession>>>,
    tags: Option<&crate::lazy::Lazy<Vec<(String, String)>>>,
) -> Vec<(String, String)> {
    use crate::aws::services::agentcore::AgentCoreBrowserDetailSection as S;
    let idx = match section {
        S::Tags => return agentcore_tags_lines(tags),
        S::Overview => 0,
        S::Config => 1,
        S::Sessions => 2,
    };
    let mut eager = vec![
        ("Name".to_string(), b.name.clone()),
        ("ID".to_string(), b.browser_id.clone()),
        ("ARN".to_string(), b.arn.clone()),
        ("Status".to_string(), b.status.clone()),
        ("Created".to_string(), b.created.clone()),
    ];
    if !b.description.is_empty() {
        eager.push(("Description".to_string(), b.description.clone()));
    }
    agentcore_tool_section_lines(true, eager, idx, detail, sessions)
}

pub fn agentcore_code_interpreter_section_lines(
    c: &crate::aws::services::agentcore::AgentCoreCodeInterpreter,
    section: crate::aws::services::agentcore::AgentCoreCodeInterpreterDetailSection,
    detail: Option<&crate::lazy::Lazy<Box<crate::aws::services::agentcore::AgentCoreToolDetail>>>,
    sessions: Option<&crate::lazy::Lazy<Vec<crate::aws::services::agentcore::AgentCoreToolSession>>>,
    tags: Option<&crate::lazy::Lazy<Vec<(String, String)>>>,
) -> Vec<(String, String)> {
    use crate::aws::services::agentcore::AgentCoreCodeInterpreterDetailSection as S;
    let idx = match section {
        S::Tags => return agentcore_tags_lines(tags),
        S::Overview => 0,
        S::Config => 1,
        S::Sessions => 2,
    };
    let mut eager = vec![
        ("Name".to_string(), c.name.clone()),
        ("ID".to_string(), c.interpreter_id.clone()),
        ("ARN".to_string(), c.arn.clone()),
        ("Status".to_string(), c.status.clone()),
        ("Created".to_string(), c.created.clone()),
    ];
    if !c.description.is_empty() {
        eager.push(("Description".to_string(), c.description.clone()));
    }
    agentcore_tool_section_lines(false, eager, idx, detail, sessions)
}

// ── AgentCore Policy + Registry (the preview families with panes) ─────────────

pub(super) fn render_agentcore_policy_split(
    app: &App,
    p: &crate::aws::services::agentcore::AgentCorePolicy,
    area: Rect,
    frame: &mut Frame,
) {
    render_waf_simple_split(
        app,
        "AgentCore Policy",
        p.name(),
        &p.policy_id,
        &p.enforcement_mode,
        &descriptor_tabs(
            app,
            &crate::aws::services::agentcore::AGENTCORE_POLICY_SECTIONS,
        ),
        area,
        frame,
    );
}

pub fn agentcore_policy_section_lines(
    p: &crate::aws::services::agentcore::AgentCorePolicy,
    section: crate::aws::services::agentcore::AgentCorePolicyDetailSection,
    tags: Option<&crate::lazy::Lazy<Vec<(String, String)>>>,
) -> Vec<(String, String)> {
    use crate::aws::services::agentcore::AgentCorePolicyDetailSection as S;
    match section {
        S::Tags => agentcore_tags_lines(tags),
        S::Overview => {
            let mut rows = vec![
                ("Name".to_string(), p.name.clone()),
                ("ID".to_string(), p.policy_id.clone()),
                ("ARN".to_string(), p.arn.clone()),
                ("Status".to_string(), p.status.clone()),
                (
                    "Enforcement".to_string(),
                    if p.enforcement_mode.eq_ignore_ascii_case("LOG_ONLY") {
                        // Spell out the consequence: LOG_ONLY reads like a
                        // healthy state but blocks nothing.
                        format!("{} (not blocking)", p.enforcement_mode)
                    } else {
                        p.enforcement_mode.clone()
                    },
                ),
                // The engine this policy belongs to — jumpable back to the
                // Policy tab, where engines are listed alongside policies.
                ("Policy Engine".to_string(), p.engine_id.clone()),
                ("Created".to_string(), p.created.clone()),
                ("Updated".to_string(), p.updated.clone()),
            ];
            if !p.description.is_empty() {
                rows.push(("Description".to_string(), p.description.clone()));
            }
            if !p.status_reasons.is_empty() {
                rows.push((String::new(), String::new()));
                rows.push(("Status Reasons".to_string(), String::new()));
                rows.push((String::new(), String::new()));
                for r in &p.status_reasons {
                    rows.push((format!("  ⚠ {}", r), String::new()));
                }
            }
            rows
        }
        S::Definition => {
            if p.definition.is_empty() {
                return vec![
                    (String::new(), String::new()),
                    ("  (no policy definition returned)".to_string(), String::new()),
                ];
            }
            let mut rows = vec![
                ("Type".to_string(), p.definition_kind.clone()),
                (String::new(), String::new()),
            ];
            for line in p.definition.lines() {
                rows.push((format!("  {}", line), String::new()));
            }
            rows.push((String::new(), String::new()));
            rows.push((
                "  · e opens the definition in $EDITOR".to_string(),
                String::new(),
            ));
            rows
        }
    }
}

pub(super) fn render_agentcore_registry_split(
    app: &App,
    r: &crate::aws::services::agentcore::AgentCoreRegistry,
    area: Rect,
    frame: &mut Frame,
) {
    render_waf_simple_split(
        app,
        "AgentCore Registry",
        r.name(),
        &r.registry_id,
        &r.status,
        &descriptor_tabs(
            app,
            &crate::aws::services::agentcore::AGENTCORE_REGISTRY_SECTIONS,
        ),
        area,
        frame,
    );
}

pub fn agentcore_registry_section_lines(
    reg: &crate::aws::services::agentcore::AgentCoreRegistry,
    section: crate::aws::services::agentcore::AgentCoreRegistryDetailSection,
    records: Option<
        &crate::lazy::Lazy<Vec<crate::aws::services::agentcore::AgentCoreRegistryRecord>>,
    >,
    tags: Option<&crate::lazy::Lazy<Vec<(String, String)>>>,
) -> Vec<(String, String)> {
    use crate::aws::services::agentcore::AgentCoreRegistryDetailSection as S;
    use crate::lazy::Lazy;
    match section {
        S::Tags => agentcore_tags_lines(tags),
        S::Overview => {
            let mut rows = vec![
                ("Name".to_string(), reg.name.clone()),
                ("ID".to_string(), reg.registry_id.clone()),
                ("ARN".to_string(), reg.arn.clone()),
                ("Status".to_string(), reg.status.clone()),
                ("Authorizer".to_string(), reg.authorizer_type.clone()),
                ("Created".to_string(), reg.created.clone()),
                ("Updated".to_string(), reg.updated.clone()),
            ];
            if !reg.description.is_empty() {
                rows.push(("Description".to_string(), reg.description.clone()));
            }
            if !reg.status_reason.is_empty() {
                rows.push((String::new(), String::new()));
                rows.push((format!("  ⚠ {}", reg.status_reason), String::new()));
            }
            rows
        }
        S::Records => {
            if let Some(rows) = agentcore_list_preamble(records, "no catalogued records") {
                return rows;
            }
            let Some(Lazy::Loaded(rs)) = records else {
                return Vec::new();
            };
            let mut rows = vec![(format!("Records ({})", rs.len()), String::new())];
            rows.push((String::new(), String::new()));
            for r in rs {
                rows.push(("Record".to_string(), r.name.clone()));
                rows.extend(ac_row("ID", &r.record_id));
                rows.extend(ac_row("Type", &r.descriptor_type));
                rows.extend(ac_row("Version", &r.version));
                rows.extend(ac_row("Status", &r.status));
                rows.extend(ac_row("Description", &r.description));
                rows.extend(ac_row("Updated", &r.updated));
                rows.push((String::new(), String::new()));
            }
            rows
        }
    }
}

// ── Harness ───────────────────────────────────────────────────────────────────

pub(super) fn render_agentcore_harness_split(
    app: &App,
    h: &crate::aws::services::agentcore::AgentCoreHarness,
    area: Rect,
    frame: &mut Frame,
) {
    render_waf_simple_split(
        app,
        "AgentCore Harness",
        h.name(),
        &h.harness_id,
        &h.status,
        &descriptor_tabs(
            app,
            &crate::aws::services::agentcore::AGENTCORE_HARNESS_SECTIONS,
        ),
        area,
        frame,
    );
}

pub fn agentcore_harness_section_lines(
    h: &crate::aws::services::agentcore::AgentCoreHarness,
    section: crate::aws::services::agentcore::AgentCoreHarnessDetailSection,
    detail: Option<&crate::lazy::Lazy<Box<crate::aws::services::agentcore::AgentCoreHarnessDetail>>>,
    endpoints: Option<
        &crate::lazy::Lazy<Vec<crate::aws::services::agentcore::AgentCoreHarnessEndpoint>>,
    >,
    versions: Option<
        &crate::lazy::Lazy<Vec<crate::aws::services::agentcore::AgentCoreHarnessVersion>>,
    >,
    tags: Option<&crate::lazy::Lazy<Vec<(String, String)>>>,
) -> Vec<(String, String)> {
    use crate::aws::services::agentcore::AgentCoreHarnessDetailSection as S;
    use crate::lazy::Lazy;

    // Seven of the nine sections come out of the one `GetHarness` bundle.
    let bundle = |rows: &mut Vec<(String, String)>| -> Option<
        &crate::aws::services::agentcore::AgentCoreHarnessDetail,
    > {
        match detail {
            None | Some(Lazy::Loading) => {
                rows.push((String::new(), String::new()));
                rows.push(("  Loading…".to_string(), String::new()));
                None
            }
            Some(Lazy::Error(err)) => {
                rows.push((String::new(), String::new()));
                rows.extend(error_rows(err));
                None
            }
            Some(Lazy::Loaded(d)) => Some(d.as_ref()),
        }
    };

    match section {
        S::Tags => agentcore_tags_lines(tags),
        S::Overview => {
            let mut rows = vec![
                ("Name".to_string(), h.name.clone()),
                ("ID".to_string(), h.harness_id.clone()),
                ("ARN".to_string(), h.arn.clone()),
                ("Status".to_string(), h.status.clone()),
                ("Created".to_string(), h.created.clone()),
                ("Updated".to_string(), h.updated.clone()),
            ];
            if !h.version.is_empty() {
                rows.push(("Version".to_string(), h.version.clone()));
            }
            if let Some(d) = bundle(&mut rows) {
                rows.push((String::new(), String::new()));
                rows.push(("Execution Role".to_string(), d.role_arn.clone()));

                rows.push((String::new(), String::new()));
                rows.push(("Loop Limits".to_string(), String::new()));
                rows.push((String::new(), String::new()));
                rows.push((
                    "Max Iterations".to_string(),
                    d.max_iterations
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| "default".to_string()),
                ));
                rows.push((
                    "Max Tokens".to_string(),
                    d.max_tokens
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| "default".to_string()),
                ));
                rows.push((
                    "Timeout".to_string(),
                    d.timeout_seconds
                        .map(|v| format!("{}s", v))
                        .unwrap_or_else(|| "default".to_string()),
                ));
                if !d.truncation_strategy.is_empty() {
                    rows.push(("Truncation".to_string(), d.truncation_strategy.clone()));
                    for (k, v) in &d.truncation_detail {
                        rows.push((format!("  {}", k), v.clone()));
                    }
                }

                rows.push((String::new(), String::new()));
                rows.push(("Auth".to_string(), String::new()));
                rows.push((String::new(), String::new()));
                rows.push((
                    "Authorizer".to_string(),
                    if d.authorizer_kind.is_empty() {
                        "IAM (SigV4)".to_string()
                    } else {
                        d.authorizer_kind.clone()
                    },
                ));
                if !d.jwt_discovery_url.is_empty() {
                    rows.push(("Discovery URL".to_string(), d.jwt_discovery_url.clone()));
                }
                ac_list_rows(&mut rows, "Allowed Audience", &d.jwt_allowed_audience);
                ac_list_rows(&mut rows, "Allowed Clients", &d.jwt_allowed_clients);
                ac_list_rows(&mut rows, "Allowed Scopes", &d.jwt_allowed_scopes);

                if !d.failure_reason.is_empty() {
                    rows.push((String::new(), String::new()));
                    rows.push(("Failure".to_string(), String::new()));
                    rows.push((format!("  ⚠ {}", d.failure_reason), String::new()));
                }
            }
            rows
        }
        S::Model => {
            let mut rows = Vec::new();
            if let Some(d) = bundle(&mut rows) {
                if d.model_provider.is_empty() {
                    rows.push(("  (no model configured)".to_string(), String::new()));
                    return rows;
                }
                rows.push(("Provider".to_string(), d.model_provider.clone()));
                rows.push(("Model".to_string(), d.model_id.clone()));
                if !d.model_api_format.is_empty() {
                    rows.push(("API Format".to_string(), d.model_api_format.clone()));
                }
                if !d.model_api_base.is_empty() {
                    rows.push(("API Base".to_string(), d.model_api_base.clone()));
                }
                if !d.model_api_key_arn.is_empty() {
                    // A Secrets Manager reference — the key itself never
                    // leaves Secrets Manager, and nothing here fetches it.
                    rows.push(("API Key Secret".to_string(), d.model_api_key_arn.clone()));
                }

                rows.push((String::new(), String::new()));
                rows.push(("Sampling".to_string(), String::new()));
                rows.push((String::new(), String::new()));
                rows.push((
                    "Max Tokens".to_string(),
                    d.model_max_tokens
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| "default".to_string()),
                ));
                rows.push((
                    "Temperature".to_string(),
                    d.model_temperature
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| "default".to_string()),
                ));
                rows.push((
                    "Top P".to_string(),
                    d.model_top_p
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| "default".to_string()),
                ));
                if let Some(k) = d.model_top_k {
                    rows.push(("Top K".to_string(), k.to_string()));
                }
                if !d.model_additional_params.is_empty() {
                    rows.push((String::new(), String::new()));
                    rows.push(("Additional Parameters".to_string(), String::new()));
                    rows.push((String::new(), String::new()));
                    for line in d.model_additional_params.lines() {
                        rows.push((format!("  {}", line), String::new()));
                    }
                }
            }
            rows
        }
        S::Prompt => {
            let mut rows = Vec::new();
            if let Some(d) = bundle(&mut rows) {
                if d.system_prompt.is_empty() {
                    rows.push(("  (no system prompt)".to_string(), String::new()));
                    return rows;
                }
                rows.push((
                    format!("System Prompt ({} blocks)", d.system_prompt.len()),
                    String::new(),
                ));
                rows.push((String::new(), String::new()));
                for (i, block) in d.system_prompt.iter().enumerate() {
                    if i > 0 {
                        rows.push((String::new(), String::new()));
                    }
                    // Prompts are free text and routinely multi-line — one
                    // content row per line so `/` and the visual selection
                    // work over them normally.
                    for line in block.lines() {
                        rows.push((format!("  {}", line), String::new()));
                    }
                }
            }
            rows
        }
        S::Tools => {
            let mut rows = Vec::new();
            if let Some(d) = bundle(&mut rows) {
                if d.tools.is_empty() {
                    rows.push(("  (no tools)".to_string(), String::new()));
                } else {
                    rows.push((format!("Tools ({})", d.tools.len()), String::new()));
                    rows.push((String::new(), String::new()));
                    for t in &d.tools {
                        let label = if t.name.is_empty() {
                            t.kind.clone()
                        } else {
                            t.name.clone()
                        };
                        rows.push(("Tool".to_string(), label));
                        rows.extend(ac_row("Type", &t.kind));
                        if !t.detail.is_empty() {
                            rows.extend(ac_row(&t.detail_label, &t.detail));
                        }
                        for (k, v) in &t.extra {
                            rows.extend(ac_row(k, v));
                        }
                        rows.push((String::new(), String::new()));
                    }
                }
                if !d.allowed_tools.is_empty() {
                    rows.push((
                        format!("Allowed Tools ({})", d.allowed_tools.len()),
                        String::new(),
                    ));
                    rows.push((String::new(), String::new()));
                    for t in &d.allowed_tools {
                        rows.push((format!("  {}", t), String::new()));
                    }
                }
            }
            rows
        }
        S::Skills => {
            let mut rows = Vec::new();
            if let Some(d) = bundle(&mut rows) {
                if d.skills.is_empty() {
                    rows.push(("  (no skills)".to_string(), String::new()));
                    return rows;
                }
                rows.push((format!("Skills ({})", d.skills.len()), String::new()));
                rows.push((String::new(), String::new()));
                for s in &d.skills {
                    rows.push(("Source".to_string(), s.kind.clone()));
                    rows.extend(ac_row("Location", &s.value));
                    for (k, v) in &s.extra {
                        rows.extend(ac_row(k, v));
                    }
                    rows.push((String::new(), String::new()));
                }
            }
            rows
        }
        S::Memory => {
            let mut rows = Vec::new();
            if let Some(d) = bundle(&mut rows) {
                if d.memory_kind.is_empty() {
                    rows.push(("  (no memory configuration)".to_string(), String::new()));
                    return rows;
                }
                rows.push(("Memory".to_string(), d.memory_kind.clone()));
                if d.memory_kind == "Disabled" {
                    rows.push((String::new(), String::new()));
                    rows.push((
                        "  the loop keeps no state between invocations".to_string(),
                        String::new(),
                    ));
                    return rows;
                }
                // The store ARN jumps to the Memory tab through the generic
                // AgentCore ARN classifier.
                if !d.memory_arn.is_empty() {
                    rows.push(("Store".to_string(), d.memory_arn.clone()));
                }
                if !d.memory_actor_id.is_empty() {
                    rows.push(("Actor".to_string(), d.memory_actor_id.clone()));
                }
                if let Some(c) = d.memory_messages_count {
                    rows.push(("Messages Loaded".to_string(), c.to_string()));
                }
                if let Some(e) = d.memory_event_expiry {
                    rows.push(("Event Expiry".to_string(), format!("{} days", e)));
                }
                if !d.memory_encryption_key_arn.is_empty() {
                    rows.push((
                        "Encryption Key".to_string(),
                        d.memory_encryption_key_arn.clone(),
                    ));
                }
                if !d.memory_strategies.is_empty() {
                    rows.push((String::new(), String::new()));
                    rows.push((
                        format!("Strategies ({})", d.memory_strategies.len()),
                        String::new(),
                    ));
                    rows.push((String::new(), String::new()));
                    for s in &d.memory_strategies {
                        rows.push((format!("  {}", s), String::new()));
                    }
                }
                if !d.memory_retrieval.is_empty() {
                    rows.push((String::new(), String::new()));
                    rows.push((
                        format!("Retrieval ({} namespaces)", d.memory_retrieval.len()),
                        String::new(),
                    ));
                    rows.push((String::new(), String::new()));
                    for (ns, cfg) in &d.memory_retrieval {
                        rows.push((ns.clone(), cfg.clone()));
                    }
                }
            }
            rows
        }
        S::Environment => {
            let mut rows = Vec::new();
            if let Some(d) = bundle(&mut rows) {
                if d.env_runtime_arn.is_empty() && d.env_artifact_kind.is_empty() {
                    rows.push((
                        "  (managed environment — nothing configured)".to_string(),
                        String::new(),
                    ));
                } else {
                    if !d.env_runtime_arn.is_empty() {
                        rows.push(("Runtime".to_string(), d.env_runtime_name.clone()));
                        rows.push(("Runtime ID".to_string(), d.env_runtime_id.clone()));
                        rows.push(("Runtime ARN".to_string(), d.env_runtime_arn.clone()));
                    }
                    if !d.env_artifact_kind.is_empty() {
                        rows.push((String::new(), String::new()));
                        rows.push(("Artifact".to_string(), d.env_artifact_kind.clone()));
                        if !d.env_artifact_value.is_empty() {
                            rows.push(("Container Image".to_string(), d.env_artifact_value.clone()));
                        }
                    }
                    rows.push((String::new(), String::new()));
                    rows.push(("Network".to_string(), String::new()));
                    rows.push((String::new(), String::new()));
                    rows.push((
                        "Network Mode".to_string(),
                        if d.env_network_mode.is_empty() {
                            "default".to_string()
                        } else {
                            d.env_network_mode.clone()
                        },
                    ));
                    ac_list_rows(&mut rows, "Subnets", &d.env_subnets);
                    ac_list_rows(&mut rows, "Security Groups", &d.env_security_groups);
                    rows.push((String::new(), String::new()));
                    rows.push(("Lifecycle".to_string(), String::new()));
                    rows.push((String::new(), String::new()));
                    rows.push((
                        "Idle Session Timeout".to_string(),
                        d.env_idle_timeout
                            .map(|v| format!("{}s", v))
                            .unwrap_or_else(|| "default".to_string()),
                    ));
                    rows.push((
                        "Max Lifetime".to_string(),
                        d.env_max_lifetime
                            .map(|v| format!("{}s", v))
                            .unwrap_or_else(|| "default".to_string()),
                    ));
                    if !d.env_filesystem_kinds.is_empty() {
                        rows.push((String::new(), String::new()));
                        rows.push((
                            format!("Filesystems ({})", d.env_filesystem_kinds.len()),
                            String::new(),
                        ));
                        rows.push((String::new(), String::new()));
                        for f in &d.env_filesystem_kinds {
                            rows.push((format!("  {}", f), String::new()));
                        }
                    }
                }
                rows.push((String::new(), String::new()));
                if d.environment_variables.is_empty() {
                    rows.push(("  (no environment variables)".to_string(), String::new()));
                } else {
                    rows.push((
                        format!("Environment ({})", d.environment_variables.len()),
                        String::new(),
                    ));
                    rows.push((String::new(), String::new()));
                    for (k, v) in &d.environment_variables {
                        rows.push((k.clone(), v.clone()));
                    }
                }
            }
            rows
        }
        S::Endpoints => {
            if let Some(rows) = agentcore_list_preamble(endpoints, "no endpoints") {
                return rows;
            }
            let Some(Lazy::Loaded(eps)) = endpoints else {
                return Vec::new();
            };
            let mut rows = vec![(format!("Endpoints ({})", eps.len()), String::new())];
            rows.push((String::new(), String::new()));
            for e in eps {
                rows.push(("Endpoint".to_string(), e.name.clone()));
                rows.extend(ac_row("Status", &e.status));
                rows.extend(ac_row("Live Version", &e.live_version));
                rows.extend(ac_row("Target Version", &e.target_version));
                rows.extend(ac_row("Description", &e.description));
                rows.extend(ac_row("Updated", &e.updated));
                if !e.failure_reason.is_empty() {
                    rows.push((format!("  ⚠ {}", e.failure_reason), String::new()));
                }
                rows.push((String::new(), String::new()));
            }
            rows
        }
        S::Versions => {
            if let Some(rows) = agentcore_list_preamble(versions, "no other versions") {
                return rows;
            }
            let Some(Lazy::Loaded(vs)) = versions else {
                return Vec::new();
            };
            let mut rows = vec![(format!("Versions ({})", vs.len()), String::new())];
            rows.push((String::new(), String::new()));
            for v in vs {
                let live = if v.version == h.version {
                    "  ← current"
                } else {
                    ""
                };
                rows.push(("Version".to_string(), format!("{}{}", v.version, live)));
                rows.extend(ac_row("Status", &v.status));
                rows.extend(ac_row("Updated", &v.updated));
                if !v.failure_reason.is_empty() {
                    rows.push((format!("  ⚠ {}", v.failure_reason), String::new()));
                }
                rows.push((String::new(), String::new()));
            }
            rows
        }
    }
}

// ── Configuration bundle ──────────────────────────────────────────────────────

pub(super) fn render_agentcore_bundle_split(
    app: &App,
    b: &crate::aws::services::agentcore::AgentCoreConfigBundle,
    area: Rect,
    frame: &mut Frame,
) {
    render_waf_simple_split(
        app,
        "AgentCore Config Bundle",
        b.name(),
        &b.bundle_id,
        "",
        &descriptor_tabs(app, &crate::aws::services::agentcore::AGENTCORE_BUNDLE_SECTIONS),
        area,
        frame,
    );
}

pub fn agentcore_bundle_section_lines(
    b: &crate::aws::services::agentcore::AgentCoreConfigBundle,
    section: crate::aws::services::agentcore::AgentCoreConfigBundleDetailSection,
    detail: Option<
        &crate::lazy::Lazy<Box<crate::aws::services::agentcore::AgentCoreConfigBundleDetail>>,
    >,
    versions: Option<
        &crate::lazy::Lazy<Vec<crate::aws::services::agentcore::AgentCoreConfigBundleVersion>>,
    >,
    tags: Option<&crate::lazy::Lazy<Vec<(String, String)>>>,
) -> Vec<(String, String)> {
    use crate::aws::services::agentcore::AgentCoreConfigBundleDetailSection as S;
    use crate::lazy::Lazy;

    let bundle = |rows: &mut Vec<(String, String)>| -> Option<
        &crate::aws::services::agentcore::AgentCoreConfigBundleDetail,
    > {
        match detail {
            None | Some(Lazy::Loading) => {
                rows.push((String::new(), String::new()));
                rows.push(("  Loading…".to_string(), String::new()));
                None
            }
            Some(Lazy::Error(err)) => {
                rows.push((String::new(), String::new()));
                rows.extend(error_rows(err));
                None
            }
            Some(Lazy::Loaded(d)) => Some(d.as_ref()),
        }
    };

    match section {
        S::Tags => agentcore_tags_lines(tags),
        S::Overview => {
            let mut rows = vec![
                ("Name".to_string(), b.name.clone()),
                ("ID".to_string(), b.bundle_id.clone()),
                ("ARN".to_string(), b.arn.clone()),
                ("Created".to_string(), b.created.clone()),
            ];
            if !b.description.is_empty() {
                rows.push(("Description".to_string(), b.description.clone()));
            }
            if let Some(d) = bundle(&mut rows) {
                rows.push((String::new(), String::new()));
                rows.push(("Live Version".to_string(), d.version_id.clone()));
                rows.push(("Updated".to_string(), d.updated.clone()));
                if !d.kms_key_arn.is_empty() {
                    rows.push(("Encryption Key".to_string(), d.kms_key_arn.clone()));
                }
                if !d.branch_name.is_empty()
                    || !d.commit_message.is_empty()
                    || !d.created_by.is_empty()
                    || !d.parent_versions.is_empty()
                {
                    rows.push((String::new(), String::new()));
                    rows.push(("Lineage".to_string(), String::new()));
                    rows.push((String::new(), String::new()));
                    if !d.branch_name.is_empty() {
                        rows.push(("Branch".to_string(), d.branch_name.clone()));
                    }
                    if !d.created_by.is_empty() {
                        rows.push(("Created By".to_string(), d.created_by.clone()));
                    }
                    if !d.commit_message.is_empty() {
                        rows.push(("Commit".to_string(), d.commit_message.clone()));
                    }
                    ac_list_rows(&mut rows, "Parents", &d.parent_versions);
                }
            }
            rows
        }
        S::Components => {
            let mut rows = Vec::new();
            if let Some(d) = bundle(&mut rows) {
                if d.components.is_empty() {
                    rows.push(("  (no components)".to_string(), String::new()));
                    return rows;
                }
                rows.push((
                    format!("Components ({})", d.components.len()),
                    String::new(),
                ));
                rows.push((String::new(), String::new()));
                // Component configuration is an untyped Document per component,
                // so it renders as pretty JSON rather than a guessed schema.
                for (name, json) in &d.components {
                    rows.push((name.clone(), String::new()));
                    rows.push((String::new(), String::new()));
                    for line in json.lines() {
                        rows.push((format!("  {}", line), String::new()));
                    }
                    rows.push((String::new(), String::new()));
                }
            }
            rows
        }
        S::Versions => {
            if let Some(rows) = agentcore_list_preamble(versions, "no versions") {
                return rows;
            }
            let Some(Lazy::Loaded(vs)) = versions else {
                return Vec::new();
            };
            let live = match detail {
                Some(Lazy::Loaded(d)) => d.version_id.clone(),
                _ => String::new(),
            };
            let mut rows = vec![(format!("Versions ({})", vs.len()), String::new())];
            rows.push((String::new(), String::new()));
            for v in vs {
                let marker = if !live.is_empty() && v.version_id == live {
                    "  ← live"
                } else {
                    ""
                };
                rows.push((
                    "Version".to_string(),
                    format!("{}{}", v.version_id, marker),
                ));
                rows.extend(ac_row("Created", &v.created));
                rows.extend(ac_row("Branch", &v.branch_name));
                rows.extend(ac_row("Created By", &v.created_by));
                rows.extend(ac_row("Commit", &v.commit_message));
                if !v.parent_versions.is_empty() {
                    rows.extend(ac_row("Parents", &v.parent_versions.join(", ")));
                }
                rows.push((String::new(), String::new()));
            }
            rows
        }
    }
}

// ── Payments ──────────────────────────────────────────────────────────────────
//
// Configuration only. Nothing in these two panes reads a payment instrument,
// a balance or a payment token — those data-plane calls are off-limits here
// the same way Secrets Manager values are, and there is no reveal key for them.

pub(super) fn render_agentcore_payment_manager_split(
    app: &App,
    m: &crate::aws::services::agentcore::AgentCorePaymentManager,
    area: Rect,
    frame: &mut Frame,
) {
    render_waf_simple_split(
        app,
        "AgentCore Payment Manager",
        m.name(),
        &m.manager_id,
        &m.status,
        &descriptor_tabs(
            app,
            &crate::aws::services::agentcore::AGENTCORE_PAYMENT_MANAGER_SECTIONS,
        ),
        area,
        frame,
    );
}

pub fn agentcore_payment_manager_section_lines(
    m: &crate::aws::services::agentcore::AgentCorePaymentManager,
    section: crate::aws::services::agentcore::AgentCorePaymentManagerDetailSection,
    detail: Option<&crate::lazy::Lazy<Box<crate::aws::services::agentcore::AgentCorePaymentDetail>>>,
    connectors: Option<
        &crate::lazy::Lazy<Vec<crate::aws::services::agentcore::AgentCorePaymentConnector>>,
    >,
    tags: Option<&crate::lazy::Lazy<Vec<(String, String)>>>,
) -> Vec<(String, String)> {
    use crate::aws::services::agentcore::AgentCorePaymentManagerDetailSection as S;
    use crate::lazy::Lazy;

    let bundle = |rows: &mut Vec<(String, String)>| -> Option<
        &crate::aws::services::agentcore::AgentCorePaymentDetail,
    > {
        match detail {
            None | Some(Lazy::Loading) => {
                rows.push((String::new(), String::new()));
                rows.push(("  Loading…".to_string(), String::new()));
                None
            }
            Some(Lazy::Error(err)) => {
                rows.push((String::new(), String::new()));
                rows.extend(error_rows(err));
                None
            }
            Some(Lazy::Loaded(d)) => Some(d.as_ref()),
        }
    };

    match section {
        S::Tags => agentcore_tags_lines(tags),
        S::Overview => {
            let mut rows = vec![
                ("Name".to_string(), m.name.clone()),
                ("ID".to_string(), m.manager_id.clone()),
                ("ARN".to_string(), m.arn.clone()),
                ("Status".to_string(), m.status.clone()),
                ("Role".to_string(), m.role_arn.clone()),
                ("Created".to_string(), m.created.clone()),
                ("Updated".to_string(), m.updated.clone()),
            ];
            if !m.description.is_empty() {
                rows.push(("Description".to_string(), m.description.clone()));
            }
            if let Some(d) = bundle(&mut rows) {
                if !d.workload_identity_arn.is_empty() {
                    rows.push((String::new(), String::new()));
                    rows.push((
                        "Workload Identity".to_string(),
                        d.workload_identity_arn.clone(),
                    ));
                }
            }
            rows
        }
        S::Auth => {
            let mut rows = vec![("Authorizer Type".to_string(), m.authorizer_type.clone())];
            if let Some(d) = bundle(&mut rows) {
                rows.push((String::new(), String::new()));
                rows.push((
                    "Authorizer".to_string(),
                    if d.authorizer_kind.is_empty() {
                        "IAM (SigV4)".to_string()
                    } else {
                        d.authorizer_kind.clone()
                    },
                ));
                if !d.jwt_discovery_url.is_empty() {
                    rows.push(("Discovery URL".to_string(), d.jwt_discovery_url.clone()));
                }
                ac_list_rows(&mut rows, "Allowed Audience", &d.jwt_allowed_audience);
                ac_list_rows(&mut rows, "Allowed Clients", &d.jwt_allowed_clients);
                ac_list_rows(&mut rows, "Allowed Scopes", &d.jwt_allowed_scopes);
                if !d.workload_identity_arn.is_empty() {
                    rows.push((String::new(), String::new()));
                    rows.push((
                        "Workload Identity".to_string(),
                        d.workload_identity_arn.clone(),
                    ));
                }
            }
            rows
        }
        S::Connectors => {
            if let Some(rows) = agentcore_list_preamble(connectors, "no connectors") {
                return rows;
            }
            let Some(Lazy::Loaded(cs)) = connectors else {
                return Vec::new();
            };
            let mut rows = vec![(format!("Connectors ({})", cs.len()), String::new())];
            let capped = cs.len() >= crate::aws::services::agentcore::MAX_PAYMENT_CONNECTORS;
            rows.push((String::new(), String::new()));
            for c in cs {
                rows.push(("Connector".to_string(), c.name.clone()));
                rows.extend(ac_row("ID", &c.connector_id));
                rows.extend(ac_row("Vendor", &c.kind));
                rows.extend(ac_row("Status", &c.status));
                rows.extend(ac_row("Description", &c.description));
                rows.extend(ac_row("Updated", &c.updated));
                for arn in &c.credential_provider_arns {
                    rows.push(("  Credential Provider".to_string(), arn.clone()));
                }
                rows.push((String::new(), String::new()));
            }
            if capped {
                rows.push((
                    format!(
                        "  · credential wiring deepened for the first {}",
                        crate::aws::services::agentcore::MAX_PAYMENT_CONNECTORS
                    ),
                    String::new(),
                ));
            }
            rows
        }
    }
}

pub(super) fn render_agentcore_payment_cred_split(
    app: &App,
    p: &crate::aws::services::agentcore::AgentCorePaymentCredProvider,
    area: Rect,
    frame: &mut Frame,
) {
    // The header's "id" slot carries the **vendor**, not `id()`: this type's
    // id is its ARN, which is far too long for a header line and is already
    // the first thing Overview shows.
    render_waf_simple_split(
        app,
        "AgentCore Payment Credentials",
        p.name(),
        &p.vendor,
        "",
        &descriptor_tabs(
            app,
            &crate::aws::services::agentcore::AGENTCORE_PAYMENT_CRED_SECTIONS,
        ),
        area,
        frame,
    );
}

pub fn agentcore_payment_cred_section_lines(
    p: &crate::aws::services::agentcore::AgentCorePaymentCredProvider,
    section: crate::aws::services::agentcore::AgentCorePaymentCredProviderDetailSection,
    detail: Option<&crate::lazy::Lazy<Box<crate::aws::services::agentcore::AgentCorePaymentDetail>>>,
    tags: Option<&crate::lazy::Lazy<Vec<(String, String)>>>,
) -> Vec<(String, String)> {
    use crate::aws::services::agentcore::AgentCorePaymentCredProviderDetailSection as S;
    use crate::lazy::Lazy;

    match section {
        S::Tags => agentcore_tags_lines(tags),
        S::Overview => vec![
            ("Name".to_string(), p.name.clone()),
            ("Vendor".to_string(), p.vendor.clone()),
            ("ARN".to_string(), p.arn.clone()),
            ("Created".to_string(), p.created.clone()),
            ("Updated".to_string(), p.updated.clone()),
        ],
        S::Vendor => {
            let mut rows = Vec::new();
            let d = match detail {
                None | Some(Lazy::Loading) => {
                    rows.push(("  Loading…".to_string(), String::new()));
                    return rows;
                }
                Some(Lazy::Error(err)) => {
                    rows.extend(error_rows(err));
                    return rows;
                }
                Some(Lazy::Loaded(d)) => d.as_ref(),
            };
            if d.vendor_kind.is_empty() {
                rows.push((
                    "  (no vendor configuration reported)".to_string(),
                    String::new(),
                ));
                return rows;
            }
            rows.push(("Vendor".to_string(), d.vendor_kind.clone()));
            if !d.vendor_fields.is_empty() {
                rows.push((String::new(), String::new()));
                for (k, v) in &d.vendor_fields {
                    rows.push((k.clone(), v.clone()));
                }
            }
            if !d.vendor_secret_refs.is_empty() {
                rows.push((String::new(), String::new()));
                rows.push(("Secret References".to_string(), String::new()));
                rows.push((String::new(), String::new()));
                for (label, arn) in &d.vendor_secret_refs {
                    rows.push((label.clone(), arn.clone()));
                }
                rows.push((String::new(), String::new()));
                // Spelled out because the row labels read like they might be
                // the credentials themselves. They are pointers; the values
                // live in Secrets Manager and are not read from here.
                rows.push((
                    "  · these are Secrets Manager ARNs, not the credentials"
                        .to_string(),
                    String::new(),
                ));
            }
            rows
        }
    }
}

// ── Identity ──────────────────────────────────────────────────────────────────
//
// All three panes share `lazy.agentcore_identity_detail` (keyed by ARN) and the
// same three-arm bundle accessor. The secret rows are Secrets Manager ARNs —
// pointers. No API returns an OAuth2 client secret or an API key, and nothing
// here asks for one.

/// The shared `None | Loading` / `Error` / `Loaded` unwrap for the three
/// Identity panes.
pub(super) fn ac_identity_bundle<'a>(
    detail: Option<&'a crate::lazy::Lazy<Box<crate::aws::services::agentcore::AgentCoreIdentityDetail>>>,
    rows: &mut Vec<(String, String)>,
) -> Option<&'a crate::aws::services::agentcore::AgentCoreIdentityDetail> {
    use crate::lazy::Lazy;
    match detail {
        None | Some(Lazy::Loading) => {
            rows.push((String::new(), String::new()));
            rows.push(("  Loading…".to_string(), String::new()));
            None
        }
        Some(Lazy::Error(err)) => {
            rows.push((String::new(), String::new()));
            rows.extend(error_rows(err));
            None
        }
        Some(Lazy::Loaded(d)) => Some(d.as_ref()),
    }
}

/// The Secret section body, identical for both credential-provider kinds:
/// which secret backs the provider, who owns it, and the vault key protecting
/// it.
pub(super) fn ac_secret_section_rows(
    detail: Option<&crate::lazy::Lazy<Box<crate::aws::services::agentcore::AgentCoreIdentityDetail>>>,
    vault: Option<&crate::lazy::Lazy<crate::aws::services::agentcore::AgentCoreTokenVault>>,
    what: &str,
) -> Vec<(String, String)> {
    use crate::lazy::Lazy;
    let mut rows = Vec::new();
    if let Some(d) = ac_identity_bundle(detail, &mut rows) {
        if d.secret_arn.is_empty() {
            rows.push((
                format!("  (no {} reported)", what.to_lowercase()),
                String::new(),
            ));
        } else {
            rows.push((what.to_string(), d.secret_arn.clone()));
            if !d.secret_json_key.is_empty() {
                rows.push(("JSON Key".to_string(), d.secret_json_key.clone()));
            }
            if !d.secret_source.is_empty() {
                // MANAGED = AgentCore created and owns the secret; EXTERNAL =
                // you brought your own. It decides who may rotate it.
                rows.push((
                    "Source".to_string(),
                    match d.secret_source.to_ascii_uppercase().as_str() {
                        "MANAGED" => "MANAGED (AgentCore owns it)".to_string(),
                        "EXTERNAL" => "EXTERNAL (you own it)".to_string(),
                        other => other.to_string(),
                    },
                ));
            }
            rows.push((String::new(), String::new()));
            rows.push((
                "  · a Secrets Manager ARN — the credential is not read here"
                    .to_string(),
                String::new(),
            ));
        }
    }

    rows.push((String::new(), String::new()));
    rows.push(("Token Vault".to_string(), String::new()));
    rows.push((String::new(), String::new()));
    match vault {
        None | Some(Lazy::Loading) => rows.push(("  Loading…".to_string(), String::new())),
        Some(Lazy::Error(err)) => rows.extend(error_rows(err)),
        Some(Lazy::Loaded(v)) => {
            rows.push(("Vault".to_string(), v.vault_id.clone()));
            rows.push((
                "Key Type".to_string(),
                match v.key_type.to_ascii_uppercase().as_str() {
                    "CUSTOMER_MANAGED_KEY" => "✓ customer-managed key".to_string(),
                    "SERVICE_MANAGED_KEY" => "service-managed key".to_string(),
                    "" => "unknown".to_string(),
                    other => other.to_string(),
                },
            ));
            if !v.kms_key_arn.is_empty() {
                rows.push(("KMS Key".to_string(), v.kms_key_arn.clone()));
            }
            if !v.last_modified.is_empty() {
                rows.push(("Last Modified".to_string(), v.last_modified.clone()));
            }
            rows.push((String::new(), String::new()));
            rows.push((
                "  · the vault is regional and protects every provider's secret"
                    .to_string(),
                String::new(),
            ));
        }
    }
    rows
}

pub(super) fn render_agentcore_workload_identity_split(
    app: &App,
    w: &crate::aws::services::agentcore::AgentCoreWorkloadIdentity,
    area: Rect,
    frame: &mut Frame,
) {
    render_waf_simple_split(
        app,
        "AgentCore Workload Identity",
        w.name(),
        "",
        "",
        &descriptor_tabs(
            app,
            &crate::aws::services::agentcore::AGENTCORE_WORKLOAD_IDENTITY_SECTIONS,
        ),
        area,
        frame,
    );
}

pub fn agentcore_workload_identity_section_lines(
    w: &crate::aws::services::agentcore::AgentCoreWorkloadIdentity,
    section: crate::aws::services::agentcore::AgentCoreWorkloadIdentityDetailSection,
    detail: Option<&crate::lazy::Lazy<Box<crate::aws::services::agentcore::AgentCoreIdentityDetail>>>,
    tags: Option<&crate::lazy::Lazy<Vec<(String, String)>>>,
) -> Vec<(String, String)> {
    use crate::aws::services::agentcore::AgentCoreWorkloadIdentityDetailSection as S;

    match section {
        S::Tags => agentcore_tags_lines(tags),
        S::Overview => {
            let mut rows = vec![
                ("Name".to_string(), w.name.clone()),
                ("ARN".to_string(), w.arn.clone()),
            ];
            if let Some(d) = ac_identity_bundle(detail, &mut rows) {
                rows.push((String::new(), String::new()));
                rows.push(("Created".to_string(), d.created.clone()));
                rows.push(("Updated".to_string(), d.updated.clone()));
                rows.push((
                    "Return URLs".to_string(),
                    d.return_urls.len().to_string(),
                ));
            }
            rows.push((String::new(), String::new()));
            // AgentCore provisions a workload identity for every runtime,
            // gateway and payment manager, and the API exposes no flag
            // separating those from ones you created — so this list is longer
            // than a console view that only shows what you made by hand.
            // Say so rather than let the count look wrong.
            rows.push((
                "  · AgentCore auto-creates one of these per runtime,".to_string(),
                String::new(),
            ));
            rows.push((
                "    gateway and payment manager; the API marks no".to_string(),
                String::new(),
            ));
            rows.push((
                "    difference between those and ones you created".to_string(),
                String::new(),
            ));
            rows
        }
        S::ReturnUrls => {
            let mut rows = Vec::new();
            if let Some(d) = ac_identity_bundle(detail, &mut rows) {
                if d.return_urls.is_empty() {
                    rows.push((
                        "  (no allowed OAuth2 return URLs)".to_string(),
                        String::new(),
                    ));
                    rows.push((String::new(), String::new()));
                    rows.push((
                        "  three-legged OAuth callbacks are not permitted for".to_string(),
                        String::new(),
                    ));
                    rows.push((
                        "  this identity until a return URL is allow-listed".to_string(),
                        String::new(),
                    ));
                } else {
                    rows.push((
                        format!("Allowed Return URLs ({})", d.return_urls.len()),
                        String::new(),
                    ));
                    rows.push((String::new(), String::new()));
                    for u in &d.return_urls {
                        rows.push((format!("  {}", u), String::new()));
                    }
                }
            }
            rows
        }
    }
}

pub(super) fn render_agentcore_oauth2_provider_split(
    app: &App,
    o: &crate::aws::services::agentcore::AgentCoreOAuth2Provider,
    area: Rect,
    frame: &mut Frame,
) {
    render_waf_simple_split(
        app,
        "AgentCore OAuth2 Provider",
        o.name(),
        &o.vendor,
        "",
        &descriptor_tabs(
            app,
            &crate::aws::services::agentcore::AGENTCORE_OAUTH2_PROVIDER_SECTIONS,
        ),
        area,
        frame,
    );
}

pub fn agentcore_oauth2_provider_section_lines(
    o: &crate::aws::services::agentcore::AgentCoreOAuth2Provider,
    section: crate::aws::services::agentcore::AgentCoreOAuth2ProviderDetailSection,
    detail: Option<&crate::lazy::Lazy<Box<crate::aws::services::agentcore::AgentCoreIdentityDetail>>>,
    vault: Option<&crate::lazy::Lazy<crate::aws::services::agentcore::AgentCoreTokenVault>>,
    tags: Option<&crate::lazy::Lazy<Vec<(String, String)>>>,
) -> Vec<(String, String)> {
    use crate::aws::services::agentcore::AgentCoreOAuth2ProviderDetailSection as S;

    match section {
        S::Tags => agentcore_tags_lines(tags),
        S::Overview => {
            let mut rows = vec![
                ("Name".to_string(), o.name.clone()),
                ("Vendor".to_string(), o.vendor.clone()),
                ("ARN".to_string(), o.arn.clone()),
                ("Created".to_string(), o.created.clone()),
                ("Updated".to_string(), o.updated.clone()),
            ];
            if let Some(d) = ac_identity_bundle(detail, &mut rows) {
                rows.push((String::new(), String::new()));
                if !d.status.is_empty() {
                    rows.push(("Status".to_string(), d.status.clone()));
                }
                if !d.callback_url.is_empty() {
                    // AgentCore's own callback — what you register with the
                    // provider at the other end.
                    rows.push(("Callback URL".to_string(), d.callback_url.clone()));
                }
                if !d.client_id.is_empty() {
                    rows.push(("Client ID".to_string(), d.client_id.clone()));
                }
                if !d.failure_reason.is_empty() {
                    rows.push((String::new(), String::new()));
                    rows.push(("Failure".to_string(), String::new()));
                    rows.push((format!("  ⚠ {}", d.failure_reason), String::new()));
                }
            }
            rows
        }
        S::Provider => {
            let mut rows = Vec::new();
            if let Some(d) = ac_identity_bundle(detail, &mut rows) {
                rows.push(("Vendor".to_string(), o.vendor.clone()));
                if !d.client_id.is_empty() {
                    rows.push(("Client ID".to_string(), d.client_id.clone()));
                }

                rows.push((String::new(), String::new()));
                rows.push(("Authorization Server".to_string(), String::new()));
                rows.push((String::new(), String::new()));
                // Exactly one of the two `Oauth2Discovery` arms is populated:
                // a discovery URL, or the metadata spelled out inline.
                if !d.discovery_url.is_empty() {
                    rows.push(("Discovery URL".to_string(), d.discovery_url.clone()));
                } else if !d.issuer.is_empty() || !d.token_endpoint.is_empty() {
                    rows.push(("Issuer".to_string(), d.issuer.clone()));
                    rows.push((
                        "Authorization Endpoint".to_string(),
                        d.authorization_endpoint.clone(),
                    ));
                    rows.push(("Token Endpoint".to_string(), d.token_endpoint.clone()));
                    ac_list_rows(&mut rows, "Response Types", &d.response_types);
                    ac_list_rows(
                        &mut rows,
                        "Token Auth Methods",
                        &d.token_endpoint_auth_methods,
                    );
                } else {
                    rows.push(("  (no discovery configuration)".to_string(), String::new()));
                }

                if !d.client_auth_method.is_empty() {
                    rows.push((String::new(), String::new()));
                    rows.push((
                        "Client Authentication".to_string(),
                        d.client_auth_method.clone(),
                    ));
                }
                if !d.on_behalf_of_grant_type.is_empty() {
                    rows.push((String::new(), String::new()));
                    rows.push(("On-Behalf-Of Exchange".to_string(), String::new()));
                    rows.push((String::new(), String::new()));
                    rows.push((
                        "Grant Type".to_string(),
                        d.on_behalf_of_grant_type.clone(),
                    ));
                    if !d.on_behalf_of_token_content.is_empty() {
                        rows.push((
                            "Actor Token".to_string(),
                            d.on_behalf_of_token_content.clone(),
                        ));
                    }
                    ac_list_rows(&mut rows, "Actor Scopes", &d.on_behalf_of_scopes);
                }
                if !d.private_endpoint.is_empty() {
                    rows.push((String::new(), String::new()));
                    rows.push(("Private Endpoint".to_string(), String::new()));
                    rows.push((String::new(), String::new()));
                    rows.push(("Type".to_string(), d.private_endpoint.clone()));
                    for (k, v) in &d.private_endpoint_detail {
                        rows.push((k.clone(), v.clone()));
                    }
                }
                if !d.private_endpoint_overrides.is_empty() {
                    rows.push((String::new(), String::new()));
                    rows.push((
                        format!(
                            "Endpoint Overrides ({})",
                            d.private_endpoint_overrides.len()
                        ),
                        String::new(),
                    ));
                    rows.push((String::new(), String::new()));
                    for (domain, kind) in &d.private_endpoint_overrides {
                        rows.push((domain.clone(), kind.clone()));
                    }
                }
            }
            rows
        }
        S::Secret => ac_secret_section_rows(detail, vault, "Client Secret"),
    }
}

pub(super) fn render_agentcore_api_key_provider_split(
    app: &App,
    k: &crate::aws::services::agentcore::AgentCoreApiKeyProvider,
    area: Rect,
    frame: &mut Frame,
) {
    render_waf_simple_split(
        app,
        "AgentCore API Key Provider",
        k.name(),
        "",
        "",
        &descriptor_tabs(
            app,
            &crate::aws::services::agentcore::AGENTCORE_API_KEY_PROVIDER_SECTIONS,
        ),
        area,
        frame,
    );
}

pub fn agentcore_api_key_provider_section_lines(
    k: &crate::aws::services::agentcore::AgentCoreApiKeyProvider,
    section: crate::aws::services::agentcore::AgentCoreApiKeyProviderDetailSection,
    detail: Option<&crate::lazy::Lazy<Box<crate::aws::services::agentcore::AgentCoreIdentityDetail>>>,
    vault: Option<&crate::lazy::Lazy<crate::aws::services::agentcore::AgentCoreTokenVault>>,
    tags: Option<&crate::lazy::Lazy<Vec<(String, String)>>>,
) -> Vec<(String, String)> {
    use crate::aws::services::agentcore::AgentCoreApiKeyProviderDetailSection as S;

    match section {
        S::Tags => agentcore_tags_lines(tags),
        S::Overview => {
            let mut rows = vec![
                ("Name".to_string(), k.name.clone()),
                ("ARN".to_string(), k.arn.clone()),
                ("Created".to_string(), k.created.clone()),
                ("Updated".to_string(), k.updated.clone()),
            ];
            if let Some(d) = ac_identity_bundle(detail, &mut rows) {
                if !d.secret_arn.is_empty() {
                    rows.push((String::new(), String::new()));
                    rows.push(("API Key Secret".to_string(), d.secret_arn.clone()));
                }
            }
            rows
        }
        S::Secret => ac_secret_section_rows(detail, vault, "API Key Secret"),
    }
}

// ── Policy engine ─────────────────────────────────────────────────────────────

pub(super) fn render_agentcore_policy_engine_split(
    app: &App,
    e: &crate::aws::services::agentcore::AgentCorePolicyEngine,
    area: Rect,
    frame: &mut Frame,
) {
    render_waf_simple_split(
        app,
        "AgentCore Policy Engine",
        e.name(),
        &e.engine_id,
        &e.status,
        &descriptor_tabs(
            app,
            &crate::aws::services::agentcore::AGENTCORE_POLICY_ENGINE_SECTIONS,
        ),
        area,
        frame,
    );
}

pub fn agentcore_policy_engine_section_lines(
    e: &crate::aws::services::agentcore::AgentCorePolicyEngine,
    section: crate::aws::services::agentcore::AgentCorePolicyEngineDetailSection,
    policies: &[&crate::aws::services::agentcore::AgentCorePolicy],
    generations: Option<
        &crate::lazy::Lazy<Vec<crate::aws::services::agentcore::AgentCorePolicyGeneration>>,
    >,
    tags: Option<&crate::lazy::Lazy<Vec<(String, String)>>>,
) -> Vec<(String, String)> {
    use crate::aws::services::agentcore::AgentCorePolicyEngineDetailSection as S;

    match section {
        S::Tags => agentcore_tags_lines(tags),
        S::Overview => {
            let mut rows = vec![
                ("Name".to_string(), e.name.clone()),
                ("ID".to_string(), e.engine_id.clone()),
                ("ARN".to_string(), e.arn.clone()),
                ("Status".to_string(), e.status.clone()),
                ("Created".to_string(), e.created.clone()),
                ("Updated".to_string(), e.updated.clone()),
            ];
            if !e.description.is_empty() {
                rows.push(("Description".to_string(), e.description.clone()));
            }
            if !e.encryption_key_arn.is_empty() {
                rows.push(("Encryption Key".to_string(), e.encryption_key_arn.clone()));
            }
            rows.push(("Policies".to_string(), policies.len().to_string()));
            // How many are actually blocking — the number you open a policy
            // engine to find. LOG_ONLY policies are healthy and enforce
            // nothing, so a count of "policies" alone is misleading.
            let enforcing = policies
                .iter()
                .filter(|p| !p.enforcement_mode.eq_ignore_ascii_case("LOG_ONLY"))
                .count();
            rows.push((
                "Enforcing".to_string(),
                if policies.is_empty() {
                    "0".to_string()
                } else if enforcing == 0 {
                    format!("⚠ 0 of {} — all log-only", policies.len())
                } else {
                    format!("{} of {}", enforcing, policies.len())
                },
            ));
            if !e.status_reasons.is_empty() {
                rows.push((String::new(), String::new()));
                rows.push(("Status Reasons".to_string(), String::new()));
                rows.push((String::new(), String::new()));
                for r in &e.status_reasons {
                    rows.push((format!("  {}", r), String::new()));
                }
            }
            rows
        }
        S::Generations => {
            use crate::aws::services::agentcore::{
                AgentCorePolicyGenerationAsset as Asset, MAX_POLICY_GENERATIONS,
            };
            if let Some(rows) = agentcore_list_preamble(generations, "no policy generations") {
                return rows;
            }
            let Some(crate::lazy::Lazy::Loaded(gens)) = generations else {
                return Vec::new();
            };

            // Lead with the count that matters: assets whose findings say the
            // generated policy doesn't mean what the source fragment asked
            // for. Buried in a per-asset list, that is exactly what gets
            // missed.
            let flagged: usize = gens
                .iter()
                .flat_map(|g| g.assets.iter())
                .filter(|a| a.has_problem_finding())
                .count();
            let mut rows = vec![(format!("Generations ({})", gens.len()), String::new())];
            rows.push((
                "Flagged Assets".to_string(),
                if flagged == 0 {
                    "✓ none".to_string()
                } else {
                    format!("⚠ {}", flagged)
                },
            ));
            rows.push((String::new(), String::new()));

            for g in gens {
                rows.push((
                    "Generation".to_string(),
                    if g.name.is_empty() {
                        g.generation_id.clone()
                    } else {
                        g.name.clone()
                    },
                ));
                rows.extend(ac_row("ID", &g.generation_id));
                rows.extend(ac_row("Status", &g.status));
                // The `Resource` union's ARN — jumpable to whatever the
                // generation was built against.
                rows.extend(ac_row("Target", &g.target_arn));
                rows.extend(ac_row("Created", &g.created));
                rows.extend(ac_row("Updated", &g.updated));
                rows.extend(ac_row("Findings", &g.findings));
                for r in &g.status_reasons {
                    rows.push((format!("  ⚠ {}", r), String::new()));
                }

                if !g.assets_loaded {
                    // Distinguish "not fetched" from "none" — reporting an
                    // un-listed generation as empty would read as a clean one.
                    rows.push((
                        format!(
                            "  · assets not listed (newest {} only)",
                            MAX_POLICY_GENERATIONS
                        ),
                        String::new(),
                    ));
                } else if g.assets.is_empty() {
                    rows.push(("  (no generated assets)".to_string(), String::new()));
                } else {
                    rows.push((String::new(), String::new()));
                    rows.push((format!("  Assets ({})", g.assets.len()), String::new()));
                    for a in &g.assets {
                        rows.push((String::new(), String::new()));
                        rows.push((
                            "  Asset".to_string(),
                            if a.has_problem_finding() {
                                format!("⚠ {}", a.asset_id)
                            } else {
                                a.asset_id.clone()
                            },
                        ));
                        if !a.source_text.is_empty() {
                            rows.push(("  From".to_string(), String::new()));
                            for line in a.source_text.lines() {
                                rows.push((format!("      {}", line), String::new()));
                            }
                        }
                        if a.definition.is_empty() {
                            rows.push((
                                "      (no policy generated from this fragment)".to_string(),
                                String::new(),
                            ));
                        } else {
                            rows.push(("  Policy".to_string(), String::new()));
                            for line in a.definition.lines() {
                                rows.push((format!("      {}", line), String::new()));
                            }
                        }
                        for (kind, desc) in &a.findings {
                            let text = if desc.is_empty() {
                                kind.clone()
                            } else {
                                format!("{} — {}", kind, desc)
                            };
                            // `⚠` on a content row renders WARNING-coloured;
                            // benign findings stay plain.
                            rows.push((
                                if Asset::finding_is_problem(kind) {
                                    format!("      ⚠ {}", text)
                                } else {
                                    format!("      {}", text)
                                },
                                String::new(),
                            ));
                        }
                    }
                }
                rows.push((String::new(), String::new()));
            }
            rows.push((
                "  · e opens every generated policy in this section".to_string(),
                String::new(),
            ));
            rows
        }
        S::Policies => {
            if policies.is_empty() {
                return vec![
                    (String::new(), String::new()),
                    ("  (no policies on this engine)".to_string(), String::new()),
                ];
            }
            let mut rows = vec![(format!("Policies ({})", policies.len()), String::new())];
            rows.push((String::new(), String::new()));
            for p in policies {
                rows.push(("Policy".to_string(), p.name.clone()));
                rows.extend(ac_row("ID", &p.policy_id));
                rows.extend(ac_row("Status", &p.status));
                rows.extend(ac_row("Enforcement", &p.enforcement_mode));
                rows.extend(ac_row("Definition", &p.definition_kind));
                rows.extend(ac_row("Description", &p.description));
                rows.push((String::new(), String::new()));
            }
            rows
        }
    }
}

// ── Resource policy (runtime + gateway) ───────────────────────────────────────

/// The Resource Policy section body, shared by the runtime and gateway panes.
/// Answers one question — who outside this account can invoke this — so the
/// principal list leads and external principals are called out. `e` opens the
/// raw document.
pub(super) fn agentcore_resource_policy_lines(
    state: Option<&crate::lazy::Lazy<crate::aws::services::agentcore::AgentCoreResourcePolicy>>,
    account_id: &str,
    what: &str,
) -> Vec<(String, String)> {
    use crate::aws::services::agentcore::principal_is_external;
    use crate::lazy::Lazy;

    let p = match state {
        None | Some(Lazy::Loading) => {
            return vec![
                (String::new(), String::new()),
                ("  Loading…".to_string(), String::new()),
            ]
        }
        Some(Lazy::Error(err)) => {
            let mut rows = vec![(String::new(), String::new())];
            rows.extend(error_rows(err));
            return rows;
        }
        Some(Lazy::Loaded(p)) => p,
    };

    if p.raw.trim().is_empty() {
        // No policy attached is the normal case and a *good* one — say what it
        // means rather than leaving an empty section.
        return vec![
            ("Resource Policy".to_string(), "none".to_string()),
            (String::new(), String::new()),
            (
                format!("  no resource-based policy on this {}", what),
                String::new(),
            ),
            (
                "  access is governed by IAM identity policies alone".to_string(),
                String::new(),
            ),
        ];
    }

    if p.unparsed {
        let mut rows = vec![
            (
                "  ⚠ policy did not parse as a statement list".to_string(),
                String::new(),
            ),
            (String::new(), String::new()),
        ];
        for line in p.raw.lines() {
            rows.push((format!("  {}", line), String::new()));
        }
        return rows;
    }

    let external: Vec<&str> = p
        .statements
        .iter()
        .filter(|s| s.effect.eq_ignore_ascii_case("Allow"))
        .flat_map(|s| s.principals.iter())
        .filter(|pr| principal_is_external(pr, account_id))
        .map(|s| s.as_str())
        .collect();

    let mut rows = vec![(
        "Statements".to_string(),
        p.statements.len().to_string(),
    )];
    rows.push((
        "External Access".to_string(),
        if account_id.is_empty() {
            // Without a caller identity there is nothing to compare against,
            // so claiming "none" would be a lie.
            "unknown — account id not resolved".to_string()
        } else if external.is_empty() {
            "✓ none — all principals are in this account".to_string()
        } else {
            format!("⚠ {} principal(s) outside this account", external.len())
        },
    ));
    if !external.is_empty() {
        rows.push((String::new(), String::new()));
        for pr in &external {
            rows.push((
                format!("  ⚠ {}", if *pr == "*" { "* (anyone)" } else { pr }),
                String::new(),
            ));
        }
    }

    for s in &p.statements {
        rows.push((String::new(), String::new()));
        rows.push((
            if s.sid.is_empty() {
                "Statement".to_string()
            } else {
                format!("Statement — {}", s.sid)
            },
            String::new(),
        ));
        rows.push((String::new(), String::new()));
        rows.push((
            "Effect".to_string(),
            // Deny is the safe direction here, so mark Allow rather than Deny.
            if s.effect.eq_ignore_ascii_case("Allow") {
                "Allow".to_string()
            } else {
                s.effect.clone()
            },
        ));
        ac_list_rows(&mut rows, "Principals", &s.principals);
        ac_list_rows(&mut rows, "Actions", &s.actions);
        ac_list_rows(&mut rows, "Conditions", &s.conditions);
    }

    rows.push((String::new(), String::new()));
    rows.push((
        "  · e opens the raw policy document".to_string(),
        String::new(),
    ));
    rows
}

// ── Tags (every AgentCore pane) ───────────────────────────────────────────────

/// The Tags section body, shared by all fifteen AgentCore panes. One
/// `ListTagsForResource` per resource, keyed by ARN — see the Tags note in
/// `agentcore.rs` for why this is a section and not `tag:` search.
pub(super) fn agentcore_tags_lines(
    state: Option<&crate::lazy::Lazy<Vec<(String, String)>>>,
) -> Vec<(String, String)> {
    use crate::lazy::Lazy;
    match state {
        None | Some(Lazy::Loading) => vec![("  Loading…".to_string(), String::new())],
        Some(Lazy::Error(err)) => error_rows(err),
        Some(Lazy::Loaded(tags)) if tags.is_empty() => {
            vec![
                ("  (no tags)".to_string(), String::new()),
                (String::new(), String::new()),
                // Say it, because the absence is otherwise ambiguous with a
                // permission gap — and because `tag:` silently not covering
                // this service is exactly the sort of thing people assume is
                // a bug in the query rather than a documented limit.
                (
                    "  · AgentCore tags load per resource, so `tag:` search".to_string(),
                    String::new(),
                ),
                (
                    "    does not filter this service".to_string(),
                    String::new(),
                ),
            ]
        }
        Some(Lazy::Loaded(tags)) => {
            let mut rows = vec![(format!("Tags ({})", tags.len()), String::new())];
            rows.push((String::new(), String::new()));
            for (k, v) in tags {
                rows.push((k.clone(), v.clone()));
            }
            rows
        }
    }
}
