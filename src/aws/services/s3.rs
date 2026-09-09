use crate::aws::client::AwsClients;
use crate::aws::region::Region;
use crate::aws::resource::{Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::error::Result;
use crate::event::{Event, LoadProgress};
use async_trait::async_trait;
use aws_sdk_s3::Client as S3Client;
use std::any::Any;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, Semaphore};

/// Bucket versioning status. `Suspended` is deliberately distinct from
/// `Disabled`: a suspended bucket still retains every old version (and its
/// storage cost) — collapsing the two to a bool once made suspended buckets
/// read as never-versioned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VersioningStatus {
    Enabled,
    Suspended,
    Disabled,
}

impl VersioningStatus {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Enabled => "Enabled",
            Self::Suspended => "Suspended",
            Self::Disabled => "Disabled",
        }
    }
}

/// Public access block configuration for S3 bucket
#[derive(Clone, Debug)]
pub struct PublicAccessBlockConfig {
    pub block_public_acls: bool,
    pub ignore_public_acls: bool,
    pub block_public_policy: bool,
    pub restrict_public_buckets: bool,
}

impl PublicAccessBlockConfig {
    pub fn all_blocked(&self) -> bool {
        self.block_public_acls
            && self.ignore_public_acls
            && self.block_public_policy
            && self.restrict_public_buckets
    }

    pub fn partially_blocked(&self) -> bool {
        !self.all_blocked()
            && (self.block_public_acls
                || self.ignore_public_acls
                || self.block_public_policy
                || self.restrict_public_buckets)
    }

    /// The state of a bucket with *no* PAB configuration at all (the API
    /// reports that via `NoSuchPublicAccessBlockConfiguration`) — genuinely
    /// nothing blocked. Not to be used for lookup *failures*, which must
    /// render as unknown, never as "public allowed".
    fn not_configured() -> Self {
        Self {
            block_public_acls: false,
            ignore_public_acls: false,
            block_public_policy: false,
            restrict_public_buckets: false,
        }
    }
}

/// One bucket ACL grant, pre-classified: `public` marks the AllUsers /
/// AuthenticatedUsers group grants — the classic S3 exposure vector.
#[derive(Clone, Debug)]
pub struct AclGrant {
    pub grantee: String,
    pub permission: String,
    pub public: bool,
}

/// Enhanced encryption configuration details
#[derive(Clone, Debug)]
pub struct EncryptionConfig {
    pub enabled: bool,
    pub algorithm: Option<String>, // "AES256" or "aws:kms"
    pub kms_master_key_id: Option<String>,
    pub bucket_key_enabled: bool,
}

impl EncryptionConfig {
    pub fn format_summary(&self) -> String {
        if !self.enabled {
            return "Not configured".to_string();
        }

        match &self.algorithm {
            Some(algo) if algo.contains("kms") => {
                let key_display = self
                    .kms_master_key_id
                    .as_ref()
                    .map(|k| {
                        if k.len() > 30 {
                            format!("{}...", &k[..30])
                        } else {
                            k.clone()
                        }
                    })
                    .unwrap_or("default".to_string());
                format!(
                    "SSE-KMS (Key: {}){}",
                    key_display,
                    if self.bucket_key_enabled {
                        " + S3 Bucket Key"
                    } else {
                        ""
                    }
                )
            }
            Some(algo) => format!("SSE-S3 ({})", algo),
            None => "Enabled".to_string(),
        }
    }
}

/// Access logging configuration
#[derive(Clone, Debug)]
pub struct LoggingConfig {
    pub target_bucket: String,
    pub target_prefix: String,
}

/// Website configuration
#[derive(Clone, Debug)]
pub struct WebsiteConfig {
    pub enabled: bool,
    pub index_document: String,
    pub error_document: Option<String>,
    pub redirect_all_requests_to: Option<String>,
}

/// Replication configuration
#[derive(Clone, Debug)]
pub struct ReplicationConfig {
    pub enabled: bool,
    pub rules_count: usize,
    pub destinations: Vec<String>, // ARNs
}

/// CORS rule
#[derive(Clone, Debug)]
pub struct CorsRule {
    pub allowed_origins: Vec<String>,
    pub allowed_methods: Vec<String>,
    pub allowed_headers: Option<Vec<String>>,
    pub max_age_seconds: Option<i32>,
}

/// Object lock configuration
#[derive(Clone, Debug)]
pub struct ObjectLockConfig {
    pub enabled: bool,
    pub mode: Option<String>, // "GOVERNANCE" or "COMPLIANCE"
    pub retention_days: Option<i32>,
}

/// Event notification configuration
#[derive(Clone, Debug)]
pub struct NotificationConfig {
    pub topics: Vec<String>,           // SNS topic ARNs
    pub queues: Vec<String>,           // SQS queue ARNs
    pub lambda_functions: Vec<String>, // Lambda ARNs
    pub eventbridge: bool,             // EventBridge delivery enabled
}

pub struct S3Service {
    client: S3Client,
    current_region: Region,
}

impl S3Service {
    pub fn new(aws_clients: &AwsClients) -> Self {
        Self {
            client: aws_clients.s3_client(),
            current_region: aws_clients.current_region(),
        }
    }
}

#[async_trait]
impl AwsService for S3Service {
    fn service_type(&self) -> ServiceType {
        ServiceType::S3
    }

    fn name(&self) -> &str {
        "S3 Buckets"
    }

    async fn list_resources(&self) -> Result<Vec<Box<dyn Resource>>> {
        let mut resources: Vec<Box<dyn Resource>> = Vec::new();

        // List all buckets (global operation)
        let response = self.client.list_buckets().send().await?;

        // Collect all bucket processing tasks
        let mut tasks = Vec::new();

        for bucket in response.buckets() {
            let bucket_name = bucket.name().unwrap_or("unknown").to_string();
            let creation_date = bucket
                .creation_date()
                .map(|dt| dt.to_string())
                .unwrap_or_else(|| "unknown".to_string());

            let client = self.client.clone();

            // Spawn parallel tasks to fetch bucket details
            tasks.push(tokio::spawn(async move {
                S3Bucket::from_sdk(&client, bucket_name, creation_date).await
            }));
        }

        // Wait for all tasks to complete
        let results = futures::future::join_all(tasks).await;

        // Get current region as string for comparison
        let current_region_str = self.current_region.as_str();

        for result in results {
            match result {
                Ok(bucket) => {
                    // In-region buckets keep their fetched details; other-region
                    // buckets are shown as dimmed stubs (Enter switches region).
                    if bucket.region == current_region_str {
                        resources.push(Box::new(bucket) as Box<dyn Resource>);
                    } else {
                        resources.push(Box::new(S3Bucket::stub(
                            bucket.name.clone(),
                            bucket.creation_date.clone(),
                            bucket.region.clone(),
                        )) as Box<dyn Resource>);
                    }
                }
                Err(e) => {
                    // Log task join error but continue processing other buckets
                    eprintln!("Failed to join bucket fetch task: {}", e);
                }
            }
        }

        Ok(resources)
    }

