//! Macro recorder + player wiring tests.
//!
//! These matter more than most: the recorder's whole job is to *not* record
//! what it was given (a cursor key becomes a row identity, five search
//! keystrokes become one step, a picker's keys become an outcome), and every
//! one of those transformations is invisible until a macro replays wrong on a
//! real account. Everything here drives the genuine `handle_event` key path
//! and then `macro_tick`, the same order the main loop uses.

use super::*;
use crate::macros::{Macro, MacroPlayer, MacroRecorder, MacroStep};

/// Start recording with a known credential baseline, bypassing the picker.
fn recorder(app: &mut App) {
    app.macro_recorder = Some(MacroRecorder {
        steps: Vec::new(),
        pending: None,
        last_region: app.current_region.as_str().to_string(),
        last_profile: None,
        last_assumed: None,
        last_s3: None,
        last_service: app.current_service,
        service_selector_was_open: false,
    });
}

fn steps(app: &App) -> Vec<MacroStep> {
    app.macro_recorder
        .as_ref()
        .map(|r| r.steps.clone())
        .unwrap_or_default()
}

/// One full main-loop turn: dispatch the key, then run the record/play pass.
async fn press(app: &mut App, tx: &mpsc::UnboundedSender<Event>, code: KeyCode) {
    app.handle_event(Event::Key(key(code)), tx).await.unwrap();
    app.macro_tick(tx);
}

/// A loaded Organizations account list, so cursor keys have somewhere to land.
fn two_accounts(app: &mut App) {
    use crate::aws::services::organizations::OrgAccount;
    let mk = |id: &str, name: &str| {
        Box::new(OrgAccount::from_sdk(
            &aws_sdk_organizations::types::Account::builder()
                .id(id)
                .arn(format!(
                    "arn:aws:organizations::111111111111:account/o-mock/{}",
                    id
                ))
                .name(name)
                .email(format!("{}@example.com", name))
                .build(),
        )) as Box<dyn Resource>
    };
    app.current_service = Some(ServiceType::Organizations);
    // The Accounts sub-tab, so the view's type filter admits these rows —
    // anything that calls `update_search` re-applies the real filter.
    app.org_view = crate::app::OrgView::Accounts;
    app.loading = false;
    app.search_active = false;
    app.search_query.clear();
    app.resources = vec![mk("111111111111", "root"), mk("222222222222", "prod")];
    app.filtered_resources = vec![0, 1];
    app.selected_index = Some(0);
    app.details_focused = false;
}

#[tokio::test]
async fn cursor_movement_records_the_row_it_landed_on_not_the_key() {
    let (mut app, tx, _rx) = test_app().await;
    two_accounts(&mut app);
    recorder(&mut app);

    press(&mut app, &tx, KeyCode::Char('j')).await;

    // `j` means "row 2 of whatever the API returned today". Recording the key
    // would pin the macro to a list position; recording the id pins it to the
    // account the user actually chose.
    match steps(&app).as_slice() {
        [MacroStep::SelectId { id, .. }] => assert_eq!(id, "222222222222"),
        other => panic!("expected one SelectId, got {:?}", other),
    }
}

#[tokio::test]
async fn a_run_of_cursor_keys_collapses_to_one_selection() {
    let (mut app, tx, _rx) = test_app().await;
    two_accounts(&mut app);
    recorder(&mut app);

    press(&mut app, &tx, KeyCode::Char('j')).await;
    press(&mut app, &tx, KeyCode::Char('k')).await;
    press(&mut app, &tx, KeyCode::Char('j')).await;

    // Three keys, one destination — replaying the intermediate hops would be
    // pure latency, and each one is a chance to abort on a missing row.
    match steps(&app).as_slice() {
        [MacroStep::SelectId { id, .. }] => assert_eq!(id, "222222222222"),
        other => panic!("expected one collapsed SelectId, got {:?}", other),
    }
}

