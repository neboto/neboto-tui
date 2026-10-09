//! Lambda Layers tab (#102): the sub-tab filter, the eager Used By section
//! derived from loaded functions, and the function ↔ layer row jumps.

use super::*;
use crate::app::{JumpView, LambdaView};
use crate::aws::services::lambda::{LambdaFunction, LambdaLayer, LambdaLayerDetailSection};
use ratatui::{backend::TestBackend, Terminal};

const ACCT: &str = "123456789012";

fn layer(name: &str, latest: i64) -> LambdaLayer {
    let arn = format!("arn:aws:lambda:us-east-1:{ACCT}:layer:{name}");
    LambdaLayer::from_sdk(
        &aws_sdk_lambda::types::LayersListItem::builder()
            .layer_name(name)
            .layer_arn(&arn)
            .latest_matching_version(
                aws_sdk_lambda::types::LayerVersionsListItem::builder()
                    .version(latest)
                    .layer_version_arn(format!("{arn}:{latest}"))
                    .build(),
            )
            .build(),
    )
}

fn func(name: &str, layers: &[String]) -> LambdaFunction {
    let mut b = aws_sdk_lambda::types::FunctionConfiguration::builder()
        .function_name(name)
        .function_arn(format!("arn:aws:lambda:us-east-1:{ACCT}:function:{name}"));
    for l in layers {
        b = b.layers(aws_sdk_lambda::types::Layer::builder().arn(l).build());
    }
    LambdaFunction::from_sdk(&b.build())
}

fn utils_v(v: i64) -> String {
    format!("arn:aws:lambda:us-east-1:{ACCT}:layer:utils:{v}")
}

fn screen_of(app: &App) -> String {
    let mut terminal = Terminal::new(TestBackend::new(170, 45)).unwrap();
    terminal.draw(|f| crate::render_app(app, f)).unwrap();
    let buf = terminal.backend().buffer();
    (0..buf.area.height)
        .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

fn load(app: &mut App) {
    select_mock(app, ServiceType::Lambda, Box::new(func("api", &[utils_v(3)])));
    app.resources = vec![
        Box::new(func("api", &[utils_v(3), "arn:aws:lambda:us-east-1:464622532012:layer:Datadog:65".into()])),
        Box::new(func("report", &[utils_v(2)])),
        Box::new(func("idle", &[])),
        Box::new(layer("utils", 3)),
        Box::new(layer("unused", 1)),
    ];
}

#[tokio::test]
async fn layers_tab_lists_only_layers() {
    let (mut app, tx, _rx) = test_app().await;
    load(&mut app);
    app.lambda_view = LambdaView::Functions;
    app.update_search();
    assert_eq!(app.filtered_resources.len(), 3);
    app.handle_event(Event::Key(key(KeyCode::Char('2'))), &tx).await.unwrap();
    assert_eq!(app.lambda_view, LambdaView::Layers);
    let types: Vec<&str> =
        app.filtered_resources.iter().map(|&i| app.resources[i].resource_type()).collect();
    assert_eq!(types, ["Lambda Layer", "Lambda Layer"]);
    let screen = screen_of(&app);
    assert!(screen.contains("Functions") && screen.contains("Layers"), "{screen}");
}

#[tokio::test]
async fn used_by_groups_loaded_functions_by_version() {
    let (mut app, tx, _rx) = test_app().await;
    load(&mut app);
    app.lambda_view = LambdaView::Layers;
    app.update_search();
    app.selected_index = Some(0);
    app.details_focused = true;
    app.reset_detail_section_to_default(&tx);
    let used_by = crate::aws::services::lambda::LAMBDA_LAYER_SECTIONS
        .sections
        .iter()
        .position(|s| s.label == "Used By")
        .unwrap();
    app.set_detail_section(used_by, &tx);
    assert_eq!(
        LambdaLayerDetailSection::from_index(app.detail_section_idx),
        LambdaLayerDetailSection::UsedBy
    );
    let rows = app.get_detail_lines();
    let pos = |k: &str, v: &str| rows.iter().position(|(a, b)| a == k && b == v);
    let v3 = pos("Version 3 (latest)", "").expect("latest group");
    let v2 = pos("Version 2", "").expect("older group");
    let api = pos("  Function", "api").unwrap();
    let report = pos("  Function", "report").unwrap();
    assert!(v3 < api && api < v2 && v2 < report, "{rows:?}");
    assert!(pos("  Function", "idle").is_none());

    // Enter on a function row opens it on the Functions tab.
    app.details_selected_index = Some(report);
    app.handle_event(Event::Key(key(KeyCode::Enter)), &tx).await.unwrap();
    assert_eq!(app.lambda_view, LambdaView::Functions);
    let sel = app.get_selected_resource().unwrap();
    assert_eq!(sel.name(), "report");
}

#[tokio::test]
async fn function_layer_row_jumps_to_own_layers_only() {
    let (mut app, tx, _rx) = test_app().await;
    load(&mut app);
    app.lambda_view = LambdaView::Functions;
    app.update_search();
    app.selected_index = Some(0);
    app.details_focused = true;
    app.reset_detail_section_to_default(&tx);
    let rows = app.get_detail_lines();
    assert!(
        rows.iter().any(|(_, v)| v.contains("1 external layer")),
        "external note: {rows:?}"
    );
    let ext = app.lambda_row_jump_target("  Layer 2", "arn:aws:lambda:us-east-1:464622532012:layer:Datadog:65");
    assert!(ext.is_none());
    let own = rows.iter().position(|(k, v)| k == "  Layer 1" && *v == utils_v(3)).unwrap();
    let t = app.lambda_row_jump_target("  Layer 1", &utils_v(3)).unwrap();
    assert!(matches!(t.view, JumpView::Lambda(LambdaView::Layers)));

    app.details_selected_index = Some(own);
    app.handle_event(Event::Key(key(KeyCode::Enter)), &tx).await.unwrap();
    assert_eq!(app.lambda_view, LambdaView::Layers);
    assert_eq!(app.get_selected_resource().unwrap().name(), "utils");
}
