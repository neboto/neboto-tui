//! DataSync (issue #96): the task pane's Overview with its lazy "last
//! execution" block, the Executions tab, a location's sibling-filtered Tasks,
//! the ARN jumps between the three tabs, and the `m` / `t` gates.

use super::*;
use crate::app::DataSyncView;
use crate::aws::services::datasync::{
    DataSyncAgent, DataSyncExecution, DataSyncExecutions, DataSyncLocation, DataSyncLocationDetailSection, DataSyncTask,
    DataSyncTaskDetailSection, LocationKind,
};
use crate::lazy::Lazy;
use crate::ui::widgets::details_pane::resource_jump_target;
use std::collections::HashMap;

const TASK_ARN: &str = "arn:aws:datasync:us-east-1:123456789012:task/task-0aaa";
const SRC_ARN: &str = "arn:aws:datasync:us-east-1:123456789012:location/loc-0src";
const DST_ARN: &str = "arn:aws:datasync:us-east-1:123456789012:location/loc-0dst";
const EXEC_ARN: &str = "arn:aws:datasync:us-east-1:123456789012:task/task-0aaa/execution/exec-0new";

fn task() -> DataSyncTask {
    DataSyncTask {
        arn: TASK_ARN.into(),
        name: "nightly-archive".into(),
        status: "AVAILABLE".into(),
        mode: "ENHANCED".into(),
        source_arn: SRC_ARN.into(),
        destination_arn: DST_ARN.into(),
        source_uri: Some("nfs://nas.corp.example/export".into()),
        destination_uri: Some("s3://archive-bucket/nightly/".into()),
        log_group_arn: Some("arn:aws:logs:us-east-1:123456789012:log-group:/aws/datasync:*".into()),
        schedule: Some("cron(0 2 * * ? *)".into()),
        schedule_status: Some("DISABLED".into()),
        schedule_disabled_by: Some("SERVICE".into()),
        schedule_disabled_reason: Some("Repeated execution failures".into()),
        described: true,
        ..Default::default()
    }
}

fn location(arn: &str, uri: &str) -> DataSyncLocation {
    DataSyncLocation {
        arn: arn.into(),
        uri: uri.into(),
        kind: LocationKind::from_uri(uri),
        tags: HashMap::new(),
    }
}

fn execution(id: &str, status: &str, start_secs: i64) -> DataSyncExecution {
    DataSyncExecution {
        arn: format!("{TASK_ARN}/execution/{id}"),
        status: status.into(),
        start_secs,
        files_transferred: 1200,
        files_estimated: 1500,
        bytes_transferred: 5 * 1024 * 1024 * 1024,
        error_code: (status == "ERROR").then(|| "SourceUnreachable".to_string()),
        ..Default::default()
    }
}

