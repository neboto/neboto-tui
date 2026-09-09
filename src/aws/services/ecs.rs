use crate::aws::client::AwsClients;
use crate::aws::resource::sg_refs;
use crate::aws::resource::{native_state_label, Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::aws::services::cloudwatch::fmt_epoch_secs;
use crate::aws::services::ec2::MetricsTimeRange;
use crate::error::Result;
use crate::event::{Event, LoadProgress};
use async_trait::async_trait;
use aws_sdk_cloudwatch::types::Datapoint;
use aws_sdk_ecs::types::{
    ClusterField, DesiredStatus, ServiceField, TaskDefinitionField, TaskField,
};
use aws_sdk_ecs::Client as EcsClient;
use std::any::Any;
use std::collections::HashMap;
use tokio::sync::mpsc;

pub struct EcsService {
    client: EcsClient,
}

impl EcsService {
    pub fn new(aws_clients: &AwsClients) -> Self {
        Self {
            client: aws_clients.ecs_client(),
        }
    }
}

#[async_trait]
impl AwsService for EcsService {
    fn service_type(&self) -> ServiceType {
        ServiceType::ECS
    }

    fn name(&self) -> &str {
        "ECS"
    }

    async fn list_resources(&self) -> Result<Vec<Box<dyn Resource>>> {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        self.list_resources_streaming(tx, ServiceType::ECS).await?;
        Ok(vec![])
    }

    async fn list_resources_streaming(
        &self,
        event_tx: mpsc::UnboundedSender<Event>,
        service_type: ServiceType,
    ) -> Result<()> {
        let mut total = 0usize;

        // Phase 1: List and describe clusters
        let mut cluster_arns: Vec<String> = Vec::new();
        let mut cluster_pager = self.client.list_clusters().into_paginator().items().send();
        while let Some(result) = cluster_pager.next().await {
            match result {
                Ok(arn) => cluster_arns.push(arn),
                Err(e) => {
                    let _ = event_tx.send(Event::ResourceLoadError {
                        service: service_type,
                        error: format!("Failed to list ECS clusters: {}", e),
                    });
                    return Ok(());
                }
            }
        }

        let mut cluster_resources: Vec<Box<dyn Resource>> = Vec::new();
        for batch in cluster_arns.chunks(100) {
            match self
                .client
                .describe_clusters()
                .set_clusters(Some(batch.to_vec()))
                .include(ClusterField::Tags)
                .include(ClusterField::Settings)
                .include(ClusterField::Statistics)
                .send()
                .await
            {
                Ok(resp) => {
                    for cluster in resp.clusters() {
                        cluster_resources.push(Box::new(EcsCluster::from_sdk(cluster)));
                    }
                }
                Err(e) => {
                    let _ = event_tx.send(Event::ResourceLoadError {
                        service: service_type,
                        error: format!("Failed to describe ECS clusters: {}", e),
                    });
                    return Ok(());
                }
            }
        }

        let cluster_count = cluster_resources.len();
        total += cluster_count;

        if !cluster_resources.is_empty() {
            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                service: service_type,
                resources: cluster_resources,
                progress: LoadProgress {
                    loaded_count: total,
                    total_count: None,
                    status_message: Some(format!(
                        "Loaded {} clusters. Loading services...",
                        cluster_count
                    )),
                },
            });
        }

        // Phase 2: Load services for each cluster
        let mut all_service_resources: Vec<Box<dyn Resource>> = Vec::new();
        for cluster_arn in &cluster_arns {
            let mut svc_arns: Vec<String> = Vec::new();
            let mut svc_pager = self
                .client
                .list_services()
                .cluster(cluster_arn)
                .into_paginator()
                .items()
                .send();
            while let Some(result) = svc_pager.next().await {
                match result {
                    Ok(arn) => svc_arns.push(arn),
                    Err(e) => {
                        let _ = event_tx.send(Event::ResourceLoadWarning {
                            service: service_type,
                            warning: format!(
                                "services in {}: {}",
                                cluster_arn.rsplit('/').next().unwrap_or(cluster_arn),
                                crate::error::sdk_error_message(&e)
                            ),
                        });
                        break;
                    }
                }
            }

            // describe_services accepts up to 10 at a time
            for batch in svc_arns.chunks(10) {
                if let Ok(resp) = self
                    .client
                    .describe_services()
                    .cluster(cluster_arn)
                    .set_services(Some(batch.to_vec()))
                    .include(ServiceField::Tags)
                    .send()
                    .await
                {
                    for svc in resp.services() {
                        all_service_resources.push(Box::new(EcsServiceInfo::from_sdk(svc)));
                    }
                }
            }
        }

        let svc_count = all_service_resources.len();
        total += svc_count;

        if !all_service_resources.is_empty() {
            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                service: service_type,
                resources: all_service_resources,
                progress: LoadProgress {
                    loaded_count: total,
                    total_count: None,
                    status_message: Some(format!(
                        "Loaded {} services. Loading task definitions...",
                        svc_count
                    )),
                },
            });
        }

        // Phase 3: List active task definition ARNs (parse family/revision from ARN — full
        // container detail is fetched lazily on detail-pane open via describe_task_definition).
        let mut taskdef_resources: Vec<Box<dyn Resource>> = Vec::new();
        let mut taskdef_pager = self
            .client
            .list_task_definitions()
            .status(aws_sdk_ecs::types::TaskDefinitionStatus::Active)
            .into_paginator()
            .items()
            .send();
        while let Some(result) = taskdef_pager.next().await {
            match result {
                Ok(arn) => taskdef_resources.push(Box::new(EcsTaskDefinition::from_arn(&arn))),
                Err(_) => break,
            }
        }

        let taskdef_count = taskdef_resources.len();
        total += taskdef_count;

        if !taskdef_resources.is_empty() {
            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                service: service_type,
                resources: taskdef_resources,
                progress: LoadProgress {
                    loaded_count: total,
                    total_count: None,
                    status_message: Some(format!(
                        "Loaded {} task definitions. Loading tasks...",
                        taskdef_count
                    )),
                },
            });
        }

        // Phase 4: Load tasks per cluster. We fetch RUNNING tasks plus a capped
        // window of recently-STOPPED tasks (ECS retains stopped tasks ~1h) so a
        // failed / cycled-out task stays visible long enough to inspect its stop
        // reason and container exit codes, instead of vanishing the moment it
        // dies. Stopped tasks are sorted most-recent-first and capped per
        // cluster so a crash-looping service can't flood the list.
        const MAX_STOPPED_TASKS_PER_CLUSTER: usize = 100;
        let mut task_resources: Vec<Box<dyn Resource>> = Vec::new();
        for cluster_arn in &cluster_arns {
            // Running tasks (default desiredStatus=RUNNING).
            let mut running_arns: Vec<String> = Vec::new();
            let mut running_pager = self
                .client
                .list_tasks()
                .cluster(cluster_arn)
                .into_paginator()
                .items()
                .send();
            while let Some(result) = running_pager.next().await {
                match result {
                    Ok(arn) => running_arns.push(arn),
                    Err(_) => break,
                }
            }

            for batch in running_arns.chunks(100) {
                if let Ok(resp) = self
                    .client
                    .describe_tasks()
                    .cluster(cluster_arn)
                    .set_tasks(Some(batch.to_vec()))
                    .include(TaskField::Tags)
                    .send()
                    .await
                {
                    for task in resp.tasks() {
                        task_resources.push(Box::new(EcsTask::from_sdk(task)));
                    }
                }
            }

            // Recently-stopped tasks, capped.
            let mut stopped_arns: Vec<String> = Vec::new();
            let mut stopped_pager = self
                .client
                .list_tasks()
                .cluster(cluster_arn)
                .desired_status(DesiredStatus::Stopped)
                .into_paginator()
                .items()
                .send();
            while let Some(result) = stopped_pager.next().await {
                match result {
                    Ok(arn) => {
                        stopped_arns.push(arn);
                        if stopped_arns.len() >= MAX_STOPPED_TASKS_PER_CLUSTER {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }

            let mut stopped: Vec<(i64, EcsTask)> = Vec::new();
            for batch in stopped_arns.chunks(100) {
                if let Ok(resp) = self
                    .client
                    .describe_tasks()
                    .cluster(cluster_arn)
                    .set_tasks(Some(batch.to_vec()))
                    .include(TaskField::Tags)
                    .send()
                    .await
                {
                    for task in resp.tasks() {
                        let epoch = task.stopped_at().map(|d| d.secs()).unwrap_or(0);
                        stopped.push((epoch, EcsTask::from_sdk(task)));
                    }
                }
            }
            // Most-recently-stopped first.
            stopped.sort_by(|a, b| b.0.cmp(&a.0));
            for (_, task) in stopped {
                task_resources.push(Box::new(task));
            }
        }

        total += task_resources.len();

        if !task_resources.is_empty() {
            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                service: service_type,
                resources: task_resources,
                progress: LoadProgress {
                    loaded_count: total,
                    total_count: None,
                    status_message: None,
                },
            });
        }

        let _ = event_tx.send(Event::ResourcesFullyLoaded {
            service: service_type,
            total_count: total,
        });

        Ok(())
    }

    async fn get_resource_details(&self, id: &str) -> Result<Box<dyn Resource>> {
        Err(crate::error::Error::ResourceNotFound(id.to_string()))
    }
}

