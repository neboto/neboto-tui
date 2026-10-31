//! A TGW attachment's Overview answers where its traffic goes (its route
//! table, where its routes propagate) and adds what only its kind's own
//! describe carries: a VPC attachment's subnets and appliance mode, a
//! peering's far side (a cross-region jump), a Connect attachment's peers.
//! A route table gets the other half: which attachments propagate into it.

use super::*;
use crate::aws::region::Region;
use crate::aws::services::transit_gateway::*;
use crate::lazy::Lazy;
use aws_sdk_ec2::types::TransitGatewayAttachmentResourceType as Kind;

const TGW: &str = "tgw-0aaaaaaaaaaaaaaa1";
const ATT: &str = "tgw-attach-0aaaaaaaaaaaaaaa1";
const RT: &str = "tgw-rtb-0aaaaaaaaaaaaaaa1";

fn has(body: &[(String, String)], k: &str, v: &str) -> bool {
    body.iter().any(|(bk, bv)| bk == k && bv == v)
}

/// An attachment of `kind`, selected with the detail pane focused (which
/// fires the Overview's hook).
async fn app_on_attachment(kind: Kind, resource: &str) -> (App, mpsc::UnboundedSender<Event>, mpsc::UnboundedReceiver<Event>) {
    let (mut app, tx, rx) = test_app().await;
    app.account_id = Some("111111111111".into());
    let att = TgwAttachment::from_sdk(
        &aws_sdk_ec2::types::TransitGatewayAttachment::builder()
            .transit_gateway_attachment_id(ATT)
            .transit_gateway_id(TGW)
            .resource_type(kind)
            .resource_id(resource)
            .association(
                aws_sdk_ec2::types::TransitGatewayAttachmentAssociation::builder()
                    .transit_gateway_route_table_id(RT)
                    .build(),
            )
            .build(),
    );
    select_mock(&mut app, ServiceType::TransitGateway, Box::new(att));
    app.details_focused = true;
    app.reset_detail_section_to_default(&tx);
    (app, tx, rx)
}

fn propagates_to(tables: &[&str]) -> std::result::Result<Vec<TgwAttachmentPropagation>, String> {
    Ok(tables
        .iter()
        .map(|t| TgwAttachmentPropagation { route_table_id: t.to_string(), state: "enabled".into() })
        .collect())
}

fn apply(app: &mut App, detail: TgwAttachmentDetail) {
    app.lazy.tgw_attachment_detail.apply(ATT.to_string(), Ok(detail));
}

#[tokio::test]
async fn a_vpc_attachment_shows_its_routing_and_subnets() {
    let (mut app, tx, _rx) = app_on_attachment(Kind::Vpc, "vpc-0abc").await;
    assert!(matches!(app.lazy.tgw_attachment_detail.get(ATT), Some(Lazy::Loading)), "the Overview fetches");
    let body = app.get_detail_lines();
    assert!(has(&body, "Route Table", RT), "{body:#?}");
    assert!(has(&body, "Propagates To", "Loading…"), "{body:#?}");
    assert!(has(&body, "VPC", "") && has(&body, "Details", "Loading…"), "{body:#?}");

    apply(
        &mut app,
        TgwAttachmentDetail {
            propagations: propagates_to(&[RT, "tgw-rtb-0bbbbbbbbbbbbbbb2"]),
            kind: Ok(TgwKindDetail::Vpc(TgwVpcDetail {
                subnet_ids: vec!["subnet-0aaa".into(), "subnet-0bbb".into()],
                appliance_mode: false,
                dns_support: true,
                sg_referencing: false,
            })),
        },
    );
    let body = app.get_detail_lines();
    assert!(has(&body, "Propagates To", "2 route tables — 2 Association"), "{body:#?}");
    assert!(
        has(&body, "Appliance Mode", "disabled — an inspection VPC needs it for symmetric return traffic"),
        "{body:#?}"
    );
    assert!(has(&body, "DNS Support", "✓ enabled"), "{body:#?}");
    assert!(has(&body, "Subnets (2)", "") && has(&body, "  subnet-0aaa", ""), "{body:#?}");

    // The Association tab lists the tables.
    app.set_detail_section(1, &tx);
    let tab = app.get_detail_lines();
    assert!(has(&tab, "Propagates To (2)", ""), "{tab:#?}");
    assert!(tab.iter().any(|(k, _)| k.contains("tgw-rtb-0bbbbbbbbbbbbbbb2") && k.contains("enabled")), "{tab:#?}");
}

