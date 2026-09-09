use crate::app::App;
use crate::ui::theme;
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

/// S3's top row is a two-mode toggle, not a resource sub-tab row: **Bucket**
/// (the detail pane, whose six setting views are in-pane sections 1–6) and
/// **▸ Objects** (the file browser, keyed `o`, which owns the pane while open).
pub fn render_s3_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let browsing = app.s3_object_browser.visible;

    let mut spans: Vec<Span> = vec![Span::raw(" ")];

    let chip = |spans: &mut Vec<Span>, key: char, label: String, active: bool| {
        if active {
            spans.push(Span::styled(
                key.to_string(),
                Style::default().fg(theme::aws_orange()).add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::styled(
                format!(" {} ", label),
                Style::default()
                    .fg(Color::Black)
                    .bg(theme::aws_orange())
                    .add_modifier(Modifier::BOLD),
            ));
        } else {
            spans.push(Span::styled(key.to_string(), Style::default().fg(theme::accent())));
            spans.push(Span::styled(format!(" {} ", label), Style::default().fg(crate::ui::theme::text_muted())));
        }
    };

    // Bucket mode is active whenever the browser isn't open. It's a passive
    // state indicator (you're already here / `Esc` returns from Objects), so
    // it's not clickable — only the Objects chip is. The leading-space chip
    // glyph mirrors the keyed chips but isn't bound to a key.
    let bucket_label = "Bucket".to_string();
    chip(&mut spans, ' ', bucket_label.clone(), !browsing);
    spans.push(Span::styled(" │ ", Style::default().fg(theme::text_dim())));
    // Objects mode (▸ signals it's an interactive view), keyed `o`.
    let objects_label = "▸ Objects".to_string();
    chip(&mut spans, 'o', objects_label.clone(), browsing);

    // Only the Objects chip is clickable (key `o`). Offset past the leading
    // space + the (key + " Bucket ") chip + the " │ " separator.
    let bucket_chip_w = 1 + bucket_label.chars().count() + 2;
    let leading = 1 + bucket_chip_w + 3;
    app.record_subtab_regions(area, leading as u16, vec![('o', objects_label.chars().count())]);

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}