/// Format an optional SDK DateTime (epoch seconds) into a readable string.
fn fmt_opt_secs(secs: Option<i64>) -> String {
    secs.map(fmt_epoch_secs).unwrap_or_default()
}

/// Short name from an ARN or family:revision string (last path segment).
fn short_arn(arn: &str) -> String {
    arn.rsplit('/').next().unwrap_or(arn).to_string()
}

// ── ECS Cluster ───────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct EcsCluster {
    pub cluster_arn: String,
    pub cluster_name: String,
    pub status: String,
    pub running_tasks: i32,
    pub pending_tasks: i32,
    pub active_services: i32,
    pub container_instances: i32,
    pub container_insights: Option<String>,
    pub capacity_providers: Vec<String>,
    pub default_strategy: Vec<String>,
    pub settings: Vec<(String, String)>,
    pub statistics: Vec<(String, String)>,
    pub tags: HashMap<String, String>,
}

impl EcsCluster {
    pub fn from_sdk(cluster: &aws_sdk_ecs::types::Cluster) -> Self {
        let mut tags = HashMap::new();
        for tag in cluster.tags() {
            if let (Some(k), Some(v)) = (tag.key(), tag.value()) {
                tags.insert(k.to_string(), v.to_string());
            }
        }

        let mut settings = Vec::new();
        let mut container_insights = None;
        for s in cluster.settings() {
            if let (Some(name), Some(value)) = (s.name(), s.value()) {
                let name = name.as_str().to_string();
                if name == "containerInsights" {
                    container_insights = Some(value.to_string());
                }
                settings.push((name, value.to_string()));
            }
        }

        let statistics = cluster
            .statistics()
            .iter()
            .filter_map(|kv| match (kv.name(), kv.value()) {
                (Some(n), Some(v)) => Some((n.to_string(), v.to_string())),
                _ => None,
            })
            .collect();

        let default_strategy = cluster
            .default_capacity_provider_strategy()
            .iter()
            .map(|item| {
                format!(
                    "{} (base={}, weight={})",
                    item.capacity_provider(),
                    item.base(),
                    item.weight()
                )
            })
            .collect();

        Self {
            cluster_arn: cluster.cluster_arn().unwrap_or_default().to_string(),
            cluster_name: cluster.cluster_name().unwrap_or_default().to_string(),
            status: cluster.status().unwrap_or_default().to_string(),
            running_tasks: cluster.running_tasks_count(),
            pending_tasks: cluster.pending_tasks_count(),
            active_services: cluster.active_services_count(),
            container_instances: cluster.registered_container_instances_count(),
            container_insights,
            capacity_providers: cluster
                .capacity_providers()
                .iter()
                .map(|s| s.to_string())
                .collect(),
            default_strategy,
            settings,
            statistics,
            tags,
        }
    }
}

crate::sections! {
    pub enum EcsClusterDetailSection,
    pub static ECS_CLUSTER_SECTIONS = [
        Overview "Overview",
        Settings "Settings",
        Tags "Tags",
    ]
}

impl Resource for EcsCluster {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&ECS_CLUSTER_SECTIONS)
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws ecs describe-clusters --clusters {}",
            crate::aws::resource::shell_quote(self.id())
        ))
    }

    fn id(&self) -> &str {
        &self.cluster_arn
    }

    fn name(&self) -> &str {
        &self.cluster_name
    }

    fn resource_type(&self) -> &str {
        "ECS Cluster"
    }

    fn state(&self) -> ResourceState {
        match self.status.as_str() {
            "ACTIVE" => ResourceState::Available,
            "INACTIVE" => ResourceState::Unavailable,
            "PROVISIONING" => ResourceState::Creating,
            "DEPROVISIONING" => ResourceState::Deleting,
            _ => ResourceState::Unknown(self.status.clone()),
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
            self.cluster_arn.clone(),
            self.cluster_name.clone(),
            self.status.clone(),
        ];
        for (k, v) in &self.tags {
            parts.push(format!("{}:{}", k, v));
        }
        parts.join(" ")
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Cluster ARN".to_string(), self.cluster_arn.clone()),
            ("Cluster Name".to_string(), self.cluster_name.clone()),
            ("Status".to_string(), self.status.clone()),
            ("Running Tasks".to_string(), self.running_tasks.to_string()),
            ("Pending Tasks".to_string(), self.pending_tasks.to_string()),
            (
                "Active Services".to_string(),
                self.active_services.to_string(),
            ),
            (
                "Container Instances".to_string(),
                self.container_instances.to_string(),
            ),
        ]
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{region}.console.aws.amazon.com/ecs/v2/clusters/{}/services?region={region}",
            self.cluster_name
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── ECS Service ───────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct EcsDeployment {
    pub status: String,
    pub task_definition: String,
    pub desired: i32,
    pub pending: i32,
    pub running: i32,
    pub failed: i32,
    pub rollout_state: Option<String>,
    pub rollout_state_reason: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    /// Epoch seconds of `updated_at` — for the timeline lens.
    pub updated_at_secs: i64,
}

#[derive(Clone, Debug)]
pub struct EcsServiceEvent {
    pub created_at: String,
    /// Epoch seconds of `created_at` — for the timeline lens.
    pub created_at_secs: i64,
    pub message: String,
}

#[derive(Clone, Debug)]
pub struct EcsServiceLb {
    pub target_group_name: String, // readable name (for display + the ELB jump)
    pub load_balancer_name: String,
    pub container_name: String,
    pub container_port: Option<i32>,
}

