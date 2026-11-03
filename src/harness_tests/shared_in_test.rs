//! A resource shared in from another account reads as shared, not as one
//! made here — one rule (`shared_owner`) for every type: `⇄ shared` in the
//! list, `⇄ shared from <owner>` in the pane header, nothing otherwise. A
//! transit gateway's tabs that AWS truncates for a non-owner (only this
//! account's attachments, none of the owner's route tables) each say so in
//! one line.

use super::*;
use crate::aws::resource::shared_owner;
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

/// The pane header carries the fact, from the one central header builder.
#[tokio::test]
async fn a_shared_gateway_says_so_in_the_header() {
    let (mut app, tx, _rx) = app_on(OWNER).await;
    lines_in(&mut app, &tx, TgwDetailSection::Overview);
    assert!(screen(&app).contains(&format!("available  ·  ⇄ shared from {OWNER}")));
}

fn security_group(owner: &str) -> Box<dyn Resource> {
    Box::new(crate::aws::services::ec2::SecurityGroup::from_sdk(
        &aws_sdk_ec2::types::SecurityGroup::builder()
            .group_id("sg-0123456789abcdef0")
            .group_name("web")
            .vpc_id("vpc-0123456789abcdef0")
            .owner_id(owner)
            .build(),
    ))
}

/// A security group's header no longer names its owner on every group —
/// only one shared in from another account says anything.
#[tokio::test]
async fn a_security_group_header_names_the_owner_only_when_shared() {
    let (mut app, _tx, _rx) = app_on(HERE).await;
    select_mock(&mut app, ServiceType::EC2, security_group(HERE));
    let own = screen(&app);
    assert!(!own.contains(&format!("owner {HERE}")) && !own.contains("⇄ shared"), "{own}");

    select_mock(&mut app, ServiceType::EC2, security_group(OWNER));
    assert!(screen(&app).contains(&format!("⇄ shared from {OWNER}")));
}

/// AWS-managed prefix lists belong to AWS, not another account.
#[test]
fn an_aws_managed_prefix_list_is_not_shared() {
    let list = |owner: &str| {
        crate::aws::services::vpc::PrefixList::from_sdk(
            &aws_sdk_ec2::types::ManagedPrefixList::builder()
                .prefix_list_id("pl-0123456789abcdef0")
                .owner_id(owner)
                .build(),
        )
    };
    assert_eq!(shared_owner(&list("AWS"), Some(HERE)), None);
    assert_eq!(shared_owner(&list(OWNER), Some(HERE)), Some(OWNER));
    assert_eq!(shared_owner(&list(HERE), Some(HERE)), None);
}

/// A type whose API says `SHARED_WITH_ME` is believed over the account
/// comparison, so it's marked even while the caller's account is unknown.
#[test]
fn a_share_status_beats_the_owner_comparison() {
    use aws_sdk_route53resolver::types::{ResolverRule, ShareStatus};
    let rule = |status: ShareStatus, owner: &str| {
        crate::aws::services::route53resolver::ResolverRule::from_sdk(
            &ResolverRule::builder().id("rslvr-rr-1").share_status(status).owner_id(owner).build(),
        )
    };
    assert_eq!(shared_owner(&rule(ShareStatus::SharedWithMe, OWNER), None), Some(OWNER));
    // SHARED_BY_ME is ours, shared out: not shared in, whatever the owner.
    assert_eq!(shared_owner(&rule(ShareStatus::SharedByMe, OWNER), Some(HERE)), None);
    // AWS's own Internet Resolver rule isn't another account's.
    assert_eq!(shared_owner(&rule(ShareStatus::NotShared, "Route 53 Resolver"), Some(HERE)), None);
}

fn profile(status: aws_sdk_route53profiles::types::ShareStatus) -> Box<dyn Resource> {
    Box::new(crate::aws::services::route53profiles::Route53Profile::from_sdk(
        &aws_sdk_route53profiles::types::ProfileSummary::builder()
            .id("rp-0123456789abcdef0")
            .name("dns-baseline")
            .share_status(status)
            .build(),
    ))
}

/// The Profile header showed the raw share status as if it were a state.
#[tokio::test]
async fn a_profile_header_says_shared_not_the_raw_status() {
    use aws_sdk_route53profiles::types::ShareStatus;
    let (mut app, _tx, _rx) = app_on(HERE).await;
    select_mock(&mut app, ServiceType::Route53Profiles, profile(ShareStatus::NotShared));
    let own = header_row(&app, "Route53 Profile");
    assert!(!own.contains("NOT_SHARED") && !own.contains("⇄"), "{own}");

    select_mock(&mut app, ServiceType::Route53Profiles, profile(ShareStatus::SharedWithMe));
    let shared = header_row(&app, "Route53 Profile");
    assert!(shared.contains("⇄ shared") && !shared.contains("SHARED_WITH_ME"), "{shared}");
}

/// The detail pane's header row: the line under the border titled `pane`.
fn header_row(app: &App, pane: &str) -> String {
    let mut terminal = Terminal::new(TestBackend::new(200, 40)).unwrap();
    terminal.draw(|f| crate::render_app(app, f)).unwrap();
    let buf = terminal.backend().buffer();
    let rows: Vec<String> = (0..buf.area.height)
        .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect())
        .collect();
    let top = rows.iter().position(|r| r.contains(&format!("╭ {pane}"))).expect("pane border");
    // The right-hand pane only — the list shares the row.
    let row = &rows[top + 1];
    row.rsplit("││").next().unwrap_or(row).to_string()
}

#[test]
fn the_owner_hook_covers_the_other_shareable_types() {
    let dhcp = crate::aws::services::vpc::DhcpOptionsSet::from_sdk(
        &aws_sdk_ec2::types::DhcpOptions::builder().dhcp_options_id("dopt-1").owner_id(OWNER).build(),
    );
    assert_eq!(shared_owner(&dhcp, Some(HERE)), Some(OWNER));
    let vpc_sg = crate::aws::services::vpc::VpcSecurityGroup::from_sdk(
        &aws_sdk_ec2::types::SecurityGroup::builder().group_id("sg-1").owner_id(OWNER).build(),
    );
    assert_eq!(shared_owner(&vpc_sg, Some(HERE)), Some(OWNER));
    let snap = crate::aws::services::redshift::RedshiftSnapshot::from_cluster_sdk(
        &aws_sdk_redshift::types::Snapshot::builder().snapshot_identifier("snap-1").owner_account(OWNER).build(),
    );
    assert_eq!(shared_owner(&snap, Some(HERE)), Some(OWNER));
}
