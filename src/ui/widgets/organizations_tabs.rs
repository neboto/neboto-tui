use crate::app::{App, OrgView};
use crate::ui::widgets::subtab_bar::render_subtab_bar;
use ratatui::{layout::Rect, Frame};

pub fn render_org_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let v = app.org_view;
    let tabs = [
        ('1', "Overview", v == OrgView::Overview),
        ('2', "Accounts", v == OrgView::Accounts),
        ('3', "OUs", v == OrgView::Ous),
        ('4', "Policies", v == OrgView::Policies),
        ('5', "Delegated", v == OrgView::Delegated),
        ('6', "Trusted", v == OrgView::Trusted),
    ];
    render_subtab_bar(app, area, frame, &tabs);
}
