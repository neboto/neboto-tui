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
    assert!(screen.contains(" 1 1h ") && screen.contains("]   AWS/Lambda"), "{screen}");
    assert!(!screen.contains("FunctionName="), "the title already names the function: {screen}");
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

/// Every spec-charted type the demo account has: `m` opens the spec view and
/// its first fetch lands with every metric answered (no `not in the
/// response`, no client error), home-region targets included.
#[tokio::test]
async fn every_spec_kind_in_the_demo_charts_cleanly() {
    let mut app = App::from_parts(crate::config::Config::default(), AwsClients::new_demo_for_test().await).unwrap();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut charted: Vec<String> = Vec::new();
    for svc in ServiceType::all() {
        if svc.is_virtual() {
            continue;
        }
        let Some(client) = app.services.get(&svc).cloned() else { continue };
        let Ok((rows, _)) = crate::headless::list_all(client, svc).await else { continue };
        let mut seen = std::collections::HashSet::new();
        for r in rows {
            if !seen.insert(r.resource_type().to_string()) {
                continue;
            }
            let kind = r.resource_type().to_string();
            select_mock(&mut app, svc, r);
            if app.selected_metric_target().is_none() {
                continue;
            }
            press(&mut app, &tx, KeyCode::Char('m')).await;
            assert_eq!(app.metrics_in_pane, Some(MetricsKind::Spec), "{kind}");
            settle(&mut app, &tx, &mut rx).await;
            let st = app.spec_metrics.as_ref().unwrap();
            let bad: Vec<String> = st.series.iter().filter_map(|s| s.error.clone()).collect();
            assert!(bad.is_empty(), "{kind}: {bad:?}");
            assert!(st.series.iter().all(|s| !s.points.is_empty()), "{kind}: a series came back empty");
            charted.push(kind);
            press(&mut app, &tx, KeyCode::Esc).await;
        }
    }
    assert!(charted.len() >= 3, "the demo has a few spec kinds: {charted:?}");
    eprintln!("charted: {charted:?}");
}

/// Every spec arm is reached by a harness mock, names the namespace and home
/// region it should, and charts cleanly against the demo's generated
/// `GetMetricData` — the coverage the demo account's own resources lack.
#[tokio::test]
async fn every_spec_target_has_a_mock_that_charts() {
    let mut app = App::from_parts(crate::config::Config::default(), AwsClients::new_demo_for_test().await).unwrap();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut seen: std::collections::BTreeMap<String, Option<&'static str>> = Default::default();
    for (svc, label, r) in all_mocks() {
        select_mock(&mut app, svc, r);
        let Some(target) = app.selected_metric_target() else { continue };
        seen.insert(target.namespace.clone(), target.home);
        assert!(target.dimensions.first().is_some_and(|(_, v)| !v.is_empty()), "{label}: no primary dimension");
        press(&mut app, &tx, KeyCode::Char('m')).await;
        settle(&mut app, &tx, &mut rx).await;
        let st = app.spec_metrics.as_ref().unwrap();
        let bad: Vec<String> = st.series.iter().filter_map(|s| s.error.clone()).collect();
        assert!(bad.is_empty() && st.series.iter().all(|s| !s.points.is_empty()), "{label}: {bad:?}");
        press(&mut app, &tx, KeyCode::Esc).await;
    }
    // AWS/Bedrock is the one arm left out: foundation models and inference
    // profiles are flat panes, so the harness has no mock for them.
    for ns in [
        "AWS/EC2", "AWS/EBS", "AWS/Lambda", "AWS/RDS", "AWS/DynamoDB", "AWS/EFS", "AWS/ElastiCache",
        "AWS/Kinesis", "AWS/Firehose", "AWS/Redshift", "AWS/Redshift-Serverless", "AWS/SQS", "AWS/SNS",
        "AWS/States", "AWS/Transfer", "AWS/NATGateway", "AWS/VPN", "AWS/DX", "AWS/ECR",
        "AWS/WorkSpaces", "AWS/Athena", "Glue", "AWS/TransitGateway", "AWS/Logs", "AWS/Events",
        "AWS/Route53Resolver", "AWS/CodeBuild", "AWS/CloudFront", "AWS/Route53", "AWS/GlobalAccelerator",
        "AWS/ECS", "AWS/AutoScaling", "ContainerInsights", "AWS/ApiGateway", "AWS/ApplicationELB", "AWS/ES",
        "AWS/DMS", "ECS/ContainerInsights", "AWS/WAFV2",
    ] {
        assert!(seen.contains_key(ns), "no mock reaches the {ns} spec arm: {seen:?}");
    }
    assert_eq!(seen["AWS/CloudFront"], Some("us-east-1"));
    assert_eq!(seen["AWS/Route53"], Some("us-east-1"));
    assert_eq!(seen["AWS/GlobalAccelerator"], Some("us-west-2"));
    assert_eq!(seen["AWS/SQS"], None);
}

