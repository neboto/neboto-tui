//! A Transit Gateway route table opens on an Overview: its identity rows,
//! a Routing summary (default association / propagation, how many
//! attachments associate, the default route, blackholes), and the first
//! routes, capped with a pointer at the Routes tab.

use super::*;
use crate::aws::services::transit_gateway::{TgwAttachment, TgwRoute, TgwRouteTable};

const RT_ID: &str = "tgw-rtb-0123456789abcdef0";
const TGW_ID: &str = "tgw-0123456789abcdef0";

fn route(cidr: &str, state: &str, kind: &str, att: &str, res: &str) -> TgwRoute {
    TgwRoute {
        cidr: cidr.to_string(),
        state: state.to_string(),
        route_type: kind.to_string(),
        attachment_id: att.to_string(),
        resource_id: res.to_string(),
        resource_type: if res.is_empty() { String::new() } else { "vpc".to_string() },
    }
}

fn attachment(id: &str, kind: aws_sdk_ec2::types::TransitGatewayAttachmentResourceType, rt: &str) -> Box<dyn Resource> {
    Box::new(TgwAttachment::from_sdk(
        &aws_sdk_ec2::types::TransitGatewayAttachment::builder()
            .transit_gateway_attachment_id(id)
            .transit_gateway_id(TGW_ID)
            .resource_type(kind)
            .association(
                aws_sdk_ec2::types::TransitGatewayAttachmentAssociation::builder()
                    .transit_gateway_route_table_id(rt)
                    .build(),
            )
            .build(),
    ))
}

/// The route table selected and focused, with two attachments associated
/// to it and one to another table loaded beside it.
async fn app_on_route_table() -> (App, mpsc::UnboundedSender<Event>, mpsc::UnboundedReceiver<Event>) {
    use aws_sdk_ec2::types::TransitGatewayAttachmentResourceType as Kind;
    let (mut app, tx, rx) = test_app().await;
    let rt = TgwRouteTable::from_sdk(
        &aws_sdk_ec2::types::TransitGatewayRouteTable::builder()
            .transit_gateway_route_table_id(RT_ID)
            .transit_gateway_id(TGW_ID)
            .state(aws_sdk_ec2::types::TransitGatewayRouteTableState::Available)
            .default_association_route_table(true)
            .default_propagation_route_table(false)
            .tags(aws_sdk_ec2::types::Tag::builder().key("Name").value("core-rt").build())
            .build(),
    );
    select_mock(&mut app, ServiceType::TransitGateway, Box::new(rt));
    app.resources.push(attachment("tgw-attach-0aaaaaaaaaaaaaaa1", Kind::Vpc, RT_ID));
    app.resources.push(attachment("tgw-attach-0aaaaaaaaaaaaaaa2", Kind::Vpn, RT_ID));
    app.resources.push(attachment("tgw-attach-0aaaaaaaaaaaaaaa3", Kind::Vpc, "tgw-rtb-0ffffffffffffffff"));
    app.details_focused = true;
    app.reset_detail_section_to_default(&tx);
    (app, tx, rx)
}

fn has(body: &[(String, String)], k: &str, v: &str) -> bool {
    body.iter().any(|(bk, bv)| bk == k && bv == v)
}

