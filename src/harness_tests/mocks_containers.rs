//! Mock resources for the layer-2 harness — see mod.rs.
use super::Mock;
use crate::aws::service::ServiceType;
use crate::aws::services as svc;
use std::collections::HashMap;

pub(super) fn mocks() -> Vec<Mock> {
    vec![
        (
            ServiceType::ECS,
            "EcsCluster",
            Box::new(svc::ecs::EcsCluster::from_sdk(
                &aws_sdk_ecs::types::Cluster::builder()
                    .cluster_arn("arn:aws:ecs:us-east-1:123456789012:cluster/mock-cluster")
                    .cluster_name("mock-cluster")
                    .status("ACTIVE")
                    .build(),
            )),
        ),
        (
            ServiceType::ECS,
            "EcsServiceInfo",
            Box::new(svc::ecs::EcsServiceInfo::from_sdk(
                &aws_sdk_ecs::types::Service::builder()
                    .service_arn("arn:aws:ecs:us-east-1:123456789012:service/mock-cluster/mock-svc")
                    .service_name("mock-svc")
                    .cluster_arn("arn:aws:ecs:us-east-1:123456789012:cluster/mock-cluster")
                    .status("ACTIVE")
                    .build(),
            )),
        ),
        (
            ServiceType::ECS,
            "EcsTask",
            Box::new(svc::ecs::EcsTask::from_sdk(
                &aws_sdk_ecs::types::Task::builder()
                    .task_arn("arn:aws:ecs:us-east-1:123456789012:task/mock-cluster/0123456789abcdef0123456789abcdef")
                    .cluster_arn("arn:aws:ecs:us-east-1:123456789012:cluster/mock-cluster")
                    .last_status("RUNNING")
                    .desired_status("RUNNING")
                    .build(),
            )),
        ),
        (
            ServiceType::ECS,
            "EcsTaskDefinition",
            Box::new(svc::ecs::EcsTaskDefinition::from_arn(
                "arn:aws:ecs:us-east-1:123456789012:task-definition/mock-family:1",
            )),
        ),
        (
            ServiceType::Eks,
            "EksCluster",
            Box::new(svc::eks::EksCluster::from_sdk(
                &aws_sdk_eks::types::Cluster::builder()
                    .name("mock-eks")
                    .arn("arn:aws:eks:us-east-1:123456789012:cluster/mock-eks")
                    .status(aws_sdk_eks::types::ClusterStatus::Active)
                    .version("1.29")
                    .build(),
            )),
        ),
        (
            ServiceType::Ecr,
            "EcrRepository",
            Box::new(svc::ecr::EcrRepository {
                name: "mock-repo".to_string(),
                arn: "arn:aws:ecr:us-east-1:123456789012:repository/mock-repo".to_string(),
                registry_id: "123456789012".to_string(),
                uri: "123456789012.dkr.ecr.us-east-1.amazonaws.com/mock-repo".to_string(),
                created: None,
                image_tag_mutability: "MUTABLE".to_string(),
                scan_on_push: false,
                encryption_type: "AES256".to_string(),
                kms_key: None,
                tags: HashMap::new(),
            }),
        ),
        (
            ServiceType::Code,
            "CodeArtifactRepo",
            Box::new(svc::code::CodeArtifactRepo {
                name: "mock-artifact-repo".to_string(),
                domain_name: "mock-domain".to_string(),
                domain_owner: "123456789012".to_string(),
                admin_account: "123456789012".to_string(),
                arn: "arn:aws:codeartifact:us-east-1:123456789012:repository/mock-domain/mock-artifact-repo".to_string(),
                description: String::new(),
                created: String::new(),
                tags: HashMap::new(),
            }),
        ),
        (
            ServiceType::Code,
            "CodeBuildProject",
            Box::new(svc::code::CodeBuildProject {
                name: "mock-build".to_string(),
                arn: "arn:aws:codebuild:us-east-1:123456789012:project/mock-build".to_string(),
                description: String::new(),
                source_type: "CODEPIPELINE".to_string(),
                source_location: String::new(),
                source_version: String::new(),
                buildspec: String::new(),
                environment_type: "LINUX_CONTAINER".to_string(),
                compute_type: "BUILD_GENERAL1_SMALL".to_string(),
                image: "aws/codebuild/standard:7.0".to_string(),
                privileged_mode: false,
                env_vars: Vec::new(),
                artifacts_type: "NO_ARTIFACTS".to_string(),
                artifacts_location: String::new(),
                artifacts_name: String::new(),
                cache_type: "NO_CACHE".to_string(),
                cache_location: String::new(),
                vpc_id: String::new(),
                vpc_subnets: Vec::new(),
                vpc_security_groups: Vec::new(),
                log_cw_group: String::new(),
                log_cw_stream: String::new(),
                log_cw_status: "ENABLED".to_string(),
                log_s3_location: String::new(),
                log_s3_status: "DISABLED".to_string(),
                badge_enabled: false,
                queued_timeout_minutes: 480,
                concurrent_build_limit: None,
                created: String::new(),
                last_modified: String::new(),
                service_role: "arn:aws:iam::123456789012:role/mock-codebuild-role".to_string(),
                timeout_minutes: 60,
                tags: HashMap::new(),
            }),
        ),
        (
            ServiceType::Code,
            "CodeCommitRepo",
            Box::new(svc::code::CodeCommitRepo {
                name: "mock-repo".to_string(),
                arn: "arn:aws:codecommit:us-east-1:123456789012:mock-repo".to_string(),
                description: String::new(),
                clone_url_http: "https://git-codecommit.us-east-1.amazonaws.com/v1/repos/mock-repo".to_string(),
                clone_url_ssh: String::new(),
                clone_url_grc: "codecommit::us-east-1://mock-repo".to_string(),
                default_branch: "main".to_string(),
                last_modified: String::new(),
                account_id: "123456789012".to_string(),
                tags: HashMap::new(),
            }),
        ),
        (
            ServiceType::Code,
            "CodeCommitPullRequest",
            Box::new(svc::code::CodeCommitPullRequest {
                pr_id: "42".to_string(),
                title: "mock change".to_string(),
                description: "a mock pull request".to_string(),
                repo: "mock-repo".to_string(),
                display_name: "mock-repo #42 mock change".to_string(),
                author: "mock-user".to_string(),
                author_arn: "arn:aws:iam::123456789012:user/mock-user".to_string(),
                status: "OPEN".to_string(),
                is_merged: false,
                merged_by: String::new(),
                merge_commit_id: String::new(),
                merge_option: String::new(),
                source_ref: "feature/x".to_string(),
                dest_ref: "main".to_string(),
                source_commit: "1111111111111111".to_string(),
                dest_commit: "2222222222222222".to_string(),
                merge_base: "3333333333333333".to_string(),
                created: "2026-08-27T00:00:00Z".to_string(),
                created_ms: 1,
                last_activity: "2026-08-27T00:00:00Z".to_string(),
                revision_id: "rev-1".to_string(),
                approval_rule_names: Vec::new(),
                tags: HashMap::new(),
            }),
        ),
        (
            ServiceType::Code,
            "CodeDeployGroup",
            Box::new(svc::code::CodeDeployGroup {
                label: "mock-app / mock-group".to_string(),
                app_name: "mock-app".to_string(),
                group_name: "mock-group".to_string(),
                group_id: "00000000-0000-0000-0000-000000000000".to_string(),
                deployment_config: "CodeDeployDefault.OneAtATime".to_string(),
                compute_platform: "Server".to_string(),
                service_role: "arn:aws:iam::123456789012:role/mock-codedeploy-role".to_string(),
                deployment_type: String::new(),
                deployment_option: String::new(),
                auto_rollback_enabled: false,
                auto_rollback_events: Vec::new(),
                ec2_tag_filters: Vec::new(),
                asg_names: Vec::new(),
                ecs_services: Vec::new(),
                last_attempted: None,
                last_successful: None,
                tags: HashMap::new(),
            }),
        ),
        (
            ServiceType::Code,
            "CodePipeline",
            Box::new(svc::code::CodePipeline {
                name: "mock-pipeline".to_string(),
                version: 1,
                pipeline_type: "V2".to_string(),
                created: String::new(),
                updated: String::new(),
                tags: HashMap::new(),
            }),
        ),
        (
            ServiceType::Code,
            "CodePipelineExecution",
            Box::new(svc::code::CodePipelineExecution::from_summary(
                &aws_sdk_codepipeline::types::PipelineExecutionSummary::builder()
                    .pipeline_execution_id("11111111-2222-3333-4444-555555555555")
                    .status(aws_sdk_codepipeline::types::PipelineExecutionStatus::InProgress)
                    .start_time(aws_smithy_types::DateTime::from_secs(0))
                    .build(),
                "mock-pipeline",
            )),
        ),
    ]
}
