//! Organizations member-account switch discoverability (issue #86): the `s`
//! hint shows in the status bar and in the unfocused account preview, only
//! for accounts `s` can actually switch into.

use super::*;
use crate::aws::services::organizations::OrgAccount;

fn account(id: &str, status: &str) -> Box<dyn Resource> {
    Box::new(OrgAccount::from_sdk(
        &aws_sdk_organizations::types::Account::builder()
            .id(id)
            .arn(format!("arn:aws:organizations::111111111111:account/o-mock/{id}"))
            .name(format!("acct-{id}"))
            .email(format!("{id}@example.com"))
            .status(aws_sdk_organizations::types::AccountStatus::from(status))
            .build(),
    ))
}

fn screen(app: &App) -> String {
    let mut terminal = Terminal::new(TestBackend::new(200, 40)).unwrap();
    terminal.draw(|f| crate::render_app(app, f)).unwrap();
    let buf = terminal.backend().buffer().clone();
    (0..buf.area.height)
        .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

fn hint_rows(app: &App) -> Vec<String> {
    app.get_detail_lines()
        .into_iter()
        .filter(|(_, v)| v.contains("press s"))
        .map(|(_, v)| v)
        .collect()
}

#[tokio::test]
async fn active_member_account_advertises_s_in_bar_and_preview() {
    let (mut app, _tx, _rx) = test_app().await;
    app.org_view = crate::app::OrgView::Accounts;
    app.account_id = Some("111111111111".to_string());
    select_mock(&mut app, ServiceType::Organizations, account("222222222222", "ACTIVE"));

    assert!(app.supports_org_assume());
    assert!(!app.supports_session(), "the session hint must not double up");
    assert_eq!(
        hint_rows(&app),
        vec!["· press s to assume OrganizationAccountAccessRole in this account".to_string()]
    );
    let s = screen(&app);
    assert!(s.contains("assume role"), "status bar hint missing:\n{s}");
    assert!(s.contains("press s to assume"), "preview hint missing:\n{s}");

    // Detail pane: the status bar keeps offering it.
    app.details_focused = true;
    assert!(screen(&app).contains("assume role"));
}

#[tokio::test]
async fn several_roles_name_the_picker_and_last_used() {
    let (mut app, _tx, _rx) = test_app().await;
    app.org_access_roles = vec!["OrganizationAccountAccessRole".into(), "AWSControlTowerExecution".into()];
    app.org_access_role = "AWSControlTowerExecution".into();
    select_mock(&mut app, ServiceType::Organizations, account("222222222222", "ACTIVE"));
    assert_eq!(
        hint_rows(&app),
        vec!["· press s to pick a role to assume in this account (last used: AWSControlTowerExecution)".to_string()]
    );
}

#[tokio::test]
async fn suspended_or_current_account_gets_no_hint() {
    let (mut app, _tx, _rx) = test_app().await;
    app.account_id = Some("111111111111".to_string());

    select_mock(&mut app, ServiceType::Organizations, account("222222222222", "SUSPENDED"));
    assert!(!app.supports_org_assume());
    assert!(hint_rows(&app).is_empty());

    select_mock(&mut app, ServiceType::Organizations, account("111111111111", "ACTIVE"));
    assert!(!app.supports_org_assume());
    assert!(hint_rows(&app).is_empty());
}

#[tokio::test]
async fn export_strips_the_hint_row() {
    let (mut app, _tx, _rx) = test_app().await;
    select_mock(&mut app, ServiceType::Organizations, account("222222222222", "ACTIVE"));
    let lines = app.get_detail_lines();
    assert!(lines.iter().any(|(_, v)| v.contains("press s")), "pane should carry the hint");
    let json = crate::export::detail_value_for_test(&lines).to_string();
    assert!(json.contains("222222222222"));
    assert!(!json.contains("press s"), "hint leaked into export: {json}");
}
