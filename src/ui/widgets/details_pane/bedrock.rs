use super::*;

// ═══════════════════════════════════════════════════════════════════════════════
// Bedrock Guardrail — split pane (full policy detail)
// ═══════════════════════════════════════════════════════════════════════════════

pub(super) fn render_bedrock_guardrail_split(
    app: &App,
    g: &BedrockGuardrail,
    area: Rect,
    frame: &mut Frame,
) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 7, "");

    let mut block = theme::pane_block("Guardrail", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let header_lines = build_bedrock_guardrail_header_lines(g);
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
    render_bedrock_guardrail_section_tabs(app, chunks[2], frame);
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub(super) fn build_bedrock_guardrail_header_lines(g: &BedrockGuardrail) -> Vec<Line<'static>> {
    let mut lines = vec![Line::raw("")];

    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            g.guardrail_name.clone(),
            Style::default()
                .fg(crate::ui::theme::text_primary())
                .add_modifier(Modifier::BOLD),
        ),
    ]));

    let status_color = match g.status.to_uppercase().as_str() {
        "READY" => theme::success(),
        "FAILED" => theme::error(),
        "CREATING" | "UPDATING" => theme::warning(),
        "DELETING" => theme::error(),
        _ => theme::text_dim(),
    };
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(g.guardrail_id.clone(), Style::default().fg(theme::text_dim())),
        Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
        Span::styled(g.status.clone(), Style::default().fg(status_color)),
        Span::styled("  ·  v", Style::default().fg(theme::text_dim())),
        Span::styled(g.version.clone(), Style::default().fg(theme::text_dim())),
    ]));

    lines.push(Line::raw(""));
    lines
}

pub(super) fn render_bedrock_guardrail_section_tabs(app: &App, area: Rect, frame: &mut Frame) {
    
    let _cur = BedrockGuardrailDetailSection::from_index(app.detail_section_index());
    let tabs = descriptor_tabs(app, &crate::aws::services::bedrock::BEDROCK_GUARDRAIL_SECTIONS);
    render_section_tab_bar(app, area, frame, &tabs);
}

// ── Bedrock split panes ──────────────────────────────────────────────────────
pub(super) fn gr_hdr(s: impl Into<String>) -> (String, String) {
    (s.into(), String::new())
}
pub(super) fn gr_kv(k: impl Into<String>, v: impl Into<String>) -> (String, String) {
    (k.into(), v.into())
}
pub(super) fn gr_plain(s: impl Into<String>) -> (String, String) {
    (format!(" {}", s.into()), String::new())
}
pub(super) fn gr_blank() -> (String, String) {
    (String::new(), String::new())
}

/// Render a per-direction "STRENGTH  ACTION (off)" cell.
pub(super) fn gr_direction(strength: &str, action: &str, enabled: bool) -> String {
    let mut parts: Vec<String> = Vec::new();
    if !strength.is_empty() {
        parts.push(strength.to_string());
    }
    if !action.is_empty() {
        parts.push(action.to_string());
    }
    let mut s = parts.join("  ·  ");
    if s.is_empty() {
        s = "-".to_string();
    }
    if !enabled {
        s.push_str("  (off)");
    }
    s
}

pub fn bedrock_guardrail_section_lines(
    section: BedrockGuardrailDetailSection,
    detail: Option<&Lazy<Box<GuardrailFull>>>,
) -> Vec<(String, String)> {
    let full = match detail {
        None | Some(Lazy::Loading) => {
            return vec![gr_plain("Loading guardrail policy…")];
        }
        Some(Lazy::Error(e)) => {
            return error_rows(e);
        }
        Some(Lazy::Loaded(f)) => f.as_ref(),
    };
    match section {
        BedrockGuardrailDetailSection::Overview => guardrail_overview_lines(full),
        BedrockGuardrailDetailSection::ContentFilters => guardrail_content_filter_lines(full),
        BedrockGuardrailDetailSection::DeniedTopics => guardrail_topic_lines(full),
        BedrockGuardrailDetailSection::WordFilters => guardrail_word_lines(full),
        BedrockGuardrailDetailSection::SensitiveInfo => guardrail_sensitive_lines(full),
        BedrockGuardrailDetailSection::Grounding => guardrail_grounding_lines(full),
        BedrockGuardrailDetailSection::Advanced => guardrail_advanced_lines(full),
    }
}