    async fn list_resources_streaming(
        &self,
        event_tx: mpsc::UnboundedSender<Event>,
        service_type: ServiceType,
    ) -> Result<()> {
        // Phase 1: List all bucket names (fast, global)
        let list_response = match self.client.list_buckets().send().await {
            Ok(response) => response,
            Err(e) => {
                let _ = event_tx.send(Event::ResourceLoadError {
                    service: service_type,
                    error: e.to_string(),
                });
                return Err(e.into());
            }
        };

        let all_buckets: Vec<(String, String)> = list_response
            .buckets()
            .iter()
            .map(|bucket| {
                let name = bucket.name().unwrap_or("unknown").to_string();
                let creation_date = bucket
                    .creation_date()
                    .map(|dt| dt.to_string())
                    .unwrap_or_else(|| "unknown".to_string());
                (name, creation_date)
            })
            .collect();

        let total_buckets = all_buckets.len();

        // Send initial progress
        let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
            service: service_type,
            resources: vec![],
            progress: LoadProgress {
                loaded_count: 0,
                total_count: Some(total_buckets),
                status_message: Some(format!("Filtering {} buckets by region...", total_buckets)),
            },
        });

        // Phase 2: Get bucket locations with rate limiting (20 concurrent max, 200ms delays)
        let current_region_str = self.current_region.as_str();
        let semaphore = Arc::new(Semaphore::new(20)); // Max 20 concurrent location fetches
        let client = Arc::new(self.client.clone());

        let mut location_tasks = Vec::new();
        for (bucket_name, creation_date) in all_buckets {
            let permit = semaphore.clone();
            let client_clone = client.clone();
            let name_clone = bucket_name.clone();

            location_tasks.push(tokio::spawn(async move {
                let _permit = permit.acquire().await.unwrap();
                let region = S3Bucket::fetch_bucket_region(&client_clone, &name_clone).await;
                drop(_permit);
                (bucket_name, creation_date, region)
            }));
        }

        // Process location results in batches with delays
        let mut buckets_in_region: Vec<(String, String)> = Vec::new();
        // Buckets in other regions: keep name + region so the UI can show them
        // dimmed (with `Enter` to switch region), rather than hiding them.
        let mut buckets_other_region: Vec<(String, String, String)> = Vec::new();
        let mut processed_count = 0;

        // Process tasks in chunks of 20
        let mut task_iter = location_tasks.into_iter();
        loop {
            let chunk: Vec<_> = task_iter.by_ref().take(20).collect();
            if chunk.is_empty() {
                break;
            }

            let results = futures::future::join_all(chunk).await;

            for result in results {
                processed_count += 1;
                if let Ok((name, creation_date, region)) = result {
                    if region == current_region_str {
                        buckets_in_region.push((name, creation_date));
                    } else {
                        buckets_other_region.push((name, creation_date, region));
                    }
                }

                // Send progress update every 20 buckets
                if processed_count % 20 == 0 {
                    let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                        service: service_type,
                        resources: vec![],
                        progress: LoadProgress {
                            loaded_count: buckets_in_region.len(),
                            total_count: Some(total_buckets),
                            status_message: Some(format!(
                                "Filtered {}/{} buckets... Found {} in {}",
                                processed_count,
                                total_buckets,
                                buckets_in_region.len(),
                                current_region_str
                            )),
                        },
                    });
                }
            }

            // Add delay between batches to avoid rate limiting
            if processed_count < total_buckets {
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }

        let buckets_to_load = buckets_in_region.len();

        // Tell the UI how many buckets live in other regions (filtered out).
        let _ = event_tx.send(Event::S3RegionSummary {
            other_regions: total_buckets.saturating_sub(buckets_to_load),
        });

        // Send progress after region filtering complete
        let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
            service: service_type,
            resources: vec![],
            progress: LoadProgress {
                loaded_count: 0,
                total_count: Some(buckets_to_load),
                status_message: Some(format!(
                    "Found {} buckets in {}. Fetching details...",
                    buckets_to_load, current_region_str
                )),
            },
        });

        // Phase 3: Fetch full details only for buckets in current region
        // Use smaller batches (5-10) for detail fetching to avoid rate limiting
        let mut total_loaded = 0;

        for chunk in buckets_in_region.chunks(5) {
            let mut detail_tasks = Vec::new();

            for (bucket_name, creation_date) in chunk {
                let client_clone = client.clone();
                let name_clone = bucket_name.clone();
                let date_clone = creation_date.clone();

                detail_tasks.push(tokio::spawn(async move {
                    S3Bucket::from_sdk_with_details(
                        &client_clone,
                        name_clone,
                        date_clone,
                        current_region_str.to_string(),
                    )
                    .await
                }));
            }

            let results = futures::future::join_all(detail_tasks).await;

            let mut batch_resources: Vec<Box<dyn Resource>> = Vec::new();
            for result in results {
                if let Ok(bucket) = result {
                    batch_resources.push(Box::new(bucket) as Box<dyn Resource>);
                }
            }

            total_loaded += batch_resources.len();

            // Send partial update with this batch
            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                service: service_type,
                resources: batch_resources,
                progress: LoadProgress {
                    loaded_count: total_loaded,
                    total_count: Some(buckets_to_load),
                    status_message: Some(format!(
                        "Loaded {}/{} bucket details...",
                        total_loaded, buckets_to_load
                    )),
                },
            });

            // Small delay between detail batches
            if total_loaded < buckets_to_load {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }

        // Phase 4: append the other-region buckets as lightweight stubs (no
        // detail fetches — those need a client in the bucket's home region).
        // They render dimmed; `Enter` switches region and reloads with details.
        if !buckets_other_region.is_empty() {
            let stubs: Vec<Box<dyn Resource>> = buckets_other_region
                .into_iter()
                .map(|(name, creation_date, region)| {
                    Box::new(S3Bucket::stub(name, creation_date, region)) as Box<dyn Resource>
                })
                .collect();
            total_loaded += stubs.len();
            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                service: service_type,
                resources: stubs,
                progress: LoadProgress {
                    loaded_count: total_loaded,
                    total_count: Some(total_loaded),
                    status_message: None,
                },
            });
        }

        // Send completion event
        let _ = event_tx.send(Event::ResourcesFullyLoaded {
            service: service_type,
            total_count: total_loaded,
        });

        Ok(())
    }

    async fn get_resource_details(&self, bucket_name: &str) -> Result<Box<dyn Resource>> {
        // For a single bucket, fetch its details
        let creation_date = {
            let response = self.client.list_buckets().send().await?;
            response
                .buckets()
                .iter()
                .find(|b| b.name() == Some(bucket_name))
                .and_then(|b| b.creation_date())
                .map(|dt| dt.to_string())
                .unwrap_or_else(|| "unknown".to_string())
        };

        let bucket = S3Bucket::from_sdk(&self.client, bucket_name.to_string(), creation_date).await;
        Ok(Box::new(bucket) as Box<dyn Resource>)
    }
}

#[derive(Clone, Debug)]
pub struct S3Bucket {
    pub name: String,
    pub region: String,
    pub creation_date: String,
    pub tags: HashMap<String, String>,
    pub versioning: VersioningStatus,
    pub mfa_delete: bool,

    // TIER 1 FIELDS - Essential security/compliance
    pub bucket_policy: Option<String>, // JSON policy document or error message
    /// `None` = the `GetPublicAccessBlock` call failed (AccessDenied etc.) —
    /// the posture is *unknown*, which renders as such, never as wide open.
    pub public_access_block: Option<PublicAccessBlockConfig>,
    pub encryption_config: EncryptionConfig,
    pub logging_config: Option<LoggingConfig>,
    pub lifecycle_rules_count: usize,
    pub lifecycle_summary: Option<String>,
}

/// TIER 2 — detailed configuration, fetched lazily on first detail-pane view
/// (12 extra API calls), kept off `S3Bucket` so the list load stays cheap.
#[derive(Clone, Debug)]
pub struct S3BucketDetails {
    pub website_config: Option<WebsiteConfig>,
    pub replication_config: Option<ReplicationConfig>,
    pub cors_rules: Option<Vec<CorsRule>>,
    pub object_lock_config: Option<ObjectLockConfig>,
    pub acceleration_status: Option<String>,
    pub notification_config: Option<NotificationConfig>,
    pub ownership_controls: Option<String>,
    pub acl_grants: Option<Vec<AclGrant>>,
    /// `GetBucketPolicyStatus` — AWS's own "is this bucket public" verdict.
    /// `None` when there is no bucket policy or the call failed.
    pub policy_status_public: Option<bool>,
    pub inventory_configs_count: Option<usize>,
    pub analytics_configs_count: Option<usize>,
    pub metrics_configs_count: Option<usize>,
    pub intelligent_tiering_count: Option<usize>,
}

