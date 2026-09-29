//! Batch (issue #42): the stuck-queue verdict across queue / compute
//! environment / job, the Jobs `f` status groups, the job pane's failure
//! detail + log target, and ARN jumps between the sub-tabs.

use super::*;
use crate::aws::services::batch::{
    BatchCeDetailSection, BatchComputeEnv, BatchJob, BatchJobDetailSection, BatchJobQueue,
    BatchQueueDetailSection,
};
use aws_sdk_batch::types::*;

const ACCT: &str = "arn:aws:batch:us-east-1:123456789012";

fn queue() -> BatchJobQueue {
    BatchJobQueue::from_sdk(
        &JobQueueDetail::builder()
            .job_queue_name("etl")
            .job_queue_arn(format!("{ACCT}:job-queue/etl"))
            .state(JqState::Enabled)
            .status(JqStatus::Valid)
            .priority(10)
            .compute_environment_order(
                ComputeEnvironmentOrder::builder()
                    .order(1)
                    .compute_environment(format!("{ACCT}:compute-environment/spot-ce"))
                    .build(),
            )
            .build(),
    )
}

fn ce(desired: i32) -> BatchComputeEnv {
    BatchComputeEnv::from_sdk(
        &ComputeEnvironmentDetail::builder()
            .compute_environment_name("spot-ce")
            .compute_environment_arn(format!("{ACCT}:compute-environment/spot-ce"))
            .r#type(CeType::Managed)
            .state(CeState::Enabled)
            .status(CeStatus::Valid)
            .compute_resources(
                ComputeResource::builder()
                    .r#type(CrType::Spot)
                    .minv_cpus(0)
                    .desiredv_cpus(desired)
                    .maxv_cpus(256)
                    .instance_types("optimal")
                    .subnets("subnet-0abc")
                    .security_group_ids("sg-0abc")
                    .build(),
            )
            .build(),
    )
}

fn job(id: &str, status: JobStatus) -> BatchJob {
    let mut b = JobDetail::builder()
        .job_id(id)
        .job_arn(format!("{ACCT}:job/{id}"))
        .job_name(format!("nightly-{id}"))
        .job_queue(format!("{ACCT}:job-queue/etl"))
        .job_definition(format!("{ACCT}:job-definition/nightly:3"))
        .status(status.clone())
        .created_at(1_700_000_000_000);
    if status == JobStatus::Failed {
        b = b
            .status_reason("Essential container in task exited")
            .started_at(1_700_000_060_000)
            .stopped_at(1_700_000_120_000)
            .container(
                ContainerDetail::builder()
                    .image("123456789012.dkr.ecr.us-east-1.amazonaws.com/etl:latest")
                    .exit_code(137)
                    .reason("OutOfMemoryError: Container killed due to memory usage")
                    .log_stream_name("nightly/default/abc123")
                    .log_configuration(
                        LogConfiguration::builder()
                            .log_driver(LogDriver::Awslogs)
                            .options("awslogs-group", "/etl/jobs")
                            .build(),
                    )
                    .build(),
            )
            .attempts(
                AttemptDetail::builder()
                    .started_at(1_700_000_060_000)
                    .stopped_at(1_700_000_120_000)
                    .container(
                        AttemptContainerDetail::builder()
                            .exit_code(137)
                            .reason("OutOfMemoryError: Container killed due to memory usage")
                            .build(),
                    )
                    .build(),
            );
    }
    BatchJob::from_sdk(&b.build())
}

fn install(app: &mut App, view: crate::app::BatchView, selected: usize, desired: i32) {
    select_mock(app, ServiceType::Batch, Box::new(queue()));
    app.resources = vec![
        Box::new(queue()),
        Box::new(ce(desired)),
        Box::new(job("j-run", JobStatus::Runnable)),
        Box::new(job("j-fail", JobStatus::Failed)),
        Box::new(job("j-ok", JobStatus::Running)),
    ];
    app.batch_view = view;
    app.update_search();
    app.selected_index = Some(selected);
}

#[tokio::test]
async fn stuck_queue_verdict_names_the_compute_environment() {
    let (mut app, tx, _rx) = test_app().await;
    install(&mut app, crate::app::BatchView::ComputeEnvironments, 0, 0);
    app.details_focused = true;
    app.reset_detail_section_to_default(&tx);
    assert_eq!(app.detail_section_idx, BatchCeDetailSection::Overview as usize);
    let lines = app.get_detail_lines();
    assert!(
        lines.iter().any(|(_, v)| v.starts_with("⚠ desired vCPU 0 with 1 RUNNABLE job")),
        "{lines:?}"
    );
    app.detail_section_idx = BatchCeDetailSection::Queues as usize;
    let lines = app.get_detail_lines();
    assert!(lines.iter().any(|(k, v)| k.trim() == "RUNNABLE Jobs" && v == "⚠ 1"), "{lines:?}");

    // The queue sees the same thing from its side.
    app.batch_view = crate::app::BatchView::Queues;
    app.update_search();
    app.selected_index = Some(0);
    app.detail_section_idx = BatchQueueDetailSection::Overview as usize;
    let lines = app.get_detail_lines();
    assert!(
        lines.iter().any(|(_, v)| v.contains("no compute environment can take them")),
        "{lines:?}"
    );

    // Once it scales, the verdict softens to a plain count.
    install(&mut app, crate::app::BatchView::Queues, 0, 4);
    app.detail_section_idx = BatchQueueDetailSection::Overview as usize;
    let lines = app.get_detail_lines();
    assert!(lines.iter().any(|(_, v)| v == "· 1 RUNNABLE job(s) waiting"), "{lines:?}");
}

