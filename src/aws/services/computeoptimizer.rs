//! AWS Compute Optimizer — the right-sizing lens behind the lazy **Optimizer**
//! section on the EC2 instance / EBS volume / Lambda function / ASG / ECS
//! service detail panes (B7). Not a `ServiceType`: there is no Compute
//! Optimizer list view, only per-resource `Get*Recommendations` calls filtered
//! by ARN, plus a one-shot `GetEnrollmentStatus` cached on `App` so an
//! unenrolled account shows a hint instead of five failing sections.
//!
//! Everything is flattened here into the display-ready [`OptimizerRec`] /
//! [`OptimizerOption`] so `details_pane::optimizer_lines` stays a dumb
//! row-builder shared by all five panes.

use aws_sdk_computeoptimizer::Client;

/// Account/region enrollment, fetched once per session (`App.co_enrollment`).
#[derive(Debug, Clone)]
pub enum CoEnrollment {
    Active,
    /// Not enrolled — `.0` is the status (Inactive / Pending / Failed), with
    /// the reason appended when AWS provides one.
    Inactive(String),
    Error(String),
}

/// Which `Get*Recommendations` call the selected resource maps to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CoTargetKind {
    Ec2Instance,
    EbsVolume,
    LambdaFunction,
    Asg,
    EcsService,
}

/// One recommendation, flattened for display. Lives in
/// `LazyStore::optimizer_recs` keyed by resource ARN; `Loaded(None)` = the
/// call succeeded but Compute Optimizer has no recommendation for this
/// resource (yet) — normal for new resources (14-day lookback), non-Fargate
/// ECS services, and unsupported configs.
#[derive(Debug, Clone, Default)]
pub struct OptimizerRec {
    /// Raw finding code (`Overprovisioned` / `Underprovisioned` / `Optimized`
    /// / `NotOptimized` / `Unavailable`) — render via [`display_finding`].
    pub finding: String,
    pub finding_reasons: Vec<String>,
    pub lookback_days: f64,
    /// Current performance risk (EC2/ASG/Lambda/ECS: VeryLow…VeryHigh).
    pub performance_risk: Option<String>,
    /// Current configuration rows, e.g. ("Instance type", "m5.2xlarge").
    pub current: Vec<(String, String)>,
    /// Observed utilization rows, e.g. ("CPU (Maximum)", "4.2%").
    pub utilization: Vec<(String, String)>,
    pub options: Vec<OptimizerOption>,
}

/// One recommendation option (up to three per resource).
#[derive(Debug, Clone, Default)]
pub struct OptimizerOption {
    pub rank: i32,
    /// What to change to: an instance type, a volume config, a memory size,
    /// or an ECS cpu/memory pair.
    pub label: String,
    /// 0–5 performance-risk score (EC2/EBS/ASG options).
    pub performance_risk: Option<f64>,
    pub migration_effort: Option<String>,
    /// Pre-formatted savings, e.g. "save ≈ $12.34/mo (18%)".
    pub savings: Option<String>,
    /// Projected utilization at this option, e.g. "CPU (Maximum) → 22.4%".
    pub projected: Vec<String>,
}

// ── ARN builders (EC2/EBS rows don't carry ARNs) ──────────────────────────────

pub fn ec2_instance_arn(region: &str, account: &str, instance_id: &str) -> String {
    format!("arn:aws:ec2:{}:{}:instance/{}", region, account, instance_id)
}

pub fn ebs_volume_arn(region: &str, account: &str, volume_id: &str) -> String {
    format!("arn:aws:ec2:{}:{}:volume/{}", region, account, volume_id)
}

// ── Display helpers ───────────────────────────────────────────────────────────

/// Human form of a finding code, prefixed so `style_detail_row` colors it
/// (✓ green, ⚠ yellow) without bespoke styling.
pub fn display_finding(raw: &str) -> String {
    let lower = raw.to_ascii_lowercase();
    if lower.contains("overprovisioned") || lower.contains("over_provisioned") {
        "⚠ Over-provisioned".to_string()
    } else if lower.contains("underprovisioned") || lower.contains("under_provisioned") {
        "⚠ Under-provisioned".to_string()
    } else if lower == "notoptimized" || lower == "not_optimized" {
        "⚠ Not optimized".to_string()
    } else if lower == "optimized" {
        "✓ Optimized".to_string()
    } else if raw.is_empty() {
        "—".to_string()
    } else {
        raw.to_string()
    }
}

