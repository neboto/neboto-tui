//! #245: change markers on the `m` charts — `m` seeds the `W` timeline's
//! sources into the markers' own state, `c` toggles them, and the two
//! timeline consumers never coexist.

use super::*;
use crate::timeline::TimelineSource;

fn ecs_service() -> (ServiceType, Box<dyn Resource>) {
    all_mocks()
        .into_iter()
        .find(|(_, l, _)| *l == "EcsServiceInfo")
        .map(|(s, _, r)| (s, r))
        .expect("an EcsServiceInfo mock")
}

async fn press(app: &mut App, tx: &mpsc::UnboundedSender<Event>, c: char) {
    app.handle_event(Event::Key(crossterm::event::KeyEvent::from(KeyCode::Char(c))), tx).await.unwrap();
}

#[tokio::test]
async fn m_seeds_markers_and_c_toggles_them() {
    let (mut app, tx, _rx) = test_app().await;
    let (svc, r) = ecs_service();
    select_mock(&mut app, svc, r);

    press(&mut app, &tx, 'm').await;
    assert_eq!(app.metrics_in_pane, Some(MetricsKind::Spec));
    assert!(app.metrics_window_end > 0, "the fetch window's end is recorded on open");
    let tl = app.chart_timeline.as_ref().expect("m seeds the markers' timeline");
    assert!(tl.pending.contains(&TimelineSource::Trail), "{:?}", tl.pending);
    assert!(app.trail_in_pane.is_none());

    // Hiding keeps the rows; showing again doesn't refetch them.
    press(&mut app, &tx, 'c').await;
    assert!(!app.chart_markers);
    assert!(app.chart_timeline.is_some());
    let generation = app.trail_generation;
    press(&mut app, &tx, 'c').await;
    assert!(app.chart_markers);
    assert_eq!(app.trail_generation, generation, "no second fetch");

    // Closing drops them; with markers off, `m` fetches nothing.
    press(&mut app, &tx, 'm').await;
    assert!(app.metrics_in_pane.is_none() && app.chart_timeline.is_none());
    app.chart_markers = false;
    press(&mut app, &tx, 'm').await;
    assert!(app.chart_timeline.is_none(), "markers off: no timeline fetch");
}

#[tokio::test]
async fn the_w_lens_and_the_markers_never_hold_timeline_state_together() {
    let (mut app, tx, _rx) = test_app().await;
    let (svc, r) = ecs_service();
    select_mock(&mut app, svc, r);
    press(&mut app, &tx, 'm').await;
    assert!(app.chart_timeline.is_some());
    press(&mut app, &tx, 'm').await;

    press(&mut app, &tx, 'W').await;
    assert!(app.trail_in_pane.is_some());
    assert!(app.chart_timeline.is_none(), "opening W clears the markers' state");
}

