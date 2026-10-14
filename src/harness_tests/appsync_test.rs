//! AppSync (issue #96): the API pane renders its sections, lazy sections
//! fetch on enter, API keys never carry the key value, label-keyed rows
//! jump to Cognito / DynamoDB / OpenSearch / CloudWatch, and the API ARN
//! jumps back to the API row.

use super::*;
use crate::aws::services::appsync::{
    AppSyncApi, AppSyncApiDetailSection, AppSyncApiKey, AppSyncDataSource, APPSYNC_API_SECTIONS,
};
use crate::lazy::Lazy;

const API_ID: &str = "abc123gqlapi";

fn api() -> AppSyncApi {
    use aws_sdk_appsync::types::{AuthenticationType, DefaultAction, GraphqlApi, UserPoolConfig};
    AppSyncApi::from_sdk(
        &GraphqlApi::builder()
            .api_id(API_ID)
            .name("storefront-graphql")
            .arn(format!("arn:aws:appsync:us-east-1:123456789012:apis/{API_ID}"))
            .authentication_type(AuthenticationType::AmazonCognitoUserPools)
            .user_pool_config(
                UserPoolConfig::builder()
                    .user_pool_id("us-east-1_AbCdEf123")
                    .aws_region("us-east-1")
                    .default_action(DefaultAction::Allow)
                    .build()
                    .unwrap(),
            )
            .uris("GRAPHQL", "https://abc.appsync-api.us-east-1.amazonaws.com/graphql")
            .tags("team", "storefront")
            .build(),
    )
}

fn section_idx(label: &str) -> usize {
    APPSYNC_API_SECTIONS
        .sections
        .iter()
        .position(|s| s.label == label)
        .expect("section label")
}

