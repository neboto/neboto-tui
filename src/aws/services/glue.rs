use crate::aws::client::AwsClients;
use crate::aws::resource::{native_state_label, Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::error::Result;
use crate::event::{Event, LoadProgress};
use async_trait::async_trait;
use aws_sdk_glue::Client as GlueClient;
use std::any::Any;
use std::collections::HashMap;
use tokio::sync::mpsc;

/// AWS Glue — five sub-tabs (Databases / Tables / Crawlers / Jobs / Job Runs),
/// one `ServiceType`, browse-only. Each type streams as its own batch and is
/// **error-tolerant**: a permission gap on any one API records a status message
/// and moves on rather than blanking the whole service.
///
/// - **Databases** — `GetDatabases` (name, location URI, description,
///   parameters). Flat details.
/// - **Tables** — first-class, fuzzy-searchable. Streamed eagerly via an N+1
///   `GetTables` per database (capped at [`MAX_TABLES`] total so a big catalog
///   doesn't stall the load). Split pane Overview / Schema / Storage — columns,
///   partition keys, SerDe, and the S3 `location` (jumpable).
/// - **Crawlers** — `GetCrawlers` (state, schedule, last-crawl status/error,
///   targets, output database). Split pane Overview / Targets / Configuration.
/// - **Jobs** — `GetJobs` (role, glue version, worker type, command/script,
///   default arguments). Split pane Overview / Command / Arguments.
/// - **Job Runs** — an N+1 `GetJobRuns` per job (capped, most-recent-first —
///   same shape as the ECS stopped-task window): state, duration, DPU-hours,
///   error. Split pane Overview / Arguments; `t` tails its CloudWatch log group.
pub struct GlueService {
    client: GlueClient,
}

/// Job runs shown per job — a representative recent window, not the full
/// history (browse-only). Matches the ECS stopped-task window shape.
const MAX_JOB_RUNS_PER_JOB: i32 = 20;
/// Soft ceiling on eagerly-loaded tables so a catalog with tens of thousands of
/// tables doesn't turn the initial load into a marathon. When hit, the load
/// stops adding tables and records how many databases went unscanned.
const MAX_TABLES: usize = 2000;

impl GlueService {
    pub fn new(aws_clients: &AwsClients) -> Self {
        Self {
            client: aws_clients.glue_client(),
        }
    }
}

#[async_trait]
impl AwsService for GlueService {
    fn service_type(&self) -> ServiceType {
        ServiceType::Glue
    }

    fn name(&self) -> &str {
        "Glue"
    }

    async fn list_resources(&self) -> Result<Vec<Box<dyn Resource>>> {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        self.list_resources_streaming(tx, ServiceType::Glue).await?;
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
            warning: format!("Glue {}: {}", stage, e),
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

        // ── Phase 1: Databases ────────────────────────────────────────────────
        // Collect the names so the Tables phase can iterate them.
        let mut db_names: Vec<String> = Vec::new();
        let mut databases: Vec<GlueDatabase> = Vec::new();
        let mut paginator = self.client.get_databases().into_paginator().send();
        while let Some(page) = paginator.next().await {
            match page {
                Ok(p) => {
                    for d in p.database_list() {
                        db_names.push(d.name().to_string());
                        databases.push(GlueDatabase::from_sdk(d));
                    }
                }
                Err(e) => {
                    let _ = event_tx.send(err("databases", &crate::error::sdk_error_message(&e)));
                    break;
                }
            }
        }
        if !databases.is_empty() {
            total += databases.len();
            let batch: Vec<Box<dyn Resource>> = databases
                .into_iter()
                .map(|d| Box::new(d) as Box<dyn Resource>)
                .collect();
            emit(batch, total, Some("Loading tables…"), false);
        }

        // ── Phase 2: Tables (per database, capped) ────────────────────────────
        let mut tables: Vec<GlueTable> = Vec::new();
        let mut truncated_dbs = 0usize;
        'dbs: for db in &db_names {
            if tables.len() >= MAX_TABLES {
                truncated_dbs += 1;
                continue;
            }
            let mut t_paginator = self
                .client
                .get_tables()
                .database_name(db)
                .into_paginator()
                .send();
            loop {
                match t_paginator.next().await {
                    Some(Ok(p)) => {
                        for t in p.table_list() {
                            tables.push(GlueTable::from_sdk(db, t));
                            if tables.len() >= MAX_TABLES {
                                // Stop this database mid-stream; the outer loop
                                // will count the rest as unscanned.
                                continue 'dbs;
                            }
                        }
                    }
                    // A federated / Lake Formation-restricted database may deny
                    // GetTables — skip it, keep the rest.
                    Some(Err(_)) => break,
                    None => break,
                }
            }
        }
        if !tables.is_empty() {
            total += tables.len();
            let status = if truncated_dbs > 0 {
                format!(
                    "Loading crawlers… ({} tables cap hit, {} databases unscanned)",
                    MAX_TABLES, truncated_dbs
                )
            } else {
                "Loading crawlers…".to_string()
            };
            let batch: Vec<Box<dyn Resource>> = tables
                .into_iter()
                .map(|t| Box::new(t) as Box<dyn Resource>)
                .collect();
            emit(batch, total, Some(&status), false);
        }

        // ── Phase 3: Crawlers ─────────────────────────────────────────────────
        let mut crawlers: Vec<GlueCrawler> = Vec::new();
        let mut c_paginator = self.client.get_crawlers().into_paginator().send();
        loop {
            match c_paginator.next().await {
                Some(Ok(p)) => {
                    for c in p.crawlers() {
                        crawlers.push(GlueCrawler::from_sdk(c));
                    }
                }
                Some(Err(e)) => {
                    let _ = event_tx.send(err("crawlers", &crate::error::sdk_error_message(&e)));
                    break;
                }
                None => break,
            }
        }
        if !crawlers.is_empty() {
            total += crawlers.len();
            let batch: Vec<Box<dyn Resource>> = crawlers
                .into_iter()
                .map(|c| Box::new(c) as Box<dyn Resource>)
                .collect();
            emit(batch, total, Some("Loading jobs…"), false);
        }

        // ── Phase 4: Jobs ─────────────────────────────────────────────────────
        let mut job_names: Vec<String> = Vec::new();
        let mut jobs: Vec<GlueJob> = Vec::new();
        let mut j_paginator = self.client.get_jobs().into_paginator().send();
        loop {
            match j_paginator.next().await {
                Some(Ok(p)) => {
                    for j in p.jobs() {
                        if let Some(n) = j.name() {
                            job_names.push(n.to_string());
                        }
                        jobs.push(GlueJob::from_sdk(j));
                    }
                }
                Some(Err(e)) => {
                    let _ = event_tx.send(err("jobs", &crate::error::sdk_error_message(&e)));
                    break;
                }
                None => break,
            }
        }
        if !jobs.is_empty() {
            total += jobs.len();
            let batch: Vec<Box<dyn Resource>> = jobs
                .into_iter()
                .map(|j| Box::new(j) as Box<dyn Resource>)
                .collect();
            emit(batch, total, Some("Loading job runs…"), false);
        }

        // ── Phase 5: Job Runs (per job, capped, most-recent-first) ────────────
        let mut runs: Vec<GlueJobRun> = Vec::new();
        for job in &job_names {
            match self
                .client
                .get_job_runs()
                .job_name(job)
                .max_results(MAX_JOB_RUNS_PER_JOB)
                .send()
                .await
            {
                Ok(resp) => {
                    for r in resp.job_runs() {
                        runs.push(GlueJobRun::from_sdk(r));
                    }
                }
                Err(_) => continue,
            }
        }
        if !runs.is_empty() {
            // Most-recent-first across all jobs.
            runs.sort_by(|a, b| b.started_epoch.cmp(&a.started_epoch));
            total += runs.len();
            let batch: Vec<Box<dyn Resource>> = runs
                .into_iter()
                .map(|r| Box::new(r) as Box<dyn Resource>)
                .collect();
            emit(batch, total, None, true);
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

// ── Helpers ──────────────────────────────────────────────────────────────────

fn fmt_epoch(dt: Option<&aws_smithy_types::DateTime>) -> Option<String> {
    dt.map(|t| crate::aws::services::cloudwatch::fmt_epoch_secs(t.secs()))
}

// ── Databases ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct GlueDatabase {
    pub name: String,
    pub description: Option<String>,
    pub location_uri: Option<String>,
    pub catalog_id: Option<String>,
    pub created: Option<String>,
    pub parameters: Vec<(String, String)>,
    pub tags: HashMap<String, String>,
}

impl GlueDatabase {
    pub fn from_sdk(d: &aws_sdk_glue::types::Database) -> Self {
        let mut parameters: Vec<(String, String)> = d
            .parameters()
            .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default();
        parameters.sort_by(|a, b| a.0.cmp(&b.0));
        Self {
            name: d.name().to_string(),
            description: d.description().map(|s| s.to_string()),
            location_uri: d.location_uri().map(|s| s.to_string()),
            catalog_id: d.catalog_id().map(|s| s.to_string()),
            created: fmt_epoch(d.create_time()),
            parameters,
            tags: HashMap::new(),
        }
    }
}

impl Resource for GlueDatabase {
    fn id(&self) -> &str {
        &self.name
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn resource_type(&self) -> &str {
        "Glue Database"
    }
    fn state(&self) -> ResourceState {
        ResourceState::stateless()
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        format!(
            "{} {} {} database glue catalog",
            self.name,
            self.description.as_deref().unwrap_or(""),
            self.location_uri.as_deref().unwrap_or("")
        )
    }
    fn details(&self) -> Vec<(String, String)> {
        let mut rows = vec![("Name".to_string(), self.name.clone())];
        if let Some(d) = &self.description {
            rows.push(("Description".to_string(), d.clone()));
        }
        if let Some(l) = &self.location_uri {
            rows.push(("Location".to_string(), l.clone()));
        }
        if let Some(c) = &self.catalog_id {
            rows.push(("Catalog ID".to_string(), c.clone()));
        }
        if let Some(c) = &self.created {
            rows.push(("Created".to_string(), c.clone()));
        }
        if !self.parameters.is_empty() {
            rows.push((String::new(), String::new()));
            rows.push(("Parameters".to_string(), String::new())); // group header
            for (k, v) in &self.parameters {
                rows.push((format!("  {}", k), v.clone()));
            }
        }
        rows
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/glue/home?region={}#/v2/data-catalog/databases/view/{}",
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

// ── Tables ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct GlueTable {
    pub database: String,
    pub name: String,
    pub description: Option<String>,
    pub owner: Option<String>,
    pub table_type: Option<String>,
    pub created: Option<String>,
    pub updated: Option<String>,
    pub columns: Vec<(String, String, Option<String>)>, // (name, type, comment)
    pub partition_keys: Vec<(String, String)>,          // (name, type)
    pub location: Option<String>,
    pub input_format: Option<String>,
    pub output_format: Option<String>,
    pub serde_library: Option<String>,
    pub compressed: bool,
    pub storage_parameters: Vec<(String, String)>,
    pub tags: HashMap<String, String>,
}

impl GlueTable {
    pub fn from_sdk(database: &str, t: &aws_sdk_glue::types::Table) -> Self {
        let sd = t.storage_descriptor();
        let columns = sd
            .map(|s| {
                s.columns()
                    .iter()
                    .map(|c| {
                        (
                            c.name().to_string(),
                            c.r#type().unwrap_or("").to_string(),
                            c.comment().map(|s| s.to_string()),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        let mut storage_parameters: Vec<(String, String)> = sd
            .and_then(|s| s.serde_info())
            .and_then(|si| si.parameters())
            .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default();
        storage_parameters.sort_by(|a, b| a.0.cmp(&b.0));
        Self {
            database: database.to_string(),
            name: t.name().to_string(),
            description: t.description().map(|s| s.to_string()),
            owner: t.owner().map(|s| s.to_string()),
            table_type: t.table_type().map(|s| s.to_string()),
            created: fmt_epoch(t.create_time()),
            updated: fmt_epoch(t.update_time()),
            columns,
            partition_keys: t
                .partition_keys()
                .iter()
                .map(|c| (c.name().to_string(), c.r#type().unwrap_or("").to_string()))
                .collect(),
            location: sd.and_then(|s| s.location()).map(|s| s.to_string()),
            input_format: sd.and_then(|s| s.input_format()).map(|s| s.to_string()),
            output_format: sd.and_then(|s| s.output_format()).map(|s| s.to_string()),
            serde_library: sd
                .and_then(|s| s.serde_info())
                .and_then(|si| si.serialization_library())
                .map(|s| s.to_string()),
            compressed: sd.map(|s| s.compressed()).unwrap_or(false),
            storage_parameters,
            tags: HashMap::new(),
        }
    }
}

crate::sections! {
    pub enum GlueTableDetailSection,
    pub static GLUE_TABLE_SECTIONS = [
        Overview "Overview",
        Schema "Schema",
        Storage "Storage",
    ]
}

impl Resource for GlueTable {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&GLUE_TABLE_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.name
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn resource_type(&self) -> &str {
        "Glue Table"
    }
    fn state(&self) -> ResourceState {
        ResourceState::stateless()
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        format!(
            "{} {} {} {} table glue",
            self.name,
            self.database,
            self.table_type.as_deref().unwrap_or(""),
            self.location.as_deref().unwrap_or("")
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
            "https://{}.console.aws.amazon.com/glue/home?region={}#/v2/data-catalog/tables/view/{}?database={}",
            region, region, self.name, self.database
        ))
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── Crawlers ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct GlueCrawler {
    pub name: String,
    pub state: String, // READY / RUNNING / STOPPING
    pub database_name: Option<String>,
    pub role: Option<String>,
    pub description: Option<String>,
    pub table_prefix: Option<String>,
    pub schedule_expression: Option<String>,
    pub schedule_state: Option<String>,
    pub last_crawl_status: Option<String>,
    pub last_crawl_error: Option<String>,
    pub last_crawl_start: Option<String>,
    pub recrawl_behavior: Option<String>,
    pub update_behavior: Option<String>,
    pub delete_behavior: Option<String>,
    pub classifiers: Vec<String>,
    pub security_configuration: Option<String>,
    pub created: Option<String>,
    pub last_updated: Option<String>,
    /// Pre-flattened target rows (label, value) — S3 paths render as `s3://…`
    /// so the generic jump classifier makes them jumpable.
    pub target_rows: Vec<(String, String)>,
    pub tags: HashMap<String, String>,
}

impl GlueCrawler {
    pub fn from_sdk(c: &aws_sdk_glue::types::Crawler) -> Self {
        let mut target_rows: Vec<(String, String)> = Vec::new();
        if let Some(t) = c.targets() {
            for s3 in t.s3_targets() {
                if let Some(p) = s3.path() {
                    target_rows.push(("  S3 Path".to_string(), p.to_string()));
                }
            }
            for j in t.jdbc_targets() {
                if let Some(p) = j.path() {
                    let conn = j.connection_name().unwrap_or("");
                    let v = if conn.is_empty() {
                        p.to_string()
                    } else {
                        format!("{} ({})", p, conn)
                    };
                    target_rows.push(("  JDBC Path".to_string(), v));
                }
            }
            for d in t.dynamo_db_targets() {
                if let Some(p) = d.path() {
                    target_rows.push(("  DynamoDB Path".to_string(), p.to_string()));
                }
            }
            for cat in t.catalog_targets() {
                let tables = cat.tables().join(", ");
                target_rows.push((
                    "  Catalog".to_string(),
                    format!("{} / {}", cat.database_name(), tables),
                ));
            }
            for d in t.delta_targets() {
                for tbl in d.delta_tables() {
                    target_rows.push(("  Delta Table".to_string(), tbl.clone()));
                }
            }
            for i in t.iceberg_targets() {
                for p in i.paths() {
                    target_rows.push(("  Iceberg Path".to_string(), p.clone()));
                }
            }
            for m in t.mongo_db_targets() {
                if let Some(p) = m.path() {
                    target_rows.push(("  MongoDB Path".to_string(), p.to_string()));
                }
            }
        }
        let last_crawl = c.last_crawl();
        Self {
            name: c.name().unwrap_or_default().to_string(),
            state: c
                .state()
                .map(|s| s.as_str().to_string())
                .unwrap_or_else(|| "UNKNOWN".to_string()),
            database_name: c.database_name().map(|s| s.to_string()),
            role: c.role().map(|s| s.to_string()),
            description: c.description().map(|s| s.to_string()),
            table_prefix: c.table_prefix().filter(|s| !s.is_empty()).map(|s| s.to_string()),
            schedule_expression: c
                .schedule()
                .and_then(|s| s.schedule_expression())
                .map(|s| s.to_string()),
            schedule_state: c
                .schedule()
                .and_then(|s| s.state())
                .map(|s| s.as_str().to_string()),
            last_crawl_status: last_crawl
                .and_then(|l| l.status())
                .map(|s| s.as_str().to_string()),
            last_crawl_error: last_crawl
                .and_then(|l| l.error_message())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string()),
            last_crawl_start: last_crawl.and_then(|l| fmt_epoch(l.start_time())),
            recrawl_behavior: c
                .recrawl_policy()
                .and_then(|r| r.recrawl_behavior())
                .map(|b| b.as_str().to_string()),
            update_behavior: c
                .schema_change_policy()
                .and_then(|s| s.update_behavior())
                .map(|b| b.as_str().to_string()),
            delete_behavior: c
                .schema_change_policy()
                .and_then(|s| s.delete_behavior())
                .map(|b| b.as_str().to_string()),
            classifiers: c.classifiers().to_vec(),
            security_configuration: c
                .crawler_security_configuration()
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string()),
            created: fmt_epoch(c.creation_time()),
            last_updated: fmt_epoch(c.last_updated()),
            target_rows,
            tags: HashMap::new(),
        }
    }
}

crate::sections! {
    pub enum GlueCrawlerDetailSection,
    pub static GLUE_CRAWLER_SECTIONS = [
        Overview "Overview",
        Targets "Targets",
        Configuration "Configuration",
    ]
}

impl Resource for GlueCrawler {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&GLUE_CRAWLER_SECTIONS)
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws glue get-crawler --name {}",
            crate::aws::resource::shell_quote(self.id())
        ))
    }

    fn id(&self) -> &str {
        &self.name
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn resource_type(&self) -> &str {
        "Glue Crawler"
    }
    fn state(&self) -> ResourceState {
        match self.state.as_str() {
            "READY" => ResourceState::Available,
            "RUNNING" => ResourceState::Pending,
            "STOPPING" => ResourceState::Pending,
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
            "{} {} {} crawler glue",
            self.name,
            self.state,
            self.database_name.as_deref().unwrap_or("")
        )
    }
    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Name".to_string(), self.name.clone()),
            ("State".to_string(), self.state.clone()),
        ]
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/glue/home?region={}#/v2/data-catalog/crawlers/view/{}",
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

// ── Jobs ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct GlueJob {
    pub name: String,
    pub description: Option<String>,
    pub role: Option<String>,
    pub glue_version: Option<String>,
    pub command_name: Option<String>, // glueetl / pythonshell / gluestreaming
    pub script_location: Option<String>,
    pub python_version: Option<String>,
    pub runtime: Option<String>,
    pub worker_type: Option<String>,
    pub number_of_workers: Option<i32>,
    pub max_capacity: Option<f64>,
    pub timeout_min: Option<i32>,
    pub max_retries: i32,
    pub max_concurrent_runs: Option<i32>,
    pub execution_class: Option<String>,
    pub connections: Vec<String>,
    pub default_arguments: Vec<(String, String)>,
    pub created: Option<String>,
    pub last_modified: Option<String>,
    pub tags: HashMap<String, String>,
}

impl GlueJob {
    pub fn from_sdk(j: &aws_sdk_glue::types::Job) -> Self {
        let cmd = j.command();
        let mut default_arguments: Vec<(String, String)> = j
            .default_arguments()
            .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default();
        default_arguments.sort_by(|a, b| a.0.cmp(&b.0));
        Self {
            name: j.name().unwrap_or_default().to_string(),
            description: j.description().map(|s| s.to_string()),
            role: j.role().map(|s| s.to_string()),
            glue_version: j.glue_version().map(|s| s.to_string()),
            command_name: cmd.and_then(|c| c.name()).map(|s| s.to_string()),
            script_location: cmd.and_then(|c| c.script_location()).map(|s| s.to_string()),
            python_version: cmd.and_then(|c| c.python_version()).map(|s| s.to_string()),
            runtime: cmd.and_then(|c| c.runtime()).map(|s| s.to_string()),
            worker_type: j.worker_type().map(|w| w.as_str().to_string()),
            number_of_workers: j.number_of_workers(),
            max_capacity: j.max_capacity(),
            timeout_min: j.timeout(),
            max_retries: j.max_retries(),
            max_concurrent_runs: j.execution_property().map(|e| e.max_concurrent_runs()),
            execution_class: j.execution_class().map(|e| e.as_str().to_string()),
            connections: j
                .connections()
                .map(|c| c.connections().to_vec())
                .unwrap_or_default(),
            default_arguments,
            created: fmt_epoch(j.created_on()),
            last_modified: fmt_epoch(j.last_modified_on()),
            tags: HashMap::new(),
        }
    }
}

crate::sections! {
    pub enum GlueJobDetailSection,
    pub static GLUE_JOB_SECTIONS = [
        Overview "Overview",
        Command "Command",
        Arguments "Arguments",
    ]
}

impl Resource for GlueJob {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&GLUE_JOB_SECTIONS)
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws glue get-job --job-name {}",
            crate::aws::resource::shell_quote(self.id())
        ))
    }

    fn id(&self) -> &str {
        &self.name
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn resource_type(&self) -> &str {
        "Glue Job"
    }
    fn state(&self) -> ResourceState {
        ResourceState::stateless()
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        format!(
            "{} {} {} job glue etl",
            self.name,
            self.command_name.as_deref().unwrap_or(""),
            self.glue_version.as_deref().unwrap_or("")
        )
    }
    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Name".to_string(), self.name.clone()),
            (
                "Type".to_string(),
                self.command_name.clone().unwrap_or_default(),
            ),
        ]
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/glue/home?region={}#/v2/etl-configuration/jobs/view/{}",
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

// ── Job Runs ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct GlueJobRun {
    pub id: String,
    pub job_name: String,
    pub list_name: String, // "{job}  ·  {started}" for the list row
    pub state: String,     // STARTING / RUNNING / SUCCEEDED / FAILED / TIMEOUT / STOPPED
    pub started: Option<String>,
    pub started_epoch: i64,
    pub completed: Option<String>,
    pub execution_time_secs: i32,
    pub dpu_seconds: Option<f64>,
    pub max_capacity: Option<f64>,
    pub worker_type: Option<String>,
    pub number_of_workers: Option<i32>,
    pub glue_version: Option<String>,
    pub execution_class: Option<String>,
    pub attempt: i32,
    pub trigger_name: Option<String>,
    pub error_message: Option<String>,
    /// The run's CloudWatch log group (`/aws-glue/jobs/output` unless overridden);
    /// the log *stream* is the run id.
    pub log_group_name: Option<String>,
    pub arguments: Vec<(String, String)>,
    pub tags: HashMap<String, String>,
}

impl GlueJobRun {
    pub fn from_sdk(r: &aws_sdk_glue::types::JobRun) -> Self {
        let job_name = r.job_name().unwrap_or_default().to_string();
        let started = fmt_epoch(r.started_on());
        let mut arguments: Vec<(String, String)> = r
            .arguments()
            .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default();
        arguments.sort_by(|a, b| a.0.cmp(&b.0));
        let list_name = match &started {
            Some(s) => format!("{}  ·  {}", job_name, s),
            None => job_name.clone(),
        };
        Self {
            id: r.id().unwrap_or_default().to_string(),
            job_name,
            list_name,
            state: r
                .job_run_state()
                .map(|s| s.as_str().to_string())
                .unwrap_or_else(|| "UNKNOWN".to_string()),
            started,
            started_epoch: r.started_on().map(|t| t.secs()).unwrap_or(0),
            completed: fmt_epoch(r.completed_on()),
            execution_time_secs: r.execution_time(),
            dpu_seconds: r.dpu_seconds(),
            max_capacity: r.max_capacity(),
            worker_type: r.worker_type().map(|w| w.as_str().to_string()),
            number_of_workers: r.number_of_workers(),
            glue_version: r.glue_version().map(|s| s.to_string()),
            execution_class: r.execution_class().map(|e| e.as_str().to_string()),
            attempt: r.attempt(),
            trigger_name: r.trigger_name().filter(|s| !s.is_empty()).map(|s| s.to_string()),
            error_message: r.error_message().filter(|s| !s.is_empty()).map(|s| s.to_string()),
            log_group_name: r.log_group_name().map(|s| s.to_string()),
            arguments,
            tags: HashMap::new(),
        }
    }

    /// DPU-hours consumed (the cost proxy), when the run reports `DPUSeconds`.
    pub fn dpu_hours(&self) -> Option<f64> {
        self.dpu_seconds.map(|s| s / 3600.0)
    }

    /// The CloudWatch log group to tail — the run's own group, else the Glue
    /// default. The log *stream* is the run id.
    pub fn log_group(&self) -> String {
        self.log_group_name
            .clone()
            .unwrap_or_else(|| "/aws-glue/jobs/output".to_string())
    }
}

crate::sections! {
    pub enum GlueJobRunDetailSection,
    pub static GLUE_JOB_RUN_SECTIONS = [
        Overview "Overview",
        Arguments "Arguments",
    ]
}

impl Resource for GlueJobRun {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&GLUE_JOB_RUN_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.id
    }
    fn name(&self) -> &str {
        &self.list_name
    }
    fn resource_type(&self) -> &str {
        "Glue Job Run"
    }
    fn state(&self) -> ResourceState {
        match self.state.as_str() {
            "SUCCEEDED" => ResourceState::Available,
            "STARTING" | "RUNNING" | "STOPPING" | "WAITING" => ResourceState::Pending,
            "FAILED" | "ERROR" | "TIMEOUT" => ResourceState::Unavailable,
            "STOPPED" => ResourceState::Stopped,
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
            "{} {} {} job run glue",
            self.job_name, self.state, self.id
        )
    }
    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Job".to_string(), self.job_name.clone()),
            ("State".to_string(), self.state.clone()),
            ("Run ID".to_string(), self.id.clone()),
        ]
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/glue/home?region={}#/v2/etl-configuration/jobs/view/{}/runs/{}",
            region, region, self.job_name, self.id
        ))
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dpu_hours_from_seconds() {
        let mut run = GlueJobRun {
            id: "jr_1".into(),
            job_name: "etl".into(),
            list_name: "etl".into(),
            state: "SUCCEEDED".into(),
            started: None,
            started_epoch: 0,
            completed: None,
            execution_time_secs: 0,
            dpu_seconds: Some(7200.0),
            max_capacity: None,
            worker_type: None,
            number_of_workers: None,
            glue_version: None,
            execution_class: None,
            attempt: 0,
            trigger_name: None,
            error_message: None,
            log_group_name: None,
            arguments: Vec::new(),
            tags: HashMap::new(),
        };
        assert_eq!(run.dpu_hours(), Some(2.0));
        assert_eq!(run.log_group(), "/aws-glue/jobs/output");
        run.log_group_name = Some("/custom".into());
        assert_eq!(run.log_group(), "/custom");
    }
}

