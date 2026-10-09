use crate::app::{App, LambdaView};
use crate::ui::widgets::subtab_bar::render_subtab_bar;
use ratatui::{layout::Rect, Frame};

pub fn render_lambda_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let v = app.lambda_view;
    let tabs = [
        ('1', "Functions", v == LambdaView::Functions),
        ('2', "Layers", v == LambdaView::Layers),
    ];
    render_subtab_bar(app, area, frame, &tabs);
}
