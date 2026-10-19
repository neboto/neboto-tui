//! #92 step 2: opening `R` lists the current service in each configured
//! region (into that region's cache slot) and the picker shows the counts.

use super::*;
use crate::aws::region::Region;
use crate::ui::widgets::region_selector::RegionCount;
use crossterm::event::KeyModifiers;

/// A demo-account app with config `regions`. `for_region` keeps the demo
/// HTTP client, so the fan-out makes real SDK calls against the fixtures.
async fn fanout_app(regions: &str) -> (App, mpsc::UnboundedSender<Event>, mpsc::UnboundedReceiver<Event>) {
    let config: crate::config::Config = toml::from_str(&format!("regions = {regions}")).unwrap();
    let mut app = App::from_parts(config, AwsClients::new_demo_for_test().await).unwrap();
    app.current_service = Some(ServiceType::EC2);
    let (tx, rx) = mpsc::unbounded_channel();
    (app, tx, rx)
}

async fn open_picker(app: &mut App, tx: &mpsc::UnboundedSender<Event>) {
    let r = crossterm::event::KeyEvent::new(KeyCode::Char('R'), KeyModifiers::SHIFT);
    app.handle_event(Event::Key(r), tx).await.unwrap();
    assert!(app.region_selector.visible);
}

/// Handle fan-out results until nothing is `Loading`.
async fn settle(app: &mut App, tx: &mpsc::UnboundedSender<Event>, rx: &mut mpsc::UnboundedReceiver<Event>) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while app.region_fanout.values().any(|s| *s == RegionCount::Loading) {
        assert!(std::time::Instant::now() < deadline, "fan-out never settled");
        let Ok(Some(ev)) = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv()).await else {
            continue;
        };
        if matches!(ev, Event::RegionFanoutLoaded { .. }) {
            app.handle_event(ev, tx).await.unwrap();
        }
    }
}

