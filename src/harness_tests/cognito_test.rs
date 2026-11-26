//! Cognito (#230): a member row in a user pool's Groups section jumps to
//! that user on the Users tab, by the stored username even when the row
//! shows the email, and a member the Users list doesn't hold says so.

use super::*;
use crate::app::{CognitoView, JumpView};
use crate::aws::services::cognito::{
    CognitoGroup, CognitoGroupMember, CognitoGroups, CognitoUser, CognitoUserPool, CognitoUserPoolDetailSection,
};

fn pool() -> CognitoUserPool {
    let r = all_mocks().into_iter().find(|(_, l, _)| *l == "CognitoUserPool").unwrap().2;
    r.as_any().downcast_ref::<CognitoUserPool>().unwrap().clone()
}

fn user(pool_id: &str, username: &str) -> CognitoUser {
    CognitoUser {
        key: format!("{pool_id}/{username}"),
        username: username.into(),
        pool_id: pool_id.into(),
        status: "CONFIRMED".into(),
        enabled: true,
        created: None,
        last_modified: None,
        email: None,
        phone: None,
        email_verified: false,
        phone_verified: false,
        attributes: Default::default(),
    }
}

fn member(username: &str, label: &str) -> CognitoGroupMember {
    CognitoGroupMember { username: username.into(), label: label.into(), enabled: true, status: None }
}

const SUB: &str = "7d1e5c2a-0b3f-4e8a-9c61-2f4d8b0a1e93";

async fn on_groups() -> (App, mpsc::UnboundedSender<Event>, mpsc::UnboundedReceiver<Event>) {
    let (mut app, tx, rx) = test_app().await;
    let p = pool();
    let id = p.id.clone();
    select_mock(&mut app, ServiceType::Cognito, Box::new(p));
    app.resources.push(Box::new(user(&id, "alice")));
    app.resources.push(Box::new(user(&id, SUB)));
    app.lazy.cognito_groups.apply(
        id,
        Ok(CognitoGroups {
            groups: vec![CognitoGroup {
                name: "admins".into(),
                description: None,
                precedence: None,
                role_arn: None,
                members: Ok(vec![member("alice", "alice"), member(SUB, "bob@example.com"), member("carol", "carol")]),
                members_truncated: false,
            }],
            truncated: false,
        }),
    );
    app.details_focused = true;
    app.detail_section_idx = CognitoUserPoolDetailSection::Groups as usize;
    (app, tx, rx)
}

/// `⏎` on the member row whose text is `label`.
async fn enter_on(app: &mut App, tx: &mpsc::UnboundedSender<Event>, label: &str) {
    let row = format!("    {label}");
    let i = app.get_detail_lines_filtered().iter().position(|(k, _)| *k == row).unwrap_or_else(|| panic!("no {row:?} row"));
    app.details_selected_index = Some(i);
    app.handle_key(key(KeyCode::Enter), tx).await.unwrap();
}

#[tokio::test]
async fn a_group_member_jumps_to_the_user_by_username() {
    let (mut app, tx, _rx) = on_groups().await;
    let pool_id = pool().id;

    // The row shows the email; the jump goes by the username behind it.
    let t = app.cognito_row_jump_target("    bob@example.com", "").expect("member row");
    assert!(matches!(t.view, JumpView::Cognito(CognitoView::Users)));
    assert_eq!(t.id, format!("{pool_id}/{SUB}"));
    // Not a member row: the count, a non-member name, the wrong indent.
    assert!(app.cognito_row_jump_target("  Members", "3").is_none());
    assert!(app.cognito_row_jump_target("    nobody", "").is_none());
    assert!(app.cognito_row_jump_target("      alice", "").is_none());

    enter_on(&mut app, &tx, "bob@example.com").await;
    assert_eq!(app.cognito_view, CognitoView::Users);
    assert_eq!(app.get_selected_resource().map(|r| r.id().to_string()), Some(format!("{pool_id}/{SUB}")));

    // Outside @cognito the row shape means nothing.
    app.current_service = Some(ServiceType::EC2);
    assert!(app.cognito_row_jump_target("    alice", "").is_none());
}

#[tokio::test]
async fn a_member_the_users_list_does_not_hold_says_so() {
    let (mut app, tx, _rx) = on_groups().await;
    enter_on(&mut app, &tx, "carol").await;
    assert_eq!(app.cognito_view, CognitoView::Users);
    let msg = app.success_message.clone().unwrap_or_default();
    assert_eq!(msg, "carol isn't in the Users list", "{msg}");
    assert!(app.pending_jump.is_none(), "a miss isn't left pending");
}
