use crate::aws::resource::{ActionResult, Resource, ResourceAction};
use crate::error::Result;
use crate::event::Event;
use async_trait::async_trait;
use tokio::sync::mpsc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum ServiceType {
    EC2,
    RDS,
    ECS,
    VPC,
    CloudTrail,
    S3,
    IdentityCenter,
    CloudFormation,
    Lambda,
    Route53,
    Acm,
    CloudWatch,
    IAM,
    Organizations,
    Elb,
    Asg,
    Messaging,
    Secrets,
    Cost,
    CloudFront,
    Kms,
    Config,
    Waf,
    TransitGateway,
    TrustedAdvisor,
    DirectConnect,
    GlobalAccelerator,
    ApiGateway,
    ServiceQuotas,
    DynamoDb,
    Eks,
    Efs,
    EventBridge,
    GuardDuty,
    SecurityHub,
    Cognito,
    Inspector,
    Ecr,
    Backup,
    NetworkFirewall,
    StepFunctions,
    Code,
    Fsx,
    Health,
    ResourceExplorer,
    ResourceGroups,
    Ram,
    Transfer,
    Route53Resolver,
    Route53Profiles,
    Workspaces,
    ElastiCache,
    OpenSearch,
    Kinesis,
    Bedrock,
    AgentCore,
    Ssm,
    Athena,
    Glue,
    Ses,
    Msk,
    Fms,
    Redshift,
    Budgets,
    Invoices,
    ControlTower,
    ServiceCatalog,
    S3Files,
    S3Tables,
}

impl ServiceType {
    #[allow(dead_code)]
    pub fn all() -> Vec<ServiceType> {
        vec![
            ServiceType::EC2,
            ServiceType::RDS,
            ServiceType::ECS,
            ServiceType::VPC,
            ServiceType::CloudTrail,
            ServiceType::S3,
            ServiceType::IdentityCenter,
            ServiceType::CloudFormation,
            ServiceType::Lambda,
            ServiceType::Route53,
            ServiceType::Acm,
            ServiceType::CloudWatch,
            ServiceType::IAM,
            ServiceType::Organizations,
            ServiceType::Elb,
            ServiceType::Asg,
            ServiceType::Messaging,
            ServiceType::Secrets,
            ServiceType::Cost,
            ServiceType::CloudFront,
            ServiceType::Kms,
            ServiceType::Config,
            ServiceType::Waf,
            ServiceType::TransitGateway,
            ServiceType::TrustedAdvisor,
            ServiceType::DirectConnect,
            ServiceType::GlobalAccelerator,
            ServiceType::ApiGateway,
            ServiceType::ServiceQuotas,
            ServiceType::DynamoDb,
            ServiceType::Eks,
            ServiceType::Efs,
            ServiceType::EventBridge,
            ServiceType::GuardDuty,
            ServiceType::SecurityHub,
            ServiceType::Cognito,
            ServiceType::Inspector,
            ServiceType::Ecr,
            ServiceType::Backup,
            ServiceType::NetworkFirewall,
            ServiceType::StepFunctions,
            ServiceType::Code,
            ServiceType::Fsx,
            ServiceType::Health,
            ServiceType::ResourceExplorer,
            ServiceType::ResourceGroups,
            ServiceType::Ram,
            ServiceType::Transfer,
            ServiceType::Route53Resolver,
            ServiceType::Route53Profiles,
            ServiceType::Workspaces,
            ServiceType::ElastiCache,
            ServiceType::OpenSearch,
            ServiceType::Kinesis,
            ServiceType::Bedrock,
            ServiceType::AgentCore,
            ServiceType::Ssm,
            ServiceType::Athena,
            ServiceType::Glue,
            ServiceType::Ses,
            ServiceType::Msk,
            ServiceType::Fms,
            ServiceType::Redshift,
            ServiceType::Budgets,
            ServiceType::Invoices,
            ServiceType::ControlTower,
            ServiceType::ServiceCatalog,
            ServiceType::S3Files,
            ServiceType::S3Tables,
        ]
    }

