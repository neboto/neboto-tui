use crate::app::{App, SsmView};
use crate::ui::widgets::subtab_bar::render_subtab_bar;
use ratatui::{layout::Rect, Frame};

pub fn render_ssm_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let v = app.ssm_view;
    let tabs = [
        ('1', "Parameters", v == SsmView::Parameters),
        ('2', "Documents", v == SsmView::Documents),
        ('3', "Fleet", v == SsmView::Fleet),
        ('4', "Patch", v == SsmView::Patch),
        ('5', "Associations", v == SsmView::Associations),
        ('6', "Run Command", v == SsmView::RunCommand),
        ('7', "Automation", v == SsmView::Automation),
        ('8', "Maint Windows", v == SsmView::MaintWindows),
        ('9', "OpsItems", v == SsmView::OpsItems),
        ('0', "Sessions", v == SsmView::Sessions),
    ];
    render_subtab_bar(app, area, frame, &tabs);
}
