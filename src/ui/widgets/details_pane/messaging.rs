use super::*;

// ── SQS Queue split pane ──────────────────────────────────────────────────

pub(super) fn render_sqs_queue_split(app: &App, queue: &SqsQueue, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 4, "");

    let mut block = theme::pane_block("SQS Queue", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let header_lines = build_sqs_header_lines(queue);
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
    render_sqs_section_tabs(app, chunks[2], frame);
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub(super) fn build_sqs_header_lines(queue: &SqsQueue) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = vec![Line::raw("")];

    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            queue.name.clone(),
            Style::default()
                .fg(crate::ui::theme::text_primary())
                .add_modifier(Modifier::BOLD),
        ),
        if queue.is_dlq() {
            Span::styled(
                "  [DLQ]",
                Style::default()
                    .fg(theme::warning())
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            Span::raw("")
        },
    ]));

    lines.push(Line::raw(""));
    lines.push(header_kv(
        "Type",
        if queue.is_fifo { "FIFO" } else { "Standard" },
    ));
    lines.push(header_kv("Available", &queue.messages_available.to_string()));
    lines.push(header_kv("In Flight", &queue.messages_in_flight.to_string()));
    lines.push(header_kv("Delayed", &queue.messages_delayed.to_string()));
    lines.push(header_kv(
        "Encryption",
        if queue.kms_key_id.is_some() {
            "SSE-KMS"
        } else if queue.sse_sqs {
            "SSE-SQS"
        } else {
            "None"
        },
    ));

    lines.push(Line::raw(""));
    lines
}

pub(super) fn render_sqs_section_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let tabs = descriptor_tabs(app, &crate::aws::services::messaging::SQS_QUEUE_SECTIONS);
    render_section_tab_bar(app, area, frame, &tabs);
}

pub fn sqs_queue_section_lines(
    queue: &SqsQueue,
    section: SqsQueueDetailSection,
) -> Vec<(String, String)> {
    match section {
        SqsQueueDetailSection::Attributes => sqs_attributes_lines(queue),
        SqsQueueDetailSection::Redrive => sqs_redrive_lines(queue),
        SqsQueueDetailSection::Permissions => sqs_permissions_lines(queue),
        SqsQueueDetailSection::Tags => sqs_tags_lines(queue),
    }
}

pub(super) fn sqs_attributes_lines(queue: &SqsQueue) -> Vec<(String, String)> {
    let mut rows = vec![];

    rows.push(("Name".to_string(), queue.name.clone()));
    rows.push((
        "Type".to_string(),
        if queue.is_fifo { "FIFO".to_string() } else { "Standard".to_string() },
    ));
    if !queue.arn.is_empty() {
        rows.push(("ARN".to_string(), queue.arn.clone()));
    }

    rows.push(("".to_string(), "".to_string()));
    rows.push(("Messages".to_string(), "".to_string()));
    rows.push(("  Available".to_string(), queue.messages_available.to_string()));
    rows.push(("  In Flight".to_string(), queue.messages_in_flight.to_string()));
    rows.push(("  Delayed".to_string(), queue.messages_delayed.to_string()));
    rows.push(("  Total".to_string(), queue.total_messages().to_string()));

    rows.push(("".to_string(), "".to_string()));
    if let Some(vt) = queue.visibility_timeout {
        rows.push(("Visibility Timeout".to_string(), fmt_secs(vt)));
    }
    if let Some(ret) = queue.message_retention_secs {
        rows.push(("Retention".to_string(), fmt_secs(ret)));
    }
    if let Some(delay) = queue.delay_seconds {
        rows.push(("Delivery Delay".to_string(), fmt_secs(delay)));
    }
    if let Some(wait) = queue.receive_wait_secs {
        rows.push((
            "Receive Wait".to_string(),
            if wait == 0 { "0s (short poll)".to_string() } else { fmt_secs(wait) },
        ));
    }
    if let Some(size) = queue.max_message_size {
        rows.push(("Max Message Size".to_string(), format!("{} KiB", size / 1024)));
    }
    if let Some(dedup) = queue.content_based_dedup {
        rows.push((
            "Content Dedup".to_string(),
            if dedup { "On".to_string() } else { "Off".to_string() },
        ));
    }

    rows.push(("".to_string(), "".to_string()));
    match &queue.kms_key_id {
        Some(kms) => {
            rows.push(("Encryption".to_string(), "SSE-KMS".to_string()));
            rows.push(("  KMS Key".to_string(), kms.clone())); // arn/alias → jumpable
        }
        None if queue.sse_sqs => rows.push(("Encryption".to_string(), "SSE-SQS".to_string())),
        None => rows.push(("Encryption".to_string(), "None".to_string())),
    }
    rows.push((
        "Access Policy".to_string(),
        if queue.policy.is_some() {
            "✓ set (see Permissions)".to_string()
        } else {
            "none".to_string()
        },
    ));

    if !queue.created.is_empty() {
        rows.push(("".to_string(), "".to_string()));
        rows.push(("Created".to_string(), queue.created.clone()));
    }
    if !queue.last_modified.is_empty() {
        rows.push(("Last Modified".to_string(), queue.last_modified.clone()));
    }

    rows.push(("".to_string(), "".to_string()));
    rows
}

