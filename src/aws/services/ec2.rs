use crate::aws::client::AwsClients;
use crate::aws::resource::sg_refs;
use crate::aws::resource::{native_state_label, Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::error::Result;
use crate::event::{Event, LoadProgress};
use async_trait::async_trait;
use aws_sdk_cloudwatch::primitives::DateTime as CwDateTime;
use aws_sdk_ec2::Client as Ec2Client;
use std::any::Any;
use std::collections::HashMap;
use tokio::sync::mpsc;

/// A role contained in an EC2 instance profile (from `iam:GetInstanceProfile`).
#[derive(Clone, Debug)]
pub struct InstanceProfileRole {
    pub role_name: String,
    pub role_arn: String,
}

/// Resolve the role(s) inside an instance profile via `iam:GetInstanceProfile`.
/// Keyed by instance-profile name (the trailing segment of the profile ARN).
pub async fn fetch_instance_profile_roles(
    client: aws_sdk_iam::Client,
    profile_name: String,
) -> Result<Vec<InstanceProfileRole>> {
    let resp = client
        .get_instance_profile()
        .instance_profile_name(&profile_name)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;

    let mut roles = Vec::new();
    if let Some(profile) = resp.instance_profile() {
        for r in profile.roles() {
            roles.push(InstanceProfileRole {
                role_name: r.role_name().to_string(),
                role_arn: r.arn().to_string(),
            });
        }
    }

    Ok(roles)
}

/// An instance's launch user data via `DescribeInstanceAttribute`
/// (`Attribute=userData`) — it isn't part of `DescribeInstances`, so it's a
/// separate lazy call. The API returns it base64-encoded; decode to the
/// human-readable script/cloud-config, falling back to the raw value if it
/// isn't valid base64/UTF-8 (e.g. a gzip-compressed payload). `Ok(None)` means
/// the instance has no user data configured.
pub async fn fetch_instance_user_data(
    client: aws_sdk_ec2::Client,
    instance_id: String,
) -> Result<Option<String>> {
    let resp = client
        .describe_instance_attribute()
        .instance_id(&instance_id)
        .attribute(aws_sdk_ec2::types::InstanceAttributeName::UserData)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;

    let raw = resp
        .user_data()
        .and_then(|av| av.value())
        .filter(|s| !s.is_empty());

    Ok(raw.map(|b64| {
        aws_smithy_types::base64::decode(b64)
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .unwrap_or_else(|| b64.to_string())
    }))
}

/// SSM Session Manager connectability for an EC2 instance, derived from
/// `ssm:DescribeInstanceInformation`. Only instances with the SSM agent
/// registered appear in that call at all; an instance absent from the map
/// is "not managed" and cannot be connected to via Session Manager.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SsmInstanceStatus {
    Online,
    ConnectionLost,
    Inactive,
}

impl SsmInstanceStatus {
    /// Human-readable label for the detail pane.
    pub fn label(&self) -> &'static str {
        match self {
            SsmInstanceStatus::Online => "Online",
            SsmInstanceStatus::ConnectionLost => "Connection lost",
            SsmInstanceStatus::Inactive => "Inactive",
        }
    }

    /// Whether a Session Manager session is likely to succeed right now.
    pub fn is_connectable(&self) -> bool {
        matches!(self, SsmInstanceStatus::Online)
    }
}

/// Fetch SSM-managed instance status for every registered instance in the
/// region, keyed by instance id. Instances not registered with SSM simply
/// won't appear in the returned map.
pub async fn fetch_ssm_instance_info(
    client: aws_sdk_ssm::Client,
) -> Result<HashMap<String, SsmInstanceStatus>> {
    use aws_sdk_ssm::types::PingStatus;

    let mut out: HashMap<String, SsmInstanceStatus> = HashMap::new();
    let mut next_token: Option<String> = None;

    loop {
        let resp = client
            .describe_instance_information()
            .max_results(50)
            .set_next_token(next_token.clone())
            .send()
            .await
            .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;

        for info in resp.instance_information_list() {
            if let Some(id) = info.instance_id() {
                let status = match info.ping_status() {
                    Some(PingStatus::Online) => SsmInstanceStatus::Online,
                    Some(PingStatus::ConnectionLost) => SsmInstanceStatus::ConnectionLost,
                    _ => SsmInstanceStatus::Inactive,
                };
                out.insert(id.to_string(), status);
            }
        }

        match resp.next_token() {
            Some(t) if !t.is_empty() => next_token = Some(t.to_string()),
            _ => break,
        }
    }

    Ok(out)
}

fn format_port_range(from: Option<i32>, to: Option<i32>, protocol: &str) -> String {
    if protocol == "-1" {
        return "All".to_string();
    }
    match (from, to) {
        (Some(f), Some(t)) if f == t => f.to_string(),
        (Some(f), Some(t)) => format!("{}-{}", f, t),
        _ => "All".to_string(),
    }
}

pub struct Ec2Service {
    client: Ec2Client,
}

impl Ec2Service {
    pub fn new(aws_clients: &AwsClients) -> Self {
        Self {
            client: aws_clients.ec2_client(),
        }
    }
}

#[async_trait]
impl AwsService for Ec2Service {
    fn service_type(&self) -> ServiceType {
        ServiceType::EC2
    }

    fn name(&self) -> &str {
        "EC2 Instances"
    }

    async fn list_resources(&self) -> Result<Vec<Box<dyn Resource>>> {
        let mut resources: Vec<Box<dyn Resource>> = Vec::new();

        // Paginated like the streaming path — one page caps at 1000 instances.
        let mut pages = self.client.describe_instances().into_paginator().send();
        while let Some(page) = pages.next().await {
            for reservation in page?.reservations() {
                for instance in reservation.instances() {
                    let ec2_instance = Ec2Instance::from_sdk(instance);
                    resources.push(Box::new(ec2_instance) as Box<dyn Resource>);
                }
            }
        }

        // AMIs (self-owned only).
        {
            let mut token: Option<String> = None;
            loop {
                let resp = self
                    .client
                    .describe_images()
                    .owners("self")
                    .set_next_token(token.clone())
                    .send()
                    .await;
                match resp {
                    Ok(resp) => {
                        for img in resp.images() {
                            resources.push(Box::new(Ami::from_sdk(img)) as Box<dyn Resource>);
                        }
                        token = crate::aws::pagination::next_page_token(resp.next_token(), &token);
                        if token.is_none() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        }

        // Snapshots (self-owned only).
        {
            let mut token: Option<String> = None;
            loop {
                let resp = self
                    .client
                    .describe_snapshots()
                    .owner_ids("self")
                    .set_next_token(token.clone())
                    .send()
                    .await;
                match resp {
                    Ok(resp) => {
                        for s in resp.snapshots() {
                            resources.push(Box::new(EbsSnapshot::from_sdk(s)) as Box<dyn Resource>);
                        }
                        token = crate::aws::pagination::next_page_token(resp.next_token(), &token);
                        if token.is_none() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        }

        // Launch Templates (versions lazy).
        {
            let mut token: Option<String> = None;
            loop {
                let resp = self
                    .client
                    .describe_launch_templates()
                    .set_next_token(token.clone())
                    .send()
                    .await;
                match resp {
                    Ok(resp) => {
                        for lt in resp.launch_templates() {
                            resources
                                .push(Box::new(LaunchTemplate::from_sdk(lt)) as Box<dyn Resource>);
                        }
                        token = crate::aws::pagination::next_page_token(resp.next_token(), &token);
                        if token.is_none() {
                            break;
                        }
                    }
                    Err(_) => break,
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
        use std::collections::HashSet;

        let mut total_loaded = 0;

        // Phase 1: Instances
        {
            let mut paginator = self.client.describe_instances().into_paginator().send();
            let mut seen_ids: HashSet<String> = HashSet::new();

            while let Some(result) = paginator.next().await {
                match result {
                    Ok(output) => {
                        let mut batch: Vec<Box<dyn Resource>> = Vec::new();
                        for reservation in output.reservations() {
                            for instance in reservation.instances() {
                                let id = instance.instance_id().unwrap_or("unknown").to_string();
                                if seen_ids.insert(id) {
                                    batch.push(Box::new(Ec2Instance::from_sdk(instance)));
                                    total_loaded += 1;
                                }
                            }
                        }
                        if !batch.is_empty() {
                            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                                service: service_type,
                                resources: batch,
                                progress: LoadProgress {
                                    loaded_count: total_loaded,
                                    total_count: None,
                                    status_message: Some(format!(
                                        "Loaded {} instances…",
                                        total_loaded
                                    )),
                                },
                            });
                        }
                    }
                    Err(e) => {
                        let _ = event_tx.send(Event::ResourceLoadError {
                            service: service_type,
                            error: e.to_string(),
                        });
                        return Err(e.into());
                    }
                }
            }
        }

        // Phase 2: Security Groups (paginated — a bare `.send()` caps at
        // 1000 and >1000 SGs is common in older accounts)
        {
            let mut pages = self
                .client
                .describe_security_groups()
                .into_paginator()
                .send();
            while let Some(page) = pages.next().await {
                match page {
                    Ok(resp) => {
                        let mut batch: Vec<Box<dyn Resource>> = Vec::new();
                        for sg in resp.security_groups() {
                            batch.push(Box::new(SecurityGroup::from_sdk(sg)));
                            total_loaded += 1;
                        }
                        if !batch.is_empty() {
                            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                                service: service_type,
                                resources: batch,
                                progress: LoadProgress {
                                    loaded_count: total_loaded,
                                    total_count: None,
                                    status_message: Some(
                                        "Loading security groups…".to_string(),
                                    ),
                                },
                            });
                        }
                    }
                    Err(e) => {
                        let _ = event_tx.send(Event::ResourceLoadWarning {
                            service: service_type,
                            warning: format!(
                                "Security groups: {}",
                                crate::error::sdk_error_message(&e)
                            ),
                        });
                        break;
                    }
                }
            }
        }

        // Phase 3: EBS Volumes (paginated — one page caps at 1000 volumes)
        {
            let mut pages = self.client.describe_volumes().into_paginator().send();
            while let Some(page) = pages.next().await {
                match page {
                    Ok(resp) => {
                        let mut batch: Vec<Box<dyn Resource>> = Vec::new();
                        for vol in resp.volumes() {
                            batch.push(Box::new(EbsVolume::from_sdk(vol)));
                            total_loaded += 1;
                        }
                        if !batch.is_empty() {
                            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                                service: service_type,
                                resources: batch,
                                progress: LoadProgress {
                                    loaded_count: total_loaded,
                                    total_count: None,
                                    status_message: Some("Loading EBS volumes…".to_string()),
                                },
                            });
                        }
                    }
                    Err(e) => {
                        let _ = event_tx.send(Event::ResourceLoadWarning {
                            service: service_type,
                            warning: format!(
                                "EBS volumes: {}",
                                crate::error::sdk_error_message(&e)
                            ),
                        });
                        break;
                    }
                }
            }
        }

        // Phase 4: Network Interfaces (paginated — ENIs are the resource
        // most likely to exceed one 1000-item page: every Lambda-in-VPC /
        // ELB node / EKS pod mints them, and a bare `.send()` truncated).
        {
            let mut pages = self
                .client
                .describe_network_interfaces()
                .into_paginator()
                .send();
            while let Some(page) = pages.next().await {
                match page {
                    Ok(resp) => {
                        let mut batch: Vec<Box<dyn Resource>> = Vec::new();
                        for eni in resp.network_interfaces() {
                            batch.push(Box::new(NetworkInterface::from_sdk(eni)));
                            total_loaded += 1;
                        }
                        if !batch.is_empty() {
                            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                                service: service_type,
                                resources: batch,
                                progress: LoadProgress {
                                    loaded_count: total_loaded,
                                    total_count: None,
                                    status_message: Some(
                                        "Loading network interfaces…".to_string(),
                                    ),
                                },
                            });
                        }
                    }
                    Err(e) => {
                        let _ = event_tx.send(Event::ResourceLoadWarning {
                            service: service_type,
                            warning: format!(
                                "Network interfaces: {}",
                                crate::error::sdk_error_message(&e)
                            ),
                        });
                        break;
                    }
                }
            }
        }

        // Phase 5: AMIs (self-owned only — public/Amazon AMIs are enormous).
        {
            let mut token: Option<String> = None;
            loop {
                match self
                    .client
                    .describe_images()
                    .owners("self")
                    .set_next_token(token.clone())
                    .send()
                    .await
                {
                    Ok(resp) => {
                        let mut batch: Vec<Box<dyn Resource>> = Vec::new();
                        for img in resp.images() {
                            batch.push(Box::new(Ami::from_sdk(img)));
                            total_loaded += 1;
                        }
                        if !batch.is_empty() {
                            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                                service: service_type,
                                resources: batch,
                                progress: LoadProgress {
                                    loaded_count: total_loaded,
                                    total_count: None,
                                    status_message: Some("Loading AMIs…".to_string()),
                                },
                            });
                        }
                        token = crate::aws::pagination::next_page_token(resp.next_token(), &token);
                        if token.is_none() {
                            break;
                        }
                    }
                    Err(e) => {
                        let _ = event_tx.send(Event::ResourceLoadWarning {
                            service: service_type,
                            warning: format!("AMIs: {}", crate::error::sdk_error_message(&e)),
                        });
                        break;
                    }
                }
            }
        }

