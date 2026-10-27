//! A `/` search that finds nothing on the current sub-tab follows the
//! matches to the first tab that has some, comes back when the starting tab
//! matches again (or on `Esc`), and the tabs with no matches are dimmed.

use super::*;
use crate::app::Ec2View;
use crate::aws::services::ec2::{Ec2Instance, SecurityGroup};

/// EC2 with an instance `webserver` and a security group `payments-sg`, on
/// the Instances tab.
async fn ec2_app() -> (App, mpsc::UnboundedSender<Event>, mpsc::UnboundedReceiver<Event>) {
    let (mut app, tx, rx) = test_app().await;
    let instance = Ec2Instance::from_sdk(
        &aws_sdk_ec2::types::Instance::builder()
            .instance_id("i-0abc")
            .tags(aws_sdk_ec2::types::Tag::builder().key("Name").value("webserver").build())
            .build(),
    );
    select_mock(&mut app, ServiceType::EC2, Box::new(instance));
    app.resources.push(Box::new(SecurityGroup::from_sdk(
        &aws_sdk_ec2::types::SecurityGroup::builder()
            .group_id("sg-0abc")
            .group_name("payments-sg")
            .build(),
    )));
    app.ec2_view = Ec2View::Instances;
    app.update_search();
    (app, tx, rx)
}

async fn press(app: &mut App, tx: &mpsc::UnboundedSender<Event>, code: KeyCode) {
    app.handle_key(key(code), tx).await.unwrap();
}

/// Type `text` into the open search and run the (normally debounced) filter.
async fn type_str(app: &mut App, tx: &mpsc::UnboundedSender<Event>, text: &str) {
    for c in text.chars() {
        press(app, tx, KeyCode::Char(c)).await;
    }
    app.update_search();
}

async fn clear_query(app: &mut App, tx: &mpsc::UnboundedSender<Event>) {
    while !app.search_query.is_empty() {
        press(app, tx, KeyCode::Backspace).await;
    }
    app.update_search();
}

#[tokio::test]
async fn sub_tab_table_follows_the_number_keys() {
    let (mut app, tx, _rx) = test_app().await;
    let mut seen = std::collections::HashSet::new();
    for (service, _, mock) in all_mocks() {
        if !seen.insert(service) {
            continue;
        }
        select_mock(&mut app, service, mock);
        let filters = app.sub_tab_filters();
        for (i, filter) in filters.iter().enumerate() {
            let digit = if i == 9 { '0' } else { char::from_digit(i as u32 + 1, 10).unwrap() };
            app.details_focused = false;
            app.handle_key(key(KeyCode::Char(digit)), &tx).await.unwrap();
            assert_eq!(
                app.active_type_filter(),
                Some(*filter),
                "{service:?}: key {digit} doesn't show sub_tab_table!'s tab {i} — \
                 the table's variants must be in key order"
            );
            assert_eq!(app.sub_tab_index(), Some(i), "{service:?}: key {digit}");
        }
    }
}

#[tokio::test]
async fn a_search_with_no_matches_here_follows_them_to_their_tab() {
    let (mut app, tx, _rx) = ec2_app().await;
    press(&mut app, &tx, KeyCode::Char('/')).await;
    type_str(&mut app, &tx, "payments").await;
    assert_eq!(app.ec2_view, Ec2View::SecurityGroups);
    assert_eq!(app.filtered_resources.len(), 1);
    assert_eq!(&app.sub_tab_match_counts[..2], &[0, 1]);

    // The starting tab matches again: back to it.
    clear_query(&mut app, &tx).await;
    assert_eq!(app.ec2_view, Ec2View::Instances, "an empty query is home");
    type_str(&mut app, &tx, "webserver").await;
    assert_eq!(app.ec2_view, Ec2View::Instances);
    assert_eq!(&app.sub_tab_match_counts[..2], &[1, 0], "{:?}", app.sub_tab_match_counts);
}

#[tokio::test]
async fn the_starting_tab_wins_while_it_has_matches() {
    let (mut app, tx, _rx) = ec2_app().await;
    app.ec2_view = Ec2View::SecurityGroups;
    press(&mut app, &tx, KeyCode::Char('/')).await;
    // `0abc` is in both ids.
    type_str(&mut app, &tx, "0abc").await;
    assert_eq!(app.ec2_view, Ec2View::SecurityGroups);
}

#[tokio::test]
async fn esc_goes_back_to_the_starting_tab_and_enter_stays() {
    let (mut app, tx, _rx) = ec2_app().await;
    press(&mut app, &tx, KeyCode::Char('/')).await;
    type_str(&mut app, &tx, "payments").await;
    assert_eq!(app.ec2_view, Ec2View::SecurityGroups);
    press(&mut app, &tx, KeyCode::Esc).await;
    app.update_search();
    assert_eq!(app.ec2_view, Ec2View::Instances);
    assert!(app.sub_tab_match_counts.is_empty());

    press(&mut app, &tx, KeyCode::Char('/')).await;
    type_str(&mut app, &tx, "payments").await;
    press(&mut app, &tx, KeyCode::Enter).await;
    app.update_search();
    assert_eq!(app.ec2_view, Ec2View::SecurityGroups, "⏎ keeps the tab it found");

    // A tab picked by hand afterwards is left alone, even with no matches.
    press(&mut app, &tx, KeyCode::Char('1')).await;
    assert_eq!(app.ec2_view, Ec2View::Instances);
    assert!(app.filtered_resources.is_empty());
}

#[tokio::test]
async fn tabs_with_no_matches_are_dimmed() {
    let (mut app, tx, _rx) = ec2_app().await;
    let mut terminal = Terminal::new(TestBackend::new(200, 40)).unwrap();
    // The colour the "Instances" chip label is drawn in.
    let mut label_fg = |app: &App| {
        terminal.draw(|f| crate::render_app(app, f)).unwrap();
        let buf = terminal.backend().buffer().clone();
        let w = buf.area.width as usize;
        let cells = buf.content();
        let text: String = cells.iter().map(|c| c.symbol()).collect();
        let at = text.find("1 Instances").expect("Instances chip on screen");
        let i = text[..at].chars().count() + 2;
        assert!(i % w < w - 1);
        cells[i].fg
    };
    app.ec2_view = Ec2View::SecurityGroups;
    app.update_search();
    let normal = label_fg(&app);
    press(&mut app, &tx, KeyCode::Char('/')).await;
    type_str(&mut app, &tx, "payments").await;
    assert_eq!(app.ec2_view, Ec2View::SecurityGroups);
    assert_eq!(label_fg(&app), crate::ui::theme::text_dim(), "no matches on Instances");
    assert_ne!(normal, crate::ui::theme::text_dim());
    // No numbers added to the labels.
    let screen: String = terminal.backend().buffer().content().iter().map(|c| c.symbol()).collect();
    assert!(screen.contains("1 Instances  │ 2 Security Groups  │"), "{screen}");
}