#[derive(Clone, Debug)]
pub struct EcsServiceInfo {
    pub service_arn: String,
    pub service_name: String,
    #[allow(dead_code)]
    pub cluster_arn: String,
    pub cluster_name: String,
    pub status: String,
    pub desired_count: i32,
    pub running_count: i32,
    pub pending_count: i32,
    pub task_definition: String,
    pub launch_type: String,
    pub platform_version: Option<String>,
    pub scheduling_strategy: Option<String>,
    pub deployment_controller: Option<String>,
    pub created_at: String,
    pub role_arn: Option<String>,
    pub propagate_tags: Option<String>,
    pub enable_execute_command: bool,
    // Deployment configuration
    pub min_healthy_percent: Option<i32>,
    pub max_percent: Option<i32>,
    pub circuit_breaker: Option<(bool, bool)>, // (enable, rollback)
    pub deployments: Vec<EcsDeployment>,
    // Networking
    pub load_balancers: Vec<EcsServiceLb>,
    pub service_registries: Vec<String>,
    pub awsvpc_subnets: Vec<String>,
    pub awsvpc_security_groups: Vec<String>,
    pub assign_public_ip: Option<String>,
    pub capacity_provider_strategy: Vec<String>,
    pub events: Vec<EcsServiceEvent>,
    pub tags: HashMap<String, String>,
}

impl EcsServiceInfo {
    pub fn from_sdk(svc: &aws_sdk_ecs::types::Service) -> Self {
        let mut tags = HashMap::new();
        for tag in svc.tags() {
            if let (Some(k), Some(v)) = (tag.key(), tag.value()) {
                tags.insert(k.to_string(), v.to_string());
            }
        }

        let cluster_name = short_arn(svc.cluster_arn().unwrap_or_default());
        let task_definition = short_arn(svc.task_definition().unwrap_or_default());

        let (min_healthy_percent, max_percent, circuit_breaker) = match svc.deployment_configuration()
        {
            Some(dc) => (
                dc.minimum_healthy_percent(),
                dc.maximum_percent(),
                dc.deployment_circuit_breaker()
                    .map(|cb| (cb.enable(), cb.rollback())),
            ),
            None => (None, None, None),
        };

        let deployments = svc
            .deployments()
            .iter()
            .map(|d| EcsDeployment {
                status: d.status().unwrap_or_default().to_string(),
                task_definition: short_arn(d.task_definition().unwrap_or_default()),
                desired: d.desired_count(),
                pending: d.pending_count(),
                running: d.running_count(),
                failed: d.failed_tasks(),
                rollout_state: d.rollout_state().map(|s| s.as_str().to_string()),
                rollout_state_reason: d.rollout_state_reason().map(|s| s.to_string()),
                created_at: fmt_opt_secs(d.created_at().map(|dt| dt.secs())),
                updated_at: fmt_opt_secs(d.updated_at().map(|dt| dt.secs())),
                updated_at_secs: d.updated_at().map(|dt| dt.secs()).unwrap_or(0),
            })
            .collect();

        let load_balancers = svc
            .load_balancers()
            .iter()
            .map(|lb| {
                // Target group ARN = `…:targetgroup/NAME/id`; the *name* is the
                // segment after `targetgroup/` (short_arn would give the id).
                let target_group_name = lb
                    .target_group_arn()
                    .and_then(|arn| {
                        arn.split_once(":targetgroup/")
                            .and_then(|(_, rest)| rest.split('/').next())
                            .map(|s| s.to_string())
                    })
                    .unwrap_or_default();
                EcsServiceLb {
                    target_group_name,
                    load_balancer_name: lb.load_balancer_name().unwrap_or_default().to_string(),
                    container_name: lb.container_name().unwrap_or_default().to_string(),
                    container_port: lb.container_port(),
                }
            })
            .collect();

        let service_registries = svc
            .service_registries()
            .iter()
            .filter_map(|r| r.registry_arn().map(short_arn))
            .collect();

        let (awsvpc_subnets, awsvpc_security_groups, assign_public_ip) =
            match svc.network_configuration().and_then(|n| n.awsvpc_configuration()) {
                Some(c) => (
                    c.subnets().to_vec(),
                    c.security_groups().to_vec(),
                    c.assign_public_ip().map(|a| a.as_str().to_string()),
                ),
                None => (Vec::new(), Vec::new(), None),
            };

        let capacity_provider_strategy = svc
            .capacity_provider_strategy()
            .iter()
            .map(|item| {
                format!(
                    "{} (base={}, weight={})",
                    item.capacity_provider(),
                    item.base(),
                    item.weight()
                )
            })
            .collect();

        let events = svc
            .events()
            .iter()
            .take(50)
            .map(|e| EcsServiceEvent {
                created_at: fmt_opt_secs(e.created_at().map(|dt| dt.secs())),
                created_at_secs: e.created_at().map(|dt| dt.secs()).unwrap_or(0),
                message: e.message().unwrap_or_default().to_string(),
            })
            .collect();

        Self {
            service_arn: svc.service_arn().unwrap_or_default().to_string(),
            service_name: svc.service_name().unwrap_or_default().to_string(),
            cluster_arn: svc.cluster_arn().unwrap_or_default().to_string(),
            cluster_name,
            status: svc.status().unwrap_or_default().to_string(),
            desired_count: svc.desired_count(),
            running_count: svc.running_count(),
            pending_count: svc.pending_count(),
            task_definition,
            launch_type: svc
                .launch_type()
                .map(|lt| lt.as_str().to_string())
                .unwrap_or_else(|| "UNKNOWN".to_string()),
            platform_version: svc.platform_version().map(|s| s.to_string()),
            scheduling_strategy: svc.scheduling_strategy().map(|s| s.as_str().to_string()),
            deployment_controller: svc
                .deployment_controller()
                .map(|c| c.r#type().as_str().to_string()),
            created_at: fmt_opt_secs(svc.created_at().map(|dt| dt.secs())),
            role_arn: svc.role_arn().map(|s| s.to_string()),
            propagate_tags: svc.propagate_tags().map(|p| p.as_str().to_string()),
            enable_execute_command: svc.enable_execute_command(),
            min_healthy_percent,
            max_percent,
            circuit_breaker,
            deployments,
            load_balancers,
            service_registries,
            awsvpc_subnets,
            awsvpc_security_groups,
            assign_public_ip,
            capacity_provider_strategy,
            events,
            tags,
        }
    }
}

crate::sections! {
    pub enum EcsServiceDetailSection,
    pub static ECS_SERVICE_SECTIONS = [
        Overview "Overview",
        Deployments "Deployments",
        Tasks "Tasks" => crate::app::App::trigger_ecs_service_tasks_load,
        Networking "Networking",
        Events "Events",
        Tags "Tags",
        Optimizer "Optimizer" => crate::app::App::trigger_optimizer_load,
    ]
}

impl Resource for EcsServiceInfo {
    fn references(&self) -> Vec<(String, String)> {
        let mut v = sg_refs(self.security_group_ids());
        let mut r = |l: &str, x: &str| v.push((l.to_string(), x.to_string()));
        for x in &self.awsvpc_subnets { r("Subnet", x); }
        r("Cluster", &self.cluster_arn);
        r("Cluster", &self.cluster_name);
        r("Task Definition", &self.task_definition);
        if let Some(x) = &self.role_arn { r("Role", x); }
        for lb in &self.load_balancers {
            r("Target Group", &lb.target_group_name);
            r("Load Balancer", &lb.load_balancer_name);
        }
        for x in &self.service_registries { r("Service Registry", x); }
        v
    }

