//! AgentCore depth (#97): the panes that grew a lazy `Get*`, the bundle
//! version diff, the two `e` overrides and the memory browser's macro step.
//! The section-order tests already cover the new panes' wiring; these drive
//! real data through the lazy maps and check what the rows say.

use super::*;
use crate::aws::services::agentcore as ac;
use crate::macros::{MacroPlayer, MacroRecorder, MacroStep};

fn mock(label: &str) -> Box<dyn Resource> {
    all_mocks()
        .into_iter()
        .find(|m| m.1 == label)
        .unwrap_or_else(|| panic!("no mock {label}"))
        .2
}

/// An app with `label`'s mock selected and the detail pane on `section`.
async fn on_section(label: &str, section: usize) -> (App, mpsc::UnboundedSender<Event>) {
    let (mut app, tx, _rx) = test_app().await;
    select_mock(&mut app, ServiceType::AgentCore, mock(label));
    app.details_focused = true;
    app.detail_section_idx = section;
    (app, tx)
}

fn body(app: &App) -> Vec<(String, String)> {
    app.get_detail_lines()
}

fn has(rows: &[(String, String)], needle: &str) -> bool {
    rows.iter().any(|(k, v)| k.contains(needle) || v.contains(needle))
}

fn component(name: &str, json: &str) -> (String, String) {
    (name.to_string(), json.to_string())
}

#[tokio::test]
async fn bundle_versions_diff_each_component_against_live() {
    use ac::AgentCoreConfigBundleDetailSection as S;
    let (mut app, _tx) = on_section("AgentCoreConfigBundle", S::Versions as usize).await;
    app.lazy.agentcore_bundle_detail.apply(
        "mock-bundle".into(),
        Ok(Box::new(ac::AgentCoreConfigBundleDetail {
            version_id: "v2".into(),
            components: vec![component("model", "{\n  \"id\": \"new\"\n}"), component("prompt", "{}")],
            ..Default::default()
        })),
    );
    let v = |id: &str, components| ac::AgentCoreConfigBundleVersion {
        version_id: id.into(),
        created: String::new(),
        branch_name: String::new(),
        commit_message: String::new(),
        created_by: String::new(),
        parent_versions: Vec::new(),
        components,
    };
    app.lazy.agentcore_bundle_versions.apply(
        "mock-bundle".into(),
        Ok(vec![
            v("v2", Some(vec![component("model", "{\n  \"id\": \"new\"\n}"), component("prompt", "{}")])),
            v("v1", Some(vec![component("model", "{\n  \"id\": \"old\"\n}"), component("prompt", "{}")])),
            v("v0", None),
        ]),
    );
    let rows = body(&app);
    let find = |key: &str, from: usize| {
        rows.iter()
            .skip(from)
            .position(|(k, _)| k.trim() == key)
            .map(|p| p + from)
            .unwrap_or_else(|| panic!("no {key} after {from}: {rows:#?}"))
    };
    let v1 = rows.iter().position(|(_, v)| v == "v1").expect("v1 row");
    let model = find("Component model", v1);
    assert_eq!(rows[model].1, "≠ live");
    // The changed component prints its JSON; the unchanged one doesn't.
    assert!(rows[model + 2].0.contains("\"old\""), "{rows:#?}");
    let prompt = find("Component prompt", v1);
    assert_eq!(rows[prompt].1, "same as live");
    // Past the deepening, the version says so instead of looking empty.
    assert!(has(&rows, "components not fetched"));
}