pub(super) fn sqs_permissions_lines(queue: &SqsQueue) -> Vec<(String, String)> {
    let mut rows = vec![("".to_string(), "".to_string())];
    match queue.pretty_policy() {
        None => {
            rows.push(("  No access policy".to_string(), "".to_string()));
        }
        Some(pretty) => {
            // Pretty-print the JSON policy as plain content lines; `e`
            // opens the raw document in $EDITOR.
            rows.push(("Access Policy (e to edit)".to_string(), "".to_string()));
            rows.push(("".to_string(), "".to_string()));
            for line in pretty.lines() {
                rows.push((format!("  {}", line), "".to_string()));
            }
        }
    }
    rows.push(("".to_string(), "".to_string()));
    rows
}

pub(super) fn sqs_redrive_lines(queue: &SqsQueue) -> Vec<(String, String)> {
    let mut rows = vec![("".to_string(), "".to_string())];

    // This queue's dead-letter target (where its failed messages go).
    if let Some(dlq) = &queue.dlq_target_arn {
        rows.push(("Dead-Letter Queue".to_string(), "".to_string()));
        let dlq_name = dlq.rsplit(':').next().unwrap_or(dlq);
        rows.push(("  Target".to_string(), dlq_name.to_string()));
        if let Some(max) = queue.max_receive_count {
            rows.push(("  Max Receives".to_string(), max.to_string()));
        }
        rows.push(("  Target ARN".to_string(), dlq.clone()));
    } else {
        rows.push((
            "  No dead-letter queue configured".to_string(),
            "".to_string(),
        ));
    }

    // Whether this queue is itself a DLQ for other queues.
    if queue.is_dlq() {
        rows.push(("".to_string(), "".to_string()));
        rows.push(("This Queue is a DLQ".to_string(), "".to_string()));
        if let Some(perm) = &queue.redrive_permission {
            rows.push(("  Permission".to_string(), perm.clone()));
        }
        if queue.source_queue_arns.is_empty() {
            rows.push(("  Sources".to_string(), "all queues in account".to_string()));
        } else {
            rows.push((
                "  Sources".to_string(),
                format!("{} queue(s)", queue.source_queue_arns.len()),
            ));
            for src in &queue.source_queue_arns {
                // Full ARN as the value so it's Enter-jumpable to the source queue.
                rows.push(("  Source".to_string(), src.clone()));
            }
        }
    }

    rows.push(("".to_string(), "".to_string()));
    rows
}

pub(super) fn sqs_tags_lines(queue: &SqsQueue) -> Vec<(String, String)> {
    let mut rows = vec![("".to_string(), "".to_string())];

    if queue.tags.is_empty() {
        rows.push(("  No tags".to_string(), "".to_string()));
    } else {
        let mut sorted: Vec<(&String, &String)> = queue.tags.iter().collect();
        sorted.sort_by_key(|(k, _)| k.as_str());
        for (key, value) in sorted {
            rows.push((format!("  {}", key), value.clone()));
        }
    }

    rows.push(("".to_string(), "".to_string()));
    rows
}

// ── SNS Topic split pane ──────────────────────────────────────────────────

pub(super) fn render_sns_topic_split(app: &App, topic: &SnsTopic, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 4, "");

    let mut block = theme::pane_block("SNS Topic", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let header_lines = build_sns_header_lines(topic);
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
    render_sns_section_tabs(app, chunks[2], frame);
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub(super) fn build_sns_header_lines(topic: &SnsTopic) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = vec![Line::raw("")];

    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            topic.name.clone(),
            Style::default()
                .fg(crate::ui::theme::text_primary())
                .add_modifier(Modifier::BOLD),
        ),
    ]));

    lines.push(Line::raw(""));
    lines.push(header_kv(
        "Type",
        if topic.is_fifo { "FIFO" } else { "Standard" },
    ));
    lines.push(header_kv(
        "Subscriptions",
        &topic.subscriptions_confirmed.to_string(),
    ));
    if topic.subscriptions_pending > 0 {
        lines.push(header_kv("Pending", &topic.subscriptions_pending.to_string()));
    }
    lines.push(header_kv(
        "Encryption",
        if topic.kms_key_id.is_some() { "SSE-KMS" } else { "None" },
    ));

    lines.push(Line::raw(""));
    lines
}

pub(super) fn render_sns_section_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let tabs = descriptor_tabs(app, &crate::aws::services::messaging::SNS_TOPIC_SECTIONS);
    render_section_tab_bar(app, area, frame, &tabs);
}

pub fn sns_topic_section_lines(
    topic: &SnsTopic,
    section: SnsTopicDetailSection,
    subs_state: Option<&crate::lazy::Lazy<Vec<crate::aws::services::messaging::SnsSubscription>>>,
) -> Vec<(String, String)> {
    match section {
        SnsTopicDetailSection::Config => sns_config_lines(topic),
        SnsTopicDetailSection::Subscriptions => sns_subscriptions_lines(subs_state),
        SnsTopicDetailSection::Permissions => sns_permissions_lines(topic),
        SnsTopicDetailSection::Tags => sns_tags_lines(topic),
    }
}

