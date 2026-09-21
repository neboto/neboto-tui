//! The hosted-zone Records section once put the record type past the key
//! column's hard cap, so `A` / `CNAME` / `TTL` were clipped out of every row
//! in every pane width (#17). The section-order test only checks section
//! labels, which is why it shipped — this renders real rows.

use super::*;
use crate::aws::services::route53::{R53HostedZone, R53Record};
use aws_sdk_route53::types::{HostedZone, ResourceRecord, ResourceRecordSet, RrType};

const ZONE_ID: &str = "/hostedzone/Z0123456789ABCDEFGHIJ";

fn record(name: &str, rtype: RrType, ttl: i64, values: &[&str]) -> R53Record {
    let mut b = ResourceRecordSet::builder()
        .name(name)
        .r#type(rtype)
        .ttl(ttl);
    for v in values {
        b = b.resource_records(ResourceRecord::builder().value(*v).build().unwrap());
    }
    R53Record::from_sdk(&b.build().unwrap(), ZONE_ID, "example.com.")
}

/// Render the zone's Records section at `width` columns and return the
/// screen as one string per terminal row.
async fn render_records(width: u16, details_only: bool) -> Vec<String> {
    let (mut app, tx, _rx) = test_app().await;
    let zone = R53HostedZone::from_sdk(
        &HostedZone::builder()
            .id(ZONE_ID)
            .name("example.com.")
            .caller_reference("mock")
            .build()
            .unwrap(),
    );
    select_mock(&mut app, ServiceType::Route53, Box::new(zone));
    app.details_focused = true;
    if details_only {
        app.layout_mode = crate::app::LayoutMode::DetailsOnly;
    }
    // Fires the Records trigger (a dead-endpoint fetch we never await) …
    app.reset_detail_section_to_default(&tx);
    // … and delivers the rows the way the apply-closure would.
    app.lazy.r53_zone_records.apply(
        ZONE_ID.to_string(),
        Ok(vec![
            record("example.com.", RrType::A, 300, &["185.0.0.1"]),
            record("www.example.com.", RrType::Cname, 60, &["example.com"]),
            record(
                "example.com.",
                RrType::Txt,
                3600,
                &[
                    "\"v=spf1 include:_spf.example.net ~all\"",
                    "\"ünïcödé-at-the-boundary-of-forty-chars-x\"",
                ],
            ),
        ]),
    );

    let backend = TestBackend::new(width, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| crate::render_app(&app, f)).unwrap();
    let buf = terminal.backend().buffer();
    (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf.get(x, y).symbol().to_string())
                .collect::<String>()
        })
        .collect()
}

#[tokio::test]
async fn records_show_type_before_name_in_a_split_pane_at_80_columns() {
    let screen = render_records(80, false).await;
    let joined = screen.join("\n");
    assert!(
        joined.contains("A      example.com"),
        "A record type + name missing:\n{joined}"
    );
    assert!(
        joined.contains("CNAME  www.example.com"),
        "CNAME record type + name missing:\n{joined}"
    );
    assert!(
        joined.contains("TXT    example.com"),
        "TXT type missing:\n{joined}"
    );
    // Every value starts on the same column as the header's, and no row
    // overflows into a wrapped line: the pane clips at its right edge, so a
    // row is never wider than the terminal.
    assert!(screen.iter().all(|l| l.chars().count() == 80));
}

#[tokio::test]
async fn records_show_value_and_ttl_in_a_full_width_pane() {
    let screen = render_records(120, true).await;
    let joined = screen.join("\n");
    for needle in ["185.0.0.1  ·  300", "example.com  ·  60", "·  3600"] {
        assert!(joined.contains(needle), "{needle:?} missing:\n{joined}");
    }
    // The multi-byte TXT value crosses the 40-char cut and must truncate by
    // chars, not bytes (a byte slice panicked here).
    assert!(
        joined.contains("ünïcödé"),
        "truncated TXT continuation row missing:\n{joined}"
    );
}
