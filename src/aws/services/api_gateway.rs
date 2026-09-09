use crate::aws::client::AwsClients;
use crate::aws::resource::{native_state_label, Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::error::Result;
use crate::event::{Event, LoadProgress};
use async_trait::async_trait;
use aws_sdk_apigateway::Client as V1Client;
use aws_sdk_apigatewayv2::Client as V2Client;
use std::any::Any;
use std::collections::HashMap;
use tokio::sync::mpsc;

/// API Gateway — one service wrapping two SDK clients: v1 (REST APIs) and v2
/// (HTTP + WebSocket APIs). Two resource types over one list, split by the
/// sub-tab filter. Children (resources/routes/stages/authorizers) are fetched
/// lazily per API as one combined call; tags come inline on the list load.
pub struct ApiGatewayService {
    v1: V1Client,
    v2: V2Client,
}

impl ApiGatewayService {
    pub fn new(aws_clients: &AwsClients) -> Self {
        Self {
            v1: aws_clients.apigateway_client(),
            v2: aws_clients.apigatewayv2_client(),
        }
    }
}

#[async_trait]
impl AwsService for ApiGatewayService {
    fn service_type(&self) -> ServiceType {
        ServiceType::ApiGateway
    }

    fn name(&self) -> &str {
        "API Gateway"
    }

    async fn list_resources(&self) -> Result<Vec<Box<dyn Resource>>> {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        self.list_resources_streaming(tx, ServiceType::ApiGateway)
            .await?;
        Ok(vec![])
    }

    async fn list_resources_streaming(
        &self,
        event_tx: mpsc::UnboundedSender<Event>,
        service_type: ServiceType,
    ) -> Result<()> {
        // Fetch v1 REST + v2 HTTP + custom domains + usage plans + VPC links
        // concurrently, each emitting its own batch. (The management plane's
        // rate limit is per-call, not per-connection; five list calls in
        // flight is fine.)
        let (rest, http, domains, plans, links_v1, links_v2) = tokio::join!(
            fetch_rest_apis(self.v1.clone()),
            fetch_http_apis(self.v2.clone()),
            fetch_custom_domains(self.v1.clone(), self.v2.clone()),
            fetch_usage_plans(self.v1.clone()),
            fetch_vpc_links_v1(self.v1.clone()),
            fetch_vpc_links_v2(self.v2.clone()),
        );

        let mut total = 0usize;

        match rest {
            Ok(apis) => {
                total += apis.len();
                if !apis.is_empty() {
                    let batch: Vec<Box<dyn Resource>> =
                        apis.into_iter().map(|a| Box::new(a) as Box<dyn Resource>).collect();
                    let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                        service: service_type,
                        resources: batch,
                        progress: LoadProgress {
                            loaded_count: total,
                            total_count: None,
                            status_message: Some("Loading REST APIs…".to_string()),
                        },
                    });
                }
            }
            Err(e) => {
                let _ = event_tx.send(Event::ResourceLoadWarning {
                    service: service_type,
                    warning: format!("REST APIs: {}", e),
                });
            }
        }

        match http {
            Ok(apis) => {
                total += apis.len();
                if !apis.is_empty() {
                    let batch: Vec<Box<dyn Resource>> =
                        apis.into_iter().map(|a| Box::new(a) as Box<dyn Resource>).collect();
                    let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                        service: service_type,
                        resources: batch,
                        progress: LoadProgress {
                            loaded_count: total,
                            total_count: None,
                            status_message: Some("Loading HTTP APIs…".to_string()),
                        },
                    });
                }
            }
            Err(e) => {
                let _ = event_tx.send(Event::ResourceLoadWarning {
                    service: service_type,
                    warning: format!("HTTP APIs: {}", e),
                });
            }
        }

        match domains {
            Ok(doms) => {
                total += doms.len();
                if !doms.is_empty() {
                    let batch: Vec<Box<dyn Resource>> =
                        doms.into_iter().map(|d| Box::new(d) as Box<dyn Resource>).collect();
                    let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                        service: service_type,
                        resources: batch,
                        progress: LoadProgress {
                            loaded_count: total,
                            total_count: None,
                            status_message: Some("Loading custom domains…".to_string()),
                        },
                    });
                }
            }
            Err(e) => {
                let _ = event_tx.send(Event::ResourceLoadWarning {
                    service: service_type,
                    warning: format!("custom domains: {}", e),
                });
            }
        }

        match plans {
            Ok(items) => {
                total += items.len();
                if !items.is_empty() {
                    let batch: Vec<Box<dyn Resource>> =
                        items.into_iter().map(|p| Box::new(p) as Box<dyn Resource>).collect();
                    let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                        service: service_type,
                        resources: batch,
                        progress: LoadProgress {
                            loaded_count: total,
                            total_count: None,
                            status_message: Some("Loading usage plans…".to_string()),
                        },
                    });
                }
            }
            Err(e) => {
                let _ = event_tx.send(Event::ResourceLoadWarning {
                    service: service_type,
                    warning: format!("usage plans: {}", e),
                });
            }
        }

        // VPC links: merge both control planes (distinct id spaces, no dedup);
        // each side degrades to a warning on its own.
        let mut links: Vec<ApiVpcLink> = Vec::new();
        match links_v1 {
            Ok(v) => links.extend(v),
            Err(e) => {
                let _ = event_tx.send(Event::ResourceLoadWarning {
                    service: service_type,
                    warning: format!("VPC links (REST): {}", e),
                });
            }
        }
        match links_v2 {
            Ok(v) => links.extend(v),
            Err(e) => {
                let _ = event_tx.send(Event::ResourceLoadWarning {
                    service: service_type,
                    warning: format!("VPC links (HTTP): {}", e),
                });
            }
        }
        links.sort_by(|a, b| a.name.cmp(&b.name));
        total += links.len();
        if !links.is_empty() {
            let batch: Vec<Box<dyn Resource>> =
                links.into_iter().map(|l| Box::new(l) as Box<dyn Resource>).collect();
            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                service: service_type,
                resources: batch,
                progress: LoadProgress {
                    loaded_count: total,
                    total_count: None,
                    status_message: Some("Loading VPC links…".to_string()),
                },
            });
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

// ── List-phase fetches ────────────────────────────────────────────────────────