// ── Job metrics (`m`) ─────────────────────────────────────────────────────────

pub use crate::aws::services::ec2::{parse_metric_datapoints, MetricsTimeRange};

/// `Glue`-namespace per-job metrics (note: no `AWS/` prefix). Published only
/// when the job has **job metrics enabled** (`--enable-metrics`); everything
/// comes back empty otherwise — the overlay's no-data hint covers that.
/// Aggregate driver metrics use dims JobName / JobRunId=ALL / Type=count and
/// are cumulative within a run.
#[derive(Debug, Clone)]
pub struct GlueJobMetricsData {
    pub time_range: MetricsTimeRange,
    pub bytes_read: Vec<(f64, f64)>,
    pub records_read: Vec<(f64, f64)>,
    pub elapsed_ms: Vec<(f64, f64)>,
    pub completed_tasks: Vec<(f64, f64)>,
    pub failed_tasks: Vec<(f64, f64)>,
    pub completed_stages: Vec<(f64, f64)>,
    pub x_max: f64,
}

#[derive(Debug, Clone)]
pub enum GlueJobMetricsState {
    Loading,
    Loaded(GlueJobMetricsData),
}

pub async fn fetch_glue_job_metrics(
    cw: aws_sdk_cloudwatch::Client,
    job_name: String,
    time_range: MetricsTimeRange,
) -> crate::error::Result<GlueJobMetricsData> {
    use aws_sdk_cloudwatch::types::{Dimension, Statistic};

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let start = now - time_range.duration_secs();
    let period = time_range.period_secs();
    let start_dt = aws_sdk_cloudwatch::primitives::DateTime::from_secs(start);
    let end_dt = aws_sdk_cloudwatch::primitives::DateTime::from_secs(now);

    let dim = |name: &'static str, value: String| {
        Dimension::builder().name(name).value(value).build()
    };
    let metric = |name: &'static str| {
        cw.get_metric_statistics()
            .namespace("Glue")
            .metric_name(name)
            .dimensions(dim("JobName", job_name.clone()))
            .dimensions(dim("JobRunId", "ALL".to_string()))
            .dimensions(dim("Type", "count".to_string()))
            .start_time(start_dt)
            .end_time(end_dt)
            .period(period)
            .set_statistics(Some(vec![Statistic::Maximum]))
            .send()
    };

    let (bytes, records, elapsed, completed, failed, stages) = tokio::join!(
        metric("glue.driver.aggregate.bytesRead"),
        metric("glue.driver.aggregate.recordsRead"),
        metric("glue.driver.aggregate.elapsedTime"),
        metric("glue.driver.aggregate.numCompletedTasks"),
        metric("glue.driver.aggregate.numFailedTasks"),
        metric("glue.driver.aggregate.numCompletedStages"),
    );

    Ok(GlueJobMetricsData {
        time_range,
        bytes_read: parse_metric_datapoints(bytes, start),
        records_read: parse_metric_datapoints(records, start),
        elapsed_ms: parse_metric_datapoints(elapsed, start),
        completed_tasks: parse_metric_datapoints(completed, start),
        failed_tasks: parse_metric_datapoints(failed, start),
        completed_stages: parse_metric_datapoints(stages, start),
        x_max: time_range.duration_secs() as f64,
    })
}
