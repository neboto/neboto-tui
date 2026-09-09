use crate::app::{App, IamView};
use crate::ui::widgets::subtab_bar::render_subtab_bar;
use ratatui::{layout::Rect, Frame};

pub fn render_iam_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let v = app.iam_view;
    let tabs = [
        ('1', "Roles", v == IamView::Roles),
        ('2', "Policies", v == IamView::Policies),
        ('3', "Users", v == IamView::Users),
        ('4', "Groups", v == IamView::Groups),
        ('5', "Analyzer", v == IamView::AccessAnalyzer),
        ('6', "Providers", v == IamView::IdentityProviders),
        ('7', "Account", v == IamView::Account),
    ];
    render_subtab_bar(app, area, frame, &tabs);
}