/// Lazy-load state for a bucket's Tier-2 detailed configuration.
impl S3Bucket {
    pub async fn from_sdk(client: &S3Client, name: String, creation_date: String) -> Self {
        // Fetch bucket region
        let region = Self::fetch_bucket_region(client, &name).await;

        // Fetch bucket details
        Self::from_sdk_with_details(client, name, creation_date, region).await
    }

    /// A lightweight bucket row for one that lives in another region. We know
    /// its name + region (from the Phase-2 location lookup) but skip the Tier-1
    /// detail fetches — those need a client in the bucket's home region. Shown
    /// dimmed in the list; `Enter` switches region and reloads it with details.
    pub fn stub(name: String, creation_date: String, region: String) -> Self {
        Self {
            name,
            region,
            creation_date,
            tags: HashMap::new(),
            versioning: VersioningStatus::Disabled,
            mfa_delete: false,
            bucket_policy: None,
            public_access_block: None,
            encryption_config: EncryptionConfig {
                enabled: false,
                algorithm: None,
                kms_master_key_id: None,
                bucket_key_enabled: false,
            },
            logging_config: None,
            lifecycle_rules_count: 0,
            lifecycle_summary: None,
        }
    }

    /// Fetch only the bucket region (lightweight operation)
    pub async fn fetch_bucket_region(client: &S3Client, name: &str) -> String {
        match client.get_bucket_location().bucket(name).send().await {
            Ok(response) => {
                let location_constraint = response.location_constraint();
                match location_constraint {
                    // Legacy buckets created before regional naming report the
                    // constraint "EU", which is really eu-west-1 (Ireland).
                    Some(constraint) if constraint.as_str() == "EU" => {
                        "eu-west-1".to_string()
                    }
                    // us-east-1 buckets report an *empty* constraint, which the
                    // SDK surfaces as Some("") (not None) — treat it as us-east-1
                    // so the bucket isn't stuck with a blank, unmatchable region.
                    Some(constraint) if constraint.as_str().is_empty() => {
                        "us-east-1".to_string()
                    }
                    Some(constraint) => constraint.as_str().to_string(),
                    // Empty constraint (absent element) also means us-east-1.
                    None => "us-east-1".to_string(),
                }
            }
            Err(_) => "unknown".to_string(),
        }
    }

    /// Create bucket with full details, region already known (optimization)
    pub async fn from_sdk_with_details(
        client: &S3Client,
        name: String,
        creation_date: String,
        region: String,
    ) -> Self {
        // Existing API calls
        let tags_task = client.get_bucket_tagging().bucket(&name).send();
        let versioning_task = client.get_bucket_versioning().bucket(&name).send();
        let encryption_task = client.get_bucket_encryption().bucket(&name).send();

        // NEW Tier 1 API calls
        let policy_task = client.get_bucket_policy().bucket(&name).send();
        let public_access_block_task = client.get_public_access_block().bucket(&name).send();
        let logging_task = client.get_bucket_logging().bucket(&name).send();
        let lifecycle_task = client
            .get_bucket_lifecycle_configuration()
            .bucket(&name)
            .send();

        // Execute all in parallel
        let (
            tags_result,
            versioning_result,
            encryption_result,
            policy_result,
            public_access_result,
            logging_result,
            lifecycle_result,
        ) = tokio::join!(
            tags_task,
            versioning_task,
            encryption_task,
            policy_task,
            public_access_block_task,
            logging_task,
            lifecycle_task,
        );

        // Parse tags
        let tags = match tags_result {
            Ok(response) => {
                let mut tag_map = HashMap::new();
                for tag in response.tag_set() {
                    tag_map.insert(tag.key().to_string(), tag.value().to_string());
                }
                tag_map
            }
            Err(_) => HashMap::new(),
        };

        // Parse versioning — Enabled / Suspended / Disabled are three distinct
        // states (Suspended still retains old versions), plus MFA Delete.
        let (versioning, mfa_delete) = match versioning_result {
            Ok(response) => {
                use aws_sdk_s3::types::{BucketVersioningStatus, MfaDeleteStatus};
                let status = match response.status() {
                    Some(BucketVersioningStatus::Enabled) => VersioningStatus::Enabled,
                    Some(BucketVersioningStatus::Suspended) => VersioningStatus::Suspended,
                    _ => VersioningStatus::Disabled,
                };
                let mfa = matches!(response.mfa_delete(), Some(MfaDeleteStatus::Enabled));
                (status, mfa)
            }
            Err(_) => (VersioningStatus::Disabled, false),
        };

        // Bucket Policy - distinguish between not configured vs access denied
        let bucket_policy = match policy_result {
            Ok(response) => response.policy().map(|p| p.to_string()),
            Err(e) => {
                let err_str = e.to_string();
                if err_str.contains("AccessDenied") || err_str.contains("Access Denied") {
                    Some("<Access Denied - Insufficient permissions>".to_string())
                } else {
                    None // Policy not configured
                }
            }
        };

        // Public Access Block. Only NoSuchPublicAccessBlockConfiguration means
        // "nothing blocked" — any other failure (AccessDenied…) leaves the
        // posture unknown (`None`), which must never render as wide open.
        let public_access_block = match public_access_result {
            Ok(response) => {
                let config = response.public_access_block_configuration();
                Some(PublicAccessBlockConfig {
                    block_public_acls: config.and_then(|c| c.block_public_acls()).unwrap_or(false),
                    ignore_public_acls: config
                        .and_then(|c| c.ignore_public_acls())
                        .unwrap_or(false),
                    block_public_policy: config
                        .and_then(|c| c.block_public_policy())
                        .unwrap_or(false),
                    restrict_public_buckets: config
                        .and_then(|c| c.restrict_public_buckets())
                        .unwrap_or(false),
                })
            }
            Err(e) => {
                use aws_sdk_s3::error::ProvideErrorMetadata;
                if e.code() == Some("NoSuchPublicAccessBlockConfiguration") {
                    Some(PublicAccessBlockConfig::not_configured())
                } else {
                    None
                }
            }
        };

        // Enhanced Encryption - extract algorithm, KMS key, bucket key
        let encryption_config = match encryption_result {
            Ok(response) => {
                let rules = response
                    .server_side_encryption_configuration()
                    .map(|c| c.rules());

                if let Some(rules) = rules {
                    if let Some(rule) = rules.first() {
                        let algo = rule
                            .apply_server_side_encryption_by_default()
                            .map(|d| d.sse_algorithm().as_str().to_string());

                        let kms_key = rule
                            .apply_server_side_encryption_by_default()
                            .and_then(|d| d.kms_master_key_id())
                            .map(|k| k.to_string());

                        let bucket_key = rule.bucket_key_enabled().unwrap_or(false);

                        EncryptionConfig {
                            enabled: true,
                            algorithm: algo,
                            kms_master_key_id: kms_key,
                            bucket_key_enabled: bucket_key,
                        }
                    } else {
                        EncryptionConfig {
                            enabled: false,
                            algorithm: None,
                            kms_master_key_id: None,
                            bucket_key_enabled: false,
                        }
                    }
                } else {
                    EncryptionConfig {
                        enabled: false,
                        algorithm: None,
                        kms_master_key_id: None,
                        bucket_key_enabled: false,
                    }
                }
            }
            Err(_) => EncryptionConfig {
                enabled: false,
                algorithm: None,
                kms_master_key_id: None,
                bucket_key_enabled: false,
            },
        };

        // Logging - extract target bucket and prefix
        let logging_config = match logging_result {
            Ok(response) => response.logging_enabled().map(|l| LoggingConfig {
                target_bucket: l.target_bucket().to_string(),
                target_prefix: l.target_prefix().to_string(),
            }),
            Err(_) => None,
        };

        // Lifecycle - count rules and create summary
        let (lifecycle_rules_count, lifecycle_summary) = match lifecycle_result {
            Ok(response) => {
                let rules = response.rules();
                let count = rules.len();

                // Create simple summary from first rule
                let summary = rules.first().map(|rule| {
                    let mut parts = Vec::new();

                    if let Some(expiration) = rule.expiration() {
                        if let Some(days) = expiration.days() {
                            parts.push(format!("Delete after {} days", days));
                        }
                    }

                    for transition in rule.transitions() {
                        if let (Some(days), Some(class)) =
                            (transition.days(), transition.storage_class())
                        {
                            parts.push(format!("→ {} after {} days", class.as_str(), days));
                        }
                    }

                    if parts.is_empty() {
                        "Custom rules configured".to_string()
                    } else {
                        parts.join(", ")
                    }
                });

                (count, summary)
            }
            Err(_) => (0, None),
        };

        Self {
            name,
            region,
            creation_date,
            tags,
            versioning,
            mfa_delete,
            bucket_policy,
            public_access_block,
            encryption_config,
            logging_config,
            lifecycle_rules_count,
            lifecycle_summary,
        }
    }