        // Phase 6: Snapshots (self-owned only).
        {
            let mut token: Option<String> = None;
            loop {
                match self
                    .client
                    .describe_snapshots()
                    .owner_ids("self")
                    .set_next_token(token.clone())
                    .send()
                    .await
                {
                    Ok(resp) => {
                        let mut batch: Vec<Box<dyn Resource>> = Vec::new();
                        for s in resp.snapshots() {
                            batch.push(Box::new(EbsSnapshot::from_sdk(s)));
                            total_loaded += 1;
                        }
                        if !batch.is_empty() {
                            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                                service: service_type,
                                resources: batch,
                                progress: LoadProgress {
                                    loaded_count: total_loaded,
                                    total_count: None,
                                    status_message: Some("Loading snapshots…".to_string()),
                                },
                            });
                        }
                        token = crate::aws::pagination::next_page_token(resp.next_token(), &token);
                        if token.is_none() {
                            break;
                        }
                    }
                    Err(e) => {
                        let _ = event_tx.send(Event::ResourceLoadWarning {
                            service: service_type,
                            warning: format!("Snapshots: {}", crate::error::sdk_error_message(&e)),
                        });
                        break;
                    }
                }
            }
        }

        // Phase 7: Launch Templates (versions are lazy — see fetch_launch_template_versions).
        {
            let mut token: Option<String> = None;
            loop {
                match self
                    .client
                    .describe_launch_templates()
                    .set_next_token(token.clone())
                    .send()
                    .await
                {
                    Ok(resp) => {
                        let mut batch: Vec<Box<dyn Resource>> = Vec::new();
                        for lt in resp.launch_templates() {
                            batch.push(Box::new(LaunchTemplate::from_sdk(lt)));
                            total_loaded += 1;
                        }
                        if !batch.is_empty() {
                            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                                service: service_type,
                                resources: batch,
                                progress: LoadProgress {
                                    loaded_count: total_loaded,
                                    total_count: None,
                                    status_message: Some("Loading launch templates…".to_string()),
                                },
                            });
                        }
                        token = crate::aws::pagination::next_page_token(resp.next_token(), &token);
                        if token.is_none() {
                            break;
                        }
                    }
                    Err(e) => {
                        let _ = event_tx.send(Event::ResourceLoadWarning {
                            service: service_type,
                            warning: format!("Launch templates: {}", crate::error::sdk_error_message(&e)),
                        });
                        break;
                    }
                }
            }
        }

        // Phase 8: Elastic IPs (DescribeAddresses returns all in one call — no paginator).
        match self.client.describe_addresses().send().await {
            Ok(resp) => {
                let mut batch: Vec<Box<dyn Resource>> = Vec::new();
                for addr in resp.addresses() {
                    batch.push(Box::new(ElasticIp::from_sdk(addr)));
                    total_loaded += 1;
                }
                if !batch.is_empty() {
                    let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                        service: service_type,
                        resources: batch,
                        progress: LoadProgress {
                            loaded_count: total_loaded,
                            total_count: None,
                            status_message: Some("Loading Elastic IPs…".to_string()),
                        },
                    });
                }
            }
            Err(e) => {
                let _ = event_tx.send(Event::ResourceLoadWarning {
                    service: service_type,
                    warning: format!("Elastic IPs: {}", crate::error::sdk_error_message(&e)),
                });
            }
        }

        let _ = event_tx.send(Event::ResourcesFullyLoaded {
            service: service_type,
            total_count: total_loaded,
        });

        Ok(())
    }

    async fn get_resource_details(&self, id: &str) -> Result<Box<dyn Resource>> {
        let response = self
            .client
            .describe_instances()
            .instance_ids(id)
            .send()
            .await?;

        for reservation in response.reservations() {
            if let Some(instance) = reservation.instances().first() {
                let ec2_instance = Ec2Instance::from_sdk(instance);
                return Ok(Box::new(ec2_instance) as Box<dyn Resource>);
            }
        }

        Err(crate::error::Error::ResourceNotFound(id.to_string()))
    }
}

/// A network interface embedded in an EC2 instance (from describe_instances, no extra API call).
#[derive(Clone, Debug, Default)]
pub struct EmbeddedNic {
    pub interface_id: String,
    pub description: String,
    pub status: String,
    pub private_ip: Option<String>,
    pub public_ip: Option<String>,
    pub subnet_id: Option<String>,
    pub vpc_id: Option<String>,
    pub mac_address: Option<String>,
    pub security_groups: Vec<(String, String)>, // (id, name)
}

/// A block device mapping embedded in an EC2 instance (from describe_instances).
#[derive(Clone, Debug, Default)]
pub struct EmbeddedVolume {
    pub device_name: String,
    pub volume_id: String,
    pub status: String,
    pub delete_on_termination: bool,
    pub is_root: bool,
}

#[derive(Clone, Debug)]
pub struct Ec2Instance {
    pub instance_id: String,
    pub instance_type: String,
    pub state: String,
    pub public_ip: Option<String>,
    pub private_ip: Option<String>,
    pub tags: HashMap<String, String>,
    pub launch_time: Option<String>,
    pub availability_zone: Option<String>,
    pub vpc_id: Option<String>,
    pub subnet_id: Option<String>,
    pub cost: Option<f64>,
    // Extended metadata (all from describe_instances — no extra API calls)
    pub ami_id: Option<String>,
    pub key_name: Option<String>,
    pub architecture: Option<String>,
    pub platform: Option<String>,
    pub monitoring_state: Option<String>,
    pub iam_profile_arn: Option<String>,
    #[allow(dead_code)] // used during from_sdk to compute block_devices[].is_root
    pub root_device_name: Option<String>,
    pub root_device_type: Option<String>,
    pub security_groups: Vec<(String, String)>, // (id, name)
    pub network_interfaces: Vec<EmbeddedNic>,
    pub block_devices: Vec<EmbeddedVolume>,
}

impl Ec2Instance {
    pub fn from_sdk(instance: &aws_sdk_ec2::types::Instance) -> Self {
        let instance_id = instance.instance_id().unwrap_or("unknown").to_string();

        let instance_type = instance
            .instance_type()
            .map(|t| t.as_str().to_string())
            .unwrap_or_else(|| "unknown".to_string());

        let state = instance
            .state()
            .and_then(|s| s.name())
            .map(|n| n.as_str().to_string())
            .unwrap_or_else(|| "unknown".to_string());

        let public_ip = instance.public_ip_address().map(|ip| ip.to_string());

        let private_ip = instance.private_ip_address().map(|ip| ip.to_string());

        let launch_time = instance
            .launch_time()
            .map(|t| {
                t.fmt(aws_sdk_ec2::primitives::DateTimeFormat::DateTime)
                    .ok()
            })
            .flatten()
            .map(|s| s.to_string());

        let availability_zone = instance
            .placement()
            .and_then(|p| p.availability_zone())
            .map(|az| az.to_string());

        let vpc_id = instance.vpc_id().map(|id| id.to_string());

        let subnet_id = instance.subnet_id().map(|id| id.to_string());

        let ami_id = instance.image_id().map(|s| s.to_string());
        let key_name = instance.key_name().map(|s| s.to_string());
        let architecture = instance.architecture().map(|a| a.as_str().to_string());
        let platform = instance.platform().map(|p| p.as_str().to_string());
        let monitoring_state = instance
            .monitoring()
            .and_then(|m| m.state())
            .map(|s| s.as_str().to_string());
        let iam_profile_arn = instance
            .iam_instance_profile()
            .and_then(|p| p.arn())
            .map(|s| s.to_string());
        let root_device_name = instance.root_device_name().map(|s| s.to_string());
        let root_device_type = instance
            .root_device_type()
            .map(|t| t.as_str().to_string());

        let security_groups: Vec<(String, String)> = instance
            .security_groups()
            .iter()
            .map(|sg| {
                (
                    sg.group_id().unwrap_or("").to_string(),
                    sg.group_name().unwrap_or("").to_string(),
                )
            })
            .collect();

        let network_interfaces: Vec<EmbeddedNic> = instance
            .network_interfaces()
            .iter()
            .map(|ni| EmbeddedNic {
                interface_id: ni.network_interface_id().unwrap_or("").to_string(),
                description: ni.description().unwrap_or("").to_string(),
                status: ni
                    .status()
                    .map(|s| s.as_str().to_string())
                    .unwrap_or_default(),
                private_ip: ni.private_ip_address().map(|s| s.to_string()),
                public_ip: ni
                    .association()
                    .and_then(|a| a.public_ip())
                    .map(|s| s.to_string()),
                subnet_id: ni.subnet_id().map(|s| s.to_string()),
                vpc_id: ni.vpc_id().map(|s| s.to_string()),
                mac_address: ni.mac_address().map(|s| s.to_string()),
                security_groups: ni
                    .groups()
                    .iter()
                    .map(|g| {
                        (
                            g.group_id().unwrap_or("").to_string(),
                            g.group_name().unwrap_or("").to_string(),
                        )
                    })
                    .collect(),
            })
            .collect();

        let root_dev = root_device_name.as_deref().unwrap_or("");
        let block_devices: Vec<EmbeddedVolume> = instance
            .block_device_mappings()
            .iter()
            .map(|bdm| EmbeddedVolume {
                is_root: bdm.device_name().unwrap_or("") == root_dev,
                device_name: bdm.device_name().unwrap_or("").to_string(),
                volume_id: bdm
                    .ebs()
                    .and_then(|e| e.volume_id())
                    .unwrap_or("")
                    .to_string(),
                status: bdm
                    .ebs()
                    .and_then(|e| e.status())
                    .map(|s| s.as_str().to_string())
                    .unwrap_or_default(),
                delete_on_termination: bdm
                    .ebs()
                    .and_then(|e| e.delete_on_termination())
                    .unwrap_or(false),
            })
            .collect();

        // Parse tags
        let mut tags = HashMap::new();
        for tag in instance.tags() {
            if let (Some(key), Some(value)) = (tag.key(), tag.value()) {
                tags.insert(key.to_string(), value.to_string());
            }
        }

        Self {
            instance_id,
            instance_type,
            state,
            public_ip,
            private_ip,
            tags,
            launch_time,
            availability_zone,
            vpc_id,
            subnet_id,
            cost: None,
            ami_id,
            key_name,
            architecture,
            platform,
            monitoring_state,
            iam_profile_arn,
            root_device_name,
            root_device_type,
            security_groups,
            network_interfaces,
            block_devices,
        }
    }
}

