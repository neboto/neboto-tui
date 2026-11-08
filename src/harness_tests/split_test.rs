//! `|` splits out: another neboto in a pane beside this one, started at the
//! same place with `--at`.

use super::*;
use crate::split::Host;

/// EC2 with an instance and a security group loaded, the instance selected.
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
    (app, tx, rx)
}

/// The `--at` value out of a launched shell command.
fn place_in(cmd: &str) -> crate::app::NavLocation {
    let at = cmd.split_whitespace().skip_while(|w| *w != "--at").nth(1).expect("--at in the command");
    crate::split::decode_place(at.trim_end_matches(';')).unwrap()
}

#[tokio::test]
async fn bar_splits_a_tmux_pane_at_this_section_and_the_new_pane_lands_there() {
    let (mut app, tx, _rx) = ec2_app().await;
    let id = app.get_selected_resource().unwrap().id().to_string();
    app.handle_key(key(KeyCode::Enter), &tx).await.unwrap();
    app.handle_key(key(KeyCode::Char('2')), &tx).await.unwrap();
    assert!(app.details_focused);
    let section = app.detail_section_idx;

    crate::split::set_test_host(Some(Host::Tmux));
    app.handle_key(key(KeyCode::Char('|')), &tx).await.unwrap();
    crate::split::set_test_host(None);

    let launched = crate::split::take_launched();
    assert_eq!(launched.len(), 1, "{:?}", app.error_message);
    let (prog, args) = &launched[0];
    assert_eq!(prog, "tmux");
    assert_eq!(&args[..2], ["split-window", "-h"]);
    let cmd = args.last().unwrap();
    assert!(cmd.contains(" -s @ec2 "), "{cmd}");
    assert!(cmd.contains(&format!(" -r {} ", app.current_region.as_str())), "{cmd}");
    assert!(app.success_message.as_deref().is_some_and(|m| m.contains("tmux")), "{:?}", app.success_message);

    let loc = place_in(cmd);
    assert_eq!(loc.selected_id.as_deref(), Some(id.as_str()));
    assert!(loc.details_focused);

    // The new pane: the same list, on another row and in the list pane,
    // until the startup restore runs.
    let (mut other, tx2, _rx2) = ec2_app().await;
    other.selected_index = Some(1);
    other.startup_location = Some(loc);
    other.restore_startup_location(&tx2);
    assert_eq!(other.get_selected_resource().map(|r| r.id().to_string()), Some(id));
    assert!(other.details_focused);
    assert_eq!(other.detail_section_idx, section);
}

#[tokio::test]
async fn with_nothing_to_split_the_command_is_copied() {
    let (mut app, tx, _rx) = ec2_app().await;
    crate::split::set_test_host(None);
    app.handle_key(key(KeyCode::Char('|')), &tx).await.unwrap();
    assert!(crate::split::take_launched().is_empty());
    let copied = app.clipboard_last().expect("the command is copied").to_string();
    assert!(copied.contains("--at "), "{copied}");
    assert!(place_in(&copied).selected_id.is_some());
}

/// Every service with sub-tabs records which one it was on, or a split
/// pane (and Ctrl-O, and a bookmark) lands on its first tab instead —
/// `@orgs net` on Accounts came back as no match on Overview.
#[tokio::test]
async fn every_sub_tabbed_service_records_its_sub_tab() {
    // No `JumpView` variant yet. Toggle rows (S3 objects, Cost, CloudTrail
    // scope, Service Quotas, RAM, X-Ray window) aren't tabs; the rest are,
    // and each needs checking that its tab doesn't fetch on entry before it
    // gets one. Don't add to this list.
    use ServiceType::*;
    const NOT_YET: &[ServiceType] = &[
        CloudTrail, S3, Cost, Config, TrustedAdvisor, ServiceQuotas, GuardDuty, Cognito,
        Backup, Fsx, Ram, Kinesis, Bedrock, Athena, Glue, Ses, Fms, Redshift, XRay, Changes,
        ControlTower, ServiceCatalog,
    ];
    let (mut app, _tx, _rx) = test_app().await;
    let missing: Vec<ServiceType> = ServiceType::all()
        .into_iter()
        .filter(|s| crate::app::has_sub_tabs(*s) && !NOT_YET.contains(s))
        .filter(|s| {
            app.current_service = Some(*s);
            matches!(app.current_jump_view(), crate::app::JumpView::None)
        })
        .collect();
    assert_eq!(missing, Vec::<ServiceType>::new(), "add a JumpView arm to current_jump_view");
}

/// Split one tmux pane and hand back the place it was given.
async fn split_place(app: &mut App, tx: &mpsc::UnboundedSender<Event>) -> crate::app::NavLocation {
    crate::split::set_test_host(Some(Host::Tmux));
    app.handle_key(key(KeyCode::Char('|')), tx).await.unwrap();
    crate::split::set_test_host(None);
    let launched = crate::split::take_launched();
    assert_eq!(launched.len(), 1, "{:?}", app.error_message);
    place_in(launched[0].1.last().unwrap())
}

