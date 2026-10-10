use super::*;

// ── Lambda Layer (eager Overview + Used By, lazy Versions) ─────────────────

/// Used By row label for a consuming function — `lambda_row_jump_target`
/// keys on it, so the jump and the row change together.
pub const LAMBDA_LAYER_ROW_FUNCTION: &str = "  Function";

pub fn lambda_layer_section_lines(
    layer: &crate::aws::services::lambda::LambdaLayer,
    section: crate::aws::services::lambda::LambdaLayerDetailSection,
    versions: Option<&Lazy<Vec<crate::aws::services::lambda::LambdaLayerVersion>>>,
    functions: &[&LambdaFunction],
) -> Vec<(String, String)> {
    use crate::aws::services::lambda::LambdaLayerDetailSection as S;
    let kv = |k: &str, v: String| (k.to_string(), v);
    let blank = || (String::new(), String::new());
    let or_dash = |v: &str| if v.is_empty() { "—".to_string() } else { v.to_string() };
    let mut rows = vec![blank()];
    match section {
        S::Overview => {
            rows.push(kv("Layer Name", layer.name.clone()));
            rows.push(kv("ARN", layer.arn.clone()));
            rows.push(blank());
            rows.push(kv("Latest Version", layer.latest_version.to_string()));
            rows.push(kv("Version ARN", layer.latest_version_arn.clone()));
            rows.push(kv("Description", or_dash(&layer.description)));
            rows.push(kv("Created", or_dash(layer.created.split('.').next().unwrap_or(""))));
            rows.push(kv("Runtimes", or_dash(&layer.runtimes.join(", "))));
            rows.push(kv("Architectures", or_dash(&layer.architectures.join(", "))));
            rows.push(kv("License", or_dash(&layer.license)));
            let used = crate::aws::services::lambda::layer_usage(&layer.arn, functions);
            let n: usize = used.iter().map(|(_, f)| f.len()).sum();
            rows.push(blank());
            rows.push(kv("Used By", format!("{n} loaded function{}", if n == 1 { "" } else { "s" })));
        }
        S::Versions => match versions {
            None | Some(Lazy::Loading) => rows.push(kv("", "Loading…".to_string())),
            Some(Lazy::Error(e)) => rows.extend(error_rows(e)),
            Some(Lazy::Loaded(vs)) if vs.is_empty() => rows.push(kv("", "No versions".to_string())),
            Some(Lazy::Loaded(vs)) => {
                let used = crate::aws::services::lambda::layer_usage(&layer.arn, functions);
                for (i, v) in vs.iter().enumerate() {
                    if i > 0 {
                        rows.push(blank());
                    }
                    rows.push((format!("Version {}", v.version), String::new()));
                    rows.push(kv("ARN", v.arn.clone()));
                    rows.push(kv("Created", or_dash(v.created.split('.').next().unwrap_or(""))));
                    if !v.description.is_empty() {
                        rows.push(kv("Description", v.description.clone()));
                    }
                    rows.push(kv("Runtimes", or_dash(&v.runtimes.join(", "))));
                    rows.push(kv("Architectures", or_dash(&v.architectures.join(", "))));
                    if !v.license.is_empty() {
                        rows.push(kv("License", v.license.clone()));
                    }
                    let n = used.iter().find(|(ver, _)| *ver == v.version).map_or(0, |(_, f)| f.len());
                    rows.push(kv("Used By", format!("{n} loaded function{}", if n == 1 { "" } else { "s" })));
                }
            }
        },
        S::UsedBy => {
            let used = crate::aws::services::lambda::layer_usage(&layer.arn, functions);
            if used.is_empty() {
                rows.push(kv("", "No loaded function uses this layer".to_string()));
            }
            for (i, (version, fns)) in used.iter().enumerate() {
                if i > 0 {
                    rows.push(blank());
                }
                let latest = if *version == layer.latest_version { " (latest)" } else { "" };
                rows.push((format!("Version {version}{latest}"), String::new()));
                for f in fns {
                    rows.push(kv(LAMBDA_LAYER_ROW_FUNCTION, f.function_name.clone()));
                }
            }
            rows.push(blank());
            // No AWS API lists a layer's consumers: this is the functions in
            // this load (region + account) whose layer list names it.
            rows.push(kv(
                "",
                "· functions in this region and account only — other accounts may use it too".to_string(),
            ));
        }
    }
    rows.push(blank());
    rows
}