/// A web ACL charts its five actions on one chart, keyed `WebACL` +
/// `Rule=ALL`, with `Region` only for a REGIONAL ACL: CLOUDFRONT-scope ACLs
/// publish in us-east-1 with no `Region` dimension at all. Its rule tables
/// sit beside the chart; a failed breakdown is a warning there, never an
/// error for the series.
#[tokio::test]
async fn a_web_acl_charts_its_actions_with_the_rule_tables_beside() {
    let mut app = App::from_parts(crate::config::Config::default(), AwsClients::new_demo_for_test().await).unwrap();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let acl = all_mocks().into_iter().find(|(_, l, _)| *l == "WafWebAcl").unwrap().2;
    select_mock(&mut app, ServiceType::Waf, acl.clone_box());
    let dims = |t: &crate::aws::metrics::MetricTarget| -> Vec<String> {
        t.dimensions.iter().map(|(n, v)| format!("{n}={v}")).collect()
    };

    let regional = app.selected_metric_target().unwrap();
    let region = app.aws_clients.current_region().to_string();
    assert_eq!(dims(&regional), ["WebACL=mock-web-acl".to_string(), "Rule=ALL".into(), format!("Region={region}")]);
    assert_eq!(regional.home, None);
    assert!(regional.breakdown.is_some());

    app.waf_scope = crate::aws::services::waf::WafScope::CloudFront;
    let cloudfront = app.selected_metric_target().unwrap();
    assert_eq!(dims(&cloudfront), ["WebACL=mock-web-acl", "Rule=ALL"]);
    assert_eq!(cloudfront.home, Some("us-east-1"));
    app.waf_scope = crate::aws::services::waf::WafScope::Regional;

    press(&mut app, &tx, KeyCode::Char('m')).await;
    assert_eq!(app.metrics_in_pane, Some(MetricsKind::Spec));
    settle(&mut app, &tx, &mut rx).await;
    let st = app.spec_metrics.as_ref().unwrap();
    let labels: Vec<&str> = st.series.iter().map(|s| s.query.label.as_str()).collect();
    assert_eq!(labels, ["Allowed", "Blocked", "Counted", "CAPTCHA", "Challenge", "Block rate"]);
    assert!(st.series[..5].iter().all(|s| s.query.group.as_deref() == Some("actions")), "one chart");
    assert!(st.series[5].query.expression.is_some(), "the rate is metric math");
    assert!(st.series.iter().all(|s| s.error.is_none() && !s.points.is_empty()), "the series chart");
    assert!(st.breakdown.is_some(), "the breakdown came back with them");

    press(&mut app, &tx, KeyCode::Char('Z')).await;
    let screen = screen_of(&app);
    assert!(screen.contains("Metrics — Web ACL · mock-web-acl"), "{screen}");
    assert!(screen.contains("Totals · last 1h"), "{screen}");
    assert!(screen.contains("WebACL=mock-web-acl, Rule=ALL"), "the looked-up dimension is shown: {screen}");
}

