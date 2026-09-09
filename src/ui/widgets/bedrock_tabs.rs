use crate::app::{App, BedrockView};
use crate::ui::widgets::subtab_bar::render_subtab_bar;
use ratatui::{layout::Rect, Frame};

pub fn render_bedrock_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let views = [
        ('1', "Foundation Models", BedrockView::FoundationModels),
        ('2', "Inference Profiles", BedrockView::InferenceProfiles),
        ('3', "Guardrails", BedrockView::Guardrails),
        ('4', "Knowledge Bases", BedrockView::KnowledgeBases),
        ('5', "Agents", BedrockView::Agents),
        ('6', "Prompts", BedrockView::Prompts),
        ('7', "Flows", BedrockView::Flows),
        ('8', "Custom Models", BedrockView::CustomModels),
        ('9', "Imported Models", BedrockView::ImportedModels),
    ];

    let tabs: Vec<(char, &str, bool)> = views
        .iter()
        .map(|(key, label, view)| (*key, *label, app.bedrock_view == *view))
        .collect();

    render_subtab_bar(app, area, frame, &tabs);
}
