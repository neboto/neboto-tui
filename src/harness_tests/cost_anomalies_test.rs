//! Cost Anomalies view (issue #100): `8` switches the Cost list to anomalies
//! (own cache variant), `1`–`7` / `t` return to spend at the grouping the
//! user left, `Tab` reaches Anomalies as a fifth stop, and the pane renders.

use super::*;
use crate::aws::services::cost::{CostAnomaly, CostGroupBy, CostPeriod};

fn press(c: char) -> Event {
    Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))
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
async fn eight_opens_anomalies_and_digits_return_to_spend() {
    let (mut app, tx, _rx) = test_app().await;
    select_mock(&mut app, ServiceType::Cost, Box::new(CostAnomaly::mock()));
    app.cost_group_by = CostGroupBy::Region;
    app.cost_period = CostPeriod::LastMonth;

    app.handle_event(press('8'), &tx).await.unwrap();
    assert!(app.cost_anomalies);
    assert_eq!(app.cache_variant_for_test(ServiceType::Cost).as_deref(), Some("Anomalies"));
    assert_eq!(app.cost_group_by, CostGroupBy::Region, "grouping kept for the way back");

    // `t` returns to spend without cycling the period.
    app.handle_event(press('t'), &tx).await.unwrap();
    assert!(!app.cost_anomalies);
    assert_eq!(app.cost_period, CostPeriod::LastMonth);
    assert_eq!(
        app.cache_variant_for_test(ServiceType::Cost).as_deref(),
        Some("Region-LastMonth")
    );

    app.handle_event(press('8'), &tx).await.unwrap();
    app.handle_event(press('1'), &tx).await.unwrap();
    assert!(!app.cost_anomalies);
    assert_eq!(app.cost_group_by, CostGroupBy::Service);
}

#[test]
fn tab_cycle_includes_anomalies_as_a_fifth_stop() {
    use CostGroupBy as G;
    assert_eq!(App::cost_tab_step(G::UsageType, false, true, false, false), (G::UsageType, true));
    assert_eq!(App::cost_tab_step(G::UsageType, true, true, false, false), (G::Service, false));
    assert_eq!(App::cost_tab_step(G::Service, false, false, false, false), (G::Service, true));
    assert_eq!(App::cost_tab_step(G::Service, true, false, false, false), (G::UsageType, false));
    assert_eq!(App::cost_tab_step(G::Region, false, true, false, false), (G::UsageType, false));
}

#[tokio::test]
async fn anomalies_chip_row_and_pane_render() {
    let (mut app, tx, _rx) = test_app().await;
    select_mock(&mut app, ServiceType::Cost, Box::new(CostAnomaly::mock()));
    app.cost_anomalies = true;
    let screen = screen_of(&app);
    assert!(screen.contains("Anomalies"), "the 8 chip is on the tab row");
    assert!(screen.contains("$96.40 over"), "the list row shows the impact, not the UUID");
    assert!(screen.contains("ongoing"), "wide list carries the state word");

    app.focus_details_panel(&tx);
    let screen = screen_of(&app);
    assert!(screen.contains("Root causes"));
    assert!(screen.contains("Total impact"));
}