pub(super) fn guardrail_overview_lines(f: &GuardrailFull) -> Vec<(String, String)> {
    let mut rows = vec![
        gr_kv("Name", f.name.clone()),
        gr_kv("Guardrail ID", f.id.clone()),
        gr_kv("ARN", f.arn.clone()),
        gr_kv("Status", f.status.clone()),
        gr_kv("Version", f.version.clone()),
    ];
    if !f.description.is_empty() {
        rows.push(gr_kv("Description", f.description.clone()));
    }
    rows.push(gr_blank());

    rows.push(gr_hdr("Policies"));
    rows.push(gr_kv("Content Filters", f.content_filters.len().to_string()));
    rows.push(gr_kv("Denied Topics", f.topics.len().to_string()));
    rows.push(gr_kv(
        "Word Filters",
        format!(
            "{} custom · {} managed",
            f.words.len(),
            f.managed_word_lists.len()
        ),
    ));
    rows.push(gr_kv(
        "Sensitive Info",
        format!("{} PII · {} regex", f.pii_entities.len(), f.regexes.len()),
    ));
    rows.push(gr_kv(
        "Contextual Grounding",
        f.grounding_filters.len().to_string(),
    ));
    if !f.automated_reasoning_policies.is_empty() {
        rows.push(gr_kv(
            "Automated Reasoning",
            f.automated_reasoning_policies.len().to_string(),
        ));
    }
    rows.push(gr_blank());

    if !f.created_at.is_empty() {
        rows.push(gr_kv("Created", f.created_at.clone()));
    }
    if !f.updated_at.is_empty() {
        rows.push(gr_kv("Updated", f.updated_at.clone()));
    }
    rows.push(gr_blank());
    rows
}

pub(super) fn guardrail_content_filter_lines(f: &GuardrailFull) -> Vec<(String, String)> {
    if f.content_filters.is_empty() {
        return vec![gr_plain("No content filters configured.")];
    }
    let mut rows = Vec::new();
    if !f.content_tier.is_empty() {
        rows.push(gr_kv("Tier", f.content_tier.clone()));
        rows.push(gr_blank());
    }
    for filter in &f.content_filters {
        rows.push(gr_hdr(filter.filter_type.clone()));
        rows.push(gr_kv(
            "Prompt (input)",
            gr_direction(&filter.input_strength, &filter.input_action, filter.input_enabled),
        ));
        rows.push(gr_kv(
            "Response (output)",
            gr_direction(&filter.output_strength, &filter.output_action, filter.output_enabled),
        ));
        if !filter.input_modalities.is_empty() || !filter.output_modalities.is_empty() {
            let mut mods = filter.input_modalities.clone();
            for m in &filter.output_modalities {
                if !mods.contains(m) {
                    mods.push(m.clone());
                }
            }
            rows.push(gr_kv("Modalities", mods.join(", ")));
        }
        rows.push(gr_blank());
    }
    rows
}

pub(super) fn guardrail_topic_lines(f: &GuardrailFull) -> Vec<(String, String)> {
    if f.topics.is_empty() {
        return vec![gr_plain("No denied topics configured.")];
    }
    let mut rows = Vec::new();
    if !f.topic_tier.is_empty() {
        rows.push(gr_kv("Tier", f.topic_tier.clone()));
        rows.push(gr_blank());
    }
    for t in &f.topics {
        rows.push(gr_hdr(t.name.clone()));
        if !t.definition.is_empty() {
            rows.push(gr_kv("Definition", t.definition.clone()));
        }
        let action = gr_direction("", &t.input_action, true);
        let out = gr_direction("", &t.output_action, true);
        if action != "-" || out != "-" {
            rows.push(gr_kv("Action", format!("in: {}   out: {}", action, out)));
        }
        for ex in &t.examples {
            rows.push(gr_plain(format!("• {}", ex)));
        }
        rows.push(gr_blank());
    }
    rows
}