crate::sections! {
    pub enum Ec2InstanceDetailSection,
    pub static EC2_INSTANCE_SECTIONS = [
        Details "Details",
        Security "Security" => crate::app::App::trigger_ec2_instance_profile_roles_load,
        Networking "Networking",
        Storage "Storage",
        UserData "User Data" => crate::app::App::trigger_ec2_instance_user_data_load,
        Tags "Tags",
        Optimizer "Optimizer" => crate::app::App::trigger_optimizer_load,
    ]
}

impl Resource for Ec2Instance {
    fn references(&self) -> Vec<(String, String)> {
        let mut v = sg_refs(self.security_group_ids());
        let mut r = |l: &str, x: &str| v.push((l.to_string(), x.to_string()));
        if let Some(x) = &self.subnet_id { r("Subnet", x); }
        if let Some(x) = &self.vpc_id { r("VPC", x); }
        if let Some(x) = &self.iam_profile_arn { r("IAM Instance Profile", x); }
        if let Some(x) = &self.ami_id { r("AMI", x); }
        if let Some(x) = &self.key_name { r("Key Pair", x); }
        for nic in &self.network_interfaces {
            r("Network Interface", &nic.interface_id);
            if let Some(x) = &nic.subnet_id { r("Subnet", x); }
            for (id, _) in &nic.security_groups { r("Security Group", id); }
        }
        for bd in &self.block_devices { r("Volume", &bd.volume_id); }
        v
    }

    fn security_group_ids(&self) -> Vec<String> {
        self.security_groups.iter().map(|(id, _)| id.clone()).collect()
    }

    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&EC2_INSTANCE_SECTIONS)
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws ec2 describe-instances --instance-ids {}",
            crate::aws::resource::shell_quote(self.id())
        ))
    }

    fn id(&self) -> &str {
        &self.instance_id
    }

    fn name(&self) -> &str {
        self.tags
            .get("Name")
            .map(|s| s.as_str())
            .unwrap_or(&self.instance_id)
    }

    fn resource_type(&self) -> &str {
        "EC2 Instance"
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn state(&self) -> ResourceState {
        match self.state.as_str() {
            "running" => ResourceState::Running,
            "stopped" => ResourceState::Stopped,
            "pending" => ResourceState::Pending,
            "stopping" => ResourceState::Pending,
            "shutting-down" => ResourceState::Deleting,
            "terminated" => ResourceState::Terminated,
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
        // Include all searchable fields for EC2 instances
        let mut search_parts = vec![
            self.instance_id.clone(),
            self.name().to_string(),
            self.instance_type.clone(),
            self.state.clone(),
        ];

        if let Some(public_ip) = &self.public_ip {
            search_parts.push(public_ip.clone());
        }

        if let Some(private_ip) = &self.private_ip {
            search_parts.push(private_ip.clone());
        }

        if let Some(az) = &self.availability_zone {
            search_parts.push(az.clone());
        }

        if let Some(vpc_id) = &self.vpc_id {
            search_parts.push(vpc_id.clone());
        }

        if let Some(subnet_id) = &self.subnet_id {
            search_parts.push(subnet_id.clone());
        }

        // Add tags
        for (k, v) in &self.tags {
            search_parts.push(format!("{}:{}", k, v));
        }

        search_parts.join(" ")
    }

    fn estimated_monthly_cost(&self) -> Option<f64> {
        self.cost
    }

    fn cost_trend(&self) -> Option<&str> {
        // Trend will be calculated separately in the UI layer
        // For now, return None
        None
    }

    fn details(&self) -> Vec<(String, String)> {
        let mut details = vec![
            ("Instance ID".to_string(), self.instance_id.clone()),
            ("Type".to_string(), self.instance_type.clone()),
            ("State".to_string(), self.state.clone()),
        ];

        if let Some(az) = &self.availability_zone {
            details.push(("Availability Zone".to_string(), az.clone()));
        }

        if let Some(launch) = &self.launch_time {
            details.push(("Launch Time".to_string(), launch.clone()));
        }

        if let Some(ip) = &self.public_ip {
            details.push(("Public IP".to_string(), ip.clone()));
        }

        if let Some(ip) = &self.private_ip {
            details.push(("Private IP".to_string(), ip.clone()));
        }

        // Add cost information if available
        if let Some(cost) = self.cost {
            details.push((
                "Estimated Monthly Cost".to_string(),
                format!("${:.2}", cost),
            ));
        }

        // Tags are handled separately by the UI via the tags() method
        // Don't include them in details to avoid duplication

        details
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{region}.console.aws.amazon.com/ec2/home?region={region}#Instances:instanceId={}",
            self.instance_id
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
}

// ── Security Group ────────────────────────────────────────────────────────────

/// A single, flattened security-group rule: one (protocol, port-range, source)
/// triple. AWS groups rules by protocol+port with multiple sources, so each
/// CIDR / SG reference / prefix-list of an `IpPermission` becomes its own rule.
#[derive(Clone, Debug)]
pub struct SgRule {
    pub protocol: String,             // "All" / "TCP" / "UDP" / "ICMP" / proto num
    pub ports: String,                // "443" / "80-443" / "All"
    pub source: String,               // CIDR, sg-id, or pl-id
    pub source_label: Option<String>, // SG name when the source is an SG reference
    pub description: Option<String>,
}

/// Flatten a list of `IpPermission`s into individual rules, preserving CIDRs,
/// IPv6 ranges, referenced security groups (with name), prefix lists, and
/// per-source descriptions.
pub fn parse_sg_rules(perms: &[aws_sdk_ec2::types::IpPermission]) -> Vec<SgRule> {
    let mut rules = Vec::new();
    for p in perms {
        let proto_raw = p.ip_protocol().unwrap_or("-1");
        let protocol = if proto_raw == "-1" {
            "All".to_string()
        } else {
            proto_raw.to_uppercase()
        };
        let ports = format_port_range(p.from_port(), p.to_port(), proto_raw);

        let mut push = |source: String, label: Option<String>, desc: Option<&str>| {
            rules.push(SgRule {
                protocol: protocol.clone(),
                ports: ports.clone(),
                source,
                source_label: label,
                description: desc.filter(|d| !d.is_empty()).map(|d| d.to_string()),
            });
        };

        for r in p.ip_ranges() {
            if let Some(cidr) = r.cidr_ip() {
                push(cidr.to_string(), None, r.description());
            }
        }
        for r in p.ipv6_ranges() {
            if let Some(cidr) = r.cidr_ipv6() {
                push(cidr.to_string(), None, r.description());
            }
        }
        for g in p.user_id_group_pairs() {
            let id = g.group_id().unwrap_or("sg-?").to_string();
            push(
                id,
                g.group_name().map(|s| s.to_string()),
                g.description(),
            );
        }
        for pl in p.prefix_list_ids() {
            if let Some(plid) = pl.prefix_list_id() {
                push(plid.to_string(), None, pl.description());
            }
        }
    }
    rules
}

/// Render a set of SG rules as detail rows under a group header, e.g.
/// `Inbound (2 rules)` then `  TCP 443    0.0.0.0/0`, with each rule's
/// description (if any) appended after a tab as a muted inline note.
pub fn sg_rule_rows(label: &str, rules: &[SgRule]) -> Vec<(String, String)> {
    let mut rows = vec![
        (
            format!(
                "{} ({} rule{})",
                label,
                rules.len(),
                if rules.len() == 1 { "" } else { "s" }
            ),
            String::new(),
        ),
        (String::new(), String::new()),
    ];

    if rules.is_empty() {
        rows.push((format!("  No {} rules", label.to_lowercase()), String::new()));
        return rows;
    }

    // Plain content lines (key starts with a space, empty value) so they render
    // as a fixed-width table rather than key/value pairs. Source goes last so a
    // long IPv6 CIDR or sg-id+name can't break column alignment.
    for r in rules {
        let source = match &r.source_label {
            Some(name) if !name.is_empty() => format!("{} ({})", r.source, name),
            _ => r.source.clone(),
        };
        let rule = format!("  {:<5}  {:<11}  →  {}", r.protocol, r.ports, source);
        // Attach the per-rule description (e.g. "my home ip") to the same line
        // after a tab; `style_detail_row` renders the trailing part as a muted
        // note so it reads as an annotation on the rule, not another data column.
        let line = match &r.description {
            Some(desc) => format!("{rule}\t{desc}"),
            None => rule,
        };
        rows.push((line, String::new()));
    }
    rows
}

#[derive(Clone, Debug)]
pub struct SecurityGroup {
    pub group_id: String,
    pub group_name: String,
    pub description: String,
    pub vpc_id: Option<String>,
    pub inbound_rules: Vec<SgRule>,
    pub outbound_rules: Vec<SgRule>,
    pub tags: HashMap<String, String>,
}

impl SecurityGroup {
    pub fn from_sdk(sg: &aws_sdk_ec2::types::SecurityGroup) -> Self {
        let group_id = sg.group_id().unwrap_or("unknown").to_string();
        let group_name = sg.group_name().unwrap_or("unknown").to_string();
        let description = sg.description().unwrap_or("").to_string();
        let vpc_id = sg.vpc_id().map(|s| s.to_string());

        let mut tags = HashMap::new();
        for tag in sg.tags() {
            if let (Some(k), Some(v)) = (tag.key(), tag.value()) {
                tags.insert(k.to_string(), v.to_string());
            }
        }

        Self {
            group_id,
            group_name,
            description,
            vpc_id,
            inbound_rules: parse_sg_rules(sg.ip_permissions()),
            outbound_rules: parse_sg_rules(sg.ip_permissions_egress()),
            tags,
        }
    }
}

crate::sections! {
    pub enum SecurityGroupDetailSection,
    pub static SECURITY_GROUP_SECTIONS = [
        Inbound "Inbound",
        Outbound "Outbound",
        UsedBy "Used By" => crate::app::App::trigger_sg_enis_load,
        Tags "Tags",
    ]
}

impl Resource for SecurityGroup {
    fn security_group_ids(&self) -> Vec<String> {
        vec![self.group_id.clone()]
    }

    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&SECURITY_GROUP_SECTIONS)
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws ec2 describe-security-groups --group-ids {}",
            crate::aws::resource::shell_quote(self.id())
        ))
    }

    fn id(&self) -> &str {
        &self.group_id
    }

    fn name(&self) -> &str {
        self.tags
            .get("Name")
            .map(|s| s.as_str())
            .unwrap_or(&self.group_name)
    }

    fn resource_type(&self) -> &str {
        "Security Group"
    }

    fn state(&self) -> ResourceState {
        ResourceState::stateless()
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        let mut parts = vec![
            self.group_id.clone(),
            self.group_name.clone(),
            self.description.clone(),
        ];
        if let Some(vpc) = &self.vpc_id {
            parts.push(vpc.clone());
        }
        for (k, v) in &self.tags {
            parts.push(format!("{}:{}", k, v));
        }
        parts.join(" ")
    }

    fn details(&self) -> Vec<(String, String)> {
        let mut details = vec![
            ("Group ID".to_string(), self.group_id.clone()),
            ("Group Name".to_string(), self.group_name.clone()),
            ("Description".to_string(), self.description.clone()),
        ];
        if let Some(vpc) = &self.vpc_id {
            details.push(("VPC ID".to_string(), vpc.clone()));
        }
        details.push(("".to_string(), "".to_string()));
        details.extend(sg_rule_rows("Inbound", &self.inbound_rules));
        details.push(("".to_string(), "".to_string()));
        details.extend(sg_rule_rows("Outbound", &self.outbound_rules));
        details
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{region}.console.aws.amazon.com/ec2/home?region={region}#SecurityGroup:groupId={}",
            self.group_id
        ))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
}