#[tokio::test]
async fn a_search_run_records_as_one_atomic_step_and_the_opener_is_dropped() {
    let (mut app, tx, _rx) = test_app().await;
    two_accounts(&mut app);
    recorder(&mut app);

    // `@` opens the search bar pre-seeded, then the prefix is typed.
    press(&mut app, &tx, KeyCode::Char('@')).await;
    for c in "orgs".chars() {
        press(&mut app, &tx, KeyCode::Char(c)).await;
    }
    press(&mut app, &tx, KeyCode::Enter).await;

    // Replaying the six keystrokes would re-run `update_search` on every
    // partial prefix, and a partial prefix can fire a load of its own.
    assert_eq!(steps(&app), vec![MacroStep::Search("@orgs".to_string())]);
}

#[tokio::test]
async fn an_abandoned_filter_is_dropped_but_an_abandoned_service_switch_is_kept() {
    let (mut app, tx, _rx) = test_app().await;

    // Esc on a plain filter: the recording never showed a filtered list, so
    // replaying one would be wrong.
    two_accounts(&mut app);
    recorder(&mut app);
    press(&mut app, &tx, KeyCode::Char('/')).await;
    for c in "prod".chars() {
        press(&mut app, &tx, KeyCode::Char(c)).await;
    }
    press(&mut app, &tx, KeyCode::Esc).await;
    assert_eq!(steps(&app), vec![]);

    // Esc on an `@service` switch: Esc does not undo the switch, so the step
    // has to survive or the macro lands on the wrong service.
    two_accounts(&mut app);
    recorder(&mut app);
    press(&mut app, &tx, KeyCode::Char('@')).await;
    for c in "ec2".chars() {
        press(&mut app, &tx, KeyCode::Char(c)).await;
    }
    press(&mut app, &tx, KeyCode::Esc).await;
    assert_eq!(steps(&app), vec![MacroStep::Search("@ec2".to_string())]);
}

#[tokio::test]
async fn keys_pressed_inside_a_picker_are_not_recorded() {
    let (mut app, tx, _rx) = test_app().await;
    two_accounts(&mut app);
    recorder(&mut app);

    // `S` opens the service picker; the keys after it are filter text, which
    // reproduces nothing on replay.
    app.handle_event(
        Event::Key(KeyEvent::new(KeyCode::Char('S'), KeyModifiers::SHIFT)),
        &tx,
    )
    .await
    .unwrap();
    app.macro_tick(&tx);
    assert!(app.service_selector.visible);
    for c in "vpc".chars() {
        press(&mut app, &tx, KeyCode::Char(c)).await;
    }
    assert_eq!(steps(&app), vec![], "picker keys leaked into the recording");

    press(&mut app, &tx, KeyCode::Enter).await;
    // The outcome is what gets recorded — and only on the picker's closing
    // edge, so an `@service` search can't also produce one.
    assert!(
        matches!(steps(&app).as_slice(), [MacroStep::SwitchService(_)]),
        "expected a SwitchService outcome, got {:?}",
        steps(&app)
    );
}

#[tokio::test]
async fn an_assume_role_checkpoint_replaces_the_key_that_opened_it() {
    let (mut app, _tx, _rx) = test_app().await;
    two_accounts(&mut app);
    recorder(&mut app);

    // `s` on an account row fires the assume directly when one role is
    // configured, so the key lands in the step list first...
    app.macro_recorder.as_mut().unwrap().steps = vec![MacroStep::Key {
        key: "s".to_string(),
        ctrl: false,
        shift: false,
        alt: false,
    }];
    // ...and is superseded when the STS round-trip lands a pass or two later.
    // (Simulated: the real change arrives through `AwsClients`, which needs a
    // live STS call.)
    let mut fake = MacroRecorder {
        steps: std::mem::take(&mut app.macro_recorder.as_mut().unwrap().steps),
        ..Default::default()
    };
    App::drop_trailing_opener(&mut fake.steps);
    fake.steps.push(MacroStep::AssumeRole {
        account_id: "222222222222".to_string(),
        account_name: "prod".to_string(),
        role_name: "AWSControlTowerExecution".to_string(),
    });

    assert_eq!(
        fake.steps.len(),
        1,
        "the `s` opener survived alongside its own outcome: {:?}",
        fake.steps
    );
    assert!(matches!(fake.steps[0], MacroStep::AssumeRole { .. }));
}