pub(super) fn guardrail_word_lines(f: &GuardrailFull) -> Vec<(String, String)> {
    if f.words.is_empty() && f.managed_word_lists.is_empty() {
        return vec![gr_plain("No word filters configured.")];
    }
    let mut rows = Vec::new();

    if !f.managed_word_lists.is_empty() {
        rows.push(gr_hdr("Managed Word Lists"));
        for m in &f.managed_word_lists {
            rows.push(gr_kv(
                m.word_type.clone(),
                format!(
                    "in: {}   out: {}",
                    gr_direction("", &m.input_action, true),
                    gr_direction("", &m.output_action, true)
                ),
            ));
        }
        rows.push(gr_blank());
    }

    if !f.words.is_empty() {
        rows.push(gr_hdr(format!("Custom Words ({})", f.words.len())));
        for w in &f.words {
            rows.push(gr_plain(w.text.clone()));
        }
        rows.push(gr_blank());
    }
    rows
}

pub(super) fn guardrail_sensitive_lines(f: &GuardrailFull) -> Vec<(String, String)> {
    if f.pii_entities.is_empty() && f.regexes.is_empty() {
        return vec![gr_plain("No sensitive-information filters configured.")];
    }
    let mut rows = Vec::new();

    if !f.pii_entities.is_empty() {
        rows.push(gr_hdr(format!("PII Entities ({})", f.pii_entities.len())));
        for e in &f.pii_entities {
            rows.push(gr_kv(e.entity_type.clone(), e.action.clone()));
        }
        rows.push(gr_blank());
    }

    if !f.regexes.is_empty() {
        rows.push(gr_hdr(format!("Regex Patterns ({})", f.regexes.len())));
        for r in &f.regexes {
            rows.push(gr_hdr(r.name.clone()));
            if !r.description.is_empty() {
                rows.push(gr_kv("Description", r.description.clone()));
            }
            rows.push(gr_kv("Pattern", r.pattern.clone()));
            rows.push(gr_kv("Action", r.action.clone()));
            rows.push(gr_blank());
        }
    }
    rows
}

pub(super) fn guardrail_grounding_lines(f: &GuardrailFull) -> Vec<(String, String)> {
    if f.grounding_filters.is_empty() {
        return vec![gr_plain("No contextual-grounding filters configured.")];
    }
    let mut rows = Vec::new();
    for filter in &f.grounding_filters {
        rows.push(gr_hdr(filter.filter_type.clone()));
        rows.push(gr_kv("Threshold", format!("{:.2}", filter.threshold)));
        if !filter.action.is_empty() {
            rows.push(gr_kv("Action", filter.action.clone()));
        }
        rows.push(gr_kv(
            "Enabled",
            if filter.enabled { "Yes" } else { "No" }.to_string(),
        ));
        rows.push(gr_blank());
    }
    rows
}

pub(super) fn guardrail_advanced_lines(f: &GuardrailFull) -> Vec<(String, String)> {
    let mut rows = Vec::new();

    rows.push(gr_hdr("Blocked Messaging"));
    if !f.blocked_input_messaging.is_empty() {
        rows.push(gr_kv("Input", f.blocked_input_messaging.clone()));
    }
    if !f.blocked_outputs_messaging.is_empty() {
        rows.push(gr_kv("Output", f.blocked_outputs_messaging.clone()));
    }
    rows.push(gr_blank());

    if !f.kms_key_arn.is_empty() {
        rows.push(gr_hdr("Encryption"));
        rows.push(gr_kv("KMS Key", f.kms_key_arn.clone()));
        rows.push(gr_blank());
    }

    if !f.cross_region_profile.is_empty() {
        rows.push(gr_hdr("Cross-Region"));
        rows.push(gr_kv("Guardrail Profile", f.cross_region_profile.clone()));
        rows.push(gr_blank());
    }

    if !f.automated_reasoning_policies.is_empty() {
        rows.push(gr_hdr("Automated Reasoning Policies"));
        for p in &f.automated_reasoning_policies {
            rows.push(gr_plain(p.clone()));
        }
        rows.push(gr_blank());
    }

    if !f.status_reasons.is_empty() {
        rows.push(gr_hdr("Status Reasons"));
        for s in &f.status_reasons {
            rows.push(gr_plain(s.clone()));
        }
        rows.push(gr_blank());
    }

    if !f.failure_recommendations.is_empty() {
        rows.push(gr_hdr("Failure Recommendations"));
        for s in &f.failure_recommendations {
            rows.push(gr_plain(s.clone()));
        }
        rows.push(gr_blank());
    }

    if rows.is_empty() {
        return vec![gr_plain("No advanced configuration.")];
    }
    rows
}