    fn security_group_ids(&self) -> Vec<String> {
        self.awsvpc_security_groups.clone()
    }

    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&ECS_SERVICE_SECTIONS)
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws ecs describe-services --cluster {} --services {}",
            crate::aws::resource::shell_quote(&self.cluster_name),
            crate::aws::resource::shell_quote(&self.service_name)
        ))
    }

    fn id(&self) -> &str {
        &self.service_arn
    }

    fn name(&self) -> &str {
        &self.service_name
    }

    fn resource_type(&self) -> &str {
        "ECS Service"
    }

    fn state(&self) -> ResourceState {
        // Surface deployment rollout health at a glance: a failed rollout reads
        // red and an in-progress one yellow, so a bad deploy is obvious in the
        // list without opening the service. Falls back to service status.
        if let Some(primary) = self.deployments.iter().find(|d| d.status == "PRIMARY") {
            match primary.rollout_state.as_deref() {
                Some("FAILED") => return ResourceState::Unavailable,
                Some("IN_PROGRESS") => return ResourceState::Pending,
                _ => {}
            }
            // No explicit rollout state (non-ECS deployment controller) but more
            // than one active deployment means the old one is still draining.
            if self.deployments.len() > 1 {
                return ResourceState::Pending;
            }
        }
        match self.status.as_str() {
            "ACTIVE" => ResourceState::Available,
            "INACTIVE" => ResourceState::Unavailable,
            _ => ResourceState::Unknown(self.status.clone()),
        }
    }

    fn state_label(&self) -> String {
        // Mirrors state(): rollout health first, then the service status.
        if let Some(primary) = self.deployments.iter().find(|d| d.status == "PRIMARY") {
            match primary.rollout_state.as_deref() {
                Some("FAILED") => return "rollout failed".to_string(),
                Some("IN_PROGRESS") => return "rollout in progress".to_string(),
                _ => {}
            }
            if self.deployments.len() > 1 {
                return "draining".to_string();
            }
        }
        native_state_label(&self.status, || self.state())
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        let mut parts = vec![
            self.service_arn.clone(),
            self.service_name.clone(),
            self.cluster_name.clone(),
            self.status.clone(),
            self.task_definition.clone(),
            self.launch_type.clone(),
        ];
        for (k, v) in &self.tags {
            parts.push(format!("{}:{}", k, v));
        }
        parts.join(" ")
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Service ARN".to_string(), self.service_arn.clone()),
            ("Service Name".to_string(), self.service_name.clone()),
            ("Cluster".to_string(), self.cluster_name.clone()),
            ("Status".to_string(), self.status.clone()),
            ("Desired Count".to_string(), self.desired_count.to_string()),
            ("Running Count".to_string(), self.running_count.to_string()),
            ("Pending Count".to_string(), self.pending_count.to_string()),
            ("Task Definition".to_string(), self.task_definition.clone()),
            ("Launch Type".to_string(), self.launch_type.clone()),
        ]
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{region}.console.aws.amazon.com/ecs/v2/clusters/{}/services/{}/health?region={region}",
            self.cluster_name, self.service_name
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── ECS Task Definition ───────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct EcsTaskDefinition {
    pub arn: String,
    pub family: String,
    pub revision: i32,
    pub status: String,
    pub tags: HashMap<String, String>,
}

impl EcsTaskDefinition {
    /// Parse family and revision from the ARN to avoid per-definition describe API calls
    /// during the list phase. Full container detail is fetched lazily on detail-pane open.
    /// ARN format: arn:aws:ecs:region:account:task-definition/family:revision
    pub fn from_arn(arn: &str) -> Self {
        let last_part = arn.rsplit('/').next().unwrap_or_default();
        let mut parts = last_part.splitn(2, ':');
        let family = parts.next().unwrap_or(last_part).to_string();
        let revision = parts
            .next()
            .and_then(|r| r.parse::<i32>().ok())
            .unwrap_or(0);

        Self {
            arn: arn.to_string(),
            family,
            revision,
            status: "ACTIVE".to_string(),
            tags: HashMap::new(),
        }
    }
}

crate::sections! {
    pub enum EcsTaskDefDetailSection,
    pub static ECS_TASKDEF_SECTIONS = [
        Overview "Overview" => crate::app::App::trigger_ecs_taskdef_details_load,
        Containers "Containers" => crate::app::App::trigger_ecs_taskdef_details_load,
        Volumes "Volumes" => crate::app::App::trigger_ecs_taskdef_details_load,
        Tags "Tags" => crate::app::App::trigger_ecs_taskdef_details_load,
    ]
}

impl Resource for EcsTaskDefinition {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&ECS_TASKDEF_SECTIONS)
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws ecs describe-task-definition --task-definition {}",
            crate::aws::resource::shell_quote(self.id())
        ))
    }

    fn id(&self) -> &str {
        &self.arn
    }

    fn name(&self) -> &str {
        &self.family
    }

    fn resource_type(&self) -> &str {
        "Task Definition"
    }

    fn state(&self) -> ResourceState {
        ResourceState::stateless()
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!("{} {} {}", self.arn, self.family, self.revision)
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("ARN".to_string(), self.arn.clone()),
            ("Family".to_string(), self.family.clone()),
            ("Revision".to_string(), self.revision.to_string()),
            ("Status".to_string(), self.status.clone()),
        ]
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{region}.console.aws.amazon.com/ecs/v2/task-definitions/{}/{revision}?region={region}",
            self.family,
            revision = self.revision
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── ECS Task Definition lazy detail ─────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct EcsContainerDef {
    pub name: String,
    pub image: String,
    pub cpu: i32,
    pub memory: Option<i32>,
    pub memory_reservation: Option<i32>,
    pub essential: bool,
    pub port_mappings: Vec<String>,
    pub entry_point: Vec<String>,
    pub command: Vec<String>,
    pub environment: Vec<(String, String)>,
    pub secrets: Vec<(String, String)>, // (name, value_from)
    pub log_driver: Option<String>,
    pub log_options: Vec<(String, String)>,
    pub has_health_check: bool,
    pub mount_points: usize,
}

#[derive(Clone, Debug)]
pub struct EcsVolumeDef {
    pub name: String,
    pub kind: String, // host / efs / docker / configured-at-launch
}

#[derive(Clone, Debug)]
pub struct EcsTaskDefinitionDetails {
    pub task_role_arn: Option<String>,
    pub execution_role_arn: Option<String>,
    pub network_mode: Option<String>,
    pub cpu: Option<String>,
    pub memory: Option<String>,
    pub requires_compatibilities: Vec<String>,
    pub cpu_architecture: Option<String>,
    pub os_family: Option<String>,
    pub pid_mode: Option<String>,
    pub ipc_mode: Option<String>,
    pub registered_at: String,
    pub containers: Vec<EcsContainerDef>,
    pub volumes: Vec<EcsVolumeDef>,
    pub tags: Vec<(String, String)>,
}

