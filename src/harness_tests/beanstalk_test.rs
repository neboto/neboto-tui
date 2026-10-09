//! Elastic Beanstalk (issue #89): a Cluster Mode environment's Overview
//! names its EKS cluster, the Resources section fetches lazily, the row
//! classifier routes the cluster / ASG / version rows, and the environment
//! ARN jump lands on the Environments tab; `U` on the EKS cluster lists
//! the environments sharing it.

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

fn named_env(id: &str, name: &str, cluster: bool) -> EbEnvironment {
    let mut e = env(cluster);
    e.id = id.to_string();
    e.name = name.to_string();
    e
}

/// The cross-service feature of #89: `U` on an EKS cluster lists the
/// Cluster Mode environments that share it, off the warm Beanstalk cache
/// (the list load resolves each Cluster env's `ClusterArn` eagerly), and
/// leaves Standard environments out.
#[tokio::test]
async fn refs_lens_on_an_eks_cluster_lists_the_beanstalk_environments_on_it() {
    let (mut app, tx, _rx) = test_app().await;
    let region = app.current_region;
    app.cache.insert(
        ServiceType::Beanstalk,
        region,
        None,
        vec![
            Box::new(named_env("e-next01", "portal-next", true)),
            Box::new(named_env("e-next02", "portal-next-canary", true)),
            Box::new(named_env("e-legacy", "portal-legacy", false)),
        ],
    );
    let cluster = crate::aws::services::eks::EksCluster::from_sdk(
        &aws_sdk_eks::types::Cluster::builder()
            .name("eb-shared")
            .arn(CLUSTER_ARN)
            .status(aws_sdk_eks::types::ClusterStatus::Active)
            .build(),
    );
    select_mock(&mut app, ServiceType::Eks, Box::new(cluster));

    app.handle_key(KeyEvent::new(KeyCode::Char('U'), KeyModifiers::SHIFT), &tx)
        .await
        .unwrap();
    let st = app.refs_in_pane.as_ref().expect("refs lens opens");
    let all: Vec<(&str, &str)> = st.all_rows.iter().map(|r| (r.name.as_str(), r.via.as_str())).collect();
    let eb: Vec<&crate::references::RefRow> =
        st.all_rows.iter().filter(|r| r.service == ServiceType::Beanstalk).collect();
    let mut names: Vec<&str> = eb.iter().map(|r| r.name.as_str()).collect();
    names.sort();
    assert_eq!(names, vec!["portal-next", "portal-next-canary"], "rows: {all:?}");
    assert!(eb.iter().all(|r| r.via.starts_with("EKS Cluster")), "via: {all:?}");

    let screen = screen_of(&app);
    assert!(screen.contains("portal-next-canary"), "lens shows the env:\n{screen}");
    assert!(!screen.contains("portal-legacy"), "Standard env is not on the cluster:\n{screen}");
}

/// The application pane's Environments section labels the version row with
/// `EB_ROW_VERSION`, so it jumps to the Versions tab keyed `app@label`.
#[tokio::test]
async fn application_pane_version_row_jumps_to_the_version() {
    use crate::aws::services::beanstalk::{EbApplication, EbApplicationDetailSection};
    use crate::ui::widgets::details_pane::{eb_application_section_lines, EB_ROW_VERSION};
    let (mut app, _tx, _rx) = test_app().await;
    let a = EbApplication::from_sdk(
        &aws_sdk_elasticbeanstalk::types::ApplicationDescription::builder()
            .application_name("acme-portal")
            .build(),
    );
    let e = env(true);
    let rows = eb_application_section_lines(&a, EbApplicationDetailSection::Environments, &[&e], &[]);
    let (k, v) = rows
        .iter()
        .find(|(k, _)| k.trim() == EB_ROW_VERSION)
        .expect("version row uses EB_ROW_VERSION");
    assert_eq!(v, "v2.0.0");

    select_mock(&mut app, ServiceType::Beanstalk, Box::new(a));
    let t = app.eb_row_jump_target(k, v).expect("version row jumps");
    assert!(matches!(t.view, JumpView::Beanstalk(BeanstalkView::Versions)));
    assert_eq!(t.id, "acme-portal@v2.0.0");
}

/// Unmarked event rows are padded to the ⚠/✗ marker's width, so the time
/// column starts at the same offset on every row.
#[test]
fn events_rows_align_across_severities() {
    use crate::aws::services::beanstalk::EbEvent;
    use crate::ui::widgets::details_pane::{eb_environment_section_lines, EbEnvLazy};
    let evs = Lazy::Loaded(
        ["INFO", "WARN", "ERROR"]
            .into_iter()
            .map(|sev| EbEvent {
                time: Some("2026-10-09 10:00:00".into()),
                severity: sev.into(),
                message: "m".into(),
            })
            .collect::<Vec<_>>(),
    );
    let rows = eb_environment_section_lines(
        &env(false),
        EbEnvironmentDetailSection::Events,
        EbEnvLazy { health: None, events: Some(&evs), config: None, resources: None },
    );
    let offsets: Vec<usize> = rows
        .iter()
        .filter_map(|(k, _)| k.find("2026-10-09").map(|i| k[..i].chars().count()))
        .collect();
    assert_eq!(offsets.len(), 3, "rows: {rows:?}");
    assert!(offsets.iter().all(|&o| o == offsets[0]), "offsets {offsets:?} in {rows:?}");
}
