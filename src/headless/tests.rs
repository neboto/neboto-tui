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
