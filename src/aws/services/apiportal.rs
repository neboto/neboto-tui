use crate::aws::client::AwsClients;
use crate::aws::resource::{native_state_label, shell_quote, Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::error::Result;
use crate::event::{Event, LoadProgress};
use async_trait::async_trait;
use aws_sdk_apiportal::Client as ApiPortalClient;
use futures::stream::StreamExt;
use std::any::Any;
use std::collections::HashMap;
use tokio::sync::mpsc;

/// API Portal (`@apiportal`) — the APIs discovery found behind API Gateway,
/// load balancers and CloudFront, and the managed APIs, portals and products
/// built on them. Five sub-tabs: **Inventory**, **Managed APIs**,
/// **Portals**, **Products**, **Discovery**. One region-scoped client.
///
/// Load order: discovery first (`GetDiscoveryConfiguration` +
/// `ListDiscoveryStatuses`, plus the organization configuration where the
/// caller may read it) so an empty inventory can say why; then the
/// inventory, streamed per page; then managed APIs (`ListManagedApis` +
/// `GetManagedApi` each, for the findings and tags); then portals and
/// products.
///
/// An inventory item's managed API is matched locally on its signature
/// (resource ARN, method, host, port, path) — `GetApiInventoryItem` adds
/// only that id, so it isn't called.
///
/// Lazy sections: a managed API's **Events** (`ListManagedApiEvents`) and a
/// product's **APIs** (`ListManagedApis` filtered by product).
///
/// Read-only: `StartDiscovery` / `StopDiscovery` / `Put*` / `Create*` and
/// the rest of the write surface are never called.
pub struct ApiPortalService {
    client: ApiPortalClient,
}

impl ApiPortalService {
    pub fn new(aws_clients: &AwsClients) -> Self {
        Self {
            client: aws_clients.apiportal_client(),
        }
    }
}

fn fmt_dt(dt: &aws_smithy_types::DateTime) -> String {
    crate::aws::services::cloudwatch::fmt_epoch_secs(dt.secs())
}

/// `GetManagedApi` fan-out.
const DESCRIBE_FANOUT: usize = 5;

/// Most inventory items one load keeps. Discovery folds hosts that differ in
/// one label (`<*>.example.com`), so a real inventory rarely gets near it.
pub const MAX_INVENTORY: usize = 5000;

/// Most events the Events section lists (newest first).
pub const MAX_EVENTS: usize = 100;

/// `MANAGED_ZOMBIE` → `managed zombie`, unknown values as sent.
pub fn enum_word(s: &str) -> String {
    s.to_lowercase().replace('_', " ")
}

/// `API_GATEWAY` → `API Gateway`; a source added later reads as sent.
pub fn source_label(s: &str) -> String {
    match s {
        "API_GATEWAY" => "API Gateway".to_string(),
        "ELB" => "Elastic Load Balancing".to_string(),
        "CLOUDFRONT" => "CloudFront".to_string(),
        other => other.to_string(),
    }
}

/// The one string both an inventory item and a managed API carry for the
/// same API: `GET api.example.com:443 /v1/users`.
pub fn signature(method: &str, host: &str, port: i32, path: &str) -> String {
    format!("{method} {host}:{port} {path}")
}

#[async_trait]
impl AwsService for ApiPortalService {
    fn service_type(&self) -> ServiceType {
        ServiceType::ApiPortal
    }

    fn name(&self) -> &str {
        "API Portal"
    }

    async fn list_resources(&self) -> Result<Vec<Box<dyn Resource>>> {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        self.list_resources_streaming(tx, ServiceType::ApiPortal).await?;
        Ok(vec![])
    }

    async fn list_resources_streaming(
        &self,
        event_tx: mpsc::UnboundedSender<Event>,
        service_type: ServiceType,
    ) -> Result<()> {
        let warn = |msg: String| {
            let _ = event_tx.send(Event::ResourceLoadWarning {
                service: service_type,
                warning: msg,
            });
        };
        let mut loaded = 0usize;
        let mut send = |batch: Vec<Box<dyn Resource>>, status: Option<&str>| {
            loaded += batch.len();
            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                service: service_type,
                resources: batch,
                progress: LoadProgress {
                    loaded_count: loaded,
                    total_count: None,
                    status_message: status.map(str::to_string),
                },
            });
        };
        let mut failures: Vec<String> = Vec::new();

        // ── Discovery ─────────────────────────────────────────────────────────
        // ResourceNotFound means discovery was never configured: a row that
        // says so, not a warning.
        let mut discovery = ApiDiscovery::account();
        match self.client.get_discovery_configuration().send().await {
            Ok(out) => {
                if let Some(c) = out.discovery_configuration() {
                    discovery.fill_account(c);
                }
                discovery.tags = out.tags().cloned().unwrap_or_default();
            }
            Err(e) => {
                let not_found = e
                    .as_service_error()
                    .is_some_and(|s| s.is_resource_not_found_exception());
                if !not_found {
                    let msg = crate::error::sdk_error_message(&e);
                    discovery.error = Some(msg.clone());
                    failures.push(msg);
                }
            }
        }
        let mut token: Option<String> = None;
        loop {
            match self.client.list_discovery_statuses().max_results(100).set_next_token(token.clone()).send().await {
                Ok(p) => {
                    discovery.statuses.extend(p.items().iter().map(DiscoveryStatus::from_sdk));
                    token = crate::aws::pagination::next_page_token(p.next_token(), &token);
                    if token.is_none() {
                        break;
                    }
                }
                Err(e) => {
                    // Already explained by the configuration's own error.
                    if discovery.error.is_none() && discovery.status.is_some() {
                        warn(format!("API Portal discovery statuses: {}", crate::error::sdk_error_message(&e)));
                    }
                    break;
                }
            }
        }
        let mut first: Vec<Box<dyn Resource>> = vec![Box::new(discovery)];
        // Only the management account or delegated administrator can read
        // the organization configuration: silent everywhere else.
        if let Ok(out) = self.client.get_organization_discovery_configuration().send().await {
            if let Some(c) = out.discovery_configuration().filter(|c| !c.arn().is_empty()) {
                let mut org = ApiDiscovery::organization(c);
                org.tags = out.tags().cloned().unwrap_or_default();
                first.push(Box::new(org));
            }
        }
        send(first, Some("Loading inventory…"));

        // ── Inventory, streamed per page ──────────────────────────────────────
        let mut items: Vec<ApiInventoryItem> = Vec::new();
        let mut token: Option<String> = None;
        let mut capped = false;
        loop {
            match self.client.list_api_inventory_items().max_results(100).set_next_token(token.clone()).send().await {
                Ok(p) => {
                    let page: Vec<ApiInventoryItem> = p.items().iter().map(ApiInventoryItem::from_sdk).collect();
                    let room = MAX_INVENTORY - items.len();
                    capped = page.len() > room;
                    items.extend(page.into_iter().take(room));
                    token = crate::aws::pagination::next_page_token(p.next_token(), &token);
                    if token.is_none() || items.len() >= MAX_INVENTORY {
                        capped |= token.is_some();
                        break;
                    }
                }
                Err(e) => {
                    let msg = crate::error::sdk_error_message(&e);
                    warn(format!("API Portal inventory: {msg}"));
                    failures.push(msg);
                    break;
                }
            }
        }
        if capped {
            warn(format!("API Portal inventory: showing the first {MAX_INVENTORY} items"));
        }

        // ── Managed APIs, each described for findings and tags ────────────────
        let mut summaries = Vec::new();
        let mut token: Option<String> = None;
        loop {
            match self.client.list_managed_apis().max_results(100).set_next_token(token.clone()).send().await {
                Ok(p) => {
                    summaries.extend(p.items().iter().cloned());
                    token = crate::aws::pagination::next_page_token(p.next_token(), &token);
                    if token.is_none() {
                        break;
                    }
                }
                Err(e) => {
                    let msg = crate::error::sdk_error_message(&e);
                    warn(format!("API Portal managed APIs: {msg}"));
                    failures.push(msg);
                    break;
                }
            }
        }
        let client = &self.client;
        let total = summaries.len();
        let described: Vec<(ManagedApi, Option<String>)> = futures::stream::iter(summaries)
            .map(|s| async move {
                match client.get_managed_api().managed_api_id(s.managed_api_id()).send().await {
                    Ok(out) => (ManagedApi::from_get(&out), None),
                    Err(e) => (ManagedApi::from_summary(&s), Some(crate::error::sdk_error_message(&e))),
                }
            })
            .buffered(DESCRIBE_FANOUT)
            .collect()
            .await;
        let errors: Vec<&String> = described.iter().filter_map(|(_, e)| e.as_ref()).collect();
        if let Some(first) = errors.first() {
            warn(format!("API Portal managed API details: {} of {total} failed ({first})", errors.len()));
        }
        let mut managed: Vec<ManagedApi> = described.into_iter().map(|(m, _)| m).collect();
        managed.sort_by(|a, b| a.signature.cmp(&b.signature));

        // Link each item to its managed API by signature.
        let by_sig: HashMap<(String, String), String> = managed
            .iter()
            .map(|m| ((m.resource_arn.clone(), m.signature.clone()), m.id.clone()))
            .collect();
        for i in &mut items {
            i.managed_api_id = by_sig.get(&(i.resource_arn.clone(), i.signature())).cloned();
        }
        for m in &mut managed {
            m.inventory_item = items
                .iter()
                .find(|i| i.resource_arn == m.resource_arn && i.signature() == m.signature)
                .map(|i| i.id.clone());
        }
        for chunk in items.chunks(500) {
            send(chunk.iter().cloned().map(|i| Box::new(i) as Box<dyn Resource>).collect(), None);
        }
        send(managed.into_iter().map(|m| Box::new(m) as Box<dyn Resource>).collect(), Some("Loading portals…"));

        // ── Portals and products ──────────────────────────────────────────────
        let mut portals: Vec<Box<dyn Resource>> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            match self.client.list_portals().max_results(100).set_next_token(token.clone()).send().await {
                Ok(p) => {
                    portals.extend(p.items().iter().map(|s| Box::new(ApiPortal::from_sdk(s)) as Box<dyn Resource>));
                    token = crate::aws::pagination::next_page_token(p.next_token(), &token);
                    if token.is_none() {
                        break;
                    }
                }
                Err(e) => {
                    let msg = crate::error::sdk_error_message(&e);
                    warn(format!("API Portal portals: {msg}"));
                    failures.push(msg);
                    break;
                }
            }
        }
        let mut products: Vec<Box<dyn Resource>> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            match self.client.list_products().max_results(100).set_next_token(token.clone()).send().await {
                Ok(p) => {
                    products.extend(p.items().iter().map(|s| Box::new(ApiProduct::from_sdk(s)) as Box<dyn Resource>));
                    token = crate::aws::pagination::next_page_token(p.next_token(), &token);
                    if token.is_none() {
                        break;
                    }
                }
                Err(e) => {
                    let msg = crate::error::sdk_error_message(&e);
                    warn(format!("API Portal products: {msg}"));
                    failures.push(msg);
                    break;
                }
            }
        }
        portals.extend(products);
        send(portals, None);

        // Every call failed: a missing apiportal: permission, or a region
        // without the service.
        if failures.len() >= 5 {
            let _ = event_tx.send(Event::ResourceLoadError {
                service: service_type,
                error: format!("API Portal unavailable: {}", failures[0]),
            });
            return Ok(());
        }

        let _ = event_tx.send(Event::ResourcesFullyLoaded {
            service: service_type,
            total_count: loaded,
        });
        Ok(())
    }

    async fn get_resource_details(&self, _id: &str) -> Result<Box<dyn Resource>> {
        Err(crate::error::Error::NotImplemented)
    }
}

