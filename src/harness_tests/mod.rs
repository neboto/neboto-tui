//! Layer-2 TUI wiring tests — no emulator, no network, plain `cargo test`.
//!
//! The `App` is built offline (`App::new_for_test`: default config + clients
//! pinned to a dead localhost endpoint) and mock resources are injected
//! directly, so these tests exercise the wiring the unit tests can't reach:
//!
//! - **Registration parity**: every `ServiceType` has a `build_services`
//!   entry, before and after `recreate_services` (the region/profile-switch
//!   path).
//! - **Section invariants**: for every split-pane resource type, the digit
//!   keys, the `Tab`/`Shift-Tab` cycle, and the `walk!` snapshot must agree on
//!   section order, and the post-focus default must be the first section —
//!   the invariant CLAUDE.md otherwise enforces by prose.
//! - **Keymap smash**: drive the full detail-pane keymap over every mock and
//!   render each frame to a `TestBackend` — renderer panics and
//!   index-out-of-bounds bugs surface without a terminal or an AWS account.
//!
//! Mocks live in the `mocks_*` sibling files (one per service batch), each
//! exposing `mocks() -> Vec<Mock>`. A new split pane needs a mock here or the
//! coverage silently shrinks — add one alongside the `walk!` entry.

use super::*;
use crate::aws::resource::Resource;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

mod deep_export_test;
mod load_stream_test;
mod macros_test;
mod state_filter_test;
mod mocks_compute;
mod mocks_containers;
mod mocks_ai;
mod mocks_data;
mod mocks_edge;
mod mocks_identity;
mod mocks_ops;
mod mocks_security;