fn screen_of(app: &App) -> String {
    let backend = TestBackend::new(170, 60);
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
async fn task_overview_fetches_the_last_execution_and_flags_a_service_disabled_schedule() {
    let (mut app, tx, _rx) = test_app().await;
    select_mock(&mut app, ServiceType::DataSync, Box::new(task()));
    app.details_focused = true;
    app.layout_mode = crate::app::LayoutMode::DetailsOnly;
    app.reset_detail_section_to_default(&tx);
    // Drilling in fires the Overview's hook — the same fetch the tab uses.
    assert!(matches!(app.lazy.datasync_executions.get(TASK_ARN), Some(Lazy::Loading)));

    app.lazy.datasync_executions.apply(
        TASK_ARN.to_string(),
        Ok(DataSyncExecutions {
            runs: vec![execution("exec-0new", "ERROR", 200), execution("exec-0old", "SUCCESS", 100)],
            total: 2,
        }),
    );
    let screen = screen_of(&app);
    for needle in [
        "DataSync Task",
        "nightly-archive",
        "✗ disabled by DataSync",
        "Repeated execution failures",
        "nfs://nas.corp.example/export",
        "s3://archive-bucket/nightly/",
        "exec-0new",
        "✗ SourceUnreachable",
        "1200 of 1500 estimated",
        "1 more — 2 Executions",
        "/aws/datasync (t to tail)",
    ] {
        assert!(screen.contains(needle), "missing {needle:?} in:\n{screen}");
    }
    assert!(!screen.contains("exec-0old"), "the Overview previews only the last run");

    assert!(app.supports_metrics_overlay());
    assert!(app.supports_log_tail());

    // The Executions tab lists both, newest first, from the same fetch.
    app.detail_section_idx = DataSyncTaskDetailSection::Executions as usize;
    let lines = app.get_detail_lines();
    let pos = |id: &str| lines.iter().position(|(k, _)| k == id).unwrap_or_else(|| panic!("{id}: {lines:?}"));
    assert!(pos("exec-0new") < pos("exec-0old"));
    assert!(lines.iter().any(|(k, v)| k == "Failed" && v.starts_with("✗ 1")), "{lines:?}");
}

#[tokio::test]
async fn a_task_without_a_log_group_has_nothing_to_tail() {
    let (mut app, _tx, _rx) = test_app().await;
    let mut t = task();
    t.log_group_arn = None;
    select_mock(&mut app, ServiceType::DataSync, Box::new(t));
    assert!(!app.supports_log_tail());
    assert!(app.supports_metrics_overlay());
}

#[tokio::test]
async fn location_lists_the_tasks_using_it_and_configuration_is_lazy() {
    let (mut app, tx, _rx) = test_app().await;
    let src = location(SRC_ARN, "nfs://nas.corp.example/export");
    select_mock(&mut app, ServiceType::DataSync, Box::new(src.clone()));
    app.resources = vec![
        Box::new(src),
        Box::new(location(DST_ARN, "s3://archive-bucket/nightly/")),
        Box::new(task()),
    ];
    app.filtered_resources = vec![0];
    app.selected_index = Some(0);
    app.details_focused = true;
    app.layout_mode = crate::app::LayoutMode::DetailsOnly;
    app.reset_detail_section_to_default(&tx);

    let lines = app.get_detail_lines();
    assert!(lines.iter().any(|(k, v)| k == "Used By" && v == "1 task"), "{lines:?}");
    assert!(lines.iter().any(|(k, _)| k == "nightly-archive (source)"), "{lines:?}");
    assert!(lines.iter().any(|(_, v)| v == TASK_ARN));
    assert!(app.lazy.datasync_location_config.get(SRC_ARN).is_none(), "Overview doesn't describe");

    app.detail_section_idx = DataSyncLocationDetailSection::Configuration as usize;
    app.trigger_datasync_location_config_load(&tx);
    assert!(matches!(app.lazy.datasync_location_config.get(SRC_ARN), Some(Lazy::Loading)));
    app.lazy.datasync_location_config.apply(
        SRC_ARN.to_string(),
        Ok(vec![("NFS Version".to_string(), "NFS4_1".to_string())]),
    );
    let lines = app.get_detail_lines();
    assert!(lines.iter().any(|(k, v)| k == "NFS Version" && v == "NFS4_1"), "{lines:?}");

    // A location nothing uses says so.
    select_mock(&mut app, ServiceType::DataSync, Box::new(location(DST_ARN, "s3://orphan/")));
    app.detail_section_idx = 0;
    let lines = app.get_detail_lines();
    assert!(lines.iter().any(|(k, v)| k == "Used By" && v.starts_with('⚠')), "{lines:?}");
}

#[test]
fn datasync_arns_jump_to_their_tab_and_an_execution_lands_on_its_task() {
    use crate::app::JumpView;
    for (arn, view, id) in [
        (TASK_ARN, DataSyncView::Tasks, TASK_ARN),
        (SRC_ARN, DataSyncView::Locations, SRC_ARN),
        (
            "arn:aws:datasync:us-east-1:123456789012:agent/agent-0abc",
            DataSyncView::Agents,
            "arn:aws:datasync:us-east-1:123456789012:agent/agent-0abc",
        ),
        (EXEC_ARN, DataSyncView::Tasks, TASK_ARN),
    ] {
        let target = resource_jump_target("  ARN", arn, ServiceType::DataSync).unwrap_or_else(|| panic!("{arn}"));
        assert_eq!(target.service, ServiceType::DataSync, "{arn}");
        assert_eq!(target.id, id, "{arn}");
        assert!(matches!(target.view, JumpView::DataSync(v) if v == view), "{arn}");
    }
}

#[tokio::test]
async fn sub_tabs_filter_by_type() {
    let (mut app, tx, _rx) = test_app().await;
    select_mock(&mut app, ServiceType::DataSync, Box::new(task()));
    app.resources = vec![
        Box::new(task()),
        Box::new(location(SRC_ARN, "nfs://nas.corp.example/export")),
        Box::new(DataSyncAgent { arn: "arn:aws:datasync:us-east-1:123456789012:agent/agent-0abc".into(), ..Default::default() }),
    ];
    app.details_focused = false;
    for (key, want) in [('1', "DataSync Task"), ('2', "DataSync Location"), ('3', "DataSync Agent")] {
        app.handle_event(Event::Key(KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE)), &tx)
            .await
            .unwrap();
        let types: Vec<&str> = app
            .filtered_resources
            .iter()
            .map(|&i| app.resources[i].resource_type())
            .collect();
        assert_eq!(types, [want], "key {key}");
    }
}
