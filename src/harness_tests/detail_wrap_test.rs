//! Detail wrap (`Ctrl-W`, #23): a value too long for any pane width — a
//! 255-char TXT/DKIM string, policy JSON in a tag — continues on
//! hanging-indent rows instead of clipping. Rendering changes, the rows
//! don't: the cursor, copy and selection stay logical, and the scroll is
//! computed in screen rows so a wrapped cursor row is never cut off.

use super::*;
use crate::aws::services::route53::{R53HostedZone, R53ZoneDetail};
use aws_sdk_route53::types::HostedZone;

const ZONE_ID: &str = "/hostedzone/Z0123456789ABCDEFGHIJ";
/// Ends the long value; it's on screen only if the whole value is.
const TAIL: &str = "TAILEND";

/// 255 chars, the TXT string ceiling, with no spaces to break on.
fn long_value() -> String {
    let body: String = "v=DKIM1;k=rsa;p=MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEA"
        .chars()
        .cycle()
        .take(255 - TAIL.len())
        .collect();
    format!("{body}{TAIL}")
}

/// A zone focused on its Tags section: 30 short tags, then the long one
/// (sorted last), so the body is taller than the pane.
async fn app_on_long_tag(
    wrap: bool,
) -> (
    App,
    mpsc::UnboundedSender<Event>,
    mpsc::UnboundedReceiver<Event>,
) {
    let (mut app, tx, rx) = test_app().await;
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
    app.detail_wrap = wrap;
    app.reset_detail_section_to_default(&tx);
    let tags_idx = crate::aws::services::route53::R53_ZONE_SECTIONS.len() - 1;
    app.set_detail_section(tags_idx, &tx);

    let mut tags: Vec<(String, String)> =
        (0..30).map(|i| (format!("tag-{i:02}"), format!("v{i}"))).collect();
    tags.push(("zz-dkim".to_string(), long_value()));
    let detail = R53ZoneDetail {
        associated: Vec::new(),
        authorized: Vec::new(),
        tags,
        tags_error: None,
        name_servers: Vec::new(),
        dnssec: None,
        query_log_group: None,
        record_limit: None,
    };
    app.lazy
        .r53_zone_detail
        .apply(ZONE_ID.to_string(), Ok(Box::new(detail)));
    (app, tx, rx)
}

/// Draw at 80×40 (split pane) and return the detail body's rows only — the
/// list pane would otherwise interleave with continuation rows.
fn body_rows(app: &App) -> Vec<String> {
    let mut terminal = Terminal::new(TestBackend::new(80, 40)).unwrap();
    terminal.draw(|f| crate::render_app(app, f)).unwrap();
    let area = app.mouse_geom.get().detail_body_area.expect("detail body drawn");
    let buf = terminal.backend().buffer();
    (area.y..area.y + area.height)
        .map(|y| {
            (area.x..area.x + area.width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect::<String>()
        })
        .collect()
}

fn ctrl_w() -> KeyEvent {
    KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL)
}

/// Index of the long tag's logical row.
fn long_row(app: &App) -> usize {
    app.get_detail_lines()
        .iter()
        .position(|(k, _)| k.trim() == "zz-dkim")
        .expect("long tag row")
}

#[tokio::test]
async fn long_value_is_fully_readable_at_80_columns_with_wrap_on() {
    let (mut app, _tx, _rx) = app_on_long_tag(true).await;
    app.details_selected_index = Some(long_row(&app));
    let rows = body_rows(&app);
    let squashed: String = rows.concat().split_whitespace().collect();
    assert!(
        squashed.contains(&long_value()),
        "the whole 255-char value should be on screen:\n{}",
        rows.join("\n")
    );
    // Continuation rows hang under the value column, not at column 0.
    let first = rows.iter().position(|r| r.contains("zz-dkim")).unwrap();
    let value_col = rows[first].find(": ").unwrap() + 2;
    let cont = &rows[first + 1];
    assert!(
        cont[..value_col].trim().is_empty() && !cont[value_col..].trim().is_empty(),
        "continuation should start at the value column ({value_col}):\n{}",
        rows.join("\n")
    );
}

