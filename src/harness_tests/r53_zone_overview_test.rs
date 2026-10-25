//! The hosted zone pane opens on an Overview: the one-row header has no
//! room for the zone id or comment, and the detail bundle's facts (record
//! limit, DNSSEC, query logging, name servers, VPCs) sat only in Info and
//! Sharing. Overview holds them, then the first records.

use super::*;
use crate::aws::services::route53::{R53Dnssec, R53HostedZone, R53Record, R53ZoneDetail, R53ZoneVpc};
use aws_sdk_route53::types::{HostedZone, HostedZoneConfig, ResourceRecord, ResourceRecordSet, RrType};

const ZONE_ID: &str = "/hostedzone/Z0123456789ABCDEFGHIJ";

fn zone(private: bool, records: i64) -> R53HostedZone {
    R53HostedZone::from_sdk(
        &HostedZone::builder()
            .id(ZONE_ID)
            .name("example.com.")
            .caller_reference("mock")
            .resource_record_set_count(records)
            .config(
                HostedZoneConfig::builder()
                    .private_zone(private)
                    .comment("Customer-facing apex")
                    .build(),
            )
            .build()
            .unwrap(),
    )
}

fn record(name: &str, value: &str) -> R53Record {
    let set = ResourceRecordSet::builder()
        .name(name)
        .r#type(RrType::A)
        .ttl(300)
        .resource_records(ResourceRecord::builder().value(value).build().unwrap())
        .build()
        .unwrap();
    R53Record::from_sdk(&set, ZONE_ID, "example.com.")
}

fn detail() -> R53ZoneDetail {
    R53ZoneDetail {
        associated: Vec::new(),
        authorized: Vec::new(),
        tags: Vec::new(),
        tags_error: None,
        name_servers: (1..=4).map(|i| format!("ns-{i}.awsdns-0{i}.org")).collect(),
        dnssec: Some(R53Dnssec {
            status: "ACTION_NEEDED".to_string(),
            status_message: Some("KMS key is disabled".to_string()),
            keys: Vec::new(),
        }),
        query_log_group: Some(("/aws/route53/example.com".to_string(), "us-east-1".to_string())),
        record_limit: Some(10_000),
    }
}

/// A zone drilled into the way a user would: focus + the default
/// section's on-enter hook.
async fn focused(
    z: R53HostedZone,
) -> (App, mpsc::UnboundedSender<Event>, mpsc::UnboundedReceiver<Event>) {
    let (mut app, tx, rx) = test_app().await;
    app.current_region = crate::aws::region::Region::UsEast1;
    select_mock(&mut app, ServiceType::Route53, Box::new(z));
    app.details_focused = true;
    app.reset_detail_section_to_default(&tx);
    (app, tx, rx)
}

fn has(body: &[(String, String)], k: &str, v: &str) -> bool {
    body.iter().any(|(bk, bv)| bk == k && bv == v)
}

#[tokio::test]
async fn a_hosted_zone_opens_on_an_overview_led_by_its_id() {
    let (app, _tx, _rx) = focused(zone(false, 12)).await;
    assert_eq!(app.selected_descriptor().unwrap().sections[0].label, "Overview");
    assert_eq!(app.detail_section_idx, 0);
    let body = app.get_detail_lines();
    assert_eq!(body[0], ("Zone ID".to_string(), "Z0123456789ABCDEFGHIJ".to_string()));
    assert!(has(&body, "Zone Name", "example.com."), "{body:#?}");
    assert!(has(&body, "Type", "Public"), "{body:#?}");
    assert!(has(&body, "Comment", "Customer-facing apex"), "{body:#?}");
}

#[tokio::test]
async fn the_overview_hook_starts_the_detail_and_records_fetches() {
    let (app, _tx, _rx) = focused(zone(false, 12)).await;
    assert!(matches!(app.lazy.r53_zone_detail.get(ZONE_ID), Some(crate::lazy::Lazy::Loading)));
    assert!(matches!(app.lazy.r53_zone_records.get(ZONE_ID), Some(crate::lazy::Lazy::Loading)));
    let body = app.get_detail_lines();
    assert!(has(&body, "Query Logging", "Loading…"), "{body:#?}");
}

/// A zone too big to page on a drill-in gets its summary but no records
/// fetch — the preview points at the Records tab instead.
#[tokio::test]
async fn a_big_zone_is_not_paged_for_the_preview() {
    let (app, _tx, _rx) = focused(zone(false, 50_000)).await;
    assert!(matches!(app.lazy.r53_zone_detail.get(ZONE_ID), Some(crate::lazy::Lazy::Loading)));
    assert!(app.lazy.r53_zone_records.get(ZONE_ID).is_none());
    let body = app.get_detail_lines();
    assert!(
        body.iter().any(|(k, v)| k.is_empty() && v.contains("2 Records")),
        "{body:#?}"
    );
}