// ── Inventory item ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct ApiInventoryItem {
    /// `<signature> <resource ARN>` — the signature alone repeats across
    /// resources that answer the same host and path.
    pub id: String,
    pub method: String,
    pub host: String,
    pub port: i32,
    pub path: String,
    pub account_id: String,
    pub resource_arn: String,
    pub region: String,
    /// `API_GATEWAY` / `ELB` / `CLOUDFRONT`, or a source added later.
    pub source: String,
    pub first_seen: Option<String>,
    pub last_seen: Option<String>,
    /// `PUBLIC` / `PRIVATE`.
    pub visibility: Option<String>,
    /// `ENABLED` / `DISABLED`.
    pub waf_status: Option<String>,
    /// `SHADOW` / `MANAGED` / `MANAGED_ZOMBIE` / `MANAGED_ORPHANED`.
    pub classification: Option<String>,
    /// `SELF` / `ORGANIZATION`; absent while `SHADOW`.
    pub managed_by: Option<String>,
    /// `LLM` / `MCP` / `A2A`, or a protocol added later.
    pub workloads: Vec<String>,
    /// The managed API with this signature, when this account loaded it.
    pub managed_api_id: Option<String>,
    pub tags: HashMap<String, String>,
}

impl ApiInventoryItem {
    pub fn from_sdk(i: &aws_sdk_apiportal::types::ApiInventoryItem) -> Self {
        let mut item = Self {
            method: i.method().as_str().to_string(),
            host: i.host().to_string(),
            port: i.port(),
            path: i.path().to_string(),
            account_id: i.account_id().to_string(),
            resource_arn: i.resource_arn().to_string(),
            region: i.region().to_string(),
            source: i.discovery_source().as_str().to_string(),
            first_seen: i.first_seen().map(fmt_dt),
            last_seen: i.last_seen().map(fmt_dt),
            visibility: i.visibility().map(|v| v.as_str().to_string()),
            waf_status: i.waf_status().map(|v| v.as_str().to_string()),
            classification: i.classification().map(|v| v.as_str().to_string()),
            managed_by: i.managed_by().map(|v| v.as_str().to_string()),
            workloads: i.workloads().iter().map(|w| w.as_str().to_string()).collect(),
            ..Default::default()
        };
        item.id = format!("{} {}", item.signature(), item.resource_arn);
        item
    }

