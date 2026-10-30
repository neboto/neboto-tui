//! `⌫` / `⌃F` / bookmarks across an org member-account switch: every place
//! remembers the session it was captured under, and restoring one from
//! another account replays the switch (`NavLocation.credentials`).

use super::*;
use crate::app::{NavAssumedRole, NavCredentials};
use crate::aws::client::AssumedOrgRole;
use crossterm::event::KeyModifiers;

fn instance(id: &str) -> Box<dyn Resource> {
    Box::new(crate::aws::services::ec2::Ec2Instance::from_sdk(
        &aws_sdk_ec2::types::Instance::builder().instance_id(id).build(),
    ))
}

async fn drain(app: &mut App, tx: &mpsc::UnboundedSender<Event>, rx: &mut mpsc::UnboundedReceiver<Event>) {
    while let Ok(ev) = rx.try_recv() {
        // Load-stream events from the dead test endpoint are noise here.
        if matches!(ev, Event::LoadStream { .. }) {
            continue;
        }
        app.handle_event(ev, tx).await.unwrap();
    }
}

async fn press(app: &mut App, tx: &mpsc::UnboundedSender<Event>, k: KeyEvent) {
    app.handle_event(Event::Key(k), tx).await.unwrap();
}

fn selected(app: &App) -> Option<String> {
    app.get_selected_resource().map(|r| r.id().to_string())
}

/// Show `id` as the EC2 list's only row, selected — what a load would leave.
fn show(app: &mut App, id: &str) {
    app.cache.insert(ServiceType::EC2, app.current_region, None, vec![instance(id)]);
    app.current_service = Some(ServiceType::EC2);
    app.load_service_resources();
    app.update_search();
    app.resolve_pending_jump(&mpsc::unbounded_channel().0);
    if app.selected_index.is_none() {
        app.selected_index = Some(0);
    }
}

fn prod_role() -> AssumedOrgRole {
    AssumedOrgRole {
        account_id: "111122223333".into(),
        account_name: "prod".into(),
        role_name: "OrganizationAccountAccessRole".into(),
    }
}

/// The issue's case: from the management account, `s` into a member account,
/// then `⌫` goes back to where you pressed it — out of the role — and `⌃F`
/// asks to assume it again.
#[tokio::test]
async fn back_out_of_a_member_account_and_forward_into_it() {
    let (mut app, tx, mut rx) = test_app().await;
    show(&mut app, "i-0mgmt");
    // What a successful `s` does: "here" is recorded under the base session,
    // then the role is assumed (the STS half can't run offline).
    app.record_location();
    app.aws_clients.set_assumed_role_for_test(prod_role());
    show(&mut app, "i-0member");
    assert_eq!(selected(&app).as_deref(), Some("i-0member"));

    // ⌫: drops the role and lands on the row we left, not on @org.
    press(&mut app, &tx, key(KeyCode::Backspace)).await;
    drain(&mut app, &tx, &mut rx).await;
    assert!(app.aws_clients.current_assumed_role().is_none(), "back in the base account");
    assert_eq!(app.current_service, Some(ServiceType::EC2));
    assert_eq!(app.pending_jump.as_deref(), Some("i-0mgmt"), "waiting on the base account's list");
    show(&mut app, "i-0mgmt");
    assert_eq!(selected(&app).as_deref(), Some("i-0mgmt"));
    assert!(app.pending_credential_restore.is_none());

    // Forward names the member account it's going back into.
    let fwd = &app.nav_history[app.nav_forward_index().expect("somewhere to go forward to")];
    assert_eq!(fwd.selected_id.as_deref(), Some("i-0member"));
    let other = fwd.other_credentials(&app.current_credentials()).map(|c| c.label());
    assert_eq!(other.as_deref(), Some("prod (111122223333)"));

    // ⌃F: asks for the same AssumeRole `s` makes.
    press(&mut app, &tx, KeyEvent::new(KeyCode::Char('f'), KeyModifiers::CONTROL)).await;
    let mut asked = None;
    while let Ok(ev) = rx.try_recv() {
        if let Event::OrgRoleSwitchRequested { account_id, role_name, .. } = ev {
            asked = Some((account_id, role_name));
        }
    }
    assert_eq!(
        asked,
        Some(("111122223333".to_string(), "OrganizationAccountAccessRole".to_string()))
    );
    assert!(app.pending_credential_restore.is_some());
}

