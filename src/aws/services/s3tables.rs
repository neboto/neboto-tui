use crate::aws::client::AwsClients;
use crate::aws::resource::{shell_quote, Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::error::Result;
use crate::event::{Event, LoadProgress};
use async_trait::async_trait;
use aws_sdk_s3tables::Client as S3TablesClient;
use std::any::Any;
use std::collections::HashMap;
use tokio::sync::mpsc;

const MAX_TABLE_BUCKETS: usize = 100;
const MAX_TABLES_PER_BUCKET: usize = 200;
const MAX_TABLES_TOTAL: usize = 500;
const MAX_NAMESPACES: usize = 100;

/// Amazon S3 Tables — table buckets holding Apache Iceberg tables, with
/// managed maintenance (compaction, snapshot expiry, unreferenced-file
/// removal). Two resource types: table buckets (`ListTableBuckets`) and the
/// tables inside them (per-bucket `ListTables`). Read-only: no
/// create/delete/put/rename APIs called.
pub struct S3TablesService {
    client: S3TablesClient,
}

impl S3TablesService {
    pub fn new(aws_clients: &AwsClients) -> Self {
        Self {
            client: aws_clients.s3tables_client(),
        }
    }
}

#[async_trait]
impl AwsService for S3TablesService {
    fn service_type(&self) -> ServiceType {
        ServiceType::S3Tables
    }

    fn name(&self) -> &str {
        "S3 Tables"
    }

    async fn list_resources(&self) -> Result<Vec<Box<dyn Resource>>> {
        Ok(vec![])
    }

    async fn list_resources_streaming(
        &self,
        event_tx: mpsc::UnboundedSender<Event>,
        service_type: ServiceType,
    ) -> Result<()> {
        let mut total = 0usize;

        // Phase 1 — table buckets. A failure here is total (nothing else can
        // list without the bucket ARNs).
        let mut buckets: Vec<S3TableBucket> = Vec::new();
        let mut pager = self.client.list_table_buckets().into_paginator().send();
        loop {
            match pager.next().await {
                Some(Ok(page)) => {
                    let batch: Vec<S3TableBucket> = page
                        .table_buckets()
                        .iter()
                        .map(S3TableBucket::from_sdk)
                        .collect();
                    if batch.is_empty() {
                        continue;
                    }
                    buckets.extend(batch.iter().cloned());
                    total += batch.len();
                    let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                        service: service_type,
                        resources: batch
                            .into_iter()
                            .map(|b| Box::new(b) as Box<dyn Resource>)
                            .collect(),
                        progress: LoadProgress {
                            loaded_count: total,
                            total_count: None,
                            status_message: None,
                        },
                    });
                    if buckets.len() >= MAX_TABLE_BUCKETS {
                        break;
                    }
                }
                Some(Err(e)) => {
                    let _ = event_tx.send(Event::ResourceLoadError {
                        service: service_type,
                        error: crate::error::sdk_error_message(&e),
                    });
                    return Ok(());
                }
                None => break,
            }
        }

        // Phase 2 — the tables inside each bucket. A per-bucket failure warns
        // and keeps going (never a mid-stream ResourceLoadError).
        let mut tables_total = 0usize;
        for bucket in &buckets {
            match fetch_tables_for_bucket(&self.client, &bucket.arn, &bucket.name).await {
                Ok(tables) => {
                    if tables.is_empty() {
                        continue;
                    }
                    tables_total += tables.len();
                    total += tables.len();
                    let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                        service: service_type,
                        resources: tables
                            .into_iter()
                            .map(|t| Box::new(t) as Box<dyn Resource>)
                            .collect(),
                        progress: LoadProgress {
                            loaded_count: total,
                            total_count: None,
                            status_message: None,
                        },
                    });
                    if tables_total >= MAX_TABLES_TOTAL {
                        let _ = event_tx.send(Event::ResourceLoadWarning {
                            service: service_type,
                            warning: format!("tables capped at {}", MAX_TABLES_TOTAL),
                        });
                        break;
                    }
                }
                Err(e) => {
                    let _ = event_tx.send(Event::ResourceLoadWarning {
                        service: service_type,
                        warning: format!("tables in {}: {}", bucket.name, e),
                    });
                }
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

/// All tables in one bucket, every namespace, capped.
async fn fetch_tables_for_bucket(
    client: &S3TablesClient,
    bucket_arn: &str,
    bucket_name: &str,
) -> Result<Vec<S3Table>> {
    let mut out = Vec::new();
    let mut pager = client
        .list_tables()
        .table_bucket_arn(bucket_arn)
        .into_paginator()
        .send();
    loop {
        match pager.next().await {
            Some(Ok(page)) => {
                for t in page.tables() {
                    out.push(S3Table::from_sdk(t, bucket_arn, bucket_name));
                    if out.len() >= MAX_TABLES_PER_BUCKET {
                        return Ok(out);
                    }
                }
            }
            Some(Err(e)) => {
                return Err(crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))
            }
            None => break,
        }
    }
    Ok(out)
}

