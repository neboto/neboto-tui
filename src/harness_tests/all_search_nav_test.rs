//! Ctrl-O back to an `@all` result list must not switch to (and reload) the
//! service the search was typed over. Such a load is discarded uncached while
//! @all holds the screen, so a lapsed cache there re-fetched it on every
//! round trip.

use super::*;

#[tokio::test]
async fn ctrl_o_back_to_all_results_stays_put_and_loads_nothing() {
    let (mut app, tx, _rx) = test_app().await;
    let group = all_mocks()
        .into_iter()
        .find(|(s, l, _)| *s == ServiceType::EC2 && *l == "SecurityGroup")
        .map(|(_, _, r)| r)
        .unwrap();
    let (id, name) = (group.id().to_string(), group.name().to_string());

    // EC2 is warm; IAM, the service on screen, has nothing cached.
    let region = app.current_region;
    app.cache.insert(ServiceType::EC2, region, None, vec![group]);
    select_mock(&mut app, ServiceType::IAM, all_mocks().remove(0).2);
    app.resources.clear();
    app.filtered_resources.clear();
    app.selected_index = None;

    // `@all <name>` over IAM, then ⏎ to the EC2 result.
    app.search_query = format!("@all {name}");
    app.update_search();
    assert!(app.all_search_mode);
    assert_eq!(app.get_selected_resource_id().as_deref(), Some(id.as_str()));
    app.search_active = false;
    app.handle_key(key(KeyCode::Enter), &tx).await.unwrap();
    assert_eq!(app.current_service, Some(ServiceType::EC2));
    assert!(!app.all_search_mode);
    assert!(!app.loading, "EC2 came from the cache");

    // Ctrl-O: back on the result list, same row, and no service switch.
    app.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL), &tx)
        .await
        .unwrap();
    assert!(app.all_search_mode);
    assert_eq!(app.get_selected_resource_id().as_deref(), Some(id.as_str()));
    assert_eq!(app.current_service, Some(ServiceType::EC2), "didn't switch back to IAM");
    assert!(!app.loading, "nothing was fetched");
}
