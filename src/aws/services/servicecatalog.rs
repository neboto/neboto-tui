use crate::aws::client::AwsClients;
use crate::aws::pagination::next_page_token;
use crate::aws::resource::{native_state_label, shell_quote, Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::error::Result;
use crate::event::{Event, LoadProgress};
use async_trait::async_trait;
use aws_sdk_servicecatalog::Client as ScClient;
use std::any::Any;
use std::collections::HashMap;
use tokio::sync::mpsc;

/// Product/portfolio catalogs are admin-bounded but provisioned products and
/// record history can pile up; cap every list.
const MAX_PORTFOLIOS: usize = 200;
const MAX_PRODUCTS: usize = 300;
const MAX_PROVISIONED: usize = 300;
const MAX_TAG_OPTIONS: usize = 300;
const MAX_PORTFOLIO_PRODUCTS: usize = 100;
const MAX_PP_RECORDS: usize = 50;
const MAX_ACCESS_ITEMS: usize = 100;
/// Per-version `DescribeProvisioningArtifact` enrichment is N+1 on the product
/// pane — bound it. Versions past the cap still list from the summaries; the
/// pane annotates the cutoff.
pub(crate) const MAX_ARTIFACT_DETAILS: usize = 25;

/// AWS Service Catalog — sub-tabs Portfolios / Products / Provisioned Products /
/// TagOptions, all through the **admin-view** APIs (`SearchProductsAsAdmin`,
/// `SearchProvisionedProducts` with the Account access filter) so the whole
/// account's inventory shows, not just what's shared with the caller.
/// Read-only: no provisioning APIs are ever called.
pub struct ServiceCatalogService {
    client: ScClient,
}

impl ServiceCatalogService {
    pub fn new(aws_clients: &AwsClients) -> Self {
        Self {
            client: aws_clients.servicecatalog_client(),
        }
    }
}

#[async_trait]
impl AwsService for ServiceCatalogService {
    fn service_type(&self) -> ServiceType {
        ServiceType::ServiceCatalog
    }

    fn name(&self) -> &str {
        "Service Catalog"
    }

    async fn list_resources(&self) -> Result<Vec<Box<dyn Resource>>> {
        Ok(vec![])
    }

    /// Four phases, one per sub-tab. Every phase is independent: a family that
    /// fails sends `ResourceLoadWarning` and the rest still stream. The one
    /// deliberate silence: `TagOptionNotMigratedException` from `ListTagOptions`
    /// (the TagOptions library was never enabled — true for most accounts, so a
    /// warning on every load would be noise; the tab's empty state explains).
    async fn list_resources_streaming(
        &self,
        event_tx: mpsc::UnboundedSender<Event>,
        service_type: ServiceType,
    ) -> Result<()> {
        let mut total = 0usize;

        let emit = |batch: Vec<Box<dyn Resource>>, total: &mut usize| {
            if batch.is_empty() {
                return;
            }
            *total += batch.len();
            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                service: service_type,
                resources: batch,
                progress: LoadProgress {
                    loaded_count: *total,
                    total_count: None,
                    status_message: None,
                },
            });
        };
        let warn = |phase: &str, e: String| {
            let _ = event_tx.send(Event::ResourceLoadWarning {
                service: service_type,
                warning: format!("{}: {}", phase, e),
            });
        };

        // ── Portfolios ───────────────────────────────────────────────────────
        let mut count = 0usize;
        let mut pager = self.client.list_portfolios().into_paginator().send();
        loop {
            match pager.next().await {
                Some(Ok(page)) => {
                    let batch: Vec<Box<dyn Resource>> = page
                        .portfolio_details()
                        .iter()
                        .map(|p| Box::new(ScPortfolio::from_sdk(p)) as Box<dyn Resource>)
                        .collect();
                    count += batch.len();
                    emit(batch, &mut total);
                    if count >= MAX_PORTFOLIOS {
                        break;
                    }
                }
                Some(Err(e)) => {
                    warn("Portfolios", crate::error::sdk_error_message(&e));
                    break;
                }
                None => break,
            }
        }

        // ── Products (admin view: every product in the account) ──────────────
        let mut count = 0usize;
        let mut pager = self.client.search_products_as_admin().into_paginator().send();
        loop {
            match pager.next().await {
                Some(Ok(page)) => {
                    let batch: Vec<Box<dyn Resource>> = page
                        .product_view_details()
                        .iter()
                        .map(|p| Box::new(ScProduct::from_sdk(p)) as Box<dyn Resource>)
                        .collect();
                    count += batch.len();
                    emit(batch, &mut total);
                    if count >= MAX_PRODUCTS {
                        break;
                    }
                }
                Some(Err(e)) => {
                    warn("Products", crate::error::sdk_error_message(&e));
                    break;
                }
                None => break,
            }
        }

        // ── Provisioned products (account-wide) ──────────────────────────────
        let mut count = 0usize;
        let mut pager = self
            .client
            .search_provisioned_products()
            .access_level_filter(account_access_filter())
            .into_paginator()
            .send();
        loop {
            match pager.next().await {
                Some(Ok(page)) => {
                    let batch: Vec<Box<dyn Resource>> = page
                        .provisioned_products()
                        .iter()
                        .map(|p| Box::new(ScProvisionedProduct::from_sdk(p)) as Box<dyn Resource>)
                        .collect();
                    count += batch.len();
                    emit(batch, &mut total);
                    if count >= MAX_PROVISIONED {
                        break;
                    }
                }
                Some(Err(e)) => {
                    warn("Provisioned products", crate::error::sdk_error_message(&e));
                    break;
                }
                None => break,
            }
        }

        // ── TagOptions ───────────────────────────────────────────────────────
        let mut count = 0usize;
        let mut pager = self.client.list_tag_options().into_paginator().send();
        loop {
            match pager.next().await {
                Some(Ok(page)) => {
                    let batch: Vec<Box<dyn Resource>> = page
                        .tag_option_details()
                        .iter()
                        .map(|t| Box::new(ScTagOption::from_sdk(t)) as Box<dyn Resource>)
                        .collect();
                    count += batch.len();
                    emit(batch, &mut total);
                    if count >= MAX_TAG_OPTIONS {
                        break;
                    }
                }
                Some(Err(e)) => {
                    let not_migrated = e
                        .as_service_error()
                        .is_some_and(|se| se.is_tag_option_not_migrated_exception());
                    if !not_migrated {
                        warn("TagOptions", crate::error::sdk_error_message(&e));
                    }
                    break;
                }
                None => break,
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

/// `AccessLevelFilter { key: Account, value: "self" }` — "self" is the only
/// value the API accepts; with key Account it means "everything in this
/// account", which is the admin-browser perspective.
fn account_access_filter() -> aws_sdk_servicecatalog::types::AccessLevelFilter {
    aws_sdk_servicecatalog::types::AccessLevelFilter::builder()
        .key(aws_sdk_servicecatalog::types::AccessLevelFilterKey::Account)
        .value("self")
        .build()
}


// ── ScPortfolio ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ScPortfolio {
    pub id: String,
    pub arn: String,
    pub name: String,
    pub description: String,
    pub provider: String,
    pub created: Option<String>,
    pub tags: HashMap<String, String>,
}

impl ScPortfolio {
    pub fn from_sdk(p: &aws_sdk_servicecatalog::types::PortfolioDetail) -> Self {
        Self {
            id: p.id().unwrap_or_default().to_string(),
            arn: p.arn().unwrap_or_default().to_string(),
            name: p.display_name().unwrap_or_default().to_string(),
            description: p.description().unwrap_or_default().to_string(),
            provider: p.provider_name().unwrap_or_default().to_string(),
            created: p.created_time().map(|d| fmt_epoch_secs(d.secs())),
            tags: HashMap::new(),
        }
    }
}

crate::sections! {
    pub enum ScPortfolioDetailSection,
    pub static SC_PORTFOLIO_SECTIONS = [
        Details "Details",
        Products "Products" => crate::app::App::trigger_sc_portfolio_products_load,
        Principals "Principals" => crate::app::App::trigger_sc_portfolio_access_load,
        Constraints "Constraints" => crate::app::App::trigger_sc_portfolio_access_load,
        Shares "Shares" => crate::app::App::trigger_sc_portfolio_shares_load,
        Tags "Tags" => crate::app::App::trigger_sc_portfolio_extras_load,
    ]
}

impl Resource for ScPortfolio {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&SC_PORTFOLIO_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.id
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn resource_type(&self) -> &str {
        "SC Portfolio"
    }
    fn state(&self) -> ResourceState {
        ResourceState::stateless()
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        format!("{} {} {} {}", self.name, self.id, self.provider, self.description)
    }
    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Portfolio".to_string(), self.name.clone()),
            ("ID".to_string(), self.id.clone()),
            ("Provider".to_string(), self.provider.clone()),
        ]
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws servicecatalog describe-portfolio --id {}",
            shell_quote(&self.id)
        ))
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── ScProduct ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ScProduct {
    pub id: String,
    pub arn: String,
    pub name: String,
    pub owner: String,
    pub product_type: String,
    pub distributor: String,
    pub short_description: String,
    pub support_email: String,
    pub support_url: String,
    pub has_default_path: bool,
    pub status: String,
    pub created: Option<String>,
    pub tags: HashMap<String, String>,
}