// ── Table bucket ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct S3TableBucket {
    /// The ARN doubles as the id — table-bucket APIs are ARN-keyed.
    pub arn: String,
    pub name: String,
    pub owner_account_id: String,
    /// "customer" or "aws" (a bucket managed by another AWS service).
    pub bucket_type: String,
    pub created: String,
    // Tags aren't in the list output — the Tags section fetches them via
    // ListTagsForResource (`lazy.s3tables_bucket_extras`), so this stays
    // empty (the app-wide `tag:` filter won't see them; documented).
    pub tags: HashMap<String, String>,
    search_blob: String,
}

impl S3TableBucket {
    pub fn from_sdk(b: &aws_sdk_s3tables::types::TableBucketSummary) -> Self {
        let name = b.name().to_string();
        let bucket_type = b
            .r#type()
            .map(|t| t.as_str().to_string())
            .unwrap_or_else(|| "customer".to_string());
        let search_blob = format!("{} {} {}", name, b.owner_account_id(), bucket_type);
        Self {
            arn: b.arn().to_string(),
            name,
            owner_account_id: b.owner_account_id().to_string(),
            bucket_type,
            created: fmt_epoch_secs(b.created_at().secs()),
            tags: HashMap::new(),
            search_blob,
        }
    }
}

crate::sections! {
    pub enum S3TableBucketDetailSection,
    pub static S3TABLE_BUCKET_SECTIONS = [
        Details "Details" => crate::app::App::trigger_s3tables_bucket_extras_load,
        Namespaces "Namespaces" => crate::app::App::trigger_s3tables_namespaces_load,
        Maintenance "Maintenance" => crate::app::App::trigger_s3tables_bucket_maintenance_load,
        Policy "Policy" => crate::app::App::trigger_s3tables_bucket_policy_load,
        Tags "Tags" => crate::app::App::trigger_s3tables_bucket_extras_load,
    ]
}

impl Resource for S3TableBucket {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&S3TABLE_BUCKET_SECTIONS)
    }

    fn id(&self) -> &str {
        &self.arn
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn resource_type(&self) -> &str {
        "S3 Table Bucket"
    }

    fn state(&self) -> ResourceState {
        ResourceState::stateless()
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        self.search_blob.clone()
    }

    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws s3tables get-table-bucket --table-bucket-arn {}",
            shell_quote(&self.arn)
        ))
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Name".to_string(), self.name.clone()),
            ("Type".to_string(), self.bucket_type.clone()),
            ("Owner Account".to_string(), self.owner_account_id.clone()),
            ("Created".to_string(), self.created.clone()),
            ("ARN".to_string(), self.arn.clone()),
        ]
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── Table ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct S3Table {
    /// The table ARN (`…:bucket/<name>/table/<uuid>`) doubles as the id.
    pub arn: String,
    /// `namespace.table` — the name a query engine uses.
    pub full_name: String,
    pub table_name: String,
    pub namespace: String,
    /// "customer" or "aws" (created/managed by another AWS service).
    pub table_type: String,
    /// The managing service's principal for aws-type tables ("" otherwise).
    pub managed_by: String,
    pub bucket_arn: String,
    pub bucket_name: String,
    pub created: String,
    pub modified: String,
    // Tags come from the lazy ListTagsForResource fetch (Details/Tags), so
    // this stays empty like the bucket's.
    pub tags: HashMap<String, String>,
    search_blob: String,
}

