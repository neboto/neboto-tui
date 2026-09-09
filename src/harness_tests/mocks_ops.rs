//! Mock resources for the layer-2 harness — see mod.rs.
use super::Mock;
use crate::aws::service::ServiceType;
use crate::aws::services as svc;
use std::collections::HashMap;

pub(super) fn mocks() -> Vec<Mock> {
    vec![
        // ── CloudWatch ──────────────────────────────────────────────────────
        (
            ServiceType::CloudWatch,
            "CwAlarm",
            Box::new(svc::cloudwatch::CwAlarm::from_sdk(
                &aws_sdk_cloudwatch::types::MetricAlarm::builder()
                    .alarm_name("mock-alarm")
                    .build(),
            )),
        ),
        (
            ServiceType::CloudWatch,
            "CwCompositeAlarm",
            Box::new(svc::cloudwatch::CwCompositeAlarm::from_sdk(
                &aws_sdk_cloudwatch::types::CompositeAlarm::builder()
                    .alarm_name("mock-composite-alarm")
                    .build(),
            )),
        ),
        (
            ServiceType::CloudWatch,
            "CwDashboard",
            Box::new(svc::cloudwatch::CwDashboard::from_sdk(
                &aws_sdk_cloudwatch::types::DashboardEntry::builder()
                    .dashboard_name("mock-dashboard")
                    .build(),
            )),
        ),
        (
            ServiceType::CloudWatch,
            "CwLogGroup",
            Box::new(svc::cloudwatch::CwLogGroup::from_sdk(
                &aws_sdk_cloudwatchlogs::types::LogGroup::builder()
                    .log_group_name("/mock/log-group")
                    .build(),
            )),
        ),
        (
            ServiceType::CloudWatch,
            "CwMetricStream",
            Box::new(svc::cloudwatch::CwMetricStream::from_sdk(
                &aws_sdk_cloudwatch::types::MetricStreamEntry::builder()
                    .name("mock-metric-stream")
                    .arn("arn:aws:cloudwatch:us-east-1:123456789012:metric-stream/mock-metric-stream")
                    .state("running")
                    .build(),
            )),
        ),
        (
            ServiceType::CloudWatch,
            "CwAnomalyDetector",
            Box::new(svc::cloudwatch::CwAnomalyDetector::from_sdk(
                &aws_sdk_cloudwatch::types::AnomalyDetector::builder()
                    .single_metric_anomaly_detector(
                        aws_sdk_cloudwatch::types::SingleMetricAnomalyDetector::builder()
                            .namespace("AWS/EC2")
                            .metric_name("CPUUtilization")
                            .stat("Average")
                            .build(),
                    )
                    .build(),
            )),
        ),
        (
            ServiceType::CloudWatch,
            "CwInsightRule",
            Box::new(svc::cloudwatch::CwInsightRule::from_sdk(
                &aws_sdk_cloudwatch::types::InsightRule::builder()
                    .name("mock-insight-rule")
                    .state("ENABLED")
                    .schema("{\"Name\":\"CloudWatchLogRule\",\"Version\":1}")
                    .definition("{\"Keys\":[\"$.ip\"]}")
                    .build(),
            )),
        ),
        (
            ServiceType::CloudWatch,
            "CwAccountPolicy",
            Box::new(svc::cloudwatch::CwAccountPolicy::from_sdk(
                &aws_sdk_cloudwatchlogs::types::AccountPolicy::builder()
                    .policy_name("mock-account-policy")
                    .policy_type(aws_sdk_cloudwatchlogs::types::PolicyType::DataProtectionPolicy)
                    .policy_document("{\"Name\":\"data-protection\"}")
                    .build(),
            )),
        ),
        (
            ServiceType::CloudWatch,
            "OamSink",
            Box::new(svc::oam::OamSink::from_sdk(
                &aws_sdk_oam::types::ListSinksItem::builder()
                    .name("mock-sink")
                    .arn("arn:aws:oam:us-east-1:123456789012:sink/mock-sink-id")
                    .id("mock-sink-id")
                    .build(),
            )),
        ),
        (
            ServiceType::CloudWatch,
            "OamLink",
            Box::new(svc::oam::OamLink::from_sdk(
                &aws_sdk_oam::types::ListLinksItem::builder()
                    .label("mock-link")
                    .arn("arn:aws:oam:us-east-1:123456789012:link/mock-link-id")
                    .id("mock-link-id")
                    .sink_arn("arn:aws:oam:us-east-1:999999999999:sink/mock-sink-id")
                    .resource_types("AWS::CloudWatch::Metric")
                    .build(),
            )),
        ),
        // ── CloudFormation ──────────────────────────────────────────────────
        (
            ServiceType::CloudFormation,
            "CfnStack",
            Box::new(svc::cloudformation::CfnStack::from_sdk(
                &aws_sdk_cloudformation::types::Stack::builder()
                    .stack_name("mock-stack")
                    .stack_id("arn:aws:cloudformation:us-east-1:123456789012:stack/mock-stack/abc")
                    .build(),
            )),
        ),
        (
            ServiceType::CloudFormation,
            "CfnDeletedStack",
            Box::new(svc::cloudformation::CfnStack::from_deleted_summary(
                &aws_sdk_cloudformation::types::StackSummary::builder()
                    .stack_name("mock-deleted-stack")
                    .stack_id(
                        "arn:aws:cloudformation:us-east-1:123456789012:stack/mock-deleted-stack/def",
                    )
                    .stack_status(aws_sdk_cloudformation::types::StackStatus::DeleteComplete)
                    .creation_time(aws_smithy_types::DateTime::from_secs(0))
                    .deletion_time(aws_smithy_types::DateTime::from_secs(86400))
                    .build(),
            )),
        ),
        (
            ServiceType::CloudFormation,
            "CfnStackSet",
            Box::new(svc::cloudformation::CfnStackSet::from_summary(
                &aws_sdk_cloudformation::types::StackSetSummary::builder()
                    .stack_set_name("mock-stack-set")
                    .stack_set_id("mock-stack-set:abc")
                    .build(),
            )),
        ),
        (
            ServiceType::CloudFormation,
            "CfnExport",
            Box::new(svc::cloudformation::CfnExport::from_sdk(
                &aws_sdk_cloudformation::types::Export::builder()
                    .name("mock-export")
                    .value("mock-value")
                    .exporting_stack_id(
                        "arn:aws:cloudformation:us-east-1:123456789012:stack/mock-stack/abc",
                    )
                    .build(),
            )),
        ),
        // ── SSM ──────────────────────────────────────────────────────────────
        (
            ServiceType::Ssm,
            "SsmParameter",
            Box::new(svc::ssm::SsmParameter::from_sdk(
                &aws_sdk_ssm::types::ParameterMetadata::builder()
                    .name("/mock/parameter")
                    .build(),
            )),
        ),
        (
            ServiceType::Ssm,
            "SsmDocument",
            Box::new(svc::ssm::SsmDocument::from_sdk(
                &aws_sdk_ssm::types::DocumentIdentifier::builder()
                    .name("mock-document")
                    .build(),
            )),
        ),
        (
            ServiceType::Ssm,
            "SsmAssociation",
            Box::new(svc::ssm::SsmAssociation::from_sdk(
                &aws_sdk_ssm::types::Association::builder()
                    .association_id("00000000-0000-0000-0000-000000000000")
                    .name("mock-document")
                    .build(),
            )),
        ),
        (
            ServiceType::Ssm,
            "SsmManagedInstance",
            Box::new(svc::ssm::SsmManagedInstance::from_sdk(
                &aws_sdk_ssm::types::InstanceInformation::builder()
                    .instance_id("mi-0123456789abcdef0")
                    .build(),
            )),
        ),
        (
            ServiceType::Ssm,
            "SsmCommand",
            Box::new(svc::ssm::SsmCommand::from_sdk(
                &aws_sdk_ssm::types::Command::builder()
                    .command_id("00000000-0000-0000-0000-000000000001")
                    .document_name("mock-document")
                    .build(),
            )),
        ),
        (
            ServiceType::Ssm,
            "SsmAutomationExecution",
            Box::new(svc::ssm::SsmAutomationExecution::from_sdk(
                &aws_sdk_ssm::types::AutomationExecutionMetadata::builder()
                    .automation_execution_id("00000000-0000-0000-0000-000000000002")
                    .document_name("mock-runbook")
                    .build(),
            )),
        ),
        (
            ServiceType::Ssm,
            "SsmMaintWindow",
            Box::new(svc::ssm::SsmMaintWindow::from_sdk(
                &aws_sdk_ssm::types::MaintenanceWindowIdentity::builder()
                    .window_id("mw-0123456789abcdef0")
                    .name("mock-window")
                    .build(),
            )),
        ),
        (
            ServiceType::Ssm,
            "SsmPatchBaseline",
            Box::new(svc::ssm::SsmPatchBaseline::from_sdk(
                &aws_sdk_ssm::types::PatchBaselineIdentity::builder()
                    .baseline_id("pb-0123456789abcdef0")
                    .baseline_name("mock-baseline")
                    .build(),
            )),
        ),
        (
            ServiceType::Ssm,
            "SsmOpsItem",
            Box::new(svc::ssm::SsmOpsItem::from_sdk(
                &aws_sdk_ssm::types::OpsItemSummary::builder()
                    .ops_item_id("oi-0123456789ab")
                    .title("mock ops item")
                    .build(),
            )),
        ),
        // ── Trusted Advisor ──────────────────────────────────────────────────
        (
            ServiceType::TrustedAdvisor,
            "TaCheck",
            Box::new(svc::trusted_advisor::TaCheck {
                id: "mock-check-id".to_string(),
                name: "Mock Check".to_string(),
                category: "cost_optimizing".to_string(),
                description: "A mock Trusted Advisor check.".to_string(),
                status: "ok".to_string(),
                resources_flagged: 0,
                resources_processed: 10,
                resources_suppressed: 0,
                metadata_cols: vec!["Resource".to_string(), "Status".to_string()],
                tags: HashMap::new(),
                org_scope: false,
                org_accounts: vec![],
            }),
        ),
        // Org-aggregated variant — carries the reduced Summary/Accounts
        // descriptor, so the harness must see its split renderer too.
        (
            ServiceType::TrustedAdvisor,
            "TaOrgCheck",
            Box::new(svc::trusted_advisor::TaCheck {
                id: "mock-org-check-id".to_string(),
                name: "Mock Org Check".to_string(),
                category: "security".to_string(),
                description: "A mock org-aggregated Trusted Advisor check.".to_string(),
                status: "error".to_string(),
                resources_flagged: 3,
                resources_processed: 20,
                resources_suppressed: 0,
                metadata_cols: vec![],
                tags: HashMap::new(),
                org_scope: true,
                org_accounts: vec![svc::trusted_advisor::TaAccountStatus {
                    account_id: "111111111111".to_string(),
                    account_name: "mock-member".to_string(),
                    status: "error".to_string(),
                    flagged: 3,
                    processed: 20,
                    suppressed: 0,
                }],
            }),
        ),
        // Priority recommendation — its own resource type with an
        // Overview/Accounts/Resources pane.
        (
            ServiceType::TrustedAdvisor,
            "TaRecommendation",
            Box::new(svc::trusted_advisor::TaRecommendation {
                id: "mock-rec-id".to_string(),
                arn: "arn:aws:trustedadvisor:::organization-recommendation/mock-rec-id"
                    .to_string(),
                name: "Mock Priority Recommendation".to_string(),
                status: "warning".to_string(),
                lifecycle: "in_progress".to_string(),
                rec_type: "priority".to_string(),
                pillars: vec!["security".to_string()],
                source: "manual".to_string(),
                aws_services: vec!["iam".to_string()],
                error_count: 0,
                warning_count: 2,
                ok_count: 5,
                created_at: "2024-01-01T00:00:00Z".to_string(),
                last_updated: "2024-02-01T00:00:00Z".to_string(),
                affected_accounts: vec!["mock-member (111111111111)".to_string()],
                tags: HashMap::new(),
            }),
        ),
        // ── Health ───────────────────────────────────────────────────────────
        (
            ServiceType::Health,
            "HealthEvent",
            Box::new(svc::health::HealthEvent {
                arn: "arn:aws:health:us-east-1::event/EC2/AWS_EC2_OPERATIONAL_ISSUE/mock-event"
                    .to_string(),
                service: "EC2".to_string(),
                region: "us-east-1".to_string(),
                availability_zone: String::new(),
                event_type_code: "AWS_EC2_OPERATIONAL_ISSUE".to_string(),
                category: "issue".to_string(),
                status_code: "open".to_string(),
                scope: "ACCOUNT_SPECIFIC".to_string(),
                start_time: "2024-01-01T00:00:00Z".to_string(),
                end_time: String::new(),
                last_updated_time: "2024-01-01T00:00:00Z".to_string(),
            }),
        ),
        // ── Resource Groups ──────────────────────────────────────────────────
        (
            ServiceType::ResourceGroups,
            "ResourceGroup",
            Box::new(svc::resource_groups::ResourceGroup {
                name: "mock-group".to_string(),
                arn: "arn:aws:resource-groups:us-east-1:123456789012:group/mock-group".to_string(),
                description: "A mock resource group.".to_string(),
                query_type: Some("TAG_FILTERS_1_0".to_string()),
                tags: HashMap::new(),
            }),
        ),
        // ── CloudTrail ───────────────────────────────────────────────────────
        (
            ServiceType::CloudTrail,
            "CloudTrailEvent",
            Box::new(svc::cloudtrail::CloudTrailEvent::from_sdk(
                &aws_sdk_cloudtrail::types::Event::builder()
                    .event_id("00000000-0000-0000-0000-000000000003")
                    .event_name("MockEvent")
                    .username("mock-user")
                    .cloud_trail_event(
                        r#"{"eventVersion":"1.08","awsRegion":"us-east-1","eventSource":"ec2.amazonaws.com","eventName":"MockEvent","userIdentity":{"type":"IAMUser","arn":"arn:aws:iam::123456789012:user/mock-user"}}"#,
                    )
                    .build(),
            )),
        ),
        (
            ServiceType::CloudTrail,
            "CloudTrailTrail",
            Box::new(svc::cloudtrail::CloudTrailTrail::from_sdk(
                &aws_sdk_cloudtrail::types::Trail::builder()
                    .name("mock-trail")
                    .trail_arn("arn:aws:cloudtrail:us-east-1:123456789012:trail/mock-trail")
                    .s3_bucket_name("mock-trail-bucket")
                    .build(),
                Err("mock: status unavailable".to_string()),
                Err("mock: selectors unavailable".to_string()),
            )),
        ),
        // ── Backup ───────────────────────────────────────────────────────────
        (
            ServiceType::Backup,
            "BackupPlan",
            Box::new(svc::backup::BackupPlan {
                id: "mock-plan-id".to_string(),
                name: "mock-plan".to_string(),
                arn: "arn:aws:backup:us-east-1:123456789012:backup-plan:mock-plan-id".to_string(),
                last_execution: None,
                tags: HashMap::new(),
            }),
        ),
        (
            ServiceType::Backup,
            "BackupVault",
            Box::new(svc::backup::BackupVault {
                name: "mock-vault".to_string(),
                arn: "arn:aws:backup:us-east-1:123456789012:backup-vault:mock-vault".to_string(),
                recovery_points: 0,
                encryption_key_arn: String::new(),
                locked: false,
                created: None,
                tags: HashMap::new(),
            }),
        ),
        // ── Control Tower ───────────────────────────────────────────────────
        (
            ServiceType::ControlTower,
            "LandingZone",
            Box::new(svc::controltower::LandingZone::from_sdk(
                "arn:aws:controltower:us-east-1:123456789012:landingzone/MOCKLZID",
                None,
            )),
        ),
        (
            ServiceType::ControlTower,
            "EnabledControl",
            Box::new(svc::controltower::EnabledControl::from_sdk(
                &aws_sdk_controltower::types::EnabledControlSummary::builder()
                    .arn("arn:aws:controltower:us-east-1:123456789012:enabledcontrol/MOCKEC")
                    .control_identifier(
                        "arn:aws:controltower:us-east-1::control/AWS-GR_MOCK_CONTROL",
                    )
                    .target_identifier(
                        "arn:aws:organizations::123456789012:ou/o-mock/ou-mock-11111111",
                    )
                    .build(),
                &HashMap::new(),
                &svc::controltower::ControlCatalog::default(),
            )),
        ),
        (
            ServiceType::ControlTower,
            "EnabledBaseline",
            Box::new(svc::controltower::EnabledBaseline::from_sdk(
                &aws_sdk_controltower::types::EnabledBaselineSummary::builder()
                    .arn("arn:aws:controltower:us-east-1:123456789012:enabledbaseline/MOCKEB")
                    .baseline_identifier(
                        "arn:aws:controltower:us-east-1::baseline/MOCKBASELINE",
                    )
                    .target_identifier(
                        "arn:aws:organizations::123456789012:ou/o-mock/ou-mock-11111111",
                    )
                    .build()
                    .expect("mock enabled baseline builds"),
                &HashMap::new(),
                &HashMap::new(),
            )),
        ),
        (
            ServiceType::ControlTower,
            "CtOp",
            Box::new(svc::controltower::CtOp::from_control_op(
                &aws_sdk_controltower::types::ControlOperationSummary::builder()
                    .operation_identifier("00000000-mock-op")
                    .operation_type(aws_sdk_controltower::types::ControlOperationType::EnableControl)
                    .build(),
                &HashMap::new(),
            )),
        ),
        (
            ServiceType::ControlTower,
            "CatalogControl",
            Box::new(svc::controltower::CatalogControl::new(
                svc::controltower::CatalogMeta {
                    arn: "arn:aws:controlcatalog:::control/mockuuid".to_string(),
                    name: "Mock catalog control".to_string(),
                    description: "A mock control for the harness".to_string(),
                    ..Default::default()
                },
                vec!["ou-mock-11111111 (Mock OU)".to_string()],
            )),
        ),
        (
            ServiceType::ControlTower,
            "CtAccount",
            Box::new(svc::controltower::CtAccount::build(
                &svc::controltower::OrgAccountInfo {
                    id: "123456789012".to_string(),
                    name: "mock-audit".to_string(),
                    email: Some("mock@example.com".to_string()),
                    status: Some("ACTIVE".to_string()),
                    joined_method: Some("CREATED".to_string()),
                    joined_at: Some("2024-01-01 00:00".to_string()),
                },
                &HashMap::from([(
                    "123456789012".to_string(),
                    ("ou-mock-11111111".to_string(), "Root / Security".to_string()),
                )]),
                &HashMap::from([(
                    "123456789012".to_string(),
                    (Some("SUCCEEDED".to_string()), Some("IN_SYNC".to_string())),
                )]),
                &HashMap::from([("ou-mock-11111111".to_string(), (3, 1))]),
                &HashMap::from([("123456789012".to_string(), (5, 2))]),
            )),
        ),
        (
            ServiceType::ControlTower,
            "CtCompliance",
            Box::new(svc::controltower::CtCompliance::from_sdk(
                "aws-controltower-MockAggregator",
                &aws_sdk_config::types::AggregateComplianceByConfigRule::builder()
                    .config_rule_name("AWSControlTower_AWS-GR_MOCK_RULE")
                    .account_id("123456789012")
                    .aws_region("us-east-1")
                    .build(),
                &HashMap::new(),
                &svc::controltower::ControlCatalog::default(),
            )),
        ),
        // ── Service Catalog ──────────────────────────────────────────────────
        (
            ServiceType::ServiceCatalog,
            "ScPortfolio",
            Box::new(svc::servicecatalog::ScPortfolio::from_sdk(
                &aws_sdk_servicecatalog::types::PortfolioDetail::builder()
                    .id("port-mock1234567890")
                    .display_name("mock-portfolio")
                    .provider_name("mock-provider")
                    .build(),
            )),
        ),
        (
            ServiceType::ServiceCatalog,
            "ScProduct",
            Box::new(svc::servicecatalog::ScProduct::from_sdk(
                &aws_sdk_servicecatalog::types::ProductViewDetail::builder()
                    .product_view_summary(
                        aws_sdk_servicecatalog::types::ProductViewSummary::builder()
                            .product_id("prod-mock1234567890")
                            .name("mock-product")
                            .owner("mock-owner")
                            .build(),
                    )
                    .status(aws_sdk_servicecatalog::types::Status::Available)
                    .build(),
            )),
        ),
        (
            ServiceType::ServiceCatalog,
            "ScProvisionedProduct",
            Box::new(svc::servicecatalog::ScProvisionedProduct::from_sdk(
                &aws_sdk_servicecatalog::types::ProvisionedProductAttribute::builder()
                    .id("pp-mock1234567890")
                    .name("mock-provisioned-product")
                    .status(aws_sdk_servicecatalog::types::ProvisionedProductStatus::Available)
                    .product_id("prod-mock1234567890")
                    .product_name("mock-product")
                    .build(),
            )),
        ),
        // ── S3 Files ─────────────────────────────────────────────────────────
        (
            ServiceType::S3Files,
            "S3FileSystem",
            Box::new(svc::s3files::S3FileSystem::from_sdk(
                &aws_sdk_s3files::types::ListFileSystemsDescription::builder()
                    .file_system_id("fs-0123456789abcdef01")
                    .file_system_arn(
                        "arn:aws:s3files:us-east-1:123456789012:file-system/fs-0123456789abcdef01",
                    )
                    .name("mock-file-system")
                    .bucket("arn:aws:s3:::mock-bucket")
                    .status(aws_sdk_s3files::types::LifeCycleState::Available)
                    .role_arn("arn:aws:iam::123456789012:role/mock-s3files-role")
                    .owner_id("123456789012")
                    .creation_time(aws_smithy_types::DateTime::from_secs(1_700_000_000))
                    .build()
                    .expect("mock file system builds"),
            )),
        ),
        // ── S3 Tables ────────────────────────────────────────────────────────
        (
            ServiceType::S3Tables,
            "S3TableBucket",
            Box::new(svc::s3tables::S3TableBucket::from_sdk(
                &aws_sdk_s3tables::types::TableBucketSummary::builder()
                    .arn("arn:aws:s3tables:us-east-1:123456789012:bucket/mock-table-bucket")
                    .name("mock-table-bucket")
                    .owner_account_id("123456789012")
                    .created_at(aws_smithy_types::DateTime::from_secs(1_700_000_000))
                    .build()
                    .expect("mock table bucket builds"),
            )),
        ),
        (
            ServiceType::S3Tables,
            "S3Table",
            Box::new(svc::s3tables::S3Table::from_sdk(
                &aws_sdk_s3tables::types::TableSummary::builder()
                    .namespace("mock_namespace")
                    .name("mock_table")
                    .r#type(aws_sdk_s3tables::types::TableType::Customer)
                    .table_arn(
                        "arn:aws:s3tables:us-east-1:123456789012:bucket/mock-table-bucket/table/0000aaaa-1111-2222-3333-4444bbbbcccc",
                    )
                    .created_at(aws_smithy_types::DateTime::from_secs(1_700_000_000))
                    .modified_at(aws_smithy_types::DateTime::from_secs(1_700_000_000))
                    .build()
                    .expect("mock table builds"),
                "arn:aws:s3tables:us-east-1:123456789012:bucket/mock-table-bucket",
                "mock-table-bucket",
            )),
        ),
    ]
}