#[tokio::test]
async fn online_eval_names_its_evaluators_from_the_sibling_rows() {
    use ac::AgentCoreOnlineEvalDetailSection as S;
    let (mut app, _tx) = on_section("AgentCoreOnlineEval", S::Evaluators as usize).await;
    app.resources.push(mock("AgentCoreEvaluator"));
    app.lazy.agentcore_online_eval_detail.apply(
        "mock-online-eval".into(),
        Ok(Box::new(ac::AgentCoreOnlineEvalDetail {
            evaluator_ids: vec!["mock-evaluator".into(), "Builtin.Unknown".into()],
            ..Default::default()
        })),
    );
    let rows = body(&app);
    assert!(
        rows.iter().any(|(k, v)| k.trim() == "mock-evaluator" && v.starts_with("mock-evaluator ·")),
        "{rows:#?}"
    );
    // An id the Evaluation tab hasn't listed still shows, bare.
    assert!(rows.iter().any(|(k, _)| k.trim() == "Builtin.Unknown"));
}

#[tokio::test]
async fn a_builtin_evaluator_says_its_config_is_not_exposed() {
    use ac::AgentCoreEvaluatorDetailSection as S;
    let (mut app, _tx) = on_section("AgentCoreEvaluator", S::Config as usize).await;
    app.lazy
        .agentcore_evaluator_detail
        .apply("mock-evaluator".into(), Ok(Box::default()));
    assert!(has(&body(&app), "built-in evaluators don't expose theirs"));

    app.lazy.agentcore_evaluator_detail.apply(
        "mock-evaluator".into(),
        Ok(Box::new(ac::AgentCoreEvaluatorDetail {
            config_kind: "LLM-as-a-judge".into(),
            instructions: "Judge the answer.\nBe strict.".into(),
            scale_kind: "Numerical".into(),
            scale: vec![("Good".into(), "1".into(), "fully correct".into())],
            ..Default::default()
        })),
    );
    let rows = body(&app);
    assert!(rows.iter().any(|(k, v)| k == "Good = 1" && v == "fully correct"));
    assert!(rows.iter().any(|(k, _)| k == "  Be strict."));
}

#[tokio::test]
async fn dataset_examples_say_when_they_are_a_preview() {
    use ac::AgentCoreDatasetDetailSection as S;
    let (mut app, _tx) = on_section("AgentCoreDataset", S::Examples as usize).await;
    app.lazy.agentcore_dataset_examples.apply(
        "mock-dataset".into(),
        Ok(ac::AgentCoreDatasetExamples {
            version: "3".into(),
            examples: vec!["{\n  \"input\": \"hi\"\n}".into()],
            truncated: true,
        }),
    );
    let rows = body(&app);
    assert!(rows.iter().any(|(k, _)| k.starts_with("Examples (first 1 of")), "{rows:#?}");
    assert!(has(&rows, "\"input\": \"hi\""));
}

#[tokio::test]
async fn e_on_a_harness_prompt_opens_the_prompt_as_text() {
    use ac::AgentCoreHarnessDetailSection as S;
    let (mut app, _tx) = on_section("AgentCoreHarness", S::Prompt as usize).await;
    assert!(app.editor_override_content().is_none(), "unfetched falls through");
    app.lazy.agentcore_harness_detail.apply(
        "mock-harness".into(),
        Ok(Box::new(ac::AgentCoreHarnessDetail {
            system_prompt: vec!["You are helpful.".into(), "Never guess.".into()],
            ..Default::default()
        })),
    );
    let (text, suffix) = app.editor_override_content().expect("prompt override");
    assert_eq!(suffix, ".txt");
    assert_eq!(text, "You are helpful.\n\nNever guess.");

    // Other sections keep the snapshot.
    app.detail_section_idx = S::Overview as usize;
    assert!(app.editor_override_content().is_none());
}

#[tokio::test]
async fn e_on_bundle_components_opens_them_as_one_json_object() {
    use ac::AgentCoreConfigBundleDetailSection as S;
    let (mut app, _tx) = on_section("AgentCoreConfigBundle", S::Components as usize).await;
    app.lazy.agentcore_bundle_detail.apply(
        "mock-bundle".into(),
        Ok(Box::new(ac::AgentCoreConfigBundleDetail {
            components: vec![component("model", "{\n  \"id\": \"m\"\n}")],
            ..Default::default()
        })),
    );
    let (text, suffix) = app.editor_override_content().expect("components override");
    assert_eq!(suffix, ".json");
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["model"]["configuration"]["id"], "m");
}

