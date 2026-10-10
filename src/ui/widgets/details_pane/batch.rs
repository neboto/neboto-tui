use super::*;

// ── Batch split panes ───────────────────────────────────────────────────────

pub(super) fn render_batch_queue_split(
    app: &App,
    q: &crate::aws::services::batch::BatchJobQueue,
    area: Rect,
    frame: &mut Frame,
) {
    let subtitle = format!("{} · priority {}", q.state_label(), q.priority);
    render_simple_split(
        app,
        area,
        frame,
        "Batch Job Queue",
        &q.name,
        &subtitle,
        &descriptor_tabs(app, &crate::aws::services::batch::BATCH_QUEUE_SECTIONS),
    );
}

pub(super) fn render_batch_ce_split(
    app: &App,
    c: &crate::aws::services::batch::BatchComputeEnv,
    area: Rect,
    frame: &mut Frame,
) {
    let mut subtitle = format!("{} · {}", c.ce_type, c.state_label());
    if let Some(p) = &c.provisioning {
        subtitle = format!("{} {} · {}", c.ce_type, p, c.state_label());
    }
    render_simple_split(
        app,
        area,
        frame,
        "Batch Compute Environment",
        &c.name,
        &subtitle,
        &descriptor_tabs(app, &crate::aws::services::batch::BATCH_CE_SECTIONS),
    );
}

pub(super) fn render_batch_job_split(
    app: &App,
    j: &crate::aws::services::batch::BatchJob,
    area: Rect,
    frame: &mut Frame,
) {
    let subtitle = format!("{} · {} · {}", j.status, j.queue, j.job_id);
    render_simple_split(
        app,
        area,
        frame,
        "Batch Job",
        &j.name,
        &subtitle,
        &descriptor_tabs(app, &crate::aws::services::batch::BATCH_JOB_SECTIONS),
    );
}

pub(super) fn render_batch_jobdef_split(
    app: &App,
    d: &crate::aws::services::batch::BatchJobDefinition,
    area: Rect,
    frame: &mut Frame,
) {
    let subtitle = format!("{} · {}", d.def_type, d.status.to_lowercase());
    render_simple_split(
        app,
        area,
        frame,
        "Batch Job Definition",
        &d.label,
        &subtitle,
        &descriptor_tabs(app, &crate::aws::services::batch::BATCH_JOBDEF_SECTIONS),
    );
}

pub(super) fn batch_vcpu_rows(c: &crate::aws::services::batch::BatchComputeEnv, indent: &str) -> Vec<(String, String)> {
    let n = |v: Option<i32>| v.map(|v| v.to_string()).unwrap_or_else(|| "—".to_string());
    let mut rows = Vec::new();
    if c.is_managed() {
        rows.push((
            format!("{indent}vCPU min / desired / max"),
            format!("{} / {} / {}", n(c.min_vcpus), n(c.desired_vcpus), n(c.max_vcpus)),
        ));
    } else if let Some(u) = c.unmanaged_vcpus {
        rows.push((format!("{indent}Unmanaged vCPU"), u.to_string()));
    }
    rows
}

pub(super) fn batch_status_value(label: &str, blocked: bool) -> String {
    if blocked {
        format!("✗ {label}")
    } else {
        label.to_string()
    }
}