#[tokio::test]
async fn a_peering_attachment_names_its_far_side_and_jumps_there() {
    let (mut app, tx, _rx) = app_on_attachment(Kind::Peering, "tgw-0ppppppppppppppp9").await;
    apply(
        &mut app,
        TgwAttachmentDetail {
            propagations: propagates_to(&[]),
            kind: Ok(TgwKindDetail::Peering(TgwPeeringDetail {
                requester: TgwPeeringSide { tgw_id: TGW.into(), owner_id: "111111111111".into(), region: "us-east-1".into(), core_network_id: None },
                accepter: TgwPeeringSide {
                    tgw_id: "tgw-0ppppppppppppppp9".into(),
                    owner_id: "222222222222".into(),
                    region: "eu-west-1".into(),
                    core_network_id: None,
                },
                accepter_attachment_id: None,
                status_code: "available".into(),
                status_message: String::new(),
                dynamic_routing: Some(false),
            })),
        },
    );
    let body = app.get_detail_lines();
    assert!(has(&body, "Propagates To", "none — its routes appear in no table on their own"), "{body:#?}");
    assert!(has(&body, "This Side", "requester"), "{body:#?}");
    assert!(has(&body, "Peer Gateway", "tgw-0ppppppppppppppp9"), "{body:#?}");
    assert!(has(&body, "Peer Account", "222222222222"), "{body:#?}");
    assert!(has(&body, "Peer Region", "eu-west-1"), "{body:#?}");
    assert!(has(&body, "", "· the peer is in another account — P / s to browse it"), "{body:#?}");
    assert!(has(&body, "Status", "available"), "{body:#?}");

    // ⏎ on the peer gateway switches to its region.
    let row = body.iter().position(|(k, _)| k == "Peer Gateway").unwrap();
    app.details_selected_index = Some(row);
    app.handle_key(key(KeyCode::Enter), &tx).await.unwrap();
    assert_eq!(app.pending_jump.as_deref(), Some("tgw-0ppppppppppppppp9"));
    assert_eq!(app.pending_jump_region, Some(Region::EuWest1));
}

#[tokio::test]
async fn a_connect_attachment_lists_its_peers_and_bgp() {
    let (mut app, _tx, _rx) = app_on_attachment(Kind::Connect, "tgw-attach-0transport").await;
    let session = |status: &str| TgwBgpSession {
        tgw_asn: Some(64512),
        peer_asn: Some(65000),
        tgw_address: "169.254.6.2".into(),
        peer_address: "169.254.6.1".into(),
        status: status.into(),
    };
    apply(
        &mut app,
        TgwAttachmentDetail {
            propagations: propagates_to(&[RT]),
            kind: Ok(TgwKindDetail::Connect(TgwConnectDetail {
                transport_attachment_id: "tgw-attach-0transport".into(),
                protocol: "gre".into(),
                peers: Ok(vec![TgwConnectPeer {
                    id: "tgw-connect-peer-0a".into(),
                    state: "available".into(),
                    tgw_address: "192.0.2.10".into(),
                    peer_address: "10.0.0.5".into(),
                    inside_cidrs: vec!["169.254.6.0/29".into()],
                    bgp: vec![session("up"), session("down")],
                }]),
            })),
        },
    );
    let body = app.get_detail_lines();
    assert!(has(&body, "Transport Attachment", "tgw-attach-0transport"), "{body:#?}");
    assert!(has(&body, "Connect Peers (1)", ""), "{body:#?}");
    assert!(has(&body, "Peer", "tgw-connect-peer-0a · available"), "{body:#?}");
    assert!(has(&body, "  GRE", "192.0.2.10 → 10.0.0.5"), "{body:#?}");
    assert!(has(&body, "  BGP", "64512 169.254.6.2 ↔ 65000 169.254.6.1 · ✓ up"), "{body:#?}");
    assert!(has(&body, "  BGP", "64512 169.254.6.2 ↔ 65000 169.254.6.1 · ✗ down"), "{body:#?}");
}