/// Trim a float to at most one decimal ("4.2", "3000").
fn trim_f64(v: f64) -> String {
    if (v - v.round()).abs() < 0.05 {
        format!("{}", v.round() as i64)
    } else {
        format!("{:.1}", v)
    }
}

/// "CPU (Maximum)" → "4.2%" style pair; `%` only where the metric is a
/// percentage (CPU / MEMORY on EC2, ASG and ECS).
fn fmt_metric(name: &str, statistic: &str, value: f64, percent: bool) -> (String, String) {
    let key = if statistic.is_empty() {
        name.to_string()
    } else {
        format!("{} ({})", name, statistic)
    };
    let val = if percent {
        format!("{}%", trim_f64(value))
    } else {
        trim_f64(value)
    };
    (key, val)
}

fn is_percent_metric(name: &str) -> bool {
    matches!(name.to_ascii_uppercase().as_str(), "CPU" | "MEMORY")
}

fn fmt_savings_parts(pct: f64, monthly: Option<(String, f64)>) -> Option<String> {
    match monthly {
        Some((cur, value)) if value > 0.0 => {
            let sym = if cur == "USD" { "$".to_string() } else { format!("{} ", cur) };
            Some(format!("save ≈ {}{:.2}/mo ({}%)", sym, value, trim_f64(pct)))
        }
        _ if pct > 0.0 => Some(format!("save ≈ {}%", trim_f64(pct))),
        _ => None,
    }
}

fn fmt_savings(
    so: Option<&aws_sdk_computeoptimizer::types::SavingsOpportunity>,
) -> Option<String> {
    let so = so?;
    let monthly = so.estimated_monthly_savings().map(|e| {
        (
            e.currency().map(|c| c.as_str().to_string()).unwrap_or_default(),
            e.value(),
        )
    });
    fmt_savings_parts(so.savings_opportunity_percentage(), monthly)
}

fn err_msg<E, R>(e: &aws_sdk_computeoptimizer::error::SdkError<E, R>) -> String
where
    E: std::error::Error
        + aws_sdk_computeoptimizer::error::ProvideErrorMetadata
        + Send
        + Sync
        + 'static,
    R: std::fmt::Debug,
{
    crate::error::sdk_error_message(e)
}

// ── Fetchers ──────────────────────────────────────────────────────────────────

/// One-shot enrollment check; all failure modes fold into the enum so the
/// caller just stores it.
pub async fn fetch_enrollment(client: Client) -> CoEnrollment {
    match client.get_enrollment_status().send().await {
        Ok(resp) => {
            let status = resp
                .status()
                .map(|s| s.as_str().to_string())
                .unwrap_or_else(|| "Unknown".to_string());
            if status == "Active" {
                CoEnrollment::Active
            } else {
                let reason = resp
                    .status_reason()
                    .filter(|r| !r.is_empty())
                    .map(|r| format!(" — {}", r))
                    .unwrap_or_default();
                CoEnrollment::Inactive(format!("{}{}", status, reason))
            }
        }
        Err(e) => CoEnrollment::Error(err_msg(&e)),
    }
}

/// Fetch + flatten the recommendation for one resource. `Ok(None)` = no
/// recommendation exists (a normal state, not an error).
pub async fn fetch_recommendation(
    client: Client,
    kind: CoTargetKind,
    arn: String,
) -> Result<Option<OptimizerRec>, String> {
    match kind {
        CoTargetKind::Ec2Instance => fetch_ec2(client, arn).await,
        CoTargetKind::EbsVolume => fetch_ebs(client, arn).await,
        CoTargetKind::LambdaFunction => fetch_lambda(client, arn).await,
        CoTargetKind::Asg => fetch_asg(client, arn).await,
        CoTargetKind::EcsService => fetch_ecs(client, arn).await,
    }
}

/// A per-resource entry in the response's `errors` list (e.g. "resource type
/// not supported") surfaces as the section's error text.
fn recommendation_error(code: Option<&str>, message: Option<&str>) -> String {
    match (code, message) {
        (Some(c), Some(m)) => format!("{}: {}", c, m),
        (_, Some(m)) => m.to_string(),
        (Some(c), _) => c.to_string(),
        _ => "recommendation unavailable".to_string(),
    }
}

