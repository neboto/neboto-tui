//! DocumentDB and Neptune (issue #96): the RDS API listed under one engine.
//! The rows are the RDS types, so what's pinned here is what varies by
//! family: the type names behind the sub-tabs, the jumps that stay in the
//! service, the metric namespace, the CLI service and the log group.

use super::*;
use crate::app::{DocDbView, JumpView, NeptuneView, RdsView};
use crate::aws::services::rds::{RdsCluster, RdsFamily, RdsInstance, RdsParamGroup, RdsSnapshot, RdsSubnetGroup};
use crate::ui::widgets::details_pane::resource_jump_target;

fn cluster(engine: &str, family: RdsFamily) -> RdsCluster {
    RdsCluster::from_sdk(
        &aws_sdk_rds::types::DbCluster::builder()
            .db_cluster_identifier("orders")
            .engine(engine)
            .enabled_cloudwatch_logs_exports("audit")
            .build(),
    )
    .listed(family)
}

fn instance(engine: &str, family: RdsFamily) -> RdsInstance {
    RdsInstance::from_sdk(
        &aws_sdk_rds::types::DbInstance::builder()
            .db_instance_identifier("orders-1")
            .db_cluster_identifier("orders")
            .engine(engine)
            .build(),
    )
    .listed(family)
}

fn family_rows(engine: &str, pg_family: &str, family: RdsFamily) -> Vec<Box<dyn Resource>> {
    vec![
        Box::new(cluster(engine, family)),
        Box::new(instance(engine, family)),
        Box::new(
            RdsSnapshot::from_cluster(
                &aws_sdk_rds::types::DbClusterSnapshot::builder()
                    .db_cluster_snapshot_identifier("orders-snap")
                    .engine(engine)
                    .build(),
            )
            .listed(family),
        ),
        Box::new(
            RdsParamGroup::from_cluster_sdk(
                &aws_sdk_rds::types::DbClusterParameterGroup::builder()
                    .db_cluster_parameter_group_name("orders-pg")
                    .db_parameter_group_family(pg_family)
                    .build(),
            )
            .listed(family),
        ),
        Box::new(
            RdsSubnetGroup::from_sdk(&aws_sdk_rds::types::DbSubnetGroup::builder().db_subnet_group_name("db").build())
                .listed(family),
        ),
    ]
}

#[tokio::test]
async fn sub_tabs_filter_by_the_familys_type_names() {
    for (service, engine, pg, family, prefix) in [
        (ServiceType::DocumentDB, "docdb", "docdb5.0", RdsFamily::DocDb, "DocumentDB"),
        (ServiceType::Neptune, "neptune", "neptune1.3", RdsFamily::Neptune, "Neptune"),
    ] {
        let (mut app, tx, _rx) = test_app().await;
        select_mock(&mut app, service, Box::new(cluster(engine, family)));
        app.resources = family_rows(engine, pg, family);
        for (key, kind) in
            [('1', "Cluster"), ('2', "Instance"), ('3', "Snapshot"), ('4', "Parameter Group"), ('5', "Subnet Group")]
        {
            app.handle_event(Event::Key(KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE)), &tx)
                .await
                .unwrap();
            let types: Vec<&str> = app.filtered_resources.iter().map(|&i| app.resources[i].resource_type()).collect();
            assert_eq!(types, [format!("{prefix} {kind}")], "{service:?} key {key}");
        }
        // Tab wraps from the last tab back to Clusters.
        app.handle_event(Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)), &tx)
            .await
            .unwrap();
        let types: Vec<&str> = app.filtered_resources.iter().map(|&i| app.resources[i].resource_type()).collect();
        assert_eq!(types, [format!("{prefix} Cluster")], "{service:?} Tab");
    }
}