    /// Parse the acceleration status from its SDK response.
    fn parse_acceleration(
        output: Option<
            aws_sdk_s3::operation::get_bucket_accelerate_configuration::GetBucketAccelerateConfigurationOutput,
        >,
    ) -> Option<String> {
        output.and_then(|a| a.status().map(|s| s.as_str().to_string()))
    }

    /// Parse website configuration from AWS SDK response
    fn parse_website_config(
        output: Option<aws_sdk_s3::operation::get_bucket_website::GetBucketWebsiteOutput>,
    ) -> Option<WebsiteConfig> {
        output.map(|config| {
            let index_doc = config
                .index_document()
                .map(|d| d.suffix().to_string())
                .unwrap_or_else(|| "index.html".to_string());

            let error_doc = config.error_document().map(|d| d.key().to_string());

            let redirect = config
                .redirect_all_requests_to()
                .map(|r| r.host_name().to_string());

            WebsiteConfig {
                enabled: true,
                index_document: index_doc,
                error_document: error_doc,
                redirect_all_requests_to: redirect,
            }
        })
    }

    /// Parse replication configuration from AWS SDK response
    fn parse_replication_config(
        output: Option<aws_sdk_s3::operation::get_bucket_replication::GetBucketReplicationOutput>,
    ) -> Option<ReplicationConfig> {
        output.and_then(|config| {
            config.replication_configuration().map(|rc| {
                let rules = rc.rules();
                let destinations: Vec<String> = rules
                    .iter()
                    .filter_map(|rule| rule.destination())
                    .map(|dest| dest.bucket().to_string())
                    .collect();

                ReplicationConfig {
                    enabled: !rules.is_empty(),
                    rules_count: rules.len(),
                    destinations,
                }
            })
        })
    }

    /// Parse CORS configuration from AWS SDK response
    fn parse_cors_config(
        output: Option<aws_sdk_s3::operation::get_bucket_cors::GetBucketCorsOutput>,
    ) -> Option<Vec<CorsRule>> {
        output.map(|config| {
            config
                .cors_rules()
                .iter()
                .map(|rule| {
                    let origins = rule
                        .allowed_origins()
                        .iter()
                        .map(|o| o.to_string())
                        .collect();

                    let methods = rule
                        .allowed_methods()
                        .iter()
                        .map(|m| m.to_string())
                        .collect();

                    let headers = if rule.allowed_headers().is_empty() {
                        None
                    } else {
                        Some(
                            rule.allowed_headers()
                                .iter()
                                .map(|h| h.to_string())
                                .collect(),
                        )
                    };

                    CorsRule {
                        allowed_origins: origins,
                        allowed_methods: methods,
                        allowed_headers: headers,
                        max_age_seconds: rule.max_age_seconds(),
                    }
                })
                .collect()
        })
    }

    /// Parse object lock configuration from AWS SDK response
    fn parse_object_lock_config(
        output: Option<
            aws_sdk_s3::operation::get_object_lock_configuration::GetObjectLockConfigurationOutput,
        >,
    ) -> Option<ObjectLockConfig> {
        output.and_then(|config| {
            config.object_lock_configuration().map(|olc| {
                let enabled = olc
                    .object_lock_enabled()
                    .map(|e| e.as_str() == "Enabled")
                    .unwrap_or(false);

                let (mode, retention_days) = olc
                    .rule()
                    .and_then(|rule| rule.default_retention())
                    .map(|dr| {
                        let mode = dr.mode().map(|m| m.as_str().to_string());
                        let days = dr.days();
                        (mode, days)
                    })
                    .unwrap_or((None, None));

                ObjectLockConfig {
                    enabled,
                    mode,
                    retention_days,
                }
            })
        })
    }

    /// Parse notification configuration from AWS SDK response
    fn parse_notification_config(
        output: Option<aws_sdk_s3::operation::get_bucket_notification_configuration::GetBucketNotificationConfigurationOutput>,
    ) -> Option<NotificationConfig> {
        output.map(|config| {
            let topics: Vec<String> = config
                .topic_configurations()
                .iter()
                .map(|tc| tc.topic_arn().to_string())
                .collect();

            let queues: Vec<String> = config
                .queue_configurations()
                .iter()
                .map(|qc| qc.queue_arn().to_string())
                .collect();

            let lambda_functions: Vec<String> = config
                .lambda_function_configurations()
                .iter()
                .map(|lc| lc.lambda_function_arn().to_string())
                .collect();

            NotificationConfig {
                topics,
                queues,
                lambda_functions,
                eventbridge: config.event_bridge_configuration().is_some(),
            }
        })
    }

    /// Parse ownership controls from AWS SDK response
    fn parse_ownership_controls(
        output: Option<
            aws_sdk_s3::operation::get_bucket_ownership_controls::GetBucketOwnershipControlsOutput,
        >,
    ) -> Option<String> {
        output.and_then(|config| {
            config
                .ownership_controls()
                .and_then(|oc| oc.rules().first())
                .map(|rule| rule.object_ownership().as_str().to_string())
        })
    }