// ── Lambda Function Split Pane ────────────────────────────────────────────────

pub(super) fn render_lambda_function_split(
    app: &App,
    func: &LambdaFunction,
    area: Rect,
    frame: &mut Frame,
) {
    let focused = app.details_focused;
    let extras = if LambdaDetailSection::from_index(app.detail_section_index()) == LambdaDetailSection::Code {
        "d download"
    } else {
        ""
    };
    let footer = detail_footer(app, focused, 6, extras);

    let mut block = theme::pane_block("Lambda Function", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let header_lines = build_lambda_header_lines(func);
    let header_h = header_lines.len() as u16;

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(header_h),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(inner);

    frame.render_widget(Paragraph::new(header_lines), chunks[0]);
    render_hr(chunks[1], frame);
    render_lambda_section_tabs(app, chunks[2], frame);
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub(super) fn build_lambda_header_lines(func: &LambdaFunction) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = vec![Line::raw("")];

    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            func.function_name.clone(),
            Style::default()
                .fg(crate::ui::theme::text_primary())
                .add_modifier(Modifier::BOLD),
        ),
    ]));

    if !func.description.is_empty() {
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(
                func.description.clone(),
                Style::default().fg(theme::text_dim()),
            ),
        ]));
    }

    lines.push(Line::raw(""));
    lines.push(header_kv("Runtime", &func.runtime));
    lines.push(header_kv(
        "Memory / Timeout",
        &format!("{} MB  ·  {} s", func.memory_size, func.timeout),
    ));
    lines.push(header_kv("Code Size", &func.formatted_code_size()));
    if let Some(last_mod) = func.last_modified.split('.').next() {
        lines.push(header_kv("Last Modified", last_mod));
    }

    lines.push(Line::raw(""));
    lines
}

pub(super) fn render_lambda_section_tabs(app: &App, area: Rect, frame: &mut Frame) {
    render_section_tab_bar(
        app,
        area,
        frame,
        &descriptor_tabs(app, &crate::aws::services::lambda::LAMBDA_SECTIONS),
    );
}

pub fn lambda_section_lines(
    func: &LambdaFunction,
    section: LambdaDetailSection,
    code_state: Option<&crate::lazy::Lazy<Box<crate::aws::services::lambda::LambdaCodeInfo>>>,
    esm_state: Option<&crate::lazy::Lazy<Vec<crate::aws::services::lambda::LambdaEsmInfo>>>,
    eb_rules_state: Option<&crate::lazy::Lazy<Vec<crate::aws::services::eventbridge::EbRule>>>,
    conc_state: Option<&crate::lazy::Lazy<crate::aws::services::lambda::LambdaConcurrency>>,
    downloading: bool,
    optimizer: Option<&Lazy<Option<crate::aws::services::computeoptimizer::OptimizerRec>>>,
    enrollment: Option<&crate::aws::services::computeoptimizer::CoEnrollment>,
) -> Vec<(String, String)> {
    match section {
        LambdaDetailSection::Config => lambda_config_lines(func, conc_state),
        LambdaDetailSection::Code => lambda_code_lines(code_state, downloading),
        LambdaDetailSection::Triggers => lambda_triggers_lines(esm_state, eb_rules_state),
        LambdaDetailSection::Environment => lambda_env_lines(func),
        LambdaDetailSection::Tags => lambda_tags_lines(func, code_state),
        LambdaDetailSection::Optimizer => optimizer_lines(optimizer, enrollment),
    }
}

