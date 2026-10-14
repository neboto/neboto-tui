//! `@audit` (#124 stage 1): findings computed from warm caches, `⏎` into
//! the resource, `F` by severity, and nothing fetched along the way.

use super::*;
use aws_sdk_ec2::types::{IpPermission, IpRange, SecurityGroup as SdkSg};

fn open_group(id: &str, port: i32) -> Box<dyn Resource> {
    let perm = IpPermission::builder()
        .ip_protocol("tcp")
        .from_port(port)
        .to_port(port)
        .ip_ranges(IpRange::builder().cidr_ip("0.0.0.0/0").build())
        .build();
    Box::new(crate::aws::services::ec2::SecurityGroup::from_sdk(
        &SdkSg::builder()
            .group_id(id)
            .group_name(format!("{id}-name"))
            .ip_permissions(perm)
            .build(),
    ))
}

/// EC2 warm with an SSH-open group and a 443-open one; `@audit` opened.
async fn audit_over_warm_ec2() -> (App, mpsc::UnboundedSender<Event>, mpsc::UnboundedReceiver<Event>) {
    let (mut app, tx, rx) = test_app().await;
    let region = app.current_region;
    app.cache.insert(
        ServiceType::EC2,
        region,
        None,
        vec![open_group("sg-ssh", 22), open_group("sg-web", 443)],
    );
    app.switch_service(ServiceType::Audit);
    app.update_search();
    assert!(!app.loading, "@audit computes in place — no load");
    (app, tx, rx)
}

fn finding_ids(app: &App) -> Vec<String> {
    app.filtered_resources
        .iter()
        .map(|&i| &app.resources[i])
        .filter(|r| !r.state_label().is_empty())
        .map(|r| r.id().to_string())
        .collect()
}

#[tokio::test]
async fn findings_come_from_the_cache_worst_first_with_coverage_rows() {
    let (app, _tx, _rx) = audit_over_warm_ec2().await;
    assert_eq!(
        finding_ids(&app),
        vec!["sg-open-world:sg-ssh", "sg-open-world:sg-web"],
        "SSH (high) before 443 (low)"
    );
    // RDS / S3 / EKS / IAM aren't cached: each of their checks says so.
    let not_checked = app
        .resources
        .iter()
        .filter(|r| r.resource_type() == "Not checked")
        .count();
    assert!(not_checked >= 5, "got {not_checked} not-checked rows");
    assert!(app.cache.get(&ServiceType::Audit, &app.current_region, None).is_none(), "never cached");
}

#[tokio::test]
async fn enter_opens_the_resource_and_ctrl_o_comes_back() {
    let (mut app, tx, _rx) = audit_over_warm_ec2().await;
    app.selected_index = Some(0);
    app.handle_key(key(KeyCode::Enter), &tx).await.unwrap();
    assert_eq!(app.current_service, Some(ServiceType::EC2));
    assert_eq!(app.get_selected_resource_id().as_deref(), Some("sg-ssh"));
    assert!(app.details_focused, "lands in the group's detail pane");

    app.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL), &tx)
        .await
        .unwrap();
    assert_eq!(app.current_service, Some(ServiceType::Audit));
}

#[tokio::test]
async fn resource_row_in_the_finding_pane_jumps_too() {
    let (mut app, tx, _rx) = audit_over_warm_ec2().await;
    app.selected_index = Some(1);
    app.handle_key(key(KeyCode::Char('l')), &tx).await.unwrap();
    assert!(app.details_focused, "l reads the finding");
    assert_eq!(app.current_service, Some(ServiceType::Audit));
    let rows = app.get_detail_lines();
    let at = rows.iter().position(|(k, _)| k == "Resource ID").unwrap();
    assert_eq!(rows[at].1, "sg-web");
    app.details_selected_index = Some(at);
    app.handle_key(key(KeyCode::Enter), &tx).await.unwrap();
    assert_eq!(app.current_service, Some(ServiceType::EC2));
    assert_eq!(app.get_selected_resource_id().as_deref(), Some("sg-web"));
}

#[tokio::test]
async fn f_filters_by_severity_and_skips_coverage_rows() {
    let (mut app, tx, _rx) = audit_over_warm_ec2().await;
    app.handle_key(KeyEvent::new(KeyCode::Char('F'), KeyModifiers::SHIFT), &tx)
        .await
        .unwrap();
    assert_eq!(app.list_state_filter.as_deref(), Some("high"));
    assert_eq!(app.filtered_resources.len(), 1);
}

#[tokio::test]
async fn coverage_row_enter_reads_it_instead_of_jumping() {
    let (mut app, tx, _rx) = audit_over_warm_ec2().await;
    let at = app
        .filtered_resources
        .iter()
        .position(|&i| app.resources[i].resource_type() == "Not checked")
        .unwrap();
    app.selected_index = Some(at);
    app.handle_key(key(KeyCode::Enter), &tx).await.unwrap();
    assert_eq!(app.current_service, Some(ServiceType::Audit));
    assert!(app.details_focused);
}
