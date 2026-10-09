//! The demo dataset: one made-up account, `acme-prod`, with problems planted
//! in it to find. Responses are real wire-format bodies (XML for EC2 / STS,
//! awsJson for ECS) with `{{…}}` placeholders — see `demo::render`.
//!
//! The story — each item is one thing a showcase clip can find:
//! - `web-sg` allows SSH from 0.0.0.0/0 ("temp debug access", never
//!   removed); `W` on it shows jsmith opened it five days ago
//! - an unattached 500 GiB gp2 volume, detached and forgotten
//! - ECS service `orders-worker`: deployment of revision 15 failed and the
//!   circuit breaker rolled it back. The stopped task and its log say why
//!   (missing `DATABASE_URL`), revision 15's env says `DB_URL`, and `W`
//!   shows the CI role deployed it
//! - `storefront-web` → its target group → `storefront-alb` → `alb-sg` →
//!   `web-sg` is the Enter-to-follow chain; everything on it is owned by
//!   the `storefront-prod` stack, which has drifted (the target group's
//!   health-check interval, changed in the console — `W` says by whom)
//! - `orders-pipeline` stack is in UPDATE_ROLLBACK_COMPLETE (a DLQ rename):
//!   its template builds the redrive ARN from the `DlqName` parameter while
//!   the DLQ itself keeps a hardcoded name
//! - `github-actions-deploy` got AdministratorAccess three days ago
//! - `orders-api` (Lambda) tails live logs with occasional DynamoDB
//!   throttles, and its errors alarm is firing; `nightly-report` runs on a deprecated runtime and its log
//!   group never expires (45 GB)
//! - `legacy-admin-alb` has no targets; IAM has a stale `legacy-reporting-role`
//! - ECR: `storefront-web:v2.31.0` (running) has an unfixed HIGH openssl
//!   finding; `orders-worker:v1.19.0`'s Used By lists the stopped tasks of
//!   the failed deployment (matched by the digest they pinned)
//! - Beanstalk app `acme-portal` runs one Standard and one Cluster Mode
//!   environment: `portal-legacy` is Red (it sits behind the target-less
//!   `legacy-admin-alb`), `portal-next` is Green on a shared EKS cluster
//! - a Secrets Manager secret `prod/orders/db` and a SecureString parameter
//!   `/orders/db/password` — metadata only, for proving no value is fetched
//! - Cost Anomaly Detection (`@cost`, `8`) flagged CloudWatch six days ago
//!   — DataProcessing-Bytes, i.e. log ingestion, and `nightly-report`'s
//!   never-expiring group is the story behind it — plus Fargate vCPU from
//!   the `orders-worker` restart loop; a closed EC2 spike was marked planned
//!
//! Order matters: the first entry whose `when` substrings all appear in the
//! request body wins, so put narrow matches before the catch-all. (Batch
//! calls like `DescribeTasks` get one fixture per id set the app asks for; a
//! fixture that filters its items by the requested ids is the scalable
//! version, left for when the dataset grows.)

pub struct Fixture {
    pub service: &'static str,
    pub operation: &'static str,
    /// Substrings the request body must all contain (empty = any request).
    pub when: &'static [&'static str],
    pub body: &'static str,
}

macro_rules! fx {
    ($svc:literal, $op:literal, $file:literal) => {
        Fixture {
            service: $svc,
            operation: $op,
            when: &[],
            body: include_str!(concat!("fixtures/", $svc, "/", $file)),
        }
    };
    ($svc:literal, $op:literal, when [$($w:literal),+], $file:literal) => {
        Fixture {
            service: $svc,
            operation: $op,
            when: &[$($w),+],
            body: include_str!(concat!("fixtures/", $svc, "/", $file)),
        }
    };
}