impl ScProduct {
    pub fn from_sdk(d: &aws_sdk_servicecatalog::types::ProductViewDetail) -> Self {
        let s = d.product_view_summary();
        Self {
            id: s.and_then(|s| s.product_id()).unwrap_or_default().to_string(),
            arn: d.product_arn().unwrap_or_default().to_string(),
            name: s.and_then(|s| s.name()).unwrap_or_default().to_string(),
            owner: s.and_then(|s| s.owner()).unwrap_or_default().to_string(),
            product_type: s
                .and_then(|s| s.r#type())
                .map(|t| t.as_str().to_string())
                .unwrap_or_default(),
            distributor: s.and_then(|s| s.distributor()).unwrap_or_default().to_string(),
            short_description: s
                .and_then(|s| s.short_description())
                .unwrap_or_default()
                .to_string(),
            support_email: s.and_then(|s| s.support_email()).unwrap_or_default().to_string(),
            support_url: s.and_then(|s| s.support_url()).unwrap_or_default().to_string(),
            has_default_path: s.map(|s| s.has_default_path()).unwrap_or(false),
            status: d.status().map(|s| s.as_str().to_string()).unwrap_or_default(),
            created: d.created_time().map(|d| fmt_epoch_secs(d.secs())),
            tags: HashMap::new(),
        }
    }
}

crate::sections! {
    pub enum ScProductDetailSection,
    pub static SC_PRODUCT_SECTIONS = [
        Details "Details",
        Versions "Versions" => crate::app::App::trigger_sc_product_details_load,
        Portfolios "Portfolios" => crate::app::App::trigger_sc_product_details_load,
        Tags "Tags" => crate::app::App::trigger_sc_product_details_load,
    ]
}

impl Resource for ScProduct {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&SC_PRODUCT_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.id
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn resource_type(&self) -> &str {
        "SC Product"
    }
    fn state(&self) -> ResourceState {
        match self.status.as_str() {
            "AVAILABLE" => ResourceState::Available,
            "CREATING" => ResourceState::Pending,
            "FAILED" => ResourceState::Unavailable,
            "" => ResourceState::Available,
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
            "{} {} {} {} {}",
            self.name, self.id, self.owner, self.product_type, self.short_description
        )
    }
    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Product".to_string(), self.name.clone()),
            ("ID".to_string(), self.id.clone()),
            ("Owner".to_string(), self.owner.clone()),
            ("Type".to_string(), self.product_type.clone()),
        ]
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws servicecatalog describe-product-as-admin --id {}",
            shell_quote(&self.id)
        ))
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── ScProvisionedProduct ─────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ScProvisionedProduct {
    pub id: String,
    pub arn: String,
    pub name: String,
    pub pp_type: String,
    pub status: String,
    pub status_message: String,
    pub created: Option<String>,
    pub product_id: String,
    pub product_name: String,
    pub artifact_id: String,
    pub artifact_name: String,
    /// The backing CloudFormation stack ARN (for CFN-type products) — the
    /// detail pane renders it as a jumpable "Stack ARN" row.
    pub physical_id: String,
    pub last_record_id: String,
    pub user_arn: String,
    pub tags: HashMap<String, String>,
}

