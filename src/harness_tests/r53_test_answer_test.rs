//! #103: a record's Test answer section (`TestDNSAnswer`, `x`-gated) and a
//! health check's last failure reason (`GetHealthCheckLastFailureReason`,
//! folded into the Status section's bundle).

use super::*;
use crate::aws::services::route53::{
    R53HealthObservation, R53HealthStatus, R53TestAnswer, R53_HEALTH_CHECK_SECTIONS,
    R53_RECORD_SECTIONS,
};

fn mock(label: &str) -> (ServiceType, Box<dyn Resource>) {
    let (svc, _, res) = all_mocks()
        .into_iter()
        .find(|(_, l, _)| *l == label)
        .unwrap_or_else(|| panic!("no mock {label}"));
    (svc, res)
}

fn section_idx(desc: &crate::sections::SectionDescriptor, label: &str) -> usize {
    desc.sections.iter().position(|s| s.label == label).unwrap()
}

fn body(app: &App) -> String {
    app.get_detail_lines()
        .into_iter()
        .map(|(k, v)| format!("{k} {v}"))
        .collect::<Vec<_>>()
        .join("\n")
}

async fn press(app: &mut App, c: char, tx: &mpsc::UnboundedSender<Event>) {
    let _ = app.handle_key(key(KeyCode::Char(c)), tx).await;
}

async fn on_record_test_section() -> (App, mpsc::UnboundedSender<Event>, String) {
    let (mut app, tx, _rx) = test_app().await;
    let (svc, res) = mock("R53Record");
    let id = res.id().to_string();
    select_mock(&mut app, svc, res);
    app.details_focused = true;
    app.reset_detail_section_to_default(&tx);
    app.set_detail_section(section_idx(&R53_RECORD_SECTIONS, "Test answer"), &tx);
    (app, tx, id)
}

#[tokio::test]
async fn the_test_never_runs_without_x() {
    let (app, _tx, id) = on_record_test_section().await;
    // Entering the section (and the default section before it) fired nothing.
    assert!(app.lazy.r53_test_answer.get(&id).is_none());
    assert!(app.supports_dns_test(), "x hint should show on the Test answer section");
    let b = body(&app);
    assert!(b.contains("press x to ask"), "{b}");
}

#[tokio::test]
async fn x_asks_and_x_again_asks_again() {
    let (mut app, tx, id) = on_record_test_section().await;
    press(&mut app, 'x', &tx).await;
    assert!(matches!(
        app.lazy.r53_test_answer.get(&id),
        Some(crate::lazy::Lazy::Loading)
    ));

    app.lazy.r53_test_answer.apply(
        id.clone(),
        Ok(R53TestAnswer {
            nameserver: "ns-1.awsdns-01.org".into(),
            response_code: "NOERROR".into(),
            protocol: "UDP".into(),
            record_data: vec!["10.0.0.1".into()],
            asked_at: "12:00:00".into(),
        }),
    );
    let b = body(&app);
    assert!(b.contains("NOERROR"), "{b}");
    assert!(b.contains("10.0.0.1"), "{b}");
    assert!(b.contains("✓ matches this record's values"), "{b}");

    // A second `x` drops the answer and asks again (a weighted set resamples).
    press(&mut app, 'x', &tx).await;
    assert!(matches!(
        app.lazy.r53_test_answer.get(&id),
        Some(crate::lazy::Lazy::Loading)
    ));
}

#[tokio::test]
async fn a_different_answer_is_called_out() {
    let (mut app, _tx, id) = on_record_test_section().await;
    app.lazy.r53_test_answer.apply(
        id,
        Ok(R53TestAnswer {
            nameserver: "ns".into(),
            response_code: "NOERROR".into(),
            protocol: "UDP".into(),
            record_data: vec!["10.9.9.9".into()],
            asked_at: "12:00:00".into(),
        }),
    );
    let b = body(&app);
    assert!(b.contains("differs from this record's values"), "{b}");
}

#[tokio::test]
async fn details_section_keeps_the_flat_rows_and_the_zone_jump() {
    let (mut app, tx, _rx) = test_app().await;
    let (svc, res) = mock("R53Record");
    let flat = res.details();
    select_mock(&mut app, svc, res);
    app.details_focused = true;
    app.reset_detail_section_to_default(&tx);
    let lines = app.get_detail_lines();
    for row in &flat {
        assert!(lines.contains(row), "Details lost {row:?}");
    }
    // `r53_row_jump_target` keys on the "Zone ID" label.
    let (k, v) = lines.iter().find(|(k, _)| k == "Zone ID").unwrap();
    assert!(app.r53_row_jump_target(k, v).is_some(), "Zone ID row should still jump");
}

fn obs(region: &str, status: &str, at: &str) -> R53HealthObservation {
    R53HealthObservation {
        region: region.into(),
        status: status.into(),
        checked_time: Some(at.into()),
    }
}

async fn on_health_status(st: R53HealthStatus) -> String {
    let (mut app, tx, _rx) = test_app().await;
    let (svc, res) = mock("R53HealthCheck");
    let id = res.id().to_string();
    select_mock(&mut app, svc, res);
    app.details_focused = true;
    app.reset_detail_section_to_default(&tx);
    app.set_detail_section(section_idx(&R53_HEALTH_CHECK_SECTIONS, "Status"), &tx);
    app.lazy.r53_health_status.apply(id, Ok(st));
    body(&app)
}

#[tokio::test]
async fn last_failures_show_newest_first_under_a_healthy_status() {
    let b = on_health_status(R53HealthStatus {
        observations: vec![obs("us-east-1", "Success: HTTP 200", "2026-10-08T12:00:00Z")],
        last_failures: vec![
            obs("eu-west-1", "Failure: HTTP 503 (older)", "2026-10-08T10:00:00Z"),
            obs("us-east-1", "Failure: connection timed out", "2026-10-08T11:00:00Z"),
        ],
        last_failure_error: None,
    })
    .await;
    assert!(b.contains("reporting healthy: 1/1"), "{b}");
    assert!(b.contains("Last failure per checker"), "{b}");
    let newer = b.find("connection timed out").expect(&b);
    let older = b.find("HTTP 503 (older)").expect(&b);
    assert!(newer < older, "newest failure should come first:\n{b}");
}

#[tokio::test]
async fn a_failed_last_failure_call_warns_and_keeps_the_live_status() {
    let b = on_health_status(R53HealthStatus {
        observations: vec![obs("us-east-1", "Success: HTTP 200", "2026-10-08T12:00:00Z")],
        last_failures: vec![],
        last_failure_error: Some("AccessDenied: GetHealthCheckLastFailureReason".into()),
    })
    .await;
    assert!(b.contains("Success: HTTP 200"), "{b}");
    assert!(b.contains("⚠ AccessDenied"), "{b}");
    assert!(!b.contains("No failures on record"), "{b}");
}
