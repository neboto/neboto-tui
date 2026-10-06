//! A first session with only a mouse (#128): the splash's popular-service
//! chips and its `S`/`R`/`P`/`?` rows are click targets, and the `S` service
//! picker selects on click and opens on a double-click.

use super::*;
use crate::app::ClickAction;
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

fn draw(app: &App) -> Vec<String> {
    let mut terminal = Terminal::new(TestBackend::new(160, 40)).unwrap();
    terminal.draw(|f| crate::render_app(app, f)).unwrap();
    let buf = terminal.backend().buffer();
    (0..buf.area.height)
        .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect())
        .collect()
}

fn text_at(screen: &[String], rect: Rect) -> String {
    screen[rect.y as usize]
        .chars()
        .skip(rect.x as usize)
        .take(rect.width as usize)
        .collect()
}

fn region(app: &App, want: impl Fn(&ClickAction) -> bool) -> Option<Rect> {
    app.click_regions.borrow().iter().find(|r| want(&r.action)).map(|r| r.rect)
}

async fn mouse(app: &mut App, tx: &mpsc::UnboundedSender<Event>, kind: MouseEventKind, rect: Rect) {
    let ev = MouseEvent {
        kind,
        column: rect.x + rect.width / 2,
        row: rect.y,
        modifiers: KeyModifiers::NONE,
    };
    app.handle_event(Event::Mouse(ev), tx).await.unwrap();
}

async fn click(app: &mut App, tx: &mpsc::UnboundedSender<Event>, rect: Rect) {
    mouse(app, tx, MouseEventKind::Down(MouseButton::Left), rect).await;
}

async fn splash_app() -> (App, mpsc::UnboundedSender<Event>, mpsc::UnboundedReceiver<Event>) {
    let (mut app, tx, rx) = test_app().await;
    app.current_service = None;
    (app, tx, rx)
}

#[tokio::test]
async fn splash_service_chips_load_that_service() {
    let (mut app, tx, _rx) = splash_app().await;
    let screen = draw(&app);
    assert!(screen.iter().any(|l| l.contains("Nothing loaded")), "the splash is up");

    let s3 = region(&app, |a| matches!(a, ClickAction::Service(ServiceType::S3)))
        .expect("an @s3 chip on the splash");
    assert_eq!(text_at(&screen, s3), "@s3");
    // Drawing again must not stack a second set of targets.
    draw(&app);
    let chips = app
        .click_regions
        .borrow()
        .iter()
        .filter(|r| matches!(r.action, ClickAction::Service(ServiceType::S3)))
        .count();
    assert_eq!(chips, 1);

    click(&mut app, &tx, s3).await;
    assert_eq!(app.current_service, Some(ServiceType::S3));
}

#[tokio::test]
async fn splash_rows_open_the_pickers_and_quit_is_not_clickable() {
    let (mut app, tx, _rx) = splash_app().await;
    let screen = draw(&app);
    let press = |c: char| move |a: &ClickAction| matches!(a, ClickAction::Press(KeyCode::Char(k)) if *k == c);
    // The splash row for `c`, told apart from the status bar's own `R`/`?`
    // targets by the text it covers.
    let row = |app: &App, c: char, label: &str| {
        app.click_regions
            .borrow()
            .iter()
            .filter(|r| press(c)(&r.action))
            .map(|r| r.rect)
            .find(|r| text_at(&screen, *r).contains(label))
            .unwrap_or_else(|| panic!("no splash row for {c} ({label})"))
    };

    let r = row(&app, 'R', "Switch AWS region");
    assert!(region(&app, press('q')).is_none(), "a stray click must not quit");
    // `@ec2` and `/` only open a text prompt, so their rows aren't targets.
    for label in ["Load a service by prefix", "Search within a service"] {
        assert!(
            !app.click_regions.borrow().iter().any(|r| text_at(&screen, r.rect).contains(label)),
            "{label:?} is keyboard-only"
        );
    }

    click(&mut app, &tx, r).await;
    assert!(app.region_selector.visible);
    app.region_selector.visible = false;

    let help = row(&app, '?', "keybinding help");
    click(&mut app, &tx, help).await;
    assert!(app.help_visible);
    app.help_visible = false;

    let s = row(&app, 'S', "service picker");
    click(&mut app, &tx, s).await;
    assert!(app.service_selector.visible);
}

/// The picker's recorded row for `idx`, checking it shows `name`.
fn picker_row(app: &App, idx: usize) -> Rect {
    app.popup_hits
        .borrow()
        .rows
        .iter()
        .find(|(i, _)| *i == idx)
        .map(|(_, r)| *r)
        .unwrap_or_else(|| panic!("no picker row {idx}"))
}

#[tokio::test]
async fn service_picker_click_selects_and_double_click_opens() {
    let (mut app, tx, _rx) = splash_app().await;
    app.service_selector.show(None);
    let screen = draw(&app);

    let first = app.service_selector.selected_index;
    let target = {
        let hits = app.popup_hits.borrow();
        assert!(!hits.rows.is_empty(), "visible service rows are recorded");
        // Category headers aren't targets.
        assert!(hits.rows.iter().all(|(i, _)| *i != 0));
        hits.rows.iter().map(|(i, _)| *i).find(|i| *i > first + 1).unwrap()
    };
    let rect = picker_row(&app, target);

    // One click highlights without opening.
    click(&mut app, &tx, rect).await;
    assert_eq!(app.service_selector.selected_index, target);
    let service = app.service_selector.selected_service().unwrap();
    assert!(text_at(&screen, rect).contains(service.short_name()));
    assert!(app.service_selector.visible);

    // The wheel moves the highlight and goes nowhere else.
    mouse(&mut app, &tx, MouseEventKind::ScrollDown, rect).await;
    assert_ne!(app.service_selector.selected_index, target);
    mouse(&mut app, &tx, MouseEventKind::ScrollUp, rect).await;
    assert_eq!(app.service_selector.selected_index, target);

    // A second click on the highlighted row opens it.
    click(&mut app, &tx, rect).await;
    assert!(!app.service_selector.visible);
    assert_eq!(app.current_service, Some(service));
}

#[tokio::test]
async fn service_picker_double_click_opens_and_outside_closes() {
    let (mut app, tx, _rx) = splash_app().await;
    app.service_selector.show(None);
    draw(&app);

    // A click outside the popup closes it, loading nothing.
    click(&mut app, &tx, Rect { x: 0, y: 39, width: 1, height: 1 }).await;
    assert!(!app.service_selector.visible);
    assert_eq!(app.current_service, None);

    app.service_selector.show(None);
    draw(&app);
    let first = app.service_selector.selected_index;
    let idx = app.popup_hits.borrow().rows.iter().map(|(i, _)| *i).find(|i| *i != first).unwrap();
    let rect = picker_row(&app, idx);
    click(&mut app, &tx, rect).await;
    let service = app.service_selector.selected_service().unwrap();
    // Simulates the double: the second press lands within the window.
    click(&mut app, &tx, rect).await;
    assert_eq!(app.current_service, Some(service));

    // A right-click closes, as Esc does.
    app.service_selector.show(None);
    draw(&app);
    mouse(&mut app, &tx, MouseEventKind::Down(MouseButton::Right), rect).await;
    assert!(!app.service_selector.visible);
}
