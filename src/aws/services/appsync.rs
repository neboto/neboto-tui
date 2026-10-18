use crate::aws::client::AwsClients;
use crate::aws::resource::{Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::error::Result;
use crate::event::{Event, LoadProgress};
use async_trait::async_trait;
use aws_sdk_appsync::Client as AppSyncClient;
use futures::stream::StreamExt;
use std::any::Any;
use std::collections::HashMap;
use tokio::sync::mpsc;

/// AWS AppSync — single list of **GraphQL APIs** (standard + merged) with a
/// split detail pane. `ListGraphqlApis` carries the whole API config (auth
/// modes, endpoints, logging, WAF, tags), so the list is one paginated call
/// with no per-API describe. Everything under an API — data sources,
/// resolvers, functions, API keys, the schema — is a lazy section.
///
/// Event APIs (`ListApis`, the v2 channel-namespace APIs) are not listed yet.
pub struct AppSyncService {
    client: AppSyncClient,
}

impl AppSyncService {
    pub fn new(aws_clients: &AwsClients) -> Self {
        Self {
            client: aws_clients.appsync_client(),
        }
    }
}

#[async_trait]
impl AwsService for AppSyncService {
    fn service_type(&self) -> ServiceType {
        ServiceType::AppSync
    }

    fn name(&self) -> &str {
        "AppSync"
    }

    async fn list_resources(&self) -> Result<Vec<Box<dyn Resource>>> {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        self.list_resources_streaming(tx, ServiceType::AppSync).await?;
        Ok(vec![])
    }

    async fn list_resources_streaming(
        &self,
        event_tx: mpsc::UnboundedSender<Event>,
        service_type: ServiceType,
    ) -> Result<()> {
        let mut total = 0usize;
        let mut token: Option<String> = None;
        loop {
            let mut req = self.client.list_graphql_apis();
            if let Some(t) = &token {
                req = req.next_token(t);
            }
            let page = match req.send().await {
                Ok(p) => p,
                Err(e) => {
                    if total == 0 {
                        let _ = event_tx.send(Event::ResourceLoadError {
                            service: service_type,
                            error: format!(
                                "Failed to list AppSync APIs: {}",
                                crate::error::sdk_error_message(&e)
                            ),
                        });
                        return Ok(());
                    }
                    // A later page failing keeps what already streamed.
                    let _ = event_tx.send(Event::ResourceLoadWarning {
                        service: service_type,
                        warning: format!(
                            "AppSync APIs: {}",
                            crate::error::sdk_error_message(&e)
                        ),
                    });
                    break;
                }
            };
            let batch: Vec<Box<dyn Resource>> = page
                .graphql_apis()
                .iter()
                .map(|a| Box::new(AppSyncApi::from_sdk(a)) as Box<dyn Resource>)
                .collect();
            if !batch.is_empty() {
                total += batch.len();
                let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                    service: service_type,
                    resources: batch,
                    progress: LoadProgress {
                        loaded_count: total,
                        total_count: None,
                        status_message: None,
                    },
                });
            }
            token = crate::aws::pagination::next_page_token(page.next_token(), &token);
            if token.is_none() {
                break;
            }
        }
        let _ = event_tx.send(Event::ResourcesFullyLoaded {
            service: service_type,
            total_count: total,
        });
        Ok(())
    }

    async fn get_resource_details(&self, _id: &str) -> Result<Box<dyn Resource>> {
        Err(crate::error::Error::NotImplemented)
    }
}

// ── AppSyncApi ────────────────────────────────────────────────────────────────

/// One authorization mode — the API's primary mode or an additional one.
/// Only the fields for its own type are set.
#[derive(Debug, Clone, Default)]
pub struct AppSyncAuthMode {
    pub auth_type: String,
    pub user_pool_id: Option<String>,
    pub user_pool_region: Option<String>,
    /// Primary-mode user pools only: ALLOW / DENY for tokens that match no
    /// `@aws_auth` group.
    pub user_pool_default_action: Option<String>,
    pub app_id_client_regex: Option<String>,
    pub oidc_issuer: Option<String>,
    pub oidc_client_id: Option<String>,
    pub lambda_authorizer_uri: Option<String>,
    pub lambda_result_ttl: Option<i32>,
}