    /// Whether this service's data is account-global rather than region-scoped.
    /// Used to key the resource cache on a fixed sentinel region instead of
    /// the currently-selected region, so switching regions doesn't force a
    /// refetch of identical data.
    pub fn is_global(&self) -> bool {
        matches!(
            self,
            ServiceType::IAM
                | ServiceType::Organizations
                | ServiceType::Cost
                | ServiceType::CloudFront
                | ServiceType::TrustedAdvisor
                | ServiceType::GlobalAccelerator
                | ServiceType::Health
                | ServiceType::ResourceExplorer
                | ServiceType::Budgets
                | ServiceType::Invoices
        )
    }

    /// Category display order for grouped listings (the `S` service picker).
    /// Every `category()` value must appear here — the picker only renders
    /// categories from this list.
    pub const CATEGORIES: &'static [&'static str] = &[
        "Compute",
        "Containers",
        "Storage",
        "Database",
        "Networking & Content Delivery",
        "Security & Identity",
        "Analytics",
        "ML & AI",
        "App Integration",
        "Management & Governance",
        "Developer Tools",
        "Cost Management",
    ];

    /// Console-style category, used to group the service picker. Keep values
    /// in sync with `CATEGORIES`.
    pub fn category(&self) -> &'static str {
        match self {
            ServiceType::EC2
            | ServiceType::Lambda
            | ServiceType::Asg
            | ServiceType::Workspaces => "Compute",
            ServiceType::ECS | ServiceType::Eks | ServiceType::Ecr => "Containers",
            ServiceType::S3
            | ServiceType::S3Files
            | ServiceType::S3Tables
            | ServiceType::Efs
            | ServiceType::Fsx
            | ServiceType::Backup
            | ServiceType::Transfer => "Storage",
            ServiceType::RDS | ServiceType::DynamoDb | ServiceType::ElastiCache => "Database",
            ServiceType::VPC
            | ServiceType::Elb
            | ServiceType::Route53
            | ServiceType::Route53Resolver
            | ServiceType::Route53Profiles
            | ServiceType::CloudFront
            | ServiceType::TransitGateway
            | ServiceType::DirectConnect
            | ServiceType::GlobalAccelerator
            | ServiceType::ApiGateway => "Networking & Content Delivery",
            ServiceType::IAM
            | ServiceType::IdentityCenter
            | ServiceType::Cognito
            | ServiceType::Kms
            | ServiceType::Secrets
            | ServiceType::Acm
            | ServiceType::Waf
            | ServiceType::NetworkFirewall
            | ServiceType::GuardDuty
            | ServiceType::SecurityHub
            | ServiceType::Inspector
            | ServiceType::Fms => "Security & Identity",
            ServiceType::Athena
            | ServiceType::Glue
            | ServiceType::Kinesis
            | ServiceType::Msk
            | ServiceType::Redshift
            | ServiceType::OpenSearch => "Analytics",
            ServiceType::Bedrock | ServiceType::AgentCore => "ML & AI",
            ServiceType::Messaging
            | ServiceType::EventBridge
            | ServiceType::StepFunctions
            | ServiceType::Ses => "App Integration",
            ServiceType::CloudFormation
            | ServiceType::CloudWatch
            | ServiceType::CloudTrail
            | ServiceType::Config
            | ServiceType::Ssm
            | ServiceType::Organizations
            | ServiceType::TrustedAdvisor
            | ServiceType::Health
            | ServiceType::ServiceQuotas
            | ServiceType::ResourceExplorer
            | ServiceType::ResourceGroups
            | ServiceType::Ram
            | ServiceType::ControlTower
            | ServiceType::ServiceCatalog => "Management & Governance",
            ServiceType::Code => "Developer Tools",
            ServiceType::Cost | ServiceType::Budgets | ServiceType::Invoices => "Cost Management",
        }
    }

    pub fn name(&self) -> &str {
        match self {
            ServiceType::EC2 => "EC2",
            ServiceType::RDS => "RDS",
            ServiceType::ECS => "ECS",
            ServiceType::VPC => "VPC",
            ServiceType::CloudTrail => "CloudTrail",
            ServiceType::S3 => "S3",
            ServiceType::IdentityCenter => "IAM IDC",
            ServiceType::CloudFormation => "CFN",
            ServiceType::Lambda => "Lambda",
            ServiceType::Route53 => "Route53",
            ServiceType::Acm => "ACM",
            ServiceType::CloudWatch => "CloudWatch",
            ServiceType::IAM => "IAM",
            ServiceType::Organizations => "Organizations",
            ServiceType::Elb => "ELB",
            ServiceType::Asg => "Auto Scaling",
            ServiceType::Messaging => "SQS/SNS",
            ServiceType::Secrets => "Secrets Manager",
            ServiceType::Ssm => "Systems Manager",
            ServiceType::Cost => "Cost",
            ServiceType::CloudFront => "CloudFront",
            ServiceType::Kms => "KMS",
            ServiceType::Config => "AWS Config",
            ServiceType::Waf => "WAF",
            ServiceType::TransitGateway => "Transit Gateway",
            ServiceType::TrustedAdvisor => "Trusted Advisor",
            ServiceType::DirectConnect => "Direct Connect",
            ServiceType::GlobalAccelerator => "Global Accelerator",
            ServiceType::ApiGateway => "API Gateway",
            ServiceType::ServiceQuotas => "Service Quotas",
            ServiceType::DynamoDb => "DynamoDB",
            ServiceType::Eks => "EKS",
            ServiceType::Efs => "EFS",
            ServiceType::EventBridge => "EventBridge",
            ServiceType::GuardDuty => "GuardDuty",
            ServiceType::SecurityHub => "Security Hub",
            ServiceType::Cognito => "Cognito",
            ServiceType::Inspector => "Inspector",
            ServiceType::Ecr => "ECR",
            ServiceType::Backup => "AWS Backup",
            ServiceType::NetworkFirewall => "Network Firewall",
            ServiceType::StepFunctions => "Step Functions",
            ServiceType::Code => "CodeSuite",
            ServiceType::Fsx => "FSx",
            ServiceType::Health => "AWS Health",
            ServiceType::ResourceExplorer => "Resource Explorer",
            ServiceType::ResourceGroups => "Resource Groups",
            ServiceType::Ram => "Resource Access Manager",
            ServiceType::Transfer => "Transfer Family",
            ServiceType::Route53Resolver => "Route53 Resolver",
            ServiceType::Route53Profiles => "Route53 Profiles",
            ServiceType::Workspaces => "WorkSpaces",
            ServiceType::ElastiCache => "ElastiCache",
            ServiceType::OpenSearch => "OpenSearch",
            ServiceType::Kinesis => "Kinesis",
            ServiceType::Bedrock => "Bedrock",
            ServiceType::AgentCore => "Bedrock AgentCore",
            ServiceType::Athena => "Athena",
            ServiceType::Glue => "Glue",
            ServiceType::Ses => "SES",
            ServiceType::Msk => "MSK",
            ServiceType::Fms => "Firewall Manager",
            ServiceType::Redshift => "Redshift",
            ServiceType::Budgets => "Budgets",
            ServiceType::Invoices => "Invoices",
            ServiceType::ControlTower => "Control Tower",
            ServiceType::ServiceCatalog => "Service Catalog",
            ServiceType::S3Files => "S3 Files",
            ServiceType::S3Tables => "S3 Tables",
        }
    }

    /// Abbreviated name for the compact tab bar.
    pub fn short_name(&self) -> &str {
        match self {
            ServiceType::EC2 => "EC2",
            ServiceType::RDS => "RDS",
            ServiceType::ECS => "ECS",
            ServiceType::VPC => "VPC",
            ServiceType::CloudTrail => "CT",
            ServiceType::S3 => "S3",
            ServiceType::IdentityCenter => "IDC",
            ServiceType::CloudFormation => "CFN",
            ServiceType::Lambda => "λ",
            ServiceType::Route53 => "R53",
            ServiceType::Acm => "ACM",
            ServiceType::CloudWatch => "CW",
            ServiceType::IAM => "IAM",
            ServiceType::Organizations => "Org",
            ServiceType::Elb => "ELB",
            ServiceType::Asg => "ASG",
            ServiceType::Messaging => "MSG",
            ServiceType::Secrets => "SEC",
            ServiceType::Ssm => "SSM",
            ServiceType::Cost => "$",
            ServiceType::CloudFront => "CF",
            ServiceType::Kms => "KMS",
            ServiceType::Config => "CFG",
            ServiceType::Waf => "WAF",
            ServiceType::TransitGateway => "TGW",
            ServiceType::TrustedAdvisor => "TA",
            ServiceType::DirectConnect => "DX",
            ServiceType::GlobalAccelerator => "GA",
            ServiceType::ApiGateway => "APIG",
            ServiceType::ServiceQuotas => "SQ",
            ServiceType::DynamoDb => "DDB",
            ServiceType::Eks => "EKS",
            ServiceType::Efs => "EFS",
            ServiceType::EventBridge => "EVB",
            ServiceType::GuardDuty => "GD",
            ServiceType::SecurityHub => "SH",
            ServiceType::Cognito => "COG",
            ServiceType::Inspector => "INSP",
            ServiceType::Ecr => "ECR",
            ServiceType::Backup => "BAK",
            ServiceType::NetworkFirewall => "ANFW",
            ServiceType::StepFunctions => "SFN",
            ServiceType::Code => "Code",
            ServiceType::Fsx => "FSX",
            ServiceType::Health => "HLTH",
            ServiceType::ResourceExplorer => "EXPL",
            ServiceType::ResourceGroups => "RG",
            ServiceType::Ram => "RAM",
            ServiceType::Transfer => "TFR",
            ServiceType::Route53Resolver => "R53R",
            ServiceType::Route53Profiles => "R53P",
            ServiceType::Workspaces => "WKS",
            ServiceType::ElastiCache => "ELC",
            ServiceType::OpenSearch => "OS",
            ServiceType::Kinesis => "KDS",
            ServiceType::Bedrock => "BR",
            ServiceType::AgentCore => "ACR",
            ServiceType::Athena => "ATH",
            ServiceType::Glue => "GLU",
            ServiceType::Ses => "SES",
            ServiceType::Msk => "MSK",
            ServiceType::Fms => "FMS",
            ServiceType::Redshift => "RSH",
            ServiceType::Budgets => "BUD",
            ServiceType::Invoices => "INV",
            ServiceType::ControlTower => "CTW",
            ServiceType::ServiceCatalog => "SC",
            ServiceType::S3Files => "S3F",
            ServiceType::S3Tables => "S3T",
        }
    }

    pub fn description(&self) -> &str {
        match self {
            ServiceType::EC2 => "EC2 Instances",
            ServiceType::RDS => "RDS Databases",
            ServiceType::ECS => "ECS Clusters & Tasks",
            ServiceType::VPC => "VPC Resources",
            ServiceType::CloudTrail => "CloudTrail Events",
            ServiceType::S3 => "S3 Buckets",
            ServiceType::IdentityCenter => "IAM Identity Center",
            ServiceType::CloudFormation => "CloudFormation Stacks",
            ServiceType::Lambda => "Lambda Functions",
            ServiceType::Route53 => "Route53 Hosted Zones",
            ServiceType::Acm => "ACM Certificates",
            ServiceType::CloudWatch => "CloudWatch Alarms & Logs",
            ServiceType::IAM => "IAM Roles, Policies, Users & Groups",
            ServiceType::Organizations => "Organizations Accounts, OUs & SCPs",
            ServiceType::Elb => "Load Balancers & Target Groups",
            ServiceType::Asg => "Auto Scaling Groups",
            ServiceType::Messaging => "SQS Queues & SNS Topics",
            ServiceType::Secrets => "Secrets Manager",
            ServiceType::Ssm => "Parameters, Documents, Fleet, Patch",
            ServiceType::Cost => "Cost & Billing",
            ServiceType::CloudFront => "CloudFront Distributions",
            ServiceType::Kms => "KMS Keys",
            ServiceType::Config => "Config Rules & Compliance",
            ServiceType::Waf => "WAF Web ACLs, IP Sets & Rule Groups",
            ServiceType::TransitGateway => "Transit Gateways, Attachments & Route Tables",
            ServiceType::TrustedAdvisor => "Trusted Advisor Checks",
            ServiceType::DirectConnect => "Connections, Virtual Interfaces, LAGs & Gateways",
            ServiceType::GlobalAccelerator => "Global Accelerators",
            ServiceType::ApiGateway => "REST & HTTP APIs, Domains, Usage Plans & VPC Links",
            ServiceType::ServiceQuotas => "Service Quotas & Limits",
            ServiceType::DynamoDb => "DynamoDB Tables",
            ServiceType::Eks => "EKS Clusters",
            ServiceType::Efs => "EFS File Systems",
            ServiceType::EventBridge => "EventBridge Rules & Buses",
            ServiceType::GuardDuty => "GuardDuty Findings & Detectors",
            ServiceType::SecurityHub => "Security Hub Findings, Standards & Insights",
            ServiceType::Cognito => "Cognito User & Identity Pools",
            ServiceType::Inspector => "Inspector Vulnerability Findings",
            ServiceType::Ecr => "Container Registry",
            ServiceType::Backup => "Backup Vaults, Plans, Resources & Jobs",
            ServiceType::NetworkFirewall => "Firewalls, Policies & Rule Groups",
            ServiceType::StepFunctions => "Step Functions State Machines",
            ServiceType::Code => "CodeCommit, CodeBuild, CodePipeline",
            ServiceType::Fsx => "FSx File Systems",
            ServiceType::Health => "AWS Health / Personal Health Dashboard",
            ServiceType::ResourceExplorer => "Cross-region resource search (Global View)",
            ServiceType::ResourceGroups => "Resource Groups",
            ServiceType::Ram => "Resource Shares",
            ServiceType::Transfer => "Transfer Servers",
            ServiceType::Route53Resolver => "Route53 Resolver Endpoints & Rules",
            ServiceType::Route53Profiles => "Route53 Profiles — DNS config shared across VPCs",
            ServiceType::Workspaces => "WorkSpaces",
            ServiceType::ElastiCache => "Redis / Valkey / Memcached Caches",
            ServiceType::OpenSearch => "OpenSearch / Elasticsearch Domains",
            ServiceType::Kinesis => "Kinesis Data Streams",
            ServiceType::Bedrock => {
                "Foundation Models, Inference Profiles, Guardrails, Knowledge Bases"
            }
            ServiceType::AgentCore => "Agent Runtimes, Gateways, Memory, Identity, Built-in Tools",
            ServiceType::Athena => "Workgroups, Catalogs, Databases, Queries",
            ServiceType::Glue => "Databases, Tables, Crawlers, Jobs, Job Runs",
            ServiceType::Ses => "Identities, Configuration Sets, Suppression List",
            ServiceType::Msk => "Kafka clusters (provisioned + serverless)",
            ServiceType::Fms => "Firewall Manager Policies, App/Protocol Lists, Resource Sets",
            ServiceType::Redshift => "Clusters, Serverless Workgroups, Snapshots",
            ServiceType::Budgets => "Cost & Usage Budgets, Alerts, Subscribers",
            ServiceType::Invoices => "Invoice Summaries & PDF Download",
            ServiceType::ControlTower => "Landing Zone, Enabled Controls, Baselines & Operations",
            ServiceType::ServiceCatalog => "Portfolios, Products, Provisioned Products & TagOptions",
            ServiceType::S3Files => "S3 file systems, mount targets & access points",
            ServiceType::S3Tables => "Table buckets, Iceberg tables, namespaces & maintenance",
        }
    }

    /// Parse a service prefix string to ServiceType
    /// Supports multiple aliases for each service
    pub fn from_prefix(prefix: &str) -> Option<ServiceType> {
        match prefix.to_lowercase().as_str() {
            "ec2" | "instances" => Some(ServiceType::EC2),
            "vpc" | "networks" | "network" => Some(ServiceType::VPC),
            "rds" | "databases" | "db" => Some(ServiceType::RDS),
            "ecs" | "containers" | "clusters" => Some(ServiceType::ECS),
            "cloudtrail" | "ct" | "trail" => Some(ServiceType::CloudTrail),
            "s3" | "buckets" | "bucket" => Some(ServiceType::S3),
            "idc" | "sso" | "ic" | "identitycenter" => Some(ServiceType::IdentityCenter),
            "cfn" | "cloudformation" | "cf" | "stacks" => Some(ServiceType::CloudFormation),
            "lambda" | "fn" | "functions" => Some(ServiceType::Lambda),
            "r53" | "route53" | "dns" | "zones" => Some(ServiceType::Route53),
            "acm" | "certs" | "certificates" | "tls" | "ssl" => Some(ServiceType::Acm),
            "cw" | "cloudwatch" | "alarms" | "logs" => Some(ServiceType::CloudWatch),
            "iam" | "roles" | "policies" => Some(ServiceType::IAM),
            "orgs" | "organizations" | "org" | "accounts" | "ous" | "scps" => {
                Some(ServiceType::Organizations)
            }
            "elb" | "lb" | "alb" | "nlb" | "loadbalancers" | "targetgroups" | "tg" => {
                Some(ServiceType::Elb)
            }
            "asg" | "autoscaling" | "scaling" | "scalinggroups" => Some(ServiceType::Asg),
            "sqs" | "sns" | "msg" | "messaging" | "queues" | "topics" => {
                Some(ServiceType::Messaging)
            }
            "secrets" | "secret" | "sm" | "secretsmanager" => Some(ServiceType::Secrets),
            "ssm" | "params" | "parameters" | "param" | "paramstore" | "parameterstore"
            | "systemsmanager" | "documents" | "ssmdoc" | "ssmdocs" | "fleet"
            | "managedinstances" | "patch" | "patchmanager" => Some(ServiceType::Ssm),
            "cost" | "costs" | "billing" | "bill" | "ce" | "spend" => Some(ServiceType::Cost),
            "cloudfront" | "cdn" | "distributions" => Some(ServiceType::CloudFront),
            "kms" | "keys" | "key" | "encryption" => Some(ServiceType::Kms),
            "config" | "awsconfig" | "compliance" | "rules" => Some(ServiceType::Config),
            "waf" | "acl" | "webacl" | "wafv2" => Some(ServiceType::Waf),
            "tgw" | "transit" | "transitgateway" | "transitgateways" => {
                Some(ServiceType::TransitGateway)
            }
            "ta" | "trustedadvisor" | "advisor" | "ta-checks" => Some(ServiceType::TrustedAdvisor),
            "dx" | "directconnect" | "dc" | "directconnections" => {
                Some(ServiceType::DirectConnect)
            }
            "ga" | "globalaccelerator" | "accelerator" | "accelerators" => {
                Some(ServiceType::GlobalAccelerator)
            }
            "apigw" | "apigateway" | "api" | "apis" => Some(ServiceType::ApiGateway),
            "quotas" | "limits" | "sq" | "servicequotas" => Some(ServiceType::ServiceQuotas),
            "ddb" | "dynamodb" | "dynamo" => Some(ServiceType::DynamoDb),
            "eks" | "kubernetes" | "k8s" => Some(ServiceType::Eks),
            "efs" | "filesystem" | "filesystems" | "nfs" => Some(ServiceType::Efs),
            "events" | "eventbridge" | "eb" => Some(ServiceType::EventBridge),
            "gd" | "guardduty" | "threats" => Some(ServiceType::GuardDuty),
            "sh" | "securityhub" | "shub" => Some(ServiceType::SecurityHub),
            "cognito" | "userpool" | "userpools" | "idp" => Some(ServiceType::Cognito),
            "inspector" | "vulns" | "cve" | "inspector2" => Some(ServiceType::Inspector),
            "ecr" | "registry" | "images" => Some(ServiceType::Ecr),
            "backup" | "bak" | "awsbackup" => Some(ServiceType::Backup),
            "anfw" | "networkfirewall" | "firewall" => Some(ServiceType::NetworkFirewall),
            "sfn" | "stepfunctions" | "states" => Some(ServiceType::StepFunctions),
            "code" | "codecommit" | "codebuild" | "codepipeline" | "pipeline" | "pipelines" => {
                Some(ServiceType::Code)
            }
            "fsx" | "lustre" | "ontap" | "openzfs" => Some(ServiceType::Fsx),
            "health" | "phd" | "healthevents" => Some(ServiceType::Health),
            "explorer" | "global" | "search" | "re" => Some(ServiceType::ResourceExplorer),
            "rg" | "resourcegroups" | "groups" => Some(ServiceType::ResourceGroups),
            "ram" | "share" | "resourceaccess" => Some(ServiceType::Ram),
            "transfer" | "sftp" | "ftp" | "ftps" => Some(ServiceType::Transfer),
            "resolver" | "r53r" | "r53resolver" => Some(ServiceType::Route53Resolver),
            "profiles" | "r53profiles" | "dnsprofiles" | "profile" => {
                Some(ServiceType::Route53Profiles)
            }
            "workspaces" | "wks" | "wsp" => Some(ServiceType::Workspaces),
            "elasticache" | "redis" | "valkey" | "memcached" | "cache" | "elc" => {
                Some(ServiceType::ElastiCache)
            }
            "opensearch" | "os" | "elasticsearch" | "es" | "search-domain" => {
                Some(ServiceType::OpenSearch)
            }
            "kinesis" | "kds" | "streams" | "datastream" | "datastreams" => {
                Some(ServiceType::Kinesis)
            }
            "bedrock" | "br" | "genai" | "llm" | "models" | "foundationmodels" | "guardrails"
            | "knowledgebases" | "kb" => Some(ServiceType::Bedrock),
            // AgentCore is a separate service from Bedrock — see the naming
            // gotcha in CLAUDE.md. `agents` deliberately lands here, not on
            // Bedrock Agents: AgentCore is where hosted agents live now.
            "agentcore" | "ac" | "bac" | "bedrock-agentcore" | "bedrockagentcore" | "runtimes"
            | "agents" => Some(ServiceType::AgentCore),
            "athena" | "ath" | "presto" | "trino" | "workgroups" | "queries" => {
                Some(ServiceType::Athena)
            }
            "glue" | "etl" | "crawler" | "crawlers" | "datacatalog" => Some(ServiceType::Glue),
            "ses" | "sesv2" | "email" | "identities" | "suppression" => Some(ServiceType::Ses),
            "msk" | "kafka" | "managedkafka" => Some(ServiceType::Msk),
            "fms" | "firewallmanager" | "fwmanager" => Some(ServiceType::Fms),
            "redshift" | "rs" | "warehouse" | "datawarehouse" | "dwh" => {
                Some(ServiceType::Redshift)
            }
            "budgets" | "budget" | "bud" => Some(ServiceType::Budgets),
            "invoices" | "invoice" | "inv" | "invoicing" => Some(ServiceType::Invoices),
            "controltower" | "tower" | "landingzone" | "lz" => Some(ServiceType::ControlTower),
            "sc" | "catalog" | "servicecatalog" => Some(ServiceType::ServiceCatalog),
            "s3files" | "s3f" | "files" => Some(ServiceType::S3Files),
            "s3tables" | "s3t" | "tables" => Some(ServiceType::S3Tables),
            _ => None,
        }
    }

    /// Get the canonical prefix for this service (with @ symbol)
    #[allow(dead_code)]
    pub fn prefix(&self) -> &str {
        match self {
            ServiceType::EC2 => "@ec2",
            ServiceType::VPC => "@vpc",
            ServiceType::RDS => "@rds",
            ServiceType::ECS => "@ecs",
            ServiceType::CloudTrail => "@cloudtrail",
            ServiceType::S3 => "@s3",
            ServiceType::IdentityCenter => "@idc",
            ServiceType::CloudFormation => "@cfn",
            ServiceType::Lambda => "@lambda",
            ServiceType::Route53 => "@r53",
            ServiceType::Acm => "@acm",
            ServiceType::CloudWatch => "@cw",
            ServiceType::IAM => "@iam",
            ServiceType::Organizations => "@orgs",
            ServiceType::Elb => "@elb",
            ServiceType::Asg => "@asg",
            ServiceType::Messaging => "@sqs",
            ServiceType::Secrets => "@secrets",
            ServiceType::Ssm => "@ssm",
            ServiceType::Cost => "@cost",
            ServiceType::CloudFront => "@cloudfront",
            ServiceType::Kms => "@kms",
            ServiceType::Config => "@config",
            ServiceType::Waf => "@waf",
            ServiceType::TransitGateway => "@tgw",
            ServiceType::TrustedAdvisor => "@ta",
            ServiceType::DirectConnect => "@dx",
            ServiceType::GlobalAccelerator => "@ga",
            ServiceType::ApiGateway => "@apigw",
            ServiceType::ServiceQuotas => "@quotas",
            ServiceType::DynamoDb => "@ddb",
            ServiceType::Eks => "@eks",
            ServiceType::Efs => "@efs",
            ServiceType::EventBridge => "@events",
            ServiceType::GuardDuty => "@gd",
            ServiceType::SecurityHub => "@sh",
            ServiceType::Cognito => "@cognito",
            ServiceType::Inspector => "@inspector",
            ServiceType::Ecr => "@ecr",
            ServiceType::Backup => "@backup",
            ServiceType::NetworkFirewall => "@anfw",
            ServiceType::StepFunctions => "@sfn",
            ServiceType::Code => "@code",
            ServiceType::Fsx => "@fsx",
            ServiceType::Health => "@health",
            ServiceType::ResourceExplorer => "@explorer",
            ServiceType::ResourceGroups => "@resourcegroups",
            ServiceType::Ram => "@ram",
            ServiceType::Transfer => "@transfer",
            ServiceType::ElastiCache => "@elasticache",
            ServiceType::OpenSearch => "@opensearch",
            ServiceType::Kinesis => "@kinesis",
            ServiceType::Route53Resolver => "@resolver",
            ServiceType::Route53Profiles => "@profiles",
            ServiceType::Workspaces => "@workspaces",
            ServiceType::Bedrock => "@bedrock",
            ServiceType::AgentCore => "@agentcore",
            ServiceType::Athena => "@athena",
            ServiceType::Glue => "@glue",
            ServiceType::Ses => "@ses",
            ServiceType::Msk => "@msk",
            ServiceType::Fms => "@fms",
            ServiceType::Redshift => "@redshift",
            ServiceType::Budgets => "@budgets",
            ServiceType::Invoices => "@invoices",
            ServiceType::ControlTower => "@controltower",
            ServiceType::ServiceCatalog => "@sc",
            ServiceType::S3Files => "@s3files",
            ServiceType::S3Tables => "@s3tables",
        }
    }
}

