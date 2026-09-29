//! X-Ray (issue #41): trace pane Overview/Root Cause and the lazy Segments
//! section, the `F` state chips as the faults filter, the `[`/`]` window
//! (variant cache + rebuilt service), and the node "Jump To" row.

use super::*;
use crate::aws::services::xray::{
    trace_detail_from_documents, XRayNode, XRayTrace, XRayTraceDetailSection, XRayWindow,
};
use crate::lazy::Lazy;

const TRACE_ID: &str = "1-5f000000-abcdef0123456789abcdef01";

fn fault_trace() -> XRayTrace {
    use aws_sdk_xray::types::*;
    XRayTrace::from_sdk(
        &TraceSummary::builder()
            .id(TRACE_ID)
            .duration(1.5)
            .has_fault(true)
            .http(
                Http::builder()
                    .http_method("POST")
                    .http_url("https://api.example.com/orders?x=1")
                    .http_status(502)
                    .build(),
            )
            .fault_root_causes(
                FaultRootCause::builder()
                    .services(
                        FaultRootCauseService::builder()
                            .name("orders-fn")
                            .entity_path(
                                FaultRootCauseEntity::builder()
                                    .name("DynamoDB")
                                    .exceptions(
                                        RootCauseException::builder()
                                            .name("ProvisionedThroughputExceededException")
                                            .message("Rate exceeded")
                                            .build(),
                                    )
                                    .build(),
                            )
                            .build(),
                    )
                    .build(),
            )
            .build(),
    )
}

fn ok_trace() -> XRayTrace {
    XRayTrace::from_sdk(
        &aws_sdk_xray::types::TraceSummary::builder()
            .id("1-5f000000-000000000000000000000002")
            .duration(0.05)
            .build(),
    )
}

fn screen_of(app: &App) -> String {
    let backend = TestBackend::new(170, 45);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| crate::render_app(app, f)).unwrap();
    let buf = terminal.backend().buffer();
    (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn trace_pane_shows_outcome_root_cause_and_lazy_segments() {
    let (mut app, tx, _rx) = test_app().await;
    select_mock(&mut app, ServiceType::XRay, Box::new(fault_trace()));
    app.details_focused = true;
    app.layout_mode = crate::app::LayoutMode::DetailsOnly;
    app.reset_detail_section_to_default(&tx);

    let screen = screen_of(&app);
    for needle in ["POST /orders?x=1", "✗ fault (5xx)", "✗ 502", "1.50 s"] {
        assert!(screen.contains(needle), "missing {needle:?} in:\n{screen}");
    }

    app.detail_section_idx = XRayTraceDetailSection::RootCause as usize;
    let lines = app.get_detail_lines();
    assert!(
        lines.iter().any(|(_, v)| v.contains("ProvisionedThroughputExceededException: Rate exceeded")),
        "{lines:?}"
    );

    app.detail_section_idx = XRayTraceDetailSection::Segments as usize;
    app.trigger_xray_trace_load(&tx);
    assert!(matches!(app.lazy.xray_traces.get(TRACE_ID), Some(Lazy::Loading)));
    app.lazy.xray_traces.apply(
        TRACE_ID.to_string(),
        Ok(trace_detail_from_documents(
            &[r#"{"name":"orders-fn","start_time":10.0,"end_time":11.5,"fault":true,
                 "subsegments":[{"name":"DynamoDB","start_time":10.1,"end_time":11.4,"fault":true}]}"#
                .to_string()],
            false,
        )),
    );
    let screen = screen_of(&app);
    assert!(screen.contains("✗ orders-fn"), "{screen}");
    assert!(screen.contains("✗   DynamoDB"), "subsegment indented:\n{screen}");

    // `e` on Segments opens the raw trace JSON.
    let (text, ext) = app.editor_override_content().expect("raw trace override");
    assert_eq!(ext, ".json");
    assert!(text.contains("DynamoDB"));
}

#[tokio::test]
async fn state_chips_are_the_fault_filter() {
    let (mut app, tx, _rx) = test_app().await;
    select_mock(&mut app, ServiceType::XRay, Box::new(fault_trace()));
    app.xray_view = crate::app::XRayView::Traces;
    app.resources = vec![Box::new(fault_trace()), Box::new(ok_trace())];
    app.update_search();
    assert_eq!(app.filtered_resources.len(), 2);
    // `F` cycles through the states present — "fault" is one of them.
    let mut labels = Vec::new();
    for _ in 0..3 {
        app.handle_event(Event::Key(KeyEvent::new(KeyCode::Char('F'), KeyModifiers::SHIFT)), &tx)
            .await
            .unwrap();
        labels.push(
            app.filtered_resources
                .iter()
                .map(|&i| app.resources[i].state_label())
                .collect::<Vec<_>>(),
        );
    }
    assert!(labels.contains(&vec!["fault".to_string()]), "{labels:?}");
}

#[tokio::test]
async fn window_keys_rebuild_and_variant_cache() {
    let (mut app, tx, _rx) = test_app().await;
    select_mock(&mut app, ServiceType::XRay, Box::new(ok_trace()));
    assert_eq!(app.xray_window, XRayWindow::OneHour);
    app.handle_event(Event::Key(KeyEvent::new(KeyCode::Char(']'), KeyModifiers::NONE)), &tx)
        .await
        .unwrap();
    assert_eq!(app.xray_window, XRayWindow::SixHours);
    app.handle_event(Event::Key(KeyEvent::new(KeyCode::Char(']'), KeyModifiers::NONE)), &tx)
        .await
        .unwrap();
    assert_eq!(app.xray_window, XRayWindow::SixHours, "saturates at 6h");
    app.handle_event(Event::Key(KeyEvent::new(KeyCode::Char('['), KeyModifiers::NONE)), &tx)
        .await
        .unwrap();
    assert_eq!(app.xray_window, XRayWindow::OneHour);
    assert!(screen_of(&app).contains(" 1h "), "window chip rendered");
}

#[tokio::test]
async fn node_jump_to_row_targets_the_named_service() {
    let nodes = XRayNode::from_graph(
        &[aws_sdk_xray::types::Service::builder()
            .reference_id(0)
            .name("orders-fn")
            .r#type("AWS::Lambda::Function")
            .build()],
        XRayWindow::OneHour,
    );
    let lines = crate::ui::widgets::details_pane::xray_node_section_lines(
        &nodes[0],
        crate::aws::services::xray::XRayNodeDetailSection::Overview,
    );
    let (k, v) = lines.iter().find(|(k, _)| k == "Jump To").expect("Jump To row");
    let target = crate::ui::widgets::details_pane::resource_jump_target(k, v, ServiceType::XRay)
        .expect("jumpable");
    assert_eq!(target.service, ServiceType::Lambda);
    assert_eq!(target.id, "orders-fn");
}