#[derive(Debug, Clone)]
pub struct AppSyncApi {
    pub api_id: String,
    pub name: String,
    pub arn: String,
    /// GRAPHQL or MERGED.
    pub api_type: String,
    /// GLOBAL or PRIVATE.
    pub visibility: String,
    pub auth: AppSyncAuthMode,
    pub additional_auth: Vec<AppSyncAuthMode>,
    /// `uris` map: GRAPHQL / REALTIME endpoint URLs.
    pub uris: Vec<(String, String)>,
    pub xray_enabled: bool,
    pub waf_web_acl_arn: Option<String>,
    pub field_log_level: Option<String>,
    pub exclude_verbose_content: bool,
    pub logs_role_arn: Option<String>,
    pub merged_api_execution_role_arn: Option<String>,
    pub owner: Option<String>,
    pub owner_contact: Option<String>,
    pub introspection: Option<String>,
    pub query_depth_limit: i32,
    pub resolver_count_limit: i32,
    pub tags: HashMap<String, String>,
}

/// Every AppSync API logs to this group when field logging is on.
pub fn appsync_log_group(api_id: &str) -> String {
    format!("/aws/appsync/apis/{api_id}")
}

fn user_pool_mode(
    auth_type: String,
    pool: Option<&aws_sdk_appsync::types::UserPoolConfig>,
) -> AppSyncAuthMode {
    AppSyncAuthMode {
        auth_type,
        user_pool_id: pool.map(|p| p.user_pool_id().to_string()),
        user_pool_region: pool.map(|p| p.aws_region().to_string()),
        user_pool_default_action: pool.map(|p| p.default_action().as_str().to_string()),
        app_id_client_regex: pool.and_then(|p| p.app_id_client_regex().map(str::to_string)),
        ..Default::default()
    }
}

impl AppSyncApi {
    pub fn from_sdk(a: &aws_sdk_appsync::types::GraphqlApi) -> Self {
        let auth_type = a
            .authentication_type()
            .map(|t| t.as_str().to_string())
            .unwrap_or_default();
        let mut auth = user_pool_mode(auth_type, a.user_pool_config());
        if let Some(o) = a.open_id_connect_config() {
            auth.oidc_issuer = Some(o.issuer().to_string());
            auth.oidc_client_id = o.client_id().map(str::to_string);
        }
        if let Some(l) = a.lambda_authorizer_config() {
            auth.lambda_authorizer_uri = Some(l.authorizer_uri().to_string());
            auth.lambda_result_ttl = Some(l.authorizer_result_ttl_in_seconds());
        }
        let additional_auth = a
            .additional_authentication_providers()
            .iter()
            .map(|p| {
                let mut m = AppSyncAuthMode {
                    auth_type: p
                        .authentication_type()
                        .map(|t| t.as_str().to_string())
                        .unwrap_or_default(),
                    ..Default::default()
                };
                if let Some(c) = p.user_pool_config() {
                    m.user_pool_id = Some(c.user_pool_id().to_string());
                    m.user_pool_region = Some(c.aws_region().to_string());
                    m.app_id_client_regex = c.app_id_client_regex().map(str::to_string);
                }
                if let Some(o) = p.open_id_connect_config() {
                    m.oidc_issuer = Some(o.issuer().to_string());
                    m.oidc_client_id = o.client_id().map(str::to_string);
                }
                if let Some(l) = p.lambda_authorizer_config() {
                    m.lambda_authorizer_uri = Some(l.authorizer_uri().to_string());
                    m.lambda_result_ttl = Some(l.authorizer_result_ttl_in_seconds());
                }
                m
            })
            .collect();
        let sorted_pairs = |m: Option<&HashMap<String, String>>| {
            let mut v: Vec<(String, String)> = m
                .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                .unwrap_or_default();
            v.sort();
            v
        };
        let log = a.log_config();
        Self {
            api_id: a.api_id().unwrap_or_default().to_string(),
            name: a.name().unwrap_or_default().to_string(),
            arn: a.arn().unwrap_or_default().to_string(),
            api_type: a
                .api_type()
                .map(|t| t.as_str().to_string())
                .unwrap_or_else(|| "GRAPHQL".to_string()),
            visibility: a
                .visibility()
                .map(|v| v.as_str().to_string())
                .unwrap_or_else(|| "GLOBAL".to_string()),
            auth,
            additional_auth,
            uris: sorted_pairs(a.uris()),
            xray_enabled: a.xray_enabled(),
            waf_web_acl_arn: a.waf_web_acl_arn().map(str::to_string),
            field_log_level: log.map(|l| l.field_log_level().as_str().to_string()),
            exclude_verbose_content: log.map(|l| l.exclude_verbose_content()).unwrap_or(false),
            logs_role_arn: log
                .map(|l| l.cloud_watch_logs_role_arn().to_string())
                .filter(|s| !s.is_empty()),
            merged_api_execution_role_arn: a.merged_api_execution_role_arn().map(str::to_string),
            owner: a.owner().map(str::to_string),
            owner_contact: a.owner_contact().map(str::to_string),
            introspection: a.introspection_config().map(|c| c.as_str().to_string()),
            query_depth_limit: a.query_depth_limit(),
            resolver_count_limit: a.resolver_count_limit(),
            tags: a.tags().cloned().unwrap_or_default(),
        }
    }

