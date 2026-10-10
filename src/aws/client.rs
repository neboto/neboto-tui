use crate::aws::region::Region;
use crate::error::Result;
use aws_config::SdkConfig;

/// A cross-account role assumed from the Organizations accounts list (the
/// management-account → member-account `OrganizationAccountAccessRole` hop).
/// Sessions are always scoped down with the AWS-managed `ReadOnlyAccess`
/// policy, so the temporary credentials cannot mutate anything even though
/// the underlying role is typically AdministratorAccess.
#[derive(Debug, Clone)]
pub struct AssumedOrgRole {
    pub account_id: String,
    pub account_name: String,
    pub role_name: String,
}

/// Session policy pinned onto every assumed-role session — makes the
/// credentials cryptographically read-only regardless of what the member
/// role itself allows.
const READONLY_POLICY_ARN: &str = "arn:aws:iam::aws:policy/ReadOnlyAccess";

/// Inline session policy layered *alongside* `READONLY_POLICY_ARN` for read
/// APIs the AWS-managed `ReadOnlyAccess` policy simply doesn't cover — an
/// assumed session would otherwise fail them with "because no session policy
/// allows …" even though the underlying role is admin (Control Tower's
/// `List*`/`Get*` surfaced this; `support:Describe*` keeps the Trusted
/// Advisor org fan-out working through member-account sessions).
///
/// Multiple session policies are combined into one effective session policy
/// (an Allow in *any* of them counts) which is then intersected with the
/// role's identity policy — so this widens the session only by the read verbs
/// listed here and cannot make it writable.
const READONLY_SUPPLEMENT_POLICY: &str = r#"{
  "Version": "2012-10-17",
  "Statement": [
    {
      "Effect": "Allow",
      "Action": [
        "controltower:List*",
        "controltower:Get*",
        "controlcatalog:List*",
        "controlcatalog:Get*",
        "support:Describe*"
      ],
      "Resource": "*"
    }
  ]
}"#;

pub struct AwsClients {
    config: SdkConfig,
    region: Region,
    /// Active named profile (None = default credential chain). Persisted so a
    /// region switch keeps the same profile.
    profile: Option<String>,
    /// Custom endpoint URL (e.g. a local floci/LocalStack emulator at
    /// `http://localhost:4566`). When set, every client targets it, S3 uses
    /// path-style addressing, and dummy credentials are injected. Persisted so
    /// region/profile switches keep pointing at the mock. See `resolve_endpoint`.
    endpoint_url: Option<String>,
    /// Cross-account role assumed on top of the base profile credentials
    /// (None = browsing the base account). Persisted so a region switch stays
    /// in the assumed account; a profile switch drops it (an explicit
    /// credential change supersedes the hop).
    assumed_role: Option<AssumedOrgRole>,
    /// Set when the credentials come from a `credential_process` that may
    /// prompt in the terminal (#151); `prime_prompting_credentials` resolves
    /// those before the TUI starts.
    prompting_credentials: bool,
}

impl AwsClients {
    /// Core constructor: build an `SdkConfig` for the given profile/region and
    /// optional custom endpoint. A `Some(endpoint)` (a local emulator) also
    /// injects dummy `test`/`test` credentials so it works with no real creds.
    async fn build(
        profile: Option<String>,
        region: Option<Region>,
        endpoint: Option<String>,
        assumed_role: Option<AssumedOrgRole>,
    ) -> Result<Self> {
        let mut loader = sdk_defaults();
        if let Some(r) = region {
            loader = loader.region(r.to_sdk_region());
        }
        if let Some(ref name) = profile {
            loader = loader.profile_name(name.clone());
        }
        // Demo mode (`--demo`): every request is answered from embedded
        // fixtures by the replay HTTP client — no network, no credentials.
        // A profile or role switch rebuilds through here and stays in demo.
        let demo = crate::demo::enabled();
        if demo {
            loader = loader
                .http_client(crate::demo::http_client())
                .credentials_provider(mock_credentials());
        } else if let Some(ref ep) = endpoint {
            loader = loader
                .endpoint_url(ep.clone())
                .credentials_provider(mock_credentials());
        }
        let mut config = loader.load().await;

        // An explicitly chosen profile must supply the credentials. The SDK's
        // default chain (what `loader.profile_name` feeds) tries the
        // `AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY` environment pair
        // *before* the named profile, so with keys exported in the shell a
        // `P` switch announced the new profile while every call kept signing
        // with the old account's keys — the top bar showed the new name over
        // the old account id. Pin the profile-file provider directly instead,
        // which is what `aws --profile` does: the explicit choice wins over
        // the environment. (The emulator branch keeps its mock creds.)
        if let (Some(name), None, false) = (profile.as_deref(), endpoint.as_deref(), demo) {
            config = with_profile_credentials(config, name);
        }

        // A `credential_process` can prompt on the tty (granted + pass → gpg
        // pinentry). Route it through the terminal handoff so the prompt gets
        // a clean screen instead of fighting the TUI for it (#151).
        let prompting_credentials = endpoint.is_none()
            && !demo
            && credential_process_profile(profile.as_deref()).is_some();
        if prompting_credentials {
            let name = credential_process_profile(profile.as_deref()).unwrap_or_default();
            if let Some(inner) = config.credentials_provider() {
                config = config
                    .into_builder()
                    .credentials_provider(
                        aws_credential_types::provider::SharedCredentialsProvider::new(
                            TerminalCredentialProcess::new(inner, name),
                        ),
                    )
                    .build();
            }
        }

        // Layer the assumed role on top of the base credentials. The provider
        // re-runs AssumeRole when the session expires (the SDK's identity
        // cache holds the temporary credentials until then), so a long-lived
        // browse session keeps working past the 1h default.
        if let (Some(role), false) = (&assumed_role, demo) {
            let role_arn = format!(
                "arn:aws:iam::{}:role/{}",
                role.account_id, role.role_name
            );
            let provider = aws_config::sts::AssumeRoleProvider::builder(role_arn)
                .session_name("neboto-readonly")
                .policy_arns(vec![READONLY_POLICY_ARN.to_string()])
                .policy(READONLY_SUPPLEMENT_POLICY)
                .configure(&config)
                .build()
                .await;
            config = config
                .into_builder()
                .credentials_provider(
                    aws_credential_types::provider::SharedCredentialsProvider::new(provider),
                )
                .build();
        }

        let region = region
            .or_else(|| config.region().and_then(Region::from_sdk_region))
            .unwrap_or(Region::UsEast1);

        Ok(Self {
            config,
            region,
            profile,
            endpoint_url: endpoint,
            assumed_role,
            prompting_credentials,
        })
    }