pub(super) fn sns_subscriptions_lines(state: Option<&crate::lazy::Lazy<Vec<crate::aws::services::messaging::SnsSubscription>>>) -> Vec<(String, String)> {
    let mut rows = vec![("".to_string(), "".to_string())];

    match state {
        None | Some(crate::lazy::Lazy::Loading) => {
            rows.push(("  Loading subscriptions…".to_string(), "".to_string()));
        }
        Some(crate::lazy::Lazy::Loaded(subs)) => {
            if subs.is_empty() {
                rows.push(("  No subscriptions".to_string(), "".to_string()));
            } else {
                for sub in subs {
                    let status = if sub.confirmed { "confirmed" } else { "PENDING" };
                    rows.push((sub.protocol.clone(), status.to_string()));
                    if !sub.endpoint.is_empty() {
                        // Endpoint as the *value* so an ARN endpoint (lambda/sqs/
                        // sns) is Enter-jumpable; non-ARN endpoints (email/https)
                        // simply render without a jump arrow.
                        rows.push(("  Endpoint".to_string(), sub.endpoint.clone()));
                    }
                }
            }
        }
        Some(crate::lazy::Lazy::Error(e)) => {
            rows.extend(error_rows(e));
        }
    }

    rows.push(("".to_string(), "".to_string()));
    rows
}

pub(super) fn sns_config_lines(topic: &SnsTopic) -> Vec<(String, String)> {
    let mut rows = vec![];

    rows.push(("Name".to_string(), topic.name.clone()));
    rows.push((
        "Type".to_string(),
        if topic.is_fifo { "FIFO".to_string() } else { "Standard".to_string() },
    ));
    if let Some(display) = &topic.display_name {
        rows.push(("Display Name".to_string(), display.clone()));
    }
    rows.push(("ARN".to_string(), topic.arn.clone()));
    if let Some(owner) = &topic.owner {
        rows.push(("Owner".to_string(), owner.clone()));
    }

    rows.push(("".to_string(), "".to_string()));
    rows.push(("Subscriptions".to_string(), "".to_string()));
    rows.push(("  Confirmed".to_string(), topic.subscriptions_confirmed.to_string()));
    rows.push(("  Pending".to_string(), topic.subscriptions_pending.to_string()));
    rows.push(("  Deleted".to_string(), topic.subscriptions_deleted.to_string()));

    rows.push(("".to_string(), "".to_string()));
    match &topic.kms_key_id {
        Some(k) => {
            rows.push(("Encryption".to_string(), "SSE-KMS".to_string()));
            rows.push(("  KMS Key".to_string(), k.clone())); // arn/alias → jumpable
        }
        None => rows.push(("Encryption".to_string(), "None".to_string())),
    }
    rows.push((
        "Access Policy".to_string(),
        if topic.policy.is_some() {
            "✓ set (see Permissions)".to_string()
        } else {
            "none".to_string()
        },
    ));

    if let Some(policy) = &topic.effective_delivery_policy {
        rows.push(("".to_string(), "".to_string()));
        rows.push(("Delivery Policy".to_string(), "".to_string()));
        // Pretty-print the JSON if it parses; emit each line as plain content.
        let pretty = serde_json::from_str::<serde_json::Value>(policy)
            .ok()
            .and_then(|v| serde_json::to_string_pretty(&v).ok())
            .unwrap_or_else(|| policy.clone());
        for line in pretty.lines() {
            rows.push((format!("  {}", line), "".to_string()));
        }
    }

    rows.push(("".to_string(), "".to_string()));
    rows
}

pub(super) fn sns_permissions_lines(topic: &SnsTopic) -> Vec<(String, String)> {
    let mut rows = vec![("".to_string(), "".to_string())];
    match topic.pretty_policy() {
        None => {
            rows.push(("  No access policy".to_string(), "".to_string()));
        }
        Some(pretty) => {
            // Pretty-print the JSON policy as plain content lines; `e`
            // opens the raw document in $EDITOR.
            rows.push(("Access Policy (e to edit)".to_string(), "".to_string()));
            rows.push(("".to_string(), "".to_string()));
            for line in pretty.lines() {
                rows.push((format!("  {}", line), "".to_string()));
            }
        }
    }
    rows.push(("".to_string(), "".to_string()));
    rows
}

pub(super) fn sns_tags_lines(topic: &SnsTopic) -> Vec<(String, String)> {
    if let Some(err) = &topic.tags_error {
        return error_rows(err);
    }
    let mut rows = vec![("".to_string(), "".to_string())];

    if topic.tags.is_empty() {
        rows.push(("  No tags".to_string(), "".to_string()));
    } else {
        let mut sorted: Vec<(&String, &String)> = topic.tags.iter().collect();
        sorted.sort_by_key(|(k, _)| k.as_str());
        for (key, value) in sorted {
            rows.push((format!("  {}", key), value.clone()));
        }
    }

    rows.push(("".to_string(), "".to_string()));
    rows
}
