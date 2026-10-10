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
async fn seven_opens_anomalies_and_digits_return_to_spend() {
    let (mut app, tx, _rx) = test_app().await;
    select_mock(&mut app, ServiceType::Cost, Box::new(CostAnomaly::mock()));
    app.cost_group_by = CostGroupBy::Region;
    app.cost_period = CostPeriod::LastMonth;

    app.handle_event(press('7'), &tx).await.unwrap();
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

    app.handle_event(press('7'), &tx).await.unwrap();
    app.handle_event(press('1'), &tx).await.unwrap();
    assert!(!app.cost_anomalies);
    assert_eq!(app.cost_group_by, CostGroupBy::Service);
}

#[tokio::test]
async fn brackets_step_the_period_and_stop_at_the_ends() {
    let (mut app, tx, _rx) = test_app().await;
    select_mock(&mut app, ServiceType::Cost, Box::new(CostAnomaly::mock()));
    app.cost_anomalies = false;
    assert_eq!(app.cost_period, CostPeriod::Mtd);

    app.handle_event(press(']'), &tx).await.unwrap();
    assert_eq!(app.cost_period, CostPeriod::LastMonth);
    app.handle_event(press(']'), &tx).await.unwrap();
    app.handle_event(press(']'), &tx).await.unwrap();
    assert_eq!(app.cost_period, CostPeriod::Last3Months, "] stops at the widest");
    app.handle_event(press('['), &tx).await.unwrap();
    assert_eq!(app.cost_period, CostPeriod::LastMonth);

    // From Anomalies a bracket returns to spend at the period left.
    app.handle_event(press('7'), &tx).await.unwrap();
    app.handle_event(press('['), &tx).await.unwrap();
    assert!(!app.cost_anomalies);
    assert_eq!(app.cost_period, CostPeriod::LastMonth);
}

#[test]
fn tab_cycle_includes_anomalies_as_the_last_stop() {
    use CostGroupBy as G;
    assert_eq!(App::cost_tab_step(G::CostCategory, false, true), (G::CostCategory, true));
    assert_eq!(App::cost_tab_step(G::CostCategory, true, true), (G::Service, false));
    assert_eq!(App::cost_tab_step(G::Service, false, false), (G::Service, true));
    assert_eq!(App::cost_tab_step(G::Service, true, false), (G::CostCategory, false));
    assert_eq!(App::cost_tab_step(G::Region, false, true), (G::UsageType, false));
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