#[tokio::test]
async fn bar_from_the_charts_reopens_them_at_their_range() {
    use crate::aws::services::ec2::MetricsTimeRange;
    let (mut app, tx, _rx) = ec2_app().await;
    app.handle_key(key(KeyCode::Char('m')), &tx).await.unwrap();
    assert_eq!(app.metrics_in_pane, Some(crate::app::MetricsKind::Spec));
    // 1h → the next range up, so it can't be the default it opens on.
    app.handle_key(key(KeyCode::Char(']')), &tx).await.unwrap();
    let range = app.spec_metrics.as_ref().unwrap().range;
    assert_ne!(range, MetricsTimeRange::OneHour);

    let loc = split_place(&mut app, &tx).await;
    assert_eq!(loc.lens, Some(crate::app::NavLens::Metrics { range: Some(range) }));

    let (mut other, tx2, _rx2) = ec2_app().await;
    other.startup_location = Some(loc);
    other.restore_startup_location(&tx2);
    assert_eq!(other.metrics_in_pane, Some(crate::app::MetricsKind::Spec));
    assert_eq!(other.spec_metrics.as_ref().map(|m| m.range), Some(range));
}

#[tokio::test]
async fn bar_in_a_filter_being_typed_is_a_character() {
    let (mut app, tx, _rx) = ec2_app().await;
    app.log_tail.visible = true;
    app.log_tail.filter_active = true;
    crate::split::set_test_host(Some(Host::Tmux));
    app.handle_key(key(KeyCode::Char('|')), &tx).await.unwrap();
    crate::split::set_test_host(None);
    assert!(crate::split::take_launched().is_empty());
    assert_eq!(app.log_tail.filter, "|");
}

/// A place on a folder in the object browser (a split, or a folder
/// bookmark) opens the listing the folder is in, on the folder — not the
/// folder's own contents.
#[tokio::test]
async fn an_object_browser_place_on_a_folder_opens_its_parent_listing() {
    let (mut app, tx, _rx) = test_app().await;
    let loc = crate::app::NavLocation {
        service: ServiceType::S3,
        view: crate::app::JumpView::None,
        query: String::new(),
        selected_id: Some("logs".into()),
        label: String::new(),
        s3_object_path: Some(("logs".into(), "AWSLogs/2023/02/".into())),
        details_focused: false,
        detail_section: None,
        region: None,
        credentials: None,
        lens: None,
    };
    app.startup_location = Some(loc);
    app.restore_startup_location(&tx);
    assert!(app.s3_object_browser.visible);
    assert_eq!(app.s3_object_browser.prefix, "AWSLogs/2023/");
    assert_eq!(app.pending_s3_object_key.as_deref(), Some("AWSLogs/2023/02/"));
}

#[tokio::test]
async fn kitty_without_remote_control_opens_a_window_and_says_why() {
    let (mut app, tx, _rx) = ec2_app().await;
    crate::split::set_test_host(Some(Host::Kitty));
    crate::split::set_test_failing(&["kitten"]);
    app.handle_key(key(KeyCode::Char('|')), &tx).await.unwrap();
    crate::split::set_test_failing(&[]);
    crate::split::set_test_host(None);
    let progs: Vec<String> = crate::split::take_launched().into_iter().map(|(p, _)| p).collect();
    assert_eq!(progs, ["kitten", "kitty"]);
    let msg = app.success_message.clone().unwrap_or_default();
    assert!(msg.starts_with("Split out into a new kitty window") && msg.contains("no split: kitten"), "{msg}");
}

#[tokio::test]
async fn when_every_launch_fails_the_reason_shows_and_the_command_is_copied() {
    let (mut app, tx, _rx) = ec2_app().await;
    crate::split::set_test_host(Some(Host::Ghostty));
    crate::split::set_test_failing(&["osascript", "ghostty"]);
    app.handle_key(key(KeyCode::Char('|')), &tx).await.unwrap();
    crate::split::set_test_failing(&[]);
    crate::split::set_test_host(None);
    assert!(!crate::split::take_launched().is_empty());
    let err = app.error_message.clone().unwrap_or_default();
    assert!(err.starts_with("Can't split:") && err.contains("copied"), "{err}");
    assert!(app.clipboard_last().is_some_and(|c| c.contains("--at ")));
}

#[tokio::test]
async fn split_command_beats_the_detected_terminal() {
    let (mut app, tx, _rx) = ec2_app().await;
    app.split_command = Some("my-term --right -- {cmd}".into());
    crate::split::set_test_host(Some(Host::Tmux));
    app.handle_key(key(KeyCode::Char('|')), &tx).await.unwrap();
    crate::split::set_test_host(None);
    let launched = crate::split::take_launched();
    assert_eq!(launched.len(), 1);
    assert_eq!(launched[0].0, "sh");
    assert!(launched[0].1[1].starts_with("my-term --right -- '"), "{:?}", launched[0].1);
    assert!(place_in(&launched[0].1[1]).selected_id.is_some());
}
