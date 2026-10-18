use super::*;

use crate::aws::services::appsync::{
    appsync_log_group, opensearch_domain_from_endpoint, AppSyncApi, AppSyncApiDetailSection,
    AppSyncApiKey, AppSyncAuthMode, AppSyncDataSource, AppSyncFunction, AppSyncResolvers,
};

// ── AppSync GraphQL API split pane ────────────────────────────────────────────

pub(super) fn render_appsync_api_split(app: &App, api: &AppSyncApi, area: Rect, frame: &mut Frame) {
    let subtitle = format!(
        "{} · {} · {}",
        api.api_type,
        api.visibility,
        auth_label(&api.auth.auth_type)
    );
    render_simple_split(
        app,
        area,
        frame,
        "AppSync API",
        &api.name,
        &subtitle,
        &descriptor_tabs(app, &crate::aws::services::appsync::APPSYNC_API_SECTIONS),
    );
}

/// Labels the AppSync row classifier (`App::appsync_row_jump_target`) keys
/// on — renaming a row here without the classifier silently kills its jump.
/// ARN-valued rows (Lambda, roles, WAF, RDS cluster, event bus) jump through
/// the generic classifier and need no label.
pub const APPSYNC_ROW_USER_POOL: &str = "User Pool";
pub const APPSYNC_ROW_DDB_TABLE: &str = "DynamoDB Table";
pub const APPSYNC_ROW_OPENSEARCH: &str = "OpenSearch Domain";
pub const APPSYNC_ROW_LOG_GROUP: &str = "Log Group";