#[tokio::test]
async fn the_tui_teardown_keys_are_never_recorded() {
    let (mut app, tx, _rx) = test_app().await;
    two_accounts(&mut app);
    app.details_focused = true;
    recorder(&mut app);

    // `e` hands the terminal to $EDITOR, which drops and rebuilds the event
    // channel the player writes to.
    press(&mut app, &tx, KeyCode::Char('e')).await;
    assert!(
        !steps(&app)
            .iter()
            .any(|s| matches!(s, MacroStep::Key { key, .. } if key == "e")),
        "recorded the editor key: {:?}",
        steps(&app)
    );
}

#[tokio::test]
async fn playback_holds_while_a_load_is_in_flight() {
    let (mut app, tx, _rx) = test_app().await;
    two_accounts(&mut app);
    app.macro_player = Some(MacroPlayer {
        name: "t".to_string(),
        steps: vec![MacroStep::SelectId {
            id: "222222222222".to_string(),
            label: "prod".to_string(),
        }],
        index: 0,
        last_step_at: None,
    });

    // The whole point of the gate: firing the next step into a list that is
    // still streaming selects the wrong row, or nothing at all.
    app.loading = true;
    app.macro_tick(&tx);
    assert_eq!(app.macro_player.as_ref().unwrap().index, 0);

    app.loading = false;
    app.macro_tick(&tx);
    assert_eq!(app.selected_index, Some(1));
}

#[tokio::test]
async fn playback_stops_when_a_recorded_row_is_gone() {
    let (mut app, tx, _rx) = test_app().await;
    two_accounts(&mut app);
    app.macro_player = Some(MacroPlayer {
        name: "audit".to_string(),
        steps: vec![
            MacroStep::SelectId {
                id: "999999999999".to_string(),
                label: "closed-account".to_string(),
            },
            MacroStep::ExitRole,
        ],
        index: 0,
        last_step_at: None,
    });

    app.macro_tick(&tx);

    // Continuing would run the remaining steps against whatever row happened
    // to be selected — silently the wrong account.
    assert!(app.macro_player.is_none(), "playback continued past a missing row");
    let err = app.error_message.clone().unwrap_or_default();
    assert!(err.contains("audit") && err.contains("closed-account"), "{}", err);
}

#[tokio::test]
async fn a_search_step_replays_in_one_shot() {
    let (mut app, tx, _rx) = test_app().await;
    two_accounts(&mut app);
    app.macro_player = Some(MacroPlayer {
        name: "t".to_string(),
        steps: vec![MacroStep::Search("prod".to_string())],
        index: 0,
        last_step_at: None,
    });

    app.macro_tick(&tx);

    assert_eq!(app.search_query, "prod");
    assert!(!app.search_active, "replay left the search bar capturing keys");
    assert!(
        !app.search_needs_update,
        "replay deferred the query instead of applying it"
    );
    assert_eq!(app.filtered_resources.len(), 1);
}

#[tokio::test]
async fn a_key_step_replays_through_the_event_channel() {
    let (mut app, tx, mut rx) = test_app().await;
    two_accounts(&mut app);
    app.macro_player = Some(MacroPlayer {
        name: "t".to_string(),
        steps: vec![MacroStep::Key {
            key: "2".to_string(),
            ctrl: false,
            shift: false,
            alt: false,
        }],
        index: 0,
        last_step_at: None,
    });

    app.macro_tick(&tx);

    // Fed back through the normal channel so the key path can't tell a
    // replayed press from a real one.
    match rx.try_recv() {
        Ok(Event::Key(k)) => assert_eq!(k.code, KeyCode::Char('2')),
        other => panic!("expected an injected Key event, got {:?}", other.is_ok()),
    }
}

