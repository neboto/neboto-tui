//! The instance pane's Console section (#19): renders `GetConsoleOutput`
//! results as content lines. The section-order test only checks the tab
//! label, so this drives the lazy map with real rows — including the
//! "nothing posted yet" and error arms — and reads the screen.

use super::*;
use crate::aws::services::ec2::{ConsoleOutput, Ec2Instance, Ec2InstanceDetailSection};
use crate::lazy::Lazy;

const INSTANCE_ID: &str = "i-0123456789abcdef0";

/// Focus an instance on its Console section, deliver `result` the way the
/// apply-closure would, and return the 140x30 screen as one string.
async fn render_console(result: std::result::Result<Option<ConsoleOutput>, String>) -> String {
    let (mut app, tx, _rx) = test_app().await;
    let instance = Ec2Instance::from_sdk(
        &aws_sdk_ec2::types::Instance::builder()
            .instance_id(INSTANCE_ID)
            .build(),
    );
    select_mock(&mut app, ServiceType::EC2, Box::new(instance));
    app.details_focused = true;
    app.layout_mode = crate::app::LayoutMode::DetailsOnly;
    app.reset_detail_section_to_default(&tx);
    app.detail_section_idx = Ec2InstanceDetailSection::Console as usize;
    // The on-enter hook fires a dead-endpoint fetch we never await …
    app.trigger_ec2_instance_console_load(&tx);
    assert!(
        matches!(
            app.lazy.ec2_instance_console.get(INSTANCE_ID),
            Some(Lazy::Loading)
        ),
        "entering the section must start the fetch"
    );
    // … and this is its result landing.
    app.lazy
        .ec2_instance_console
        .apply(INSTANCE_ID.to_string(), result);

    let backend = TestBackend::new(140, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| crate::render_app(&app, f)).unwrap();
    let buf = terminal.backend().buffer();
    (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf.get(x, y).symbol().to_string())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn console_section_renders_captured_row_and_log_lines() {
    let screen = render_console(Ok(Some(ConsoleOutput {
        captured_at: Some("2026-09-21T10:15:00Z".to_string()),
        captured_secs: Some(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64
                - 150,
        ),
        lines: vec![
            "[    0.000000] Linux version 6.1.0".to_string(),
            "[    2.318221] cloud-init[1234]: Cloud-init v. 23.4 running".to_string(),
        ],
    })))
    .await;
    assert!(
        screen.contains("6 Console"),
        "section tab missing:\n{screen}"
    );
    assert!(
        screen.contains("Captured"),
        "captured-at row missing:\n{screen}"
    );
    assert!(
        screen.contains("2026-09-21T10:15:00Z (2m ago)"),
        "timestamp + age missing:\n{screen}"
    );
    assert!(
        screen.contains("[    0.000000] Linux version 6.1.0"),
        "first log line missing:\n{screen}"
    );
    assert!(
        screen.contains("cloud-init[1234]: Cloud-init v. 23.4 running"),
        "second log line missing:\n{screen}"
    );
}

#[tokio::test]
async fn console_section_explains_when_nothing_has_been_posted_yet() {
    let screen = render_console(Ok(None)).await;
    assert!(
        screen.contains("No console output available yet"),
        "empty state missing:\n{screen}"
    );
}

#[tokio::test]
async fn console_section_shows_fetch_errors_inline() {
    let screen = render_console(Err(
        "Console output unavailable: UnauthorizedOperation".to_string()
    ))
    .await;
    assert!(
        screen.contains("⚠ Console output unavailable: UnauthorizedOperation"),
        "error row missing:\n{screen}"
    );
}
