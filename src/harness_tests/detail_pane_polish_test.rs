//! Detail pane fit and finish, after nebaz's pane: sections are evenly
//! spaced (none opens or ends on a blank row, so `G` lands on content), the view holds still while
//! the cursor moves inside it, the section bar scrolls to the active section,
//! the `/` filter marks and counts its matches with `n`/`N` between them, and
//! the footer's keys are coloured click targets.

use super::*;
use crate::app::ClickAction;
use crate::aws::services::route53::{R53HostedZone, R53ZoneDetail};
use aws_sdk_route53::types::HostedZone;
use ratatui::style::Modifier;

const ZONE_ID: &str = "/hostedzone/Z0123456789ABCDEFGHIJ";

/// A zone focused on its Tags section, 40 tags long: taller than the pane.
async fn app_on_tags() -> (App, mpsc::UnboundedSender<Event>, mpsc::UnboundedReceiver<Event>) {
    let (mut app, tx, rx) = test_app().await;
    let zone = R53HostedZone::from_sdk(
        &HostedZone::builder().id(ZONE_ID).name("example.com.").caller_reference("mock").build().unwrap(),
    );
    select_mock(&mut app, ServiceType::Route53, Box::new(zone));
    app.details_focused = true;
    app.reset_detail_section_to_default(&tx);
    let tags_idx = crate::aws::services::route53::R53_ZONE_SECTIONS.len() - 1;
    app.set_detail_section(tags_idx, &tx);
    let detail = R53ZoneDetail {
        associated: Vec::new(),
        authorized: Vec::new(),
        tags: (0..40).map(|i| (format!("tag-{i:02}"), format!("v{i}"))).collect(),
        tags_error: None,
        name_servers: Vec::new(),
        dnssec: None,
        query_log_group: None,
        record_limit: None,
    };
    app.lazy.r53_zone_detail.apply(ZONE_ID.to_string(), Ok(Box::new(detail)));
    (app, tx, rx)
}

fn draw(app: &App, w: u16, h: u16) -> Terminal<TestBackend> {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal.draw(|f| crate::render_app(app, f)).unwrap();
    terminal
}

