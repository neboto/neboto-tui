//! Change timeline (`W`): the operational sources — Auto Scaling activities
//! and RDS events — fire for the resources they belong to, and only those.

use super::*;
use crate::aws::services::cloudtrail::TrailRange;
use crate::timeline::TimelineSource;

fn mock(label: &str) -> (ServiceType, Box<dyn Resource>) {
    all_mocks()
        .into_iter()
        .find(|(_, l, _)| *l == label)
        .map(|(s, _, r)| (s, r))
        .unwrap_or_else(|| panic!("a {label} mock"))
}

fn pending(app: &App) -> Vec<TimelineSource> {
    app.trail_in_pane.as_ref().expect("lens open").pending.clone()
}

#[tokio::test]
async fn an_rds_instance_gets_rds_events_and_a_retention_note_past_14_days() {
    let (mut app, tx, _rx) = test_app().await;
    let (svc, r) = mock("RdsInstance");
    select_mock(&mut app, svc, r);
    app.open_trail_lens(&tx);
    assert!(pending(&app).contains(&TimelineSource::Rds));
    assert!(!pending(&app).contains(&TimelineSource::Scaling));
    let notes = |app: &App| app.trail_in_pane.as_ref().unwrap().aux_notes.join(" | ");
    assert!(!notes(&app).contains("14 days"), "7d fits inside RDS retention");

    app.trail_in_pane.as_mut().unwrap().range = TrailRange::D90;
    app.reseed_trail_lens(&tx);
    assert!(notes(&app).contains("RDS keeps 14 days"), "{}", notes(&app));
}

#[tokio::test]
async fn an_instance_in_an_asg_gets_its_scaling_activities() {
    let (mut app, tx, _rx) = test_app().await;
    let (svc, r) = mock("Ec2Instance");
    let mut inst = r
        .as_any()
        .downcast_ref::<crate::aws::services::ec2::Ec2Instance>()
        .unwrap()
        .clone();
    inst.tags.remove("aws:autoscaling:groupName");
    select_mock(&mut app, svc, Box::new(inst.clone()));
    app.open_trail_lens(&tx);
    assert!(!pending(&app).contains(&TimelineSource::Scaling), "no group tag, no ASG source");

    inst.tags.insert("aws:autoscaling:groupName".into(), "web-asg".into());
    select_mock(&mut app, svc, Box::new(inst));
    app.open_trail_lens(&tx);
    assert!(pending(&app).contains(&TimelineSource::Scaling));
    assert!(!pending(&app).contains(&TimelineSource::Rds));
}
