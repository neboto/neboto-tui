//! The CloudTrail Events list holds only the newest `CT_MAX_EVENTS`. A local
//! `/` search that finds nothing there once read as "no such event" when the
//! event was simply older (a `ListFunctions` call ten minutes back, in an
//! account busy with neboto's own reads). The list must say it's cut off,
//! and `f` must carry the search over to CloudTrail's server-side lookup.

use super::*;
use crate::aws::services::cloudtrail::{CloudTrailEvent, CtLookupAttr, CT_MAX_EVENTS};
use crate::ui::widgets::ct_filter_modal::CtFilterPhase;

fn event(i: usize) -> Box<dyn Resource> {
    Box::new(CloudTrailEvent::from_sdk(
        &aws_sdk_cloudtrail::types::Event::builder()
            .event_id(format!("00000000-0000-0000-0000-{i:012}"))
            .event_name("DescribeInstances")
            .cloud_trail_event(
                r#"{"eventVersion":"1.08","awsRegion":"us-east-1","eventSource":"ec2.amazonaws.com","eventName":"DescribeInstances","readOnly":true}"#,
            )
            .build(),
    ))
}

fn screen(app: &App) -> String {
    let mut terminal = Terminal::new(TestBackend::new(160, 32)).unwrap();
    terminal.draw(|f| crate::render_app(app, f)).unwrap();
    let buf = terminal.backend().buffer().clone();
    (0..buf.area.height)
        .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

async fn events_view(count: usize) -> (App, mpsc::UnboundedSender<Event>, mpsc::UnboundedReceiver<Event>) {
    let (mut app, tx, rx) = test_app().await;
    app.current_service = Some(ServiceType::CloudTrail);
    app.cloudtrail_view = crate::app::CloudTrailView::Events;
    app.loading = false;
    app.resources = (0..count).map(event).collect();
    app.search_query = "ListFunc".to_string();
    app.update_search();
    assert!(app.filtered_resources.is_empty());
    (app, tx, rx)
}

#[tokio::test]
async fn a_capped_list_says_so_and_f_carries_the_search_over() {
    let (mut app, tx, _rx) = events_view(CT_MAX_EVENTS).await;
    assert!(app.ct_events_capped());
    let text = screen(&app);
    assert!(text.contains(&format!("newest {CT_MAX_EVENTS} only")), "{text}");
    assert!(text.contains(&format!("in the {CT_MAX_EVENTS} newest events")), "{text}");
    assert!(text.contains("f search all of CloudTrail"), "{text}");

    app.handle_event(Event::Key(key(KeyCode::Char('f'))), &tx).await.unwrap();
    let m = &app.ct_filter_modal;
    assert!(m.visible);
    assert_eq!(m.phase, CtFilterPhase::Value, "seeded: ⏎ applies");
    assert_eq!(m.selected_attr(), Some(CtLookupAttr::EventName));
    assert_eq!(m.value, "ListFunc");
}

#[tokio::test]
async fn an_uncapped_list_keeps_the_plain_no_match() {
    let (app, _tx, _rx) = events_view(10).await;
    assert!(!app.ct_events_capped());
    let text = screen(&app);
    assert!(!text.contains("newest"), "{text}");
    assert!(text.contains("No matches for “ListFunc”"), "{text}");
}