// ═══════════════════════════════════════════════════════════════════════════════
// Bedrock Knowledge Base — split pane (config / vector store / data sources / ingestion)
// ═══════════════════════════════════════════════════════════════════════════════

pub(super) fn render_bedrock_kb_split(
    app: &App,
    kb: &BedrockKnowledgeBase,
    area: Rect,
    frame: &mut Frame,
) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 5, "");

    let mut block = theme::pane_block("Knowledge Base", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let header_lines = build_bedrock_kb_header_lines(kb);
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
    render_bedrock_kb_section_tabs(app, chunks[2], frame);
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub(super) fn build_bedrock_kb_header_lines(kb: &BedrockKnowledgeBase) -> Vec<Line<'static>> {
    let mut lines = vec![Line::raw("")];

    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            kb.kb_name.clone(),
            Style::default()
                .fg(crate::ui::theme::text_primary())
                .add_modifier(Modifier::BOLD),
        ),
    ]));

    let status_color = match kb.status.to_uppercase().as_str() {
        "ACTIVE" => theme::success(),
        "FAILED" => theme::error(),
        "CREATING" | "UPDATING" => theme::warning(),
        "DELETING" | "DELETE_UNSUCCESSFUL" => theme::error(),
        _ => theme::text_dim(),
    };
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(kb.kb_id.clone(), Style::default().fg(theme::text_dim())),
        Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
        Span::styled(kb.status.clone(), Style::default().fg(status_color)),
    ]));

    lines.push(Line::raw(""));
    lines
}

pub(super) fn render_bedrock_kb_section_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let tabs = descriptor_tabs(app, &crate::aws::services::bedrock::BEDROCK_KB_SECTIONS);
    render_section_tab_bar(app, area, frame, &tabs);
}

pub fn bedrock_kb_section_lines(
    section: BedrockKbDetailSection,
    detail: Option<&Lazy<Box<KnowledgeBaseFull>>>,
    data_sources: Option<&Lazy<Vec<KbDataSource>>>,
    ingestion: Option<&Lazy<Vec<KbIngestionJob>>>,
) -> Vec<(String, String)> {
    match section {
        BedrockKbDetailSection::Overview => with_kb(detail, kb_overview_lines),
        BedrockKbDetailSection::Configuration => with_kb(detail, kb_configuration_lines),
        BedrockKbDetailSection::VectorStore => with_kb(detail, kb_vector_store_lines),
        BedrockKbDetailSection::DataSources => kb_data_sources_lines(data_sources),
        BedrockKbDetailSection::Ingestion => kb_ingestion_lines(ingestion),
    }
}

/// Resolve the lazy KB detail, or return a loading/error placeholder.
pub(super) fn with_kb(
    detail: Option<&Lazy<Box<KnowledgeBaseFull>>>,
    f: fn(&KnowledgeBaseFull) -> Vec<(String, String)>,
) -> Vec<(String, String)> {
    match detail {
        None | Some(Lazy::Loading) => vec![gr_plain("Loading knowledge base…")],
        Some(Lazy::Error(e)) => error_rows(e),
        Some(Lazy::Loaded(kb)) => f(kb),
    }
}

pub(super) fn kb_overview_lines(kb: &KnowledgeBaseFull) -> Vec<(String, String)> {
    let mut rows = vec![
        gr_kv("Name", kb.name.clone()),
        gr_kv("Knowledge Base ID", kb.id.clone()),
        gr_kv("ARN", kb.arn.clone()),
        gr_kv("Status", kb.status.clone()),
    ];
    if !kb.kb_type.is_empty() {
        rows.push(gr_kv("Type", kb.kb_type.clone()));
    }
    if !kb.description.is_empty() {
        rows.push(gr_kv("Description", kb.description.clone()));
    }
    rows.push(gr_blank());
    if !kb.role_arn.is_empty() {
        rows.push(gr_kv("Service Role", kb.role_arn.clone()));
        rows.push(gr_blank());
    }
    if !kb.created_at.is_empty() {
        rows.push(gr_kv("Created", kb.created_at.clone()));
    }
    if !kb.updated_at.is_empty() {
        rows.push(gr_kv("Updated", kb.updated_at.clone()));
    }
    if !kb.failure_reasons.is_empty() {
        rows.push(gr_blank());
        rows.push(gr_hdr("Failure Reasons"));
        for r in &kb.failure_reasons {
            rows.push(gr_plain(r.clone()));
        }
    }
    rows.push(gr_blank());
    rows
}