async fn fetch_rest_apis(client: V1Client) -> Result<Vec<RestApi>> {
    let mut out = Vec::new();
    let mut position: Option<String> = None;
    loop {
        let mut req = client.get_rest_apis();
        if let Some(p) = &position {
            req = req.position(p);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
        for a in resp.items() {
            out.push(RestApi::from_sdk(a));
        }
        position = crate::aws::pagination::next_page_token(resp.position(), &position);
        if position.is_none() {
            break;
        }
    }
    Ok(out)
}

async fn fetch_http_apis(client: V2Client) -> Result<Vec<HttpApi>> {
    let mut out = Vec::new();
    let mut next: Option<String> = None;
    loop {
        let mut req = client.get_apis();
        if let Some(t) = &next {
            req = req.next_token(t);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
        for a in resp.items() {
            out.push(HttpApi::from_sdk(a));
        }
        next = crate::aws::pagination::next_page_token(resp.next_token(), &next);
        if next.is_none() {
            break;
        }
    }
    Ok(out)
}

/// Fetch custom domains from both v1 and v2 and merge by domain name (a domain
/// can back both REST and HTTP APIs, so it may appear in both lists).
async fn fetch_custom_domains(v1: V1Client, v2: V2Client) -> Result<Vec<ApiCustomDomain>> {
    let (r1, r2) = tokio::join!(
        async {
            let mut out = Vec::new();
            let mut position: Option<String> = None;
            loop {
                let mut req = v1.get_domain_names();
                if let Some(p) = &position {
                    req = req.position(p);
                }
                let resp = req
                    .send()
                    .await
                    .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
                for d in resp.items() {
                    out.push(ApiCustomDomain::from_v1(d));
                }
                position = crate::aws::pagination::next_page_token(resp.position(), &position);
                if position.is_none() {
                    break;
                }
            }
            Ok::<_, crate::error::Error>(out)
        },
        async {
            let mut out = Vec::new();
            let mut next: Option<String> = None;
            loop {
                let mut req = v2.get_domain_names();
                if let Some(t) = &next {
                    req = req.next_token(t);
                }
                let resp = req
                    .send()
                    .await
                    .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
                for d in resp.items() {
                    out.push(ApiCustomDomain::from_v2(d));
                }
                next = crate::aws::pagination::next_page_token(resp.next_token(), &next);
                if next.is_none() {
                    break;
                }
            }
            Ok::<_, crate::error::Error>(out)
        },
    );

    // Either control plane may be absent in a region; tolerate one side erroring.
    let mut merged: Vec<ApiCustomDomain> = r1.unwrap_or_default();
    for d in r2.unwrap_or_default() {
        if let Some(existing) = merged.iter_mut().find(|m| m.domain_name == d.domain_name) {
            existing.serves = "REST, HTTP".to_string();
        } else {
            merged.push(d);
        }
    }
    merged.sort_by(|a, b| a.domain_name.cmp(&b.domain_name));
    Ok(merged)
}

// ── RestApi (v1) ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct RestApi {
    pub id: String,
    pub name: String,
    pub description: String,
    pub endpoint_type: String, // EDGE / REGIONAL / PRIVATE
    pub api_key_source: String,
    pub created: Option<String>,
    /// Resource policy (IAM-style JSON), returned inline + escaped by GetRestApis.
    pub policy: Option<String>,
    /// True when the default execute-api endpoint is disabled (hardened —
    /// callers must come through a custom domain).
    pub default_endpoint_disabled: bool,
    pub binary_media_types: Vec<String>,
    /// Payload size threshold (bytes) above which responses compress; None =
    /// compression disabled.
    pub minimum_compression_size: Option<i32>,
    pub tags: HashMap<String, String>,
}

impl RestApi {
    pub fn from_sdk(a: &aws_sdk_apigateway::types::RestApi) -> Self {
        Self {
            id: a.id().unwrap_or_default().to_string(),
            name: a.name().unwrap_or_default().to_string(),
            description: a.description().unwrap_or_default().to_string(),
            endpoint_type: a
                .endpoint_configuration()
                .map(|c| {
                    c.types()
                        .iter()
                        .map(|t| t.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default(),
            api_key_source: a
                .api_key_source()
                .map(|s| s.as_str().to_string())
                .unwrap_or_default(),
            created: a.created_date().map(|d| fmt_epoch_secs(d.secs())),
            policy: a
                .policy()
                .map(|s| s.to_string())
                .filter(|s| !s.is_empty()),
            default_endpoint_disabled: a.disable_execute_api_endpoint(),
            binary_media_types: a.binary_media_types().to_vec(),
            minimum_compression_size: a.minimum_compression_size(),
            tags: a.tags().cloned().unwrap_or_default(),
        }
    }

    /// Pretty-printed resource policy, or `None` when there isn't one. The API
    /// returns the document with its JSON quotes backslash-escaped, so unescape
    /// before parsing.
    pub fn pretty_policy(&self) -> Option<String> {
        let raw = self.policy.as_ref()?;
        let unescaped = raw.replace("\\\"", "\"").replace("\\\\", "\\");
        let pretty = serde_json::from_str::<serde_json::Value>(&unescaped)
            .ok()
            .and_then(|v| serde_json::to_string_pretty(&v).ok())
            .unwrap_or(unescaped);
        Some(pretty)
    }
}

crate::sections! {
    pub enum RestApiDetailSection,
    pub static REST_API_SECTIONS = [
        Overview "Overview",
        Resources "Resources" => crate::app::App::trigger_rest_api_details_load,
        Stages "Stages" => crate::app::App::trigger_rest_api_details_load,
        Authorizers "Authorizers" => crate::app::App::trigger_rest_api_details_load,
        Deployments "Deployments" => crate::app::App::trigger_rest_api_details_load,
        Tags "Tags",
    ]
}

impl Resource for RestApi {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&REST_API_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.id
    }
    fn name(&self) -> &str {
        if self.name.is_empty() {
            &self.id
        } else {
            &self.name
        }
    }
    fn resource_type(&self) -> &str {
        "REST API"
    }
    fn state(&self) -> ResourceState {
        ResourceState::stateless()
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        format!(
            "{} {} {} {}",
            self.id, self.name, self.description, self.endpoint_type
        )
    }
    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("ID".to_string(), self.id.clone()),
            ("Name".to_string(), self.name.clone()),
            ("Endpoint Type".to_string(), self.endpoint_type.clone()),
            ("API Key Source".to_string(), self.api_key_source.clone()),
            (
                "Created".to_string(),
                self.created.clone().unwrap_or_default(),
            ),
        ]
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{region}.console.aws.amazon.com/apigateway/main/apis/{}/resources?api={}&region={region}",
            self.id, self.id
        ))
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── HttpApi (v2) ──────────────────────────────────────────────────────────────

/// Flattened v2 CORS configuration (HTTP APIs only).
#[derive(Debug, Clone)]
pub struct ApiCors {
    pub allow_origins: Vec<String>,
    pub allow_methods: Vec<String>,
    pub allow_headers: Vec<String>,
    pub expose_headers: Vec<String>,
    pub allow_credentials: bool,
    pub max_age: Option<i32>,
}

#[derive(Debug, Clone)]
pub struct HttpApi {
    pub id: String,
    pub name: String,
    pub protocol: String, // HTTP / WEBSOCKET
    pub endpoint: String,
    pub description: String,
    pub created: Option<String>,
    /// True when the default execute-api endpoint is disabled.
    pub default_endpoint_disabled: bool,
    /// WebSocket APIs' route selection expression (e.g. `$request.body.action`).
    pub route_selection_expression: String,
    pub cors: Option<ApiCors>,
    pub tags: HashMap<String, String>,
}

impl HttpApi {
    pub fn from_sdk(a: &aws_sdk_apigatewayv2::types::Api) -> Self {
        Self {
            id: a.api_id().unwrap_or_default().to_string(),
            name: a.name().unwrap_or_default().to_string(),
            protocol: a
                .protocol_type()
                .map(|p| p.as_str().to_string())
                .unwrap_or_default(),
            endpoint: a.api_endpoint().unwrap_or_default().to_string(),
            description: a.description().unwrap_or_default().to_string(),
            created: a.created_date().map(|d| fmt_epoch_secs(d.secs())),
            default_endpoint_disabled: a.disable_execute_api_endpoint().unwrap_or(false),
            route_selection_expression: a
                .route_selection_expression()
                .unwrap_or_default()
                .to_string(),
            cors: a.cors_configuration().map(|c| ApiCors {
                allow_origins: c.allow_origins().to_vec(),
                allow_methods: c.allow_methods().to_vec(),
                allow_headers: c.allow_headers().to_vec(),
                expose_headers: c.expose_headers().to_vec(),
                allow_credentials: c.allow_credentials().unwrap_or(false),
                max_age: c.max_age(),
            }),
            tags: a.tags().cloned().unwrap_or_default(),
        }
    }
}

crate::sections! {
    pub enum HttpApiDetailSection,
    pub static HTTP_API_SECTIONS = [
        Overview "Overview",
        Routes "Routes" => crate::app::App::trigger_http_api_details_load,
        Stages "Stages" => crate::app::App::trigger_http_api_details_load,
        Authorizers "Authorizers" => crate::app::App::trigger_http_api_details_load,
        Tags "Tags",
    ]
}

impl Resource for HttpApi {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&HTTP_API_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.id
    }
    fn name(&self) -> &str {
        if self.name.is_empty() {
            &self.id
        } else {
            &self.name
        }
    }
    fn resource_type(&self) -> &str {
        "HTTP API"
    }
    fn state(&self) -> ResourceState {
        ResourceState::stateless()
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        format!(
            "{} {} {} {} {}",
            self.id, self.name, self.protocol, self.description, self.endpoint
        )
    }
    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("ID".to_string(), self.id.clone()),
            ("Name".to_string(), self.name.clone()),
            ("Protocol".to_string(), self.protocol.clone()),
            ("Endpoint".to_string(), self.endpoint.clone()),
            (
                "Created".to_string(),
                self.created.clone().unwrap_or_default(),
            ),
        ]
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{region}.console.aws.amazon.com/apigateway/main/api-detail?api={}&region={region}",
            self.id
        ))
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── ApiCustomDomain (v1 + v2) ─────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ApiCustomDomain {
    pub domain_name: String,
    pub endpoint_type: String, // EDGE / REGIONAL
    pub certificate_arn: String,
    pub security_policy: String, // TLS_1_0 / TLS_1_2
    pub status: String,          // AVAILABLE / UPDATING / PENDING
    pub target: String,          // the CloudFront/regional target hostname
    pub serves: String,          // "REST" / "HTTP" / "REST, HTTP"
    /// Mutual-TLS truststore ("s3://bucket/key (version)"), when configured.
    pub mtls_truststore: Option<String>,
    pub tags: HashMap<String, String>,
}