#[tokio::test]
async fn playback_finishes_and_reports_rather_than_hanging() {
    let (mut app, tx, _rx) = test_app().await;
    two_accounts(&mut app);
    app.macro_player = Some(MacroPlayer {
        name: "done".to_string(),
        steps: vec![MacroStep::Search("prod".to_string())],
        index: 1, // already past the end
        last_step_at: None,
    });

    app.macro_tick(&tx);

    assert!(app.macro_player.is_none());
    assert!(app.success_message.unwrap_or_default().contains("done"));
}

#[tokio::test]
async fn the_macro_key_reaches_both_panes() {
    let (mut app, tx, _rx) = test_app().await;
    two_accounts(&mut app);

    // The list pane, via the early intercept.
    press(&mut app, &tx, KeyCode::Char(',')).await;
    assert!(app.macro_picker_visible);
    press(&mut app, &tx, KeyCode::Char(',')).await;
    assert!(!app.macro_picker_visible);

    // The detail pane. Its keymap block ends in `_ => {}` + `return`, so it
    // swallows every key it doesn't name — a global arm never gets there.
    app.details_focused = true;
    press(&mut app, &tx, KeyCode::Char(',')).await;
    assert!(
        app.macro_picker_visible,
        "`,` was swallowed by the detail-pane keymap"
    );
}

#[tokio::test]
async fn a_comma_typed_into_a_filter_stays_a_comma() {
    let (mut app, tx, _rx) = test_app().await;
    two_accounts(&mut app);

    press(&mut app, &tx, KeyCode::Char('/')).await;
    press(&mut app, &tx, KeyCode::Char('a')).await;
    press(&mut app, &tx, KeyCode::Char(',')).await;
    press(&mut app, &tx, KeyCode::Char('b')).await;

    assert!(!app.macro_picker_visible, "`,` opened the picker mid-search");
    assert_eq!(app.search_query, "a,b");
}

#[tokio::test]
async fn the_macro_key_is_left_alone_inside_an_overlay() {
    let (mut app, tx, _rx) = test_app().await;
    two_accounts(&mut app);
    // Overlays own the keymap and several have their own text inputs; `Esc`
    // out first rather than guessing which ones accept a comma.
    app.s3_object_browser.visible = true;

    press(&mut app, &tx, KeyCode::Char(',')).await;

    assert!(!app.macro_picker_visible);
}

#[tokio::test]
async fn s3_browsing_records_the_destination_not_the_navigation() {
    use crate::aws::services::s3::S3Entry;
    let obj = |key: &str| S3Entry::Object {
        key: key.to_string(),
        size: 1,
        last_modified: None,
        storage_class: "STANDARD".to_string(),
    };
    let (mut app, tx, _rx) = test_app().await;
    two_accounts(&mut app);
    recorder(&mut app);

    // Stand the browser up as if `o` had opened it and a page had landed.
    app.s3_object_browser.visible = true;
    app.s3_object_browser.bucket = "logs".to_string();
    app.s3_object_browser.set_prefix("2026/07/".to_string());
    app.s3_object_browser.entries = vec![obj("2026/07/a.log"), obj("2026/07/b.log")];
    app.s3_object_browser.selected = 0;
    app.macro_tick(&tx);

    // Moving the cursor replaces the step rather than appending: the browser's
    // index is into a filtered, sorted, partially-paginated view, so only the
    // destination is meaningful.
    app.s3_object_browser.selected = 1;
    app.macro_tick(&tx);

    match steps(&app).as_slice() {
        [MacroStep::S3Object { bucket, prefix, key }] => {
            assert_eq!(bucket, "logs");
            assert_eq!(prefix, "2026/07/");
            assert_eq!(key, "2026/07/b.log");
        }
        other => panic!("expected one collapsed S3Object, got {:?}", other),
    }

    // Closing the browser records nothing extra.
    app.s3_object_browser.visible = false;
    app.macro_tick(&tx);
    assert_eq!(steps(&app).len(), 1);
}

