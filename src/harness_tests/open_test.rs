//! `neboto open` (#331): start at a resource — on whichever sub-tab it
//! sits, at a section by any-case name — and say so when it isn't there.

use super::*;
use crate::cli::OpenArgs;

/// EC2 with an instance and a security group loaded, on the Instances tab.
async fn ec2_app() -> (App, mpsc::UnboundedSender<Event>, mpsc::UnboundedReceiver<Event>) {
    let (mut app, tx, rx) = test_app().await;
    let pick = |label: &str| {
        all_mocks()
            .into_iter()
            .find(|(s, l, _)| *s == ServiceType::EC2 && *l == label)
            .map(|(_, _, r)| r)
            .unwrap()
    };
    select_mock(&mut app, ServiceType::EC2, pick("Ec2Instance"));
    app.resources.push(pick("SecurityGroup"));
    app.update_search();
    app.details_focused = false;
    (app, tx, rx)
}

fn sg_id(app: &App) -> String {
    app.resources.iter().find(|r| r.resource_type() == "Security Group").unwrap().id().to_string()
}

/// Run `neboto open <target…>` against the loaded app, as `main` would.
async fn open(app: &mut App, tx: &mpsc::UnboundedSender<Event>, target: &[&str], f: impl FnOnce(&mut OpenArgs)) {
    let mut args = OpenArgs {
        target: target.iter().map(|s| s.to_string()).collect(),
        section: None,
        charts: None,
        metric: None,
        only: false,
        full: false,
    };
    f(&mut args);
    let (loc, req) = crate::open::plan(&args).unwrap();
    app.startup_location = Some(loc);
    app.pending_open = Some(req);
    app.restore_startup_location(tx);
}

#[tokio::test]
async fn a_resource_on_another_sub_tab_opens_there_focused() {
    let (mut app, tx, _rx) = ec2_app().await;
    let id = sg_id(&app);
    open(&mut app, &tx, &["ec2", &id], |_| {}).await;
    assert_eq!(app.ec2_view, crate::app::Ec2View::SecurityGroups);
    assert_eq!(app.get_selected_resource().map(|r| r.id().to_string()), Some(id));
    assert!(app.details_focused);
    assert!(app.error_message.is_none(), "{:?}", app.error_message);
}

#[tokio::test]
async fn a_section_opens_by_any_case_name() {
    let (mut app, tx, _rx) = ec2_app().await;
    let id = sg_id(&app);
    open(&mut app, &tx, &["ec2", &id], |a| a.section = Some("INBOUND".into())).await;
    let labels = app.headless_section_labels();
    assert_eq!(labels.get(app.detail_section_idx).map(String::as_str), Some("Inbound"));
}

#[tokio::test]
async fn an_unknown_section_lists_the_real_ones() {
    let (mut app, tx, _rx) = ec2_app().await;
    let id = sg_id(&app);
    open(&mut app, &tx, &["ec2", &id], |a| a.section = Some("nope".into())).await;
    let msg = app.error_message.clone().unwrap_or_default();
    assert!(msg.contains("no section \"nope\"") && msg.contains("Inbound"), "{msg}");
    assert_eq!(app.detail_section_idx, 0);
}

#[tokio::test]
async fn charts_on_a_type_without_them_say_so() {
    let (mut app, tx, _rx) = ec2_app().await;
    let id = sg_id(&app);
    open(&mut app, &tx, &["ec2", &id], |a| a.charts = Some("1h".into())).await;
    let msg = app.error_message.clone().unwrap_or_default();
    assert!(msg.contains("no charts for a Security Group"), "{msg}");
}

#[tokio::test]
async fn full_gives_the_detail_pane_the_width() {
    let (mut app, tx, _rx) = ec2_app().await;
    let id = sg_id(&app);
    open(&mut app, &tx, &["ec2", &id], |a| a.full = true).await;
    assert_eq!(app.layout_mode, crate::app::LayoutMode::DetailsOnly);
}

#[tokio::test]
async fn a_missing_resource_is_searched_for_never_row_one() {
    let (mut app, tx, _rx) = ec2_app().await;
    open(&mut app, &tx, &["ec2", "i-not-here"], |_| {}).await;
    assert_eq!(app.search_query, "i-not-here");
    assert!(!app.details_focused);
    let msg = app.error_message.clone().unwrap_or_default();
    assert!(msg.contains("no EC2 resource \"i-not-here\""), "{msg}");
}

#[tokio::test]
async fn a_message_raised_mid_load_survives_the_load_completing() {
    let (mut app, tx, _rx) = ec2_app().await;
    let id = sg_id(&app);
    app.loading = true;
    open(&mut app, &tx, &["ec2", &id], |a| a.section = Some("nope".into())).await;
    app.handle_event(Event::ResourcesFullyLoaded { service: ServiceType::EC2, total_count: 2 }, &tx).await.unwrap();
    let msg = app.error_message.clone().unwrap_or_default();
    assert!(msg.contains("no section"), "{msg}");
}

/// The demo's `orders-api` Lambda, loaded and on screen (not selected).
async fn demo_lambda() -> (App, mpsc::UnboundedSender<Event>, mpsc::UnboundedReceiver<Event>) {
    let mut app = App::from_parts(crate::config::Config::default(), AwsClients::new_demo_for_test().await).unwrap();
    let (tx, rx) = mpsc::unbounded_channel();
    let svc = app.services[&ServiceType::Lambda].clone();
    let (rows, _) = crate::headless::list_all(svc, ServiceType::Lambda).await.ok().unwrap();
    let mut rows = rows.into_iter();
    select_mock(&mut app, ServiceType::Lambda, rows.next().unwrap());
    app.resources.extend(rows);
    app.update_search();
    (app, tx, rx)
}