/// Triggers section: the function's event source mappings (poll-based — the
/// answer to "why isn't my queue draining") plus the EventBridge rules that
/// invoke it (push-based). Per-mapping / per-rule group with a jumpable ARN,
/// enabled state, and (for mappings) last processing result.
pub(super) fn lambda_triggers_lines(
    esm_state: Option<&crate::lazy::Lazy<Vec<crate::aws::services::lambda::LambdaEsmInfo>>>,
    eb_rules_state: Option<&crate::lazy::Lazy<Vec<crate::aws::services::eventbridge::EbRule>>>,
) -> Vec<(String, String)> {
    let mut rows = vec![("".to_string(), "".to_string())];

    match esm_state {
        None | Some(crate::lazy::Lazy::Loading) => {
            rows.push(("  Loading event source mappings…".to_string(), "".to_string()));
        }
        Some(crate::lazy::Lazy::Error(e)) => {
            rows.extend(error_rows(e));
        }
        Some(crate::lazy::Lazy::Loaded(esms)) if esms.is_empty() => {
            rows.push(("  No event source mappings".to_string(), "".to_string()));
            rows.push(("".to_string(), "".to_string()));
            rows.push((
                "  Only poll-based triggers (SQS, Kinesis, DynamoDB Streams, Kafka, MQ)"
                    .to_string(),
                "".to_string(),
            ));
            rows.push((
                "  appear here. Other push invokers (API Gateway, S3, SNS) call the"
                    .to_string(),
                "".to_string(),
            ));
            rows.push((
                "  function directly and are configured on the source service."
                    .to_string(),
                "".to_string(),
            ));
        }
        Some(crate::lazy::Lazy::Loaded(esms)) => {
            for (i, esm) in esms.iter().enumerate() {
                if i > 0 {
                    rows.push(("".to_string(), "".to_string()));
                }
                rows.push((esm.source_label().to_string(), "".to_string()));

                if let Some(arn) = &esm.event_source_arn {
                    rows.push(("Source".to_string(), arn.clone()));
                }
                let state_val = if esm.is_enabled() {
                    format!("✓ {}", esm.state)
                } else if esm.state.eq_ignore_ascii_case("Disabled") {
                    format!("✗ {}", esm.state)
                } else {
                    // Creating / Enabling / Disabling / Updating / Deleting
                    format!("⚠ {}", esm.state)
                };
                rows.push(("Status".to_string(), state_val));
                if let Some(reason) = &esm.state_transition_reason {
                    // AWS uses "USER_INITIATED" / "User action" for normal
                    // transitions; anything else is worth reading.
                    if !reason.is_empty()
                        && !reason.eq_ignore_ascii_case("USER_INITIATED")
                        && !reason.eq_ignore_ascii_case("User action")
                    {
                        rows.push(("  Reason".to_string(), reason.clone()));
                    }
                }
                if let Some(result) = &esm.last_processing_result {
                    let val = if result == "OK" {
                        format!("✓ {}", result)
                    } else if result == "No records processed" {
                        result.clone()
                    } else {
                        format!("✗ {}", result)
                    };
                    rows.push(("Last Result".to_string(), val));
                }
                if let Some(modified) = &esm.last_modified {
                    rows.push(("Last Modified".to_string(), modified.clone()));
                }

                if let Some(batch) = esm.batch_size {
                    let mut batching = format!("{} records", batch);
                    if let Some(window) = esm.max_batching_window_secs {
                        if window > 0 {
                            batching.push_str(&format!(" · window {}s", window));
                        }
                    }
                    rows.push(("Batch".to_string(), batching));
                }
                if let Some(pos) = &esm.starting_position {
                    rows.push(("Starting Position".to_string(), pos.clone()));
                }
                if let Some(par) = esm.parallelization_factor {
                    if par > 1 {
                        rows.push(("Parallelization".to_string(), format!("{par} per shard")));
                    }
                }
                if let Some(conc) = esm.max_concurrency {
                    rows.push(("Max Concurrency".to_string(), conc.to_string()));
                }
                if let Some(retries) = esm.maximum_retry_attempts {
                    // -1 = retry until the record expires (the stream default).
                    let val = if retries < 0 {
                        "until record expires".to_string()
                    } else {
                        retries.to_string()
                    };
                    rows.push(("Max Retries".to_string(), val));
                }
                if let Some(age) = esm.maximum_record_age_secs {
                    if age > 0 {
                        rows.push(("Max Record Age".to_string(), format!("{age} s")));
                    }
                }
                if esm.bisect_batch_on_error == Some(true) {
                    rows.push(("Bisect On Error".to_string(), "✓ enabled".to_string()));
                }
                if esm.report_batch_item_failures {
                    rows.push((
                        "Partial Failures".to_string(),
                        "✓ ReportBatchItemFailures".to_string(),
                    ));
                }
                if let Some(dest) = &esm.on_failure_destination {
                    rows.push(("On-Failure Dest".to_string(), dest.clone()));
                }
                if !esm.topics.is_empty() {
                    rows.push(("Topics".to_string(), esm.topics.join(", ")));
                }
                if !esm.queues.is_empty() {
                    rows.push(("Queues".to_string(), esm.queues.join(", ")));
                }
                for pattern in &esm.filter_patterns {
                    rows.push(("Filter".to_string(), pattern.clone()));
                }
                rows.push(("UUID".to_string(), esm.uuid.clone()));
            }
        }
    }

    rows.push(("".to_string(), "".to_string()));
    rows.push(("EventBridge Rules".to_string(), "".to_string())); // group header

    match eb_rules_state {
        None | Some(crate::lazy::Lazy::Loading) => {
            rows.push(("  Loading EventBridge rules…".to_string(), "".to_string()));
        }
        Some(crate::lazy::Lazy::Error(e)) => {
            rows.extend(error_rows(e));
        }
        Some(crate::lazy::Lazy::Loaded(rules)) if rules.is_empty() => {
            rows.push(("  No rules target this function".to_string(), "".to_string()));
        }
        Some(crate::lazy::Lazy::Loaded(rules)) => {
            for (i, rule) in rules.iter().enumerate() {
                if i > 0 {
                    rows.push(("".to_string(), "".to_string()));
                }
                rows.push((rule.name.clone(), "".to_string()));
                let state_val = if rule.state.eq_ignore_ascii_case("ENABLED") {
                    format!("✓ {}", rule.state)
                } else if rule.state.eq_ignore_ascii_case("DISABLED") {
                    format!("✗ {}", rule.state)
                } else {
                    format!("⚠ {}", rule.state)
                };
                rows.push(("Status".to_string(), state_val));
                rows.push(("Bus".to_string(), rule.bus_name.clone()));
                if let Some(sched) = &rule.schedule {
                    rows.push(("Schedule".to_string(), sched.clone()));
                } else if rule.event_pattern.is_some() {
                    rows.push(("Trigger".to_string(), "Event pattern".to_string()));
                }
                if !rule.description.is_empty() {
                    rows.push(("Description".to_string(), rule.description.clone()));
                }
                // Cross-service jump anchor back to the EventBridge rule.
                rows.push(("Rule ARN".to_string(), rule.arn.clone()));
            }
        }
    }

    rows.push(("".to_string(), "".to_string()));
    rows
}