/// `AMAZON_COGNITO_USER_POOLS` → `Cognito user pools` — the console's words.
pub fn auth_label(auth_type: &str) -> &str {
    match auth_type {
        "API_KEY" => "API key",
        "AWS_IAM" => "IAM",
        "AMAZON_COGNITO_USER_POOLS" => "Cognito user pools",
        "OPENID_CONNECT" => "OpenID Connect",
        "AWS_LAMBDA" => "Lambda authorizer",
        "" => "—",
        other => other,
    }
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn kv(k: &str, v: impl Into<String>) -> (String, String) {
    (k.to_string(), v.into())
}

fn spacer() -> (String, String) {
    (String::new(), String::new())
}

fn auth_mode_rows(rows: &mut Vec<(String, String)>, m: &AppSyncAuthMode, primary: bool) {
    rows.push(kv("  Type", auth_label(&m.auth_type)));
    if let Some(id) = &m.user_pool_id {
        rows.push(kv(&format!("  {APPSYNC_ROW_USER_POOL}"), id.clone()));
        if let Some(r) = &m.user_pool_region {
            rows.push(kv("  Pool Region", r.clone()));
        }
        if primary {
            if let Some(a) = &m.user_pool_default_action {
                rows.push(kv("  Default Action", a.clone()));
            }
        }
        if let Some(re) = &m.app_id_client_regex {
            rows.push(kv("  App Client Regex", re.clone()));
        }
    }
    if let Some(iss) = &m.oidc_issuer {
        rows.push(kv("  Issuer", iss.clone()));
        if let Some(c) = &m.oidc_client_id {
            rows.push(kv("  Client ID", c.clone()));
        }
    }
    if let Some(uri) = &m.lambda_authorizer_uri {
        rows.push(kv("  Authorizer", uri.clone())); // Lambda ARN — jumpable
        if let Some(ttl) = m.lambda_result_ttl {
            rows.push(kv("  Result TTL", format!("{ttl}s")));
        }
    }
}

fn data_source_rows(rows: &mut Vec<(String, String)>, ds: &AppSyncDataSource) {
    rows.push((ds.name.clone(), String::new())); // group header
    rows.push(kv("  Type", ds.ds_type.clone()));
    if let Some(d) = &ds.description {
        rows.push(kv("  Description", d.clone()));
    }
    if let Some(t) = &ds.dynamodb_table {
        rows.push(kv(&format!("  {APPSYNC_ROW_DDB_TABLE}"), t.clone()));
    }
    if let Some(f) = &ds.lambda_function_arn {
        rows.push(kv("  Function", f.clone()));
    }
    if let Some(e) = &ds.opensearch_endpoint {
        match opensearch_domain_from_endpoint(e) {
            Some(d) => rows.push(kv(&format!("  {APPSYNC_ROW_OPENSEARCH}"), d)),
            None => rows.push(kv("  Endpoint", e.clone())),
        }
    }
    if let Some(e) = &ds.http_endpoint {
        rows.push(kv("  Endpoint", e.clone()));
    }
    if let Some(b) = &ds.event_bus_arn {
        rows.push(kv("  Event Bus", b.clone()));
    }
    if let Some(c) = &ds.rds_cluster_arn {
        rows.push(kv("  Cluster", c.clone()));
        if let Some(db) = &ds.rds_database {
            rows.push(kv("  Database", db.clone()));
        }
        if let Some(s) = &ds.rds_secret_arn {
            // A reference to the credential, never resolved.
            rows.push(kv("  Credentials Secret", s.clone()));
        }
    }
    if let Some(r) = &ds.region {
        rows.push(kv("  Region", r.clone()));
    }
    if let Some(role) = &ds.service_role_arn {
        rows.push(kv("  Service Role", role.clone())); // IAM ARN — jumpable
    }
    rows.push(spacer());
}

fn function_rows(rows: &mut Vec<(String, String)>, f: &AppSyncFunction) {
    rows.push((f.name.clone(), String::new())); // group header
    rows.push(kv("  Function ID", f.function_id.clone()));
    if let Some(d) = &f.description {
        rows.push(kv("  Description", d.clone()));
    }
    rows.push(kv("  Data Source", f.data_source.clone().unwrap_or_else(|| "—".to_string())));
    rows.push(kv("  Runtime", f.runtime.clone()));
    if f.max_batch_size > 0 {
        rows.push(kv("  Max Batch Size", f.max_batch_size.to_string()));
    }
    rows.push(spacer());
}

pub fn api_key_rows(keys: &[AppSyncApiKey], now: i64) -> Vec<(String, String)> {
    if keys.is_empty() {
        return vec![kv("", "No API keys")];
    }
    let mut rows = Vec::new();
    for k in keys {
        rows.push((k.masked_id.clone(), String::new())); // group header
        if let Some(d) = &k.description {
            rows.push(kv("  Description", d.clone()));
        }
        let expires = crate::aws::services::cloudwatch::fmt_epoch_secs(k.expires);
        if k.expires <= now {
            rows.push(kv("  Expires", format!("✗ {expires} UTC (expired)")));
            if k.deletes > 0 {
                rows.push(kv(
                    "  Deleted After",
                    format!("{} UTC", crate::aws::services::cloudwatch::fmt_epoch_secs(k.deletes)),
                ));
            }
        } else if k.expires - now < 7 * 86400 {
            rows.push(kv("  Expires", format!("⚠ {expires} UTC (within 7 days)")));
        } else {
            rows.push(kv("  Expires", format!("{expires} UTC")));
        }
        rows.push(spacer());
    }
    rows.push(kv("", "· key values are never shown: the key id is the secret"));
    rows
}

fn resolver_rows(r: &AppSyncResolvers, functions: Option<&[AppSyncFunction]>) -> Vec<(String, String)> {
    let mut rows = Vec::new();
    if r.types_scanned < r.types_total {
        rows.push(kv(
            "",
            format!(
                "⚠ Only the first {} of {} object types were walked (Query / Mutation / Subscription first) — AppSync lists resolvers one type at a time",
                r.types_scanned, r.types_total
            ),
        ));
        rows.push(spacer());
    }
    if !r.failed_types.is_empty() {
        rows.push(kv(
            "",
            match &r.failure {
                Some(e) => format!("⚠ Resolvers unavailable for: {} — {e}", r.failed_types.join(", ")),
                None => format!("⚠ Resolvers unavailable for: {}", r.failed_types.join(", ")),
            },
        ));
        rows.push(spacer());
    }
    if r.resolvers.is_empty() {
        rows.push(kv("", "No resolvers"));
        return rows;
    }
    let fn_name = |id: &str| -> String {
        functions
            .and_then(|fs| fs.iter().find(|f| f.function_id == id))
            .map(|f| f.name.clone())
            .unwrap_or_else(|| id.to_string())
    };
    for res in &r.resolvers {
        rows.push((format!("{}.{}", res.type_name, res.field_name), String::new()));
        rows.push(kv("  Kind", res.kind.clone()));
        if res.kind == "PIPELINE" {
            let names: Vec<String> = res.pipeline_functions.iter().map(|id| fn_name(id.as_str())).collect();
            rows.push(kv(
                "  Pipeline",
                if names.is_empty() { "—".to_string() } else { names.join(" → ") },
            ));
        } else {
            rows.push(kv(
                "  Data Source",
                res.data_source.clone().unwrap_or_else(|| "—".to_string()),
            ));
        }
        rows.push(kv("  Runtime", res.runtime.clone()));
        if let Some(ttl) = res.caching_ttl {
            rows.push(kv("  Caching TTL", format!("{ttl}s")));
        }
        if res.max_batch_size > 0 {
            rows.push(kv("  Max Batch Size", res.max_batch_size.to_string()));
        }
        rows.push(spacer());
    }
    rows.push(kv("", "· press e to open the resolver code"));
    rows
}

pub fn appsync_api_section_lines(app: &App, api: &AppSyncApi, section: AppSyncApiDetailSection) -> Vec<(String, String)> {
    let lazy = &app.lazy;
    let id = api.api_id.as_str();
    match section {
        AppSyncApiDetailSection::Overview => {
            let mut rows = vec![
                kv("API ID", api.api_id.clone()),
                kv("Type", api.api_type.clone()),
                kv("Visibility", api.visibility.clone()),
                kv("Primary Auth", auth_label(&api.auth.auth_type)),
            ];
            if !api.additional_auth.is_empty() {
                let extra: Vec<&str> = api.additional_auth.iter().map(|m| auth_label(&m.auth_type)).collect();
                rows.push(kv("Additional Auth", extra.join(", ")));
            }
            if !api.uris.is_empty() {
                rows.push(spacer());
                rows.push(kv("Endpoints", ""));
                for (k, v) in &api.uris {
                    rows.push(kv(&format!("  {k}"), v.clone()));
                }
            }
            rows.push(spacer());
            rows.push(kv("Observability", ""));
            rows.push(kv(
                "  X-Ray Tracing",
                if api.xray_enabled { "✓ enabled" } else { "✗ disabled" },
            ));
            match api.field_log_level.as_deref() {
                Some(level) if level != "NONE" => {
                    rows.push(kv("  Field Logging", format!("✓ {level}")));
                    rows.push(kv(
                        "  Verbose Content",
                        if api.exclude_verbose_content { "excluded" } else { "included" },
                    ));
                    rows.push(kv(&format!("  {APPSYNC_ROW_LOG_GROUP}"), appsync_log_group(&api.api_id)));
                }
                _ => rows.push(kv("  Field Logging", "✗ off")),
            }
            if let Some(r) = &api.logs_role_arn {
                rows.push(kv("  Logs Role", r.clone()));
            }
            rows.push(spacer());
            rows.push(kv("Protection", ""));
            rows.push(kv(
                "  WAF Web ACL",
                api.waf_web_acl_arn.clone().unwrap_or_else(|| "✗ none".to_string()),
            ));
            if let Some(i) = &api.introspection {
                rows.push(kv("  Introspection", i.clone()));
            }
            rows.push(kv(
                "  Query Depth Limit",
                if api.query_depth_limit > 0 { api.query_depth_limit.to_string() } else { "unlimited".to_string() },
            ));
            rows.push(kv(
                "  Resolver Count Limit",
                if api.resolver_count_limit > 0 { api.resolver_count_limit.to_string() } else { "default (10000)".to_string() },
            ));
            if api.api_type == "MERGED" || api.owner.is_some() {
                rows.push(spacer());
                rows.push(kv("Ownership", ""));
                if let Some(o) = &api.owner {
                    rows.push(kv("  Owner", o.clone()));
                }
                if let Some(c) = &api.owner_contact {
                    rows.push(kv("  Owner Contact", c.clone()));
                }
                if let Some(r) = &api.merged_api_execution_role_arn {
                    rows.push(kv("  Merged API Execution Role", r.clone()));
                }
            }
            rows.push(spacer());
            rows.push(kv("ARN", api.arn.clone()));
            rows
        }
        AppSyncApiDetailSection::Auth => {
            let mut rows = vec![kv("Primary", "")];
            auth_mode_rows(&mut rows, &api.auth, true);
            for (i, m) in api.additional_auth.iter().enumerate() {
                rows.push(spacer());
                rows.push(kv(&format!("Additional #{}", i + 1), ""));
                auth_mode_rows(&mut rows, m, false);
            }
            rows
        }
        AppSyncApiDetailSection::DataSources => match lazy.appsync_data_sources.get(id) {
            None | Some(Lazy::Loading) => vec![kv("", "Loading data sources…")],
            Some(Lazy::Error(e)) => error_rows(e),
            Some(Lazy::Loaded(list)) if list.is_empty() => vec![kv("", "No data sources")],
            Some(Lazy::Loaded(list)) => {
                let mut rows = Vec::new();
                for ds in list {
                    data_source_rows(&mut rows, ds);
                }
                rows
            }
        },
        AppSyncApiDetailSection::Resolvers => match lazy.appsync_resolvers.get(id) {
            None | Some(Lazy::Loading) => vec![kv("", "Loading resolvers…")],
            Some(Lazy::Error(e)) => error_rows(e),
            Some(Lazy::Loaded(r)) => {
                let fns = match lazy.appsync_functions.get(id) {
                    Some(Lazy::Loaded(f)) => Some(f.as_slice()),
                    _ => None,
                };
                resolver_rows(r, fns)
            }
        },
        AppSyncApiDetailSection::Functions => match lazy.appsync_functions.get(id) {
            None | Some(Lazy::Loading) => vec![kv("", "Loading functions…")],
            Some(Lazy::Error(e)) => error_rows(e),
            Some(Lazy::Loaded(list)) if list.is_empty() => {
                vec![kv("", "No functions (pipeline resolvers use these)")]
            }
            Some(Lazy::Loaded(list)) => {
                let mut rows = Vec::new();
                for f in list {
                    function_rows(&mut rows, f);
                }
                rows.push(kv("", "· press e to open the function code"));
                rows
            }
        },
        AppSyncApiDetailSection::ApiKeys => match lazy.appsync_api_keys.get(id) {
            None | Some(Lazy::Loading) => vec![kv("", "Loading API keys…")],
            Some(Lazy::Error(e)) => error_rows(e),
            Some(Lazy::Loaded(keys)) => api_key_rows(keys, now_secs()),
        },
        AppSyncApiDetailSection::Schema => match lazy.appsync_schema.get(id) {
            None | Some(Lazy::Loading) => vec![kv("", "Loading schema…")],
            Some(Lazy::Error(e)) => error_rows(e),
            Some(Lazy::Loaded(s)) if s.trim().is_empty() => vec![kv("", "No schema")],
            Some(Lazy::Loaded(s)) => s.lines().map(|l| (format!("  {l}"), String::new())).collect(),
        },
        AppSyncApiDetailSection::Tags => map_tags_lines(&api.tags),
    }
}

/// What `e` opens on the Resolvers / Functions sections: every resolver's
/// (or function's) code in one document, headed by its name, plus the file
/// extension to open it with. JS resolvers carry `code`; VTL ones carry
/// request/response mapping templates. Any JS makes it a `.js` file (`//`
/// headers); all-VTL is `.vtl`, headed with VTL's `##` comments.
pub fn appsync_code_document(
    entries: impl Iterator<Item = (String, Option<String>, Option<String>, Option<String>)>,
) -> (String, &'static str) {
    let entries: Vec<_> = entries.collect();
    let js = entries.iter().any(|(_, code, _, _)| code.is_some());
    let (comment, ext) = if js { ("//", ".js") } else { ("##", ".vtl") };
    let mut out = String::new();
    for (title, code, req, resp) in entries {
        out.push_str(&format!("{comment} ── {title} ──\n"));
        match (code, req, resp) {
            (Some(c), _, _) => out.push_str(&c),
            (None, req, resp) => {
                if let Some(r) = req {
                    out.push_str("## request mapping template\n");
                    out.push_str(&r);
                    out.push('\n');
                }
                if let Some(r) = resp {
                    out.push_str("## response mapping template\n");
                    out.push_str(&r);
                }
            }
        }
        out.push_str("\n\n");
    }
    (out, ext)
}

#[cfg(test)]
mod appsync_tests {
    use super::*;
    use crate::aws::services::appsync::{AppSyncResolver, MAX_RESOLVER_TYPES};

    #[test]
    fn api_keys_render_masked_with_expiry_state() {
        let now = 1_800_000_000;
        let keys = vec![
            AppSyncApiKey { masked_id: "da2-…aaaa".into(), description: Some("ci".into()), expires: now - 3600, deletes: now + 86400 },
            AppSyncApiKey { masked_id: "da2-…bbbb".into(), description: None, expires: now + 3 * 86400, deletes: 0 },
            AppSyncApiKey { masked_id: "da2-…cccc".into(), description: None, expires: now + 90 * 86400, deletes: 0 },
        ];
        let rows = api_key_rows(&keys, now);
        let text = format!("{rows:?}");
        assert!(text.contains("da2-…aaaa") && text.contains("(expired)"));
        assert!(text.contains("within 7 days"));
        assert!(rows.iter().any(|(k, v)| k == "  Expires" && !v.starts_with('✗') && !v.starts_with('⚠')));
        assert_eq!(api_key_rows(&[], now), vec![kv("", "No API keys")]);
    }

    #[test]
    fn resolvers_name_pipeline_functions_and_flag_the_cap() {
        let r = AppSyncResolvers {
            resolvers: vec![AppSyncResolver {
                type_name: "Mutation".into(),
                field_name: "placeOrder".into(),
                kind: "PIPELINE".into(),
                pipeline_functions: vec!["fn1".into(), "fn2".into()],
                runtime: "APPSYNC_JS 1.0.0".into(),
                ..Default::default()
            }],
            types_total: MAX_RESOLVER_TYPES + 20,
            types_scanned: MAX_RESOLVER_TYPES,
            failed_types: vec![],
            failure: None,
        };
        let fns = vec![AppSyncFunction { function_id: "fn1".into(), name: "validate".into(), ..Default::default() }];
        let rows = resolver_rows(&r, Some(&fns));
        assert!(rows.iter().any(|(_, v)| v.starts_with("⚠ Only the first 100 of 120")));
        assert!(rows.iter().any(|(k, _)| k == "Mutation.placeOrder"));
        assert!(rows.iter().any(|(k, v)| k == "  Pipeline" && v == "validate → fn2"));
    }

    #[test]
    fn failed_resolver_types_carry_the_aws_reason() {
        let r = AppSyncResolvers {
            failed_types: vec!["Mutation".into(), "Query".into()],
            failure: Some("AccessDeniedException: not authorized to perform appsync:ListResolvers".into()),
            ..Default::default()
        };
        let rows = resolver_rows(&r, None);
        assert!(rows.iter().any(|(_, v)| v
            == "⚠ Resolvers unavailable for: Mutation, Query — AccessDeniedException: not authorized to perform appsync:ListResolvers"));
    }

    #[test]
    fn code_document_prefers_js_code_then_templates() {
        let (doc, ext) = appsync_code_document(
            vec![
                ("Query.a".to_string(), Some("export function request() {}".to_string()), None, None),
                ("Query.b".to_string(), None, Some("{\"version\": \"2018-05-29\"}".to_string()), Some("$util.toJson($ctx.result)".to_string())),
            ]
            .into_iter(),
        );
        assert!(doc.contains("// ── Query.a ──\nexport function request() {}"));
        assert!(doc.contains("## request mapping template\n{\"version\""));
        assert!(doc.contains("## response mapping template\n$util.toJson"));
        assert_eq!(ext, ".js");
    }

    #[test]
    fn all_vtl_code_opens_as_vtl() {
        let (doc, ext) = appsync_code_document(
            vec![(
                "Query.b".to_string(),
                None,
                Some("{\"version\": \"2018-05-29\"}".to_string()),
                Some("$util.toJson($ctx.result)".to_string()),
            )]
            .into_iter(),
        );
        assert_eq!(ext, ".vtl");
        assert!(doc.starts_with("## ── Query.b ──\n"), "{doc}");
    }
}
