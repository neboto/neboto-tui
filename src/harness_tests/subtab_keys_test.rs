//! `H` / `L` step through sub-tabs from either pane, and `h` in the list pane
//! no longer retraces history (only `⌫` / `Ctrl-O` do).

use super::*;
use crate::app::Ec2View;

/// EC2 with one instance and one security group loaded, on the Instances tab.
async fn ec2_app() -> (App, mpsc::UnboundedSender<Event>, mpsc::UnboundedReceiver<Event>) {
    let (mut app, tx, rx) = test_app().await;
    let pick = |label: &str| {
        all_mocks()
            .into_iter()
            .find(|(s, l, _)| *s == ServiceType::EC2 && *l == label)
            .map(|(_, _, r)| r)
            .unwrap()
    };
    let instance = pick("Ec2Instance");
    let group = pick("SecurityGroup");
    select_mock(&mut app, ServiceType::EC2, instance);
    app.resources.push(group);
    app.ec2_view = Ec2View::Instances;
    app.update_search();
    (app, tx, rx)
}

fn shift(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::SHIFT)
}

#[tokio::test]
async fn h_l_step_sub_tabs_from_the_list() {
    let (mut app, tx, _rx) = ec2_app().await;
    app.handle_key(shift('L'), &tx).await.unwrap();
    assert_eq!(app.ec2_view, Ec2View::SecurityGroups);
    assert!(!app.details_focused);
    app.handle_key(shift('H'), &tx).await.unwrap();
    assert_eq!(app.ec2_view, Ec2View::Instances);
    app.handle_key(shift('H'), &tx).await.unwrap();
    assert_eq!(app.ec2_view, Ec2View::ElasticIps, "wraps like Shift-Tab");
}

#[tokio::test]
async fn h_l_from_the_detail_pane_keep_it_focused_on_the_new_tab() {
    let (mut app, tx, _rx) = ec2_app().await;
    app.handle_key(key(KeyCode::Enter), &tx).await.unwrap();
    assert!(app.details_focused);
    app.handle_key(shift('L'), &tx).await.unwrap();
    assert_eq!(app.ec2_view, Ec2View::SecurityGroups, "L is a sub-tab, not a section");
    assert!(app.details_focused, "still in the detail pane");
    assert_eq!(app.detail_section_idx, 0, "on the new resource's first section");
    assert_eq!(
        app.get_selected_resource().map(|r| r.resource_type().to_string()),
        Some("Security Group".to_string())
    );

    // A tab with nothing on it drops back to the list rather than an empty pane.
    app.handle_key(shift('L'), &tx).await.unwrap();
    assert_eq!(app.ec2_view, Ec2View::EbsVolumes);
    assert!(!app.details_focused);
}

#[tokio::test]
async fn h_l_are_text_while_searching() {
    let (mut app, tx, _rx) = ec2_app().await;
    app.handle_key(key(KeyCode::Char('/')), &tx).await.unwrap();
    app.handle_key(shift('L'), &tx).await.unwrap();
    assert_eq!(app.ec2_view, Ec2View::Instances);
    assert_eq!(app.search_query, "L");
}

#[tokio::test]
async fn h_in_the_list_does_not_pop_history_but_backspace_does() {
    let (mut app, tx, _rx) = ec2_app().await;
    app.error_message = None;
    app.handle_key(key(KeyCode::Char('h')), &tx).await.unwrap();
    assert_eq!(app.error_message, None, "h is a no-op in the list pane");
    app.handle_key(key(KeyCode::Backspace), &tx).await.unwrap();
    assert_eq!(app.error_message.as_deref(), Some("No previous location"));
}

#[tokio::test]
async fn keycast_names_the_sub_tab_not_the_reset_section() {
    let (mut app, tx, _rx) = ec2_app().await;
    app.keycast = Some(Default::default());
    app.handle_key(key(KeyCode::Enter), &tx).await.unwrap();
    app.keycast_tick();
    app.handle_key(shift('L'), &tx).await.unwrap();
    app.keycast_tick();
    let last = app.keycast.as_ref().unwrap().entries.back().cloned().unwrap();
    assert_eq!(last.key, "L");
    assert_eq!(last.action, app.active_type_filter().map(str::to_string));
}
