//! Ctrl-O back to an `@all` result list must not switch to (and reload) the
//! service the search was typed over. Such a load is discarded uncached while
//! @all holds the screen, so a lapsed cache there re-fetched it on every
//! round trip.

use super::*;

/// `@all <name>` typed over IAM (nothing cached there), with one EC2
/// security group warm in the cache and selected in the results.
async fn all_over_iam() -> (App, mpsc::UnboundedSender<Event>, mpsc::UnboundedReceiver<Event>, String) {
    let (mut app, tx, rx) = test_app().await;
    let group = all_mocks()
        .into_iter()
        .find(|(s, l, _)| *s == ServiceType::EC2 && *l == "SecurityGroup")
        .map(|(_, _, r)| r)
        .unwrap();
    let (id, name) = (group.id().to_string(), group.name().to_string());
    let region = app.current_region;
    app.cache.insert(ServiceType::EC2, region, None, vec![group]);
    select_mock(&mut app, ServiceType::IAM, all_mocks().remove(0).2);
    app.resources.clear();
    app.filtered_resources.clear();
    app.selected_index = None;
    app.search_query = format!("@all {name}");
    app.update_search();
    app.search_active = false;
    assert!(app.all_search_mode);
    assert_eq!(app.get_selected_resource_id().as_deref(), Some(id.as_str()));
    (app, tx, rx, id)
}

#[tokio::test]
async fn ctrl_o_back_to_all_results_stays_put_and_loads_nothing() {
    let (mut app, tx, _rx, id) = all_over_iam().await;

    // ⏎ commits: jump to the EC2 result.
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

#[tokio::test]
async fn l_peeks_in_place_and_h_returns_to_the_same_results() {
    let (mut app, tx, _rx, id) = all_over_iam().await;
    app.handle_key(key(KeyCode::Char('l')), &tx).await.unwrap();
    assert!(app.details_focused, "l opens the detail pane");
    assert!(app.all_search_mode, "…over the @all results, not in EC2");
    assert_eq!(app.current_service, Some(ServiceType::IAM));
    assert_eq!(app.get_selected_resource_id().as_deref(), Some(id.as_str()));
    assert!(!app.loading, "a peek fetches no list");

    app.handle_key(key(KeyCode::Char('h')), &tx).await.unwrap();
    assert!(!app.details_focused);
    assert!(app.all_search_mode);
    assert_eq!(app.get_selected_resource_id().as_deref(), Some(id.as_str()));
}

#[tokio::test]
async fn sub_tab_keys_do_nothing_to_the_service_underneath() {
    let (mut app, tx, _rx, _) = all_over_iam().await;
    let before = format!("{:?}", app.iam_view);
    for k in [KeyCode::Char('2'), KeyCode::Tab, KeyCode::BackTab] {
        app.handle_key(key(k), &tx).await.unwrap();
    }
    app.handle_key(KeyEvent::new(KeyCode::Char('L'), KeyModifiers::SHIFT), &tx).await.unwrap();
    assert_eq!(format!("{:?}", app.iam_view), before, "IAM's tab untouched");
    assert!(app.all_search_mode);
    assert_eq!(app.current_service, Some(ServiceType::IAM));
}

#[tokio::test]
async fn r_under_all_reloads_sections_not_the_hidden_service() {
    let (mut app, tx, _rx, id) = all_over_iam().await;
    app.handle_key(key(KeyCode::Char('l')), &tx).await.unwrap();
    let epoch = app.lazy.epoch();
    app.handle_key(key(KeyCode::Char('r')), &tx).await.unwrap();
    assert!(app.lazy.epoch() > epoch, "section data dropped for a refetch");
    assert!(!app.loading, "IAM, underneath, wasn't reloaded");
    assert!(app.all_search_mode && app.details_focused);
    assert_eq!(app.get_selected_resource_id().as_deref(), Some(id.as_str()));

    // From the list too: the rows are rebuilt from the caches, nothing fetched.
    app.handle_key(key(KeyCode::Char('h')), &tx).await.unwrap();
    app.handle_key(key(KeyCode::Char('r')), &tx).await.unwrap();
    assert!(!app.loading);
    assert!(app.all_search_mode);
    assert_eq!(app.get_selected_resource_id().as_deref(), Some(id.as_str()));
}

const ALL_CHIP: &str = " @all  │ ";

fn top_row(app: &App) -> String {
    let mut terminal = Terminal::new(TestBackend::new(200, 30)).unwrap();
    terminal.draw(|f| crate::render_app(app, f)).unwrap();
    let buf = terminal.backend().buffer().clone();
    // The service strip: the row carrying the `@all` chip, if any.
    (0..buf.area.height)
        .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>())
        .find(|row| row.contains(ALL_CHIP))
        .unwrap_or_default()
}