impl S3Table {
    pub fn from_sdk(
        t: &aws_sdk_s3tables::types::TableSummary,
        bucket_arn: &str,
        bucket_name: &str,
    ) -> Self {
        let table_name = t.name().to_string();
        let namespace = t.namespace().join(".");
        let full_name = if namespace.is_empty() {
            table_name.clone()
        } else {
            format!("{}.{}", namespace, table_name)
        };
        let table_type = t.r#type().as_str().to_string();
        let managed_by = t.managed_by_service().unwrap_or_default().to_string();
        let search_blob = format!(
            "{} {} {} {} {}",
            full_name, bucket_name, table_type, managed_by, t.table_arn()
        );
        Self {
            arn: t.table_arn().to_string(),
            full_name,
            table_name,
            namespace,
            table_type,
            managed_by,
            bucket_arn: bucket_arn.to_string(),
            bucket_name: bucket_name.to_string(),
            created: fmt_epoch_secs(t.created_at().secs()),
            modified: fmt_epoch_secs(t.modified_at().secs()),
            tags: HashMap::new(),
            search_blob,
        }
    }
}

crate::sections! {
    pub enum S3TableDetailSection,
    pub static S3TABLE_SECTIONS = [
        Details "Details" => crate::app::App::trigger_s3tables_table_extras_load,
        Maintenance "Maintenance" => crate::app::App::trigger_s3tables_table_maintenance_load,
        Policy "Policy" => crate::app::App::trigger_s3tables_table_policy_load,
        Tags "Tags" => crate::app::App::trigger_s3tables_table_extras_load,
    ]
}

impl Resource for S3Table {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&S3TABLE_SECTIONS)
    }

    fn id(&self) -> &str {
        &self.arn
    }

    fn name(&self) -> &str {
        &self.full_name
    }

    fn resource_type(&self) -> &str {
        "S3 Table"
    }

    fn state(&self) -> ResourceState {
        ResourceState::stateless()
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        self.search_blob.clone()
    }

    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws s3tables get-table --table-arn {}",
            shell_quote(&self.arn)
        ))
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Table".to_string(), self.full_name.clone()),
            ("Bucket".to_string(), self.bucket_name.clone()),
            ("Type".to_string(), self.table_type.clone()),
            ("Created".to_string(), self.created.clone()),
            ("Modified".to_string(), self.modified.clone()),
            ("ARN".to_string(), self.arn.clone()),
        ]
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── Lazy: bucket extras (encryption + tags) ──────────────────────────────────

#[derive(Debug, Clone)]
pub struct S3TableBucketExtras {
    /// `(algorithm, kms_key_arn)`; `None` = no bucket-level encryption
    /// configuration (SSE-S3 default applies).
    pub encryption: Option<(String, String)>,
    pub tags: Vec<(String, String)>,
}

pub async fn fetch_s3tables_bucket_extras(
    client: S3TablesClient,
    arn: String,
) -> Result<S3TableBucketExtras> {
    let encryption = match client
        .get_table_bucket_encryption()
        .table_bucket_arn(&arn)
        .send()
        .await
    {
        Ok(resp) => resp.encryption_configuration().map(|c| {
            (
                c.sse_algorithm().as_str().to_string(),
                c.kms_key_arn().unwrap_or_default().to_string(),
            )
        }),
        Err(e) => {
            // No explicit config 404s — that's the SSE-S3 default, not an error.
            if e.as_service_error().is_some_and(|se| se.is_not_found_exception()) {
                None
            } else {
                return Err(crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)));
            }
        }
    };
    let tags = fetch_s3tables_tags(&client, &arn).await?;
    Ok(S3TableBucketExtras { encryption, tags })
}

/// Sorted `(key, value)` tags for any s3tables ARN.
async fn fetch_s3tables_tags(
    client: &S3TablesClient,
    arn: &str,
) -> Result<Vec<(String, String)>> {
    let resp = client
        .list_tags_for_resource()
        .resource_arn(arn)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
    let mut tags: Vec<(String, String)> = resp
        .tags()
        .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default();
    tags.sort();
    Ok(tags)
}

// ── Lazy: namespaces ─────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct S3TablesNamespace {
    pub name: String,
    pub created: String,
    pub created_by: String,
    pub capped: bool,
}