/// "s3://bucket/truststore.pem (abc123)" from a truststore uri + version.
fn fmt_truststore(uri: Option<&str>, version: Option<&str>) -> Option<String> {
    let uri = uri.filter(|s| !s.is_empty())?;
    Some(match version.filter(|s| !s.is_empty()) {
        Some(v) => format!("{} ({})", uri, v),
        None => uri.to_string(),
    })
}

impl ApiCustomDomain {
    pub fn from_v1(d: &aws_sdk_apigateway::types::DomainName) -> Self {
        let endpoint_type = d
            .endpoint_configuration()
            .map(|c| c.types().iter().map(|t| t.as_str()).collect::<Vec<_>>().join(", "))
            .unwrap_or_default();
        // Edge domains expose a distribution target; regional ones a regional target.
        let target = d
            .regional_domain_name()
            .filter(|s| !s.is_empty())
            .or_else(|| d.distribution_domain_name().filter(|s| !s.is_empty()))
            .unwrap_or_default()
            .to_string();
        let certificate_arn = d
            .certificate_arn()
            .filter(|s| !s.is_empty())
            .or_else(|| d.regional_certificate_arn().filter(|s| !s.is_empty()))
            .unwrap_or_default()
            .to_string();
        Self {
            domain_name: d.domain_name().unwrap_or_default().to_string(),
            endpoint_type,
            certificate_arn,
            security_policy: d
                .security_policy()
                .map(|s| s.as_str().to_string())
                .unwrap_or_default(),
            status: d
                .domain_name_status()
                .map(|s| s.as_str().to_string())
                .unwrap_or_default(),
            target,
            serves: "REST".to_string(),
            mtls_truststore: d.mutual_tls_authentication().and_then(|m| {
                fmt_truststore(m.truststore_uri(), m.truststore_version())
            }),
            tags: d.tags().cloned().unwrap_or_default(),
        }
    }

    pub fn from_v2(d: &aws_sdk_apigatewayv2::types::DomainName) -> Self {
        let cfg = d.domain_name_configurations().first();
        Self {
            domain_name: d.domain_name().unwrap_or_default().to_string(),
            endpoint_type: cfg
                .and_then(|c| c.endpoint_type())
                .map(|t| t.as_str().to_string())
                .unwrap_or_default(),
            certificate_arn: cfg
                .and_then(|c| c.certificate_arn())
                .unwrap_or_default()
                .to_string(),
            security_policy: cfg
                .and_then(|c| c.security_policy())
                .map(|s| s.as_str().to_string())
                .unwrap_or_default(),
            status: cfg
                .and_then(|c| c.domain_name_status())
                .map(|s| s.as_str().to_string())
                .unwrap_or_default(),
            target: cfg
                .and_then(|c| c.api_gateway_domain_name())
                .unwrap_or_default()
                .to_string(),
            serves: "HTTP".to_string(),
            mtls_truststore: d.mutual_tls_authentication().and_then(|m| {
                fmt_truststore(m.truststore_uri(), m.truststore_version())
            }),
            tags: d.tags().cloned().unwrap_or_default(),
        }
    }
}

crate::sections! {
    pub enum ApiDomainDetailSection,
    pub static API_DOMAIN_SECTIONS = [
        Details "Details",
        Mappings "Mappings" => crate::app::App::trigger_api_domain_details_load,
        Tags "Tags",
    ]
}

impl Resource for ApiCustomDomain {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&API_DOMAIN_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.domain_name
    }
    fn name(&self) -> &str {
        &self.domain_name
    }
    fn resource_type(&self) -> &str {
        "API Custom Domain"
    }
    fn state(&self) -> ResourceState {
        match self.status.as_str() {
            "AVAILABLE" => ResourceState::Available,
            "UPDATING" | "PENDING" | "PENDING_CERTIFICATE_REIMPORT"
            | "PENDING_OWNERSHIP_VERIFICATION" => ResourceState::Pending,
            other => ResourceState::Unknown(other.to_string()),
        }
    }

    fn state_label(&self) -> String {
        native_state_label(&self.status, || self.state())
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        format!(
            "{} {} {} {}",
            self.domain_name, self.endpoint_type, self.serves, self.target
        )
    }
    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Domain".to_string(), self.domain_name.clone()),
            ("Endpoint Type".to_string(), self.endpoint_type.clone()),
            ("Status".to_string(), self.status.clone()),
            ("Serves".to_string(), self.serves.clone()),
            ("Target".to_string(), self.target.clone()),
        ]
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{region}.console.aws.amazon.com/apigateway/main/publish/domain-names?domain={}&region={region}",
            self.domain_name
        ))
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── Shared child types ────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct RestResource {
    pub path: String,
    /// Method name + resolved backend (when integration lookup succeeded).
    pub methods: Vec<RestMethod>,
}