async fn fetch_ec2(client: Client, arn: String) -> Result<Option<OptimizerRec>, String> {
    let resp = client
        .get_ec2_instance_recommendations()
        .instance_arns(&arn)
        .send()
        .await
        .map_err(|e| err_msg(&e))?;
    if let Some(err) = resp.errors().first() {
        if resp.instance_recommendations().is_empty() {
            return Err(recommendation_error(err.code(), err.message()));
        }
    }
    let Some(r) = resp.instance_recommendations().first() else {
        return Ok(None);
    };
    let current = vec![(
        "Instance type".to_string(),
        r.current_instance_type().unwrap_or("—").to_string(),
    )];
    Ok(Some(OptimizerRec {
        finding: r.finding().map(|f| f.as_str().to_string()).unwrap_or_default(),
        finding_reasons: r
            .finding_reason_codes()
            .iter()
            .map(|c| c.as_str().to_string())
            .collect(),
        lookback_days: r.look_back_period_in_days(),
        performance_risk: r.current_performance_risk().map(|p| p.as_str().to_string()),
        current,
        utilization: r
            .utilization_metrics()
            .iter()
            .map(|m| {
                let name = m.name().map(|n| n.as_str()).unwrap_or("metric");
                fmt_metric(
                    name,
                    m.statistic().map(|s| s.as_str()).unwrap_or(""),
                    m.value(),
                    is_percent_metric(name),
                )
            })
            .collect(),
        options: r
            .recommendation_options()
            .iter()
            .map(|o| OptimizerOption {
                rank: o.rank(),
                label: o.instance_type().unwrap_or("—").to_string(),
                performance_risk: Some(o.performance_risk()),
                migration_effort: o.migration_effort().map(|m| m.as_str().to_string()),
                savings: fmt_savings(o.savings_opportunity()),
                projected: o
                    .projected_utilization_metrics()
                    .iter()
                    .map(|m| {
                        let name = m.name().map(|n| n.as_str()).unwrap_or("metric");
                        let (k, v) = fmt_metric(
                            name,
                            m.statistic().map(|s| s.as_str()).unwrap_or(""),
                            m.value(),
                            is_percent_metric(name),
                        );
                        format!("{} → {}", k, v)
                    })
                    .collect(),
            })
            .collect(),
    }))
}

async fn fetch_ebs(client: Client, arn: String) -> Result<Option<OptimizerRec>, String> {
    let resp = client
        .get_ebs_volume_recommendations()
        .volume_arns(&arn)
        .send()
        .await
        .map_err(|e| err_msg(&e))?;
    if let Some(err) = resp.errors().first() {
        if resp.volume_recommendations().is_empty() {
            return Err(recommendation_error(err.code(), err.message()));
        }
    }
    let Some(r) = resp.volume_recommendations().first() else {
        return Ok(None);
    };
    fn volume_label(c: Option<&aws_sdk_computeoptimizer::types::VolumeConfiguration>) -> String {
        let Some(c) = c else { return "—".to_string() };
        let mut s = format!(
            "{} {} GiB",
            c.volume_type().unwrap_or("—"),
            c.volume_size()
        );
        if c.volume_baseline_iops() > 0 {
            s.push_str(&format!(", {} IOPS", c.volume_baseline_iops()));
        }
        if c.volume_baseline_throughput() > 0 {
            s.push_str(&format!(", {} MBps", c.volume_baseline_throughput()));
        }
        s
    }
    let current = vec![("Volume".to_string(), volume_label(r.current_configuration()))];
    Ok(Some(OptimizerRec {
        finding: r.finding().map(|f| f.as_str().to_string()).unwrap_or_default(),
        finding_reasons: Vec::new(),
        lookback_days: r.look_back_period_in_days(),
        performance_risk: r.current_performance_risk().map(|p| p.as_str().to_string()),
        current,
        utilization: r
            .utilization_metrics()
            .iter()
            .map(|m| {
                fmt_metric(
                    m.name().map(|n| n.as_str()).unwrap_or("metric"),
                    m.statistic().map(|s| s.as_str()).unwrap_or(""),
                    m.value(),
                    false,
                )
            })
            .collect(),
        options: r
            .volume_recommendation_options()
            .iter()
            .map(|o| OptimizerOption {
                rank: o.rank(),
                label: volume_label(o.configuration()),
                performance_risk: Some(o.performance_risk()),
                migration_effort: None,
                savings: fmt_savings(o.savings_opportunity()),
                projected: Vec::new(),
            })
            .collect(),
    }))
}

