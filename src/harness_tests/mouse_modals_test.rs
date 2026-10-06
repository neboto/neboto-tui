//! The centered modals take the mouse (#127): the wheel moves a picker's
//! highlight, a click highlights a row, a double-click (or a click on the row
//! already highlighted) confirms it as `⏎` does, and a click outside or a
//! right-click closes as `Esc` does.

use super::*;
use crate::aws::services::ec2::Ec2Instance;
use crate::aws::region::Region;
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

fn draw(app: &App) {
    let mut terminal = Terminal::new(TestBackend::new(160, 40)).unwrap();
    terminal.draw(|f| crate::render_app(app, f)).unwrap();
}

async fn mouse(app: &mut App, tx: &mpsc::UnboundedSender<Event>, kind: MouseEventKind, at: (u16, u16)) {
    let ev = MouseEvent { kind, column: at.0, row: at.1, modifiers: KeyModifiers::NONE };
    app.handle_event(Event::Mouse(ev), tx).await.unwrap();
    draw(app);
}

async fn click(app: &mut App, tx: &mpsc::UnboundedSender<Event>, at: (u16, u16)) {
    mouse(app, tx, MouseEventKind::Down(MouseButton::Left), at).await;
}

/// Where to click to hit the picker row for item `idx`.
fn row(app: &App, idx: usize) -> (u16, u16) {
    let hits = app.popup_hits.borrow();
    let r = hits
        .rows
        .iter()
        .find(|(i, _)| *i == idx)
        .unwrap_or_else(|| panic!("row {idx} isn't a target: {:?}", hits.rows))
        .1;
    (r.x + 2, r.y)
}

const OUTSIDE: (u16, u16) = (0, 39);

#[tokio::test]
async fn region_picker_click_wheel_confirm() {
    let (mut app, tx, mut rx) = test_app().await;
    app.region_selector.show(Region::UsEast1);
    draw(&app);
    let start = app.region_selector.selected_index;
    let target = start + 2;

    let at = row(&app, target);
    click(&mut app, &tx, at).await;
    assert_eq!(app.region_selector.selected_index, target);
    assert!(app.region_selector.visible, "one click only highlights");

    let at = row(&app, target);
    mouse(&mut app, &tx, MouseEventKind::ScrollDown, at).await;
    assert_eq!(app.region_selector.selected_index, target + 1);
    let at = row(&app, target);
    mouse(&mut app, &tx, MouseEventKind::ScrollUp, at).await;
    assert_eq!(app.region_selector.selected_index, target);

    let want = app.region_selector.selected_region().unwrap();
    let at = row(&app, target);
    click(&mut app, &tx, at).await;
    assert!(!app.region_selector.visible, "a click on the highlighted row confirms");
    let mut switched = None;
    while let Ok(ev) = rx.try_recv() {
        if let Event::RegionSwitchRequested { region } = ev {
            switched = Some(region);
        }
    }
    assert_eq!(switched, Some(want));
}

#[tokio::test]
async fn pickers_close_on_an_outside_click_or_right_click() {
    let (mut app, tx, _rx) = test_app().await;
    app.region_selector.show(Region::UsEast1);
    draw(&app);
    click(&mut app, &tx, OUTSIDE).await;
    assert!(!app.region_selector.visible);

    app.quota_service_selector.show("ec2");
    draw(&app);
    mouse(&mut app, &tx, MouseEventKind::Down(MouseButton::Right), OUTSIDE).await;
    assert!(!app.quota_service_selector.visible);

    let roles = vec!["ReadOnly".to_string(), "Admin".to_string()];
    app.org_role_selector.show(&roles, "ReadOnly", "111122223333".into(), "dev".into());
    draw(&app);
    let at = row(&app, 1);
    click(&mut app, &tx, at).await;
    assert_eq!(app.org_role_selector.selected_index, 1);
    click(&mut app, &tx, OUTSIDE).await;
    assert!(!app.org_role_selector.visible);
}

