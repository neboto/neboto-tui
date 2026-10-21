//! #241: `@changes` lists the demo account's recent write events newest
//! first, under source tabs, with the window chips on the tab row.

use super::*;

fn screen_of(app: &App) -> String {
    let backend = TestBackend::new(170, 45);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| crate::render_app(app, f)).unwrap();
    let buf = terminal.backend().buffer().clone();
    (0..buf.area.height)
        .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn demo_changes_list_newest_first_by_source_tab() {
    let mut app = App::from_parts(crate::config::Config::default(), AwsClients::new_demo_for_test().await).unwrap();
    let svc = app.services[&ServiceType::Changes].clone();
    let (rows, _) = crate::headless::list_all(svc, ServiceType::Changes).await.ok().unwrap();
    assert!(!rows.is_empty(), "the demo trail has write events in the last hour");
    app.current_service = Some(ServiceType::Changes);
    app.loading = false;
    // Reverse the load order: the default sort must restore newest first.
    app.resources = rows.into_iter().rev().collect();
    app.update_search();

    let times: Vec<i64> = app
        .filtered_resources
        .iter()
        .map(|&i| app.resources[i].as_any().downcast_ref::<crate::aws::services::changes::ChangeEvent>().unwrap().ts_secs)
        .collect();
    assert!(times.windows(2).all(|w| w[0] >= w[1]), "newest first: {times:?}");

    let screen = screen_of(&app);
    assert!(screen.contains("CloudTrail") && screen.contains("Deploys"), "{screen}");
    assert!(screen.contains("[ ] Window") && screen.contains(" 1h "), "{screen}");
    assert!(screen.contains("UpdateService  orders-worker"), "{screen}");

    // The Deploys tab has no demo stacks/pipelines in the window.
    app.handle_event(Event::Key(crossterm::event::KeyEvent::from(KeyCode::Char('4'))), &mpsc::unbounded_channel().0)
        .await
        .unwrap();
    assert_eq!(app.changes_view, crate::app::ChangesView::Deploys);
    assert!(app.filtered_resources.is_empty());

    // `]` widens the window: a new cache variant.
    let (tx, _rx) = mpsc::unbounded_channel();
    app.handle_event(Event::Key(crossterm::event::KeyEvent::from(KeyCode::Char(']'))), &tx).await.unwrap();
    assert_eq!(app.changes_window.label(), "6h");
}
