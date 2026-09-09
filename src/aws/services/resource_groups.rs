use crate::aws::client::AwsClients;
use crate::aws::resource::{Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::error::Result;
use crate::event::{Event, LoadProgress};
use async_trait::async_trait;
use aws_sdk_resourcegroups::Client as ResourceGroupsClient;
use std::any::Any;
use std::collections::HashMap;
use tokio::sync::mpsc;

pub struct ResourceGroupsService {
    client: ResourceGroupsClient,
}

impl ResourceGroupsService {
    pub fn new(aws_clients: &AwsClients) -> Self {
        Self {
            client: aws_clients.resourcegroups_client(),
        }
    }
}

#[async_trait]
impl AwsService for ResourceGroupsService {
    fn service_type(&self) -> ServiceType {
        ServiceType::ResourceGroups
    }

    fn name(&self) -> &str {
        "Resource Groups"
    }

    async fn list_resources(&self) -> Result<Vec<Box<dyn Resource>>> {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        self.list_resources_streaming(tx, ServiceType::ResourceGroups)
            .await?;
        Ok(vec![])
    }

    async fn list_resources_streaming(
        &self,
        event_tx: mpsc::UnboundedSender<Event>,
        service_type: ServiceType,
    ) -> Result<()> {
        let mut total = 0usize;
        let mut next_token: Option<String> = None;

        loop {
            let mut req = self.client.list_groups();
            if let Some(token) = next_token.take() {
                req = req.next_token(token);
            }

            match req.send().await {
                Ok(resp) => {
                    let groups = resp.group_identifiers();

                    if !groups.is_empty() {
                        let batch: Vec<Box<dyn Resource>> = groups
                            .iter()
                            .map(|gi| {
                                let name = gi.group_name().unwrap_or_default().to_string();
                                let arn = gi.group_arn().unwrap_or_default().to_string();
                                // Description not available from ListGroups; fetch lazily
                                Box::new(ResourceGroup {
                                    name,
                                    arn,
                                    description: String::new(),
                                    query_type: None,
                                    tags: HashMap::new(),
                                }) as Box<dyn Resource>
                            })
                            .collect();

                        total += batch.len();
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

                    next_token = resp.next_token().map(|s| s.to_string());
                    if next_token.is_none() {
                        break;
                    }
                }
                Err(e) => {
                    let _ = event_tx.send(Event::ResourceLoadError {
                        service: service_type,
                        error: format!("Failed to list resource groups: {}", e),
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

// ── ResourceGroup ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ResourceGroup {
    pub name: String,
    pub arn: String,
    pub description: String,
    pub query_type: Option<String>,
    pub tags: HashMap<String, String>,
}

crate::sections! {
    pub enum ResourceGroupDetailSection,
    pub static RESOURCE_GROUP_SECTIONS = [
        Details "Details" => crate::app::App::trigger_resource_group_query_load,
        Query "Query" => crate::app::App::trigger_resource_group_query_load,
        Resources "Resources" => crate::app::App::trigger_resource_group_resources_load,
        Tags "Tags" => crate::app::App::trigger_resource_group_tags_load,
    ]
}

impl Resource for ResourceGroup {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&RESOURCE_GROUP_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.name
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn resource_type(&self) -> &str {
        "Resource Group"
    }

    fn state(&self) -> ResourceState {
        ResourceState::stateless()
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!("{} {} {}", self.name, self.arn, self.description)
    }

    fn details(&self) -> Vec<(String, String)> {
        let mut d = vec![
            ("Name".to_string(), self.name.clone()),
            ("ARN".to_string(), self.arn.clone()),
            ("Description".to_string(), self.description.clone()),
        ];
        if let Some(ref qt) = self.query_type {
            d.push(("Query Type".to_string(), qt.clone()));
        }
        d
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/resource-groups/group/{}?region={}",
            region, self.name, region
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
pub struct ResourceGroupQuery {
    pub query_type: String,
    pub query: String,
}

#[derive(Debug, Clone)]
pub struct ResourceGroupMember {
    pub arn: String,
    pub resource_type: String,
}

// ── Lazy fetch functions ──────────────────────────────────────────────────────

pub async fn fetch_resource_group_query(
    client: ResourceGroupsClient,
    group_name: String,
) -> std::result::Result<ResourceGroupQuery, String> {
    match client.get_group_query().group(&group_name).send().await {
        Ok(resp) => {
            if let Some(gq) = resp.group_query() {
                let rq = gq.resource_query();
                let query_type = rq
                    .map(|q| q.r#type().as_str().to_string())
                    .unwrap_or_default();
                let query = rq
                    .map(|q| q.query().to_string())
                    .unwrap_or_default();
                Ok(ResourceGroupQuery { query_type, query })
            } else {
                Err("No query returned".to_string())
            }
        }
        Err(e) => Err(format!("{}", e)),
    }
}

pub async fn fetch_resource_group_resources(
    client: ResourceGroupsClient,
    group_name: String,
) -> std::result::Result<Vec<ResourceGroupMember>, String> {
    let mut members = Vec::new();
    let mut next_token: Option<String> = None;

    loop {
        let mut req = client.list_group_resources().group(&group_name);
        if let Some(token) = next_token.take() {
            req = req.next_token(token);
        }

        match req.send().await {
            Ok(resp) => {
                for res in resp.resources() {
                    if let Some(ri) = res.identifier() {
                        let arn = ri.resource_arn().unwrap_or_default().to_string();
                        let resource_type = ri.resource_type().unwrap_or_default().to_string();
                        members.push(ResourceGroupMember {
                            arn,
                            resource_type,
                        });
                    }
                }

                next_token = resp.next_token().map(|s| s.to_string());
                if next_token.is_none() {
                    break;
                }
            }
            Err(e) => return Err(format!("{}", e)),
        }
    }
    Ok(members)
}

pub async fn fetch_resource_group_tags(
    client: ResourceGroupsClient,
    arn: String,
) -> std::result::Result<HashMap<String, String>, String> {
    match client.get_tags().arn(&arn).send().await {
        Ok(resp) => {
            let tags = resp
                .tags()
                .map(|t| t.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                .unwrap_or_default();
            Ok(tags)
        }
        Err(e) => Err(format!("{}", e)),
    }
}