/// Lazy-load full task-definition detail (containers, roles, network mode, …) keyed by ARN.
pub async fn fetch_task_definition_details(
    client: EcsClient,
    arn: String,
) -> Result<EcsTaskDefinitionDetails> {
    let resp = client
        .describe_task_definition()
        .task_definition(&arn)
        .include(TaskDefinitionField::Tags)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;

    let mut tags = Vec::new();
    for tag in resp.tags() {
        if let (Some(k), Some(v)) = (tag.key(), tag.value()) {
            tags.push((k.to_string(), v.to_string()));
        }
    }

    let td = match resp.task_definition() {
        Some(td) => td,
        None => {
            return Err(crate::error::Error::ResourceNotFound(arn));
        }
    };

    let runtime = td.runtime_platform();
    let cpu_architecture = runtime
        .and_then(|r| r.cpu_architecture())
        .map(|a| a.as_str().to_string());
    let os_family = runtime
        .and_then(|r| r.operating_system_family())
        .map(|o| o.as_str().to_string());

    let containers = td
        .container_definitions()
        .iter()
        .map(|c| {
            let port_mappings = c
                .port_mappings()
                .iter()
                .map(|p| {
                    let proto = p
                        .protocol()
                        .map(|pr| pr.as_str().to_string())
                        .unwrap_or_else(|| "tcp".to_string());
                    match (p.container_port(), p.host_port()) {
                        (Some(cp), Some(hp)) if hp != cp => format!("{}:{}/{}", hp, cp, proto),
                        (Some(cp), _) => format!("{}/{}", cp, proto),
                        _ => proto,
                    }
                })
                .collect();

            let environment = c
                .environment()
                .iter()
                .filter_map(|kv| match (kv.name(), kv.value()) {
                    (Some(n), Some(v)) => Some((n.to_string(), v.to_string())),
                    _ => None,
                })
                .collect();

            let secrets = c
                .secrets()
                .iter()
                .map(|s| (s.name().to_string(), short_arn(s.value_from())))
                .collect();

            let (log_driver, log_options) = match c.log_configuration() {
                Some(lc) => (
                    Some(lc.log_driver().as_str().to_string()),
                    lc.options()
                        .map(|o| {
                            let mut v: Vec<(String, String)> =
                                o.iter().map(|(k, val)| (k.clone(), val.clone())).collect();
                            v.sort_by(|a, b| a.0.cmp(&b.0));
                            v
                        })
                        .unwrap_or_default(),
                ),
                None => (None, Vec::new()),
            };

            EcsContainerDef {
                name: c.name().unwrap_or_default().to_string(),
                image: c.image().unwrap_or_default().to_string(),
                cpu: c.cpu(),
                memory: c.memory(),
                memory_reservation: c.memory_reservation(),
                essential: c.essential().unwrap_or(true),
                port_mappings,
                entry_point: c.entry_point().to_vec(),
                command: c.command().to_vec(),
                environment,
                secrets,
                log_driver,
                log_options,
                has_health_check: c.health_check().is_some(),
                mount_points: c.mount_points().len(),
            }
        })
        .collect();

    let volumes = td
        .volumes()
        .iter()
        .map(|v| {
            let kind = if v.efs_volume_configuration().is_some() {
                "EFS".to_string()
            } else if v.host().is_some() {
                "host".to_string()
            } else if v.configured_at_launch().unwrap_or(false) {
                "configured-at-launch".to_string()
            } else {
                "docker".to_string()
            };
            EcsVolumeDef {
                name: v.name().unwrap_or_default().to_string(),
                kind,
            }
        })
        .collect();

    Ok(EcsTaskDefinitionDetails {
        task_role_arn: td.task_role_arn().map(|s| s.to_string()),
        execution_role_arn: td.execution_role_arn().map(|s| s.to_string()),
        network_mode: td.network_mode().map(|n| n.as_str().to_string()),
        cpu: td.cpu().map(|s| s.to_string()),
        memory: td.memory().map(|s| s.to_string()),
        requires_compatibilities: td
            .requires_compatibilities()
            .iter()
            .map(|c| c.as_str().to_string())
            .collect(),
        cpu_architecture,
        os_family,
        pid_mode: td.pid_mode().map(|p| p.as_str().to_string()),
        ipc_mode: td.ipc_mode().map(|i| i.as_str().to_string()),
        registered_at: fmt_opt_secs(td.registered_at().map(|dt| dt.secs())),
        containers,
        volumes,
        tags,
    })
}

/// Fetch recent CloudWatch log events for a running/stopped task's containers.
///
/// The log destination lives on the task *definition* (the container's
/// `awslogs` driver config), so this reads the definition, computes each
/// container's log stream (`{stream-prefix}/{container}/{task-id}`), and pulls
/// the most recent events via `get_log_events`. Only the `awslogs` driver is
/// viewable here — other drivers (awsfirelens, splunk, …) ship logs elsewhere.
pub async fn fetch_task_logs(
    ecs_client: EcsClient,
    logs_client: aws_sdk_cloudwatchlogs::Client,
    task_def: String,
    task_id: String,
    container_names: Vec<String>,
    limit: i32,
) -> Result<String> {
    let resp = ecs_client
        .describe_task_definition()
        .task_definition(&task_def)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;

    let td = resp
        .task_definition()
        .ok_or_else(|| crate::error::Error::ResourceNotFound(task_def.clone()))?;

    let mut out = String::new();

    for cd in td.container_definitions() {
        let cname = cd.name().unwrap_or_default().to_string();
        // Restrict to containers that actually exist on this task, when known.
        if !container_names.is_empty() && !container_names.contains(&cname) {
            continue;
        }

        let lc = match cd.log_configuration() {
            Some(lc) => lc,
            None => {
                out.push_str(&format!("─── {} ───\n  no log configuration\n\n", cname));
                continue;
            }
        };

        let driver = lc.log_driver().as_str();
        if driver != "awslogs" {
            out.push_str(&format!(
                "─── {} ───\n  log driver '{}' not viewable here (only awslogs)\n\n",
                cname, driver
            ));
            continue;
        }

        let opts = lc.options();
        let group = opts.and_then(|o| o.get("awslogs-group")).cloned();
        let prefix = opts.and_then(|o| o.get("awslogs-stream-prefix")).cloned();

        let (group, prefix) = match (group, prefix) {
            (Some(g), Some(p)) => (g, p),
            _ => {
                out.push_str(&format!(
                    "─── {} ───\n  awslogs config missing group or stream-prefix\n\n",
                    cname
                ));
                continue;
            }
        };

        let stream = format!("{}/{}/{}", prefix, cname, task_id);
        out.push_str(&format!("─── {} → {} : {} ───\n", cname, group, stream));

        match logs_client
            .get_log_events()
            .log_group_name(&group)
            .log_stream_name(&stream)
            .limit(limit)
            .start_from_head(false)
            .send()
            .await
        {
            Ok(ev) => {
                let events = ev.events();
                if events.is_empty() {
                    out.push_str("  (no recent log events)\n");
                } else {
                    for e in events {
                        let ts = e
                            .timestamp()
                            .map(|ms| fmt_epoch_secs(ms / 1000))
                            .unwrap_or_default();
                        let msg = e.message().unwrap_or_default();
                        for (i, line) in msg.trim_end().lines().enumerate() {
                            if i == 0 {
                                out.push_str(&format!("{}  {}\n", ts, line));
                            } else {
                                out.push_str(&format!("{}  {}\n", " ".repeat(ts.len()), line));
                            }
                        }
                    }
                }
            }
            Err(e) => {
                out.push_str(&format!("  error fetching logs: {}\n", e));
            }
        }
        out.push('\n');
    }

    if out.is_empty() {
        out.push_str("No containers with log configuration found on this task definition.");
    }
    Ok(out)
}

// ── ECS Task ──────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct EcsTaskContainer {
    pub name: String,
    pub last_status: String,
    pub health_status: Option<String>,
    pub exit_code: Option<i32>,
    pub reason: Option<String>,
    pub image: String,
}

#[derive(Clone, Debug)]
pub struct EcsTaskEni {
    pub eni_id: Option<String>,
    pub private_ip: Option<String>,
    pub subnet_id: Option<String>,
}

#[derive(Clone, Debug)]
pub struct EcsTask {
    pub task_arn: String,
    pub task_id: String,
    /// Readable list label: `service · shortid` for service-managed tasks,
    /// else the full task id.
    pub display_name: String,
    /// Owning ECS service name, parsed from a `service:NAME` group (None for
    /// standalone / run-task tasks).
    pub service_name: Option<String>,
    #[allow(dead_code)]
    pub cluster_arn: String,
    pub cluster_name: String,
    pub task_definition: String,
    pub last_status: String,
    pub desired_status: String,
    pub launch_type: String,
    pub cpu: String,
    pub memory: String,
    pub group: String,
    pub platform_version: Option<String>,
    pub health_status: Option<String>,
    pub availability_zone: Option<String>,
    pub capacity_provider: Option<String>,
    pub started_by: Option<String>,
    pub connectivity: Option<String>,
    pub created_at: String,
    pub started_at: String,
    pub stopped_at: String,
    pub stop_code: Option<String>,
    pub stopped_reason: Option<String>,
    pub containers: Vec<EcsTaskContainer>,
    pub enis: Vec<EcsTaskEni>,
    pub tags: HashMap<String, String>,
}