/// Each half of the bundle fails on its own: a denied kind describe still
/// shows the propagations, and the error renders inline.
#[tokio::test]
async fn a_failed_half_still_shows_the_other() {
    let (mut app, _tx, _rx) = app_on_attachment(Kind::Vpc, "vpc-0abc").await;
    apply(
        &mut app,
        TgwAttachmentDetail {
            propagations: propagates_to(&[RT]),
            kind: Err("You are not authorized to perform this operation.".into()),
        },
    );
    let body = app.get_detail_lines();
    assert!(has(&body, "Propagates To", "1 route table — 2 Association"), "{body:#?}");
    assert!(has(&body, "  ⚠ You are not authorized to perform this operation.", ""), "{body:#?}");
}

/// A kind with nothing past the attachment list gets no kind block — and
/// no `Loading…` that would never finish.
#[tokio::test]
async fn a_vpn_attachment_has_no_kind_block() {
    let (mut app, _tx, _rx) = app_on_attachment(Kind::Vpn, "vpn-0abc").await;
    let body = app.get_detail_lines();
    assert!(!has(&body, "Details", "Loading…"), "{body:#?}");
    apply(&mut app, TgwAttachmentDetail { propagations: propagates_to(&[RT]), kind: Ok(TgwKindDetail::None) });
    let body = app.get_detail_lines();
    assert!(!body.iter().any(|(k, _)| k == "VPC" || k == "Peering" || k == "Connect"), "{body:#?}");
}

#[tokio::test]
async fn a_route_table_lists_what_propagates_into_it() {
    let (mut app, tx, _rx) = test_app().await;
    let rt = TgwRouteTable::from_sdk(
        &aws_sdk_ec2::types::TransitGatewayRouteTable::builder()
            .transit_gateway_route_table_id(RT)
            .transit_gateway_id(TGW)
            .build(),
    );
    select_mock(&mut app, ServiceType::TransitGateway, Box::new(rt));
    app.details_focused = true;
    app.reset_detail_section_to_default(&tx);
    // The Overview summarises both, so it fetches both.
    assert!(matches!(app.lazy.tgw_routes.get(RT), Some(Lazy::Loading)));
    assert!(matches!(app.lazy.tgw_route_table_propagations.get(RT), Some(Lazy::Loading)));
    assert!(has(&app.get_detail_lines(), "Propagations", "Loading…"));

    let prop = |att: &str, kind: &str, res: &str| TgwPropagation {
        attachment_id: att.into(),
        resource_id: res.into(),
        resource_type: kind.into(),
        state: "enabled".into(),
    };
    app.lazy.tgw_route_table_propagations.apply(
        RT.to_string(),
        Ok(vec![prop(ATT, "vpc", "vpc-0abc"), prop("tgw-attach-0b", "vpc", "vpc-0def"), prop("tgw-attach-0c", "vpn", "vpn-0abc")]),
    );
    let body = app.get_detail_lines();
    assert!(has(&body, "Propagations", "3 attachments (2 vpc · 1 vpn) — 3 Propagations"), "{body:#?}");

    app.set_detail_section(2, &tx);
    let tab = app.get_detail_lines();
    assert!(has(&tab, "Propagations (3)", ""), "{tab:#?}");
    assert!(tab.iter().any(|(k, _)| k.contains(ATT) && k.contains("vpc-0abc") && k.contains("enabled")), "{tab:#?}");
}
