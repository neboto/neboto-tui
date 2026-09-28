//! AWS Batch (`@batch`): job queues, compute environments, jobs and job
//! definitions. Everything the "why is my job stuck in RUNNABLE / why did it
//! fail" question needs is on the Describe calls, so there are no lazy
//! sections — the panes cross-reference each other from the loaded rows
//! (a queue's jobs, a compute environment's queues and waiting jobs).

use crate::aws::client::AwsClients;
use crate::aws::resource::{shell_quote, Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::error::Result;
use crate::event::{Event, LoadProgress};
use async_trait::async_trait;
use aws_sdk_batch::Client as BatchClient;
use futures::stream::{self, StreamExt};
use std::any::Any;
use std::collections::HashMap;
use tokio::sync::mpsc;

/// Per-queue, per-status cap on `ListJobs`. SUCCEEDED can hold thousands of
/// rows for the retention window; the stuck/failed questions are about the
/// recent ones.
pub const MAX_JOBS_PER_STATUS: usize = 200;

/// `ListJobs` without a status filter returns RUNNING only, so every status
/// is queried separately.
pub const JOB_STATUSES: [&str; 7] = [
    "SUBMITTED",
    "PENDING",
    "RUNNABLE",
    "STARTING",
    "RUNNING",
    "SUCCEEDED",
    "FAILED",
];

/// The default awslogs group Batch writes to when a job definition doesn't
/// name its own.
pub const DEFAULT_LOG_GROUP: &str = "/aws/batch/job";

pub struct BatchService {
    client: BatchClient,
}

impl BatchService {
    pub fn new(clients: &AwsClients) -> Self {
        Self {
            client: clients.batch_client(),
        }
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// `2026-09-27 04:12 UTC` from epoch milliseconds.
pub fn fmt_ms(ms: i64) -> String {
    crate::aws::services::step_functions::fmt_epoch_secs(ms / 1000)
}

/// The last `/`-segment of an ARN (`…:job-queue/etl` → `etl`,
/// `…:job-definition/etl:3` → `etl:3`). A bare name passes through.
pub fn arn_tail(arn: &str) -> String {
    arn.rsplit('/').next().unwrap_or(arn).to_string()
}

#[async_trait]
impl AwsService for BatchService {
    fn service_type(&self) -> ServiceType {
        ServiceType::Batch
    }

    fn name(&self) -> &str {
        "Batch"
    }

    async fn list_resources(&self) -> Result<Vec<Box<dyn Resource>>> {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        self.list_resources_streaming(tx, ServiceType::Batch).await?;
        Ok(vec![])
    }

    async fn list_resources_streaming(
        &self,
        event_tx: mpsc::UnboundedSender<Event>,
        service_type: ServiceType,
    ) -> Result<()> {
        let warn = |msg: String| {
            let _ = event_tx.send(Event::ResourceLoadWarning {
                service: service_type,
                warning: msg,
            });
        };
        let mut total = 0usize;
        let mut send = |batch: Vec<Box<dyn Resource>>, status: Option<&str>| {
            total += batch.len();
            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                service: service_type,
                resources: batch,
                progress: LoadProgress {
                    loaded_count: total,
                    total_count: None,
                    status_message: status.map(str::to_string),
                },
            });
        };

        // ── Job queues ────────────────────────────────────────────────────────
        let mut queues: Vec<BatchJobQueue> = Vec::new();
        let mut queues_failed = None;
        let mut pages = self.client.describe_job_queues().into_paginator().send();
        while let Some(page) = pages.next().await {
            match page {
                Ok(p) => queues.extend(p.job_queues().iter().map(BatchJobQueue::from_sdk)),
                Err(e) => {
                    queues_failed = Some(crate::error::sdk_error_message(&e));
                    break;
                }
            }
        }
        // Scheduling order: highest priority first.
        queues.sort_by_key(|q| std::cmp::Reverse(q.priority));
        let queue_arns: Vec<(String, String)> =
            queues.iter().map(|q| (q.arn.clone(), q.name.clone())).collect();
        send(
            queues.into_iter().map(|q| Box::new(q) as Box<dyn Resource>).collect(),
            Some("Loading compute environments…"),
        );

        // ── Compute environments ──────────────────────────────────────────────
        let mut ces: Vec<Box<dyn Resource>> = Vec::new();
        let mut ces_failed = None;
        let mut pages = self.client.describe_compute_environments().into_paginator().send();
        while let Some(page) = pages.next().await {
            match page {
                Ok(p) => ces.extend(
                    p.compute_environments()
                        .iter()
                        .map(|c| Box::new(BatchComputeEnv::from_sdk(c)) as Box<dyn Resource>),
                ),
                Err(e) => {
                    ces_failed = Some(crate::error::sdk_error_message(&e));
                    break;
                }
            }
        }
        send(ces, Some("Loading job definitions…"));

        if let (Some(q), Some(_)) = (&queues_failed, &ces_failed) {
            let _ = event_tx.send(Event::ResourceLoadError {
                service: service_type,
                error: format!("Batch unavailable: {q}"),
            });
            return Ok(());
        }
        if let Some(q) = queues_failed {
            warn(format!("Batch job queues: {q}"));
        }
        if let Some(c) = ces_failed {
            warn(format!("Batch compute environments: {c}"));
        }

        // ── Job definitions (ACTIVE revisions) ────────────────────────────────
        let mut defs: Vec<BatchJobDefinition> = Vec::new();
        let mut pages = self
            .client
            .describe_job_definitions()
            .status("ACTIVE")
            .into_paginator()
            .send();
        while let Some(page) = pages.next().await {
            match page {
                Ok(p) => defs.extend(p.job_definitions().iter().map(BatchJobDefinition::from_sdk)),
                Err(e) => {
                    warn(format!("Batch job definitions: {}", crate::error::sdk_error_message(&e)));
                    break;
                }
            }
        }
        defs.sort_by(|a, b| a.def_name.cmp(&b.def_name).then(b.revision.cmp(&a.revision)));
        send(
            defs.into_iter().map(|d| Box::new(d) as Box<dyn Resource>).collect(),
            Some("Loading jobs…"),
        );

        // ── Jobs: ListJobs per (queue, status), then DescribeJobs ×100 ────────
        // Owned work items + a cloned client per future: borrowed iterators
        // here trip async-trait's higher-ranked Send check.
        let work: Vec<(BatchClient, String, String, String)> = queue_arns
            .iter()
            .flat_map(|(arn, name)| {
                JOB_STATUSES
                    .iter()
                    .map(|s| (self.client.clone(), arn.clone(), name.clone(), s.to_string()))
                    .collect::<Vec<_>>()
            })
            .collect();
        let listings: Vec<(String, String, std::result::Result<(Vec<String>, bool), String>)> =
            stream::iter(work)
            .map(|(client, arn, name, status)| async move {
                let mut ids = Vec::new();
                let mut capped = false;
                let mut pages = client
                    .list_jobs()
                    .job_queue(arn)
                    .job_status(aws_sdk_batch::types::JobStatus::from(status.as_str()))
                    .into_paginator()
                    .send();
                while let Some(page) = pages.next().await {
                    match page {
                        Ok(p) => {
                            ids.extend(p.job_summary_list().iter().filter_map(|j| j.job_id().map(str::to_string)));
                            if ids.len() >= MAX_JOBS_PER_STATUS {
                                ids.truncate(MAX_JOBS_PER_STATUS);
                                capped = true;
                                break;
                            }
                        }
                        Err(e) => return (name, status, Err(crate::error::sdk_error_message(&e))),
                    }
                }
                (name, status, Ok((ids, capped)))
            })
            .buffer_unordered(6)
            .collect()
            .await;

        let mut ids: Vec<String> = Vec::new();
        let mut capped: Vec<String> = Vec::new();
        let mut list_errors: Vec<String> = Vec::new();
        for (queue, status, res) in listings {
            match res {
                Ok((mut got, was_capped)) => {
                    if was_capped {
                        capped.push(format!("{queue} {}", status.to_lowercase()));
                    }
                    ids.append(&mut got);
                }
                Err(e) => list_errors.push(format!("{queue}: {e}")),
            }
        }
        if let Some(first) = list_errors.first() {
            warn(format!(
                "Batch ListJobs failed for {} queue/status pair(s) — {first}",
                list_errors.len()
            ));
        }
        if !capped.is_empty() {
            capped.sort();
            warn(format!(
                "Batch: job list capped at {MAX_JOBS_PER_STATUS} per queue and status ({})",
                capped.join(", ")
            ));
        }
        ids.sort();
        ids.dedup();

        let chunks: Vec<(BatchClient, Vec<String>)> =
            ids.chunks(100).map(|c| (self.client.clone(), c.to_vec())).collect();
        let described: Vec<std::result::Result<Vec<BatchJob>, String>> = stream::iter(chunks)
            .map(|(client, chunk)| async move {
                client
                    .describe_jobs()
                    .set_jobs(Some(chunk))
                    .send()
                    .await
                    .map(|r| r.jobs().iter().map(BatchJob::from_sdk).collect())
                    .map_err(|e| crate::error::sdk_error_message(&e))
            })
            .buffer_unordered(4)
            .collect()
            .await;
        let mut jobs: Vec<BatchJob> = Vec::new();
        for r in described {
            match r {
                Ok(mut j) => jobs.append(&mut j),
                Err(e) => warn(format!("Batch DescribeJobs: {e}")),
            }
        }
        // Newest first — the batches finish in completion order, which says
        // nothing.
        jobs.sort_by_key(|j| std::cmp::Reverse(j.created_ms));
        send(
            jobs.into_iter().map(|j| Box::new(j) as Box<dyn Resource>).collect(),
            None,
        );

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

fn tags_of(t: Option<&HashMap<String, String>>) -> HashMap<String, String> {
    t.cloned().unwrap_or_default()
}

// ── Job queue ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct BatchJobQueue {
    pub name: String,
    pub arn: String,
    /// ENABLED / DISABLED.
    pub state: String,
    /// CREATING / UPDATING / DELETING / DELETED / VALID / INVALID.
    pub status: String,
    pub status_reason: Option<String>,
    pub priority: i32,
    /// `(order, compute environment name)`, in the order the scheduler
    /// tries them.
    pub ce_order: Vec<(i32, String)>,
    /// Service environments (SageMaker training queues) instead of CEs.
    pub service_envs: Vec<(i32, String)>,
    pub scheduling_policy: Option<String>,
    pub queue_type: Option<String>,
    /// `(state, after, action, reason)` — what Batch does with a job stuck
    /// in `state` too long.
    pub time_limit_actions: Vec<(String, i32, String, String)>,
    pub tags: HashMap<String, String>,
}

impl BatchJobQueue {
    pub fn from_sdk(q: &aws_sdk_batch::types::JobQueueDetail) -> Self {
        let mut ce_order: Vec<(i32, String)> = q
            .compute_environment_order()
            .iter()
            .map(|o| (o.order().unwrap_or(0), arn_tail(o.compute_environment().unwrap_or_default())))
            .collect();
        ce_order.sort();
        let mut service_envs: Vec<(i32, String)> = q
            .service_environment_order()
            .iter()
            .map(|o| (o.order().unwrap_or(0), arn_tail(o.service_environment().unwrap_or_default())))
            .collect();
        service_envs.sort();
        BatchJobQueue {
            name: q.job_queue_name().unwrap_or_default().to_string(),
            arn: q.job_queue_arn().unwrap_or_default().to_string(),
            state: q.state().map(|s| s.as_str().to_string()).unwrap_or_default(),
            status: q.status().map(|s| s.as_str().to_string()).unwrap_or_default(),
            status_reason: q.status_reason().filter(|s| !s.is_empty()).map(str::to_string),
            priority: q.priority().unwrap_or(0),
            ce_order,
            service_envs,
            scheduling_policy: q.scheduling_policy_arn().map(str::to_string),
            queue_type: q.job_queue_type().map(|t| t.as_str().to_string()),
            time_limit_actions: q
                .job_state_time_limit_actions()
                .iter()
                .map(|a| {
                    (
                        a.state().map(|s| s.as_str().to_string()).unwrap_or_default(),
                        a.max_time_seconds().unwrap_or(0),
                        a.action().map(|s| s.as_str().to_string()).unwrap_or_default(),
                        a.reason().unwrap_or_default().to_string(),
                    )
                })
                .collect(),
            tags: tags_of(q.tags()),
        }
    }

    pub fn uses_ce(&self, ce: &str) -> bool {
        self.ce_order.iter().any(|(_, n)| n == ce)
    }
}

crate::sections! {
    pub enum BatchQueueDetailSection,
    pub static BATCH_QUEUE_SECTIONS = [
        Overview "Overview",
        ComputeEnvironments "Compute Environments",
        Jobs "Jobs",
        Tags "Tags",
    ]
}

impl Resource for BatchJobQueue {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&BATCH_QUEUE_SECTIONS)
    }

    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws batch describe-job-queues --job-queues {}",
            shell_quote(&self.name)
        ))
    }

    fn cli_actions(&self) -> Vec<crate::aws::cli_actions::CliAction> {
        use crate::aws::cli_actions::{CliAction, CliTier};
        let q = shell_quote(&self.name);
        vec![
            CliAction::new(
                CliTier::Inspect,
                "list RUNNABLE jobs",
                format!("aws batch list-jobs --job-queue {q} --job-status RUNNABLE"),
            ),
            CliAction::new(
                CliTier::Inspect,
                "list FAILED jobs",
                format!("aws batch list-jobs --job-queue {q} --job-status FAILED"),
            ),
        ]
    }

    fn id(&self) -> &str {
        &self.name
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn resource_type(&self) -> &str {
        "Batch Job Queue"
    }

    fn state(&self) -> ResourceState {
        if self.state == "DISABLED" {
            return ResourceState::Stopped;
        }
        match self.status.as_str() {
            "VALID" => ResourceState::Available,
            "INVALID" => ResourceState::Unavailable,
            "CREATING" | "UPDATING" => ResourceState::Pending,
            "DELETING" | "DELETED" => ResourceState::Deleting,
            s => ResourceState::Unknown(s.to_string()),
        }
    }

    fn state_label(&self) -> String {
        if self.state == "DISABLED" {
            "disabled".to_string()
        } else {
            crate::aws::resource::native_state_label(&self.status, || self.state())
        }
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        let ces: Vec<&str> = self.ce_order.iter().map(|(_, n)| n.as_str()).collect();
        format!("{} {} {} batch job queue", self.name, self.status, ces.join(" "))
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Queue".to_string(), self.name.clone()),
            ("Status".to_string(), self.state_label()),
            ("Priority".to_string(), self.priority.to_string()),
            ("ARN".to_string(), self.arn.clone()),
        ]
    }

    fn references(&self) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = self
            .ce_order
            .iter()
            .map(|(_, n)| ("Compute Environment".to_string(), n.clone()))
            .collect();
        if let Some(p) = &self.scheduling_policy {
            out.push(("Scheduling Policy".to_string(), p.clone()));
        }
        out
    }

    fn trail_lookup_keys(&self) -> Vec<String> {
        vec![self.name.clone(), self.arn.clone()]
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{region}.console.aws.amazon.com/batch/home?region={region}#queues/detail/{}",
            self.arn
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── Compute environment ───────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct BatchComputeEnv {
    pub name: String,
    pub arn: String,
    /// MANAGED / UNMANAGED.
    pub ce_type: String,
    /// EC2 / SPOT / FARGATE / FARGATE_SPOT (managed only).
    pub provisioning: Option<String>,
    /// ENABLED / DISABLED.
    pub state: String,
    /// CREATING / UPDATING / DELETING / DELETED / VALID / INVALID.
    pub status: String,
    pub status_reason: Option<String>,
    /// ECS / EKS.
    pub orchestration: Option<String>,
    pub min_vcpus: Option<i32>,
    pub desired_vcpus: Option<i32>,
    pub max_vcpus: Option<i32>,
    pub unmanaged_vcpus: Option<i32>,
    pub instance_types: Vec<String>,
    pub allocation_strategy: Option<String>,
    pub image_id: Option<String>,
    pub launch_template: Option<String>,
    pub instance_role: Option<String>,
    pub spot_fleet_role: Option<String>,
    pub bid_percentage: Option<i32>,
    pub subnets: Vec<String>,
    pub security_groups: Vec<String>,
    pub service_role: Option<String>,
    pub ecs_cluster_arn: Option<String>,
    pub eks_cluster_arn: Option<String>,
    pub eks_namespace: Option<String>,
    pub tags: HashMap<String, String>,
}

impl BatchComputeEnv {
    // `image_id` is deprecated for ec2Configuration overrides but still
    // set on environments created that way.
    #[allow(deprecated)]
    pub fn from_sdk(c: &aws_sdk_batch::types::ComputeEnvironmentDetail) -> Self {
        let cr = c.compute_resources();
        let lt = cr.and_then(|r| r.launch_template()).map(|t| {
            let id = t
                .launch_template_name()
                .or(t.launch_template_id())
                .unwrap_or_default();
            match t.version() {
                Some(v) => format!("{id} (version {v})"),
                None => id.to_string(),
            }
        });
        BatchComputeEnv {
            name: c.compute_environment_name().unwrap_or_default().to_string(),
            arn: c.compute_environment_arn().unwrap_or_default().to_string(),
            ce_type: c.r#type().map(|t| t.as_str().to_string()).unwrap_or_default(),
            provisioning: cr.and_then(|r| r.r#type()).map(|t| t.as_str().to_string()),
            state: c.state().map(|s| s.as_str().to_string()).unwrap_or_default(),
            status: c.status().map(|s| s.as_str().to_string()).unwrap_or_default(),
            status_reason: c.status_reason().filter(|s| !s.is_empty()).map(str::to_string),
            orchestration: c.container_orchestration_type().map(|t| t.as_str().to_string()),
            min_vcpus: cr.and_then(|r| r.minv_cpus()),
            desired_vcpus: cr.and_then(|r| r.desiredv_cpus()),
            max_vcpus: cr.and_then(|r| r.maxv_cpus()),
            unmanaged_vcpus: c.unmanagedv_cpus(),
            instance_types: cr.map(|r| r.instance_types().to_vec()).unwrap_or_default(),
            allocation_strategy: cr
                .and_then(|r| r.allocation_strategy())
                .map(|a| a.as_str().to_string()),
            image_id: cr.and_then(|r| r.image_id()).map(str::to_string),
            launch_template: lt,
            instance_role: cr.and_then(|r| r.instance_role()).map(str::to_string),
            spot_fleet_role: cr.and_then(|r| r.spot_iam_fleet_role()).map(str::to_string),
            bid_percentage: cr.and_then(|r| r.bid_percentage()),
            subnets: cr.map(|r| r.subnets().to_vec()).unwrap_or_default(),
            security_groups: cr.map(|r| r.security_group_ids().to_vec()).unwrap_or_default(),
            service_role: c.service_role().map(str::to_string),
            ecs_cluster_arn: c.ecs_cluster_arn().map(str::to_string),
            eks_cluster_arn: c.eks_configuration().and_then(|e| e.eks_cluster_arn()).map(str::to_string),
            eks_namespace: c
                .eks_configuration()
                .and_then(|e| e.kubernetes_namespace())
                .map(str::to_string),
            tags: tags_of(c.tags()),
        }
    }

    pub fn is_managed(&self) -> bool {
        self.ce_type == "MANAGED"
    }

    pub fn is_fargate(&self) -> bool {
        self.provisioning.as_deref().is_some_and(|p| p.starts_with("FARGATE"))
    }

    /// Why this environment can't take work right now, if it can't: the
    /// first of disabled / INVALID / (managed) desired vCPU 0. `None` for
    /// an environment that can place jobs.
    pub fn blocker(&self) -> Option<&'static str> {
        if self.state == "DISABLED" {
            Some("disabled")
        } else if self.status == "INVALID" {
            Some("INVALID")
        } else if self.is_managed() && self.desired_vcpus == Some(0) {
            Some("desired vCPU 0")
        } else {
            None
        }
    }
}

crate::sections! {
    pub enum BatchCeDetailSection,
    pub static BATCH_CE_SECTIONS = [
        Overview "Overview",
        Compute "Compute",
        Network "Network",
        Queues "Queues",
        Tags "Tags",
    ]
}

impl Resource for BatchComputeEnv {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&BATCH_CE_SECTIONS)
    }

    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws batch describe-compute-environments --compute-environments {}",
            shell_quote(&self.name)
        ))
    }

    fn id(&self) -> &str {
        &self.name
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn resource_type(&self) -> &str {
        "Batch Compute Environment"
    }

    fn state(&self) -> ResourceState {
        if self.state == "DISABLED" {
            return ResourceState::Stopped;
        }
        match self.status.as_str() {
            "VALID" => ResourceState::Available,
            "INVALID" => ResourceState::Unavailable,
            "CREATING" | "UPDATING" => ResourceState::Pending,
            "DELETING" | "DELETED" => ResourceState::Deleting,
            s => ResourceState::Unknown(s.to_string()),
        }
    }

    fn state_label(&self) -> String {
        if self.state == "DISABLED" {
            "disabled".to_string()
        } else {
            crate::aws::resource::native_state_label(&self.status, || self.state())
        }
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!(
            "{} {} {} {} {} batch compute environment",
            self.name,
            self.ce_type,
            self.provisioning.as_deref().unwrap_or(""),
            self.status,
            self.instance_types.join(" ")
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Compute Environment".to_string(), self.name.clone()),
            ("Status".to_string(), self.state_label()),
            ("ARN".to_string(), self.arn.clone()),
        ]
    }

    fn security_group_ids(&self) -> Vec<String> {
        self.security_groups.clone()
    }

    fn references(&self) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = Vec::new();
        for s in &self.subnets {
            out.push(("Subnet".to_string(), s.clone()));
        }
        for g in &self.security_groups {
            out.push(("Security Group".to_string(), g.clone()));
        }
        for (label, v) in [
            ("Instance Role", &self.instance_role),
            ("Service Role", &self.service_role),
            ("ECS Cluster", &self.ecs_cluster_arn),
            ("EKS Cluster", &self.eks_cluster_arn),
        ] {
            if let Some(v) = v {
                out.push((label.to_string(), v.clone()));
            }
        }
        out
    }

    fn trail_lookup_keys(&self) -> Vec<String> {
        vec![self.name.clone(), self.arn.clone()]
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{region}.console.aws.amazon.com/batch/home?region={region}#compute-environments/detail/{}",
            self.arn
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── Job ───────────────────────────────────────────────────────────────────────

/// One container of a job — the single `container` of a classic job, or one
/// of an ECS-properties (multi-container) job's containers.
#[derive(Debug, Clone, Default)]
pub struct BatchContainer {
    pub name: Option<String>,
    pub image: Option<String>,
    pub vcpus: Option<String>,
    pub memory_mib: Option<String>,
    pub gpus: Option<String>,
    pub command: Vec<String>,
    pub exit_code: Option<i32>,
    pub reason: Option<String>,
    pub log_stream: Option<String>,
    /// `awslogs-group` from the log configuration, else the Batch default.
    pub log_group: Option<String>,
    pub log_driver: Option<String>,
    pub job_role: Option<String>,
    pub execution_role: Option<String>,
    pub instance_type: Option<String>,
    pub task_arn: Option<String>,
}

/// `(vcpus, memory MiB, gpus)` from resource requirements, falling back to
/// the deprecated top-level `vcpus` / `memory` fields.
fn resources_of(
    reqs: &[aws_sdk_batch::types::ResourceRequirement],
    vcpus: Option<i32>,
    memory: Option<i32>,
) -> (Option<String>, Option<String>, Option<String>) {
    let get = |t: &str| {
        reqs.iter()
            .find(|r| r.r#type().map(|x| x.as_str()) == Some(t))
            .and_then(|r| r.value())
            .map(str::to_string)
    };
    (
        get("VCPU").or(vcpus.map(|v| v.to_string())),
        get("MEMORY").or(memory.map(|m| m.to_string())),
        get("GPU"),
    )
}

/// `(driver, group)` — the group is the `awslogs-group` option when the
/// driver is awslogs (or unset, which means awslogs), else `None`.
fn log_target(cfg: Option<&aws_sdk_batch::types::LogConfiguration>) -> (Option<String>, Option<String>) {
    let driver = cfg.and_then(|c| c.log_driver()).map(|d| d.as_str().to_string());
    let awslogs = driver.as_deref().is_none_or(|d| d == "awslogs");
    let group = if awslogs {
        Some(
            cfg.and_then(|c| c.options())
                .and_then(|o| o.get("awslogs-group"))
                .cloned()
                .unwrap_or_else(|| DEFAULT_LOG_GROUP.to_string()),
        )
    } else {
        None
    };
    (driver, group)
}

impl BatchContainer {
    // Older jobs set top-level vcpus / memory instead of resourceRequirements.
    #[allow(deprecated)]
    fn from_container(c: &aws_sdk_batch::types::ContainerDetail) -> Self {
        let (vcpus, memory_mib, gpus) = resources_of(c.resource_requirements(), c.vcpus(), c.memory());
        let (log_driver, log_group) = log_target(c.log_configuration());
        BatchContainer {
            name: None,
            image: c.image().map(str::to_string),
            vcpus,
            memory_mib,
            gpus,
            command: c.command().to_vec(),
            exit_code: c.exit_code(),
            reason: c.reason().map(str::to_string),
            log_stream: c.log_stream_name().map(str::to_string),
            log_group,
            log_driver,
            job_role: c.job_role_arn().map(str::to_string),
            execution_role: c.execution_role_arn().map(str::to_string),
            instance_type: c.instance_type().map(str::to_string),
            task_arn: c.task_arn().map(str::to_string),
        }
    }

    fn from_task_container(
        c: &aws_sdk_batch::types::TaskContainerDetails,
        task: &aws_sdk_batch::types::EcsTaskDetails,
    ) -> Self {
        let (vcpus, memory_mib, gpus) = resources_of(c.resource_requirements(), None, None);
        let (log_driver, log_group) = log_target(c.log_configuration());
        BatchContainer {
            name: c.name().map(str::to_string),
            image: c.image().map(str::to_string),
            vcpus,
            memory_mib,
            gpus,
            command: c.command().to_vec(),
            exit_code: c.exit_code(),
            reason: c.reason().map(str::to_string),
            log_stream: c.log_stream_name().map(str::to_string),
            log_group,
            log_driver,
            job_role: task.task_role_arn().map(str::to_string),
            execution_role: task.execution_role_arn().map(str::to_string),
            instance_type: None,
            task_arn: task.task_arn().map(str::to_string),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct BatchAttempt {
    pub started_ms: Option<i64>,
    pub stopped_ms: Option<i64>,
    pub status_reason: Option<String>,
    pub exit_code: Option<i32>,
    pub reason: Option<String>,
    pub log_stream: Option<String>,
}

#[derive(Debug, Clone)]
pub struct BatchJob {
    pub job_id: String,
    pub arn: String,
    pub name: String,
    pub queue: String,
    pub queue_arn: String,
    pub status: String,
    pub status_reason: Option<String>,
    /// Job definition ARN.
    pub job_definition: String,
    pub created_ms: Option<i64>,
    pub started_ms: Option<i64>,
    pub stopped_ms: Option<i64>,
    pub attempts: Vec<BatchAttempt>,
    pub retry_attempts: Option<i32>,
    pub timeout_secs: Option<i32>,
    pub containers: Vec<BatchContainer>,
    /// The array size on a parent array job.
    pub array_size: Option<i32>,
    /// The index on an array child.
    pub array_index: Option<i32>,
    /// Child status → count, on a parent array job.
    pub array_summary: Vec<(String, i32)>,
    /// `(job id, dependency type)`.
    pub depends_on: Vec<(String, String)>,
    pub platform: Vec<String>,
    /// EKS-orchestrated: the cluster the pod ran on.
    pub eks_cluster_arn: Option<String>,
    pub eks_pod: Option<String>,
    pub multinode: bool,
    pub share_identifier: Option<String>,
    pub scheduling_priority: Option<i32>,
    pub is_cancelled: bool,
    pub is_terminated: bool,
    pub parameters: Vec<(String, String)>,
    pub tags: HashMap<String, String>,
}

impl BatchJob {
    pub fn from_sdk(j: &aws_sdk_batch::types::JobDetail) -> Self {
        let mut containers: Vec<BatchContainer> = Vec::new();
        if let Some(c) = j.container() {
            containers.push(BatchContainer::from_container(c));
        }
        if let Some(ecs) = j.ecs_properties() {
            for task in ecs.task_properties() {
                for c in task.containers() {
                    containers.push(BatchContainer::from_task_container(c, task));
                }
            }
        }
        let attempts = j
            .attempts()
            .iter()
            .map(|a| {
                // Classic jobs carry `container`; multi-container ones put the
                // containers under `taskProperties` — the first one stands in.
                let (exit_code, reason, log_stream) = match a.container() {
                    Some(c) => (
                        c.exit_code(),
                        c.reason().map(str::to_string),
                        c.log_stream_name().map(str::to_string),
                    ),
                    None => a
                        .task_properties()
                        .iter()
                        .flat_map(|t| t.containers())
                        .next()
                        .map(|c| {
                            (
                                c.exit_code(),
                                c.reason().map(str::to_string),
                                c.log_stream_name().map(str::to_string),
                            )
                        })
                        .unwrap_or_default(),
                };
                BatchAttempt {
                    started_ms: a.started_at(),
                    stopped_ms: a.stopped_at(),
                    status_reason: a.status_reason().map(str::to_string),
                    exit_code,
                    reason,
                    log_stream,
                }
            })
            .collect();
        let ap = j.array_properties();
        let mut array_summary: Vec<(String, i32)> = ap
            .and_then(|a| a.status_summary())
            .map(|m| m.iter().map(|(k, v)| (k.clone(), *v)).collect())
            .unwrap_or_default();
        // Lifecycle order, not hash order.
        array_summary.sort_by_key(|(s, _)| JOB_STATUSES.iter().position(|x| x == s).unwrap_or(99));
        let mut parameters: Vec<(String, String)> = j
            .parameters()
            .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default();
        parameters.sort();
        let eks = j.eks_attempts().last();
        BatchJob {
            job_id: j.job_id().unwrap_or_default().to_string(),
            arn: j.job_arn().unwrap_or_default().to_string(),
            name: j.job_name().unwrap_or_default().to_string(),
            queue: arn_tail(j.job_queue().unwrap_or_default()),
            queue_arn: j.job_queue().unwrap_or_default().to_string(),
            status: j.status().map(|s| s.as_str().to_string()).unwrap_or_default(),
            status_reason: j.status_reason().filter(|s| !s.is_empty()).map(str::to_string),
            job_definition: j.job_definition().unwrap_or_default().to_string(),
            created_ms: j.created_at(),
            started_ms: j.started_at(),
            stopped_ms: j.stopped_at(),
            attempts,
            retry_attempts: j.retry_strategy().and_then(|r| r.attempts()),
            timeout_secs: j.timeout().and_then(|t| t.attempt_duration_seconds()),
            containers,
            array_size: ap.and_then(|a| a.size()),
            array_index: ap.and_then(|a| a.index()),
            array_summary,
            depends_on: j
                .depends_on()
                .iter()
                .map(|d| {
                    (
                        d.job_id().unwrap_or_default().to_string(),
                        d.r#type().map(|t| t.as_str().to_string()).unwrap_or_default(),
                    )
                })
                .collect(),
            platform: j.platform_capabilities().iter().map(|p| p.as_str().to_string()).collect(),
            eks_cluster_arn: eks.and_then(|e| e.eks_cluster_arn()).map(str::to_string),
            eks_pod: eks.and_then(|e| e.pod_name()).map(str::to_string),
            multinode: j.node_properties().is_some() || j.node_details().is_some(),
            share_identifier: j.share_identifier().map(str::to_string),
            scheduling_priority: j.scheduling_priority(),
            is_cancelled: j.is_cancelled().unwrap_or(false),
            is_terminated: j.is_terminated().unwrap_or(false),
            parameters,
            tags: tags_of(j.tags()),
        }
    }

    /// The job definition as `name:revision`.
    pub fn job_definition_label(&self) -> String {
        arn_tail(&self.job_definition)
    }

    /// `(log group, log stream)` for `t` — the first container with a stream
    /// on an awslogs driver. `None` before the job starts, on a non-awslogs
    /// driver, and on EKS jobs (their logs live in the cluster).
    pub fn log_target(&self) -> Option<(String, String)> {
        self.containers.iter().find_map(|c| {
            Some((c.log_group.clone()?, c.log_stream.clone()?))
        })
    }

    pub fn is_active(&self) -> bool {
        matches!(
            self.status.as_str(),
            "SUBMITTED" | "PENDING" | "RUNNABLE" | "STARTING" | "RUNNING"
        )
    }

    /// Milliseconds this job has spent since creation (active jobs) — what
    /// "stuck in RUNNABLE for how long" reads off.
    pub fn age_ms(&self) -> Option<i64> {
        self.created_ms.map(|c| (now_ms() - c).max(0))
    }

    /// `started → stopped` (or `→ now` while running).
    pub fn run_ms(&self) -> Option<i64> {
        let start = self.started_ms?;
        let end = self.stopped_ms.unwrap_or_else(now_ms);
        Some((end - start).max(0))
    }
}

crate::sections! {
    pub enum BatchJobDetailSection,
    pub static BATCH_JOB_SECTIONS = [
        Overview "Overview",
        Container "Container",
        Attempts "Attempts",
        Dependencies "Array & Deps",
        Tags "Tags",
    ]
}

impl Resource for BatchJob {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&BATCH_JOB_SECTIONS)
    }

    fn cli_command(&self) -> Option<String> {
        Some(format!("aws batch describe-jobs --jobs {}", shell_quote(&self.job_id)))
    }

    fn cli_actions(&self) -> Vec<crate::aws::cli_actions::CliAction> {
        use crate::aws::cli_actions::{CliAction, CliTier};
        let mut out = vec![CliAction::batchable(
            CliTier::Inspect,
            "describe-jobs",
            "aws batch describe-jobs --jobs",
            &self.job_id,
            "",
        )];
        if let Some((group, stream)) = self.log_target() {
            out.push(CliAction::new(
                CliTier::Inspect,
                "tail logs",
                format!(
                    "aws logs tail {} --log-stream-names {} --since 1h",
                    shell_quote(&group),
                    shell_quote(&stream)
                ),
            ));
        }
        if self.array_size.is_some() {
            out.push(CliAction::new(
                CliTier::Inspect,
                "list array children",
                format!("aws batch list-jobs --array-job-id {}", shell_quote(&self.job_id)),
            ));
        }
        out
    }

    fn id(&self) -> &str {
        &self.job_id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn resource_type(&self) -> &str {
        "Batch Job"
    }

    fn state(&self) -> ResourceState {
        match self.status.as_str() {
            "RUNNING" => ResourceState::Running,
            "SUCCEEDED" => ResourceState::Available,
            "FAILED" => ResourceState::Unavailable,
            "SUBMITTED" | "PENDING" | "RUNNABLE" | "STARTING" => ResourceState::Pending,
            s => ResourceState::Unknown(s.to_string()),
        }
    }

    fn state_label(&self) -> String {
        crate::aws::resource::native_state_label(&self.status, || self.state())
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!(
            "{} {} {} {} {} {} batch job",
            self.name,
            self.job_id,
            self.queue,
            self.job_definition_label(),
            self.status,
            self.status_reason.as_deref().unwrap_or("")
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Job".to_string(), self.name.clone()),
            ("Job ID".to_string(), self.job_id.clone()),
            ("Status".to_string(), self.status.clone()),
            ("Queue".to_string(), self.queue.clone()),
        ]
    }

    fn references(&self) -> Vec<(String, String)> {
        let mut out = vec![
            ("Job Queue".to_string(), self.queue.clone()),
            ("Job Definition".to_string(), self.job_definition_label()),
        ];
        for c in &self.containers {
            if let Some(r) = &c.job_role {
                out.push(("Job Role".to_string(), r.clone()));
            }
            if let Some(r) = &c.execution_role {
                out.push(("Execution Role".to_string(), r.clone()));
            }
        }
        out
    }

    fn trail_lookup_keys(&self) -> Vec<String> {
        vec![self.job_id.clone(), self.arn.clone()]
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{region}.console.aws.amazon.com/batch/home?region={region}#jobs/detail/{}",
            self.job_id
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── Job definition ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct BatchJobDefinition {
    /// `name:revision` — what jobs reference and what the row shows.
    pub label: String,
    pub def_name: String,
    pub revision: i32,
    pub arn: String,
    pub status: String,
    /// container / multinode.
    pub def_type: String,
    pub orchestration: Option<String>,
    pub platform: Vec<String>,
    pub containers: Vec<BatchContainer>,
    pub retry_attempts: Option<i32>,
    /// `action on-status/on-reason/on-exit-code` per rule.
    pub retry_rules: Vec<String>,
    pub timeout_secs: Option<i32>,
    pub scheduling_priority: Option<i32>,
    pub propagate_tags: bool,
    pub parameters: Vec<(String, String)>,
    pub tags: HashMap<String, String>,
}

impl BatchJobDefinition {
    #[allow(deprecated)]
    pub fn from_sdk(d: &aws_sdk_batch::types::JobDefinition) -> Self {
        let def_name = d.job_definition_name().unwrap_or_default().to_string();
        let revision = d.revision().unwrap_or(0);
        let mut containers = Vec::new();
        if let Some(c) = d.container_properties() {
            let (vcpus, memory_mib, gpus) = resources_of(c.resource_requirements(), c.vcpus(), c.memory());
            let (log_driver, log_group) = log_target(c.log_configuration());
            containers.push(BatchContainer {
                image: c.image().map(str::to_string),
                vcpus,
                memory_mib,
                gpus,
                command: c.command().to_vec(),
                log_group,
                log_driver,
                job_role: c.job_role_arn().map(str::to_string),
                execution_role: c.execution_role_arn().map(str::to_string),
                instance_type: c.instance_type().map(str::to_string),
                ..Default::default()
            });
        }
        if let Some(ecs) = d.ecs_properties() {
            for task in ecs.task_properties() {
                for c in task.containers() {
                    let (vcpus, memory_mib, gpus) = resources_of(c.resource_requirements(), None, None);
                    let (log_driver, log_group) = log_target(c.log_configuration());
                    containers.push(BatchContainer {
                        name: c.name().map(str::to_string),
                        image: c.image().map(str::to_string),
                        vcpus,
                        memory_mib,
                        gpus,
                        command: c.command().to_vec(),
                        log_group,
                        log_driver,
                        job_role: task.task_role_arn().map(str::to_string),
                        execution_role: task.execution_role_arn().map(str::to_string),
                        ..Default::default()
                    });
                }
            }
        }
        let retry = d.retry_strategy();
        let retry_rules = retry
            .map(|r| {
                r.evaluate_on_exit()
                    .iter()
                    .map(|e| {
                        let mut conds = Vec::new();
                        if let Some(s) = e.on_status_reason() {
                            conds.push(format!("status reason {s}"));
                        }
                        if let Some(s) = e.on_reason() {
                            conds.push(format!("reason {s}"));
                        }
                        if let Some(s) = e.on_exit_code() {
                            conds.push(format!("exit code {s}"));
                        }
                        let action = e.action().map(|a| a.as_str().to_string()).unwrap_or_default();
                        if conds.is_empty() {
                            action
                        } else {
                            format!("{action} on {}", conds.join(" and "))
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        let mut parameters: Vec<(String, String)> = d
            .parameters()
            .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default();
        parameters.sort();
        BatchJobDefinition {
            label: format!("{def_name}:{revision}"),
            def_name,
            revision,
            arn: d.job_definition_arn().unwrap_or_default().to_string(),
            status: d.status().unwrap_or_default().to_string(),
            def_type: d.r#type().unwrap_or_default().to_string(),
            orchestration: d.container_orchestration_type().map(|t| t.as_str().to_string()),
            platform: d.platform_capabilities().iter().map(|p| p.as_str().to_string()).collect(),
            containers,
            retry_attempts: retry.and_then(|r| r.attempts()),
            retry_rules,
            timeout_secs: d.timeout().and_then(|t| t.attempt_duration_seconds()),
            scheduling_priority: d.scheduling_priority(),
            propagate_tags: d.propagate_tags().unwrap_or(false),
            parameters,
            tags: tags_of(d.tags()),
        }
    }
}

crate::sections! {
    pub enum BatchJobDefDetailSection,
    pub static BATCH_JOBDEF_SECTIONS = [
        Overview "Overview",
        Container "Container",
        Tags "Tags",
    ]
}

impl Resource for BatchJobDefinition {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&BATCH_JOBDEF_SECTIONS)
    }

    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws batch describe-job-definitions --job-definitions {}",
            shell_quote(&self.label)
        ))
    }

    fn id(&self) -> &str {
        &self.label
    }

    fn name(&self) -> &str {
        &self.label
    }

    fn resource_type(&self) -> &str {
        "Batch Job Definition"
    }

    fn state(&self) -> ResourceState {
        ResourceState::stateless()
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        let images: Vec<&str> = self.containers.iter().filter_map(|c| c.image.as_deref()).collect();
        format!("{} {} {} batch job definition", self.label, self.def_type, images.join(" "))
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Job Definition".to_string(), self.label.clone()),
            ("Type".to_string(), self.def_type.clone()),
            ("ARN".to_string(), self.arn.clone()),
        ]
    }

    fn references(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for c in &self.containers {
            if let Some(r) = &c.job_role {
                out.push(("Job Role".to_string(), r.clone()));
            }
            if let Some(r) = &c.execution_role {
                out.push(("Execution Role".to_string(), r.clone()));
            }
            if let Some(i) = &c.image {
                out.push(("Image".to_string(), i.clone()));
            }
        }
        out
    }

    fn trail_lookup_keys(&self) -> Vec<String> {
        vec![self.def_name.clone(), self.arn.clone()]
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{region}.console.aws.amazon.com/batch/home?region={region}#job-definition/detail/{}",
            self.arn
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arn_tail_reduces_batch_arns() {
        assert_eq!(arn_tail("arn:aws:batch:us-east-1:1:job-queue/etl"), "etl");
        assert_eq!(arn_tail("arn:aws:batch:us-east-1:1:job-definition/etl:3"), "etl:3");
        assert_eq!(arn_tail("etl"), "etl");
    }

    #[test]
    fn log_target_defaults_to_the_batch_group_and_skips_other_drivers() {
        use aws_sdk_batch::types::{LogConfiguration, LogDriver};
        assert_eq!(log_target(None), (None, Some(DEFAULT_LOG_GROUP.to_string())));
        let custom = LogConfiguration::builder()
            .log_driver(LogDriver::Awslogs)
            .options("awslogs-group", "/my/jobs")
            .build();
        assert_eq!(log_target(Some(&custom)).1.as_deref(), Some("/my/jobs"));
        let splunk = LogConfiguration::builder().log_driver(LogDriver::Splunk).build();
        assert_eq!(log_target(Some(&splunk)).1, None);
    }
}
