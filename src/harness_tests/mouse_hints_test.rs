//! The mouse route to actions (#129): status-bar key hints, the status-bar
//! region name and the service strip's badges are click targets. Each press
//! lands where focus already is, and each target sits on the text it names.

use super::*;
use crate::app::ClickAction;
use crate::aws::services::ec2::Ec2Instance;
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

fn instance(id: &str) -> Box<dyn Resource> {
    Box::new(Ec2Instance::from_sdk(
        &aws_sdk_ec2::types::Instance::builder().instance_id(id).build(),
    ))
}

/// Draw a frame (regions are recorded as it draws) and return the screen rows.
fn draw(app: &App) -> Vec<String> {
    let mut terminal = Terminal::new(TestBackend::new(160, 30)).unwrap();
    terminal.draw(|f| crate::render_app(app, f)).unwrap();
    let buf = terminal.backend().buffer();
    (0..buf.area.height)
        .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect())
        .collect()
}

/// The rect of the `Press(code)` target, checking it sits on `label`.
fn target(app: &App, screen: &[String], code: KeyCode, label: &str) -> Rect {
    let rect = app
        .click_regions
        .borrow()
        .iter()
        .find(|r| matches!(r.action, ClickAction::Press(c) if c == code))
        .unwrap_or_else(|| panic!("no click target for {code:?}"))
        .rect;
    let text: String = screen[rect.y as usize]
        .chars()
        .skip(rect.x as usize)
        .take(rect.width as usize)
        .collect();
    assert!(text.contains(label), "{code:?}'s target covers {text:?}, not {label:?}");
    rect
}

fn has_target(app: &App, code: KeyCode) -> bool {
    app.click_regions
        .borrow()
        .iter()
        .any(|r| matches!(r.action, ClickAction::Press(c) if c == code))
}

async fn click(app: &mut App, tx: &mpsc::UnboundedSender<Event>, rect: Rect) {
    let ev = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: rect.x + rect.width / 2,
        row: rect.y,
        modifiers: KeyModifiers::NONE,
    };
    app.handle_event(Event::Mouse(ev), tx).await.unwrap();
}

#[tokio::test]
async fn list_hints_open_their_views_and_quit_is_not_clickable() {
    let (mut app, tx, _rx) = test_app().await;
    select_mock(&mut app, ServiceType::EC2, instance("i-0aaa"));

    let screen = draw(&app);
    // `W` is uppercase: replayed with SHIFT, as a terminal reports it.
    let w = target(&app, &screen, KeyCode::Char('W'), "W trail");
    assert!(!has_target(&app, KeyCode::Char('q')), "a stray click must not quit");
    assert!(!has_target(&app, KeyCode::Char('j')), "j/k names two keys");
    click(&mut app, &tx, w).await;
    assert!(app.trail_in_pane.is_some(), "clicking `W trail` opens the timeline");
}

#[tokio::test]
async fn detail_hints_act_in_the_detail_pane() {
    let (mut app, tx, _rx) = test_app().await;
    select_mock(&mut app, ServiceType::EC2, instance("i-0aaa"));
    app.details_focused = true;

    let screen = draw(&app);
    let esc = target(&app, &screen, KeyCode::Esc, "Esc back");
    click(&mut app, &tx, esc).await;
    assert!(!app.details_focused, "clicking `Esc back` returns to the list");
}

#[tokio::test]
async fn region_name_and_strip_hint_open_their_pickers() {
    let (mut app, tx, _rx) = test_app().await;
    select_mock(&mut app, ServiceType::EC2, instance("i-0aaa"));

    let screen = draw(&app);
    let region = app.current_region.display_name().to_string();
    let r = target(&app, &screen, KeyCode::Char('R'), &region);
    click(&mut app, &tx, r).await;
    assert!(app.region_selector.visible, "clicking the region opens the region picker");

    app.region_selector.visible = false;
    let screen = draw(&app);
    let s = target(&app, &screen, KeyCode::Char('S'), "S: services");
    click(&mut app, &tx, s).await;
    assert!(app.service_selector.visible, "clicking `S: services` opens the service picker");
}

#[tokio::test]
async fn search_hints_confirm_but_letters_are_not_typed() {
    let (mut app, tx, _rx) = test_app().await;
    select_mock(&mut app, ServiceType::EC2, instance("i-0aaa"));
    app.search_active = true;
    app.search_query = "web".to_string();

    let screen = draw(&app);
    // The strip's `S` badge is still drawn; a click must not type an `S`.
    let s = target(&app, &screen, KeyCode::Char('S'), "S: services");
    click(&mut app, &tx, s).await;
    assert_eq!(app.search_query, "web");
    assert!(!app.service_selector.visible);

    let enter = target(&app, &screen, KeyCode::Enter, "⏎ confirm");
    click(&mut app, &tx, enter).await;
    assert!(!app.search_active, "clicking `⏎ confirm` confirms the search");
}