/// `Tab` into the totals panel, `j` to a rule and `⏎` charts that rule's
/// own actions (`Rule=<name>`, the ACL's WebACL and Region kept); `d`
/// goes back to the ACL's set. A label row charts with its namespace and
/// name.
#[tokio::test]
async fn enter_on_a_rule_charts_its_own_actions() {
    use crate::aws::services::waf::{WafRuleTraffic, WafTraffic};
    let mut app = App::from_parts(crate::config::Config::default(), AwsClients::new_demo_for_test().await).unwrap();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let acl = all_mocks().into_iter().find(|(_, l, _)| *l == "WafWebAcl").unwrap().2;
    select_mock(&mut app, ServiceType::Waf, acl.clone_box());
    press(&mut app, &tx, KeyCode::Char('m')).await;
    settle(&mut app, &tx, &mut rx).await;
    // The demo has no SEARCH answers, so the tables are put in by hand.
    let region = app.aws_clients.current_region().to_string();
    let base = vec![("WebACL".to_string(), "mock-web-acl".to_string()), ("Region".to_string(), region.clone())];
    let row = |name: &str, blocked: f64| WafRuleTraffic { name: name.into(), blocked, ..Default::default() };
    let traffic = WafTraffic {
        totals: Default::default(),
        rules: vec![row("Default_Action", 900.0), row("RateLimit-per-IP", 40.0)],
        labels: vec![row("awswaf:managed:aws:bot-control:bot:category:http_library", 5.0)],
    };
    app.spec_metrics.as_mut().unwrap().breakdown =
        Some(Ok(crate::aws::metrics::BreakdownData { summary: traffic.summary(), tables: traffic.tables(&base) }));

    press(&mut app, &tx, KeyCode::Tab).await;
    press(&mut app, &tx, KeyCode::Char('j')).await;
    assert_eq!(app.spec_metrics.as_ref().unwrap().table_cursor, Some(1));
    press(&mut app, &tx, KeyCode::Enter).await;
    settle(&mut app, &tx, &mut rx).await;
    let st = app.spec_metrics.as_ref().unwrap();
    assert_eq!(st.drilled.as_deref(), Some("RateLimit-per-IP"));
    assert_eq!(st.series.len(), 5, "the actions, no rate");
    let dims: Vec<String> = st.series[0].query.dimensions.iter().map(|(n, v)| format!("{n}={v}")).collect();
    assert_eq!(dims, ["WebACL=mock-web-acl".to_string(), format!("Region={region}"), "Rule=RateLimit-per-IP".into()]);
    assert!(st.series.iter().all(|s| s.error.is_none() && !s.points.is_empty()));
    let screen = screen_of(&app);
    assert!(screen.contains("▸ RateLimit-per-IP"), "the range line names it: {screen}");

    // The label below the two rules: namespace and name split back apart.
    press(&mut app, &tx, KeyCode::Char('j')).await;
    press(&mut app, &tx, KeyCode::Enter).await;
    settle(&mut app, &tx, &mut rx).await;
    let st = app.spec_metrics.as_ref().unwrap();
    let dims = &st.series[0].query.dimensions;
    assert!(dims.contains(&("LabelNamespace".into(), "awswaf:managed:aws:bot-control:bot:category".into())), "{dims:?}");
    assert!(dims.contains(&("LabelName".into(), "http_library".into())), "{dims:?}");
    assert!(!dims.iter().any(|(n, _)| n == "Rule"), "{dims:?}");

    // Esc leaves the tables, not the view; `d` is the ACL's set again.
    press(&mut app, &tx, KeyCode::Esc).await;
    assert_eq!(app.metrics_in_pane, Some(MetricsKind::Spec));
    assert_eq!(app.spec_metrics.as_ref().unwrap().table_cursor, None);
    press(&mut app, &tx, KeyCode::Char('d')).await;
    settle(&mut app, &tx, &mut rx).await;
    let st = app.spec_metrics.as_ref().unwrap();
    assert_eq!((st.series.len(), st.drilled.as_deref()), (6, None));
}

