//! The replay connector drives real SDK clients — these tests are the proof
//! that each wire format (EC2 query XML, awsJson, awsQuery XML, restJson)
//! and the empty fallbacks deserialize, not just that a fixture file exists.

use super::*;

async fn demo_config() -> aws_config::SdkConfig {
    aws_config::defaults(aws_config::BehaviorVersion::latest())
        .region(aws_config::Region::new(REGION))
        .credentials_provider(aws_sdk_ec2::config::Credentials::new("demo", "demo", None, None, "demo"))
        .http_client(http_client())
        .load()
        .await
}

#[test]
fn render_expands_placeholders_relative_to_now() {
    let now = 1_790_000_000;
    assert_eq!(render("{{account}}/{{region}}", "eu-west-1", now), "123456789012/eu-west-1");
    assert_eq!(render("{{epoch:now-15m}}", REGION, now), (now - 900).to_string());
    assert_eq!(render("{{iso:now}}", REGION, now), "2026-09-21T14:13:20.000Z");
    assert_eq!(render("{{epoch:now-1h+30s}}", REGION, now), (now - 3570).to_string());
    // Unknown tokens and unterminated braces pass through untouched.
    assert_eq!(render("{{nope}} {{", REGION, now), "{{nope}} {{");
}

#[test]
fn fixture_lookup_prefers_the_narrow_match() {
    let stopped = fixtures::find("ecs", "ListTasks", r#"{"cluster":"x","desiredStatus":"STOPPED"}"#).unwrap();
    assert!(stopped.contains("5d1e0c7a"));
    let running = fixtures::find("ecs", "ListTasks", r#"{"cluster":"x"}"#).unwrap();
    assert!(!running.contains("5d1e0c7a"));
    assert!(fixtures::find("ecs", "ListContainerInstances", "{}").is_none());
}

#[tokio::test]
async fn ec2_query_xml_fixtures_deserialize() {
    let ec2 = aws_sdk_ec2::Client::new(&demo_config().await);
    let resp = ec2.describe_instances().send().await.expect("DescribeInstances");
    let instances: Vec<_> = resp.reservations().iter().flat_map(|r| r.instances()).collect();
    assert_eq!(instances.len(), 3);
    assert_eq!(instances[0].instance_id(), Some("i-0a1b2c3d4e5f60001"));
    assert!(instances[0].launch_time().is_some(), "templated timestamp parses");
    assert_eq!(
        instances[2].state().and_then(|s| s.name()).map(|n| n.as_str()),
        Some("stopped")
    );

    let sgs = ec2.describe_security_groups().send().await.expect("DescribeSecurityGroups");
    let web = &sgs.security_groups()[0];
    let ssh = web.ip_permissions().iter().find(|p| p.from_port() == Some(22)).unwrap();
    assert_eq!(ssh.ip_ranges()[0].cidr_ip(), Some("0.0.0.0/0"), "the planted open SSH rule");

    let one = ec2
        .describe_security_groups()
        .group_ids("sg-0alb000000000003")
        .send()
        .await
        .expect("DescribeSecurityGroups by id");
    let ids: Vec<_> = one.security_groups().iter().filter_map(|g| g.group_id()).collect();
    assert_eq!(ids, ["sg-0alb000000000003"], "only the group asked for");

    let vols = ec2.describe_volumes().send().await.expect("DescribeVolumes");
    assert!(vols.volumes().iter().any(|v| v.attachments().is_empty()), "the orphaned volume");

    // No fixture → an empty success, not an error.
    let images = ec2.describe_images().send().await.expect("empty DescribeImages");
    assert!(images.images().is_empty());
}

#[tokio::test]
async fn ecs_json_fixtures_deserialize_and_match_per_request() {
    let ecs = aws_sdk_ecs::Client::new(&demo_config().await);
    let clusters = ecs.list_clusters().send().await.expect("ListClusters");
    assert_eq!(clusters.cluster_arns().len(), 1);

    let svcs = ecs
        .describe_services()
        .cluster("acme-prod")
        .services("orders-worker")
        .send()
        .await
        .expect("DescribeServices");
    let worker = svcs.services().iter().find(|s| s.service_name() == Some("orders-worker")).unwrap();
    assert!(worker
        .deployments()
        .iter()
        .any(|d| d.rollout_state().map(|r| r.as_str()) == Some("FAILED")));
    assert!(worker.events()[0].created_at().is_some(), "epoch timestamps parse");

    let td = ecs
        .describe_task_definition()
        .task_definition("orders-worker:15")
        .send()
        .await
        .expect("DescribeTaskDefinition");
    let env = td.task_definition().unwrap().container_definitions()[0].environment();
    assert!(env.iter().any(|e| e.name() == Some("DB_URL")), "the renamed variable");

    let stopped = ecs
        .describe_tasks()
        .cluster("acme-prod")
        .tasks("arn:aws:ecs:us-east-1:123456789012:task/acme-prod/5d1e0c7a9b8f4e2d8c6a4b2e0f1d3c5a")
        .send()
        .await
        .expect("DescribeTasks");
    assert_eq!(stopped.tasks()[0].containers()[0].exit_code(), Some(1));

    let none = ecs.list_container_instances().send().await.expect("empty JSON");
    assert!(none.container_instance_arns().is_empty());
}

#[tokio::test]
async fn awsquery_fixtures_and_empty_results_deserialize() {
    let config = demo_config().await;
    let sts = aws_sdk_sts::Client::new(&config);
    let me = sts.get_caller_identity().send().await.expect("GetCallerIdentity");
    assert_eq!(me.account(), Some(ACCOUNT));

    let iam = aws_sdk_iam::Client::new(&config);
    let roles = iam.list_roles().send().await.expect("ListRoles");
    assert!(roles.roles().iter().any(|r| r.role_name() == "github-actions-deploy"));
    let role = iam.get_role().role_name("orders-api-lambda").send().await.expect("GetRole");
    let role = role.role().unwrap();
    assert_eq!(role.role_name(), "orders-api-lambda", "the per-role fixture, not the first");
    assert!(role.role_last_used().is_some());
    let none = iam.list_saml_providers().send().await.expect("empty ListSAMLProviders");
    assert!(none.saml_provider_list().is_empty());

    let elb = aws_sdk_elasticloadbalancingv2::Client::new(&config);
    let lbs = elb.describe_load_balancers().send().await.expect("DescribeLoadBalancers");
    assert_eq!(lbs.load_balancers().len(), 2);
    let tgs = elb.describe_target_groups().send().await.expect("DescribeTargetGroups");
    let arn = tgs.target_groups()[0].target_group_arn().unwrap().to_string();
    assert!(arn.ends_with("targetgroup/storefront-web/0a1b2c3d4e5f6a7b"), "ECS points at this ARN");
    let health = elb
        .describe_target_health()
        .target_group_arn(arn)
        .send()
        .await
        .expect("DescribeTargetHealth");
    assert_eq!(health.target_health_descriptions().len(), 3);

    let cfn = aws_sdk_cloudformation::Client::new(&config);
    let stacks = cfn.describe_stacks().send().await.expect("DescribeStacks");
    assert!(stacks
        .stacks()
        .iter()
        .any(|s| s.stack_status().map(|s| s.as_str()) == Some("UPDATE_ROLLBACK_COMPLETE")));
    let drift = cfn
        .describe_stack_resource_drifts()
        .stack_name(stacks.stacks()[0].stack_id().unwrap())
        .send()
        .await
        .expect("DescribeStackResourceDrifts");
    assert_eq!(drift.stack_resource_drifts()[0].property_differences().len(), 1);
    let tpl = cfn.get_template().stack_name("storefront-prod").send().await.expect("GetTemplate");
    assert!(tpl.template_body().unwrap().contains("HealthCheckIntervalSeconds: 30"), "XML-escaped YAML");
}

#[tokio::test]
async fn rest_json_fixtures_route_by_path_and_can_fail() {
    let lambda = aws_sdk_lambda::Client::new(&demo_config().await);
    let fns = lambda.list_functions().send().await.expect("ListFunctions");
    assert_eq!(fns.functions().len(), 3);
    let f = lambda.get_function().function_name("nightly-report").send().await.expect("GetFunction");
    assert_eq!(
        f.configuration().and_then(|c| c.runtime()).map(|r| r.as_str()),
        Some("python3.9"),
        "the per-function fixture"
    );
    let err = lambda
        .get_function_event_invoke_config()
        .function_name("orders-api")
        .send()
        .await
        .expect_err("a !status 404 fixture is an error");
    assert!(err.into_service_error().is_resource_not_found_exception());
}

#[tokio::test]
async fn cost_anomalies_fixture_deserializes() {
    let ce = aws_sdk_costexplorer::Client::new(&demo_config().await);
    let interval = aws_sdk_costexplorer::types::AnomalyDateInterval::builder()
        .start_date("2026-01-01")
        .build()
        .unwrap();
    let resp = ce.get_anomalies().date_interval(interval).send().await.expect("GetAnomalies");
    assert_eq!(resp.anomalies().len(), 3);
    let first = &resp.anomalies()[0];
    assert_eq!(first.dimension_value(), Some("AmazonCloudWatch"));
    assert_eq!(first.root_causes().len(), 2);
    assert!(first.impact().unwrap().total_impact() > 100.0);
    assert!(first.anomaly_end_date().is_none(), "the CloudWatch anomaly is ongoing");
}

#[tokio::test]
async fn cloudtrail_and_logs_fixtures_deserialize() {
    let config = demo_config().await;
    let ct = aws_sdk_cloudtrail::Client::new(&config);
    let attr = aws_sdk_cloudtrail::types::LookupAttribute::builder()
        .attribute_key(aws_sdk_cloudtrail::types::LookupAttributeKey::ResourceName)
        .attribute_value("sg-0web0000000000001")
        .build()
        .unwrap();
    let evs = ct.lookup_events().lookup_attributes(attr).send().await.expect("LookupEvents");
    let e = &evs.events()[0];
    assert_eq!(e.event_name(), Some("AuthorizeSecurityGroupIngress"));
    assert!(e.event_time().is_some());
    assert!(e.cloud_trail_event().unwrap().contains("temp debug access"));

    let logs = aws_sdk_cloudwatchlogs::Client::new(&config);
    let groups = logs.describe_log_groups().send().await.expect("DescribeLogGroups");
    assert!(groups.log_groups().iter().all(|g| g.creation_time().unwrap_or(0) > 1_000_000_000_000), "ms");
    let tail = logs
        .filter_log_events()
        .log_group_name("/aws/lambda/orders-api")
        .start_time(chrono::Utc::now().timestamp_millis() - 60_000)
        .send()
        .await
        .expect("generated FilterLogEvents");
    assert!(tail.events().len() >= 39);
}

/// Every stored response, rendered, is well-formed: JSON parses and XML tags
/// balance. The SDK tests above exercise a sample per protocol; this catches
/// a hand edit that breaks a fixture no test happens to request.
#[test]
fn every_fixture_renders_to_well_formed_output() {
    assert!(xml_balanced("<a><b/><c x=\"1\">t</c></a>").is_ok());
    assert!(xml_balanced("<a><b></a>").is_err(), "the checker catches a dropped close");
    for f in fixtures::FIXTURES {
        let rendered = render(f.body, REGION, 1_790_000_000);
        let (_, body) = split_status(rendered);
        let what = format!("{} {} (when {:?})", f.service, f.operation, f.when);
        assert!(!body.contains("{{"), "{what}: unexpanded placeholder");
        if body.trim_start().starts_with('<') {
            if let Err(e) = xml_balanced(&body) {
                panic!("{what}: {e}");
            }
        } else if let Err(e) = serde_json::from_str::<serde_json::Value>(&body) {
            panic!("{what}: {e}");
        }
    }
}

/// Open/close tags pair up, in order. Not a parser — enough to catch a
/// dropped or mistyped closing tag.
fn xml_balanced(xml: &str) -> Result<(), String> {
    let mut stack: Vec<&str> = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find('<') {
        let end = rest[start..].find('>').ok_or("unterminated tag")? + start;
        let tag = &rest[start + 1..end];
        rest = &rest[end + 1..];
        if tag.starts_with('?') || tag.starts_with('!') || tag.ends_with('/') {
            continue;
        }
        if let Some(name) = tag.strip_prefix('/') {
            match stack.pop() {
                Some(open) if open == name => {}
                other => return Err(format!("</{name}> closes {other:?}")),
            }
        } else {
            stack.push(tag.split_whitespace().next().unwrap_or(tag));
        }
    }
    match stack.as_slice() {
        [] => Ok(()),
        open => Err(format!("unclosed {open:?}")),
    }
}