/// (owning service, concrete-type label for failure messages, the resource)
pub(crate) type Mock = (ServiceType, &'static str, Box<dyn Resource>);

fn all_mocks() -> Vec<Mock> {
    let mut out = Vec::new();
    out.extend(mocks_compute::mocks());
    out.extend(mocks_containers::mocks());
    out.extend(mocks_ai::mocks());
    out.extend(mocks_data::mocks());
    out.extend(mocks_edge::mocks());
    out.extend(mocks_identity::mocks());
    out.extend(mocks_ops::mocks());
    out.extend(mocks_security::mocks());
    // Guard against the registry silently shrinking (a batch file returning
    // an empty Vec after a bad merge). 140 walk! types were covered when this
    // was written — bump the floor when panes are added, never lower it.
    assert!(
        out.len() >= 163,
        "mock registry shrank to {} entries — a mocks_* batch file lost coverage",
        out.len()
    );
    out
}

async fn test_app() -> (
    App,
    mpsc::UnboundedSender<Event>,
    mpsc::UnboundedReceiver<Event>,
) {
    // The keymap smash presses the export keys — divert the files they write
    // into a temp dir so test runs never litter the working directory.
    let export_dir = std::env::temp_dir().join("neboto-test-exports");
    let _ = std::fs::create_dir_all(&export_dir);
    std::env::set_var("NEBOTO_EXPORT_DIR", &export_dir);

    let app = App::new_for_test().await;
    let (tx, rx) = mpsc::unbounded_channel();
    (app, tx, rx)
}

/// Install a single mock as the loaded + selected resource, bypassing the
/// fetch path. `filtered_resources` is set directly so the current sub-tab
/// view's type filter can't hide the mock; anything that legitimately calls
/// `update_search` afterwards re-applies the real filter (which is part of
/// what the tests observe).
fn select_mock(app: &mut App, service: ServiceType, resource: Box<dyn Resource>) {
    app.current_service = Some(service);
    app.search_active = false;
    app.search_query.clear();
    app.all_search_mode = false;
    app.all_search_sources.clear();
    app.loading = false;
    app.resources = vec![resource];
    app.filtered_resources = vec![0];
    app.selected_index = Some(0);
    app.details_focused = false;
    app.details_selected_index = None;
    app.detail_flat_mode = false;
    app.layout_mode = LayoutMode::Split;
    app.metrics_in_pane = None;
    app.trail_in_pane = None;
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn section_names(sections: &[(String, Vec<(String, String)>)]) -> Vec<String> {
    sections.iter().map(|(n, _)| n.clone()).collect()
}

/// The generic checker: digit keys, Tab/Shift-Tab cycle, and the `walk!`
/// snapshot must all agree on section order; the post-focus default must be
/// the first section. Flat resources (0–1 sections) pass trivially.
async fn check_section_invariants(
    app: &mut App,
    tx: &mpsc::UnboundedSender<Event>,
    label: &str,
) {
    app.focus_details_panel(tx);
    let (sections, active) = app.detail_sections_snapshot_with_active();
    if sections.len() <= 1 {
        return;
    }
    let names = section_names(&sections);
    assert_eq!(
        active,
        Some(0),
        "{label}: default section after focus must be the first walk! section (sections: {names:?})"
    );

    // Digit keys 1..=9 must select walk! sections in order.
    for (i, name) in names.iter().enumerate().take(9) {
        let ch = char::from_digit(i as u32 + 1, 10).unwrap();
        app.handle_key(key(KeyCode::Char(ch)), tx).await.unwrap();
        let (s, a) = app.detail_sections_snapshot_with_active();
        assert_eq!(
            section_names(&s),
            names,
            "{label}: section list changed while pressing digit keys"
        );
        assert_eq!(
            a,
            Some(i),
            "{label}: digit '{ch}' should land on section {name:?} (walk! order {names:?})"
        );
    }

    // Tab must cycle forward in walk! order and wrap.
    app.focus_details_panel(tx);
    for step in 1..=names.len() {
        app.cycle_detail_section_next(tx);
        let (_, a) = app.detail_sections_snapshot_with_active();
        assert_eq!(
            a,
            Some(step % names.len()),
            "{label}: Tab cycle step {step} diverged from walk! order {names:?}"
        );
    }

    // Shift-Tab from the default must wrap to the last section.
    app.focus_details_panel(tx);
    app.cycle_detail_section_prev(tx);
    let (_, a) = app.detail_sections_snapshot_with_active();
    assert_eq!(
        a,
        Some(names.len() - 1),
        "{label}: Shift-Tab from the default should wrap to the last section"
    );
}

#[tokio::test]
async fn every_service_type_is_registered() {
    let (mut app, _tx, _rx) = test_app().await;
    for st in ServiceType::all() {
        assert!(
            app.services.contains_key(&st),
            "{st:?} missing from build_services at startup"
        );
    }
    app.recreate_services();
    for st in ServiceType::all() {
        assert!(
            app.services.contains_key(&st),
            "{st:?} missing from build_services after recreate_services \
             (region/profile switch would silently lose it)"
        );
    }
}

/// Every mock resource type must be reachable through some sub-tab of its
/// owning service — `align_view_to_resource_type` (the `@all` jump landing)
/// must find a view whose type filter lists it. A tabbed service whose
/// `align` arm is missing a variant (or a new tab added without one) fails
/// here instead of silently stranding `@all` jumps on the default tab.
#[tokio::test]
async fn every_resource_type_lands_on_a_sub_tab() {
    let (mut app, _tx, _rx) = test_app().await;
    for (service, label, mock) in all_mocks() {
        app.current_service = Some(service);
        app.all_search_mode = false;
        let rtype = mock.resource_type().to_string();
        app.align_view_to_resource_type(&rtype);
        if let Some(filter) = app.active_type_filter() {
            assert!(
                type_filter_matches(filter, &rtype),
                "{label}: no sub-tab of {service:?} lists resource type {rtype:?} — \
                 an @all jump to it would strand on the default tab \
                 (align_view_to_resource_type missing the variant?)"
            );
        }
    }
}

#[tokio::test]
async fn split_pane_section_wiring_matches_walk_order() {
    let (mut app, tx, _rx) = test_app().await;
    for (service, label, mock) in all_mocks() {
        select_mock(&mut app, service, mock);
        check_section_invariants(&mut app, &tx, label).await;
    }
}

/// A type declaring `detail_sections()` also needs a downcast arm in
/// `render_details_pane` — the dispatch is a hand-written chain, and a type
/// missing from it renders through the generic flat path instead: the
/// descriptor, the section renderer and its lazy hooks all exist and none of
/// them are ever reached. That's invisible to every other test (nothing
/// panics, the section walk still agrees with itself), and it shipped once —
/// Security Hub's Insights pane declared three sections and rendered three
/// flat rows.
///
/// The tab bar is the tell: only the split-pane skeleton draws it. Render
/// wide enough that the chip strip can't window any label away, then require
/// every section's label on screen.
#[tokio::test]
async fn every_split_pane_type_reaches_a_split_renderer() {
    let (mut app, tx, _rx) = test_app().await;
    for (service, label, mock) in all_mocks() {
        let Some(desc) = mock.detail_sections() else {
            continue; // Deliberately flat — nothing to dispatch to.
        };
        let labels: Vec<&str> = desc.sections.iter().map(|s| s.label).collect();
        select_mock(&mut app, service, mock);
        app.details_focused = true;
        app.reset_detail_section_to_default(&tx);

        let backend = TestBackend::new(300, 60);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| crate::render_app(&app, f))
            .unwrap_or_else(|e| panic!("{label}: draw failed: {e}"));

        let screen: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        for section in &labels {
            assert!(
                screen.contains(section),
                "{label}: section “{section}” never rendered — \
                 `render_details_pane` is probably missing a downcast arm for \
                 this type, so it falls through to the flat `details()` view",
            );
        }
    }
}

/// Every key a detail pane responds to, in an order that opens and then
/// closes each overlay so later keys aren't swallowed.
fn smash_sequence() -> Vec<KeyEvent> {
    let mut keys: Vec<KeyEvent> = Vec::new();
    keys.push(key(KeyCode::Char('l'))); // drill into the detail pane
    for d in '1'..='9' {
        keys.push(key(KeyCode::Char(d)));
    }
    keys.extend([
        key(KeyCode::Tab),
        key(KeyCode::BackTab),
        key(KeyCode::Char('j')),
        key(KeyCode::Char('j')),
        key(KeyCode::Char('k')),
        key(KeyCode::Char('G')),
        key(KeyCode::Char('g')),
        key(KeyCode::Char('g')), // gg
        key(KeyCode::Char('[')),
        key(KeyCode::Char('[')), // [[
        key(KeyCode::Char(']')),
        key(KeyCode::Char(']')), // ]]
        key(KeyCode::Char('V')), // visual mode
        key(KeyCode::Char('j')),
        key(KeyCode::Char('y')),
        key(KeyCode::Esc),
        key(KeyCode::Char('\\')), // flat view on
        key(KeyCode::Char('j')),
        key(KeyCode::Tab),
        key(KeyCode::Char('\\')), // flat view off
        key(KeyCode::Char('m')), // metrics overlay (loads fail fast offline)
        key(KeyCode::Char('[')),
        key(KeyCode::Char(']')),
        key(KeyCode::Esc),
        key(KeyCode::Char('W')), // CloudTrail lens
        key(KeyCode::Esc),
        key(KeyCode::Char('U')), // referenced-by lens (cache walk, zero API)
        key(KeyCode::Char('f')),
        key(KeyCode::Char('j')),
        key(KeyCode::Esc),
        key(KeyCode::Char('N')), // network-access lens (fetch fails fast offline)
        key(KeyCode::Char('t')),
        key(KeyCode::Char('j')),
        key(KeyCode::Esc),
        key(KeyCode::Char('Z')), // full-width toggle
        key(KeyCode::Char('Z')),
        key(KeyCode::Char('X')), // export
        key(KeyCode::Char('h')), // back to list
        key(KeyCode::Char('z')), // sort cycle
        key(KeyCode::Char('F')), // state filter cycle
        key(KeyCode::Char('a')), // noise toggle
        key(KeyCode::Char('a')),
        // List-pane visual selection: toggle, extend, copy, select-all,
        // deep-export (mock lazy sections leave it on the press-again toast
        // for split panes; flat mocks export), cancel.
        key(KeyCode::Char('V')),
        key(KeyCode::Char('j')),
        key(KeyCode::Char('J')),
        key(KeyCode::Char('K')),
        key(KeyCode::Char('y')),
        KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL),
        key(KeyCode::Char('X')),
        key(KeyCode::Esc),
        key(KeyCode::Esc),
    ]);
    keys
}

