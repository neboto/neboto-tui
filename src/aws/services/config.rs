//! Secrets Manager.
//!
//! Secrets deliberately surface **only metadata** during listing (name,
//! rotation/freshness) — never the secret value. The value is fetched on
//! demand by an explicit, opt-in reveal action (`x` key) via
//! `fetch_secret_value`, and is opened transiently in `$EDITOR`. It is
//! never cached in `App` state, never written to a tag/detail row, and never
//! copied to the clipboard unless the user then explicitly does so.
//!
//! (SSM Parameter Store lives in the sibling [`crate::aws::services::ssm`]
//! module under the dedicated `@ssm` service.)

use crate::aws::client::AwsClients;
use crate::aws::resource::{Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::error::Result;
use crate::event::{Event, LoadProgress};
use async_trait::async_trait;
use aws_sdk_secretsmanager::Client as SmClient;
use std::any::Any;
use std::collections::HashMap;
use tokio::sync::mpsc;

pub struct ConfigService {
    sm: SmClient,
}

impl ConfigService {
    pub fn new(aws_clients: &AwsClients) -> Self {
        Self {
            sm: aws_clients.secretsmanager_client(),
        }
    }
}

fn fmt_dt(dt: Option<&aws_sdk_secretsmanager::primitives::DateTime>) -> String {
    dt.map(|t| {
        let s = t.to_string();
        s.split('.').next().unwrap_or(&s).replace('T', " ")
    })
    .unwrap_or_default()
}

#[async_trait]
impl AwsService for ConfigService {
    fn service_type(&self) -> ServiceType {
        ServiceType::Secrets
    }

    fn name(&self) -> &str {
        "Secrets Manager"
    }

    async fn list_resources(&self) -> Result<Vec<Box<dyn Resource>>> {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        self.list_resources_streaming(tx, ServiceType::Secrets).await?;
        Ok(vec![])
    }

    async fn list_resources_streaming(
        &self,
        event_tx: mpsc::UnboundedSender<Event>,
        service_type: ServiceType,
    ) -> Result<()> {
        let mut total = 0usize;

        // ── Secrets Manager ─────────────────────────────────────────────
        let mut s_paginator = self.sm.list_secrets().into_paginator().send();
        while let Some(result) = s_paginator.next().await {
            match result {
                Ok(page) => {
                    let batch: Vec<Box<dyn Resource>> = page
                        .secret_list()
                        .iter()
                        .map(|s| Box::new(SecretEntry::from_sdk(s)) as Box<dyn Resource>)
                        .collect();
                    if batch.is_empty() {
                        continue;
                    }
                    total += batch.len();
                    let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                        service: service_type,
                        resources: batch,
                        progress: LoadProgress {
                            loaded_count: total,
                            total_count: None,
                            status_message: Some("Loading secrets…".to_string()),
                        },
                    });
                }
                Err(e) => {
                    let _ = event_tx.send(Event::ResourceLoadError {
                        service: service_type,
                        error: format!("Failed to list secrets: {}", e),
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

// ── SecretEntry ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct SecretEntry {
    pub name: String,
    pub arn: String,
    pub description: Option<String>,
    pub kms_key_id: Option<String>,
    pub rotation_enabled: bool,
    pub rotation_lambda_arn: Option<String>,
    pub rotation_after_days: Option<i64>,
    pub rotation_schedule: Option<String>,
    pub last_rotated: String,
    pub last_changed: String,
    pub last_accessed: String,
    pub next_rotation: String,
    pub created: String,
    pub owning_service: Option<String>,
    pub tags: HashMap<String, String>,
}

impl SecretEntry {
    pub fn from_sdk(s: &aws_sdk_secretsmanager::types::SecretListEntry) -> Self {
        let tags: HashMap<String, String> = s
            .tags()
            .iter()
            .filter_map(|t| Some((t.key()?.to_string(), t.value()?.to_string())))
            .collect();

        let rules = s.rotation_rules();

        Self {
            name: s.name().unwrap_or("").to_string(),
            arn: s.arn().unwrap_or("").to_string(),
            description: s.description().map(|d| d.to_string()),
            kms_key_id: s.kms_key_id().map(|k| k.to_string()),
            rotation_enabled: s.rotation_enabled().unwrap_or(false),
            rotation_lambda_arn: s.rotation_lambda_arn().map(|a| a.to_string()),
            rotation_after_days: rules.and_then(|r| r.automatically_after_days()),
            rotation_schedule: rules.and_then(|r| r.schedule_expression().map(|s| s.to_string())),
            last_rotated: fmt_dt(s.last_rotated_date()),
            last_changed: fmt_dt(s.last_changed_date()),
            last_accessed: fmt_dt(s.last_accessed_date()),
            next_rotation: fmt_dt(s.next_rotation_date()),
            created: fmt_dt(s.created_date()),
            owning_service: s.owning_service().map(|o| o.to_string()),
            tags,
        }
    }
}

crate::sections! {
    pub enum SecretDetailSection,
    pub static SECRET_SECTIONS = [
        Details "Details",
        Rotation "Rotation",
        Tags "Tags",
    ]
}

impl Resource for SecretEntry {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&SECRET_SECTIONS)
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws secretsmanager describe-secret --secret-id {}",
            crate::aws::resource::shell_quote(&self.name)
        ))
    }

    fn id(&self) -> &str {
        &self.arn
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn resource_type(&self) -> &str {
        "Secret"
    }

    fn state(&self) -> ResourceState {
        if self.rotation_enabled {
            ResourceState::Running
        } else {
            ResourceState::Available
        }
    }

    fn state_label(&self) -> String {
        if self.rotation_enabled { "rotation enabled" } else { "rotation disabled" }.to_string()
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!(
            "{} {} {}",
            self.name,
            self.description.clone().unwrap_or_default(),
            if self.rotation_enabled { "rotating" } else { "" }
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Name".to_string(), self.name.clone()),
            (
                "Rotation".to_string(),
                if self.rotation_enabled { "Enabled".to_string() } else { "Disabled".to_string() },
            ),
            ("Last Changed".to_string(), self.last_changed.clone()),
        ]
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/secretsmanager/secret?name={}&region={}",
            region, self.name, region
        ))
    }
}

// ── On-demand value reveal (opt-in `x`; value never cached) ───────────────────

/// Pretty-print a value if it parses as JSON, otherwise return it unchanged.
fn maybe_pretty(value: &str) -> String {
    serde_json::from_str::<serde_json::Value>(value)
        .ok()
        .and_then(|v| serde_json::to_string_pretty(&v).ok())
        .unwrap_or_else(|| value.to_string())
}

/// Fetch a secret's value on demand. When `pretty` is true, JSON secret
/// strings are pretty-printed for display; when false, the exact value is
/// returned (used for clipboard copy). Returns the value to be used
/// transiently — callers must not cache it.
pub async fn fetch_secret_value(sm: SmClient, secret_id: String, pretty: bool) -> Result<String> {
    let resp = sm
        .get_secret_value()
        .secret_id(&secret_id)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;

    if let Some(s) = resp.secret_string() {
        Ok(if pretty { maybe_pretty(s) } else { s.to_string() })
    } else if let Some(blob) = resp.secret_binary() {
        Ok(format!("<binary secret, {} bytes>", blob.as_ref().len()))
    } else {
        Ok("<empty secret>".to_string())
    }
}