    /// Create new AWS clients with default region from AWS config and an optional
    /// explicit endpoint override (e.g. from the config file's `endpoint_url`).
    /// The `AWS_ENDPOINT_URL` env var still wins over the explicit value.
    pub async fn new_with_endpoint(endpoint: Option<String>) -> Result<Self> {
        Self::build(None, None, resolve_endpoint(endpoint), None).await
    }

    /// Test-only offline clients: pinned region + a dead localhost endpoint
    /// (which also injects the mock credentials), so construction never
    /// consults the environment and any stray fetch fails fast with
    /// connection-refused instead of dialing AWS.
    #[cfg(test)]
    pub(crate) async fn new_for_test() -> Self {
        Self::build(
            None,
            Some(Region::UsEast1),
            Some("http://127.0.0.1:1".to_string()),
            None,
        )
        .await
        .expect("offline client build")
    }

    /// Test-only demo clients: the replay HTTP client of `--demo`, without
    /// flipping the process-wide `demo::enable()` flag — that would move every
    /// other test's dead-endpoint clients onto fixture data too.
    #[cfg(test)]
    pub(crate) async fn new_demo_for_test() -> Self {
        let config = sdk_defaults()
            .region(Region::UsEast1.to_sdk_region())
            .http_client(crate::demo::http_client())
            .credentials_provider(mock_credentials())
            .load()
            .await;
        Self {
            config,
            region: Region::UsEast1,
            profile: None,
            endpoint_url: None,
            assumed_role: None,
            prompting_credentials: false,
        }
    }

    /// Get the current region
    pub fn current_region(&self) -> Region {
        self.region
    }

    /// The underlying `SdkConfig`. Exposed so a service can build clients pinned
    /// to a region it discovers at runtime (e.g. Resource Explorer targeting its
    /// aggregator-index region) rather than the currently-selected one.
    pub fn sdk_config(&self) -> &aws_config::SdkConfig {
        &self.config
    }

    /// The active profile name, if any.
    pub fn current_profile(&self) -> Option<&str> {
        self.profile.as_deref()
    }

    /// The active custom endpoint, if pointing at a mock emulator.
    pub fn current_endpoint(&self) -> Option<&str> {
        self.endpoint_url.as_deref()
    }

    /// The cross-account role currently assumed, if any.
    pub fn current_assumed_role(&self) -> Option<&AssumedOrgRole> {
        self.assumed_role.as_ref()
    }

    /// Test-only: mark a member-account role as assumed without the STS call.
    #[cfg(test)]
    pub(crate) fn set_assumed_role_for_test(&mut self, role: AssumedOrgRole) {
        self.assumed_role = Some(role);
    }

    /// Switch to a different region, keeping the active profile + endpoint
    /// (and, when browsing a member account, the assumed role).
    pub async fn switch_region(&self, new_region: Region) -> Result<Self> {
        Self::build(
            self.profile.clone(),
            Some(new_region),
            self.endpoint_url.clone(),
            self.assumed_role.clone(),
        )
        .await
    }

    /// Switch to a different profile, keeping the current region + endpoint.
    /// Drops any assumed role — picking a profile is an explicit credential
    /// change that supersedes the cross-account hop.
    pub async fn switch_profile(&self, profile: String) -> Result<Self> {
        Self::build(Some(profile), Some(self.region), self.endpoint_url.clone(), None).await
    }

    /// Assume `role_name` in a member account (the Organizations `s` action),
    /// keeping the current profile/region/endpoint. The AssumeRole is
    /// validated eagerly with a direct STS call using the *base* credentials —
    /// a missing role (invited accounts don't get `OrganizationAccountAccessRole`)
    /// or a denied `sts:AssumeRole` surfaces here as a clean error and leaves
    /// the current clients untouched.
    pub async fn assume_org_role(
        &self,
        account_id: &str,
        account_name: &str,
        role_name: &str,
    ) -> Result<Self> {
        let role = AssumedOrgRole {
            account_id: account_id.to_string(),
            account_name: account_name.to_string(),
            role_name: role_name.to_string(),
        };

        // Validate from *base* (non-assumed) credentials. Role chaining is not
        // wanted: the member role trusts the management account, so a hop from
        // member account A to member account B must re-assume from base creds.
        let base = Self::build(
            self.profile.clone(),
            Some(self.region),
            self.endpoint_url.clone(),
            None,
        )
        .await?;

        // Probe with the same scoped-down policy the real session uses, so a
        // missing role / denied sts:AssumeRole errors here (via the
        // From<SdkError> conversion → the real AWS message) instead of as an
        // opaque credentials failure on the first resource load.
        let role_arn = format!("arn:aws:iam::{}:role/{}", account_id, role_name);
        base.sts_client()
            .assume_role()
            .role_arn(&role_arn)
            .role_session_name("neboto-readonly")
            .policy_arns(
                aws_sdk_sts::types::PolicyDescriptorType::builder()
                    .arn(READONLY_POLICY_ARN)
                    .build(),
            )
            .policy(READONLY_SUPPLEMENT_POLICY)
            .duration_seconds(900)
            .send()
            .await?;

        Self::build(
            self.profile.clone(),
            Some(self.region),
            self.endpoint_url.clone(),
            Some(role),
        )
        .await
    }

