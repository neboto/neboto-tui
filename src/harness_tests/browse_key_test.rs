//! `o` browses what's inside the selection — S3 objects, DynamoDB items,
//! an AgentCore memory store's sessions — so `i` can be the info card, as
//! in nebaz. `i` on those types says where the browser went.

use super::*;

fn mock(label: &str) -> (ServiceType, Box<dyn crate::aws::resource::Resource>) {
    all_mocks().into_iter().find(|(_, l, _)| *l == label).map(|(s, _, r)| (s, r)).unwrap()
}

#[tokio::test]
async fn o_opens_the_dynamodb_item_browser_and_i_points_there() {
    for details in [false, true] {
        let (mut app, tx, _rx) = test_app().await;
        let (svc, table) = mock("DdbTable");
        select_mock(&mut app, svc, table);
        if details {
            app.handle_key(key(KeyCode::Enter), &tx).await.unwrap();
            assert!(app.details_focused);
        }
        assert!(app.supports_contents_browser());
        app.handle_key(key(KeyCode::Char('i')), &tx).await.unwrap();
        assert!(!app.ddb_browser.visible, "i no longer opens it (details={details})");
        assert!(app.success_message.as_deref().is_some_and(|m| m.contains("moved to o")), "{:?}", app.success_message);
        app.handle_key(key(KeyCode::Char('o')), &tx).await.unwrap();
        assert!(app.ddb_browser.visible, "o opens it (details={details})");
    }
}

#[tokio::test]
async fn the_status_hint_names_o_for_every_browsable_type() {
    let (mut app, _tx, _rx) = test_app().await;
    for label in ["DdbTable", "AgentCoreMemory"] {
        let (svc, r) = mock(label);
        select_mock(&mut app, svc, r);
        assert!(app.supports_contents_browser(), "{label}");
    }
    let (svc, r) = mock("Ec2Instance");
    select_mock(&mut app, svc, r);
    assert!(!app.supports_contents_browser());
}