#[tokio::test]
async fn opening_r_counts_the_current_tab_in_each_configured_region() {
    let (mut app, tx, mut rx) = fanout_app(r#"["us-east-1", "eu-west-1", "ap-southeast-2"]"#).await;
    assert_eq!(app.current_region, Region::UsEast1);
    open_picker(&mut app, &tx).await;
    // The other two load; the current region is the normal load's.
    assert_eq!(app.region_fanout.get(&(ServiceType::EC2, Region::EuWest1)), Some(&RegionCount::Loading));
    assert!(!app.region_fanout.contains_key(&(ServiceType::EC2, Region::UsEast1)));
    assert_eq!(app.region_counts().get(&Region::EuWest1), Some(&RegionCount::Loading));
    settle(&mut app, &tx, &mut rx).await;

    // Instances tab: the demo account's three instances, not every row.
    let counts = app.region_counts();
    assert_eq!(counts.get(&Region::EuWest1), Some(&RegionCount::Count(3)));
    assert_eq!(counts.get(&Region::ApSoutheast2), Some(&RegionCount::Count(3)));
    // Not listed yet → nothing, never 0.
    assert_eq!(counts.get(&Region::UsEast1), None);
    assert_eq!(counts.get(&Region::UsWest2), None);
    assert_eq!(app.region_count_label().as_deref(), Some("EC2 Instance"));
    // The rows went to the cache, so switching there is a cache hit.
    assert!(app.cache.get_ref(&ServiceType::EC2, &Region::EuWest1, None).is_some());

    // On screen: the counts beside their regions, and what's counted.
    let screen = screen_of(&app);
    assert!(screen.contains("EC2 Instance per region"), "{screen}");
    let row = |code: &str| screen.lines().find(|l| l.contains(code)).unwrap_or_default().to_string();
    assert!(row("eu-west-1 ").contains("EU West (Ireland)           3 "), "{}", row("eu-west-1 "));
    assert!(!row("us-west-2 ").contains("  0 "), "not listed must not read as zero: {}", row("us-west-2 "));

    // Another tab counts its own type from the same cache.
    app.ec2_view = Ec2View::SecurityGroups;
    assert_eq!(app.region_counts().get(&Region::EuWest1), Some(&RegionCount::Count(4)));

    // Fresh caches: opening again fetches nothing.
    open_picker(&mut app, &tx).await;
    assert!(app.region_fanout.is_empty());
}

#[tokio::test]
async fn fanout_results_from_another_account_or_a_denied_region() {
    let (mut app, tx, _rx) = fanout_app(r#"["eu-west-1", "ap-southeast-2"]"#).await;
    let rows = || -> Vec<Box<dyn Resource>> { Vec::new() };
    let event = |generation, region, result| Event::RegionFanoutLoaded {
        generation,
        service: ServiceType::EC2,
        region,
        result,
    };

    // A profile/role switch bumped the generation: the late result is dropped.
    app.region_fanout_generation = 1;
    app.handle_event(event(0, Region::EuWest1, Ok(rows())), &tx).await.unwrap();
    assert!(app.cache.get_ref(&ServiceType::EC2, &Region::EuWest1, None).is_none());

    let denied = "UnauthorizedOperation: You are not authorized to perform this operation. explicit deny in a service control policy";
    app.handle_event(event(1, Region::EuWest1, Err(denied.into())), &tx).await.unwrap();
    app.handle_event(event(1, Region::ApSoutheast2, Err("dispatch failure".into())), &tx).await.unwrap();
    let counts = app.region_counts();
    assert_eq!(counts.get(&Region::EuWest1), Some(&RegionCount::Denied));
    assert_eq!(counts.get(&Region::ApSoutheast2), Some(&RegionCount::Failed));

    // Reopening retries the failure but not the deny.
    open_picker(&mut app, &tx).await;
    assert_eq!(app.region_fanout.get(&(ServiceType::EC2, Region::EuWest1)), Some(&RegionCount::Denied));
    assert_eq!(app.region_fanout.get(&(ServiceType::EC2, Region::ApSoutheast2)), Some(&RegionCount::Loading));
}

#[tokio::test]
async fn no_fanout_without_regions_or_for_global_and_scoped_services() {
    // No config `regions`: the picker is today's.
    let (mut app, tx, _rx) = test_app().await;
    app.current_service = Some(ServiceType::EC2);
    open_picker(&mut app, &tx).await;
    assert!(app.region_fanout.is_empty());
    assert!(app.region_counts().is_empty());

    let (mut app, tx, _rx) = fanout_app(r#"["eu-west-1"]"#).await;
    // Global: nothing to list per region.
    app.current_service = Some(ServiceType::IAM);
    open_picker(&mut app, &tx).await;
    assert!(app.region_fanout.is_empty());
    assert_eq!(app.region_count_label(), None);
    // Variant-cached: its list depends on App state a fresh client set
    // wouldn't have (GuardDuty's severity scope).
    app.region_selector.hide();
    app.current_service = Some(ServiceType::GuardDuty);
    open_picker(&mut app, &tx).await;
    assert!(app.region_fanout.is_empty());
}

fn screen_of(app: &App) -> String {
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(170, 50)).unwrap();
    terminal.draw(|f| crate::render_app(app, f)).unwrap();
    let buf = terminal.backend().buffer();
    (0..buf.area.height)
        .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Switching back to a region whose list is cached shows it straight away.
/// A cache hit in `load_service_resources` fills `resources` but leaves the
/// filtering to the caller; the region switch never did it, which nothing
/// noticed while every switch cleared the cache.
#[tokio::test]
async fn switching_back_to_a_cached_region_shows_its_list() {
    let (mut app, tx, _rx) = test_app().await;
    let instance: Box<dyn Resource> = Box::new(crate::aws::services::ec2::Ec2Instance::from_sdk(
        &aws_sdk_ec2::types::Instance::builder().instance_id("i-0sydney").build(),
    ));
    app.current_service = Some(ServiceType::EC2);
    app.cache.insert(ServiceType::EC2, Region::ApSoutheast2, None, vec![instance]);
    assert_ne!(app.current_region, Region::ApSoutheast2);
    // A cross-region jump (Resource Explorer) waiting on this region too.
    app.pending_jump = Some("i-0sydney".into());
    app.pending_jump_region = Some(Region::ApSoutheast2);

    app.handle_event(Event::RegionSwitchRequested { region: Region::ApSoutheast2 }, &tx)
        .await
        .unwrap();
    assert_eq!(app.current_region, Region::ApSoutheast2);
    assert!(!app.loading, "a cache hit isn't a load");
    let shown: Vec<&str> = app.filtered_resources.iter().map(|&i| app.resources[i].id()).collect();
    assert_eq!(shown, ["i-0sydney"]);
    // …and the jump resolved instead of waiting for a load that never comes.
    assert_eq!(app.pending_jump, None);
    assert_eq!(app.get_selected_resource().map(|r| r.id()), Some("i-0sydney"));
}

fn instance(id: &str) -> Box<dyn Resource> {
    Box::new(crate::aws::services::ec2::Ec2Instance::from_sdk(
        &aws_sdk_ec2::types::Instance::builder().instance_id(id).build(),
    ))
}

/// Handle whatever the last step queued (the picker's and nav-back's
/// `RegionSwitchRequested`) — the main loop's job.
async fn drain(app: &mut App, tx: &mpsc::UnboundedSender<Event>, rx: &mut mpsc::UnboundedReceiver<Event>) {
    while let Ok(ev) = rx.try_recv() {
        app.handle_event(ev, tx).await.unwrap();
    }
}

async fn press(app: &mut App, tx: &mpsc::UnboundedSender<Event>, k: crossterm::event::KeyEvent) {
    app.handle_event(Event::Key(k), tx).await.unwrap();
}

fn selected(app: &App) -> Option<String> {
    app.get_selected_resource().map(|r| r.id().to_string())
}

/// `⌫` / `⌃F` across a region switch: the switch is a history step, going
/// back switches region and lands on the row, and the footer hint says which
/// region it's going to.
#[tokio::test]
async fn back_and_forward_cross_region_switches() {
    let (mut app, tx, mut rx) = test_app().await;
    let home = app.current_region;
    let away = Region::ApSoutheast2;
    assert_ne!(home, away);
    app.cache.insert(ServiceType::EC2, home, None, vec![instance("i-0home")]);
    app.cache.insert(ServiceType::EC2, away, None, vec![instance("i-0away")]);
    app.current_service = Some(ServiceType::EC2);
    app.load_service_resources();
    app.update_search();
    app.selected_index = Some(0);
    assert_eq!(selected(&app).as_deref(), Some("i-0home"));

    // R → pick ap-southeast-2 → ⏎.
    press(&mut app, &tx, crossterm::event::KeyEvent::new(KeyCode::Char('R'), KeyModifiers::SHIFT)).await;
    for c in "ap-southeast-2".chars() {
        press(&mut app, &tx, key(KeyCode::Char(c))).await;
    }
    press(&mut app, &tx, key(KeyCode::Enter)).await;
    drain(&mut app, &tx, &mut rx).await;
    assert_eq!(app.current_region, away);
    app.selected_index = Some(0);
    assert_eq!(selected(&app).as_deref(), Some("i-0away"));
    assert_eq!(app.nav_back_hint().as_deref(), Some(&*format!("EC2 i-0home · {home}")));

    // ⌫: back to the home region, on the row we left.
    press(&mut app, &tx, key(KeyCode::Backspace)).await;
    drain(&mut app, &tx, &mut rx).await;
    assert_eq!(app.current_region, home);
    assert_eq!(selected(&app).as_deref(), Some("i-0home"));
    assert_eq!(app.pending_jump, None);
    assert_eq!(app.nav_forward_hint().as_deref(), Some("EC2 i-0away · ap-southeast-2"));

    // ⌃F: forward to where we went.
    press(&mut app, &tx, crossterm::event::KeyEvent::new(KeyCode::Char('f'), KeyModifiers::CONTROL)).await;
    drain(&mut app, &tx, &mut rx).await;
    assert_eq!(app.current_region, away);
    assert_eq!(selected(&app).as_deref(), Some("i-0away"));
}

/// A global service's location carries no region, so going back to it never
/// switches region; neither does a bookmark saved before regions were.
#[tokio::test]
async fn global_and_regionless_locations_stay_in_the_current_region() {
    let (mut app, tx, mut rx) = test_app().await;
    let here = app.current_region;
    app.current_service = Some(ServiceType::IAM);
    assert_eq!(app.current_location().unwrap().region, None);
    app.current_service = Some(ServiceType::EC2);
    assert_eq!(app.current_location().unwrap().region.as_deref(), Some(here.as_str()));

    let mut old = app.current_location().unwrap();
    old.region = None;
    app.jump_to_location(old, &tx);
    drain(&mut app, &tx, &mut rx).await;
    assert_eq!(app.current_region, here);
    assert_eq!(app.pending_jump_region, None);
}
