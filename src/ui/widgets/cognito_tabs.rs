use crate::app::{App, CognitoView};
use crate::ui::widgets::subtab_bar::render_subtab_bar;
use ratatui::{layout::Rect, Frame};

pub fn render_cognito_tabs(app: &App, area: Rect, frame: &mut Frame) {
    let v = app.cognito_view;
    let tabs = [
        ('1', "User Pools", v == CognitoView::UserPools),
        ('2', "Identity Pools", v == CognitoView::IdentityPools),
        ('3', "Users", v == CognitoView::Users),
    ];
    render_subtab_bar(app, area, frame, &tabs);
}