/// A distribution with a WAFv2 ACL charts the ACL's blocked count on its
/// Requests chart, keyed `WebACL` + `Rule=ALL` (no `Region`: CloudFront
/// scope) in us-east-1; when other loaded distributions share the ACL the
/// row says it's the ACL's total. A WAF Classic id adds nothing.
#[tokio::test]
async fn a_distribution_charts_its_web_acl_s_blocks_beside_its_requests() {
    use crate::aws::services::cloudfront::CfDistribution;
    let mut app = App::from_parts(crate::config::Config::default(), AwsClients::new_demo_for_test().await).unwrap();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mock = all_mocks().into_iter().find(|(svc, _, r)| *svc == ServiceType::CloudFront && r.as_any().is::<CfDistribution>()).unwrap().2;
    let mut dist = mock.as_any().downcast_ref::<CfDistribution>().unwrap().clone();
    dist.web_acl_id = "arn:aws:wafv2:us-east-1:123456789012:global/webacl/edge-acl/0123abcd".to_string();
    select_mock(&mut app, ServiceType::CloudFront, Box::new(dist.clone()));

    let target = app.selected_metric_target().unwrap();
    let row = target.extra.iter().find(|q| q.namespace == "AWS/WAFV2").expect("the WAF row");
    assert_eq!(row.label, "Blocked by WAF");
    assert_eq!(row.dimensions, [("WebACL".to_string(), "edge-acl".to_string()), ("Rule".to_string(), "ALL".to_string())]);
    assert_eq!(row.group.as_deref(), Some("requests"), "on the Requests chart");
    assert!(target.resolve.is_some());

    // A second distribution on the same ACL: the ACL's total isn't this one's.
    let mut other = dist.clone();
    other.id = "E2OTHER".to_string();
    app.resources.push(Box::new(other));
    let row = app.selected_metric_target().unwrap().extra.into_iter().find(|q| q.namespace == "AWS/WAFV2").unwrap();
    assert_eq!(row.label, "Blocked by WAF (ACL, 2 dists)");
    app.resources.truncate(1);

    press(&mut app, &tx, KeyCode::Char('m')).await;
    settle(&mut app, &tx, &mut rx).await;
    let st = app.spec_metrics.as_ref().unwrap();
    let waf = st.series.iter().find(|s| s.query.namespace == "AWS/WAFV2").expect("charted");
    assert!(waf.error.is_none() && !waf.points.is_empty(), "{:?}", waf.error);
    assert!(st.target.resolve.is_none(), "looked up once");

    let mut classic = dist;
    classic.web_acl_id = "a1b2c3d4-classic".to_string();
    select_mock(&mut app, ServiceType::CloudFront, Box::new(classic));
    assert!(app.selected_metric_target().unwrap().extra.is_empty(), "WAF Classic isn't WAFV2");
}

/// `5` reaches back 30 days (a stopped instance's last week of data), the
/// change markers' timeline widens with it, and `W` — which the markers'
/// legend offers — hands that timeline to the lens instead of doing nothing;
/// closing the lens goes back to the charts.
#[tokio::test]
async fn thirty_days_widens_the_markers_and_w_opens_their_timeline() {
    let mut app = App::from_parts(crate::config::Config::default(), AwsClients::new_demo_for_test().await).unwrap();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let svc = app.services[&ServiceType::Lambda].clone();
    let (rows, _) = crate::headless::list_all(svc, ServiceType::Lambda).await.ok().unwrap();
    select_mock(&mut app, ServiceType::Lambda, rows.into_iter().next().unwrap());
    press(&mut app, &tx, KeyCode::Char('m')).await;
    settle(&mut app, &tx, &mut rx).await;
    use crate::aws::services::cloudtrail::TrailRange;
    assert_eq!(app.chart_timeline.as_ref().map(|t| t.range), Some(TrailRange::D7));

    press(&mut app, &tx, KeyCode::Char('5')).await;
    settle(&mut app, &tx, &mut rx).await;
    let st = app.spec_metrics.as_ref().unwrap();
    assert_eq!(st.range, crate::aws::services::ec2::MetricsTimeRange::ThirtyDays);
    assert!(st.series.iter().all(|s| s.points.len() == 240), "30d at 3h");
    assert_eq!(app.chart_timeline.as_ref().map(|t| t.range), Some(TrailRange::D30));
    assert!(screen_of(&app).contains(" 5 30d "));
    // `]` wraps from the longest back to the shortest.
    press(&mut app, &tx, KeyCode::Char(']')).await;
    assert_eq!(app.spec_metrics.as_ref().unwrap().range, crate::aws::services::ec2::MetricsTimeRange::OneHour);

    let shift_w = KeyEvent::new(KeyCode::Char('W'), KeyModifiers::SHIFT);
    app.handle_event(Event::Key(shift_w), &tx).await.unwrap();
    assert!(app.metrics_in_pane.is_none() && app.spec_metrics.is_none());
    let lens = app.trail_in_pane.as_ref().expect("W opens the timeline");
    assert_eq!(lens.range, TrailRange::D30, "the markers' own timeline, not a refetch");
    assert!(app.chart_timeline.is_none());

    // Esc goes back to the charts as they were left — the range, the
    // picked stat, the cursor — and the timeline back behind the markers.
    let generation = app.spec_metrics_generation;
    press(&mut app, &tx, KeyCode::Esc).await;
    assert!(app.trail_in_pane.is_none());
    assert_eq!(app.metrics_in_pane, Some(MetricsKind::Spec));
    let st = app.spec_metrics.as_ref().expect("the view comes back");
    assert_eq!(st.range, crate::aws::services::ec2::MetricsTimeRange::OneHour);
    assert_eq!(app.chart_timeline.as_ref().map(|t| t.range), Some(TrailRange::D30));
    // That `]` fetch never landed (the lens was up): it's asked again.
    assert!(app.spec_metrics_generation > generation, "a fetch lost behind the lens is redone");
    settle(&mut app, &tx, &mut rx).await;
    assert!(app.spec_metrics.as_ref().unwrap().series.iter().all(|s| s.points.len() == 60));

    // A settled view comes back without a refetch.
    app.handle_event(Event::Key(shift_w), &tx).await.unwrap();
    let generation = app.spec_metrics_generation;
    press(&mut app, &tx, KeyCode::Esc).await;
    assert_eq!(app.metrics_in_pane, Some(MetricsKind::Spec));
    assert_eq!(app.spec_metrics_generation, generation);

    // Esc again closes the charts as usual; a later W/Esc from the
    // detail pane has nothing to go back to.
    press(&mut app, &tx, KeyCode::Esc).await;
    assert!(app.metrics_in_pane.is_none());
    app.handle_event(Event::Key(shift_w), &tx).await.unwrap();
    press(&mut app, &tx, KeyCode::Esc).await;
    assert!(app.trail_in_pane.is_none() && app.metrics_in_pane.is_none());
}