#[tokio::test]
async fn service_strip_counts_matches_and_marks_what_wasnt_searched() {
    let (mut app, _tx, _rx, _) = all_over_iam().await;
    // S3: warm but empty — searched, no match. Lambda: visited, cache gone.
    let region = app.current_region;
    app.cache.insert(ServiceType::S3, region, None, Vec::new());
    app.visited_services.insert(ServiceType::Lambda);
    let query = app.search_query.clone();
    app.search_query = query;
    app.update_search();

    let counts = app.all_search_match_counts();
    assert_eq!(counts.get(&ServiceType::EC2), Some(&1));
    assert!(app.all_search_searched.contains(&ServiceType::S3));
    assert!(!app.all_search_searched.contains(&ServiceType::Lambda));

    let row = top_row(&app);
    assert!(row.contains(ALL_CHIP), "{row}");
    for (svc, label) in [
        (ServiceType::EC2, "1"),
        (ServiceType::S3, "0"),
        (ServiceType::Lambda, "–"),
        (ServiceType::IAM, "–"),
    ] {
        let chip = format!(" {} {} ", svc.short_name(), label);
        assert!(row.contains(&chip), "missing {chip:?} in {row}");
    }

    // Leaving @all restores the plain strip.
    app.search_query.clear();
    app.update_search();
    assert!(!app.all_search_mode);
    assert!(top_row(&app).is_empty(), "the @all chip is gone");
}


/// `@all mock` over Lambda, with every EC2 and IAM mock warm in the cache —
/// two services with several matches each.
async fn all_over_two_services() -> (App, mpsc::UnboundedSender<Event>, mpsc::UnboundedReceiver<Event>) {
    let (mut app, tx, rx) = test_app().await;
    let region = app.current_region;
    for svc in [ServiceType::EC2, ServiceType::IAM] {
        let rows: Vec<Box<dyn Resource>> =
            all_mocks().into_iter().filter(|(s, _, _)| *s == svc).map(|(_, _, r)| r).collect();
        app.cache.insert(svc, region, None, rows);
    }
    let lambda = all_mocks().into_iter().find(|(s, _, _)| *s == ServiceType::Lambda).unwrap().2;
    select_mock(&mut app, ServiceType::Lambda, lambda);
    app.search_query = "@all mock".to_string();
    app.update_search();
    app.search_active = false;
    assert!(app.all_search_mode);
    (app, tx, rx)
}

fn screen_rows(app: &App) -> Vec<Vec<String>> {
    let mut terminal = Terminal::new(TestBackend::new(200, 30)).unwrap();
    terminal.draw(|f| crate::render_app(app, f)).unwrap();
    let buf = terminal.backend().buffer().clone();
    (0..buf.area.height)
        .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect())
        .collect()
}