    /// Re-point an `SdkConfig` at **another account** in the same region, via
    /// the same read-only assumed session the member-account switch uses.
    ///
    /// This exists for one specific shape: data that can only be read from an
    /// account other than the one being browsed, where hopping the whole app
    /// would lose the view you're in. Control Tower is the case — its APIs
    /// answer only in the management account while its Config aggregator
    /// lives only in the audit account, so one screen needs both.
    ///
    /// Unlike `assume_org_role` this does not validate eagerly: the caller is
    /// a best-effort load phase whose failure is already a warning, so a
    /// missing role surfaces there rather than tearing down the view.
    pub async fn assume_config_for_account(
        base: &SdkConfig,
        account_id: &str,
        role_name: &str,
    ) -> SdkConfig {
        let role_arn = format!("arn:aws:iam::{}:role/{}", account_id, role_name);
        let provider = aws_config::sts::AssumeRoleProvider::builder(role_arn)
            .session_name("neboto-readonly")
            .policy_arns(vec![READONLY_POLICY_ARN.to_string()])
            .policy(READONLY_SUPPLEMENT_POLICY)
            .configure(base)
            .build()
            .await;
        base.clone()
            .into_builder()
            .credentials_provider(
                aws_credential_types::provider::SharedCredentialsProvider::new(provider),
            )
            .build()
    }

    /// Resolve a prompting `credential_process` now, while the terminal is
    /// still in normal mode, so its passphrase prompt shows before the TUI
    /// rather than under it. The result is cached by the provider, so the
    /// first request doesn't run the process again. A failure is left for the
    /// first load to report, as it would be without this.
    pub async fn prime_prompting_credentials(&self) {
        use aws_credential_types::provider::ProvideCredentials;
        if !self.prompting_credentials {
            return;
        }
        if let Some(provider) = self.config.credentials_provider() {
            let _ = provider.provide_credentials().await;
        }
    }

    /// Drop the assumed role and return to the base profile credentials,
    /// keeping the current region + endpoint.
    pub async fn drop_assumed_role(&self) -> Result<Self> {
        Self::build(self.profile.clone(), Some(self.region), self.endpoint_url.clone(), None).await
    }

    pub fn ec2_client(&self) -> aws_sdk_ec2::Client {
        aws_sdk_ec2::Client::new(&self.config)
    }

    #[allow(dead_code)]
    pub fn rds_client(&self) -> aws_sdk_rds::Client {
        aws_sdk_rds::Client::new(&self.config)
    }

    pub fn pi_client(&self) -> aws_sdk_pi::Client {
        aws_sdk_pi::Client::new(&self.config)
    }

    pub fn ecs_client(&self) -> aws_sdk_ecs::Client {
        aws_sdk_ecs::Client::new(&self.config)
    }

    pub fn ecr_client(&self) -> aws_sdk_ecr::Client {
        aws_sdk_ecr::Client::new(&self.config)
    }

    pub fn ec2_vpc_client(&self) -> aws_sdk_ec2::Client {
        // VPC uses EC2 client
        aws_sdk_ec2::Client::new(&self.config)
    }

    pub fn cloudtrail_client(&self) -> aws_sdk_cloudtrail::Client {
        aws_sdk_cloudtrail::Client::new(&self.config)
    }

    pub fn s3_client(&self) -> aws_sdk_s3::Client {
        // Against a local emulator, virtual-host bucket addressing
        // (`bucket.localhost`) doesn't resolve — force path-style.
        if self.endpoint_url.is_some() {
            let cfg = aws_sdk_s3::config::Builder::from(&self.config)
                .force_path_style(true)
                .build();
            aws_sdk_s3::Client::from_conf(cfg)
        } else {
            aws_sdk_s3::Client::new(&self.config)
        }
    }

    pub fn ssoadmin_client(&self) -> aws_sdk_ssoadmin::Client {
        aws_sdk_ssoadmin::Client::new(&self.config)
    }

    pub fn identitystore_client(&self) -> aws_sdk_identitystore::Client {
        aws_sdk_identitystore::Client::new(&self.config)
    }

    pub fn cloudformation_client(&self) -> aws_sdk_cloudformation::Client {
        aws_sdk_cloudformation::Client::new(&self.config)
    }

    pub fn lambda_client(&self) -> aws_sdk_lambda::Client {
        aws_sdk_lambda::Client::new(&self.config)
    }

    pub fn route53_client(&self) -> aws_sdk_route53::Client {
        aws_sdk_route53::Client::new(&self.config)
    }

    pub fn route53resolver_client(&self) -> aws_sdk_route53resolver::Client {
        aws_sdk_route53resolver::Client::new(&self.config)
    }

    pub fn route53profiles_client(&self) -> aws_sdk_route53profiles::Client {
        aws_sdk_route53profiles::Client::new(&self.config)
    }

    pub fn acm_client(&self) -> aws_sdk_acm::Client {
        aws_sdk_acm::Client::new(&self.config)
    }

    pub fn sfn_client(&self) -> aws_sdk_sfn::Client {
        aws_sdk_sfn::Client::new(&self.config)
    }

    /// Resource Groups Tagging API — list-time tags for services whose own
    /// tag APIs are per-resource (`crate::aws::tags::ListTimeTags`, #29).
    pub fn tagging_client(&self) -> aws_sdk_resourcegroupstagging::Client {
        aws_sdk_resourcegroupstagging::Client::new(&self.config)
    }

    pub fn cloudwatch_client(&self) -> aws_sdk_cloudwatch::Client {
        aws_sdk_cloudwatch::Client::new(&self.config)
    }

    pub fn logs_client(&self) -> aws_sdk_cloudwatchlogs::Client {
        aws_sdk_cloudwatchlogs::Client::new(&self.config)
    }

    pub fn oam_client(&self) -> aws_sdk_oam::Client {
        aws_sdk_oam::Client::new(&self.config)
    }

    pub fn sts_client(&self) -> aws_sdk_sts::Client {
        aws_sdk_sts::Client::new(&self.config)
    }

    pub fn iam_client(&self) -> aws_sdk_iam::Client {
        aws_sdk_iam::Client::new(&self.config)
    }

    pub fn organizations_client(&self) -> aws_sdk_organizations::Client {
        aws_sdk_organizations::Client::new(&self.config)
    }

    pub fn controltower_client(&self) -> aws_sdk_controltower::Client {
        aws_sdk_controltower::Client::new(&self.config)
    }

    pub fn controlcatalog_client(&self) -> aws_sdk_controlcatalog::Client {
        aws_sdk_controlcatalog::Client::new(&self.config)
    }

    pub fn servicecatalog_client(&self) -> aws_sdk_servicecatalog::Client {
        aws_sdk_servicecatalog::Client::new(&self.config)
    }

    pub fn s3files_client(&self) -> aws_sdk_s3files::Client {
        aws_sdk_s3files::Client::new(&self.config)
    }

