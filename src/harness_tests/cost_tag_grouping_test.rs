//! Cost grouping by cost-allocation tag (`9`) and cost category (`0`), #101:
//! the first press opens the key picker, `⏎` groups by the pick (its own
//! per-key cache variant), the key is remembered, `Tab` reaches the keyed
//! stops only once chosen, and the chip row names the key.

use super::*;
use crate::aws::services::cost::{CostGroupBy, CostLineItem, CostPeriod};

fn press(c: char) -> Event {
    Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))
}

fn code(c: KeyCode) -> Event {
    Event::Key(KeyEvent::new(c, KeyModifiers::NONE))
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

/// Seed the picker's list as if `GetTags` had answered (no network here).
fn seed_tag_keys(app: &mut App, keys: &[&str]) {
    app.lazy.cost_group_keys.apply(
        "tags".to_string(),
        Ok(keys.iter().map(|k| k.to_string()).collect()),
    );
}

#[tokio::test]
async fn nine_opens_the_picker_and_enter_groups_by_the_tag() {
    let (mut app, tx, _rx) = test_app().await;
    select_mock(&mut app, ServiceType::Cost, Box::new(CostLineItem::mock()));
    seed_tag_keys(&mut app, &["CostCenter", "env", "team"]);

    app.handle_event(press('9'), &tx).await.unwrap();
    assert!(app.cost_key_picker.visible, "no key yet → picker");
    assert_eq!(app.cost_group_by, CostGroupBy::Service, "nothing switches until a pick");
    let screen = screen_of(&app);
    assert!(screen.contains("Group cost by tag"));
    assert!(screen.contains("CostCenter"));

    for c in "tea".chars() {
        app.handle_event(press(c), &tx).await.unwrap();
    }
    app.handle_event(code(KeyCode::Enter), &tx).await.unwrap();
    assert!(!app.cost_key_picker.visible);
    assert_eq!(app.cost_group_by, CostGroupBy::Tag);
    assert_eq!(app.cost_tag_key.as_deref(), Some("team"));
    assert_eq!(
        app.cache_variant_for_test(ServiceType::Cost).as_deref(),
        Some("Tag:team-Mtd"),
        "the key is part of the variant, or team and env would share a result set"
    );

    // Back to Service, then `9` returns straight to the remembered key.
    app.handle_event(press('1'), &tx).await.unwrap();
    assert_eq!(app.cost_group_by, CostGroupBy::Service);
    app.handle_event(press('9'), &tx).await.unwrap();
    assert!(!app.cost_key_picker.visible, "remembered key → no picker");
    assert_eq!(app.cost_group_by, CostGroupBy::Tag);

    // `9` again while showing the tag view re-opens the picker.
    app.handle_event(press('9'), &tx).await.unwrap();
    assert!(app.cost_key_picker.visible);
    app.handle_event(code(KeyCode::Esc), &tx).await.unwrap();
    assert!(!app.cost_key_picker.visible);
    assert_eq!(app.cost_tag_key.as_deref(), Some("team"), "Esc keeps the key");
}

#[tokio::test]
async fn enter_with_no_match_groups_by_the_typed_category() {
    let (mut app, tx, _rx) = test_app().await;
    select_mock(&mut app, ServiceType::Cost, Box::new(CostLineItem::mock()));
    app.lazy
        .cost_group_keys
        .apply("categories".to_string(), Ok(Vec::new()));
    app.cost_period = CostPeriod::LastMonth;

    app.handle_event(press('0'), &tx).await.unwrap();
    assert!(app.cost_key_picker.visible);
    assert!(screen_of(&app).contains("No cost categories defined"));
    for c in "Team".chars() {
        app.handle_event(press(c), &tx).await.unwrap();
    }
    app.handle_event(code(KeyCode::Enter), &tx).await.unwrap();
    assert_eq!(app.cost_group_by, CostGroupBy::CostCategory);
    assert_eq!(app.cost_category.as_deref(), Some("Team"));
    assert_eq!(
        app.cache_variant_for_test(ServiceType::Cost).as_deref(),
        Some("CostCategory:Team-LastMonth")
    );
}

#[test]
fn tab_cycle_adds_keyed_stops_only_once_chosen() {
    use CostGroupBy as G;
    // No keys: Usage Type → Anomalies, as before.
    assert_eq!(App::cost_tab_step(G::UsageType, false, true, false, false), (G::UsageType, true));
    // Tag chosen: Usage Type → Tag → Anomalies.
    assert_eq!(App::cost_tab_step(G::UsageType, false, true, true, false), (G::Tag, false));
    assert_eq!(App::cost_tab_step(G::Tag, false, true, true, false), (G::Tag, true));
    // Both: Tag → Category; backward from Anomalies lands on Category.
    assert_eq!(App::cost_tab_step(G::Tag, false, true, true, true), (G::CostCategory, false));
    assert_eq!(App::cost_tab_step(G::Service, true, false, true, true), (G::CostCategory, false));
    // Forward from Anomalies wraps to Service.
    assert_eq!(App::cost_tab_step(G::Tag, true, true, true, true), (G::Service, false));
}

#[tokio::test]
async fn chip_row_names_the_chosen_key() {
    let (mut app, _tx, _rx) = test_app().await;
    select_mock(&mut app, ServiceType::Cost, Box::new(CostLineItem::mock()));
    let screen = screen_of(&app);
    assert!(screen.contains("Tag") && screen.contains("Category"), "both chips render keyless");
    app.cost_tag_key = Some("team".to_string());
    app.cost_group_by = CostGroupBy::Tag;
    assert!(screen_of(&app).contains("Tag:team"));
}
