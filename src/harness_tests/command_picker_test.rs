//! `:` lists the keys that run here, by name, and `⏎` replays the row's
//! key through the event channel.

use super::*;
use crate::aws::services::ec2::Ec2Instance;

fn instance() -> Box<dyn Resource> {
    Box::new(Ec2Instance::from_sdk(
        &aws_sdk_ec2::types::Instance::builder()
            .instance_id("i-0abc")
            .security_groups(aws_sdk_ec2::types::GroupIdentifier::builder().group_id("sg-0abc").build())
            .build(),
    ))
}

async fn press(app: &mut App, tx: &mpsc::UnboundedSender<Event>, code: KeyCode) {
    app.handle_event(Event::Key(crossterm::event::KeyEvent::from(code)), tx).await.unwrap();
}

async fn type_text(app: &mut App, tx: &mpsc::UnboundedSender<Event>, text: &str) {
    for c in text.chars() {
        press(app, tx, KeyCode::Char(c)).await;
    }
}

fn keys(app: &App) -> Vec<&'static str> {
    crate::commands::picker_rows(app, &app.command_picker.query).iter().map(|c| c.key).collect()
}

#[tokio::test]
async fn colon_runs_a_command_by_name() {
    let (mut app, tx, mut rx) = test_app().await;
    select_mock(&mut app, ServiceType::EC2, instance());

    press(&mut app, &tx, KeyCode::Char(':')).await;
    assert!(app.command_picker.visible);
    // No query: the keys this instance has lead, list-pane rows only.
    let listed = keys(&app);
    assert_eq!(listed[0], "m", "{listed:?}");
    assert!(listed.contains(&"N") && listed.contains(&"z"), "{listed:?}");
    assert!(!listed.contains(&"x / Y"), "an instance has no value to reveal: {listed:?}");
    assert!(!listed.contains(&"Z"), "detail-pane rows stay out of the list pane: {listed:?}");
    assert!(!listed.contains(&"j / k ↑ / ↓"), "not one key: {listed:?}");

    let screen = {
        let mut terminal = Terminal::new(TestBackend::new(140, 40)).unwrap();
        terminal.draw(|f| crate::render_app(&app, f)).unwrap();
        let buf = terminal.backend().buffer().clone();
        (0..buf.area.height)
            .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    };
    assert!(screen.contains("Commands") && screen.contains("here"), "{screen}");

    type_text(&mut app, &tx, "network").await;
    assert_eq!(keys(&app)[0], "N");
    press(&mut app, &tx, KeyCode::Enter).await;
    assert!(!app.command_picker.visible);

    // The row's key arrives as a key event, as if pressed.
    let Ok(Event::Key(k)) = rx.try_recv() else { panic!("no replayed key") };
    assert_eq!(k.code, KeyCode::Char('N'));
    app.handle_event(Event::Key(k), &tx).await.unwrap();
    assert!(app.access_in_pane.is_some(), "N opened the access lens");
}

#[tokio::test]
async fn colon_is_text_in_the_search_bar_and_esc_closes_the_picker() {
    let (mut app, tx, _rx) = test_app().await;
    select_mock(&mut app, ServiceType::EC2, instance());
    press(&mut app, &tx, KeyCode::Char('/')).await;
    press(&mut app, &tx, KeyCode::Char(':')).await;
    assert!(!app.command_picker.visible);
    assert!(app.search_query.ends_with(':'), "{:?}", app.search_query);
    press(&mut app, &tx, KeyCode::Esc).await;

    press(&mut app, &tx, KeyCode::Char(':')).await;
    type_text(&mut app, &tx, "q").await;
    assert!(app.running, "q is filter text");
    press(&mut app, &tx, KeyCode::Esc).await;
    assert!(!app.command_picker.visible);

    // From the detail pane, detail rows join and list rows leave.
    app.details_focused = true;
    press(&mut app, &tx, KeyCode::Char(':')).await;
    let listed = keys(&app);
    assert!(listed.contains(&"Z") && !listed.contains(&"z"), "{listed:?}");
}