async fn fetch_lambda(client: Client, arn: String) -> Result<Option<OptimizerRec>, String> {
    let resp = client
        .get_lambda_function_recommendations()
        .function_arns(&arn)
        .send()
        .await
        .map_err(|e| err_msg(&e))?;
    let Some(r) = resp.lambda_function_recommendations().first() else {
        return Ok(None);
    };
    let current = vec![
        (
            "Memory size".to_string(),
            format!("{} MB", r.current_memory_size()),
        ),
        (
            "Invocations (lookback)".to_string(),
            r.number_of_invocations().to_string(),
        ),
    ];
    Ok(Some(OptimizerRec {
        finding: r.finding().map(|f| f.as_str().to_string()).unwrap_or_default(),
        finding_reasons: r
            .finding_reason_codes()
            .iter()
            .map(|c| c.as_str().to_string())
            .collect(),
        lookback_days: r.lookback_period_in_days(),
        performance_risk: r.current_performance_risk().map(|p| p.as_str().to_string()),
        current,
        utilization: r
            .utilization_metrics()
            .iter()
            .map(|m| {
                fmt_metric(
                    m.name().map(|n| n.as_str()).unwrap_or("metric"),
                    m.statistic().map(|s| s.as_str()).unwrap_or(""),
                    m.value(),
                    false,
                )
            })
            .collect(),
        options: r
            .memory_size_recommendation_options()
            .iter()
            .map(|o| OptimizerOption {
                rank: o.rank(),
                label: format!("{} MB", o.memory_size()),
                performance_risk: None,
                migration_effort: None,
                savings: fmt_savings(o.savings_opportunity()),
                projected: o
                    .projected_utilization_metrics()
                    .iter()
                    .map(|m| {
                        format!(
                            "{} ({}) → {}",
                            m.name().map(|n| n.as_str()).unwrap_or("metric"),
                            m.statistic().map(|s| s.as_str()).unwrap_or(""),
                            trim_f64(m.value())
                        )
                    })
                    .collect(),
            })
            .collect(),
    }))
}

async fn fetch_asg(client: Client, arn: String) -> Result<Option<OptimizerRec>, String> {
    let resp = client
        .get_auto_scaling_group_recommendations()
        .auto_scaling_group_arns(&arn)
        .send()
        .await
        .map_err(|e| err_msg(&e))?;
    if let Some(err) = resp.errors().first() {
        if resp.auto_scaling_group_recommendations().is_empty() {
            return Err(recommendation_error(err.code(), err.message()));
        }
    }
    let Some(r) = resp.auto_scaling_group_recommendations().first() else {
        return Ok(None);
    };
    fn asg_label(
        c: Option<&aws_sdk_computeoptimizer::types::AutoScalingGroupConfiguration>,
    ) -> String {
        let Some(c) = c else { return "—".to_string() };
        let types = if c.mixed_instance_types().is_empty() {
            c.instance_type().unwrap_or("—").to_string()
        } else {
            c.mixed_instance_types().join(", ")
        };
        format!(
            "{} (desired {}, min {}, max {})",
            types,
            c.desired_capacity(),
            c.min_size(),
            c.max_size()
        )
    }
    let current = vec![("Configuration".to_string(), asg_label(r.current_configuration()))];
    Ok(Some(OptimizerRec {
        finding: r.finding().map(|f| f.as_str().to_string()).unwrap_or_default(),
        finding_reasons: Vec::new(),
        lookback_days: r.look_back_period_in_days(),
        performance_risk: r.current_performance_risk().map(|p| p.as_str().to_string()),
        current,
        utilization: r
            .utilization_metrics()
            .iter()
            .map(|m| {
                let name = m.name().map(|n| n.as_str()).unwrap_or("metric");
                fmt_metric(
                    name,
                    m.statistic().map(|s| s.as_str()).unwrap_or(""),
                    m.value(),
                    is_percent_metric(name),
                )
            })
            .collect(),
        options: r
            .recommendation_options()
            .iter()
            .map(|o| OptimizerOption {
                rank: o.rank(),
                label: asg_label(o.configuration()),
                performance_risk: Some(o.performance_risk()),
                migration_effort: o.migration_effort().map(|m| m.as_str().to_string()),
                savings: fmt_savings(o.savings_opportunity()),
                projected: o
                    .projected_utilization_metrics()
                    .iter()
                    .map(|m| {
                        let name = m.name().map(|n| n.as_str()).unwrap_or("metric");
                        let (k, v) = fmt_metric(
                            name,
                            m.statistic().map(|s| s.as_str()).unwrap_or(""),
                            m.value(),
                            is_percent_metric(name),
                        );
                        format!("{} → {}", k, v)
                    })
                    .collect(),
            })
            .collect(),
    }))
}

