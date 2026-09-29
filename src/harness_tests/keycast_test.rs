//! Keycast (`--show-keys`) through the real key path: `handle_event`, then
//! `keycast_tick`, the order the main loop uses — and a rendered frame, so
//! the box is proven to draw, not just to hold entries.

use super::*;

fn two_accounts(app: &mut App) {
    use crate::aws::services::organizations::OrgAccount;
    let mk = |id: &str, name: &str| {
        Box::new(OrgAccount::from_sdk(
            &aws_sdk_organizations::types::Account::builder()
                .id(id)
                .arn(format!("arn:aws:organizations::111111111111:account/o-mock/{id}"))
                .name(name)
                .email(format!("{name}@example.com"))
                .build(),
        )) as Box<dyn Resource>
    };
    app.current_service = Some(ServiceType::Organizations);
    app.org_view = crate::app::OrgView::Accounts;
    app.loading = false;
    app.search_active = false;
    app.search_query.clear();
    app.resources = vec![mk("111111111111", "root"), mk("222222222222", "prod")];
    app.filtered_resources = vec![0, 1];
    app.selected_index = Some(0);
    app.details_focused = false;
}

async fn press(app: &mut App, tx: &mpsc::UnboundedSender<Event>, code: KeyCode) {
    app.handle_event(Event::Key(key(code)), tx).await.unwrap();
    app.keycast_tick();
}

fn shown(app: &App) -> Vec<(String, Option<String>)> {
    app.keycast
        .as_ref()
        .unwrap()
        .entries
        .iter()
        .map(|e| (e.key.clone(), e.action.clone()))
        .collect()
}

fn screen(app: &App) -> String {
    let mut terminal = Terminal::new(TestBackend::new(140, 32)).unwrap();
    terminal.draw(|f| crate::render_app(app, f)).unwrap();
    let buf = terminal.backend().buffer().clone();
    (0..buf.area.height)
        .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn off_by_default_and_draws_nothing() {
    let (mut app, tx, _rx) = test_app().await;
    two_accounts(&mut app);
    assert!(app.keycast.is_none());
    press(&mut app, &tx, KeyCode::Enter).await;
    assert!(!screen(&app).contains(" keys "));
}

#[tokio::test]
async fn labels_keys_by_what_they_did_and_draws_the_box() {
    let (mut app, tx, _rx) = test_app().await;
    two_accounts(&mut app);
    app.keycast = Some(Default::default());

    press(&mut app, &tx, KeyCode::Enter).await;
    press(&mut app, &tx, KeyCode::Char('W')).await;
    assert_eq!(
        shown(&app),
        vec![
            ("⏎".to_string(), Some("open".to_string())),
            ("W".to_string(), Some("change timeline".to_string())),
        ]
    );
    let s = screen(&app);
    assert!(s.contains(" keys "), "the box's title");
    assert!(s.contains("W  change timeline") || s.contains(" W  change timeline"), "{s}");
}

#[tokio::test]
async fn typed_search_text_never_shows() {
    let (mut app, tx, _rx) = test_app().await;
    two_accounts(&mut app);
    app.keycast = Some(Default::default());

    press(&mut app, &tx, KeyCode::Char('/')).await;
    for c in "prod".chars() {
        press(&mut app, &tx, KeyCode::Char(c)).await;
    }
    press(&mut app, &tx, KeyCode::Enter).await;
    assert_eq!(
        shown(&app),
        vec![
            ("/".to_string(), Some("search".to_string())),
            ("⏎".to_string(), Some("search".to_string())),
        ],
        "the query's letters are not keys to show"
    );
}