#[derive(Debug, Clone)]
pub struct RestMethod {
    pub http_method: String,
    /// Jump-ready backend: a clean Lambda ARN, an HTTP URL, or a short
    /// description ("MOCK", "AWS: …"). Empty when not resolved.
    pub backend: String,
}

#[derive(Debug, Clone)]
pub struct HttpRoute {
    pub route_key: String, // "GET /items"
    pub target: String,    // raw "integrations/<id>" (kept for reference)
    /// Jump-ready backend resolved from the route's integration (clean Lambda
    /// ARN / HTTP URL / description). Empty when unresolved.
    pub backend: String,
    pub authorization: String,
}

/// Extract the bare Lambda function ARN embedded in an API Gateway integration
/// URI (`arn:aws:apigateway:…:lambda:path/2015-03-31/functions/<LAMBDA-ARN>/invocations`).
/// Returns the clean `arn:aws:lambda:…:function:NAME` so the row jumps to Lambda.
fn extract_lambda_arn(uri: &str) -> Option<String> {
    let start = uri.find("arn:aws:lambda:")?;
    let rest = &uri[start..];
    let end = rest.find("/invocations").unwrap_or(rest.len());
    Some(rest[..end].to_string())
}

/// Turn an integration (type + uri) into a jump-ready backend string.
fn humanize_integration(integration_type: &str, uri: &str) -> String {
    if let Some(lambda) = extract_lambda_arn(uri) {
        return lambda;
    }
    match integration_type {
        "HTTP" | "HTTP_PROXY" => uri.to_string(),
        "MOCK" => "MOCK".to_string(),
        "" => uri.to_string(),
        other => {
            if uri.is_empty() {
                other.to_string()
            } else {
                format!("{}: {}", other, uri)
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct ApiStage {
    pub name: String,
    pub deployment: String,
    pub info: String, // cache/auto-deploy summary
    /// CloudWatch Logs destination ARN for access logs (drives `t` tail). Empty
    /// when access logging isn't configured for the stage.
    pub access_log_destination: String,
    /// The access-log format string (empty when logging is off).
    pub access_log_format: String,
    pub throttle: String,           // "1000/s burst 2000" or ""
    pub waf_web_acl_arn: String,    // REST stages only
    /// Canary summary ("10% → deployment abc123"); REST stages only, empty
    /// when no canary is active.
    pub canary: String,
    pub variables: Vec<(String, String)>,
}

#[derive(Debug, Clone)]
pub struct ApiAuthorizer {
    pub name: String,
    pub kind: String, // REQUEST / JWT / TOKEN / COGNITO_USER_POOLS
    pub identity_source: String,
}

/// One deployment in a REST API's history (newest first, capped).
#[derive(Debug, Clone)]
pub struct ApiDeployment {
    pub id: String,
    pub description: String,
    pub created: String,
}

// ── Combined lazy details ─────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct RestApiDetails {
    pub resources: Vec<RestResource>,
    pub stages: Vec<ApiStage>,
    pub authorizers: Vec<ApiAuthorizer>,
    /// Newest-first, capped at `MAX_DEPLOYMENTS`; `deployments_total` keeps
    /// the uncapped count for the "showing latest N of M" row.
    pub deployments: Vec<ApiDeployment>,
    pub deployments_total: usize,
}

#[derive(Debug, Clone)]
pub struct HttpApiDetails {
    pub routes: Vec<HttpRoute>,
    pub stages: Vec<ApiStage>,
    pub authorizers: Vec<ApiAuthorizer>,
}

#[derive(Debug, Clone)]
pub struct DomainMapping {
    pub path: String,   // base path / api mapping key ("(none)" when empty)
    pub api_id: String,
    pub stage: String,
    pub kind: String, // REST / HTTP
}

// ── Lazy fetches ──────────────────────────────────────────────────────────────

pub async fn fetch_rest_api_details(client: V1Client, api_id: String) -> Result<RestApiDetails> {
    // Resources (paginated). We deliberately do NOT `embed("methods")`: that
    // pulls the full Method objects, whose request/response/integration maps the
    // API Gateway service can return with null values, which the SDK's dense-map
    // deserializer rejects ("dense map cannot contain null values"). We only need
    // the method names, which are the keys of `resourceMethods` without the embed.
    // First pass: collect (resource_id, path, method names). We avoid
    // `embed("methods")` (its null-valued maps crash the SDK deserializer), so
    // method *backends* are resolved in a bounded second pass below.
    struct RawResource {
        id: String,
        path: String,
        methods: Vec<String>,
    }
    let mut raw: Vec<RawResource> = Vec::new();
    let mut position: Option<String> = None;
    loop {
        let mut req = client.get_resources().rest_api_id(&api_id);
        if let Some(p) = &position {
            req = req.position(p);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
        for r in resp.items() {
            let mut methods: Vec<String> = r
                .resource_methods()
                .map(|m| m.keys().cloned().collect())
                .unwrap_or_default();
            methods.sort();
            raw.push(RawResource {
                id: r.id().unwrap_or_default().to_string(),
                path: r.path().unwrap_or_default().to_string(),
                methods,
            });
        }
        position = crate::aws::pagination::next_page_token(resp.position(), &position);
        if position.is_none() {
            break;
        }
    }
    raw.sort_by(|a, b| a.path.cmp(&b.path));

    // Second pass: resolve each method's integration backend, bounded and capped
    // (one GetIntegration per resource+method — large APIs can have hundreds, so
    // cap to keep the detail load snappy; methods past the cap show without a
    // backend rather than blocking).
    // NOTE: API Gateway management plane has a low rate limit (~10 rps shared
    // across all management API calls per account/region). We keep concurrency
    // low to avoid TooManyRequestsException.
    use futures::stream::{self, StreamExt};
    const MAX_INTEGRATION_LOOKUPS: usize = 120;
    let mut lookups: Vec<(String, String)> = Vec::new(); // (resource_id, method)
    for r in &raw {
        for m in &r.methods {
            if lookups.len() >= MAX_INTEGRATION_LOOKUPS {
                break;
            }
            lookups.push((r.id.clone(), m.clone()));
        }
    }
    let backends: HashMap<(String, String), String> = stream::iter(lookups)
        .map(|(rid, method)| {
            let client = client.clone();
            let api_id = api_id.clone();
            async move {
                let resp = client
                    .get_integration()
                    .rest_api_id(&api_id)
                    .resource_id(&rid)
                    .http_method(&method)
                    .send()
                    .await
                    .ok()?;
                let itype = resp.r#type().map(|t| t.as_str()).unwrap_or("");
                let uri = resp.uri().unwrap_or_default();
                Some(((rid, method), humanize_integration(itype, uri)))
            }
        })
        .buffer_unordered(3)
        .filter_map(|x| async move { x })
        .collect()
        .await;

    let resources: Vec<RestResource> = raw
        .into_iter()
        .map(|r| {
            let methods = r
                .methods
                .iter()
                .map(|m| RestMethod {
                    http_method: m.clone(),
                    backend: backends.get(&(r.id.clone(), m.clone())).cloned().unwrap_or_default(),
                })
                .collect();
            RestResource {
                path: r.path,
                methods,
            }
        })
        .collect();

    // Stages (no pagination).
    let stages_resp = client
        .get_stages()
        .rest_api_id(&api_id)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
    let stages = stages_resp
        .item()
        .iter()
        .map(|s| {
            let cache = if s.cache_cluster_enabled() {
                "cache: enabled"
            } else {
                "cache: disabled"
            };
            // Stage-default throttle lives under the "*/*" method setting.
            let throttle = s
                .method_settings()
                .and_then(|ms| ms.get("*/*"))
                .and_then(|m| {
                    let rate = m.throttling_rate_limit();
                    let burst = m.throttling_burst_limit();
                    (rate > 0.0 || burst > 0)
                        .then(|| format!("{:.0}/s burst {}", rate, burst))
                })
                .unwrap_or_default();
            let mut variables: Vec<(String, String)> = s
                .variables()
                .map(|v| v.iter().map(|(k, val)| (k.clone(), val.clone())).collect())
                .unwrap_or_default();
            variables.sort_by(|a, b| a.0.cmp(&b.0));
            // Active canary: a percentage of traffic pinned to another deployment.
            let canary = s
                .canary_settings()
                .map(|c| {
                    let pct = c.percent_traffic();
                    let dep = c.deployment_id().unwrap_or_default();
                    if dep.is_empty() {
                        format!("{}% canary", pct)
                    } else {
                        format!("{}% → deployment {}", pct, dep)
                    }
                })
                .filter(|_| s.canary_settings().map(|c| c.percent_traffic() > 0.0).unwrap_or(false))
                .unwrap_or_default();
            ApiStage {
                name: s.stage_name().unwrap_or_default().to_string(),
                deployment: s.deployment_id().unwrap_or_default().to_string(),
                info: cache.to_string(),
                access_log_destination: s
                    .access_log_settings()
                    .and_then(|a| a.destination_arn())
                    .unwrap_or_default()
                    .to_string(),
                access_log_format: s
                    .access_log_settings()
                    .and_then(|a| a.format())
                    .unwrap_or_default()
                    .to_string(),
                throttle,
                waf_web_acl_arn: s.web_acl_arn().unwrap_or_default().to_string(),
                canary,
                variables,
            }
        })
        .collect();

    // Authorizers (paginated).
    let mut authorizers = Vec::new();
    let mut position: Option<String> = None;
    loop {
        let mut req = client.get_authorizers().rest_api_id(&api_id);
        if let Some(p) = &position {
            req = req.position(p);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
        for a in resp.items() {
            authorizers.push(ApiAuthorizer {
                name: a.name().unwrap_or_default().to_string(),
                kind: a
                    .r#type()
                    .map(|t| t.as_str().to_string())
                    .unwrap_or_default(),
                identity_source: a.identity_source().unwrap_or_default().to_string(),
            });
        }
        position = crate::aws::pagination::next_page_token(resp.position(), &position);
        if position.is_none() {
            break;
        }
    }

    // Deployments (paginated). History can be long; keep the newest few.
    const MAX_DEPLOYMENTS: usize = 25;
    let mut raw_deployments: Vec<(i64, ApiDeployment)> = Vec::new();
    let mut position: Option<String> = None;
    loop {
        let mut req = client.get_deployments().rest_api_id(&api_id);
        if let Some(p) = &position {
            req = req.position(p);
        }
        let resp = match req.send().await {
            Ok(r) => r,
            Err(_) => break, // best-effort; the section shows what it has
        };
        for d in resp.items() {
            let epoch = d.created_date().map(|t| t.secs()).unwrap_or(0);
            raw_deployments.push((
                epoch,
                ApiDeployment {
                    id: d.id().unwrap_or_default().to_string(),
                    description: d.description().unwrap_or_default().to_string(),
                    created: d.created_date().map(|t| fmt_epoch_secs(t.secs())).unwrap_or_default(),
                },
            ));
        }
        position = crate::aws::pagination::next_page_token(resp.position(), &position);
        if position.is_none() {
            break;
        }
    }
    raw_deployments.sort_by_key(|(epoch, _)| std::cmp::Reverse(*epoch));
    let deployments_total = raw_deployments.len();
    let deployments: Vec<ApiDeployment> = raw_deployments
        .into_iter()
        .take(MAX_DEPLOYMENTS)
        .map(|(_, d)| d)
        .collect();

    Ok(RestApiDetails {
        resources,
        stages,
        authorizers,
        deployments,
        deployments_total,
    })
}

pub async fn fetch_http_api_details(client: V2Client, api_id: String) -> Result<HttpApiDetails> {
    // Integrations first (one paginated call) → map integration id → backend, so
    // each route's `integrations/<id>` target resolves to a real Lambda/URL.
    let mut integrations: HashMap<String, String> = HashMap::new();
    let mut next: Option<String> = None;
    loop {
        let mut req = client.get_integrations().api_id(&api_id);
        if let Some(t) = &next {
            req = req.next_token(t);
        }
        let resp = match req.send().await {
            Ok(r) => r,
            Err(_) => break, // integrations are best-effort; routes still render
        };
        for i in resp.items() {
            if let Some(id) = i.integration_id() {
                let itype = i.integration_type().map(|t| t.as_str()).unwrap_or("");
                let uri = i.integration_uri().unwrap_or_default();
                integrations.insert(id.to_string(), humanize_integration(itype, uri));
            }
        }
        next = crate::aws::pagination::next_page_token(resp.next_token(), &next);
        if next.is_none() {
            break;
        }
    }

    // Routes (paginated).
    let mut routes = Vec::new();
    let mut next: Option<String> = None;
    loop {
        let mut req = client.get_routes().api_id(&api_id);
        if let Some(t) = &next {
            req = req.next_token(t);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
        for r in resp.items() {
            let target = r.target().unwrap_or_default().to_string();
            // target = "integrations/<id>" → resolved backend.
            let backend = target
                .strip_prefix("integrations/")
                .and_then(|id| integrations.get(id).cloned())
                .unwrap_or_default();
            routes.push(HttpRoute {
                route_key: r.route_key().unwrap_or_default().to_string(),
                target,
                backend,
                authorization: r
                    .authorization_type()
                    .map(|a| a.as_str().to_string())
                    .unwrap_or_default(),
            });
        }
        next = crate::aws::pagination::next_page_token(resp.next_token(), &next);
        if next.is_none() {
            break;
        }
    }
    routes.sort_by(|a, b| a.route_key.cmp(&b.route_key));

    // Stages (paginated).
    let mut stages = Vec::new();
    let mut next: Option<String> = None;
    loop {
        let mut req = client.get_stages().api_id(&api_id);
        if let Some(t) = &next {
            req = req.next_token(t);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
        for s in resp.items() {
            let auto = if s.auto_deploy().unwrap_or(false) {
                "auto-deploy: on"
            } else {
                "auto-deploy: off"
            };
            let throttle = s
                .default_route_settings()
                .and_then(|d| {
                    let rate = d.throttling_rate_limit();
                    let burst = d.throttling_burst_limit();
                    match (rate, burst) {
                        (Some(r), Some(b)) => Some(format!("{:.0}/s burst {}", r, b)),
                        _ => None,
                    }
                })
                .unwrap_or_default();
            let mut variables: Vec<(String, String)> = s
                .stage_variables()
                .map(|v| v.iter().map(|(k, val)| (k.clone(), val.clone())).collect())
                .unwrap_or_default();
            variables.sort_by(|a, b| a.0.cmp(&b.0));
            stages.push(ApiStage {
                name: s.stage_name().unwrap_or_default().to_string(),
                deployment: s.deployment_id().unwrap_or_default().to_string(),
                info: auto.to_string(),
                access_log_destination: s
                    .access_log_settings()
                    .and_then(|a| a.destination_arn())
                    .unwrap_or_default()
                    .to_string(),
                access_log_format: s
                    .access_log_settings()
                    .and_then(|a| a.format())
                    .unwrap_or_default()
                    .to_string(),
                throttle,
                waf_web_acl_arn: String::new(),
                canary: String::new(),
                variables,
            });
        }
        next = crate::aws::pagination::next_page_token(resp.next_token(), &next);
        if next.is_none() {
            break;
        }
    }

    // Authorizers (paginated).
    let mut authorizers = Vec::new();
    let mut next: Option<String> = None;
    loop {
        let mut req = client.get_authorizers().api_id(&api_id);
        if let Some(t) = &next {
            req = req.next_token(t);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
        for a in resp.items() {
            authorizers.push(ApiAuthorizer {
                name: a.name().unwrap_or_default().to_string(),
                kind: a
                    .authorizer_type()
                    .map(|t| t.as_str().to_string())
                    .unwrap_or_default(),
                identity_source: a.identity_source().join(", "),
            });
        }
        next = crate::aws::pagination::next_page_token(resp.next_token(), &next);
        if next.is_none() {
            break;
        }
    }

    Ok(HttpApiDetails {
        routes,
        stages,
        authorizers,
    })
}

/// Fetch a custom domain's mappings from both control planes (v1 base-path
/// mappings + v2 API mappings); a domain only on one side just yields nothing
/// from the other, so per-side errors are tolerated.
pub async fn fetch_api_domain_details(
    v1: V1Client,
    v2: V2Client,
    domain_name: String,
) -> Result<Vec<DomainMapping>> {
    let dn = domain_name.clone();
    let (r1, r2) = tokio::join!(
        async move {
            let mut out = Vec::new();
            let mut position: Option<String> = None;
            loop {
                let mut req = v1.get_base_path_mappings().domain_name(&dn);
                if let Some(p) = &position {
                    req = req.position(p);
                }
                let resp = req.send().await.ok()?;
                for m in resp.items() {
                    let path = m.base_path().unwrap_or_default();
                    out.push(DomainMapping {
                        path: if path.is_empty() || path == "(none)" {
                            "(none)".to_string()
                        } else {
                            path.to_string()
                        },
                        api_id: m.rest_api_id().unwrap_or_default().to_string(),
                        stage: m.stage().unwrap_or_default().to_string(),
                        kind: "REST".to_string(),
                    });
                }
                position = crate::aws::pagination::next_page_token(resp.position(), &position);
                if position.is_none() {
                    break;
                }
            }
            Some(out)
        },
        async move {
            let mut out = Vec::new();
            let mut next: Option<String> = None;
            loop {
                let mut req = v2.get_api_mappings().domain_name(&domain_name);
                if let Some(t) = &next {
                    req = req.next_token(t);
                }
                let resp = req.send().await.ok()?;
                for m in resp.items() {
                    let key = m.api_mapping_key().unwrap_or_default();
                    out.push(DomainMapping {
                        path: if key.is_empty() {
                            "(none)".to_string()
                        } else {
                            key.to_string()
                        },
                        api_id: m.api_id().unwrap_or_default().to_string(),
                        stage: m.stage().unwrap_or_default().to_string(),
                        kind: "HTTP".to_string(),
                    });
                }
                next = crate::aws::pagination::next_page_token(resp.next_token(), &next);
                if next.is_none() {
                    break;
                }
            }
            Some(out)
        },
    );

    let mut mappings = r1.unwrap_or_default();
    mappings.extend(r2.unwrap_or_default());
    mappings.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(mappings)
}

// ── ApiUsagePlan (v1 / REST only) ─────────────────────────────────────────────

/// A usage plan — the throttle/quota envelope that API keys buy into. REST
/// (v1) only; HTTP APIs have no usage plans. Everything but the key list comes
/// inline from GetUsagePlans.
#[derive(Debug, Clone)]
pub struct ApiUsagePlan {
    pub id: String,
    pub name: String,
    pub description: String,
    pub throttle_rate: Option<f64>,
    pub throttle_burst: Option<i32>,
    /// (limit, period) e.g. (10000, "MONTH"); None = no quota.
    pub quota: Option<(i32, String)>,
    /// (api_id, stage) pairs this plan covers.
    pub api_stages: Vec<(String, String)>,
    pub tags: HashMap<String, String>,
}

impl ApiUsagePlan {
    pub fn from_sdk(p: &aws_sdk_apigateway::types::UsagePlan) -> Self {
        let id = p.id().unwrap_or_default().to_string();
        let name = p.name().unwrap_or_default().to_string();
        Self {
            name: if name.is_empty() { id.clone() } else { name },
            id,
            description: p.description().unwrap_or_default().to_string(),
            throttle_rate: p.throttle().map(|t| t.rate_limit()).filter(|r| *r > 0.0),
            throttle_burst: p.throttle().map(|t| t.burst_limit()).filter(|b| *b > 0),
            quota: p
                .quota()
                .map(|q| {
                    (
                        q.limit(),
                        q.period().map(|x| x.as_str().to_string()).unwrap_or_default(),
                    )
                })
                .filter(|(l, _)| *l > 0),
            api_stages: p
                .api_stages()
                .iter()
                .map(|s| {
                    (
                        s.api_id().unwrap_or_default().to_string(),
                        s.stage().unwrap_or_default().to_string(),
                    )
                })
                .collect(),
            tags: p.tags().cloned().unwrap_or_default(),
        }
    }
}

crate::sections! {
    pub enum ApiUsagePlanDetailSection,
    pub static API_USAGE_PLAN_SECTIONS = [
        Overview "Overview",
        Stages "API Stages",
        Keys "Keys" => crate::app::App::trigger_usage_plan_keys_load,
    ]
}

impl Resource for ApiUsagePlan {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&API_USAGE_PLAN_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.id
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn resource_type(&self) -> &str {
        "API Usage Plan"
    }
    fn state(&self) -> ResourceState {
        ResourceState::stateless()
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        format!("{} {} {}", self.id, self.name, self.description)
    }
    fn details(&self) -> Vec<(String, String)> {
        let mut d = vec![
            ("ID".to_string(), self.id.clone()),
            ("Name".to_string(), self.name.clone()),
        ];
        if let Some(rate) = self.throttle_rate {
            d.push(("Throttle".to_string(), format!("{:.0}/s", rate)));
        }
        if let Some((limit, period)) = &self.quota {
            d.push(("Quota".to_string(), format!("{} / {}", limit, period)));
        }
        d.push(("API Stages".to_string(), self.api_stages.len().to_string()));
        d
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{region}.console.aws.amazon.com/apigateway/main/usage-plans?region={region}&usagePlan={}",
            self.id
        ))
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

async fn fetch_usage_plans(client: V1Client) -> Result<Vec<ApiUsagePlan>> {
    let mut out = Vec::new();
    let mut position: Option<String> = None;
    loop {
        let mut req = client.get_usage_plans();
        if let Some(p) = &position {
            req = req.position(p);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
        for p in resp.items() {
            out.push(ApiUsagePlan::from_sdk(p));
        }
        position = crate::aws::pagination::next_page_token(resp.position(), &position);
        if position.is_none() {
            break;
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// One API key attached to a usage plan. The API returns the key **value**
/// too — deliberately never captured (read-only secrets rule). The `type`
/// field is omitted: it's always API_KEY.
#[derive(Debug, Clone)]
pub struct UsagePlanKeyInfo {
    pub id: String,
    pub name: String,
}


pub async fn fetch_usage_plan_keys(
    client: V1Client,
    plan_id: String,
) -> Result<Vec<UsagePlanKeyInfo>> {
    let mut out = Vec::new();
    let mut position: Option<String> = None;
    loop {
        let mut req = client.get_usage_plan_keys().usage_plan_id(&plan_id);
        if let Some(p) = &position {
            req = req.position(p);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
        for k in resp.items() {
            out.push(UsagePlanKeyInfo {
                id: k.id().unwrap_or_default().to_string(),
                name: k.name().unwrap_or_default().to_string(),
            });
        }
        position = crate::aws::pagination::next_page_token(resp.position(), &position);
        if position.is_none() {
            break;
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

// ── ApiVpcLink (v1 + v2) ──────────────────────────────────────────────────────

/// A VPC link from either control plane. v1 links target NLB ARNs (REST
/// private integrations); v2 links pin subnets + security groups (HTTP APIs).
/// The id spaces are distinct, so the merged list needs no dedup.
#[derive(Debug, Clone)]
pub struct ApiVpcLink {
    pub id: String,
    pub name: String,
    pub kind: String, // REST / HTTP
    pub status: String,
    pub status_message: String,
    pub target_arns: Vec<String>,     // v1
    pub subnet_ids: Vec<String>,      // v2
    pub security_group_ids: Vec<String>, // v2
    pub created: Option<String>,      // v2
    pub tags: HashMap<String, String>,
}

impl ApiVpcLink {
    pub fn from_v1(l: &aws_sdk_apigateway::types::VpcLink) -> Self {
        Self {
            id: l.id().unwrap_or_default().to_string(),
            name: l.name().unwrap_or_default().to_string(),
            kind: "REST".to_string(),
            status: l
                .status()
                .map(|s| s.as_str().to_string())
                .unwrap_or_default(),
            status_message: l.status_message().unwrap_or_default().to_string(),
            target_arns: l.target_arns().to_vec(),
            subnet_ids: Vec::new(),
            security_group_ids: Vec::new(),
            created: None,
            tags: l.tags().cloned().unwrap_or_default(),
        }
    }

    pub fn from_v2(l: &aws_sdk_apigatewayv2::types::VpcLink) -> Self {
        Self {
            id: l.vpc_link_id().unwrap_or_default().to_string(),
            name: l.name().unwrap_or_default().to_string(),
            kind: "HTTP".to_string(),
            status: l
                .vpc_link_status()
                .map(|s| s.as_str().to_string())
                .unwrap_or_default(),
            status_message: l.vpc_link_status_message().unwrap_or_default().to_string(),
            target_arns: Vec::new(),
            subnet_ids: l.subnet_ids().to_vec(),
            security_group_ids: l.security_group_ids().to_vec(),
            created: l.created_date().map(|d| fmt_epoch_secs(d.secs())),
            tags: l.tags().cloned().unwrap_or_default(),
        }
    }
}

impl Resource for ApiVpcLink {
    fn security_group_ids(&self) -> Vec<String> {
        self.security_group_ids.clone()
    }

    fn id(&self) -> &str {
        &self.id
    }
    fn name(&self) -> &str {
        if self.name.is_empty() {
            &self.id
        } else {
            &self.name
        }
    }
    fn resource_type(&self) -> &str {
        "API VPC Link"
    }
    fn state(&self) -> ResourceState {
        match self.status.as_str() {
            "AVAILABLE" => ResourceState::Available,
            "FAILED" | "INACTIVE" => ResourceState::Unavailable,
            "PENDING" => ResourceState::Pending,
            "DELETING" => ResourceState::Deleting,
            other => ResourceState::Unknown(other.to_string()),
        }
    }

    fn state_label(&self) -> String {
        native_state_label(&self.status, || self.state())
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        format!("{} {} {} {}", self.id, self.name, self.kind, self.status)
    }
    fn details(&self) -> Vec<(String, String)> {
        let mut d = vec![
            ("ID".to_string(), self.id.clone()),
            ("Name".to_string(), self.name.clone()),
            ("Serves".to_string(), format!("{} APIs", self.kind)),
            ("Status".to_string(), self.status.clone()),
        ];
        if !self.status_message.is_empty() {
            d.push(("Status Message".to_string(), self.status_message.clone()));
        }
        if let Some(created) = &self.created {
            d.push(("Created".to_string(), created.clone()));
        }
        // Value rows are jump-ready: NLB ARNs → ELB, subnet-/sg- ids → VPC/EC2.
        for arn in &self.target_arns {
            d.push(("Target NLB".to_string(), arn.clone()));
        }
        for s in &self.subnet_ids {
            d.push(("Subnet".to_string(), s.clone()));
        }
        for sg in &self.security_group_ids {
            d.push(("Security Group".to_string(), sg.clone()));
        }
        d
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{region}.console.aws.amazon.com/apigateway/main/vpc-links/list?region={region}"
        ))
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

async fn fetch_vpc_links_v1(client: V1Client) -> Result<Vec<ApiVpcLink>> {
    let mut out = Vec::new();
    let mut position: Option<String> = None;
    loop {
        let mut req = client.get_vpc_links();
        if let Some(p) = &position {
            req = req.position(p);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
        for l in resp.items() {
            out.push(ApiVpcLink::from_v1(l));
        }
        position = crate::aws::pagination::next_page_token(resp.position(), &position);
        if position.is_none() {
            break;
        }
    }
    Ok(out)
}

async fn fetch_vpc_links_v2(client: V2Client) -> Result<Vec<ApiVpcLink>> {
    let mut out = Vec::new();
    let mut next: Option<String> = None;
    loop {
        let mut req = client.get_vpc_links();
        if let Some(t) = &next {
            req = req.next_token(t);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
        for l in resp.items() {
            out.push(ApiVpcLink::from_v2(l));
        }
        next = crate::aws::pagination::next_page_token(resp.next_token(), &next);
        if next.is_none() {
            break;
        }
    }
    Ok(out)
}

// ── Metrics (AWS/ApiGateway) ──────────────────────────────────────────────────

pub use crate::aws::services::ec2::MetricsTimeRange;

/// Which chart set an API publishes. REST, HTTP, and WebSocket APIs share the
/// `AWS/ApiGateway` namespace but use different metric names: REST has
/// `4XXError`/`5XXError` + cache metrics, HTTP has lowercase `4xx`/`5xx` +
/// `DataProcessed`, WebSocket has connect/message counts and per-kind errors.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ApiMetricsFlavor {
    Rest,
    Http,
    WebSocket,
}

#[derive(Debug, Clone)]
pub struct ApiMetricsData {
    pub time_range: MetricsTimeRange,
    pub flavor: ApiMetricsFlavor,
    /// Requests (REST/HTTP `Count`) or WebSocket `ConnectCount`.
    pub count: Vec<(f64, f64)>,
    /// `4XXError` / `4xx` / WebSocket `ClientError`.
    pub error_4xx: Vec<(f64, f64)>,
    /// `5XXError` / `5xx` / WebSocket `ExecutionError`.
    pub error_5xx: Vec<(f64, f64)>,
    pub latency: Vec<(f64, f64)>,      // Latency ms (Average; empty for WS)
    pub integration: Vec<(f64, f64)>,  // IntegrationLatency ms (Average)
    pub cache_hit: Vec<(f64, f64)>,    // REST only
    pub cache_miss: Vec<(f64, f64)>,   // REST only
    pub data_processed: Vec<(f64, f64)>, // HTTP only (bytes)
    pub message_count: Vec<(f64, f64)>,  // WebSocket only
    pub integration_errors: Vec<(f64, f64)>, // WebSocket only
    pub x_max: f64,
}

#[derive(Debug, Clone)]
pub enum ApiMetricsState {
    Loading,
    Loaded(Box<ApiMetricsData>),
}

/// Fetch the `AWS/ApiGateway` dashboard metrics. REST APIs key on the `ApiName`
/// dimension, HTTP + WebSocket APIs on `ApiId`; the flavor picks the metric
/// name set (see `ApiMetricsFlavor`).
pub async fn fetch_api_metrics(
    cw_client: aws_sdk_cloudwatch::Client,
    dimension_name: String,
    dimension_value: String,
    flavor: ApiMetricsFlavor,
    time_range: MetricsTimeRange,
) -> Result<ApiMetricsData> {
    use aws_sdk_cloudwatch::types::{Dimension, Statistic};

    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let start_secs = now_secs - time_range.duration_secs();
    let period = time_range.period_secs();
    let start_dt = aws_sdk_cloudwatch::primitives::DateTime::from_secs(start_secs);
    let end_dt = aws_sdk_cloudwatch::primitives::DateTime::from_secs(now_secs);

    let make_dim = || {
        Dimension::builder()
            .name(&dimension_name)
            .value(&dimension_value)
            .build()
    };

    let metric = |name: &'static str, stat: Statistic| {
        cw_client
            .get_metric_statistics()
            .namespace("AWS/ApiGateway")
            .metric_name(name)
            .dimensions(make_dim())
            .start_time(start_dt)
            .end_time(end_dt)
            .period(period)
            .set_statistics(Some(vec![stat]))
            .send()
    };

    // Seven query slots regardless of flavor (the unused ones cost one empty
    // GetMetricStatistics each — cheaper than three bespoke join! blocks).
    let (count_name, e4_name, e5_name, extra_a, extra_b) = match flavor {
        ApiMetricsFlavor::Rest => ("Count", "4XXError", "5XXError", "CacheHitCount", "CacheMissCount"),
        ApiMetricsFlavor::Http => ("Count", "4xx", "5xx", "DataProcessed", "DataProcessed"),
        ApiMetricsFlavor::WebSocket => {
            ("ConnectCount", "ClientError", "ExecutionError", "MessageCount", "IntegrationError")
        }
    };
    let (count_r, e4_r, e5_r, lat_r, int_r, extra_a_r, extra_b_r) = tokio::join!(
        metric(count_name, Statistic::Sum),
        metric(e4_name, Statistic::Sum),
        metric(e5_name, Statistic::Sum),
        metric("Latency", Statistic::Average),
        metric("IntegrationLatency", Statistic::Average),
        metric(extra_a, Statistic::Sum),
        metric(extra_b, Statistic::Sum),
    );

    let parse_sum = |resp: std::result::Result<
        aws_sdk_cloudwatch::operation::get_metric_statistics::GetMetricStatisticsOutput,
        _,
    >| -> Vec<(f64, f64)> {
        let dps = match resp {
            Ok(r) => r.datapoints().to_vec(),
            Err(_) => vec![],
        };
        let mut pts: Vec<(f64, f64)> = dps
            .iter()
            .filter_map(|dp| Some((dp.timestamp()?.secs() as f64 - start_secs as f64, dp.sum()?)))
            .collect();
        pts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        pts
    };
    let parse_avg = |resp: std::result::Result<
        aws_sdk_cloudwatch::operation::get_metric_statistics::GetMetricStatisticsOutput,
        _,
    >| -> Vec<(f64, f64)> {
        let dps = match resp {
            Ok(r) => r.datapoints().to_vec(),
            Err(_) => vec![],
        };
        let mut pts: Vec<(f64, f64)> = dps
            .iter()
            .filter_map(|dp| {
                Some((dp.timestamp()?.secs() as f64 - start_secs as f64, dp.average()?))
            })
            .collect();
        pts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        pts
    };

    let extra_a_pts = parse_sum(extra_a_r);
    let extra_b_pts = parse_sum(extra_b_r);
    let (cache_hit, cache_miss, data_processed, message_count, integration_errors) = match flavor {
        ApiMetricsFlavor::Rest => (extra_a_pts, extra_b_pts, vec![], vec![], vec![]),
        ApiMetricsFlavor::Http => (vec![], vec![], extra_a_pts, vec![], vec![]),
        ApiMetricsFlavor::WebSocket => (vec![], vec![], vec![], extra_a_pts, extra_b_pts),
    };

    Ok(ApiMetricsData {
        time_range,
        flavor,
        count: parse_sum(count_r),
        error_4xx: parse_sum(e4_r),
        error_5xx: parse_sum(e5_r),
        latency: parse_avg(lat_r),
        integration: parse_avg(int_r),
        cache_hit,
        cache_miss,
        data_processed,
        message_count,
        integration_errors,
        x_max: time_range.duration_secs() as f64,
    })
}

// ── Timestamp helpers ─────────────────────────────────────────────────────────

fn fmt_epoch_secs(secs: i64) -> String {
    let days = secs / 86400;
    let rem = secs % 86400;
    let (h, mi) = (rem / 3600, (rem % 3600) / 60);
    let (y, mo, d) = epoch_days_to_ymd(days);
    format!("{:04}-{:02}-{:02} {:02}:{:02} UTC", y, mo, d, h, mi)
}

fn epoch_days_to_ymd(mut days: i64) -> (i32, u8, u8) {
    let mut year = 1970i32;
    loop {
        let dy = if is_leap(year) { 366 } else { 365 };
        if days < dy {
            break;
        }
        days -= dy;
        year += 1;
    }
    let dm = [
        31u8,
        if is_leap(year) { 29 } else { 28 },
        31, 30, 31, 30, 31, 31, 30, 31, 30, 31,
    ];
    let mut month = 1u8;
    for &len in &dm {
        if days < len as i64 {
            break;
        }
        days -= len as i64;
        month += 1;
    }
    (year, month, (days + 1) as u8)
}

fn is_leap(y: i32) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}