    /// Every auth mode, primary first.
    pub fn auth_modes(&self) -> impl Iterator<Item = &AppSyncAuthMode> {
        std::iter::once(&self.auth).chain(self.additional_auth.iter())
    }

    /// The GraphQL endpoint URL, if AppSync returned one.
    pub fn graphql_url(&self) -> Option<&str> {
        self.uris
            .iter()
            .find(|(k, _)| k == "GRAPHQL")
            .map(|(_, v)| v.as_str())
    }

    /// Whether field-level logging is on (anything but NONE).
    pub fn logging_enabled(&self) -> bool {
        self.field_log_level.as_deref().is_some_and(|l| l != "NONE")
    }
}

crate::sections! {
    pub enum AppSyncApiDetailSection,
    pub static APPSYNC_API_SECTIONS = [
        Overview "Overview",
        Auth "Auth",
        DataSources "Data Sources" => crate::app::App::trigger_appsync_data_sources_load,
        Resolvers "Resolvers" => crate::app::App::trigger_appsync_resolvers_load,
        Functions "Functions" => crate::app::App::trigger_appsync_functions_load,
        ApiKeys "API Keys" => crate::app::App::trigger_appsync_api_keys_load,
        Schema "Schema" => crate::app::App::trigger_appsync_schema_load,
        Tags "Tags",
    ]
}

