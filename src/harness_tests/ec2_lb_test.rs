//! The instance pane's Load Balancing section: renders the target groups an
//! instance is registered in (`elb::fetch_instance_lb_membership`). Drives
//! the lazy map with real rows — member, not-a-member and error arms — and
//! reads the screen, plus the `⏎` jump from a Target Group row.

use super::*;
use crate::aws::services::ec2::{Ec2Instance, Ec2InstanceDetailSection};
use crate::aws::services::elb::{InstanceLbInfo, InstanceLbMembership};
use crate::lazy::Lazy;

const INSTANCE_ID: &str = "i-0123456789abcdef0";
const TG_ARN: &str =
    "arn:aws:elasticloadbalancing:us-east-1:123456789012:targetgroup/web-tg/0123456789abcdef";
const LB_ARN: &str =
    "arn:aws:elasticloadbalancing:us-east-1:123456789012:loadbalancer/app/web-alb/0123456789abcdef";

async fn app_with_lb(result: std::result::Result<InstanceLbInfo, String>) -> App {
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
    app.detail_section_idx = Ec2InstanceDetailSection::LoadBalancing as usize;
    app.trigger_ec2_instance_lb_load(&tx);
    assert!(
        matches!(app.lazy.ec2_instance_lb.get(INSTANCE_ID), Some(Lazy::Loading)),
        "entering the section must start the fetch"
    );
    app.lazy.ec2_instance_lb.apply(INSTANCE_ID.to_string(), result);
    app
}

fn screen_of(app: &App) -> String {
    let backend = TestBackend::new(160, 40);
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

fn member() -> InstanceLbInfo {
    InstanceLbInfo {
        memberships: vec![InstanceLbMembership {
            target_group_arn: TG_ARN.to_string(),
            target_group_name: "web-tg".to_string(),
            protocol: "HTTP".to_string(),
            group_port: Some(80),
            target_port: Some(8080),
            availability_zone: Some("us-east-1a".to_string()),
            state: "unhealthy".to_string(),
            reason: Some("Target.Timeout".to_string()),
            description: Some("Request timed out".to_string()),
            load_balancer_arns: vec![LB_ARN.to_string()],
            matched_ip: None,
            via_asg: true,
        }],
        candidates: 3,
        checked: 3,
        asg_name: Some("web-asg".to_string()),
        asg_target_groups: 1,
        warnings: vec![],
    }
}

#[tokio::test]
async fn member_instance_shows_group_lb_and_health() {
    let app = app_with_lb(Ok(member())).await;
    let screen = screen_of(&app);
    for want in [
        "4 Load Balancing",
        "✓ ALB web-alb",
        "0/1 target healthy",
        "web-asg (1 target group attached)",
        "web-tg",
        "unhealthy — Target.Timeout",
        "HTTP:80 → instance :8080",
    ] {
        assert!(screen.contains(want), "{want:?} missing:\n{screen}");
    }
}

#[tokio::test]
async fn target_group_row_jumps_to_elb() {
    let app = app_with_lb(Ok(member())).await;
    let rows = app.get_detail_lines();
    let (k, v) = rows
        .iter()
        .find(|(k, _)| k == "Target Group")
        .expect("Target Group row");
    let t = crate::ui::widgets::details_pane::resource_jump_target(k, v, ServiceType::EC2)
        .expect("Target Group row must be jumpable");
    assert_eq!(t.service, ServiceType::Elb);
    assert_eq!(t.id, "web-tg");
}

#[tokio::test]
async fn non_member_and_error_arms() {
    let app = app_with_lb(Ok(InstanceLbInfo {
        checked: 5,
        candidates: 5,
        ..Default::default()
    }))
    .await;
    let screen = screen_of(&app);
    assert!(screen.contains("✗ No"), "not-a-member row missing:\n{screen}");
    assert!(screen.contains("5 same-VPC target groups"), "checked row missing:\n{screen}");

    let app = app_with_lb(Err("Target groups unavailable: AccessDenied".to_string())).await;
    let screen = screen_of(&app);
    assert!(screen.contains("AccessDenied"), "error row missing:\n{screen}");
}
