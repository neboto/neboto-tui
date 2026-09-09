use crate::app::{App, SesView};
use crate::ui::widgets::subtab_bar::render_subtab_bar;
use ratatui::{layout::Rect, Frame};

pub fn render_ses_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let v = app.ses_view;
    let tabs = [
        ('1', "Identities", v == SesView::Identities),
        ('2', "Configuration Sets", v == SesView::ConfigurationSets),
        ('3', "Suppression List", v == SesView::Suppression),
    ];
    render_subtab_bar(app, area, frame, &tabs);
}
