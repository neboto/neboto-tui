//! DMS (issue #40): the task pane's Overview + lazy Tables section, the
//! instance pane's sibling-filtered Tasks section, the ARN jump from a task
//! to its endpoint, and the `m` / `t` gates.

use super::*;
use crate::aws::services::dms::{
    DmsEndpoint, DmsInstance, DmsInstanceDetailSection, DmsTableStat, DmsTableStats, DmsTask,
    DmsTaskDetailSection,
};
use crate::lazy::Lazy;
use std::collections::HashMap;

const TASK_ARN: &str = "arn:aws:dms:us-east-1:123456789012:task:TASKRES1";
const INST_ARN: &str = "arn:aws:dms:us-east-1:123456789012:rep:INSTRES1";
const SRC_ARN: &str = "arn:aws:dms:us-east-1:123456789012:endpoint:SRCRES1";

fn task(stop_reason: Option<&str>) -> DmsTask {
    let t = aws_sdk_databasemigration::types::ReplicationTask::builder()
        .replication_task_identifier("orders-cdc")
        .replication_task_arn(TASK_ARN)
        .replication_instance_arn(INST_ARN)
        .source_endpoint_arn(SRC_ARN)
        .status("stopped")
        .set_stop_reason(stop_reason.map(str::to_string))
        .migration_type(aws_sdk_databasemigration::types::MigrationTypeValue::FullLoadAndCdc)
        .replication_task_settings(r#"{"Logging":{"EnableLogging":true}}"#)
        .build();
    DmsTask::from_sdk(
        &t,
        &HashMap::from([(INST_ARN.to_string(), "prod-repl".to_string())]),
        &HashMap::from([(SRC_ARN.to_string(), ("orders-src".to_string(), "PostgreSQL".to_string()))]),
    )
}

fn instance() -> DmsInstance {
    DmsInstance::from_sdk(
        &aws_sdk_databasemigration::types::ReplicationInstance::builder()
            .replication_instance_identifier("prod-repl")
            .replication_instance_arn(INST_ARN)
            .replication_instance_class("dms.r5.large")
            .replication_instance_status("available")
            .build(),
    )
}

fn endpoint() -> DmsEndpoint {
    DmsEndpoint::from_sdk(
        &aws_sdk_databasemigration::types::Endpoint::builder()
            .endpoint_identifier("orders-src")
            .endpoint_arn(SRC_ARN)
            .engine_name("postgres")
            .status("active")
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
                .map(|x| buf.get(x, y).symbol().to_string())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn task_overview_flags_an_error_stop_and_names_its_endpoints() {
    let (mut app, tx, _rx) = test_app().await;
    select_mock(&mut app, ServiceType::Dms, Box::new(task(Some("Stop Reason FATAL_ERROR Error Level FATAL"))));
    app.details_focused = true;
    app.layout_mode = crate::app::LayoutMode::DetailsOnly;
    app.reset_detail_section_to_default(&tx);

    let screen = screen_of(&app);
    for needle in [
        "DMS Replication Task",
        "stopped (error)",
        "✗ Stop Reason FATAL_ERROR",
        "orders-src (PostgreSQL)",
        "prod-repl",
        "dms-tasks-prod-repl (t to tail)",
    ] {
        assert!(screen.contains(needle), "missing {needle:?} in:\n{screen}");
    }

    // `m` and `t` are offered: the instance name is known.
    assert!(app.supports_metrics_overlay());
    assert!(app.supports_log_tail());
}

#[tokio::test]
async fn tables_section_fetches_lazily_and_lists_errors_first() {
    let (mut app, tx, _rx) = test_app().await;
    select_mock(&mut app, ServiceType::Dms, Box::new(task(None)));
    app.details_focused = true;
    app.layout_mode = crate::app::LayoutMode::DetailsOnly;
    app.reset_detail_section_to_default(&tx);
    assert!(app.lazy.dms_table_stats.get(TASK_ARN).is_none(), "Overview doesn't fetch tables");

    app.detail_section_idx = DmsTaskDetailSection::Tables as usize;
    app.trigger_dms_table_stats_load(&tx);
    assert!(matches!(app.lazy.dms_table_stats.get(TASK_ARN), Some(Lazy::Loading)));

    app.lazy.dms_table_stats.apply(
        TASK_ARN.to_string(),
        Ok(DmsTableStats {
            tables: vec![
                DmsTableStat {
                    schema: "public".into(),
                    table: "orders".into(),
                    state: "Table error".into(),
                    full_load_rows: 10,
                    ..Default::default()
                },
                DmsTableStat {
                    schema: "public".into(),
                    table: "customers".into(),
                    state: "Table completed".into(),
                    full_load_rows: 500,
                    ..Default::default()
                },
            ],
            truncated: false,
        }),
    );
    let screen = screen_of(&app);
    assert!(screen.contains("✗ 1"), "errored count:\n{screen}");
    let err = screen.find("public.orders").expect("errored table shown");
    let ok = screen.find("public.customers").expect("ok table shown");
    assert!(err < ok, "errored tables sort first");
}

#[tokio::test]
async fn instance_tasks_section_lists_sibling_tasks_and_task_arn_jumps_to_endpoint() {
    let (mut app, tx, _rx) = test_app().await;
    select_mock(&mut app, ServiceType::Dms, Box::new(instance()));
    app.resources = vec![Box::new(instance()), Box::new(task(None)), Box::new(endpoint())];
    app.filtered_resources = vec![0];
    app.selected_index = Some(0);
    app.details_focused = true;
    app.layout_mode = crate::app::LayoutMode::DetailsOnly;
    app.reset_detail_section_to_default(&tx);
    app.detail_section_idx = DmsInstanceDetailSection::Tasks as usize;

    let lines = app.get_detail_lines();
    assert!(lines.iter().any(|(k, _)| k == "orders-cdc"), "{lines:?}");
    assert!(lines.iter().any(|(_, v)| v == TASK_ARN));

    // The task ARN row is a jump into DMS (sub-tab aligned on landing).
    let target = crate::ui::widgets::details_pane::resource_jump_target(
        "  Task ARN",
        TASK_ARN,
        ServiceType::Dms,
    )
    .expect("DMS ARNs jump");
    assert_eq!(target.service, ServiceType::Dms);
    assert_eq!(target.id, TASK_ARN);
    assert!(crate::ui::widgets::details_pane::resource_jump_target(
        "  Endpoint ARN",
        SRC_ARN,
        ServiceType::Dms
    )
    .is_some());
}

#[tokio::test]
async fn sub_tabs_filter_by_type() {
    let (mut app, tx, _rx) = test_app().await;
    select_mock(&mut app, ServiceType::Dms, Box::new(task(None)));
    app.resources = vec![Box::new(task(None)), Box::new(instance()), Box::new(endpoint())];
    app.details_focused = false;
    for (key, want) in [('1', "DMS Task"), ('2', "DMS Replication Instance"), ('3', "DMS Endpoint")] {
        app.handle_event(Event::Key(KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE)), &tx)
            .await
            .unwrap();
        let types: Vec<&str> = app
            .filtered_resources
            .iter()
            .map(|&i| app.resources[i].resource_type())
            .collect();
        assert_eq!(types, [want], "tab {key}");
    }
}