#[tokio::test]
async fn keys_pressed_inside_the_s3_browser_are_not_recorded() {
    let (mut app, tx, _rx) = test_app().await;
    two_accounts(&mut app);
    recorder(&mut app);
    app.s3_object_browser.visible = true;
    app.s3_object_browser.bucket = "logs".to_string();

    for c in ['j', 'j', 'k'] {
        press(&mut app, &tx, KeyCode::Char(c)).await;
    }

    // Only the location checkpoint, never the navigation keys.
    assert!(
        steps(&app)
            .iter()
            .all(|s| matches!(s, MacroStep::S3Object { .. })),
        "browser keys leaked into the recording: {:?}",
        steps(&app)
    );
}

#[tokio::test]
async fn playback_holds_while_an_object_listing_is_in_flight() {
    let (mut app, tx, _rx) = test_app().await;
    two_accounts(&mut app);
    app.macro_player = Some(MacroPlayer {
        name: "t".to_string(),
        steps: vec![MacroStep::Search("prod".to_string())],
        index: 0,
        last_step_at: None,
    });

    // The browser paginates on its own flag, not `App::loading` — the gate has
    // to know about it or the next step fires into a half-listed folder.
    app.s3_object_browser.loading = true;
    app.macro_tick(&tx);
    assert_eq!(app.macro_player.as_ref().unwrap().index, 0);

    app.s3_object_browser.loading = false;
    app.macro_tick(&tx);
    assert_eq!(app.macro_player.as_ref().unwrap().index, 1);
}

#[tokio::test]
async fn an_s3_step_replays_through_the_bookmark_restore_path() {
    let (mut app, tx, _rx) = test_app().await;
    two_accounts(&mut app);
    app.macro_player = Some(MacroPlayer {
        name: "t".to_string(),
        steps: vec![MacroStep::S3Object {
            bucket: "logs".to_string(),
            prefix: "2026/07/".to_string(),
            key: "2026/07/b.log".to_string(),
        }],
        index: 0,
        last_step_at: None,
    });

    app.macro_tick(&tx);

    assert!(app.s3_object_browser.visible);
    assert_eq!(app.s3_object_browser.bucket, "logs");
    assert_eq!(app.s3_object_browser.prefix, "2026/07/");
    // Resolved by the `S3ObjectsLoaded` handler once the page arrives — the
    // same machinery an S3 bookmark restore uses.
    assert_eq!(app.pending_s3_object_key.as_deref(), Some("2026/07/b.log"));
    assert!(app.s3_object_browser.loading, "the listing was never requested");
}

#[tokio::test]
async fn the_startup_flag_arms_a_saved_macro_by_name() {
    let (mut app, tx, _rx) = test_app().await;
    two_accounts(&mut app);
    app.macros = vec![Macro {
        name: "sh-audit".to_string(),
        steps: vec![MacroStep::Search("prod".to_string())],
    }];

    app.arm_startup_macro("sh-audit");

    // Armed exactly as the picker arms it — no separate playback path, so the
    // quiescence gate covers the startup load too.
    assert_eq!(app.macro_player.as_ref().unwrap().name, "sh-audit");
    assert_eq!(app.macro_player.as_ref().unwrap().index, 0);
    app.macro_tick(&tx);
    assert_eq!(app.search_query, "prod");
}