impl EcsTask {
    pub fn from_sdk(task: &aws_sdk_ecs::types::Task) -> Self {
        let mut tags = HashMap::new();
        for tag in task.tags() {
            if let (Some(k), Some(v)) = (tag.key(), tag.value()) {
                tags.insert(k.to_string(), v.to_string());
            }
        }

        let task_arn = task.task_arn().unwrap_or_default().to_string();
        let task_id = short_arn(&task_arn);

        // A service-managed task's group is `service:<name>`; surface that so the
        // Tasks list is readable instead of a wall of 32-char ids.
        let group = task.group().unwrap_or_default().to_string();
        let service_name = group
            .strip_prefix("service:")
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());
        let short_id: String = task_id.chars().take(8).collect();
        let display_name = match &service_name {
            Some(svc) => format!("{} · {}", svc, short_id),
            None => task_id.clone(),
        };

        let cluster_arn = task.cluster_arn().unwrap_or_default().to_string();
        let cluster_name = short_arn(&cluster_arn);
        let task_definition = short_arn(task.task_definition_arn().unwrap_or_default());

        let containers = task
            .containers()
            .iter()
            .map(|c| EcsTaskContainer {
                name: c.name().unwrap_or_default().to_string(),
                last_status: c.last_status().unwrap_or_default().to_string(),
                health_status: c.health_status().map(|h| h.as_str().to_string()),
                exit_code: c.exit_code(),
                reason: c.reason().map(|s| s.to_string()),
                image: c.image().unwrap_or_default().to_string(),
            })
            .collect();

        // Extract ENI details from ElasticNetworkInterface attachments.
        let enis = task
            .attachments()
            .iter()
            .filter(|a| a.r#type() == Some("ElasticNetworkInterface"))
            .map(|a| {
                let lookup = |key: &str| {
                    a.details()
                        .iter()
                        .find(|kv| kv.name() == Some(key))
                        .and_then(|kv| kv.value())
                        .map(|s| s.to_string())
                };
                EcsTaskEni {
                    eni_id: lookup("networkInterfaceId"),
                    private_ip: lookup("privateIPv4Address"),
                    subnet_id: lookup("subnetId"),
                }
            })
            .collect();

        Self {
            task_arn,
            task_id,
            display_name,
            service_name,
            cluster_arn,
            cluster_name,
            task_definition,
            last_status: task.last_status().unwrap_or_default().to_string(),
            desired_status: task.desired_status().unwrap_or_default().to_string(),
            launch_type: task
                .launch_type()
                .map(|lt| lt.as_str().to_string())
                .unwrap_or_else(|| "UNKNOWN".to_string()),
            cpu: task.cpu().unwrap_or_default().to_string(),
            memory: task.memory().unwrap_or_default().to_string(),
            group,
            platform_version: task.platform_version().map(|s| s.to_string()),
            health_status: task.health_status().map(|h| h.as_str().to_string()),
            availability_zone: task.availability_zone().map(|s| s.to_string()),
            capacity_provider: task.capacity_provider_name().map(|s| s.to_string()),
            started_by: task.started_by().map(|s| s.to_string()),
            connectivity: task.connectivity().map(|c| c.as_str().to_string()),
            created_at: fmt_opt_secs(task.created_at().map(|dt| dt.secs())),
            started_at: fmt_opt_secs(task.started_at().map(|dt| dt.secs())),
            stopped_at: fmt_opt_secs(task.stopped_at().map(|dt| dt.secs())),
            stop_code: task.stop_code().map(|c| c.as_str().to_string()),
            stopped_reason: task.stopped_reason().map(|s| s.to_string()),
            containers,
            enis,
            tags,
        }
    }

    pub fn is_stopped(&self) -> bool {
        self.stop_code.is_some() || self.last_status == "STOPPED"
    }
}

crate::sections! {
    pub enum EcsTaskDetailSection,
    pub static ECS_TASK_SECTIONS = [
        Overview "Overview",
        Containers "Containers",
        Networking "Networking",
        Tags "Tags",
    ]
}

impl Resource for EcsTask {
    fn references(&self) -> Vec<(String, String)> {
        let mut v = Vec::new();
        let mut r = |l: &str, x: &str| v.push((l.to_string(), x.to_string()));
        r("Cluster", &self.cluster_arn);
        r("Cluster", &self.cluster_name);
        r("Task Definition", &self.task_definition);
        if let Some(x) = &self.service_name { r("Service", x); }
        for eni in &self.enis {
            if let Some(x) = &eni.eni_id { r("Network Interface", x); }
            if let Some(x) = &eni.subnet_id { r("Subnet", x); }
        }
        for c in &self.containers { r("Image", &c.image); }
        v
    }

    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&ECS_TASK_SECTIONS)
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws ecs describe-tasks --cluster {} --tasks {}",
            crate::aws::resource::shell_quote(&self.cluster_name),
            crate::aws::resource::shell_quote(&self.task_arn)
        ))
    }

    fn id(&self) -> &str {
        &self.task_arn
    }

    fn name(&self) -> &str {
        &self.display_name
    }

    fn resource_type(&self) -> &str {
        "ECS Task"
    }

    fn state(&self) -> ResourceState {
        match self.last_status.as_str() {
            "RUNNING" => ResourceState::Running,
            "STOPPED" => ResourceState::Stopped,
            "PENDING" | "PROVISIONING" | "ACTIVATING" => ResourceState::Pending,
            "DEPROVISIONING" | "DEACTIVATING" | "STOPPING" => ResourceState::Deleting,
            _ => ResourceState::Unknown(self.last_status.clone()),
        }
    }

    fn state_label(&self) -> String {
        native_state_label(&self.last_status, || self.state())
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        let mut parts = vec![
            self.task_arn.clone(),
            self.task_id.clone(),
            self.cluster_name.clone(),
            self.task_definition.clone(),
            self.last_status.clone(),
            self.launch_type.clone(),
            self.group.clone(),
        ];
        for (k, v) in &self.tags {
            parts.push(format!("{}:{}", k, v));
        }
        parts.join(" ")
    }

    fn details(&self) -> Vec<(String, String)> {
        let mut d = vec![
            ("Task ARN".to_string(), self.task_arn.clone()),
            ("Cluster".to_string(), self.cluster_name.clone()),
            ("Task Definition".to_string(), self.task_definition.clone()),
            ("Last Status".to_string(), self.last_status.clone()),
            ("Desired Status".to_string(), self.desired_status.clone()),
            ("Launch Type".to_string(), self.launch_type.clone()),
        ];
        if !self.cpu.is_empty() {
            d.push(("CPU".to_string(), self.cpu.clone()));
        }
        if !self.memory.is_empty() {
            d.push(("Memory".to_string(), self.memory.clone()));
        }
        if !self.group.is_empty() {
            d.push(("Group".to_string(), self.group.clone()));
        }
        d
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{region}.console.aws.amazon.com/ecs/v2/clusters/{}/tasks/{}/configuration?region={region}",
            self.cluster_name, self.task_id
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── ECS service metrics (CloudWatch) ─────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct EcsMetricsData {
    pub time_range: MetricsTimeRange,
    pub cpu: Vec<(f64, f64)>,     // (secs_from_start, avg %)
    pub memory: Vec<(f64, f64)>,  // (secs_from_start, avg %)
    pub running: Vec<(f64, f64)>, // (secs_from_start, avg count)
    pub pending: Vec<(f64, f64)>, // (secs_from_start, avg count)
    pub x_max: f64,
}

#[derive(Debug, Clone)]
pub enum EcsMetricsState {
    Loading,
    Loaded(EcsMetricsData),
}

fn parse_dp_avg(datapoints: &[Datapoint], start_secs: i64) -> Vec<(f64, f64)> {
    let mut pts: Vec<(f64, f64)> = datapoints
        .iter()
        .filter_map(|dp| {
            let t = (dp.timestamp()?.secs() - start_secs) as f64;
            Some((t, dp.average().unwrap_or(0.0)))
        })
        .collect();
    pts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    pts
}

pub async fn fetch_ecs_metrics(
    cw_client: aws_sdk_cloudwatch::Client,
    cluster_name: String,
    service_name: String,
    time_range: MetricsTimeRange,
) -> Result<EcsMetricsData> {
    use aws_sdk_cloudwatch::types::{Dimension, Statistic};

    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let start_secs = now_secs - time_range.duration_secs();
    let period = time_range.period_secs();

    let start_dt = aws_sdk_cloudwatch::primitives::DateTime::from_secs(start_secs);
    let end_dt = aws_sdk_cloudwatch::primitives::DateTime::from_secs(now_secs);

    let make_dims = || {
        vec![
            Dimension::builder()
                .name("ClusterName")
                .value(&cluster_name)
                .build(),
            Dimension::builder()
                .name("ServiceName")
                .value(&service_name)
                .build(),
        ]
    };

    // ECS service utilization lives in the Container Insights namespace
    // (`CpuUtilized`/`MemoryUtilized` vs `*Reserved`), not `AWS/ECS` — the
    // latter only publishes CPUUtilization/MemoryUtilization for services whose
    // task defs declare CPU/memory reservations, and never carries task counts.
    // CPU% / Memory% are derived from the utilized-vs-reserved pair.
    // Requires Container Insights enabled on the cluster.
    let metric = |name: &'static str| {
        cw_client
            .get_metric_statistics()
            .namespace("ECS/ContainerInsights")
            .metric_name(name)
            .set_dimensions(Some(make_dims()))
            .start_time(start_dt.clone())
            .end_time(end_dt.clone())
            .period(period)
            .set_statistics(Some(vec![Statistic::Average]))
            .send()
    };

    let (cpu_u_r, cpu_res_r, mem_u_r, mem_res_r, run_r, pend_r) = tokio::join!(
        metric("CpuUtilized"),
        metric("CpuReserved"),
        metric("MemoryUtilized"),
        metric("MemoryReserved"),
        metric("RunningTaskCount"),
        metric("PendingTaskCount"),
    );

    let dps = |r: std::result::Result<
        aws_sdk_cloudwatch::operation::get_metric_statistics::GetMetricStatisticsOutput,
        _,
    >| match r {
        Ok(o) => o.datapoints().to_vec(),
        Err(_) => vec![],
    };

    Ok(EcsMetricsData {
        time_range,
        cpu: ratio_pct(&dps(cpu_u_r), &dps(cpu_res_r), start_secs),
        memory: ratio_pct(&dps(mem_u_r), &dps(mem_res_r), start_secs),
        running: parse_dp_avg(&dps(run_r), start_secs),
        pending: parse_dp_avg(&dps(pend_r), start_secs),
        x_max: time_range.duration_secs() as f64,
    })
}