impl ScProvisionedProduct {
    pub fn from_sdk(p: &aws_sdk_servicecatalog::types::ProvisionedProductAttribute) -> Self {
        Self {
            id: p.id().unwrap_or_default().to_string(),
            arn: p.arn().unwrap_or_default().to_string(),
            name: p.name().unwrap_or_default().to_string(),
            pp_type: p.r#type().unwrap_or_default().to_string(),
            status: p.status().map(|s| s.as_str().to_string()).unwrap_or_default(),
            status_message: p.status_message().unwrap_or_default().to_string(),
            created: p.created_time().map(|d| fmt_epoch_secs(d.secs())),
            product_id: p.product_id().unwrap_or_default().to_string(),
            product_name: p.product_name().unwrap_or_default().to_string(),
            artifact_id: p.provisioning_artifact_id().unwrap_or_default().to_string(),
            artifact_name: p.provisioning_artifact_name().unwrap_or_default().to_string(),
            physical_id: p.physical_id().unwrap_or_default().to_string(),
            last_record_id: p.last_record_id().unwrap_or_default().to_string(),
            user_arn: p.user_arn().unwrap_or_default().to_string(),
            tags: p
                .tags()
                .iter()
                .map(|t| (t.key().to_string(), t.value().to_string()))
                .collect(),
        }
    }
}

crate::sections! {
    pub enum ScProvisionedProductDetailSection,
    pub static SC_PP_SECTIONS = [
        Details "Details",
        Outputs "Outputs" => crate::app::App::trigger_sc_pp_outputs_load,
        History "History" => crate::app::App::trigger_sc_pp_records_load,
        Tags "Tags",
    ]
}

impl Resource for ScProvisionedProduct {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&SC_PP_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.id
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn resource_type(&self) -> &str {
        "SC Provisioned Product"
    }
    fn state(&self) -> ResourceState {
        match self.status.as_str() {
            "AVAILABLE" => ResourceState::Available,
            "UNDER_CHANGE" | "PLAN_IN_PROGRESS" => ResourceState::Pending,
            "ERROR" => ResourceState::Unavailable,
            "" => ResourceState::Unknown("UNKNOWN".to_string()),
            // TAINTED: last operation failed but the last good version still runs.
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
            "{} {} {} {} {}",
            self.name, self.id, self.product_name, self.pp_type, self.physical_id
        )
    }
    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Provisioned Product".to_string(), self.name.clone()),
            ("ID".to_string(), self.id.clone()),
            ("Status".to_string(), self.status.clone()),
            ("Product".to_string(), self.product_name.clone()),
        ]
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws servicecatalog describe-provisioned-product --id {}",
            shell_quote(&self.id)
        ))
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── ScTagOption ──────────────────────────────────────────────────────────────