pub(super) fn kb_configuration_lines(kb: &KnowledgeBaseFull) -> Vec<(String, String)> {
    let mut rows = Vec::new();
    rows.push(gr_kv(
        "KB Type",
        if kb.kb_type.is_empty() { "-".to_string() } else { kb.kb_type.clone() },
    ));
    rows.push(gr_blank());

    if kb.embedding_model_arn.is_empty() {
        rows.push(gr_plain("No vector embedding configuration."));
        return rows;
    }

    rows.push(gr_hdr("Embedding"));
    rows.push(gr_kv("Model", kb.embedding_model_arn.clone()));
    if let Some(dims) = kb.embedding_dimensions {
        rows.push(gr_kv("Dimensions", dims.to_string()));
    }
    if !kb.embedding_data_type.is_empty() {
        rows.push(gr_kv("Data Type", kb.embedding_data_type.clone()));
    }
    rows.push(gr_blank());
    rows
}

pub(super) fn kb_vector_store_lines(kb: &KnowledgeBaseFull) -> Vec<(String, String)> {
    if kb.storage_type.is_empty() {
        return vec![gr_plain("No vector store configured (managed / non-vector KB).")];
    }
    let mut rows = vec![gr_kv("Store Type", kb.storage_type.clone()), gr_blank()];
    for (k, v) in &kb.storage_detail {
        rows.push(gr_kv(k.clone(), v.clone()));
    }
    rows.push(gr_blank());
    rows
}

pub(super) fn kb_data_sources_lines(state: Option<&Lazy<Vec<KbDataSource>>>) -> Vec<(String, String)> {
    let sources = match state {
        None | Some(Lazy::Loading) => {
            return vec![gr_plain("Loading data sources…")];
        }
        Some(Lazy::Error(e)) => return error_rows(e),
        Some(Lazy::Loaded(s)) => s,
    };
    if sources.is_empty() {
        return vec![gr_plain("No data sources.")];
    }
    let mut rows = Vec::new();
    for ds in sources {
        rows.push(gr_hdr(ds.name.clone()));
        rows.push(gr_kv("Status", ds.status.clone()));
        if !ds.ds_type.is_empty() {
            rows.push(gr_kv("Type", ds.ds_type.clone()));
        }
        if !ds.description.is_empty() {
            rows.push(gr_kv("Description", ds.description.clone()));
        }
        for (k, v) in &ds.source_detail {
            rows.push(gr_kv(k.clone(), v.clone()));
        }
        if !ds.chunking.is_empty() {
            rows.push(gr_kv("Chunking", ds.chunking.clone()));
        }
        if !ds.data_deletion_policy.is_empty() {
            rows.push(gr_kv("On Delete", ds.data_deletion_policy.clone()));
        }
        rows.push(gr_kv("Data Source ID", ds.id.clone()));
        if !ds.updated_at.is_empty() {
            rows.push(gr_kv("Updated", ds.updated_at.clone()));
        }
        for r in &ds.failure_reasons {
            rows.push(gr_plain(format!("⚠ {}", r)));
        }
        rows.push(gr_blank());
    }
    rows
}

pub(super) fn kb_ingestion_lines(state: Option<&Lazy<Vec<KbIngestionJob>>>) -> Vec<(String, String)> {
    let jobs = match state {
        None | Some(Lazy::Loading) => {
            return vec![gr_plain("Loading ingestion jobs…")];
        }
        Some(Lazy::Error(e)) => return error_rows(e),
        Some(Lazy::Loaded(j)) => j,
    };
    if jobs.is_empty() {
        return vec![gr_plain("No ingestion jobs.")];
    }
    let mut rows = Vec::new();
    for j in jobs {
        let when = if j.started_at.is_empty() { "-".to_string() } else { j.started_at.clone() };
        rows.push(gr_hdr(format!("{}  ·  {}", when, j.status)));
        rows.push(gr_kv(
            "Docs",
            format!(
                "{} scanned · {} indexed · {} deleted · {} failed",
                j.docs_scanned, j.docs_indexed, j.docs_deleted, j.docs_failed
            ),
        ));
        if !j.updated_at.is_empty() {
            rows.push(gr_kv("Updated", j.updated_at.clone()));
        }
        rows.push(gr_kv("Data Source", j.data_source_id.clone()));
        rows.push(gr_kv("Job ID", j.job_id.clone()));
        rows.push(gr_blank());
    }
    rows
}

