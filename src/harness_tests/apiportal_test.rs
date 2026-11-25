//! API Portal (#361): an inventory item's exposure and its links, the empty
//! inventory explained by discovery, a managed API's lazy Events, a
//! product's portals and lazy APIs, and the five sub-tabs.

use super::*;
use crate::app::{ApiPortalView, JumpView};
use crate::aws::services::apiportal::{
    ApiDiscovery, ApiInventoryItem, ApiPortal, ApiProduct, ManagedApi, ManagedApiDetailSection, ManagedApiEvent,
    ManagedApiEvents, ManagedApiFinding,
};
use crate::lazy::Lazy;
use crate::ui::widgets::details_pane::resource_jump_target;

const STAGE_ARN: &str = "arn:aws:apigateway:us-east-1::/restapis/abc123/stages/prod";
const PRODUCT_ARN: &str = "arn:aws:apiportal:us-east-1:123456789012:product/pr-1";

fn item() -> ApiInventoryItem {
    let mut i = ApiInventoryItem {
        method: "POST".into(),
        host: "mcp.example.com".into(),
        port: 443,
        path: "/mcp".into(),
        account_id: "123456789012".into(),
        resource_arn: STAGE_ARN.into(),
        region: "us-east-1".into(),
        source: "API_GATEWAY".into(),
        visibility: Some("PUBLIC".into()),
        waf_status: Some("DISABLED".into()),
        classification: Some("SHADOW".into()),
        workloads: vec!["MCP".into()],
        ..Default::default()
    };
    i.id = format!("{} {}", i.signature(), i.resource_arn);
    i
}

fn managed() -> ManagedApi {
    ManagedApi {
        id: "ma-1".into(),
        arn: "arn:aws:apiportal:us-east-1:123456789012:managed-api/ma-1".into(),
        signature: "POST mcp.example.com:443 /mcp".into(),
        resource_arn: STAGE_ARN.into(),
        findings: vec![
            ManagedApiFinding {
                posture_id: "waf".into(),
                status: "FALSE".into(),
                reason: Some("STAGE_WAF_NOT_ASSOCIATED".into()),
                updated: None,
            },
            ManagedApiFinding { posture_id: "tls".into(), status: "TRUE".into(), ..Default::default() },
        ],
        described: true,
        ..Default::default()
    }
}

fn product() -> ApiProduct {
    ApiProduct {
        id: "pr-1".into(),
        arn: PRODUCT_ARN.into(),
        name: "Payments".into(),
        status: Some("ACTIVE".into()),
        ..Default::default()
    }
}

