//! Amazon MQ (issue #96): the broker pane's Overview, the configuration's
//! sibling-filtered Used By and its lazy Document / Revisions, the jumps
//! between the two tabs, and the `m` / `t` gates.

use super::*;
use crate::app::{JumpView, MqView};
use crate::aws::services::mq::{
    MqBroker, MqBrokerDetailSection, MqConfigDetailSection, MqConfigDocument, MqConfiguration, MqInstance, MqRevision,
};
use crate::lazy::Lazy;
use crate::ui::widgets::details_pane::resource_jump_target;

const BROKER_ARN: &str = "arn:aws:mq:us-east-1:123456789012:broker:orders:b-1234abcd-56ef";
const CONFIG_ARN: &str = "arn:aws:mq:us-east-1:123456789012:configuration:c-9876fedc-54ba";

fn broker() -> MqBroker {
    MqBroker {
        arn: BROKER_ARN.into(),
        id: "b-1234abcd-56ef".into(),
        name: "orders".into(),
        status: "RUNNING".into(),
        engine: "ACTIVEMQ".into(),
        engine_version: Some("5.18".into()),
        pending_engine_version: Some("5.19".into()),
        deployment_mode: "ACTIVE_STANDBY_MULTI_AZ".into(),
        instance_type: "mq.m5.large".into(),
        publicly_accessible: Some(true),
        auth_strategy: Some("SIMPLE".into()),
        aws_owned_key: Some(true),
        subnets: vec!["subnet-0aaa".into(), "subnet-0bbb".into()],
        security_groups: vec!["sg-0aaa".into()],
        instances: vec![
            MqInstance {
                console_url: Some("https://b-1234abcd-56ef-1.mq.us-east-1.amazonaws.com:8162".into()),
                endpoints: vec!["ssl://b-1234abcd-56ef-1.mq.us-east-1.amazonaws.com:61617".into()],
                ip_address: Some("10.0.1.10".into()),
            },
            MqInstance {
                console_url: None,
                endpoints: vec!["ssl://b-1234abcd-56ef-2.mq.us-east-1.amazonaws.com:61617".into()],
                ip_address: Some("10.0.2.10".into()),
            },
        ],
        config_current: Some(("c-9876fedc-54ba".into(), 2)),
        logs_general: Some(true),
        general_log_group: Some("/aws/amazonmq/broker/b-1234abcd-56ef/general".into()),
        users: vec![("app".into(), None), ("ops".into(), Some("update".into()))],
        described: true,
        ..Default::default()
    }
}

fn config() -> MqConfiguration {
    MqConfiguration {
        arn: CONFIG_ARN.into(),
        id: "c-9876fedc-54ba".into(),
        name: "orders-config".into(),
        engine: "ACTIVEMQ".into(),
        engine_version: Some("5.18".into()),
        latest_revision: 3,
        ..Default::default()
    }
}

fn screen_of(app: &App) -> String {
    let backend = TestBackend::new(170, 60);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| crate::render_app(app, f)).unwrap();
    let buf = terminal.backend().buffer();
    (0..buf.area.height)
        .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn broker_overview_flags_public_access_and_pending_changes() {
    let (mut app, tx, _rx) = test_app().await;
    select_mock(&mut app, ServiceType::Mq, Box::new(broker()));
    app.details_focused = true;
    app.layout_mode = crate::app::LayoutMode::DetailsOnly;
    app.reset_detail_section_to_default(&tx);
    let screen = screen_of(&app);
    for needle in [
        "MQ Broker",
        "orders",
        "ActiveMQ 5.18",
        "active/standby multi-AZ",
        "⚠ yes — endpoints resolve to public addresses",
        "AWS owned key",
        "c-9876fedc-54ba",
        "⚠ 1 change waiting",
        "ssl://b-1234abcd-56ef-1.mq.us-east-1.amazonaws.com:61617",
        "/aws/amazonmq/broker/b-1234abcd-56ef/general (t to tail)",
    ] {
        assert!(screen.contains(needle), "missing {needle:?} in:\n{screen}");
    }
    assert!(app.supports_metrics_overlay());
    assert!(app.supports_log_tail());

    // ActiveMQ charts the first instance; RabbitMQ the broker name.
    let dims = |app: &App| app.selected_metric_target().unwrap().dimensions.clone();
    assert_eq!(dims(&app), [("Broker".to_string(), "orders-1".to_string())]);

    app.detail_section_idx = MqBrokerDetailSection::Users as usize;
    let lines = app.get_detail_lines();
    assert!(lines.iter().any(|(k, v)| k == "  ops" && v == "⚠ pending update"), "{lines:?}");

    app.detail_section_idx = MqBrokerDetailSection::Maintenance as usize;
    let lines = app.get_detail_lines();
    assert!(lines.iter().any(|(k, v)| k == "  Engine Version" && v == "⚠ → 5.19"), "{lines:?}");

    let mut rabbit = broker();
    rabbit.engine = "RABBITMQ".into();
    rabbit.users.clear();
    rabbit.logs_general = Some(false);
    select_mock(&mut app, ServiceType::Mq, Box::new(rabbit));
    assert_eq!(dims(&app), [("Broker".to_string(), "orders".to_string())]);
    assert!(!app.supports_log_tail(), "logging off: nothing to tail");
    app.detail_section_idx = MqBrokerDetailSection::Users as usize;
    let lines = app.get_detail_lines();
    assert!(lines.iter().any(|(_, v)| v.contains("RabbitMQ users live in the broker")), "{lines:?}");
}

