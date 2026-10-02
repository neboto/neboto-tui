//! `s` shell sessions run the `aws` CLI on the base profile; no flag can
//! reproduce an assumed org-role session. While a role is assumed, `s` must
//! say so rather than open a session against the wrong account (#85).

use super::*;
use crate::aws::client::AssumedOrgRole;
use crate::aws::services::ec2::Ec2Instance;
use crate::aws::services::ecs::EcsTask;

fn assume(app: &mut App) {
    app.aws_clients.set_assumed_role_for_test(AssumedOrgRole {
        account_id: "210987654321".to_string(),
        account_name: "member".to_string(),
        role_name: "OrganizationAccountAccessRole".to_string(),
    });
}

async fn press_s(app: &mut App, tx: &mpsc::UnboundedSender<Event>) {
    app.handle_event(Event::Key(key(KeyCode::Char('s'))), tx).await.unwrap();
}

fn instance() -> Box<dyn Resource> {
    Box::new(Ec2Instance::from_sdk(
        &aws_sdk_ec2::types::Instance::builder().instance_id("i-0aaa").build(),
    ))
}

#[tokio::test]
async fn ssm_session_opens_normally_but_not_under_an_assumed_role() {
    let (mut app, tx, _rx) = test_app().await;
    select_mock(&mut app, ServiceType::EC2, instance());
    press_s(&mut app, &tx).await;
    assert!(app.ssm_session_modal.visible, "base credentials: s opens the session menu");
    app.ssm_session_modal.hide();

    assume(&mut app);
    app.error_message = None;
    press_s(&mut app, &tx).await;
    assert!(!app.ssm_session_modal.visible);
    assert!(!app.session_requested && app.pending_session_cmd.is_none());
    let err = app.error_message.as_deref().unwrap_or_default();
    assert!(err.contains("org role is assumed"), "{err}");
}

#[tokio::test]
async fn ecs_exec_is_blocked_under_an_assumed_role() {
    let (mut app, tx, _rx) = test_app().await;
    let task = EcsTask::from_sdk(
        &aws_sdk_ecs::types::Task::builder()
            .task_arn("arn:aws:ecs:us-east-1:123456789012:task/c/0123456789abcdef0123456789abcdef")
            .cluster_arn("arn:aws:ecs:us-east-1:123456789012:cluster/c")
            .last_status("RUNNING")
            .containers(
                aws_sdk_ecs::types::Container::builder()
                    .name("app")
                    .last_status("RUNNING")
                    .build(),
            )
            .build(),
    );
    select_mock(&mut app, ServiceType::ECS, Box::new(task));
    assume(&mut app);
    press_s(&mut app, &tx).await;
    assert!(!app.session_requested && app.pending_session_cmd.is_none());
    let err = app.error_message.as_deref().unwrap_or_default();
    assert!(err.contains("org role is assumed"), "{err}");
}
