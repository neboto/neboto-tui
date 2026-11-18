//! PrivateLink provider side (issue #96): endpoint services this account
//! publishes, on VPC's Endpoints tab beside the consumer's endpoints. The
//! Overview's connection summary shares the Connections fetch; a connection
//! from another account names its endpoint without offering a jump; the
//! consumer endpoint of an own service links back to it.

use super::*;
use crate::app::{JumpView, VpcView};
use crate::aws::services::vpc::{
    EpsConnection, EpsConnections, EpsNotification, EpsPermission, VpcEndpoint, VpcEndpointService,
    VpcEndpointServiceDetailSection as S,
};
use crate::lazy::Lazy;
use crate::ui::widgets::details_pane::resource_jump_target;

fn mock(label: &str) -> Box<dyn crate::aws::resource::Resource> {
    all_mocks().into_iter().find(|(_, l, _)| *l == label).unwrap().2
}

fn render_to_string(app: &App) -> String {
    let backend = TestBackend::new(170, 60);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| crate::render_app(app, f)).unwrap();
    let buf = terminal.backend().buffer();
    (0..buf.area.height)
        .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

const SVC_ID: &str = "vpce-svc-0123456789abcdef0";
const SVC_NAME: &str = "com.amazonaws.vpce.us-east-1.vpce-svc-0123456789abcdef0";

fn service() -> VpcEndpointService {
    mock("VpcEndpointService").as_any().downcast_ref::<VpcEndpointService>().unwrap().clone()
}

fn consumer() -> VpcEndpoint {
    let mut ep = mock("VpcEndpoint").as_any().downcast_ref::<VpcEndpoint>().unwrap().clone();
    ep.id = "vpce-0local0000000000".into();
    ep.service_name = SVC_NAME.into();
    ep.kind = "Interface".into();
    ep
}

fn connection(id: &str, owner: &str, state: &str) -> EpsConnection {
    EpsConnection {
        endpoint_id: id.into(),
        owner: owner.into(),
        state: state.into(),
        created: Some("2026-11-01 10:00 UTC".into()),
        region: None,
        ip_type: Some("ipv4".into()),
        dns_names: vec![],
        lb_arns: vec![],
    }
}

fn focus(app: &mut App, tx: &mpsc::UnboundedSender<Event>) {
    app.resources = vec![Box::new(service()), Box::new(consumer())];
    app.filtered_resources = vec![0];
    app.selected_index = Some(0);
    app.details_focused = true;
    app.layout_mode = crate::app::LayoutMode::DetailsOnly;
    app.reset_detail_section_to_default(tx);
}

