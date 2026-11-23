//! The info card (`i`, `src/cards.rs`): it opens over a tab that has one,
//! from either pane, draws the card, copies blocks and numbered links,
//! opens the link under the cursor, and closes on `Esc`. A tab without a
//! card says so. Ported from nebaz's `info_card_test.rs`.

use super::*;
use crate::app::Ec2View;

fn render_to_string(app: &App) -> String {
    let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
    terminal.draw(|f| crate::render_app(app, f)).unwrap();
    let buf = terminal.backend().buffer();
    (0..buf.area.height)
        .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

fn mock(label: &str) -> (ServiceType, Box<dyn crate::aws::resource::Resource>) {
    all_mocks().into_iter().find(|(_, l, _)| *l == label).map(|(s, _, r)| (s, r)).unwrap()
}

async fn on_security_groups() -> (App, mpsc::UnboundedSender<Event>, mpsc::UnboundedReceiver<Event>) {
    let (mut app, tx, rx) = test_app().await;
    let (svc, sg) = mock("SecurityGroup");
    select_mock(&mut app, svc, sg);
    app.ec2_view = Ec2View::SecurityGroups;
    (app, tx, rx)
}

fn toast(app: &App) -> String {
    app.success_message.clone().unwrap_or_default()
}

#[tokio::test]
async fn i_opens_the_security_group_card_copies_and_esc_closes_it() {
    let (mut app, tx, _rx) = on_security_groups().await;
    assert!(app.supports_info_card());
    app.handle_key(key(KeyCode::Char('i')), &tx).await.unwrap();
    assert!(app.info_card.is_some());
    let screen = render_to_string(&app);
    assert!(screen.contains("About · Security Groups"), "{screen}");
    assert!(screen.contains("open to world"), "{screen}");

    app.handle_key(key(KeyCode::Char('2')), &tx).await.unwrap();
    assert!(toast(&app).starts_with("Copied the link"), "{}", toast(&app));
    assert!(app.info_card.is_some(), "copying keeps the card open");

    // y copies the block under the cursor; V and j select a range.
    app.handle_key(key(KeyCode::Char('j')), &tx).await.unwrap();
    app.handle_key(key(KeyCode::Char('y')), &tx).await.unwrap();
    assert!(toast(&app).starts_with("Copied the line"), "{}", toast(&app));
    app.handle_key(key(KeyCode::Char('V')), &tx).await.unwrap();
    app.handle_key(key(KeyCode::Char('j')), &tx).await.unwrap();
    app.handle_key(key(KeyCode::Char('j')), &tx).await.unwrap();
    assert_eq!(app.info_card.as_ref().unwrap().range(), (1, 3));
    app.handle_key(key(KeyCode::Char('y')), &tx).await.unwrap();
    assert!(toast(&app).starts_with("Copied the selection"));
    assert_eq!(app.info_card.as_ref().unwrap().anchor, None, "copying ends the selection");
    app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL), &tx).await.unwrap();
    app.handle_key(key(KeyCode::Char('y')), &tx).await.unwrap();
    assert!(toast(&app).starts_with("Copied the card"));

    // The card owns the keys: `j` didn't move the list, `o` didn't browse.
    assert!(!app.details_focused);

    // Esc ends a selection first, then closes.
    app.handle_key(key(KeyCode::Char('V')), &tx).await.unwrap();
    app.handle_key(key(KeyCode::Esc), &tx).await.unwrap();
    assert!(app.info_card.is_some());
    app.handle_key(key(KeyCode::Esc), &tx).await.unwrap();
    assert!(app.info_card.is_none());

    // From the detail pane too, and `i` closes it again.
    app.handle_key(key(KeyCode::Enter), &tx).await.unwrap();
    assert!(app.details_focused);
    app.handle_key(key(KeyCode::Char('i')), &tx).await.unwrap();
    assert!(app.info_card.is_some(), "i works with the detail pane focused");
    app.handle_key(key(KeyCode::Char('i')), &tx).await.unwrap();
    assert!(app.info_card.is_none());
}

#[tokio::test]
async fn o_opens_the_link_under_the_cursor() {
    let (mut app, tx, _rx) = on_security_groups().await;
    app.handle_key(key(KeyCode::Char('i')), &tx).await.unwrap();
    app.handle_key(key(KeyCode::Char('o')), &tx).await.unwrap();
    assert_eq!(toast(&app), "Move to a link to open it (1–9 copy one)");
    assert_eq!(app.last_opened_url, None);

    app.handle_key(key(KeyCode::End), &tx).await.unwrap();
    app.handle_key(key(KeyCode::Char('o')), &tx).await.unwrap();
    assert_eq!(app.last_opened_url.as_deref(), Some("https://docs.aws.amazon.com/vpc/latest/userguide/amazon-vpc-limits.html"));
    assert_eq!(toast(&app), "Opened “Quotas” in the browser");
}

#[tokio::test]
async fn cost_has_a_card_without_sub_tabs_and_other_tabs_say_they_have_none() {
    let (mut app, tx, _rx) = test_app().await;
    let (svc, row) = mock("CostLineItem");
    select_mock(&mut app, svc, row);
    app.handle_key(key(KeyCode::Char('i')), &tx).await.unwrap();
    assert_eq!(app.info_card.as_ref().map(|c| c.card().label), Some("Cost"));
    app.handle_key(key(KeyCode::Esc), &tx).await.unwrap();

    let (svc, sg) = mock("SecurityGroup");
    select_mock(&mut app, svc, sg);
    app.ec2_view = Ec2View::Instances;
    assert!(!app.supports_info_card());
    app.handle_key(key(KeyCode::Char('i')), &tx).await.unwrap();
    assert!(app.info_card.is_none());
    assert_eq!(toast(&app), "No info card for this tab yet");

    // On a type whose browser `i` used to open, the message says where it went.
    let (svc, table) = mock("DdbTable");
    select_mock(&mut app, svc, table);
    app.handle_key(key(KeyCode::Char('i')), &tx).await.unwrap();
    assert_eq!(toast(&app), "No info card for this tab yet · the browser moved to o");
}

#[tokio::test]
async fn the_org_policies_card_follows_the_tab() {
    let (mut app, tx, _rx) = test_app().await;
    let (svc, scp) = mock("OrgScp");
    select_mock(&mut app, svc, scp);
    app.org_view = crate::app::OrgView::Policies;
    app.handle_key(key(KeyCode::Char('i')), &tx).await.unwrap();
    assert_eq!(app.info_card.as_ref().map(|c| c.card().label), Some("Organization Policies"));
}