async fn fetch_ecs(client: Client, arn: String) -> Result<Option<OptimizerRec>, String> {
    let resp = client
        .get_ecs_service_recommendations()
        .service_arns(&arn)
        .send()
        .await
        .map_err(|e| err_msg(&e))?;
    if let Some(err) = resp.errors().first() {
        if resp.ecs_service_recommendations().is_empty() {
            return Err(recommendation_error(err.code(), err.message()));
        }
    }
    let Some(r) = resp.ecs_service_recommendations().first() else {
        return Ok(None);
    };
    fn cpu_mem(cpu: Option<i32>, memory: Option<i32>) -> String {
        format!(
            "cpu {} / memory {} MB",
            cpu.map(|v| v.to_string()).unwrap_or_else(|| "—".into()),
            memory.map(|v| v.to_string()).unwrap_or_else(|| "—".into())
        )
    }
    let mut current = Vec::new();
    if let Some(c) = r.current_service_configuration() {
        current.push(("Task size".to_string(), cpu_mem(c.cpu(), c.memory())));
    }
    if let Some(lt) = r.launch_type() {
        current.push(("Launch type".to_string(), lt.as_str().to_string()));
    }
    Ok(Some(OptimizerRec {
        finding: r.finding().map(|f| f.as_str().to_string()).unwrap_or_default(),
        finding_reasons: r
            .finding_reason_codes()
            .iter()
            .map(|c| c.as_str().to_string())
            .collect(),
        lookback_days: r.lookback_period_in_days(),
        performance_risk: r.current_performance_risk().map(|p| p.as_str().to_string()),
        current,
        utilization: r
            .utilization_metrics()
            .iter()
            .map(|m| {
                let name = m.name().map(|n| n.as_str()).unwrap_or("metric");
                fmt_metric(
                    name,
                    m.statistic().map(|s| s.as_str()).unwrap_or(""),
                    m.value(),
                    is_percent_metric(name),
                )
            })
            .collect(),
        options: r
            .service_recommendation_options()
            .iter()
            .enumerate()
            .map(|(i, o)| OptimizerOption {
                rank: (i + 1) as i32,
                label: cpu_mem(o.cpu(), o.memory()),
                performance_risk: None,
                migration_effort: None,
                savings: fmt_savings(o.savings_opportunity()),
                projected: o
                    .projected_utilization_metrics()
                    .iter()
                    .map(|m| {
                        let name = m.name().map(|n| n.as_str()).unwrap_or("metric");
                        format!(
                            "{} → {}%",
                            name,
                            trim_f64(m.upper_bound_value())
                        )
                    })
                    .collect(),
            })
            .collect(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_finding_maps_codes() {
        assert_eq!(display_finding("Overprovisioned"), "⚠ Over-provisioned");
        assert_eq!(display_finding("OVER_PROVISIONED"), "⚠ Over-provisioned");
        assert_eq!(display_finding("Underprovisioned"), "⚠ Under-provisioned");
        assert_eq!(display_finding("Optimized"), "✓ Optimized");
        assert_eq!(display_finding("NotOptimized"), "⚠ Not optimized");
        assert_eq!(display_finding("Unavailable"), "Unavailable");
        assert_eq!(display_finding(""), "—");
    }

    #[test]
    fn arn_builders_shape() {
        assert_eq!(
            ec2_instance_arn("eu-west-1", "123456789012", "i-abc"),
            "arn:aws:ec2:eu-west-1:123456789012:instance/i-abc"
        );
        assert_eq!(
            ebs_volume_arn("us-east-1", "123456789012", "vol-1"),
            "arn:aws:ec2:us-east-1:123456789012:volume/vol-1"
        );
    }

    #[test]
    fn savings_formatting() {
        assert_eq!(
            fmt_savings_parts(18.0, Some(("USD".into(), 12.34))),
            Some("save ≈ $12.34/mo (18%)".to_string())
        );
        assert_eq!(
            fmt_savings_parts(7.5, None),
            Some("save ≈ 7.5%".to_string())
        );
        // Optimized resources report zero savings — no row at all.
        assert_eq!(fmt_savings_parts(0.0, Some(("USD".into(), 0.0))), None);
        assert_eq!(fmt_savings_parts(0.0, None), None);
    }

    #[test]
    fn metric_formatting() {
        assert_eq!(
            fmt_metric("CPU", "Maximum", 4.23, true),
            ("CPU (Maximum)".to_string(), "4.2%".to_string())
        );
        assert_eq!(
            fmt_metric("VolumeReadOpsPerSecond", "Maximum", 3000.0, false),
            (
                "VolumeReadOpsPerSecond (Maximum)".to_string(),
                "3000".to_string()
            )
        );
    }
}