#[tokio::test]
async fn keymap_smash_never_panics() {
    let (mut app, tx, _rx) = test_app().await;
    for (service, label, mock) in all_mocks() {
        select_mock(&mut app, service, mock);
        let backend = TestBackend::new(140, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        app.flat_detail_tick(&tx);
        terminal
            .draw(|f| crate::render_app(&app, f))
            .unwrap_or_else(|e| panic!("{label}: initial draw failed: {e}"));
        for k in smash_sequence() {
            app.handle_key(k, &tx)
                .await
                .unwrap_or_else(|e| panic!("{label}: key {k:?} errored: {e}"));
            app.flat_detail_tick(&tx);
            terminal
                .draw(|f| crate::render_app(&app, f))
                .unwrap_or_else(|e| panic!("{label}: draw after {k:?} failed: {e}"));
        }
    }
}

/// List-pane visual selection: range math, select-all, the copy verb's
/// selection-awareness, and the positional cancellation rules (Esc, list
/// rebuild, drill-in).
#[tokio::test]
async fn list_visual_selection_selects_copies_and_cancels() {
    let (mut app, tx, _rx) = test_app().await;
    let (service, _, mock) = all_mocks().into_iter().next().unwrap();
    select_mock(&mut app, service, mock.clone_box());
    // Three identical rows are enough to exercise the range.
    app.resources = vec![mock.clone_box(), mock.clone_box(), mock.clone_box()];
    app.filtered_resources = vec![0, 1, 2];
    app.selected_index = Some(0);

    // V anchors at the cursor; plain j extends the range downward.
    app.handle_key(key(KeyCode::Char('V')), &tx).await.unwrap();
    assert_eq!(app.list_visual_range(), Some((0, 0)));
    app.handle_key(key(KeyCode::Char('j')), &tx).await.unwrap();
    assert_eq!(app.list_visual_range(), Some((0, 1)));
    assert!(app.list_row_in_selection(1));
    assert!(!app.list_row_in_selection(2));

    // Esc cancels.
    app.handle_key(key(KeyCode::Esc), &tx).await.unwrap();
    assert_eq!(app.list_visual_range(), None);

    // Ctrl-A selects every visible row.
    app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL), &tx)
        .await
        .unwrap();
    assert_eq!(app.list_visual_range(), Some((0, 2)));

    // `y` operates on the selection (Markdown table copy) and drops it.
    // Clipboard may be unavailable headless — only selection state is asserted.
    app.handle_key(key(KeyCode::Char('y')), &tx).await.unwrap();
    assert_eq!(app.list_visual_range(), None);

    // Any list rebuild (sort, state filter, query edits — all funnel through
    // update_search) cancels the positional selection.
    app.handle_key(key(KeyCode::Char('V')), &tx).await.unwrap();
    assert!(app.list_visual_range().is_some());
    app.update_search();
    assert_eq!(app.list_visual_range(), None);

    // Drill-in cancels it too.
    app.selected_index = Some(0);
    app.handle_key(key(KeyCode::Char('V')), &tx).await.unwrap();
    app.handle_key(key(KeyCode::Enter), &tx).await.unwrap();
    assert!(app.details_focused);
    assert_eq!(app.list_visual_range(), None);
}

