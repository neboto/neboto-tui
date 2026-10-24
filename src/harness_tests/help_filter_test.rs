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

/// The lines of help's "Here" section: the rows between its title and the
/// next blank line, left column only.
fn here_rows(app: &App) -> Vec<String> {
    let screen = screen_of(app);
    let lines: Vec<&str> = screen.lines().collect();
    let Some(start) = lines.iter().position(|l| l.contains("  Here ")) else {
        return Vec::new();
    };
    let byte = lines[start].find("Here").unwrap();
    let col = lines[start][..byte].chars().count();
    lines[start + 1..]
        .iter()
        .map(|l| l.chars().skip(col).take(38).collect::<String>().trim().to_string())
        .take_while(|l| !l.is_empty())
        .collect()
}

#[tokio::test]
async fn help_leads_with_what_works_on_the_selection() {
    use crate::aws::services::{config::SecretEntry, ec2::Ec2Instance};
    let (mut app, tx, _rx) = test_app().await;

    let instance = Ec2Instance::from_sdk(
        &aws_sdk_ec2::types::Instance::builder()
            .instance_id("i-0abc")
            .security_groups(aws_sdk_ec2::types::GroupIdentifier::builder().group_id("sg-0abc").build())
            .build(),
    );
    select_mock(&mut app, ServiceType::EC2, Box::new(instance));
    press(&mut app, &tx, KeyCode::Char('?')).await;
    let here = here_rows(&app);
    for key in ["m ", "W ", "N ", "s "] {
        assert!(here.iter().any(|r| r.starts_with(key)), "{key:?} missing from {here:?}");
    }
    assert!(!here.iter().any(|r| r.starts_with("x ")), "{here:?}");

    // Filtering drops "Here": its rows are listed in their own sections.
    for c in "session".chars() {
        press(&mut app, &tx, KeyCode::Char(c)).await;
    }
    assert!(here_rows(&app).is_empty());
    press(&mut app, &tx, KeyCode::Esc).await;
    press(&mut app, &tx, KeyCode::Esc).await;

    let secret = SecretEntry::from_sdk(
        &aws_sdk_secretsmanager::types::SecretListEntry::builder()
            .name("db-password")
            .arn("arn:aws:secretsmanager:us-east-1:123456789012:secret:db-password-abc123")
            .build(),
    );
    select_mock(&mut app, ServiceType::Secrets, Box::new(secret));
    press(&mut app, &tx, KeyCode::Char('?')).await;
    let here = here_rows(&app);
    assert!(here.iter().any(|r| r.starts_with("x / Y")), "{here:?}");
    assert!(!here.iter().any(|r| r.starts_with("s ") || r.starts_with("N ")), "{here:?}");
}

#[tokio::test]
async fn help_calls_out_the_command_picker_and_colon_opens_it() {
    let (mut app, tx, _rx) = test_app().await;
    press(&mut app, &tx, KeyCode::Char('?')).await;
    let screen = screen_of(&app);
    assert!(screen.contains(" :  run any key by name "), "{screen}");
    assert!(screen.contains(": run a command"), "footer: {screen}");

    // Mid-filter `:` is text; on an empty filter it opens the picker.
    press(&mut app, &tx, KeyCode::Char('a')).await;
    press(&mut app, &tx, KeyCode::Char(':')).await;
    assert_eq!(app.help_query, "a:");
    press(&mut app, &tx, KeyCode::Esc).await;
    press(&mut app, &tx, KeyCode::Char(':')).await;
    assert!(!app.help_visible && app.command_picker.visible);
}