    pub fn s3tables_client(&self) -> aws_sdk_s3tables::Client {
        aws_sdk_s3tables::Client::new(&self.config)
    }

    pub fn accessanalyzer_client(&self) -> aws_sdk_accessanalyzer::Client {
        aws_sdk_accessanalyzer::Client::new(&self.config)
    }

    pub fn elb_client(&self) -> aws_sdk_elasticloadbalancingv2::Client {
        aws_sdk_elasticloadbalancingv2::Client::new(&self.config)
    }

    pub fn asg_client(&self) -> aws_sdk_autoscaling::Client {
        aws_sdk_autoscaling::Client::new(&self.config)
    }

    pub fn compute_optimizer_client(&self) -> aws_sdk_computeoptimizer::Client {
        aws_sdk_computeoptimizer::Client::new(&self.config)
    }

    pub fn sqs_client(&self) -> aws_sdk_sqs::Client {
        aws_sdk_sqs::Client::new(&self.config)
    }

    pub fn sns_client(&self) -> aws_sdk_sns::Client {
        aws_sdk_sns::Client::new(&self.config)
    }

    pub fn secretsmanager_client(&self) -> aws_sdk_secretsmanager::Client {
        aws_sdk_secretsmanager::Client::new(&self.config)
    }

    pub fn ssm_client(&self) -> aws_sdk_ssm::Client {
        aws_sdk_ssm::Client::new(&self.config)
    }

    /// Cost Explorer is a global service served from a single `us-east-1`
    /// endpoint, so the client is pinned there regardless of the currently
    /// selected region (mirrored by `ServiceType::is_global()` keying the cache
    /// on `us-east-1`).
    pub fn costexplorer_client(&self) -> aws_sdk_costexplorer::Client {
        let cfg = aws_sdk_costexplorer::config::Builder::from(&self.config)
            .region(aws_sdk_costexplorer::config::Region::new("us-east-1"))
            .build();
        aws_sdk_costexplorer::Client::from_conf(cfg)
    }

    /// AWS Budgets is a global service served from a single us-east-1
    /// endpoint, like Cost Explorer.
    pub fn budgets_client(&self) -> aws_sdk_budgets::Client {
        let cfg = aws_sdk_budgets::config::Builder::from(&self.config)
            .region(aws_sdk_budgets::config::Region::new("us-east-1"))
            .build();
        aws_sdk_budgets::Client::from_conf(cfg)
    }

    /// The Invoicing API is a global service served from a single us-east-1
    /// endpoint, like Cost Explorer / Budgets.
    pub fn invoicing_client(&self) -> aws_sdk_invoicing::Client {
        let cfg = aws_sdk_invoicing::config::Builder::from(&self.config)
            .region(aws_sdk_invoicing::config::Region::new("us-east-1"))
            .build();
        aws_sdk_invoicing::Client::from_conf(cfg)
    }

    /// CloudFront is a global service served from a single us-east-1 endpoint;
    /// pin the client there regardless of the selected region (mirrored by
    /// ServiceType::is_global() keying the cache on us-east-1).
    pub fn cloudfront_client(&self) -> aws_sdk_cloudfront::Client {
        let cfg = aws_sdk_cloudfront::config::Builder::from(&self.config)
            .region(aws_sdk_cloudfront::config::Region::new("us-east-1"))
            .build();
        aws_sdk_cloudfront::Client::from_conf(cfg)
    }

    pub fn kms_client(&self) -> aws_sdk_kms::Client {
        aws_sdk_kms::Client::new(&self.config)
    }

    pub fn ram_client(&self) -> aws_sdk_ram::Client {
        aws_sdk_ram::Client::new(&self.config)
    }

    pub fn workspaces_client(&self) -> aws_sdk_workspaces::Client {
        aws_sdk_workspaces::Client::new(&self.config)
    }

    pub fn awsconfig_client(&self) -> aws_sdk_config::Client {
        aws_sdk_config::Client::new(&self.config)
    }

    /// The account this session is currently browsing, when it's an assumed
    /// one. `None` means the base profile's own account.
    pub fn assumed_account_id(&self) -> Option<&str> {
        self.assumed_role.as_ref().map(|r| r.account_id.as_str())
    }


    pub fn wafv2_client(&self) -> aws_sdk_wafv2::Client {
        aws_sdk_wafv2::Client::new(&self.config)
    }

    pub fn directconnect_client(&self) -> aws_sdk_directconnect::Client {
        aws_sdk_directconnect::Client::new(&self.config)
    }

    pub fn apigateway_client(&self) -> aws_sdk_apigateway::Client {
        let retry = aws_config::retry::RetryConfig::standard().with_max_attempts(5);
        let config = aws_sdk_apigateway::config::Builder::from(&self.config)
            .retry_config(retry)
            .build();
        aws_sdk_apigateway::Client::from_conf(config)
    }

    pub fn apigatewayv2_client(&self) -> aws_sdk_apigatewayv2::Client {
        let retry = aws_config::retry::RetryConfig::standard().with_max_attempts(5);
        let config = aws_sdk_apigatewayv2::config::Builder::from(&self.config)
            .retry_config(retry)
            .build();
        aws_sdk_apigatewayv2::Client::from_conf(config)
    }

    pub fn service_quotas_client(&self) -> aws_sdk_servicequotas::Client {
        aws_sdk_servicequotas::Client::new(&self.config)
    }

    pub fn dynamodb_client(&self) -> aws_sdk_dynamodb::Client {
        aws_sdk_dynamodb::Client::new(&self.config)
    }

    pub fn eks_client(&self) -> aws_sdk_eks::Client {
        aws_sdk_eks::Client::new(&self.config)
    }

    pub fn efs_client(&self) -> aws_sdk_efs::Client {
        aws_sdk_efs::Client::new(&self.config)
    }

    pub fn elasticache_client(&self) -> aws_sdk_elasticache::Client {
        aws_sdk_elasticache::Client::new(&self.config)
    }

    pub fn opensearch_client(&self) -> aws_sdk_opensearch::Client {
        aws_sdk_opensearch::Client::new(&self.config)
    }

    pub fn kinesis_client(&self) -> aws_sdk_kinesis::Client {
        aws_sdk_kinesis::Client::new(&self.config)
    }

