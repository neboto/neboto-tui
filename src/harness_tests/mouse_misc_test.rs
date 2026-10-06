//! The smaller mouse routes (#130): the `‹`/`›` overflow markers step a chip,
//! a click on the search bar opens search, the detail pane's title bar
//! toggles full width, and one click on a `→` follows its link.

use super::*;
use crate::app::ClickAction;
use crate::aws::services::ec2::Ec2Instance;
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

fn instance() -> Box<dyn Resource> {
    Box::new(Ec2Instance::from_sdk(
        &aws_sdk_ec2::types::Instance::builder()
            .instance_id("i-0aaa")
            .vpc_id("vpc-0abc")
            .build(),
    ))
}

fn draw(app: &App, w: u16) {
    let mut terminal = Terminal::new(TestBackend::new(w, 30)).unwrap();
    terminal.draw(|f| crate::render_app(app, f)).unwrap();
}

fn region(app: &App, want: impl Fn(&ClickRegion) -> bool) -> Option<Rect> {
    app.click_regions.borrow().iter().find(|r| want(r)).map(|r| r.rect)
}

async fn click(app: &mut App, tx: &mpsc::UnboundedSender<Event>, r: Rect) {
    let ev = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: r.x + r.width / 2,
        row: r.y,
        modifiers: KeyModifiers::NONE,
    };
    app.handle_event(Event::Mouse(ev), tx).await.unwrap();
}

#[tokio::test]
async fn overflow_marker_steps_one_sub_tab() {
    let (mut app, tx, _rx) = test_app().await;
    select_mock(&mut app, ServiceType::EC2, instance());
    // Narrow enough that EC2's sub-tabs overflow on the right.
    draw(&app, 50);
    let marker = region(&app, |r| r.rect.width == 2 && matches!(r.action, ClickAction::Key(_)))
        .expect("a › marker target");
    let before = app.ec2_view;
    click(&mut app, &tx, marker).await;
    assert_ne!(app.ec2_view, before, "the marker switches to the next hidden tab");
}

#[tokio::test]
async fn search_bar_click_opens_search() {
    let (mut app, tx, _rx) = test_app().await;
    select_mock(&mut app, ServiceType::EC2, instance());
    app.details_focused = true;
    draw(&app, 140);
    let bar = region(&app, |r| matches!(r.action, ClickAction::Key('/'))).expect("search bar target");
    click(&mut app, &tx, bar).await;
    assert!(app.search_active, "global search, not the detail body filter");
    draw(&app, 140);
    assert!(region(&app, |r| matches!(r.action, ClickAction::Key('/'))).is_none());
}

#[tokio::test]
async fn detail_title_bar_toggles_full_width() {
    let (mut app, tx, _rx) = test_app().await;
    select_mock(&mut app, ServiceType::EC2, instance());
    draw(&app, 140);
    let geom = app.mouse_geom.get();
    let body = geom.detail_body_area.expect("detail body drawn");
    let title = region(&app, |r| {
        matches!(r.action, ClickAction::Press(KeyCode::Char('Z'))) && r.rect.y < body.y
    })
    .expect("title-bar target");
    let before = app.layout_mode;
    click(&mut app, &tx, title).await;
    assert_ne!(app.layout_mode, before);
}

#[tokio::test]
async fn one_click_on_an_arrow_follows_the_link() {
    let (mut app, tx, _rx) = test_app().await;
    select_mock(&mut app, ServiceType::EC2, instance());
    app.handle_key(key(KeyCode::Char('l')), &tx).await.unwrap();
    let row = app
        .get_detail_lines_filtered()
        .iter()
        .position(|(_, v)| v.contains("vpc-0abc"))
        .expect("a VPC row in the default section");
    app.details_selected_index = Some(row);
    draw(&app, 140);
    let arrow = region(&app, |r| matches!(r.action, ClickAction::FollowJump(i) if i == row))
        .expect("an arrow target on the VPC row");
    // Not on the cursor row any more: the click must still follow *its* row.
    app.details_selected_index = Some(0);
    click(&mut app, &tx, arrow).await;
    assert_eq!(app.current_service, Some(ServiceType::VPC));

    // The unfocused preview records no arrow targets.
    select_mock(&mut app, ServiceType::EC2, instance());
    app.details_focused = false;
    draw(&app, 140);
    assert!(region(&app, |r| matches!(r.action, ClickAction::FollowJump(_))).is_none());
}