pub(super) fn lambda_code_lines(
    code_state: Option<&crate::lazy::Lazy<Box<crate::aws::services::lambda::LambdaCodeInfo>>>,
    downloading: bool,
) -> Vec<(String, String)> {
    let mut rows = vec![("".to_string(), "".to_string())];

    match code_state {
        None | Some(crate::lazy::Lazy::Loading) => {
            rows.push(("  Loading code info…".to_string(), "".to_string()));
        }
        Some(crate::lazy::Lazy::Error(e)) => {
            rows.extend(error_rows(e));
        }
        Some(crate::lazy::Lazy::Loaded(info)) => {
            if let Some(state) = &info.state {
                let mut v = state.clone();
                if let Some(reason) = &info.state_reason {
                    v.push_str(&format!(" — {}", reason));
                }
                rows.push(("State".to_string(), v));
            }
            if let Some(status) = &info.last_update_status {
                let mut v = status.clone();
                if let Some(reason) = &info.last_update_status_reason {
                    v.push_str(&format!(" — {}", reason));
                }
                rows.push(("Last Update".to_string(), v));
            }
            rows.push(("Package Type".to_string(), info.package_type.clone()));
            if let Some(repo) = &info.repository_type {
                rows.push(("Repository".to_string(), repo.clone()));
            }
            rows.push(("Runtime".to_string(), info.runtime.clone()));
            if !info.handler.is_empty() {
                rows.push(("Handler".to_string(), info.handler.clone()));
            }
            rows.push((
                "Code Size".to_string(),
                fmt_bytes(info.code_size),
            ));
            if let Some(sha) = &info.code_sha256 {
                rows.push(("SHA256".to_string(), sha.clone()));
            }

            rows.push(("".to_string(), "".to_string()));
            if info.is_zip() {
                if downloading {
                    rows.push(("  Downloading & extracting…".to_string(), "".to_string()));
                } else {
                    rows.push((
                        "  Press d to download & extract the source package".to_string(),
                        "".to_string(),
                    ));
                }
            } else {
                // Container-image function: no zip; show the image instead.
                if let Some(uri) = &info.image_uri {
                    rows.push(("Image URI".to_string(), uri.clone()));
                }
                rows.push((
                    "  Container-image function — no zip package to download".to_string(),
                    "".to_string(),
                ));
            }
        }
    }

    rows.push(("".to_string(), "".to_string()));
    rows
}

