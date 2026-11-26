//! CloudFront (#363): an origin's mTLS certificate shows on Origins and
//! jumps to ACM; a multi-tenant distribution lists its tenants with their
//! customizations, and a direct one says it has none without fetching.

use super::*;
use crate::aws::services::cloudfront::{CfDistribution, CfDistributionDetailSection, CfOrigin, CfTenant, CfTenantsData};
use crate::ui::widgets::details_pane::resource_jump_target;
use aws_sdk_cloudfront::types as cf;

const CERT: &str = "arn:aws:acm:us-east-1:123456789012:certificate/0aaaaaaa-1111-2222-3333-444444444444";
const ACL: &str = "arn:aws:wafv2:us-east-1:123456789012:global/webacl/tenant-acl/0123abcd-4567";

fn distribution() -> CfDistribution {
    let r = all_mocks().into_iter().find(|(_, l, _)| *l == "CfDistribution").unwrap().2;
    r.as_any().downcast_ref::<CfDistribution>().unwrap().clone()
}

fn origin(mtls: &str) -> CfOrigin {
    CfOrigin {
        id: "api".into(),
        domain_name: "api.example.com".into(),
        origin_path: String::new(),
        kind: "Custom".into(),
        protocol_policy: "https-only".into(),
        oac_id: String::new(),
        shield_region: String::new(),
        custom_headers: 0,
        connection: String::new(),
        custom_timeouts: String::new(),
        mtls_cert_arn: mtls.into(),
    }
}

fn tenant() -> CfTenant {
    let dt = aws_smithy_types::DateTime::from_secs(1_700_000_000);
    let summary = cf::DistributionTenantSummary::builder()
        .id("dt_acme")
        .distribution_id("E1MOCK0DISTRIB")
        .name("acme")
        .arn("arn:aws:cloudfront::123456789012:distribution-tenant/dt_acme")
        .domains(cf::DomainResult::builder().domain("acme.example.com").status(cf::DomainStatus::Active).build().unwrap())
        .domains(cf::DomainResult::builder().domain("old.acme.example.com").status(cf::DomainStatus::Inactive).build().unwrap())
        .connection_group_id("cg_1")
        .customizations(
            cf::Customizations::builder()
                .web_acl(cf::WebAclCustomization::builder().action(cf::CustomizationActionType::Override).arn(ACL).build().unwrap())
                .certificate(cf::Certificate::builder().arn(CERT).build().unwrap())
                .build(),
        )
        .created_time(dt)
        .last_modified_time(dt)
        .e_tag("E1")
        .enabled(true)
        .status("Deployed")
        .build()
        .unwrap();
    CfTenant::from_summary(&summary)
}

fn rows(app: &App) -> Vec<(String, String)> {
    app.get_detail_lines_filtered()
}

#[tokio::test]
async fn an_origin_mtls_certificate_shows_and_jumps_to_acm() {
    let (mut app, _tx, _rx) = test_app().await;
    let mut d = distribution();
    d.origins = vec![origin(CERT), origin("")];
    select_mock(&mut app, ServiceType::CloudFront, Box::new(d));
    app.details_focused = true;
    app.detail_section_idx = CfDistributionDetailSection::Origins as usize;

    let mtls: Vec<_> = rows(&app).into_iter().filter(|(k, _)| k.trim() == "Origin mTLS Cert").collect();
    assert_eq!(mtls.len(), 1, "only the origin with a certificate gets the row: {mtls:?}");
    assert_eq!(mtls[0].1, CERT);
    let t = resource_jump_target(&mtls[0].0, &mtls[0].1, ServiceType::CloudFront).expect("ACM jump");
    assert_eq!(t.service, ServiceType::Acm);
}

#[tokio::test]
async fn a_direct_distribution_has_no_tenants_and_never_fetches() {
    let (mut app, tx, _rx) = test_app().await;
    let d = distribution();
    let id = d.id.clone();
    select_mock(&mut app, ServiceType::CloudFront, Box::new(d));
    app.details_focused = true;
    app.handle_key(key(KeyCode::Char('7')), &tx).await.unwrap();
    assert_eq!(app.detail_section_idx, CfDistributionDetailSection::Tenants as usize);
    assert!(app.lazy.cf_tenants.get(&id).is_none(), "a direct distribution fetched tenants");
    let r = rows(&app);
    assert!(r.iter().any(|(_, v)| v.starts_with("Not a multi-tenant distribution")), "{r:?}");
    app.detail_section_idx = CfDistributionDetailSection::Overview as usize;
    assert!(!rows(&app).iter().any(|(k, _)| k == "Connection Mode"));
}

#[tokio::test]
async fn a_multi_tenant_distribution_lists_its_tenants_and_their_overrides() {
    let (mut app, _tx, _rx) = test_app().await;
    let mut d = distribution();
    d.connection_mode = cf::ConnectionMode::TenantOnly.as_str().into();
    let id = d.id.clone();
    select_mock(&mut app, ServiceType::CloudFront, Box::new(d));
    app.lazy.cf_tenants.apply(id, Ok(CfTenantsData { items: vec![tenant()], truncated: true }));
    app.details_focused = true;

    app.detail_section_idx = CfDistributionDetailSection::Overview as usize;
    assert!(rows(&app).iter().any(|(k, v)| k == "Connection Mode" && v.starts_with("Multi-tenant")));

    app.detail_section_idx = CfDistributionDetailSection::Tenants as usize;
    let r = rows(&app);
    let has = |k: &str, v: &str| r.iter().any(|(rk, rv)| rk.trim() == k && rv == v);
    assert!(r.iter().any(|(k, _)| k == "Tenants (first 1)"), "{r:?}");
    assert!(has("Domain", "acme.example.com"));
    assert!(has("Domain", "⚠ old.acme.example.com (inactive)"));
    assert!(has("Connection Group", "cg_1"));
    assert!(has("WAF (override)", ACL));
    assert!(has("Certificate", CERT));

    // The override rows jump: WAF to the ACL, the certificate to ACM.
    let waf = resource_jump_target("  WAF (override)", ACL, ServiceType::CloudFront).expect("WAF jump");
    assert_eq!(waf.service, ServiceType::Waf);
    let acm = resource_jump_target("  Certificate", CERT, ServiceType::CloudFront).expect("ACM jump");
    assert_eq!(acm.service, ServiceType::Acm);
}
