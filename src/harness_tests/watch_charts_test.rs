//! Watch mode refreshes the charts (`m`) instead of pausing under them —
//! never faster than once a minute, and a dashboard keeps its grid while a
//! watch pass refreshes it.

use super::*;
use crate::app::MetricsKind;
use crate::aws::services::cloudwatch::{CwDashboardMetricsData, CwDashboardMetricsState};
use crate::aws::services::ec2::MetricsTimeRange;
use std::time::Duration;

fn mock(service: ServiceType, label: &str) -> Box<dyn crate::aws::resource::Resource> {
    all_mocks()
        .into_iter()
        .find(|(s, l, _)| *s == service && *l == label)
        .map(|(_, _, r)| r)
        .unwrap()
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64
}

/// EC2 charts open, the first fetch settled, watch on at 10s.
async fn ec2_charts() -> (App, mpsc::UnboundedSender<Event>, mpsc::UnboundedReceiver<Event>) {
    let (mut app, tx, rx) = test_app().await;
    select_mock(&mut app, ServiceType::EC2, mock(ServiceType::EC2, "Ec2Instance"));
    app.update_search();
    app.handle_key(key(KeyCode::Char('m')), &tx).await.unwrap();
    assert_eq!(app.metrics_in_pane, Some(MetricsKind::Spec));
    app.spec_metrics.as_mut().unwrap().loading = false;
    app.watch.enabled = true;
    app.watch.interval = Duration::from_secs(10);
    (app, tx, rx)
}

#[tokio::test]
async fn watch_refetches_the_charts_once_a_minute_not_every_interval() {
    let (mut app, tx, _rx) = ec2_charts().await;
    // 30s old: past the 10s interval, inside the one-minute floor.
    app.metrics_window_end = now() - 30;
    app.watch_tick(&tx);
    assert!(!app.spec_metrics.as_ref().unwrap().loading);
    assert!(!app.watch_refresh_active(), "the list behind the charts isn't refreshed");

    app.metrics_window_end = now() - 61;
    app.watch_tick(&tx);
    assert!(app.spec_metrics.as_ref().unwrap().loading);
    assert!(now() - app.metrics_window_end <= 1, "the window moves to now");
    assert_eq!(app.watch_cadence(), Duration::from_secs(60));
    assert!(app.watch_spinning());
}

#[tokio::test]
async fn watch_keeps_its_range_and_holds_off_under_a_modal_or_the_picker() {
    let (mut app, tx, _rx) = ec2_charts().await;
    app.handle_key(key(KeyCode::Char('3')), &tx).await.unwrap();
    app.spec_metrics.as_mut().unwrap().loading = false;
    app.metrics_window_end = now() - 61;

    app.help_visible = true;
    app.watch_tick(&tx);
    assert!(!app.spec_metrics.as_ref().unwrap().loading, "not under help");
    app.help_visible = false;

    app.watch.interval = Duration::from_secs(120);
    app.watch_tick(&tx);
    assert!(!app.spec_metrics.as_ref().unwrap().loading, "a longer interval than the floor is kept");
    app.metrics_window_end = now() - 121;
    app.watch_tick(&tx);
    let st = app.spec_metrics.as_ref().unwrap();
    assert!(st.loading);
    assert_eq!(st.range, MetricsTimeRange::TwentyFourHours);
}

#[tokio::test]
async fn a_watch_pass_keeps_the_dashboard_drawn_even_when_it_fails() {
    let (mut app, tx, _rx) = test_app().await;
    select_mock(&mut app, ServiceType::CloudWatch, mock(ServiceType::CloudWatch, "CwDashboard"));
    app.cw_view = crate::app::CwView::Dashboards;
    app.update_search();
    app.handle_key(key(KeyCode::Char('m')), &tx).await.unwrap();
    assert_eq!(app.metrics_in_pane, Some(MetricsKind::CwDashboard));
    let name = app.get_selected_resource().unwrap().name().to_string();
    let data = CwDashboardMetricsData {
        time_range: MetricsTimeRange::SixHours,
        x_max: 0.0,
        widgets: Vec::new(),
        series: Default::default(),
        alarms: Default::default(),
        skipped_regions: Vec::new(),
        math_error: None,
        capped: false,
    };
    app.cw_dashboard_metrics.insert(name.clone(), CwDashboardMetricsState::Loaded(Box::new(data)));
    app.watch.enabled = true;
    app.metrics_window_end = now() - 61;

    app.watch_tick(&tx);
    assert_eq!(app.cw_dashboard_watch_pending.as_deref(), Some(name.as_str()));
    assert!(matches!(app.cw_dashboard_metrics.get(&name), Some(CwDashboardMetricsState::Loaded(_))));
    // In flight: the next tick doesn't stack another.
    app.metrics_window_end = now() - 61;
    app.watch_tick(&tx);

    app.handle_event(Event::CwDashboardMetricsFailed { name: name.clone(), error: "Throttling".into() }, &tx)
        .await
        .unwrap();
    assert!(matches!(app.cw_dashboard_metrics.get(&name), Some(CwDashboardMetricsState::Loaded(_))));
    assert!(app.error_message.as_deref().is_some_and(|m| m.contains("Throttling")), "{:?}", app.error_message);
    assert!(app.cw_dashboard_watch_pending.is_none());
}

#[tokio::test]
async fn cost_and_s3_charts_have_nothing_to_refresh() {
    let (mut app, _tx, _rx) = test_app().await;
    app.watch.enabled = true;
    app.metrics_in_pane = Some(MetricsKind::Cost);
    assert_eq!(app.chart_watch_interval(), None);
    assert_eq!(app.watch_cadence(), app.watch.interval);
}