    /// Parse ACL grants — grantee + permission per grant, with the public
    /// group grants (AllUsers / AuthenticatedUsers) flagged. A bare count
    /// here once hid a public-read ACL entirely.
    fn parse_acl_grants(
        output: Option<aws_sdk_s3::operation::get_bucket_acl::GetBucketAclOutput>,
    ) -> Option<Vec<AclGrant>> {
        output.map(|acl| {
            let owner_id = acl.owner().and_then(|o| o.id()).map(str::to_string);
            acl.grants()
                .iter()
                .map(|grant| {
                    let permission = grant
                        .permission()
                        .map(|p| p.as_str().to_string())
                        .unwrap_or_else(|| "?".to_string());
                    let (grantee, public) = match grant.grantee() {
                        Some(g) => {
                            if let Some(uri) = g.uri() {
                                match uri {
                                    u if u.ends_with("/AllUsers") => {
                                        ("Everyone (AllUsers)".to_string(), true)
                                    }
                                    u if u.ends_with("/AuthenticatedUsers") => {
                                        ("Any AWS account (AuthenticatedUsers)".to_string(), true)
                                    }
                                    u if u.ends_with("/LogDelivery") => {
                                        ("S3 Log Delivery group".to_string(), false)
                                    }
                                    u => (u.to_string(), false),
                                }
                            } else if let Some(email) = g.email_address() {
                                (email.to_string(), false)
                            } else {
                                let id = g.id().unwrap_or("");
                                if owner_id.as_deref() == Some(id) {
                                    ("Bucket owner".to_string(), false)
                                } else {
                                    let short =
                                        if id.len() > 16 { &id[..16] } else { id };
                                    match g.display_name() {
                                        Some(d) if !d.is_empty() => {
                                            (format!("{} ({}…)", d, short), false)
                                        }
                                        _ => (format!("canonical {}…", short), false),
                                    }
                                }
                            }
                        }
                        None => ("unknown grantee".to_string(), false),
                    };
                    AclGrant {
                        grantee,
                        permission,
                        public,
                    }
                })
                .collect()
        })
    }
}

/// Fetch a bucket's Tier-2 detailed configuration (12 parallel API calls).
/// Called lazily on first detail-pane view, keyed by bucket name. Each call
/// tolerates per-API errors (missing config / AccessDenied) by leaving the
/// corresponding field `None`.
pub async fn fetch_bucket_details(client: S3Client, bucket_name: String) -> Result<S3BucketDetails> {
    let (website, replication, cors, obj_lock, accel, notif, ownership, acl) = tokio::join!(
        client.get_bucket_website().bucket(&bucket_name).send(),
        client.get_bucket_replication().bucket(&bucket_name).send(),
        client.get_bucket_cors().bucket(&bucket_name).send(),
        client
            .get_object_lock_configuration()
            .bucket(&bucket_name)
            .send(),
        client
            .get_bucket_accelerate_configuration()
            .bucket(&bucket_name)
            .send(),
        client
            .get_bucket_notification_configuration()
            .bucket(&bucket_name)
            .send(),
        client
            .get_bucket_ownership_controls()
            .bucket(&bucket_name)
            .send(),
        client.get_bucket_acl().bucket(&bucket_name).send(),
    );

    let (inventory, analytics, metrics, intelligent_tier, policy_status) = tokio::join!(
        client
            .list_bucket_inventory_configurations()
            .bucket(&bucket_name)
            .send(),
        client
            .list_bucket_analytics_configurations()
            .bucket(&bucket_name)
            .send(),
        client
            .list_bucket_metrics_configurations()
            .bucket(&bucket_name)
            .send(),
        client
            .list_bucket_intelligent_tiering_configurations()
            .bucket(&bucket_name)
            .send(),
        client.get_bucket_policy_status().bucket(&bucket_name).send(),
    );

    Ok(S3BucketDetails {
        website_config: S3Bucket::parse_website_config(website.ok()),
        replication_config: S3Bucket::parse_replication_config(replication.ok()),
        cors_rules: S3Bucket::parse_cors_config(cors.ok()),
        object_lock_config: S3Bucket::parse_object_lock_config(obj_lock.ok()),
        acceleration_status: S3Bucket::parse_acceleration(accel.ok()),
        notification_config: S3Bucket::parse_notification_config(notif.ok()),
        ownership_controls: S3Bucket::parse_ownership_controls(ownership.ok()),
        acl_grants: S3Bucket::parse_acl_grants(acl.ok()),
        // Errors here include "no bucket policy" — both fold to None (no verdict).
        policy_status_public: policy_status
            .ok()
            .and_then(|r| r.policy_status().and_then(|ps| ps.is_public())),
        inventory_configs_count: inventory
            .ok()
            .map(|i| i.inventory_configuration_list().len()),
        analytics_configs_count: analytics
            .ok()
            .map(|a| a.analytics_configuration_list().len()),
        metrics_configs_count: metrics.ok().map(|m| m.metrics_configuration_list().len()),
        intelligent_tiering_count: intelligent_tier
            .ok()
            .map(|t| t.intelligent_tiering_configuration_list().len()),
    })
}

crate::sections! {
    pub enum S3BucketDetailSection,
    pub static S3_BUCKET_SECTIONS = [
        Overview "Overview",
        Security "Security" => crate::app::App::trigger_s3_details_load,
        Operations "Operations" => crate::app::App::trigger_s3_details_load,
        Website "Website" => crate::app::App::trigger_s3_details_load,
        Advanced "Advanced" => crate::app::App::trigger_s3_details_load,
        Metadata "Metadata" => crate::app::App::trigger_s3_storage_metrics_load,
        FileSystems "File Systems" => crate::app::App::trigger_s3_bucket_filesystems_load,
        Tags "Tags",
    ]
}

