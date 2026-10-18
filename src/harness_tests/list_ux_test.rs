//! #118 polish: pinned service chips, `/` search history, the detail
//! footer's `⌫ back to …` hint, and the list's per-type summary column.

use super::*;
use crate::aws::services::ec2::Ec2Instance;
use crate::aws::services::ecs::EcsServiceInfo;
use crate::aws::services::lambda::LambdaFunction;
use crate::aws::services::rds::{RdsCluster, RdsInstance};

async fn press(app: &mut App, tx: &mpsc::UnboundedSender<Event>, code: KeyCode) {
    app.handle_event(Event::Key(key(code)), tx).await.unwrap();
}

async fn type_str(app: &mut App, tx: &mpsc::UnboundedSender<Event>, text: &str) {
    for c in text.chars() {
        press(app, tx, KeyCode::Char(c)).await;
    }
}

#[tokio::test]
async fn pinned_services_show_as_chips_before_they_load() {
    let config: crate::config::Config =
        toml::from_str(r#"pinned_services = ["ecs", "lambda", "nosuchservice"]"#).unwrap();
    let app = App::from_parts(config, AwsClients::new_for_test().await).unwrap();
    assert!(app.visited_services.contains(&ServiceType::ECS));
    assert!(app.visited_services.contains(&ServiceType::Lambda));
    // No default_service: still the splash, nothing loading.
    assert_eq!(app.current_service, None);
    assert_eq!(app.visited_services.len(), 2);
}

#[tokio::test]
async fn up_and_down_walk_the_search_history_and_restore_the_draft() {
    let (mut app, tx, _rx) = test_app().await;
    app.current_service = Some(ServiceType::EC2);
    for query in ["web", "api", "web"] {
        app.search_active = true;
        app.search_query.clear();
        type_str(&mut app, &tx, query).await;
        press(&mut app, &tx, KeyCode::Enter).await;
    }
    // A repeat moves to the end rather than appearing twice.
    assert_eq!(app.search_history, vec!["api".to_string(), "web".to_string()]);

    app.search_active = true;
    app.search_query = "dr".to_string();
    press(&mut app, &tx, KeyCode::Up).await;
    assert_eq!(app.search_query, "web");
    press(&mut app, &tx, KeyCode::Up).await;
    assert_eq!(app.search_query, "api");
    // At the oldest entry, ↑ stays put.
    press(&mut app, &tx, KeyCode::Up).await;
    assert_eq!(app.search_query, "api");
    press(&mut app, &tx, KeyCode::Down).await;
    assert_eq!(app.search_query, "web");
    // ↓ past the newest gives back what was being typed.
    press(&mut app, &tx, KeyCode::Down).await;
    assert_eq!(app.search_query, "dr");
    press(&mut app, &tx, KeyCode::Down).await;
    assert_eq!(app.search_query, "dr");

    // Editing a recalled entry starts a fresh walk from the newest.
    press(&mut app, &tx, KeyCode::Up).await;
    press(&mut app, &tx, KeyCode::Up).await;
    type_str(&mut app, &tx, "-v2").await;
    assert_eq!(app.search_query, "api-v2");
    press(&mut app, &tx, KeyCode::Up).await;
    assert_eq!(app.search_query, "web");

    // Blank submissions aren't recorded.
    app.search_query.clear();
    press(&mut app, &tx, KeyCode::Enter).await;
    assert_eq!(app.search_history.len(), 2);
}

#[tokio::test]
async fn detail_footer_names_where_back_goes() {
    let (mut app, _tx, _rx) = test_app().await;
    let instance = |id: &str| -> Box<dyn Resource> {
        Box::new(Ec2Instance::from_sdk(&aws_sdk_ec2::types::Instance::builder().instance_id(id).build()))
    };
    app.current_service = Some(ServiceType::EC2);
    app.resources = vec![instance("i-0here")];
    app.filtered_resources = vec![0];
    app.selected_index = Some(0);
    assert_eq!(app.nav_back_hint(), None, "no history, no hint");

    // History pointing at where we already are says nothing.
    app.record_location();
    assert_eq!(app.nav_back_hint(), None);

    app.nav_history[0].selected_id = Some("arn:aws:lambda:us-east-1:111122223333:function:orders".into());
    app.nav_history[0].service = ServiceType::Lambda;
    assert_eq!(app.nav_back_hint().as_deref(), Some("λ orders"));

    app.details_focused = true;
    let screen = draw(&app);
    assert!(screen.contains("⌫ back to λ orders"), "footer: {screen}");
    assert!(screen.contains("h list"), "h should say it goes to the list: {screen}");
}

fn draw(app: &App) -> String {
    let mut terminal = Terminal::new(TestBackend::new(200, 30)).unwrap();
    terminal.draw(|f| crate::render_app(app, f)).unwrap();
    let buf = terminal.backend().buffer();
    (0..buf.area.height)
        .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn list_summaries_say_what_the_row_is() {
    let ec2 = |tag: Option<&str>| {
        let mut b = aws_sdk_ec2::types::Instance::builder()
            .instance_id("i-0abc")
            .instance_type(aws_sdk_ec2::types::InstanceType::T3Micro);
        if let Some(n) = tag {
            b = b.tags(aws_sdk_ec2::types::Tag::builder().key("Name").value(n).build());
        }
        Ec2Instance::from_sdk(&b.build())
    };
    // Named: the type, then the id the name hides.
    assert_eq!(ec2(Some("web-1")).list_summary().as_deref(), Some("t3.micro · i-0abc"));
    // Untagged: the name column already is the id.
    assert_eq!(ec2(None).list_summary().as_deref(), Some("t3.micro"));

    let lambda = |runtime: Option<aws_sdk_lambda::types::Runtime>| {
        let mut b = aws_sdk_lambda::types::FunctionConfiguration::builder()
            .function_name("orders")
            .function_arn("arn:aws:lambda:us-east-1:111122223333:function:orders");
        if let Some(r) = runtime {
            b = b.runtime(r);
        }
        LambdaFunction::from_sdk(&b.build())
    };
    assert_eq!(
        lambda(Some(aws_sdk_lambda::types::Runtime::Python312)).list_summary().as_deref(),
        Some("python3.12")
    );
    // A container-image function has no runtime: plain id column.
    assert_eq!(lambda(None).list_summary(), None);

    let rds = RdsInstance::from_sdk(
        &aws_sdk_rds::types::DbInstance::builder()
            .db_instance_identifier("orders-db")
            .engine("postgres")
            .engine_version("16.3")
            .db_instance_class("db.r6g.large")
            .build(),
    );
    assert_eq!(rds.list_summary().as_deref(), Some("postgres 16.3 · db.r6g.large"));

    let cluster = |mode: &str| {
        RdsCluster::from_sdk(
            &aws_sdk_rds::types::DbCluster::builder()
                .db_cluster_identifier("orders")
                .engine("aurora-postgresql")
                .engine_version("16.4")
                .engine_mode(mode)
                .build(),
        )
    };
    assert_eq!(cluster("provisioned").list_summary().as_deref(), Some("aurora-postgresql 16.4"));
    assert_eq!(cluster("serverless").list_summary().as_deref(), Some("aurora-postgresql 16.4 · serverless"));

    let ecs = |running: i32, desired: i32, pending: i32| {
        EcsServiceInfo::from_sdk(
            &aws_sdk_ecs::types::Service::builder()
                .service_arn("arn:aws:ecs:us-east-1:111122223333:service/prod/web")
                .service_name("web")
                .running_count(running)
                .desired_count(desired)
                .pending_count(pending)
                .build(),
        )
    };
    assert_eq!(ecs(2, 3, 0).list_summary().as_deref(), Some("2/3 running"));
    assert_eq!(ecs(1, 3, 2).list_summary().as_deref(), Some("1/3 running · 2 pending"));
}

#[tokio::test]
async fn the_list_shows_the_summary_column() {
    let (mut app, _tx, _rx) = test_app().await;
    app.current_service = Some(ServiceType::EC2);
    app.resources = vec![Box::new(Ec2Instance::from_sdk(
        &aws_sdk_ec2::types::Instance::builder()
            .instance_id("i-0abc")
            .instance_type(aws_sdk_ec2::types::InstanceType::T3Micro)
            .tags(aws_sdk_ec2::types::Tag::builder().key("Name").value("web-1").build())
            .build(),
    ))];
    app.filtered_resources = vec![0];
    app.selected_index = Some(0);
    let screen = draw(&app);
    assert!(screen.contains("t3.micro · i-0abc"), "{screen}");
}

// ── Back-history sync: what the `⌫ back to …` hint made visible ──────────

fn ec2_rows(app: &mut App, ids: &[&str]) {
    app.current_service = Some(ServiceType::EC2);
    app.resources = ids
        .iter()
        .map(|id| -> Box<dyn Resource> {
            Box::new(Ec2Instance::from_sdk(&aws_sdk_ec2::types::Instance::builder().instance_id(*id).build()))
        })
        .collect();
    app.filtered_resources = (0..ids.len()).collect();
    app.selected_index = Some(0);
}

/// Back to A, move to B, jump away: back (and the hint) must say B — the
/// place you left from — not the A you passed through.
#[tokio::test]
async fn back_returns_to_where_you_left_not_where_you_went_back_to() {
    let (mut app, _tx, _rx) = test_app().await;
    ec2_rows(&mut app, &["i-a", "i-b"]);
    app.record_location(); // jump away from A
    app.nav_cursor = Some(0); // ⌫ back to A
    app.selected_index = Some(1); // j to B
    app.record_location(); // jump away from B
    app.current_service = Some(ServiceType::Lambda);
    assert_eq!(app.nav_back_hint().as_deref(), Some("EC2 i-b"));
    assert_eq!(app.nav_history.len(), 1);
}

/// `@lambda` from EC2, then ⌫: back lands on EC2 and stays there. The
/// switch used to record the half-typed `@lambda` as EC2's query, so the
/// restore re-ran it and bounced straight back to Lambda.
#[tokio::test]
async fn back_after_a_service_search_does_not_bounce_forward() {
    let (mut app, tx, _rx) = test_app().await;
    ec2_rows(&mut app, &["i-a"]);
    app.search_active = true;
    app.search_query = "@lambda".into();
    app.update_search();
    press(&mut app, &tx, KeyCode::Enter).await;
    assert_eq!(app.current_service, Some(ServiceType::Lambda));
    assert_eq!(app.nav_history[0].query, "");

    press(&mut app, &tx, KeyCode::Backspace).await;
    if app.search_needs_update {
        app.update_search();
    }
    assert_eq!(app.current_service, Some(ServiceType::EC2));
    assert_eq!(app.search_query, "");
    assert_eq!(app.pending_jump.as_deref(), Some("i-a"), "the row is re-selected");
}

/// A filter on the list you're leaving is kept — only the switch query is dropped.
#[tokio::test]
async fn a_same_service_filter_is_still_recorded() {
    let (mut app, _tx, _rx) = test_app().await;
    ec2_rows(&mut app, &["i-a"]);
    app.search_query = "web".into();
    app.record_location();
    assert_eq!(app.nav_history[0].query, "web");
    app.nav_history.clear();
    app.search_query = "@all web".into();
    app.record_location();
    assert_eq!(app.nav_history[0].query, "@all web", "@all results are a place to come back to");
}

/// ⌫ goes back from the detail pane too (it used to be list-only, while the
/// footer hint named it), and the hint is a click target that does the same.
#[tokio::test]
async fn back_hint_key_and_click_both_go_back_from_the_detail_pane() {
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    let (mut app, tx, _rx) = test_app().await;
    ec2_rows(&mut app, &["i-a", "i-b"]);
    app.record_location(); // leaving A
    app.selected_index = Some(1);
    app.details_focused = true;

    press(&mut app, &tx, KeyCode::Backspace).await;
    assert_eq!(app.nav_cursor, Some(0), "⌫ in the detail pane walks history");
    assert_eq!(landed_on(&app).as_deref(), Some("i-a"));

    // Again from B, by mouse.
    app.nav_cursor = None;
    app.pending_jump = None;
    app.selected_index = Some(1);
    app.details_focused = true;
    let screen = draw(&app);
    let rect = app
        .click_regions
        .borrow()
        .iter()
        .find(|r| matches!(r.action, ClickAction::Press(KeyCode::Backspace)))
        .expect("the back hint is clickable")
        .rect;
    let row = screen.lines().nth(rect.y as usize).unwrap();
    let covered: String = row.chars().skip(rect.x as usize).take(rect.width as usize).collect();
    assert_eq!(covered, "⌫ back to EC2 i-a", "region sits exactly on the hint");

    let ev = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: rect.x + rect.width / 2,
        row: rect.y,
        modifiers: KeyModifiers::NONE,
    };
    app.handle_event(Event::Mouse(ev), &tx).await.unwrap();
    assert_eq!(app.nav_cursor, Some(0));
    assert_eq!(landed_on(&app).as_deref(), Some("i-a"));
}

/// The restored row: still pending, or already resolved (the rows are loaded).
fn landed_on(app: &App) -> Option<String> {
    app.pending_jump.clone().or_else(|| app.get_selected_resource().map(|r| r.id().to_string()))
}

/// No history, no hint, no region.
#[tokio::test]
async fn no_back_region_without_history() {
    let (mut app, _tx, _rx) = test_app().await;
    ec2_rows(&mut app, &["i-a"]);
    app.details_focused = true;
    draw(&app);
    assert!(!app
        .click_regions
        .borrow()
        .iter()
        .any(|r| matches!(r.action, ClickAction::Press(KeyCode::Backspace))));
}

// ── Forward (Ctrl-F) ─────────────────────────────────────────────────────

fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

/// A → jump → B, ⌫ back to A, Ctrl-F forward to B again. Going back from
/// live records B first — before, "here" was never in the history, so
/// there was nothing to come forward to.
#[tokio::test]
async fn forward_returns_to_where_back_left() {
    let (mut app, tx, _rx) = test_app().await;
    ec2_rows(&mut app, &["i-a", "i-b"]);
    app.record_location(); // jump away from A …
    app.selected_index = Some(1); // … landing on B
    app.details_focused = true;
    assert_eq!(app.nav_forward_hint(), None, "live: nothing ahead");

    press(&mut app, &tx, KeyCode::Backspace).await;
    assert_eq!(landed_on(&app).as_deref(), Some("i-a"));
    assert_eq!(app.nav_forward_hint().as_deref(), Some("EC2 i-b"));
    assert_eq!(app.nav_back_hint(), None, "A is the oldest");

    app.handle_event(Event::Key(ctrl('f')), &tx).await.unwrap();
    assert_eq!(landed_on(&app).as_deref(), Some("i-b"));
    assert_eq!(app.nav_forward_hint(), None, "B is the newest");
    assert_eq!(app.nav_back_hint().as_deref(), Some("EC2 i-a"));
    assert_eq!(app.nav_history.len(), 2, "back and forth adds no entries");
}

/// Back, then a new jump: the forward branch is gone, as in a browser.
#[tokio::test]
async fn a_new_jump_after_going_back_drops_forward() {
    let (mut app, tx, _rx) = test_app().await;
    ec2_rows(&mut app, &["i-a", "i-b", "i-c"]);
    app.record_location(); // A → B
    app.selected_index = Some(1);
    press(&mut app, &tx, KeyCode::Backspace).await; // back to A, B ahead
    app.pending_jump = None;
    app.selected_index = Some(2); // move to C, then jump away
    app.record_location();
    assert_eq!(app.nav_forward_hint(), None);
    let ids: Vec<_> = app.nav_history.iter().map(|l| l.selected_id.clone().unwrap()).collect();
    assert_eq!(ids, vec!["i-c"], "A was replaced by where we left from; B dropped");
}

/// Ctrl-F must not be eaten by a plain-`f` arm: a log group's detail pane
/// binds `f` (quick search) without checking modifiers.
#[tokio::test]
async fn ctrl_f_is_forward_even_where_f_means_something() {
    let (mut app, tx, _rx) = test_app().await;
    let group = |name: &str| -> Box<dyn Resource> {
        Box::new(crate::aws::services::cloudwatch::CwLogGroup::from_sdk(
            &aws_sdk_cloudwatchlogs::types::LogGroup::builder()
                .log_group_name(name)
                .arn(format!("arn:aws:logs:us-east-1:111122223333:log-group:{name}"))
                .build(),
        ))
    };
    app.current_service = Some(ServiceType::CloudWatch);
    app.resources = vec![group("/a"), group("/b")];
    app.filtered_resources = vec![0, 1];
    app.selected_index = Some(0);
    app.record_location();
    app.selected_index = Some(1);
    app.details_focused = true;
    press(&mut app, &tx, KeyCode::Backspace).await;
    app.details_focused = true;

    app.handle_event(Event::Key(ctrl('f')), &tx).await.unwrap();
    assert!(!app.log_tail.visible, "Ctrl-F opened the quick search instead");
    assert_eq!(app.nav_cursor, Some(1));
}

/// The forward hint is a click target too.
#[tokio::test]
async fn forward_hint_is_clickable() {
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    let (mut app, tx, _rx) = test_app().await;
    ec2_rows(&mut app, &["i-a", "i-b"]);
    app.record_location();
    app.selected_index = Some(1);
    press(&mut app, &tx, KeyCode::Backspace).await;
    app.details_focused = true;
    let screen = draw(&app);
    let rect = app
        .click_regions
        .borrow()
        .iter()
        .find(|r| matches!(r.action, ClickAction::NavForward))
        .expect("the forward hint is clickable")
        .rect;
    let row = screen.lines().nth(rect.y as usize).unwrap();
    let covered: String = row.chars().skip(rect.x as usize).take(rect.width as usize).collect();
    assert_eq!(covered, "⌃F forward to EC2 i-b");
    let ev = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: rect.x + 1,
        row: rect.y,
        modifiers: KeyModifiers::NONE,
    };
    app.handle_event(Event::Mouse(ev), &tx).await.unwrap();
    assert_eq!(landed_on(&app).as_deref(), Some("i-b"));
}
