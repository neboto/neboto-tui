//! Headless subcommands against the `--demo` account, through the real
//! service code (demo replay clients — no network).

use super::*;
use clap::Parser;

fn args(service: &str) -> LsArgs {
    LsArgs {
        service: service.to_string(),
        resource_type: None,
        filter: None,
        state: None,
        hide_noise: false,
        limit: None,
    }
}

async fn demo_list(svc: ServiceType) -> Result<(Vec<Box<dyn Resource>>, Vec<String>), Failure> {
    let clients = AwsClients::new_demo_for_test().await;
    let roles = vec!["OrganizationAccountAccessRole".to_string()];
    let services = App::build_services(&clients, None, &roles);
    list_all(services[&svc].clone(), svc).await
}

fn names(rows: &[Box<dyn Resource>]) -> Vec<&str> {
    rows.iter().map(|r| r.name()).collect()
}

#[tokio::test]
async fn ls_collects_every_streamed_batch() {
    let (rows, warnings) = demo_list(ServiceType::EC2).await.unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    let types = distinct_types(&rows);
    assert_eq!(types, ["EC2 Instance", "Security Group", "EBS Volume"]);
    assert!(names(&rows).contains(&"legacy-reporting-data"));
}

#[tokio::test]
async fn filters_compose_like_the_tui_search() {
    let (rows, _) = demo_list(ServiceType::EC2).await.unwrap();

    let a = LsArgs { resource_type: Some("volume".into()), state: Some("AVAILABLE".into()), ..args("ec2") };
    assert_eq!(names(&filter(rows.clone(), &a)), ["legacy-reporting-data"]);

    // Fuzzy text ranks; exact tag terms gate.
    let a = LsArgs { filter: Some("web tag:team=storefront".into()), ..args("ec2") };
    let got = filter(rows.clone(), &a);
    assert!(!got.is_empty());
    assert!(got.iter().all(|r| r.tags().get("team").map(String::as_str) == Some("storefront")));
    assert!(got[0].name().starts_with("web"));

    let a = LsArgs { limit: Some(2), ..args("ec2") };
    assert_eq!(filter(rows, &a).len(), 2);
}

#[test]
fn type_filter_takes_the_full_type_or_its_last_words() {
    assert!(type_matches("role", "IAM Role"));
    assert!(type_matches("IAM ROLE", "IAM Role"));
    assert!(type_matches("user|role", "IAM Role"));
    assert!(type_matches("security group", "Security Group"));
    assert!(!type_matches("rol", "IAM Role"), "whole words only");
    assert!(!type_matches("iam", "IAM Role"), "the last word(s), not the first");
    assert!(!type_matches("", "IAM Role"));
}

#[tokio::test]
async fn json_output_is_a_versioned_document() {
    let (rows, _) = demo_list(ServiceType::EC2).await.unwrap();
    let out = render_list(ServiceType::EC2, "us-east-1", &rows, OutputFormat::Json);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["schema"], SCHEMA);
    assert_eq!(v["service"], "ec2");
    assert_eq!(v["count"].as_u64().unwrap() as usize, rows.len());
    let first = &v["resources"][0];
    for key in ["type", "id", "name", "state", "tags"] {
        assert!(first.get(key).is_some(), "missing {key}: {first}");
    }

    let md = render_list(ServiceType::EC2, "us-east-1", &rows, OutputFormat::Md);
    assert!(md.contains("| Type | ID | Name | State | Tags |"));
    let csv = render_list(ServiceType::EC2, "us-east-1", &rows, OutputFormat::Csv);
    assert!(csv.starts_with("Type,ID,Name,State,"));
    let table = render_list(ServiceType::EC2, "us-east-1", &rows, OutputFormat::Table);
    assert!(table.lines().next().unwrap().starts_with("TYPE"));
    assert_eq!(table.lines().count(), rows.len() + 1);
}

#[tokio::test]
async fn every_service_lists_or_fails_cleanly() {
    // The demo answers anything it has no fixture for with an empty success,
    // so every service's load must run to completion here — a hang or a
    // panic in the event draining would show up as this test never ending.
    for svc in ServiceType::all() {
        let res = tokio::time::timeout(std::time::Duration::from_secs(30), demo_list(svc)).await;
        assert!(res.is_ok(), "{} never finished listing", svc.name());
    }
}

#[test]
fn service_names_resolve_with_or_without_the_at() {
    assert_eq!(resolve_service("@ec2").unwrap(), ServiceType::EC2);
    assert_eq!(resolve_service("lambda").unwrap(), ServiceType::Lambda);
    assert_eq!(resolve_service("functions").unwrap(), ServiceType::Lambda);
    let err = resolve_service("@nope").unwrap_err();
    assert_eq!(err.code(), 2);
}