    pub fn signature(&self) -> String {
        signature(&self.method, &self.host, self.port, &self.path)
    }

    pub fn is_public(&self) -> bool {
        self.visibility.as_deref() == Some("PUBLIC")
    }

    pub fn waf_off(&self) -> bool {
        self.waf_status.as_deref() == Some("DISABLED")
    }

    pub fn is_shadow(&self) -> bool {
        self.classification.as_deref() == Some("SHADOW")
    }

    /// Carries LLM or MCP traffic — an AI front door.
    pub fn is_ai(&self) -> bool {
        self.workloads.iter().any(|w| w == "LLM" || w == "MCP")
    }
}

crate::sections! {
    pub enum ApiItemDetailSection,
    pub static API_ITEM_SECTIONS = [
        Overview "Overview",
    ]
}

impl Resource for ApiInventoryItem {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&API_ITEM_SECTIONS)
    }

    fn references(&self) -> Vec<(String, String)> {
        vec![("Resource".to_string(), self.resource_arn.clone())]
    }

    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws apiportal get-api-inventory-item --method {} --host {} --port {} --path {} --resource-arn {}",
            shell_quote(&self.method),
            shell_quote(&self.host),
            self.port,
            shell_quote(&self.path),
            shell_quote(&self.resource_arn),
        ))
    }

    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        // The id starts with the signature; the ARN after it is for
        // uniqueness, not for reading.
        let sig_len = self.id.len() - self.resource_arn.len();
        self.id[..sig_len].trim_end()
    }

    fn resource_type(&self) -> &str {
        "API Inventory Item"
    }

    fn state(&self) -> ResourceState {
        match self.classification.as_deref() {
            Some("MANAGED") => ResourceState::Available,
            Some(c) => ResourceState::Unknown(enum_word(c)),
            None => ResourceState::stateless(),
        }
    }

    fn state_label(&self) -> String {
        native_state_label(&self.classification.as_deref().map(enum_word).unwrap_or_default(), || self.state())
    }

    fn list_summary(&self) -> Option<String> {
        let mut parts = vec![source_label(&self.source)];
        if self.is_public() {
            parts.push("public".to_string());
        }
        if self.waf_off() {
            parts.push("no WAF".to_string());
        }
        parts.extend(self.workloads.iter().cloned());
        Some(parts.join(" · "))
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!(
            "{} {} {} {} {} {} {} {} {} api inventory",
            self.signature(),
            self.resource_arn,
            self.account_id,
            source_label(&self.source),
            self.visibility.as_deref().unwrap_or(""),
            self.classification.as_deref().map(enum_word).unwrap_or_default(),
            if self.waf_off() { "no waf" } else { "waf" },
            self.workloads.join(" "),
            self.region,
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("API".to_string(), self.signature()),
            ("Resource".to_string(), self.resource_arn.clone()),
            ("Classification".to_string(), self.state_label()),
        ]
    }

    fn console_url(&self, _region: &str) -> Option<String> {
        None
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── Managed API ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct ManagedApiFinding {
    pub posture_id: String,
    /// `TRUE` (satisfied) / `FALSE` / `INSUFFICIENT_DATA` / `NOT_APPLICABLE`.
    pub status: String,
    /// Present for `FALSE` and `INSUFFICIENT_DATA`; an unknown reason reads
    /// as sent.
    pub reason: Option<String>,
    pub updated: Option<String>,
}

impl ManagedApiFinding {
    pub fn failing(&self) -> bool {
        self.status == "FALSE"
    }
}

#[derive(Debug, Clone, Default)]
pub struct ManagedApi {
    pub id: String,
    pub arn: String,
    pub signature: String,
    pub resource_arn: String,
    pub created: Option<String>,
    pub updated: Option<String>,
    pub findings: Vec<ManagedApiFinding>,
    /// The inventory item with this signature, when it loaded.
    pub inventory_item: Option<String>,
    pub tags: HashMap<String, String>,
    /// False when `GetManagedApi` failed and only the summary is known.
    pub described: bool,
}

impl ManagedApi {
    fn sig_of(s: Option<&aws_sdk_apiportal::types::ApiSignature>) -> (String, String) {
        match s {
            Some(s) => (
                signature(s.method().as_str(), s.hostname(), s.port(), s.path()),
                s.resource_arn().to_string(),
            ),
            None => Default::default(),
        }
    }

    pub fn from_summary(s: &aws_sdk_apiportal::types::ManagedApiSummary) -> Self {
        let (signature, resource_arn) = Self::sig_of(s.api_signature());
        Self {
            id: s.managed_api_id().to_string(),
            arn: s.managed_api_arn().to_string(),
            signature,
            resource_arn,
            created: Some(fmt_dt(s.created_at())),
            updated: Some(fmt_dt(s.updated_at())),
            ..Default::default()
        }
    }

    pub fn from_get(o: &aws_sdk_apiportal::operation::get_managed_api::GetManagedApiOutput) -> Self {
        let (signature, resource_arn) = Self::sig_of(o.api_signature());
        Self {
            id: o.managed_api_id().to_string(),
            arn: o.managed_api_arn().to_string(),
            signature,
            resource_arn,
            created: Some(fmt_dt(o.created_at())),
            updated: Some(fmt_dt(o.updated_at())),
            findings: o
                .findings()
                .iter()
                .map(|f| ManagedApiFinding {
                    posture_id: f.posture_id().unwrap_or_default().to_string(),
                    status: f.posture_status().map(|v| v.as_str().to_string()).unwrap_or_default(),
                    reason: f.reason().map(|v| v.as_str().to_string()),
                    updated: f.updated_at().map(fmt_dt),
                })
                .collect(),
            tags: o.tags().cloned().unwrap_or_default(),
            described: true,
            inventory_item: None,
        }
    }

    pub fn failing(&self) -> usize {
        self.findings.iter().filter(|f| f.failing()).count()
    }
}

crate::sections! {
    pub enum ManagedApiDetailSection,
    pub static MANAGED_API_SECTIONS = [
        Overview "Overview",
        Findings "Findings",
        Events "Events" => crate::app::App::trigger_apiportal_events_load,
        Tags "Tags",
    ]
}

impl Resource for ManagedApi {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&MANAGED_API_SECTIONS)
    }

    fn references(&self) -> Vec<(String, String)> {
        vec![("Resource".to_string(), self.resource_arn.clone())]
    }

    fn cli_command(&self) -> Option<String> {
        Some(format!("aws apiportal get-managed-api --managed-api-id {}", shell_quote(&self.id)))
    }

    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        if self.signature.is_empty() {
            &self.id
        } else {
            &self.signature
        }
    }

    fn resource_type(&self) -> &str {
        "Managed API"
    }

    // A failing posture check is the state worth seeing in the list.
    fn state(&self) -> ResourceState {
        if !self.described {
            ResourceState::stateless()
        } else if self.failing() > 0 {
            ResourceState::Unknown("findings".to_string())
        } else {
            ResourceState::Available
        }
    }

    fn state_label(&self) -> String {
        match (self.described, self.failing()) {
            (false, _) => String::new(),
            (true, 0) => "passing".to_string(),
            (true, n) => format!("{n} failing"),
        }
    }

    fn list_summary(&self) -> Option<String> {
        Some(self.id.clone())
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        let reasons: Vec<&str> = self.findings.iter().filter_map(|f| f.reason.as_deref()).collect();
        format!(
            "{} {} {} {} managed api",
            self.signature,
            self.id,
            self.resource_arn,
            reasons.join(" ").to_lowercase(),
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("API".to_string(), self.name().to_string()),
            ("Managed API ID".to_string(), self.id.clone()),
            ("ARN".to_string(), self.arn.clone()),
        ]
    }

    fn console_url(&self, _region: &str) -> Option<String> {
        None
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── Portal ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct ApiPortal {
    pub id: String,
    pub arn: String,
    pub name: String,
    pub description: Option<String>,
    pub domain_name: Option<String>,
    pub default_domain: Option<String>,
    pub certificate_arn: Option<String>,
    /// `PUBLISHED`, `PUBLISH_FAILED`, …
    pub publish_status: Option<String>,
    pub publish_description: Option<String>,
    /// `(exception, message)` of the last failed publish.
    pub status_exception: Option<(String, String)>,
    pub updated: Option<String>,
    pub published: Option<String>,
    pub product_arns: Vec<String>,
    pub rum_app_monitor: Option<String>,
    /// `Cognito user pool` / `none`.
    pub authorization: Option<String>,
    pub cognito_user_pool_arn: Option<String>,
    pub preview_status: Option<String>,
    pub preview_url: Option<String>,
    pub tags: HashMap<String, String>,
}

impl ApiPortal {
    pub fn from_sdk(p: &aws_sdk_apiportal::types::PortalSummary) -> Self {
        let ep = p.endpoint_configuration();
        let content = p.portal_content();
        let auth = p.authorization().and_then(|a| a.authorization_config());
        let cognito = auth.and_then(|c| c.as_cognito_config().ok());
        Self {
            id: p.portal_id().to_string(),
            arn: p.portal_arn().to_string(),
            name: content.map(|c| c.display_name().to_string()).unwrap_or_default(),
            description: content.and_then(|c| c.description()).filter(|d| !d.is_empty()).map(str::to_string),
            domain_name: ep.and_then(|e| e.domain_name()).map(str::to_string),
            default_domain: ep.map(|e| e.portal_default_domain_name().to_string()).filter(|d| !d.is_empty()),
            certificate_arn: ep.and_then(|e| e.certificate_arn()).map(str::to_string),
            publish_status: p.publish_status().map(|v| v.as_str().to_string()),
            publish_description: p.publish_description().filter(|d| !d.is_empty()).map(str::to_string),
            status_exception: p.status_exception().map(|e| {
                (
                    e.exception().map(|v| v.as_str().to_string()).unwrap_or_default(),
                    e.message().unwrap_or_default().to_string(),
                )
            }),
            updated: Some(fmt_dt(p.updated_at())),
            published: p.published_at().map(fmt_dt),
            product_arns: p.included_product_arns().to_vec(),
            rum_app_monitor: p.rum_app_monitor_name().map(str::to_string),
            authorization: auth.map(|c| if c.is_cognito_config() { "Cognito user pool" } else { "none" }.to_string()),
            cognito_user_pool_arn: cognito.map(|c| c.user_pool_arn().to_string()),
            preview_status: p.preview().map(|v| v.preview_status().as_str().to_string()),
            preview_url: p.preview().and_then(|v| v.preview_url()).map(str::to_string),
            tags: HashMap::new(),
        }
    }

    /// The address people open: the custom domain, else the default one.
    pub fn url(&self) -> Option<String> {
        self.domain_name.clone().or_else(|| self.default_domain.clone()).map(|d| format!("https://{d}"))
    }
}

crate::sections! {
    pub enum ApiPortalDetailSection,
    pub static API_PORTAL_SECTIONS = [
        Overview "Overview",
        Products "Products",
    ]
}

impl Resource for ApiPortal {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&API_PORTAL_SECTIONS)
    }

    fn references(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        if let Some(c) = &self.certificate_arn {
            out.push(("Certificate".to_string(), c.clone()));
        }
        if let Some(u) = &self.cognito_user_pool_arn {
            out.push(("User Pool".to_string(), u.clone()));
        }
        out
    }

    fn cli_command(&self) -> Option<String> {
        Some(format!("aws apiportal get-portal --portal-id {}", shell_quote(&self.id)))
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
        "API Portal"
    }

    fn state(&self) -> ResourceState {
        match self.publish_status.as_deref() {
            Some("PUBLISHED") => ResourceState::Running,
            Some("PUBLISH_IN_PROGRESS" | "DISABLE_IN_PROGRESS") => ResourceState::Pending,
            Some("PUBLISH_FAILED" | "DISABLE_FAILED") => ResourceState::Unavailable,
            Some("DISABLED") => ResourceState::Stopped,
            Some(other) => ResourceState::Unknown(enum_word(other)),
            None => ResourceState::stateless(),
        }
    }

    fn state_label(&self) -> String {
        native_state_label(&self.publish_status.as_deref().map(enum_word).unwrap_or_default(), || self.state())
    }

    fn list_summary(&self) -> Option<String> {
        let n = self.product_arns.len();
        let products = format!("{n} product{}", if n == 1 { "" } else { "s" });
        Some(match self.url() {
            Some(u) => format!("{u} · {products}"),
            None => products,
        })
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!(
            "{} {} {} {} {} api portal",
            self.name,
            self.id,
            self.domain_name.as_deref().unwrap_or(""),
            self.default_domain.as_deref().unwrap_or(""),
            self.description.as_deref().unwrap_or(""),
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Portal".to_string(), self.name().to_string()),
            ("Portal ID".to_string(), self.id.clone()),
            ("ARN".to_string(), self.arn.clone()),
        ]
    }

    fn console_url(&self, _region: &str) -> Option<String> {
        None
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── Product ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct ApiProduct {
    pub id: String,
    pub arn: String,
    pub name: String,
    pub description: Option<String>,
    pub updated: Option<String>,
    /// `ACTIVE` / `IN_PROGRESS` / `FAILED`.
    pub status: Option<String>,
    pub failure_reason: Option<String>,
    /// How the product picks its APIs, e.g. `rule based`.
    pub selection_mode: Option<String>,
    /// `(key, values)` of each rule filter.
    pub rule_filters: Vec<(String, Vec<String>)>,
    pub tags: HashMap<String, String>,
}

impl ApiProduct {
    pub fn from_sdk(p: &aws_sdk_apiportal::types::ProductSummary) -> Self {
        let sel = p.api_selection_configuration();
        Self {
            id: p.product_id().to_string(),
            arn: p.product_arn().to_string(),
            name: p.display_name().to_string(),
            description: Some(p.description().to_string()).filter(|d| !d.is_empty()),
            updated: Some(fmt_dt(p.updated_at())),
            status: p.status().map(|v| v.as_str().to_string()),
            failure_reason: p.failure_reason().map(|v| v.as_str().to_string()),
            selection_mode: sel.map(|s| enum_word(s.api_selection_mode().as_str())),
            rule_filters: sel
                .map(|s| {
                    s.rule_filters()
                        .iter()
                        .map(|f| (f.key().as_str().to_string(), f.values().to_vec()))
                        .collect()
                })
                .unwrap_or_default(),
            tags: HashMap::new(),
        }
    }
}

crate::sections! {
    pub enum ApiProductDetailSection,
    pub static API_PRODUCT_SECTIONS = [
        Overview "Overview" => crate::app::App::trigger_apiportal_product_apis_load,
        Apis "APIs" => crate::app::App::trigger_apiportal_product_apis_load,
    ]
}

impl Resource for ApiProduct {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&API_PRODUCT_SECTIONS)
    }

    fn cli_command(&self) -> Option<String> {
        Some(format!("aws apiportal get-product --product-id {}", shell_quote(&self.id)))
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
        "API Product"
    }

    fn state(&self) -> ResourceState {
        match self.status.as_deref() {
            Some("ACTIVE") => ResourceState::Available,
            Some("IN_PROGRESS") => ResourceState::Pending,
            Some("FAILED") => ResourceState::Unavailable,
            Some(other) => ResourceState::Unknown(enum_word(other)),
            None => ResourceState::stateless(),
        }
    }

    fn state_label(&self) -> String {
        native_state_label(&self.status.as_deref().map(enum_word).unwrap_or_default(), || self.state())
    }

    fn list_summary(&self) -> Option<String> {
        self.selection_mode.clone().or_else(|| Some(self.id.clone()))
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!(
            "{} {} {} api product",
            self.name,
            self.id,
            self.description.as_deref().unwrap_or(""),
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Product".to_string(), self.name().to_string()),
            ("Product ID".to_string(), self.id.clone()),
            ("ARN".to_string(), self.arn.clone()),
        ]
    }

    fn console_url(&self, _region: &str) -> Option<String> {
        None
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── Discovery ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct DiscoveryStatus {
    pub account_id: Option<String>,
    pub source: String,
    /// `RUNNING` / `PROVISIONING` / `STOPPED` / `FAILED`.
    pub status: String,
    pub failure_reason: Option<String>,
}

impl DiscoveryStatus {
    fn from_sdk(s: &aws_sdk_apiportal::types::DiscoveryStatus) -> Self {
        Self {
            account_id: s.account_id().map(str::to_string),
            source: s.discovery_source().as_str().to_string(),
            status: s.status().as_str().to_string(),
            failure_reason: s.failure_reason().map(|v| v.as_str().to_string()),
        }
    }
}

/// One discovery configuration: the account's own (always a row, so the
/// tab can say "not configured"), and the organization's where the caller
/// may read it.
#[derive(Debug, Clone, Default)]
pub struct ApiDiscovery {
    /// `account` / `organization`.
    pub id: String,
    pub arn: Option<String>,
    /// `ENABLED` / `DISABLED` / `UPDATING`; `None` when never configured.
    pub status: Option<String>,
    pub sources: Vec<String>,
    pub features: Vec<String>,
    pub excluded_resources: Vec<String>,
    pub created: Option<String>,
    pub updated: Option<String>,
    /// Per-source (and, org-wide, per-account) crawler status. Account row only.
    pub statuses: Vec<DiscoveryStatus>,
    /// Why the configuration couldn't be read (not "never configured").
    pub error: Option<String>,
    pub tags: HashMap<String, String>,
}

impl ApiDiscovery {
    pub fn account() -> Self {
        Self { id: "account".to_string(), ..Default::default() }
    }

    fn fill_account(&mut self, c: &aws_sdk_apiportal::types::DiscoveryConfigurationData) {
        self.arn = Some(c.arn().to_string());
        self.status = c.status().map(|v| v.as_str().to_string());
        self.sources = c.discovery_sources().iter().map(|s| s.as_str().to_string()).collect();
        self.features = c.features().iter().map(|f| f.as_str().to_string()).collect();
        self.excluded_resources = c.excluded_resources().to_vec();
        self.created = c.created_at().map(fmt_dt);
        self.updated = c.updated_at().map(fmt_dt);
    }

    fn organization(c: &aws_sdk_apiportal::types::OrganizationDiscoveryConfigurationData) -> Self {
        Self {
            id: "organization".to_string(),
            arn: Some(c.arn().to_string()),
            status: c.status().map(|v| v.as_str().to_string()),
            sources: c.discovery_sources().iter().map(|s| s.as_str().to_string()).collect(),
            features: c.features().iter().map(|f| f.as_str().to_string()).collect(),
            created: c.created_at().map(fmt_dt),
            updated: c.updated_at().map(fmt_dt),
            ..Default::default()
        }
    }

    pub fn is_organization(&self) -> bool {
        self.id == "organization"
    }

    /// Why the inventory is empty, as `(title, hint)` — `None` when
    /// discovery looks healthy and an empty inventory is just no APIs yet.
    pub fn empty_inventory_reason(&self) -> Option<(String, String)> {
        if let Some(e) = &self.error {
            return Some(("Discovery configuration unreadable".to_string(), e.clone()));
        }
        match self.status.as_deref() {
            None => Some((
                "API discovery isn't on in this account/region".to_string(),
                "no discovery configuration · 5 Discovery".to_string(),
            )),
            Some("DISABLED") => Some((
                "API discovery is disabled".to_string(),
                "the configuration exists but is off · 5 Discovery".to_string(),
            )),
            _ => {
                if let Some(f) = self.statuses.iter().find(|s| s.status == "FAILED") {
                    let why = f.failure_reason.as_deref().map(enum_word).unwrap_or_else(|| "no reason given".to_string());
                    return Some((
                        format!("{} discovery failed", source_label(&f.source)),
                        format!("{why} · 5 Discovery"),
                    ));
                }
                if self.statuses.iter().any(|s| s.status == "PROVISIONING") {
                    return Some((
                        "Discovery is still starting".to_string(),
                        "the first crawl hasn't finished · r refresh".to_string(),
                    ));
                }
                None
            }
        }
    }
}

crate::sections! {
    pub enum ApiDiscoveryDetailSection,
    pub static API_DISCOVERY_SECTIONS = [
        Overview "Overview",
        Statuses "Statuses",
        Tags "Tags",
    ]
}

impl Resource for ApiDiscovery {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&API_DISCOVERY_SECTIONS)
    }

    fn cli_command(&self) -> Option<String> {
        Some(if self.is_organization() {
            "aws apiportal get-organization-discovery-configuration".to_string()
        } else {
            "aws apiportal get-discovery-configuration".to_string()
        })
    }

    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        if self.is_organization() {
            "Organization discovery"
        } else {
            "Account discovery"
        }
    }

    fn resource_type(&self) -> &str {
        "API Discovery"
    }

    fn state(&self) -> ResourceState {
        match self.status.as_deref() {
            Some("ENABLED") => ResourceState::Running,
            Some("UPDATING") => ResourceState::Pending,
            Some("DISABLED") => ResourceState::Stopped,
            Some(other) => ResourceState::Unknown(enum_word(other)),
            None if self.error.is_some() => ResourceState::Unknown("unreadable".to_string()),
            None => ResourceState::Unknown("not configured".to_string()),
        }
    }

    fn list_summary(&self) -> Option<String> {
        if self.sources.is_empty() {
            return Some("—".to_string());
        }
        Some(self.sources.iter().map(|s| source_label(s)).collect::<Vec<_>>().join(" · "))
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!("{} {} api discovery", self.name(), self.status.as_deref().unwrap_or("not configured"))
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Discovery".to_string(), self.name().to_string()),
            ("Status".to_string(), self.state_label()),
        ]
    }

    fn console_url(&self, _region: &str) -> Option<String> {
        None
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── Lazy: managed API events, product APIs ────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct ManagedApiEvent {
    pub id: String,
    pub event_type: Option<String>,
    /// `EVENT_STARTED` / `EVENT_IN_PROGRESS` / `EVENT_ENDED`.
    pub status: Option<String>,
    pub label: Option<String>,
    pub category: Option<String>,
    pub confidence: Option<String>,
    pub web_acl_arn: Option<String>,
    pub started: Option<String>,
    pub ended: Option<String>,
    pub started_secs: i64,
}

#[derive(Debug, Clone, Default)]
pub struct ManagedApiEvents {
    pub items: Vec<ManagedApiEvent>,
    pub truncated: bool,
}

/// The last 30 days of a managed API's security events (the API's default
/// window), newest first, capped at [`MAX_EVENTS`].
pub async fn fetch_managed_api_events(client: ApiPortalClient, id: String) -> Result<ManagedApiEvents> {
    let mut out = ManagedApiEvents::default();
    let mut token: Option<String> = None;
    loop {
        let page = client
            .list_managed_api_events()
            .managed_api_id(&id)
            .max_results(20)
            .set_next_token(token.clone())
            .send()
            .await
            .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
        out.items.extend(page.items().iter().map(|e| {
            let waf = e.waf_api_security_event_detail();
            ManagedApiEvent {
                id: e.event_id().to_string(),
                event_type: e.event_type().map(|v| v.as_str().to_string()),
                status: e.status().map(|v| v.as_str().to_string()),
                label: waf.and_then(|w| w.label()).map(str::to_string),
                category: waf.and_then(|w| w.category()).map(str::to_string),
                confidence: waf.and_then(|w| w.confidence()).map(|v| v.as_str().to_string()),
                web_acl_arn: waf.and_then(|w| w.web_acl_arn()).map(str::to_string),
                started: e.started_at().map(fmt_dt),
                ended: e.ended_at().map(fmt_dt),
                started_secs: e.started_at().map(|d| d.secs()).unwrap_or_default(),
            }
        }));
        token = crate::aws::pagination::next_page_token(page.next_token(), &token);
        if token.is_none() {
            break;
        }
        if out.items.len() >= MAX_EVENTS {
            out.truncated = true;
            break;
        }
    }
    out.items.sort_by_key(|e| std::cmp::Reverse(e.started_secs));
    if out.items.len() > MAX_EVENTS {
        out.truncated = true;
        out.items.truncate(MAX_EVENTS);
    }
    Ok(out)
}

/// `(managed API id, signature)` of each API a product includes.
pub async fn fetch_product_apis(client: ApiPortalClient, product_id: String) -> Result<Vec<(String, String)>> {
    let mut out = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let page = client
            .list_managed_apis()
            .product_id(&product_id)
            .max_results(100)
            .set_next_token(token.clone())
            .send()
            .await
            .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
        out.extend(page.items().iter().map(|m| {
            let api = ManagedApi::from_summary(m);
            (api.id.clone(), api.name().to_string())
        }));
        token = crate::aws::pagination::next_page_token(page.next_token(), &token);
        if token.is_none() {
            break;
        }
    }
    out.sort_by(|a, b| a.1.cmp(&b.1));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_sdk_apiportal::types as t;

    fn item(classification: t::Classification) -> ApiInventoryItem {
        ApiInventoryItem::from_sdk(
            &t::ApiInventoryItem::builder()
                .method(t::HttpMethod::Get)
                .host("api.example.com")
                .port(443)
                .path("/v1/users")
                .account_id("123456789012")
                .resource_arn("arn:aws:elasticloadbalancing:us-east-1:123456789012:loadbalancer/app/web/abc")
                .region("us-east-1")
                .discovery_source(t::DiscoverySource::from("SOMETHING_NEW"))
                .visibility(t::Visibility::Public)
                .waf_status(t::WafStatus::Disabled)
                .classification(classification)
                .workloads(t::Workload::Mcp)
                .workloads(t::Workload::from("FUTURE_PROTOCOL"))
                .build()
                .unwrap(),
        )
    }

    #[test]
    fn an_item_reads_its_signature_and_unknown_values_as_sent() {
        let i = item(t::Classification::Shadow);
        assert_eq!(i.name(), "GET api.example.com:443 /v1/users");
        assert!(i.id.ends_with("loadbalancer/app/web/abc"));
        assert_eq!(i.source, "SOMETHING_NEW");
        assert_eq!(source_label(&i.source), "SOMETHING_NEW");
        assert_eq!(i.workloads, ["MCP", "FUTURE_PROTOCOL"]);
        assert!(i.is_public() && i.waf_off() && i.is_shadow() && i.is_ai());
        assert_eq!(i.state_label(), "shadow");
        assert_eq!(i.list_summary().unwrap(), "SOMETHING_NEW · public · no WAF · MCP · FUTURE_PROTOCOL");
    }

    #[test]
    fn an_unconfigured_account_explains_its_empty_inventory() {
        let d = ApiDiscovery::account();
        assert_eq!(d.state_label(), "not configured");
        assert!(d.empty_inventory_reason().unwrap().0.contains("isn't on"));

        let mut on = ApiDiscovery::account();
        on.status = Some("ENABLED".into());
        assert!(on.empty_inventory_reason().is_none());
        on.statuses.push(DiscoveryStatus {
            source: "ELB".into(),
            status: "FAILED".into(),
            failure_reason: Some("ACCESS_DENIED".into()),
            ..Default::default()
        });
        let (title, hint) = on.empty_inventory_reason().unwrap();
        assert_eq!(title, "Elastic Load Balancing discovery failed");
        assert!(hint.starts_with("access denied"));
    }
}