/// TagOptions-library entry. Flat `details()` — no split pane (the ElasticIp
/// precedent: the flat view already shows everything the list API returns).
#[derive(Debug, Clone)]
pub struct ScTagOption {
    pub id: String,
    /// Precomputed `key=value` — `name()` returns `&str`.
    pub label: String,
    pub key: String,
    pub value: String,
    pub active: bool,
    pub owner: String,
    pub tags: HashMap<String, String>,
}

impl ScTagOption {
    pub fn from_sdk(t: &aws_sdk_servicecatalog::types::TagOptionDetail) -> Self {
        let key = t.key().unwrap_or_default().to_string();
        let value = t.value().unwrap_or_default().to_string();
        Self {
            id: t.id().unwrap_or_default().to_string(),
            label: format!("{}={}", key, value),
            key,
            value,
            active: t.active().unwrap_or(false),
            owner: t.owner().unwrap_or_default().to_string(),
            tags: HashMap::new(),
        }
    }
}

impl Resource for ScTagOption {
    fn id(&self) -> &str {
        &self.id
    }
    fn name(&self) -> &str {
        &self.label
    }
    fn resource_type(&self) -> &str {
        "SC TagOption"
    }
    fn state(&self) -> ResourceState {
        if self.active {
            ResourceState::Available
        } else {
            ResourceState::Unknown("INACTIVE".to_string())
        }
    }

    fn state_label(&self) -> String {
        if self.active { "active" } else { "inactive" }.to_string()
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        format!("{} {} {} {}", self.key, self.value, self.id, self.owner)
    }
    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Key".to_string(), self.key.clone()),
            ("Value".to_string(), self.value.clone()),
            ("Active".to_string(), self.active.to_string()),
            ("ID".to_string(), self.id.clone()),
            ("Owner".to_string(), self.owner.clone()),
        ]
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws servicecatalog describe-tag-option --id {}",
            shell_quote(&self.id)
        ))
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── Lazy: a portfolio's products ─────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ScPortfolioProduct {
    pub id: String,
    pub name: String,
    pub owner: String,
    pub product_type: String,
    pub short_description: String,
}

pub async fn fetch_sc_portfolio_products(
    client: ScClient,
    portfolio_id: String,
) -> Result<Vec<ScPortfolioProduct>> {
    let mut products = Vec::new();
    let mut pager = client
        .search_products_as_admin()
        .portfolio_id(&portfolio_id)
        .into_paginator()
        .send();
    loop {
        match pager.next().await {
            Some(Ok(page)) => {
                for d in page.product_view_details() {
                    let s = d.product_view_summary();
                    products.push(ScPortfolioProduct {
                        id: s.and_then(|s| s.product_id()).unwrap_or_default().to_string(),
                        name: s.and_then(|s| s.name()).unwrap_or_default().to_string(),
                        owner: s.and_then(|s| s.owner()).unwrap_or_default().to_string(),
                        product_type: s
                            .and_then(|s| s.r#type())
                            .map(|t| t.as_str().to_string())
                            .unwrap_or_default(),
                        short_description: s
                            .and_then(|s| s.short_description())
                            .unwrap_or_default()
                            .to_string(),
                    });
                }
                if products.len() >= MAX_PORTFOLIO_PRODUCTS {
                    break;
                }
            }
            Some(Err(e)) => return Err(crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e))),
            None => break,
        }
    }
    Ok(products)
}

// ── Lazy: a portfolio's principals + constraints (one fetch, two sections) ───

#[derive(Debug, Clone)]
pub struct ScConstraint {
    pub id: String,
    pub ctype: String,
    pub description: String,
    pub product_id: String,
}

#[derive(Debug, Clone)]
pub struct ScPortfolioAccess {
    /// `(principal_arn, principal_type)`
    pub principals: Vec<(String, String)>,
    pub constraints: Vec<ScConstraint>,
}

pub async fn fetch_sc_portfolio_access(
    client: ScClient,
    portfolio_id: String,
) -> Result<ScPortfolioAccess> {
    let mut principals = Vec::new();
    let mut pager = client
        .list_principals_for_portfolio()
        .portfolio_id(&portfolio_id)
        .into_paginator()
        .send();
    loop {
        match pager.next().await {
            Some(Ok(page)) => {
                for p in page.principals() {
                    principals.push((
                        p.principal_arn().unwrap_or_default().to_string(),
                        p.principal_type()
                            .map(|t| t.as_str().to_string())
                            .unwrap_or_default(),
                    ));
                }
                if principals.len() >= MAX_ACCESS_ITEMS {
                    break;
                }
            }
            Some(Err(e)) => return Err(crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e))),
            None => break,
        }
    }

    let mut constraints = Vec::new();
    let mut pager = client
        .list_constraints_for_portfolio()
        .portfolio_id(&portfolio_id)
        .into_paginator()
        .send();
    loop {
        match pager.next().await {
            Some(Ok(page)) => {
                for c in page.constraint_details() {
                    constraints.push(ScConstraint {
                        id: c.constraint_id().unwrap_or_default().to_string(),
                        ctype: c.r#type().unwrap_or_default().to_string(),
                        description: c.description().unwrap_or_default().to_string(),
                        product_id: c.product_id().unwrap_or_default().to_string(),
                    });
                }
                if constraints.len() >= MAX_ACCESS_ITEMS {
                    break;
                }
            }
            Some(Err(e)) => return Err(crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e))),
            None => break,
        }
    }

    Ok(ScPortfolioAccess {
        principals,
        constraints,
    })
}