/// The "why is nothing running" verdict for a compute environment with
/// RUNNABLE jobs waiting on the queues it serves.
pub(super) fn batch_ce_waiting_rows(
    c: &crate::aws::services::batch::BatchComputeEnv,
    waiting: usize,
) -> Vec<(String, String)> {
    if waiting == 0 {
        return Vec::new();
    }
    let jobs = if waiting == 1 { "1 RUNNABLE job".to_string() } else { format!("{waiting} RUNNABLE jobs") };
    // Short verdict line + dim hint lines: the pane body doesn't wrap.
    let (verdict, hints): (String, &[&str]) = match c.blocker() {
        Some("disabled") => (format!("✗ disabled with {jobs} waiting"), &["· enable it, or attach another environment to the queue"]),
        Some("INVALID") => (format!("✗ INVALID with {jobs} waiting"), &["· Status Reason above says what AWS rejected"]),
        Some(_) if c.is_fargate() => (
            format!("⚠ desired vCPU 0 with {jobs} waiting"),
            &[
                "· if it stays at 0, the usual causes:",
                "·   the job's vCPU / memory isn't a valid Fargate size",
                "·   the subnets are out of IPs or have no route to ECR",
            ],
        ),
        Some(_) => (
            format!("⚠ desired vCPU 0 with {jobs} waiting"),
            &[
                "· if it stays at 0, the usual causes:",
                "·   the job's vCPU / memory fits no allowed instance type",
                "·   the subnets are out of IPs or have no route to ECS / ECR",
                "·   the instance role or service role is wrong",
            ],
        ),
        None if c.is_managed() && c.desired_vcpus.is_some() && c.desired_vcpus >= c.max_vcpus => (
            format!("⚠ at max vCPU with {jobs} waiting"),
            &["· raise max vCPU, or the jobs wait for capacity"],
        ),
        None => (format!("· {jobs} waiting on the queues it serves"), &[]),
    };
    let mut rows = vec![(String::new(), String::new()), ("".to_string(), verdict)];
    rows.extend(hints.iter().map(|h| ("".to_string(), h.to_string())));
    rows
}