#[tokio::test]
async fn overview_shares_the_connections_fetch_and_flags_pending_requests() {
    let (mut app, tx, _rx) = test_app().await;
    select_mock(&mut app, ServiceType::VPC, Box::new(service()));
    focus(&mut app, &tx);
    assert!(
        matches!(app.lazy.eps_connections.get(SVC_ID), Some(Lazy::Loading)),
        "drill-in fires the Overview hook"
    );
    assert!(app.lazy.eps_permissions.get(SVC_ID).is_none(), "permissions wait for their tab");

    app.lazy.eps_connections.apply(
        SVC_ID.to_string(),
        Ok(EpsConnections {
            connections: vec![
                connection("vpce-0aaa", "111122223333", "pendingAcceptance"),
                connection("vpce-0bbb", "444455556666", "available"),
                connection("vpce-0local0000000000", "123456789012", "available"),
            ],
            truncated: false,
            notifications: Ok(vec![EpsNotification {
                id: "vpce-nfn-1".into(),
                topic_arn: "arn:aws:sns:us-east-1:123456789012:privatelink".into(),
                events: vec!["Connect".into(), "Accept".into()],
                state: "Enabled".into(),
            }]),
        }),
    );
    let lines = app.get_detail_lines();
    let has = |k: &str, v: &str| lines.iter().any(|(a, b)| a.trim() == k && b == v);
    assert!(has("Service ID", SVC_ID), "{lines:?}");
    assert!(has("Acceptance", "✓ required — each request waits for you to accept it"), "{lines:?}");
    assert!(has("Pending Acceptance", "⚠ 1 waiting for you"), "{lines:?}");
    assert!(has("Endpoints", "3 (1 pendingAcceptance, 2 available)"), "{lines:?}");
    assert!(has("Network LB", "arn:aws:elasticloadbalancing:us-east-1:123456789012:loadbalancer/net/api/0123456789abcdef"));

    // Connections: a foreign endpoint is a header only; this account's own
    // loaded endpoint gets a jumpable `Endpoint` row.
    app.set_detail_section(S::Connections as usize, &tx);
    let lines = app.get_detail_lines();
    let header = lines
        .iter()
        .find(|(k, _)| k.starts_with("vpce-0aaa"))
        .unwrap_or_else(|| panic!("{lines:?}"));
    assert!(header.1.is_empty());
    // This account's own loaded endpoint is offered (it is a loaded row).
    let own = lines.iter().find(|(k, _)| k.starts_with("vpce-0local")).unwrap();
    assert!(app.generic_jump_allowed(&resource_jump_target(&own.0, &own.1, ServiceType::VPC).unwrap(), &own.1));
    let foreign = resource_jump_target(&header.0, &header.1, ServiceType::VPC).expect("the classifier sees an id");
    assert!(!app.generic_jump_allowed(&foreign, &header.1), "no jump to a foreign endpoint");
    // Principals are other accounts' ARNs.
    assert!(lines.iter().any(|(k, v)| k.trim() == "State" && v == "⚠ pendingAcceptance"));
    assert!(lines.iter().any(|(k, v)| k.trim() == "SNS Topic" && v.ends_with(":privatelink")));

    app.set_detail_section(S::Permissions as usize, &tx);
    assert!(matches!(app.lazy.eps_permissions.get(SVC_ID), Some(Lazy::Loading)));
    app.lazy.eps_permissions.apply(
        SVC_ID.to_string(),
        Ok(vec![EpsPermission { principal: "*".into(), principal_type: "All".into() }]),
    );
    let lines = app.get_detail_lines();
    assert!(lines.iter().any(|(k, _)| k.contains("any AWS principal can request a connection")), "{lines:?}");
    let arn = "arn:aws:iam::111122223333:root";
    app.lazy.eps_permissions = Default::default();
    app.lazy.eps_permissions.apply(
        SVC_ID.to_string(),
        Ok(vec![EpsPermission { principal: arn.into(), principal_type: "Account".into() }]),
    );
    let lines = app.get_detail_lines();
    let row = lines.iter().find(|(k, _)| k.contains(arn)).unwrap_or_else(|| panic!("{lines:?}"));
    assert!(resource_jump_target(&row.0, &row.1, ServiceType::VPC).is_none(), "another account's principal");
    assert!(!lines.iter().any(|(k, _)| k.contains("without review")), "acceptance is required here");

    // The pane renders through its split renderer, every section on the bar.
    let screen = render_to_string(&app);
    for label in ["Overview", "Connections", "Permissions", "Tags", "Endpoint Service"] {
        assert!(screen.contains(label), "missing {label:?}:\n{screen}");
    }
}

#[tokio::test]
async fn ids_jump_both_ways_between_consumer_and_provider() {
    let t = resource_jump_target("Endpoint Service", SVC_ID, ServiceType::EC2).expect("vpce-svc- id");
    assert_eq!((t.service, t.id.as_str()), (ServiceType::VPC, SVC_ID));
    assert!(matches!(t.view, JumpView::Vpc(VpcView::Endpoints)));
    assert!(VpcView::Endpoints.resource_type_filter().split('|').any(|f| f == "Endpoint Service"));

    // A consumer endpoint of a service this account publishes names it.
    let (mut app, _tx, _rx) = test_app().await;
    select_mock(&mut app, ServiceType::VPC, Box::new(consumer()));
    app.resources = vec![Box::new(consumer()), Box::new(service())];
    app.filtered_resources = vec![0];
    app.selected_index = Some(0);
    let lines = app.get_detail_lines();
    assert!(lines.iter().any(|(k, v)| k.trim() == "Endpoint Service" && v == SVC_ID), "{lines:?}");

    // …and an AWS service endpoint doesn't.
    let mut s3 = consumer();
    s3.service_name = "com.amazonaws.us-east-1.s3".into();
    app.resources = vec![Box::new(s3), Box::new(service())];
    let lines = app.get_detail_lines();
    assert!(!lines.iter().any(|(k, _)| k.trim() == "Endpoint Service"), "{lines:?}");
}

#[tokio::test]
async fn charts_sum_over_the_load_balancers() {
    let (mut app, _tx, _rx) = test_app().await;
    select_mock(&mut app, ServiceType::VPC, Box::new(service()));
    assert!(app.supports_metrics_overlay());
    let t = app.selected_metric_target().expect("metric target");
    assert_eq!(t.namespace, "AWS/PrivateLinkServices");
    assert_eq!(t.dimensions, [("Service Id".to_string(), SVC_ID.to_string())]);
}
