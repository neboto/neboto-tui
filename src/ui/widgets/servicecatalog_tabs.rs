use crate::app::{App, ScView};
use crate::ui::widgets::subtab_bar::render_subtab_bar;
use ratatui::{layout::Rect, Frame};

pub fn render_servicecatalog_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let views = [
        ('1', "Portfolios", ScView::Portfolios),
        ('2', "Products", ScView::Products),
        ('3', "Provisioned", ScView::ProvisionedProducts),
        ('4', "TagOptions", ScView::TagOptions),
    ];

    let tabs: Vec<(char, &str, bool)> = views
        .iter()
        .map(|(key, label, view)| (*key, *label, app.sc_view == *view))
        .collect();

    render_subtab_bar(app, area, frame, &tabs);
}