// ── Lazy: a portfolio's shares ───────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ScShare {
    pub principal_id: String,
    pub share_type: String,
    pub accepted: bool,
    pub share_tag_options: bool,
    pub share_principals: bool,
}

/// `DescribePortfolioShares` takes one share type per call, so loop all four.
/// Per-type failures are collected; the whole fetch errors only when every
/// type fails (a member account typically gets AccessDenied on all of them —
/// that renders once via `error_rows`, not four times).
pub async fn fetch_sc_portfolio_shares(
    client: ScClient,
    portfolio_id: String,
) -> Result<Vec<ScShare>> {
    use aws_sdk_servicecatalog::types::DescribePortfolioShareType as ShareType;
    let types = [
        ShareType::Account,
        ShareType::Organization,
        ShareType::OrganizationalUnit,
        ShareType::OrganizationMemberAccount,
    ];
    let mut shares = Vec::new();
    let mut last_err: Option<crate::error::Error> = None;
    let mut any_ok = false;
    for t in types {
        let mut pager = client
            .describe_portfolio_shares()
            .portfolio_id(&portfolio_id)
            .r#type(t)
            .into_paginator()
            .send();
        loop {
            match pager.next().await {
                Some(Ok(page)) => {
                    any_ok = true;
                    for s in page.portfolio_share_details() {
                        shares.push(ScShare {
                            principal_id: s.principal_id().unwrap_or_default().to_string(),
                            share_type: s
                                .r#type()
                                .map(|t| t.as_str().to_string())
                                .unwrap_or_default(),
                            accepted: s.accepted(),
                            share_tag_options: s.share_tag_options(),
                            share_principals: s.share_principals(),
                        });
                    }
                }
                Some(Err(e)) => {
                    last_err = Some(crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)));
                    break;
                }
                None => break,
            }
        }
    }
    if !any_ok {
        if let Some(e) = last_err {
            return Err(e);
        }
    }
    Ok(shares)
}

// ── Lazy: a portfolio's tags + tag options (DescribePortfolio) ───────────────

#[derive(Debug, Clone)]
pub struct ScPortfolioExtras {
    pub tags: Vec<(String, String)>,
    /// `(key=value, active)` for each associated TagOption.
    pub tag_options: Vec<(String, bool)>,
}

pub async fn fetch_sc_portfolio_extras(
    client: ScClient,
    portfolio_id: String,
) -> Result<ScPortfolioExtras> {
    match client.describe_portfolio().id(&portfolio_id).send().await {
        Ok(resp) => Ok(ScPortfolioExtras {
            tags: resp
                .tags()
                .iter()
                .map(|t| (t.key().to_string(), t.value().to_string()))
                .collect(),
            tag_options: resp
                .tag_options()
                .iter()
                .map(|t| {
                    (
                        format!(
                            "{}={}",
                            t.key().unwrap_or_default(),
                            t.value().unwrap_or_default()
                        ),
                        t.active().unwrap_or(false),
                    )
                })
                .collect(),
        }),
        Err(e) => Err(crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e))),
    }
}

// ── Lazy: a product's admin details (one fetch feeds three sections) ─────────

#[derive(Debug, Clone)]
pub struct ScArtifact {
    pub id: String,
    pub name: String,
    pub description: String,
    pub created: Option<String>,
    // Filled by the per-version DescribeProvisioningArtifact enrichment
    // (best-effort — a denied/failed describe leaves them empty and the
    // summary row still renders).
    pub artifact_type: String,
    pub active: Option<bool>,
    /// DEFAULT / DEPRECATED — whether end-users should still launch this.
    pub guidance: String,
    pub source_revision: String,
    /// Presigned S3 URL for the template body (verbose `info` map). Fetched
    /// over plain HTTP only on `e` — never eagerly.
    pub template_url: String,
    /// For products imported from a running stack: the stack ARN.
    pub imported_from: String,
    pub params: Vec<ScArtifactParam>,
}

/// One CloudFormation parameter of a provisioning artifact.
#[derive(Debug, Clone)]
pub struct ScArtifactParam {
    pub key: String,
    pub param_type: String,
    pub default: String,
    pub description: String,
    pub no_echo: bool,
}

#[derive(Debug, Clone)]
pub struct ScProductAdminDetails {
    pub artifacts: Vec<ScArtifact>,
    /// True when the version list ran past `MAX_ARTIFACT_DETAILS` and the
    /// tail rows carry summary data only.
    pub artifact_details_capped: bool,
    pub tags: Vec<(String, String)>,
    /// `(key=value, active)` for each associated TagOption.
    pub tag_options: Vec<(String, bool)>,
    /// `(portfolio_id, portfolio_name)` this product is published into.
    pub portfolios: Vec<(String, String)>,
}