#[tokio::test]
async fn a_route_table_opens_on_an_overview_and_fetches_its_routes() {
    let (mut app, _tx, _rx) = app_on_route_table().await;
    assert_eq!(app.selected_descriptor().unwrap().sections[0].label, "Overview");
    assert_eq!(app.detail_section_idx, 0);
    // The routes feed the Overview's summary, so focusing it starts the fetch.
    assert!(matches!(app.lazy.tgw_routes.get(RT_ID), Some(crate::lazy::Lazy::Loading)));

    let body = app.get_detail_lines();
    assert_eq!(body[0], ("Route Table ID".to_string(), RT_ID.to_string()));
    assert!(has(&body, "Name", "core-rt"), "{body:#?}");
    assert!(has(&body, "Transit Gateway", TGW_ID), "{body:#?}");
    assert!(has(&body, "Default Association", "✓ yes — new attachments associate here"), "{body:#?}");
    assert!(has(&body, "Default Propagation", "no"), "{body:#?}");
    assert!(has(&body, "Associations", "2 attachments (1 vpc · 1 vpn)"), "{body:#?}");
    assert!(has(&body, "Routes", "Loading…"), "{body:#?}");

    app.lazy.tgw_routes.apply(
        RT_ID.to_string(),
        Ok(vec![
            route("0.0.0.0/0", "active", "static", "tgw-attach-0aaaaaaaaaaaaaaa1", "vpc-0123456789abcdef0"),
            route("10.1.0.0/16", "active", "propagated", "tgw-attach-0aaaaaaaaaaaaaaa1", "vpc-0123456789abcdef0"),
            route("10.9.0.0/16", "blackhole", "static", "", ""),
        ]),
    );
    let body = app.get_detail_lines();
    assert!(has(&body, "Routes", "3 (2 static · 1 propagated)"), "{body:#?}");
    assert!(has(&body, "Default Route", "0.0.0.0/0 → vpc-0123456789abcdef0 (vpc)"), "{body:#?}");
    assert!(has(&body, "  ⚠ 10.9.0.0/16 is a blackhole — traffic to it is dropped", ""), "{body:#?}");
    assert!(has(&body, "Routes (3)", ""), "{body:#?}");
    assert!(body.iter().any(|(k, _)| k.contains("10.1.0.0/16") && k.contains("propagated")), "{body:#?}");
    assert!(!body.iter().any(|(_, v)| v.contains("more —")), "three routes need no cap note");
}

#[tokio::test]
async fn the_overview_caps_long_route_and_blackhole_lists() {
    let (mut app, tx, _rx) = app_on_route_table().await;
    let mut routes: Vec<TgwRoute> = (0..12)
        .map(|i| route(&format!("10.{i}.0.0/16"), "active", "propagated", "tgw-attach-0aaaaaaaaaaaaaaa1", "vpc-0123456789abcdef0"))
        .collect();
    routes.extend((0..5).map(|i| route(&format!("192.168.{i}.0/24"), "blackhole", "static", "", "")));
    app.lazy.tgw_routes.apply(RT_ID.to_string(), Ok(routes));
    let body = app.get_detail_lines();
    assert!(has(&body, "Default Route", "none — traffic matching no route is dropped"), "{body:#?}");
    let warnings = body.iter().filter(|(k, _)| k.starts_with("  ⚠")).count();
    assert_eq!(warnings, 3, "blackholes are capped: {body:#?}");
    assert!(has(&body, "", "· 2 more blackhole routes — 2 Routes"), "{body:#?}");
    let lines = body.iter().filter(|(k, _)| k.contains("10.") && k.contains("propagated")).count();
    assert_eq!(lines, 8, "{body:#?}");
    assert!(has(&body, "", "· 9 more — 2 Routes"), "{body:#?}");

    // The Routes tab still has all of them, in the same line format.
    app.set_detail_section(1, &tx);
    let tab = app.get_detail_lines();
    assert_eq!(tab.iter().filter(|(k, _)| k.contains("propagated") || k.contains("blackhole")).count(), 17);
    let first_overview = body.iter().find(|(k, _)| k.contains("10.0.0.0/16")).unwrap();
    assert!(tab.contains(first_overview), "Overview and Routes share the line format");
}

#[tokio::test]
async fn overview_ids_jump_but_the_own_id_does_not() {
    let (mut app, tx, _rx) = app_on_route_table().await;
    let body = app.get_detail_lines();

    app.details_selected_index = Some(0);
    let history = app.nav_history.len();
    app.handle_key(key(KeyCode::Enter), &tx).await.unwrap();
    assert_eq!(app.nav_history.len(), history, "Enter on the own-id row records no jump");

    let tgw_row = body.iter().position(|(k, _)| k == "Transit Gateway").unwrap();
    app.details_selected_index = Some(tgw_row);
    app.handle_key(key(KeyCode::Enter), &tx).await.unwrap();
    assert_eq!(app.pending_jump.as_deref(), Some(TGW_ID), "the gateway row jumps");
}