// ═══════════════════════════════════════════════════════════════════════════════
// Bedrock Agent — split pane (overview / action groups / aliases / knowledge bases)
// ═══════════════════════════════════════════════════════════════════════════════

pub(super) fn render_bedrock_agent_split(app: &App, agent: &BedrockAgent, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 4, "");

    let mut block = theme::pane_block("Agent", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let header_lines = build_bedrock_agent_header_lines(agent);
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
    render_bedrock_agent_section_tabs(app, chunks[2], frame);
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub(super) fn build_bedrock_agent_header_lines(agent: &BedrockAgent) -> Vec<Line<'static>> {
    let mut lines = vec![Line::raw("")];

    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            agent.agent_name.clone(),
            Style::default()
                .fg(crate::ui::theme::text_primary())
                .add_modifier(Modifier::BOLD),
        ),
    ]));

    let status_color = match agent.status.to_uppercase().as_str() {
        "PREPARED" => theme::success(),
        "FAILED" => theme::error(),
        "CREATING" | "PREPARING" | "UPDATING" | "VERSIONING" => theme::warning(),
        "DELETING" => theme::error(),
        _ => theme::text_dim(),
    };
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(agent.agent_id.clone(), Style::default().fg(theme::text_dim())),
        Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
        Span::styled(agent.status.clone(), Style::default().fg(status_color)),
    ]));

    lines.push(Line::raw(""));
    lines
}

pub(super) fn render_bedrock_agent_section_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let tabs = descriptor_tabs(app, &crate::aws::services::bedrock::BEDROCK_AGENT_SECTIONS);
    render_section_tab_bar(app, area, frame, &tabs);
}

pub fn bedrock_agent_section_lines(
    section: BedrockAgentDetailSection,
    detail: Option<&Lazy<Box<AgentFull>>>,
    action_groups: Option<&Lazy<Vec<AgentActionGroup>>>,
    aliases: Option<&Lazy<Vec<AgentAlias>>>,
    knowledge_bases: Option<&Lazy<Vec<AgentKb>>>,
) -> Vec<(String, String)> {
    match section {
        BedrockAgentDetailSection::Overview => match detail {
            None | Some(Lazy::Loading) => vec![gr_plain("Loading agent…")],
            Some(Lazy::Error(e)) => error_rows(e),
            Some(Lazy::Loaded(a)) => agent_overview_lines(a),
        },
        BedrockAgentDetailSection::ActionGroups => agent_action_group_lines(action_groups),
        BedrockAgentDetailSection::Aliases => agent_alias_lines(aliases),
        BedrockAgentDetailSection::KnowledgeBases => agent_kb_lines(knowledge_bases),
    }
}

pub(super) fn agent_overview_lines(a: &AgentFull) -> Vec<(String, String)> {
    let mut rows = vec![
        gr_kv("Name", a.name.clone()),
        gr_kv("Agent ID", a.id.clone()),
        gr_kv("ARN", a.arn.clone()),
        gr_kv("Status", a.status.clone()),
        gr_kv("Version", a.version.clone()),
    ];
    if !a.description.is_empty() {
        rows.push(gr_kv("Description", a.description.clone()));
    }
    rows.push(gr_blank());

    rows.push(gr_hdr("Model"));
    rows.push(gr_kv(
        "Foundation Model",
        if a.foundation_model.is_empty() { "-".to_string() } else { a.foundation_model.clone() },
    ));
    if !a.orchestration_type.is_empty() {
        rows.push(gr_kv("Orchestration", a.orchestration_type.clone()));
    }
    rows.push(gr_kv("Idle Session TTL", format!("{}s", a.idle_ttl_secs)));
    if !a.memory.is_empty() {
        rows.push(gr_kv("Memory", a.memory.clone()));
    }
    rows.push(gr_blank());

    if !a.guardrail_id.is_empty() {
        rows.push(gr_hdr("Guardrail"));
        rows.push(gr_kv("Identifier", a.guardrail_id.clone()));
        if !a.guardrail_version.is_empty() {
            rows.push(gr_kv("Version", a.guardrail_version.clone()));
        }
        rows.push(gr_blank());
    }

    rows.push(gr_hdr("Security"));
    if !a.role_arn.is_empty() {
        rows.push(gr_kv("Service Role", a.role_arn.clone()));
    }
    if !a.kms_key_arn.is_empty() {
        rows.push(gr_kv("KMS Key", a.kms_key_arn.clone()));
    }
    rows.push(gr_blank());

    if !a.instruction.is_empty() {
        rows.push(gr_hdr("Instruction"));
        for line in a.instruction.lines() {
            rows.push(gr_plain(line.to_string()));
        }
        rows.push(gr_blank());
    }

    if !a.created_at.is_empty() {
        rows.push(gr_kv("Created", a.created_at.clone()));
    }
    if !a.updated_at.is_empty() {
        rows.push(gr_kv("Updated", a.updated_at.clone()));
    }
    if !a.prepared_at.is_empty() {
        rows.push(gr_kv("Prepared", a.prepared_at.clone()));
    }
    if !a.failure_reasons.is_empty() {
        rows.push(gr_blank());
        rows.push(gr_hdr("Failure Reasons"));
        for r in &a.failure_reasons {
            rows.push(gr_plain(format!("⚠ {}", r)));
        }
    }
    rows.push(gr_blank());
    rows
}