pub async fn fetch_sc_product_details(
    client: ScClient,
    product_id: String,
) -> Result<Box<ScProductAdminDetails>> {
    // DescribeProductAsAdmin returns artifacts + tags + tag options in one call.
    let resp = match client.describe_product_as_admin().id(&product_id).send().await {
        Ok(r) => r,
        Err(e) => return Err(crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e))),
    };
    let mut artifacts: Vec<ScArtifact> = resp
        .provisioning_artifact_summaries()
        .iter()
        .map(|a| ScArtifact {
            id: a.id().unwrap_or_default().to_string(),
            name: a.name().unwrap_or_default().to_string(),
            description: a.description().unwrap_or_default().to_string(),
            created: a.created_time().map(|d| fmt_epoch_secs(d.secs())),
            artifact_type: String::new(),
            active: None,
            guidance: String::new(),
            source_revision: String::new(),
            template_url: String::new(),
            imported_from: String::new(),
            params: Vec::new(),
        })
        .collect();
    let artifact_details_capped = artifacts.len() > MAX_ARTIFACT_DETAILS;

    // Per-version enrichment: type/active/guidance, the template's presigned
    // URL (verbose `info`), and its CFN parameters. Best-effort per version —
    // a failed describe leaves the summary row as-is.
    for a in artifacts.iter_mut().take(MAX_ARTIFACT_DETAILS) {
        let Ok(d) = client
            .describe_provisioning_artifact()
            .product_id(&product_id)
            .provisioning_artifact_id(&a.id)
            .verbose(true)
            .include_provisioning_artifact_parameters(true)
            .send()
            .await
        else {
            continue;
        };
        if let Some(det) = d.provisioning_artifact_detail() {
            a.artifact_type = det.r#type().map(|t| t.as_str().to_string()).unwrap_or_default();
            a.active = det.active();
            a.guidance = det.guidance().map(|g| g.as_str().to_string()).unwrap_or_default();
            a.source_revision = det.source_revision().unwrap_or_default().to_string();
        }
        if let Some(info) = d.info() {
            // Describe publishes "TemplateUrl"; older/imported artifacts may
            // carry the create-time key names instead.
            a.template_url = ["TemplateUrl", "LoadTemplateFromURL"]
                .iter()
                .find_map(|k| info.get(*k).cloned())
                .unwrap_or_default();
            a.imported_from = ["ImportedFromPhysicalId", "ImportFromPhysicalId"]
                .iter()
                .find_map(|k| info.get(*k).cloned())
                .unwrap_or_default();
        }
        a.params = d
            .provisioning_artifact_parameters()
            .iter()
            .map(|p| ScArtifactParam {
                key: p.parameter_key().unwrap_or_default().to_string(),
                param_type: p.parameter_type().unwrap_or_default().to_string(),
                default: p.default_value().unwrap_or_default().to_string(),
                description: p.description().unwrap_or_default().to_string(),
                no_echo: p.is_no_echo(),
            })
            .collect();
    }
    let tags = resp
        .tags()
        .iter()
        .map(|t| (t.key().to_string(), t.value().to_string()))
        .collect();
    let tag_options = resp
        .tag_options()
        .iter()
        .map(|t| {
            (
                format!(
                    "{}={}",
                    t.key().unwrap_or_default(),
                    t.value().unwrap_or_default()
                ),
                t.active().unwrap_or(false),
            )
        })
        .collect();

    // Best-effort: the portfolios this product is published into.
    let mut portfolios = Vec::new();
    let mut pager = client
        .list_portfolios_for_product()
        .product_id(&product_id)
        .into_paginator()
        .send();
    loop {
        match pager.next().await {
            Some(Ok(page)) => {
                for p in page.portfolio_details() {
                    portfolios.push((
                        p.id().unwrap_or_default().to_string(),
                        p.display_name().unwrap_or_default().to_string(),
                    ));
                }
            }
            Some(Err(_)) => break,
            None => break,
        }
    }

    Ok(Box::new(ScProductAdminDetails {
        artifacts,
        artifact_details_capped,
        tags,
        tag_options,
        portfolios,
    }))
}

/// How to fetch a template URL. `DescribeProvisioningArtifact`'s verbose
/// `TemplateUrl` is the **raw S3 object URL** of the admin's template bucket
/// (NOT presigned — an anonymous GET 403s), so S3-shaped URLs go through the
/// authenticated `GetObject` path. Plain HTTP is kept only for URLs that
/// carry their own signature (presigned) or aren't S3 at all.
pub enum ScTemplateSource {
    S3 { bucket: String, key: String, region: Option<String> },
    Http(String),
}

pub fn classify_template_url(url: &str) -> ScTemplateSource {
    let lower = url.to_ascii_lowercase();
    let presigned = lower.contains("x-amz-signature=") || lower.contains("&signature=");
    if !presigned {
        if let Some((bucket, key, region)) = parse_s3_https_url(url) {
            return ScTemplateSource::S3 { bucket, key, region };
        }
    }
    ScTemplateSource::Http(url.to_string())
}