#[tokio::test]
async fn an_unknown_startup_macro_names_the_alternatives() {
    let (mut app, _tx, _rx) = test_app().await;
    app.macros = vec![
        Macro { name: "sh-audit".to_string(), steps: vec![] },
        Macro { name: "cost-check".to_string(), steps: vec![] },
    ];

    app.arm_startup_macro("sh-audi");

    assert!(app.macro_player.is_none());
    // A typo is the likely cause, and this is the only discovery path before
    // the TUI is up.
    let err = app.error_message.clone().unwrap_or_default();
    assert!(err.contains("sh-audit") && err.contains("cost-check"), "{}", err);
}

#[tokio::test]
async fn an_unknown_startup_macro_keeps_an_existing_startup_warning() {
    let (mut app, _tx, _rx) = test_app().await;
    app.error_message = Some("theme: unknown key `foo`".to_string());

    app.arm_startup_macro("nope");

    // The config/theme warning shares the one status-bar slot; clobbering it
    // would hide a real problem behind a CLI typo.
    let err = app.error_message.clone().unwrap_or_default();
    assert!(err.contains("theme:"), "{}", err);
    assert!(err.contains("nope"), "{}", err);
}

#[tokio::test]
async fn only_a_service_switching_macro_supersedes_the_startup_load() {
    let (mut app, _tx, _rx) = test_app().await;
    let arm = |app: &mut App, first: MacroStep| {
        app.macro_player = Some(MacroPlayer {
            name: "m".to_string(),
            steps: vec![first],
            index: 0,
            last_step_at: None,
        });
    };

    // Opens by switching service: the default service's list would be fetched
    // and thrown away.
    arm(&mut app, MacroStep::Search("@orgs".to_string()));
    assert!(app.macro_supersedes_startup_load());
    arm(&mut app, MacroStep::SwitchService(ServiceType::VPC));
    assert!(app.macro_supersedes_startup_load());

    // Opens against the startup view — skipping the load would leave the first
    // step with nothing to select.
    arm(
        &mut app,
        MacroStep::SelectId { id: "i-1".to_string(), label: "web".to_string() },
    );
    assert!(!app.macro_supersedes_startup_load());
    // A plain filter query is not a service switch.
    arm(&mut app, MacroStep::Search("prod".to_string()));
    assert!(!app.macro_supersedes_startup_load());

    app.macro_player = None;
    assert!(!app.macro_supersedes_startup_load());
}

#[tokio::test]
async fn the_recorded_walkthrough_is_replayable_end_to_end() {
    // The flow this feature was built for: org accounts → pick one → assume →
    // Security Hub. Asserted as a whole because the value is in the sequence,
    // not any single step.
    let (mut app, tx, _rx) = test_app().await;
    two_accounts(&mut app);
    recorder(&mut app);

    press(&mut app, &tx, KeyCode::Char('@')).await;
    for c in "orgs".chars() {
        press(&mut app, &tx, KeyCode::Char(c)).await;
    }
    press(&mut app, &tx, KeyCode::Enter).await;
    press(&mut app, &tx, KeyCode::Char('2')).await;
    press(&mut app, &tx, KeyCode::Char('j')).await;

    let recorded = steps(&app);
    assert_eq!(
        recorded,
        vec![
            MacroStep::Search("@orgs".to_string()),
            MacroStep::Key {
                key: "2".to_string(),
                ctrl: false,
                shift: false,
                alt: false
            },
            MacroStep::SelectId {
                id: "222222222222".to_string(),
                label: "prod".to_string()
            },
        ],
        "the walkthrough recorded as something other than search/sub-tab/select"
    );

    // Every step must survive the disk round-trip — a macro that only works
    // in the session that recorded it is not a macro.
    let saved = Macro {
        name: "sh-audit".to_string(),
        steps: recorded.clone(),
    };
    let back: Vec<Macro> =
        serde_json::from_str(&serde_json::to_string(&[saved]).unwrap()).unwrap();
    assert_eq!(back[0].steps, recorded);
}
