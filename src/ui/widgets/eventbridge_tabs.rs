use crate::app::{App, EventBridgeView};
use crate::ui::widgets::subtab_bar::render_subtab_bar;
use ratatui::{layout::Rect, Frame};

pub fn render_eventbridge_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let v = app.eventbridge_view;
    let tabs = [
        ('1', "Rules", v == EventBridgeView::Rules),
        ('2', "Event Buses", v == EventBridgeView::EventBuses),
        ('3', "Archives", v == EventBridgeView::Archives),
        ('4', "Replays", v == EventBridgeView::Replays),
        ('5', "Schedules", v == EventBridgeView::Schedules),
        ('6', "Pipes", v == EventBridgeView::Pipes),
    ];
    render_subtab_bar(app, area, frame, &tabs);
}