// ── EBS Volume ────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct EbsVolume {
    pub volume_id: String,
    pub size_gb: i32,
    pub volume_type: String,
    pub state: String,
    pub availability_zone: String,
    pub encrypted: bool,
    pub iops: Option<i32>,
    pub throughput: Option<i32>,
    pub snapshot_id: Option<String>,
    pub attached_instance_id: Option<String>,
    pub kms_key_id: Option<String>,
    pub create_time: String,
    pub multi_attach: bool,
    pub attachments: Vec<EbsAttachment>,
    pub tags: HashMap<String, String>,
}

#[derive(Clone, Debug)]
pub struct EbsAttachment {
    pub instance_id: String,
    pub device: String,
    pub state: String,
    pub delete_on_termination: bool,
}

impl EbsVolume {
    pub fn from_sdk(vol: &aws_sdk_ec2::types::Volume) -> Self {
        let volume_id = vol.volume_id().unwrap_or("unknown").to_string();
        let size_gb = vol.size().unwrap_or(0);
        let volume_type = vol
            .volume_type()
            .map(|t| t.as_str().to_string())
            .unwrap_or_else(|| "unknown".to_string());
        let state = vol
            .state()
            .map(|s| s.as_str().to_string())
            .unwrap_or_else(|| "unknown".to_string());
        let availability_zone = vol.availability_zone().unwrap_or("unknown").to_string();
        let encrypted = vol.encrypted().unwrap_or(false);
        let iops = vol.iops();
        let throughput = vol.throughput();
        let snapshot_id = vol.snapshot_id().filter(|s| !s.is_empty()).map(|s| s.to_string());
        let attachments: Vec<EbsAttachment> = vol
            .attachments()
            .iter()
            .map(|a| EbsAttachment {
                instance_id: a.instance_id().unwrap_or_default().to_string(),
                device: a.device().unwrap_or_default().to_string(),
                state: a
                    .state()
                    .map(|s| s.as_str().to_string())
                    .unwrap_or_default(),
                delete_on_termination: a.delete_on_termination().unwrap_or(false),
            })
            .collect();
        let attached_instance_id = attachments.first().map(|a| a.instance_id.clone());

        let mut tags = HashMap::new();
        for tag in vol.tags() {
            if let (Some(k), Some(v)) = (tag.key(), tag.value()) {
                tags.insert(k.to_string(), v.to_string());
            }
        }

        Self {
            volume_id,
            size_gb,
            volume_type,
            state,
            availability_zone,
            encrypted,
            iops,
            throughput,
            snapshot_id,
            attached_instance_id,
            kms_key_id: vol.kms_key_id().filter(|s| !s.is_empty()).map(|s| s.to_string()),
            create_time: vol
                .create_time()
                .map(|t| t.to_string())
                .unwrap_or_default(),
            multi_attach: vol.multi_attach_enabled().unwrap_or(false),
            attachments,
            tags,
        }
    }
}

crate::sections! {
    pub enum EbsVolumeDetailSection,
    pub static EBS_VOLUME_SECTIONS = [
        Details "Details",
        Attachments "Attachments",
        Snapshots "Snapshots" => crate::app::App::trigger_ebs_snapshots_load,
        Tags "Tags",
        Optimizer "Optimizer" => crate::app::App::trigger_optimizer_load,
    ]
}

impl Resource for EbsVolume {
    fn references(&self) -> Vec<(String, String)> {
        let mut v = Vec::new();
        let mut r = |l: &str, x: &str| v.push((l.to_string(), x.to_string()));
        if let Some(x) = &self.attached_instance_id { r("Attached Instance", x); }
        for a in &self.attachments { r("Attached Instance", &a.instance_id); }
        if let Some(x) = &self.kms_key_id { r("KMS Key", x); }
        if let Some(x) = &self.snapshot_id { r("Snapshot", x); }
        v
    }

    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&EBS_VOLUME_SECTIONS)
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws ec2 describe-volumes --volume-ids {}",
            crate::aws::resource::shell_quote(self.id())
        ))
    }

    fn id(&self) -> &str {
        &self.volume_id
    }

    fn name(&self) -> &str {
        self.tags
            .get("Name")
            .map(|s| s.as_str())
            .unwrap_or(&self.volume_id)
    }

    fn resource_type(&self) -> &str {
        "EBS Volume"
    }

    fn state(&self) -> ResourceState {
        match self.state.as_str() {
            "available" => ResourceState::Available,
            "in-use" => ResourceState::Running,
            "creating" => ResourceState::Pending,
            "deleting" | "deleted" => ResourceState::Deleting,
            "error" => ResourceState::Unknown("error".to_string()),
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
        let mut parts = vec![
            self.volume_id.clone(),
            self.name().to_string(),
            self.volume_type.clone(),
            self.state.clone(),
            self.availability_zone.clone(),
            format!("{}GiB", self.size_gb),
        ];
        if let Some(inst) = &self.attached_instance_id {
            parts.push(inst.clone());
        }
        if let Some(snap) = &self.snapshot_id {
            parts.push(snap.clone());
        }
        for (k, v) in &self.tags {
            parts.push(format!("{}:{}", k, v));
        }
        parts.join(" ")
    }

    fn details(&self) -> Vec<(String, String)> {
        let mut details = vec![
            ("Volume ID".to_string(), self.volume_id.clone()),
            ("Size".to_string(), format!("{} GiB", self.size_gb)),
            ("Type".to_string(), self.volume_type.clone()),
            ("State".to_string(), self.state.clone()),
            ("Availability Zone".to_string(), self.availability_zone.clone()),
            ("Encrypted".to_string(), if self.encrypted { "Yes" } else { "No" }.to_string()),
        ];
        if let Some(iops) = self.iops {
            details.push(("IOPS".to_string(), iops.to_string()));
        }
        if let Some(tp) = self.throughput {
            details.push(("Throughput".to_string(), format!("{} MiB/s", tp)));
        }
        if let Some(snap) = &self.snapshot_id {
            details.push(("Snapshot".to_string(), snap.clone()));
        }
        if let Some(inst) = &self.attached_instance_id {
            details.push(("Attached To".to_string(), inst.clone()));
        }
        details
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{region}.console.aws.amazon.com/ec2/home?region={region}#VolumeDetails:volumeId={}",
            self.volume_id
        ))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
}

// ── Network Interface ─────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct NetworkInterface {
    pub interface_id: String,
    pub description: String,
    pub interface_type: String,
    /// Primary private IPv4 — the list-row summary IP.
    pub private_ip: Option<String>,
    pub public_ip: Option<String>,
    /// Every private IPv4 on the interface: (ip, is_primary). Multi-IP ENIs
    /// (EKS nodes, secondary-IP setups) carry many — all searchable.
    pub private_ips: Vec<(String, bool)>,
    pub ipv6_addresses: Vec<String>,
    pub private_dns: Option<String>,
    pub subnet_id: Option<String>,
    pub vpc_id: Option<String>,
    pub availability_zone: Option<String>,
    pub status: String,
    pub attached_instance_id: Option<String>,
    pub attachment: Option<EniAttachment>,
    /// (group id, group name)
    pub security_groups: Vec<(String, String)>,
    pub owner_id: Option<String>,
    /// Who created the interface — managed-service ENIs carry the service's
    /// requester id (e.g. amazon-elb, the Lambda service principal).
    pub requester_id: Option<String>,
    pub requester_managed: bool,
    pub tags: HashMap<String, String>,
}

#[derive(Clone, Debug)]
pub struct EniAttachment {
    pub instance_id: Option<String>,
    /// "amazon-elb" / "amazon-aws" for managed-service attachments.
    pub instance_owner: Option<String>,
    pub device_index: Option<i32>,
    pub status: Option<String>,
    pub delete_on_termination: bool,
}

