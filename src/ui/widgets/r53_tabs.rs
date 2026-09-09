use crate::app::{App, R53View};
use crate::ui::theme;
use crate::ui::widgets::subtab_bar::subtab_bar_spans;
use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

pub fn render_r53_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let v = app.r53_view;
    let tabs = [
        ('1', "Zones", v == R53View::Zones),
        ('2', "Records", v == R53View::Records),
        ('3', "Health Checks", v == R53View::HealthChecks),
    ];
    let mut spans = subtab_bar_spans(app, area, &tabs);

    // Records: say what the cross-zone list covers — zones still loading and
    // zones the budget skipped — so a missing record has an explanation.
    if v == R53View::Records {
        let loading = app.lazy.r53_zone_records.loading_count();
        let (big, budget) = app.r53_records_skipped;
        let mut notes: Vec<String> = Vec::new();
        if loading > 0 {
            notes.push(format!("loading {} zone(s)", loading));
        }
        if big > 0 {
            notes.push(format!("{} zone(s) over the record budget", big));
        }
        if budget > 0 {
            notes.push(format!("{} zone(s) past the zone budget", budget));
        }
        if !notes.is_empty() {
            spans.push(Span::styled(
                format!("    · {}", notes.join(" · ")),
                Style::default().fg(theme::text_dim()),
            ));
        }
    }

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}