#[tokio::test]
async fn configuration_lists_its_brokers_and_its_document_is_lazy() {
    let (mut app, tx, _rx) = test_app().await;
    select_mock(&mut app, ServiceType::Mq, Box::new(config()));
    app.resources = vec![Box::new(config()), Box::new(broker())];
    app.filtered_resources = vec![0];
    app.selected_index = Some(0);
    app.details_focused = true;
    app.layout_mode = crate::app::LayoutMode::DetailsOnly;
    app.reset_detail_section_to_default(&tx);

    let lines = app.get_detail_lines();
    assert!(lines.iter().any(|(k, v)| k == "  orders" && v == "b-1234abcd-56ef"), "{lines:?}");
    assert!(lines.iter().any(|(k, v)| k == "    Running" && v == "⚠ revision 2 — latest is 3"), "{lines:?}");
    assert!(app.lazy.mq_config_document.get("c-9876fedc-54ba").is_none(), "Overview doesn't fetch");

    app.set_detail_section(MqConfigDetailSection::Document as usize, &tx);
    assert!(matches!(app.lazy.mq_config_document.get("c-9876fedc-54ba"), Some(Lazy::Loading)));
    app.lazy.mq_config_document.apply(
        "c-9876fedc-54ba".to_string(),
        Ok(MqConfigDocument { revision: 3, lines: vec!["<broker>".into(), "</broker>".into()], ..Default::default() }),
    );
    let lines = app.get_detail_lines();
    assert!(lines.iter().any(|(k, v)| k == "  <broker>" && v.is_empty()), "{lines:?}");

    app.set_detail_section(MqConfigDetailSection::Revisions as usize, &tx);
    assert!(matches!(app.lazy.mq_config_revisions.get("c-9876fedc-54ba"), Some(Lazy::Loading)));
    app.lazy.mq_config_revisions.apply(
        "c-9876fedc-54ba".to_string(),
        Ok(vec![MqRevision { revision: 3, ..Default::default() }, MqRevision { revision: 2, ..Default::default() }]),
    );
    let lines = app.get_detail_lines();
    let pos = |k: &str| lines.iter().position(|(key, _)| key == k).unwrap_or_else(|| panic!("{k}: {lines:?}"));
    assert!(pos("Revision 3") < pos("Revision 2"));
    assert!(lines.iter().any(|(k, v)| k == "  Running On" && v == "orders"), "{lines:?}");

    // A configuration nothing uses says so.
    app.resources = vec![Box::new(config())];
    app.detail_section_idx = 0;
    let lines = app.get_detail_lines();
    assert!(lines.iter().any(|(_, v)| v.starts_with("⚠ no broker")), "{lines:?}");
}

#[tokio::test]
async fn ids_and_arns_jump_between_the_two_tabs() {
    for (arn, view, id) in [
        (BROKER_ARN, MqView::Brokers, "b-1234abcd-56ef"),
        (CONFIG_ARN, MqView::Configurations, "c-9876fedc-54ba"),
    ] {
        let target = resource_jump_target("  ARN", arn, ServiceType::Lambda).unwrap_or_else(|| panic!("{arn}"));
        assert_eq!((target.service, target.id.as_str()), (ServiceType::Mq, id), "{arn}");
        assert!(matches!(target.view, JumpView::Mq(v) if v == view), "{arn}");
    }

    // A broker's Configuration row leads to the configuration; its own id
    // row leads nowhere; outside @mq a `c-…` value means nothing.
    let (mut app, _tx, _rx) = test_app().await;
    select_mock(&mut app, ServiceType::Mq, Box::new(broker()));
    app.resources = vec![Box::new(broker()), Box::new(config())];
    app.filtered_resources = vec![0];
    app.selected_index = Some(0);
    assert!(app.mq_row_jump_target("  team", "c-suite").is_none(), "a tag value is not an id");
    let t = app.mq_row_jump_target("  Configuration", "c-9876fedc-54ba").expect("configuration row");
    assert!(matches!(t.view, JumpView::Mq(MqView::Configurations)));
    assert!(app.mq_row_jump_target("Broker ID", "b-1234abcd-56ef").is_none());
    assert!(app.mq_row_jump_target("  Pending", "c-1 revision 2").is_none());
    app.current_service = Some(ServiceType::EC2);
    assert!(app.mq_row_jump_target("  Configuration", "c-9876fedc-54ba").is_none());
}

#[tokio::test]
async fn sub_tabs_filter_by_type() {
    let (mut app, tx, _rx) = test_app().await;
    select_mock(&mut app, ServiceType::Mq, Box::new(broker()));
    app.resources = vec![Box::new(broker()), Box::new(config())];
    app.details_focused = false;
    for (key, want) in [('2', "MQ Configuration"), ('1', "MQ Broker")] {
        app.handle_event(Event::Key(KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE)), &tx).await.unwrap();
        let types: Vec<&str> = app.filtered_resources.iter().map(|&i| app.resources[i].resource_type()).collect();
        assert_eq!(types, [want], "key {key}");
    }
}