    pub fn firehose_client(&self) -> aws_sdk_firehose::Client {
        aws_sdk_firehose::Client::new(&self.config)
    }

    pub fn athena_client(&self) -> aws_sdk_athena::Client {
        aws_sdk_athena::Client::new(&self.config)
    }

    pub fn glue_client(&self) -> aws_sdk_glue::Client {
        aws_sdk_glue::Client::new(&self.config)
    }

    pub fn sesv2_client(&self) -> aws_sdk_sesv2::Client {
        aws_sdk_sesv2::Client::new(&self.config)
    }

    pub fn kafka_client(&self) -> aws_sdk_kafka::Client {
        aws_sdk_kafka::Client::new(&self.config)
    }

    pub fn batch_client(&self) -> aws_sdk_batch::Client {
        aws_sdk_batch::Client::new(&self.config)
    }

    pub fn xray_client(&self) -> aws_sdk_xray::Client {
        aws_sdk_xray::Client::new(&self.config)
    }

    pub fn dms_client(&self) -> aws_sdk_databasemigration::Client {
        aws_sdk_databasemigration::Client::new(&self.config)
    }

    pub fn beanstalk_client(&self) -> aws_sdk_elasticbeanstalk::Client {
        aws_sdk_elasticbeanstalk::Client::new(&self.config)
    }

    pub fn fms_client(&self) -> aws_sdk_fms::Client {
        aws_sdk_fms::Client::new(&self.config)
    }

    pub fn redshift_client(&self) -> aws_sdk_redshift::Client {
        aws_sdk_redshift::Client::new(&self.config)
    }

    pub fn redshift_serverless_client(&self) -> aws_sdk_redshiftserverless::Client {
        aws_sdk_redshiftserverless::Client::new(&self.config)
    }

    pub fn transfer_client(&self) -> aws_sdk_transfer::Client {
        aws_sdk_transfer::Client::new(&self.config)
    }

    pub fn eventbridge_client(&self) -> aws_sdk_eventbridge::Client {
        aws_sdk_eventbridge::Client::new(&self.config)
    }

    pub fn scheduler_client(&self) -> aws_sdk_scheduler::Client {
        aws_sdk_scheduler::Client::new(&self.config)
    }

    pub fn pipes_client(&self) -> aws_sdk_pipes::Client {
        aws_sdk_pipes::Client::new(&self.config)
    }

    pub fn guardduty_client(&self) -> aws_sdk_guardduty::Client {
        aws_sdk_guardduty::Client::new(&self.config)
    }

    pub fn securityhub_client(&self) -> aws_sdk_securityhub::Client {
        aws_sdk_securityhub::Client::new(&self.config)
    }

    pub fn cognito_idp_client(&self) -> aws_sdk_cognitoidentityprovider::Client {
        aws_sdk_cognitoidentityprovider::Client::new(&self.config)
    }

    pub fn cognito_identity_client(&self) -> aws_sdk_cognitoidentity::Client {
        aws_sdk_cognitoidentity::Client::new(&self.config)
    }

    pub fn inspector2_client(&self) -> aws_sdk_inspector2::Client {
        aws_sdk_inspector2::Client::new(&self.config)
    }

    pub fn backup_client(&self) -> aws_sdk_backup::Client {
        aws_sdk_backup::Client::new(&self.config)
    }

    pub fn network_firewall_client(&self) -> aws_sdk_networkfirewall::Client {
        aws_sdk_networkfirewall::Client::new(&self.config)
    }

    pub fn codecommit_client(&self) -> aws_sdk_codecommit::Client {
        aws_sdk_codecommit::Client::new(&self.config)
    }

    pub fn codebuild_client(&self) -> aws_sdk_codebuild::Client {
        aws_sdk_codebuild::Client::new(&self.config)
    }

    pub fn codepipeline_client(&self) -> aws_sdk_codepipeline::Client {
        aws_sdk_codepipeline::Client::new(&self.config)
    }

    pub fn codedeploy_client(&self) -> aws_sdk_codedeploy::Client {
        aws_sdk_codedeploy::Client::new(&self.config)
    }

    pub fn codeartifact_client(&self) -> aws_sdk_codeartifact::Client {
        aws_sdk_codeartifact::Client::new(&self.config)
    }

    pub fn fsx_client(&self) -> aws_sdk_fsx::Client {
        aws_sdk_fsx::Client::new(&self.config)
    }

    pub fn bedrock_client(&self) -> aws_sdk_bedrock::Client {
        aws_sdk_bedrock::Client::new(&self.config)
    }

    pub fn bedrockagent_client(&self) -> aws_sdk_bedrockagent::Client {
        aws_sdk_bedrockagent::Client::new(&self.config)
    }

    /// Bedrock AgentCore **control** plane (`bedrock-agentcore-control`) — the
    /// runtime/gateway/memory/identity/tool resource definitions. Distinct from
    /// `bedrockagent_client` (Bedrock Agents, a different service).
    pub fn agentcore_control_client(&self) -> aws_sdk_bedrockagentcorecontrol::Client {
        aws_sdk_bedrockagentcorecontrol::Client::new(&self.config)
    }

    /// Bedrock AgentCore **data** plane (`bedrock-agentcore`) — memory
    /// actors/sessions/events and tool sessions. Read paths only.
    pub fn agentcore_client(&self) -> aws_sdk_bedrockagentcore::Client {
        aws_sdk_bedrockagentcore::Client::new(&self.config)
    }

    pub fn health_client(&self) -> aws_sdk_health::Client {
        let cfg = aws_sdk_health::config::Builder::from(&self.config)
            .region(aws_sdk_health::config::Region::new("us-east-1"))
            .build();
        aws_sdk_health::Client::from_conf(cfg)
    }

    pub fn resourcegroups_client(&self) -> aws_sdk_resourcegroups::Client {
        aws_sdk_resourcegroups::Client::new(&self.config)
    }

    /// Global Accelerator's control plane lives only in `us-west-2`; pin the
    /// client there regardless of the selected region. (`is_global()` keys the
    /// *cache* on us-east-1 — independent of this client region, both correct.)
    pub fn globalaccelerator_client(&self) -> aws_sdk_globalaccelerator::Client {
        let cfg = aws_sdk_globalaccelerator::config::Builder::from(&self.config)
            .region(aws_sdk_globalaccelerator::config::Region::new("us-west-2"))
            .build();
        aws_sdk_globalaccelerator::Client::from_conf(cfg)
    }

