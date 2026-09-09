//! Mock resources for the ML & AI services — see mod.rs.
//!
//! Bedrock AgentCore's nine split panes plus its flat types. Every split
//! pane needs an entry here or the section-order and split-renderer tests
//! never see it.
use super::Mock;
use crate::aws::service::ServiceType;
use crate::aws::services as svc;

fn dt() -> aws_smithy_types::DateTime {
    aws_smithy_types::DateTime::from_secs(0)
}

pub(super) fn mocks() -> Vec<Mock> {
    use aws_sdk_bedrockagentcorecontrol::types as ac;

    vec![
        (
            ServiceType::AgentCore,
            "AgentCoreRuntime",
            Box::new(svc::agentcore::AgentCoreRuntime::from_sdk(
                &ac::AgentRuntime::builder()
                    .agent_runtime_id("mock-runtime")
                    .agent_runtime_arn(
                        "arn:aws:bedrock-agentcore:us-east-1:123456789012:runtime/mock-runtime",
                    )
                    .agent_runtime_name("mock-runtime")
                    .agent_runtime_version("1")
                    .description("mock agent runtime")
                    .status(ac::AgentRuntimeStatus::Ready)
                    .last_updated_at(dt())
                    .build()
                    .expect("mock agent runtime builds"),
            )),
        ),
        (
            ServiceType::AgentCore,
            "AgentCoreGateway",
            Box::new(svc::agentcore::AgentCoreGateway::from_sdk(
                &ac::GatewaySummary::builder()
                    .gateway_id("mock-gateway")
                    .name("mock-gateway")
                    .status(ac::GatewayStatus::Ready)
                    .protocol_type(ac::GatewayProtocolType::Mcp)
                    .authorizer_type(ac::AuthorizerType::CustomJwt)
                    .created_at(dt())
                    .updated_at(dt())
                    .build()
                    .expect("mock gateway builds"),
            )),
        ),
        (
            ServiceType::AgentCore,
            "AgentCoreMemory",
            Box::new(svc::agentcore::AgentCoreMemory::from_sdk(
                &ac::MemorySummary::builder()
                    .id("mock-memory")
                    .arn("arn:aws:bedrock-agentcore:us-east-1:123456789012:memory/mock-memory")
                    .status(ac::MemoryStatus::Active)
                    .created_at(dt())
                    .updated_at(dt())
                    .build()
                    .expect("mock memory builds"),
            )),
        ),
        (
            ServiceType::AgentCore,
            "AgentCoreBrowser",
            Box::new(svc::agentcore::AgentCoreBrowser::from_sdk(
                &ac::BrowserSummary::builder()
                    .browser_id("mock-browser")
                    .browser_arn(
                        "arn:aws:bedrock-agentcore:us-east-1:123456789012:browser/mock-browser",
                    )
                    .name("mock-browser")
                    .status(ac::BrowserStatus::Ready)
                    .created_at(dt())
                    .build()
                    .expect("mock browser builds"),
            )),
        ),
        (
            ServiceType::AgentCore,
            "AgentCoreCodeInterpreter",
            Box::new(svc::agentcore::AgentCoreCodeInterpreter::from_sdk(
                &ac::CodeInterpreterSummary::builder()
                    .code_interpreter_id("mock-interpreter")
                    .code_interpreter_arn(
                        "arn:aws:bedrock-agentcore:us-east-1:123456789012:code-interpreter/mock-interpreter",
                    )
                    .name("mock-interpreter")
                    .status(ac::CodeInterpreterStatus::Ready)
                    .created_at(dt())
                    .build()
                    .expect("mock code interpreter builds"),
            )),
        ),
        (
            ServiceType::AgentCore,
            "AgentCoreBrowserProfile",
            Box::new(svc::agentcore::AgentCoreBrowserProfile::from_sdk(
                &ac::BrowserProfileSummary::builder()
                    .profile_id("mock-profile")
                    .profile_arn(
                        "arn:aws:bedrock-agentcore:us-east-1:123456789012:browser-profile/mock-profile",
                    )
                    .name("mock-profile")
                    .status(ac::BrowserProfileStatus::Ready)
                    .created_at(dt())
                    .last_updated_at(dt())
                    .build()
                    .expect("mock browser profile builds"),
            )),
        ),
        (
            ServiceType::AgentCore,
            "AgentCoreWorkloadIdentity",
            Box::new(svc::agentcore::AgentCoreWorkloadIdentity::from_sdk(
                &ac::WorkloadIdentityType::builder()
                    .name("mock-identity")
                    .workload_identity_arn(
                        "arn:aws:bedrock-agentcore:us-east-1:123456789012:workload-identity-directory/default/workload-identity/mock-identity",
                    )
                    .build()
                    .expect("mock workload identity builds"),
            )),
        ),
        (
            ServiceType::AgentCore,
            "AgentCoreOAuth2Provider",
            Box::new(svc::agentcore::AgentCoreOAuth2Provider::from_sdk(
                &ac::Oauth2CredentialProviderItem::builder()
                    .name("mock-oauth2")
                    .credential_provider_vendor(ac::CredentialProviderVendorType::GithubOauth2)
                    .credential_provider_arn(
                        "arn:aws:bedrock-agentcore:us-east-1:123456789012:token-vault/default/oauth2credentialprovider/mock-oauth2",
                    )
                    .created_time(dt())
                    .last_updated_time(dt())
                    .build()
                    .expect("mock oauth2 provider builds"),
            )),
        ),
        (
            ServiceType::AgentCore,
            "AgentCoreApiKeyProvider",
            Box::new(svc::agentcore::AgentCoreApiKeyProvider::from_sdk(
                &ac::ApiKeyCredentialProviderItem::builder()
                    .name("mock-apikey")
                    .credential_provider_arn(
                        "arn:aws:bedrock-agentcore:us-east-1:123456789012:token-vault/default/apikeycredentialprovider/mock-apikey",
                    )
                    .created_time(dt())
                    .last_updated_time(dt())
                    .build()
                    .expect("mock api key provider builds"),
            )),
        ),
        // ── Preview families (tabs 6-8) ─────────────────────────────────
        (
            ServiceType::AgentCore,
            "AgentCorePolicyEngine",
            Box::new(svc::agentcore::AgentCorePolicyEngine::from_sdk(
                &ac::PolicyEngine::builder()
                    .policy_engine_id("mock-engine")
                    .policy_engine_arn(
                        "arn:aws:bedrock-agentcore:us-east-1:123456789012:policy-engine/mock-engine",
                    )
                    .name("mock-engine")
                    .status(ac::PolicyEngineStatus::Active)
                    .set_status_reasons(Some(Vec::new()))
                    .created_at(dt())
                    .updated_at(dt())
                    .build()
                    .expect("mock policy engine builds"),
            )),
        ),
        (
            ServiceType::AgentCore,
            "AgentCorePolicy",
            Box::new(svc::agentcore::AgentCorePolicy::from_sdk(
                &ac::Policy::builder()
                    .policy_id("mock-policy")
                    .policy_arn(
                        "arn:aws:bedrock-agentcore:us-east-1:123456789012:policy/mock-policy",
                    )
                    .name("mock-policy")
                    .policy_engine_id("mock-engine")
                    .status(ac::PolicyStatus::Active)
                    .set_status_reasons(Some(Vec::new()))
                    .enforcement_mode(ac::EnforcementMode::LogOnly)
                    .definition(ac::PolicyDefinition::Cedar(
                        ac::CedarPolicy::builder()
                            .statement("permit(principal, action, resource);")
                            .build()
                            .expect("mock cedar policy builds"),
                    ))
                    .created_at(dt())
                    .updated_at(dt())
                    .build()
                    .expect("mock policy builds"),
            )),
        ),
        (
            ServiceType::AgentCore,
            "AgentCoreEvaluator",
            Box::new(svc::agentcore::AgentCoreEvaluator::from_sdk(
                &ac::EvaluatorSummary::builder()
                    .evaluator_id("mock-evaluator")
                    .evaluator_arn(
                        "arn:aws:bedrock-agentcore:us-east-1:123456789012:evaluator/mock-evaluator",
                    )
                    .evaluator_name("mock-evaluator")
                    .evaluator_type(ac::EvaluatorType::Builtin)
                    .status(ac::EvaluatorStatus::Active)
                    .created_at(dt())
                    .updated_at(dt())
                    .build()
                    .expect("mock evaluator builds"),
            )),
        ),
        (
            ServiceType::AgentCore,
            "AgentCoreOnlineEval",
            Box::new(svc::agentcore::AgentCoreOnlineEval::from_sdk(
                &ac::OnlineEvaluationConfigSummary::builder()
                    .online_evaluation_config_id("mock-online-eval")
                    .online_evaluation_config_arn(
                        "arn:aws:bedrock-agentcore:us-east-1:123456789012:online-evaluation-config/mock-online-eval",
                    )
                    .online_evaluation_config_name("mock-online-eval")
                    .status(ac::OnlineEvaluationConfigStatus::Active)
                    .execution_status(ac::OnlineEvaluationExecutionStatus::Enabled)
                    .created_at(dt())
                    .updated_at(dt())
                    .build()
                    .expect("mock online evaluation builds"),
            )),
        ),
        (
            ServiceType::AgentCore,
            "AgentCoreDataset",
            Box::new(svc::agentcore::AgentCoreDataset::from_sdk(
                &ac::DatasetSummary::builder()
                    .dataset_id("mock-dataset")
                    .dataset_arn(
                        "arn:aws:bedrock-agentcore:us-east-1:123456789012:dataset/mock-dataset",
                    )
                    .dataset_name("mock-dataset")
                    .status(ac::DatasetStatus::Active)
                    .schema_type(ac::DatasetSchemaType::AgentcoreEvaluationPredefinedV1)
                    .example_count(0)
                    .created_at(dt())
                    .updated_at(dt())
                    .build()
                    .expect("mock dataset builds"),
            )),
        ),
        (
            ServiceType::AgentCore,
            "AgentCoreRegistry",
            Box::new(svc::agentcore::AgentCoreRegistry::from_sdk(
                &ac::RegistrySummary::builder()
                    .registry_id("mock-registry")
                    .registry_arn(
                        "arn:aws:bedrock-agentcore:us-east-1:123456789012:registry/mock-registry",
                    )
                    .name("mock-registry")
                    .status(ac::RegistryStatus::Ready)
                    .created_at(dt())
                    .updated_at(dt())
                    .build()
                    .expect("mock registry builds"),
            )),
        ),
        (
            ServiceType::AgentCore,
            "AgentCoreHarness",
            Box::new(svc::agentcore::AgentCoreHarness::from_sdk(
                &ac::HarnessSummary::builder()
                    .harness_id("mock-harness")
                    .harness_name("mock-harness")
                    .arn("arn:aws:bedrock-agentcore:us-east-1:123456789012:harness/mock-harness")
                    .status(ac::HarnessStatus::Ready)
                    .harness_version("1")
                    .created_at(dt())
                    .updated_at(dt())
                    .build()
                    .expect("mock harness builds"),
            )),
        ),
        (
            ServiceType::AgentCore,
            "AgentCoreConfigBundle",
            Box::new(svc::agentcore::AgentCoreConfigBundle::from_sdk(
                &ac::ConfigurationBundleSummary::builder()
                    .bundle_id("mock-bundle")
                    .bundle_name("mock-bundle")
                    .bundle_arn("arn:aws:bedrock-agentcore:us-east-1:123456789012:bundle/mock-bundle")
                    .description("mock configuration bundle")
                    .created_at(dt())
                    .build()
                    .expect("mock configuration bundle builds"),
            )),
        ),
        (
            ServiceType::AgentCore,
            "AgentCorePaymentManager",
            Box::new(svc::agentcore::AgentCorePaymentManager::from_sdk(
                &ac::PaymentManagerSummary::builder()
                    .payment_manager_id("mock-payment-manager")
                    .payment_manager_arn(
                        "arn:aws:bedrock-agentcore:us-east-1:123456789012:payment-manager/mock",
                    )
                    .name("mock-payment-manager")
                    .authorizer_type(ac::PaymentsAuthorizerType::AwsIam)
                    .role_arn("arn:aws:iam::123456789012:role/mock-payments")
                    .status(ac::PaymentManagerStatus::Ready)
                    .created_at(dt())
                    .last_updated_at(dt())
                    .build()
                    .expect("mock payment manager builds"),
            )),
        ),
        (
            ServiceType::AgentCore,
            "AgentCorePaymentCredProvider",
            Box::new(svc::agentcore::AgentCorePaymentCredProvider::from_sdk(
                &ac::PaymentCredentialProviderItem::builder()
                    .name("mock-payment-credentials")
                    .credential_provider_vendor(ac::PaymentCredentialProviderVendorType::StripePrivy)
                    .credential_provider_arn(
                        "arn:aws:bedrock-agentcore:us-east-1:123456789012:payment-credential-provider/mock",
                    )
                    .created_time(dt())
                    .last_updated_time(dt())
                    .build()
                    .expect("mock payment credential provider builds"),
            )),
        ),
    ]
}
