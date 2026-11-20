//! The WAF console's Security analytics dashboard (unsampled, 21 days) has
//! no API, so the web ACL's Insights section — 3 hours of samples — points
//! at it, and `O` on that section opens it instead of the ACL's own page.

use super::*;
use crate::aws::services::waf::{WafWebAcl, WafWebAclDetailSection as S};

fn acl(scope: &str, region: &str) -> WafWebAcl {
    let kind = if scope == "CLOUDFRONT" { "global" } else { "regional" };
    WafWebAcl {
        id: "0123abcd".into(),
        name: "edge".into(),
        arn: format!("arn:aws:wafv2:{region}:123456789012:{kind}/webacl/edge/0123abcd"),
        description: String::new(),
        scope: scope.into(),
    }
}

#[tokio::test]
async fn insights_points_at_security_analytics_and_o_opens_it() {
    let (mut app, tx, _rx) = test_app().await;
    let regional = acl("REGIONAL", "ap-southeast-2");
    let want = "https://ap-southeast-2.console.aws.amazon.com/wafv2-pro/security-analytics?region=ap-southeast-2";
    select_mock(&mut app, ServiceType::Waf, Box::new(regional.clone()));
    app.details_focused = true;
    app.reset_detail_section_to_default(&tx);

    // Overview: `O` is still the ACL's own page.
    let own = app.selected_console_url(&regional).unwrap();
    assert!(own.contains("/wafv2/homev2/web-acl/edge/0123abcd/"), "{own}");

    app.set_detail_section(S::Insights as usize, &tx);
    assert_eq!(app.selected_console_url(&regional).as_deref(), Some(want));
    // While the samples load, nothing extra; once they settle, the pointer.
    assert!(!app.get_detail_lines().iter().any(|(_, v)| v == want));
    app.lazy.waf_insights.apply("edge".to_string(), Err("AccessDenied".to_string()));
    let lines = app.get_detail_lines();
    assert!(lines.iter().any(|(k, v)| k.trim() == "Security analytics" && v == want), "{lines:?}");

    // The list pane's `O` ignores the detail section.
    app.details_focused = false;
    assert_eq!(app.selected_console_url(&regional), Some(own));

    // A CLOUDFRONT ACL opens in us-east-1 whatever region neboto is in.
    let cf = acl("CLOUDFRONT", "us-east-1");
    app.details_focused = true;
    assert!(app.selected_console_url(&cf).unwrap().ends_with("security-analytics?region=us-east-1"));
}