/// The deep-export size guard: `Ctrl-A` + `X` on an oversized list refuses
/// with the cap message and keeps the selection so it can be narrowed.
#[tokio::test]
async fn deep_export_cap_refuses_oversized_selections() {
    let (mut app, tx, _rx) = test_app().await;
    let (service, _, mock) = all_mocks().into_iter().next().unwrap();
    select_mock(&mut app, service, mock.clone_box());
    app.resources = (0..51).map(|_| mock.clone_box()).collect();
    app.filtered_resources = (0..51).collect();
    app.selected_index = Some(0);

    app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL), &tx)
        .await
        .unwrap();
    assert_eq!(app.list_visual_range(), Some((0, 50)));
    app.handle_key(key(KeyCode::Char('X')), &tx).await.unwrap();
    let err = app.error_message.clone().unwrap_or_default();
    assert!(err.contains("capped"), "expected cap refusal, got: {err:?}");
    assert_eq!(
        app.list_visual_range(),
        Some((0, 50)),
        "the selection must survive a cap refusal"
    );
}

/// Manual refresh must drop the lazy detail-section store — LazyMap's
/// contains-guard otherwise serves the first fetch (e.g. an ECR repo's
/// Images list) for the rest of the session — and, when the detail pane is
/// focused, re-fire the open section's trigger so it refetches instead of
/// spinning forever over the emptied map.
#[tokio::test]
async fn refresh_clears_the_lazy_store_and_refires_the_open_section() {
    let (mut app, tx, _rx) = test_app().await;
    let repo = all_mocks()
        .into_iter()
        .find(|(s, _, _)| *s == ServiceType::Ecr)
        .expect("ECR repo mock")
        .2;
    select_mock(&mut app, ServiceType::Ecr, repo);

    // Simulate an already-loaded Images section for the selected repo.
    app.lazy
        .ecr_repo_images
        .insert_loading("mock-repo".to_string());
    app.lazy.ecr_repo_images.apply("mock-repo".to_string(), Ok(vec![]));
    let epoch = app.lazy.epoch();

    // List-pane `r`: the whole store is replaced and the epoch bumps so
    // stale in-flight applies drop — a redeployed image shows on refresh.
    app.handle_key(key(KeyCode::Char('r')), &tx).await.unwrap();
    assert!(
        !app.lazy.ecr_repo_images.contains("mock-repo"),
        "list-pane refresh must clear lazy detail data"
    );
    assert!(app.lazy.epoch() > epoch, "refresh must bump the lazy epoch");

    // Detail-pane `r` on the Images section (no targeted fetch for ECR →
    // full refresh): the stale entry is dropped AND the section's trigger
    // re-fires, leaving a fresh in-flight fetch rather than nothing.
    select_mock(
        &mut app,
        ServiceType::Ecr,
        all_mocks()
            .into_iter()
            .find(|(s, _, _)| *s == ServiceType::Ecr)
            .unwrap()
            .2,
    );
    app.lazy
        .ecr_repo_images
        .insert_loading("mock-repo".to_string());
    app.lazy.ecr_repo_images.apply("mock-repo".to_string(), Ok(vec![]));
    app.handle_key(key(KeyCode::Enter), &tx).await.unwrap();
    app.handle_key(key(KeyCode::Char('2')), &tx).await.unwrap();
    assert!(app.details_focused);
    app.handle_key(key(KeyCode::Char('r')), &tx).await.unwrap();
    assert!(
        matches!(
            app.lazy.ecr_repo_images.get("mock-repo"),
            Some(crate::lazy::Lazy::Loading)
        ),
        "detail-pane refresh must re-fire the open section's fetch"
    );
}