#[tokio::test]
async fn f_cycles_job_status_groups() {
    let (mut app, tx, _rx) = test_app().await;
    install(&mut app, crate::app::BatchView::Jobs, 0, 0);
    let names = |app: &App| {
        app.filtered_resources
            .iter()
            .map(|&i| app.resources[i].id().to_string())
            .collect::<Vec<_>>()
    };
    assert_eq!(names(&app).len(), 3);
    let press = KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE);
    app.handle_event(Event::Key(press), &tx).await.unwrap();
    assert_eq!(app.batch_job_filter, crate::app::BatchJobStatusFilter::Active);
    assert_eq!(names(&app), vec!["j-run", "j-ok"]);
    app.handle_event(Event::Key(press), &tx).await.unwrap();
    assert_eq!(names(&app), vec!["j-fail"]);
    let screen = {
        let backend = TestBackend::new(170, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| crate::render_app(&app, f)).unwrap();
        let buf = terminal.backend().buffer().clone();
        (0..buf.area.height)
            .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    };
    assert!(screen.contains("status: failed"), "{screen}");
}

#[tokio::test]
async fn failed_job_pane_shows_the_cause_and_tails_its_stream() {
    let (mut app, tx, _rx) = test_app().await;
    let j = job("j-fail", JobStatus::Failed);
    assert_eq!(j.log_target(), Some(("/etl/jobs".to_string(), "nightly/default/abc123".to_string())));
    select_mock(&mut app, ServiceType::Batch, Box::new(j));
    app.details_focused = true;
    app.reset_detail_section_to_default(&tx);

    let lines = app.get_detail_lines();
    assert!(lines.iter().any(|(k, v)| k == "Status Reason" && v.starts_with("✗ Essential")), "{lines:?}");
    assert!(lines.iter().any(|(k, v)| k == "Ran For" && v == "1m 00s"), "{lines:?}");

    app.detail_section_idx = BatchJobDetailSection::Container as usize;
    let lines = app.get_detail_lines();
    assert!(lines.iter().any(|(k, v)| k == "Exit Code" && v == "✗ 137"), "{lines:?}");
    assert!(lines.iter().any(|(k, v)| k == "Log Group" && v == "/etl/jobs"), "{lines:?}");

    app.detail_section_idx = BatchJobDetailSection::Attempts as usize;
    let lines = app.get_detail_lines();
    assert!(lines.iter().any(|(k, v)| k.trim() == "Reason" && v.contains("OutOfMemoryError")), "{lines:?}");

    assert!(app.supports_log_tail());
    // A job that hasn't started has no stream — `t` says so instead of
    // tailing the whole default group.
    assert_eq!(job("j-run", JobStatus::Runnable).log_target(), None);
}

#[tokio::test]
async fn job_rows_jump_to_their_queue_and_definition() {
    use crate::app::{BatchView, JumpView};
    use crate::ui::widgets::details_pane::resource_jump_target;
    let j = job("j-fail", JobStatus::Failed);
    let lines = crate::ui::widgets::details_pane::batch_job_section_lines(&j, BatchJobDetailSection::Overview);
    let row = |key: &str| lines.iter().find(|(k, _)| k == key).cloned().expect(key);

    let (k, v) = row("Queue");
    let t = resource_jump_target(&k, &v, ServiceType::Batch).expect("queue jumpable");
    assert_eq!((t.service, t.id.as_str()), (ServiceType::Batch, "etl"));
    assert!(matches!(t.view, JumpView::Batch(BatchView::Queues)));

    let (k, v) = row("Job Definition");
    let t = resource_jump_target(&k, &v, ServiceType::Batch).expect("definition jumpable");
    assert_eq!(t.id, "nightly:3");
    assert!(matches!(t.view, JumpView::Batch(BatchView::JobDefinitions)));

    // The queue's Jobs section lists job ARNs, which land on the Jobs tab.
    let q = queue();
    let jobs = [&j];
    let lines = crate::ui::widgets::details_pane::batch_queue_section_lines(
        &q,
        BatchQueueDetailSection::Jobs,
        &[],
        &jobs,
    );
    let (k, v) = lines.iter().find(|(k, _)| k.trim() == "nightly-j-fail").cloned().expect("job row");
    let t = resource_jump_target(&k, &v, ServiceType::Batch).expect("job jumpable");
    assert_eq!(t.id, "j-fail");
    assert!(matches!(t.view, JumpView::Batch(BatchView::Jobs)));
}