/// `@rds` lists every engine, as the RDS console does: the same DocumentDB
/// cluster is an "RDS Cluster" there, so it lands on the Clusters tab.
#[test]
fn the_listing_service_names_the_type_and_the_engine_decides_the_rest() {
    let under_rds = cluster("docdb", RdsFamily::Rds);
    assert_eq!(under_rds.resource_type(), "RDS Cluster");
    assert_eq!(under_rds.cli_command().unwrap(), "aws docdb describe-db-clusters --db-cluster-identifier orders");
    assert_eq!(under_rds.preferred_log_group().unwrap().0, "/aws/docdb/orders/audit");

    let neptune = cluster("neptune", RdsFamily::Neptune);
    assert_eq!(neptune.resource_type(), "Neptune Cluster");
    assert_eq!(neptune.cli_command().unwrap(), "aws neptune describe-db-clusters --db-cluster-identifier orders");
    assert_eq!(neptune.preferred_log_group().unwrap().0, "/aws/neptune/orders/audit");

    let aurora = cluster("aurora-postgresql", RdsFamily::Rds);
    assert_eq!(aurora.cli_command().unwrap(), "aws rds describe-db-clusters --db-cluster-identifier orders");
    assert_eq!(aurora.preferred_log_group().unwrap().0, "/aws/rds/cluster/orders/audit");

    // An instance of either engine has no exports of its own.
    assert!(instance("docdb", RdsFamily::DocDb).preferred_log_group().is_none());
}

#[tokio::test]
async fn row_and_arn_jumps_stay_in_the_service() {
    let (mut app, _tx, _rx) = test_app().await;
    select_mock(&mut app, ServiceType::DocumentDB, Box::new(instance("docdb", RdsFamily::DocDb)));
    let t = app.rds_row_jump_target("Cluster", "orders").unwrap();
    assert_eq!(t.service, ServiceType::DocumentDB);
    assert!(matches!(t.view, JumpView::DocDb(DocDbView::Clusters)));
    let t = app.rds_row_jump_target("Parameter Group", "orders-pg (in-sync)").unwrap();
    assert!(matches!(t.view, JumpView::DocDb(DocDbView::ParamGroups)));
    assert_eq!(t.id, "orders-pg");
    assert!(app.rds_row_jump_target("Option Group", "default:docdb").is_none(), "no option groups here");

    select_mock(&mut app, ServiceType::Neptune, Box::new(cluster("neptune", RdsFamily::Neptune)));
    let t = app.rds_row_jump_target("Writer", "orders-1").unwrap();
    assert_eq!(t.service, ServiceType::Neptune);
    assert!(matches!(t.view, JumpView::Neptune(NeptuneView::Instances)));

    // `@rds` is unchanged.
    select_mock(&mut app, ServiceType::RDS, Box::new(cluster("aurora-postgresql", RdsFamily::Rds)));
    let t = app.rds_row_jump_target("Option Group", "default:mysql-8-0").unwrap();
    assert!(matches!(t.view, JumpView::Rds(RdsView::OptionGroups)));

    let arn = "arn:aws:rds:us-east-1:123456789012:cluster:orders";
    let t = resource_jump_target("  ARN", arn, ServiceType::Neptune).unwrap();
    assert_eq!(t.service, ServiceType::Neptune);
    assert!(matches!(t.view, JumpView::Neptune(NeptuneView::Clusters)));
    let t = resource_jump_target("  ARN", arn, ServiceType::Lambda).unwrap();
    assert_eq!(t.service, ServiceType::RDS);
}

#[tokio::test]
async fn charts_use_the_engines_namespace_wherever_the_row_is_listed() {
    let (mut app, _tx, _rx) = test_app().await;
    for (service, row, namespace) in [
        (ServiceType::DocumentDB, cluster("docdb", RdsFamily::DocDb), "AWS/DocDB"),
        (ServiceType::RDS, cluster("docdb", RdsFamily::Rds), "AWS/DocDB"),
        (ServiceType::Neptune, cluster("neptune", RdsFamily::Neptune), "AWS/Neptune"),
        (ServiceType::RDS, cluster("aurora-mysql", RdsFamily::Rds), "AWS/RDS"),
    ] {
        select_mock(&mut app, service, Box::new(row));
        assert!(app.supports_metrics_overlay());
        let target = app.selected_metric_target().expect("a cluster charts");
        assert_eq!(target.namespace, namespace, "{service:?}");
    }
}
