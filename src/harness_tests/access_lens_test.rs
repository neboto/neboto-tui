//! #246: the `N` lens lists the resource's subnet NACLs under its security
//! group rules — fetched against the demo fixtures, or from a warm VPC cache.

use super::*;
use crossterm::event::KeyModifiers;

fn screen_of(app: &App) -> String {
    let backend = TestBackend::new(190, 45);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| crate::render_app(app, f)).unwrap();
    let buf = terminal.backend().buffer().clone();
    (0..buf.area.height)
        .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The demo account with one EC2 instance (by name) selected.
async fn demo_instance(name: &str) -> (App, mpsc::UnboundedSender<Event>, mpsc::UnboundedReceiver<Event>) {
    let mut app = App::from_parts(crate::config::Config::default(), AwsClients::new_demo_for_test().await).unwrap();
    let svc = app.services[&ServiceType::EC2].clone();
    let (rows, _) = crate::headless::list_all(svc, ServiceType::EC2).await.ok().unwrap();
    let r = rows.into_iter().find(|r| r.name() == name).unwrap();
    select_mock(&mut app, ServiceType::EC2, r);
    let (tx, rx) = mpsc::unbounded_channel();
    (app, tx, rx)
}

async fn press_n(app: &mut App, tx: &mpsc::UnboundedSender<Event>, rx: &mut mpsc::UnboundedReceiver<Event>) {
    let n = crossterm::event::KeyEvent::new(KeyCode::Char('N'), KeyModifiers::SHIFT);
    app.handle_event(Event::Key(n), tx).await.unwrap();
    assert!(app.access_in_pane.is_some());
    while app.access_in_pane.as_ref().unwrap().loading {
        let ev = tokio::time::timeout(std::time::Duration::from_secs(10), rx.recv()).await.unwrap().unwrap();
        if matches!(ev, Event::AccessLensLoaded { .. }) {
            app.handle_event(ev, tx).await.unwrap();
        }
    }
}

#[tokio::test]
async fn private_subnet_acl_rows_follow_the_group_rules_in_rule_order() {
    let (mut app, tx, mut rx) = demo_instance("reporting-etl").await;
    press_n(&mut app, &tx, &mut rx).await;
    let st = app.access_in_pane.as_ref().unwrap();
    assert_eq!(st.subnet_ids, vec!["subnet-0a11prv00000001a".to_string()]);
    // The demo answers with both ACLs; only the private subnet's is kept.
    assert_eq!(st.nacls.len(), 1, "{:?}", st.nacls.iter().map(|n| &n.acl_id).collect::<Vec<_>>());
    assert!(st.missing_subnets().is_empty());
    // Inbound by default, rule order (not port order), the catch-all as `*`.
    let rules: Vec<_> = st.nacl_rows.iter().map(|r| (r.rule.as_str(), r.allow)).collect();
    assert_eq!(rules, [("90", false), ("100", true), ("110", true), ("*", false)]);
    // Selection runs on past the group rules into the ACL's.
    let first_nacl = st.rows.len();
    let screen = screen_of(&app);
    assert!(screen.contains("1 NACL deny rule"), "{screen}");
    let ssh = screen.lines().find(|l| l.contains("NACL #90")).unwrap_or_default();
    assert!(ssh.contains("deny") && ssh.contains("acl-0acme00private01"), "{screen}");

    app.access_in_pane.as_mut().unwrap().selected = first_nacl;
    assert!(app.access_in_pane.as_ref().unwrap().selected_text().unwrap().contains("NACL acl-0acme00private01 #90 deny"));
    // ⏎ jumps to the ACL in @vpc.
    app.handle_event(Event::Key(crossterm::event::KeyEvent::from(KeyCode::Enter)), &tx).await.unwrap();
    assert!(app.access_in_pane.is_none());
    assert_eq!(app.current_service, Some(ServiceType::VPC));
    assert_eq!(app.pending_jump.as_deref(), Some("acl-0acme00private01"));
}

#[tokio::test]
async fn default_acl_reads_as_allow_all_and_a_warm_vpc_cache_skips_the_fetch() {
    let (mut app, tx, mut rx) = demo_instance("web-1").await;
    // Warm the VPC cache with the demo's ACLs: the lens then needs no
    // DescribeNetworkAcls (only the security groups are fetched).
    let vpc = app.services[&ServiceType::VPC].clone();
    let (acls, _) = crate::headless::list_all(vpc, ServiceType::VPC).await.ok().unwrap();
    assert!(acls.iter().any(|r| r.resource_type() == "NetworkAcl"));
    app.cache.insert(ServiceType::VPC, app.current_region, None, acls);
    let n = crossterm::event::KeyEvent::new(KeyCode::Char('N'), KeyModifiers::SHIFT);
    app.handle_event(Event::Key(n), &tx).await.unwrap();
    // Resolved on open, before any fetch result is handled.
    assert_eq!(app.access_in_pane.as_ref().unwrap().nacls.len(), 1);
    assert!(app.access_in_pane.as_ref().unwrap().missing_subnets().is_empty());
    while app.access_in_pane.as_ref().unwrap().loading {
        let ev = tokio::time::timeout(std::time::Duration::from_secs(10), rx.recv()).await.unwrap().unwrap();
        if matches!(ev, Event::AccessLensLoaded { .. }) {
            app.handle_event(ev, &tx).await.unwrap();
        }
    }

    let st = app.access_in_pane.as_ref().unwrap();
    assert_eq!(st.nacls.len(), 1);
    assert_eq!(st.nacls[0].acl_id, "acl-0acme00default01");
    assert!(crate::ui::widgets::access_lens::nacl_allows_all(&st.nacls[0]));
    assert!(st.nacl_error.is_none());
    let screen = screen_of(&app);
    assert!(screen.contains("acl-0acme00default01 allow-all · 1 subnet"), "{screen}");
    assert!(!screen.contains("NACL deny rule"), "the catch-all `*` isn't a deny worth flagging: {screen}");
}
