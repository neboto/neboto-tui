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