impl Resource for S3Bucket {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&S3_BUCKET_SECTIONS)
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!("aws s3 ls s3://{}", self.name))
    }

    fn id(&self) -> &str {
        &self.name
    }

    fn name(&self) -> &str {
        // S3 buckets use Name tag as display name, fall back to bucket name
        self.tags
            .get("Name")
            .map(|s| s.as_str())
            .unwrap_or(&self.name)
    }

    fn resource_type(&self) -> &str {
        "S3 Bucket"
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn state(&self) -> ResourceState {
        // S3 buckets don't have states, always available
        ResourceState::stateless()
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        // Include all searchable fields for S3 buckets
        let mut search_parts = vec![
            self.name.clone(),
            self.region.clone(),
            self.creation_date.clone(),
        ];

        // Add tags
        for (k, v) in &self.tags {
            search_parts.push(format!("{}:{}", k, v));
        }

        search_parts.join(" ")
    }

    fn details(&self) -> Vec<(String, String)> {
        let mut details = vec![
            ("Bucket Name".to_string(), self.name.clone()),
            ("Region".to_string(), self.region.clone()),
            ("Creation Date".to_string(), self.creation_date.clone()),
        ];

        // Security & compliance section
        details.push(("".to_string(), "".to_string())); // Separator
        details.push(("Security & Compliance".to_string(), "".to_string()));

        // Public Access Block with visual indicators
        let pab_status = match &self.public_access_block {
            Some(pab) if pab.all_blocked() => "✓ Fully Blocked (Secure)".to_string(),
            Some(pab) if pab.partially_blocked() => format!(
                "⚠ Partial (ACLs:{} Policy:{} Buckets:{})",
                if pab.block_public_acls { "✓" } else { "✗" },
                if pab.block_public_policy { "✓" } else { "✗" },
                if pab.restrict_public_buckets { "✓" } else { "✗" }
            ),
            Some(_) => "✗ Public Access Allowed".to_string(),
            None => "⚠ Unknown (GetPublicAccessBlock failed)".to_string(),
        };
        details.push(("Public Access Block".to_string(), pab_status));

        // Bucket Policy
        if let Some(policy) = &self.bucket_policy {
            if policy.contains("Access Denied") {
                details.push(("Bucket Policy".to_string(), policy.clone()));
            } else {
                details.push(("Bucket Policy".to_string(), "Configured".to_string()));
            }
        } else {
            details.push(("Bucket Policy".to_string(), "Not configured".to_string()));
        }

        // Encryption using helper method
        details.push((
            "Encryption".to_string(),
            self.encryption_config.format_summary(),
        ));

        // Operations section
        details.push(("".to_string(), "".to_string()));
        details.push(("Operations".to_string(), "".to_string()));

        // Versioning
        details.push(("Versioning".to_string(), self.versioning.label().to_string()));
        if self.mfa_delete {
            details.push(("MFA Delete".to_string(), "Enabled".to_string()));
        }

        // Logging
        if let Some(logging) = &self.logging_config {
            details.push((
                "Access Logging".to_string(),
                format!(
                    "Enabled → s3://{}/{}",
                    logging.target_bucket, logging.target_prefix
                ),
            ));
        } else {
            details.push(("Access Logging".to_string(), "Not configured".to_string()));
        }

        // Lifecycle
        if self.lifecycle_rules_count > 0 {
            details.push((
                "Lifecycle Rules".to_string(),
                format!(
                    "{} rule{} configured",
                    self.lifecycle_rules_count,
                    if self.lifecycle_rules_count == 1 {
                        ""
                    } else {
                        "s"
                    }
                ),
            ));
            if let Some(summary) = &self.lifecycle_summary {
                details.push(("  Summary".to_string(), summary.clone()));
            }
        } else {
            details.push(("Lifecycle Rules".to_string(), "Not configured".to_string()));
        }

        details
    }

    fn console_url(&self, _region: &str) -> Option<String> {
        Some(format!(
            "https://s3.console.aws.amazon.com/s3/buckets/{}",
            self.name
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
}

impl S3Bucket {
    /// The bucket policy pretty-printed as JSON (compact JSON from the API is
    /// reformatted for readability). `None` when no policy is configured or
    /// access was denied fetching it.
    pub fn pretty_policy(&self) -> Option<String> {
        let raw = self
            .bucket_policy
            .as_ref()
            .filter(|p| !p.contains("Access Denied"))?;
        let pretty = serde_json::from_str::<serde_json::Value>(raw)
            .ok()
            .and_then(|v| serde_json::to_string_pretty(&v).ok())
            .unwrap_or_else(|| raw.clone());
        Some(pretty)
    }
}

// ── Object browser (list / download) ─────────────────────────────────────────

/// Maximum object size we'll pull down for inline preview/edit. Downloads
/// (the `d` action) are never capped — this only gates `v`/`e`.
pub const PREVIEW_MAX_BYTES: i64 = 1024 * 1024; // 1 MiB

/// One row in the object browser: a folder (CommonPrefix), an object, or —
/// in versions mode — one version (or delete marker) of a key.
#[derive(Clone, Debug)]
pub enum S3Entry {
    /// A "folder" — the full prefix, e.g. `logs/2026/`.
    Folder { prefix: String },
    Object {
        key: String,
        size: i64,
        last_modified: Option<String>,
        storage_class: String,
    },
    /// One entry from `ListObjectVersions` — a real version or a delete
    /// marker. `version_id` is `"null"` for objects written before
    /// versioning was enabled.
    Version {
        key: String,
        version_id: String,
        is_latest: bool,
        delete_marker: bool,
        size: i64,
        last_modified: Option<String>,
        storage_class: String,
    },
}

impl S3Entry {
    pub fn is_folder(&self) -> bool {
        matches!(self, S3Entry::Folder { .. })
    }

    /// The full S3 key/prefix this entry points at.
    pub fn full_path(&self) -> &str {
        match self {
            S3Entry::Folder { prefix } => prefix,
            S3Entry::Object { key, .. } => key,
            S3Entry::Version { key, .. } => key,
        }
    }

    /// Display name relative to the current prefix (the last path segment).
    pub fn display_name(&self, current_prefix: &str) -> String {
        let full = self.full_path();
        full.strip_prefix(current_prefix).unwrap_or(full).to_string()
    }

    /// `(key, version_id)` when this row has fetchable *content* — an object,
    /// or a version that isn't a delete marker (GET/HEAD on a delete marker's
    /// version id has no body / returns 405). Folders and delete markers are
    /// `None`.
    pub fn content_ref(&self) -> Option<(&str, Option<&str>)> {
        match self {
            S3Entry::Object { key, .. } => Some((key, None)),
            S3Entry::Version {
                key,
                version_id,
                delete_marker: false,
                ..
            } => Some((key, Some(version_id))),
            _ => None,
        }
    }
}

/// A page of object-browser results.
#[derive(Clone, Debug)]
pub struct S3ObjectPage {
    pub entries: Vec<S3Entry>,
    pub next_token: Option<String>,
}

/// List one page of a bucket at `prefix`. In folder mode `/` is the delimiter
/// so the result reads like a directory (`CommonPrefixes` → folders, `Contents`
/// → objects); in `recursive` mode the delimiter is dropped so every object
/// under the prefix (at any depth) is returned flat. `token` continues a page.
pub async fn fetch_s3_objects(
    client: S3Client,
    bucket: String,
    prefix: String,
    token: Option<String>,
    recursive: bool,
) -> Result<S3ObjectPage> {
    let mut req = client
        .list_objects_v2()
        .bucket(&bucket)
        .prefix(&prefix)
        .max_keys(1000);
    if !recursive {
        req = req.delimiter("/");
    }
    if let Some(t) = token {
        req = req.continuation_token(t);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;

    let mut entries: Vec<S3Entry> = Vec::new();

    // Folders first (alphabetical, as returned).
    for cp in resp.common_prefixes() {
        if let Some(p) = cp.prefix() {
            entries.push(S3Entry::Folder {
                prefix: p.to_string(),
            });
        }
    }

    // Then objects. Skip the key that exactly equals the prefix — that's the
    // zero-byte "folder marker" object, not a real file in this view.
    for obj in resp.contents() {
        let key = obj.key().unwrap_or_default().to_string();
        if key == prefix {
            continue;
        }
        entries.push(S3Entry::Object {
            key,
            size: obj.size().unwrap_or(0),
            last_modified: obj.last_modified().map(|d| fmt_epoch_secs(d.secs())),
            storage_class: obj
                .storage_class()
                .map(|s| s.as_str().to_string())
                .unwrap_or_else(|| "STANDARD".to_string()),
        });
    }

    let next_token = if resp.is_truncated().unwrap_or(false) {
        resp.next_continuation_token().map(|s| s.to_string())
    } else {
        None
    };

    Ok(S3ObjectPage {
        entries,
        next_token,
    })
}

/// Separator for packing `ListObjectVersions`' two continuation markers
/// (`NextKeyMarker` + `NextVersionIdMarker`) into the browser's single
/// `next_token` string. `\x00` can't appear in an S3 key or version id.
const VERSION_TOKEN_SEP: char = '\u{0}';

/// List one page of a bucket's **version history** at `prefix` — every object
/// version plus the delete markers `ListObjectsV2` hides. Same folder /
/// recursive semantics as `fetch_s3_objects`; `token` is a packed
/// key-marker + version-id-marker pair from a previous page's `next_token`.
pub async fn fetch_s3_object_versions(
    client: S3Client,
    bucket: String,
    prefix: String,
    token: Option<String>,
    recursive: bool,
) -> Result<S3ObjectPage> {
    let mut req = client
        .list_object_versions()
        .bucket(&bucket)
        .prefix(&prefix)
        .max_keys(1000);
    if !recursive {
        req = req.delimiter("/");
    }
    if let Some(t) = token {
        let (key_marker, vid_marker) = match t.split_once(VERSION_TOKEN_SEP) {
            Some((k, v)) => (k.to_string(), Some(v.to_string()).filter(|v| !v.is_empty())),
            None => (t, None),
        };
        req = req.key_marker(key_marker).set_version_id_marker(vid_marker);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;

    let mut entries: Vec<S3Entry> = Vec::new();
    for cp in resp.common_prefixes() {
        if let Some(p) = cp.prefix() {
            entries.push(S3Entry::Folder {
                prefix: p.to_string(),
            });
        }
    }

    // Versions and delete markers come back as two arrays; interleave them
    // back into per-key newest-first order (the folder-marker key is skipped
    // like in the object listing).
    let mut rows: Vec<S3Entry> = Vec::new();
    for v in resp.versions() {
        let key = v.key().unwrap_or_default().to_string();
        if key == prefix {
            continue;
        }
        rows.push(S3Entry::Version {
            key,
            version_id: v.version_id().unwrap_or_default().to_string(),
            is_latest: v.is_latest().unwrap_or(false),
            delete_marker: false,
            size: v.size().unwrap_or(0),
            last_modified: v.last_modified().map(|d| fmt_epoch_secs(d.secs())),
            storage_class: v
                .storage_class()
                .map(|s| s.as_str().to_string())
                .unwrap_or_else(|| "STANDARD".to_string()),
        });
    }
    for m in resp.delete_markers() {
        let key = m.key().unwrap_or_default().to_string();
        if key == prefix {
            continue;
        }
        rows.push(S3Entry::Version {
            key,
            version_id: m.version_id().unwrap_or_default().to_string(),
            is_latest: m.is_latest().unwrap_or(false),
            delete_marker: true,
            size: 0,
            last_modified: m.last_modified().map(|d| fmt_epoch_secs(d.secs())),
            storage_class: String::new(),
        });
    }
    rows.sort_by(|a, b| {
        let (ka, ta) = match a {
            S3Entry::Version { key, last_modified, .. } => (key, last_modified),
            _ => unreachable!(),
        };
        let (kb, tb) = match b {
            S3Entry::Version { key, last_modified, .. } => (key, last_modified),
            _ => unreachable!(),
        };
        ka.cmp(kb).then(tb.cmp(ta)) // key ascending, newest version first
    });
    entries.extend(rows);

    let next_token = if resp.is_truncated().unwrap_or(false) {
        resp.next_key_marker().map(|k| {
            format!(
                "{}{}{}",
                k,
                VERSION_TOKEN_SEP,
                resp.next_version_id_marker().unwrap_or_default()
            )
        })
    } else {
        None
    };

    Ok(S3ObjectPage {
        entries,
        next_token,
    })
}

/// Download an object's full body into memory (a specific version when
/// `version` is set).
pub async fn download_s3_object(
    client: S3Client,
    bucket: String,
    key: String,
    version: Option<String>,
) -> Result<Vec<u8>> {
    let resp = client
        .get_object()
        .bucket(&bucket)
        .key(&key)
        .set_version_id(version)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
    let data = resp
        .body
        .collect()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(format!("read object body: {}", e)))?;
    Ok(data.into_bytes().to_vec())
}

/// Download the first `len` bytes of an object (HTTP Range) — for previewing
/// the head of a file too large to pull whole.
pub async fn download_s3_object_range(
    client: S3Client,
    bucket: String,
    key: String,
    version: Option<String>,
    len: i64,
) -> Result<Vec<u8>> {
    let resp = client
        .get_object()
        .bucket(&bucket)
        .key(&key)
        .set_version_id(version)
        .range(format!("bytes=0-{}", len.saturating_sub(1)))
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
    let data = resp
        .body
        .collect()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(format!("read object body: {}", e)))?;
    Ok(data.into_bytes().to_vec())
}

/// Generate a presigned HTTPS GET URL for an object, valid for `expires_secs`.
pub async fn presign_s3_object(
    client: S3Client,
    bucket: String,
    key: String,
    version: Option<String>,
    expires_secs: u64,
) -> Result<String> {
    let cfg = aws_sdk_s3::presigning::PresigningConfig::expires_in(std::time::Duration::from_secs(
        expires_secs,
    ))
    .map_err(|e| crate::error::Error::AwsSdk(format!("presign config: {}", e)))?;
    let req = client
        .get_object()
        .bucket(&bucket)
        .key(&key)
        .set_version_id(version)
        .presigned(cfg)
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
    Ok(req.uri().to_string())
}

/// Decode bytes for text preview: refuse if it looks binary (a NUL byte in the
/// first 8 KiB), otherwise lossily decode (a range fetch may cut a multi-byte
/// char at the boundary) and drop the trailing partial line.
pub fn decode_text_preview(bytes: &[u8], truncated: bool) -> Option<String> {
    let probe = &bytes[..bytes.len().min(8192)];
    if probe.contains(&0) {
        return None;
    }
    let mut text = String::from_utf8_lossy(bytes).into_owned();
    if truncated {
        if let Some(pos) = text.rfind('\n') {
            text.truncate(pos);
        }
    }
    Some(text)
}

/// Human-readable object size.
pub fn fmt_object_size(bytes: i64) -> String {
    const KB: i64 = 1024;
    const MB: i64 = 1024 * KB;
    const GB: i64 = 1024 * MB;
    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

/// `YYYY-MM-DD HH:MM` from epoch seconds (no chrono dependency).
fn fmt_epoch_secs(secs: i64) -> String {
    let days = secs / 86_400;
    let rem = secs % 86_400;
    let (h, mi) = (rem / 3600, (rem % 3600) / 60);
    let (y, mo, d) = epoch_days_to_ymd(days);
    format!("{:04}-{:02}-{:02} {:02}:{:02}", y, mo, d, h, mi)
}

fn epoch_days_to_ymd(mut days: i64) -> (i32, u8, u8) {
    let mut year = 1970i32;
    loop {
        let dy = if is_leap_year(year) { 366 } else { 365 };
        if days < dy {
            break;
        }
        days -= dy;
        year += 1;
    }
    let dm = [
        31u8,
        if is_leap_year(year) { 29 } else { 28 },
        31, 30, 31, 30, 31, 31, 30, 31, 30, 31,
    ];
    let mut month = 1u8;
    for &dim in &dm {
        if days < dim as i64 {
            break;
        }
        days -= dim as i64;
        month += 1;
    }
    (year, month, (days + 1) as u8)
}

fn is_leap_year(y: i32) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

// ── Object metadata panel (head_object) ──────────────────────────────────────

/// Lazy-load state for a single object's metadata (the `i` info panel).
/// Richer per-object metadata from `head_object` (what the console's object
/// overview shows) — beyond the size/date/class already in the listing.
#[derive(Clone, Debug)]
pub struct S3ObjectMeta {
    pub content_type: Option<String>,
    pub content_length: i64,
    pub last_modified: Option<String>,
    pub etag: Option<String>,
    pub storage_class: Option<String>,
    pub encryption: Option<String>,
    pub kms_key_id: Option<String>,
    pub version_id: Option<String>,
    pub cache_control: Option<String>,
    pub content_encoding: Option<String>,
    pub content_disposition: Option<String>,
    pub parts_count: Option<i32>,
    pub user_metadata: Vec<(String, String)>,
}

impl S3ObjectMeta {
    /// Flat (label, value) rows for the detail panel, skipping absent fields.
    pub fn rows(&self) -> Vec<(String, String)> {
        let mut rows: Vec<(String, String)> = vec![
            ("Size".to_string(), fmt_object_size(self.content_length)),
            (
                "Content-Type".to_string(),
                self.content_type.clone().unwrap_or_else(|| "—".to_string()),
            ),
        ];
        if let Some(lm) = &self.last_modified {
            rows.push(("Last Modified".to_string(), lm.clone()));
        }
        rows.push((
            "Storage Class".to_string(),
            self.storage_class.clone().unwrap_or_else(|| "STANDARD".to_string()),
        ));
        let enc = match (&self.encryption, &self.kms_key_id) {
            (Some(e), Some(k)) => Some(format!("{} ({})", e, k)),
            (Some(e), None) => Some(e.clone()),
            _ => None,
        };
        if let Some(e) = enc {
            rows.push(("Encryption".to_string(), e));
        }
        if let Some(etag) = &self.etag {
            rows.push(("ETag".to_string(), etag.clone()));
        }
        if let Some(v) = &self.version_id {
            rows.push(("Version ID".to_string(), v.clone()));
        }
        if let Some(c) = &self.cache_control {
            rows.push(("Cache-Control".to_string(), c.clone()));
        }
        if let Some(c) = &self.content_encoding {
            rows.push(("Content-Encoding".to_string(), c.clone()));
        }
        if let Some(c) = &self.content_disposition {
            rows.push(("Content-Disposition".to_string(), c.clone()));
        }
        if let Some(p) = self.parts_count {
            if p > 1 {
                rows.push(("Parts".to_string(), p.to_string()));
            }
        }
        for (k, v) in &self.user_metadata {
            rows.push((format!("x-amz-meta-{}", k), v.clone()));
        }
        rows
    }
}

/// Fetch a single object's metadata (HEAD — no body transfer; a specific
/// version when `version` is set).
pub async fn fetch_s3_object_meta(
    client: S3Client,
    bucket: String,
    key: String,
    version: Option<String>,
) -> Result<S3ObjectMeta> {
    let r = client
        .head_object()
        .bucket(&bucket)
        .key(&key)
        .set_version_id(version)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
    Ok(S3ObjectMeta {
        content_type: r.content_type().map(|s| s.to_string()),
        content_length: r.content_length().unwrap_or(0),
        last_modified: r.last_modified().map(|d| fmt_epoch_secs(d.secs())),
        etag: r.e_tag().map(|s| s.trim_matches('"').to_string()),
        storage_class: r.storage_class().map(|s| s.as_str().to_string()),
        encryption: r.server_side_encryption().map(|s| s.as_str().to_string()),
        kms_key_id: r.ssekms_key_id().map(|s| s.to_string()),
        version_id: r.version_id().map(|s| s.to_string()),
        cache_control: r.cache_control().map(|s| s.to_string()),
        content_encoding: r.content_encoding().map(|s| s.to_string()),
        content_disposition: r.content_disposition().map(|s| s.to_string()),
        parts_count: r.parts_count(),
        user_metadata: r
            .metadata()
            .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default(),
    })
}

// ── Bucket storage metrics (CloudWatch, for the Metadata view) ───────────────

/// Lazy-load state for a bucket's CloudWatch storage metrics.
/// Daily storage metrics from CloudWatch (`AWS/S3`) — approximate (~24h stale).
#[derive(Clone, Debug)]
pub struct S3StorageMetrics {
    pub size_bytes: Option<i64>,
    pub object_count: Option<i64>,
    pub as_of: Option<String>,
}

/// The two S3 storage series via `GetMetricData` `SUM(SEARCH(…))` over
/// `[start, end]`: `BucketSizeBytes` summed across **every** StorageType
/// series and `NumberOfObjects`. Returns `(size_pts, object_pts)` as
/// `(epoch_secs, value)` sorted by time. SEARCH is what picks up the IA /
/// Glacier / Intelligent-Tiering series — a fixed
/// `StorageType=StandardStorage` dimension once charted a Glacier-heavy
/// bucket as near-empty.
async fn s3_storage_series(
    cw: &aws_sdk_cloudwatch::Client,
    bucket: &str,
    start_secs: i64,
    end_secs: i64,
) -> std::result::Result<(Vec<(i64, f64)>, Vec<(i64, f64)>), String> {
    use aws_sdk_cloudwatch::primitives::DateTime;
    use aws_sdk_cloudwatch::types::MetricDataQuery;
    let query = |id: &str, metric: &str| {
        MetricDataQuery::builder()
            .id(id.to_string())
            .expression(format!(
                "SUM(SEARCH('{{AWS/S3,BucketName,StorageType}} MetricName=\"{}\" BucketName=\"{}\"', 'Average', 86400))",
                metric, bucket
            ))
            .build()
    };
    let resp = cw
        .get_metric_data()
        .metric_data_queries(query("size", "BucketSizeBytes"))
        .metric_data_queries(query("objects", "NumberOfObjects"))
        .start_time(DateTime::from_secs(start_secs))
        .end_time(DateTime::from_secs(end_secs))
        .send()
        .await
        .map_err(|e| crate::error::sdk_error_message(&e))?;
    let mut size: Vec<(i64, f64)> = Vec::new();
    let mut objects: Vec<(i64, f64)> = Vec::new();
    for r in resp.metric_data_results() {
        let pts: Vec<(i64, f64)> = r
            .timestamps()
            .iter()
            .zip(r.values().iter())
            .map(|(t, v)| (t.secs(), *v))
            .collect();
        match r.id() {
            Some("size") => size = pts,
            Some("objects") => objects = pts,
            _ => {}
        }
    }
    size.sort_by_key(|p| p.0);
    objects.sort_by_key(|p| p.0);
    Ok((size, objects))
}

/// Fetch a bucket's size + object count from CloudWatch daily storage metrics.
/// Best-effort: missing datapoints (new bucket / not yet populated / no metric
/// permission) come back as `None` rather than an error.
pub async fn fetch_s3_storage_metrics(
    cw: aws_sdk_cloudwatch::Client,
    bucket: String,
) -> Result<S3StorageMetrics> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let (size, objects) = s3_storage_series(&cw, &bucket, now - 3 * 86_400, now)
        .await
        .unwrap_or_default();

    let size_latest = size.last().copied();
    let count_latest = objects.last().copied();
    Ok(S3StorageMetrics {
        size_bytes: size_latest.map(|(_, v)| v as i64),
        object_count: count_latest.map(|(_, v)| v as i64),
        as_of: size_latest
            .map(|(t, _)| t)
            .or(count_latest.map(|(t, _)| t))
            .map(fmt_epoch_secs),
    })
}

// ── Charted storage metrics (`m` overlay) — AWS/S3, daily datapoints ──────────
//
// S3 storage metrics are emitted **once per day**, so the shared
// `MetricsTimeRange` (max 7d, sub-hour periods) is a poor fit — the overlay
// always charts a fixed 30-day / daily window instead. Two series: bucket size
// (summed across every StorageType via SEARCH) and object count.

/// Number of days of daily storage datapoints the `m` overlay charts.
pub const S3_METRICS_DAYS: i64 = 30;

#[derive(Debug, Clone)]
pub struct S3MetricsData {
    /// `(x_offset_secs_from_window_start, value)` points, sorted by time.
    pub size_bytes: Vec<(f64, f64)>,
    pub object_count: Vec<(f64, f64)>,
    pub x_max: f64,
}

#[derive(Debug, Clone)]
pub enum S3MetricsState {
    Loading,
    Loaded(S3MetricsData),
}

/// Pull the full 30-day daily series for a bucket's size + object count.
/// Best-effort: a metric with no data (new/empty bucket, missing permission)
/// charts as an empty series rather than erroring.
pub async fn fetch_s3_bucket_metrics(
    cw: aws_sdk_cloudwatch::Client,
    bucket: String,
) -> Result<S3MetricsData> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let start = now - S3_METRICS_DAYS * 86_400;
    let (size, objects) = s3_storage_series(&cw, &bucket, start, now)
        .await
        .unwrap_or_default();

    let rel = |pts: Vec<(i64, f64)>| -> Vec<(f64, f64)> {
        pts.into_iter()
            .map(|(t, v)| (t as f64 - start as f64, v))
            .collect()
    };
    Ok(S3MetricsData {
        size_bytes: rel(size),
        object_count: rel(objects),
        x_max: (S3_METRICS_DAYS * 86_400) as f64,
    })
}
