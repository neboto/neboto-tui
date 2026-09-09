use crate::app::{App, WafScope, WafView};
use crate::ui::theme;
use crate::ui::widgets::subtab_bar::subtab_bar_spans;
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

pub fn render_waf_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let v = app.waf_view;
    let tabs = [
        ('1', "Web ACLs", v == WafView::WebAcls),
        ('2', "IP Sets", v == WafView::IpSets),
        ('3', "Rule Groups", v == WafView::RuleGroups),
    ];
    let mut spans = subtab_bar_spans(app, area, &tabs);

    // Scope toggle (cycled with `t`) — same pattern as Cost's period selector.
    spans.push(Span::styled("     ", Style::default()));
    // Column where the scope toggle begins (cycled with `t` on click).
    let toggle_col = Line::from(spans.clone()).width() as u16;
    spans.push(Span::styled("t Scope ", Style::default().fg(theme::text_dim())));

    let scopes = [
        (WafScope::Regional, "Regional"),
        (WafScope::CloudFront, "CloudFront"),
    ];
    for (i, (scope, label)) in scopes.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(" ", Style::default()));
        }
        let active = app.waf_scope == *scope;
        let style = if active {
            Style::default()
                .fg(Color::Black)
                .bg(theme::aws_orange())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme::text_dim())
        };
        spans.push(Span::styled(format!(" {} ", label), style));
    }

    // Clicking anywhere on the scope toggle cycles it (`t`).
    let toggle_w = (Line::from(spans.clone()).width() as u16).saturating_sub(toggle_col);
    app.push_click_region(
        Rect {
            x: area.x + toggle_col,
            y: area.y,
            width: toggle_w,
            height: 1,
        },
        crate::app::ClickAction::Key('t'),
    );

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}