pub(super) fn lambda_config_lines(
    func: &LambdaFunction,
    conc_state: Option<&crate::lazy::Lazy<crate::aws::services::lambda::LambdaConcurrency>>,
) -> Vec<(String, String)> {
    let mut rows = vec![];

    rows.push(("Function Name".to_string(), func.function_name.clone()));
    rows.push(("ARN".to_string(), func.function_arn.clone()));
    rows.push(("".to_string(), "".to_string()));

    rows.push(("Runtime".to_string(), func.runtime.clone()));
    rows.push(("Handler".to_string(), func.handler.clone()));
    rows.push(("Memory".to_string(), format!("{} MB", func.memory_size)));
    rows.push(("Timeout".to_string(), format!("{} s", func.timeout)));
    rows.push(("Code Size".to_string(), func.formatted_code_size()));

    if !func.description.is_empty() {
        rows.push(("".to_string(), "".to_string()));
        rows.push(("Description".to_string(), func.description.clone()));
    }

    rows.push(("".to_string(), "".to_string()));
    rows.push(("IAM Role".to_string(), func.role.clone()));

    if !func.log_group.is_empty() {
        rows.push(("".to_string(), "".to_string()));
        rows.push(("Log Group".to_string(), func.log_group.clone()));
        rows.push(("  ".to_string(), "press t to tail".to_string()));
    }

    if func.vpc_id.is_some() || !func.subnet_ids.is_empty() {
        rows.push(("".to_string(), "".to_string()));
        rows.push(("VPC".to_string(), func.vpc_id.clone().unwrap_or_else(|| "-".to_string())));
        if !func.subnet_ids.is_empty() {
            rows.push(("Subnets".to_string(), func.subnet_ids.join(", ")));
        }
        if !func.security_group_ids.is_empty() {
            rows.push(("Security Groups".to_string(), func.security_group_ids.join(", ")));
        }
    } else {
        rows.push(("".to_string(), "".to_string()));
        rows.push(("VPC".to_string(), "Not attached".to_string()));
    }

    if !func.layers.is_empty() {
        rows.push(("".to_string(), "".to_string()));
        rows.push(("Layers".to_string(), format!("{} attached", func.layers.len())));
        let own = crate::aws::services::lambda::arn_account(&func.function_arn);
        let mut external = 0;
        for (i, arn) in func.layers.iter().enumerate() {
            rows.push((format!("  Layer {}", i + 1), arn.clone()));
            let owner = crate::aws::services::lambda::split_layer_version_arn(arn).map(|(_, a, _)| a);
            if owner.is_some() && owner != own {
                external += 1;
            }
        }
        if external > 0 {
            // Shared from another account (AWS-managed, a vendor's): not in
            // this account's ListLayers, so those rows have no jump.
            rows.push((
                "".to_string(),
                format!(
                    "· {external} external layer{} (another account) — not on the Layers tab",
                    if external == 1 { "" } else { "s" }
                ),
            ));
        }
    }

    rows.extend(lambda_concurrency_lines(func, conc_state));

    rows.push(("".to_string(), "".to_string()));
    rows
}

