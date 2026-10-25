//! The spec-driven metrics view (`aws::metrics`): `m` on a Lambda function
//! charts its curated set from one `GetMetricData` call against the demo
//! account, one row per metric, and the selected metric's group shares a
//! chart; the range, stat, picker and mouse all stay inside the view.

use super::*;
use crate::app::MetricsKind;

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

/// Handle events until the view has its series (or its picker its metrics).
async fn settle(app: &mut App, tx: &mpsc::UnboundedSender<Event>, rx: &mut mpsc::UnboundedReceiver<Event>) {
    let busy = |app: &App| {
        app.spec_metrics.as_ref().is_some_and(|m| m.loading || m.picker.as_ref().is_some_and(|p| p.loading))
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while busy(app) {
        assert!(std::time::Instant::now() < deadline, "metrics never loaded");
        if let Ok(Some(ev)) = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv()).await {
            app.handle_event(ev, tx).await.unwrap();
        }
    }
}

async fn press(app: &mut App, tx: &mpsc::UnboundedSender<Event>, code: KeyCode) {
    app.handle_event(Event::Key(key(code)), tx).await.unwrap();
}

#[tokio::test]
async fn m_on_a_function_charts_its_curated_set() {
    let mut app = App::from_parts(crate::config::Config::default(), AwsClients::new_demo_for_test().await).unwrap();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let svc = app.services[&ServiceType::Lambda].clone();
    let (rows, _) = crate::headless::list_all(svc, ServiceType::Lambda).await.ok().unwrap();
    let orders = rows.into_iter().find(|r| r.name() == "orders-api").expect("the demo's orders-api");
    select_mock(&mut app, ServiceType::Lambda, orders);

    press(&mut app, &tx, KeyCode::Char('m')).await;
    assert_eq!(app.metrics_in_pane, Some(MetricsKind::Spec));
    assert!(app.details_focused);
    settle(&mut app, &tx, &mut rx).await;

    let st = app.spec_metrics.as_ref().unwrap();
    let labels: Vec<&str> = st.series.iter().map(|s| s.query.label.as_str()).collect();
    assert_eq!(labels, ["Invocations", "Errors", "Throttles", "Duration", "Concurrency"]);
    assert!(st.series.iter().all(|s| s.error.is_none() && s.points.len() == 60), "an hour at 1m");
    let errors = &st.series[1].points;
    assert!(errors[50..].iter().map(|p| p.1).sum::<f64>() > 0.0, "the demo's error climb");

    let screen = screen_of(&app);
    assert!(screen.contains("Metrics — Lambda function · orders-api"), "{screen}");
    assert!(screen.contains(" 1 1h ") && screen.contains("default set · AWS/Lambda"), "{screen}");
    assert!(screen.contains("▸ Invocations") && screen.contains("(sum)"), "{screen}");
    assert!(screen.contains("Duration") && screen.contains(" ms"), "{screen}");
    // Invocations shares its chart with errors and throttles: a legend.
    assert!(screen.contains("Invocations · Invocations (sum)"), "{screen}");

    // `j` to Duration (no group): the chart is its own, in ms.
    for _ in 0..3 {
        press(&mut app, &tx, KeyCode::Char('j')).await;
    }
    assert!(screen_of(&app).contains("Duration · Duration (avg)"));

    // `s` cycles that metric's stat and refetches; `3` is 24h at 1h.
    press(&mut app, &tx, KeyCode::Char('s')).await;
    press(&mut app, &tx, KeyCode::Char('3')).await;
    settle(&mut app, &tx, &mut rx).await;
    let st = app.spec_metrics.as_ref().unwrap();
    assert_eq!(st.queries[3].stat, crate::aws::metrics::Stat::Maximum);
    assert_eq!(st.series[3].points.len(), 24);
    assert_eq!(app.spec_metrics_range, crate::aws::services::ec2::MetricsTimeRange::TwentyFourHours);

    // `a` opens the picker (the demo has no ListMetrics answer), Esc steps
    // back out of it, and the view is still there.
    press(&mut app, &tx, KeyCode::Char('a')).await;
    settle(&mut app, &tx, &mut rx).await;
    assert!(screen_of(&app).contains("CloudWatch has no metrics with this resource's dimensions"));
    press(&mut app, &tx, KeyCode::Esc).await;
    assert!(app.spec_metrics.as_ref().unwrap().picker.is_none());
    assert_eq!(app.metrics_in_pane, Some(MetricsKind::Spec));

    // A click on a row moves the cursor there.
    screen_of(&app);
    let (idx, rect) = app.popup_hits.borrow().rows[1];
    let click = crossterm::event::MouseEvent {
        kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
        column: rect.x + 3,
        row: rect.y,
        modifiers: KeyModifiers::NONE,
    };
    app.handle_event(Event::Mouse(click), &tx).await.unwrap();
    assert_eq!(app.spec_metrics.as_ref().unwrap().selected, idx);

    press(&mut app, &tx, KeyCode::Esc).await;
    assert!(app.metrics_in_pane.is_none() && app.spec_metrics.is_none());
}

#[tokio::test]
async fn a_late_answer_for_a_superseded_fetch_is_dropped() {
    let mut app = App::from_parts(crate::config::Config::default(), AwsClients::new_demo_for_test().await).unwrap();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let svc = app.services[&ServiceType::Lambda].clone();
    let (rows, _) = crate::headless::list_all(svc, ServiceType::Lambda).await.ok().unwrap();
    select_mock(&mut app, ServiceType::Lambda, rows.into_iter().next().unwrap());
    press(&mut app, &tx, KeyCode::Char('m')).await;
    // A range change before the first answer lands: only the 6h one counts.
    press(&mut app, &tx, KeyCode::Char('2')).await;
    settle(&mut app, &tx, &mut rx).await;
    let st = app.spec_metrics.as_ref().unwrap();
    assert!(st.series.iter().all(|s| s.points.len() == 72), "6h at 5m, never the 1h answer");
}