#[tokio::test]
async fn ssm_menu_rows_select_and_its_input_stage_backs_out() {
    use crate::ui::widgets::ssm_session_modal::SsmModalStage;
    let (mut app, tx, _rx) = test_app().await;
    app.ssm_session_modal.show("i-0aaa".into(), "web".into());
    draw(&app);
    // Row 1 is a port forward, which asks for its parameters rather than
    // launching anything.
    let at = row(&app, 1);
    click(&mut app, &tx, at).await;
    assert_eq!(app.ssm_session_modal.menu_index, 1);
    let at = row(&app, 1);
    click(&mut app, &tx, at).await;
    assert!(matches!(app.ssm_session_modal.stage, SsmModalStage::Input(_)));
    // Outside, as Esc does there: back to the menu, not closed.
    click(&mut app, &tx, OUTSIDE).await;
    assert!(matches!(app.ssm_session_modal.stage, SsmModalStage::Menu));
    assert!(app.ssm_session_modal.visible);
}

#[tokio::test]
async fn help_scrolls_with_the_wheel_and_closes_outside() {
    let (mut app, tx, _rx) = test_app().await;
    app.help_visible = true;
    draw(&app);
    assert!(app.help_max_scroll.get() > 0, "help overflows a 40-row screen");
    mouse(&mut app, &tx, MouseEventKind::ScrollDown, (80, 20)).await;
    assert_eq!(app.help_scroll, 3);
    mouse(&mut app, &tx, MouseEventKind::ScrollUp, (80, 20)).await;
    assert_eq!(app.help_scroll, 0);
    click(&mut app, &tx, (80, 20)).await;
    assert!(app.help_visible, "a click inside help does nothing");
    click(&mut app, &tx, OUTSIDE).await;
    assert!(!app.help_visible);
}

#[tokio::test]
async fn jump_list_double_click_jumps() {
    let (mut app, tx, _rx) = test_app().await;
    for id in ["i-0aaa", "i-0bbb", "i-0ccc"] {
        let r = Ec2Instance::from_sdk(&aws_sdk_ec2::types::Instance::builder().instance_id(id).build());
        select_mock(&mut app, ServiceType::EC2, Box::new(r));
        app.record_location();
    }
    assert!(app.nav_history.len() >= 2);
    app.jump_list_visible = true;
    app.jump_list_selected = 0;
    draw(&app);
    let at = row(&app, 1);
    click(&mut app, &tx, at).await;
    assert_eq!(app.jump_list_selected, 1);
    assert!(app.jump_list_visible);
    // The second press of a double lands on the now-highlighted row.
    click(&mut app, &tx, at).await;
    assert!(!app.jump_list_visible);
}

#[tokio::test]
async fn macro_name_prompt_ignores_the_mouse() {
    let (mut app, tx, _rx) = test_app().await;
    app.macro_recorder = Some(Default::default());
    app.macro_name_input = Some("my-macro".into());
    app.macro_picker_visible = true;
    draw(&app);
    click(&mut app, &tx, OUTSIDE).await;
    mouse(&mut app, &tx, MouseEventKind::Down(MouseButton::Right), OUTSIDE).await;
    assert!(app.macro_picker_visible, "a stray click must not discard a recording");
    assert!(app.macro_recorder.is_some());
}

#[tokio::test]
async fn ct_filter_wheel_leaves_the_range_alone_and_outside_closes() {
    let (mut app, tx, _rx) = test_app().await;
    let q = app.ct_query.clone();
    app.ct_filter_modal.show(&q);
    draw(&app);
    let before = (app.ct_filter_modal.attr_index, format!("{:?}", app.ct_filter_modal.range));
    mouse(&mut app, &tx, MouseEventKind::ScrollDown, (80, 20)).await;
    let after = (app.ct_filter_modal.attr_index, format!("{:?}", app.ct_filter_modal.range));
    assert_eq!(after, before);
    click(&mut app, &tx, OUTSIDE).await;
    assert!(!app.ct_filter_modal.visible);
}