    /// The AWS Support API (Trusted Advisor) is only served from `us-east-1`;
    /// pin the client there regardless of the selected region. `is_global()`
    /// keys the cache on us-east-1 to match.
    pub fn support_client(&self) -> aws_sdk_support::Client {
        let cfg = aws_sdk_support::config::Builder::from(&self.config)
            .region(aws_sdk_support::config::Region::new("us-east-1"))
            .build();
        aws_sdk_support::Client::from_conf(cfg)
    }

    /// The newer Trusted Advisor API (Priority recommendations). Same
    /// us-east-1 pinning as the Support API — org recommendations are global.
    pub fn trustedadvisor_client(&self) -> aws_sdk_trustedadvisor::Client {
        let cfg = aws_sdk_trustedadvisor::config::Builder::from(&self.config)
            .region(aws_sdk_trustedadvisor::config::Region::new("us-east-1"))
            .build();
        aws_sdk_trustedadvisor::Client::from_conf(cfg)
    }

    /// WAFv2 CLOUDFRONT scope must use us-east-1 endpoint.
    pub fn wafv2_cloudfront_client(&self) -> aws_sdk_wafv2::Client {
        let cfg = aws_sdk_wafv2::config::Builder::from(&self.config)
            .region(aws_sdk_wafv2::config::Region::new("us-east-1"))
            .build();
        aws_sdk_wafv2::Client::from_conf(cfg)
    }

    /// S3 client pinned to a specific region — for fetching objects whose URL
    /// names a bucket outside the current region (Service Catalog template
    /// buckets).
    pub fn s3_client_for_region(&self, region: &str) -> aws_sdk_s3::Client {
        let mut builder = aws_sdk_s3::config::Builder::from(&self.config)
            .region(aws_sdk_s3::config::Region::new(region.to_string()));
        if self.endpoint_url.is_some() {
            builder = builder.force_path_style(true);
        }
        aws_sdk_s3::Client::from_conf(builder.build())
    }

    /// CloudWatch Logs client pinned to a specific region — for tailing log
    /// groups that live outside the current region (a CLOUDFRONT-scope WAF
    /// web ACL logs to us-east-1).
    pub fn logs_client_for_region(&self, region: &str) -> aws_sdk_cloudwatchlogs::Client {
        let cfg = aws_sdk_cloudwatchlogs::config::Builder::from(&self.config)
            .region(aws_sdk_cloudwatchlogs::config::Region::new(region.to_string()))
            .build();
        aws_sdk_cloudwatchlogs::Client::from_conf(cfg)
    }

    /// CloudWatch client pinned to us-east-1 (for global-scope metrics like WAF CLOUDFRONT).
    pub fn cloudwatch_us_east_1_client(&self) -> aws_sdk_cloudwatch::Client {
        let cfg = aws_sdk_cloudwatch::config::Builder::from(&self.config)
            .region(aws_sdk_cloudwatch::config::Region::new("us-east-1"))
            .build();
        aws_sdk_cloudwatch::Client::from_conf(cfg)
    }

    /// CloudWatch client pinned to us-west-2 — Global Accelerator's home region,
    /// the only place it publishes `AWS/GlobalAccelerator` metrics.
    pub fn cloudwatch_us_west_2_client(&self) -> aws_sdk_cloudwatch::Client {
        let cfg = aws_sdk_cloudwatch::config::Builder::from(&self.config)
            .region(aws_sdk_cloudwatch::config::Region::new("us-west-2"))
            .build();
        aws_sdk_cloudwatch::Client::from_conf(cfg)
    }

    /// CloudWatch client for an arbitrary region — unlike the two pinned
    /// factories above, the region isn't known until something is read at
    /// runtime (a dashboard widget declares its own `region`).
    pub fn cloudwatch_client_in(&self, region: &str) -> aws_sdk_cloudwatch::Client {
        cloudwatch_client_for(&self.config, region)
    }

}

/// Build a CloudWatch client for `region` off an already-resolved base config.
/// The SDK loader every client starts from. The app name puts `app/neboto`
/// in each request's User-Agent, so CloudTrail's `userAgent` field shows
/// which calls neboto made — anyone can audit the read-only claim from their
/// own trail. Configs derived with `into_builder` (profile pinning, assumed
/// roles) keep it.
fn sdk_defaults() -> aws_config::ConfigLoader {
    aws_config::defaults(aws_config::BehaviorVersion::latest())
        .app_name(aws_config::AppName::new("neboto").expect("valid app name"))
}

/// Free-standing so a spawned fetch can call it with a cloned `SdkConfig`.
pub fn cloudwatch_client_for(config: &SdkConfig, region: &str) -> aws_sdk_cloudwatch::Client {
    let cfg = aws_sdk_cloudwatch::config::Builder::from(config)
        .region(aws_sdk_cloudwatch::config::Region::new(region.to_string()))
        .build();
    aws_sdk_cloudwatch::Client::from_conf(cfg)
}

/// Replace `config`'s credentials provider with the profile-file provider for
/// `name` (static keys, `source_profile` + `role_arn` chains, SSO,
/// `credential_process` — everything the shared config supports), bypassing
/// the default chain's environment-first lookup. The region is carried over
/// so an in-profile role chain has an STS endpoint to talk to.
fn with_profile_credentials(config: SdkConfig, name: &str) -> SdkConfig {
    let provider_config = aws_config::provider_config::ProviderConfig::without_region()
        .with_region(config.region().cloned());
    let provider = aws_config::profile::ProfileFileCredentialsProvider::builder()
        .profile_name(name)
        .configure(&provider_config)
        .build();
    config
        .into_builder()
        .credentials_provider(
            aws_credential_types::provider::SharedCredentialsProvider::new(provider),
        )
        .build()
}

/// A credentials provider whose `credential_process` may need the terminal.
/// Every time it really runs the process it hands the terminal over
/// (`tui::with_terminal`), and it keeps the result so the run before the TUI
/// starts serves the first request, and a burst of parallel fetches runs the
/// process once.
#[derive(Debug)]
struct TerminalCredentialProcess {
    inner: aws_credential_types::provider::SharedCredentialsProvider,
    profile: String,
    cached: tokio::sync::Mutex<Option<aws_credential_types::Credentials>>,
}

