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


async fn key(app: &mut App, tx: &mpsc::UnboundedSender<Event>, c: char) {
    let ev = crossterm::event::KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
    app.handle_event(Event::Key(ev), tx).await.unwrap();
}

/// `C` in the browser offers the read on screen and `get-item` for the
/// selected item — `get-item` first once the item is open — each with the
/// session's `--region` like every other copied command.
#[tokio::test]
async fn c_offers_the_read_and_get_item() {
    use crate::ui::widgets::ddb_item_browser::{BrowserFocus, Page};
    let (mut app, tx, _rx) = test_app().await;
    app.ddb_browser.open(&DdbTable::test_table("orders"));
    app.ddb_browser.query = "pk = u".into();
    app.ddb_browser.ran = app.ddb_browser.plan().ok();
    let item = [("pk", "u"), ("sk", "1")]
        .into_iter()
        .map(|(k, v)| (k.to_string(), AttributeValue::S(v.into())))
        .collect();
    app.ddb_browser.apply_page(Page { items: vec![item], last_key: None, scanned: 1, consumed: 0.5 }, false, true);
    app.ddb_browser.focus = BrowserFocus::Results;

    key(&mut app, &tx, 'C').await;
    let picker = app.cli_picker.take().expect("picker with two rows");
    let labels: Vec<&str> = picker.rows.iter().map(|r| r.label.as_str()).collect();
    assert_eq!(labels, ["query", "get-item"]);
    assert!(picker.rows[0].command.starts_with("aws dynamodb query --table-name orders"));
    assert!(picker.rows[0].command.contains("--region "));

    app.ddb_browser.show_detail = true;
    key(&mut app, &tx, 'C').await;
    let picker = app.cli_picker.take().expect("picker");
    assert_eq!(picker.rows[0].label, "get-item");
}
