//! An ELB target group opens on an Overview: its ARN and identity, a
//! health summary (state counts, the unhealthy targets' reasons, the
//! attached load balancers, the health check in a line), then the first
//! eight targets with a pointer to the Health tab.

use super::*;
use crate::aws::services::elb::{TargetGroup, TargetHealthEntry};

const TG_ARN: &str = "arn:aws:elasticloadbalancing:us-east-1:123456789012:targetgroup/web-tg/0123456789abcdef";
const LB_ARN: &str = "arn:aws:elasticloadbalancing:us-east-1:123456789012:loadbalancer/app/web-alb/fedcba9876543210";

fn target_group() -> TargetGroup {
    use aws_sdk_elasticloadbalancingv2::types::{Matcher, ProtocolEnum, TargetTypeEnum};
    TargetGroup::from_sdk(
        &aws_sdk_elasticloadbalancingv2::types::TargetGroup::builder()
            .target_group_arn(TG_ARN)
            .target_group_name("web-tg")
            .protocol(ProtocolEnum::Http)
            .port(80)
            .protocol_version("HTTP1")
            .target_type(TargetTypeEnum::Instance)
            .vpc_id("vpc-0123456789abcdef0")
            .load_balancer_arns(LB_ARN)
            .health_check_enabled(true)
            .health_check_protocol(ProtocolEnum::Http)
            .health_check_path("/health")
            .health_check_port("traffic-port")
            .health_check_interval_seconds(30)
            .healthy_threshold_count(5)
            .unhealthy_threshold_count(2)
            .matcher(Matcher::builder().http_code("200").build())
            .build(),
    )
}

fn target(id: &str, state: &str, description: Option<&str>) -> TargetHealthEntry {
    TargetHealthEntry {
        target_id: id.to_string(),
        port: Some(80),
        availability_zone: Some("us-east-1a".to_string()),
        state: state.to_string(),
        reason: description.map(|_| "Target.ResponseCodeMismatch".to_string()),
        description: description.map(str::to_string),
    }
}

async fn focused_on(tg: TargetGroup) -> (App, mpsc::UnboundedSender<Event>, mpsc::UnboundedReceiver<Event>) {
    let (mut app, tx, rx) = test_app().await;
    select_mock(&mut app, ServiceType::Elb, Box::new(tg));
    app.details_focused = true;
    app.reset_detail_section_to_default(&tx);
    (app, tx, rx)
}

fn has(body: &[(String, String)], k: &str, v: &str) -> bool {
    body.iter().any(|(bk, bv)| bk == k && bv == v)
}

#[tokio::test]
async fn a_target_group_opens_on_an_overview_and_fetches_health() {
    let (app, _tx, _rx) = focused_on(target_group()).await;
    assert_eq!(app.selected_descriptor().unwrap().sections[0].label, "Overview");
    assert_eq!(app.detail_section_idx, 0);
    // The Overview's health summary starts the same fetch Health uses.
    assert!(matches!(app.lazy.target_group_health.get(TG_ARN), Some(crate::lazy::Lazy::Loading)));

    let body = app.get_detail_lines();
    // The header shows the name, but only body rows can be copied with `y`.
    assert_eq!(body[0], ("Target Group ARN".to_string(), TG_ARN.to_string()));
    assert!(has(&body, "Name", "web-tg"), "{body:#?}");
    assert!(has(&body, "Protocol", "HTTP:80 (HTTP1)"), "{body:#?}");
    assert!(has(&body, "Target Type", "instance"), "{body:#?}");
    assert!(has(&body, "VPC", "vpc-0123456789abcdef0"), "{body:#?}");
    assert!(has(&body, "Targets", "Loading…"), "{body:#?}");
    assert!(has(&body, "Check", "HTTP /health :traffic-port every 30s, 5/2, codes 200"), "{body:#?}");
    assert!(has(&body, "Load Balancers (1)", "") && has(&body, "web-alb", LB_ARN), "{body:#?}");
}

#[tokio::test]
async fn all_healthy_reads_as_counts_with_no_warnings() {
    let (mut app, _tx, _rx) = focused_on(target_group()).await;
    app.lazy.target_group_health.apply(
        TG_ARN.to_string(),
        Ok(vec![target("i-0aaaaaaaaaaaaaaa1", "healthy", None), target("i-0aaaaaaaaaaaaaaa2", "healthy", None)]),
    );
    let body = app.get_detail_lines();
    assert!(has(&body, "Targets", "2 healthy"), "{body:#?}");
    assert!(!body.iter().any(|(k, _)| k.contains('⚠')), "{body:#?}");
    assert!(has(&body, "Registered Targets (2)", ""), "{body:#?}");
    assert!(body.iter().any(|(k, _)| k.contains("i-0aaaaaaaaaaaaaaa2") && k.contains("healthy")), "{body:#?}");
}

