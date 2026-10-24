//! The status bar's key hints: `:` / `?` / `q` are pinned and never
//! trimmed; the rest yield from the end on a narrow bar.

use super::*;
use crate::aws::services::ec2::Ec2Instance;

fn bar(app: &App, width: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, 30)).unwrap();
    terminal.draw(|f| crate::render_app(app, f)).unwrap();
    let buf = terminal.backend().buffer().clone();
    let y = buf.area.height - 1;
    (0..width).map(|x| buf[(x, y)].symbol()).collect()
}

async fn instance_app() -> App {
    let (mut app, _tx, _rx) = test_app().await;
    let instance = Ec2Instance::from_sdk(
        &aws_sdk_ec2::types::Instance::builder()
            .instance_id("i-0abc")
            .security_groups(aws_sdk_ec2::types::GroupIdentifier::builder().group_id("sg-0abc").build())
            .build(),
    );
    select_mock(&mut app, ServiceType::EC2, Box::new(instance));
    app
}

#[tokio::test]
async fn help_and_commands_survive_a_narrow_bar() {
    let mut app = instance_app().await;
    for width in [100, 140, 220] {
        let line = bar(&app, width);
        println!("list {width}: {line}");
        assert!(line.contains(": commands  ·  ? help  ·  q quit"), "{width}: {line}");
        assert!(line.contains("/ search  ·  ⏎ open"), "{width}: {line}");
        assert!(!line.contains("j/k") && !line.contains("H/L") && !line.contains("bookmarks"), "{line}");
    }
    // The selection's own keys come before the every-row ones: at 100
    // columns `m metrics` shows and `y copy id` gives way.
    let narrow = bar(&app, 100);
    assert!(narrow.contains("⏎ open  ·  m metrics") && !narrow.contains("copy id"), "{narrow}");
    let wide = bar(&app, 220);
    assert!(wide.contains("N access") && wide.contains("y copy id"), "{wide}");

    app.details_focused = true;
    for width in [100, 140] {
        let line = bar(&app, width);
        println!("detail {width}: {line}");
        assert!(line.contains(": commands  ·  ? help"), "{width}: {line}");
        assert!(line.trim_start().starts_with("m metrics"), "{width}: {line}");
        assert!(!line.contains("q quit") && !line.contains("j/k"), "{line}");
    }
}

#[tokio::test]
async fn clicking_the_commands_hint_opens_the_picker() {
    let mut app = instance_app().await;
    let (tx, _rx) = mpsc::unbounded_channel();
    let line = bar(&app, 140);
    let col = line.find(": commands").map(|b| line[..b].chars().count()).unwrap() as u16;
    let click = |kind| crossterm::event::MouseEvent {
        kind,
        column: col,
        row: 29,
        modifiers: KeyModifiers::NONE,
    };
    use crossterm::event::{MouseButton, MouseEventKind};
    app.handle_event(Event::Mouse(click(MouseEventKind::Down(MouseButton::Left))), &tx).await.unwrap();
    app.handle_event(Event::Mouse(click(MouseEventKind::Up(MouseButton::Left))), &tx).await.unwrap();
    assert!(app.command_picker.visible);
}