/// The Config section's Concurrency + Async Invocation groups. Split out
/// because it is the one lazy part of an otherwise eager section.
pub(super) fn lambda_concurrency_lines(
    func: &LambdaFunction,
    conc_state: Option<&crate::lazy::Lazy<crate::aws::services::lambda::LambdaConcurrency>>,
) -> Vec<(String, String)> {
    let mut rows = vec![("".to_string(), "".to_string())];
    rows.push(("Concurrency".to_string(), "".to_string()));

    let conc = match conc_state {
        None | Some(crate::lazy::Lazy::Loading) => {
            rows.push(("  Loading…".to_string(), "".to_string()));
            None
        }
        Some(crate::lazy::Lazy::Error(e)) => {
            rows.extend(error_rows(e));
            None
        }
        Some(crate::lazy::Lazy::Loaded(c)) => Some(c),
    };

    if let Some(c) = conc {
        match c.reserved {
            // Reserved 0 is a deliberate kill switch, not "unset" — it caps the
            // function at zero concurrent executions and throttles everything.
            Some(0) => rows.push((
                "  Reserved".to_string(),
                "⚠ 0 — every invocation is throttled".to_string(),
            )),
            Some(n) => rows.push((
                "  Reserved".to_string(),
                format!("{n} (carved out of the account pool)"),
            )),
            None => rows.push((
                "  Reserved".to_string(),
                "Not set — shares the account pool".to_string(),
            )),
        }

        if c.provisioned.is_empty() {
            rows.push(("  Provisioned".to_string(), "None".to_string()));
        } else {
            rows.push((
                "  Provisioned".to_string(),
                format!("{} allocation(s)", c.provisioned.len()),
            ));
            for p in &c.provisioned {
                let allocated = p
                    .allocated
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "—".to_string());
                let requested = p
                    .requested
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "—".to_string());
                let mark = if p.status.eq_ignore_ascii_case("READY") {
                    "✓"
                } else if p.status.eq_ignore_ascii_case("FAILED") {
                    "✗"
                } else {
                    "⚠"
                };
                rows.push((
                    format!("    {}", p.qualifier),
                    format!(
                        "{mark} {} · {allocated}/{requested} allocated",
                        p.status
                    ),
                ));
                if let Some(reason) = &p.status_reason {
                    rows.push((format!("      ⚠ {reason}"), String::new()));
                }
            }
        }
    }

    rows.push(("".to_string(), "".to_string()));
    rows.push(("Async Invocation".to_string(), "".to_string()));

    // Eager — the DLQ arrives on ListFunctions. Rendered before the lazy rows so
    // it is readable even while the fetch is in flight.
    if let Some(dlq) = &func.dead_letter_arn {
        rows.push(("  Dead Letter Queue".to_string(), dlq.clone()));
    }

    if let Some(c) = conc {
        if let Some(dest) = &c.on_failure {
            rows.push(("  On Failure".to_string(), dest.clone()));
        }
        if let Some(dest) = &c.on_success {
            rows.push(("  On Success".to_string(), dest.clone()));
        }

        // Lambda applies these whether or not an EventInvokeConfig exists; say
        // which are real settings and which are the service defaults.
        let (retries, age, suffix) = if c.has_event_invoke_config {
            (
                c.max_retry_attempts.unwrap_or(2),
                c.max_event_age_secs.unwrap_or(21_600),
                "",
            )
        } else {
            (2, 21_600, " (default)")
        };
        rows.push((
            "  Retry Attempts".to_string(),
            format!("{retries}{suffix}"),
        ));
        rows.push((
            "  Max Event Age".to_string(),
            format!("{}{suffix}", fmt_secs(age as i64)),
        ));

        if func.dead_letter_arn.is_none() && c.on_failure.is_none() {
            rows.push((
                "  · no DLQ or on-failure destination — events are dropped after the last retry"
                    .to_string(),
                String::new(),
            ));
        }

        for w in &c.warnings {
            rows.push((format!("  ⚠ {w}"), String::new()));
        }
    }

    rows
}

pub(super) fn lambda_env_lines(func: &LambdaFunction) -> Vec<(String, String)> {
    let mut rows = vec![("".to_string(), "".to_string())];

    if func.env_vars.is_empty() {
        rows.push(("  No environment variables".to_string(), "".to_string()));
    } else {
        let mut sorted: Vec<(&String, &String)> = func.env_vars.iter().collect();
        sorted.sort_by_key(|(k, _)| k.as_str());
        for (key, value) in sorted {
            let display_value = if value.len() > 80 {
                format!("{}…", &value[..80])
            } else {
                value.clone()
            };
            rows.push((format!("  {}", key), display_value));
        }
    }

    rows.push(("".to_string(), "".to_string()));
    rows
}

/// Tags come from the lazy `GetFunction` code info — `ListFunctions`
/// returns none, so `func.tags` is only populated by mocks/tests. The
/// section has the code-load on-enter hook, so viewing it fetches.
pub(super) fn lambda_tags_lines(
    func: &LambdaFunction,
    code_state: Option<&crate::lazy::Lazy<Box<crate::aws::services::lambda::LambdaCodeInfo>>>,
) -> Vec<(String, String)> {
    if !func.tags.is_empty() {
        return map_tags_lines(&func.tags);
    }
    match code_state {
        None | Some(crate::lazy::Lazy::Loading) => {
            vec![
                ("".to_string(), "".to_string()),
                ("  Loading tags…".to_string(), "".to_string()),
            ]
        }
        Some(crate::lazy::Lazy::Error(e)) => {
            let mut rows = vec![("".to_string(), "".to_string())];
            rows.extend(error_rows(e));
            rows
        }
        Some(crate::lazy::Lazy::Loaded(info)) => map_tags_lines(&info.tags),
    }
}