impl TerminalCredentialProcess {
    /// A cached result this close to expiry is refreshed instead of served.
    const REFRESH_BEFORE: std::time::Duration = std::time::Duration::from_secs(60);

    fn new(
        inner: aws_credential_types::provider::SharedCredentialsProvider,
        profile: String,
    ) -> Self {
        Self {
            inner,
            profile,
            cached: tokio::sync::Mutex::new(None),
        }
    }
}

impl aws_credential_types::provider::ProvideCredentials for TerminalCredentialProcess {
    fn provide_credentials<'a>(
        &'a self,
    ) -> aws_credential_types::provider::future::ProvideCredentials<'a>
    where
        Self: 'a,
    {
        aws_credential_types::provider::future::ProvideCredentials::new(async move {
            let mut cached = self.cached.lock().await;
            if let Some(creds) = cached.as_ref() {
                let fresh = creds.expiry().is_none_or(|exp| {
                    exp > std::time::SystemTime::now() + Self::REFRESH_BEFORE
                });
                if fresh {
                    return Ok(creds.clone());
                }
            }
            let note = format!(
                "Getting credentials for profile {} (credential_process)…",
                self.profile
            );
            let result = crate::tui::with_terminal(&note, self.inner.provide_credentials()).await;
            if let Ok(creds) = &result {
                *cached = Some(creds.clone());
            }
            result
        })
    }
}

/// The profile whose credentials come from a `credential_process` — `profile`
/// or, with none chosen, `AWS_PROFILE` / `default` — or None. Follows
/// `source_profile` chains, since a role profile sourcing a
/// `credential_process` one runs the process too. With no profile chosen and
/// static keys in the environment the default chain never reaches a profile,
/// so that is None too.
fn credential_process_profile(profile: Option<&str>) -> Option<String> {
    let name = match profile {
        Some(p) => p.to_string(),
        None => {
            let env_keys = std::env::var("AWS_ACCESS_KEY_ID").is_ok_and(|v| !v.is_empty());
            if env_keys {
                return None;
            }
            std::env::var("AWS_PROFILE")
                .ok()
                .filter(|p| !p.is_empty())
                .unwrap_or_else(|| "default".to_string())
        }
    };
    let home = std::env::var("HOME").unwrap_or_default();
    let config = std::env::var("AWS_CONFIG_FILE")
        .unwrap_or_else(|_| format!("{}/.aws/config", home));
    let creds = std::env::var("AWS_SHARED_CREDENTIALS_FILE")
        .unwrap_or_else(|_| format!("{}/.aws/credentials", home));
    let config = std::fs::read_to_string(config).unwrap_or_default();
    let creds = std::fs::read_to_string(creds).unwrap_or_default();
    chain_uses_credential_process(&config, &creds, &name).then_some(name)
}

/// Whether `name`'s credentials, following `source_profile` hops, come from a
/// `credential_process`. Pure over the two files' text, for tests.
fn chain_uses_credential_process(config: &str, creds: &str, name: &str) -> bool {
    use std::collections::HashMap;

    // profile -> key -> value; the credentials file's keys win, as in the SDK.
    let mut profiles: HashMap<String, HashMap<String, String>> = HashMap::new();
    let mut parse = |text: &str, is_config: bool| {
        let mut current: Option<String> = None;
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                continue;
            }
            if let Some(section) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                let section = section.trim();
                current = if !is_config || section == "default" {
                    Some(section.to_string())
                } else {
                    section
                        .strip_prefix("profile ")
                        .map(|p| p.trim().to_string())
                };
                continue;
            }
            if let (Some(profile), Some((key, value))) = (&current, line.split_once('=')) {
                profiles
                    .entry(profile.clone())
                    .or_default()
                    .insert(key.trim().to_ascii_lowercase(), value.trim().to_string());
            }
        }
    };
    parse(config, true);
    parse(creds, false);

    // The SDK's own order: a role profile hops to its `source_profile`
    // (staying put on a self-reference); the base profile then uses web
    // identity, SSO or a login session ahead of `credential_process`.
    let mut name = name.to_string();
    for _ in 0..10 {
        let Some(keys) = profiles.get(&name) else {
            return false;
        };
        if keys.contains_key("role_arn") {
            match keys.get("source_profile") {
                _ if keys.contains_key("credential_source") => return false,
                Some(next) if *next != name => {
                    name = next.clone();
                    continue;
                }
                Some(_) => {}
                None => return false,
            }
        }
        let earlier = [
            "web_identity_token_file",
            "sso_session",
            "sso_start_url",
            "sso_account_id",
            "login_session",
        ];
        if earlier.iter().any(|k| keys.contains_key(*k)) {
            return false;
        }
        return keys.get("credential_process").is_some_and(|v| !v.is_empty());
    }
    false
}

#[cfg(test)]
mod credential_process_tests {
    use super::chain_uses_credential_process as uses;

    const CONFIG: &str = "\
[default]
region = eu-west-1

[profile granted]
credential_process = granted credential-process --profile granted

[profile admin]
role_arn = arn:aws:iam::111111111111:role/Admin
source_profile = granted

[profile sso]
sso_session = corp
sso_account_id = 222222222222

[sso-session corp]
credential_process = not-a-profile

[profile loop]
role_arn = arn:aws:iam::111111111111:role/Loop
source_profile = loop

[profile ec2]
role_arn = arn:aws:iam::111111111111:role/Admin
credential_source = Ec2InstanceMetadata
credential_process = ignored-with-a-credential-source
";

    #[test]
    fn a_direct_credential_process_is_found() {
        assert!(uses(CONFIG, "", "granted"));
    }

    #[test]
    fn a_source_profile_chain_reaches_it() {
        assert!(uses(CONFIG, "", "admin"));
    }

    #[test]
    fn other_providers_are_not_flagged() {
        assert!(!uses(CONFIG, "", "default"));
        assert!(!uses(CONFIG, "", "sso"));
        assert!(!uses(CONFIG, "", "corp"));
        assert!(!uses(CONFIG, "", "missing"));
        assert!(!uses(CONFIG, "", "loop"));
        assert!(!uses(CONFIG, "", "ec2"));
    }