pub async fn fetch_s3tables_namespaces(
    client: S3TablesClient,
    bucket_arn: String,
) -> Result<Vec<S3TablesNamespace>> {
    let mut out = Vec::new();
    let mut capped = false;
    let mut pager = client
        .list_namespaces()
        .table_bucket_arn(&bucket_arn)
        .into_paginator()
        .send();
    'outer: loop {
        match pager.next().await {
            Some(Ok(page)) => {
                for n in page.namespaces() {
                    out.push(S3TablesNamespace {
                        name: n.namespace().join("."),
                        created: fmt_epoch_secs(n.created_at().secs()),
                        created_by: n.created_by().to_string(),
                        capped: false,
                    });
                    if out.len() >= MAX_NAMESPACES {
                        capped = true;
                        break 'outer;
                    }
                }
            }
            Some(Err(e)) => {
                return Err(crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))
            }
            None => break,
        }
    }
    if capped {
        if let Some(last) = out.last_mut() {
            last.capped = true;
        }
    }
    Ok(out)
}

// ── Lazy: maintenance (bucket + table) ───────────────────────────────────────

/// One maintenance entry: `(job type, status, settings summary)`.
pub type MaintenanceRow = (String, String, String);

pub async fn fetch_s3tables_bucket_maintenance(
    client: S3TablesClient,
    bucket_arn: String,
) -> Result<Vec<MaintenanceRow>> {
    let resp = client
        .get_table_bucket_maintenance_configuration()
        .table_bucket_arn(&bucket_arn)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
    let mut rows: Vec<MaintenanceRow> = resp
        .configuration()
        .iter()
        .map(|(ty, v)| {
            let status = v
                .status()
                .map(|s| s.as_str().to_string())
                .unwrap_or_else(|| "—".to_string());
            let settings = v
                .settings()
                .and_then(|s| s.as_iceberg_unreferenced_file_removal().ok())
                .map(|u| {
                    format!(
                        "unreferenced after {} days · non-current after {} days",
                        u.unreferenced_days().map(|d| d.to_string()).unwrap_or_else(|| "—".to_string()),
                        u.non_current_days().map(|d| d.to_string()).unwrap_or_else(|| "—".to_string()),
                    )
                })
                .unwrap_or_default();
            (ty.as_str().to_string(), status, settings)
        })
        .collect();
    rows.sort();
    Ok(rows)
}

/// A table's maintenance configuration plus per-job last-run status.
#[derive(Debug, Clone)]
pub struct S3TableMaintenance {
    pub config: Vec<MaintenanceRow>,
    /// `(job type, status, last run, failure message)`
    pub jobs: Vec<(String, String, String, String)>,
}

pub async fn fetch_s3tables_table_maintenance(
    client: S3TablesClient,
    bucket_arn: String,
    namespace: String,
    name: String,
) -> Result<S3TableMaintenance> {
    let cfg = client
        .get_table_maintenance_configuration()
        .table_bucket_arn(&bucket_arn)
        .namespace(&namespace)
        .name(&name)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
    let mut config: Vec<MaintenanceRow> = cfg
        .configuration()
        .iter()
        .map(|(ty, v)| {
            let status = v
                .status()
                .map(|s| s.as_str().to_string())
                .unwrap_or_else(|| "—".to_string());
            let settings = match v.settings() {
                Some(s) => {
                    if let Ok(c) = s.as_iceberg_compaction() {
                        format!(
                            "target file size {} MB{}",
                            c.target_file_size_mb().map(|m| m.to_string()).unwrap_or_else(|| "—".to_string()),
                            c.strategy().map(|st| format!(" · {}", st.as_str())).unwrap_or_default(),
                        )
                    } else if let Ok(sn) = s.as_iceberg_snapshot_management() {
                        format!(
                            "keep ≥ {} snapshots · max age {} h",
                            sn.min_snapshots_to_keep().map(|m| m.to_string()).unwrap_or_else(|| "—".to_string()),
                            sn.max_snapshot_age_hours().map(|h| h.to_string()).unwrap_or_else(|| "—".to_string()),
                        )
                    } else {
                        String::new()
                    }
                }
                None => String::new(),
            };
            (ty.as_str().to_string(), status, settings)
        })
        .collect();
    config.sort();

    // Last-run status per job — best-effort (a table that never ran anything
    // still answers, but tolerate a denial by showing the config alone).
    let jobs = match client
        .get_table_maintenance_job_status()
        .table_bucket_arn(&bucket_arn)
        .namespace(&namespace)
        .name(&name)
        .send()
        .await
    {
        Ok(resp) => {
            let mut jobs: Vec<(String, String, String, String)> = resp
                .status()
                .iter()
                .map(|(ty, v)| {
                    (
                        ty.as_str().to_string(),
                        v.status().as_str().to_string(),
                        v.last_run_timestamp()
                            .map(|t| fmt_epoch_secs(t.secs()))
                            .unwrap_or_default(),
                        v.failure_message().unwrap_or_default().to_string(),
                    )
                })
                .collect();
            jobs.sort();
            jobs
        }
        Err(_) => Vec::new(),
    };

    Ok(S3TableMaintenance { config, jobs })
}