impl std::fmt::Display for ServiceType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}

#[async_trait]
pub trait AwsService: Send + Sync {
    /// Service identifier
    #[allow(dead_code)]
    fn service_type(&self) -> ServiceType;

    /// Human-readable name
    #[allow(dead_code)]
    fn name(&self) -> &str;

    /// List all resources for this service
    async fn list_resources(&self) -> Result<Vec<Box<dyn Resource>>>;

    /// List resources with incremental updates via events
    /// Default implementation falls back to list_resources() for backward compatibility
    async fn list_resources_streaming(
        &self,
        event_tx: mpsc::UnboundedSender<Event>,
        service_type: ServiceType,
    ) -> Result<()> {
        // Default implementation: load all at once and send as single batch
        match self.list_resources().await {
            Ok(resources) => {
                let _ = event_tx.send(Event::ResourcesLoaded {
                    service: service_type,
                    resources,
                });
                Ok(())
            }
            Err(e) => {
                let _ = event_tx.send(Event::ResourceLoadError {
                    service: service_type,
                    error: e.to_string(),
                });
                Err(e)
            }
        }
    }

    /// Get detailed info for a specific resource
    #[allow(dead_code)]
    async fn get_resource_details(&self, id: &str) -> Result<Box<dyn Resource>>;

    /// Future: Execute action on resource
    #[allow(dead_code)]
    async fn execute_action(
        &self,
        _resource_id: &str,
        _action: ResourceAction,
    ) -> Result<ActionResult> {
        Err(crate::error::Error::NotImplemented)
    }
}
