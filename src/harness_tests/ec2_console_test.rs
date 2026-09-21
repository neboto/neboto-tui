//! The instance pane's Console section (#19): renders `GetConsoleOutput`
//! results as content lines. The section-order test only checks the tab
//! label, so this drives the lazy map with real rows — including the
//! "nothing posted yet" and error arms — and reads the screen.

use super::*;
use crate::aws::services::ec2::{ConsoleOutput, Ec2Instance, Ec2InstanceDetailSection};
use crate::lazy::Lazy;

const INSTANCE_ID: &str = "i-0123456789abcdef0";

/// An app focused on a mock instance's Console section with `result`
/// delivered the way the apply-closure would.
async fn app_with_console(result: std::result::Result<Option<ConsoleOutput>, String>) -> App {
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
    app
}

/// Render the Console section and return the 140x30 screen as one string.
async fn render_console(result: std::result::Result<Option<ConsoleOutput>, String>) -> String {
    let app = app_with_console(result).await;
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

fn sample_output() -> ConsoleOutput {
    ConsoleOutput {
        captured_at: Some("2026-09-21T10:15:00Z".to_string()),
        captured_secs: None,
        lines: vec![
            "[    0.000000] Linux version 6.1.0".to_string(),
            "[    1.000000] Kernel panic - not syncing".to_string(),
        ],
    }
}

#[tokio::test]
async fn e_on_the_console_section_opens_the_raw_log_not_the_snapshot_json() {
    let app = app_with_console(Ok(Some(sample_output()))).await;
    let (text, suffix) = app
        .editor_override_content()
        .expect("loaded console output must override the snapshot");
    assert_eq!(suffix, ".log");
    assert_eq!(
        text,
        "# i-0123456789abcdef0 · console output captured 2026-09-21T10:15:00Z\n\
         [    0.000000] Linux version 6.1.0\n\
         [    1.000000] Kernel panic - not syncing\n"
    );
}

#[tokio::test]
async fn e_falls_through_to_the_snapshot_while_console_is_empty_or_on_other_sections() {
    let app = app_with_console(Ok(None)).await;
    assert!(app.editor_override_content().is_none());

    let mut app = app_with_console(Ok(Some(sample_output()))).await;
    app.detail_section_idx = Ec2InstanceDetailSection::Details as usize;
    assert!(app.editor_override_content().is_none());
}

#[tokio::test]
async fn e_on_the_user_data_section_opens_the_script_with_a_sniffed_suffix() {
    let mut app = app_with_console(Ok(None)).await;
    app.detail_section_idx = Ec2InstanceDetailSection::UserData as usize;
    for (script, suffix) in [
        ("#!/bin/bash\napt-get update\n", ".sh"),
        ("#cloud-config\npackages:\n  - nginx\n", ".yaml"),
        ("Content-Type: multipart/mixed; boundary=\"x\"\n", ".txt"),
    ] {
        app.lazy
            .ec2_instance_user_data
            .apply(INSTANCE_ID.to_string(), Ok(Some(script.to_string())));
        assert_eq!(
            app.editor_override_content(),
            Some((script.to_string(), suffix)),
            "suffix for {script:?}"
        );
        app.lazy.ec2_instance_user_data = Default::default();
    }
}