fn portal() -> ApiPortal {
    ApiPortal {
        id: "p-1".into(),
        arn: "arn:aws:apiportal:us-east-1:123456789012:portal/p-1".into(),
        name: "Developer hub".into(),
        publish_status: Some("PUBLISHED".into()),
        default_domain: Some("p-1.portal.example.com".into()),
        authorization: Some("none".into()),
        product_arns: vec![PRODUCT_ARN.into()],
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

fn load(app: &mut App, resources: Vec<Box<dyn crate::aws::resource::Resource>>) {
    app.resources = resources;
    app.filtered_resources = vec![0];
    app.selected_index = Some(0);
    app.details_focused = true;
    app.layout_mode = crate::app::LayoutMode::DetailsOnly;
}

#[tokio::test]
async fn a_shadow_item_shows_its_exposure_and_leads_to_its_managed_api() {
    let (mut app, tx, _rx) = test_app().await;
    let mut i = item();
    i.managed_api_id = Some("ma-1".into());
    select_mock(&mut app, ServiceType::ApiPortal, Box::new(i.clone()));
    load(&mut app, vec![Box::new(i.clone()), Box::new(managed())]);
    app.reset_detail_section_to_default(&tx);

    let screen = screen_of(&app);
    for needle in [
        "POST mcp.example.com:443 /mcp",
        "⚠ shadow — no account in the organization manages this API",
        "⚠ public — reachable from the internet",
        "⚠ no web ACL on the front door",
        "✗ public, unmanaged and without WAF — and it serves AI traffic",
        "API Gateway",
    ] {
        assert!(screen.contains(needle), "missing {needle:?} in:\n{screen}");
    }

    let t = app.apiportal_row_jump_target("Managed API", "ma-1").expect("managed API row");
    assert!(matches!(t.view, JumpView::ApiPortal(ApiPortalView::ManagedApis)));
    // The item's own id and the stage ARN aren't API Portal rows.
    assert!(app.apiportal_row_jump_target("API", &i.id).is_none());
    assert!(app.apiportal_row_jump_target("  Resource", STAGE_ARN).is_none());
    // The stage ARN goes to API Gateway through the generic classifier.
    let t = resource_jump_target("  Resource", STAGE_ARN, ServiceType::ApiPortal).expect("stage ARN jumps");
    assert_eq!((t.service, t.id.as_str()), (ServiceType::ApiGateway, "abc123"));
    assert!(matches!(t.view, JumpView::ApiGw(crate::app::ApiGatewayView::RestApis)));
    let http = "arn:aws:apigateway:us-east-1::/apis/h7k2/stages/$default";
    let t = resource_jump_target("  Resource", http, ServiceType::ApiPortal).expect("HTTP API ARN jumps");
    assert_eq!(t.id, "h7k2");
    assert!(matches!(t.view, JumpView::ApiGw(crate::app::ApiGatewayView::HttpApis)));
}

#[tokio::test]
async fn an_empty_inventory_says_discovery_is_off() {
    let (mut app, _tx, _rx) = test_app().await;
    select_mock(&mut app, ServiceType::ApiPortal, Box::new(ApiDiscovery::account()));
    app.resources = vec![Box::new(ApiDiscovery::account())];
    app.apiportal_view = ApiPortalView::Inventory;
    app.details_focused = false;
    app.update_search();
    assert!(app.filtered_resources.is_empty());
    let screen = screen_of(&app);
    assert!(screen.contains("API discovery isn't on in this account/region"), "{screen}");
}

#[tokio::test]
async fn a_managed_api_lists_failing_checks_and_fetches_events_on_open() {
    let (mut app, tx, _rx) = test_app().await;
    select_mock(&mut app, ServiceType::ApiPortal, Box::new(managed()));
    load(&mut app, vec![Box::new(managed())]);
    app.reset_detail_section_to_default(&tx);

    let lines = app.get_detail_lines();
    assert!(lines.iter().any(|(k, v)| k == "  Checks" && v == "✗ 1 of 2 failing"), "{lines:?}");
    assert!(lines.iter().any(|(k, v)| k == "  waf" && v == "✗ stage waf not associated"), "{lines:?}");
    assert!(app.lazy.apiportal_events.get("ma-1").is_none(), "Overview doesn't fetch events");

    app.set_detail_section(ManagedApiDetailSection::Events as usize, &tx);
    assert!(matches!(app.lazy.apiportal_events.get("ma-1"), Some(Lazy::Loading)));
    app.lazy.apiportal_events.apply(
        "ma-1".to_string(),
        Ok(ManagedApiEvents {
            items: vec![ManagedApiEvent {
                id: "ev-1".into(),
                label: Some("awswaf:managed:aws:api-security:sqli".into()),
                status: Some("EVENT_IN_PROGRESS".into()),
                ..Default::default()
            }],
            truncated: false,
        }),
    );
    let lines = app.get_detail_lines();
    assert!(lines.iter().any(|(k, _)| k == "awswaf:managed:aws:api-security:sqli"), "{lines:?}");
    assert!(lines.iter().any(|(k, v)| k == "  Status" && v == "in progress"), "{lines:?}");
}

#[tokio::test]
async fn a_product_names_its_portals_and_a_portal_its_products() {
    let (mut app, tx, _rx) = test_app().await;
    select_mock(&mut app, ServiceType::ApiPortal, Box::new(product()));
    load(&mut app, vec![Box::new(product()), Box::new(portal())]);
    app.reset_detail_section_to_default(&tx);
    // The Overview previews the APIs, so it shares the tab's fetch.
    assert!(matches!(app.lazy.apiportal_product_apis.get("pr-1"), Some(Lazy::Loading)));
    app.lazy.apiportal_product_apis.apply("pr-1".into(), Ok(vec![("ma-1".into(), "GET a:443 /x".into())]));
    let lines = app.get_detail_lines();
    assert!(lines.iter().any(|(k, v)| k == "  Developer hub" && v == "p-1"), "{lines:?}");
    assert!(lines.iter().any(|(k, v)| k == "  GET a:443 /x" && v == "ma-1"), "{lines:?}");
    let t = app.apiportal_row_jump_target("  Developer hub", "p-1").expect("portal row");
    assert!(matches!(t.view, JumpView::ApiPortal(ApiPortalView::Portals)));

    // The portal's product row carries the ARN, which leads to the product.
    load(&mut app, vec![Box::new(portal()), Box::new(product())]);
    let lines = app.get_detail_lines();
    assert!(lines.iter().any(|(k, v)| k == "  Payments" && v == PRODUCT_ARN), "{lines:?}");
    assert!(lines.iter().any(|(_, v)| v.starts_with("⚠ none — anyone with the URL")), "{lines:?}");
    let t = app.apiportal_row_jump_target("  Payments", PRODUCT_ARN).expect("product row");
    assert_eq!(t.id, "pr-1");
    assert!(matches!(t.view, JumpView::ApiPortal(ApiPortalView::Products)));
}

#[tokio::test]
async fn sub_tabs_filter_by_type() {
    let (mut app, tx, _rx) = test_app().await;
    select_mock(&mut app, ServiceType::ApiPortal, Box::new(item()));
    app.resources = vec![
        Box::new(item()),
        Box::new(managed()),
        Box::new(portal()),
        Box::new(product()),
        Box::new(ApiDiscovery::account()),
    ];
    app.details_focused = false;
    for (key, want) in [
        ('2', "Managed API"),
        ('3', "API Portal"),
        ('4', "API Product"),
        ('5', "API Discovery"),
        ('1', "API Inventory Item"),
    ] {
        app.handle_event(Event::Key(KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE)), &tx).await.unwrap();
        let types: Vec<&str> = app.filtered_resources.iter().map(|&i| app.resources[i].resource_type()).collect();
        assert_eq!(types, [want], "key {key}");
    }
    app.handle_event(Event::Key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT)), &tx).await.unwrap();
    assert_eq!(app.apiportal_view, ApiPortalView::Discovery, "Shift-Tab wraps");
}
