use crate::app::{AgentCoreView, App};
use crate::ui::widgets::subtab_bar::render_subtab_bar;
use ratatui::{layout::Rect, Frame};

pub fn render_agentcore_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let views = [
        ('1', "Runtimes", AgentCoreView::Runtimes),
        ('2', "Gateways", AgentCoreView::Gateways),
        ('3', "Memory", AgentCoreView::Memory),
        ('4', "Identity", AgentCoreView::Identity),
        ('5', "Tools", AgentCoreView::Tools),
        ('6', "Policy", AgentCoreView::Policy),
        ('7', "Evaluation", AgentCoreView::Evaluation),
        ('8', "Registry", AgentCoreView::Registry),
        ('9', "Harness", AgentCoreView::Harness),
        ('0', "Payments", AgentCoreView::Payments),
    ];

    let tabs: Vec<(char, &str, bool)> = views
        .iter()
        .map(|(key, label, view)| (*key, *label, app.agentcore_view == *view))
        .collect();

    render_subtab_bar(app, area, frame, &tabs);
}
