//! Network Firewall (#362): an internet gateway firewall in the warm cache
//! is picked up for the `N` lens, and `⏎` on its layer goes to it.

use super::*;
use crate::aws::services::network_firewall::{NfwFirewall, NfwModes};
use crate::ui::widgets::access_lens::{AccessDirection, AccessLensState};

fn firewall(name: &str, igw: bool) -> NfwFirewall {
    let r = all_mocks().into_iter().find(|(_, l, _)| *l == "NfwFirewall").unwrap().2;
    let mut f = r.as_any().downcast_ref::<NfwFirewall>().unwrap().clone();
    f.name = name.to_string();
    f.arn = format!("arn:aws:network-firewall:us-east-1:123456789012:firewall/{name}");
    f.vpc_id = "vpc-1".to_string();
    f.modes = NfwModes { igw_enforcement: igw.then(|| "POLICY_CONTROLLED".to_string()), ..Default::default() };
    f
}

#[tokio::test]
async fn the_lens_takes_igw_firewalls_from_the_cache_and_jumps_to_one() {
    let (mut app, tx, _rx) = test_app().await;
    let region = app.current_region;
    app.cache.insert(
        ServiceType::NetworkFirewall,
        region,
        None,
        vec![Box::new(firewall("edge", true)), Box::new(firewall("subnets", false))],
    );
    // A standard firewall isn't a gateway layer.
    let fws = app.cached_igw_firewalls();
    assert_eq!(fws.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["edge"]);

    let mut st = AccessLensState::open("i-1".into(), vec!["sg-a".into()], vec![], AccessDirection::Inbound);
    st.igw_firewalls = fws;
    st.add_groups(vec![crate::aws::services::ec2::SecurityGroup {
        group_id: "sg-a".into(),
        group_name: "app".into(),
        description: String::new(),
        vpc_id: Some("vpc-1".into()),
        inbound_rules: vec![],
        outbound_rules: vec![],
        tags: Default::default(),
        owner_id: None,
        unused: false,
    }]);
    st.goto_top();
    let arn = st.selected_firewall().expect("the firewall layer is selectable");
    app.details_focused = true;
    app.access_in_pane = Some(st);

    app.handle_key(key(KeyCode::Enter), &tx).await.unwrap();
    assert!(app.access_in_pane.is_none(), "⏎ leaves the lens");
    assert_eq!(app.current_service, Some(ServiceType::NetworkFirewall));
    // The cache is warm, so the jump lands at once.
    assert_eq!(app.get_selected_resource().map(|r| r.id().to_string()), Some(arn));
}
