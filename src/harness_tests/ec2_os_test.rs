//! EC2 instances say which OS they are: `PlatformDetails` (set for every
//! instance, unlike `Platform`, which only Windows gets) feeds the search,
//! the list's second column and the pane's Platform row. Before this a
//! "linux" search could only match by accident, on tags.

use super::*;
use crate::aws::resource::Resource;
use crate::aws::services::ec2::{Ec2Instance, OsFamily};
use crate::search::fuzzy::FuzzyMatcher;

fn instance(id: &str, name: &str, platform_details: Option<&str>, windows: bool) -> Ec2Instance {
    let mut b = aws_sdk_ec2::types::Instance::builder()
        .instance_id(id)
        .instance_type(aws_sdk_ec2::types::InstanceType::T3Large)
        .architecture(aws_sdk_ec2::types::ArchitectureValues::X8664)
        .tags(aws_sdk_ec2::types::Tag::builder().key("Name").value(name).build());
    if let Some(d) = platform_details {
        b = b.platform_details(d);
    }
    if windows {
        b = b.platform(aws_sdk_ec2::types::PlatformValues::Windows);
    }
    Ec2Instance::from_sdk(&b.build())
}

fn fleet() -> Vec<Box<dyn Resource>> {
    vec![
        // A Windows box whose tags spell l…i…n…u…x: the old false positive.
        Box::new(instance("i-0win", "lin-ux-jumpbox", Some("Windows"), true)),
        Box::new(instance("i-0sql", "reporting", Some("Windows with SQL Server Standard"), true)),
        Box::new(instance("i-0al2023", "api", Some("Linux/UNIX"), false)),
        Box::new(instance("i-0rhel", "billing", Some("Red Hat Enterprise Linux"), false)),
        Box::new(instance("i-0ubuntu", "workers", Some("Ubuntu Pro"), false)),
        Box::new(instance("i-0none", "emulated", None, false)),
    ]
}

fn ids_matching(query: &str) -> Vec<String> {
    let fleet = fleet();
    let mut ids: Vec<String> = FuzzyMatcher::new()
        .filter_resources(query, &fleet)
        .into_iter()
        .map(|(i, _)| fleet[i].id().to_string())
        .collect();
    ids.sort();
    ids
}

#[test]
fn the_os_family_and_label_come_from_platform_details() {
    let f = fleet();
    let get = |id: &str| f.iter().find(|r| r.id() == id).unwrap().as_any().downcast_ref::<Ec2Instance>().unwrap().clone();
    let cases = [
        ("i-0win", Some(OsFamily::Windows), Some("Windows")),
        ("i-0sql", Some(OsFamily::Windows), Some("Windows + SQL")),
        ("i-0al2023", Some(OsFamily::Linux), Some("Linux")),
        ("i-0rhel", Some(OsFamily::Linux), Some("RHEL")),
        ("i-0ubuntu", Some(OsFamily::Linux), Some("Ubuntu Pro")),
        ("i-0none", None, None),
    ];
    for (id, family, label) in cases {
        let i = get(id);
        assert_eq!(i.os_family(), family, "{id}");
        assert_eq!(i.os_label().as_deref(), label, "{id}");
    }
    // `Platform` alone (no details) still says Windows.
    let bare = instance("i-0bare", "x", None, true);
    assert_eq!(bare.os_family(), Some(OsFamily::Windows));

    // The list's second column: type, then the OS.
    assert_eq!(get("i-0rhel").list_summary().as_deref(), Some("t3.large · RHEL · i-0rhel"));
    assert_eq!(get("i-0none").list_summary().as_deref(), Some("t3.large · i-0none"));
}

#[test]
fn a_linux_search_finds_the_linux_instances() {
    let linux = ids_matching("linux");
    for id in ["i-0al2023", "i-0rhel", "i-0ubuntu"] {
        assert!(linux.contains(&id.to_string()), "{id} missing from {linux:?}");
    }
    assert!(!linux.contains(&"i-0sql".to_string()), "{linux:?}");
    // Fuzzy still lets the Windows box's tag spell it out, but the real
    // matches rank above it.
    let fleet = fleet();
    let ranked: Vec<&str> =
        FuzzyMatcher::new().filter_resources("linux", &fleet).into_iter().map(|(i, _)| fleet[i].id()).collect();
    assert!(ranked[..3].iter().all(|id| ["i-0al2023", "i-0rhel", "i-0ubuntu"].contains(id)), "{ranked:?}");

    let windows = ids_matching("windows");
    assert_eq!(windows, ["i-0sql", "i-0win"]);
    assert_eq!(ids_matching("sql server"), ["i-0sql"]);
    assert_eq!(ids_matching("rhel"), ["i-0rhel"]);
}

#[tokio::test]
async fn the_pane_shows_aws_s_label_and_never_guesses() {
    let (mut app, tx, _rx) = test_app().await;
    for (inst, want) in [
        (instance("i-0rhel", "billing", Some("Red Hat Enterprise Linux"), false), Some("Red Hat Enterprise Linux  (x86_64)")),
        (instance("i-0none", "emulated", None, false), None),
    ] {
        select_mock(&mut app, ServiceType::EC2, Box::new(inst));
        app.details_focused = true;
        app.reset_detail_section_to_default(&tx);
        let lines = app.get_detail_lines();
        let platform = lines.iter().find(|(k, _)| k.trim() == "Platform").map(|(_, v)| v.as_str());
        assert_eq!(platform, want, "{lines:?}");
        if want.is_none() {
            assert!(lines.iter().any(|(k, v)| k.trim() == "Architecture" && v == "x86_64"), "{lines:?}");
            assert!(!lines.iter().any(|(_, v)| v.contains("Linux/UNIX")), "no guess: {lines:?}");
        }
    }
}