impl NetworkInterface {
    pub fn from_sdk(eni: &aws_sdk_ec2::types::NetworkInterface) -> Self {
        let interface_id = eni.network_interface_id().unwrap_or("unknown").to_string();
        let description = eni.description().unwrap_or("").to_string();
        let interface_type = eni
            .interface_type()
            .map(|t| t.as_str().to_string())
            .unwrap_or_else(|| "interface".to_string());
        let private_ip = eni.private_ip_address().map(|s| s.to_string());
        let public_ip = eni
            .association()
            .and_then(|a| a.public_ip())
            .map(|s| s.to_string());
        let private_ips: Vec<(String, bool)> = eni
            .private_ip_addresses()
            .iter()
            .filter_map(|p| {
                p.private_ip_address()
                    .map(|ip| (ip.to_string(), p.primary().unwrap_or(false)))
            })
            .collect();
        let ipv6_addresses: Vec<String> = eni
            .ipv6_addresses()
            .iter()
            .filter_map(|a| a.ipv6_address().map(|s| s.to_string()))
            .collect();
        let private_dns = eni
            .private_dns_name()
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());
        let subnet_id = eni.subnet_id().map(|s| s.to_string());
        let vpc_id = eni.vpc_id().map(|s| s.to_string());
        let availability_zone = eni.availability_zone().map(|s| s.to_string());
        let status = eni
            .status()
            .map(|s| s.as_str().to_string())
            .unwrap_or_else(|| "unknown".to_string());
        let attachment = eni.attachment().map(|a| EniAttachment {
            instance_id: a.instance_id().map(|s| s.to_string()),
            instance_owner: a.instance_owner_id().map(|s| s.to_string()),
            device_index: a.device_index(),
            status: a.status().map(|s| s.as_str().to_string()),
            delete_on_termination: a.delete_on_termination().unwrap_or(false),
        });
        let attached_instance_id = attachment
            .as_ref()
            .and_then(|a| a.instance_id.clone());
        let security_groups: Vec<(String, String)> = eni
            .groups()
            .iter()
            .map(|g| {
                (
                    g.group_id().unwrap_or_default().to_string(),
                    g.group_name().unwrap_or_default().to_string(),
                )
            })
            .collect();
        let owner_id = eni.owner_id().map(|s| s.to_string());
        let requester_id = eni.requester_id().map(|s| s.to_string());
        let requester_managed = eni.requester_managed().unwrap_or(false);

        let mut tags = HashMap::new();
        for tag in eni.tag_set() {
            if let (Some(k), Some(v)) = (tag.key(), tag.value()) {
                tags.insert(k.to_string(), v.to_string());
            }
        }

        Self {
            interface_id,
            description,
            interface_type,
            private_ip,
            public_ip,
            private_ips,
            ipv6_addresses,
            private_dns,
            subnet_id,
            vpc_id,
            availability_zone,
            status,
            attached_instance_id,
            attachment,
            security_groups,
            owner_id,
            requester_id,
            requester_managed,
            tags,
        }
    }
}

crate::sections! {
    pub enum EniDetailSection,
    pub static ENI_SECTIONS = [
        Overview "Overview",
        Addresses "Addresses",
        Attachment "Attachment",
        Tags "Tags",
    ]
}

impl Resource for NetworkInterface {
    fn references(&self) -> Vec<(String, String)> {
        let mut v = sg_refs(self.security_group_ids());
        let mut r = |l: &str, x: &str| v.push((l.to_string(), x.to_string()));
        if let Some(x) = &self.subnet_id { r("Subnet", x); }
        if let Some(x) = &self.vpc_id { r("VPC", x); }
        if let Some(x) = &self.attached_instance_id { r("Attached Instance", x); }
        v
    }

    fn security_group_ids(&self) -> Vec<String> {
        self.security_groups.iter().map(|(id, _)| id.clone()).collect()
    }

    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&ENI_SECTIONS)
    }

    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws ec2 describe-network-interfaces --network-interface-ids {}",
            crate::aws::resource::shell_quote(self.id())
        ))
    }

    fn id(&self) -> &str {
        &self.interface_id
    }

    fn name(&self) -> &str {
        self.tags
            .get("Name")
            .map(|s| s.as_str())
            .unwrap_or(&self.interface_id)
    }

    fn resource_type(&self) -> &str {
        "Network Interface"
    }

    fn state(&self) -> ResourceState {
        match self.status.as_str() {
            "available" => ResourceState::Available,
            "in-use" => ResourceState::Running,
            "attaching" | "detaching" => ResourceState::Pending,
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
        let mut parts = vec![
            self.interface_id.clone(),
            self.description.clone(),
            self.interface_type.clone(),
            self.status.clone(),
        ];
        // Every address — the ENI list is the reverse index for "whose IP
        // is this?", so secondaries and IPv6 must match too.
        if let Some(ip) = &self.private_ip {
            parts.push(ip.clone());
        }
        for (ip, _) in &self.private_ips {
            parts.push(ip.clone());
        }
        parts.extend(self.ipv6_addresses.iter().cloned());
        if let Some(ip) = &self.public_ip {
            parts.push(ip.clone());
        }
        if let Some(dns) = &self.private_dns {
            parts.push(dns.clone());
        }
        if let Some(sub) = &self.subnet_id {
            parts.push(sub.clone());
        }
        if let Some(vpc) = &self.vpc_id {
            parts.push(vpc.clone());
        }
        if let Some(z) = &self.availability_zone {
            parts.push(z.clone());
        }
        if let Some(inst) = &self.attached_instance_id {
            parts.push(inst.clone());
        }
        for (id, name) in &self.security_groups {
            parts.push(id.clone());
            parts.push(name.clone());
        }
        if let Some(r) = &self.requester_id {
            parts.push(r.clone());
        }
        for (k, v) in &self.tags {
            parts.push(format!("{}:{}", k, v));
        }
        parts.join(" ")
    }

    fn details(&self) -> Vec<(String, String)> {
        let mut details = vec![
            ("Interface ID".to_string(), self.interface_id.clone()),
            ("Type".to_string(), self.interface_type.clone()),
            ("Status".to_string(), self.status.clone()),
        ];
        if !self.description.is_empty() {
            details.push(("Description".to_string(), self.description.clone()));
        }
        if let Some(ip) = &self.private_ip {
            details.push(("Private IP".to_string(), ip.clone()));
        }
        for (ip, primary) in &self.private_ips {
            if !primary {
                details.push(("Secondary IP".to_string(), ip.clone()));
            }
        }
        if let Some(ip) = &self.public_ip {
            details.push(("Public IP".to_string(), ip.clone()));
        }
        for a in &self.ipv6_addresses {
            details.push(("IPv6".to_string(), a.clone()));
        }
        if let Some(sub) = &self.subnet_id {
            details.push(("Subnet ID".to_string(), sub.clone()));
        }
        if let Some(vpc) = &self.vpc_id {
            details.push(("VPC ID".to_string(), vpc.clone()));
        }
        if let Some(z) = &self.availability_zone {
            details.push(("Availability Zone".to_string(), z.clone()));
        }
        for (id, _) in &self.security_groups {
            details.push(("Security Group".to_string(), id.clone()));
        }
        if let Some(inst) = &self.attached_instance_id {
            details.push(("Attached To".to_string(), inst.clone()));
        }
        details
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{region}.console.aws.amazon.com/ec2/home?region={region}#NetworkInterface:networkInterfaceId={}",
            self.interface_id
        ))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
}

// ── EC2 metrics (CloudWatch) ──────────────────────────────────────────────────

