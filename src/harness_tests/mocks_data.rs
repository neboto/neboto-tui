//! Mock resources for the layer-2 harness — see mod.rs.
use super::Mock;
use crate::aws::service::ServiceType;
use crate::aws::services as svc;
use std::collections::HashMap;

pub(super) fn mocks() -> Vec<Mock> {
    vec![
        (
            ServiceType::Budgets,
            "BudgetItem",
            Box::new(svc::budgets::BudgetItem::from_sdk(
                "123456789012",
                &aws_sdk_budgets::types::Budget::builder()
                    .budget_name("mock-budget")
                    .budget_type(aws_sdk_budgets::types::BudgetType::Cost)
                    .time_unit(aws_sdk_budgets::types::TimeUnit::Monthly)
                    .build()
                    .expect("mock budget builds"),
            )),
        ),
        (
            ServiceType::Invoices,
            "Invoice",
            Box::new(svc::invoicing::Invoice::from_sdk(
                &aws_sdk_invoicing::types::InvoiceSummary::builder()
                    .invoice_id("mock-invoice")
                    .build(),
            )),
        ),
        (
            ServiceType::Redshift,
            "RedshiftCluster",
            Box::new(svc::redshift::RedshiftCluster::from_sdk(
                &aws_sdk_redshift::types::Cluster::builder()
                    .cluster_identifier("mock-cluster")
                    .build(),
            )),
        ),
        (
            ServiceType::Redshift,
            "RedshiftWorkgroup",
            Box::new(svc::redshift::RedshiftWorkgroup::from_sdk(
                &aws_sdk_redshiftserverless::types::Workgroup::builder()
                    .workgroup_name("mock-wg")
                    .build(),
                None,
            )),
        ),
        (
            ServiceType::RDS,
            "RdsInstance",
            Box::new(svc::rds::RdsInstance::from_sdk(
                &aws_sdk_rds::types::DbInstance::builder()
                    .db_instance_identifier("mock-db")
                    .build(),
            )),
        ),
        (
            ServiceType::RDS,
            "RdsCluster",
            Box::new(svc::rds::RdsCluster::from_sdk(
                &aws_sdk_rds::types::DbCluster::builder()
                    .db_cluster_identifier("mock-cluster")
                    .build(),
            )),
        ),
        (
            ServiceType::RDS,
            "RdsSnapshot",
            Box::new(svc::rds::RdsSnapshot::from_db(
                &aws_sdk_rds::types::DbSnapshot::builder()
                    .db_snapshot_identifier("mock-snap")
                    .db_instance_identifier("mock-db")
                    .build(),
            )),
        ),
        (
            ServiceType::RDS,
            "RdsParamGroup",
            Box::new(svc::rds::RdsParamGroup::from_sdk(
                &aws_sdk_rds::types::DbParameterGroup::builder()
                    .db_parameter_group_name("mock-pg")
                    .db_parameter_group_family("postgres16")
                    .build(),
            )),
        ),
        (
            ServiceType::RDS,
            "RdsOptionGroup",
            Box::new(svc::rds::RdsOptionGroup::from_sdk(
                &aws_sdk_rds::types::OptionGroup::builder()
                    .option_group_name("mock-og")
                    .engine_name("mysql")
                    .build(),
            )),
        ),
        (
            ServiceType::RDS,
            "RdsSubnetGroup",
            Box::new(svc::rds::RdsSubnetGroup::from_sdk(
                &aws_sdk_rds::types::DbSubnetGroup::builder()
                    .db_subnet_group_name("mock-sng")
                    .vpc_id("vpc-123")
                    .build(),
            )),
        ),
        (
            ServiceType::DynamoDb,
            "DdbTable",
            Box::new(svc::dynamodb::DdbTable::from_sdk(
                &aws_sdk_dynamodb::types::TableDescription::builder()
                    .table_name("mock-table")
                    .table_arn("arn:aws:dynamodb:us-east-1:123456789012:table/mock-table")
                    .build(),
            )),
        ),
        (
            ServiceType::Athena,
            "AthenaWorkgroup",
            Box::new(svc::athena::AthenaWorkgroup::from_sdk(
                &aws_sdk_athena::types::WorkGroup::builder()
                    .name("mock-wg")
                    .build()
                    .unwrap(),
            )),
        ),
        (
            ServiceType::Athena,
            "AthenaDatabase",
            Box::new(svc::athena::AthenaDatabase::from_sdk(
                "mock-catalog",
                &aws_sdk_athena::types::Database::builder()
                    .name("mock-db")
                    .build()
                    .unwrap(),
            )),
        ),
        (
            ServiceType::Athena,
            "AthenaQueryExecution",
            Box::new(svc::athena::AthenaQueryExecution::from_sdk(
                &aws_sdk_athena::types::QueryExecution::builder()
                    .query_execution_id("mock-query-id")
                    .query("SELECT 1")
                    .build(),
            )),
        ),
        (
            ServiceType::Athena,
            "AthenaNamedQuery",
            Box::new(svc::athena::AthenaNamedQuery::from_sdk(
                &aws_sdk_athena::types::NamedQuery::builder()
                    .name("mock-named-query")
                    .database("mock-db")
                    .query_string("SELECT 1")
                    .build()
                    .unwrap(),
            )),
        ),
        (
            ServiceType::Glue,
            "GlueTable",
            Box::new(svc::glue::GlueTable::from_sdk(
                "mock-db",
                &aws_sdk_glue::types::Table::builder()
                    .name("mock-table")
                    .build()
                    .unwrap(),
            )),
        ),
        (
            ServiceType::Glue,
            "GlueCrawler",
            Box::new(svc::glue::GlueCrawler::from_sdk(
                &aws_sdk_glue::types::Crawler::builder()
                    .name("mock-crawler")
                    .build(),
            )),
        ),
        (
            ServiceType::Glue,
            "GlueJob",
            Box::new(svc::glue::GlueJob::from_sdk(
                &aws_sdk_glue::types::Job::builder().name("mock-job").build(),
            )),
        ),
        (
            ServiceType::Glue,
            "GlueJobRun",
            Box::new(svc::glue::GlueJobRun::from_sdk(
                &aws_sdk_glue::types::JobRun::builder()
                    .id("mock-run-id")
                    .job_name("mock-job")
                    .build(),
            )),
        ),
        (
            ServiceType::Kinesis,
            "KinesisStream",
            Box::new(svc::kinesis::KinesisStream::from_sdk(
                &aws_sdk_kinesis::types::StreamDescriptionSummary::builder()
                    .stream_name("mock-stream")
                    .stream_arn("arn:aws:kinesis:us-east-1:123456789012:stream/mock-stream")
                    .stream_status(aws_sdk_kinesis::types::StreamStatus::Active)
                    .retention_period_hours(24)
                    .stream_creation_timestamp(aws_smithy_types::DateTime::from_secs(0))
                    .set_enhanced_monitoring(Some(Vec::new()))
                    .open_shard_count(1)
                    .build()
                    .unwrap(),
            )),
        ),
        (
            ServiceType::Kinesis,
            "FirehoseStream",
            Box::new(svc::kinesis::FirehoseStream::from_sdk(
                &aws_sdk_firehose::types::DeliveryStreamDescription::builder()
                    .delivery_stream_name("mock-firehose")
                    .delivery_stream_arn(
                        "arn:aws:firehose:us-east-1:123456789012:deliverystream/mock-firehose",
                    )
                    .delivery_stream_status(aws_sdk_firehose::types::DeliveryStreamStatus::Active)
                    .delivery_stream_type(aws_sdk_firehose::types::DeliveryStreamType::DirectPut)
                    .version_id("1")
                    .set_destinations(Some(Vec::new()))
                    .has_more_destinations(false)
                    .build()
                    .unwrap(),
            )),
        ),
        (
            ServiceType::Msk,
            "MskCluster",
            Box::new(svc::msk::MskCluster::from_sdk(
                &aws_sdk_kafka::types::Cluster::builder()
                    .cluster_name("mock-msk")
                    .cluster_arn("arn:aws:kafka:us-east-1:123456789012:cluster/mock-msk/abc")
                    .build(),
            )),
        ),
        (
            ServiceType::OpenSearch,
            "OpenSearchDomain",
            Box::new(svc::opensearch::OpenSearchDomain::from_sdk(
                &aws_sdk_opensearch::types::DomainStatus::builder()
                    .domain_id("123456789012/mock-domain")
                    .domain_name("mock-domain")
                    .arn("arn:aws:es:us-east-1:123456789012:domain/mock-domain")
                    .build()
                    .unwrap(),
            )),
        ),
        (
            ServiceType::ElastiCache,
            "ElastiCacheCluster",
            Box::new(svc::elasticache::ElastiCacheCluster::from_cache_cluster(
                &aws_sdk_elasticache::types::CacheCluster::builder()
                    .cache_cluster_id("mock-cache-cluster")
                    .build(),
            )),
        ),
        (
            ServiceType::Efs,
            "EfsFileSystem",
            Box::new(svc::efs::EfsFileSystem::from_sdk(
                &aws_sdk_efs::types::FileSystemDescription::builder()
                    .owner_id("123456789012")
                    .creation_token("mock-token")
                    .file_system_id("fs-0123456789abcdef0")
                    .creation_time(aws_smithy_types::DateTime::from_secs(0))
                    .life_cycle_state(aws_sdk_efs::types::LifeCycleState::Available)
                    .performance_mode(aws_sdk_efs::types::PerformanceMode::GeneralPurpose)
                    .set_tags(Some(Vec::new()))
                    .build()
                    .unwrap(),
                svc::efs::EfsLifecycle::default(),
            )),
        ),
        (
            ServiceType::Fsx,
            "FsxFileSystem",
            Box::new(svc::fsx::FsxFileSystem {
                file_system_id: "fs-0123456789abcdef0".to_string(),
                arn: "arn:aws:fsx:us-east-1:123456789012:file-system/fs-0123456789abcdef0"
                    .to_string(),
                fs_name: "mock-fsx".to_string(),
                file_system_type: "WINDOWS".to_string(),
                lifecycle: "AVAILABLE".to_string(),
                storage_capacity_gib: 32,
                storage_type: "SSD".to_string(),
                dns_name: "mock-fsx.example.com".to_string(),
                vpc_id: "vpc-0123456789abcdef0".to_string(),
                subnet_ids: Vec::new(),
                network_interface_ids: Vec::new(),
                kms_key_id: String::new(),
                creation_time: String::new(),
                failure_details: String::new(),
                config: svc::fsx::FsxConfig::Unknown,
                tags: HashMap::new(),
            }),
        ),
        (
            ServiceType::Fsx,
            "FsxVolume",
            Box::new(svc::fsx::FsxVolume {
                volume_id: "fsvol-0123456789abcdef0".to_string(),
                name: "mock-volume".to_string(),
                file_system_id: "fs-0123456789abcdef0".to_string(),
                volume_type: "ONTAP".to_string(),
                lifecycle: "AVAILABLE".to_string(),
                size_bytes: 1024,
                path: "/mock".to_string(),
                svm_id: "svm-0123456789abcdef0".to_string(),
                notes: Vec::new(),
                used_bytes: None,
                utilization_pct: None,
                tags: HashMap::new(),
            }),
        ),
        (
            ServiceType::StepFunctions,
            "SfnStateMachine",
            Box::new(svc::step_functions::SfnStateMachine::from_summary(
                &aws_sdk_sfn::types::StateMachineListItem::builder()
                    .state_machine_arn(
                        "arn:aws:states:us-east-1:123456789012:stateMachine:mock-sm",
                    )
                    .name("mock-sm")
                    .r#type(aws_sdk_sfn::types::StateMachineType::Standard)
                    .creation_date(aws_smithy_types::DateTime::from_secs(0))
                    .build()
                    .unwrap(),
            )),
        ),
        (
            ServiceType::StepFunctions,
            "SfnExecution",
            Box::new(svc::step_functions::SfnExecution::from_list_item(
                &aws_sdk_sfn::types::ExecutionListItem::builder()
                    .execution_arn(
                        "arn:aws:states:us-east-1:123456789012:execution:mock-sm:mock-exec",
                    )
                    .state_machine_arn(
                        "arn:aws:states:us-east-1:123456789012:stateMachine:mock-sm",
                    )
                    .name("mock-exec")
                    .status(aws_sdk_sfn::types::ExecutionStatus::Running)
                    .start_date(aws_smithy_types::DateTime::from_secs(0))
                    .build()
                    .unwrap(),
                "mock-sm",
            )),
        ),
        (
            ServiceType::Transfer,
            "TransferServer",
            Box::new(svc::transfer::TransferServer::from_sdk(
                &aws_sdk_transfer::types::DescribedServer::builder()
                    .server_id("s-0123456789abcdef0")
                    .arn("arn:aws:transfer:us-east-1:123456789012:server/s-0123456789abcdef0")
                    .build()
                    .unwrap(),
            )),
        ),
        (
            ServiceType::Cost,
            "CostLineItem",
            Box::new(svc::cost::CostLineItem::mock()),
        ),
    ]
}