pub fn batch_queue_section_lines(
    q: &crate::aws::services::batch::BatchJobQueue,
    section: crate::aws::services::batch::BatchQueueDetailSection,
    ces: &[&crate::aws::services::batch::BatchComputeEnv],
    jobs: &[&crate::aws::services::batch::BatchJob],
) -> Vec<(String, String)> {
    use crate::aws::services::batch::{BatchQueueDetailSection as S, JOB_STATUSES};
    let find_ce = |name: &str| ces.iter().find(|c| c.name == name);
    match section {
        S::Overview => {
            let blocked = q.state == "DISABLED" || q.status == "INVALID";
            let mut rows = vec![
                ("Queue".to_string(), q.name.clone()),
                ("Status".to_string(), batch_status_value(&q.state_label(), blocked)),
                ("State".to_string(), q.state.clone()),
            ];
            if let Some(r) = &q.status_reason {
                rows.push(("Status Reason".to_string(), r.clone()));
            }
            rows.push(("Priority".to_string(), format!("{} (higher is scheduled first)", q.priority)));
            if let Some(t) = &q.queue_type {
                rows.push(("Queue Type".to_string(), t.clone()));
            }
            rows.push((
                "Scheduling Policy".to_string(),
                q.scheduling_policy
                    .clone()
                    .unwrap_or_else(|| "FIFO (no fair-share policy)".to_string()),
            ));
            rows.push(("ARN".to_string(), q.arn.clone()));

            // The stuck-queue verdict: RUNNABLE jobs and no environment that
            // can take them.
            let runnable = jobs.iter().filter(|j| j.status == "RUNNABLE").count();
            if runnable > 0 {
                let usable = q
                    .ce_order
                    .iter()
                    .filter(|(_, n)| find_ce(n).is_some_and(|c| c.blocker().is_none()))
                    .count();
                rows.push((String::new(), String::new()));
                if q.state == "DISABLED" {
                    rows.push((String::new(), format!("⚠ disabled with {runnable} RUNNABLE job(s)")));
                    rows.push((String::new(), "· a disabled queue schedules nothing new".to_string()));
                } else if usable == 0 && !q.ce_order.is_empty() {
                    rows.push((String::new(), format!("⚠ {runnable} RUNNABLE job(s), and no compute environment can take them")));
                    rows.push((String::new(), "· 2 Compute Environments shows why".to_string()));
                } else {
                    rows.push((String::new(), format!("· {runnable} RUNNABLE job(s) waiting")));
                }
            }

            if !q.time_limit_actions.is_empty() {
                rows.push((String::new(), String::new()));
                rows.push(("Job State Time Limits".to_string(), String::new()));
                for (state, secs, action, reason) in &q.time_limit_actions {
                    rows.push((
                        format!("  {state} after {}", crate::aws::services::step_functions::fmt_duration(*secs as i64)),
                        if reason.is_empty() { action.clone() } else { format!("{action} — {reason}") },
                    ));
                }
            }
            rows
        }
        S::ComputeEnvironments => {
            if q.ce_order.is_empty() && q.service_envs.is_empty() {
                return vec![("".to_string(), "No compute environments attached".to_string())];
            }
            let mut rows = Vec::new();
            for (i, (order, name)) in q.ce_order.iter().enumerate() {
                if i > 0 {
                    rows.push((String::new(), String::new()));
                }
                rows.push((name.clone(), String::new()));
                rows.push(("  Order".to_string(), order.to_string()));
                match find_ce(name) {
                    Some(c) => {
                        let mut model = c.ce_type.clone();
                        if let Some(p) = &c.provisioning {
                            model.push_str(&format!(" · {p}"));
                        }
                        rows.push((
                            "  Status".to_string(),
                            match c.blocker() {
                                Some(b @ ("disabled" | "INVALID")) => format!("✗ {b}"),
                                _ => c.state_label(),
                            },
                        ));
                        rows.push(("  Model".to_string(), model));
                        rows.extend(batch_vcpu_rows(c, "  "));
                        rows.push(("  ARN".to_string(), c.arn.clone()));
                    }
                    None => rows.push(("".to_string(), "· not in the loaded list".to_string())),
                }
            }
            if !q.service_envs.is_empty() {
                rows.push((String::new(), String::new()));
                rows.push(("Service Environments".to_string(), String::new()));
                for (order, name) in &q.service_envs {
                    rows.push((format!("  {order}"), name.clone()));
                }
            }
            rows
        }
        S::Jobs => {
            if jobs.is_empty() {
                return vec![
                    ("".to_string(), "No jobs on this queue".to_string()),
                    (
                        "".to_string(),
                        "· Batch keeps finished jobs for a limited time — an empty list may just be age".to_string(),
                    ),
                ];
            }
            let mut rows = vec![("By Status".to_string(), String::new())];
            for s in JOB_STATUSES {
                let n = jobs.iter().filter(|j| j.status == s).count();
                if n > 0 {
                    let v = match s {
                        "FAILED" => format!("✗ {n}"),
                        "RUNNABLE" => format!("⚠ {n}"),
                        _ => n.to_string(),
                    };
                    rows.push((format!("  {s}"), v));
                }
            }
            // Newest first within each status; ARNs so ⏎ opens the job.
            for s in JOB_STATUSES {
                let mut of: Vec<&&crate::aws::services::batch::BatchJob> =
                    jobs.iter().filter(|j| j.status == s).collect();
                if of.is_empty() {
                    continue;
                }
                of.sort_by_key(|j| std::cmp::Reverse(j.created_ms));
                rows.push((String::new(), String::new()));
                rows.push((format!("{s} ({})", of.len()), String::new()));
                for j in of.iter().take(25) {
                    rows.push((format!("  {}", j.name), j.arn.clone()));
                }
                if of.len() > 25 {
                    rows.push(("".to_string(), format!("· {} more — 3 Jobs lists them all", of.len() - 25)));
                }
            }
            rows
        }
        S::Tags => tag_rows(&q.tags),
    }
}