/// Rapid service switches: a superseded stream's terminal events must not
/// clear the new load's flags (that made the partial handler drop every
/// later batch — the fast-switch "empty list until refresh" bug) and must
/// not cache the on-screen rows under the stale service's key.
#[tokio::test]
async fn stale_load_events_cannot_break_the_next_load() {
    let (mut app, tx, _rx) = test_app().await;

    // Two EC2 mocks: one already on screen, one arriving as the next batch.
    let mut ec2: Vec<Box<dyn Resource>> = all_mocks()
        .into_iter()
        .filter(|(s, _, _)| *s == ServiceType::EC2)
        .map(|(_, _, r)| r)
        .collect();
    assert!(ec2.len() >= 2, "need two EC2 mocks");
    let next_batch = ec2.pop().unwrap();
    let on_screen = ec2.pop().unwrap();

    // Mid-switch state: CloudWatch's load was still streaming when the user
    // switched to EC2 — EC2's load is in flight with one batch landed.
    select_mock(&mut app, ServiceType::EC2, on_screen);
    app.loading = true;
    app.loading_started = true;

    // The stale CloudWatch stream finishes. It must not flip `loading` (EC2's
    // stream still owns it) and must not stamp EC2's on-screen rows into the
    // CloudWatch cache slot.
    app.handle_event(
        Event::ResourcesFullyLoaded {
            service: ServiceType::CloudWatch,
            total_count: 3,
        },
        &tx,
    )
    .await
    .unwrap();
    assert!(app.loading, "stale FullyLoaded cleared the in-flight load's flag");
    assert!(
        app.cache
            .get(&ServiceType::CloudWatch, &app.current_region, None)
            .is_none(),
        "stale FullyLoaded cached the current service's rows under its own key"
    );

    // A stale error must not clear the flags or surface in the status bar.
    app.handle_event(
        Event::ResourceLoadError {
            service: ServiceType::CloudWatch,
            error: "stale".to_string(),
        },
        &tx,
    )
    .await
    .unwrap();
    assert!(app.loading, "stale LoadError cleared the in-flight load's flag");
    assert!(app.error_message.is_none(), "stale LoadError surfaced its message");

    // …so EC2's next batch still lands.
    app.handle_event(
        Event::ResourcesPartiallyLoaded {
            service: ServiceType::EC2,
            resources: vec![next_batch],
            progress: LoadProgress {
                loaded_count: 2,
                total_count: None,
                status_message: None,
            },
        },
        &tx,
    )
    .await
    .unwrap();
    assert_eq!(app.resources.len(), 2, "the in-flight load's batch was dropped");

    // And the load completes normally: flags clear, cache stamped under EC2.
    app.handle_event(
        Event::ResourcesFullyLoaded {
            service: ServiceType::EC2,
            total_count: 2,
        },
        &tx,
    )
    .await
    .unwrap();
    assert!(!app.loading);
    assert!(app
        .cache
        .get(&ServiceType::EC2, &app.current_region, None)
        .is_some());
}

/// Every explicit load supersedes in-flight streams: the generation counter
/// must advance so their forwarders go quiet (see `load_resources_async`).
#[tokio::test]
async fn load_service_resources_bumps_the_stream_generation() {
    let (mut app, _tx, _rx) = test_app().await;
    app.current_service = Some(ServiceType::EC2);
    let before = app.load_generation.load(std::sync::atomic::Ordering::SeqCst);
    app.load_service_resources();
    let after = app.load_generation.load(std::sync::atomic::Ordering::SeqCst);
    assert!(after > before, "load_service_resources must supersede older streams");
}

/// A credential switch must invalidate the identity fetch already in flight.
/// `spawn_account_info_fetch` is detached, so without the generation stamp a
/// slow `GetCallerIdentity` from the *previous* profile lands after the reset
/// and repopulates `account_id` with the old account — which is not just a
/// wrong badge: it feeds the CodePipeline tag ARN, the budget/invoice triggers,
/// and the "Already browsing account X" guard that refuses an org-role hop.
#[tokio::test]
async fn stale_account_identity_is_dropped_after_a_credential_switch() {
    let (mut app, tx, _rx) = test_app().await;
    let first = app.account_info_generation;

    app.handle_event(
        Event::AccountInfoLoaded {
            account_id: "111111111111".to_string(),
            alias: Some("base".to_string()),
            generation: first,
        },
        &tx,
    )
    .await
    .unwrap();
    assert_eq!(app.account_id.as_deref(), Some("111111111111"));

    // Credential switch: identity clears and the generation moves on.
    app.reset_account_scoped_state();
    assert!(app.account_id.is_none());
    let second = app.account_info_generation;
    assert_ne!(first, second, "a credential reset must bump the generation");

    // The old profile's fetch finally resolves — it must be ignored.
    app.handle_event(
        Event::AccountInfoLoaded {
            account_id: "111111111111".to_string(),
            alias: Some("base".to_string()),
            generation: first,
        },
        &tx,
    )
    .await
    .unwrap();
    assert_eq!(app.account_id, None, "stale identity repopulated the badge");
    assert_eq!(app.account_alias, None);

    // The new credentials' fetch is accepted.
    app.handle_event(
        Event::AccountInfoLoaded {
            account_id: "222222222222".to_string(),
            alias: None,
            generation: second,
        },
        &tx,
    )
    .await
    .unwrap();
    assert_eq!(app.account_id.as_deref(), Some("222222222222"));
}

