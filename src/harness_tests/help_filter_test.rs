//! `?` filters as you type: the table narrows to one full-width column,
//! letters are text (so `q` doesn't quit), `Esc` clears, then closes.

use super::*;

fn screen_of(app: &App) -> String {
    let backend = TestBackend::new(120, 40);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| crate::render_app(app, f)).unwrap();
    let buf = terminal.backend().buffer().clone();
    (0..buf.area.height)
        .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

async fn press(app: &mut App, tx: &mpsc::UnboundedSender<Event>, code: KeyCode) {
    app.handle_event(Event::Key(crossterm::event::KeyEvent::from(code)), tx).await.unwrap();
}

#[tokio::test]
async fn help_filters_as_you_type() {
    let (mut app, tx, _rx) = test_app().await;
    press(&mut app, &tx, KeyCode::Char('?')).await;
    assert!(app.help_visible);
    let screen = screen_of(&app);
    assert!(screen.contains("type to filter keys") && screen.contains("toggle banner"), "{screen}");

    for c in "network".chars() {
        press(&mut app, &tx, KeyCode::Char(c)).await;
    }
    assert!(app.running, "letters are filter text");
    let screen = screen_of(&app);
    // The full description fits: the two-column table cuts this one off.
    assert!(screen.contains("Network-access lens (merged security-group rules + subnet NACLs)"), "{screen}");
    assert!(!screen.contains("toggle banner"), "{screen}");

    press(&mut app, &tx, KeyCode::Char('q')).await;
    assert!(app.running && app.help_visible, "q is text while help is open");
    assert!(screen_of(&app).contains("No keys match \"networkq\""));
    press(&mut app, &tx, KeyCode::Backspace).await;
    assert_eq!(app.help_query, "network");

    // `?` is text mid-filter; Esc clears, then closes.
    press(&mut app, &tx, KeyCode::Char('?')).await;
    assert!(app.help_visible);
    press(&mut app, &tx, KeyCode::Esc).await;
    assert!(app.help_visible && app.help_query.is_empty());
    press(&mut app, &tx, KeyCode::Esc).await;
    assert!(!app.help_visible);

    // Reopening starts clean; `?` on an empty filter closes.
    press(&mut app, &tx, KeyCode::Char('?')).await;
    assert!(app.help_visible && app.help_query.is_empty());
    press(&mut app, &tx, KeyCode::Char('?')).await;
    assert!(!app.help_visible);
}
