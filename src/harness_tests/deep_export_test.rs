//! Deep export (`X` over a list visual selection) wiring.
//!
//! The failure this guards against is invisible from the code: the export
//! *arms* on the keypress and *finishes* a main-loop iteration later, once the
//! lazy detail sections it fired have landed. It used to demand a second `X`
//! press, which on a real org (dozens of accounts behind a throttled API) is
//! indistinguishable from the key doing nothing — the toast reads the same
//! whether one row or forty are still loading, and it expires while you wait.

use super::*;
use crate::aws::services::organizations::{OrgAccount, OrgAccountDetails};

fn account_id(i: usize) -> String {
    format!("{:012}", i + 1)
}

/// A loaded Organizations Accounts tab with `n` rows.
fn accounts(app: &mut App, n: usize) {
    let mk = |id: String, name: String| {
        Box::new(OrgAccount::from_sdk(
            &aws_sdk_organizations::types::Account::builder()
                .id(&id)
                .arn(format!(
                    "arn:aws:organizations::111111111111:account/o-mock/{}",
                    id
                ))
                .name(&name)
                .email(format!("{}@example.com", name))
                .build(),
        )) as Box<dyn Resource>
    };
    app.current_service = Some(ServiceType::Organizations);
    app.org_view = crate::app::OrgView::Accounts;
    app.loading = false;
    app.search_active = false;
    app.search_query.clear();
    app.resources = (0..n)
        .map(|i| mk(account_id(i), format!("acct{}", i)))
        .collect();
    app.filtered_resources = (0..n).collect();
    app.selected_index = Some(0);
    app.details_focused = false;
}

/// Land the lazy per-account details the export is waiting on.
fn land_details(app: &mut App, n: usize) {
    for i in 0..n {
        app.lazy.org_account_details.apply(
            account_id(i),
            Ok(OrgAccountDetails {
                ou_path: vec![("Root".into(), "r-abcd".into())],
                ou_path_error: None,
                policies: vec![],
                policies_error: None,
            }),
        );
    }
}

async fn press(app: &mut App, tx: &mpsc::UnboundedSender<Event>, code: KeyCode, m: KeyModifiers) {
    app.handle_event(Event::Key(KeyEvent::new(code, m)), tx)
        .await
        .unwrap();
}

#[tokio::test]
async fn select_all_then_x_exports_every_row_without_a_second_press() {
    let (mut app, tx, _rx) = test_app().await;
    accounts(&mut app, 5);

    press(&mut app, &tx, KeyCode::Char('a'), KeyModifiers::CONTROL).await;
    assert_eq!(
        (app.list_visual_anchor, app.selected_index),
        (Some(0), Some(4)),
        "Ctrl-A must select every visible row"
    );

    press(&mut app, &tx, KeyCode::Char('X'), KeyModifiers::SHIFT).await;
    // The press arms the export and fires the lazy triggers; nothing is
    // written yet, and the status line says what it's waiting for.
    assert!(app.pending_deep_export.is_some(), "X should arm the export");
    assert!(
        app.deep_export_progress
            .as_deref()
            .is_some_and(|p| p.contains("0/5")),
        "progress should count what's ready, got {:?}",
        app.deep_export_progress
    );
    assert!(app.success_message.is_none(), "no toast until it exports");

    // A tick with the fetches still outstanding must not export or give up.
    app.deep_export_tick(&tx);
    assert!(app.pending_deep_export.is_some());

    land_details(&mut app, 5);
    app.deep_export_tick(&tx);

    assert!(
        app.pending_deep_export.is_none() && app.deep_export_progress.is_none(),
        "the export should complete once the sections land"
    );
    assert_eq!(app.error_message, None);
    let msg = app.success_message.clone().unwrap_or_default();
    assert!(
        msg.starts_with("Exported 5 "),
        "all five rows should be exported, got {msg:?}"
    );
    assert!(
        app.list_visual_anchor.is_none(),
        "a completed export clears the selection"
    );
}

#[tokio::test]
async fn a_pending_deep_export_survives_the_selection_being_rebuilt() {
    // The visual selection is positional and dies on any `update_search`; the
    // armed export holds ids instead, so the wait outlives it.
    let (mut app, tx, _rx) = test_app().await;
    accounts(&mut app, 3);

    press(&mut app, &tx, KeyCode::Char('a'), KeyModifiers::CONTROL).await;
    press(&mut app, &tx, KeyCode::Char('X'), KeyModifiers::SHIFT).await;
    app.update_search();
    assert!(app.list_visual_anchor.is_none(), "rebuild drops the anchor");
    assert!(app.pending_deep_export.is_some(), "but not the export");

    land_details(&mut app, 3);
    app.deep_export_tick(&tx);
    assert!(app
        .success_message
        .as_deref()
        .is_some_and(|m| m.starts_with("Exported 3 ")));
}

#[tokio::test]
async fn esc_cancels_an_armed_deep_export() {
    let (mut app, tx, _rx) = test_app().await;
    accounts(&mut app, 3);

    press(&mut app, &tx, KeyCode::Char('a'), KeyModifiers::CONTROL).await;
    press(&mut app, &tx, KeyCode::Char('X'), KeyModifiers::SHIFT).await;
    // Esc must reach the cancel even after the anchor is gone.
    app.update_search();
    press(&mut app, &tx, KeyCode::Esc, KeyModifiers::NONE).await;

    assert!(app.pending_deep_export.is_none());
    assert!(app.deep_export_progress.is_none());

    land_details(&mut app, 3);
    app.deep_export_tick(&tx);
    assert!(
        app.success_message.is_none(),
        "a cancelled export must not fire later"
    );
}

#[tokio::test]
async fn ctrl_a_is_not_eaten_by_the_hide_noise_toggle() {
    // `a` toggles hide-noise on screens that have noise rows; the arm used to
    // ignore modifiers, so `Ctrl-A` toggled the filter (and `update_search`
    // then cleared the selection) instead of selecting all.
    let (mut app, tx, _rx) = test_app().await;
    accounts(&mut app, 3);
    app.noise_in_view = true;
    let hide_noise = app.hide_noise;

    press(&mut app, &tx, KeyCode::Char('a'), KeyModifiers::CONTROL).await;

    assert_eq!(app.hide_noise, hide_noise, "Ctrl-A is not the noise toggle");
    assert_eq!(app.list_visual_anchor, Some(0));
    assert_eq!(app.selected_index, Some(2));

    // Plain `a` still toggles it.
    press(&mut app, &tx, KeyCode::Char('a'), KeyModifiers::NONE).await;
    assert_eq!(app.hide_noise, !hide_noise);
}