/// `m` focuses the detail pane behind the overlay. If it takes that focus
/// without firing the default section's on-enter hook, closing the overlay
/// reveals a focused-but-untriggered pane: `spin_loading_row` renders a spinner
/// (focused means "a fetch is in flight"), and `LazyMap`'s contains-guard means
/// nothing ever retries. Worst on a CloudWatch dashboard, whose *first* section
/// is lazy — the entire pane sits on "Loading…" forever.
#[tokio::test]
async fn opening_metrics_from_the_list_pane_triggers_the_section_behind_it() {
    let (mut app, tx, _rx) = test_app().await;
    let (service, _l, mock) = all_mocks()
        .into_iter()
        .find(|(_, l, _)| *l == "CwDashboard")
        .expect("dashboard mock");
    let name = mock.id().to_string();
    select_mock(&mut app, service, mock);
    assert!(!app.details_focused, "starts in the list pane");
    assert!(app.lazy.cw_dashboards.get(&name).is_none());

    app.handle_key(key(KeyCode::Char('m')), &tx).await.unwrap();

    assert!(app.details_focused, "m focuses the detail pane");
    assert_eq!(app.detail_section_idx, 0);
    // The hook fired: the section's fetch is genuinely in flight, so the
    // spinner the pane draws after Esc is telling the truth.
    assert!(
        app.lazy.cw_dashboards.get(&name).is_some(),
        "the default section's on-enter hook must fire when m takes focus"
    );
}


/// Clicking a widget beats tabbing to it on any dashboard bigger than a handful,
/// and the mouse is otherwise blanket-gated while an overlay is up — so the
/// charted dashboard has to claim the event before that gate. Text panels are
/// deliberately not click targets: `Tab` skips them, so a click that put the
/// cursor on one would strand it somewhere the keyboard can't leave.
#[tokio::test]
async fn clicking_a_dashboard_widget_selects_it_and_double_click_zooms() {
    use crate::app::MetricsKind;
    use crate::aws::services::cloudwatch::{
        dashboard_query_id, parse_dashboard_body, CwDashboardMetricsData, CwDashboardMetricsState,
        CwSeries,
    };
    use crate::aws::services::ec2::MetricsTimeRange;
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    use std::collections::HashMap;

    let (mut app, tx, _rx) = test_app().await;
    let (service, _l, mock) = all_mocks()
        .into_iter()
        .find(|(_, l, _)| *l == "CwDashboard")
        .expect("dashboard mock");
    let name = mock.id().to_string();
    select_mock(&mut app, service, mock);

    let body = r##"{"widgets":[
      {"type":"text","x":0,"y":0,"width":24,"height":2,"properties":{"markdown":"# Edge"}},
      {"type":"metric","x":0,"y":2,"width":8,"height":6,"properties":{"title":"A","metrics":[["N","a"]]}},
      {"type":"metric","x":8,"y":2,"width":8,"height":6,"properties":{"title":"B","metrics":[["N","b"]]}},
      {"type":"metric","x":16,"y":2,"width":8,"height":6,"properties":{"title":"C","metrics":[["N","c"]]}}
    ]}"##;
    let widgets = parse_dashboard_body(body).1;
    let pts: Vec<(f64, f64)> = (0..30).map(|i| (i as f64 * 700.0, 5.0 + i as f64)).collect();
    let mut series: HashMap<String, Vec<CwSeries>> = HashMap::new();
    for w in 1..=3 {
        series.insert(
            dashboard_query_id(w, 0, None),
            vec![CwSeries { label: format!("s{w}"), points: pts.clone() }],
        );
    }
    app.cw_dashboard_metrics.insert(
        name,
        CwDashboardMetricsState::Loaded(Box::new(CwDashboardMetricsData {
            time_range: MetricsTimeRange::SixHours,
            x_max: MetricsTimeRange::SixHours.duration_secs() as f64,
            widgets,
            series,
            alarms: HashMap::new(),
            skipped_regions: vec![],
            math_error: None,
            capped: false,
        })),
    );
    app.metrics_in_pane = Some(MetricsKind::CwDashboard);
    app.details_focused = true;
    app.layout_mode = crate::app::LayoutMode::DetailsOnly;

    // Regions are recorded as the grid draws, so they are only valid after a frame.
    let mut terminal = Terminal::new(TestBackend::new(120, 20)).unwrap();
    terminal.draw(|f| crate::render_app(&app, f)).unwrap();

    let regions = app.cw_dashboard_regions.borrow().clone();
    assert_eq!(
        regions.iter().map(|(i, _)| *i).collect::<Vec<_>>(),
        vec![1, 2, 3],
        "the text panel at index 0 must not be a click target"
    );

    let click = |c: u16, r: u16| MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: c,
        row: r,
        modifiers: crossterm::event::KeyModifiers::NONE,
    };
    let hit = regions.iter().find(|(i, _)| *i == 3).unwrap().1;
    let (cx, cy) = (hit.x + 3, hit.y + 2);

    app.handle_event(crate::event::Event::Mouse(click(cx, cy)), &tx).await.unwrap();
    assert_eq!(app.cw_dashboard_cursor, Some(3));
    assert!(!app.cw_dashboard_zoomed, "one click only moves the cursor");

    // Second press on the same cell inside the window = double = zoom.
    app.handle_event(crate::event::Event::Mouse(click(cx, cy)), &tx).await.unwrap();
    assert!(app.cw_dashboard_zoomed);

    // Zoomed, the widget owns the whole body, and double-clicking comes back out.
    terminal.draw(|f| crate::render_app(&app, f)).unwrap();
    assert_eq!(app.cw_dashboard_regions.borrow().len(), 1);
    app.handle_event(crate::event::Event::Mouse(click(cx, cy)), &tx).await.unwrap();
    app.handle_event(crate::event::Event::Mouse(click(cx, cy)), &tx).await.unwrap();
    assert!(!app.cw_dashboard_zoomed);

    // A click in the gap above the widgets hits nothing and changes nothing.
    app.cw_dashboard_cursor = Some(2);
    app.handle_event(crate::event::Event::Mouse(click(1, 0)), &tx).await.unwrap();
    assert_eq!(app.cw_dashboard_cursor, Some(2));
}