/// The strip's click target for `@all` chip `filter`, and the text drawn
/// under it.
fn chip_under_click(app: &App, filter: Option<ServiceType>) -> (ClickAction, String) {
    let rows = screen_rows(app);
    let regions = app.click_regions.borrow();
    let r = regions
        .iter()
        .find(|r| matches!(r.action, ClickAction::AllSearchFilter(f) if f == filter))
        .unwrap_or_else(|| panic!("no click region for {filter:?}"));
    let row = &rows[r.rect.y as usize];
    let text: String = row[r.rect.x as usize..(r.rect.x + r.rect.width) as usize].concat();
    (r.action, text)
}

fn sources(app: &App) -> Vec<ServiceType> {
    app.filtered_resources.iter().map(|&i| app.all_search_sources[i]).collect()
}

#[tokio::test]
async fn a_strip_chip_click_shows_only_that_service() {
    let (mut app, tx, _rx) = all_over_two_services().await;
    let counts = app.all_search_match_counts();
    let (ec2, iam) = (counts[&ServiceType::EC2], counts[&ServiceType::IAM]);
    assert!(ec2 > 0 && iam > 0, "{counts:?}");

    // Each click target sits exactly on its chip.
    let (action, text) = chip_under_click(&app, Some(ServiceType::IAM));
    assert_eq!(text, format!(" {} {} ", ServiceType::IAM.short_name(), iam));
    assert_eq!(chip_under_click(&app, None).1, " @all ");

    app.apply_click_action(action, &tx).await.unwrap();
    assert_eq!(app.current_service, Some(ServiceType::Lambda), "no service switch underneath");
    assert!(app.all_search_mode);
    assert_eq!(app.filtered_resources.len(), iam);
    assert!(sources(&app).iter().all(|s| *s == ServiceType::IAM));
    assert_eq!(app.all_search_match_counts()[&ServiceType::EC2], ec2, "other chips keep their counts");
    let title = screen_rows(&app).iter().map(|r| r.concat()).find(|r| r.contains("All services")).unwrap();
    assert!(title.contains(&format!("{} only", ServiceType::IAM.short_name())), "{title}");

    // The same chip again, or `@all`, shows everything.
    let (action, _) = chip_under_click(&app, Some(ServiceType::IAM));
    app.apply_click_action(action, &tx).await.unwrap();
    assert_eq!(app.filtered_resources.len(), ec2 + iam);
    app.apply_click_action(ClickAction::AllSearchFilter(Some(ServiceType::EC2)), &tx).await.unwrap();
    let (action, _) = chip_under_click(&app, None);
    app.apply_click_action(action, &tx).await.unwrap();
    assert_eq!(app.filtered_resources.len(), ec2 + iam);

    // Leaving @all forgets the pick.
    app.apply_click_action(ClickAction::AllSearchFilter(Some(ServiceType::EC2)), &tx).await.unwrap();
    app.search_query.clear();
    app.update_search();
    assert_eq!(app.all_search_service_filter, None);
}

#[tokio::test]
async fn z_sorts_all_results_and_can_group_them_by_service() {
    let (mut app, tx, _rx) = all_over_two_services().await;
    let names = |app: &App| -> Vec<String> {
        app.filtered_resources.iter().map(|&i| app.resources[i].name().to_lowercase()).collect()
    };

    app.handle_key(key(KeyCode::Char('z')), &tx).await.unwrap();
    assert_eq!(app.list_sort, ListSort::NameAsc);
    let got = names(&app);
    let mut want = got.clone();
    want.sort();
    assert_eq!(got, want, "name ↑ applies over the @all query");

    for _ in 0..2 {
        app.handle_key(key(KeyCode::Char('z')), &tx).await.unwrap();
    }
    app.handle_key(key(KeyCode::Char('z')), &tx).await.unwrap();
    assert_eq!(app.list_sort, ListSort::Service);
    let got: Vec<String> = sources(&app).iter().map(|s| s.name().to_string()).collect();
    let mut want = got.clone();
    want.sort();
    assert_eq!(got, want, "grouped by service");

    app.handle_key(key(KeyCode::Char('z')), &tx).await.unwrap();
    assert_eq!(app.list_sort, ListSort::Default);
}