fn screen_of(app: &App) -> String {
    let backend = TestBackend::new(170, 45);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| crate::render_app(app, f)).unwrap();
    let buf = terminal.backend().buffer();
    (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn overview_renders_and_auth_names_the_user_pool() {
    let (mut app, tx, _rx) = test_app().await;
    select_mock(&mut app, ServiceType::AppSync, Box::new(api()));
    app.details_focused = true;
    app.layout_mode = crate::app::LayoutMode::DetailsOnly;
    app.reset_detail_section_to_default(&tx);

    let screen = screen_of(&app);
    for needle in ["AppSync API", "storefront-graphql", "Cognito user pools", "Data Sources", "API Keys"] {
        assert!(screen.contains(needle), "missing {needle:?} in:\n{screen}");
    }
    // Overview fetches nothing.
    assert!(app.lazy.appsync_data_sources.get(API_ID).is_none());

    app.detail_section_idx = section_idx("Auth");
    let lines = app.get_detail_lines();
    assert!(lines.iter().any(|(k, v)| k == "  User Pool" && v == "us-east-1_AbCdEf123"));
    let t = app
        .appsync_row_jump_target("  User Pool", "us-east-1_AbCdEf123")
        .expect("user pool row");
    assert_eq!(t.service, ServiceType::Cognito);
    assert_eq!(t.id, "us-east-1_AbCdEf123");
}

#[tokio::test]
async fn data_sources_load_lazily_and_route_by_label() {
    let (mut app, tx, _rx) = test_app().await;
    select_mock(&mut app, ServiceType::AppSync, Box::new(api()));
    app.details_focused = true;
    app.reset_detail_section_to_default(&tx);

    app.set_detail_section(section_idx("Data Sources"), &tx);
    assert!(matches!(app.lazy.appsync_data_sources.get(API_ID), Some(Lazy::Loading)));

    app.lazy.appsync_data_sources.apply(
        API_ID.to_string(),
        Ok(vec![
            AppSyncDataSource {
                name: "CatalogTable".into(),
                ds_type: "AMAZON_DYNAMODB".into(),
                dynamodb_table: Some("catalog".into()),
                ..Default::default()
            },
            AppSyncDataSource {
                name: "Search".into(),
                ds_type: "AMAZON_OPENSEARCH_SERVICE".into(),
                opensearch_endpoint: Some("https://search-products-abc123.us-east-1.es.amazonaws.com".into()),
                ..Default::default()
            },
        ]),
    );
    let lines = app.get_detail_lines();
    assert!(lines.iter().any(|(k, v)| k == "  DynamoDB Table" && v == "catalog"));
    assert!(lines.iter().any(|(k, v)| k == "  OpenSearch Domain" && v == "products"));

    let t = app.appsync_row_jump_target("  DynamoDB Table", "catalog").expect("table row");
    assert_eq!(t.service, ServiceType::DynamoDb);
    let t = app.appsync_row_jump_target("  OpenSearch Domain", "products").expect("domain row");
    assert_eq!(t.service, ServiceType::OpenSearch);
    assert_eq!(t.id, "products");
}

#[tokio::test]
async fn resolvers_section_also_fetches_functions() {
    let (mut app, tx, _rx) = test_app().await;
    select_mock(&mut app, ServiceType::AppSync, Box::new(api()));
    app.details_focused = true;
    app.reset_detail_section_to_default(&tx);
    app.set_detail_section(section_idx("Resolvers"), &tx);
    assert!(matches!(app.lazy.appsync_resolvers.get(API_ID), Some(Lazy::Loading)));
    assert!(
        matches!(app.lazy.appsync_functions.get(API_ID), Some(Lazy::Loading)),
        "pipeline steps are named from the function list"
    );
}

#[tokio::test]
async fn api_keys_section_shows_only_masked_ids() {
    let (mut app, tx, _rx) = test_app().await;
    select_mock(&mut app, ServiceType::AppSync, Box::new(api()));
    app.details_focused = true;
    app.layout_mode = crate::app::LayoutMode::DetailsOnly;
    app.reset_detail_section_to_default(&tx);
    app.set_detail_section(section_idx("API Keys"), &tx);
    assert_eq!(
        AppSyncApiDetailSection::from_index(app.detail_section_idx),
        AppSyncApiDetailSection::ApiKeys
    );

    // What fetch_api_keys stores: the id is already masked.
    let masked = crate::aws::services::appsync::mask_api_key_id("da2-abcdefghijklmnopqrstuvwxyz");
    app.lazy.appsync_api_keys.apply(
        API_ID.to_string(),
        Ok(vec![AppSyncApiKey {
            masked_id: masked.clone(),
            description: Some("mobile".into()),
            expires: 4_000_000_000,
            deletes: 4_100_000_000,
        }]),
    );
    let screen = screen_of(&app);
    assert!(screen.contains(&masked), "masked key row:\n{screen}");
    assert!(!screen.contains("abcdefgh"), "key value leaked:\n{screen}");

    // Exports carry the masked id too — never the value.
    let lines = app.get_detail_lines();
    let exported = crate::export::detail_value_for_test(&lines).to_string();
    assert!(exported.contains(&masked) && !exported.contains("abcdefgh"), "{exported}");
}

#[test]
fn api_arns_jump_to_the_api_row() {
    use crate::ui::widgets::details_pane::resource_jump_target;
    for arn in [
        format!("arn:aws:appsync:us-east-1:123456789012:apis/{API_ID}"),
        format!("arn:aws:appsync:us-east-1:123456789012:apis/{API_ID}/types/Query/resolvers/order"),
    ] {
        let t = resource_jump_target("ARN", &arn, ServiceType::Lambda).expect("appsync arn");
        assert_eq!(t.service, ServiceType::AppSync);
        assert_eq!(t.id, API_ID);
    }
}

/// A row naming a resource in another region switches region with the
/// jump; a same-region row doesn't ask for a switch.
#[tokio::test]
async fn cross_region_rows_switch_region_with_the_jump() {
    use crate::aws::region::Region;
    use aws_sdk_appsync::types::{AuthenticationType, DefaultAction, GraphqlApi, UserPoolConfig};
    let (mut app, tx, mut rx) = test_app().await;
    app.current_region = Region::UsEast1;
    let api = AppSyncApi::from_sdk(
        &GraphqlApi::builder()
            .api_id(API_ID)
            .name("storefront-graphql")
            .authentication_type(AuthenticationType::AmazonCognitoUserPools)
            .user_pool_config(
                UserPoolConfig::builder()
                    .user_pool_id("eu-west-1_AbCdEf123")
                    .aws_region("eu-west-1")
                    .default_action(DefaultAction::Allow)
                    .build()
                    .unwrap(),
            )
            .build(),
    );

    let press_on = |app: &mut App, label: &str| {
        let i = app
            .get_detail_lines_filtered()
            .iter()
            .position(|(k, _)| k == label)
            .unwrap_or_else(|| panic!("no {label:?} row"));
        app.details_selected_index = Some(i);
    };
    let switched_to = |rx: &mut mpsc::UnboundedReceiver<Event>| {
        let mut switched = None;
        while let Ok(ev) = rx.try_recv() {
            if let Event::RegionSwitchRequested { region } = ev {
                switched = Some(region);
            }
        }
        switched
    };

    // Cognito pool in eu-west-1.
    select_mock(&mut app, ServiceType::AppSync, Box::new(api.clone()));
    app.details_focused = true;
    app.reset_detail_section_to_default(&tx);
    app.set_detail_section(section_idx("Auth"), &tx);
    press_on(&mut app, "  User Pool");
    app.handle_key(key(KeyCode::Enter), &tx).await.unwrap();
    assert_eq!(app.current_service, Some(ServiceType::Cognito));
    assert_eq!(app.pending_jump_region, Some(Region::EuWest1));
    assert_eq!(switched_to(&mut rx), Some(Region::EuWest1));

    // DynamoDB table in ap-southeast-2, OpenSearch domain in the current region.
    for (label, want) in [("  DynamoDB Table", Some(Region::ApSoutheast2)), ("  OpenSearch Domain", None)] {
        select_mock(&mut app, ServiceType::AppSync, Box::new(api.clone()));
        app.pending_jump = None;
        app.pending_jump_region = None;
        app.details_focused = true;
        app.reset_detail_section_to_default(&tx);
        app.lazy.appsync_data_sources.apply(
            API_ID.to_string(),
            Ok(vec![
                AppSyncDataSource {
                    name: "CatalogTable".into(),
                    ds_type: "AMAZON_DYNAMODB".into(),
                    dynamodb_table: Some("catalog".into()),
                    region: Some("ap-southeast-2".into()),
                    ..Default::default()
                },
                AppSyncDataSource {
                    name: "Search".into(),
                    ds_type: "AMAZON_OPENSEARCH_SERVICE".into(),
                    opensearch_endpoint: Some("https://search-products-abc123.us-east-1.es.amazonaws.com".into()),
                    region: Some("us-east-1".into()),
                    ..Default::default()
                },
            ]),
        );
        app.set_detail_section(section_idx("Data Sources"), &tx);
        press_on(&mut app, label);
        app.handle_key(key(KeyCode::Enter), &tx).await.unwrap();
        assert_eq!(app.pending_jump_region, want, "{label}");
        assert_eq!(switched_to(&mut rx), want, "{label}");
    }
}
