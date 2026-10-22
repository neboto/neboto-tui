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

    // IAM changes come from us-east-1's trail by event source; the read
    // mixed into that answer never lists.
    let tx0 = mpsc::unbounded_channel().0;
    app.handle_event(Event::Key(crossterm::event::KeyEvent::from(KeyCode::Char('2'))), &tx0).await.unwrap();
    assert_eq!(app.changes_view, crate::app::ChangesView::Iam);
    let names: Vec<&str> = app.filtered_resources.iter().map(|&i| app.resources[i].name()).collect();
    assert_eq!(names.len(), 2, "{names:?}");
    assert!(names[0].contains("PutRolePolicy") && names[1].contains("AttachRolePolicy"), "{names:?}");
    assert!(!names.iter().any(|n| n.contains("GetRole")));

    let key = |c: char| Event::Key(crossterm::event::KeyEvent::from(KeyCode::Char(c)));
    let names_of = |app: &App| -> Vec<String> {
        app.filtered_resources.iter().map(|&i| app.resources[i].name().to_string()).collect()
    };

    // Config: the task definition and the service update; Config's own
    // compliance bookkeeping never lists.
    app.handle_event(key('4'), &tx0).await.unwrap();
    assert_eq!(app.changes_view, crate::app::ChangesView::Config);
    let names = names_of(&app);
    assert_eq!(names.len(), 2, "{names:?}");
    assert!(names[0].contains("Service configuration changed  orders-worker"), "{names:?}");

    // Deploys: the CI image build, started by the deploy role. The
    // month-old build #87 is outside the window.
    app.handle_event(key('5'), &tx0).await.unwrap();
    assert_eq!(app.changes_view, crate::app::ChangesView::Deploys);
    let names = names_of(&app);
    assert_eq!(names.len(), 1, "{names:?}");
    assert!(names[0].contains("build SUCCEEDED  orders-worker-image #88"), "{names:?}");
    let build = &app.resources[app.filtered_resources[0]];
    assert!(build.details().iter().any(|(k, v)| k == "Resource"
        && v.ends_with(":project/orders-worker-image")), "jumps to the project");

    // Scaling & RDS: only the orders-db parameter change in the last hour.
    app.handle_event(key('6'), &tx0).await.unwrap();
    let names = names_of(&app);
    assert_eq!(names.len(), 1, "{names:?}");
    assert!(names[0].contains("max_connections") && names[0].ends_with("orders-db"), "{names:?}");

    // `]` widens the window: a new cache variant.
    let (tx, _rx) = mpsc::unbounded_channel();
    app.handle_event(Event::Key(crossterm::event::KeyEvent::from(KeyCode::Char(']'))), &tx).await.unwrap();
    assert_eq!(app.changes_window.label(), "6h");
}

#[tokio::test]
async fn demo_changes_six_hour_window_reaches_the_asg_replacement() {
    use crate::aws::services::changes::{ChangesService, ChangesWindow};
    let clients = AwsClients::new_demo_for_test().await;
    let svc: std::sync::Arc<dyn crate::aws::service::AwsService> =
        std::sync::Arc::new(ChangesService::new(&clients, ChangesWindow::SixHours));
    let (rows, _) = crate::headless::list_all(svc, ServiceType::Changes).await.ok().unwrap();
    let scaling: Vec<&str> =
        rows.iter().filter(|r| r.resource_type() == "Scaling Activity").map(|r| r.name()).collect();
    assert_eq!(scaling.len(), 2, "{scaling:?}");
    assert!(scaling.iter().any(|n| n.contains("Terminating EC2 instance: i-0p0rta1std0000b2")), "{scaling:?}");
}
