//! A failed tag call must never read as "No tags" (#28). Each lazy bundle
//! below fetches tags best-effort alongside its real payload; when the tag
//! call fails the bundle still loads, carrying `tags_error`, and the Tags
//! section has to render that as a `⚠` row. Before the fix every one of
//! these mapped the error to an empty list — a permission gap looked exactly
//! like an untagged resource.

use super::*;
use crate::aws::services::{acm, code, identity_center, route53profiles, route53resolver, waf};

const DENIED: &str = "AccessDeniedException: not authorized to list tags";

/// Select the registry mock `label`, focus its Tags section (which fires the
/// bundle trigger and marks one key loading), hand `apply` that key to land a
/// failed-tags bundle, then check what the Tags section says.
async fn assert_tags_warn(label: &str, apply: impl FnOnce(&mut App) -> String) {
    let (mut app, tx, _rx) = test_app().await;
    let (svc, _, res) = all_mocks()
        .into_iter()
        .find(|(_, l, _)| *l == label)
        .unwrap_or_else(|| panic!("no mock {label}"));
    let tags_idx = res
        .detail_sections()
        .expect("split pane")
        .sections
        .iter()
        .position(|s| s.label == "Tags")
        .unwrap_or_else(|| panic!("{label} has no Tags section"));
    select_mock(&mut app, svc, res);
    app.details_focused = true;
    app.reset_detail_section_to_default(&tx);
    app.set_detail_section(tags_idx, &tx);

    let key = apply(&mut app);
    let body = app
        .get_detail_lines()
        .into_iter()
        .map(|(k, v)| format!("{k} {v}"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        body.contains(&format!("⚠ {DENIED}")),
        "{label} (key {key}): tag failure should render as a warning:\n{body}"
    );
    assert!(
        !body.to_lowercase().contains("no tags"),
        "{label}: a failed tag call must not read as no tags:\n{body}"
    );
}

/// The one key the Tags-section trigger marked loading.
fn loading_key<T>(map: &crate::lazy::LazyMap<T>) -> String {
    let keys: Vec<&String> = map.iter().map(|(k, _)| k).collect();
    assert_eq!(keys.len(), 1, "expected one triggered key, got {keys:?}");
    keys[0].clone()
}

fn denied() -> Option<String> {
    Some(DENIED.to_string())
}

#[tokio::test]
async fn waf_ip_set() {
    assert_tags_warn("WafIpSet", |app| {
        let key = loading_key(&app.lazy.waf_ip_set_details);
        let d = waf::WafIpSetDetail {
            ip_address_version: "IPV4".into(),
            addresses: vec![],
            tags: vec![],
            tags_error: denied(),
        };
        app.lazy.waf_ip_set_details.apply(key.clone(), Ok(Box::new(d)));
        key
    })
    .await;
}

#[tokio::test]
async fn waf_rule_group() {
    assert_tags_warn("WafRuleGroup", |app| {
        let key = loading_key(&app.lazy.waf_rule_group_details);
        let d = waf::WafRuleGroupDetail {
            capacity: 0,
            rules: vec![],
            tags: vec![],
            tags_error: denied(),
        };
        app.lazy.waf_rule_group_details.apply(key.clone(), Ok(Box::new(d)));
        key
    })
    .await;
}

#[tokio::test]
async fn resolver_endpoint() {
    assert_tags_warn("ResolverEndpoint", |app| {
        let key = loading_key(&app.lazy.resolver_endpoint_details);
        let d = route53resolver::ResolverEndpointDetail {
            ip_addresses: vec![],
            tags: vec![],
            tags_error: denied(),
        };
        app.lazy.resolver_endpoint_details.apply(key.clone(), Ok(Box::new(d)));
        key
    })
    .await;
}

#[tokio::test]
async fn resolver_rule() {
    assert_tags_warn("ResolverRule", |app| {
        let key = loading_key(&app.lazy.resolver_rule_details);
        let d = route53resolver::ResolverRuleDetail {
            associations: vec![],
            tags: vec![],
            tags_error: denied(),
        };
        app.lazy.resolver_rule_details.apply(key.clone(), Ok(Box::new(d)));
        key
    })
    .await;
}

#[tokio::test]
async fn route53_profile() {
    assert_tags_warn("Route53Profile", |app| {
        let key = loading_key(&app.lazy.r53_profile_details);
        let d = route53profiles::Route53ProfileDetail {
            owner_id: String::new(),
            status: "COMPLETE".into(),
            status_message: String::new(),
            creation_time: None,
            modification_time: None,
            vpcs: vec![],
            resources: vec![],
            tags: vec![],
            tags_error: denied(),
        };
        app.lazy.r53_profile_details.apply(key.clone(), Ok(Box::new(d)));
        key
    })
    .await;
}

#[tokio::test]
async fn acm_certificate() {
    assert_tags_warn("AcmCertificate", |app| {
        let key = loading_key(&app.lazy.acm_cert_details);
        let d = acm::AcmCertDetails {
            subject_alternative_names: vec![],
            key_algorithm: String::new(),
            renewal_eligibility: String::new(),
            in_use_by: vec![],
            issued_at: None,
            not_before: None,
            validation_options: vec![],
            tags: Default::default(),
            tags_error: denied(),
        };
        app.lazy.acm_cert_details.apply(key.clone(), Ok(d));
        key
    })
    .await;
}

#[tokio::test]
async fn code_pipeline() {
    assert_tags_warn("CodePipeline", |app| {
        let key = loading_key(&app.lazy.code_pipeline_details);
        let d = code::CodePipelineDetails {
            stages: vec![],
            role_arn: String::new(),
            execution_mode: String::new(),
            artifact_store_type: String::new(),
            artifact_store_location: String::new(),
            artifact_store_kms: String::new(),
            tags: Default::default(),
            tags_error: denied(),
        };
        app.lazy.code_pipeline_details.apply(key.clone(), Ok(d));
        key
    })
    .await;
}

#[tokio::test]
async fn identity_center_permission_set() {
    assert_tags_warn("PermissionSet", |app| {
        let key = loading_key(&app.lazy.ic_ps_access);
        let d = identity_center::PsAccess {
            inline_policy: None,
            managed: vec![],
            customer_managed: vec![],
            boundary: None,
            tags: vec![],
            tags_error: denied(),
        };
        app.lazy.ic_ps_access.apply(key.clone(), Ok(Box::new(d)));
        key
    })
    .await;
}

/// SNS reads tags at list time, per topic — the error rides the row itself.
#[tokio::test]
async fn sns_topic() {
    let (mut app, tx, _rx) = test_app().await;
    let topic = crate::aws::services::messaging::SnsTopic {
        arn: "arn:aws:sns:us-east-1:123456789012:t".into(),
        name: "t".into(),
        is_fifo: false,
        display_name: None,
        subscriptions_confirmed: 0,
        subscriptions_pending: 0,
        subscriptions_deleted: 0,
        owner: None,
        kms_key_id: None,
        policy: None,
        effective_delivery_policy: None,
        tags: Default::default(),
        tags_error: denied(),
    };
    let tags_idx = crate::aws::services::messaging::SNS_TOPIC_SECTIONS
        .sections
        .iter()
        .position(|s| s.label == "Tags")
        .unwrap();
    select_mock(&mut app, ServiceType::Messaging, Box::new(topic));
    app.details_focused = true;
    app.reset_detail_section_to_default(&tx);
    app.set_detail_section(tags_idx, &tx);
    let body = app
        .get_detail_lines()
        .into_iter()
        .map(|(k, v)| format!("{k} {v}"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(body.contains(&format!("⚠ {DENIED}")), "{body}");
    assert!(!body.contains("No tags"), "{body}");
}
