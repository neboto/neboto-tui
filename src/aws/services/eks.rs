use crate::aws::client::AwsClients;
use crate::aws::resource::sg_refs;
use crate::aws::resource::{native_state_label, Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::error::Result;
use crate::event::{Event, LoadProgress};
use async_trait::async_trait;
use aws_sdk_eks::Client as EksClient;
use std::any::Any;
use std::collections::HashMap;
use tokio::sync::mpsc;

/// EKS service — single-list of clusters with a split detail pane. Node groups,
/// Fargate profiles, and add-ons are all *per-cluster*, so they're fetched
/// lazily on first view rather than iterated across every cluster up front.
/// This is the AWS control-plane view of EKS — pods/services live in the k8s
/// API server, not here.
pub struct EksService {
    client: EksClient,
}

impl EksService {
    pub fn new(aws_clients: &AwsClients) -> Self {
        Self {
            client: aws_clients.eks_client(),
        }
    }
}

#[async_trait]
impl AwsService for EksService {
    fn service_type(&self) -> ServiceType {
        ServiceType::Eks
    }

    fn name(&self) -> &str {
        "EKS"
    }

    async fn list_resources(&self) -> Result<Vec<Box<dyn Resource>>> {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        self.list_resources_streaming(tx, ServiceType::Eks).await?;
        Ok(vec![])
    }

    async fn list_resources_streaming(
        &self,
        event_tx: mpsc::UnboundedSender<Event>,
        service_type: ServiceType,
    ) -> Result<()> {
        // Paginate cluster names, then describe each cluster concurrently (N+1).
        let mut total = 0usize;
        let mut paginator = self.client.list_clusters().into_paginator().send();

        while let Some(result) = paginator.next().await {
            match result {
                Ok(page) => {
                    let names = page.clusters();
                    if names.is_empty() {
                        continue;
                    }

                    let futs: Vec<_> = names
                        .iter()
                        .map(|name| {
                            let client = self.client.clone();
                            let name = name.clone();
                            async move {
                                client
                                    .describe_cluster()
                                    .name(&name)
                                    .send()
                                    .await
                                    .ok()
                                    .and_then(|r| r.cluster().cloned())
                                    .map(|c| EksCluster::from_sdk(&c))
                            }
                        })
                        .collect();

                    let batch: Vec<Box<dyn Resource>> = futures::future::join_all(futs)
                        .await
                        .into_iter()
                        .flatten()
                        .map(|c| Box::new(c) as Box<dyn Resource>)
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
                        error: format!("Failed to list EKS clusters: {}", e),
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

// ── EksCluster ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct EksCluster {
    pub name: String,
    pub arn: String,
    pub status: String, // ACTIVE/CREATING/UPDATING/DELETING/FAILED/PENDING
    pub version: String, // k8s version, e.g. "1.29"
    pub platform_version: String,
    pub endpoint: String,
    pub vpc_id: String,
    pub subnet_ids: Vec<String>,
    pub security_group_ids: Vec<String>,
    pub endpoint_public: bool,
    pub endpoint_private: bool,
    pub auth_mode: String, // CONFIG_MAP/API/API_AND_CONFIG_MAP
    pub logging_enabled: Vec<String>,
    pub created: Option<String>,
    pub tags: HashMap<String, String>,
    // [ENRICH]
    pub role_arn: String,                  // cluster IAM role (jump → IAM)
    pub cluster_security_group_id: String, // managed control-plane SG
    pub public_access_cidrs: Vec<String>,  // endpoint public-access allow list
    pub service_ipv4_cidr: String,
    pub service_ipv6_cidr: String,
    pub ip_family: String, // ipv4 / ipv6
    pub oidc_issuer: String, // IRSA trust issuer
    pub secrets_kms_key: String, // envelope encryption key ARN (jump → KMS)
    pub health_issues: Vec<String>, // code: message
    pub upgrade_policy: String, // STANDARD / EXTENDED
}

impl EksCluster {
    pub fn from_sdk(c: &aws_sdk_eks::types::Cluster) -> Self {
        let vpc = c.resources_vpc_config();
        let (vpc_id, subnet_ids, security_group_ids, endpoint_public, endpoint_private) =
            match vpc {
                Some(v) => (
                    v.vpc_id().unwrap_or_default().to_string(),
                    v.subnet_ids().to_vec(),
                    v.security_group_ids().to_vec(),
                    v.endpoint_public_access(),
                    v.endpoint_private_access(),
                ),
                None => (String::new(), Vec::new(), Vec::new(), false, false),
            };

        let auth_mode = c
            .access_config()
            .and_then(|a| a.authentication_mode())
            .map(|m| m.as_str().to_string())
            .unwrap_or_default();

        // Collect the log types that are actually enabled.
        let mut logging_enabled = Vec::new();
        if let Some(logging) = c.logging() {
            for setup in logging.cluster_logging() {
                if setup.enabled().unwrap_or(false) {
                    for t in setup.types() {
                        logging_enabled.push(t.as_str().to_string());
                    }
                }
            }
        }

        let (cluster_security_group_id, public_access_cidrs) = match vpc {
            Some(v) => (
                v.cluster_security_group_id().unwrap_or_default().to_string(),
                v.public_access_cidrs().to_vec(),
            ),
            None => (String::new(), Vec::new()),
        };

        let net = c.kubernetes_network_config();
        let (service_ipv4_cidr, service_ipv6_cidr, ip_family) = match net {
            Some(n) => (
                n.service_ipv4_cidr().unwrap_or_default().to_string(),
                n.service_ipv6_cidr().unwrap_or_default().to_string(),
                n.ip_family().map(|f| f.as_str().to_string()).unwrap_or_default(),
            ),
            None => (String::new(), String::new(), String::new()),
        };

        let oidc_issuer = c
            .identity()
            .and_then(|i| i.oidc())
            .and_then(|o| o.issuer())
            .unwrap_or_default()
            .to_string();

        let secrets_kms_key = c
            .encryption_config()
            .iter()
            .find_map(|e| e.provider().and_then(|p| p.key_arn()))
            .unwrap_or_default()
            .to_string();

        let health_issues = c
            .health()
            .map(|h| {
                h.issues()
                    .iter()
                    .map(|i| {
                        let code = i.code().map(|c| c.as_str()).unwrap_or("");
                        let msg = i.message().unwrap_or("");
                        if code.is_empty() {
                            msg.to_string()
                        } else {
                            format!("{}: {}", code, msg)
                        }
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        let upgrade_policy = c
            .upgrade_policy()
            .and_then(|u| u.support_type())
            .map(|s| s.as_str().to_string())
            .unwrap_or_default();

        Self {
            name: c.name().unwrap_or_default().to_string(),
            arn: c.arn().unwrap_or_default().to_string(),
            status: c.status().map(|s| s.as_str().to_string()).unwrap_or_default(),
            version: c.version().unwrap_or_default().to_string(),
            platform_version: c.platform_version().unwrap_or_default().to_string(),
            endpoint: c.endpoint().unwrap_or_default().to_string(),
            vpc_id,
            subnet_ids,
            security_group_ids,
            endpoint_public,
            endpoint_private,
            auth_mode,
            logging_enabled,
            created: c.created_at().map(|d| fmt_epoch_secs(d.secs())),
            tags: c.tags().cloned().unwrap_or_default(),
            role_arn: c.role_arn().unwrap_or_default().to_string(),
            cluster_security_group_id,
            public_access_cidrs,
            service_ipv4_cidr,
            service_ipv6_cidr,
            ip_family,
            oidc_issuer,
            secrets_kms_key,
            health_issues,
            upgrade_policy,
        }
    }

    /// Public/private/both endpoint access summary.
    pub fn endpoint_access(&self) -> String {
        match (self.endpoint_public, self.endpoint_private) {
            (true, true) => "Public and private".to_string(),
            (true, false) => "Public".to_string(),
            (false, true) => "Private".to_string(),
            (false, false) => "None".to_string(),
        }
    }
}

crate::sections! {
    pub enum EksClusterDetailSection,
    pub static EKS_CLUSTER_SECTIONS = [
        Details "Details",
        Networking "Networking",
        Compute "Compute" => crate::app::App::trigger_eks_nodegroups_load,
        Fargate "Fargate" => crate::app::App::trigger_eks_fargate_load,
        Addons "Add-ons" => crate::app::App::trigger_eks_addons_load,
        Access "Access" => crate::app::App::trigger_eks_access_load,
        Insights "Insights" => crate::app::App::trigger_eks_insights_load,
        Tags "Tags",
    ]
}

impl Resource for EksCluster {
    fn references(&self) -> Vec<(String, String)> {
        let mut v = sg_refs(self.security_group_ids());
        let mut r = |l: &str, x: &str| v.push((l.to_string(), x.to_string()));
        for x in &self.subnet_ids { r("Subnet", x); }
        r("VPC", &self.vpc_id);
        r("Role", &self.role_arn);
        r("KMS Key", &self.secrets_kms_key);
        v
    }

    fn security_group_ids(&self) -> Vec<String> {
        let mut ids = self.security_group_ids.clone();
        if !self.cluster_security_group_id.is_empty() {
            ids.push(self.cluster_security_group_id.clone());
        }
        ids
    }

    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&EKS_CLUSTER_SECTIONS)
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws eks describe-cluster --name {}",
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
        "EKS Cluster"
    }

    fn state(&self) -> ResourceState {
        // A degraded-but-ACTIVE cluster (non-empty health issues) must read red.
        if !self.health_issues.is_empty() {
            return ResourceState::Unavailable;
        }
        match self.status.as_str() {
            "ACTIVE" => ResourceState::Available,
            "CREATING" => ResourceState::Creating,
            "UPDATING" | "PENDING" => ResourceState::Pending,
            "DELETING" => ResourceState::Deleting,
            "FAILED" => ResourceState::Unavailable,
            other => ResourceState::Unknown(other.to_string()),
        }
    }

    fn state_label(&self) -> String {
        if !self.health_issues.is_empty() {
            return "degraded".to_string();
        }
        native_state_label(&self.status, || self.state())
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!(
            "{} {} {} {}",
            self.name, self.version, self.vpc_id, self.arn,
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Name".to_string(), self.name.clone()),
            ("Status".to_string(), self.status.clone()),
            ("Version".to_string(), self.version.clone()),
            ("Platform".to_string(), self.platform_version.clone()),
            ("Endpoint Access".to_string(), self.endpoint_access()),
            ("VPC".to_string(), self.vpc_id.clone()),
        ]
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/eks/home?region={}#/clusters/{}",
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

// ── Children (rendered lazily in the split pane) ──────────────────────────────

#[derive(Debug, Clone)]
pub struct EksNodeGroup {
    pub name: String,
    pub status: String,
    pub instance_types: Vec<String>,
    pub ami_type: String,
    pub capacity_type: String, // ON_DEMAND / SPOT
    pub desired: i32,
    pub min: i32,
    pub max: i32,
    pub version: String,
    pub health_issues: Vec<String>,
    // [ENRICH]
    pub release_version: String,
    pub node_role: String, // jump → IAM
    pub subnets: Vec<String>,
    pub labels: Vec<(String, String)>,
    pub taints: Vec<String>, // "key=value:EFFECT"
    pub max_unavailable: String,
    pub launch_template: String, // "name/id v<n>"
    pub created: Option<String>,
    pub disk_size: Option<i32>,
}

#[derive(Debug, Clone)]
pub struct EksFargateProfile {
    pub name: String,
    pub status: String,
    pub selectors: Vec<String>, // "namespace (labels)"
    pub subnets: Vec<String>,
    // [ENRICH]
    pub pod_execution_role: String, // jump → IAM
    pub created: Option<String>,
}

#[derive(Debug, Clone)]
pub struct EksAddon {
    pub name: String,
    pub version: String,
    pub status: String,
    pub service_account_role: String,
    // [ENRICH]
    pub latest_version: String,
    pub update_available: bool,
    pub health_issues: Vec<String>,
    pub configured: bool, // configuration_values present
}

// ── Access management [NEW] ───────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct EksAccessEntry {
    pub principal_arn: String, // jump → IAM
    pub entry_type: String,    // STANDARD / EC2_LINUX / FARGATE_LINUX / …
    pub kubernetes_groups: Vec<String>,
    pub username: String,
    pub access_policies: Vec<String>, // "policyArn (scope: cluster|namespace[…])"
}

#[derive(Debug, Clone)]
pub struct EksPodIdentity {
    pub namespace: String,
    pub service_account: String,
    pub role_arn: String, // jump → IAM
}

// ── Upgrade insights [NEW] ────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct EksInsight {
    pub name: String,
    pub category: String, // UPGRADE_READINESS / …
    pub status: String,   // PASSING / WARNING / ERROR / UNKNOWN
    pub kubernetes_version: String,
    pub recommendation: String,
    pub description: String,
    pub deprecation_details: Vec<String>,
    pub last_refresh: Option<String>,
}

/// Access entries + pod identity associations, loaded together.
#[derive(Debug, Clone)]
pub struct EksAccessData {
    pub entries: Vec<EksAccessEntry>,
    pub pod_identities: Vec<EksPodIdentity>,
}

// ── Lazy fetch functions (per cluster) ────────────────────────────────────────

/// `list_nodegroups` → `describe_nodegroup` per group (concurrent).
pub async fn fetch_eks_nodegroups(
    client: EksClient,
    cluster: String,
) -> Result<Vec<EksNodeGroup>> {
    let listed = client
        .list_nodegroups()
        .cluster_name(&cluster)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;

    let futs: Vec<_> = listed
        .nodegroups()
        .iter()
        .map(|ng| {
            let client = client.clone();
            let cluster = cluster.clone();
            let ng = ng.clone();
            async move {
                client
                    .describe_nodegroup()
                    .cluster_name(&cluster)
                    .nodegroup_name(&ng)
                    .send()
                    .await
                    .ok()
                    .and_then(|r| r.nodegroup().cloned())
            }
        })
        .collect();

    let mut out = Vec::new();
    for ng in futures::future::join_all(futs).await.into_iter().flatten() {
        let scaling = ng.scaling_config();
        let health_issues = ng
            .health()
            .map(|h| {
                h.issues()
                    .iter()
                    .filter_map(|i| i.message().map(|m| m.to_string()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let mut labels: Vec<(String, String)> = ng
            .labels()
            .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default();
        labels.sort();
        let taints: Vec<String> = ng
            .taints()
            .iter()
            .map(|t| {
                let key = t.key().unwrap_or("");
                let val = t.value().unwrap_or("");
                let effect = t.effect().map(|e| e.as_str()).unwrap_or("");
                format!("{}={}:{}", key, val, effect)
            })
            .collect();
        let max_unavailable = ng
            .update_config()
            .map(|u| {
                if let Some(n) = u.max_unavailable() {
                    n.to_string()
                } else if let Some(p) = u.max_unavailable_percentage() {
                    format!("{}%", p)
                } else {
                    String::new()
                }
            })
            .unwrap_or_default();
        let launch_template = ng
            .launch_template()
            .map(|lt| {
                let name = lt.name().or_else(|| lt.id()).unwrap_or("");
                match lt.version() {
                    Some(v) => format!("{} v{}", name, v),
                    None => name.to_string(),
                }
            })
            .unwrap_or_default();
        out.push(EksNodeGroup {
            name: ng.nodegroup_name().unwrap_or_default().to_string(),
            status: ng.status().map(|s| s.as_str().to_string()).unwrap_or_default(),
            instance_types: ng.instance_types().to_vec(),
            ami_type: ng.ami_type().map(|a| a.as_str().to_string()).unwrap_or_default(),
            capacity_type: ng
                .capacity_type()
                .map(|c| c.as_str().to_string())
                .unwrap_or_default(),
            desired: scaling.and_then(|s| s.desired_size()).unwrap_or(0),
            min: scaling.and_then(|s| s.min_size()).unwrap_or(0),
            max: scaling.and_then(|s| s.max_size()).unwrap_or(0),
            version: ng.version().unwrap_or_default().to_string(),
            health_issues,
            release_version: ng.release_version().unwrap_or_default().to_string(),
            node_role: ng.node_role().unwrap_or_default().to_string(),
            subnets: ng.subnets().to_vec(),
            labels,
            taints,
            max_unavailable,
            launch_template,
            created: ng.created_at().map(|d| fmt_epoch_secs(d.secs())),
            disk_size: ng.disk_size(),
        });
    }
    Ok(out)
}

/// `list_fargate_profiles` → `describe_fargate_profile` per profile (concurrent).
pub async fn fetch_eks_fargate(
    client: EksClient,
    cluster: String,
) -> Result<Vec<EksFargateProfile>> {
    let listed = client
        .list_fargate_profiles()
        .cluster_name(&cluster)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;

    let futs: Vec<_> = listed
        .fargate_profile_names()
        .iter()
        .map(|name| {
            let client = client.clone();
            let cluster = cluster.clone();
            let name = name.clone();
            async move {
                client
                    .describe_fargate_profile()
                    .cluster_name(&cluster)
                    .fargate_profile_name(&name)
                    .send()
                    .await
                    .ok()
                    .and_then(|r| r.fargate_profile().cloned())
            }
        })
        .collect();

    let mut out = Vec::new();
    for fp in futures::future::join_all(futs).await.into_iter().flatten() {
        let selectors = fp
            .selectors()
            .iter()
            .map(|s| {
                let ns = s.namespace().unwrap_or("*");
                match s.labels() {
                    Some(labels) if !labels.is_empty() => {
                        let mut pairs: Vec<String> =
                            labels.iter().map(|(k, v)| format!("{}={}", k, v)).collect();
                        pairs.sort();
                        format!("{} ({})", ns, pairs.join(", "))
                    }
                    _ => ns.to_string(),
                }
            })
            .collect();
        out.push(EksFargateProfile {
            name: fp.fargate_profile_name().unwrap_or_default().to_string(),
            status: fp.status().map(|s| s.as_str().to_string()).unwrap_or_default(),
            selectors,
            subnets: fp.subnets().to_vec(),
            pod_execution_role: fp.pod_execution_role_arn().unwrap_or_default().to_string(),
            created: fp.created_at().map(|d| fmt_epoch_secs(d.secs())),
        });
    }
    Ok(out)
}

/// `list_addons` → `describe_addon` per add-on (concurrent). Then, for each
/// add-on, one `describe_addon_versions(addon_name, kubernetes_version)` to find
/// the newest compatible default version and flag update-available (drift).
pub async fn fetch_eks_addons(
    client: EksClient,
    cluster: String,
    cluster_version: String,
) -> Result<Vec<EksAddon>> {
    let listed = client
        .list_addons()
        .cluster_name(&cluster)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;

    let futs: Vec<_> = listed
        .addons()
        .iter()
        .map(|name| {
            let client = client.clone();
            let cluster = cluster.clone();
            let name = name.clone();
            async move {
                client
                    .describe_addon()
                    .cluster_name(&cluster)
                    .addon_name(&name)
                    .send()
                    .await
                    .ok()
                    .and_then(|r| r.addon().cloned())
            }
        })
        .collect();

    let mut out = Vec::new();
    for a in futures::future::join_all(futs).await.into_iter().flatten() {
        let name = a.addon_name().unwrap_or_default().to_string();
        let version = a.addon_version().unwrap_or_default().to_string();
        let health_issues = a
            .health()
            .map(|h| {
                h.issues()
                    .iter()
                    .map(|i| {
                        let code = i.code().map(|c| c.as_str()).unwrap_or("");
                        let msg = i.message().unwrap_or("");
                        if code.is_empty() {
                            msg.to_string()
                        } else {
                            format!("{}: {}", code, msg)
                        }
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        // Newest compatible version for the cluster's k8s version.
        let latest_version = if cluster_version.is_empty() {
            String::new()
        } else {
            latest_addon_version(&client, &name, &cluster_version)
                .await
                .unwrap_or_default()
        };
        let update_available = !latest_version.is_empty()
            && !version.is_empty()
            && version_lt(&version, &latest_version);

        out.push(EksAddon {
            name,
            version,
            status: a.status().map(|s| s.as_str().to_string()).unwrap_or_default(),
            service_account_role: a.service_account_role_arn().unwrap_or_default().to_string(),
            latest_version,
            update_available,
            health_issues,
            configured: a.configuration_values().map(|v| !v.is_empty()).unwrap_or(false),
        });
    }
    Ok(out)
}

/// The newest add-on version compatible with the given k8s version.
async fn latest_addon_version(
    client: &EksClient,
    addon_name: &str,
    k8s_version: &str,
) -> Option<String> {
    let resp = client
        .describe_addon_versions()
        .addon_name(addon_name)
        .kubernetes_version(k8s_version)
        .send()
        .await
        .ok()?;
    let mut versions: Vec<String> = resp
        .addons()
        .iter()
        .flat_map(|info| info.addon_versions().iter())
        .filter_map(|v| v.addon_version().map(|s| s.to_string()))
        .collect();
    // Sort ascending by semantic-ish comparison; last is newest.
    versions.sort_by(|a, b| {
        if version_lt(a, b) {
            std::cmp::Ordering::Less
        } else if version_lt(b, a) {
            std::cmp::Ordering::Greater
        } else {
            std::cmp::Ordering::Equal
        }
    });
    versions.pop()
}

/// Loose semantic-version comparison for add-on versions like
/// `v1.18.1-eksbuild.3`. Compares the numeric components left-to-right.
fn version_lt(a: &str, b: &str) -> bool {
    let parse = |s: &str| -> Vec<u64> {
        s.trim_start_matches('v')
            .split(|c: char| !c.is_ascii_digit())
            .filter(|p| !p.is_empty())
            .filter_map(|p| p.parse::<u64>().ok())
            .collect()
    };
    let (pa, pb) = (parse(a), parse(b));
    for i in 0..pa.len().max(pb.len()) {
        let x = pa.get(i).copied().unwrap_or(0);
        let y = pb.get(i).copied().unwrap_or(0);
        if x != y {
            return x < y;
        }
    }
    false
}

// ── Access management fetch [NEW] ─────────────────────────────────────────────

/// `list_access_entries` → per-principal `describe_access_entry` +
/// `list_associated_access_policies` (bounded concurrency), plus
/// `list_pod_identity_associations`. Both back the one Access section.
pub async fn fetch_eks_access(client: EksClient, cluster: String) -> Result<EksAccessData> {
    use futures::stream::StreamExt;

    // Principal ARNs for the cluster.
    let mut principals: Vec<String> = Vec::new();
    let mut paginator = client
        .list_access_entries()
        .cluster_name(&cluster)
        .into_paginator()
        .send();
    while let Some(page) = paginator.next().await {
        match page {
            Ok(p) => principals.extend(p.access_entries().iter().cloned()),
            Err(e) => {
                return Err(crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))
            }
        }
    }

    // For each principal: the entry detail + associated access policies.
    let entries: Vec<EksAccessEntry> = futures::stream::iter(principals.into_iter().map(|arn| {
        let client = client.clone();
        let cluster = cluster.clone();
        async move {
            let entry = client
                .describe_access_entry()
                .cluster_name(&cluster)
                .principal_arn(&arn)
                .send()
                .await
                .ok()
                .and_then(|r| r.access_entry().cloned())?;

            // Associated access policies (with scope).
            let mut access_policies = Vec::new();
            let mut pol_pager = client
                .list_associated_access_policies()
                .cluster_name(&cluster)
                .principal_arn(&arn)
                .into_paginator()
                .send();
            while let Some(page) = pol_pager.next().await {
                if let Ok(p) = page {
                    for ap in p.associated_access_policies() {
                        let policy = ap.policy_arn().unwrap_or("").to_string();
                        let scope = ap
                            .access_scope()
                            .map(|s| {
                                let kind = s.r#type().map(|t| t.as_str()).unwrap_or("?");
                                let ns = s.namespaces();
                                if ns.is_empty() {
                                    kind.to_string()
                                } else {
                                    format!("{} [{}]", kind, ns.join(", "))
                                }
                            })
                            .unwrap_or_else(|| "cluster".to_string());
                        access_policies.push(format!("{} (scope: {})", policy, scope));
                    }
                }
            }

            Some(EksAccessEntry {
                principal_arn: entry.principal_arn().unwrap_or_default().to_string(),
                entry_type: entry.r#type().unwrap_or_default().to_string(),
                kubernetes_groups: entry.kubernetes_groups().to_vec(),
                username: entry.username().unwrap_or_default().to_string(),
                access_policies,
            })
        }
    }))
    .buffer_unordered(8)
    .filter_map(|e| async move { e })
    .collect()
    .await;

    // Pod identity associations.
    let mut pod_identities = Vec::new();
    let mut pi_pager = client
        .list_pod_identity_associations()
        .cluster_name(&cluster)
        .into_paginator()
        .send();
    while let Some(page) = pi_pager.next().await {
        if let Ok(p) = page {
            for a in p.associations() {
                pod_identities.push(EksPodIdentity {
                    namespace: a.namespace().unwrap_or_default().to_string(),
                    service_account: a.service_account().unwrap_or_default().to_string(),
                    role_arn: a.owner_arn().unwrap_or_default().to_string(),
                });
            }
        }
    }

    Ok(EksAccessData {
        entries,
        pod_identities,
    })
}

// ── Upgrade insights fetch [NEW] ──────────────────────────────────────────────

const MAX_INSIGHTS: usize = 40;

/// `list_insights` (UPGRADE_READINESS) → per-insight `describe_insight`
/// (bounded concurrency) capturing status/recommendation + deprecated-API
/// detail. Capped at `MAX_INSIGHTS`.
pub async fn fetch_eks_insights(client: EksClient, cluster: String) -> Result<Vec<EksInsight>> {
    use aws_sdk_eks::types::{Category, InsightsFilter};
    use futures::stream::StreamExt;

    let filter = InsightsFilter::builder()
        .categories(Category::UpgradeReadiness)
        .build();

    let mut ids: Vec<String> = Vec::new();
    let mut paginator = client
        .list_insights()
        .cluster_name(&cluster)
        .filter(filter)
        .into_paginator()
        .send();
    while let Some(page) = paginator.next().await {
        match page {
            Ok(p) => {
                for s in p.insights() {
                    if let Some(id) = s.id() {
                        ids.push(id.to_string());
                    }
                }
            }
            Err(e) => {
                return Err(crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))
            }
        }
        if ids.len() >= MAX_INSIGHTS {
            ids.truncate(MAX_INSIGHTS);
            break;
        }
    }

    let mut insights: Vec<EksInsight> = futures::stream::iter(ids.into_iter().map(|id| {
        let client = client.clone();
        let cluster = cluster.clone();
        async move {
            let ins = client
                .describe_insight()
                .cluster_name(&cluster)
                .id(&id)
                .send()
                .await
                .ok()
                .and_then(|r| r.insight().cloned())?;

            let status = ins
                .insight_status()
                .and_then(|s| s.status())
                .map(|s| s.as_str().to_string())
                .unwrap_or_default();

            let mut deprecation_details = Vec::new();
            if let Some(summary) = ins.category_specific_summary() {
                for dd in summary.deprecation_details() {
                    let usage = dd.usage().unwrap_or("");
                    let replaced = dd.replaced_with().unwrap_or("");
                    let mut line = if replaced.is_empty() {
                        usage.to_string()
                    } else {
                        format!("{} → {}", usage, replaced)
                    };
                    if let Some(stop) = dd.stop_serving_version() {
                        line.push_str(&format!(" (removed in {})", stop));
                    }
                    deprecation_details.push(line);
                    // Observed clients (user-agents).
                    for cs in dd.client_stats() {
                        if let Some(ua) = cs.user_agent() {
                            deprecation_details.push(format!(
                                "    {} — {} reqs/30d",
                                ua,
                                cs.number_of_requests_last30_days()
                            ));
                        }
                    }
                }
            }

            Some(EksInsight {
                name: ins.name().unwrap_or_default().to_string(),
                category: ins.category().map(|c| c.as_str().to_string()).unwrap_or_default(),
                status,
                kubernetes_version: ins.kubernetes_version().unwrap_or_default().to_string(),
                recommendation: ins.recommendation().unwrap_or_default().to_string(),
                description: ins.description().unwrap_or_default().to_string(),
                deprecation_details,
                last_refresh: ins.last_refresh_time().map(|d| fmt_epoch_secs(d.secs())),
            })
        }
    }))
    .buffer_unordered(8)
    .filter_map(|e| async move { e })
    .collect()
    .await;

    // Stable order: ERROR first, then WARNING, then the rest, then by name.
    fn rank(s: &str) -> u8 {
        match s {
            "ERROR" => 0,
            "WARNING" => 1,
            "UNKNOWN" => 2,
            _ => 3,
        }
    }
    insights.sort_by(|a, b| {
        rank(&a.status)
            .cmp(&rank(&b.status))
            .then_with(|| a.name.cmp(&b.name))
    });

    Ok(insights)
}

// ── Container Insights metrics (`m` overlay) [NEW] ────────────────────────────

#[derive(Debug, Clone)]
pub struct EksMetricsData {
    pub time_range: crate::aws::services::ec2::MetricsTimeRange,
    pub node_count: Vec<(f64, f64)>,
    pub failed_node_count: Vec<(f64, f64)>,
    pub running_pod_count: Vec<(f64, f64)>,
    pub node_cpu_util: Vec<(f64, f64)>,
    pub node_mem_util: Vec<(f64, f64)>,
    pub x_max: f64,
}

impl EksMetricsData {
    /// True when every series is empty — Container Insights is likely disabled.
    pub fn is_empty(&self) -> bool {
        self.node_count.is_empty()
            && self.failed_node_count.is_empty()
            && self.running_pod_count.is_empty()
            && self.node_cpu_util.is_empty()
            && self.node_mem_util.is_empty()
    }
}

#[derive(Debug, Clone)]
pub enum EksMetricsState {
    Loading,
    Loaded(EksMetricsData),
}

/// Pull EKS cluster metrics from `AWS/ContainerInsights` (`ClusterName` dim).
/// Returns empty series when Container Insights isn't enabled (caller shows a
/// friendly enable-note).
pub async fn fetch_eks_metrics(
    cw: aws_sdk_cloudwatch::Client,
    cluster: String,
    time_range: crate::aws::services::ec2::MetricsTimeRange,
) -> Result<EksMetricsData> {
    use aws_sdk_cloudwatch::types::{Dimension, Statistic};

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let start = now - time_range.duration_secs();
    let period = time_range.period_secs();
    let start_dt = aws_sdk_cloudwatch::primitives::DateTime::from_secs(start);
    let end_dt = aws_sdk_cloudwatch::primitives::DateTime::from_secs(now);

    let dim = || Dimension::builder().name("ClusterName").value(&cluster).build();
    let metric = |name: &'static str, stat: Statistic| {
        cw.get_metric_statistics()
            .namespace("ContainerInsights")
            .metric_name(name)
            .dimensions(dim())
            .start_time(start_dt)
            .end_time(end_dt)
            .period(period)
            .set_statistics(Some(vec![stat]))
            .send()
    };

    let (nodes, failed, pods, cpu, mem) = tokio::join!(
        metric("cluster_node_count", Statistic::Average),
        metric("cluster_failed_node_count", Statistic::Maximum),
        metric("cluster_running_pod_count", Statistic::Average),
        metric("node_cpu_utilization", Statistic::Average),
        metric("node_memory_utilization", Statistic::Average),
    );

    let parse = |resp: std::result::Result<
        aws_sdk_cloudwatch::operation::get_metric_statistics::GetMetricStatisticsOutput,
        _,
    >,
                 pick: fn(&aws_sdk_cloudwatch::types::Datapoint) -> Option<f64>|
     -> Vec<(f64, f64)> {
        let dps = match resp {
            Ok(r) => r.datapoints().to_vec(),
            Err(_) => vec![],
        };
        let mut pts: Vec<(f64, f64)> = dps
            .iter()
            .filter_map(|dp| Some((dp.timestamp()?.secs() as f64 - start as f64, pick(dp)?)))
            .collect();
        pts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        pts
    };

    Ok(EksMetricsData {
        time_range,
        node_count: parse(nodes, |dp| dp.average()),
        failed_node_count: parse(failed, |dp| dp.maximum()),
        running_pod_count: parse(pods, |dp| dp.average()),
        node_cpu_util: parse(cpu, |dp| dp.average()),
        node_mem_util: parse(mem, |dp| dp.average()),
        x_max: time_range.duration_secs() as f64,
    })
}

// ── Timestamp helpers ─────────────────────────────────────────────────────────

fn fmt_epoch_secs(secs: i64) -> String {
    let days = secs / 86400;
    let rem = secs % 86400;
    let (h, mi) = (rem / 3600, (rem % 3600) / 60);
    let (y, mo, d) = epoch_days_to_ymd(days);
    format!("{:04}-{:02}-{:02} {:02}:{:02} UTC", y, mo, d, h, mi)
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
    for &len in &dm {
        if days < len as i64 {
            break;
        }
        days -= len as i64;
        month += 1;
    }
    (year, month, (days + 1) as u8)
}

fn is_leap(y: i32) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}
