//! The replay connector drives real SDK clients — these tests are the proof
//! that each wire format (EC2 query XML, awsJson, awsQuery XML) and the empty
//! fallbacks deserialize, not just that a fixture file exists.

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
    assert_eq!(web.ip_permissions()[1].ip_ranges()[0].cidr_ip(), Some("0.0.0.0/0"));

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
    let roles = iam.list_roles().send().await.expect("empty ListRoles");
    assert!(roles.roles().is_empty());
}