/// `[` from 1h wraps to 30d, and the changes widen with it, as `]` and
/// `5` do — else the chart spans 30 days with a week of markers and `W`
/// opens on 7d (#288).
#[tokio::test]
async fn bracket_back_to_thirty_days_widens_the_markers_too() {
    let mut app = App::from_parts(crate::config::Config::default(), AwsClients::new_demo_for_test().await).unwrap();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let svc = app.services[&ServiceType::Lambda].clone();
    let (rows, _) = crate::headless::list_all(svc, ServiceType::Lambda).await.ok().unwrap();
    select_mock(&mut app, ServiceType::Lambda, rows.into_iter().next().unwrap());
    press(&mut app, &tx, KeyCode::Char('m')).await;
    settle(&mut app, &tx, &mut rx).await;
    use crate::aws::services::cloudtrail::TrailRange;
    assert_eq!(app.chart_timeline.as_ref().map(|t| t.range), Some(TrailRange::D7));

    press(&mut app, &tx, KeyCode::Char('[')).await;
    settle(&mut app, &tx, &mut rx).await;
    assert_eq!(app.spec_metrics.as_ref().unwrap().range, crate::aws::services::ec2::MetricsTimeRange::ThirtyDays);
    assert_eq!(app.chart_timeline.as_ref().map(|t| t.range), Some(TrailRange::D30));

    let shift_w = KeyEvent::new(KeyCode::Char('W'), KeyModifiers::SHIFT);
    app.handle_event(Event::Key(shift_w), &tx).await.unwrap();
    assert_eq!(app.trail_in_pane.as_ref().map(|t| t.range), Some(TrailRange::D30));
}