#[test]
fn global_flags_work_after_the_subcommand() {
    let cli = Cli::try_parse_from(["neboto", "ls", "@ec2", "-r", "eu-west-1", "-o", "json", "--demo"]).unwrap();
    assert!(matches!(cli.command, Some(Command::Ls(ref a)) if a.service == "@ec2"));
    assert_eq!(cli.region.as_deref(), Some("eu-west-1"));
    assert_eq!(cli.output, Some(OutputFormat::Json));
    assert!(cli.demo);
    // No subcommand is the TUI.
    assert!(Cli::try_parse_from(["neboto", "-s", "ec2"]).unwrap().command.is_none());
}

#[test]
fn services_lists_every_service_once() {
    let out = services(OutputFormat::Json).unwrap();
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["count"].as_u64().unwrap() as usize, ServiceType::all().len());
    assert!(v["services"].as_array().unwrap().iter().any(|s| s["prefix"] == "@ec2"));
}

// ── get ─────────────────────────────────────────────────────────────────────

fn get_args(service: &str, ids: &[&str]) -> GetArgs {
    GetArgs {
        service: service.to_string(),
        ids: ids.iter().map(|s| s.to_string()).collect(),
        resource_type: None,
        sections: Vec::new(),
        wait: 30,
    }
}

async fn demo_get(args: GetArgs, format: OutputFormat) -> Result<String, Failure> {
    let clients = AwsClients::new_demo_for_test().await;
    get(Config::default(), clients, &args, format).await
}

async fn demo_get_json(args: GetArgs) -> serde_json::Value {
    let out = demo_get(args, OutputFormat::Json).await.unwrap();
    serde_json::from_str(&out).unwrap()
}

#[tokio::test]
async fn get_returns_every_section_loaded() {
    let v = demo_get_json(get_args("@cfn", &["storefront-prod"])).await;
    assert_eq!(v["schema"], SCHEMA);
    assert_eq!(v["service"], "cfn");
    assert_eq!(v["resource"]["name"], "storefront-prod");
    let sections = v["sections"].as_object().unwrap();
    // Lazy sections (their own API calls) arrived, not placeholders.
    for name in ["Resources", "Events", "Template", "Drift"] {
        assert!(sections[name].is_object(), "{name} not loaded: {}", sections[name]);
    }
    assert_eq!(v["sections"]["Resources"]["WebService"]["Type"], "AWS::ECS::Service");
    assert!(sections.values().all(|s| !s.is_null()));

    // Every demo stack has a template, not just the one the clips use.
    for stack in ["acme-network", "orders-pipeline"] {
        let v = demo_get_json(GetArgs { sections: vec!["template".into()], ..get_args("cfn", &[stack]) }).await;
        assert_eq!(
            v["sections"]["Template"]["content"][0], "AWSTemplateFormatVersion: '2010-09-09'",
            "{stack}: {}", v["sections"]["Template"]
        );
    }
}