/// Handle events until the charts have their series.
async fn settle_charts(app: &mut App, tx: &mpsc::UnboundedSender<Event>, rx: &mut mpsc::UnboundedReceiver<Event>) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while app.spec_metrics.as_ref().is_some_and(|m| m.loading) {
        assert!(std::time::Instant::now() < deadline, "metrics never loaded");
        if let Ok(Some(ev)) = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv()).await {
            app.handle_event(ev, tx).await.unwrap();
        }
    }
}

#[tokio::test]
async fn metric_puts_the_charts_on_that_row_and_a_split_keeps_it() {
    let (mut app, tx, mut rx) = demo_lambda().await;
    open(&mut app, &tx, &["lambda", "orders-api"], |a| a.metric = Some("duration".into())).await;
    assert_eq!(app.metrics_in_pane, Some(crate::app::MetricsKind::Spec));
    settle_charts(&mut app, &tx, &mut rx).await;
    let st = app.spec_metrics.as_ref().unwrap();
    assert_eq!(st.selected_series().map(|s| s.query.label.as_str()), Some("Duration"));
    assert!(app.error_message.is_none(), "{:?}", app.error_message);
    // `|` from here hands the metric on.
    let lens = app.current_location().and_then(|l| l.lens);
    assert!(
        matches!(&lens, Some(crate::app::NavLens::Metrics { metric: Some(m), .. }) if m == "Duration"),
        "{lens:?}"
    );
}

#[tokio::test]
async fn a_metric_the_charts_lack_lists_the_rows() {
    let (mut app, tx, mut rx) = demo_lambda().await;
    open(&mut app, &tx, &["lambda", "orders-api"], |a| a.metric = Some("disk".into())).await;
    settle_charts(&mut app, &tx, &mut rx).await;
    let msg = app.error_message.clone().unwrap_or_default();
    assert!(msg.contains("\"disk\" isn't charted") && msg.contains("Duration"), "{msg}");
    assert_eq!(app.spec_metrics.as_ref().unwrap().selected, 0);
}

#[tokio::test]
async fn only_charts_the_metric_with_the_rows_sharing_its_chart_and_d_brings_the_rest() {
    let (mut app, tx, mut rx) = demo_lambda().await;
    open(&mut app, &tx, &["lambda", "orders-api"], |a| {
        a.metric = Some("errors".into());
        a.only = true;
    })
    .await;
    settle_charts(&mut app, &tx, &mut rx).await;
    let labels = |app: &App| -> Vec<String> {
        app.spec_metrics.as_ref().unwrap().series.iter().map(|s| s.query.label.clone()).collect()
    };
    // Errors is drawn with Invocations and Throttles (group "calls").
    assert_eq!(labels(&app), ["Invocations", "Errors", "Throttles"]);
    let st = app.spec_metrics.as_ref().unwrap();
    assert_eq!(st.selected_series().map(|s| s.query.label.as_str()), Some("Errors"));
    assert!(!st.curated);
    // A split from here comes back the same way.
    let lens = app.current_location().and_then(|l| l.lens);
    assert!(
        matches!(&lens, Some(crate::app::NavLens::Metrics { metric: Some(m), only: true, .. }) if m == "Errors"),
        "{lens:?}"
    );
    // `d`: the whole set again.
    app.handle_event(Event::Key(key(KeyCode::Char('d'))), &tx).await.unwrap();
    settle_charts(&mut app, &tx, &mut rx).await;
    assert_eq!(labels(&app).len(), 5);
}

#[tokio::test]
async fn only_on_an_ungrouped_metric_is_that_one_row() {
    let (mut app, tx, mut rx) = demo_lambda().await;
    open(&mut app, &tx, &["lambda", "orders-api"], |a| {
        a.metric = Some("Duration".into());
        a.only = true;
    })
    .await;
    settle_charts(&mut app, &tx, &mut rx).await;
    let st = app.spec_metrics.as_ref().unwrap();
    assert_eq!(st.series.iter().map(|s| s.query.label.as_str()).collect::<Vec<_>>(), ["Duration"]);
}

/// A computed row keeps the hidden inputs it's charted from, so a
/// refetch (a range step) still draws it.
#[tokio::test]
async fn only_on_a_computed_row_keeps_its_hidden_inputs() {
    let mut app = App::from_parts(crate::config::Config::default(), AwsClients::new_demo_for_test().await).unwrap();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let vol = all_mocks().into_iter().find(|(_, l, _)| *l == "FsxVolume").unwrap().2;
    let id = vol.id().to_string();
    select_mock(&mut app, ServiceType::Fsx, vol);
    open(&mut app, &tx, &["fsx", &id], |a| {
        a.metric = Some("read latency".into());
        a.only = true;
    })
    .await;
    settle_charts(&mut app, &tx, &mut rx).await;
    let st = app.spec_metrics.as_ref().unwrap();
    assert_eq!(st.selected_series().map(|s| s.query.label.as_str()), Some("Read latency"));
    assert!(st.queries.iter().any(|q| q.hidden), "the hidden inputs stay: {:?}", st.queries);
    app.handle_event(Event::Key(key(KeyCode::Char(']'))), &tx).await.unwrap();
    settle_charts(&mut app, &tx, &mut rx).await;
    let st = app.spec_metrics.as_ref().unwrap();
    let latency = st.series.iter().find(|s| s.query.label == "Read latency").expect("still charted");
    assert!(latency.error.is_none() && !latency.points.is_empty(), "{latency:?}");
}