/// A change-set change's "Physical ID" row must resolve a jump target to the
/// live resource (the Changes-section arm of `cfn_resource_jump_target`) —
/// for a resource type `cfn_type_jump_target` covers.
#[tokio::test]
async fn cfn_change_set_physical_id_row_jumps_to_the_live_resource() {
    use crate::aws::services::cloudformation as cfn;
    let mut app = App::new_for_test().await;
    let stack = cfn::CfnStack::from_sdk(
        &aws_sdk_cloudformation::types::Stack::builder()
            .stack_name("mock-stack")
            .stack_id("arn:aws:cloudformation:us-east-1:123456789012:stack/mock-stack/abc")
            .build(),
    );
    let stack_id = stack.stack_id.clone();
    select_mock(&mut app, ServiceType::CloudFormation, Box::new(stack));
    app.lazy.cfn_stack_changesets.apply(
        stack_id,
        Ok(vec![cfn::CfnChangeSet {
            name: "cs".to_string(),
            id: "cs-arn".to_string(),
            status: "CREATE_COMPLETE".to_string(),
            execution_status: "AVAILABLE".to_string(),
            status_reason: None,
            creation_time: String::new(),
            description: None,
            changes: vec![cfn::CfnChange {
                action: "Modify".to_string(),
                logical_id: "Fn".to_string(),
                resource_type: "AWS::Lambda::Function".to_string(),
                physical_id: Some("my-fn".to_string()),
                replacement: None,
                policy_action: None,
                details: vec![],
                before_context: None,
                after_context: None,
            }],
            hooks: vec![],
            hook_results: vec![],
        }]),
    );
    // Activate the Changes section on the live-stack descriptor.
    app.detail_section_idx = (0..16)
        .find(|&i| {
            cfn::CfnStackDetailSection::from_index(i) == cfn::CfnStackDetailSection::Changes
        })
        .expect("Changes section exists");
    let t = app
        .cfn_resource_jump_target("          Physical ID", "my-fn")
        .expect("change-set Physical ID row should resolve a jump target");
    assert_eq!(t.service, ServiceType::Lambda);
    assert_eq!(t.id, "my-fn");
}

/// `Enter` on a CodeCommit Branches-section row must re-key the Commits walk
/// to that branch and land on the Commits section (`try_cc_branch_commits` —
/// the row shape `("  {branch}", tip-info)` is load-bearing).
#[tokio::test]
async fn cc_branch_row_enter_opens_that_branchs_commits() {
    use crate::aws::services::code as code;
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::new_for_test().await;
    let repo = code::CodeCommitRepo {
        name: "my-repo".to_string(),
        arn: "arn:aws:codecommit:us-east-1:123456789012:my-repo".to_string(),
        description: String::new(),
        clone_url_http: String::new(),
        clone_url_ssh: String::new(),
        clone_url_grc: String::new(),
        default_branch: "main".to_string(),
        last_modified: String::new(),
        account_id: String::new(),
        tags: std::collections::HashMap::new(),
    };
    select_mock(&mut app, ServiceType::Code, Box::new(repo));
    app.details_focused = true;
    let mk = |name: &str, is_default: bool| code::CcBranch {
        name: name.to_string(),
        tip_commit_id: "0123456789abcdef".to_string(),
        subject: "tip subject".to_string(),
        author: "au".to_string(),
        date: "2026-08-27T00:00:00Z".to_string(),
        date_secs: 1,
        is_default,
    };
    app.lazy.cc_branches.apply(
        "my-repo".to_string(),
        Ok(code::CcBranches {
            branches: vec![mk("main", true), mk("feature/x", false)],
            unresolved: 0,
        }),
    );
    app.detail_section_idx = code::CodeCommitRepoDetailSection::Branches as usize;

    // Cursor onto the feature branch's row.
    let rows = app.get_detail_lines_filtered();
    let row = rows
        .iter()
        .position(|(k, _)| k == "  feature/x")
        .expect("branch row rendered");
    app.details_selected_index = Some(row);
    assert!(app.try_cc_branch_commits(&tx), "branch row handles Enter");
    assert_eq!(
        app.cc_commits_branch,
        Some(("my-repo".to_string(), "feature/x".to_string()))
    );
    assert_eq!(
        code::CodeCommitRepoDetailSection::from_index(app.detail_section_idx),
        code::CodeCommitRepoDetailSection::Commits,
        "Enter lands on the Commits section"
    );

    // A non-branch row (the group header) falls through to the jump chain.
    app.detail_section_idx = code::CodeCommitRepoDetailSection::Branches as usize;
    let rows = app.get_detail_lines_filtered();
    let header = rows
        .iter()
        .position(|(k, _)| k.starts_with("Branches ("))
        .expect("header row rendered");
    app.details_selected_index = Some(header);
    assert!(!app.try_cc_branch_commits(&tx));
}

