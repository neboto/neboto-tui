use crate::aws::client::AwsClients;
use crate::aws::resource::{Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::error::Result;
use crate::event::{Event, LoadProgress};
use async_trait::async_trait;
use aws_sdk_route53profiles::Client as ProfilesClient;
use std::any::Any;
use std::collections::HashMap;
use tokio::sync::mpsc;

/// Route 53 Profiles — a standalone *regional* service (same regional
/// rationale as Resolver: a Profile bundles DNS config for VPCs in one
/// region) with a single resource type, `Route53Profile`. `ListProfiles`
/// returns only thin summaries (id/arn/name/share_status); everything else —
/// status, owner, VPC associations, the DNS resources (hosted zones /
/// resolver rules / DNS Firewall rule groups) bundled into the profile, and
/// tags — is fetched lazily and bundled into one `Route53ProfileDetail`,
/// mirroring how Resolver bundles its own endpoint/rule detail sections.
pub struct Route53ProfilesService {
    client: ProfilesClient,
}

impl Route53ProfilesService {
    pub fn new(aws_clients: &AwsClients) -> Self {
        Self {
            client: aws_clients.route53profiles_client(),
        }
    }
}

#[async_trait]
impl AwsService for Route53ProfilesService {
    fn service_type(&self) -> ServiceType {
        ServiceType::Route53Profiles
    }

    fn name(&self) -> &str {
        "Route53 Profiles"
    }

    async fn list_resources(&self) -> Result<Vec<Box<dyn Resource>>> {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        self.list_resources_streaming(tx, ServiceType::Route53Profiles)
            .await?;
        Ok(vec![])
    }

    async fn list_resources_streaming(
        &self,
        event_tx: mpsc::UnboundedSender<Event>,
        service_type: ServiceType,
    ) -> Result<()> {
        let mut total = 0usize;
        let mut stream = self.client.list_profiles().into_paginator().send();
        loop {
            match stream.next().await {
                Some(Ok(page)) => {
                    let batch: Vec<Box<dyn Resource>> = page
                        .profile_summaries()
                        .iter()
                        .map(|p| Box::new(Route53Profile::from_sdk(p)) as Box<dyn Resource>)
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
                Some(Err(e)) => {
                    let _ = event_tx.send(Event::ResourceLoadError {
                        service: service_type,
                        error: format!(
                            "Failed to list Route53 Profiles: {}",
                            crate::error::sdk_error_message(&e)
                        ),
                    });
                    return Ok(());
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

// ── Route53Profile ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Route53Profile {
    pub id: String,
    pub arn: String,
    pub name: String,
    /// NOT_SHARED / SHARED_BY_ME / SHARED_WITH_ME.
    pub share_status: String,
    /// Always empty — tags are fetched lazily as part of the detail bundle;
    /// this field backs the `Resource::tags()` fallback only.
    pub tags: HashMap<String, String>,
}

impl Route53Profile {
    pub fn from_sdk(p: &aws_sdk_route53profiles::types::ProfileSummary) -> Self {
        Self {
            id: p.id().unwrap_or_default().to_string(),
            arn: p.arn().unwrap_or_default().to_string(),
            name: p.name().unwrap_or_default().to_string(),
            share_status: p
                .share_status()
                .map(|s| s.as_str().to_string())
                .unwrap_or_default(),
            tags: HashMap::new(),
        }
    }
}

crate::sections! {
    pub enum Route53ProfileDetailSection,
    pub static ROUTE53_PROFILE_SECTIONS = [
        Overview "Overview" => crate::app::App::trigger_route53_profile_load,
        Vpcs "VPCs" => crate::app::App::trigger_route53_profile_load,
        Resources "Resources" => crate::app::App::trigger_route53_profile_load,
        Tags "Tags" => crate::app::App::trigger_route53_profile_load,
    ]
}

impl Resource for Route53Profile {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&ROUTE53_PROFILE_SECTIONS)
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
        "Route53 Profile"
    }
    fn state(&self) -> ResourceState {
        // ListProfiles carries no operational status (only GetProfile, lazy,
        // does) — a bare "exists" like R53HostedZone.
        ResourceState::stateless()
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        format!("{} {} {}", self.id, self.name, self.share_status)
    }
    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Name".to_string(), self.name.clone()),
            ("ID".to_string(), self.id.clone()),
            ("ARN".to_string(), self.arn.clone()),
            ("Share Status".to_string(), self.share_status.clone()),
        ]
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── Lazy detail bundle: Overview + VPCs + Resources + Tags ────────────────────

#[derive(Debug, Clone)]
pub struct Route53ProfileVpc {
    /// The VPC's ARN, as returned — carries the raw `vpc-…` token, so the
    /// generic jump classifier makes it `Enter`-jumpable without any extra
    /// parsing.
    pub vpc_arn: String,
    pub status: String,
    pub status_message: String,
}

#[derive(Debug, Clone)]
pub struct Route53ProfileResourceAssoc {
    pub resource_arn: String,
    /// Free-text from the API — a private hosted zone, a Resolver rule, or a
    /// DNS Firewall rule group. Shown verbatim; only the first two are
    /// jumpable (DNS Firewall isn't modeled as its own resource in this app).
    pub resource_type: String,
    pub status: String,
    pub status_message: String,
}

#[derive(Debug, Clone)]
pub struct Route53ProfileDetail {
    pub owner_id: String,
    pub status: String,
    pub status_message: String,
    pub creation_time: Option<String>,
    pub modification_time: Option<String>,
    pub vpcs: Vec<Route53ProfileVpc>,
    pub resources: Vec<Route53ProfileResourceAssoc>,
    pub tags: Vec<(String, String)>,
}

/// Fetch a profile's full picture — `GetProfile` (status/owner/times),
/// `ListProfileAssociations` (VPCs), `ListProfileResourceAssociations` (the
/// bundled DNS resources), and `ListTagsForResource` — bundled into one fetch
/// shared by all four sections, like Resolver's endpoint/rule details. A
/// failure in the first three is a real error (propagated); tags degrade to
/// empty on failure, matching the R53 hosted-zone sharing fetch.
pub async fn fetch_profile_detail(
    client: ProfilesClient,
    profile_id: String,
    profile_arn: String,
) -> Result<Route53ProfileDetail> {
    let get_resp = client
        .get_profile()
        .profile_id(&profile_id)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
    let profile = get_resp.profile();
    let owner_id = profile
        .and_then(|p| p.owner_id())
        .unwrap_or_default()
        .to_string();
    let status = profile
        .and_then(|p| p.status())
        .map(|s| s.as_str().to_string())
        .unwrap_or_default();
    let status_message = profile
        .and_then(|p| p.status_message())
        .unwrap_or_default()
        .to_string();
    let creation_time = profile.and_then(|p| p.creation_time()).map(|t| t.to_string());
    let modification_time = profile
        .and_then(|p| p.modification_time())
        .map(|t| t.to_string());

    let mut vpcs = Vec::new();
    let mut next: Option<String> = None;
    loop {
        let mut req = client
            .list_profile_associations()
            .profile_id(&profile_id);
        if let Some(t) = &next {
            req = req.next_token(t);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
        for a in resp.profile_associations() {
            vpcs.push(Route53ProfileVpc {
                vpc_arn: a.resource_id().unwrap_or_default().to_string(),
                status: a.status().map(|s| s.as_str().to_string()).unwrap_or_default(),
                status_message: a.status_message().unwrap_or_default().to_string(),
            });
        }
        next = crate::aws::pagination::next_page_token(resp.next_token(), &next);
        if next.is_none() {
            break;
        }
    }

    let mut resources = Vec::new();
    let mut next: Option<String> = None;
    loop {
        let mut req = client
            .list_profile_resource_associations()
            .profile_id(&profile_id);
        if let Some(t) = &next {
            req = req.next_token(t);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
        for a in resp.profile_resource_associations() {
            resources.push(Route53ProfileResourceAssoc {
                resource_arn: a.resource_arn().unwrap_or_default().to_string(),
                resource_type: a.resource_type().unwrap_or_default().to_string(),
                status: a.status().map(|s| s.as_str().to_string()).unwrap_or_default(),
                status_message: a.status_message().unwrap_or_default().to_string(),
            });
        }
        next = crate::aws::pagination::next_page_token(resp.next_token(), &next);
        if next.is_none() {
            break;
        }
    }

    let tags = match client.list_tags_for_resource().resource_arn(&profile_arn).send().await {
        Ok(resp) => resp.tags().iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
        Err(_) => Vec::new(),
    };

    Ok(Route53ProfileDetail {
        owner_id,
        status,
        status_message,
        creation_time,
        modification_time,
        vpcs,
        resources,
        tags,
    })
}
