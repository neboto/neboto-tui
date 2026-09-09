use crate::aws::client::AwsClients;
use crate::aws::resource::{native_state_label, Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::error::Result;
use crate::event::{Event, LoadProgress};
use async_trait::async_trait;
use aws_sdk_autoscaling::Client as AsgClient;
use std::any::Any;
use std::collections::HashMap;
use tokio::sync::mpsc;

pub struct AutoScalingService {
    client: AsgClient,
}

impl AutoScalingService {
    pub fn new(aws_clients: &AwsClients) -> Self {
        Self {
            client: aws_clients.asg_client(),
        }
    }
}

#[async_trait]
impl AwsService for AutoScalingService {
    fn service_type(&self) -> ServiceType {
        ServiceType::Asg
    }

    fn name(&self) -> &str {
        "Auto Scaling"
    }

    async fn list_resources(&self) -> Result<Vec<Box<dyn Resource>>> {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        self.list_resources_streaming(tx, ServiceType::Asg).await?;
        Ok(vec![])
    }

    async fn list_resources_streaming(
        &self,
        event_tx: mpsc::UnboundedSender<Event>,
        service_type: ServiceType,
    ) -> Result<()> {
        let mut total = 0usize;

        let mut paginator = self
            .client
            .describe_auto_scaling_groups()
            .into_paginator()
            .send();
        while let Some(result) = paginator.next().await {
            match result {
                Ok(page) => {
                    let batch: Vec<Box<dyn Resource>> = page
                        .auto_scaling_groups()
                        .iter()
                        .map(|g| Box::new(AsgGroup::from_sdk(g)) as Box<dyn Resource>)
                        .collect();
                    let count = batch.len();
                    if count == 0 {
                        continue;
                    }
                    total += count;
                    let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                        service: service_type,
                        resources: batch,
                        progress: LoadProgress {
                            loaded_count: total,
                            total_count: None,
                            status_message: Some("Loading Auto Scaling groups…".to_string()),
                        },
                    });
                }
                Err(e) => {
                    let _ = event_tx.send(Event::ResourceLoadError {
                        service: service_type,
                        error: format!("Failed to list Auto Scaling groups: {}", e),
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

// ── AsgGroup ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct AsgInstance {
    pub instance_id: String,
    pub instance_type: Option<String>,
    pub availability_zone: String,
    pub lifecycle_state: String,
    pub health_status: String,
}

#[derive(Debug, Clone)]
pub struct AsgGroup {
    pub name: String,
    /// Full ARN (contains a UUID, so it can't be rebuilt from the name) —
    /// needed by the Compute Optimizer lens.
    pub arn: String,
    pub min_size: i32,
    pub max_size: i32,
    pub desired_capacity: i32,
    pub default_cooldown: i32,
    pub health_check_type: String,
    pub health_check_grace_period: Option<i32>,
    pub launch_template: Option<String>,
    pub launch_configuration: Option<String>,
    pub mixed_instances_policy: bool,
    pub availability_zones: Vec<String>,
    pub vpc_zone_identifier: Option<String>,
    pub load_balancer_names: Vec<String>,
    pub target_group_count: usize,
    pub suspended_processes: Vec<String>,
    pub status: Option<String>,
    pub created_time: String,
    pub instances: Vec<AsgInstance>,
    pub tags: HashMap<String, String>,
}

impl AsgGroup {
    pub fn from_sdk(g: &aws_sdk_autoscaling::types::AutoScalingGroup) -> Self {
        let tags: HashMap<String, String> = g
            .tags()
            .iter()
            .filter_map(|t| Some((t.key()?.to_string(), t.value()?.to_string())))
            .collect();

        let launch_template = g.launch_template().map(|lt| {
            let name = lt.launch_template_name().unwrap_or("").to_string();
            match lt.version() {
                Some(v) => format!("{} (v{})", name, v),
                None => name,
            }
        });

        let instances: Vec<AsgInstance> = g
            .instances()
            .iter()
            .map(|i| AsgInstance {
                instance_id: i.instance_id().unwrap_or("").to_string(),
                instance_type: i.instance_type().map(|s| s.to_string()),
                availability_zone: i.availability_zone().unwrap_or("").to_string(),
                lifecycle_state: i
                    .lifecycle_state()
                    .map(|s| s.as_str().to_string())
                    .unwrap_or_default(),
                health_status: i.health_status().unwrap_or("").to_string(),
            })
            .collect();

        Self {
            arn: g.auto_scaling_group_arn().unwrap_or_default().to_string(),
            name: g.auto_scaling_group_name().unwrap_or("").to_string(),
            min_size: g.min_size().unwrap_or(0),
            max_size: g.max_size().unwrap_or(0),
            desired_capacity: g.desired_capacity().unwrap_or(0),
            default_cooldown: g.default_cooldown().unwrap_or(0),
            health_check_type: g.health_check_type().unwrap_or("").to_string(),
            health_check_grace_period: g.health_check_grace_period(),
            launch_template,
            launch_configuration: g.launch_configuration_name().map(|s| s.to_string()),
            mixed_instances_policy: g.mixed_instances_policy().is_some(),
            availability_zones: g.availability_zones().to_vec(),
            vpc_zone_identifier: g.vpc_zone_identifier().map(|s| s.to_string()),
            load_balancer_names: g.load_balancer_names().to_vec(),
            target_group_count: g.traffic_sources().len(),
            suspended_processes: g
                .suspended_processes()
                .iter()
                .filter_map(|p| p.process_name().map(|s| s.to_string()))
                .collect(),
            status: g.status().map(|s| s.to_string()),
            created_time: g
                .created_time()
                .map(|t| {
                    let s = t.to_string();
                    s.split('.').next().unwrap_or(&s).replace('T', " ")
                })
                .unwrap_or_default(),
            instances,
            tags,
        }
    }

    /// Count of instances currently in the InService lifecycle state.
    pub fn in_service_count(&self) -> usize {
        self.instances
            .iter()
            .filter(|i| i.lifecycle_state == "InService")
            .count()
    }
}

crate::sections! {
    pub enum AsgGroupDetailSection,
    pub static ASG_GROUP_SECTIONS = [
        Capacity "Capacity",
        Instances "Instances",
        Activities "Activities" => crate::app::App::trigger_asg_activities_load,
        Tags "Tags",
        Optimizer "Optimizer" => crate::app::App::trigger_optimizer_load,
    ]
}

impl Resource for AsgGroup {
    fn references(&self) -> Vec<(String, String)> {
        let mut v = Vec::new();
        let mut r = |l: &str, x: &str| v.push((l.to_string(), x.to_string()));
        if let Some(x) = &self.launch_template { r("Launch Template", x); }
        if let Some(x) = &self.launch_configuration { r("Launch Configuration", x); }
        if let Some(zones) = &self.vpc_zone_identifier {
            for x in zones.split(',') { r("Subnet", x.trim()); }
        }
        for x in &self.load_balancer_names { r("Load Balancer", x); }
        for i in &self.instances { r("Instance", &i.instance_id); }
        v
    }

    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&ASG_GROUP_SECTIONS)
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws autoscaling describe-auto-scaling-groups --auto-scaling-group-names {}",
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
        "Auto Scaling Group"
    }

    fn state(&self) -> ResourceState {
        // A group with no status string is steady-state; a status string means
        // it's being deleted/updated.
        match &self.status {
            Some(_) => ResourceState::Pending,
            None => ResourceState::Running,
        }
    }

    fn state_label(&self) -> String {
        // The console's Status column is blank for a steady group.
        match &self.status {
            Some(s) => native_state_label(s, || self.state()),
            None => "steady".to_string(),
        }
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!(
            "{} {} {}",
            self.name,
            self.launch_template.clone().unwrap_or_default(),
            self.availability_zones.join(" ")
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Name".to_string(), self.name.clone()),
            (
                "Capacity".to_string(),
                format!(
                    "{} desired ({} min / {} max)",
                    self.desired_capacity, self.min_size, self.max_size
                ),
            ),
            (
                "Instances".to_string(),
                format!("{} in service / {}", self.in_service_count(), self.instances.len()),
            ),
            ("Health Check".to_string(), self.health_check_type.clone()),
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
            "https://{}.console.aws.amazon.com/ec2/home?region={}#AutoScalingGroupDetails:id={};view=details",
            region, region, self.name
        ))
    }
}

// ── Scaling activities (lazy-loaded via describe_scaling_activities) ───────────

#[derive(Debug, Clone)]
pub struct ScalingActivity {
    pub start_time: String,
    pub status_code: String,
    pub description: String,
    pub cause: String,
    pub progress: Option<i32>,
    pub status_message: Option<String>,
}

pub async fn fetch_scaling_activities(
    client: AsgClient,
    group_name: String,
) -> Result<Vec<ScalingActivity>> {
    let resp = client
        .describe_scaling_activities()
        .auto_scaling_group_name(&group_name)
        .max_records(50)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;

    let activities: Vec<ScalingActivity> = resp
        .activities()
        .iter()
        .map(|a| ScalingActivity {
            start_time: a
                .start_time()
                .map(|t| {
                    let s = t.to_string();
                    s.split('.').next().unwrap_or(&s).replace('T', " ")
                })
                .unwrap_or_default(),
            status_code: a
                .status_code()
                .map(|s| s.as_str().to_string())
                .unwrap_or_default(),
            description: a.description().unwrap_or("").to_string(),
            cause: a.cause().unwrap_or("").to_string(),
            progress: a.progress(),
            status_message: a.status_message().map(|s| s.to_string()),
        })
        .collect();

    Ok(activities)
}

use crate::aws::services::ec2::MetricsTimeRange;

#[derive(Debug, Clone)]
pub struct AsgMetricsData {
    pub time_range: MetricsTimeRange,
    pub desired: Vec<(f64, f64)>,
    pub in_service: Vec<(f64, f64)>,
    pub total: Vec<(f64, f64)>,
    pub pending: Vec<(f64, f64)>,
    pub x_max: f64,
}

#[derive(Debug, Clone)]
pub enum AsgMetricsState {
    Loading,
    Loaded(AsgMetricsData),
}

fn parse_avg(dps: &[aws_sdk_cloudwatch::types::Datapoint], start_secs: i64) -> Vec<(f64, f64)> {
    let mut pts: Vec<(f64, f64)> = dps
        .iter()
        .filter_map(|dp| {
            let t = (dp.timestamp()?.secs() - start_secs) as f64;
            Some((t, dp.average()?))
        })
        .collect();
    pts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    pts
}

/// ASG capacity metrics (namespace `AWS/AutoScaling`, dim `AutoScalingGroupName`):
/// desired / in-service / total / pending instances over time. **Requires group
/// metrics collection enabled on the ASG** — otherwise the series come back empty.
pub async fn fetch_asg_metrics(
    cw_client: aws_sdk_cloudwatch::Client,
    group_name: String,
    time_range: MetricsTimeRange,
) -> Result<AsgMetricsData> {
    use aws_sdk_cloudwatch::types::{Dimension, Statistic};

    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let start_secs = now_secs - time_range.duration_secs();
    let period = time_range.period_secs();
    let start_dt = aws_sdk_cloudwatch::primitives::DateTime::from_secs(start_secs);
    let end_dt = aws_sdk_cloudwatch::primitives::DateTime::from_secs(now_secs);

    let metric = |name: &'static str| {
        cw_client
            .get_metric_statistics()
            .namespace("AWS/AutoScaling")
            .metric_name(name)
            .dimensions(
                Dimension::builder()
                    .name("AutoScalingGroupName")
                    .value(&group_name)
                    .build(),
            )
            .start_time(start_dt.clone())
            .end_time(end_dt.clone())
            .period(period)
            .set_statistics(Some(vec![Statistic::Average]))
            .send()
    };

    let (desired, in_service, total, pending) = tokio::join!(
        metric("GroupDesiredCapacity"),
        metric("GroupInServiceInstances"),
        metric("GroupTotalInstances"),
        metric("GroupPendingInstances"),
    );

    let dps = |r: std::result::Result<
        aws_sdk_cloudwatch::operation::get_metric_statistics::GetMetricStatisticsOutput,
        _,
    >| match r {
        Ok(o) => o.datapoints().to_vec(),
        Err(_) => vec![],
    };

    Ok(AsgMetricsData {
        time_range,
        desired: parse_avg(&dps(desired), start_secs),
        in_service: parse_avg(&dps(in_service), start_secs),
        total: parse_avg(&dps(total), start_secs),
        pending: parse_avg(&dps(pending), start_secs),
        x_max: time_range.duration_secs() as f64,
    })
}
