//! Cost grouping by cost-allocation tag (`5`) and cost category (`6`), #101:
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
async fn five_opens_the_picker_and_enter_groups_by_the_tag() {
    let (mut app, tx, _rx) = test_app().await;
    select_mock(&mut app, ServiceType::Cost, Box::new(CostLineItem::mock()));
    seed_tag_keys(&mut app, &["CostCenter", "env", "team"]);

    app.handle_event(press('5'), &tx).await.unwrap();
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

    // Back to Service, then `5` returns straight to the remembered key.
    app.handle_event(press('1'), &tx).await.unwrap();
    assert_eq!(app.cost_group_by, CostGroupBy::Service);
    app.handle_event(press('5'), &tx).await.unwrap();
    assert!(!app.cost_key_picker.visible, "remembered key → no picker");
    assert_eq!(app.cost_group_by, CostGroupBy::Tag);

    // `5` again while showing the tag view re-opens the picker.
    app.handle_event(press('5'), &tx).await.unwrap();
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

    app.handle_event(press('6'), &tx).await.unwrap();
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
fn tab_cycle_walks_the_row_in_order() {
    use CostGroupBy as G;
    // Tag / Category are ordinary stops, key or no key.
    let mut at = (G::Service, false);
    let mut seen = Vec::new();
    for _ in 0..7 {
        at = App::cost_tab_step(at.0, at.1, true);
        seen.push(at);
    }
    assert_eq!(
        seen,
        [
            (G::LinkedAccount, false),
            (G::Region, false),
            (G::UsageType, false),
            (G::Tag, false),
            (G::CostCategory, false),
            (G::CostCategory, true),
            (G::Service, false),
        ]
    );
}

#[tokio::test]
async fn tab_onto_a_keyless_tag_asks_for_a_key_and_enter_opens_the_picker() {
    let (mut app, tx, _rx) = test_app().await;
    select_mock(&mut app, ServiceType::Cost, Box::new(CostLineItem::mock()));
    seed_tag_keys(&mut app, &["team"]);
    app.cost_group_by = CostGroupBy::UsageType;

    app.handle_event(code(KeyCode::Tab), &tx).await.unwrap();
    assert_eq!(app.cost_group_by, CostGroupBy::Tag);
    assert!(!app.cost_key_picker.visible, "Tab never pops a modal");
    // The keyless load answers empty without a call; land it.
    app.resources.clear();
    app.filtered_resources.clear();
    app.loading = false;
    app.loading_progress = None;
    app.load_started_at = None;
    let screen = screen_at(&app, 110);
    assert!(screen.contains("pick which tag key") && screen.contains("⏎ or 5"), "{screen}");

    app.handle_event(code(KeyCode::Enter), &tx).await.unwrap();
    assert!(app.cost_key_picker.visible);
    app.handle_event(code(KeyCode::Enter), &tx).await.unwrap();
    assert_eq!(app.cost_tag_key.as_deref(), Some("team"));
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

/// The whole screen at `width` columns.
fn screen_at(app: &App, width: u16) -> String {
    let backend = TestBackend::new(width, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    app.click_regions.borrow_mut().clear();
    terminal.draw(|f| crate::render_app(app, f)).unwrap();
    let buf = terminal.backend().buffer();
    (0..buf.area.height)
        .map(|y| (0..width).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn the_row_is_numbered_in_order_and_periods_ride_the_detail_border() {
    let (mut app, tx, _rx) = test_app().await;
    select_mock(&mut app, ServiceType::Cost, Box::new(CostLineItem::mock()));

    // 110 columns: the digits read in screen order, like every sub-tab strip.
    let screen = screen_at(&app, 110);
    assert!(
        screen.contains("1 Service  │ 2 Account  │ 3 Region  │ 4 Usage Type  │ 5 Tag  │ 6 Category    7 Anomalies"),
        "{screen}"
    );
    // The periods sit on the Cost pane's top border, every one of them.
    let y = screen.lines().position(|l| l.contains("╭ Cost ─")).expect("cost pane border");
    let border: Vec<char> = screen.lines().nth(y).unwrap().chars().collect();
    let text: String = border.iter().collect();
    assert!(text.contains("[ MTD │ Last Mo │ 3 Mo ]"), "{text}");

    // …and a click sets the period directly.
    let col = (0..border.len())
        .find(|&i| border[i..].iter().collect::<String>().starts_with("Last Mo"))
        .unwrap() as u16;
    let action = app.click_region_at(col, y as u16).expect("period chip region");
    assert!(matches!(action, crate::app::ClickAction::CostPeriod(CostPeriod::LastMonth)));
    app.apply_click_action(action, &tx).await.unwrap();
    assert_eq!(app.cost_period, CostPeriod::LastMonth);

    // 85 columns: labels shorten so every grouping stays on screen.
    let screen = screen_at(&app, 85);
    let row = screen.lines().find(|l| l.contains(" By ")).unwrap();
    for chip in ["1 Svc", "2 Acct", "3 Region", "4 Usage", "5 Tag", "6 Cat", "7 Anomalies"] {
        assert!(row.contains(chip), "{chip} missing: {row}");
    }
    assert!(!row.contains('›'), "{row}");

    // An empty list (no spend under the tag) keeps the periods on the
    // placeholder pane, so the period can still be changed by mouse.
    app.resources.clear();
    app.filtered_resources.clear();
    let screen = screen_at(&app, 110);
    assert!(screen.contains("╭ Details ─") && screen.contains("Last Mo"), "{screen}");
}
