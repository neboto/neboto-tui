//! Elastic Beanstalk (issue #89): a Cluster Mode environment's Overview
//! names its EKS cluster, the Resources section fetches lazily, the row
//! classifier routes the cluster / ASG / version rows, and the environment
//! ARN jump lands on the Environments tab.

use super::*;
use crate::app::{BeanstalkView, JumpView};
use crate::aws::services::beanstalk::{EbEnvironment, EbEnvironmentDetailSection, EbResources};
use crate::lazy::Lazy;

const CLUSTER_ARN: &str = "arn:aws:eks:us-east-1:123456789012:cluster/eb-shared";

fn env(cluster: bool) -> EbEnvironment {
    use aws_sdk_elasticbeanstalk::types::{EnvironmentDescription, EnvironmentHealth, EnvironmentStatus, EnvironmentTier};
    let (name, ty) = if cluster { ("Cluster", "EKS") } else { ("WebServer", "Standard") };
    let mut e = EbEnvironment::from_sdk(
        &EnvironmentDescription::builder()
            .environment_id("e-abc123")
            .environment_name("portal-next")
            .application_name("acme-portal")
            .version_label("v2.0.0")
            .tier(EnvironmentTier::builder().name(name).r#type(ty).build())
            .status(EnvironmentStatus::Ready)
            .health(EnvironmentHealth::Green)
            .build(),
    );
    if cluster {
        e.cluster_arn = Some(CLUSTER_ARN.to_string());
    }
    e
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
async fn cluster_environment_overview_names_its_eks_cluster() {
    let (mut app, tx, _rx) = test_app().await;
    select_mock(&mut app, ServiceType::Beanstalk, Box::new(env(true)));
    app.details_focused = true;
    app.layout_mode = crate::app::LayoutMode::DetailsOnly;
    app.reset_detail_section_to_default(&tx);

    let screen = screen_of(&app);
    for needle in ["Beanstalk Environment", "Cluster", "eb-shared", "green"] {
        assert!(screen.contains(needle), "missing {needle:?} in:\n{screen}");
    }

    // The cluster row crosses to @eks by name; the version row stays in @eb
    // and carries the application, since labels repeat across apps.
    let t = app.eb_row_jump_target("  EKS Cluster", "eb-shared").expect("cluster row");
    assert_eq!(t.service, ServiceType::Eks);
    assert_eq!(t.id, "eb-shared");
    let t = app.eb_row_jump_target("  Version Label", "v2.0.0").expect("version row");
    assert_eq!(t.service, ServiceType::Beanstalk);
    assert!(matches!(t.view, JumpView::Beanstalk(BeanstalkView::Versions)));
    assert_eq!(t.id, "acme-portal@v2.0.0");
    assert!(app.eb_row_jump_target("  EKS Cluster", "· not resolved").is_none());
}

#[tokio::test]
async fn resources_section_fetches_lazily_and_routes_the_asg() {
    let (mut app, tx, _rx) = test_app().await;
    select_mock(&mut app, ServiceType::Beanstalk, Box::new(env(false)));
    app.details_focused = true;
    app.layout_mode = crate::app::LayoutMode::DetailsOnly;
    app.reset_detail_section_to_default(&tx);
    assert!(app.lazy.eb_resources.get("e-abc123").is_none(), "Overview doesn't fetch resources");

    app.detail_section_idx = EbEnvironmentDetailSection::Resources as usize;
    app.trigger_eb_resources_load(&tx);
    assert!(matches!(app.lazy.eb_resources.get("e-abc123"), Some(Lazy::Loading)));

    app.lazy.eb_resources.apply(
        "e-abc123".to_string(),
        Ok(EbResources {
            auto_scaling_groups: vec!["awseb-e-abc123-stack-AWSEBAutoScalingGroup-XYZ".into()],
            instances: vec!["i-0123456789abcdef0".into()],
            ..Default::default()
        }),
    );
    let screen = screen_of(&app);
    assert!(screen.contains("AWSEBAutoScalingGroup-XYZ"), "ASG row:\n{screen}");
    assert!(screen.contains("i-0123456789abcdef0"), "instance row:\n{screen}");

    let t = app
        .eb_row_jump_target("  Auto Scaling Group", "awseb-e-abc123-stack-AWSEBAutoScalingGroup-XYZ")
        .expect("asg row");
    assert_eq!(t.service, ServiceType::Asg);
}

#[test]
fn environment_arn_jumps_to_the_environments_tab_by_name() {
    use crate::ui::widgets::details_pane::resource_jump_target;
    let t = resource_jump_target(
        "ARN",
        "arn:aws:elasticbeanstalk:us-east-1:123456789012:environment/acme-portal/portal-next",
        ServiceType::Beanstalk,
    )
    .expect("env arn");
    assert_eq!(t.service, ServiceType::Beanstalk);
    assert!(matches!(t.view, JumpView::Beanstalk(BeanstalkView::Environments)));
    assert_eq!(t.id, "portal-next");

    let t = resource_jump_target(
        "ARN",
        "arn:aws:elasticbeanstalk:us-east-1:123456789012:applicationversion/acme-portal/v1.14.2",
        ServiceType::Beanstalk,
    )
    .expect("version arn");
    assert_eq!(t.id, "acme-portal@v1.14.2");
}