fn row_text(t: &Terminal<TestBackend>, y: u16) -> String {
    let buf = t.backend().buffer();
    (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect()
}

fn top_row(app: &App) -> usize {
    app.detail_body_rows.borrow().first().copied().unwrap_or(0)
}

#[tokio::test]
async fn every_section_is_evenly_spaced_with_no_placeholder_keys() {
    let (mut app, tx, _rx) = test_app().await;
    let blank = |r: &(String, String)| r.0.is_empty() && r.1.is_empty();
    let header = |r: &(String, String)| !r.0.is_empty() && r.1.is_empty() && !r.0.starts_with(' ');
    for (service, label, mock) in all_mocks() {
        let Some(desc) = mock.detail_sections() else { continue };
        let n = desc.sections.len();
        select_mock(&mut app, service, mock);
        app.details_focused = true;
        app.reset_detail_section_to_default(&tx);
        for i in 0..n {
            app.set_detail_section(i, &tx);
            let lines = app.get_detail_lines();
            assert!(!lines.first().is_some_and(blank), "{label} §{i} opens on a blank row");
            assert!(!lines.last().is_some_and(blank), "{label} §{i} ends on a blank row (G lands on it)");
            for w in lines.windows(2) {
                assert!(!(blank(&w[0]) && blank(&w[1])), "{label} §{i}: two blank rows in a row");
                assert!(!(header(&w[0]) && blank(&w[1])), "{label} §{i}: a blank row under {:?}", w[0].0);
            }
            // "Nothing here" is a note (`("", "No tags")`), not a key named
            // `(none)` with a colon after it.
            for (k, v) in &lines {
                assert!(!(k.trim() == "(none)" && !v.is_empty()), "{label} §{i}: `(none): {v}` should be a note");
            }
        }
    }
}

#[tokio::test]
async fn k_moves_the_cursor_and_the_view_holds_still() {
    let (mut app, tx, _rx) = app_on_tags().await;
    app.handle_key(key(KeyCode::Char('G')), &tx).await.unwrap();
    draw(&app, 100, 30);
    let top = top_row(&app);
    assert!(top > 0, "40 tags should scroll in a 30-row terminal");
    for _ in 0..3 {
        app.handle_key(key(KeyCode::Char('k')), &tx).await.unwrap();
        draw(&app, 100, 30);
        assert_eq!(top_row(&app), top, "the view moved while the cursor was still on screen");
    }
    // It scrolls once the cursor would leave the top.
    app.handle_key(key(KeyCode::Char('g')), &tx).await.unwrap();
    app.handle_key(key(KeyCode::Char('g')), &tx).await.unwrap();
    draw(&app, 100, 30);
    assert_eq!(top_row(&app), 0);
}

#[tokio::test]
async fn the_section_bar_scrolls_to_a_section_that_would_not_fit() {
    let (mut app, tx, _rx) = test_app().await;
    let (service, _, mock) = all_mocks().into_iter().find(|(_, l, _)| *l == "Ec2Instance").unwrap();
    let desc = mock.detail_sections().unwrap();
    let last = desc.sections.len() - 1;
    let label = desc.sections[last].label;
    select_mock(&mut app, service, mock);
    app.details_focused = true;
    app.reset_detail_section_to_default(&tx);
    app.set_detail_section(last, &tx);
    let t = draw(&app, 100, 30);
    let screen: Vec<String> = (0..30).map(|y| row_text(&t, y)).collect();
    let bar = screen.iter().find(|r| r.contains(label)).unwrap_or_else(|| {
        panic!("the active section “{label}” isn't on screen:\n{}", screen.join("\n"))
    });
    assert!(bar.contains('‹'), "a marker should say sections are hidden to the left: {bar}");
}

#[tokio::test]
async fn the_filter_marks_counts_and_steps_through_its_matches() {
    let (mut app, tx, _rx) = app_on_tags().await;
    app.handle_key(key(KeyCode::Char('/')), &tx).await.unwrap();
    for c in "tag-1".chars() {
        app.handle_key(key(KeyCode::Char(c)), &tx).await.unwrap();
    }
    app.handle_key(key(KeyCode::Enter), &tx).await.unwrap();

    let matches = app.detail_match_rows();
    assert_eq!(matches.len(), 10, "tag-10 … tag-19");
    let t = draw(&app, 100, 30);
    let screen: String = (0..30).map(|y| row_text(&t, y)).collect();
    assert!(screen.contains("(10 matches)"), "{screen}");
    assert!(screen.contains("n/N next"), "{screen}");
    let buf = t.backend().buffer();
    assert!(
        buf.content().iter().any(|c| c.modifier.contains(Modifier::REVERSED) && c.symbol() == "-"),
        "the matched text should be marked in place"
    );

    app.details_selected_index = Some(matches[0]);
    app.handle_key(key(KeyCode::Char('n')), &tx).await.unwrap();
    assert_eq!(app.details_selected_index, Some(matches[1]));
    let shift_n = KeyEvent::new(KeyCode::Char('N'), KeyModifiers::SHIFT);
    app.handle_key(shift_n, &tx).await.unwrap();
    app.handle_key(shift_n, &tx).await.unwrap();
    assert_eq!(app.details_selected_index, Some(*matches.last().unwrap()), "N wraps to the last match");
    assert!(app.access_in_pane.is_none(), "N steps matches while filtering, not the access lens");
}

#[tokio::test]
async fn footer_keys_are_click_targets_on_the_bottom_border() {
    let (app, _tx, _rx) = app_on_tags().await;
    draw(&app, 120, 30);
    let pane = app.mouse_geom.get().detail_body_area.unwrap();
    let regions = app.click_regions.borrow();
    let search = regions
        .iter()
        .find(|r| matches!(r.action, ClickAction::Press(KeyCode::Char('/'))) && r.rect.y > pane.y + pane.height - 1)
        .expect("the footer's `/ search` hint should be clickable");
    assert_eq!(search.rect.width, "/ search".len() as u16);
}

/// The one-row header (`header_line`, via `render_simple_split`): dot, name,
/// state, then the facts that fit — nothing repeated, the name in the title.
#[tokio::test]
async fn simple_split_panes_have_a_one_row_header() {
    let (mut app, tx, _rx) = test_app().await;
    let (service, _, mock) = all_mocks().into_iter().find(|(_, l, _)| *l == "Vpc").unwrap();
    let name = mock.name().to_string();
    select_mock(&mut app, service, mock);
    app.details_focused = true;
    app.reset_detail_section_to_default(&tx);
    let t = draw(&app, 130, 24);
    let screen: Vec<String> = (0..24).map(|y| row_text(&t, y)).collect();
    let title = screen.iter().position(|r| r.contains(&format!("VPC · {name}"))).expect("name in the title");
    assert!(screen[title + 1].contains(&name) && screen[title + 1].contains("10.0.0.0/16"), "{}", screen[title + 1]);
    assert!(screen[title + 2].contains("────"), "the header should be one row:\n{}", screen.join("\n"));

    let line = crate::ui::widgets::details_pane::header_line(&app, &name, &["x", "x", "", "—", &name], 120);
    let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
    assert_eq!(text.matches(" x").count(), 1, "repeats and blanks are skipped: {text}");
    let narrow = crate::ui::widgets::details_pane::header_line(&app, "web", &["a long fact that cannot fit in forty columns"], 40);
    let text: String = narrow.spans.iter().map(|s| s.content.as_ref()).collect();
    assert!(text.ends_with('…') && unicode_width::UnicodeWidthStr::width(text.as_str()) <= 40, "{text}");
}

/// Every split pane's header is one row: the rows between the pane's top
/// border and the rule above the section tabs. A pane that builds its own
/// header goes through `one_row_header`; a new one that skips it fails here.
#[tokio::test]
async fn every_split_pane_has_a_one_row_header() {
    let (mut app, tx, _rx) = test_app().await;
    for (service, label, mock) in all_mocks() {
        if mock.detail_sections().is_none() {
            continue;
        }
        select_mock(&mut app, service, mock);
        app.details_focused = true;
        app.reset_detail_section_to_default(&tx);
        let t = draw(&app, 150, 30);
        let buf = t.backend().buffer();
        let body = app.mouse_geom.get().detail_body_area.unwrap_or_else(|| panic!("{label}: no body"));
        let row = |y: u16| -> String { (body.x..body.x + body.width).map(|x| buf[(x, y)].symbol().to_string()).collect() };
        let top = (0..body.y).rev().find(|&y| buf[(body.x - 1, y)].symbol() == "╭").expect("pane top border");
        let rule = (top + 1..body.y).find(|&y| row(y).starts_with("──")).expect("rule under the header");
        assert_eq!(rule - top - 1, 1, "{label}: header is {} rows:\n{}", rule - top - 1,
            (top..rule).map(row).collect::<Vec<_>>().join("\n"));
        // The border title names the resource, cut with `…` if it must be.
        let title: String = (body.x - 1..body.x + body.width + 1).map(|x| buf[(x, top)].symbol().to_string()).collect();
        let name = app.get_selected_resource().unwrap().name().to_string();
        let stem: String = name.chars().take(6).collect();
        assert!(title.contains(&name) || (title.contains(&stem) && title.contains('…')), "{label}: title {title:?} doesn't name {name:?}");
    }
}

/// A fact drawn in its own colour (a severity, an AZ) keeps it in the one
/// row; facts in the body's neutral colours go dim like the rest.
#[tokio::test]
async fn a_coloured_fact_keeps_its_colour_in_the_one_row_header() {
    use ratatui::style::{Color, Style};
    use ratatui::text::{Line, Span};
    let (mut app, tx, _rx) = test_app().await;
    let (service, _, mock) = all_mocks().into_iter().find(|(_, l, _)| *l == "Vpc").unwrap();
    select_mock(&mut app, service, mock);
    app.reset_detail_section_to_default(&tx);
    let lines = vec![
        Line::from(Span::raw("web")),
        Line::from(vec![
            Span::styled("HIGH (7.5)", Style::default().fg(Color::Red)),
            Span::styled("  ·  ", Style::default().fg(crate::ui::theme::text_dim())),
            Span::styled("us-east-1a", Style::default().fg(crate::ui::theme::text_primary())),
        ]),
    ];
    let row = crate::ui::widgets::details_pane::one_row_header(&app, lines, 120).remove(0);
    let fg = |t: &str| row.spans.iter().find(|s| s.content.as_ref() == t).and_then(|s| s.style.fg);
    assert_eq!(fg("HIGH (7.5)"), Some(Color::Red));
    assert_eq!(fg("us-east-1a"), Some(crate::ui::theme::text_dim()));
}

/// A security group opens on an Overview that keeps what the one-row
/// header drops (the description, the owner), names the rules that open
/// it to the internet, and starts the interface count's fetch on focus.
#[tokio::test]
async fn a_security_group_opens_on_an_overview() {
    use aws_sdk_ec2::types::{IpPermission, IpRange};
    let (mut app, tx, _rx) = test_app().await;
    let rule = |port: i32, cidr: &str| {
        IpPermission::builder()
            .ip_protocol("tcp")
            .from_port(port)
            .to_port(port)
            .ip_ranges(IpRange::builder().cidr_ip(cidr).build())
            .build()
    };
    let sg = crate::aws::services::ec2::SecurityGroup::from_sdk(
        &aws_sdk_ec2::types::SecurityGroup::builder()
            .group_id("sg-0a1b2c3d4e5f60718")
            .group_name("web-sg")
            .vpc_id("vpc-0123456789abcdef0")
            .owner_id("123456789012")
            .description("Web tier: HTTPS from the internet")
            .ip_permissions(rule(443, "0.0.0.0/0"))
            .ip_permissions(rule(22, "203.0.113.0/24"))
            .build(),
    );
    select_mock(&mut app, ServiceType::EC2, Box::new(sg));
    app.details_focused = true;
    app.reset_detail_section_to_default(&tx);

    assert_eq!(app.selected_descriptor().unwrap().sections[0].label, "Overview");
    assert!(matches!(
        app.lazy.sg_network_interfaces.get("sg-0a1b2c3d4e5f60718"),
        Some(crate::lazy::Lazy::Loading)
    ));
    let body = app.get_detail_lines();
    let has = |k: &str, v: &str| body.iter().any(|(bk, bv)| bk == k && bv == v);
    assert!(has("Description", "Web tier: HTTPS from the internet"), "{body:#?}");
    assert!(has("Owner", "123456789012"), "{body:#?}");
    // The header repeats these, but only body rows can be copied with `y`.
    assert_eq!(body[0], ("Group ID".to_string(), "sg-0a1b2c3d4e5f60718".to_string()));
    assert!(has("Group Name", "web-sg"), "{body:#?}");
    assert!(has("  ⚠ TCP 443 open to 0.0.0.0/0", ""), "{body:#?}");
    assert!(!body.iter().any(|(k, _)| k.contains("port 22")), "only world-open rules are flagged");
    // The rules themselves, in the Inbound tab's table form.
    assert!(has("Inbound (2 rules)", ""), "{body:#?}");
    assert!(body.iter().any(|(k, _)| k.contains("22") && k.contains("→  203.0.113.0/24")), "{body:#?}");
    assert!(has("Outbound (0 rules)", "") && has("", "No outbound rules"), "{body:#?}");

    // The group's own id is no jump: ⏎ would only re-land here.
    app.details_selected_index = Some(0);
    let history = app.nav_history.len();
    app.handle_key(key(KeyCode::Enter), &tx).await.unwrap();
    assert_eq!(app.nav_history.len(), history, "Enter on the own-id row records no jump");
    assert_eq!(app.details_selected_index, Some(0), "and leaves the cursor where it was");
    let vpc_row = body.iter().position(|(k, _)| k == "VPC").unwrap();
    app.details_selected_index = Some(vpc_row);
    app.handle_key(key(KeyCode::Enter), &tx).await.unwrap();
    assert_eq!(app.pending_jump.as_deref(), Some("vpc-0123456789abcdef0"), "the VPC row still jumps");

    let (mut app, tx, _rx) = test_app().await;
    select_mock(&mut app, ServiceType::EC2, Box::new(crate::aws::services::ec2::SecurityGroup::from_sdk(
        &aws_sdk_ec2::types::SecurityGroup::builder().group_id("sg-0a1b2c3d4e5f60718").group_name("web-sg").build(),
    )));
    app.details_focused = true;
    app.reset_detail_section_to_default(&tx);
    app.lazy.sg_network_interfaces.apply("sg-0a1b2c3d4e5f60718".into(), Ok(Vec::new()));
    assert!(app.get_detail_lines().iter().any(|(k, v)| k == "Network Interfaces" && v == "none — unused"));
}

/// A long rule list shows its first eight on the Overview and points at
/// the tab that has the rest.
#[tokio::test]
async fn the_overview_caps_a_long_rule_list() {
    use aws_sdk_ec2::types::{IpPermission, IpRange};
    let (mut app, tx, _rx) = test_app().await;
    let mut b = aws_sdk_ec2::types::SecurityGroup::builder().group_id("sg-0big").group_name("big");
    for port in 8000..8011 {
        b = b.ip_permissions(
            IpPermission::builder()
                .ip_protocol("tcp")
                .from_port(port)
                .to_port(port)
                .ip_ranges(IpRange::builder().cidr_ip("10.0.0.0/8").build())
                .build(),
        );
    }
    select_mock(&mut app, ServiceType::EC2, Box::new(crate::aws::services::ec2::SecurityGroup::from_sdk(&b.build())));
    app.details_focused = true;
    app.reset_detail_section_to_default(&tx);
    let body = app.get_detail_lines();
    assert_eq!(body.iter().filter(|(k, _)| k.contains("→  10.0.0.0/8")).count(), 8, "{body:#?}");
    assert!(body.iter().any(|(k, v)| k.is_empty() && v == "· 3 more — 2 Inbound"), "{body:#?}");
}

/// A hundred world-open rules: Exposure folds them by protocol and
/// source, names the first ports, and counts the rest.
#[tokio::test]
async fn exposure_folds_a_hundred_open_rules() {
    use aws_sdk_ec2::types::{IpPermission, IpRange, Ipv6Range};
    let (mut app, tx, _rx) = test_app().await;
    let mut b = aws_sdk_ec2::types::SecurityGroup::builder().group_id("sg-0open").group_name("open");
    for port in 8000..8100 {
        b = b.ip_permissions(
            IpPermission::builder()
                .ip_protocol("tcp")
                .from_port(port)
                .to_port(port)
                .ip_ranges(IpRange::builder().cidr_ip("0.0.0.0/0").build())
                .build(),
        );
    }
    b = b.ip_permissions(
        IpPermission::builder()
            .ip_protocol("udp")
            .from_port(53)
            .to_port(53)
            .ipv6_ranges(Ipv6Range::builder().cidr_ipv6("::/0").build())
            .build(),
    );
    select_mock(&mut app, ServiceType::EC2, Box::new(crate::aws::services::ec2::SecurityGroup::from_sdk(&b.build())));
    app.details_focused = true;
    app.reset_detail_section_to_default(&tx);
    let body = app.get_detail_lines();
    let warnings: Vec<&str> = body.iter().filter(|(k, _)| k.starts_with("  ⚠")).map(|(k, _)| k.as_str()).collect();
    assert_eq!(
        warnings,
        [
            "  ⚠ TCP 8000, 8001, 8002, 8003, 8004, 8005, 8006, 8007 +92 more open to 0.0.0.0/0",
            "  ⚠ UDP 53 open to ::/0",
        ]
    );
    assert!(body.iter().any(|(k, v)| k.is_empty() && v == "· 101 rules open to the internet — 2 Inbound has them all"), "{body:#?}");
}