/// Parse one `GetMetricStatistics` response into sorted `(t-offset, value)`
/// points, reading whichever single statistic the query asked for
/// (Sum / Maximum / Average). Shared by the simple per-dimension fetchers.
pub fn parse_metric_datapoints(
    resp: std::result::Result<
        aws_sdk_cloudwatch::operation::get_metric_statistics::GetMetricStatisticsOutput,
        aws_sdk_cloudwatch::error::SdkError<
            aws_sdk_cloudwatch::operation::get_metric_statistics::GetMetricStatisticsError,
        >,
    >,
    start: i64,
) -> Vec<(f64, f64)> {
    let mut pts: Vec<(f64, f64)> = match resp {
        Ok(r) => r
            .datapoints()
            .iter()
            .filter_map(|dp| {
                let v = dp.sum().or(dp.maximum()).or(dp.average())?;
                Some((dp.timestamp()?.secs() as f64 - start as f64, v))
            })
            .collect(),
        Err(_) => vec![],
    };
    pts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    pts
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MetricsTimeRange {
    OneHour,
    SixHours,
    TwentyFourHours,
    SevenDays,
}

impl MetricsTimeRange {
    pub fn duration_secs(self) -> i64 {
        match self {
            Self::OneHour => 3_600,
            Self::SixHours => 21_600,
            Self::TwentyFourHours => 86_400,
            Self::SevenDays => 604_800,
        }
    }

    pub fn period_secs(self) -> i32 {
        match self {
            Self::OneHour => 60,
            Self::SixHours => 300,
            Self::TwentyFourHours => 3_600,
            Self::SevenDays => 3_600,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::OneHour => "1h",
            Self::SixHours => "6h",
            Self::TwentyFourHours => "24h",
            Self::SevenDays => "7d",
        }
    }

    pub fn start_label(self) -> &'static str {
        match self {
            Self::OneHour => "1h ago",
            Self::SixHours => "6h ago",
            Self::TwentyFourHours => "24h ago",
            Self::SevenDays => "7d ago",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::OneHour => Self::SixHours,
            Self::SixHours => Self::TwentyFourHours,
            Self::TwentyFourHours => Self::SevenDays,
            Self::SevenDays => Self::OneHour,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            Self::OneHour => Self::SevenDays,
            Self::SixHours => Self::OneHour,
            Self::TwentyFourHours => Self::SixHours,
            Self::SevenDays => Self::TwentyFourHours,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Ec2MetricsData {
    pub time_range: MetricsTimeRange,
    pub cpu: Vec<(f64, f64)>,     // (secs_from_start, percent)
    pub net_in: Vec<(f64, f64)>,  // (secs_from_start, bytes/sec)
    pub net_out: Vec<(f64, f64)>, // (secs_from_start, bytes/sec)
    pub x_max: f64,
}

#[derive(Debug, Clone)]
pub enum Ec2MetricsState {
    Loading,
    Loaded(Ec2MetricsData),
}

fn parse_datapoints(
    datapoints: &[aws_sdk_cloudwatch::types::Datapoint],
    start_secs: i64,
    per_sec_divisor: f64,
) -> Vec<(f64, f64)> {
    let mut pts: Vec<(f64, f64)> = datapoints
        .iter()
        .filter_map(|dp| {
            let t = (dp.timestamp()?.secs() - start_secs) as f64;
            let v = dp.average()? / per_sec_divisor;
            Some((t, v))
        })
        .collect();
    pts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    pts
}

pub async fn fetch_ec2_metrics(
    cw_client: aws_sdk_cloudwatch::Client,
    instance_id: String,
    time_range: MetricsTimeRange,
) -> Result<Ec2MetricsData> {
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let start_secs = now_secs - time_range.duration_secs();
    let period = time_range.period_secs();

    let start_dt = CwDateTime::from_secs(start_secs);
    let end_dt = CwDateTime::from_secs(now_secs);

    let make_dim = || {
        aws_sdk_cloudwatch::types::Dimension::builder()
            .name("InstanceId")
            .value(&instance_id)
            .build()
    };

    let (cpu_resp, net_in_resp, net_out_resp) = tokio::join!(
        cw_client
            .get_metric_statistics()
            .namespace("AWS/EC2")
            .metric_name("CPUUtilization")
            .dimensions(make_dim())
            .start_time(start_dt.clone())
            .end_time(end_dt.clone())
            .period(period)
            .set_statistics(Some(vec![aws_sdk_cloudwatch::types::Statistic::Average]))
            .send(),
        cw_client
            .get_metric_statistics()
            .namespace("AWS/EC2")
            .metric_name("NetworkIn")
            .dimensions(make_dim())
            .start_time(start_dt.clone())
            .end_time(end_dt.clone())
            .period(period)
            .set_statistics(Some(vec![aws_sdk_cloudwatch::types::Statistic::Average]))
            .send(),
        cw_client
            .get_metric_statistics()
            .namespace("AWS/EC2")
            .metric_name("NetworkOut")
            .dimensions(make_dim())
            .start_time(start_dt)
            .end_time(end_dt)
            .period(period)
            .set_statistics(Some(vec![aws_sdk_cloudwatch::types::Statistic::Average]))
            .send(),
    );

    let period_f = period as f64;

    let cpu_dp = match cpu_resp { Ok(r) => r.datapoints().to_vec(), Err(_) => vec![] };
    let net_in_dp = match net_in_resp { Ok(r) => r.datapoints().to_vec(), Err(_) => vec![] };
    let net_out_dp = match net_out_resp { Ok(r) => r.datapoints().to_vec(), Err(_) => vec![] };

    Ok(Ec2MetricsData {
        time_range,
        cpu: parse_datapoints(&cpu_dp, start_secs, 1.0),
        net_in: parse_datapoints(&net_in_dp, start_secs, period_f),
        net_out: parse_datapoints(&net_out_dp, start_secs, period_f),
        x_max: time_range.duration_secs() as f64,
    })
}

#[derive(Debug, Clone)]
pub struct EbsMetricsData {
    pub time_range: MetricsTimeRange,
    pub read_iops: Vec<(f64, f64)>,     // ops/sec
    pub write_iops: Vec<(f64, f64)>,    // ops/sec
    pub read_tput: Vec<(f64, f64)>,     // MB/sec
    pub write_tput: Vec<(f64, f64)>,    // MB/sec
    pub queue_length: Vec<(f64, f64)>,  // avg outstanding ops
    pub burst_balance: Vec<(f64, f64)>, // percent (gp2/st1/sc1 only)
    pub x_max: f64,
}

#[derive(Debug, Clone)]
pub enum EbsMetricsState {
    Loading,
    Loaded(EbsMetricsData),
}

/// Parse `Sum` datapoints into a per-second rate: `sum / period / unit_divisor`
/// (unit_divisor = 1 for ops/sec, 1e6 for bytes→MB/sec).
fn parse_sum_rate(
    datapoints: &[aws_sdk_cloudwatch::types::Datapoint],
    start_secs: i64,
    period: f64,
    unit_divisor: f64,
) -> Vec<(f64, f64)> {
    let mut pts: Vec<(f64, f64)> = datapoints
        .iter()
        .filter_map(|dp| {
            let t = (dp.timestamp()?.secs() - start_secs) as f64;
            let v = dp.sum()? / period / unit_divisor;
            Some((t, v))
        })
        .collect();
    pts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    pts
}

/// EBS volume performance metrics (namespace `AWS/EBS`, dim `VolumeId`):
/// read/write IOPS + throughput (from `Volume{Read,Write}{Ops,Bytes}` Sums
/// divided by the period), average queue length, and BurstBalance %
/// (only emitted for gp2/st1/sc1 — empty otherwise).
pub async fn fetch_ebs_metrics(
    cw_client: aws_sdk_cloudwatch::Client,
    volume_id: String,
    time_range: MetricsTimeRange,
) -> Result<EbsMetricsData> {
    use aws_sdk_cloudwatch::types::Statistic;

    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let start_secs = now_secs - time_range.duration_secs();
    let period = time_range.period_secs();
    let start_dt = CwDateTime::from_secs(start_secs);
    let end_dt = CwDateTime::from_secs(now_secs);

    let make_dim = || {
        aws_sdk_cloudwatch::types::Dimension::builder()
            .name("VolumeId")
            .value(&volume_id)
            .build()
    };
    let sum_metric = |name: &'static str| {
        cw_client
            .get_metric_statistics()
            .namespace("AWS/EBS")
            .metric_name(name)
            .dimensions(make_dim())
            .start_time(start_dt.clone())
            .end_time(end_dt.clone())
            .period(period)
            .set_statistics(Some(vec![Statistic::Sum]))
            .send()
    };
    let avg_metric = |name: &'static str| {
        cw_client
            .get_metric_statistics()
            .namespace("AWS/EBS")
            .metric_name(name)
            .dimensions(make_dim())
            .start_time(start_dt.clone())
            .end_time(end_dt.clone())
            .period(period)
            .set_statistics(Some(vec![Statistic::Average]))
            .send()
    };

    let (read_ops, write_ops, read_bytes, write_bytes, queue, burst) = tokio::join!(
        sum_metric("VolumeReadOps"),
        sum_metric("VolumeWriteOps"),
        sum_metric("VolumeReadBytes"),
        sum_metric("VolumeWriteBytes"),
        avg_metric("VolumeQueueLength"),
        avg_metric("BurstBalance"),
    );

    let dps = |r: std::result::Result<
        aws_sdk_cloudwatch::operation::get_metric_statistics::GetMetricStatisticsOutput,
        _,
    >| match r {
        Ok(o) => o.datapoints().to_vec(),
        Err(_) => vec![],
    };
    let period_f = period as f64;

    Ok(EbsMetricsData {
        time_range,
        read_iops: parse_sum_rate(&dps(read_ops), start_secs, period_f, 1.0),
        write_iops: parse_sum_rate(&dps(write_ops), start_secs, period_f, 1.0),
        read_tput: parse_sum_rate(&dps(read_bytes), start_secs, period_f, 1_000_000.0),
        write_tput: parse_sum_rate(&dps(write_bytes), start_secs, period_f, 1_000_000.0),
        queue_length: parse_datapoints(&dps(queue), start_secs, 1.0),
        burst_balance: parse_datapoints(&dps(burst), start_secs, 1.0),
        x_max: time_range.duration_secs() as f64,
    })
}

// ── Security Group: lazy "used by" network interfaces ───────────────────────

#[derive(Clone, Debug)]
pub struct SgEni {
    pub eni_id: String,
    pub description: String,
    pub private_ip: String,
    pub attached_to: String, // instance id, or the interface type for AWS-managed ENIs
    pub status: String,
}

/// Fetch the network interfaces a security group is attached to (`group-id`
/// filter) — the "what would I break if I change this" view.
pub async fn fetch_sg_network_interfaces(
    client: Ec2Client,
    group_id: String,
) -> Result<Vec<SgEni>> {
    let mut out = Vec::new();
    let mut pager = client
        .describe_network_interfaces()
        .filters(
            aws_sdk_ec2::types::Filter::builder()
                .name("group-id")
                .values(&group_id)
                .build(),
        )
        .into_paginator()
        .items()
        .send();
    while let Some(result) = pager.next().await {
        match result {
            Ok(eni) => {
                let attached_to = eni
                    .attachment()
                    .and_then(|a| a.instance_id().map(|s| s.to_string()))
                    .or_else(|| eni.interface_type().map(|t| t.as_str().to_string()))
                    .unwrap_or_default();
                out.push(SgEni {
                    eni_id: eni.network_interface_id().unwrap_or_default().to_string(),
                    description: eni.description().unwrap_or_default().to_string(),
                    private_ip: eni.private_ip_address().unwrap_or_default().to_string(),
                    attached_to,
                    status: eni
                        .status()
                        .map(|s| s.as_str().to_string())
                        .unwrap_or_default(),
                });
            }
            Err(e) => return Err(crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e))),
        }
    }
    Ok(out)
}

// ── EBS volume: lazy snapshots ──────────────────────────────────────────────

/// One EBS snapshot. Backs both the per-volume lazy Snapshots section on an EBS
/// volume (`fetch_volume_snapshots`, which leaves the extra Option fields None)
/// and the account-wide **Snapshots** sub-tab (`EbsSnapshot::from_sdk`, which
/// fills them).
#[derive(Clone, Debug)]
pub struct EbsSnapshot {
    pub snapshot_id: String,
    pub state: String,
    pub progress: String,
    pub started: String,
    pub size_gb: i32,
    pub description: String,
    pub volume_id: Option<String>,
    pub encrypted: bool,
    pub kms_key_id: Option<String>,
    pub owner_id: Option<String>,
    pub storage_tier: Option<String>,
    pub state_message: Option<String>,
    pub restore_expiry: Option<String>,
    pub tags: HashMap<String, String>,
}

impl EbsSnapshot {
    pub fn from_sdk(s: &aws_sdk_ec2::types::Snapshot) -> Self {
        let mut tags = HashMap::new();
        for tag in s.tags() {
            if let (Some(k), Some(v)) = (tag.key(), tag.value()) {
                tags.insert(k.to_string(), v.to_string());
            }
        }
        Self {
            snapshot_id: s.snapshot_id().unwrap_or_default().to_string(),
            state: s
                .state()
                .map(|st| st.as_str().to_string())
                .unwrap_or_default(),
            progress: s.progress().unwrap_or_default().to_string(),
            started: s.start_time().map(|t| t.to_string()).unwrap_or_default(),
            size_gb: s.volume_size().unwrap_or(0),
            description: s.description().unwrap_or_default().to_string(),
            volume_id: s
                .volume_id()
                .filter(|v| !v.is_empty() && *v != "vol-ffffffff")
                .map(|v| v.to_string()),
            encrypted: s.encrypted().unwrap_or(false),
            kms_key_id: s.kms_key_id().filter(|k| !k.is_empty()).map(|k| k.to_string()),
            owner_id: s.owner_id().filter(|o| !o.is_empty()).map(|o| o.to_string()),
            storage_tier: s.storage_tier().map(|t| t.as_str().to_string()),
            state_message: s
                .state_message()
                .filter(|m| !m.is_empty())
                .map(|m| m.to_string()),
            restore_expiry: s.restore_expiry_time().map(|t| t.to_string()),
            tags,
        }
    }
}

