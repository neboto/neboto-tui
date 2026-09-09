use crate::aws::client::AwsClients;
use crate::aws::resource::{native_state_label, Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::error::Result;
use crate::event::{Event, LoadProgress};
use async_trait::async_trait;
use aws_sdk_athena::Client as AthenaClient;
use futures::stream::{self, StreamExt};
use std::any::Any;
use std::collections::HashMap;
use tokio::sync::mpsc;

/// Athena service — five sub-tabs (Workgroups / Data Catalogs / Databases /
/// Recent Queries / Saved Queries), one `ServiceType`, browse-only. Each type
/// streams as its own batch and is **error-tolerant**: a permission gap or a
/// federated-catalog failure records a status message and moves on rather than
/// blanking the whole service.
///
/// - **Workgroups** — `ListWorkGroups` then an N+1 `GetWorkGroup` per group
///   (result location, encryption, engine version, bytes-scanned cutoff,
///   enforce/publish/requester-pays). Split pane Overview / Configuration /
///   Tags(lazy `ListTagsForResource`).
/// - **Data Catalogs** — `ListDataCatalogs` (GLUE / HIVE / LAMBDA / FEDERATED).
///   Flat details.
/// - **Databases** — `ListDatabases` per discovered catalog, keyed
///   `catalog/db`. Split pane Overview / Tables(lazy `ListTableMetadata` —
///   columns, partition keys, table type).
/// - **Recent Queries** — `ListQueryExecutions` per workgroup (capped) →
///   `BatchGetQueryExecution` (chunks of 50): status, runtime, **data scanned**
///   (the cost proxy), the SQL, and a per-phase statistics breakdown.
/// - **Saved Queries** — `ListNamedQueries` per workgroup →
///   `BatchGetNamedQuery`: name, database, workgroup, and the SQL.
pub struct AthenaService {
    client: AthenaClient,
}

/// Per-workgroup caps so a busy account doesn't turn the load into thousands of
/// `BatchGet*` round-trips. Recent/Saved show a representative window, not the
/// full history (which is browse-only anyway).
const MAX_QUERIES_PER_WG: i32 = 50;
const MAX_NAMED_PER_WG: i32 = 50;
/// AWS `BatchGet*` hard limit.
const BATCH: usize = 50;

impl AthenaService {
    pub fn new(aws_clients: &AwsClients) -> Self {
        Self {
            client: aws_clients.athena_client(),
        }
    }
}

#[async_trait]
impl AwsService for AthenaService {
    fn service_type(&self) -> ServiceType {
        ServiceType::Athena
    }

    fn name(&self) -> &str {
        "Athena"
    }

    async fn list_resources(&self) -> Result<Vec<Box<dyn Resource>>> {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        self.list_resources_streaming(tx, ServiceType::Athena).await?;
        Ok(vec![])
    }

    async fn list_resources_streaming(
        &self,
        event_tx: mpsc::UnboundedSender<Event>,
        service_type: ServiceType,
    ) -> Result<()> {
        let mut total = 0usize;

        // Phase failures are non-fatal: warn and keep streaming the rest
        // (a fatal ResourceLoadError would make the app drop later batches).
        let err = |stage: &str, e: &dyn std::fmt::Display| Event::ResourceLoadWarning {
            service: service_type,
            warning: format!("Athena {}: {}", stage, e),
        };
        let emit = |resources: Vec<Box<dyn Resource>>,
                    loaded: usize,
                    status: Option<&str>,
                    done: bool| {
            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                service: service_type,
                resources,
                progress: LoadProgress {
                    loaded_count: loaded,
                    total_count: if done { Some(loaded) } else { None },
                    status_message: status.map(|s| s.to_string()),
                },
            });
        };

        // ── Phase 1: Workgroups ───────────────────────────────────────────────
        // ListWorkGroups → GetWorkGroup per group (full config). Collect the
        // names for the query phases below.
        let mut wg_names: Vec<String> = Vec::new();
        let mut paginator = self.client.list_work_groups().into_paginator().send();
        while let Some(page) = paginator.next().await {
            match page {
                Ok(p) => {
                    for s in p.work_groups() {
                        if let Some(n) = s.name() {
                            wg_names.push(n.to_string());
                        }
                    }
                }
                Err(e) => {
                    let _ = event_tx.send(err("workgroups", &crate::error::sdk_error_message(&e)));
                    break;
                }
            }
        }

        if !wg_names.is_empty() {
            let workgroups: Vec<AthenaWorkgroup> = stream::iter(wg_names.clone())
                .map(|name| {
                    let client = self.client.clone();
                    async move {
                        client
                            .get_work_group()
                            .work_group(&name)
                            .send()
                            .await
                            .ok()
                            .and_then(|r| r.work_group().cloned())
                            .map(|wg| AthenaWorkgroup::from_sdk(&wg))
                    }
                })
                .buffer_unordered(8)
                .filter_map(|w| async move { w })
                .collect()
                .await;

            total += workgroups.len();
            let batch: Vec<Box<dyn Resource>> = workgroups
                .into_iter()
                .map(|w| Box::new(w) as Box<dyn Resource>)
                .collect();
            emit(batch, total, Some("Loading data catalogs…"), false);
        }

        // ── Phase 2: Data Catalogs ────────────────────────────────────────────
        let mut catalog_names: Vec<String> = Vec::new();
        let mut catalogs: Vec<AthenaDataCatalog> = Vec::new();
        let mut cat_paginator = self.client.list_data_catalogs().into_paginator().send();
        loop {
            match cat_paginator.next().await {
                Some(Ok(p)) => {
                    for s in p.data_catalogs_summary() {
                        let cat = AthenaDataCatalog::from_summary(s);
                        catalog_names.push(cat.name.clone());
                        catalogs.push(cat);
                    }
                }
                Some(Err(e)) => {
                    let _ = event_tx.send(err("data catalogs", &crate::error::sdk_error_message(&e)));
                    break;
                }
                None => break,
            }
        }
        // Always have the default Glue catalog available for the databases
        // phase even if ListDataCatalogs was denied.
        if catalog_names.is_empty() {
            catalog_names.push("AwsDataCatalog".to_string());
        }
        if !catalogs.is_empty() {
            total += catalogs.len();
            let batch: Vec<Box<dyn Resource>> = catalogs
                .into_iter()
                .map(|c| Box::new(c) as Box<dyn Resource>)
                .collect();
            emit(batch, total, Some("Loading databases…"), false);
        }

        // ── Phase 3: Databases (per catalog) ──────────────────────────────────
        let mut databases: Vec<AthenaDatabase> = Vec::new();
        for catalog in &catalog_names {
            let mut db_paginator = self
                .client
                .list_databases()
                .catalog_name(catalog)
                .into_paginator()
                .send();
            loop {
                match db_paginator.next().await {
                    Some(Ok(p)) => {
                        for d in p.database_list() {
                            databases.push(AthenaDatabase::from_sdk(catalog, d));
                        }
                    }
                    // A federated / Lambda catalog often can't be listed without
                    // its connector — skip it, keep the rest.
                    Some(Err(_)) => break,
                    None => break,
                }
            }
        }
        if !databases.is_empty() {
            total += databases.len();
            let batch: Vec<Box<dyn Resource>> = databases
                .into_iter()
                .map(|d| Box::new(d) as Box<dyn Resource>)
                .collect();
            emit(batch, total, Some("Loading recent queries…"), false);
        }

        // ── Phase 4: Recent Queries (per workgroup) ───────────────────────────
        // ListQueryExecutions (capped) → BatchGetQueryExecution in chunks of 50.
        let mut query_ids: Vec<String> = Vec::new();
        for wg in &wg_names {
            match self
                .client
                .list_query_executions()
                .work_group(wg)
                .max_results(MAX_QUERIES_PER_WG)
                .send()
                .await
            {
                Ok(resp) => query_ids.extend(resp.query_execution_ids().iter().cloned()),
                Err(_) => continue,
            }
        }
        if !query_ids.is_empty() {
            let mut executions: Vec<AthenaQueryExecution> = Vec::new();
            for chunk in query_ids.chunks(BATCH) {
                if let Ok(resp) = self
                    .client
                    .batch_get_query_execution()
                    .set_query_execution_ids(Some(chunk.to_vec()))
                    .send()
                    .await
                {
                    for qe in resp.query_executions() {
                        executions.push(AthenaQueryExecution::from_sdk(qe));
                    }
                }
            }
            // Most-recent-first (by submission time).
            executions.sort_by(|a, b| b.submitted_epoch.cmp(&a.submitted_epoch));
            if !executions.is_empty() {
                total += executions.len();
                let batch: Vec<Box<dyn Resource>> = executions
                    .into_iter()
                    .map(|q| Box::new(q) as Box<dyn Resource>)
                    .collect();
                emit(batch, total, Some("Loading saved queries…"), false);
            }
        }

        // ── Phase 5: Saved Queries (per workgroup) ────────────────────────────
        let mut named_ids: Vec<String> = Vec::new();
        for wg in &wg_names {
            match self
                .client
                .list_named_queries()
                .work_group(wg)
                .max_results(MAX_NAMED_PER_WG)
                .send()
                .await
            {
                Ok(resp) => named_ids.extend(resp.named_query_ids().iter().cloned()),
                Err(_) => continue,
            }
        }
        if !named_ids.is_empty() {
            let mut named: Vec<AthenaNamedQuery> = Vec::new();
            for chunk in named_ids.chunks(BATCH) {
                if let Ok(resp) = self
                    .client
                    .batch_get_named_query()
                    .set_named_query_ids(Some(chunk.to_vec()))
                    .send()
                    .await
                {
                    for nq in resp.named_queries() {
                        named.push(AthenaNamedQuery::from_sdk(nq));
                    }
                }
            }
            named.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
            if !named.is_empty() {
                total += named.len();
                let batch: Vec<Box<dyn Resource>> = named
                    .into_iter()
                    .map(|q| Box::new(q) as Box<dyn Resource>)
                    .collect();
                emit(batch, total, None, true);
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

// ── Formatting helpers ──────────────────────────────────────────────────────

/// Human-readable byte count — the data-scanned "cost proxy" for a query.
pub fn fmt_bytes(bytes: i64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if bytes <= 0 {
        return "0 B".to_string();
    }
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{} {}", bytes, UNITS[u])
    } else {
        format!("{:.2} {}", v, UNITS[u])
    }
}

/// Milliseconds → a compact `1.2 s` / `3 m 4 s` / `250 ms` duration.
pub fn fmt_millis(ms: i64) -> String {
    if ms < 1000 {
        return format!("{} ms", ms);
    }
    let secs = ms / 1000;
    if secs < 60 {
        return format!("{:.1} s", ms as f64 / 1000.0);
    }
    let m = secs / 60;
    let s = secs % 60;
    if m < 60 {
        format!("{} m {} s", m, s)
    } else {
        format!("{} h {} m", m / 60, m % 60)
    }
}

// ── Workgroups ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct AthenaWorkgroup {
    pub name: String,
    pub state: String, // ENABLED / DISABLED
    pub description: Option<String>,
    pub created: Option<String>,
    pub effective_engine: Option<String>,
    pub selected_engine: Option<String>,
    // Configuration
    pub output_location: Option<String>,
    pub encryption_option: Option<String>,
    pub kms_key: Option<String>,
    pub expected_bucket_owner: Option<String>,
    pub enforce_config: bool,
    pub publish_cw_metrics: bool,
    pub requester_pays: bool,
    pub bytes_scanned_cutoff: Option<i64>,
    pub execution_role: Option<String>,
    pub additional_config: Option<String>,
    pub tags: HashMap<String, String>,
}

impl AthenaWorkgroup {
    pub fn from_sdk(wg: &aws_sdk_athena::types::WorkGroup) -> Self {
        let cfg = wg.configuration();
        let result = cfg.and_then(|c| c.result_configuration());
        let enc = result.and_then(|r| r.encryption_configuration());
        let engine = cfg.and_then(|c| c.engine_version());
        Self {
            name: wg.name().to_string(),
            state: wg
                .state()
                .map(|s| s.as_str().to_string())
                .unwrap_or_else(|| "UNKNOWN".to_string()),
            description: wg.description().map(|s| s.to_string()),
            created: wg
                .creation_time()
                .map(|t| crate::aws::services::cloudwatch::fmt_epoch_secs(t.secs())),
            effective_engine: engine
                .and_then(|e| e.effective_engine_version())
                .map(|s| s.to_string()),
            selected_engine: engine
                .and_then(|e| e.selected_engine_version())
                .map(|s| s.to_string()),
            output_location: result.and_then(|r| r.output_location()).map(|s| s.to_string()),
            encryption_option: enc.map(|e| e.encryption_option().as_str().to_string()),
            kms_key: enc.and_then(|e| e.kms_key()).map(|s| s.to_string()),
            expected_bucket_owner: result
                .and_then(|r| r.expected_bucket_owner())
                .map(|s| s.to_string()),
            enforce_config: cfg
                .and_then(|c| c.enforce_work_group_configuration())
                .unwrap_or(false),
            publish_cw_metrics: cfg
                .and_then(|c| c.publish_cloud_watch_metrics_enabled())
                .unwrap_or(false),
            requester_pays: cfg.and_then(|c| c.requester_pays_enabled()).unwrap_or(false),
            bytes_scanned_cutoff: cfg.and_then(|c| c.bytes_scanned_cutoff_per_query()),
            execution_role: cfg.and_then(|c| c.execution_role()).map(|s| s.to_string()),
            additional_config: cfg
                .and_then(|c| c.additional_configuration())
                .map(|s| s.to_string()),
            tags: HashMap::new(),
        }
    }
}

crate::sections! {
    pub enum AthenaWorkgroupDetailSection,
    pub static ATHENA_WORKGROUP_SECTIONS = [
        Overview "Overview",
        Configuration "Configuration",
        Tags "Tags" => crate::app::App::trigger_athena_workgroup_tags_load,
    ]
}

impl Resource for AthenaWorkgroup {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&ATHENA_WORKGROUP_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.name
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn resource_type(&self) -> &str {
        "Athena Workgroup"
    }
    fn state(&self) -> ResourceState {
        match self.state.as_str() {
            "ENABLED" => ResourceState::Available,
            "DISABLED" => ResourceState::Stopped,
            other => ResourceState::Unknown(other.to_string()),
        }
    }

    fn state_label(&self) -> String {
        native_state_label(&self.state, || self.state())
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        format!(
            "{} {} {} workgroup athena",
            self.name,
            self.state,
            self.effective_engine.as_deref().unwrap_or("")
        )
    }
    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Name".to_string(), self.name.clone()),
            ("State".to_string(), self.state.clone()),
            (
                "Engine".to_string(),
                self.effective_engine.clone().unwrap_or_default(),
            ),
        ]
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/athena/home?region={}#/workgroups/details/{}",
            region, region, self.name
        ))
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// `ListTagsForResource` on a workgroup ARN (built by the caller from account +
/// region, since the SDK returns no ARN on the summary).
pub async fn fetch_athena_workgroup_tags(
    client: AthenaClient,
    resource_arn: String,
) -> Result<Vec<(String, String)>> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut paginator = client
        .list_tags_for_resource()
        .resource_arn(&resource_arn)
        .into_paginator()
        .send();
    while let Some(page) = paginator.next().await {
        let page = page.map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
        for t in page.tags() {
            out.push((
                t.key().unwrap_or_default().to_string(),
                t.value().unwrap_or_default().to_string(),
            ));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

// ── Data Catalogs ───────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct AthenaDataCatalog {
    pub name: String,
    pub catalog_type: String, // GLUE / HIVE / LAMBDA / FEDERATED
    pub connection_type: Option<String>,
    pub status: Option<String>,
    pub error: Option<String>,
}

impl AthenaDataCatalog {
    pub fn from_summary(s: &aws_sdk_athena::types::DataCatalogSummary) -> Self {
        Self {
            name: s.catalog_name().unwrap_or_default().to_string(),
            catalog_type: s
                .r#type()
                .map(|t| t.as_str().to_string())
                .unwrap_or_else(|| "—".to_string()),
            connection_type: s.connection_type().map(|c| c.as_str().to_string()),
            status: s.status().map(|st| st.as_str().to_string()),
            error: s.error().map(|e| e.to_string()),
        }
    }
}

impl Resource for AthenaDataCatalog {
    fn id(&self) -> &str {
        &self.name
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn resource_type(&self) -> &str {
        "Athena Data Catalog"
    }
    fn state(&self) -> ResourceState {
        match self.status.as_deref() {
            None => ResourceState::Available,
            Some(s) if s.contains("COMPLETE") => ResourceState::Available,
            Some(s) if s.contains("FAILED") => ResourceState::Unavailable,
            Some(s) if s.contains("DELETE") => ResourceState::Deleting,
            Some(s) if s.contains("IN_PROGRESS") => ResourceState::Creating,
            Some(other) => ResourceState::Unknown(other.to_string()),
        }
    }

    fn state_label(&self) -> String {
        native_state_label(self.status.as_deref().unwrap_or(""), || self.state())
    }
    fn tags(&self) -> &HashMap<String, String> {
        static EMPTY: std::sync::OnceLock<HashMap<String, String>> = std::sync::OnceLock::new();
        EMPTY.get_or_init(HashMap::new)
    }
    fn search_text(&self) -> String {
        format!("{} {} data catalog athena", self.name, self.catalog_type)
    }
    fn details(&self) -> Vec<(String, String)> {
        let mut rows = vec![
            ("Name".to_string(), self.name.clone()),
            ("Type".to_string(), self.catalog_type.clone()),
        ];
        if let Some(c) = &self.connection_type {
            rows.push(("Connection Type".to_string(), c.clone()));
        }
        if let Some(s) = &self.status {
            rows.push(("Status".to_string(), s.clone()));
        }
        if let Some(e) = &self.error {
            rows.push(("Error".to_string(), e.clone()));
        }
        rows
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/athena/home?region={}#/data-sources/{}",
            region, region, self.name
        ))
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── Databases ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct AthenaDatabase {
    pub catalog: String,
    pub db_name: String,
    pub description: Option<String>,
    pub parameters: Vec<(String, String)>,
    pub tags: HashMap<String, String>,
}

impl AthenaDatabase {
    pub fn from_sdk(catalog: &str, d: &aws_sdk_athena::types::Database) -> Self {
        let mut parameters: Vec<(String, String)> = d
            .parameters()
            .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default();
        parameters.sort_by(|a, b| a.0.cmp(&b.0));
        Self {
            catalog: catalog.to_string(),
            db_name: d.name().to_string(),
            description: d.description().map(|s| s.to_string()),
            parameters,
            tags: HashMap::new(),
        }
    }

    /// `catalog/db` — unique across catalogs; also the tables-cache key.
    pub fn key(&self) -> String {
        format!("{}/{}", self.catalog, self.db_name)
    }
}

crate::sections! {
    pub enum AthenaDatabaseDetailSection,
    pub static ATHENA_DATABASE_SECTIONS = [
        Overview "Overview",
        Tables "Tables" => crate::app::App::trigger_athena_tables_load,
    ]
}

impl Resource for AthenaDatabase {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&ATHENA_DATABASE_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.db_name
    }
    fn name(&self) -> &str {
        &self.db_name
    }
    fn resource_type(&self) -> &str {
        "Athena Database"
    }
    fn state(&self) -> ResourceState {
        ResourceState::stateless()
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        format!(
            "{} {} {} database athena",
            self.db_name,
            self.catalog,
            self.description.as_deref().unwrap_or("")
        )
    }
    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Name".to_string(), self.db_name.clone()),
            ("Catalog".to_string(), self.catalog.clone()),
        ]
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/athena/home?region={}#/query-editor",
            region, region
        ))
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// One table under a database — the lazy Tables section rows.
#[derive(Debug, Clone)]
pub struct AthenaTable {
    pub name: String,
    pub table_type: Option<String>,
    pub columns: Vec<(String, String)>,        // (name, type)
    pub partition_keys: Vec<(String, String)>, // (name, type)
    pub created: Option<String>,
    pub location: Option<String>,
}