pub fn batch_ce_section_lines(
    c: &crate::aws::services::batch::BatchComputeEnv,
    section: crate::aws::services::batch::BatchCeDetailSection,
    queues: &[&crate::aws::services::batch::BatchJobQueue],
    waiting: &[&crate::aws::services::batch::BatchJob],
) -> Vec<(String, String)> {
    use crate::aws::services::batch::BatchCeDetailSection as S;
    match section {
        S::Overview => {
            let blocked = matches!(c.blocker(), Some("disabled" | "INVALID"));
            let mut rows = vec![
                ("Compute Environment".to_string(), c.name.clone()),
                ("Status".to_string(), batch_status_value(&c.state_label(), blocked)),
                ("State".to_string(), c.state.clone()),
            ];
            if let Some(r) = &c.status_reason {
                rows.push((
                    "Status Reason".to_string(),
                    if c.status == "INVALID" { format!("✗ {r}") } else { r.clone() },
                ));
            }
            rows.push(("Type".to_string(), c.ce_type.clone()));
            if let Some(p) = &c.provisioning {
                rows.push(("Provisioning".to_string(), p.clone()));
            }
            if let Some(o) = &c.orchestration {
                rows.push(("Orchestration".to_string(), o.clone()));
            }
            rows.extend(batch_vcpu_rows(c, ""));
            rows.push(("ARN".to_string(), c.arn.clone()));
            rows.extend(batch_ce_waiting_rows(c, waiting.len()));
            rows
        }
        S::Compute => {
            let mut rows = Vec::new();
            let opt = |rows: &mut Vec<(String, String)>, k: &str, v: &Option<String>| {
                if let Some(v) = v {
                    rows.push((k.to_string(), v.clone()));
                }
            };
            if !c.instance_types.is_empty() {
                rows.push(("Instance Types".to_string(), c.instance_types.join(", ")));
            } else if c.is_managed() && !c.is_fargate() {
                rows.push(("Instance Types".to_string(), "—".to_string()));
            }
            opt(&mut rows, "Allocation Strategy", &c.allocation_strategy);
            opt(&mut rows, "Image", &c.image_id);
            opt(&mut rows, "Launch Template", &c.launch_template);
            opt(&mut rows, "Instance Role", &c.instance_role);
            opt(&mut rows, "Spot Fleet Role", &c.spot_fleet_role);
            if let Some(b) = c.bid_percentage {
                rows.push(("Spot Bid".to_string(), format!("{b}% of On-Demand")));
            }
            opt(&mut rows, "Service Role", &c.service_role);
            opt(&mut rows, "ECS Cluster", &c.ecs_cluster_arn);
            opt(&mut rows, "EKS Cluster", &c.eks_cluster_arn);
            opt(&mut rows, "Kubernetes Namespace", &c.eks_namespace);
            if rows.is_empty() {
                rows.push((
                    "".to_string(),
                    "· unmanaged — you run the instances; Batch only schedules onto them".to_string(),
                ));
            }
            rows
        }
        S::Network => {
            if c.subnets.is_empty() && c.security_groups.is_empty() {
                return vec![("".to_string(), "No subnets or security groups (unmanaged environment)".to_string())];
            }
            let mut rows = vec![("Subnets".to_string(), String::new())];
            for s in &c.subnets {
                rows.push(("  Subnet".to_string(), s.clone()));
            }
            rows.push((String::new(), String::new()));
            rows.push(("Security Groups".to_string(), String::new()));
            for g in &c.security_groups {
                rows.push(("  Security Group".to_string(), g.clone()));
            }
            rows.push((String::new(), String::new()));
            rows.push(("".to_string(), "· N shows the effective rules across these groups".to_string()));
            rows
        }
        S::Queues => {
            if queues.is_empty() {
                return vec![(
                    "".to_string(),
                    "No queue uses this compute environment — it will never run a job".to_string(),
                )];
            }
            let mut rows = Vec::new();
            for (i, q) in queues.iter().enumerate() {
                if i > 0 {
                    rows.push((String::new(), String::new()));
                }
                rows.push((q.name.clone(), String::new()));
                rows.push(("  Status".to_string(), q.state_label()));
                rows.push(("  Priority".to_string(), q.priority.to_string()));
                if let Some((order, _)) = q.ce_order.iter().find(|(_, n)| *n == c.name) {
                    rows.push(("  Order on Queue".to_string(), order.to_string()));
                }
                let n = waiting.iter().filter(|j| j.queue == q.name).count();
                if n > 0 {
                    rows.push(("  RUNNABLE Jobs".to_string(), format!("⚠ {n}")));
                }
                rows.push(("  ARN".to_string(), q.arn.clone()));
            }
            rows
        }
        S::Tags => tag_rows(&c.tags),
    }
}