pub static FIXTURES: &[Fixture] = &[
    // ── STS ──────────────────────────────────────────────────────────────────
    fx!("sts", "GetCallerIdentity", "GetCallerIdentity.xml"),
    // ── IAM ──────────────────────────────────────────────────────────────────
    fx!("iam", "ListAccountAliases", "ListAccountAliases.xml"),
    // ── EC2 ──────────────────────────────────────────────────────────────────
    fx!("ec2", "DescribeInstances", "DescribeInstances.xml"),
    fx!("ec2", "DescribeSecurityGroups", "DescribeSecurityGroups.xml"),
    fx!("ec2", "DescribeVolumes", "DescribeVolumes.xml"),
    // ── ECS ──────────────────────────────────────────────────────────────────
    fx!("ecs", "ListClusters", "ListClusters.json"),
    fx!("ecs", "DescribeClusters", "DescribeClusters.json"),
    fx!("ecs", "ListServices", "ListServices.json"),
    fx!("ecs", "DescribeServices", "DescribeServices.json"),
    fx!("ecs", "ListTaskDefinitions", "ListTaskDefinitions.json"),
    fx!("ecs", "DescribeTaskDefinition", when ["orders-worker:15"], "DescribeTaskDefinition-worker-15.json"),
    fx!("ecs", "DescribeTaskDefinition", when ["orders-worker"], "DescribeTaskDefinition-worker-14.json"),
    fx!("ecs", "DescribeTaskDefinition", "DescribeTaskDefinition-web.json"),
    // A service's own tasks (its Tasks section) vs the cluster-wide load.
    // Stopped tasks exist only for orders-worker (the failed rollout).
    fx!("ecs", "ListTasks", when ["STOPPED", "storefront-web"], "ListTasks-none.json"),
    fx!("ecs", "ListTasks", when ["STOPPED"], "ListTasks-stopped.json"),
    fx!("ecs", "ListTasks", when ["orders-worker"], "ListTasks-worker.json"),
    fx!("ecs", "ListTasks", when ["storefront-web"], "ListTasks-web.json"),
    fx!("ecs", "ListTasks", "ListTasks-running.json"),
    // DescribeTasks answers for exactly the ids asked: all running (the
    // cluster-wide load), the stopped pair, or one service's tasks.
    fx!("ecs", "DescribeTasks", when ["0f1e2d3c", "3c4d5e6f"], "DescribeTasks-running.json"),
    fx!("ecs", "DescribeTasks", when ["5d1e0c7a"], "DescribeTasks-stopped.json"),
    fx!("ecs", "DescribeTasks", when ["3c4d5e6f"], "DescribeTasks-worker.json"),
    fx!("ecs", "DescribeTasks", "DescribeTasks-web.json"),
    // ── IAM ──────────────────────────────────────────────────────────────────
    // Per-principal calls match on `RoleName=` / `UserName=` / the policy
    // ARN's tail in the form body.
    fx!("iam", "ListRoles", "ListRoles.xml"),
    fx!("iam", "GetRole", when ["RoleName=storefront-web-task"], "GetRole-storefront-web-task.xml"),
    fx!("iam", "GetRole", when ["RoleName=orders-worker-task"], "GetRole-orders-worker-task.xml"),
    fx!("iam", "GetRole", when ["RoleName=ecs-task-execution"], "GetRole-ecs-task-execution.xml"),
    fx!("iam", "GetRole", when ["RoleName=orders-api-lambda"], "GetRole-orders-api-lambda.xml"),
    fx!("iam", "GetRole", when ["RoleName=github-actions-deploy"], "GetRole-github-actions-deploy.xml"),
    fx!("iam", "GetRole", when ["RoleName=OrganizationAccountAccessRole"], "GetRole-OrganizationAccountAccessRole.xml"),
    fx!("iam", "GetRole", when ["RoleName=legacy-reporting-role"], "GetRole-legacy-reporting-role.xml"),
    fx!("iam", "ListAttachedRolePolicies", when ["RoleName=storefront-web-task"], "ListAttachedRolePolicies-storefront-web-task.xml"),
    fx!("iam", "ListAttachedRolePolicies", when ["RoleName=orders-worker-task"], "ListAttachedRolePolicies-orders-worker-task.xml"),
    fx!("iam", "ListAttachedRolePolicies", when ["RoleName=ecs-task-execution"], "ListAttachedRolePolicies-ecs-task-execution.xml"),
    fx!("iam", "ListAttachedRolePolicies", when ["RoleName=orders-api-lambda"], "ListAttachedRolePolicies-orders-api-lambda.xml"),
    fx!("iam", "ListAttachedRolePolicies", when ["RoleName=github-actions-deploy"], "ListAttachedRolePolicies-github-actions-deploy.xml"),
    fx!("iam", "ListAttachedRolePolicies", when ["RoleName=OrganizationAccountAccessRole"], "ListAttachedRolePolicies-OrganizationAccountAccessRole.xml"),
    fx!("iam", "ListAttachedRolePolicies", when ["RoleName=legacy-reporting-role"], "ListAttachedRolePolicies-legacy-reporting-role.xml"),
    fx!("iam", "ListRolePolicies", when ["RoleName=orders-worker-task"], "ListRolePolicies-orders-worker-task.xml"),
    fx!("iam", "ListPolicies", "ListPolicies.xml"),
    fx!("iam", "GetPolicy", when ["storefront-web-s3"], "GetPolicy-storefront-web-s3.xml"),
    fx!("iam", "GetPolicy", when ["orders-dynamodb"], "GetPolicy-orders-dynamodb.xml"),
    fx!("iam", "GetPolicyVersion", when ["storefront-web-s3"], "GetPolicyVersion-storefront-web-s3.xml"),
    fx!("iam", "GetPolicyVersion", when ["orders-dynamodb"], "GetPolicyVersion-orders-dynamodb.xml"),
    fx!("iam", "GetPolicy", when ["%2FAdministratorAccess"], "GetPolicy-AdministratorAccess.xml"),
    fx!("iam", "GetPolicy", when ["%2FAmazonS3FullAccess"], "GetPolicy-AmazonS3FullAccess.xml"),
    fx!("iam", "GetPolicy", when ["%2FAmazonRDSReadOnlyAccess"], "GetPolicy-AmazonRDSReadOnlyAccess.xml"),
    fx!("iam", "GetPolicy", when ["%2FAmazonECS_FullAccess"], "GetPolicy-AmazonECS_FullAccess.xml"),
    fx!("iam", "GetPolicy", when ["%2FAmazonECSTaskExecutionRolePolicy"], "GetPolicy-AmazonECSTaskExecutionRolePolicy.xml"),
    fx!("iam", "GetPolicy", when ["%2FAWSLambdaBasicExecutionRole"], "GetPolicy-AWSLambdaBasicExecutionRole.xml"),
    fx!("iam", "GetPolicyVersion", when ["%2FAdministratorAccess"], "GetPolicyVersion-AdministratorAccess.xml"),
    fx!("iam", "GetPolicyVersion", when ["%2FAmazonS3FullAccess"], "GetPolicyVersion-AmazonS3FullAccess.xml"),
    fx!("iam", "GetPolicyVersion", when ["%2FAmazonRDSReadOnlyAccess"], "GetPolicyVersion-AmazonRDSReadOnlyAccess.xml"),
    fx!("iam", "GetPolicyVersion", when ["%2FAmazonECS_FullAccess"], "GetPolicyVersion-AmazonECS_FullAccess.xml"),
    fx!("iam", "GetPolicyVersion", when ["%2FAmazonECSTaskExecutionRolePolicy"], "GetPolicyVersion-AmazonECSTaskExecutionRolePolicy.xml"),
    fx!("iam", "GetPolicyVersion", when ["%2FAWSLambdaBasicExecutionRole"], "GetPolicyVersion-AWSLambdaBasicExecutionRole.xml"),
    fx!("iam", "GetRolePolicy", when ["RoleName=orders-worker-task", "PolicyName=sqs-consume-orders"], "GetRolePolicy-orders-worker-task.xml"),
    fx!("iam", "ListEntitiesForPolicy", when ["storefront-web-s3"], "ListEntitiesForPolicy-storefront-web-s3.xml"),
    fx!("iam", "ListEntitiesForPolicy", when ["orders-dynamodb"], "ListEntitiesForPolicy-orders-dynamodb.xml"),
    fx!("iam", "ListUsers", "ListUsers.xml"),
    fx!("iam", "GetUser", when ["UserName=jsmith"], "GetUser-jsmith.xml"),
    fx!("iam", "GetUser", when ["UserName=deploy-bot"], "GetUser-deploy-bot.xml"),
    fx!("iam", "ListAccessKeys", when ["UserName=jsmith"], "ListAccessKeys-jsmith.xml"),
    fx!("iam", "ListAccessKeys", when ["UserName=deploy-bot"], "ListAccessKeys-deploy-bot.xml"),
    fx!("iam", "GetAccessKeyLastUsed", when ["JSMITH"], "GetAccessKeyLastUsed-jsmith.xml"),
    fx!("iam", "GetAccessKeyLastUsed", when ["DEPLOYB"], "GetAccessKeyLastUsed-deploy-bot.xml"),
    fx!("iam", "ListMFADevices", when ["UserName=jsmith"], "ListMFADevices-jsmith.xml"),
    fx!("iam", "GetLoginProfile", when ["UserName=jsmith"], "GetLoginProfile-jsmith.xml"),
    fx!("iam", "ListGroupsForUser", when ["UserName=jsmith"], "ListGroupsForUser-jsmith.xml"),
    fx!("iam", "ListAttachedUserPolicies", when ["UserName=deploy-bot"], "ListAttachedUserPolicies-deploy-bot.xml"),
    fx!("iam", "ListGroups", "ListGroups.xml"),
    fx!("iam", "GetGroup", "GetGroup.xml"),
    fx!("iam", "ListAttachedGroupPolicies", "ListAttachedGroupPolicies.xml"),
    fx!("iam", "ListOpenIDConnectProviders", "ListOpenIDConnectProviders.xml"),
    fx!("iam", "GetOpenIDConnectProvider", "GetOpenIDConnectProvider.xml"),
    fx!("iam", "GetAccountSummary", "GetAccountSummary.xml"),
    fx!("iam", "GetAccountPasswordPolicy", "GetAccountPasswordPolicy.xml"),
    // ── Elastic Load Balancing v2 ────────────────────────────────────────────
    fx!("elasticloadbalancing", "DescribeLoadBalancers", "DescribeLoadBalancers.xml"),
    fx!("elasticloadbalancing", "DescribeTargetGroups", "DescribeTargetGroups.xml"),
    fx!("elasticloadbalancing", "DescribeTags", "DescribeTags.xml"),
    fx!("elasticloadbalancing", "DescribeTargetHealth", when ["storefront-web"], "DescribeTargetHealth-storefront.xml"),
    fx!("elasticloadbalancing", "DescribeTargetHealth", "DescribeTargetHealth-empty.xml"),
    fx!("elasticloadbalancing", "DescribeListeners", when ["storefront-alb"], "DescribeListeners-storefront.xml"),
    fx!("elasticloadbalancing", "DescribeListeners", when ["legacy-admin-alb"], "DescribeListeners-legacy.xml"),
    fx!("elasticloadbalancing", "DescribeRules", when ["1a2b3c4d5e6f7a8b"], "DescribeRules.xml"),
    fx!("elasticloadbalancing", "DescribeLoadBalancerAttributes", "DescribeLoadBalancerAttributes.xml"),
    fx!("elasticloadbalancing", "DescribeTargetGroupAttributes", "DescribeTargetGroupAttributes.xml"),
    // ── ECR (awsJson; host `api.ecr.…`) ──────────────────────────────────────
    // storefront-web's v2.31.0 (what the web tasks run) carries Inspector
    // findings; orders-worker's v1.19.0 is the failed rev-15 image (the
    // stopped tasks pin its digest), v1.18.2 is what's running after the
    // rollback, plus one untagged image whose scan FAILED.
    fx!("ecr", "DescribeRepositories", "DescribeRepositories.json"),
    fx!("ecr", "DescribeImages", when ["storefront-web"], "DescribeImages-storefront-web.json"),
    fx!("ecr", "DescribeImages", when ["orders-worker"], "DescribeImages-orders-worker.json"),
    fx!("ecr", "DescribeImageScanFindings", when ["storefront-web", "a3f1c9e2b7d4058c6e91f2a7b3d8c4e5f60718293a4b5c6d7e8f9012a3b4c5d6"], "DescribeImageScanFindings-storefront-web.json"),
    fx!("ecr", "DescribeImageScanFindings", "ScanNotFound.json"),
    // ── Lambda (restJson: the operation is `METHOD /path`) ───────────────────
    fx!("lambda", "GET /2015-03-31/functions", "ListFunctions.json"),
    fx!("lambda", "GET /2015-03-31/functions/orders-api", "GetFunction-orders-api.json"),
    fx!("lambda", "GET /2015-03-31/functions/thumbnail-resizer", "GetFunction-thumbnail-resizer.json"),
    fx!("lambda", "GET /2015-03-31/functions/nightly-report", "GetFunction-nightly-report.json"),
    fx!("lambda", "GET /2019-09-30/functions/orders-api/concurrency", "GetFunctionConcurrency-orders-api.json"),
    fx!("lambda", "GET /2019-09-25/functions/*/event-invoke-config", "NotFound.json"),
    // ── CloudFormation ───────────────────────────────────────────────────────
    // Per-stack calls pass the stack ARN, which carries the name.
    fx!("cloudformation", "DescribeStacks", "DescribeStacks.xml"),
    fx!("cloudformation", "ListStacks", "ListStacks.xml"),
    fx!("cloudformation", "DescribeStackResources", when ["storefront-prod"], "DescribeStackResources-storefront-prod.xml"),
    fx!("cloudformation", "DescribeStackResources", when ["orders-pipeline"], "DescribeStackResources-orders-pipeline.xml"),
    fx!("cloudformation", "DescribeStackResources", when ["acme-network"], "DescribeStackResources-acme-network.xml"),
    fx!("cloudformation", "DescribeStackEvents", when ["storefront-prod"], "DescribeStackEvents-storefront-prod.xml"),
    fx!("cloudformation", "DescribeStackEvents", when ["orders-pipeline"], "DescribeStackEvents-orders-pipeline.xml"),
    fx!("cloudformation", "DescribeStackEvents", when ["acme-network"], "DescribeStackEvents-acme-network.xml"),
    fx!("cloudformation", "GetTemplate", when ["storefront-prod"], "GetTemplate-storefront-prod.xml"),
    fx!("cloudformation", "GetTemplate", when ["orders-pipeline"], "GetTemplate-orders-pipeline.xml"),
    fx!("cloudformation", "GetTemplate", when ["acme-network"], "GetTemplate-acme-network.xml"),
    fx!("cloudformation", "DescribeStackResourceDrifts", when ["storefront-prod"], "DescribeStackResourceDrifts-storefront-prod.xml"),
    fx!("cloudformation", "GetStackPolicy", when ["storefront-prod"], "GetStackPolicy-storefront-prod.xml"),
    fx!("cloudformation", "ListExports", "ListExports.xml"),
    fx!("cloudformation", "ListImports", when ["acme-network"], "ListImports-network.xml"),
    // ── CloudTrail ───────────────────────────────────────────────────────────
    // `W` looks events up by ResourceName; the Event history tab asks for
    // everything. Insights are off in the demo trail.
    fx!("cloudtrail", "DescribeTrails", "DescribeTrails.json"),
    fx!("cloudtrail", "GetTrailStatus", "GetTrailStatus.json"),
    fx!("cloudtrail", "GetEventSelectors", "GetEventSelectors.json"),
    fx!("cloudtrail", "LookupEvents", when ["Insight"], "LookupEvents-none.json"),
    fx!("cloudtrail", "LookupEvents", when ["sg-0web0000000000001"], "LookupEvents-web-sg.json"),
    fx!("cloudtrail", "LookupEvents", when ["targetgroup/storefront-web"], "LookupEvents-storefront-tg.json"),
    fx!("cloudtrail", "LookupEvents", when ["orders-worker"], "LookupEvents-orders-worker.json"),
    fx!("cloudtrail", "LookupEvents", when ["github-actions-deploy"], "LookupEvents-github-actions-deploy.json"),
    fx!("cloudtrail", "LookupEvents", when ["vol-0orphan00000004"], "LookupEvents-orphan-volume.json"),
    fx!("cloudtrail", "LookupEvents", when ["ResourceName"], "LookupEvents-none.json"),
    fx!("cloudtrail", "LookupEvents", "LookupEvents.json"),
    // ── Elastic Beanstalk (awsQuery) ─────────────────────────────────────────
    // One app, two environments: `portal-legacy` (Standard, Red — its ALB is
    // the target-less `legacy-admin-alb`) and `portal-next` (Cluster Mode, on
    // a shared EKS cluster). Per-environment calls are keyed on the form
    // field the app sends: EnvironmentId for health/events/resources,
    // EnvironmentName for configuration.
    fx!("elasticbeanstalk", "DescribeEnvironments", "DescribeEnvironments.xml"),
    fx!("elasticbeanstalk", "DescribeApplications", "DescribeApplications.xml"),
    fx!("elasticbeanstalk", "DescribeApplicationVersions", "DescribeApplicationVersions.xml"),
    fx!("elasticbeanstalk", "ListTagsForResource", "ListTagsForResource-portal.xml"),
    fx!("elasticbeanstalk", "DescribeEnvironmentResources", when ["EnvironmentId=e-p0rtalstd1"], "DescribeEnvironmentResources-portal-legacy.xml"),
    fx!("elasticbeanstalk", "DescribeEnvironmentResources", when ["EnvironmentId=e-p0rtalcls1"], "DescribeEnvironmentResources-portal-next.xml"),
    fx!("elasticbeanstalk", "DescribeEnvironmentHealth", when ["EnvironmentId=e-p0rtalstd1"], "DescribeEnvironmentHealth-portal-legacy.xml"),
    fx!("elasticbeanstalk", "DescribeEnvironmentHealth", when ["EnvironmentId=e-p0rtalcls1"], "DescribeEnvironmentHealth-portal-next.xml"),
    fx!("elasticbeanstalk", "DescribeEvents", when ["EnvironmentId=e-p0rtalstd1"], "DescribeEvents-portal-legacy.xml"),
    fx!("elasticbeanstalk", "DescribeEvents", when ["EnvironmentId=e-p0rtalcls1"], "DescribeEvents-portal-next.xml"),
    fx!("elasticbeanstalk", "DescribeConfigurationSettings", when ["EnvironmentName=portal-legacy"], "DescribeConfigurationSettings-portal-legacy.xml"),
    fx!("elasticbeanstalk", "DescribeConfigurationSettings", when ["EnvironmentName=portal-next"], "DescribeConfigurationSettings-portal-next.xml"),

    // ── CloudWatch alarms (the `monitoring` endpoint, RPC v2 CBOR) ──────────
    // JSON here, encoded to CBOR on the way out (`cbor.rs`): timestamps are
    // `{"$timestamp": …}`, doubles need a decimal point. The timeline lens
    // finds alarms by dimension in the warm cache, then asks for each one's
    // history by name.
    fx!("monitoring", "DescribeAlarms", "DescribeAlarms.json"),
    fx!("monitoring", "DescribeAlarmHistory", when ["\"AlarmName\":\"orders-api-errors\""], "DescribeAlarmHistory-orders-api.json"),
    fx!("monitoring", "DescribeAlarmHistory", when ["\"AlarmName\":\"orders-worker-running-tasks\""], "DescribeAlarmHistory-orders-worker.json"),
    // ── CloudWatch Logs ──────────────────────────────────────────────────────
    // FilterLogEvents for the scripted groups is generated (see generate.rs).
    fx!("logs", "DescribeLogGroups", "DescribeLogGroups.json"),
    fx!("logs", "DescribeLogStreams", when ["/aws/lambda/orders-api"], "DescribeLogStreams-orders-api.json"),
    fx!("logs", "DescribeLogStreams", when ["/ecs/storefront-web"], "DescribeLogStreams-storefront-web.json"),
    fx!("logs", "DescribeLogStreams", when ["/ecs/orders-worker"], "DescribeLogStreams-orders-worker.json"),
    fx!("logs", "DescribeMetricFilters", when ["/aws/lambda/orders-api"], "DescribeMetricFilters-orders-api.json"),
    fx!("logs", "GetLogEvents", when ["5d1e0c7a"], "GetLogEvents-orders-worker-crash.json"),
    fx!("logs", "GetLogEvents", when ["6e2f1d8b"], "GetLogEvents-orders-worker-crash.json"),
    // ── Secrets Manager / SSM Parameter Store ───────────────────────────────
    // Metadata only: there is deliberately no GetSecretValue / GetParameter
    // fixture — `neboto get`'s guard test asserts neither is ever called.
    fx!("secretsmanager", "ListSecrets", "ListSecrets.json"),
    fx!("ssm", "DescribeParameters", "DescribeParameters.json"),
    // ── Cost Explorer (Cost Anomaly Detection) ──────────────────────────────
    // Only the Anomalies view (`8`): the spend view's daily CE series isn't
    // in the dataset yet.
    fx!("ce", "GetAnomalies", "GetAnomalies.json"),
    // ── S3 / Route 53 (restXml) ──────────────────────────────────────────────
    // Not in the dataset yet, but their list calls need a root element the
    // empty-success reply can't supply (it varies per operation).
    fx!("s3", "GET /", "ListBuckets.xml"),
    fx!("route53", "GET /2013-04-01/hostedzone", "ListHostedZones.xml"),
    fx!("route53", "GET /2013-04-01/healthcheck", "ListHealthChecks.xml"),
];

/// The body of the first fixture matching `service` / `operation` whose
/// `when` substrings all appear in `haystack` (the request URI + body).
pub fn find(service: &str, operation: &str, haystack: &str) -> Option<&'static str> {
    FIXTURES
        .iter()
        .find(|f| {
            f.service == service
                && op_matches(f.operation, operation)
                && f.when.iter().all(|w| haystack.contains(w))
        })
        .map(|f| f.body)
}

/// Exact match, except that a REST fixture's `*` path segment matches any
/// one segment: `GET /2015-03-31/functions/*` answers `GetFunction` for
/// every function name. URL-encoded names stay one segment.
fn op_matches(pattern: &str, operation: &str) -> bool {
    if !pattern.contains('*') {
        return pattern == operation;
    }
    let (p, o): (Vec<_>, Vec<_>) = (pattern.split('/').collect(), operation.split('/').collect());
    p.len() == o.len() && p.iter().zip(&o).all(|(p, o)| *p == "*" || p == o)
}
