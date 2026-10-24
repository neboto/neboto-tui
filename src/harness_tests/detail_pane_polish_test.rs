//! Detail pane fit and finish, after nebaz's pane: sections don't open or
//! end on a blank row (so `G` lands on content), the view holds still while
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
async fn no_section_opens_or_ends_on_a_blank_row() {
    let (mut app, tx, _rx) = test_app().await;
    let blank = |r: &(String, String)| r.0.is_empty() && r.1.is_empty();
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
