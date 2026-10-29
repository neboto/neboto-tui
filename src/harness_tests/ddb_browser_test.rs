//! The item browser drops a page from a query it has since replaced.
//!
//! Each Scan/Query stamps its result with `DdbBrowserState::generation`.
//! Without the stamp, a slow first page could land after you'd typed a new
//! filter and run it, replacing the fresh results with the old ones.

use super::*;
use crate::aws::services::dynamodb::DdbTable;
use aws_sdk_dynamodb::types::AttributeValue;

fn page(generation: u64, pk: &str) -> Event {
    Event::DdbItemsLoaded {
        generation,
        items: vec![[("pk".to_string(), AttributeValue::S(pk.into()))].into_iter().collect()],
        last_key: None,
        scanned: 1,
        consumed: 0.5,
        append: false,
        done: true,
    }
}

#[tokio::test]
async fn a_superseded_page_is_dropped() {
    let (mut app, tx, _rx) = test_app().await;
    app.ddb_browser.open(&DdbTable::test_table("orders"));
    let stale = app.ddb_browser.generation;
    app.ddb_browser.generation += 1; // a newer query was run
    let fresh = app.ddb_browser.generation;

    app.handle_event(page(fresh, "new"), &tx).await.unwrap();
    app.handle_event(page(stale, "old"), &tx).await.unwrap();
    app.handle_event(
        Event::DdbItemsError { generation: stale, message: "late failure".into() },
        &tx,
    )
    .await
    .unwrap();

    let b = &app.ddb_browser;
    assert_eq!(b.items.len(), 1);
    assert!(matches!(&b.items[0]["pk"], AttributeValue::S(s) if s == "new"));
    assert!(b.error.is_none());
}

#[tokio::test]
async fn a_page_after_close_is_dropped() {
    let (mut app, tx, _rx) = test_app().await;
    app.ddb_browser.open(&DdbTable::test_table("orders"));
    let in_flight = app.ddb_browser.generation;
    app.ddb_browser.close();
    app.ddb_browser.open(&DdbTable::test_table("customers"));

    app.handle_event(page(in_flight, "orders-item"), &tx).await.unwrap();
    assert!(app.ddb_browser.items.is_empty());
}

