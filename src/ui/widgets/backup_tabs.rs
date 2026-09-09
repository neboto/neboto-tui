use crate::app::{App, BackupView};
use crate::ui::theme;
use crate::ui::widgets::subtab_bar::subtab_bar_spans;
use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

pub fn render_backup_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let v = app.backup_view;
    let tabs = [
        ('1', "Vaults", v == BackupView::Vaults),
        ('2', "Plans", v == BackupView::Plans),
        ('3', "Protected", v == BackupView::ProtectedResources),
        ('4', "Jobs", v == BackupView::Jobs),
    ];
    let mut spans = subtab_bar_spans(app, area, &tabs);

    // Jobs window hint at the right.
    spans.push(Span::styled("     ", Style::default()));
    spans.push(Span::styled(
        "jobs: last 7d",
        Style::default().fg(theme::text_dim()),
    ));
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}