// ── ECS per-task / per-container metrics (enhanced observability) ─────────────

/// One container's utilization series within a task.
#[derive(Debug, Clone)]
pub struct EcsContainerSeries {
    pub name: String,
    pub cpu: Vec<(f64, f64)>,    // ContainerCpuUtilization (%)
    pub memory: Vec<(f64, f64)>, // ContainerMemoryUtilization (%)
}

#[derive(Debug, Clone)]
pub struct EcsTaskMetricsData {
    pub time_range: MetricsTimeRange,
    pub cpu: Vec<(f64, f64)>,           // TaskCpuUtilization (%)
    pub memory: Vec<(f64, f64)>,        // TaskMemoryUtilization (%)
    pub net_rx: Vec<(f64, f64)>,        // NetworkRxBytes (Bytes/s)
    pub net_tx: Vec<(f64, f64)>,        // NetworkTxBytes (Bytes/s)
    pub storage_read: Vec<(f64, f64)>,  // StorageReadBytes
    pub storage_write: Vec<(f64, f64)>, // StorageWriteBytes
    pub containers: Vec<EcsContainerSeries>,
    pub x_max: f64,
    /// True when every series came back empty — Container Insights with enhanced
    /// observability is likely not enabled on the cluster (caller shows a hint).
    pub empty: bool,
}

#[derive(Debug, Clone)]
pub enum EcsTaskMetricsState {
    Loading,
    Loaded(EcsTaskMetricsData),
}

/// Per-task + per-container metrics from `ECS/ContainerInsights`. These series
/// are only published when Container Insights with **enhanced observability** is
/// enabled on the cluster; without it every series is empty (`empty` flag → the
/// overlay shows a hint). Keyed on `ClusterName + TaskDefinitionFamily + TaskId`
/// so it works for both service-managed and standalone (run-task) tasks.
pub async fn fetch_ecs_task_metrics(
    cw_client: aws_sdk_cloudwatch::Client,
    cluster_name: String,
    task_def_family: String,
    task_id: String,
    container_names: Vec<String>,
    time_range: MetricsTimeRange,
) -> Result<EcsTaskMetricsData> {
    use aws_sdk_cloudwatch::types::{Dimension, Statistic};

    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let start_secs = now_secs - time_range.duration_secs();
    let period = time_range.period_secs();
    let start_dt = aws_sdk_cloudwatch::primitives::DateTime::from_secs(start_secs);
    let end_dt = aws_sdk_cloudwatch::primitives::DateTime::from_secs(now_secs);

    let dps = |r: std::result::Result<
        aws_sdk_cloudwatch::operation::get_metric_statistics::GetMetricStatisticsOutput,
        _,
    >| match r {
        Ok(o) => o.datapoints().to_vec(),
        Err(_) => vec![],
    };

    // The 3-dimension task-level set (no ServiceName dependency).
    let task_dims = || {
        vec![
            Dimension::builder().name("ClusterName").value(&cluster_name).build(),
            Dimension::builder()
                .name("TaskDefinitionFamily")
                .value(&task_def_family)
                .build(),
            Dimension::builder().name("TaskId").value(&task_id).build(),
        ]
    };
    let task_metric = |name: &'static str| {
        cw_client
            .get_metric_statistics()
            .namespace("ECS/ContainerInsights")
            .metric_name(name)
            .set_dimensions(Some(task_dims()))
            .start_time(start_dt.clone())
            .end_time(end_dt.clone())
            .period(period)
            .set_statistics(Some(vec![Statistic::Average]))
            .send()
    };

    let (cpu_r, mem_r, rx_r, tx_r, sr_r, sw_r) = tokio::join!(
        task_metric("TaskCpuUtilization"),
        task_metric("TaskMemoryUtilization"),
        task_metric("NetworkRxBytes"),
        task_metric("NetworkTxBytes"),
        task_metric("StorageReadBytes"),
        task_metric("StorageWriteBytes"),
    );

    // Per-container CPU% / Memory% (4-dimension set adds ContainerName).
    let mut containers: Vec<EcsContainerSeries> = Vec::new();
    for cname in &container_names {
        let cdims = || {
            vec![
                Dimension::builder().name("ClusterName").value(&cluster_name).build(),
                Dimension::builder()
                    .name("TaskDefinitionFamily")
                    .value(&task_def_family)
                    .build(),
                Dimension::builder().name("TaskId").value(&task_id).build(),
                Dimension::builder().name("ContainerName").value(cname).build(),
            ]
        };
        let cmetric = |name: &'static str| {
            cw_client
                .get_metric_statistics()
                .namespace("ECS/ContainerInsights")
                .metric_name(name)
                .set_dimensions(Some(cdims()))
                .start_time(start_dt.clone())
                .end_time(end_dt.clone())
                .period(period)
                .set_statistics(Some(vec![Statistic::Average]))
                .send()
        };
        let (ccpu, cmem) = tokio::join!(
            cmetric("ContainerCpuUtilization"),
            cmetric("ContainerMemoryUtilization"),
        );
        containers.push(EcsContainerSeries {
            name: cname.clone(),
            cpu: parse_dp_avg(&dps(ccpu), start_secs),
            memory: parse_dp_avg(&dps(cmem), start_secs),
        });
    }

    let cpu = parse_dp_avg(&dps(cpu_r), start_secs);
    let memory = parse_dp_avg(&dps(mem_r), start_secs);
    let net_rx = parse_dp_avg(&dps(rx_r), start_secs);
    let net_tx = parse_dp_avg(&dps(tx_r), start_secs);
    let storage_read = parse_dp_avg(&dps(sr_r), start_secs);
    let storage_write = parse_dp_avg(&dps(sw_r), start_secs);

    let empty = cpu.is_empty()
        && memory.is_empty()
        && net_rx.is_empty()
        && net_tx.is_empty()
        && storage_read.is_empty()
        && storage_write.is_empty()
        && containers
            .iter()
            .all(|c| c.cpu.is_empty() && c.memory.is_empty());

    Ok(EcsTaskMetricsData {
        time_range,
        cpu,
        memory,
        net_rx,
        net_tx,
        storage_read,
        storage_write,
        containers,
        x_max: time_range.duration_secs() as f64,
        empty,
    })
}