/// `ListTableMetadata` for one database — columns, partition keys, table type,
/// and the S3 `location` parameter (jumpable when it's an `s3://` URI).
pub async fn fetch_athena_tables(
    client: AthenaClient,
    catalog: String,
    database: String,
) -> Result<Vec<AthenaTable>> {
    let mut out: Vec<AthenaTable> = Vec::new();
    let mut paginator = client
        .list_table_metadata()
        .catalog_name(&catalog)
        .database_name(&database)
        .into_paginator()
        .send();
    while let Some(page) = paginator.next().await {
        let page = page.map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
        for t in page.table_metadata_list() {
            let location = t
                .parameters()
                .and_then(|p| p.get("location"))
                .map(|s| s.to_string());
            out.push(AthenaTable {
                name: t.name().to_string(),
                table_type: t.table_type().map(|s| s.to_string()),
                columns: t
                    .columns()
                    .iter()
                    .map(|c| (c.name().to_string(), c.r#type().unwrap_or("").to_string()))
                    .collect(),
                partition_keys: t
                    .partition_keys()
                    .iter()
                    .map(|c| (c.name().to_string(), c.r#type().unwrap_or("").to_string()))
                    .collect(),
                created: t
                    .create_time()
                    .map(|dt| crate::aws::services::cloudwatch::fmt_epoch_secs(dt.secs())),
                location,
            });
        }
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(out)
}

// ── Recent Queries (query executions) ───────────────────────────────────────

#[derive(Debug, Clone)]
pub struct AthenaQueryExecution {
    pub id: String,
    pub query: String,
    pub statement_type: Option<String>,
    pub state: String, // QUEUED / RUNNING / SUCCEEDED / FAILED / CANCELLED
    pub state_reason: Option<String>,
    pub error_message: Option<String>,
    pub workgroup: Option<String>,
    pub database: Option<String>,
    pub catalog: Option<String>,
    pub engine_version: Option<String>,
    pub output_location: Option<String>,
    pub submitted: Option<String>,
    pub submitted_epoch: i64,
    pub completed: Option<String>,
    // Statistics
    pub data_scanned: Option<i64>,
    pub total_time_ms: Option<i64>,
    pub engine_time_ms: Option<i64>,
    pub queue_time_ms: Option<i64>,
    pub planning_time_ms: Option<i64>,
    pub service_processing_ms: Option<i64>,
    pub result_reused: bool,
}

impl AthenaQueryExecution {
    pub fn from_sdk(qe: &aws_sdk_athena::types::QueryExecution) -> Self {
        let status = qe.status();
        let stats = qe.statistics();
        let ctx = qe.query_execution_context();
        let result_reused = stats
            .and_then(|s| s.result_reuse_information())
            .map(|r| r.reused_previous_result())
            .unwrap_or(false);
        Self {
            id: qe.query_execution_id().unwrap_or_default().to_string(),
            query: qe.query().unwrap_or_default().to_string(),
            statement_type: qe.statement_type().map(|s| s.as_str().to_string()),
            state: status
                .and_then(|s| s.state())
                .map(|s| s.as_str().to_string())
                .unwrap_or_else(|| "UNKNOWN".to_string()),
            state_reason: status.and_then(|s| s.state_change_reason()).map(|s| s.to_string()),
            error_message: status
                .and_then(|s| s.athena_error())
                .and_then(|e| e.error_message())
                .map(|s| s.to_string()),
            workgroup: qe.work_group().map(|s| s.to_string()),
            database: ctx.and_then(|c| c.database()).map(|s| s.to_string()),
            catalog: ctx.and_then(|c| c.catalog()).map(|s| s.to_string()),
            engine_version: qe
                .engine_version()
                .and_then(|e| e.effective_engine_version())
                .map(|s| s.to_string()),
            output_location: qe
                .result_configuration()
                .and_then(|r| r.output_location())
                .map(|s| s.to_string()),
            submitted: status
                .and_then(|s| s.submission_date_time())
                .map(|t| crate::aws::services::cloudwatch::fmt_epoch_secs(t.secs())),
            submitted_epoch: status
                .and_then(|s| s.submission_date_time())
                .map(|t| t.secs())
                .unwrap_or(0),
            completed: status
                .and_then(|s| s.completion_date_time())
                .map(|t| crate::aws::services::cloudwatch::fmt_epoch_secs(t.secs())),
            data_scanned: stats.and_then(|s| s.data_scanned_in_bytes()),
            total_time_ms: stats.and_then(|s| s.total_execution_time_in_millis()),
            engine_time_ms: stats.and_then(|s| s.engine_execution_time_in_millis()),
            queue_time_ms: stats.and_then(|s| s.query_queue_time_in_millis()),
            planning_time_ms: stats.and_then(|s| s.query_planning_time_in_millis()),
            service_processing_ms: stats.and_then(|s| s.service_processing_time_in_millis()),
            result_reused,
        }
    }

    /// First line of the SQL, trimmed — the list-row summary.
    pub fn query_preview(&self) -> String {
        let one_line = self.query.split_whitespace().collect::<Vec<_>>().join(" ");
        if one_line.len() > 80 {
            format!("{}…", &one_line[..80])
        } else {
            one_line
        }
    }
}

crate::sections! {
    pub enum AthenaQueryDetailSection,
    pub static ATHENA_QUERY_SECTIONS = [
        Overview "Overview",
        Query "Query",
        Statistics "Statistics",
    ]
}

impl Resource for AthenaQueryExecution {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&ATHENA_QUERY_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.id
    }
    fn name(&self) -> &str {
        &self.id
    }
    fn resource_type(&self) -> &str {
        "Athena Query"
    }
    fn state(&self) -> ResourceState {
        match self.state.as_str() {
            "SUCCEEDED" => ResourceState::Available,
            "RUNNING" | "QUEUED" => ResourceState::Pending,
            "FAILED" => ResourceState::Unavailable,
            "CANCELLED" => ResourceState::Stopped,
            other => ResourceState::Unknown(other.to_string()),
        }
    }

    fn state_label(&self) -> String {
        native_state_label(&self.state, || self.state())
    }
    fn tags(&self) -> &HashMap<String, String> {
        static EMPTY: std::sync::OnceLock<HashMap<String, String>> = std::sync::OnceLock::new();
        EMPTY.get_or_init(HashMap::new)
    }
    fn search_text(&self) -> String {
        format!(
            "{} {} {} {} query athena",
            self.id,
            self.state,
            self.workgroup.as_deref().unwrap_or(""),
            self.query_preview()
        )
    }
    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Query ID".to_string(), self.id.clone()),
            ("State".to_string(), self.state.clone()),
            (
                "Data Scanned".to_string(),
                self.data_scanned.map(fmt_bytes).unwrap_or_default(),
            ),
        ]
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/athena/home?region={}#/query-editor/history/{}",
            region, region, self.id
        ))
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── Saved Queries (named queries) ───────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct AthenaNamedQuery {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub database: String,
    pub workgroup: Option<String>,
    pub query: String,
}

impl AthenaNamedQuery {
    pub fn from_sdk(nq: &aws_sdk_athena::types::NamedQuery) -> Self {
        Self {
            id: nq.named_query_id().unwrap_or_default().to_string(),
            name: nq.name().to_string(),
            description: nq.description().map(|s| s.to_string()),
            database: nq.database().to_string(),
            workgroup: nq.work_group().map(|s| s.to_string()),
            query: nq.query_string().to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_formatting() {
        assert_eq!(fmt_bytes(0), "0 B");
        assert_eq!(fmt_bytes(512), "512 B");
        assert_eq!(fmt_bytes(1024), "1.00 KB");
        assert_eq!(fmt_bytes(1024 * 1024), "1.00 MB");
        assert_eq!(fmt_bytes(3 * 1024 * 1024 * 1024), "3.00 GB");
    }

    #[test]
    fn millis_formatting() {
        assert_eq!(fmt_millis(250), "250 ms");
        assert_eq!(fmt_millis(1500), "1.5 s");
        assert_eq!(fmt_millis(65_000), "1 m 5 s");
        assert_eq!(fmt_millis(3_600_000), "1 h 0 m");
    }
}

crate::sections! {
    pub enum AthenaSavedQueryDetailSection,
    pub static ATHENA_SAVED_QUERY_SECTIONS = [
        Overview "Overview",
        Query "Query",
    ]
}

impl Resource for AthenaNamedQuery {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&ATHENA_SAVED_QUERY_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.id
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn resource_type(&self) -> &str {
        "Athena Saved Query"
    }
    fn state(&self) -> ResourceState {
        ResourceState::stateless()
    }
    fn tags(&self) -> &HashMap<String, String> {
        static EMPTY: std::sync::OnceLock<HashMap<String, String>> = std::sync::OnceLock::new();
        EMPTY.get_or_init(HashMap::new)
    }
    fn search_text(&self) -> String {
        format!(
            "{} {} {} saved query named athena",
            self.name,
            self.database,
            self.description.as_deref().unwrap_or("")
        )
    }
    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Name".to_string(), self.name.clone()),
            ("Database".to_string(), self.database.clone()),
        ]
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/athena/home?region={}#/query-editor/saved/{}",
            region, region, self.id
        ))
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── Workgroup metrics (`m`) ───────────────────────────────────────────────────

pub use crate::aws::services::ec2::{parse_metric_datapoints, MetricsTimeRange};

/// `AWS/Athena` per-workgroup metrics. Time values are milliseconds; DPU
/// consumption only exists for provisioned-capacity workgroups (empty
/// otherwise). Workgroups can disable metric publishing
/// (`PublishCloudWatchMetricsEnabled`) — everything comes back empty then.
#[derive(Debug, Clone)]
pub struct AthenaWgMetricsData {
    pub time_range: MetricsTimeRange,
    pub processed_bytes: Vec<(f64, f64)>,
    pub total_exec_ms: Vec<(f64, f64)>,
    pub engine_exec_ms: Vec<(f64, f64)>,
    pub queue_ms: Vec<(f64, f64)>,
    pub planning_ms: Vec<(f64, f64)>,
    pub dpu_consumed: Vec<(f64, f64)>,
    pub x_max: f64,
}

#[derive(Debug, Clone)]
pub enum AthenaWgMetricsState {
    Loading,
    Loaded(AthenaWgMetricsData),
}

pub async fn fetch_athena_wg_metrics(
    cw: aws_sdk_cloudwatch::Client,
    workgroup: String,
    time_range: MetricsTimeRange,
) -> crate::error::Result<AthenaWgMetricsData> {
    use aws_sdk_cloudwatch::types::{Dimension, Statistic};

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let start = now - time_range.duration_secs();
    let period = time_range.period_secs();
    let start_dt = aws_sdk_cloudwatch::primitives::DateTime::from_secs(start);
    let end_dt = aws_sdk_cloudwatch::primitives::DateTime::from_secs(now);

    let metric = |name: &'static str, stat: Statistic| {
        cw.get_metric_statistics()
            .namespace("AWS/Athena")
            .metric_name(name)
            .dimensions(Dimension::builder().name("WorkGroup").value(&workgroup).build())
            .start_time(start_dt)
            .end_time(end_dt)
            .period(period)
            .set_statistics(Some(vec![stat]))
            .send()
    };

    let (bytes, total, engine, queue, planning, dpu) = tokio::join!(
        metric("ProcessedBytes", Statistic::Sum),
        metric("TotalExecutionTime", Statistic::Average),
        metric("EngineExecutionTime", Statistic::Average),
        metric("QueryQueueTime", Statistic::Average),
        metric("QueryPlanningTime", Statistic::Average),
        metric("DPUConsumed", Statistic::Sum),
    );

    Ok(AthenaWgMetricsData {
        time_range,
        processed_bytes: parse_metric_datapoints(bytes, start),
        total_exec_ms: parse_metric_datapoints(total, start),
        engine_exec_ms: parse_metric_datapoints(engine, start),
        queue_ms: parse_metric_datapoints(queue, start),
        planning_ms: parse_metric_datapoints(planning, start),
        dpu_consumed: parse_metric_datapoints(dpu, start),
        x_max: time_range.duration_secs() as f64,
    })
}