crate::sections! {
    pub enum SnapshotDetailSection,
    pub static EBS_SNAPSHOT_SECTIONS = [
        Details "Details",
        Tags "Tags",
    ]
}

impl Resource for EbsSnapshot {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&EBS_SNAPSHOT_SECTIONS)
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws ec2 describe-snapshots --snapshot-ids {}",
            crate::aws::resource::shell_quote(self.id())
        ))
    }

    fn id(&self) -> &str {
        &self.snapshot_id
    }

    fn name(&self) -> &str {
        self.tags
            .get("Name")
            .map(|s| s.as_str())
            .unwrap_or(&self.snapshot_id)
    }

    fn resource_type(&self) -> &str {
        "EBS Snapshot"
    }

    fn state(&self) -> ResourceState {
        match self.state.as_str() {
            "completed" => ResourceState::Available,
            "pending" => ResourceState::Pending,
            "recoverable" => ResourceState::Stopped,
            "error" => ResourceState::Unknown("error".to_string()),
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
        let mut parts = vec![
            self.snapshot_id.clone(),
            self.name().to_string(),
            self.state.clone(),
            self.description.clone(),
            format!("{}GiB", self.size_gb),
        ];
        if let Some(v) = &self.volume_id {
            parts.push(v.clone());
        }
        for (k, v) in &self.tags {
            parts.push(format!("{}:{}", k, v));
        }
        parts.join(" ")
    }

    fn details(&self) -> Vec<(String, String)> {
        let mut d = vec![
            ("Snapshot ID".to_string(), self.snapshot_id.clone()),
            ("State".to_string(), self.state.clone()),
            ("Progress".to_string(), self.progress.clone()),
            ("Size".to_string(), format!("{} GiB", self.size_gb)),
        ];
        if let Some(v) = &self.volume_id {
            d.push(("Volume".to_string(), v.clone()));
        }
        if !self.description.is_empty() {
            d.push(("Description".to_string(), self.description.clone()));
        }
        d
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{region}.console.aws.amazon.com/ec2/home?region={region}#SnapshotDetails:snapshotId={}",
            self.snapshot_id
        ))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
}

/// Fetch the snapshots taken from a volume (`volume-id` filter), most recent
/// first.
pub async fn fetch_volume_snapshots(
    client: Ec2Client,
    volume_id: String,
) -> Result<Vec<EbsSnapshot>> {
    let mut out: Vec<(i64, EbsSnapshot)> = Vec::new();
    let mut pager = client
        .describe_snapshots()
        .filters(
            aws_sdk_ec2::types::Filter::builder()
                .name("volume-id")
                .values(&volume_id)
                .build(),
        )
        .into_paginator()
        .items()
        .send();
    while let Some(result) = pager.next().await {
        match result {
            Ok(s) => {
                let epoch = s.start_time().map(|t| t.secs()).unwrap_or(0);
                out.push((epoch, EbsSnapshot::from_sdk(&s)));
            }
            Err(e) => return Err(crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e))),
        }
    }
    out.sort_by(|a, b| b.0.cmp(&a.0));
    Ok(out.into_iter().map(|(_, s)| s).collect())
}

// ── AMI ─────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct AmiBlockDevice {
    pub device_name: String,
    pub snapshot_id: Option<String>,
    pub size_gb: i32,
    pub volume_type: String,
    pub delete_on_termination: bool,
}

#[derive(Clone, Debug)]
pub struct Ami {
    pub image_id: String,
    pub name: String,
    pub description: String,
    pub state: String,
    pub architecture: String,
    pub virtualization_type: String,
    pub root_device_type: String,
    pub root_device_name: String,
    pub hypervisor: String,
    pub ena_support: bool,
    pub creation_date: Option<String>,
    pub deprecation_time: Option<String>,
    pub public: bool,
    pub platform: Option<String>,
    pub block_devices: Vec<AmiBlockDevice>,
    pub tags: HashMap<String, String>,
}

impl Ami {
    pub fn from_sdk(img: &aws_sdk_ec2::types::Image) -> Self {
        let mut tags = HashMap::new();
        for tag in img.tags() {
            if let (Some(k), Some(v)) = (tag.key(), tag.value()) {
                tags.insert(k.to_string(), v.to_string());
            }
        }
        let block_devices: Vec<AmiBlockDevice> = img
            .block_device_mappings()
            .iter()
            .filter_map(|m| {
                let ebs = m.ebs()?;
                Some(AmiBlockDevice {
                    device_name: m.device_name().unwrap_or_default().to_string(),
                    snapshot_id: ebs
                        .snapshot_id()
                        .filter(|s| !s.is_empty())
                        .map(|s| s.to_string()),
                    size_gb: ebs.volume_size().unwrap_or(0),
                    volume_type: ebs
                        .volume_type()
                        .map(|t| t.as_str().to_string())
                        .unwrap_or_default(),
                    delete_on_termination: ebs.delete_on_termination().unwrap_or(false),
                })
            })
            .collect();

        Self {
            image_id: img.image_id().unwrap_or("unknown").to_string(),
            name: img.name().unwrap_or_default().to_string(),
            description: img.description().unwrap_or_default().to_string(),
            state: img
                .state()
                .map(|s| s.as_str().to_string())
                .unwrap_or_default(),
            architecture: img
                .architecture()
                .map(|a| a.as_str().to_string())
                .unwrap_or_default(),
            virtualization_type: img
                .virtualization_type()
                .map(|v| v.as_str().to_string())
                .unwrap_or_default(),
            root_device_type: img
                .root_device_type()
                .map(|t| t.as_str().to_string())
                .unwrap_or_default(),
            root_device_name: img.root_device_name().unwrap_or_default().to_string(),
            hypervisor: img
                .hypervisor()
                .map(|h| h.as_str().to_string())
                .unwrap_or_default(),
            ena_support: img.ena_support().unwrap_or(false),
            creation_date: img
                .creation_date()
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string()),
            deprecation_time: img
                .deprecation_time()
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string()),
            public: img.public().unwrap_or(false),
            platform: img
                .platform()
                .map(|p| p.as_str().to_string())
                .filter(|p| !p.is_empty()),
            block_devices,
            tags,
        }
    }
}

crate::sections! {
    pub enum AmiDetailSection,
    pub static AMI_SECTIONS = [
        Details "Details",
        BlockDevices "Block Devices",
        Permissions "Permissions" => crate::app::App::trigger_ami_permissions_load,
        Tags "Tags",
    ]
}

impl Resource for Ami {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&AMI_SECTIONS)
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws ec2 describe-images --image-ids {}",
            crate::aws::resource::shell_quote(self.id())
        ))
    }

    fn id(&self) -> &str {
        &self.image_id
    }

    fn name(&self) -> &str {
        if !self.name.is_empty() {
            &self.name
        } else if let Some(n) = self.tags.get("Name") {
            n
        } else {
            &self.image_id
        }
    }

    fn resource_type(&self) -> &str {
        "AMI"
    }

    fn state(&self) -> ResourceState {
        match self.state.as_str() {
            "available" => ResourceState::Available,
            "pending" | "transient" => ResourceState::Pending,
            "failed" | "invalid" | "error" => ResourceState::Unknown("unavailable".to_string()),
            "deregistered" => ResourceState::Terminated,
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
        let mut parts = vec![
            self.image_id.clone(),
            self.name.clone(),
            self.description.clone(),
            self.state.clone(),
            self.architecture.clone(),
        ];
        if let Some(p) = &self.platform {
            parts.push(p.clone());
        }
        for (k, v) in &self.tags {
            parts.push(format!("{}:{}", k, v));
        }
        parts.join(" ")
    }

    fn details(&self) -> Vec<(String, String)> {
        let mut d = vec![
            ("Image ID".to_string(), self.image_id.clone()),
            ("Name".to_string(), self.name.clone()),
            ("State".to_string(), self.state.clone()),
            ("Architecture".to_string(), self.architecture.clone()),
        ];
        if let Some(c) = &self.creation_date {
            d.push(("Created".to_string(), c.clone()));
        }
        d
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{region}.console.aws.amazon.com/ec2/home?region={region}#ImageDetails:imageId={}",
            self.image_id
        ))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
}

/// Resolve the launch permissions for an AMI via `DescribeImageAttribute`
/// (`LaunchPermission`): the account ids it is shared with, plus `"public"` if
/// shared with group=all.
pub async fn fetch_ami_launch_permissions(
    client: Ec2Client,
    image_id: String,
) -> Result<Vec<String>> {
    use aws_sdk_ec2::types::ImageAttributeName;
    let resp = client
        .describe_image_attribute()
        .image_id(&image_id)
        .attribute(ImageAttributeName::LaunchPermission)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;

    let mut out = Vec::new();
    for perm in resp.launch_permissions() {
        if perm.group().is_some() {
            out.push("public".to_string());
        } else if let Some(uid) = perm.user_id() {
            out.push(uid.to_string());
        } else if let Some(org) = perm.organization_arn() {
            out.push(org.to_string());
        } else if let Some(ou) = perm.organizational_unit_arn() {
            out.push(ou.to_string());
        }
    }
    Ok(out)
}

// ── Launch Template ─────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct LaunchTemplate {
    pub id: String,
    pub name: String,
    pub default_version: i64,
    pub latest_version: i64,
    pub created_by: String,
    pub create_time: Option<String>,
    pub tags: HashMap<String, String>,
}

impl LaunchTemplate {
    pub fn from_sdk(lt: &aws_sdk_ec2::types::LaunchTemplate) -> Self {
        let mut tags = HashMap::new();
        for tag in lt.tags() {
            if let (Some(k), Some(v)) = (tag.key(), tag.value()) {
                tags.insert(k.to_string(), v.to_string());
            }
        }
        Self {
            id: lt.launch_template_id().unwrap_or("unknown").to_string(),
            name: lt.launch_template_name().unwrap_or_default().to_string(),
            default_version: lt.default_version_number().unwrap_or(0),
            latest_version: lt.latest_version_number().unwrap_or(0),
            created_by: lt.created_by().unwrap_or_default().to_string(),
            create_time: lt.create_time().map(|t| t.to_string()),
            tags,
        }
    }
}

crate::sections! {
    pub enum LaunchTemplateDetailSection,
    pub static LAUNCH_TEMPLATE_SECTIONS = [
        Details "Details",
        // Versions and Data both render from the lazy versions fetch.
        Versions "Versions" => crate::app::App::trigger_lt_versions_load,
        Data "Data" => crate::app::App::trigger_lt_versions_load,
        Tags "Tags",
    ]
}