fn memory_row(id: &str) -> ac::MemoryBrowserRow {
    ac::MemoryBrowserRow {
        id: id.to_string(),
        primary: id.to_string(),
        secondary: String::new(),
        detail: String::new(),
    }
}

#[tokio::test]
async fn memory_browsing_records_one_location_step() {
    use crate::ui::widgets::memory_browser::MemoryLevel;
    let (mut app, tx, _rx) = test_app().await;
    select_mock(&mut app, ServiceType::AgentCore, mock("AgentCoreMemory"));
    app.macro_recorder = Some(MacroRecorder {
        last_region: app.current_region.as_str().to_string(),
        last_service: app.current_service,
        ..Default::default()
    });

    // Stand the browser up at Sessions for actor-1, as `i` + `⏎` would.
    app.memory_browser.open("mock-memory".into(), Vec::new());
    app.memory_browser.level = MemoryLevel::Sessions;
    app.memory_browser.actor_id = "actor-1".into();
    app.memory_browser.rows = vec![memory_row("s-1"), memory_row("s-2")];
    app.macro_tick(&tx);
    // Moving the cursor replaces the step: only the destination counts.
    app.handle_event(Event::Key(key(KeyCode::Char('j'))), &tx).await.unwrap();
    app.macro_tick(&tx);

    let steps = app.macro_recorder.as_ref().unwrap().steps.clone();
    assert_eq!(
        steps,
        vec![MacroStep::MemoryLocation {
            memory_id: "mock-memory".into(),
            records: false,
            strategy: String::new(),
            actor: "actor-1".into(),
            session: String::new(),
            row: "s-2".into(),
        }],
        "browser keys leaked or the step didn't collapse"
    );
}

#[tokio::test]
async fn a_memory_step_replays_to_the_level_and_selects_the_row_once_it_lands() {
    use crate::ui::widgets::memory_browser::MemoryLevel;
    let (mut app, tx, _rx) = test_app().await;
    select_mock(&mut app, ServiceType::AgentCore, mock("AgentCoreMemory"));
    app.macro_player = Some(MacroPlayer {
        name: "t".into(),
        steps: vec![MacroStep::MemoryLocation {
            memory_id: "mock-memory".into(),
            records: true,
            strategy: "strat-1".into(),
            actor: String::new(),
            session: String::new(),
            row: "rec-2".into(),
        }],
        index: 0,
        last_step_at: None,
    });
    app.macro_tick(&tx);

    let b = &app.memory_browser;
    assert!(b.visible);
    assert_eq!(b.level, MemoryLevel::Records);
    // The store's detail never loaded; the recorded strategy is enough.
    assert_eq!(b.current_strategy().map(|s| s.0.as_str()), Some("strat-1"));
    assert!(b.loading, "the listing was never requested");

    app.handle_event(
        Event::MemoryBrowserLoaded {
            rows: vec![memory_row("rec-1"), memory_row("rec-2")],
            next_token: None,
            append: false,
        },
        &tx,
    )
    .await
    .unwrap();
    assert_eq!(app.memory_browser.selected_row().map(|r| r.id.as_str()), Some("rec-2"));
}

#[test]
fn a_memory_step_round_trips_and_omits_empty_parts() {
    let step = MacroStep::MemoryLocation {
        memory_id: "m".into(),
        records: false,
        strategy: String::new(),
        actor: "a".into(),
        session: String::new(),
        row: String::new(),
    };
    let json = serde_json::to_string(&step).unwrap();
    assert_eq!(json, r#"{"MemoryLocation":{"memory_id":"m","actor":"a"}}"#);
    assert_eq!(serde_json::from_str::<MacroStep>(&json).unwrap(), step);
}
