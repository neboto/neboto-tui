//! Mock resources for the layer-2 harness — see mod.rs.
use super::Mock;
use crate::aws::service::ServiceType;
use crate::aws::services as svc;
use std::collections::HashMap;

pub(super) fn mocks() -> Vec<Mock> {
    vec![
        (
            ServiceType::IAM,
            "IamRole",
            Box::new(svc::iam::IamRole::from_sdk(
                &aws_sdk_iam::types::Role::builder()
                    .path("/")
                    .role_name("mock-role")
                    .role_id("AROAMOCKROLEID123")
                    .arn("arn:aws:iam::123456789012:role/mock-role")
                    .create_date(aws_smithy_types::DateTime::from_secs(0))
                    .build()
                    .unwrap(),
            )),
        ),
        (
            ServiceType::IAM,
            "IamPolicy",
            Box::new(svc::iam::IamPolicy::from_sdk(
                &aws_sdk_iam::types::Policy::builder()
                    .arn("arn:aws:iam::123456789012:policy/mock-policy")
                    .policy_name("mock-policy")
                    .build(),
            )),
        ),
        (
            ServiceType::IAM,
            "IamUser",
            Box::new(svc::iam::IamUser::from_sdk(
                &aws_sdk_iam::types::User::builder()
                    .path("/")
                    .user_name("mock-user")
                    .user_id("AIDAMOCKUSERID123")
                    .arn("arn:aws:iam::123456789012:user/mock-user")
                    .create_date(aws_smithy_types::DateTime::from_secs(0))
                    .build()
                    .unwrap(),
            )),
        ),
        (
            ServiceType::IAM,
            "IamGroup",
            Box::new(svc::iam::IamGroup::from_sdk(
                &aws_sdk_iam::types::Group::builder()
                    .path("/")
                    .group_name("mock-group")
                    .group_id("AGPAMOCKGROUPID123")
                    .arn("arn:aws:iam::123456789012:group/mock-group")
                    .create_date(aws_smithy_types::DateTime::from_secs(0))
                    .build()
                    .unwrap(),
            )),
        ),
        (
            ServiceType::IAM,
            "AccessAnalyzerFinding",
            Box::new(svc::iam::AccessAnalyzerFinding::from_sdk(
                &aws_sdk_accessanalyzer::types::FindingSummary::builder()
                    .id("mock-finding-id")
                    .resource_type(aws_sdk_accessanalyzer::types::ResourceType::AwsS3Bucket)
                    .set_condition(Some(HashMap::new()))
                    .created_at(aws_smithy_types::DateTime::from_secs(0))
                    .analyzed_at(aws_smithy_types::DateTime::from_secs(0))
                    .updated_at(aws_smithy_types::DateTime::from_secs(0))
                    .status(aws_sdk_accessanalyzer::types::FindingStatus::Active)
                    .resource_owner_account("123456789012")
                    .build()
                    .unwrap(),
                "arn:aws:access-analyzer:us-east-1:123456789012:analyzer/mock-analyzer",
            )),
        ),
        (
            ServiceType::IAM,
            "IamIdentityProvider",
            Box::new(svc::iam::IamIdentityProvider::from_saml(
                &aws_sdk_iam::types::SamlProviderListEntry::builder()
                    .arn("arn:aws:iam::123456789012:saml-provider/mock-idp")
                    .build(),
            )),
        ),
        (
            ServiceType::IAM,
            "IamAccountSettings",
            Box::new(svc::iam::IamAccountSettings {
                display_name: "mock-alias".to_string(),
                aliases: vec!["mock-alias".to_string()],
                summary: vec![
                    ("AccountMFAEnabled".to_string(), 1),
                    ("Users".to_string(), 1),
                    ("UsersQuota".to_string(), 5000),
                ],
                summary_error: None,
                password_policy: None,
                pw_policy_error: None,
                tags: HashMap::new(),
            }),
        ),
        (
            ServiceType::Organizations,
            "OrgAccount",
            Box::new(svc::organizations::OrgAccount::from_sdk(
                &aws_sdk_organizations::types::Account::builder()
                    .id("123456789012")
                    .arn("arn:aws:organizations::123456789012:account/o-mock/123456789012")
                    .name("mock-account")
                    .email("mock@example.com")
                    .build(),
            )),
        ),
        (
            ServiceType::Organizations,
            "OrgUnit",
            Box::new(svc::organizations::OrgUnit::new(
                "ou-mock-0123abcd".to_string(),
                "arn:aws:organizations::123456789012:ou/o-mock/ou-mock-0123abcd".to_string(),
                "mock-ou".to_string(),
                "Root".to_string(),
                "r-mock".to_string(),
            )),
        ),
        (
            ServiceType::Organizations,
            "OrgScp",
            Box::new(svc::organizations::OrgScp::from_sdk(
                &aws_sdk_organizations::types::PolicySummary::builder()
                    .id("p-mockscp")
                    .arn("arn:aws:organizations::123456789012:policy/o-mock/service_control_policy/p-mockscp")
                    .name("mock-scp")
                    .build(),
            )),
        ),
        (
            ServiceType::IdentityCenter,
            "IcInstance",
            Box::new(svc::identity_center::IcInstance::from_sdk(
                &aws_sdk_ssoadmin::types::InstanceMetadata::builder()
                    .instance_arn("arn:aws:sso:::instance/ssoins-mock0123456789")
                    .identity_store_id("d-mockstore")
                    .name("mock-instance")
                    .build(),
            )),
        ),
        (
            ServiceType::IdentityCenter,
            "PermissionSet",
            Box::new(svc::identity_center::PermissionSet::from_sdk(
                &aws_sdk_ssoadmin::types::PermissionSet::builder()
                    .permission_set_arn(
                        "arn:aws:sso:::permissionSet/ssoins-mock0123456789/ps-mock0123456789",
                    )
                    .name("mock-permission-set")
                    .build(),
                "arn:aws:sso:::instance/ssoins-mock0123456789",
                "d-mockstore",
            )),
        ),
        (
            ServiceType::IdentityCenter,
            "IcUser",
            Box::new(svc::identity_center::IcUser::from_sdk(
                &aws_sdk_identitystore::types::User::builder()
                    .identity_store_id("d-mockstore")
                    .user_id("mock-user-id")
                    .user_name("mock-user")
                    .display_name("Mock User")
                    .build()
                    .unwrap(),
                "arn:aws:sso:::instance/ssoins-mock0123456789",
            )),
        ),
        (
            ServiceType::IdentityCenter,
            "IcGroup",
            Box::new(svc::identity_center::IcGroup::from_sdk(
                &aws_sdk_identitystore::types::Group::builder()
                    .group_id("mock-group-id")
                    .identity_store_id("d-mockstore")
                    .display_name("mock-group")
                    .build()
                    .unwrap(),
                "arn:aws:sso:::instance/ssoins-mock0123456789",
            )),
        ),
        (
            ServiceType::IdentityCenter,
            "IcApplication",
            Box::new(svc::identity_center::IcApplication::from_sdk(
                &aws_sdk_ssoadmin::types::Application::builder()
                    .application_arn(
                        "arn:aws:sso::123456789012:application/ssoins-mock0123456789/apl-mock",
                    )
                    .name("mock-app")
                    .build(),
                "d-mockstore",
            )),
        ),
        (
            ServiceType::Cognito,
            "CognitoUserPool",
            Box::new(svc::cognito::CognitoUserPool {
                id: "us-east-1_mockpool".to_string(),
                name: "mock-pool".to_string(),
                arn: "arn:aws:cognito-idp:us-east-1:123456789012:userpool/us-east-1_mockpool"
                    .to_string(),
                mfa_config: "OFF".to_string(),
                estimated_users: 0,
                username_attributes: Vec::new(),
                domain: None,
                created: None,
                policies_summary: "—".to_string(),
                lambda_triggers: Vec::new(),
                tags: HashMap::new(),
            }),
        ),
        (
            ServiceType::Ram,
            "RamResourceShare",
            Box::new(svc::ram::RamResourceShare::from_sdk(
                &aws_sdk_ram::types::ResourceShare::builder()
                    .resource_share_arn(
                        "arn:aws:ram:us-east-1:123456789012:resource-share/mock-share",
                    )
                    .name("mock-share")
                    .build(),
                "SELF",
            )),
        ),
        (
            ServiceType::Workspaces,
            "Workspace",
            Box::new(svc::workspaces::Workspace::from_sdk(
                &aws_sdk_workspaces::types::Workspace::builder()
                    .workspace_id("ws-mock01234")
                    .build(),
                &HashMap::new(),
                &HashMap::new(),
            )),
        ),
    ]
}