impl Resource for LaunchTemplate {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&LAUNCH_TEMPLATE_SECTIONS)
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws ec2 describe-launch-templates --launch-template-ids {}",
            crate::aws::resource::shell_quote(self.id())
        ))
    }

    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        if !self.name.is_empty() {
            &self.name
        } else if let Some(n) = self.tags.get("Name") {
            n
        } else {
            &self.id
        }
    }

    fn resource_type(&self) -> &str {
        "Launch Template"
    }

    fn state(&self) -> ResourceState {
        ResourceState::stateless()
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        let mut parts = vec![self.id.clone(), self.name.clone(), self.created_by.clone()];
        for (k, v) in &self.tags {
            parts.push(format!("{}:{}", k, v));
        }
        parts.join(" ")
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Template ID".to_string(), self.id.clone()),
            ("Name".to_string(), self.name.clone()),
            ("Default Version".to_string(), self.default_version.to_string()),
            ("Latest Version".to_string(), self.latest_version.to_string()),
        ]
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{region}.console.aws.amazon.com/ec2/home?region={region}#LaunchTemplateDetails:launchTemplateId={}",
            self.id
        ))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
}

/// An Elastic IP address (`DescribeAddresses`). Unassociated EIPs still bill,
/// so they render with a yellow (Pending) state to make idle spend pop.
#[derive(Clone, Debug)]
pub struct ElasticIp {
    /// Allocation id for VPC EIPs (`eipalloc-…`); falls back to the public IP
    /// for the (rare) EC2-Classic address with no allocation.
    pub id: String,
    pub public_ip: String,
    pub allocation_id: Option<String>,
    pub association_id: Option<String>,
    pub instance_id: Option<String>,
    pub network_interface_id: Option<String>,
    pub private_ip: Option<String>,
    pub domain: String,
    pub public_ipv4_pool: Option<String>,
    pub tags: HashMap<String, String>,
}

impl ElasticIp {
    pub fn from_sdk(a: &aws_sdk_ec2::types::Address) -> Self {
        let mut tags = HashMap::new();
        for tag in a.tags() {
            if let (Some(k), Some(v)) = (tag.key(), tag.value()) {
                tags.insert(k.to_string(), v.to_string());
            }
        }
        let public_ip = a.public_ip().unwrap_or_default().to_string();
        let allocation_id = a.allocation_id().map(|s| s.to_string());
        let id = allocation_id.clone().unwrap_or_else(|| public_ip.clone());
        Self {
            id,
            public_ip,
            allocation_id,
            association_id: a.association_id().map(|s| s.to_string()),
            instance_id: a.instance_id().filter(|s| !s.is_empty()).map(|s| s.to_string()),
            network_interface_id: a
                .network_interface_id()
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string()),
            private_ip: a.private_ip_address().map(|s| s.to_string()),
            domain: a
                .domain()
                .map(|d| d.as_str().to_string())
                .unwrap_or_else(|| "vpc".to_string()),
            public_ipv4_pool: a.public_ipv4_pool().map(|s| s.to_string()),
            tags,
        }
    }

    /// An EIP that is not attached to anything — billable idle spend.
    pub fn is_unassociated(&self) -> bool {
        self.association_id.is_none() && self.instance_id.is_none()
    }
}

impl Resource for ElasticIp {
    fn references(&self) -> Vec<(String, String)> {
        let mut v = Vec::new();
        let mut r = |l: &str, x: &str| v.push((l.to_string(), x.to_string()));
        if let Some(x) = &self.instance_id { r("Instance", x); }
        if let Some(x) = &self.network_interface_id { r("Network Interface", x); }
        v
    }

    fn cli_command(&self) -> Option<String> {
        // EC2-Classic addresses have no allocation id to describe by.
        let alloc = self.allocation_id.as_deref()?;
        Some(format!(
            "aws ec2 describe-addresses --allocation-ids {}",
            crate::aws::resource::shell_quote(alloc)
        ))
    }

    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        if let Some(n) = self.tags.get("Name") {
            n
        } else {
            &self.public_ip
        }
    }

    fn resource_type(&self) -> &str {
        "Elastic IP"
    }

    fn state(&self) -> ResourceState {
        if self.is_unassociated() {
            // Idle → yellow warning dot so wasted spend is visible in the list.
            ResourceState::Pending
        } else {
            ResourceState::Available
        }
    }

    fn state_label(&self) -> String {
        if self.is_unassociated() {
            "unassociated".to_string()
        } else {
            "associated".to_string()
        }
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        let mut parts = vec![self.id.clone(), self.public_ip.clone()];
        if let Some(i) = &self.instance_id {
            parts.push(i.clone());
        }
        if let Some(e) = &self.network_interface_id {
            parts.push(e.clone());
        }
        if let Some(p) = &self.private_ip {
            parts.push(p.clone());
        }
        for (k, v) in &self.tags {
            parts.push(format!("{}:{}", k, v));
        }
        parts.join(" ")
    }

    fn details(&self) -> Vec<(String, String)> {
        let mut d = vec![
            ("Public IP".to_string(), self.public_ip.clone()),
            (
                "Association".to_string(),
                if self.is_unassociated() {
                    "⚠ unassociated (billable)".to_string()
                } else {
                    "associated".to_string()
                },
            ),
        ];
        if let Some(a) = &self.allocation_id {
            d.push(("Allocation ID".to_string(), a.clone()));
        }
        if let Some(a) = &self.association_id {
            d.push(("Association ID".to_string(), a.clone()));
        }
        if let Some(i) = &self.instance_id {
            d.push(("Instance".to_string(), i.clone()));
        }
        if let Some(e) = &self.network_interface_id {
            d.push(("Network Interface".to_string(), e.clone()));
        }
        if let Some(p) = &self.private_ip {
            d.push(("Private IP".to_string(), p.clone()));
        }
        d.push(("Scope".to_string(), self.domain.clone()));
        if let Some(p) = &self.public_ipv4_pool {
            d.push(("IPv4 Pool".to_string(), p.clone()));
        }
        d
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{region}.console.aws.amazon.com/ec2/home?region={region}#ElasticIpDetails:AllocationId={}",
            self.allocation_id.as_deref().unwrap_or(&self.public_ip)
        ))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
}

/// One resolved launch-template version, fetched lazily.
#[derive(Clone, Debug)]
pub struct LaunchTemplateVersion {
    pub version_number: i64,
    pub is_default: bool,
    pub description: String,
    pub created_by: String,
    pub create_time: Option<String>,
    /// The version's `LaunchTemplateData`, serialized to pretty JSON for `v`/`e`.
    pub data_json: String,
}

/// Build a JSON object from the readable fields of a `ResponseLaunchTemplateData`
/// (full SDK serialization isn't derived) for the Data section / `v`/`e`.
fn launch_template_data_json(
    data: &aws_sdk_ec2::types::ResponseLaunchTemplateData,
) -> String {
    use serde_json::{json, Map, Value};
    let mut obj = Map::new();
    if let Some(it) = data.instance_type() {
        obj.insert("InstanceType".to_string(), json!(it.as_str()));
    }
    if let Some(img) = data.image_id() {
        obj.insert("ImageId".to_string(), json!(img));
    }
    if let Some(k) = data.key_name() {
        obj.insert("KeyName".to_string(), json!(k));
    }
    let sg_ids = data.security_group_ids();
    if !sg_ids.is_empty() {
        obj.insert("SecurityGroupIds".to_string(), json!(sg_ids));
    }
    let sgs = data.security_groups();
    if !sgs.is_empty() {
        obj.insert("SecurityGroups".to_string(), json!(sgs));
    }
    if let Some(prof) = data.iam_instance_profile() {
        let mut p = Map::new();
        if let Some(arn) = prof.arn() {
            p.insert("Arn".to_string(), json!(arn));
        }
        if let Some(name) = prof.name() {
            p.insert("Name".to_string(), json!(name));
        }
        if !p.is_empty() {
            obj.insert("IamInstanceProfile".to_string(), Value::Object(p));
        }
    }
    if let Some(mkt) = data.instance_market_options() {
        let mut m = Map::new();
        if let Some(t) = mkt.market_type() {
            m.insert("MarketType".to_string(), json!(t.as_str()));
        }
        if !m.is_empty() {
            obj.insert("InstanceMarketOptions".to_string(), Value::Object(m));
        }
    }
    let bdms: Vec<Value> = data
        .block_device_mappings()
        .iter()
        .map(|m| {
            let mut bm = Map::new();
            if let Some(d) = m.device_name() {
                bm.insert("DeviceName".to_string(), json!(d));
            }
            if let Some(ebs) = m.ebs() {
                let mut e = Map::new();
                if let Some(s) = ebs.volume_size() {
                    e.insert("VolumeSize".to_string(), json!(s));
                }
                if let Some(t) = ebs.volume_type() {
                    e.insert("VolumeType".to_string(), json!(t.as_str()));
                }
                if let Some(snap) = ebs.snapshot_id() {
                    e.insert("SnapshotId".to_string(), json!(snap));
                }
                if let Some(enc) = ebs.encrypted() {
                    e.insert("Encrypted".to_string(), json!(enc));
                }
                if !e.is_empty() {
                    bm.insert("Ebs".to_string(), Value::Object(e));
                }
            }
            Value::Object(bm)
        })
        .collect();
    if !bdms.is_empty() {
        obj.insert("BlockDeviceMappings".to_string(), json!(bdms));
    }
    if let Some(ud) = data.user_data() {
        // The SDK returns user data base64-encoded; decode to the human-readable
        // script/cloud-config for display. Fall back to the raw value if it isn't
        // valid base64 or valid UTF-8 (e.g. a gzip-compressed payload).
        let decoded = aws_smithy_types::base64::decode(ud)
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok());
        obj.insert(
            "UserData".to_string(),
            json!(decoded.as_deref().unwrap_or(ud)),
        );
    }
    serde_json::to_string_pretty(&Value::Object(obj)).unwrap_or_else(|_| "{}".to_string())
}

/// Fetch all versions of a launch template via `DescribeLaunchTemplateVersions`,
/// most-recent-version first. Hand-rolled pagination (token advanced via
/// `next_page_token`).
pub async fn fetch_launch_template_versions(
    client: Ec2Client,
    lt_id: String,
) -> Result<Vec<LaunchTemplateVersion>> {
    let mut out: Vec<LaunchTemplateVersion> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let resp = client
            .describe_launch_template_versions()
            .launch_template_id(&lt_id)
            .set_next_token(token.clone())
            .send()
            .await
            .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;

        for v in resp.launch_template_versions() {
            let data_json = v
                .launch_template_data()
                .map(launch_template_data_json)
                .unwrap_or_else(|| "{}".to_string());
            out.push(LaunchTemplateVersion {
                version_number: v.version_number().unwrap_or(0),
                is_default: v.default_version().unwrap_or(false),
                description: v.version_description().unwrap_or_default().to_string(),
                created_by: v.created_by().unwrap_or_default().to_string(),
                create_time: v.create_time().map(|t| t.to_string()),
                data_json,
            });
        }

        token = crate::aws::pagination::next_page_token(resp.next_token(), &token);
        if token.is_none() {
            break;
        }
    }
    out.sort_by(|a, b| b.version_number.cmp(&a.version_number));
    Ok(out)
}