pub(super) fn agent_action_group_lines(state: Option<&Lazy<Vec<AgentActionGroup>>>) -> Vec<(String, String)> {
    let groups = match state {
        None | Some(Lazy::Loading) => {
            return vec![gr_plain("Loading action groups…")];
        }
        Some(Lazy::Error(e)) => return error_rows(e),
        Some(Lazy::Loaded(g)) => g,
    };
    if groups.is_empty() {
        return vec![gr_plain("No action groups.")];
    }
    let mut rows = Vec::new();
    for g in groups {
        rows.push(gr_hdr(g.name.clone()));
        rows.push(gr_kv("State", g.state.clone()));
        if !g.description.is_empty() {
            rows.push(gr_kv("Description", g.description.clone()));
        }
        rows.push(gr_kv("ID", g.id.clone()));
        if !g.updated_at.is_empty() {
            rows.push(gr_kv("Updated", g.updated_at.clone()));
        }
        rows.push(gr_blank());
    }
    rows
}

pub(super) fn agent_alias_lines(state: Option<&Lazy<Vec<AgentAlias>>>) -> Vec<(String, String)> {
    let aliases = match state {
        None | Some(Lazy::Loading) => return vec![gr_plain("Loading aliases…")],
        Some(Lazy::Error(e)) => return error_rows(e),
        Some(Lazy::Loaded(a)) => a,
    };
    if aliases.is_empty() {
        return vec![gr_plain("No aliases.")];
    }
    let mut rows = Vec::new();
    for a in aliases {
        rows.push(gr_hdr(a.name.clone()));
        rows.push(gr_kv("Status", a.status.clone()));
        if !a.routed_versions.is_empty() {
            rows.push(gr_kv("Routes To", format!("v{}", a.routed_versions.join(", v"))));
        }
        if !a.description.is_empty() {
            rows.push(gr_kv("Description", a.description.clone()));
        }
        rows.push(gr_kv("Alias ID", a.id.clone()));
        if !a.updated_at.is_empty() {
            rows.push(gr_kv("Updated", a.updated_at.clone()));
        }
        rows.push(gr_blank());
    }
    rows
}

pub(super) fn agent_kb_lines(state: Option<&Lazy<Vec<AgentKb>>>) -> Vec<(String, String)> {
    let kbs = match state {
        None | Some(Lazy::Loading) => {
            return vec![gr_plain("Loading knowledge bases…")];
        }
        Some(Lazy::Error(e)) => return error_rows(e),
        Some(Lazy::Loaded(k)) => k,
    };
    if kbs.is_empty() {
        return vec![gr_plain("No associated knowledge bases.")];
    }
    let mut rows = Vec::new();
    for k in kbs {
        rows.push(gr_kv("Knowledge Base", k.kb_id.clone()));
        rows.push(gr_kv("State", k.state.clone()));
        if !k.description.is_empty() {
            rows.push(gr_kv("Description", k.description.clone()));
        }
        if !k.updated_at.is_empty() {
            rows.push(gr_kv("Updated", k.updated_at.clone()));
        }
        rows.push(gr_blank());
    }
    rows
}
