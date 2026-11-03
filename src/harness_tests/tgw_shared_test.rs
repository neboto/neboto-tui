//! A transit gateway shared in through RAM reads as shared, not as one made
//! here: the list row leads with `⇄ shared`, the Owner row says so, and the
//! two tabs AWS truncates for a non-owner (only this account's attachments,
//! none of the owner's route tables) each say so in one line.

use super::*;
use crate::aws::services::transit_gateway::{TgwDetailSection, TransitGateway};

const TGW_ID: &str = "tgw-0123456789abcdef0";
const HERE: &str = "222233334444";
const OWNER: &str = "111122223333";

fn gateway(owner: &str) -> TransitGateway {
    named(owner, TGW_ID, "core-network")
}

fn named(owner: &str, id: &str, name: &str) -> TransitGateway {
    use aws_sdk_ec2::types::{AutoAcceptSharedAttachmentsValue, TransitGatewayOptions};
    TransitGateway::from_sdk(
        &aws_sdk_ec2::types::TransitGateway::builder()
            .transit_gateway_id(id)
            .owner_id(owner)
            .state(aws_sdk_ec2::types::TransitGatewayState::Available)
            .description("core network")
            .tags(aws_sdk_ec2::types::Tag::builder().key("Name").value(name).build())
            .options(
                TransitGatewayOptions::builder()
                    .auto_accept_shared_attachments(AutoAcceptSharedAttachmentsValue::Enable)
                    .build(),
            )
            .build(),
    )
}

async fn app_on(owner: &str) -> (App, mpsc::UnboundedSender<Event>, mpsc::UnboundedReceiver<Event>) {
    let (mut app, tx, rx) = test_app().await;
    app.account_id = Some(HERE.to_string());
    select_mock(&mut app, ServiceType::TransitGateway, Box::new(gateway(owner)));
    (app, tx, rx)
}

fn screen(app: &App) -> String {
    let mut terminal = Terminal::new(TestBackend::new(200, 40)).unwrap();
    terminal.draw(|f| crate::render_app(app, f)).unwrap();
    terminal.backend().buffer().content().iter().map(|c| c.symbol()).collect()
}

fn lines_in(app: &mut App, tx: &mpsc::UnboundedSender<Event>, section: TgwDetailSection) -> Vec<(String, String)> {
    app.details_focused = true;
    app.set_detail_section(section as usize, tx);
    app.get_detail_lines()
}

#[tokio::test]
async fn a_shared_gateway_is_marked_in_the_list() {
    let (app, _tx, _rx) = app_on(OWNER).await;
    assert!(screen(&app).contains("⇄ shared  available"));

    let (own, _tx, _rx) = app_on(HERE).await;
    assert!(!screen(&own).contains("⇄ shared"), "an own gateway isn't marked");
}

/// Without the caller's account there's nothing to compare against, so
/// nothing is marked rather than guessed.
#[tokio::test]
async fn an_unknown_account_marks_nothing() {
    let (mut app, _tx, _rx) = app_on(OWNER).await;
    app.account_id = None;
    assert!(!screen(&app).contains("⇄ shared"));
}

#[tokio::test]
async fn a_shared_gateway_says_who_owns_it_and_what_is_missing() {
    let (mut app, tx, _rx) = app_on(OWNER).await;

    let overview = lines_in(&mut app, &tx, TgwDetailSection::Overview);
    assert!(overview.contains(&("Owner".to_string(), OWNER.to_string())), "{overview:#?}");
    // The owner's setting, not a warning about this account.
    assert!(
        overview.iter().any(|(k, v)| k == "Auto-accept Shared" && v == "enabled · the owner's setting"),
        "{overview:#?}",
    );

    let attachments = lines_in(&mut app, &tx, TgwDetailSection::Attachments);
    assert_eq!(attachments[0].0, "Attachments from this account (0)");
    assert!(attachments.iter().any(|(k, _)| k == "  No attachments from this account"), "{attachments:#?}");

    let tables = lines_in(&mut app, &tx, TgwDetailSection::RouteTables);
    assert_eq!(tables[0].0, "Route Tables", "no count of 0 for tables we can't list");
    assert!(tables.iter().any(|(_, v)| v == &format!("· only the owner, {OWNER}, can list these")), "{tables:#?}");
    assert!(!tables.iter().any(|(k, _)| k == "  No route tables"), "{tables:#?}");
}

#[tokio::test]
async fn an_own_gateway_reads_as_before() {
    let (mut app, tx, _rx) = app_on(HERE).await;
    let overview = lines_in(&mut app, &tx, TgwDetailSection::Overview);
    assert!(overview.contains(&("Owner".to_string(), HERE.to_string())), "{overview:#?}");
    assert!(overview.iter().any(|(k, v)| k == "Auto-accept Shared" && v.starts_with("⚠ enabled")), "{overview:#?}");
    let tables = lines_in(&mut app, &tx, TgwDetailSection::RouteTables);
    assert!(tables.iter().any(|(k, _)| k == "  No route tables"), "{tables:#?}");
}

/// Prints the list and detail pane for a shared gateway — run with
/// `--ignored --nocapture` to eyeball the layout.
#[tokio::test]
#[ignore]
async fn print_shared_gateway_screens() {
    let (mut app, tx, _rx) = app_on(OWNER).await;
    let own = named(HERE, "tgw-0fedcba9876543210", "edge");
    app.resources.push(Box::new(own));
    app.filtered_resources = vec![0, 1];
    for (focus, width) in [(false, 260), (true, 160)] {
        app.details_focused = focus;
        if focus {
            app.reset_detail_section_to_default(&tx);
        }
        let mut terminal = Terminal::new(TestBackend::new(width, 14)).unwrap();
        terminal.draw(|f| crate::render_app(&app, f)).unwrap();
        let buf = terminal.backend().buffer();
        for y in 0..buf.area.height {
            let row: String = (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect();
            println!("{}", row.trim_end());
        }
        println!();
    }
}