/// The change legend rides the chart it explains (its bottom border), the
/// view's own footer names `c`/`W`, and the status bar under an open view
/// shows no keys: the detail pane's would mean something else here, and
/// `:`/`?` don't run inside it.
#[tokio::test]
async fn the_change_legend_sits_on_the_chart_and_the_status_bar_steps_back() {
    let mut app = App::from_parts(crate::config::Config::default(), AwsClients::new_demo_for_test().await).unwrap();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let svc = app.services[&ServiceType::Lambda].clone();
    let (rows, _) = crate::headless::list_all(svc, ServiceType::Lambda).await.ok().unwrap();
    let orders = rows.into_iter().find(|r| r.name() == "orders-api").unwrap();
    select_mock(&mut app, ServiceType::Lambda, orders);
    press(&mut app, &tx, KeyCode::Char('m')).await;
    settle(&mut app, &tx, &mut rx).await;

    let end = app.metrics_window_end;
    let row = |ago: i64, what: &str| crate::timeline::TimelineRow {
        ts_secs: end - ago,
        time: String::new(),
        source: crate::timeline::TimelineSource::Trail,
        what: what.to_string(),
        who: String::new(),
        detail: String::new(),
        error: String::new(),
        failed: false,
        json: String::new(),
    };
    let tl = app.chart_timeline.as_mut().unwrap();
    tl.pending.clear();
    tl.rows = vec![row(600, "StopInstances"), row(1200, "StartInstances"), row(1800, "StopInstances")];

    let screen = screen_of(&app);
    let lines: Vec<&str> = screen.lines().collect();
    let caption = lines.iter().position(|l| l.contains("3 changes  StopInstances ×2 · StartInstances")).expect(&screen);
    assert!(lines[caption].contains('└'), "on the chart's bottom border: {}", lines[caption]);
    assert!(!screen.contains("c hide · W timeline"), "not on the pane border too: {screen}");
    assert!(screen.contains("c changes") && screen.contains("W timeline"), "{screen}");
    assert!(screen.contains("Esc close"), "the footer keeps Esc when it's tight: {screen}");
    let status = lines.last().unwrap();
    assert!(!status.contains("metrics") && !status.contains("back") && !status.contains("help"), "{status}");
}

/// A CloudWatch metric row charts itself, and the view says exactly what it
/// asked CloudWatch for: a metric name alone doesn't say which instance.
#[tokio::test]
async fn a_metric_row_shows_its_namespace_and_dimensions() {
    let mut app = App::from_parts(crate::config::Config::default(), AwsClients::new_demo_for_test().await).unwrap();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let metric = crate::aws::services::cloudwatch::CwMetric {
        id: "AWS/EC2/CPUUtilization/i-0abc".to_string(),
        namespace: "AWS/EC2".to_string(),
        metric_name: "CPUUtilization".to_string(),
        dimensions: vec![("InstanceId".to_string(), "i-0abc".to_string())],
        tags: Default::default(),
    };
    select_mock(&mut app, ServiceType::CloudWatch, Box::new(metric));
    press(&mut app, &tx, KeyCode::Char('m')).await;
    assert_eq!(app.metrics_in_pane, Some(MetricsKind::Spec));
    settle(&mut app, &tx, &mut rx).await;
    let screen = screen_of(&app);
    assert!(screen.contains("Metrics — CloudWatch metric · CPUUtilization"), "{screen}");
    assert!(screen.contains("]   AWS/EC2 · InstanceId=i-0abc"), "{screen}");
    assert!(app.spec_metrics.as_ref().unwrap().as_text().contains("AWS/EC2 · InstanceId=i-0abc"));
}

/// A CloudTrail lookup that stopped at its cap says so in the `W` lens:
/// the count reads `N+` and the header names the time it read back to.
#[tokio::test]
async fn a_capped_lookup_says_how_far_back_it_read() {
    let mut app = App::from_parts(crate::config::Config::default(), AwsClients::new_demo_for_test().await).unwrap();
    let (tx, _rx) = mpsc::unbounded_channel();
    let svc = app.services[&ServiceType::Lambda].clone();
    let (rows, _) = crate::headless::list_all(svc, ServiceType::Lambda).await.ok().unwrap();
    select_mock(&mut app, ServiceType::Lambda, rows.into_iter().next().unwrap());
    let shift_w = KeyEvent::new(KeyCode::Char('W'), KeyModifiers::SHIFT);
    app.handle_event(Event::Key(shift_w), &tx).await.unwrap();
    assert!(app.trail_in_pane.is_some());

    let cut = 1_800_000_000;
    let event = crate::aws::services::cloudtrail::CloudTrailEvent::from_sdk(
        &aws_sdk_cloudtrail::types::Event::builder()
            .event_name("StopInstances")
            .event_time(aws_sdk_cloudtrail::primitives::DateTime::from_secs(cut))
            .build(),
    );
    let generation = app.trail_generation;
    app.handle_event(Event::TrailLensLoaded { generation, events: vec![event], cut: Some(cut) }, &tx).await.unwrap();
    let screen = screen_of(&app);
    let at = crate::aws::services::cloudwatch::fmt_epoch_secs(cut);
    assert!(screen.contains(&format!("CloudTrail stopped at its cap: nothing before {} read", &at[5..])), "{screen}");
    assert!(screen.contains("1+"), "{screen}");
}