/// Compute a utilization percentage series by dividing a "utilized" datapoint
/// set by its "reserved" companion, aligned on matching timestamps. Used for
/// ECS Container Insights CPU%/Memory% (CpuUtilized/CpuReserved etc.). Skips
/// timestamps with no/zero reserved value; clamps to 0–100.
fn ratio_pct(num: &[Datapoint], den: &[Datapoint], start_secs: i64) -> Vec<(f64, f64)> {
    use std::collections::HashMap;
    let den_by_ts: HashMap<i64, f64> = den
        .iter()
        .filter_map(|d| Some((d.timestamp()?.secs(), d.average()?)))
        .collect();
    let mut pts: Vec<(f64, f64)> = num
        .iter()
        .filter_map(|d| {
            let ts = d.timestamp()?.secs();
            let used = d.average()?;
            let reserved = den_by_ts.get(&ts).copied()?;
            if reserved <= 0.0 {
                return None;
            }
            let pct = (used / reserved * 100.0).clamp(0.0, 100.0);
            Some(((ts - start_secs) as f64, pct))
        })
        .collect();
    pts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    pts
}

// ── Targeted single-resource refresh (detail-pane `r`) ────────────────────────

/// Re-fetch one ECS service (`describe_services` for a single service) so the
/// detail pane can update in place — e.g. while watching a forced deployment —
/// without reloading every cluster/service.
pub async fn fetch_single_service(
    client: EcsClient,
    cluster_arn: String,
    service_name: String,
) -> Option<EcsServiceInfo> {
    client
        .describe_services()
        .cluster(&cluster_arn)
        .services(&service_name)
        .include(ServiceField::Tags)
        .send()
        .await
        .ok()?
        .services()
        .first()
        .map(EcsServiceInfo::from_sdk)
}

/// Re-fetch one ECS task (`describe_tasks` for a single task ARN) for an
/// in-place detail-pane refresh.
pub async fn fetch_single_task(
    client: EcsClient,
    cluster_arn: String,
    task_arn: String,
) -> Option<EcsTask> {
    client
        .describe_tasks()
        .cluster(&cluster_arn)
        .tasks(&task_arn)
        .include(TaskField::Tags)
        .send()
        .await
        .ok()?
        .tasks()
        .first()
        .map(EcsTask::from_sdk)
}

/// Fetch the tasks belonging to one ECS service for the service detail pane's
/// `Tasks` section: running tasks plus a capped window of recently-stopped ones
/// (so a failed deployment's tasks are visible right where you're debugging,
/// without leaving the service). Running first, then stopped most-recent-first.
pub async fn fetch_service_tasks(
    client: EcsClient,
    cluster_arn: String,
    service_name: String,
) -> Result<Vec<EcsTask>> {
    const MAX_STOPPED: usize = 50;

    async fn list_and_describe(
        client: &EcsClient,
        cluster_arn: &str,
        service_name: &str,
        stopped: bool,
        cap: usize,
    ) -> Vec<EcsTask> {
        let mut builder = client
            .list_tasks()
            .cluster(cluster_arn)
            .service_name(service_name);
        if stopped {
            builder = builder.desired_status(DesiredStatus::Stopped);
        }
        let mut arns: Vec<String> = Vec::new();
        let mut pager = builder.into_paginator().items().send();
        while let Some(result) = pager.next().await {
            match result {
                Ok(arn) => {
                    arns.push(arn);
                    if arns.len() >= cap {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        let mut out: Vec<(i64, EcsTask)> = Vec::new();
        for batch in arns.chunks(100) {
            if let Ok(resp) = client
                .describe_tasks()
                .cluster(cluster_arn)
                .set_tasks(Some(batch.to_vec()))
                .include(TaskField::Tags)
                .send()
                .await
            {
                for task in resp.tasks() {
                    let epoch = task.stopped_at().map(|d| d.secs()).unwrap_or(0);
                    out.push((epoch, EcsTask::from_sdk(task)));
                }
            }
        }
        if stopped {
            out.sort_by(|a, b| b.0.cmp(&a.0));
        }
        out.into_iter().map(|(_, t)| t).collect()
    }

    let mut tasks = list_and_describe(&client, &cluster_arn, &service_name, false, 200).await;
    let stopped = list_and_describe(&client, &cluster_arn, &service_name, true, MAX_STOPPED).await;
    tasks.extend(stopped);
    Ok(tasks)
}

/// Resolve the CloudWatch Logs group + stream names for one task's `awslogs`
/// containers, for the live tail. Tails the **first** log group found (most
/// tasks share one) with the stream names of every container on that group:
/// `{prefix}/{container}/{task_id}`. Returns the group + its streams.
pub async fn resolve_ecs_task_log_streams(
    ecs_client: EcsClient,
    task_def: String,
    task_id: String,
    container_names: Vec<String>,
) -> Result<(String, Vec<String>)> {
    let resp = ecs_client
        .describe_task_definition()
        .task_definition(&task_def)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
    let td = resp
        .task_definition()
        .ok_or_else(|| crate::error::Error::ResourceNotFound(task_def.clone()))?;

    let mut group: Option<String> = None;
    let mut streams: Vec<String> = Vec::new();

    for cd in td.container_definitions() {
        let cname = cd.name().unwrap_or_default().to_string();
        if !container_names.is_empty() && !container_names.contains(&cname) {
            continue;
        }
        let lc = match cd.log_configuration() {
            Some(lc) if lc.log_driver().as_str() == "awslogs" => lc,
            _ => continue,
        };
        let opts = lc.options();
        let g = opts.and_then(|o| o.get("awslogs-group")).cloned();
        let p = opts.and_then(|o| o.get("awslogs-stream-prefix")).cloned();
        if let (Some(g), Some(p)) = (g, p) {
            // Only collect streams for the first group we lock onto.
            match &group {
                None => group = Some(g.clone()),
                Some(existing) if *existing != g => continue,
                _ => {}
            }
            streams.push(format!("{}/{}/{}", p, cname, task_id));
        }
    }

    match group {
        Some(g) => Ok((g, streams)),
        None => Err(crate::error::Error::AwsSdk(
            "No awslogs containers on this task".to_string(),
        )),
    }
}
