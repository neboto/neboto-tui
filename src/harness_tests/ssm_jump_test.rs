//! SSM rows the generic classifier can't route. A Fleet node's own `i-` id
//! was dropped by the self-jump filter (its `id()` *is* the instance id, but
//! the EC2 instance is another resource), hybrid `mi-` nodes have no EC2
//! instance at all (the generic classifier sends them to Fleet from any
//! service), and several rows name things by bare name.

use super::*;
use crate::app::{CwView, Ec2View, IamView, JumpView, SsmView};
use crate::aws::services::ssm::{
    SsmCommand, SsmManagedInstance, SsmMaintWindow, SsmOpsItem, SsmParameter,
};

const EC2_ID: &str = "i-0123456789abcdef0";
const HYBRID_ID: &str = "mi-0123456789abcdef0";

fn node(id: &str) -> SsmManagedInstance {
    SsmManagedInstance::from_sdk(
        &aws_sdk_ssm::types::InstanceInformation::builder()
            .instance_id(id)
            .iam_role("SSMServiceRole")
            .build(),
    )
}

fn command() -> SsmCommand {
    SsmCommand::from_sdk(
        &aws_sdk_ssm::types::Command::builder()
            .command_id("00000000-0000-0000-0000-000000000001")
            .document_name("AWS-RunShellScript")
            .build(),
    )
}

fn target(
    app: &mut App,
    resource: Box<dyn Resource>,
    key: &str,
    value: &str,
) -> Option<crate::app::JumpTarget> {
    select_mock(app, ServiceType::Ssm, resource);
    app.ssm_row_jump_target(key, value)
}

#[tokio::test]
async fn a_fleet_nodes_instance_id_goes_to_ec2() {
    let (mut app, tx, _rx) = test_app().await;
    select_mock(&mut app, ServiceType::Ssm, Box::new(node(EC2_ID)));
    app.ssm_view = SsmView::Fleet;
    app.details_focused = true;
    app.reset_detail_section_to_default(&tx);
    let rows = app.get_detail_lines_filtered();
    let row = rows
        .iter()
        .position(|(k, v)| k == "Instance ID" && v == EC2_ID)
        .unwrap_or_else(|| panic!("Instance ID row in {rows:?}"));
    app.details_selected_index = Some(row);
    app.handle_event(Event::Key(key(KeyCode::Enter)), &tx).await.unwrap();

    assert_eq!(app.current_service, Some(ServiceType::EC2));
    assert_eq!(app.ec2_view, Ec2View::Instances);
    assert_eq!(app.pending_jump.as_deref(), Some(EC2_ID));
}

#[tokio::test]
async fn a_hybrid_node_never_jumps_to_itself() {
    let (mut app, tx, _rx) = test_app().await;
    select_mock(&mut app, ServiceType::Ssm, Box::new(node(HYBRID_ID)));
    app.ssm_view = SsmView::Fleet;
    app.details_focused = true;
    app.reset_detail_section_to_default(&tx);
    let rows = app.get_detail_lines_filtered();
    let row = rows.iter().position(|(k, _)| k == "Instance ID").unwrap();
    app.details_selected_index = Some(row);
    app.handle_event(Event::Key(key(KeyCode::Enter)), &tx).await.unwrap();

    assert_eq!(app.current_service, Some(ServiceType::Ssm));
    assert!(app.pending_jump.is_none());
}

/// The generic classifier, so a hybrid node id leads to Fleet from any
/// service — a CloudTrail event, an SSM command target, or an invocation's
/// group header (where the id is in the key).
#[test]
fn hybrid_node_ids_go_to_fleet_from_anywhere() {
    use crate::ui::widgets::details_pane::resource_jump_target;
    for (k, v, current) in [
        ("Resource", HYBRID_ID, ServiceType::CloudTrail),
        ("  Instance", HYBRID_ID, ServiceType::Ssm),
        (&*format!("{HYBRID_ID} (web-1)"), "", ServiceType::Ssm),
    ] {
        let t = resource_jump_target(k, v, current).expect(k);
        assert_eq!(t.service, ServiceType::Ssm);
        assert!(matches!(t.view, JumpView::Ssm(SsmView::Fleet)));
        assert_eq!(t.id, HYBRID_ID);
    }
    // Not hex / wrong length: left alone.
    assert!(resource_jump_target("Name", "mi-notanodeid", ServiceType::CloudTrail).is_none());
}

#[tokio::test]
async fn bare_names_route_to_their_service() {
    let (mut app, _tx, _rx) = test_app().await;

    let t = target(&mut app, Box::new(node(EC2_ID)), "  IAM Role", "SSMServiceRole").unwrap();
    assert!(matches!(t.view, JumpView::Iam(IamView::Roles)));

    let t = target(&mut app, Box::new(command()), "  Log Group", "/ssm/run").unwrap();
    assert!(matches!(t.view, JumpView::Cw(CwView::LogGroups)));

    let mw = SsmMaintWindow::from_sdk(
        &aws_sdk_ssm::types::MaintenanceWindowIdentity::builder()
            .window_id("mw-0123456789abcdef0")
            .build(),
    );
    let t = target(&mut app, Box::new(mw.clone()), "  Task", "AWS-RunPatchBaseline").unwrap();
    assert!(matches!(t.view, JumpView::Ssm(SsmView::Documents)));
    // Lambda / Step Functions task ARNs are left to the generic classifier.
    let arn = "arn:aws:lambda:us-east-1:123456789012:function:patch";
    assert!(target(&mut app, Box::new(mw), "  Task", arn).is_none());

    let item = SsmOpsItem::from_sdk(
        &aws_sdk_ssm::types::OpsItemSummary::builder().ops_item_id("oi-0123456789ab").build(),
    );
    let t = target(&mut app, Box::new(item), "  OpsItem", "oi-ba9876543210").unwrap();
    assert!(matches!(t.view, JumpView::Ssm(SsmView::OpsItems)));

    let param = SsmParameter::from_sdk(
        &aws_sdk_ssm::types::ParameterMetadata::builder()
            .name("/app/db-password")
            .r#type(aws_sdk_ssm::types::ParameterType::SecureString)
            .key_id("1234abcd-12ab-34cd-56ef-1234567890ab")
            .build(),
    );
    let t = target(
        &mut app,
        Box::new(param),
        "KMS Key",
        "1234abcd-12ab-34cd-56ef-1234567890ab",
    )
    .unwrap();
    assert_eq!(t.service, ServiceType::Kms);
}