pub(super) fn batch_container_rows(
    c: &crate::aws::services::batch::BatchContainer,
    runtime: bool,
) -> Vec<(String, String)> {
    let mut rows = Vec::new();
    let opt = |rows: &mut Vec<(String, String)>, k: &str, v: &Option<String>| {
        if let Some(v) = v {
            rows.push((k.to_string(), v.clone()));
        }
    };
    opt(&mut rows, "Image", &c.image);
    opt(&mut rows, "vCPU", &c.vcpus);
    if let Some(m) = &c.memory_mib {
        rows.push(("Memory".to_string(), format!("{m} MiB")));
    }
    opt(&mut rows, "GPU", &c.gpus);
    if !c.command.is_empty() {
        rows.push(("Command".to_string(), c.command.join(" ")));
    }
    if runtime {
        match c.exit_code {
            Some(0) => rows.push(("Exit Code".to_string(), "✓ 0".to_string())),
            Some(n) => rows.push(("Exit Code".to_string(), format!("✗ {n}"))),
            None => {}
        }
        if let Some(r) = &c.reason {
            rows.push(("Reason".to_string(), format!("✗ {r}")));
        }
        opt(&mut rows, "Instance Type", &c.instance_type);
    } else {
        opt(&mut rows, "Instance Type", &c.instance_type);
    }
    match (&c.log_group, &c.log_driver) {
        (Some(g), _) => rows.push(("Log Group".to_string(), g.clone())),
        (None, Some(d)) => rows.push(("Log Driver".to_string(), format!("{d} (not CloudWatch)"))),
        (None, None) => {}
    }
    if runtime {
        rows.push((
            "Log Stream".to_string(),
            c.log_stream
                .clone()
                .unwrap_or_else(|| "· none yet — created when the container starts".to_string()),
        ));
    }
    opt(&mut rows, "Job Role", &c.job_role);
    opt(&mut rows, "Execution Role", &c.execution_role);
    if runtime {
        opt(&mut rows, "Task ARN", &c.task_arn);
    }
    rows
}