/// `Enter` on a `#<id>` row in the repo pane's Pull Requests section must
/// resolve a jump to that PR on the Pull Requests sub-tab.
#[tokio::test]
async fn cc_repo_pr_row_enter_jumps_to_the_pr() {
    use crate::aws::services::code as code;
    let mut app = App::new_for_test().await;
    let repo = code::CodeCommitRepo {
        name: "my-repo".to_string(),
        arn: "arn:aws:codecommit:us-east-1:123456789012:my-repo".to_string(),
        description: String::new(),
        clone_url_http: String::new(),
        clone_url_ssh: String::new(),
        clone_url_grc: String::new(),
        default_branch: "main".to_string(),
        last_modified: String::new(),
        account_id: String::new(),
        tags: std::collections::HashMap::new(),
    };
    select_mock(&mut app, ServiceType::Code, Box::new(repo));
    app.detail_section_idx = code::CodeCommitRepoDetailSection::PullRequests as usize;
    let t = app
        .cc_repo_pr_row_jump_target("  #42", "OPEN · mock change · mock-user · 2026")
        .expect("PR row resolves a jump target");
    assert_eq!(t.service, ServiceType::Code);
    assert_eq!(t.id, "42");
    assert!(matches!(
        t.view,
        crate::app::JumpView::Code(crate::app::CodeView::PullRequests)
    ));
    // Annotation rows don't jump.
    assert!(app
        .cc_repo_pr_row_jump_target("  · ⏎ on a row opens the pull request", "")
        .is_none());
    // Recently-Closed rows carry a status suffix precisely so the digits-only
    // classifier rejects them — closed PRs have no row on the sub-tab.
    assert!(app.cc_repo_pr_row_jump_target("  #42 (MERGED)", "x").is_none());
    // Outside the Pull Requests section the same shape doesn't fire.
    app.detail_section_idx = code::CodeCommitRepoDetailSection::Details as usize;
    assert!(app.cc_repo_pr_row_jump_target("  #42", "x").is_none());
}

/// Types with no explicit routing arm fall back to service-token routing —
/// landing in the owning service with a searchable id — instead of being
/// silently non-jumpable. Unknown tokens stay None.
#[test]
fn cfn_type_jump_falls_back_to_the_owning_service() {
    use crate::ui::widgets::details_pane::cfn_type_jump_target;
    // A sub-resource type nobody routes explicitly: lands in Messaging with
    // the subscription ARN reduced to its last segment.
    let t = cfn_type_jump_target(
        "AWS::SNS::Subscription",
        "arn:aws:sns:us-east-1:123456789012:my-topic:abcd-1234",
    )
    .expect("falls back by service token");
    assert_eq!(t.service, ServiceType::Messaging);
    assert_eq!(t.id, "abcd-1234");
    // Serverless-adjacent sub-resource with a plain-name physical id.
    let t = cfn_type_jump_target("AWS::DynamoDB::GlobalTable", "orders").unwrap();
    assert_eq!(t.service, ServiceType::DynamoDb);
    assert_eq!(t.id, "orders");
    // Custom resources and unbrowsed services stay non-jumpable.
    assert!(cfn_type_jump_target("Custom::MyThing", "x").is_none());
    assert!(cfn_type_jump_target("AWS::SomeNewService::Widget", "x").is_none());
    // VPC-side EC2 types must route to the VPC service, not fall back to the
    // EC2 screen by service token.
    let t = cfn_type_jump_target("AWS::EC2::VPCEndpoint", "vpce-0abc").unwrap();
    assert_eq!(t.service, ServiceType::VPC);
    assert_eq!(t.id, "vpce-0abc");
    let t = cfn_type_jump_target("AWS::EC2::TransitGateway", "tgw-0abc").unwrap();
    assert_eq!(t.service, ServiceType::TransitGateway);
}
