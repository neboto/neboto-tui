//! `F` state-filter wiring: the chips must speak each resource's own status
//! vocabulary, and a view of stateless kinds must offer nothing rather than a
//! no-op "available" chip. Both were wrong once — the filter cycled the coarse
//! `ResourceState` bucket words, so a Trusted Advisor "action recommended"
//! check filtered as "unavailable" and every IAM role as "available".

use super::*;
use crate::aws::services::iam::IamRole;
use crate::aws::services::organizations::OrgAccount;

async fn press(app: &mut App, tx: &mpsc::UnboundedSender<Event>, code: KeyCode) {
    app.handle_event(Event::Key(key(code)), tx).await.unwrap();
}

fn load(app: &mut App, service: ServiceType, rows: Vec<Box<dyn Resource>>) {
    app.current_service = Some(service);
    app.loading = false;
    app.search_active = false;
    app.search_query.clear();
    app.filtered_resources = (0..rows.len()).collect();
    app.resources = rows;
    app.selected_index = Some(0);
    app.details_focused = false;
}

fn account(id: &str, status: &str) -> Box<dyn Resource> {
    Box::new(OrgAccount::from_sdk(
        &aws_sdk_organizations::types::Account::builder()
            .id(id)
            .arn(format!("arn:aws:organizations::111111111111:account/o-mock/{}", id))
            .name(id)
            .email(format!("{}@example.com", id))
            .status(aws_sdk_organizations::types::AccountStatus::from(status))
            .build(),
    ))
}

#[tokio::test]
async fn state_filter_cycles_the_native_status_words() {
    let (mut app, tx, _rx) = test_app().await;
    app.org_view = crate::app::OrgView::Accounts;
    load(
        &mut app,
        ServiceType::Organizations,
        vec![
            account("111111111111", "ACTIVE"),
            account("222222222222", "SUSPENDED"),
            account("333333333333", "ACTIVE"),
        ],
    );

    press(&mut app, &tx, KeyCode::Char('F')).await;
    assert_eq!(app.list_state_filter.as_deref(), Some("active"));
    assert_eq!(app.filtered_resources.len(), 2);

    press(&mut app, &tx, KeyCode::Char('F')).await;
    assert_eq!(app.list_state_filter.as_deref(), Some("suspended"));
    assert_eq!(app.filtered_resources.len(), 1);

    // Past the last state wraps back to "all".
    press(&mut app, &tx, KeyCode::Char('F')).await;
    assert_eq!(app.list_state_filter, None);
    assert_eq!(app.filtered_resources.len(), 3);
}

#[tokio::test]
async fn stateless_kinds_offer_no_state_filter() {
    let (mut app, tx, _rx) = test_app().await;
    let role = |name: &str| -> Box<dyn Resource> {
        Box::new(IamRole::from_sdk(
            &aws_sdk_iam::types::Role::builder()
                .path("/")
                .role_name(name)
                .role_id(format!("AROA{}", name))
                .arn(format!("arn:aws:iam::111111111111:role/{}", name))
                .create_date(aws_smithy_types::DateTime::from_secs(0))
                .build()
                .unwrap(),
        ))
    };
    load(&mut app, ServiceType::IAM, vec![role("a"), role("b")]);
    assert_eq!(app.resources[0].state_label(), "", "an IAM role has no lifecycle state");

    press(&mut app, &tx, KeyCode::Char('F')).await;
    assert_eq!(app.list_state_filter, None);
    assert_eq!(app.filtered_resources.len(), 2, "F must not thin a stateless list");
    assert!(app.success_message.as_deref().is_some_and(|m| m.contains("No states")));
}
