//! Mock resources for the layer-2 harness — see mod.rs.
use super::Mock;
use crate::aws::service::ServiceType;
use crate::aws::services as svc;
use std::collections::HashMap;

pub(super) fn mocks() -> Vec<Mock> {
    vec![
        (
            ServiceType::CloudFront,
            "CfDistribution",
            Box::new(svc::cloudfront::CfDistribution {
                id: "E1MOCK0DISTRIB".to_string(),
                arn: "arn:aws:cloudfront::123456789012:distribution/E1MOCK0DISTRIB".to_string(),
                domain_name: "d1234567890abc.cloudfront.net".to_string(),
                status: "Deployed".to_string(),
                enabled: true,
                aliases: vec!["www.example.com".to_string()],
                comment: "mock distribution".to_string(),
                price_class: "PriceClass_All".to_string(),
                http_version: "http2".to_string(),
                is_ipv6_enabled: true,
                web_acl_id: String::new(),
                last_modified: None,
                origins: vec![],
                origin_groups: vec![],
                behaviors: vec![],
                error_responses: vec![],
                geo_restriction: svc::cloudfront::CfGeoRestriction::default(),
                viewer_cert: "default".to_string(),
                min_tls: "TLSv1.2_2021".to_string(),
                ssl_method: "sni-only".to_string(),
                staging: false,
                anycast_ip_list_id: String::new(),
                tags: HashMap::new(),
            }),
        ),
        (
            ServiceType::CloudFront,
            "CfFunction",
            Box::new(svc::cloudfront::CfFunction {
                name: "mock-function".to_string(),
                arn: "arn:aws:cloudfront::123456789012:function/mock-function".to_string(),
                status: "DEPLOYED".to_string(),
                runtime: "cloudfront-js-2.0".to_string(),
                comment: "mock function".to_string(),
                published: true,
                created: "2024-01-01T00:00:00Z".to_string(),
                last_modified: "2024-01-01T00:00:00Z".to_string(),
                kv_stores: vec![],
                tags: HashMap::new(),
            }),
        ),
        (
            ServiceType::Route53,
            "R53HostedZone",
            Box::new(svc::route53::R53HostedZone::from_sdk(
                &aws_sdk_route53::types::HostedZone::builder()
                    .id("/hostedzone/Z0123456789ABCDEFGHIJ")
                    .name("example.com.")
                    .caller_reference("mock-caller-ref")
                    .build()
                    .unwrap(),
            )),
        ),
        (
            ServiceType::Route53,
            "R53Record",
            Box::new(svc::route53::R53Record::from_sdk(
                &aws_sdk_route53::types::ResourceRecordSet::builder()
                    .name("api.example.com.")
                    .r#type(aws_sdk_route53::types::RrType::A)
                    .ttl(60)
                    .resource_records(
                        aws_sdk_route53::types::ResourceRecord::builder()
                            .value("10.0.0.1")
                            .build()
                            .unwrap(),
                    )
                    .build()
                    .unwrap(),
                "/hostedzone/Z0123456789ABCDEFGHIJ",
                "example.com.",
            )),
        ),
        (
            ServiceType::Route53,
            "R53HealthCheck",
            Box::new(svc::route53::R53HealthCheck::from_sdk(
                &aws_sdk_route53::types::HealthCheck::builder()
                    .id("mock-hc-id-0001")
                    .caller_reference("mock-caller-ref")
                    .health_check_config(
                        aws_sdk_route53::types::HealthCheckConfig::builder()
                            .r#type(aws_sdk_route53::types::HealthCheckType::Https)
                            .fully_qualified_domain_name("example.com")
                            .build()
                            .unwrap(),
                    )
                    .health_check_version(1)
                    .build()
                    .unwrap(),
            )),
        ),
        (
            ServiceType::Route53Resolver,
            "ResolverEndpoint",
            Box::new(svc::route53resolver::ResolverEndpoint::from_sdk(
                &aws_sdk_route53resolver::types::ResolverEndpoint::builder()
                    .id("rslvr-in-0123456789abcdef0")
                    .name("mock-endpoint")
                    .direction(aws_sdk_route53resolver::types::ResolverEndpointDirection::Inbound)
                    .build(),
            )),
        ),
        (
            ServiceType::Route53Resolver,
            "ResolverRule",
            Box::new(svc::route53resolver::ResolverRule::from_sdk(
                &aws_sdk_route53resolver::types::ResolverRule::builder()
                    .id("rslvr-rr-0123456789abcdef0")
                    .name("mock-rule")
                    .domain_name("example.internal.")
                    .rule_type(aws_sdk_route53resolver::types::RuleTypeOption::Forward)
                    .build(),
            )),
        ),
        // Delegation (2025-06): a third endpoint direction and a fourth rule
        // type. The DELEGATE rule has an endpoint but no targets — the arm
        // that once rendered it as a SYSTEM rule.
        (
            ServiceType::Route53Resolver,
            "ResolverEndpoint(InboundDelegation)",
            Box::new(svc::route53resolver::ResolverEndpoint::from_sdk(
                &aws_sdk_route53resolver::types::ResolverEndpoint::builder()
                    .id("rslvr-in-0fedcba9876543210")
                    .name("mock-delegation-endpoint")
                    .direction(aws_sdk_route53resolver::types::ResolverEndpointDirection::InboundDelegation)
                    .resolver_endpoint_type(aws_sdk_route53resolver::types::ResolverEndpointType::Dualstack)
                    .protocols(aws_sdk_route53resolver::types::Protocol::Do53)
                    .build(),
            )),
        ),
        (
            ServiceType::Route53Resolver,
            "ResolverRule(Delegate)",
            Box::new(svc::route53resolver::ResolverRule::from_sdk(
                &aws_sdk_route53resolver::types::ResolverRule::builder()
                    .id("rslvr-rr-0fedcba9876543210")
                    .name("mock-delegate-rule")
                    .domain_name("sub.example.internal.")
                    .rule_type(aws_sdk_route53resolver::types::RuleTypeOption::Delegate)
                    .resolver_endpoint_id("rslvr-out-0123456789abcdef0")
                    .delegation_record("ns1.example.internal")
                    .build(),
            )),
        ),
        (
            ServiceType::Route53Profiles,
            "Route53Profile",
            Box::new(svc::route53profiles::Route53Profile::from_sdk(
                &aws_sdk_route53profiles::types::ProfileSummary::builder()
                    .id("rp-0123456789abcdef0")
                    .arn("arn:aws:route53profiles:us-east-1:123456789012:profile/rp-0123456789abcdef0")
                    .name("mock-profile")
                    .share_status(aws_sdk_route53profiles::types::ShareStatus::NotShared)
                    .build(),
            )),
        ),
        (
            ServiceType::ApiGateway,
            "RestApi",
            Box::new(svc::api_gateway::RestApi::from_sdk(
                &aws_sdk_apigateway::types::RestApi::builder()
                    .id("mockrestapi")
                    .name("mock-rest-api")
                    .build(),
            )),
        ),
        (
            ServiceType::ApiGateway,
            "HttpApi",
            Box::new(svc::api_gateway::HttpApi::from_sdk(
                &aws_sdk_apigatewayv2::types::Api::builder()
                    .api_id("mockhttpapi")
                    .name("mock-http-api")
                    .protocol_type(aws_sdk_apigatewayv2::types::ProtocolType::Http)
                    .build(),
            )),
        ),
        (
            ServiceType::ApiGateway,
            "ApiCustomDomain",
            Box::new(svc::api_gateway::ApiCustomDomain::from_v1(
                &aws_sdk_apigateway::types::DomainName::builder()
                    .domain_name("api.example.com")
                    .build(),
            )),
        ),
        (
            ServiceType::ApiGateway,
            "ApiUsagePlan",
            Box::new(svc::api_gateway::ApiUsagePlan::from_sdk(
                &aws_sdk_apigateway::types::UsagePlan::builder()
                    .id("mockusageplan")
                    .name("mock-usage-plan")
                    .build(),
            )),
        ),
        (
            ServiceType::Elb,
            "LoadBalancer",
            Box::new(svc::elb::LoadBalancer::from_sdk(
                &aws_sdk_elasticloadbalancingv2::types::LoadBalancer::builder()
                    .load_balancer_arn(
                        "arn:aws:elasticloadbalancing:us-east-1:123456789012:loadbalancer/app/mock-lb/0123456789abcdef",
                    )
                    .load_balancer_name("mock-lb")
                    .build(),
            )),
        ),
        (
            ServiceType::Elb,
            "TargetGroup",
            Box::new(svc::elb::TargetGroup::from_sdk(
                &aws_sdk_elasticloadbalancingv2::types::TargetGroup::builder()
                    .target_group_arn(
                        "arn:aws:elasticloadbalancing:us-east-1:123456789012:targetgroup/mock-tg/0123456789abcdef",
                    )
                    .target_group_name("mock-tg")
                    .build(),
            )),
        ),
        (
            ServiceType::GlobalAccelerator,
            "GaAccelerator",
            Box::new(svc::global_accelerator::GaAccelerator::from_sdk(
                &aws_sdk_globalaccelerator::types::Accelerator::builder()
                    .accelerator_arn(
                        "arn:aws:globalaccelerator::123456789012:accelerator/0123456789abcdef",
                    )
                    .name("mock-accelerator")
                    .build(),
            )),
        ),
        (
            ServiceType::EventBridge,
            "EbRule",
            Box::new(svc::eventbridge::EbRule::from_sdk(
                &aws_sdk_eventbridge::types::Rule::builder()
                    .name("mock-rule")
                    .arn("arn:aws:events:us-east-1:123456789012:rule/mock-rule")
                    .build(),
                "default",
            )),
        ),
        (
            ServiceType::EventBridge,
            "EbEventBus",
            Box::new(svc::eventbridge::EbEventBus::from_sdk(
                &aws_sdk_eventbridge::types::EventBus::builder()
                    .name("mock-bus")
                    .arn("arn:aws:events:us-east-1:123456789012:event-bus/mock-bus")
                    .build(),
            )),
        ),
        (
            ServiceType::EventBridge,
            "EbSchedule",
            Box::new(svc::eventbridge::EbSchedule::from_sdk(
                &aws_sdk_scheduler::types::ScheduleSummary::builder()
                    .name("mock-schedule")
                    .arn("arn:aws:scheduler:us-east-1:123456789012:schedule/default/mock-schedule")
                    .group_name("default")
                    .build(),
            )),
        ),
        (
            ServiceType::EventBridge,
            "EbPipe",
            Box::new(svc::eventbridge::EbPipe::from_sdk(
                &aws_sdk_pipes::types::Pipe::builder()
                    .name("mock-pipe")
                    .arn("arn:aws:pipes:us-east-1:123456789012:pipe/mock-pipe")
                    .build(),
            )),
        ),
        (
            ServiceType::Ses,
            "SesIdentity",
            Box::new(svc::ses::SesIdentity::from_sdk(
                "mock@example.com",
                &aws_sdk_sesv2::types::IdentityInfo::builder()
                    .identity_type(aws_sdk_sesv2::types::IdentityType::EmailAddress)
                    .identity_name("mock@example.com")
                    .sending_enabled(true)
                    .build(),
                None,
            )),
        ),
        (
            ServiceType::Ses,
            "SesConfigSet",
            Box::new(svc::ses::SesConfigSet::from_sdk("mock-config-set", None)),
        ),
        (
            ServiceType::Messaging,
            "SqsQueue",
            Box::new(svc::messaging::SqsQueue {
                url: "https://sqs.us-east-1.amazonaws.com/123456789012/mock-queue".to_string(),
                name: "mock-queue".to_string(),
                arn: "arn:aws:sqs:us-east-1:123456789012:mock-queue".to_string(),
                is_fifo: false,
                messages_available: 0,
                messages_in_flight: 0,
                messages_delayed: 0,
                visibility_timeout: Some(30),
                message_retention_secs: Some(345600),
                max_message_size: Some(262144),
                delay_seconds: Some(0),
                receive_wait_secs: Some(0),
                content_based_dedup: None,
                kms_key_id: None,
                sse_sqs: false,
                policy: None,
                created: "2024-01-01T00:00:00Z".to_string(),
                last_modified: "2024-01-01T00:00:00Z".to_string(),
                dlq_target_arn: None,
                max_receive_count: None,
                redrive_permission: None,
                source_queue_arns: vec![],
                tags: HashMap::new(),
            }),
        ),
        (
            ServiceType::Messaging,
            "SnsTopic",
            Box::new(svc::messaging::SnsTopic {
                arn: "arn:aws:sns:us-east-1:123456789012:mock-topic".to_string(),
                name: "mock-topic".to_string(),
                is_fifo: false,
                display_name: None,
                subscriptions_confirmed: 0,
                subscriptions_pending: 0,
                subscriptions_deleted: 0,
                owner: None,
                kms_key_id: None,
                policy: None,
                effective_delivery_policy: None,
                tags: HashMap::new(),
            }),
        ),
    ]
}