// ── Lazy: policies (bucket + table) ──────────────────────────────────────────

/// `None` = no policy attached (the API 404s — an expected state, not an
/// error).
pub async fn fetch_s3tables_bucket_policy(
    client: S3TablesClient,
    bucket_arn: String,
) -> Result<Option<String>> {
    match client
        .get_table_bucket_policy()
        .table_bucket_arn(&bucket_arn)
        .send()
        .await
    {
        Ok(resp) => Ok(Some(resp.resource_policy().to_string())),
        Err(e) => {
            if e.as_service_error().is_some_and(|se| se.is_not_found_exception()) {
                Ok(None)
            } else {
                Err(crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))
            }
        }
    }
}

pub async fn fetch_s3tables_table_policy(
    client: S3TablesClient,
    bucket_arn: String,
    namespace: String,
    name: String,
) -> Result<Option<String>> {
    match client
        .get_table_policy()
        .table_bucket_arn(&bucket_arn)
        .namespace(&namespace)
        .name(&name)
        .send()
        .await
    {
        Ok(resp) => Ok(Some(resp.resource_policy().to_string())),
        Err(e) => {
            if e.as_service_error().is_some_and(|se| se.is_not_found_exception()) {
                Ok(None)
            } else {
                Err(crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))
            }
        }
    }
}

// ── Lazy: table extras (GetTable + tags) ─────────────────────────────────────

#[derive(Debug, Clone)]
pub struct S3TableExtras {
    pub format: String,
    pub metadata_location: String,
    pub warehouse_location: String,
    pub version_token: String,
    pub created_by: String,
    pub modified_by: String,
    pub tags: Vec<(String, String)>,
}

pub async fn fetch_s3tables_table_extras(
    client: S3TablesClient,
    table_arn: String,
) -> Result<S3TableExtras> {
    let resp = client
        .get_table()
        .table_arn(&table_arn)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
    let tags = fetch_s3tables_tags(&client, &table_arn).await.unwrap_or_default();
    Ok(S3TableExtras {
        format: resp.format().as_str().to_string(),
        metadata_location: resp.metadata_location().unwrap_or_default().to_string(),
        warehouse_location: resp.warehouse_location().to_string(),
        version_token: resp.version_token().to_string(),
        created_by: resp.created_by().to_string(),
        modified_by: resp.modified_by().to_string(),
        tags,
    })
}

fn fmt_epoch_secs(secs: i64) -> String {
    let days = secs / 86400;
    let time_rem = secs % 86400;
    let h = time_rem / 3600;
    let m = (time_rem % 3600) / 60;
    let (y, mo, d) = epoch_days_to_ymd(days);
    format!("{:04}-{:02}-{:02} {:02}:{:02} UTC", y, mo, d, h, m)
}

fn epoch_days_to_ymd(days: i64) -> (i64, u32, u32) {
    let mut y = 1970i64;
    let mut rem = days;
    loop {
        let len = if is_leap(y) { 366 } else { 365 };
        if rem < len {
            break;
        }
        rem -= len;
        y += 1;
    }
    let month_lens = [
        31,
        if is_leap(y) { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut mo = 1u32;
    for len in month_lens {
        if rem < len {
            break;
        }
        rem -= len;
        mo += 1;
    }
    (y, mo, rem as u32 + 1)
}

fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}
