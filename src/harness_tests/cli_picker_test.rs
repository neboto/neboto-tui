//! The `C` command picker (issue #34): which rows a resource offers, that it
//! opens from both panes, that a visual selection merges batchable commands,
//! and that a type with only a read command still copies straight away.

use super::*;
use crate::aws::cli_actions::CliTier;
use crate::aws::services::ec2::Ec2Instance;

fn instance(id: &str) -> Box<dyn Resource> {
    Box::new(Ec2Instance::from_sdk(
        &aws_sdk_ec2::types::Instance::builder().instance_id(id).build(),
    ))
}

async fn press(app: &mut App, tx: &mpsc::UnboundedSender<Event>, code: KeyCode, m: KeyModifiers) {
    app.handle_event(Event::Key(KeyEvent::new(code, m)), tx)
        .await
        .unwrap();
}

fn screen_of(app: &App) -> String {
    let backend = TestBackend::new(140, 40);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| crate::render_app(app, f)).unwrap();
    let buf = terminal.backend().buffer();
    (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn instance_picker_groups_tiers_and_previews_the_exact_command() {
    let (mut app, tx, _rx) = test_app().await;
    select_mock(&mut app, ServiceType::EC2, instance("i-0aaa"));
    press(&mut app, &tx, KeyCode::Char('C'), KeyModifiers::SHIFT).await;

    let picker = app.cli_picker.as_ref().expect("C opens the picker for an instance");
    let labels: Vec<&str> = picker.rows.iter().map(|r| r.label.as_str()).collect();
    assert_eq!(
        labels,
        [
            "describe-instances",
            "get-console-output",
            "ssm start-session",
            "start-instances",
            "stop-instances",
            "reboot-instances"
        ],
        "read command first (not duplicated by the batchable describe), then tier order"
    );
    assert_eq!(picker.selected, 0, "C⏎ still copies the read command");
    let region = app.current_region.as_str().to_string();
    assert!(picker
        .rows
        .iter()
        .all(|r| r.command.contains(&format!("--region {}", region))));
    assert!(picker.rows.iter().all(|r| r.disabled.is_none()));

    let screen = screen_of(&app);
    for needle in [
        "Copy CLI command · i-0aaa",
        "neboto never runs these",
        "Inspect",
        "Connect",
        "Change  ⚠",
        "aws ec2 describe-instances --instance-ids i-0aaa",
    ] {
        assert!(screen.contains(needle), "missing {needle:?} in:\n{screen}");
    }

    // Moving the cursor moves the preview; list keys don't leak through.
    press(&mut app, &tx, KeyCode::Char('j'), KeyModifiers::NONE).await;
    assert_eq!(app.cli_picker.as_ref().unwrap().selected, 1);
    assert!(screen_of(&app).contains("get-console-output --instance-id i-0aaa --latest"));

    press(&mut app, &tx, KeyCode::Esc, KeyModifiers::NONE).await;
    assert!(app.cli_picker.is_none(), "Esc closes");
}

#[tokio::test]
async fn digit_picks_and_closes_and_detail_pane_opens_it_too() {
    let (mut app, tx, _rx) = test_app().await;
    select_mock(&mut app, ServiceType::EC2, instance("i-0aaa"));
    app.details_focused = true;
    // `C` used to be unreachable from the detail pane (its key block
    // returns before the global arm).
    press(&mut app, &tx, KeyCode::Char('C'), KeyModifiers::SHIFT).await;
    assert!(app.cli_picker.is_some(), "C opens from the detail pane");

    press(&mut app, &tx, KeyCode::Char('5'), KeyModifiers::NONE).await;
    assert!(app.cli_picker.is_none(), "a digit copies that row and closes");
    // Headless CI has no clipboard; either way the attempt was for row 5.
    let msg = app
        .success_message
        .clone()
        .or(app.error_message.clone())
        .unwrap_or_default();
    assert!(
        msg.contains("aws ec2 stop-instances") || msg.contains("Clipboard") || msg.contains("copy"),
        "unexpected message {msg:?}"
    );
}

#[tokio::test]
async fn visual_selection_merges_batchable_commands() {
    let (mut app, tx, _rx) = test_app().await;
    select_mock(&mut app, ServiceType::EC2, instance("i-0aaa"));
    app.resources = vec![instance("i-0aaa"), instance("i-0bbb"), instance("i-0ccc")];
    app.filtered_resources = vec![0, 1, 2];
    app.selected_index = Some(2);
    app.list_visual_anchor = Some(0);

    press(&mut app, &tx, KeyCode::Char('C'), KeyModifiers::SHIFT).await;
    let picker = app.cli_picker.as_ref().expect("picker opens for a selection");
    assert_eq!(picker.subject, "3 selected");
    let stop = picker
        .rows
        .iter()
        .find(|r| r.label == "stop-instances")
        .expect("stop is batchable");
    assert!(stop
        .command
        .starts_with("aws ec2 stop-instances --instance-ids i-0aaa i-0bbb i-0ccc --region"));
    assert_eq!(stop.tier, CliTier::Change);
    assert!(
        !picker.rows.iter().any(|r| r.label == "ssm start-session"),
        "per-instance commands can't merge"
    );
    assert!(picker.rows.iter().any(|r| r.label == "describe-instances"));
}

#[tokio::test]
async fn read_only_type_copies_without_a_picker() {
    let (mut app, tx, _rx) = test_app().await;
    let sg = crate::aws::services::ec2::SecurityGroup::from_sdk(
        &aws_sdk_ec2::types::SecurityGroup::builder()
            .group_id("sg-0aaa")
            .build(),
    );
    select_mock(&mut app, ServiceType::EC2, Box::new(sg));
    press(&mut app, &tx, KeyCode::Char('C'), KeyModifiers::SHIFT).await;
    assert!(app.cli_picker.is_none(), "one row copies straight away, as C always has");
}

/// The mouse on the picker (#127): the hit targets come from the renderer, so
/// each row's rect must land on the line that shows that row's label. A click
/// highlights, a double-click copies that row (not whichever was highlighted),
/// the wheel moves the highlight, and a click outside closes it. None of it
/// reaches the list beneath (#131).
#[tokio::test]
async fn mouse_clicks_rows_double_click_copies_and_outside_closes() {
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    let mouse = |kind, column, row| {
        Event::Mouse(MouseEvent { kind, column, row, modifiers: KeyModifiers::NONE })
    };
    let left = MouseEventKind::Down(MouseButton::Left);

    let (mut app, tx, _rx) = test_app().await;
    select_mock(&mut app, ServiceType::EC2, instance("i-0aaa"));
    app.details_focused = true;
    press(&mut app, &tx, KeyCode::Char('C'), KeyModifiers::SHIFT).await;

    let screen = screen_of(&app);
    let screen_rows: Vec<&str> = screen.lines().collect();
    let hits = app.popup_hits.borrow().clone();
    let labels: Vec<String> = app.cli_picker.as_ref().unwrap().rows.iter().map(|r| r.label.clone()).collect();
    assert_eq!(hits.rows.len(), labels.len(), "every row is a target");
    for (idx, rect) in &hits.rows {
        let line = screen_rows[rect.y as usize];
        assert!(line.contains(&labels[*idx]), "row {idx}'s target is on {line:?}");
    }

    // Click "stop-instances" (row 4): highlighted, still open.
    let stop = hits.rows.iter().find(|(i, _)| *i == 4).unwrap().1;
    let (x, y) = (stop.x + 6, stop.y);
    app.handle_event(mouse(left, x, y), &tx).await.unwrap();
    assert_eq!(app.cli_picker.as_ref().map(|p| p.selected), Some(4));

    // The wheel moves the highlight, not the list underneath.
    app.handle_event(mouse(MouseEventKind::ScrollDown, x, y), &tx).await.unwrap();
    assert_eq!(app.cli_picker.as_ref().map(|p| p.selected), Some(5));
    assert!(app.details_focused, "the wheel leaked through to the list");

    // A double-click on row 4 copies row 4, though row 5 was highlighted.
    app.handle_event(mouse(left, x, y), &tx).await.unwrap();
    app.handle_event(mouse(left, x, y), &tx).await.unwrap();
    assert!(app.cli_picker.is_none(), "a double-click copies and closes");
    let msg = app.success_message.clone().or(app.error_message.clone()).unwrap_or_default();
    assert!(
        msg.contains("aws ec2 stop-instances") || msg.contains("Clipboard") || msg.contains("copy"),
        "unexpected message {msg:?}"
    );

    // Reopen; a click outside the popup closes it and changes nothing beneath.
    press(&mut app, &tx, KeyCode::Char('C'), KeyModifiers::SHIFT).await;
    screen_of(&app);
    let area = app.popup_hits.borrow().area.unwrap();
    app.handle_event(mouse(left, area.x.saturating_sub(2), area.y), &tx).await.unwrap();
    assert!(app.cli_picker.is_none(), "a click outside closes");
    assert!(app.details_focused, "the closing click leaked through to the list");
}