    #[test]
    fn the_credentials_file_counts_and_source_profile_needs_a_role() {
        // Without role_arn the SDK never reads source_profile.
        let creds = "[default]\ncredential_process = /usr/bin/fetch-creds\n\n[keys]\naws_access_key_id = AKIDEXAMPLE\nsource_profile = granted\n";
        assert!(uses(CONFIG, creds, "default"));
        assert!(!uses(CONFIG, creds, "keys"));
    }
}

/// Enumerate the named profiles configured locally, reading `~/.aws/config`
/// (`[default]` / `[profile NAME]`) and `~/.aws/credentials` (`[NAME]`), honoring
/// the `AWS_CONFIG_FILE` / `AWS_SHARED_CREDENTIALS_FILE` overrides. `default` is
/// listed first when present; the rest are sorted alphabetically.
pub fn list_profiles() -> Vec<String> {
    use std::collections::BTreeSet;

    let home = std::env::var("HOME").unwrap_or_default();
    let config_path = std::env::var("AWS_CONFIG_FILE")
        .unwrap_or_else(|_| format!("{}/.aws/config", home));
    let creds_path = std::env::var("AWS_SHARED_CREDENTIALS_FILE")
        .unwrap_or_else(|_| format!("{}/.aws/credentials", home));

    let mut set: BTreeSet<String> = BTreeSet::new();

    // config file: [default] and [profile NAME]
    if let Ok(content) = std::fs::read_to_string(&config_path) {
        for line in content.lines() {
            if let Some(name) = line
                .trim()
                .strip_prefix('[')
                .and_then(|s| s.strip_suffix(']'))
            {
                let name = name.trim();
                if name == "default" {
                    set.insert("default".to_string());
                } else if let Some(p) = name.strip_prefix("profile ") {
                    let p = p.trim();
                    if !p.is_empty() {
                        set.insert(p.to_string());
                    }
                }
            }
        }
    }

    // credentials file: [NAME]
    if let Ok(content) = std::fs::read_to_string(&creds_path) {
        for line in content.lines() {
            if let Some(name) = line
                .trim()
                .strip_prefix('[')
                .and_then(|s| s.strip_suffix(']'))
            {
                let name = name.trim();
                if !name.is_empty() {
                    set.insert(name.to_string());
                }
            }
        }
    }

    let mut profiles: Vec<String> = set.into_iter().collect();
    if let Some(pos) = profiles.iter().position(|p| p == "default") {
        let d = profiles.remove(pos);
        profiles.insert(0, d);
    }
    profiles
}

/// Resolve the custom endpoint: an explicit value (from the config file) unless
/// `AWS_ENDPOINT_URL` is set in the environment, which always wins. Empty values
/// are treated as unset.
fn resolve_endpoint(explicit: Option<String>) -> Option<String> {
    std::env::var("AWS_ENDPOINT_URL")
        .ok()
        .or(explicit)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Throwaway credentials for a local emulator (floci/LocalStack accept any
/// non-empty values). Never used against real AWS — only injected when a custom
/// endpoint is configured.
fn mock_credentials() -> aws_sdk_ec2::config::Credentials {
    aws_sdk_ec2::config::Credentials::new("test", "test", None, None, "floci")
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_credential_types::provider::ProvideCredentials;
    use std::io::Write;

    /// Every config carries the app name (CloudTrail's `userAgent` shows
    /// `app/neboto`), including one re-pointed at another account.
    #[tokio::test]
    async fn every_config_names_the_app() {
        let clients = AwsClients::new_for_test().await;
        assert_eq!(clients.sdk_config().app_name().map(|a| a.as_ref()), Some("neboto"));
        let other = AwsClients::assume_config_for_account(clients.sdk_config(), "111122223333", "Audit").await;
        assert_eq!(other.app_name().map(|a| a.as_ref()), Some("neboto"));
        let demo = AwsClients::new_demo_for_test().await;
        assert_eq!(demo.sdk_config().app_name().map(|a| a.as_ref()), Some("neboto"));
    }

    /// An explicit profile must beat `AWS_ACCESS_KEY_ID` in the environment.
    /// The SDK's default chain tries the environment first, which made a `P`
    /// profile switch a no-op whenever keys were exported in the shell: the
    /// bar showed the new profile name over the old account id. Mutates the
    /// process environment — the other tests build via `new_for_test`, which
    /// pins region + mock credentials and so never reads these variables.
    #[tokio::test]
    async fn explicit_profile_wins_over_environment_keys() {
        let dir = tempfile::tempdir().expect("tempdir");
        let creds_path = dir.path().join("credentials");
        let config_path = dir.path().join("config");
        let mut creds = std::fs::File::create(&creds_path).expect("credentials file");
        writeln!(
            creds,
            "[keyprofile]\naws_access_key_id = AKIAFROMPROFILE\naws_secret_access_key = profilesecret"
        )
        .expect("write credentials");
        std::fs::File::create(&config_path).expect("config file");

        std::env::set_var("AWS_SHARED_CREDENTIALS_FILE", &creds_path);
        std::env::set_var("AWS_CONFIG_FILE", &config_path);
        std::env::set_var("AWS_ACCESS_KEY_ID", "AKIAFROMENV");
        std::env::set_var("AWS_SECRET_ACCESS_KEY", "envsecret");

        let built = AwsClients::build(
            Some("keyprofile".to_string()),
            Some(Region::UsEast1),
            None,
            None,
        )
        .await;
        let creds = match &built {
            Ok(clients) => {
                clients
                    .config
                    .credentials_provider()
                    .expect("credentials provider")
                    .provide_credentials()
                    .await
            }
            Err(_) => unreachable!("offline build never fails"),
        };

        std::env::remove_var("AWS_SHARED_CREDENTIALS_FILE");
        std::env::remove_var("AWS_CONFIG_FILE");
        std::env::remove_var("AWS_ACCESS_KEY_ID");
        std::env::remove_var("AWS_SECRET_ACCESS_KEY");

        let creds = creds.expect("profile credentials resolve");
        assert_eq!(creds.access_key_id(), "AKIAFROMPROFILE");
    }
}