#[tokio::test]
async fn a_public_zone_summarises_dnssec_logging_and_name_servers() {
    let (mut app, _tx, _rx) = focused(zone(false, 9_500)).await;
    app.lazy.r53_zone_detail.apply(ZONE_ID.to_string(), Ok(Box::new(detail())));
    let body = app.get_detail_lines();
    assert!(has(&body, "Records", "9500 / 10000 (95%) ⚠"), "{body:#?}");
    assert!(
        has(&body, "  ⚠ 9500 of 10000 records used — near the zone's limit", ""),
        "{body:#?}"
    );
    assert!(has(&body, "DNSSEC", "⚠ ACTION_NEEDED"), "{body:#?}");
    assert!(has(&body, "  ⚠ DNSSEC: KMS key is disabled", ""), "{body:#?}");
    assert!(has(&body, "Query Logging", "/aws/route53/example.com"), "{body:#?}");
    assert!(has(&body, "Name Servers (4)", ""), "{body:#?}");
    assert!(has(&body, "  ns-1.awsdns-01.org", ""), "{body:#?}");
    assert!(!body.iter().any(|(k, _)| k == "VPCs"), "public zones have no VPCs row");
}

#[tokio::test]
async fn a_private_zone_lists_its_vpcs_capped() {
    let (mut app, tx, _rx) = focused(zone(true, 3)).await;
    let vpc = |i: usize| R53ZoneVpc {
        vpc_id: format!("vpc-0000000000000000{i}"),
        vpc_region: "us-east-1".to_string(),
    };
    let mut d = detail();
    d.name_servers.clear();
    d.dnssec = None;
    d.query_log_group = None;
    d.associated = (0..7).map(vpc).collect();
    // vpc(0) is still authorized after associating — not pending.
    d.authorized = vec![vpc(0), vpc(9)];
    app.lazy.r53_zone_detail.apply(ZONE_ID.to_string(), Ok(Box::new(d)));
    let body = app.get_detail_lines();
    assert!(has(&body, "Type", "Private"), "{body:#?}");
    assert!(has(&body, "VPC Associations (7)", ""), "{body:#?}");
    assert!(has(&body, "", "· 1 authorized, not yet associated — 3 Sharing"), "{body:#?}");
    assert!(has(&body, "Query Logging", "off"), "{body:#?}");
    assert!(!body.iter().any(|(k, _)| k == "DNSSEC"), "DNSSEC is public-only");
    assert!(!body.iter().any(|(k, _)| k.starts_with("Name Servers")));
    assert_eq!(body.iter().filter(|(k, _)| k == "VPC").count(), 5, "{body:#?}");
    assert!(has(&body, "", "· 2 more — 3 Sharing"), "{body:#?}");

    // ⏎ on an associated VPC jumps to it.
    let row = body.iter().position(|(k, _)| k == "VPC").unwrap();
    app.details_selected_index = Some(row);
    assert!(app.trigger_jump(true, &tx), "a vpc- row must jump");
    assert_eq!(app.current_service, Some(ServiceType::VPC));
}

#[tokio::test]
async fn the_records_preview_is_capped_and_its_rows_jump() {
    let (mut app, tx, _rx) = focused(zone(false, 12)).await;
    let records: Vec<R53Record> = (0..12)
        .map(|i| record(&format!("host{i:02}.example.com."), &format!("10.0.0.{i}")))
        .collect();
    app.lazy.r53_zone_records.apply(ZONE_ID.to_string(), Ok(records));
    let body = app.get_detail_lines();
    assert!(has(&body, "Records (12)", ""), "{body:#?}");
    assert_eq!(body.iter().filter(|(k, _)| k.starts_with("  A ")).count(), 8, "{body:#?}");
    assert!(has(&body, "", "· 4 more — 2 Records"), "{body:#?}");

    let row = body.iter().position(|(k, _)| k.starts_with("  A ")).unwrap();
    app.details_selected_index = Some(row);
    assert!(app.trigger_jump(true, &tx), "a preview record row must jump");
    assert_eq!(app.r53_view, crate::app::R53View::Records);
}

#[tokio::test]
async fn the_query_log_group_jumps_to_cloudwatch() {
    let (mut app, tx, _rx) = focused(zone(false, 12)).await;
    app.lazy.r53_zone_detail.apply(ZONE_ID.to_string(), Ok(Box::new(detail())));
    let body = app.get_detail_lines();
    let row = body.iter().position(|(k, _)| k == "Query Logging").unwrap();
    app.details_selected_index = Some(row);
    assert!(app.trigger_jump(true, &tx), "the log group row must jump");
    assert_eq!(app.current_service, Some(ServiceType::CloudWatch));
}

/// The id moved into the body; the header no longer spends a line on it.
#[tokio::test]
async fn the_header_drops_the_zone_id() {
    let lines = crate::ui::widgets::details_pane::build_r53_zone_header_lines(&zone(false, 12));
    let text: String = lines
        .iter()
        .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
        .collect();
    assert!(!text.contains("Z0123456789ABCDEFGHIJ"), "{text}");
    assert!(!text.contains("Customer-facing apex"), "{text}");
}
