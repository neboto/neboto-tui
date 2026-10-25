//! A VPC route table opens on an Overview: its id, whether it's the main
//! table, where its default routes send traffic (public / private /
//! isolated), any blackholes, then the first routes and subnets with
//! pointers to the full tabs.

use super::*;
use aws_sdk_ec2::types::{Route, RouteOrigin, RouteState, RouteTable, RouteTableAssociation};

fn route(dest: &str, target: &str) -> Route {
    let b = Route::builder().origin(RouteOrigin::CreateRoute).state(RouteState::Active);
    let b = if dest.contains(':') { b.destination_ipv6_cidr_block(dest) } else { b.destination_cidr_block(dest) };
    match target.split('-').next().unwrap() {
        "nat" => b.nat_gateway_id(target),
        "tgw" => b.transit_gateway_id(target),
        "pcx" => b.vpc_peering_connection_id(target),
        _ => b.gateway_id(target),
    }
    .build()
}

fn local() -> Route {
    Route::builder()
        .destination_cidr_block("10.0.0.0/16")
        .gateway_id("local")
        .origin(RouteOrigin::CreateRouteTable)
        .state(RouteState::Active)
        .build()
}

fn subnet_assoc(id: &str) -> RouteTableAssociation {
    RouteTableAssociation::builder().subnet_id(id).main(false).build()
}

fn table(routes: Vec<Route>, assocs: Vec<RouteTableAssociation>) -> crate::aws::services::vpc::RouteTable {
    crate::aws::services::vpc::RouteTable::from_sdk(
        &RouteTable::builder()
            .route_table_id("rtb-0public")
            .vpc_id("vpc-0123456789abcdef0")
            .set_routes(Some(routes))
            .set_associations(Some(assocs))
            .build(),
    )
}

fn overview(rt: &crate::aws::services::vpc::RouteTable) -> Vec<(String, String)> {
    crate::ui::widgets::details_pane::route_table_section_lines(
        rt,
        crate::aws::services::vpc::RouteTableDetailSection::Overview,
    )
}

fn value<'a>(rows: &'a [(String, String)], key: &str) -> Option<&'a str> {
    rows.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

/// The pane opens on Overview; its first row is the table's id, the
/// default route reads as public, and a subnet row jumps on Enter.
#[tokio::test]
async fn a_route_table_opens_on_an_overview() {
    let (mut app, tx, _rx) = test_app().await;
    let rt = table(
        vec![local(), route("0.0.0.0/0", "igw-0abc1234")],
        vec![subnet_assoc("subnet-0aaa1111"), subnet_assoc("subnet-0bbb2222")],
    );
    select_mock(&mut app, ServiceType::VPC, Box::new(rt));
    app.details_focused = true;
    app.reset_detail_section_to_default(&tx);

    assert_eq!(app.selected_descriptor().unwrap().sections[0].label, "Overview");
    let body = app.get_detail_lines();
    assert_eq!(body[0], ("Route Table ID".to_string(), "rtb-0public".to_string()));
    assert_eq!(value(&body, "Main"), Some("no"));
    assert_eq!(value(&body, "IPv4 default"), Some("igw-0abc1234 — public: direct internet access"));
    assert!(value(&body, "IPv6 default").is_none(), "no IPv6 routes, no IPv6 row: {body:#?}");
    assert!(body.iter().any(|(k, _)| k == "Subnets (2)"), "{body:#?}");

    let subnet = body.iter().position(|(k, _)| k.trim() == "subnet-0bbb2222").expect("subnet row");
    app.details_selected_index = Some(subnet);
    app.handle_key(key(KeyCode::Enter), &tx).await.unwrap();
    assert_eq!(app.pending_jump.as_deref(), Some("subnet-0bbb2222"), "a subnet row jumps to it");
}

/// The default route's target decides what the table means.
#[test]
fn the_default_route_reads_public_private_or_isolated() {
    let ipv4 = |routes: Vec<Route>| value(&overview(&table(routes, vec![])), "IPv4 default").unwrap().to_string();
    assert_eq!(ipv4(vec![local(), route("0.0.0.0/0", "nat-0def5678")]), "nat-0def5678 — private: outbound only, via NAT");
    assert_eq!(ipv4(vec![local(), route("0.0.0.0/0", "tgw-0aa")]), "tgw-0aa — via transit gateway");
    assert_eq!(ipv4(vec![local()]), "none — isolated: no internet route");

    let rows = overview(&table(vec![local(), route("2001:db8::/56", "local"), route("::/0", "eigw-0e1")], vec![]));
    assert_eq!(value(&rows, "IPv6 default"), Some("eigw-0e1 — outbound only (egress-only internet gateway)"));
}

/// Blackhole routes are warned about one by one, capped.
#[test]
fn blackhole_routes_are_warned_about() {
    let hole = |dest: &str| {
        Route::builder()
            .destination_cidr_block(dest)
            .transit_gateway_id("tgw-0gone")
            .origin(RouteOrigin::CreateRoute)
            .state(RouteState::Blackhole)
            .build()
    };
    let rows = overview(&table(
        vec![local(), hole("10.1.0.0/16"), hole("10.2.0.0/16"), hole("10.3.0.0/16"), hole("10.4.0.0/16")],
        vec![],
    ));
    assert!(
        rows.iter().any(|(k, v)| k == "  ⚠ blackhole  10.1.0.0/16 → tgw-0gone (target gone)" && v.is_empty()),
        "{rows:#?}"
    );
    assert_eq!(rows.iter().filter(|(k, _)| k.starts_with("  ⚠ blackhole")).count(), 3);
    assert!(rows.iter().any(|(k, v)| k.is_empty() && v == "· 1 more blackhole route — 2 Routes"), "{rows:#?}");
}

/// The main table says so, and how far its reach goes.
#[test]
fn the_main_table_says_it_serves_unassociated_subnets() {
    let rt = crate::aws::services::vpc::RouteTable::from_sdk(
        &RouteTable::builder()
            .route_table_id("rtb-0main")
            .routes(local())
            .associations(RouteTableAssociation::builder().main(true).build())
            .build(),
    );
    let rows = overview(&rt);
    assert_eq!(value(&rows, "Main"), Some("✓ yes — subnets with no table of their own use it"));
    assert!(rows.iter().any(|(k, v)| k.is_empty() && v == "Also every subnet with no explicit association"), "{rows:#?}");
}

/// Routes and subnets past eight point at their full tabs.
#[test]
fn long_tables_are_capped_with_a_pointer() {
    let routes: Vec<Route> = std::iter::once(local())
        .chain((0..11).map(|i| route(&format!("10.{}.0.0/16", i + 1), &format!("pcx-0{i:02}"))))
        .collect();
    let assocs: Vec<_> = (0..10).map(|i| subnet_assoc(&format!("subnet-0{i:02}"))).collect();
    let rows = overview(&table(routes, assocs));
    assert!(rows.iter().any(|(k, v)| k.is_empty() && v == "· 4 more — 2 Routes"), "{rows:#?}");
    assert!(rows.iter().any(|(k, v)| k.is_empty() && v == "· 2 more — 3 Associations"), "{rows:#?}");
    assert!(!rows.iter().any(|(k, _)| k.contains("pcx-010")), "the 12th route is past the cap");
    assert!(!rows.iter().any(|(k, _)| k.contains("subnet-008")), "the 9th subnet is past the cap");
}