/// Parse an https S3 object URL into `(bucket, key, region)`. Handles
/// virtual-hosted (`bucket.s3.region.amazonaws.com/key`,
/// `bucket.s3.amazonaws.com/key`, legacy `bucket.s3-region…`) and path-style
/// (`s3.region.amazonaws.com/bucket/key`, legacy `s3.amazonaws.com/bucket/key`)
/// forms. Returns None for anything that isn't an amazonaws.com S3 host.
fn parse_s3_https_url(url: &str) -> Option<(String, String, Option<String>)> {
    let rest = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://"))?;
    let (host_port, path_query) = rest.split_once('/')?;
    let host = host_port.split(':').next().unwrap_or(host_port);
    let path = path_query.split('?').next().unwrap_or(path_query);
    let host = host.strip_suffix(".amazonaws.com")?;
    // Path style: host is "s3", "s3.us-east-1", or legacy "s3-us-east-1".
    let path_style_region = if host == "s3" {
        Some(None)
    } else {
        host.strip_prefix("s3.")
            .or_else(|| host.strip_prefix("s3-"))
            .filter(|r| !r.contains('.'))
            .map(|r| Some(r.to_string()))
    };
    if let Some(region) = path_style_region {
        let (bucket, key) = path.split_once('/')?;
        if bucket.is_empty() || key.is_empty() {
            return None;
        }
        return Some((bucket.to_string(), percent_decode(key), region));
    }
    // Virtual-hosted: "{bucket}.s3[.-{region}]". rsplit so a bucket name
    // containing ".s3" still parses.
    let (bucket, tail) = host.rsplit_once(".s3")?;
    let region = match tail {
        "" => None,
        t => {
            let r = t.strip_prefix('.').or_else(|| t.strip_prefix('-'))?;
            if r.contains('.') {
                return None; // dualstack/accesspoint shapes — not handled
            }
            Some(r.to_string())
        }
    };
    if bucket.is_empty() || path.is_empty() {
        return None;
    }
    Some((bucket.to_string(), percent_decode(path), region))
}

/// Minimal %XX decoder for S3 object keys lifted out of a URL path. `+` is a
/// literal in paths (only queries encode spaces that way), so it's kept.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Some(b) = std::str::from_utf8(&bytes[i + 1..i + 3])
                .ok()
                .and_then(|h| u8::from_str_radix(h, 16).ok())
            {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Authenticated template fetch — the caller resolves the region-pinned S3
/// client from the parsed URL.
pub async fn fetch_sc_template_body_s3(
    client: aws_sdk_s3::Client,
    bucket: String,
    key: String,
) -> Result<String> {
    let resp = client
        .get_object()
        .bucket(&bucket)
        .key(&key)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
    let bytes = resp
        .body
        .collect()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(e.to_string()))?
        .into_bytes();
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Download a template body over plain HTTP — presigned or non-S3 URLs only
/// (see `classify_template_url`). Same ureq-off-the-runtime shape as the
/// Lambda code download.
pub async fn fetch_sc_template_body(url: String) -> Result<String> {
    tokio::task::spawn_blocking(move || -> std::io::Result<String> {
        use std::io::Read;
        let io_err = std::io::Error::other;
        let resp = ureq::get(&url).call().map_err(|e| io_err(e.to_string()))?;
        // CFN caps S3-hosted templates at 1 MB; read_to_string would also be
        // fine, but keep a byte read so a stray BOM can't error the whole body.
        let mut bytes: Vec<u8> = Vec::new();
        resp.into_reader().read_to_end(&mut bytes)?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    })
    .await
    .map_err(|e| crate::error::Error::AwsSdk(format!("download task failed: {e}")))?
    .map_err(|e| crate::error::Error::AwsSdk(e.to_string()))
}

// ── Lazy: a provisioned product's outputs ────────────────────────────────────

/// `(key, value, description)` triples from the last successful record.
pub async fn fetch_sc_pp_outputs(
    client: ScClient,
    pp_id: String,
) -> Result<Vec<(String, String, String)>> {
    let mut outputs = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut req = client
            .get_provisioned_product_outputs()
            .provisioned_product_id(&pp_id);
        if let Some(t) = &token {
            req = req.page_token(t);
        }
        match req.send().await {
            Ok(page) => {
                for o in page.outputs() {
                    outputs.push((
                        o.output_key().unwrap_or_default().to_string(),
                        o.output_value().unwrap_or_default().to_string(),
                        o.description().unwrap_or_default().to_string(),
                    ));
                }
                token = next_page_token(page.next_page_token(), &token);
                if token.is_none() {
                    break;
                }
            }
            Err(e) => return Err(crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e))),
        }
    }
    Ok(outputs)
}

// ── Lazy: a provisioned product's record history ─────────────────────────────

#[derive(Debug, Clone)]
pub struct ScRecord {
    pub id: String,
    pub record_type: String,
    pub status: String,
    pub created: Option<String>,
    pub updated: Option<String>,
    pub artifact_id: String,
    pub errors: Vec<String>,
}