/// A switch that fails (here: STS unreachable) leaves the view and the
/// history cursor where you still are.
#[tokio::test]
async fn a_failed_switch_leaves_you_where_you_were() {
    let (mut app, tx, mut rx) = test_app().await;
    app.aws_clients.set_assumed_role_for_test(prod_role());
    show(&mut app, "i-0member");
    app.record_location();
    app.aws_clients = crate::aws::client::AwsClients::new_for_test().await;
    show(&mut app, "i-0mgmt");

    press(&mut app, &tx, key(KeyCode::Backspace)).await;
    let cursor_before = Some(app.nav_history.len() - 1); // "here", parked
    assert!(app.pending_credential_restore.is_some());
    drain(&mut app, &tx, &mut rx).await;

    assert!(app.pending_credential_restore.is_none());
    assert!(app.aws_clients.current_assumed_role().is_none(), "still the base account");
    assert_eq!(selected(&app).as_deref(), Some("i-0mgmt"), "view untouched");
    assert_eq!(app.nav_cursor, cursor_before, "cursor put back");
    assert!(app.error_message.is_some(), "the switch said why");
}

/// Leaving the member account records it, so `⌫` after an exit goes back in.
#[tokio::test]
async fn exiting_the_role_records_the_member_place() {
    let (mut app, tx, mut rx) = test_app().await;
    app.aws_clients.set_assumed_role_for_test(prod_role());
    show(&mut app, "i-0member");
    app.handle_event(Event::OrgRoleExitRequested, &tx).await.unwrap();
    drain(&mut app, &tx, &mut rx).await;

    let back = app.nav_history.last().expect("the member place was recorded");
    assert_eq!(back.selected_id.as_deref(), Some("i-0member"));
    let creds = back.credentials.as_ref().expect("with its session");
    assert_eq!(creds.assumed.as_ref().map(|a| a.account_id.as_str()), Some("111122223333"));
}

/// A place from before credentials were captured restores in place, as it
/// always did; others ask for the switch they need — the role when the
/// profile already matches, else the profile first.
#[tokio::test]
async fn each_place_asks_for_the_switch_it_needs() {
    let (mut app, tx, mut rx) = test_app().await;
    show(&mut app, "i-0here");
    let mut old = app.current_location().unwrap();
    old.credentials = None;
    assert!(app.jump_to_location(old, &tx));
    assert!(app.pending_credential_restore.is_none());
    drain(&mut app, &tx, &mut rx).await;

    // The test clients run with no named profile.
    let here = app.current_credentials();
    assert_eq!(here.profile, None);
    let mut member = app.current_location().unwrap();
    member.credentials = Some(NavCredentials {
        profile: None,
        assumed: Some(NavAssumedRole {
            account_id: "1".into(),
            account_name: String::new(),
            role_name: "R".into(),
        }),
    });
    // Same (absent) profile, different role: reachable — an assume.
    assert!(app.jump_to_location(member.clone(), &tx));
    assert!(matches!(rx.try_recv(), Ok(Event::OrgRoleSwitchRequested { .. })));
    app.pending_credential_restore = None;

    let mut named = member;
    named.credentials = Some(NavCredentials { profile: Some("other".into()), assumed: None });
    assert!(app.jump_to_location(named, &tx));
    assert!(matches!(
        rx.try_recv(),
        Ok(Event::ProfileSwitchRequested { profile }) if profile == "other"
    ));
}