#[tokio::test]
async fn unhealthy_targets_are_counted_and_their_reasons_folded() {
    let (mut app, _tx, _rx) = focused_on(target_group()).await;
    let codes = "Health checks failed with these codes: [502]";
    app.lazy.target_group_health.apply(
        TG_ARN.to_string(),
        Ok(vec![
            target("i-0aaaaaaaaaaaaaaa1", "healthy", None),
            target("i-0bbbbbbbbbbbbbbb1", "unhealthy", Some(codes)),
            target("i-0bbbbbbbbbbbbbbb2", "unhealthy", Some(codes)),
            target("i-0ccccccccccccccc1", "unhealthy", Some("Request timed out")),
            target("i-0ddddddddddddddd1", "draining", Some("Target deregistration is in progress")),
        ]),
    );
    let body = app.get_detail_lines();
    assert!(has(&body, "Targets", "1 healthy · 3 unhealthy · 1 draining"), "{body:#?}");
    let warnings: Vec<&str> = body.iter().filter(|(k, _)| k.starts_with("  ⚠")).map(|(k, _)| k.as_str()).collect();
    assert_eq!(
        warnings,
        [
            "  ⚠ 2 targets unhealthy — Health checks failed with these codes: [502]",
            "  ⚠ i-0ccccccccccccccc1 unhealthy — Request timed out",
        ],
        "draining is a passing state, not a warning"
    );
}

#[tokio::test]
async fn the_overview_caps_a_long_target_list() {
    let (mut app, _tx, _rx) = focused_on(target_group()).await;
    let list = (0..11).map(|i| target(&format!("i-0eeeeeeeeeeeee{i:03x}"), "healthy", None)).collect();
    app.lazy.target_group_health.apply(TG_ARN.to_string(), Ok(list));
    let body = app.get_detail_lines();
    assert_eq!(body.iter().filter(|(k, _)| k.contains("i-0eeeeeeeeeeeee")).count(), 8, "{body:#?}");
    assert!(has(&body, "", "· 3 more — 2 Health"), "{body:#?}");
    // The Health tab (now key 2) still has every target.
    assert_eq!(crate::aws::services::elb::TARGET_GROUP_SECTIONS.sections[1].label, "Health");
}

#[tokio::test]
async fn overview_rows_jump_but_the_own_arn_does_not() {
    let (mut app, tx, _rx) = focused_on(target_group()).await;
    app.lazy.target_group_health.apply(TG_ARN.to_string(), Ok(vec![target("i-0aaaaaaaaaaaaaaa1", "healthy", None)]));
    let body = app.get_detail_lines();

    // The group's own ARN is no jump: ⏎ would only re-land here.
    app.details_selected_index = Some(0);
    let history = app.nav_history.len();
    app.handle_key(key(KeyCode::Enter), &tx).await.unwrap();
    assert_eq!(app.nav_history.len(), history, "Enter on the own-ARN row records no jump");

    let row = body.iter().position(|(k, _)| k.contains("i-0aaaaaaaaaaaaaaa1")).unwrap();
    app.details_selected_index = Some(row);
    app.handle_key(key(KeyCode::Enter), &tx).await.unwrap();
    assert_eq!(app.pending_jump.as_deref(), Some("i-0aaaaaaaaaaaaaaa1"), "a target row jumps to its instance");

    let (mut app, tx, _rx) = focused_on(target_group()).await;
    let body = app.get_detail_lines();
    let row = body.iter().position(|(k, v)| k == "web-alb" && v == LB_ARN).unwrap();
    app.details_selected_index = Some(row);
    app.handle_key(key(KeyCode::Enter), &tx).await.unwrap();
    assert_eq!(app.pending_jump.as_deref(), Some("web-alb"), "the load balancer row jumps");
}

#[tokio::test]
async fn distinct_failure_reasons_are_capped() {
    let (mut app, _tx, _rx) = focused_on(target_group()).await;
    let reasons: Vec<String> = (0..5).map(|i| format!("reason {i}")).collect();
    let list = reasons
        .iter()
        .enumerate()
        .map(|(i, r)| target(&format!("i-0fffffffffffffff{i}"), "unhealthy", Some(r)))
        .collect();
    app.lazy.target_group_health.apply(TG_ARN.to_string(), Ok(list));
    let body = app.get_detail_lines();
    assert_eq!(body.iter().filter(|(k, _)| k.starts_with("  ⚠")).count(), 3, "{body:#?}");
    assert!(has(&body, "", "· 2 more reasons — 2 Health"), "{body:#?}");
}