pub async fn fetch_sc_pp_records(client: ScClient, pp_id: String) -> Result<Vec<ScRecord>> {
    let mut records: Vec<(i64, ScRecord)> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut req = client
            .list_record_history()
            .access_level_filter(account_access_filter())
            .search_filter(
                aws_sdk_servicecatalog::types::ListRecordHistorySearchFilter::builder()
                    .key("provisionedproduct")
                    .value(&pp_id)
                    .build(),
            );
        if let Some(t) = &token {
            req = req.page_token(t);
        }
        match req.send().await {
            Ok(page) => {
                for r in page.record_details() {
                    let created_secs = r.created_time().map(|d| d.secs()).unwrap_or(0);
                    records.push((
                        created_secs,
                        ScRecord {
                            id: r.record_id().unwrap_or_default().to_string(),
                            record_type: r.record_type().unwrap_or_default().to_string(),
                            status: r
                                .status()
                                .map(|s| s.as_str().to_string())
                                .unwrap_or_default(),
                            created: r.created_time().map(|d| fmt_epoch_secs(d.secs())),
                            updated: r.updated_time().map(|d| fmt_epoch_secs(d.secs())),
                            artifact_id: r.provisioning_artifact_id().unwrap_or_default().to_string(),
                            errors: r
                                .record_errors()
                                .iter()
                                .map(|e| {
                                    format!(
                                        "{}: {}",
                                        e.code().unwrap_or_default(),
                                        e.description().unwrap_or_default()
                                    )
                                })
                                .collect(),
                        },
                    ));
                }
                token = next_page_token(page.next_page_token(), &token);
                if token.is_none() {
                    break;
                }
            }
            Err(e) => return Err(crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e))),
        }
    }
    // Newest first; a cap that dropped the tail would drop the failure you
    // came to read.
    records.sort_by_key(|(secs, _)| std::cmp::Reverse(*secs));
    records.truncate(MAX_PP_RECORDS);
    Ok(records.into_iter().map(|(_, r)| r).collect())
}

// ── Timestamp helpers ─────────────────────────────────────────────────────────

fn fmt_epoch_secs(secs: i64) -> String {
    let days = secs / 86400;
    let time_rem = secs % 86400;
    let h = time_rem / 3600;
    let m = (time_rem % 3600) / 60;
    let (y, mo, d) = epoch_days_to_ymd(days);
    format!("{:04}-{:02}-{:02} {:02}:{:02} UTC", y, mo, d, h, m)
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
    for &d in &dm {
        if days < d as i64 {
            break;
        }
        days -= d as i64;
        month += 1;
    }
    (year, month, (days + 1) as u8)
}

fn is_leap(y: i32) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s3(url: &str) -> Option<(String, String, Option<String>)> {
        parse_s3_https_url(url)
    }

    #[test]
    fn parses_virtual_hosted_s3_urls() {
        assert_eq!(
            s3("https://my-bucket.s3.us-east-1.amazonaws.com/templates/app.yaml"),
            Some(("my-bucket".into(), "templates/app.yaml".into(), Some("us-east-1".into())))
        );
        assert_eq!(
            s3("https://my-bucket.s3.amazonaws.com/app.json"),
            Some(("my-bucket".into(), "app.json".into(), None))
        );
        // Legacy dashed region + a bucket name containing ".s3".
        assert_eq!(
            s3("https://my.s3.stuff.s3-eu-west-1.amazonaws.com/k"),
            Some(("my.s3.stuff".into(), "k".into(), Some("eu-west-1".into())))
        );
    }

    #[test]
    fn parses_path_style_s3_urls() {
        assert_eq!(
            s3("https://s3.amazonaws.com/my-bucket/templates/app.yaml"),
            Some(("my-bucket".into(), "templates/app.yaml".into(), None))
        );
        assert_eq!(
            s3("https://s3.eu-central-1.amazonaws.com/b/k?versionId=abc"),
            Some(("b".into(), "k".into(), Some("eu-central-1".into())))
        );
    }

    #[test]
    fn decodes_percent_escapes_in_keys() {
        assert_eq!(
            s3("https://b.s3.us-west-2.amazonaws.com/dir/my%20template.yaml"),
            Some(("b".into(), "dir/my template.yaml".into(), Some("us-west-2".into())))
        );
    }

    #[test]
    fn rejects_non_s3_hosts_and_presigned_urls_stay_http() {
        assert_eq!(s3("https://example.com/bucket/key"), None);
        assert_eq!(s3("https://servicecatalog.us-east-1.amazonaws.com/x"), None);
        match classify_template_url(
            "https://b.s3.us-east-1.amazonaws.com/k?X-Amz-Signature=abc&X-Amz-Credential=x",
        ) {
            ScTemplateSource::Http(_) => {}
            ScTemplateSource::S3 { .. } => panic!("presigned URL must stay on the HTTP path"),
        }
        match classify_template_url("https://b.s3.us-east-1.amazonaws.com/k") {
            ScTemplateSource::S3 { bucket, key, region } => {
                assert_eq!(bucket, "b");
                assert_eq!(key, "k");
                assert_eq!(region.as_deref(), Some("us-east-1"));
            }
            ScTemplateSource::Http(_) => panic!("raw S3 URL must use authenticated GetObject"),
        }
    }
}
