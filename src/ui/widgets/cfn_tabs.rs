use crate::app::{App, CfnView};
use crate::ui::widgets::subtab_bar::render_subtab_bar;
use ratatui::{layout::Rect, Frame};

pub fn render_cfn_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let v = app.cfn_view;
    let tabs = [
        ('1', "Stacks", v == CfnView::Stacks),
        ('2', "StackSets", v == CfnView::StackSets),
        ('3', "Exports", v == CfnView::Exports),
        ('4', "Deleted", v == CfnView::Deleted),
    ];
    render_subtab_bar(app, area, frame, &tabs);
}
