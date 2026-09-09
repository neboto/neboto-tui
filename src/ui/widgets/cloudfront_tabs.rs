use crate::app::{App, CloudFrontView};
use crate::ui::widgets::subtab_bar::render_subtab_bar;
use ratatui::{layout::Rect, Frame};

pub fn render_cloudfront_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let v = app.cf_view;
    let tabs = [
        ('1', "Distributions", v == CloudFrontView::Distributions),
        ('2', "Functions", v == CloudFrontView::Functions),
        ('3', "Policies", v == CloudFrontView::Policies),
        ('4', "OACs", v == CloudFrontView::Oacs),
    ];
    render_subtab_bar(app, area, frame, &tabs);
}
