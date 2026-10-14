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
    let labels = app.get_selected_resource().unwrap().detail_sections().unwrap().sections;
    assert_eq!(labels[app.detail_section_idx].label, "Inbound");

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

fn public_db() -> Box<dyn Resource> {
    Box::new(crate::aws::services::rds::RdsInstance::from_sdk(
        &aws_sdk_rds::types::DbInstance::builder()
            .db_instance_identifier("orders-db")
            .publicly_accessible(true)
            .storage_encrypted(true)
            .build(),
    ))
}

fn notes(app: &App) -> Vec<String> {
    app.resources
        .iter()
        .filter_map(|r| r.as_any().downcast_ref::<crate::audit::AuditFinding>())
        .filter(|f| f.kind == crate::audit::RowKind::NotChecked)
        .map(|f| format!("{}: {}", f.check_id, f.list_note))
        .collect()
}

#[tokio::test]
async fn r_loads_the_missing_sources_into_the_cache_and_re_audits() {
    let (mut app, tx, _rx) = audit_over_warm_ec2().await;
    app.handle_key(key(KeyCode::Char('r')), &tx).await.unwrap();

    // EC2 was warm, so only the other four load — and nothing is cleared.
    assert_eq!(
        app.audit_loading,
        vec![ServiceType::RDS, ServiceType::S3, ServiceType::Eks, ServiceType::IAM]
    );
    assert!(app.cache.get(&ServiceType::EC2, &app.current_region, None).is_some());
    assert!(!app.loading, "the audit view itself never loads");
    assert!(
        notes(&app).iter().any(|n| n == "rds-public: loading…"),
        "{:?}",
        notes(&app)
    );

    // RDS lands: cached, and its finding appears.
    let generation = app.audit_generation;
    app.handle_event(
        Event::AuditSourceLoaded { generation, service: ServiceType::RDS, result: Ok((vec![public_db()], vec![])) },
        &tx,
    )
    .await
    .unwrap();
    assert!(app.cache.get(&ServiceType::RDS, &app.current_region, None).is_some());
    assert!(finding_ids(&app).contains(&"rds-public:orders-db".to_string()), "{:?}", finding_ids(&app));
    assert!(!app.audit_loading.contains(&ServiceType::RDS));

    // A failed source reports once everything has landed, and its checks
    // stay "not checked" — never a pass.
    for svc in [ServiceType::S3, ServiceType::Eks, ServiceType::IAM] {
        let result = if svc == ServiceType::S3 { Err("AccessDenied".to_string()) } else { Ok((vec![], vec![])) };
        app.handle_event(Event::AuditSourceLoaded { generation, service: svc, result }, &tx)
            .await
            .unwrap();
    }
    assert!(app.audit_loading.is_empty());
    assert!(app.error_message.as_deref().unwrap_or("").contains("@s3: AccessDenied"));
    assert!(notes(&app).iter().any(|n| n.starts_with("s3-public-access-block: not checked")));
}

#[tokio::test]
async fn a_switch_drops_audit_loads_still_in_flight() {
    let (mut app, tx, _rx) = audit_over_warm_ec2().await;
    app.handle_key(key(KeyCode::Char('r')), &tx).await.unwrap();
    let stale = app.audit_generation;
    app.reset_account_scoped_state(); // what a profile / role switch runs
    app.handle_event(
        Event::AuditSourceLoaded { generation: stale, service: ServiceType::RDS, result: Ok((vec![public_db()], vec![])) },
        &tx,
    )
    .await
    .unwrap();
    assert!(
        app.cache.get(&ServiceType::RDS, &app.current_region, None).is_none(),
        "the old account's rows must not be cached"
    );
}

#[tokio::test]
async fn r_with_every_source_cached_says_so() {
    let (mut app, tx, _rx) = audit_over_warm_ec2().await;
    let region = app.current_region;
    for svc in [ServiceType::RDS, ServiceType::S3, ServiceType::Eks, ServiceType::IAM] {
        app.cache.insert(svc, region, None, vec![]);
    }
    app.handle_key(key(KeyCode::Char('r')), &tx).await.unwrap();
    assert!(app.audit_loading.is_empty());
    assert!(app.success_message.as_deref().unwrap_or("").contains("every source is cached"));
}

#[tokio::test]
async fn enter_on_an_account_finding_lands_on_iams_account_row() {
    // The account row's search text doesn't contain its id, so the jump's
    // id-as-query once filtered it out: "no matches for iam-account-settings".
    let (mut app, tx, _rx) = test_app().await;
    let region = app.current_region;
    let account = crate::aws::services::iam::IamAccountSettings {
        display_name: "acme-prod".into(),
        aliases: vec![],
        summary: vec![("AccountMFAEnabled".into(), 1), ("AccountAccessKeysPresent".into(), 0)],
        summary_error: None,
        password_policy: None,
        pw_policy_error: None,
        tags: Default::default(),
    };
    app.cache.insert(ServiceType::IAM, region, None, vec![Box::new(account)]);
    app.switch_service(ServiceType::Audit);
    app.update_search();
    assert_eq!(finding_ids(&app), vec!["iam-password-policy:iam-account-settings"]);

    app.selected_index = Some(0);
    app.handle_key(key(KeyCode::Enter), &tx).await.unwrap();
    assert_eq!(app.current_service, Some(ServiceType::IAM));
    assert_eq!(app.get_selected_resource_id().as_deref(), Some("iam-account-settings"));
    assert!(app.details_focused);
    let labels = app.get_selected_resource().unwrap().detail_sections().unwrap().sections;
    assert_eq!(labels[app.detail_section_idx].label, "Password Policy", "lands on the problem");
}

#[test]
fn every_landing_section_is_a_real_section_label() {
    // A misspelt `Check::section` silently lands on the default section —
    // pin each name to a label some split pane actually declares.
    let labels: std::collections::HashSet<&str> = all_mocks()
        .iter()
        .filter_map(|(_, _, r)| r.detail_sections())
        .flat_map(|d| d.sections.iter().map(|s| s.label))
        .collect();
    for check in crate::audit::all_checks() {
        if let Some(section) = check.section {
            assert!(labels.contains(section), "{}: no pane has a {section:?} section", check.id);
        }
    }
}