impl Resource for AppSyncApi {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&APPSYNC_API_SECTIONS)
    }

    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws appsync get-graphql-api --api-id {}",
            crate::aws::resource::shell_quote(&self.api_id)
        ))
    }

    fn id(&self) -> &str {
        &self.api_id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn resource_type(&self) -> &str {
        "AppSync API"
    }

    /// AppSync APIs have no lifecycle state.
    fn state(&self) -> ResourceState {
        ResourceState::stateless()
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        let auth: Vec<&str> = self.auth_modes().map(|m| m.auth_type.as_str()).collect();
        format!(
            "{} {} {} {} {} {}",
            self.name,
            self.api_id,
            self.arn,
            self.api_type,
            self.visibility,
            auth.join(" "),
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("API ID".to_string(), self.api_id.clone()),
            ("Name".to_string(), self.name.clone()),
            ("Type".to_string(), self.api_type.clone()),
            ("Visibility".to_string(), self.visibility.clone()),
            ("Auth".to_string(), self.auth.auth_type.clone()),
            (
                "GraphQL Endpoint".to_string(),
                self.graphql_url().unwrap_or_default().to_string(),
            ),
            ("ARN".to_string(), self.arn.clone()),
        ]
    }

    fn references(&self) -> Vec<(String, String)> {
        let mut v = Vec::new();
        let mut r = |l: &str, x: &str| v.push((l.to_string(), x.to_string()));
        for m in self.auth_modes() {
            if let Some(x) = &m.user_pool_id {
                r("Cognito User Pool", x);
            }
            if let Some(x) = &m.lambda_authorizer_uri {
                r("Lambda Authorizer", x);
            }
        }
        if let Some(x) = &self.waf_web_acl_arn {
            r("WAF Web ACL", x);
        }
        if let Some(x) = &self.logs_role_arn {
            r("Logs Role", x);
        }
        if let Some(x) = &self.merged_api_execution_role_arn {
            r("Merged API Execution Role", x);
        }
        if self.logging_enabled() {
            r("Log Group", &appsync_log_group(&self.api_id));
        }
        v
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{region}.console.aws.amazon.com/appsync/home?region={region}#/{}/v1/home",
            self.api_id
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── Lazy: data sources ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct AppSyncDataSource {
    pub name: String,
    pub ds_type: String,
    pub description: Option<String>,
    pub service_role_arn: Option<String>,
    pub dynamodb_table: Option<String>,
    pub lambda_function_arn: Option<String>,
    pub http_endpoint: Option<String>,
    pub opensearch_endpoint: Option<String>,
    pub event_bus_arn: Option<String>,
    pub rds_cluster_arn: Option<String>,
    pub rds_database: Option<String>,
    pub rds_secret_arn: Option<String>,
    /// Region of the backing resource, when the config names one.
    pub region: Option<String>,
}

impl AppSyncDataSource {
    fn from_sdk(d: &aws_sdk_appsync::types::DataSource) -> Self {
        let mut ds = AppSyncDataSource {
            name: d.name().unwrap_or_default().to_string(),
            ds_type: d.r#type().map(|t| t.as_str().to_string()).unwrap_or_default(),
            description: d.description().filter(|s| !s.is_empty()).map(str::to_string),
            service_role_arn: d.service_role_arn().map(str::to_string),
            ..Default::default()
        };
        if let Some(c) = d.dynamodb_config() {
            ds.dynamodb_table = Some(c.table_name().to_string());
            ds.region = Some(c.aws_region().to_string());
        }
        if let Some(c) = d.lambda_config() {
            ds.lambda_function_arn = Some(c.lambda_function_arn().to_string());
        }
        if let Some(c) = d.http_config() {
            ds.http_endpoint = c.endpoint().map(str::to_string);
        }
        if let Some(c) = d.open_search_service_config() {
            ds.opensearch_endpoint = Some(c.endpoint().to_string());
            ds.region = Some(c.aws_region().to_string());
        } else if let Some(c) = d.elasticsearch_config() {
            ds.opensearch_endpoint = Some(c.endpoint().to_string());
            ds.region = Some(c.aws_region().to_string());
        }
        if let Some(c) = d.event_bridge_config() {
            ds.event_bus_arn = Some(c.event_bus_arn().to_string());
        }
        if let Some(rds) = d
            .relational_database_config()
            .and_then(|c| c.rds_http_endpoint_config())
        {
            ds.rds_cluster_arn = rds.db_cluster_identifier().map(str::to_string);
            ds.rds_database = rds.database_name().map(str::to_string);
            ds.rds_secret_arn = rds.aws_secret_store_arn().map(str::to_string);
            ds.region = rds.aws_region().map(str::to_string);
        }
        ds
    }
}

/// The OpenSearch domain name behind a domain endpoint
/// (`search-<domain>-<hash>.<region>.es.amazonaws.com`, or `vpc-…` for a VPC
/// domain). Domain names may contain hyphens, so the hash is the *last*
/// hyphen segment. `None` for anything that isn't that shape (a custom
/// endpoint), so the row just doesn't jump.
pub fn opensearch_domain_from_endpoint(endpoint: &str) -> Option<String> {
    let host = endpoint
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    let host = host.split(['/', ':']).next()?;
    if !host.contains(".es.amazonaws.com") {
        return None;
    }
    let label = host.split('.').next()?;
    let rest = label
        .strip_prefix("search-")
        .or_else(|| label.strip_prefix("vpc-"))?;
    let (domain, _hash) = rest.rsplit_once('-')?;
    (!domain.is_empty()).then(|| domain.to_string())
}

pub async fn fetch_data_sources(
    client: AppSyncClient,
    api_id: String,
) -> Result<Vec<AppSyncDataSource>> {
    let mut out = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut req = client.list_data_sources().api_id(&api_id);
        if let Some(t) = &token {
            req = req.next_token(t);
        }
        let page = req
            .send()
            .await
            .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
        out.extend(page.data_sources().iter().map(AppSyncDataSource::from_sdk));
        token = crate::aws::pagination::next_page_token(page.next_token(), &token);
        if token.is_none() {
            break;
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

// ── Lazy: functions ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct AppSyncFunction {
    pub function_id: String,
    pub name: String,
    pub description: Option<String>,
    pub data_source: Option<String>,
    /// `APPSYNC_JS 1.0.0`, or `VTL <version>` for mapping-template functions.
    pub runtime: String,
    pub max_batch_size: i32,
    pub code: Option<String>,
    pub request_template: Option<String>,
    pub response_template: Option<String>,
}

fn runtime_label(
    runtime: Option<&aws_sdk_appsync::types::AppSyncRuntime>,
    vtl_version: Option<&str>,
) -> String {
    match runtime {
        Some(r) => format!("{} {}", r.name().as_str(), r.runtime_version()),
        None => match vtl_version {
            Some(v) => format!("VTL {v}"),
            None => "VTL".to_string(),
        },
    }
}

pub async fn fetch_functions(client: AppSyncClient, api_id: String) -> Result<Vec<AppSyncFunction>> {
    let mut out = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut req = client.list_functions().api_id(&api_id);
        if let Some(t) = &token {
            req = req.next_token(t);
        }
        let page = req
            .send()
            .await
            .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
        for f in page.functions() {
            out.push(AppSyncFunction {
                function_id: f.function_id().unwrap_or_default().to_string(),
                name: f.name().unwrap_or_default().to_string(),
                description: f.description().filter(|s| !s.is_empty()).map(str::to_string),
                data_source: f.data_source_name().map(str::to_string),
                runtime: runtime_label(f.runtime(), f.function_version()),
                max_batch_size: f.max_batch_size(),
                code: f.code().map(str::to_string),
                request_template: f.request_mapping_template().map(str::to_string),
                response_template: f.response_mapping_template().map(str::to_string),
            });
        }
        token = crate::aws::pagination::next_page_token(page.next_token(), &token);
        if token.is_none() {
            break;
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

// ── Lazy: resolvers ───────────────────────────────────────────────────────────

/// How many schema types the Resolvers section walks. AppSync has no
/// API-wide `ListResolvers` — it's one call **per type** — so a large schema
/// would otherwise fan out into hundreds of requests on a Tab press.
pub const MAX_RESOLVER_TYPES: usize = 100;

#[derive(Debug, Clone, Default)]
pub struct AppSyncResolver {
    pub type_name: String,
    pub field_name: String,
    /// UNIT or PIPELINE.
    pub kind: String,
    pub data_source: Option<String>,
    /// Pipeline resolvers: the function ids, in order.
    pub pipeline_functions: Vec<String>,
    pub runtime: String,
    pub caching_ttl: Option<i64>,
    pub max_batch_size: i32,
    pub code: Option<String>,
    pub request_template: Option<String>,
    pub response_template: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct AppSyncResolvers {
    pub resolvers: Vec<AppSyncResolver>,
    /// Object types in the schema (the ones that can carry resolvers).
    pub types_total: usize,
    /// Of those, how many were walked (≤ `MAX_RESOLVER_TYPES`).
    pub types_scanned: usize,
    /// Types whose `ListResolvers` failed — their resolvers are missing.
    pub failed_types: Vec<String>,
    /// The first of those failures' AWS message (usually they share one,
    /// e.g. a missing `appsync:ListResolvers`).
    pub failure: Option<String>,
}

/// Root operation types first, so a cap never cuts Query / Mutation /
/// Subscription; everything else alphabetical.
fn type_rank(name: &str) -> (u8, &str) {
    let r = match name {
        "Query" => 0,
        "Mutation" => 1,
        "Subscription" => 2,
        _ => 3,
    };
    (r, name)
}

/// The first keyword of an SDL definition, past any leading description
/// (`"""…"""` or `"…"`), `#` comments and whitespace — `type`, `enum`, ….
fn sdl_keyword(def: &str) -> &str {
    let mut rest = def;
    loop {
        rest = rest.trim_start();
        if let Some(r) = rest.strip_prefix("\"\"\"") {
            rest = r.find("\"\"\"").map_or("", |i| &r[i + 3..]);
        } else if let Some(r) = rest.strip_prefix('"') {
            rest = r.find('"').map_or("", |i| &r[i + 1..]);
        } else if rest.starts_with('#') {
            rest = rest.find('\n').map_or("", |i| &rest[i + 1..]);
        } else {
            break;
        }
    }
    let rest = rest.strip_prefix("extend ").map_or(rest, str::trim_start);
    rest.split(|c: char| !c.is_alphanumeric()).next().unwrap_or("")
}

/// Pick which schema types to walk for resolvers: roots first, cut at
/// `cap`. Only kinds that *can't* carry a resolver (enum, input, scalar,
/// union, interface) are skipped — anything else is walked, so a definition
/// shape we don't recognise costs one extra call rather than silently
/// hiding its resolvers. Returns the chosen names and how many candidate
/// types there were in total.
pub fn resolver_types_to_scan(types: &[(String, Option<String>)], cap: usize) -> (Vec<String>, usize) {
    let mut names: Vec<&str> = types
        .iter()
        .filter(|(_, def)| {
            !matches!(
                sdl_keyword(def.as_deref().unwrap_or("")),
                "enum" | "input" | "scalar" | "union" | "interface"
            )
        })
        .map(|(n, _)| n.as_str())
        .collect();
    names.sort_by_key(|n| (type_rank(n).0, *n));
    names.dedup();
    let total = names.len();
    (names.into_iter().take(cap).map(str::to_string).collect(), total)
}

pub async fn fetch_resolvers(client: AppSyncClient, api_id: String) -> Result<AppSyncResolvers> {
    use aws_sdk_appsync::types::TypeDefinitionFormat;

    // Phase 1: the schema's types (SDL, so object types can be told apart).
    let mut types: Vec<(String, Option<String>)> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut req = client
            .list_types()
            .api_id(&api_id)
            .format(TypeDefinitionFormat::Sdl);
        if let Some(t) = &token {
            req = req.next_token(t);
        }
        let page = req
            .send()
            .await
            .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
        for t in page.types() {
            if let Some(n) = t.name() {
                types.push((n.to_string(), t.definition().map(str::to_string)));
            }
        }
        token = crate::aws::pagination::next_page_token(page.next_token(), &token);
        if token.is_none() {
            break;
        }
    }
    let (scan, types_total) = resolver_types_to_scan(&types, MAX_RESOLVER_TYPES);
    let types_scanned = scan.len();

    // Phase 2: resolvers per type, a few in flight (AppSync's control plane
    // throttles hard). One type failing doesn't sink the rest.
    let results: Vec<(String, std::result::Result<Vec<AppSyncResolver>, String>)> =
        futures::stream::iter(scan)
            .map(|type_name| {
                let client = client.clone();
                let api_id = api_id.clone();
                async move {
                    let mut out = Vec::new();
                    let mut token: Option<String> = None;
                    loop {
                        let mut req = client
                            .list_resolvers()
                            .api_id(&api_id)
                            .type_name(&type_name);
                        if let Some(t) = &token {
                            req = req.next_token(t);
                        }
                        let page = match req.send().await {
                            Ok(p) => p,
                            Err(e) => return (type_name, Err(crate::error::sdk_error_message(&e))),
                        };
                        for r in page.resolvers() {
                            out.push(AppSyncResolver {
                                type_name: r.type_name().unwrap_or(&type_name).to_string(),
                                field_name: r.field_name().unwrap_or_default().to_string(),
                                kind: r
                                    .kind()
                                    .map(|k| k.as_str().to_string())
                                    .unwrap_or_else(|| "UNIT".to_string()),
                                data_source: r.data_source_name().map(str::to_string),
                                pipeline_functions: r
                                    .pipeline_config()
                                    .map(|p| p.functions().to_vec())
                                    .unwrap_or_default(),
                                runtime: runtime_label(r.runtime(), None),
                                caching_ttl: r.caching_config().map(|c| c.ttl()),
                                max_batch_size: r.max_batch_size(),
                                code: r.code().map(str::to_string),
                                request_template: r.request_mapping_template().map(str::to_string),
                                response_template: r.response_mapping_template().map(str::to_string),
                            });
                        }
                        token = crate::aws::pagination::next_page_token(page.next_token(), &token);
                        if token.is_none() {
                            break;
                        }
                    }
                    (type_name, Ok(out))
                }
            })
            .buffer_unordered(4)
            .collect()
            .await;

    let mut resolvers = Vec::new();
    let mut failed_types = Vec::new();
    let mut failure = None;
    for (type_name, r) in results {
        match r {
            Ok(v) => resolvers.extend(v),
            Err(e) => {
                failure.get_or_insert(e);
                failed_types.push(type_name);
            }
        }
    }
    resolvers.sort_by(|a, b| {
        type_rank(&a.type_name)
            .cmp(&type_rank(&b.type_name))
            .then_with(|| a.field_name.cmp(&b.field_name))
    });
    failed_types.sort();
    Ok(AppSyncResolvers {
        resolvers,
        types_total,
        types_scanned,
        failed_types,
        failure,
    })
}

// ── Lazy: API keys (redacted) ─────────────────────────────────────────────────

/// An API key, **redacted at ingestion**. AppSync's key id *is* the secret
/// (it's what a client sends as `x-api-key`), so the full id never leaves
/// `fetch_api_keys` — not into `App`, the pane, an export or the editor.
#[derive(Debug, Clone, Default)]
pub struct AppSyncApiKey {
    pub masked_id: String,
    pub description: Option<String>,
    /// Epoch seconds (AppSync rounds down to the hour).
    pub expires: i64,
    /// Epoch seconds after which AppSync deletes the expired key.
    pub deletes: i64,
}

/// `da2-abcdefghijklmnopqrstuvwxyz` → `da2-…wxyz`. Keeps the `da2-` style
/// prefix (it names the key format, not the key) and the last four
/// characters, which is enough to tell keys apart. Anything too short to
/// keep four characters back is masked completely.
pub fn mask_api_key_id(id: &str) -> String {
    let chars: Vec<char> = id.chars().collect();
    if chars.len() < 12 {
        return "••••".to_string();
    }
    let prefix: String = match id.find('-') {
        Some(i) if i <= 4 => id[..=i].to_string(),
        _ => String::new(),
    };
    let tail: String = chars[chars.len() - 4..].iter().collect();
    format!("{prefix}…{tail}")
}

pub async fn fetch_api_keys(client: AppSyncClient, api_id: String) -> Result<Vec<AppSyncApiKey>> {
    let mut out = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut req = client.list_api_keys().api_id(&api_id);
        if let Some(t) = &token {
            req = req.next_token(t);
        }
        let page = req
            .send()
            .await
            .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
        for k in page.api_keys() {
            out.push(AppSyncApiKey {
                masked_id: mask_api_key_id(k.id().unwrap_or_default()),
                description: k.description().filter(|s| !s.is_empty()).map(str::to_string),
                expires: k.expires(),
                deletes: k.deletes(),
            });
        }
        token = crate::aws::pagination::next_page_token(page.next_token(), &token);
        if token.is_none() {
            break;
        }
    }
    out.sort_by_key(|k| k.expires);
    Ok(out)
}

// ── Lazy: schema ──────────────────────────────────────────────────────────────

/// The schema as SDL (`GetIntrospectionSchema`, directives included).
pub async fn fetch_schema(client: AppSyncClient, api_id: String) -> Result<String> {
    let resp = client
        .get_introspection_schema()
        .api_id(&api_id)
        .format(aws_sdk_appsync::types::OutputType::Sdl)
        .include_directives(true)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
    Ok(resp
        .schema()
        .map(|b| String::from_utf8_lossy(b.as_ref()).into_owned())
        .unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_key_mask_never_keeps_the_secret() {
        let id = "da2-abcdefghijklmnopqrstuvwxyz";
        let m = mask_api_key_id(id);
        assert_eq!(m, "da2-…wxyz");
        assert!(!m.contains("abcdefgh"));
        assert_eq!(mask_api_key_id("short"), "••••");
        assert_eq!(mask_api_key_id(""), "••••");
        // No recognisable prefix: tail only.
        assert_eq!(mask_api_key_id("abcdefghijklmnop"), "…mnop");
    }

    #[test]
    fn resolver_scan_keeps_roots_skips_non_object_types_and_caps() {
        let t = |n: &str, d: &str| (n.to_string(), Some(d.to_string()));
        let types = vec![
            t("Order", "type Order { id: ID! }"),
            t("OrderStatus", "enum OrderStatus { OPEN }"),
            t("OrderInput", "input OrderInput { id: ID! }"),
            t("Mutation", "type Mutation { a: Int }"),
            t("Query", "type Query { a: Int }"),
            ("Unknown".to_string(), None),
        ];
        let (scan, total) = resolver_types_to_scan(&types, 100);
        assert_eq!(scan, vec!["Query", "Mutation", "Order", "Unknown"]);
        assert_eq!(total, 4);
        let (scan, total) = resolver_types_to_scan(&types, 2);
        assert_eq!(scan, vec!["Query", "Mutation"]);
        assert_eq!(total, 4);
    }

    #[test]
    fn resolver_scan_sees_past_descriptions_and_comments() {
        let t = |n: &str, d: &str| (n.to_string(), Some(d.to_string()));
        let types = vec![
            t("Order", "\"\"\"\nAn order.\n\"\"\"\ntype Order { id: ID! }"),
            t("Product", "\"A product\" type Product @aws_iam { id: ID! }"),
            t("Cart", "# carts\ntype Cart { id: ID! }"),
            t("Status", "\"\"\"States\"\"\" enum Status { OPEN }"),
            t("Node", "interface Node { id: ID! }"),
            t("Query", "extend type Query { a: Int }"),
        ];
        let (scan, total) = resolver_types_to_scan(&types, 100);
        assert_eq!(scan, vec!["Query", "Cart", "Order", "Product"]);
        assert_eq!(total, 4);
    }

    #[test]
    fn opensearch_domain_comes_off_the_endpoint() {
        assert_eq!(
            opensearch_domain_from_endpoint("https://search-orders-index-abc123xyz.us-east-1.es.amazonaws.com"),
            Some("orders-index".to_string())
        );
        assert_eq!(
            opensearch_domain_from_endpoint("vpc-logs-q1w2e3.eu-west-1.es.amazonaws.com"),
            Some("logs".to_string())
        );
        assert_eq!(opensearch_domain_from_endpoint("https://search.example.com"), None);
    }
}