#[tokio::test]
async fn get_several_ids_across_sub_tabs() {
    // An instance and a security group live on different EC2 sub-tabs.
    let v = demo_get_json(get_args("ec2", &["web-1", "sg-0web0000000000001", "web-1"])).await;
    assert_eq!(v["count"], 2, "duplicates collapse");
    let types: Vec<&str> = v["resources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["resource"]["type"].as_str().unwrap())
        .collect();
    assert_eq!(types, ["EC2 Instance", "Security Group"]);
}

#[tokio::test]
async fn get_by_name_is_exact_and_ambiguity_is_a_usage_error() {
    let err = demo_get(get_args("ecs", &["storefront-web"]), OutputFormat::Json)
        .await
        .unwrap_err();
    assert_eq!(err.code(), 2);
    assert!(err.message().contains("ECS Service") && err.message().contains("Task Definition"));

    let v = demo_get_json(GetArgs { resource_type: Some("service".into()), ..get_args("ecs", &["storefront-web"]) }).await;
    assert_eq!(v["resource"]["type"], "ECS Service");

    let err = demo_get(get_args("lambda", &["orders"]), OutputFormat::Json).await.unwrap_err();
    assert_eq!(err.code(), 2, "a partial name isn't a match");
}

#[tokio::test]
async fn get_section_filter_prints_only_those() {
    let v = demo_get_json(GetArgs {
        sections: vec!["TEMPLATE".into(), "resources".into()],
        ..get_args("cfn", &["storefront-prod"])
    })
    .await;
    let names: Vec<&String> = v["sections"].as_object().unwrap().keys().collect();
    assert_eq!(names, ["Resources", "Template"], "pane order, any case");
    assert_eq!(v["sections"]["Template"]["content"][0], "AWSTemplateFormatVersion: '2010-09-09'");

    let err = demo_get(
        GetArgs { sections: vec!["nope".into()], ..get_args("cfn", &["storefront-prod"]) },
        OutputFormat::Json,
    )
    .await
    .unwrap_err();
    assert_eq!(err.code(), 2);
    assert!(err.message().contains("Resources"), "lists the real sections: {}", err.message());
}

#[tokio::test]
async fn get_output_has_no_key_hints() {
    let text = demo_get(get_args("lambda", &["orders-api"]), OutputFormat::Table).await.unwrap();
    let md = demo_get(get_args("lambda", &["orders-api"]), OutputFormat::Md).await.unwrap();
    for out in [&text, &md] {
        assert!(!out.to_lowercase().contains("press "), "{out}");
    }
    assert!(text.starts_with("Lambda Function: orders-api\n"));
    assert!(text.contains("── Code ──"));
}

#[tokio::test]
async fn get_every_demo_resource_settles() {
    // Every resource the demo account has, through its real pane: each must
    // resolve by id and every section must finish loading — a trigger that
    // never answers (or a pane get can't reach) fails here.
    for svc in [
        ServiceType::EC2,
        ServiceType::ECS,
        ServiceType::IAM,
        ServiceType::Lambda,
        ServiceType::CloudFormation,
        ServiceType::Elb,
        ServiceType::CloudTrail,
        ServiceType::Secrets,
        ServiceType::Ssm,
    ] {
        let (rows, _) = demo_list(svc).await.unwrap();
        // Per type, with --type: ids aren't unique across types (a CloudTrail
        // Insight and its event share one).
        for rtype in distinct_types(&rows) {
            let ids: Vec<&str> =
                rows.iter().filter(|r| r.resource_type() == rtype).map(|r| r.id()).collect();
            for chunk in ids.chunks(MAX_GET) {
                let args = GetArgs { resource_type: Some(rtype.clone()), ..get_args(svc.prefix(), chunk) };
                let v = demo_get_json(args).await;
                let docs = match v.get("resources") {
                    Some(all) => all.as_array().unwrap().clone(),
                    None => vec![v],
                };
                assert_eq!(docs.len(), chunk.len(), "{rtype}");
                for d in docs {
                    let unloaded: Vec<&String> = d["sections"]
                        .as_object()
                        .unwrap()
                        .iter()
                        .filter(|(_, s)| s.is_null())
                        .map(|(k, _)| k)
                        .collect();
                    assert!(unloaded.is_empty(), "{rtype} {}: {unloaded:?}", d["resource"]["id"]);
                }
            }
        }
    }
}

#[tokio::test]
async fn get_never_fetches_a_secret_value() {
    // The CLI must not reach anything behind the TUI's `x` / `Y`. Every pane
    // over the demo's secret and SecureString parameter, all sections — then
    // no request in this process may have asked for a value.
    for (svc, ids) in [("secrets", vec!["prod/orders/db"]), ("ssm", vec!["/orders/db/password", "/orders/log-level"])] {
        let out = demo_get(get_args(svc, &ids), OutputFormat::Json).await.unwrap();
        assert!(out.contains("SecureString") || out.contains("prod/orders/db"));
    }
    let log = crate::demo::TEST_LOG.lock().unwrap();
    assert!(log.iter().any(|(s, op, _)| s == "ssm" && op == "GetParameterHistory"), "the panes ran");
    for (service, op, body) in log.iter() {
        assert!(
            !matches!(op.as_str(), "GetSecretValue" | "BatchGetSecretValue" | "GetParameter" | "GetParameters" | "GetParametersByPath"),
            "{service} {op} fetched a value"
        );
        assert!(!body.contains("\"WithDecryption\":true"), "{service} {op} decrypted: {body}");
    }
}

#[test]
fn headless_code_cannot_reach_the_reveal_keys() {
    // `get` drives the App through its event handler and section hooks,
    // never `handle_key` — which is where `x` / `Y` (secret reveal / copy)
    // live. Keep it that way.
    let src = include_str!("../headless.rs");
    for forbidden in ["handle_key", "fetch_secret_value", "fetch_parameter_value", "trigger_secret"] {
        assert!(!src.contains(forbidden), "headless.rs mentions {forbidden}");
    }
}

#[test]
fn get_parses_ids_and_repeatable_sections() {
    let cli = Cli::try_parse_from([
        "neboto", "get", "@ec2", "i-1", "i-2", "--section", "details", "--section", "tags", "-t", "instance", "--wait", "5",
    ])
    .unwrap();
    let Some(Command::Get(a)) = cli.command else { panic!("not get") };
    assert_eq!(a.ids, ["i-1", "i-2"]);
    assert_eq!(a.sections, ["details", "tags"]);
    assert_eq!(a.resource_type.as_deref(), Some("instance"));
    assert_eq!(a.wait, 5);
    assert!(Cli::try_parse_from(["neboto", "get", "@ec2"]).is_err(), "an id is required");
}
