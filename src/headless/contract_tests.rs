//! The stable half of the headless output, pinned to `contract.json`.
//!
//! What scripts may rely on (README "Output contract"): the flags, the JSON
//! envelopes, the core resource keys, the CSV headers, the service prefixes,
//! the resource type names and each type's section names. Everything inside
//! a section is the pane, serialized, and is deliberately left out — it may
//! change in any release.
//!
//! Anything in `contract.json` that the build no longer produces is a
//! **breaking change**: it needs a new `SCHEMA` version and a release note,
//! not an edit to the file. Anything new is an addition — record it with
//! `NEBOTO_BLESS=1 cargo test contract`.

use super::*;
use clap::CommandFactory;
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;

const GOLDEN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src/headless/contract.json");

fn keys(v: &Value) -> Vec<String> {
    v.as_object().unwrap().keys().cloned().collect()
}

fn csv_header(out: &str, columns: usize) -> Vec<String> {
    out.lines().next().unwrap().split(',').take(columns).map(String::from).collect()
}

/// Every flag, by subcommand ("" = the global/TUI ones).
fn flags() -> Value {
    let cli = Cli::command();
    let longs = |c: &clap::Command| -> Vec<String> {
        let mut v: Vec<String> = c.get_arguments().filter_map(|a| a.get_long()).map(|l| format!("--{l}")).collect();
        v.sort();
        v
    };
    let mut out = Map::new();
    out.insert(String::new(), json!(longs(&cli)));
    for sub in cli.get_subcommands() {
        out.insert(sub.get_name().to_string(), json!(longs(sub)));
    }
    Value::Object(out)
}

/// Section names per resource type, from every pane's descriptor (the mocks
/// cover every split pane; a type without one is the flat "Details").
fn sections_by_type() -> Value {
    let mut out: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    for (_, _, r) in crate::app::harness_tests::all_mocks() {
        let names = match r.detail_sections() {
            Some(d) => d.sections.iter().map(|s| s.label.to_string()).collect(),
            None => vec!["Details".to_string()],
        };
        out.entry(r.resource_type().to_string()).or_insert(names);
    }
    json!(out)
}

async fn current() -> Value {
    let (rows, _) = super::tests::demo_list(ServiceType::EC2).await.unwrap();
    let ls = serde_json::from_str::<Value>(&render_list(ServiceType::EC2, "us-east-1", &rows, OutputFormat::Json)).unwrap();
    let ls_csv = render_list(ServiceType::EC2, "us-east-1", &rows, OutputFormat::Csv);

    let one = super::tests::demo_get_json(super::tests::get_args("lambda", &["orders-api"])).await;
    let several = super::tests::demo_get_json(super::tests::get_args("ec2", &["web-1", "web-2"])).await;
    let get_csv = super::tests::demo_get(super::tests::get_args("lambda", &["orders-api"]), OutputFormat::Csv)
        .await
        .unwrap();

    let svcs = serde_json::from_str::<Value>(&services(OutputFormat::Json).unwrap()).unwrap();

    let mut prefixes: Vec<String> = ServiceType::all().iter().map(|s| s.prefix().to_string()).collect();
    prefixes.sort();

    json!({
        "schema": SCHEMA,
        "flags": flags(),
        "services": {"envelope": keys(&svcs), "service": keys(&svcs["services"][0])},
        "ls": {
            "envelope": keys(&ls),
            "resource": crate::export::CORE_KEYS,
            "csv": csv_header(&ls_csv, 4),
        },
        "get": {
            "one": keys(&one),
            "several": keys(&several),
            "item": keys(&several["resources"][0]),
            "resource": keys(&one["resource"]),
            "csv": csv_header(&get_csv, 4),
        },
        "prefixes": prefixes,
        "sections": sections_by_type(),
    })
}

/// Every leaf as a path: `ls › envelope › count`,
/// `sections › Lambda Function › Code`.
fn paths(v: &Value, prefix: &str, out: &mut BTreeSet<String>) {
    let join = |k: &str| if prefix.is_empty() { k.to_string() } else { format!("{prefix} › {k}") };
    match v {
        Value::Object(m) => {
            for (k, v) in m {
                paths(v, &join(k), out);
            }
        }
        Value::Array(a) => {
            for v in a {
                paths(v, prefix, out);
            }
        }
        Value::String(s) => {
            out.insert(join(s));
        }
        other => {
            out.insert(join(&other.to_string()));
        }
    }
}

#[tokio::test]
async fn output_contract_holds() {
    let now = current().await;
    // Every core key is really on the rows, holding the resource's own value.
    let (rows, _) = super::tests::demo_list(ServiceType::EC2).await.unwrap();
    for r in &rows {
        let o = crate::export::resource_object(r.as_ref());
        assert_eq!(o["id"], r.id());
        assert_eq!(o["type"], r.resource_type());
        assert_eq!(o["name"], r.name());
        assert_eq!(o["state"], r.state_label());
    }

    if std::env::var_os("NEBOTO_BLESS").is_some() {
        std::fs::write(GOLDEN, serde_json::to_string_pretty(&now).unwrap() + "\n").unwrap();
        return;
    }
    let golden: Value = serde_json::from_str(&std::fs::read_to_string(GOLDEN).unwrap()).unwrap();
    assert_eq!(
        golden["schema"], SCHEMA,
        "SCHEMA changed: re-record the contract for the new version with NEBOTO_BLESS=1"
    );

    let (mut was, mut is) = (BTreeSet::new(), BTreeSet::new());
    paths(&golden, "", &mut was);
    paths(&now, "", &mut is);
    let removed: Vec<_> = was.difference(&is).collect();
    let added: Vec<_> = is.difference(&was).collect();
    assert!(
        removed.is_empty(),
        "BREAKING: scripts rely on these and they're gone or renamed:\n  {}\n\
         Put them back, or bump SCHEMA (a new neboto/vN), note it in the release, \
         and re-record with NEBOTO_BLESS=1 cargo test contract",
        removed.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("\n  ")
    );
    assert!(
        added.is_empty(),
        "New stable output (not breaking):\n  {}\nRecord it with NEBOTO_BLESS=1 cargo test contract",
        added.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("\n  ")
    );
}

#[test]
fn every_failure_is_exit_1_or_2() {
    assert_eq!(Failure::Usage(String::new()).code(), 2);
    assert_eq!(Failure::Error(String::new()).code(), 1);
    assert_eq!(resolve_service("@nope").unwrap_err().code(), 2);
}