pub fn batch_job_section_lines(
    j: &crate::aws::services::batch::BatchJob,
    section: crate::aws::services::batch::BatchJobDetailSection,
) -> Vec<(String, String)> {
    use crate::aws::services::batch::{fmt_ms, BatchJobDetailSection as S};
    use crate::aws::services::step_functions::fmt_duration;
    match section {
        S::Overview => {
            let status = match j.status.as_str() {
                "FAILED" => format!("✗ {}", j.status),
                "SUCCEEDED" => format!("✓ {}", j.status),
                _ => j.status.clone(),
            };
            let mut rows = vec![
                ("Job".to_string(), j.name.clone()),
                ("Job ID".to_string(), j.job_id.clone()),
                ("Status".to_string(), status),
            ];
            if let Some(r) = &j.status_reason {
                rows.push((
                    "Status Reason".to_string(),
                    if j.status == "FAILED" { format!("✗ {r}") } else { r.clone() },
                ));
            }
            // A job parked in RUNNABLE is the classic stuck-queue symptom;
            // the cause is on the compute environments, not the job.
            if j.status == "RUNNABLE" {
                if let Some(age) = j.age_ms().filter(|a| *a > 10 * 60 * 1000) {
                    rows.push((String::new(), format!("⚠ RUNNABLE for {}", fmt_duration(age / 1000))));
                    rows.push((
                        String::new(),
                        "· the cause is on the queue's compute environments — ⏎ on Queue".to_string(),
                    ));
                }
            }
            rows.push(("Queue".to_string(), j.queue_arn.clone()));
            rows.push(("Job Definition".to_string(), j.job_definition.clone()));
            if let Some(t) = j.created_ms {
                rows.push(("Created".to_string(), fmt_ms(t)));
            }
            if let Some(t) = j.started_ms {
                rows.push(("Started".to_string(), fmt_ms(t)));
            }
            if let Some(t) = j.stopped_ms {
                rows.push(("Stopped".to_string(), fmt_ms(t)));
            }
            if let Some(ms) = j.run_ms() {
                rows.push((
                    if j.stopped_ms.is_some() { "Ran For" } else { "Running For" }.to_string(),
                    fmt_duration(ms / 1000),
                ));
            }
            let attempts = j.attempts.len();
            rows.push((
                "Attempts".to_string(),
                match j.retry_attempts {
                    Some(max) => format!("{attempts} of {max}"),
                    None => attempts.to_string(),
                },
            ));
            if let Some(t) = j.timeout_secs {
                rows.push(("Attempt Timeout".to_string(), fmt_duration(t as i64)));
            }
            if !j.platform.is_empty() {
                rows.push(("Platform".to_string(), j.platform.join(", ")));
            }
            if j.multinode {
                rows.push(("Multi-node".to_string(), "yes — per-node detail is in the console".to_string()));
            }
            if let Some(s) = &j.share_identifier {
                rows.push(("Share Identifier".to_string(), s.clone()));
            }
            if let Some(p) = j.scheduling_priority {
                rows.push(("Scheduling Priority".to_string(), p.to_string()));
            }
            if let Some(c) = &j.eks_cluster_arn {
                rows.push(("EKS Cluster".to_string(), c.clone()));
            }
            if let Some(p) = &j.eks_pod {
                rows.push(("Pod".to_string(), p.clone()));
            }
            if j.is_cancelled {
                rows.push(("Cancelled".to_string(), "⚠ a cancel was requested".to_string()));
            }
            if j.is_terminated {
                rows.push(("Terminated".to_string(), "⚠ a terminate was requested".to_string()));
            }
            if !j.parameters.is_empty() {
                rows.push((String::new(), String::new()));
                rows.push(("Parameters".to_string(), String::new()));
                for (k, v) in &j.parameters {
                    rows.push((format!("  {k}"), v.clone()));
                }
            }
            rows
        }
        S::Container => {
            if j.containers.is_empty() {
                return vec![(
                    "".to_string(),
                    if j.eks_cluster_arn.is_some() {
                        "EKS job — the pod's containers live in the cluster".to_string()
                    } else {
                        "No container detail on this job".to_string()
                    },
                )];
            }
            let multi = j.containers.len() > 1;
            let mut rows = Vec::new();
            for (i, c) in j.containers.iter().enumerate() {
                if i > 0 {
                    rows.push((String::new(), String::new()));
                }
                if multi {
                    rows.push((c.name.clone().unwrap_or_else(|| format!("Container {}", i + 1)), String::new()));
                    rows.extend(batch_container_rows(c, true).into_iter().map(|(k, v)| (format!("  {k}"), v)));
                } else {
                    rows.extend(batch_container_rows(c, true));
                }
            }
            if j.log_target().is_some() {
                rows.push((String::new(), String::new()));
                rows.push(("".to_string(), "· t tails this stream".to_string()));
            }
            rows
        }
        S::Attempts => {
            if j.attempts.is_empty() {
                return vec![("".to_string(), "No attempts yet — the job hasn't started".to_string())];
            }
            let mut rows = Vec::new();
            for (i, a) in j.attempts.iter().enumerate() {
                if i > 0 {
                    rows.push((String::new(), String::new()));
                }
                rows.push((format!("Attempt {}", i + 1), String::new()));
                if let Some(t) = a.started_ms {
                    rows.push(("  Started".to_string(), fmt_ms(t)));
                }
                if let (Some(s), Some(e)) = (a.started_ms, a.stopped_ms) {
                    rows.push(("  Ran For".to_string(), fmt_duration((e - s).max(0) / 1000)));
                }
                match a.exit_code {
                    Some(0) => rows.push(("  Exit Code".to_string(), "✓ 0".to_string())),
                    Some(n) => rows.push(("  Exit Code".to_string(), format!("✗ {n}"))),
                    None => {}
                }
                if let Some(r) = &a.reason {
                    rows.push(("  Reason".to_string(), format!("✗ {r}")));
                }
                if let Some(r) = &a.status_reason {
                    rows.push(("  Status Reason".to_string(), r.clone()));
                }
                if let Some(s) = &a.log_stream {
                    rows.push(("  Log Stream".to_string(), s.clone()));
                }
            }
            rows
        }
        S::Dependencies => {
            let mut rows = Vec::new();
            if let Some(size) = j.array_size {
                rows.push(("Array Size".to_string(), size.to_string()));
            }
            if let Some(idx) = j.array_index {
                rows.push(("Array Index".to_string(), idx.to_string()));
            }
            if !j.array_summary.is_empty() {
                rows.push((String::new(), String::new()));
                rows.push(("Children by Status".to_string(), String::new()));
                for (s, n) in &j.array_summary {
                    let v = match s.as_str() {
                        "FAILED" if *n > 0 => format!("✗ {n}"),
                        _ => n.to_string(),
                    };
                    rows.push((format!("  {s}"), v));
                }
            }
            if !j.depends_on.is_empty() {
                if !rows.is_empty() {
                    rows.push((String::new(), String::new()));
                }
                rows.push(("Depends On".to_string(), String::new()));
                for (id, kind) in &j.depends_on {
                    rows.push((
                        format!("  {id}"),
                        if kind.is_empty() { "—".to_string() } else { kind.clone() },
                    ));
                }
            }
            if rows.is_empty() {
                rows.push(("".to_string(), "Not an array job, and no dependencies".to_string()));
            }
            rows
        }
        S::Tags => tag_rows(&j.tags),
    }
}

