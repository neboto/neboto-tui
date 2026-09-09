use crate::aws::client::AwsClients;
use crate::aws::resource::{native_state_label, Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::error::Result;
use crate::event::{Event, LoadProgress};
use async_trait::async_trait;
use aws_sdk_kms::Client as KmsClient;
use std::any::Any;
use std::collections::HashMap;
use tokio::sync::mpsc;

pub struct KmsService {
    client: KmsClient,
}

impl KmsService {
    pub fn new(aws_clients: &AwsClients) -> Self {
        Self {
            client: aws_clients.kms_client(),
        }
    }
}

#[async_trait]
impl AwsService for KmsService {
    fn service_type(&self) -> ServiceType {
        ServiceType::Kms
    }

    fn name(&self) -> &str {
        "KMS"
    }

    async fn list_resources(&self) -> Result<Vec<Box<dyn Resource>>> {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        self.list_resources_streaming(tx, ServiceType::Kms).await?;
        Ok(vec![])
    }

    async fn list_resources_streaming(
        &self,
        event_tx: mpsc::UnboundedSender<Event>,
        service_type: ServiceType,
    ) -> Result<()> {
        // Phase 1: fetch all aliases up front (one paginated call)
        let alias_map = self.fetch_all_aliases().await;

        // Phase 2: paginate list_keys, describe each key in batches
        let mut total = 0usize;
        let mut paginator = self.client.list_keys().into_paginator().send();

        while let Some(result) = paginator.next().await {
            match result {
                Ok(page) => {
                    let keys = page.keys();
                    if keys.is_empty() {
                        continue;
                    }

                    // Describe keys concurrently (bounded)
                    let futs: Vec<_> = keys
                        .iter()
                        .filter_map(|k| k.key_id().map(|id| id.to_string()))
                        .map(|key_id| {
                            let client = self.client.clone();
                            let aliases = alias_map
                                .get(&key_id)
                                .cloned()
                                .unwrap_or_default();
                            async move {
                                let resp = client
                                    .describe_key()
                                    .key_id(&key_id)
                                    .send()
                                    .await;
                                resp.ok()
                                    .and_then(|r| r.key_metadata)
                                    .map(|meta| KmsKey::from_metadata(&meta, aliases))
                            }
                        })
                        .collect();

                    let results = futures::future::join_all(futs).await;
                    let batch: Vec<Box<dyn Resource>> = results
                        .into_iter()
                        .flatten()
                        .map(|k| Box::new(k) as Box<dyn Resource>)
                        .collect();

                    let count = batch.len();
                    if count > 0 {
                        total += count;
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
                }
                Err(e) => {
                    let _ = event_tx.send(Event::ResourceLoadError {
                        service: service_type,
                        error: format!("Failed to list KMS keys: {}", e),
                    });
                    return Ok(());
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

impl KmsService {
    async fn fetch_all_aliases(&self) -> HashMap<String, Vec<String>> {
        let mut map: HashMap<String, Vec<String>> = HashMap::new();
        let mut paginator = self.client.list_aliases().into_paginator().send();
        while let Some(Ok(page)) = paginator.next().await {
            for alias in page.aliases() {
                if let (Some(name), Some(key_id)) =
                    (alias.alias_name(), alias.target_key_id())
                {
                    let display = name.strip_prefix("alias/").unwrap_or(name).to_string();
                    map.entry(key_id.to_string())
                        .or_default()
                        .push(display);
                }
            }
        }
        map
    }
}

// ── KmsKey ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct KmsKey {
    pub key_id: String,
    pub arn: String,
    pub aliases: Vec<String>,
    pub description: String,
    pub state: String,
    pub key_usage: String,
    pub key_spec: String,
    pub origin: String,
    pub manager: String,
    pub multi_region: bool,
    #[allow(dead_code)]
    pub rotation_enabled: Option<bool>,
    pub creation_date: Option<String>,
    pub deletion_date: Option<String>,
    pub tags: HashMap<String, String>,
}

impl KmsKey {
    pub fn from_metadata(
        meta: &aws_sdk_kms::types::KeyMetadata,
        aliases: Vec<String>,
    ) -> Self {
        Self {
            key_id: meta.key_id().to_string(),
            arn: meta.arn().unwrap_or_default().to_string(),
            aliases,
            description: meta.description().unwrap_or_default().to_string(),
            state: meta
                .key_state()
                .map(|s| s.as_str().to_string())
                .unwrap_or_default(),
            key_usage: meta
                .key_usage()
                .map(|u| u.as_str().to_string())
                .unwrap_or_default(),
            key_spec: meta
                .key_spec()
                .map(|s| s.as_str().to_string())
                .unwrap_or_default(),
            origin: meta
                .origin()
                .map(|o| o.as_str().to_string())
                .unwrap_or_default(),
            manager: meta
                .key_manager()
                .map(|m| m.as_str().to_string())
                .unwrap_or_default(),
            multi_region: meta.multi_region().unwrap_or(false),
            rotation_enabled: None,
            creation_date: meta.creation_date().map(|d| fmt_epoch_secs(d.secs())),
            deletion_date: meta.deletion_date().map(|d| fmt_epoch_secs(d.secs())),
            tags: HashMap::new(),
        }
    }
}

crate::sections! {
    pub enum KmsKeyDetailSection,
    pub static KMS_KEY_SECTIONS = [
        Details "Details" => crate::app::App::trigger_kms_rotation_load,
        Policy "Policy" => crate::app::App::trigger_kms_policy_load,
        Grants "Grants" => crate::app::App::trigger_kms_grants_load,
        Tags "Tags" => crate::app::App::trigger_kms_tags_load,
    ]
}

impl Resource for KmsKey {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&KMS_KEY_SECTIONS)
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws kms describe-key --key-id {}",
            crate::aws::resource::shell_quote(self.id())
        ))
    }

    fn id(&self) -> &str {
        &self.key_id
    }

    fn name(&self) -> &str {
        self.aliases.first().map(|s| s.as_str()).unwrap_or(&self.key_id)
    }

    fn resource_type(&self) -> &str {
        "KMS Key"
    }

    fn state(&self) -> ResourceState {
        match self.state.as_str() {
            "Enabled" => ResourceState::Available,
            "Disabled" => ResourceState::Stopped,
            "PendingDeletion" => ResourceState::Deleting,
            "PendingImport" => ResourceState::Pending,
            "Unavailable" => ResourceState::Unavailable,
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
            "{} {} {} {}",
            self.key_id,
            self.arn,
            self.aliases.join(" "),
            self.description,
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Key ID".to_string(), self.key_id.clone()),
            ("Aliases".to_string(), self.aliases.join(", ")),
            ("State".to_string(), self.state.clone()),
            ("Usage".to_string(), self.key_usage.clone()),
            ("Spec".to_string(), self.key_spec.clone()),
            ("Origin".to_string(), self.origin.clone()),
            ("Manager".to_string(), self.manager.clone()),
            ("Multi-Region".to_string(), self.multi_region.to_string()),
            (
                "Created".to_string(),
                self.creation_date.clone().unwrap_or_else(|| "—".to_string()),
            ),
        ]
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/kms/home?region={}#/kms/keys/{}",
            region, region, self.key_id
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// A grant on a key (lazy Grants section; state lives in the `LazyStore`).
#[derive(Debug, Clone)]
pub struct KmsGrant {
    pub grantee_principal: String,
    pub operations: String,
    pub name: String,
    pub retiring_principal: String,
}

// ── Lazy fetch functions ──────────────────────────────────────────────────────

pub async fn fetch_kms_rotation(client: KmsClient, key_id: String) -> Option<bool> {
    match client
        .get_key_rotation_status()
        .key_id(&key_id)
        .send()
        .await
    {
        Ok(resp) => Some(resp.key_rotation_enabled()),
        Err(_) => None, // asymmetric or AWS-managed → n/a
    }
}

pub async fn fetch_kms_policy(client: KmsClient, key_id: String) -> Result<String> {
    let resp = client
        .get_key_policy()
        .key_id(&key_id)
        .policy_name("default")
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;

    Ok(resp.policy().unwrap_or_default().to_string())
}

pub async fn fetch_kms_grants(client: KmsClient, key_id: String) -> Result<Vec<KmsGrant>> {
    let mut grants = Vec::new();
    let mut paginator = client.list_grants().key_id(&key_id).into_paginator().send();
    while let Some(Ok(page)) = paginator.next().await {
        for g in page.grants() {
            grants.push(KmsGrant {
                grantee_principal: g.grantee_principal().unwrap_or_default().to_string(),
                operations: g
                    .operations()
                    .iter()
                    .map(|o| o.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
                name: g.name().unwrap_or_default().to_string(),
                retiring_principal: g.retiring_principal().unwrap_or_default().to_string(),
            });
        }
    }
    Ok(grants)
}

pub async fn fetch_kms_tags(
    client: KmsClient,
    key_id: String,
) -> Result<HashMap<String, String>> {
    let mut tags = HashMap::new();
    let mut paginator = client
        .list_resource_tags()
        .key_id(&key_id)
        .into_paginator()
        .send();
    while let Some(Ok(page)) = paginator.next().await {
        for t in page.tags() {
            tags.insert(
                t.tag_key().to_string(),
                t.tag_value().to_string(),
            );
        }
    }
    Ok(tags)
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