#[tokio::test]
async fn wrap_is_off_by_default_and_ctrl_w_toggles_it() {
    let (mut app, tx, _rx) = app_on_long_tag(false).await;
    app.details_selected_index = Some(long_row(&app));
    let clipped: String = body_rows(&app).concat();
    assert!(!clipped.contains(TAIL), "clip mode must not show the tail");

    app.handle_key(ctrl_w(), &tx).await.unwrap();
    assert!(app.detail_wrap);
    // `w` is still watch mode, not wrap.
    assert!(!app.watch.enabled);
    let squashed: String = body_rows(&app).concat().split_whitespace().collect();
    assert!(squashed.contains(TAIL));

    app.handle_key(ctrl_w(), &tx).await.unwrap();
    assert!(!app.detail_wrap);
}

#[tokio::test]
async fn cursor_row_stays_fully_visible_after_g_and_k() {
    let (mut app, tx, _rx) = app_on_long_tag(true).await;
    let long = long_row(&app);

    // `G` lands on the trailing spacer; the long row above it is drawn too.
    app.handle_key(key(KeyCode::Char('G')), &tx).await.unwrap();
    let last = app.get_details_line_count() - 1;
    assert_eq!(app.details_selected_index, Some(last));
    body_rows(&app);
    let map = app.detail_body_rows.borrow().clone();
    assert_eq!(map.last(), Some(&last), "cursor row must be on screen");

    // `k` moves one *logical* row, onto the wrapped value — every screen
    // row of it is drawn, tail included.
    app.handle_key(key(KeyCode::Char('k')), &tx).await.unwrap();
    assert_eq!(app.details_selected_index, Some(long));
    let rows = body_rows(&app);
    let map = app.detail_body_rows.borrow().clone();
    let spans = map.iter().filter(|&&i| i == long).count();
    assert!(spans > 1, "the long value should span several screen rows");
    let squashed: String = rows.concat().split_whitespace().collect();
    assert!(squashed.contains(TAIL), "cursor row's tail clipped:\n{}", rows.join("\n"));

    // Scrolling to the top and back keeps the cursor fully visible too.
    app.handle_key(key(KeyCode::Char('g')), &tx).await.unwrap();
    app.handle_key(key(KeyCode::Char('g')), &tx).await.unwrap();
    assert_eq!(app.details_selected_index, Some(0));
    body_rows(&app);
    assert_eq!(app.detail_body_rows.borrow().first(), Some(&0));
}

#[tokio::test]
async fn click_on_a_continuation_row_selects_its_logical_row() {
    let (mut app, tx, _rx) = app_on_long_tag(true).await;
    let long = long_row(&app);
    app.details_selected_index = Some(long);
    body_rows(&app);
    let area = app.mouse_geom.get().detail_body_area.unwrap();
    let map = app.detail_body_rows.borrow().clone();
    // The last screen row of the long value is a continuation.
    let screen_row = map.iter().rposition(|&i| i == long).unwrap();
    assert_eq!(map[screen_row - 1], long, "expected a continuation row");

    app.details_selected_index = Some(0);
    let click = crossterm::event::MouseEvent {
        kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
        column: area.x + 2,
        row: area.y + screen_row as u16,
        modifiers: crossterm::event::KeyModifiers::NONE,
    };
    app.handle_event(Event::Mouse(click), &tx).await.unwrap();
    assert_eq!(app.details_selected_index, Some(long));
}

#[tokio::test]
async fn ctrl_w_in_the_body_search_deletes_a_word() {
    let (mut app, tx, _rx) = app_on_long_tag(false).await;
    app.detail_search_active = true;
    app.detail_search_query = "zz dkim".to_string();
    app.handle_key(ctrl_w(), &tx).await.unwrap();
    assert_eq!(app.detail_search_query, "zz ");
    assert!(!app.detail_wrap, "Ctrl-W while typing must not toggle wrap");
}