pub fn batch_jobdef_section_lines(
    d: &crate::aws::services::batch::BatchJobDefinition,
    section: crate::aws::services::batch::BatchJobDefDetailSection,
) -> Vec<(String, String)> {
    use crate::aws::services::batch::BatchJobDefDetailSection as S;
    use crate::aws::services::step_functions::fmt_duration;
    match section {
        S::Overview => {
            let mut rows = vec![
                ("Job Definition".to_string(), d.def_name.clone()),
                ("Revision".to_string(), d.revision.to_string()),
                ("Status".to_string(), d.status.clone()),
                ("Type".to_string(), d.def_type.clone()),
            ];
            if let Some(o) = &d.orchestration {
                rows.push(("Orchestration".to_string(), o.clone()));
            }
            if !d.platform.is_empty() {
                rows.push(("Platform".to_string(), d.platform.join(", ")));
            }
            rows.push((
                "Retry Attempts".to_string(),
                d.retry_attempts
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "1 (no retries)".to_string()),
            ));
            for r in &d.retry_rules {
                rows.push(("  Retry Rule".to_string(), r.clone()));
            }
            rows.push((
                "Attempt Timeout".to_string(),
                d.timeout_secs
                    .map(|t| fmt_duration(t as i64))
                    .unwrap_or_else(|| "none — an attempt can run forever".to_string()),
            ));
            if let Some(p) = d.scheduling_priority {
                rows.push(("Scheduling Priority".to_string(), p.to_string()));
            }
            rows.push((
                "Propagate Tags".to_string(),
                if d.propagate_tags { "yes" } else { "no" }.to_string(),
            ));
            rows.push(("ARN".to_string(), d.arn.clone()));
            if !d.parameters.is_empty() {
                rows.push((String::new(), String::new()));
                rows.push(("Parameter Defaults".to_string(), String::new()));
                for (k, v) in &d.parameters {
                    rows.push((format!("  {k}"), v.clone()));
                }
            }
            rows
        }
        S::Container => {
            if d.containers.is_empty() {
                return vec![(
                    "".to_string(),
                    if d.def_type == "multinode" {
                        "Multi-node definition — node ranges are in the console".to_string()
                    } else {
                        "No container properties (EKS pod definition)".to_string()
                    },
                )];
            }
            let multi = d.containers.len() > 1;
            let mut rows = Vec::new();
            for (i, c) in d.containers.iter().enumerate() {
                if i > 0 {
                    rows.push((String::new(), String::new()));
                }
                if multi {
                    rows.push((c.name.clone().unwrap_or_else(|| format!("Container {}", i + 1)), String::new()));
                    rows.extend(batch_container_rows(c, false).into_iter().map(|(k, v)| (format!("  {k}"), v)));
                } else {
                    rows.extend(batch_container_rows(c, false));
                }
            }
            rows
        }
        S::Tags => tag_rows(&d.tags),
    }
}
