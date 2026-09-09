use crate::aws::client::AwsClients;
use crate::aws::resource::{native_state_label, Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::aws::services::cloudwatch::fmt_epoch_secs;
use crate::error::Result;
use crate::event::{Event, LoadProgress};
use async_trait::async_trait;
use aws_sdk_ram::types::ResourceOwner;
use aws_sdk_ram::Client as RamClient;
use std::any::Any;
use std::collections::HashMap;
use tokio::sync::mpsc;

pub struct RamService {
    client: RamClient,
    owner: String, // "SELF" / "OTHER-ACCOUNTS"
}

impl RamService {
    pub fn new(aws_clients: &AwsClients, owner: String) -> Self {
        Self {
            client: aws_clients.ram_client(),
            owner,
        }
    }
}

#[async_trait]
impl AwsService for RamService {
    fn service_type(&self) -> ServiceType {
        ServiceType::Ram
    }

    fn name(&self) -> &str {
        "Resource Access Manager"
    }

    async fn list_resources(&self) -> Result<Vec<Box<dyn Resource>>> {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        self.list_resources_streaming(tx, ServiceType::Ram).await?;
        Ok(vec![])
    }

    async fn list_resources_streaming(
        &self,
        event_tx: mpsc::UnboundedSender<Event>,
        service_type: ServiceType,
    ) -> Result<()> {
        let owner = ResourceOwner::from(self.owner.as_str());
        let mut token: Option<String> = None;
        let mut total = 0usize;

        loop {
            let mut req = self
                .client
                .get_resource_shares()
                .resource_owner(owner.clone());
            if let Some(t) = &token {
                req = req.next_token(t);
            }

            let resp = match req.send().await {
                Ok(r) => r,
                Err(e) => {
                    let _ = event_tx.send(Event::ResourceLoadError {
                        service: service_type,
                        error: format!(
                            "Failed to list RAM resource shares: {}",
                            crate::error::sdk_error_message(&e)
                        ),
                    });
                    return Ok(());
                }
            };

            let next = crate::aws::pagination::next_page_token(resp.next_token(), &token);

            let batch: Vec<Box<dyn Resource>> = resp
                .resource_shares()
                .iter()
                .map(|s| {
                    Box::new(RamResourceShare::from_sdk(s, &self.owner)) as Box<dyn Resource>
                })
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

            match next {
                Some(t) => token = Some(t),
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

// ── RamResourceShare ──────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct RamResourceShare {
    pub arn: String,
    pub name: String,
    pub status: String,
    pub owning_account_id: String,
    pub owner: String, // SELF / OTHER-ACCOUNTS (scope this was loaded under)
    pub allow_external_principals: bool,
    pub feature_set: String,
    pub created_time: Option<String>,
    pub last_updated_time: Option<String>,
    pub status_message: String,
    pub tags: HashMap<String, String>,
}

impl RamResourceShare {
    pub fn from_sdk(s: &aws_sdk_ram::types::ResourceShare, owner: &str) -> Self {
        let mut tags = HashMap::new();
        for t in s.tags() {
            if let (Some(k), Some(v)) = (t.key(), t.value()) {
                tags.insert(k.to_string(), v.to_string());
            }
        }
        Self {
            arn: s.resource_share_arn().unwrap_or_default().to_string(),
            name: s.name().unwrap_or_default().to_string(),
            status: s
                .status()
                .map(|st| st.as_str().to_string())
                .unwrap_or_default(),
            owning_account_id: s.owning_account_id().unwrap_or_default().to_string(),
            owner: owner.to_string(),
            allow_external_principals: s.allow_external_principals().unwrap_or(false),
            feature_set: s
                .feature_set()
                .map(|f| f.as_str().to_string())
                .unwrap_or_default(),
            created_time: s.creation_time().map(|d| fmt_epoch_secs(d.secs())),
            last_updated_time: s.last_updated_time().map(|d| fmt_epoch_secs(d.secs())),
            status_message: s.status_message().unwrap_or_default().to_string(),
            tags,
        }
    }
}

crate::sections! {
    pub enum RamResourceShareDetailSection,
    pub static RAM_SHARE_SECTIONS = [
        Details "Details",
        Resources "Resources" => crate::app::App::trigger_ram_resources_load,
        Principals "Principals" => crate::app::App::trigger_ram_principals_load,
        Tags "Tags",
    ]
}

impl Resource for RamResourceShare {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&RAM_SHARE_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.arn
    }

    fn name(&self) -> &str {
        if !self.name.is_empty() {
            &self.name
        } else {
            self.arn.rsplit('/').next().unwrap_or(&self.arn)
        }
    }

    fn resource_type(&self) -> &str {
        "RAM Resource Share"
    }

    fn state(&self) -> ResourceState {
        match self.status.as_str() {
            "ACTIVE" => ResourceState::Available,
            "PENDING" => ResourceState::Pending,
            "FAILED" => ResourceState::Unavailable,
            "DELETING" => ResourceState::Deleting,
            "DELETED" => ResourceState::Terminated,
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
        let tags = self
            .tags
            .iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect::<Vec<_>>()
            .join(" ");
        format!(
            "{} {} {} {} {}",
            self.name, self.arn, self.owning_account_id, self.status, tags
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Name".to_string(), self.name.clone()),
            ("ARN".to_string(), self.arn.clone()),
            ("Status".to_string(), self.status.clone()),
            (
                "Owning Account".to_string(),
                self.owning_account_id.clone(),
            ),
            ("Owner".to_string(), self.owner.clone()),
            (
                "External Principals".to_string(),
                if self.allow_external_principals {
                    "Allowed"
                } else {
                    "Not allowed"
                }
                .to_string(),
            ),
            ("Feature Set".to_string(), self.feature_set.clone()),
            (
                "Created".to_string(),
                self.created_time.clone().unwrap_or_else(|| "—".to_string()),
            ),
            (
                "Updated".to_string(),
                self.last_updated_time
                    .clone()
                    .unwrap_or_else(|| "—".to_string()),
            ),
        ]
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/ram/home?region={}#ResourceShare:share={}",
            region, region, self.arn
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── Lazy-loaded state types ───────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct RamSharedResource {
    pub arn: String,
    pub status: String,
}

#[derive(Debug, Clone)]
pub struct RamPrincipal {
    pub principal: String,
    pub status: String,
    pub external: bool,
}

// ── Lazy fetch functions ──────────────────────────────────────────────────────

/// Fetch the shared resources of a resource share via
/// `GetResourceShareAssociations(RESOURCE)`. The `associatedEntity` is the
/// resource ARN (jumpable). Owner-agnostic — associations don't need the owner.
pub async fn fetch_ram_resources(
    client: RamClient,
    share_arn: String,
    owner: String,
) -> Result<Vec<RamSharedResource>> {
    let mut out = Vec::new();
    let mut token: Option<String> = None;
    let resource_owner = aws_sdk_ram::types::ResourceOwner::from(owner.as_str());
    loop {
        let mut req = client
            .list_resources()
            .resource_owner(resource_owner.clone())
            .resource_share_arns(&share_arn);
        if let Some(t) = &token {
            req = req.next_token(t);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
        let next = crate::aws::pagination::next_page_token(resp.next_token(), &token);
        for r in resp.resources() {
            out.push(RamSharedResource {
                arn: r.arn().unwrap_or_default().to_string(),
                status: r
                    .status()
                    .map(|s| s.as_str().to_string())
                    .unwrap_or_default(),
            });
        }
        match next {
            Some(t) => token = Some(t),
            None => break,
        }
    }
    Ok(out)
}

/// Fetch the principals of a resource share via
/// `GetResourceShareAssociations(PRINCIPAL)`. The `associatedEntity` is an
/// account id, an OU ARN, or the org ARN.
pub async fn fetch_ram_principals(
    client: RamClient,
    share_arn: String,
    owner: String,
) -> Result<Vec<RamPrincipal>> {
    let mut out = Vec::new();
    let mut token: Option<String> = None;
    let resource_owner = aws_sdk_ram::types::ResourceOwner::from(owner.as_str());
    loop {
        let mut req = client
            .list_principals()
            .resource_owner(resource_owner.clone())
            .resource_share_arns(&share_arn);
        if let Some(t) = &token {
            req = req.next_token(t);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
        let next = crate::aws::pagination::next_page_token(resp.next_token(), &token);
        for p in resp.principals() {
            out.push(RamPrincipal {
                principal: p.id().unwrap_or_default().to_string(),
                status: String::new(),
                external: p.external().unwrap_or(false),
            });
        }
        match next {
            Some(t) => token = Some(t),
            None => break,
        }
    }
    Ok(out)
}
