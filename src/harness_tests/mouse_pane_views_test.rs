//! The in-pane views take the mouse (#127): over the view the wheel is `↓`/`↑`,
//! a click puts the cursor on a row, a double-click is `⏎`, and a right-click
//! is `Esc`. Nothing reaches the list beneath while one is open.

use super::*;
use crate::aws::services::ec2::Ec2Instance;
use crate::references::RefRow;
use crate::ui::widgets::refs_lens::RefsLensState;
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

fn instance() -> Box<dyn Resource> {
    Box::new(Ec2Instance::from_sdk(
        &aws_sdk_ec2::types::Instance::builder().instance_id("i-0aaa").build(),
    ))
}

fn draw(app: &App) {
    let mut terminal = Terminal::new(TestBackend::new(140, 40)).unwrap();
    terminal.draw(|f| crate::render_app(app, f)).unwrap();
}

async fn mouse(app: &mut App, tx: &mpsc::UnboundedSender<Event>, kind: MouseEventKind, at: (u16, u16)) {
    let ev = MouseEvent { kind, column: at.0, row: at.1, modifiers: KeyModifiers::NONE };
    app.handle_event(Event::Mouse(ev), tx).await.unwrap();
    draw(app);
}

const LEFT: MouseEventKind = MouseEventKind::Down(MouseButton::Left);

fn row(app: &App, idx: usize) -> (u16, u16) {
    let hits = app.popup_hits.borrow();
    let r = hits.rows.iter().find(|(i, _)| *i == idx).expect("row target").1;
    (r.x + 2, r.y)
}

fn refs_app_rows() -> Vec<RefRow> {
    ["sg-0aaa", "sg-0bbb", "sg-0ccc"]
        .iter()
        .map(|id| RefRow {
            service: ServiceType::EC2,
            resource_type: "Security Group".into(),
            name: id.to_string(),
            id: id.to_string(),
            via: "Security Group".into(),
        })
        .collect()
}

#[tokio::test]
async fn refs_lens_click_wheel_double_click_and_right_click() {
    let (mut app, tx, _rx) = test_app().await;
    select_mock(&mut app, ServiceType::EC2, instance());
    let mut st = RefsLensState::open("i-0aaa".into(), vec!["i-0aaa".into()]);
    st.set_rows(refs_app_rows(), 1, 0);
    app.refs_in_pane = Some(st);
    draw(&app);

    let at = row(&app, 1);
    mouse(&mut app, &tx, LEFT, at).await;
    assert_eq!(app.refs_in_pane.as_ref().unwrap().selected, 1);

    mouse(&mut app, &tx, MouseEventKind::ScrollDown, at).await;
    assert_eq!(app.refs_in_pane.as_ref().unwrap().selected, 2);
    mouse(&mut app, &tx, MouseEventKind::ScrollUp, at).await;
    assert_eq!(app.refs_in_pane.as_ref().unwrap().selected, 1);

    // A click on the list pane beneath does nothing: the lens is the pane.
    let list = app.mouse_geom.get().list_area.expect("list drawn");
    let before = app.selected_index;
    mouse(&mut app, &tx, LEFT, (list.x + 2, list.y + 2)).await;
    assert!(app.refs_in_pane.is_some());
    assert_eq!(app.selected_index, before);

    // Right-click is Esc.
    mouse(&mut app, &tx, MouseEventKind::Down(MouseButton::Right), at).await;
    assert!(app.refs_in_pane.is_none());
}

#[tokio::test]
async fn refs_lens_double_click_jumps() {
    let (mut app, tx, _rx) = test_app().await;
    select_mock(&mut app, ServiceType::EC2, instance());
    let mut st = RefsLensState::open("i-0aaa".into(), vec!["i-0aaa".into()]);
    st.set_rows(refs_app_rows(), 1, 0);
    app.refs_in_pane = Some(st);
    draw(&app);
    let at = row(&app, 2);
    mouse(&mut app, &tx, LEFT, at).await;
    assert!(app.refs_in_pane.is_some(), "one click only moves the cursor");
    mouse(&mut app, &tx, LEFT, at).await;
    assert!(app.refs_in_pane.is_none(), "a double-click jumps, closing the lens");
}

#[tokio::test]
async fn log_tail_wheel_up_pauses_follow() {
    let (mut app, tx, _rx) = test_app().await;
    select_mock(&mut app, ServiceType::EC2, instance());
    app.log_tail.visible = true;
    app.log_tail.follow = true;
    draw(&app);
    let area = app.popup_hits.borrow().area.expect("the tail records its area");
    let mid = (area.x + area.width / 2, area.y + area.height / 2);
    mouse(&mut app, &tx, MouseEventKind::ScrollUp, mid).await;
    assert!(!app.log_tail.follow, "scrolling up pauses follow, as k does");
}

#[tokio::test]
async fn metrics_takes_only_the_right_click() {
    let (mut app, tx, _rx) = test_app().await;
    select_mock(&mut app, ServiceType::EC2, instance());
    app.handle_key(key(KeyCode::Char('m')), &tx).await.unwrap();
    assert!(app.metrics_in_pane.is_some());
    draw(&app);
    let body = app.mouse_geom.get().detail_body_area.unwrap_or(Rect::new(100, 20, 1, 1));
    let mid = (body.x + 2, body.y + 2);
    mouse(&mut app, &tx, MouseEventKind::ScrollDown, mid).await;
    mouse(&mut app, &tx, LEFT, mid).await;
    assert!(app.metrics_in_pane.is_some());
    mouse(&mut app, &tx, MouseEventKind::Down(MouseButton::Right), mid).await;
    assert!(app.metrics_in_pane.is_none());
}
