//! Hosted zone tags shipped as always-empty (#26): `ListHostedZones` carries
//! none, and the lazy bundle's `ListTagsForResource` got the `/hostedzone/`
//! path id and swallowed the resulting error into "No tags". These tests
//! pin the two halves of the fix — the eager `tags` field feeds every
//! `Resource::tags()` consumer, and a failed bundle tag call renders as a
//! warning row, never as "No tags".

use super::*;
use crate::aws::services::route53::{R53HostedZone, R53ZoneDetail};
use aws_sdk_route53::types::HostedZone;

const ZONE_ID: &str = "/hostedzone/Z0123456789ABCDEFGHIJ";

fn zone_with_tags(tags: &[(&str, &str)]) -> R53HostedZone {
    let mut zone = R53HostedZone::from_sdk(
        &HostedZone::builder()
            .id(ZONE_ID)
            .name("example.com.")
            .caller_reference("mock")
            .build()
            .unwrap(),
    );
    for (k, v) in tags {
        zone.tags.insert(k.to_string(), v.to_string());
    }
    zone
}

fn screen(app: &App) -> String {
    let backend = TestBackend::new(140, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| crate::render_app(app, f)).unwrap();
    let buf = terminal.backend().buffer();
    (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf.get(x, y).symbol().to_string())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn zone_tags_feed_the_ribbon_and_tag_filters() {
    let (mut app, _tx, _rx) = test_app().await;
    select_mock(
        &mut app,
        ServiceType::Route53,
        Box::new(zone_with_tags(&[("team", "dns"), ("env", "prod")])),
    );
    app.details_focused = true;
    app.layout_mode = crate::app::LayoutMode::DetailsOnly;

    // Ownership ribbon resolves from `Resource::tags()` alone.
    let s = screen(&app);
    assert!(
        s.contains("team dns"),
        "ribbon missing zone owner tag:\n{s}"
    );

    // `tag:key=value` is an exact filter over the same accessor.
    app.details_focused = false;
    app.search_query = "tag:env=prod".to_string();
    app.update_search();
    assert_eq!(
        app.filtered_resources.len(),
        1,
        "tag filter should match the zone"
    );
    app.search_query = "tag:env=staging".to_string();
    app.update_search();
    assert!(
        app.filtered_resources.is_empty(),
        "tag filter should exclude the zone"
    );
}

#[tokio::test]
async fn failed_bundle_tag_call_renders_a_warning_not_no_tags() {
    let (mut app, tx, _rx) = test_app().await;
    select_mock(
        &mut app,
        ServiceType::Route53,
        Box::new(zone_with_tags(&[])),
    );
    app.details_focused = true;
    app.layout_mode = crate::app::LayoutMode::DetailsOnly;
    app.reset_detail_section_to_default(&tx);
    // Tags is the last section of the zone pane.
    let tags_idx = crate::aws::services::route53::R53_ZONE_SECTIONS.len() - 1;
    app.set_detail_section(tags_idx, &tx);

    let failed = R53ZoneDetail {
        associated: Vec::new(),
        authorized: Vec::new(),
        tags: Vec::new(),
        tags_error: Some("AccessDenied: route53:ListTagsForResource".to_string()),
        name_servers: Vec::new(),
        dnssec: None,
        query_log_group: None,
        record_limit: None,
    };
    app.lazy
        .r53_zone_detail
        .apply(ZONE_ID.to_string(), Ok(Box::new(failed)));

    let s = screen(&app);
    assert!(
        s.contains("⚠ AccessDenied"),
        "tag failure should surface as a warning row:\n{s}"
    );
    assert!(
        !s.contains("No tags"),
        "a failed tag call must not read as no tags:\n{s}"
    );
}
