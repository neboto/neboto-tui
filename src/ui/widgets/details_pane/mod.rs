use crate::app::App;
use crate::aws::resource::Resource;
use crate::aws::services::transit_gateway::{TgwRouteTable, TransitGateway};
use crate::aws::services::vpc::{VpcEndpoint, VpnConnection};
use crate::aws::services::acm::AcmCertificate;
use crate::aws::services::step_functions::{execution_status_state, SfnExecution, SfnStateMachine};
use crate::aws::services::cloudformation::{CfnExport, CfnStack, CfnStackSet};
use crate::aws::services::cloudfront::{CfDistribution, CfInvalidationsData};
use crate::aws::services::kms::{KmsGrant, KmsKey};
use crate::aws::services::bedrock::{
    AgentActionGroup, AgentAlias, AgentFull, AgentKb, BedrockAgent, BedrockGuardrail,
    BedrockKnowledgeBase, GuardrailFull, KbDataSource, KbIngestionJob, KnowledgeBaseFull,
};
use crate::aws::services::resource_groups::ResourceGroup as RgGroup;
use crate::aws::services::trusted_advisor::TaCheck;
use crate::aws::services::dynamodb::DdbTable;
use crate::aws::services::direct_connect::{
    DxConnection, DxGateway, DxGwAssociation, DxGwAttachment, DxVirtualInterface,
};
use crate::aws::services::global_accelerator::GaAccelerator;
use crate::aws::services::eks::EksCluster;
use crate::aws::services::efs::EfsFileSystem;
use crate::aws::services::elasticache::{CacheKind, ElastiCacheCluster};
use crate::aws::services::opensearch::OpenSearchDomain;
use crate::aws::services::msk::MskCluster;
use crate::aws::services::budgets::BudgetItem;
use crate::aws::services::controltower::{
    CtAccount, CtCompliance, CtComplianceResource, CtEnabledBaselineDetail,
    CtEnabledControlDetail, CtOp, EnabledBaseline, EnabledControl, LandingZone,
};
use crate::aws::services::invoicing::Invoice;
use crate::aws::services::kinesis::{FirehoseStream, KinesisStream};
use crate::aws::services::redshift::{RedshiftCluster, RedshiftWorkgroup};
use crate::aws::services::athena::{
    fmt_millis, AthenaDatabase, AthenaNamedQuery, AthenaQueryExecution, AthenaWorkgroup,
};
use crate::aws::services::glue::{GlueCrawler, GlueJob, GlueJobRun, GlueTable};
use crate::aws::services::fms::{
    FmsAdminInfo, FmsComplianceDetail, FmsPolicy, FmsResourceSet, FmsResourceSetDetail,
};
use crate::aws::services::ses::{SesAccountInfo, SesConfigSet, SesIdentity};
use crate::aws::services::transfer::TransferServer;
use crate::aws::services::eventbridge::{EbEventBus, EbPipe, EbRule, EbSchedule};
use crate::aws::services::network_firewall::{NfwFirewall, NfwPolicy, NfwRuleGroup};
use crate::aws::services::guardduty::GdFinding;
use crate::aws::services::security_hub::{ShControl, ShFinding, ShOverview};
use crate::aws::services::ecr::EcrRepository;
use crate::aws::services::cognito::CognitoUserPool;
use crate::aws::services::inspector::InspFinding;
use crate::aws::services::backup::{BackupPlan, BackupVault};
use crate::aws::services::servicecatalog::{ScPortfolio, ScProduct, ScProvisionedProduct};
use crate::aws::services::api_gateway::{
    ApiCustomDomain, ApiUsagePlan, HttpApi, RestApi, UsagePlanKeyInfo,
};
use crate::aws::services::awsconfig::ConfigRule;
use crate::aws::services::waf::{WafInsights, WafMetricsState, WafWebAcl};
use crate::aws::services::cloudwatch::{CwAlarm, CwCompositeAlarm, CwDashboard, CwLogGroup};
use crate::aws::services::ec2::{Ec2Instance, NetworkInterface};
use crate::aws::services::ecs::{EcsCluster, EcsServiceInfo, EcsTask, EcsTaskDefinition};
use crate::aws::services::iam::{AccessAnalyzerFinding, IamAccountSettings, IamGroup, IamIdentityProvider, IamPolicy, IamRole, IamUser};
use crate::aws::services::lambda::LambdaFunction;
use crate::aws::services::organizations::{OrgAccount, OrgScp, OrgUnit};
use crate::aws::services::asg::AsgGroup;
use crate::aws::services::elb::{LoadBalancer, TargetGroup};
use crate::aws::services::config::SecretEntry;
use crate::aws::services::ssm::{
    SsmAssocDetail, SsmAssociation, SsmAutomationExecution, SsmAutomationStep,
    SsmBaselineDetail, SsmCmdInvocation, SsmCommand, SsmDocContent, SsmDocument,
    SsmInstanceAssoc, SsmInstancePatches, SsmInventoryData, SsmMaintWindow,
    SsmMaintWindowDetail, SsmManagedInstance, SsmOpsItem, SsmOpsItemDetail, SsmParamDetail,
    SsmParameter, SsmPatchBaseline, MAX_INSTANCE_PATCH_ROWS,
};
use crate::aws::services::cost::CostLineItem;
use crate::aws::services::messaging::{SnsTopic, SqsQueue};
use crate::aws::services::rds::{RdsCluster, RdsInstance};
use crate::aws::services::route53::R53HostedZone;
use crate::aws::services::route53::R53HealthCheck;

use crate::aws::services::direct_connect::DxLagDetailSection;
use crate::aws::services::bedrock::{BedrockAgentDetailSection, BedrockGuardrailDetailSection, BedrockKbDetailSection};
use crate::aws::services::code::{CodeArtifactRepoDetailSection, CodeBuildDetailSection, CodeCommitRepoDetailSection, CodeDeployGroupDetailSection, CodePipelineDetailSection};
use crate::aws::services::cost::CostDetailSection;
use crate::aws::services::ecr::EcrRepoDetailSection;
use crate::aws::services::ecs::{EcsClusterDetailSection, EcsServiceDetailSection, EcsTaskDefDetailSection, EcsTaskDetailSection};
use crate::aws::services::health::HealthEventDetailSection;
use crate::aws::services::asg::AsgGroupDetailSection;
use crate::aws::services::config::SecretDetailSection;
use crate::aws::services::elb::{LoadBalancerDetailSection, TargetGroupDetailSection};
use crate::aws::services::iam::{AccessAnalyzerDetailSection, IamAccountDetailSection, IamGroupDetailSection, IamIdpDetailSection, IamPolicyDetailSection, IamRoleDetailSection, IamUserDetailSection};
use crate::aws::services::messaging::{SnsTopicDetailSection, SqsQueueDetailSection};
use crate::aws::services::organizations::{
    OrgAccountDetailSection, OrgScpDetailSection, OrgUnitDetailSection,
};
use crate::aws::services::ssm::{SsmAssociationDetailSection, SsmAutomationDetailSection, SsmBaselineDetailSection, SsmCommandDetailSection, SsmDocumentDetailSection, SsmFleetDetailSection, SsmMaintWindowDetailSection, SsmOpsItemDetailSection, SsmParameterDetailSection};
use crate::aws::services::api_gateway::{ApiDomainDetailSection, ApiUsagePlanDetailSection, HttpApiDetailSection, RestApiDetailSection};
use crate::aws::services::awsconfig::ConfigRuleDetailSection;
use crate::aws::services::backup::{BackupPlanDetailSection, BackupVaultDetailSection};
use crate::aws::services::servicecatalog::{
    ScPortfolioDetailSection, ScProductDetailSection, ScProvisionedProductDetailSection,
};
use crate::aws::services::cloudtrail::{CtEventDetailSection, CtTrailDetailSection};
use crate::aws::services::cloudwatch::{CwAlarmDetailSection, CwCompositeAlarmDetailSection, CwDashboardDetailSection, CwLogGroupDetailSection};
use crate::aws::services::cognito::CognitoUserPoolDetailSection;
use crate::aws::services::guardduty::GdFindingDetailSection;
use crate::aws::services::identity_center::{IcApplicationDetailSection, IcGroupDetailSection, IcInstanceDetailSection, IcUserDetailSection, PermissionSetDetailSection};
use crate::aws::services::inspector::InspFindingDetailSection;
use crate::aws::services::network_firewall::{NfwFirewallDetailSection, NfwPolicyDetailSection, NfwRuleGroupDetailSection};
use crate::aws::services::security_hub::{ShControlDetailSection, ShFindingDetailSection};
use crate::aws::services::waf::{WafIpSetDetailSection, WafRuleGroupDetailSection, WafWebAclDetailSection};
use crate::aws::services::athena::{AthenaDatabaseDetailSection, AthenaQueryDetailSection, AthenaSavedQueryDetailSection, AthenaWorkgroupDetailSection};
use crate::aws::services::direct_connect::{DxConnectionDetailSection, DxGatewayDetailSection, DxVifDetailSection};
use crate::aws::services::efs::EfsFileSystemDetailSection;
use crate::aws::services::eks::EksClusterDetailSection;
use crate::aws::services::elasticache::ElastiCacheDetailSection;
use crate::aws::services::eventbridge::{EbEventBusDetailSection, EbPipeDetailSection, EbRuleDetailSection, EbScheduleDetailSection};
use crate::aws::services::fms::{FmsPolicyDetailSection, FmsResourceSetDetailSection};
use crate::aws::services::fsx::FsxDetailSection;
use crate::aws::services::global_accelerator::GaAcceleratorDetailSection;
use crate::aws::services::glue::{GlueCrawlerDetailSection, GlueJobDetailSection, GlueJobRunDetailSection, GlueTableDetailSection};
use crate::aws::services::kinesis::{FirehoseDetailSection, KinesisDetailSection};
use crate::aws::services::msk::MskDetailSection;
use crate::aws::services::budgets::BudgetDetailSection;
use crate::aws::services::controltower::{
    CtAccountDetailSection, CtComplianceDetailSection, EnabledBaselineDetailSection,
    EnabledControlDetailSection, LandingZoneDetailSection,
};
use crate::aws::services::opensearch::OpenSearchDetailSection;
use crate::aws::services::redshift::{RedshiftClusterDetailSection, RedshiftWorkgroupDetailSection};
use crate::aws::services::ses::{SesConfigSetDetailSection, SesIdentityDetailSection};
use crate::aws::services::transfer::TransferServerDetailSection;
use crate::aws::services::acm::{AcmCertDetailSection};
use crate::aws::services::cloudformation::{CfnExportDetailSection, CfnStackDetailSection, CfnStackSetDetailSection};
use crate::aws::services::cloudfront::{CfDistributionDetailSection, CfFunctionDetailSection};
use crate::aws::services::dynamodb::{DdbTableDetailSection};
use crate::aws::services::kms::{KmsKeyDetailSection};
use crate::aws::services::lambda::{LambdaDetailSection};
use crate::aws::services::ram::{RamResourceShareDetailSection};
use crate::aws::services::rds::{RdsClusterDetailSection, RdsInstanceDetailSection};
use crate::aws::services::resource_groups::{ResourceGroupDetailSection};
use crate::aws::services::route53::{R53HealthCheckDetailSection, R53ZoneDetailSection};
use crate::aws::services::step_functions::{SfnDetailSection, SfnExecDetailSection};
use crate::aws::services::trusted_advisor::{TaCheckDetailSection, TaOrgCheckDetailSection};
use crate::aws::services::workspaces::{WorkspaceDetailSection};
use crate::aws::services::ec2::{
    AmiDetailSection, EbsVolumeDetailSection, Ec2InstanceDetailSection,
    LaunchTemplateDetailSection, SecurityGroupDetailSection, SnapshotDetailSection,
};
use crate::aws::services::transit_gateway::{TgwDetailSection, TgwRouteTableDetailSection};
use crate::aws::services::vpc::{
    NetworkAclDetailSection, PrefixListDetailSection, RouteTableDetailSection,
    SubnetDetailSection, VpcDetailSection, VpcEndpointDetailSection, VpnConnectionDetailSection,
};
use crate::aws::services::ec2::EniDetailSection;
use crate::aws::services::fsx::FsxVolumeDetailSection;
use crate::aws::services::guardduty::{
    GdDetectorDetailSection, GdFilterDetailSection, GdMalwareScanDetailSection,
    GdMemberDetailSection, GdOverviewDetailSection,
};
use crate::aws::services::rds::RdsSnapshotDetailSection;
use crate::aws::services::transit_gateway::TgwAttachmentDetailSection;
use crate::aws::services::vpc::{
    DhcpOptionsDetailSection, NatGatewayDetailSection, VpcPeeringDetailSection,
};
use crate::aws::services::guardduty::{
    GdDetector, GdFilter, GdMalwareScan, GdMember, GdOverview,
};
use crate::aws::services::rds::RdsSnapshot;
use crate::aws::services::s3::{S3Bucket, S3BucketDetails};
use crate::aws::services::vpc::{
    pretty_dhcp_key, DhcpOptionsSet, FlowLogInfo, InternetGateway, NatGateway, NetworkAcl,
    PeeringSide, PrefixList, RouteEntry, RouteTable, Subnet, Vpc, VpcPeering,
};
use crate::lazy::Lazy;
use crate::ui::theme;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Paragraph,
    },
    Frame,
};

// One file per service, named like `src/aws/services/<svc>.rs`. Shared
// helpers (the split skeletons, row styling, jump classifiers, layout) live
// here; children reach them — and each other — through `use super::*`.
mod acm;
mod agentcore;
mod api_gateway;
mod asg;
mod athena;
mod awsconfig;
mod backup;
mod batch;
mod beanstalk;
mod bedrock;
mod budgets;
mod cloudformation;
mod cloudfront;
mod cloudtrail;
mod cloudwatch;
mod code;
mod cognito;
mod config;
mod controltower;
mod cost;
mod direct_connect;
mod dms;
mod dynamodb;
mod ec2;
mod ecr;
mod ecs;
mod efs;
mod eks;
mod elasticache;
mod elb;
mod eventbridge;
mod fms;
mod fsx;
mod global_accelerator;
mod glue;
mod guardduty;
mod health;
mod iam;
mod identity_center;
mod inspector;
mod kinesis;
mod kms;
mod lambda;
mod messaging;
mod msk;
mod network_firewall;
mod oam;
mod opensearch;
mod organizations;
mod ram;
mod rds;
mod redshift;
mod resource_groups;
mod route53;
mod route53profiles;
mod route53resolver;
mod s3files;
mod s3tables;
mod security_hub;
mod servicecatalog;
mod ses;
mod ssm;
mod step_functions;
mod transfer;
mod transit_gateway;
mod trusted_advisor;
mod vpc;
mod waf;
mod workspaces;
mod xray;
pub use acm::*;
pub use agentcore::*;
pub use api_gateway::*;
pub use asg::*;
pub use athena::*;
pub use awsconfig::*;
pub use backup::*;
pub use batch::*;
pub use beanstalk::*;
pub use bedrock::*;
pub use budgets::*;
pub use cloudformation::*;
pub use cloudfront::*;
pub use cloudtrail::*;
pub use cloudwatch::*;
pub use code::*;
pub use cognito::*;
pub use config::*;
pub use controltower::*;
pub use cost::*;
pub use direct_connect::*;
pub use dms::*;
pub use dynamodb::*;
pub use ec2::*;
pub use ecr::*;
pub use ecs::*;
pub use efs::*;
pub use eks::*;
pub use elasticache::*;
pub use elb::*;
pub use eventbridge::*;
pub use fms::*;
pub use fsx::*;
pub use global_accelerator::*;
pub use glue::*;
pub use guardduty::*;
pub use health::*;
pub use iam::*;
pub use identity_center::*;
pub use inspector::*;
pub use kinesis::*;
pub use kms::*;
pub use lambda::*;
pub use messaging::*;
pub use msk::*;
pub use network_firewall::*;
pub use oam::*;
pub use opensearch::*;
pub use organizations::*;
pub use ram::*;
pub use rds::*;
pub use redshift::*;
pub use resource_groups::*;
pub use route53::*;
pub use route53profiles::*;
pub use route53resolver::*;
pub use s3files::*;
pub use s3tables::*;
pub use security_hub::*;
pub use servicecatalog::*;
pub use ses::*;
pub use ssm::*;
pub use step_functions::*;
pub use transfer::*;
pub use transit_gateway::*;
pub use trusted_advisor::*;
pub use vpc::*;
pub use waf::*;
pub use workspaces::*;
pub use xray::*;

pub fn render_details_pane(app: &App, area: Rect, frame: &mut Frame) {
    render_details_pane_inner(app, area, frame);
    render_ownership_ribbon(app, area, frame);
    // The pane's top border (its title bar) toggles full width, as `Z` does:
    // the mouse route to the one layout key with no visible control.
    if area.height > 0 {
        app.push_click_region(
            Rect { height: 1, ..area },
            crate::app::ClickAction::Press(crossterm::event::KeyCode::Char('Z')),
        );
    }
}

/// Ownership ribbon: a dim one-liner on the pane's bottom border, left side
/// (the footer hints sit bottom-right) — `⛓ stack my-stack (AppServer) ·
/// terraform · team payments`, resolved from tags alone
/// (`crate::ownership`). One central hook so every detail renderer gets it
/// with no per-pane wiring. Suppressed while any overlay owns the pane —
/// the guard list mirrors the early returns at the top of
/// `render_details_pane_inner`; keep them in step.
fn render_ownership_ribbon(app: &App, area: Rect, frame: &mut Frame) {
    if !app.ownership_ribbon {
        return;
    }
    if app.metrics_in_pane.is_some()
        || app.s3_object_browser.visible
        || app.memory_browser.visible
        || app.ddb_browser.visible
        || app.log_tail.visible
        || app.trail_in_pane.is_some()
        || app.refs_in_pane.is_some()
        || app.access_in_pane.is_some()
        || app.terraform_state.is_some()
    {
        return;
    }
    let Some(resource) = app.get_selected_resource() else {
        return;
    };
    // Most types carry tags eagerly; Lambda only gets them from the lazy
    // `GetFunction` code info (fired on drill-in by the Config hook), so
    // fall back to that once it has landed.
    let mut tags = resource.tags();
    if tags.is_empty() {
        if let Some(func) = resource.as_any().downcast_ref::<LambdaFunction>() {
            if let Some(crate::lazy::Lazy::Loaded(info)) =
                app.lazy.lambda_code.get(&func.function_arn)
            {
                tags = &info.tags;
            }
        }
    }
    let ownership =
        crate::ownership::resource_ownership(tags, &app.owner_tags, &app.managed_by_tags);
    if ownership.is_empty() {
        return;
    }
    // Cap at half the pane so the bottom-right footer hints stay readable.
    let budget = (area.width as usize / 2).saturating_sub(4);
    if budget < 8 || area.height < 2 {
        return;
    }
    let mut text = format!("⛓ {}", ownership.summary());
    if text.chars().count() > budget {
        text = text.chars().take(budget.saturating_sub(1)).collect();
        text.push('…');
    }
    let text = format!(" {} ", text);
    let w = text.chars().count() as u16;
    let rect = Rect {
        x: area.x + 2,
        y: area.y + area.height - 1,
        width: w.min(area.width.saturating_sub(2)),
        height: 1,
    };
    frame.render_widget(
        Paragraph::new(Span::styled(
            text,
            Style::default().fg(theme::text_dim()),
        )),
        rect,
    );
}

fn render_details_pane_inner(app: &App, area: Rect, frame: &mut Frame) {
    let resource = app.get_selected_resource();

    // Metric charts rendered inside the detail pane (instead of a modal),
    // expandable with `Z`. Takes over the pane while active.
    if let Some(kind) = app.metrics_in_pane {
        crate::ui::widgets::metrics_overlay::render_metrics_in_pane(app, kind, area, frame);
        return;
    }

    // S3 object browser also lives in the detail pane (list stays visible in
    // Split; `Z` expands it full-width). It owns the pane while open.
    if app.s3_object_browser.visible {
        crate::ui::widgets::s3_object_browser::render_s3_object_browser(app, area, frame);
        return;
    }

    // AgentCore memory session browser, likewise.
    if app.memory_browser.visible {
        crate::ui::widgets::memory_browser::render_memory_browser(app, area, frame);
        return;
    }

    // DynamoDB item browser, likewise, lives in the detail pane.
    if app.ddb_browser.visible {
        crate::ui::widgets::ddb_item_browser::render_ddb_item_browser(app, area, frame);
        return;
    }

    // Live log tail also owns the detail pane while open.
    if app.log_tail.visible {
        crate::ui::widgets::log_tail::render_log_tail(app, area, frame);
        return;
    }

    // CloudTrail "who changed this?" lens owns the detail pane while open.
    if app.trail_in_pane.is_some() {
        crate::ui::widgets::trail_lens::render_trail_lens(app, area, frame);
        return;
    }
    // "Referenced by" (`U`) and network-access (`N`) lenses, same shape.
    if app.refs_in_pane.is_some() {
        crate::ui::widgets::refs_lens::render_refs_lens(app, area, frame);
        return;
    }
    if app.access_in_pane.is_some() {
        crate::ui::widgets::access_lens::render_access_lens(app, area, frame);
        return;
    }

    // Terraform state viewer — owns the detail pane while active.
    if app.terraform_state.is_some() {
        render_tf_state_pane(app, area, frame);
        return;
    }

    // Resources with split panes (header + tabs + body).
    if let Some(instance) = resource.and_then(|r| r.as_any().downcast_ref::<Ec2Instance>()) {
        render_ec2_instance_split(app, instance, area, frame);
        return;
    }
    if let Some(subnet) = resource.and_then(|r| r.as_any().downcast_ref::<Subnet>()) {
        render_subnet_split(app, subnet, area, frame);
        return;
    }
    if let Some(rt) = resource.and_then(|r| r.as_any().downcast_ref::<RouteTable>()) {
        render_route_table_split(app, rt, area, frame);
        return;
    }
    if let Some(acl) = resource.and_then(|r| r.as_any().downcast_ref::<NetworkAcl>()) {
        render_network_acl_split(app, acl, area, frame);
        return;
    }
    if let Some(stack) = resource.and_then(|r| r.as_any().downcast_ref::<CfnStack>()) {
        render_cfn_stack_split(app, stack, area, frame);
        return;
    }
    if let Some(export) = resource.and_then(|r| r.as_any().downcast_ref::<CfnExport>()) {
        render_cfn_export_split(app, export, area, frame);
        return;
    }
    if let Some(ss) = resource.and_then(|r| r.as_any().downcast_ref::<CfnStackSet>()) {
        render_cfn_stackset_split(app, ss, area, frame);
        return;
    }
    if let Some(func) = resource.and_then(|r| r.as_any().downcast_ref::<LambdaFunction>()) {
        render_lambda_function_split(app, func, area, frame);
        return;
    }
    if let Some(layer) = resource
        .and_then(|r| r.as_any().downcast_ref::<crate::aws::services::lambda::LambdaLayer>())
    {
        let subtitle = format!("Layer · latest version {}", layer.latest_version);
        render_simple_split(
            app,
            area,
            frame,
            "Lambda Layer",
            &layer.name,
            &subtitle,
            &descriptor_tabs(app, &crate::aws::services::lambda::LAMBDA_LAYER_SECTIONS),
        );
        return;
    }
    if let Some(zone) = resource.and_then(|r| r.as_any().downcast_ref::<R53HostedZone>()) {
        render_r53_zone_split(app, zone, area, frame);
        return;
    }
    if let Some(hc) = resource.and_then(|r| r.as_any().downcast_ref::<R53HealthCheck>()) {
        render_r53_health_check_split(app, hc, area, frame);
        return;
    }
    if let Some(rec) = resource
        .and_then(|r| r.as_any().downcast_ref::<crate::aws::services::route53::R53Record>())
    {
        let subtitle = format!("{} · {}", rec.record_type, rec.routing_policy());
        render_simple_split(
            app,
            area,
            frame,
            "R53 Record",
            &rec.name,
            &subtitle,
            &descriptor_tabs(app, &crate::aws::services::route53::R53_RECORD_SECTIONS),
        );
        return;
    }
    if let Some(cert) = resource.and_then(|r| r.as_any().downcast_ref::<AcmCertificate>()) {
        render_acm_cert_split(app, cert, area, frame);
        return;
    }
    if let Some(sm) = resource.and_then(|r| r.as_any().downcast_ref::<SfnStateMachine>()) {
        render_sfn_split(app, sm, area, frame);
        return;
    }
    if let Some(exec) = resource.and_then(|r| r.as_any().downcast_ref::<SfnExecution>()) {
        render_sfn_exec_split(app, exec, area, frame);
        return;
    }
    if let Some(p) = resource.and_then(|r| {
        r.as_any()
            .downcast_ref::<crate::aws::services::code::CodePipeline>()
    }) {
        render_code_pipeline_split(app, p, area, frame);
        return;
    }
    if let Some(exec) = resource.and_then(|r| {
        r.as_any()
            .downcast_ref::<crate::aws::services::code::CodePipelineExecution>()
    }) {
        render_code_exec_split(app, exec, area, frame);
        return;
    }
    if let Some(repo) = resource.and_then(|r| {
        r.as_any()
            .downcast_ref::<crate::aws::services::code::CodeCommitRepo>()
    }) {
        render_code_commit_repo_split(app, repo, area, frame);
        return;
    }
    if let Some(pr) = resource.and_then(|r| {
        r.as_any()
            .downcast_ref::<crate::aws::services::code::CodeCommitPullRequest>()
    }) {
        render_cc_pr_split(app, pr, area, frame);
        return;
    }
    if let Some(proj) = resource.and_then(|r| {
        r.as_any()
            .downcast_ref::<crate::aws::services::code::CodeBuildProject>()
    }) {
        render_code_build_split(app, proj, area, frame);
        return;
    }
    if let Some(g) = resource.and_then(|r| {
        r.as_any()
            .downcast_ref::<crate::aws::services::code::CodeDeployGroup>()
    }) {
        render_code_deploy_group_split(app, g, area, frame);
        return;
    }
    if let Some(repo) = resource.and_then(|r| {
        r.as_any()
            .downcast_ref::<crate::aws::services::code::CodeArtifactRepo>()
    }) {
        render_code_artifact_repo_split(app, repo, area, frame);
        return;
    }
    if let Some(fs) = resource.and_then(|r| {
        r.as_any()
            .downcast_ref::<crate::aws::services::fsx::FsxFileSystem>()
    }) {
        render_fsx_split(app, fs, area, frame);
        return;
    }
    if let Some(evt) = resource.and_then(|r| {
        r.as_any()
            .downcast_ref::<crate::aws::services::health::HealthEvent>()
    }) {
        render_health_split(app, evt, area, frame);
        return;
    }
    if let Some(dist) = resource.and_then(|r| r.as_any().downcast_ref::<CfDistribution>()) {
        render_cf_distribution_split(app, dist, area, frame);
        return;
    }
    if let Some(f) = resource.and_then(|r| {
        r.as_any()
            .downcast_ref::<crate::aws::services::cloudfront::CfFunction>()
    }) {
        render_cf_function_split(app, f, area, frame);
        return;
    }
    if let Some(key) = resource.and_then(|r| r.as_any().downcast_ref::<KmsKey>()) {
        render_kms_key_split(app, key, area, frame);
        return;
    }
    if let Some(g) = resource.and_then(|r| r.as_any().downcast_ref::<BedrockGuardrail>()) {
        render_bedrock_guardrail_split(app, g, area, frame);
        return;
    }
    if let Some(kb) = resource.and_then(|r| r.as_any().downcast_ref::<BedrockKnowledgeBase>()) {
        render_bedrock_kb_split(app, kb, area, frame);
        return;
    }
    if let Some(agent) = resource.and_then(|r| r.as_any().downcast_ref::<BedrockAgent>()) {
        render_bedrock_agent_split(app, agent, area, frame);
        return;
    }
    if let Some(share) = resource.and_then(|r| {
        r.as_any()
            .downcast_ref::<crate::aws::services::ram::RamResourceShare>()
    }) {
        render_ram_share_split(app, share, area, frame);
        return;
    }
    if let Some(ws) = resource.and_then(|r| {
        r.as_any()
            .downcast_ref::<crate::aws::services::workspaces::Workspace>()
    }) {
        render_workspace_split(app, ws, area, frame);
        return;
    }
    if let Some(rg) = resource.and_then(|r| r.as_any().downcast_ref::<RgGroup>()) {
        render_resource_group_split(app, rg, area, frame);
        return;
    }
    if let Some(check) = resource.and_then(|r| r.as_any().downcast_ref::<TaCheck>()) {
        render_ta_check_split(app, check, area, frame);
        return;
    }
    if let Some(rec) = resource.and_then(|r| {
        r.as_any()
            .downcast_ref::<crate::aws::services::trusted_advisor::TaRecommendation>()
    }) {
        render_ta_rec_split(app, rec, area, frame);
        return;
    }
    if let Some(table) = resource.and_then(|r| r.as_any().downcast_ref::<DdbTable>()) {
        render_ddb_table_split(app, table, area, frame);
        return;
    }
    if let Some(conn) = resource.and_then(|r| r.as_any().downcast_ref::<DxConnection>()) {
        render_dx_connection_split(app, conn, area, frame);
        return;
    }
    if let Some(gw) = resource.and_then(|r| r.as_any().downcast_ref::<DxGateway>()) {
        render_dx_gateway_split(app, gw, area, frame);
        return;
    }
    if let Some(vif) = resource.and_then(|r| r.as_any().downcast_ref::<DxVirtualInterface>()) {
        render_dx_vif_split(app, vif, area, frame);
        return;
    }
    if let Some(acc) = resource.and_then(|r| r.as_any().downcast_ref::<GaAccelerator>()) {
        render_ga_accelerator_split(app, acc, area, frame);
        return;
    }
    if let Some(cluster) = resource.and_then(|r| r.as_any().downcast_ref::<EksCluster>()) {
        render_eks_cluster_split(app, cluster, area, frame);
        return;
    }
    if let Some(fs) = resource.and_then(|r| r.as_any().downcast_ref::<EfsFileSystem>()) {
        render_efs_fs_split(app, fs, area, frame);
        return;
    }
    if let Some(c) = resource.and_then(|r| r.as_any().downcast_ref::<ElastiCacheCluster>()) {
        render_elasticache_split(app, c, area, frame);
        return;
    }
    if let Some(c) = resource.and_then(|r| r.as_any().downcast_ref::<MskCluster>()) {
        render_msk_split(app, c, area, frame);
        return;
    }
    if let Some(b) = resource.and_then(|r| r.as_any().downcast_ref::<BudgetItem>()) {
        render_budget_split(app, b, area, frame);
        return;
    }
    if let Some(z) = resource.and_then(|r| r.as_any().downcast_ref::<LandingZone>()) {
        render_landing_zone_split(app, z, area, frame);
        return;
    }
    if let Some(c) = resource.and_then(|r| r.as_any().downcast_ref::<EnabledControl>()) {
        render_enabled_control_split(app, c, area, frame);
        return;
    }
    if let Some(b) = resource.and_then(|r| r.as_any().downcast_ref::<EnabledBaseline>()) {
        render_enabled_baseline_split(app, b, area, frame);
        return;
    }
    if let Some(a) = resource.and_then(|r| r.as_any().downcast_ref::<CtAccount>()) {
        render_ct_account_split(app, a, area, frame);
        return;
    }
    if let Some(c) = resource.and_then(|r| r.as_any().downcast_ref::<CtCompliance>()) {
        render_ct_compliance_split(app, c, area, frame);
        return;
    }
    if let Some(d) = resource.and_then(|r| r.as_any().downcast_ref::<OpenSearchDomain>()) {
        render_opensearch_split(app, d, area, frame);
        return;
    }
    if let Some(s) = resource.and_then(|r| r.as_any().downcast_ref::<KinesisStream>()) {
        render_kinesis_split(app, s, area, frame);
        return;
    }
    if let Some(s) = resource.and_then(|r| r.as_any().downcast_ref::<FirehoseStream>()) {
        render_firehose_split(app, s, area, frame);
        return;
    }
    if let Some(n) = resource.and_then(|r| r.as_any().downcast_ref::<crate::aws::services::xray::XRayNode>()) {
        render_xray_node_split(app, n, area, frame);
        return;
    }
    if let Some(t) = resource.and_then(|r| r.as_any().downcast_ref::<crate::aws::services::xray::XRayTrace>()) {
        render_xray_trace_split(app, t, area, frame);
        return;
    }
    if let Some(r) = resource {
        use crate::aws::services::batch::*;
        let any = r.as_any();
        if let Some(q) = any.downcast_ref::<BatchJobQueue>() {
            render_batch_queue_split(app, q, area, frame);
            return;
        }
        if let Some(c) = any.downcast_ref::<BatchComputeEnv>() {
            render_batch_ce_split(app, c, area, frame);
            return;
        }
        if let Some(j) = any.downcast_ref::<BatchJob>() {
            render_batch_job_split(app, j, area, frame);
            return;
        }
        if let Some(d) = any.downcast_ref::<BatchJobDefinition>() {
            render_batch_jobdef_split(app, d, area, frame);
            return;
        }
    }
    if let Some(r) = resource {
        use crate::aws::services::beanstalk::{EbApplication, EbEnvironment, EbVersion};
        if let Some(e) = r.as_any().downcast_ref::<EbEnvironment>() {
            render_eb_environment_split(app, e, area, frame);
            return;
        }
        if let Some(a) = r.as_any().downcast_ref::<EbApplication>() {
            render_eb_application_split(app, a, area, frame);
            return;
        }
        if let Some(v) = r.as_any().downcast_ref::<EbVersion>() {
            render_eb_version_split(app, v, area, frame);
            return;
        }
    }
    if let Some(r) = resource {
        use crate::aws::services::dms::{DmsEndpoint, DmsInstance, DmsServerless, DmsTask};
        if let Some(t) = r.as_any().downcast_ref::<DmsTask>() {
            render_dms_task_split(app, t, area, frame);
            return;
        }
        if let Some(i) = r.as_any().downcast_ref::<DmsInstance>() {
            render_dms_instance_split(app, i, area, frame);
            return;
        }
        if let Some(e) = r.as_any().downcast_ref::<DmsEndpoint>() {
            render_dms_endpoint_split(app, e, area, frame);
            return;
        }
        if let Some(s) = r.as_any().downcast_ref::<DmsServerless>() {
            render_dms_serverless_split(app, s, area, frame);
            return;
        }
    }
    if let Some(c) = resource.and_then(|r| r.as_any().downcast_ref::<RedshiftCluster>()) {
        render_redshift_cluster_split(app, c, area, frame);
        return;
    }
    if let Some(w) = resource.and_then(|r| r.as_any().downcast_ref::<RedshiftWorkgroup>()) {
        render_redshift_workgroup_split(app, w, area, frame);
        return;
    }
    if let Some(p) = resource.and_then(|r| r.as_any().downcast_ref::<FmsPolicy>()) {
        render_fms_policy_split(app, p, area, frame);
        return;
    }
    if let Some(s) = resource.and_then(|r| r.as_any().downcast_ref::<FmsResourceSet>()) {
        render_fms_resource_set_split(app, s, area, frame);
        return;
    }
    if let Some(w) = resource.and_then(|r| r.as_any().downcast_ref::<AthenaWorkgroup>()) {
        render_athena_workgroup_split(app, w, area, frame);
        return;
    }
    if let Some(d) = resource.and_then(|r| r.as_any().downcast_ref::<AthenaDatabase>()) {
        render_athena_database_split(app, d, area, frame);
        return;
    }
    if let Some(q) = resource.and_then(|r| r.as_any().downcast_ref::<AthenaQueryExecution>()) {
        render_athena_query_split(app, q, area, frame);
        return;
    }
    if let Some(q) = resource.and_then(|r| r.as_any().downcast_ref::<AthenaNamedQuery>()) {
        render_athena_saved_query_split(app, q, area, frame);
        return;
    }
    if let Some(t) = resource.and_then(|r| r.as_any().downcast_ref::<GlueTable>()) {
        render_glue_table_split(app, t, area, frame);
        return;
    }
    if let Some(c) = resource.and_then(|r| r.as_any().downcast_ref::<GlueCrawler>()) {
        render_glue_crawler_split(app, c, area, frame);
        return;
    }
    if let Some(j) = resource.and_then(|r| r.as_any().downcast_ref::<GlueJob>()) {
        render_glue_job_split(app, j, area, frame);
        return;
    }
    if let Some(r2) = resource.and_then(|r| r.as_any().downcast_ref::<GlueJobRun>()) {
        render_glue_job_run_split(app, r2, area, frame);
        return;
    }
    if let Some(cs) = resource.and_then(|r| r.as_any().downcast_ref::<SesConfigSet>()) {
        render_ses_config_set_split(app, cs, area, frame);
        return;
    }
    if let Some(i) = resource.and_then(|r| r.as_any().downcast_ref::<SesIdentity>()) {
        render_ses_identity_split(app, i, area, frame);
        return;
    }
    if let Some(srv) = resource.and_then(|r| r.as_any().downcast_ref::<TransferServer>()) {
        render_transfer_server_split(app, srv, area, frame);
        return;
    }
    if let Some(rule) = resource.and_then(|r| r.as_any().downcast_ref::<EbRule>()) {
        render_eb_rule_split(app, rule, area, frame);
        return;
    }
    if let Some(bus) = resource.and_then(|r| r.as_any().downcast_ref::<EbEventBus>()) {
        render_eb_event_bus_split(app, bus, area, frame);
        return;
    }
    if let Some(sched) = resource.and_then(|r| r.as_any().downcast_ref::<EbSchedule>()) {
        render_eb_schedule_split(app, sched, area, frame);
        return;
    }
    if let Some(pipe) = resource.and_then(|r| r.as_any().downcast_ref::<EbPipe>()) {
        render_eb_pipe_split(app, pipe, area, frame);
        return;
    }
    if let Some(vpc) = resource.and_then(|r| r.as_any().downcast_ref::<Vpc>()) {
        render_vpc_split(app, vpc, area, frame);
        return;
    }
    if let Some(dopt) = resource.and_then(|r| r.as_any().downcast_ref::<DhcpOptionsSet>()) {
        render_dhcp_options_split(app, dopt, area, frame);
        return;
    }
    if let Some(peer) = resource.and_then(|r| r.as_any().downcast_ref::<VpcPeering>()) {
        render_vpc_peering_split(app, peer, area, frame);
        return;
    }
    if let Some(pl) = resource.and_then(|r| r.as_any().downcast_ref::<PrefixList>()) {
        render_prefix_list_split(app, pl, area, frame);
        return;
    }
    if let Some(nat) = resource.and_then(|r| r.as_any().downcast_ref::<NatGateway>()) {
        render_nat_gateway_split(app, nat, area, frame);
        return;
    }
    if let Some(eni) = resource.and_then(|r| r.as_any().downcast_ref::<NetworkInterface>()) {
        render_eni_split(app, eni, area, frame);
        return;
    }
    if let Some(att) = resource.and_then(|r| {
        r.as_any()
            .downcast_ref::<crate::aws::services::transit_gateway::TgwAttachment>()
    }) {
        render_tgw_attachment_split(app, att, area, frame);
        return;
    }
    if let Some(lag) = resource.and_then(|r| {
        r.as_any()
            .downcast_ref::<crate::aws::services::direct_connect::DxLag>()
    }) {
        render_dx_lag_split(app, lag, area, frame);
        return;
    }
    if let Some(snap) = resource.and_then(|r| r.as_any().downcast_ref::<RdsSnapshot>()) {
        render_rds_snapshot_split(app, snap, area, frame);
        return;
    }
    if let Some(pg) = resource.and_then(|r| {
        r.as_any()
            .downcast_ref::<crate::aws::services::rds::RdsParamGroup>()
    }) {
        render_rds_param_group_split(app, pg, area, frame);
        return;
    }
    if let Some(vol) = resource.and_then(|r| {
        r.as_any()
            .downcast_ref::<crate::aws::services::fsx::FsxVolume>()
    }) {
        render_fsx_volume_split(app, vol, area, frame);
        return;
    }
    if let Some(det) = resource.and_then(|r| r.as_any().downcast_ref::<GdDetector>()) {
        render_gd_detector_split(app, det, area, frame);
        return;
    }
    if let Some(ov) = resource.and_then(|r| r.as_any().downcast_ref::<GdOverview>()) {
        render_gd_overview_split(app, ov, area, frame);
        return;
    }
    if let Some(f) = resource.and_then(|r| r.as_any().downcast_ref::<GdFilter>()) {
        render_gd_filter_split(app, f, area, frame);
        return;
    }
    if let Some(m) = resource.and_then(|r| r.as_any().downcast_ref::<GdMember>()) {
        render_gd_member_split(app, m, area, frame);
        return;
    }
    if let Some(s) = resource.and_then(|r| r.as_any().downcast_ref::<GdMalwareScan>()) {
        render_gd_malware_scan_split(app, s, area, frame);
        return;
    }
    if let Some(fw) = resource.and_then(|r| r.as_any().downcast_ref::<NfwFirewall>()) {
        render_nfw_firewall_split(app, fw, area, frame);
        return;
    }
    if let Some(ev) = resource.and_then(|r| {
        r.as_any()
            .downcast_ref::<crate::aws::services::cloudtrail::CloudTrailEvent>()
    }) {
        render_ct_event_split(app, ev, area, frame);
        return;
    }
    if let Some(trail) = resource.and_then(|r| {
        r.as_any()
            .downcast_ref::<crate::aws::services::cloudtrail::CloudTrailTrail>()
    }) {
        render_ct_trail_split(app, trail, area, frame);
        return;
    }
    if let Some(ps) = resource.and_then(|r| {
        r.as_any()
            .downcast_ref::<crate::aws::services::identity_center::PermissionSet>()
    }) {
        render_permission_set_split(app, ps, area, frame);
        return;
    }
    if let Some(g) = resource.and_then(|r| {
        r.as_any()
            .downcast_ref::<crate::aws::services::identity_center::IcGroup>()
    }) {
        render_ic_group_split(app, g, area, frame);
        return;
    }
    if let Some(u) = resource.and_then(|r| {
        r.as_any()
            .downcast_ref::<crate::aws::services::identity_center::IcUser>()
    }) {
        render_ic_user_split(app, u, area, frame);
        return;
    }
    if let Some(inst) = resource.and_then(|r| {
        r.as_any()
            .downcast_ref::<crate::aws::services::identity_center::IcInstance>()
    }) {
        render_ic_instance_split(app, inst, area, frame);
        return;
    }
    if let Some(a) = resource.and_then(|r| {
        r.as_any()
            .downcast_ref::<crate::aws::services::identity_center::IcApplication>()
    }) {
        render_ic_application_split(app, a, area, frame);
        return;
    }
    if let Some(sg) = resource.and_then(|r| {
        r.as_any()
            .downcast_ref::<crate::aws::services::ec2::SecurityGroup>()
    }) {
        render_security_group_split(
            app,
            sg.name(),
            &sg.group_id,
            sg.vpc_id.as_deref(),
            &sg.description,
            area,
            frame,
        );
        return;
    }
    if let Some(sg) = resource.and_then(|r| {
        r.as_any()
            .downcast_ref::<crate::aws::services::vpc::VpcSecurityGroup>()
    }) {
        render_security_group_split(
            app,
            sg.name(),
            &sg.group_id,
            sg.vpc_id.as_deref(),
            &sg.description,
            area,
            frame,
        );
        return;
    }
    if let Some(vol) = resource.and_then(|r| {
        r.as_any().downcast_ref::<crate::aws::services::ec2::EbsVolume>()
    }) {
        render_ebs_volume_split(app, vol, area, frame);
        return;
    }
    if let Some(ami) = resource.and_then(|r| r.as_any().downcast_ref::<crate::aws::services::ec2::Ami>()) {
        render_ami_split(app, ami, area, frame);
        return;
    }
    if let Some(snap) = resource.and_then(|r| {
        r.as_any().downcast_ref::<crate::aws::services::ec2::EbsSnapshot>()
    }) {
        render_snapshot_split(app, snap, area, frame);
        return;
    }
    if let Some(lt) = resource.and_then(|r| {
        r.as_any().downcast_ref::<crate::aws::services::ec2::LaunchTemplate>()
    }) {
        render_launch_template_split(app, lt, area, frame);
        return;
    }
    if let Some(policy) = resource.and_then(|r| r.as_any().downcast_ref::<NfwPolicy>()) {
        render_nfw_policy_split(app, policy, area, frame);
        return;
    }
    if let Some(rg) = resource.and_then(|r| r.as_any().downcast_ref::<NfwRuleGroup>()) {
        render_nfw_rule_group_split(app, rg, area, frame);
        return;
    }
    if let Some(finding) = resource.and_then(|r| r.as_any().downcast_ref::<GdFinding>()) {
        render_gd_finding_split(app, finding, area, frame);
        return;
    }
    if let Some(overview) = resource.and_then(|r| r.as_any().downcast_ref::<ShOverview>()) {
        render_sh_overview_split(app, overview, area, frame);
        return;
    }
    if let Some(repo) = resource.and_then(|r| r.as_any().downcast_ref::<EcrRepository>()) {
        render_ecr_repo_split(app, repo, area, frame);
        return;
    }
    if let Some(img) = resource
        .and_then(|r| r.as_any().downcast_ref::<crate::aws::services::ecr::EcrImage>())
    {
        render_simple_split(
            app,
            area,
            frame,
            "ECR Image",
            &img.label,
            &img.digest,
            &descriptor_tabs(app, &crate::aws::services::ecr::ECR_IMAGE_SECTIONS),
        );
        return;
    }
    if let Some(finding) = resource.and_then(|r| r.as_any().downcast_ref::<ShFinding>()) {
        render_sh_finding_split(app, finding, area, frame);
        return;
    }
    if let Some(control) = resource.and_then(|r| r.as_any().downcast_ref::<ShControl>()) {
        render_sh_control_split(app, control, area, frame);
        return;
    }
    if let Some(rule) = resource.and_then(|r| {
        r.as_any()
            .downcast_ref::<crate::aws::services::security_hub::ShAutomationRule>()
    }) {
        render_sh_automation_rule_split(app, rule, area, frame);
        return;
    }
    if let Some(i) = resource.and_then(|r| {
        r.as_any()
            .downcast_ref::<crate::aws::services::security_hub::ShInsight>()
    }) {
        let subtitle = format!(
            "grouped by {}{}",
            i.group_by,
            if i.managed { " · AWS-managed" } else { "" }
        );
        render_simple_split(
            app,
            area,
            frame,
            "Security Insight",
            &i.name,
            &subtitle,
            &descriptor_tabs(app, &crate::aws::services::security_hub::SH_INSIGHT_SECTIONS),
        );
        return;
    }
    if let Some(s) = resource.and_then(|r| {
        r.as_any()
            .downcast_ref::<crate::aws::services::security_hub::ShSettings>()
    }) {
        let subtitle = if s.aggregation_configured {
            format!("aggregating into {}", s.aggregation_region)
        } else {
            "this region only".to_string()
        };
        render_simple_split(
            app,
            area,
            frame,
            "Security Hub Settings",
            "Security Hub Settings",
            &subtitle,
            &descriptor_tabs(
                app,
                &crate::aws::services::security_hub::SH_SETTINGS_SECTIONS,
            ),
        );
        return;
    }
    if let Some(p) = resource.and_then(|r| {
        r.as_any()
            .downcast_ref::<crate::aws::services::security_hub::ShConfigPolicy>()
    }) {
        let subtitle = format!(
            "{} target(s){}",
            p.targets.len(),
            if p.service_enabled {
                ""
            } else {
                " · Security Hub disabled"
            }
        );
        render_simple_split(
            app,
            area,
            frame,
            "Configuration Policy",
            &p.name,
            &subtitle,
            &descriptor_tabs(
                app,
                &crate::aws::services::security_hub::SH_CONFIG_POLICY_SECTIONS,
            ),
        );
        return;
    }
    if let Some(pool) = resource.and_then(|r| r.as_any().downcast_ref::<CognitoUserPool>()) {
        render_cognito_user_pool_split(app, pool, area, frame);
        return;
    }
    if let Some(finding) = resource.and_then(|r| r.as_any().downcast_ref::<InspFinding>()) {
        render_insp_finding_split(app, finding, area, frame);
        return;
    }
    if let Some(vault) = resource.and_then(|r| r.as_any().downcast_ref::<BackupVault>()) {
        render_backup_vault_split(app, vault, area, frame);
        return;
    }
    if let Some(plan) = resource.and_then(|r| r.as_any().downcast_ref::<BackupPlan>()) {
        render_backup_plan_split(app, plan, area, frame);
        return;
    }
    if let Some(portfolio) = resource.and_then(|r| r.as_any().downcast_ref::<ScPortfolio>()) {
        render_sc_portfolio_split(app, portfolio, area, frame);
        return;
    }
    if let Some(product) = resource.and_then(|r| r.as_any().downcast_ref::<ScProduct>()) {
        render_sc_product_split(app, product, area, frame);
        return;
    }
    if let Some(pp) = resource.and_then(|r| r.as_any().downcast_ref::<ScProvisionedProduct>()) {
        render_sc_pp_split(app, pp, area, frame);
        return;
    }
    if let Some(fs) =
        resource.and_then(|r| r.as_any().downcast_ref::<crate::aws::services::s3files::S3FileSystem>())
    {
        render_s3files_fs_split(app, fs, area, frame);
        return;
    }
    if let Some(tb) = resource
        .and_then(|r| r.as_any().downcast_ref::<crate::aws::services::s3tables::S3TableBucket>())
    {
        render_s3tables_bucket_split(app, tb, area, frame);
        return;
    }
    if let Some(t) =
        resource.and_then(|r| r.as_any().downcast_ref::<crate::aws::services::s3tables::S3Table>())
    {
        render_s3tables_table_split(app, t, area, frame);
        return;
    }
    if let Some(api) = resource.and_then(|r| r.as_any().downcast_ref::<RestApi>()) {
        render_rest_api_split(app, api, area, frame);
        return;
    }
    if let Some(api) = resource.and_then(|r| r.as_any().downcast_ref::<HttpApi>()) {
        render_http_api_split(app, api, area, frame);
        return;
    }
    if let Some(plan) = resource.and_then(|r| r.as_any().downcast_ref::<ApiUsagePlan>()) {
        render_api_usage_plan_split(app, plan, area, frame);
        return;
    }
    if let Some(domain) = resource.and_then(|r| r.as_any().downcast_ref::<ApiCustomDomain>()) {
        render_api_domain_split(app, domain, area, frame);
        return;
    }
    if let Some(tgw) = resource.and_then(|r| {
        r.as_any()
            .downcast_ref::<crate::aws::services::transit_gateway::TransitGateway>()
    }) {
        render_tgw_split(app, tgw, area, frame);
        return;
    }
    if let Some(rt) = resource.and_then(|r| {
        r.as_any()
            .downcast_ref::<crate::aws::services::transit_gateway::TgwRouteTable>()
    }) {
        render_tgw_route_table_split(app, rt, area, frame);
        return;
    }
    if let Some(ep) = resource.and_then(|r| r.as_any().downcast_ref::<VpcEndpoint>()) {
        render_vpc_endpoint_split(app, ep, area, frame);
        return;
    }
    if let Some(vpn) = resource.and_then(|r| r.as_any().downcast_ref::<VpnConnection>()) {
        render_vpn_connection_split(app, vpn, area, frame);
        return;
    }
    if let Some(rule) = resource.and_then(|r| r.as_any().downcast_ref::<ConfigRule>()) {
        render_config_rule_split(app, rule, area, frame);
        return;
    }
    if let Some(acl) = resource.and_then(|r| r.as_any().downcast_ref::<WafWebAcl>()) {
        render_waf_web_acl_split(app, acl, area, frame);
        return;
    }
    if let Some(s) = resource.and_then(|r| {
        r.as_any().downcast_ref::<crate::aws::services::waf::WafIpSet>()
    }) {
        render_waf_ip_set_split(app, s, area, frame);
        return;
    }
    if let Some(s) = resource.and_then(|r| {
        r.as_any().downcast_ref::<crate::aws::services::waf::WafRuleGroup>()
    }) {
        render_waf_rule_group_split(app, s, area, frame);
        return;
    }
    if let Some(e) = resource.and_then(|r| {
        r.as_any()
            .downcast_ref::<crate::aws::services::route53resolver::ResolverEndpoint>()
    }) {
        render_resolver_endpoint_split(app, e, area, frame);
        return;
    }
    if let Some(rule) = resource.and_then(|r| {
        r.as_any()
            .downcast_ref::<crate::aws::services::route53resolver::ResolverRule>()
    }) {
        render_resolver_rule_split(app, rule, area, frame);
        return;
    }
    if let Some(p) = resource.and_then(|r| {
        r.as_any()
            .downcast_ref::<crate::aws::services::route53profiles::Route53Profile>()
    }) {
        render_route53_profile_split(app, p, area, frame);
        return;
    }
    if let Some(r) = resource.and_then(|r| {
        r.as_any()
            .downcast_ref::<crate::aws::services::agentcore::AgentCoreRuntime>()
    }) {
        render_agentcore_runtime_split(app, r, area, frame);
        return;
    }
    if let Some(g) = resource.and_then(|r| {
        r.as_any()
            .downcast_ref::<crate::aws::services::agentcore::AgentCoreGateway>()
    }) {
        render_agentcore_gateway_split(app, g, area, frame);
        return;
    }
    if let Some(m) = resource.and_then(|r| {
        r.as_any()
            .downcast_ref::<crate::aws::services::agentcore::AgentCoreMemory>()
    }) {
        render_agentcore_memory_split(app, m, area, frame);
        return;
    }
    if let Some(b) = resource.and_then(|r| {
        r.as_any()
            .downcast_ref::<crate::aws::services::agentcore::AgentCoreBrowser>()
    }) {
        render_agentcore_browser_split(app, b, area, frame);
        return;
    }
    if let Some(c) = resource.and_then(|r| {
        r.as_any()
            .downcast_ref::<crate::aws::services::agentcore::AgentCoreCodeInterpreter>()
    }) {
        render_agentcore_code_interpreter_split(app, c, area, frame);
        return;
    }
    if let Some(p) = resource.and_then(|r| {
        r.as_any()
            .downcast_ref::<crate::aws::services::agentcore::AgentCorePolicy>()
    }) {
        render_agentcore_policy_split(app, p, area, frame);
        return;
    }
    if let Some(reg) = resource.and_then(|r| {
        r.as_any()
            .downcast_ref::<crate::aws::services::agentcore::AgentCoreRegistry>()
    }) {
        render_agentcore_registry_split(app, reg, area, frame);
        return;
    }
    if let Some(e) = resource.and_then(|r| {
        r.as_any()
            .downcast_ref::<crate::aws::services::agentcore::AgentCorePolicyEngine>()
    }) {
        render_agentcore_policy_engine_split(app, e, area, frame);
        return;
    }
    if let Some(w) = resource.and_then(|r| {
        r.as_any()
            .downcast_ref::<crate::aws::services::agentcore::AgentCoreWorkloadIdentity>()
    }) {
        render_agentcore_workload_identity_split(app, w, area, frame);
        return;
    }
    if let Some(o) = resource.and_then(|r| {
        r.as_any()
            .downcast_ref::<crate::aws::services::agentcore::AgentCoreOAuth2Provider>()
    }) {
        render_agentcore_oauth2_provider_split(app, o, area, frame);
        return;
    }
    if let Some(k) = resource.and_then(|r| {
        r.as_any()
            .downcast_ref::<crate::aws::services::agentcore::AgentCoreApiKeyProvider>()
    }) {
        render_agentcore_api_key_provider_split(app, k, area, frame);
        return;
    }
    if let Some(h) = resource.and_then(|r| {
        r.as_any()
            .downcast_ref::<crate::aws::services::agentcore::AgentCoreHarness>()
    }) {
        render_agentcore_harness_split(app, h, area, frame);
        return;
    }
    if let Some(b) = resource.and_then(|r| {
        r.as_any()
            .downcast_ref::<crate::aws::services::agentcore::AgentCoreConfigBundle>()
    }) {
        render_agentcore_bundle_split(app, b, area, frame);
        return;
    }
    if let Some(m) = resource.and_then(|r| {
        r.as_any()
            .downcast_ref::<crate::aws::services::agentcore::AgentCorePaymentManager>()
    }) {
        render_agentcore_payment_manager_split(app, m, area, frame);
        return;
    }
    if let Some(p) = resource.and_then(|r| {
        r.as_any()
            .downcast_ref::<crate::aws::services::agentcore::AgentCorePaymentCredProvider>()
    }) {
        render_agentcore_payment_cred_split(app, p, area, frame);
        return;
    }
    if let Some(alarm) = resource.and_then(|r| r.as_any().downcast_ref::<CwAlarm>()) {
        render_cw_alarm_split(app, alarm, area, frame);
        return;
    }
    if let Some(ca) = resource.and_then(|r| r.as_any().downcast_ref::<CwCompositeAlarm>()) {
        render_cw_composite_alarm_split(app, ca, area, frame);
        return;
    }
    if let Some(dash) = resource.and_then(|r| r.as_any().downcast_ref::<CwDashboard>()) {
        render_cw_dashboard_split(app, dash, area, frame);
        return;
    }
    if let Some(lg) = resource.and_then(|r| r.as_any().downcast_ref::<CwLogGroup>()) {
        render_cw_log_group_split(app, lg, area, frame);
        return;
    }
    if let Some(ms) = resource.and_then(|r| {
        r.as_any()
            .downcast_ref::<crate::aws::services::cloudwatch::CwMetricStream>()
    }) {
        render_cw_metric_stream_split(app, ms, area, frame);
        return;
    }
    if let Some(rule) = resource.and_then(|r| {
        r.as_any()
            .downcast_ref::<crate::aws::services::cloudwatch::CwInsightRule>()
    }) {
        render_cw_insight_rule_split(app, rule, area, frame);
        return;
    }
    if let Some(pol) = resource.and_then(|r| {
        r.as_any()
            .downcast_ref::<crate::aws::services::cloudwatch::CwAccountPolicy>()
    }) {
        render_cw_account_policy_split(app, pol, area, frame);
        return;
    }
    if let Some(sink) = resource.and_then(|r| {
        r.as_any()
            .downcast_ref::<crate::aws::services::oam::OamSink>()
    }) {
        render_oam_sink_split(app, sink, area, frame);
        return;
    }
    if let Some(link) = resource.and_then(|r| {
        r.as_any()
            .downcast_ref::<crate::aws::services::oam::OamLink>()
    }) {
        render_oam_link_split(app, link, area, frame);
        return;
    }
    if let Some(db) = resource.and_then(|r| r.as_any().downcast_ref::<RdsInstance>()) {
        render_rds_instance_split(app, db, area, frame);
        return;
    }
    if let Some(cluster) = resource.and_then(|r| r.as_any().downcast_ref::<RdsCluster>()) {
        render_rds_cluster_split(app, cluster, area, frame);
        return;
    }
    if let Some(lb) = resource.and_then(|r| r.as_any().downcast_ref::<LoadBalancer>()) {
        render_load_balancer_split(app, lb, area, frame);
        return;
    }
    if let Some(tg) = resource.and_then(|r| r.as_any().downcast_ref::<TargetGroup>()) {
        render_target_group_split(app, tg, area, frame);
        return;
    }
    if let Some(group) = resource.and_then(|r| r.as_any().downcast_ref::<AsgGroup>()) {
        render_asg_group_split(app, group, area, frame);
        return;
    }
    if let Some(cluster) = resource.and_then(|r| r.as_any().downcast_ref::<EcsCluster>()) {
        render_ecs_cluster_split(app, cluster, area, frame);
        return;
    }
    if let Some(svc) = resource.and_then(|r| r.as_any().downcast_ref::<EcsServiceInfo>()) {
        render_ecs_service_split(app, svc, area, frame);
        return;
    }
    if let Some(td) = resource.and_then(|r| r.as_any().downcast_ref::<EcsTaskDefinition>()) {
        render_ecs_taskdef_split(app, td, area, frame);
        return;
    }
    if let Some(task) = resource.and_then(|r| r.as_any().downcast_ref::<EcsTask>()) {
        render_ecs_task_split(app, task, area, frame);
        return;
    }
    if let Some(queue) = resource.and_then(|r| r.as_any().downcast_ref::<SqsQueue>()) {
        render_sqs_queue_split(app, queue, area, frame);
        return;
    }
    if let Some(topic) = resource.and_then(|r| r.as_any().downcast_ref::<SnsTopic>()) {
        render_sns_topic_split(app, topic, area, frame);
        return;
    }
    if let Some(secret) = resource.and_then(|r| r.as_any().downcast_ref::<SecretEntry>()) {
        render_secret_split(app, secret, area, frame);
        return;
    }
    if let Some(param) = resource.and_then(|r| r.as_any().downcast_ref::<SsmParameter>()) {
        render_ssm_parameter_split(app, param, area, frame);
        return;
    }
    if let Some(doc) = resource.and_then(|r| r.as_any().downcast_ref::<SsmDocument>()) {
        render_ssm_document_split(app, doc, area, frame);
        return;
    }
    if let Some(assoc) = resource.and_then(|r| r.as_any().downcast_ref::<SsmAssociation>()) {
        render_ssm_association_split(app, assoc, area, frame);
        return;
    }
    if let Some(inst) = resource.and_then(|r| r.as_any().downcast_ref::<SsmManagedInstance>()) {
        render_ssm_fleet_split(app, inst, area, frame);
        return;
    }
    if let Some(cmd) = resource.and_then(|r| r.as_any().downcast_ref::<SsmCommand>()) {
        render_ssm_command_split(app, cmd, area, frame);
        return;
    }
    if let Some(auto) = resource.and_then(|r| r.as_any().downcast_ref::<SsmAutomationExecution>()) {
        render_ssm_automation_split(app, auto, area, frame);
        return;
    }
    if let Some(mw) = resource.and_then(|r| r.as_any().downcast_ref::<SsmMaintWindow>()) {
        render_ssm_maint_window_split(app, mw, area, frame);
        return;
    }
    if let Some(b) = resource.and_then(|r| r.as_any().downcast_ref::<SsmPatchBaseline>()) {
        render_ssm_baseline_split(app, b, area, frame);
        return;
    }
    if let Some(item) = resource.and_then(|r| r.as_any().downcast_ref::<SsmOpsItem>()) {
        render_ssm_ops_item_split(app, item, area, frame);
        return;
    }
    if let Some(item) = resource.and_then(|r| r.as_any().downcast_ref::<CostLineItem>()) {
        render_cost_split(app, item, area, frame);
        return;
    }
    if let Some(a) = resource
        .and_then(|r| r.as_any().downcast_ref::<crate::aws::services::cost::CostAnomaly>())
    {
        render_simple_split(
            app,
            area,
            frame,
            "Cost Anomaly",
            a.name(),
            a.summary(),
            &descriptor_tabs(app, &crate::aws::services::cost::COST_ANOMALY_SECTIONS),
        );
        return;
    }
    if let Some(role) = resource.and_then(|r| r.as_any().downcast_ref::<IamRole>()) {
        render_iam_role_split(app, role, area, frame);
        return;
    }
    if let Some(finding) =
        resource.and_then(|r| r.as_any().downcast_ref::<AccessAnalyzerFinding>())
    {
        render_access_analyzer_split(app, finding, area, frame);
        return;
    }
    if let Some(policy) = resource.and_then(|r| r.as_any().downcast_ref::<IamPolicy>()) {
        render_iam_policy_split(app, policy, area, frame);
        return;
    }
    if let Some(user) = resource.and_then(|r| r.as_any().downcast_ref::<IamUser>()) {
        render_iam_user_split(app, user, area, frame);
        return;
    }
    if let Some(idp) = resource.and_then(|r| r.as_any().downcast_ref::<IamIdentityProvider>()) {
        render_iam_idp_split(app, idp, area, frame);
        return;
    }
    if let Some(settings) = resource.and_then(|r| r.as_any().downcast_ref::<IamAccountSettings>()) {
        render_iam_account_split(app, settings, area, frame);
        return;
    }
    if let Some(group) = resource.and_then(|r| r.as_any().downcast_ref::<IamGroup>()) {
        render_iam_group_split(app, group, area, frame);
        return;
    }
    if let Some(account) = resource.and_then(|r| r.as_any().downcast_ref::<OrgAccount>()) {
        render_org_account_split(app, account, area, frame);
        return;
    }
    if let Some(scp) = resource.and_then(|r| r.as_any().downcast_ref::<OrgScp>()) {
        render_org_scp_split(app, scp, area, frame);
        return;
    }
    if let Some(ou) = resource.and_then(|r| r.as_any().downcast_ref::<OrgUnit>()) {
        render_simple_split(
            app,
            area,
            frame,
            "Organizational Unit",
            &ou.ou_name,
            &format!("{}  ·  {}", ou.ou_id, ou.full_path),
            &descriptor_tabs(app, &crate::aws::services::organizations::ORG_OU_SECTIONS),
        );
        return;
    }
    if let Some(bucket) = resource.and_then(|r| r.as_any().downcast_ref::<S3Bucket>()) {
        render_s3_bucket_split(app, bucket, area, frame);
        return;
    }

    let focused = app.details_focused;

    // Pane title: resource type. (S3 buckets render via render_s3_bucket_split
    // above and never reach this flat path.)
    let title = match resource {
        Some(r) => r.resource_type().to_string(),
        None => "Details".to_string(),
    };

    // Contextual hints live in the bottom border instead of cluttering the title
    let footer = if resource.is_none() {
        String::new()
    } else {
        // Flat pane: no numbered sections, raw-view available via `v`.
        let extras = if resource.and_then(|r| r.as_any().downcast_ref::<Invoice>()).is_some() {
            "d download PDF"
        } else {
            ""
        };
        detail_footer(app, focused, 1, extras)
    };

    let mut block = theme::pane_block(&title, focused);
    if !footer.is_empty() {
        block = block.title_bottom(
            Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
                .right_aligned(),
        );
    }

    // (S3 view modes are shown — with their 1–5 keys — in the sub-tab bar.)

    let rows = app.get_detail_lines_filtered();

    if rows.is_empty() && app.detail_search_query.is_empty() {
        let inner_height = area.height.saturating_sub(2);
        let top_pad = (inner_height.saturating_sub(1) / 2) as usize;
        let mut lines = vec![Line::raw(""); top_pad];
        lines.push(Line::styled(
            "No resource selected",
            Style::default().fg(theme::text_dim()),
        ));
        let paragraph = Paragraph::new(lines)
            .block(block)
            .alignment(Alignment::Center);
        frame.render_widget(paragraph, area);
        return;
    }

    // Render the border block first so we can compute the inner area
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Reserve bottom row for the search bar when active
    let has_search = focused
        && (!app.detail_search_query.is_empty() || app.detail_search_active);

    let (content_area, search_area) = if has_search {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(1)])
            .split(inner);
        (chunks[0], Some(chunks[1]))
    } else {
        (inner, None)
    };

    // Selection highlight only applies while the pane is focused
    let selected = if focused {
        app.details_selected_index
    } else {
        None
    };
    let state = resource.map(|r| r.state());

    let cursor = selected.unwrap_or(0);
    let q_lower = app.detail_search_query.to_lowercase();

    let lines = layout_detail_body(app, &rows, content_area, cursor, |idx, key, value, key_w| {
        let line = style_detail_row(key, value, state.as_ref(), key_w);
        let jump = jump_indicator(app, key, value);
        if Some(idx) == selected {
            let sel = theme::selection_style(true);
            let mut spans: Vec<Span> = line
                .spans
                .into_iter()
                .map(|s| Span::styled(s.content, sel))
                .collect();
            if jump.is_some() {
                spans.push(Span::styled("  →", Style::default().fg(theme::aws_orange())));
            }
            Line::from(spans).style(sel)
        } else if !q_lower.is_empty() {
            let combined = format!("{}{}", key, value);
            if combined.to_lowercase().contains(&q_lower) {
                let spans: Vec<Span> = line
                    .spans
                    .into_iter()
                    .map(|s| Span::styled(s.content, s.style.fg(crate::ui::theme::text_primary())))
                    .collect();
                Line::from(spans)
            } else {
                line
            }
        } else if jump.is_some() {
            let mut spans = line.spans;
            spans.push(Span::styled("  →", Style::default().fg(theme::aws_orange())));
            Line::from(spans)
        } else {
            line
        }
    });

    frame.render_widget(Paragraph::new(lines), content_area);

    // Search bar
    if let Some(sb_area) = search_area {
        let match_count = rows.len();
        let match_info = if app.detail_search_query.is_empty() {
            String::new()
        } else {
            format!(
                " ({} match{})",
                match_count,
                if match_count == 1 { "" } else { "es" }
            )
        };
        let search_line = Line::from(vec![
            Span::styled(
                " / ",
                Style::default()
                    .fg(theme::aws_orange())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                app.detail_search_query.clone(),
                Style::default().fg(crate::ui::theme::text_primary()),
            ),
            if app.detail_search_active {
                Span::styled("█", Style::default().fg(theme::aws_orange()))
            } else {
                Span::raw("")
            },
            Span::styled(match_info, Style::default().fg(theme::text_dim())),
            Span::styled(
                "  ⏎ confirm · Esc clear",
                Style::default().fg(theme::text_dim()),
            ),
        ]);
        frame.render_widget(Paragraph::new(search_line), sb_area);
    }
}

/// Replace a lazy-section `Loading…` placeholder row at render time. With the
/// detail pane focused the fetch really is in flight (focus/section keys are
/// what trigger it), so animate the shared tick-driven spinner. Unfocused
/// (list-pane preview) the section is usually *untriggered* — nothing is
/// loading and nothing ever will until the pane is opened — so render a
/// static dim hint instead of a spinner promising progress that never comes.
/// Matches a loading message in either the value (key-value row) or the key
/// (plain content / header row).
fn spin_loading_row(key: &str, value: &str, tick: u64, focused: bool) -> Option<(String, String)> {
    fn is_loading(text: &str) -> bool {
        let t = text.trim_start();
        t.starts_with("Loading") && (t.ends_with('…') || t.ends_with("..."))
    }
    // The "· " prefix is what style_detail_row dims (annotation-row convention).
    const HINT: &str = "· not loaded — ⏎ to open";
    let spin = theme::spinner(tick);
    if is_loading(value) {
        let trimmed = value.trim_start();
        let indent = &value[..value.len() - trimmed.len()];
        let replaced = if focused {
            format!("{indent}{spin} {trimmed}")
        } else {
            format!("{indent}{HINT}")
        };
        Some((key.to_string(), replaced))
    } else if value.is_empty() && is_loading(key) {
        let trimmed = key.trim_start();
        let indent = &key[..key.len() - trimmed.len()];
        if focused {
            Some((format!("{indent}{spin} {trimmed}"), String::new()))
        } else {
            // Keep at least a two-space indent: an unindented key with an
            // empty value styles as a bold group header, not content.
            let indent = if indent.is_empty() { "  " } else { indent };
            Some((format!("{indent}{HINT}"), String::new()))
        }
    } else {
        None
    }
}

/// Style one key/value row from `App::get_detail_lines`.
/// Narrowest key column a detail body ever uses — short-key sections
/// (Overview-style panes) all line up at the same place across services.
pub(crate) const KEY_COL_MIN: usize = 24;
/// Widest the key column may grow, however long the section's keys are.
pub(crate) const KEY_COL_MAX: usize = 48;
/// Columns kept for the value when the pane is narrow: the cap is
/// `content_width − KEY_COL_VALUE_RESERVE`, clamped to `[MIN, MAX]`.
const KEY_COL_VALUE_RESERVE: usize = 20;

/// Is this row a `key: value` pair (the only shape that gets a padded key)?
/// Everything else — blank spacers, group headers, plain content lines,
/// flat-view section headers, empty-key notes — renders without a colon.
fn is_key_value_row(key: &str, value: &str) -> bool {
    !key.is_empty() && !value.is_empty() && !key.starts_with("━━ ")
}

/// Key-column width for every row of a detail body, one entry per row.
///
/// The column is **adaptive per section**: the widest key in the section
/// (by display width, so `⚠`/CJK keys don't drift), clamped to
/// `[KEY_COL_MIN, cap]` where the cap shrinks with the pane so a value
/// always keeps `KEY_COL_VALUE_RESERVE` columns. In the tabbed view the body
/// *is* one section; in the flat view the `━━ Name ━━` headers split the
/// body so a long tag key in Tags doesn't push every Overview value right.
/// Keys wider than the cap are ellipsised by `style_detail_row`, so the `:`
/// lands at the same column for every pair by construction.
fn key_col_widths(rows: &[(String, String)], content_width: u16) -> Vec<usize> {
    use unicode_width::UnicodeWidthStr;
    let cap = (content_width as usize)
        .saturating_sub(KEY_COL_VALUE_RESERVE)
        .clamp(KEY_COL_MIN, KEY_COL_MAX);
    let mut widths = vec![KEY_COL_MIN; rows.len()];
    let fill = |widths: &mut Vec<usize>, start: usize, end: usize| {
        let w = rows[start..end]
            .iter()
            .filter(|(k, v)| is_key_value_row(k, v))
            .map(|(k, _)| k.width())
            .max()
            .unwrap_or(0)
            .clamp(KEY_COL_MIN, cap);
        widths[start..end].iter_mut().for_each(|x| *x = w);
    };
    let mut start = 0;
    for (i, (k, v)) in rows.iter().enumerate() {
        if v.is_empty() && k.starts_with("━━ ") && i > start {
            fill(&mut widths, start, i);
            start = i;
        }
    }
    if start < rows.len() {
        fill(&mut widths, start, rows.len());
    }
    widths
}

/// Pad `key` to exactly `width` display columns, ellipsising a key that
/// doesn't fit (the full text stays in the row tuple for `y`/export).
fn pad_key_to_width(key: &str, width: usize) -> String {
    use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
    let kw = key.width();
    if kw <= width {
        return format!("{}{}", key, " ".repeat(width - kw));
    }
    // Keep `width - 1` columns of the key, then `…`.
    let mut out = String::new();
    let mut used = 0;
    for ch in key.chars() {
        let cw = ch.width().unwrap_or(0);
        if used + cw > width.saturating_sub(1) {
            break;
        }
        out.push(ch);
        used += cw;
    }
    out.push('…');
    used += 1;
    out.push_str(&" ".repeat(width.saturating_sub(used)));
    out
}

fn style_detail_row(
    key: &str,
    value: &str,
    state: Option<&crate::aws::resource::ResourceState>,
    key_w: usize,
) -> Line<'static> {
    // Blank spacer row
    if key.is_empty() && value.is_empty() {
        return Line::raw("");
    }

    // Empty-key note ("" / "No tags", "" / "Loading…"): a value with nothing
    // to pair it with. Render it as an indented content line — the same
    // shape as `("  No tags", "")` — instead of a lone `:` floating at the
    // key column, so both conventions look identical on screen.
    if key.is_empty() {
        let trimmed = value.trim_start();
        let style = if trimmed.starts_with('⚠') {
            Style::default().fg(theme::warning())
        } else if trimmed.starts_with('✗') {
            Style::default().fg(theme::error())
        } else if trimmed.starts_with('✓') {
            Style::default().fg(theme::success())
        } else if trimmed.starts_with("· ") {
            Style::default().fg(crate::ui::theme::text_muted())
        } else {
            Style::default().fg(crate::ui::theme::text_primary())
        };
        return Line::styled(format!("  {}", trimmed), style);
    }

    // Plain content line: indented key with no paired value (template preview, CIDR lists, etc.)
    // Must come before the group-header check to avoid appending ": " to the line.
    if value.is_empty() && key.starts_with(' ') {
        // Warning/error content lines (the `error_rows` helper, ⚠/✗ status notes)
        // get their glyph's color — a failed lazy fetch shouldn't read as body text.
        let trimmed = key.trim_start();
        if trimmed.starts_with('⚠') {
            return Line::styled(key.to_string(), Style::default().fg(theme::warning()));
        }
        if trimmed.starts_with('✗') {
            return Line::styled(key.to_string(), Style::default().fg(theme::error()));
        }
        // Annotation row ("· …", e.g. the unfocused not-loaded hint): dimmed
        // so it reads as a note about the pane, not resource data.
        if trimmed.starts_with("· ") {
            return Line::styled(
                key.to_string(),
                Style::default().fg(crate::ui::theme::text_muted()),
            );
        }
        // A tab separates fixed-width content from a trailing annotation (e.g. a
        // security-group rule's description). Render the note dimly so it reads
        // as a comment on the rule rather than another aligned column.
        if let Some((content, note)) = key.split_once('\t') {
            return Line::from(vec![
                Span::raw(content.to_string()),
                Span::styled(
                    format!("  — {}", note),
                    Style::default()
                        .fg(crate::ui::theme::text_muted())
                        .add_modifier(Modifier::ITALIC),
                ),
            ]);
        }
        return Line::raw(key.to_string());
    }

    // Flat-view section header ("━━ Name ━━━…", inserted by
    // `App::assemble_flat_detail`): accent-bold so sections read apart from
    // magenta group headers.
    if value.is_empty() && key.starts_with("━━ ") {
        return Line::from(Span::styled(
            key.to_string(),
            Style::default()
                .fg(theme::aws_orange())
                .add_modifier(Modifier::BOLD),
        ));
    }

    // Group header: a key with no value (e.g. "Encryption", "Tags")
    if value.is_empty() && !key.starts_with(' ') {
        return Line::from(Span::styled(
            key.to_string(),
            Style::default()
                .fg(crate::ui::theme::heading())
                .add_modifier(Modifier::BOLD),
        ));
    }

    // State row gets the state color
    let value_style = if key == "State" {
        let color = state
            .map(|s| theme::state_indicator(s).1)
            .unwrap_or(crate::ui::theme::text_primary());
        Style::default().fg(color).add_modifier(Modifier::BOLD)
    } else if value.starts_with('✓') {
        Style::default().fg(theme::success())
    } else if value.starts_with('✗') {
        Style::default().fg(theme::error())
    } else if value.starts_with('⚠') {
        Style::default().fg(theme::warning())
    } else if value.trim_start().starts_with("· ") {
        // Annotation value (the unfocused not-loaded hint) — dim, like the
        // plain-content "· " rows above.
        Style::default().fg(crate::ui::theme::text_muted())
    } else {
        Style::default().fg(crate::ui::theme::text_primary())
    };

    // Align the value column: pad the key to the body's key-column width
    // (`key_col_widths`) by display width so the ":" lands in one place.
    let padded_key = pad_key_to_width(key, key_w);
    Line::from(vec![
        Span::styled(format!("{}: ", padded_key), Style::default().fg(theme::accent())),
        Span::styled(value.to_string(), value_style),
    ])
}

/// S3 bucket split detail pane — consistent with every other single-list
/// service: a fixed header, an in-pane section tab bar (the six view modes),
/// and a scrollable body. The object browser is a separate mode (the `Bucket |
/// Objects` top row + `o`), not a section here.
fn render_s3_bucket_split(app: &App, bucket: &S3Bucket, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 6, "o objects");
    let mut block = theme::pane_block("S3 Bucket", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let region = if bucket.region.is_empty() {
        "—".to_string()
    } else {
        bucket.region.clone()
    };
    let header = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                bucket.name.clone(),
                Style::default().fg(crate::ui::theme::text_primary()).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(region, Style::default().fg(theme::accent())),
            Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
            Span::styled(
                match bucket.versioning {
                    crate::aws::services::s3::VersioningStatus::Enabled => "versioning on",
                    crate::aws::services::s3::VersioningStatus::Suspended => "versioning suspended",
                    crate::aws::services::s3::VersioningStatus::Disabled => "versioning off",
                },
                Style::default().fg(theme::text_dim()),
            ),
        ]),
        Line::raw(""),
    ];
    let header_h = header.len() as u16;

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(header_h),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(inner);
    frame.render_widget(Paragraph::new(header), chunks[0]);
    render_hr(chunks[1], frame);
    render_section_tab_bar(
        app,
        chunks[2],
        frame,
        &descriptor_tabs(app, &crate::aws::services::s3::S3_BUCKET_SECTIONS),
    );
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

/// Get S3 bucket details for a section.
/// Used by `App::get_detail_lines` so rendering, navigation, and copy
/// all see exactly the same rows.
pub fn s3_bucket_details(
    bucket: &S3Bucket,
    section: crate::aws::services::s3::S3BucketDetailSection,
    state: Option<&crate::lazy::Lazy<Box<S3BucketDetails>>>,
    account_id: Option<&str>,
    metrics: Option<&crate::lazy::Lazy<Box<crate::aws::services::s3::S3StorageMetrics>>>,
    filesystems: Option<&crate::lazy::Lazy<Vec<crate::aws::services::s3files::S3FileSystem>>>,
) -> Vec<(String, String)> {
    use crate::aws::services::s3::S3BucketDetailSection as S;
    // Tier-2 config is fetched lazily; `cfg` is Some once loaded, and `loading`
    // is true while the fetch is in flight (or about to start on this view).
    let cfg = match state {
        Some(crate::lazy::Lazy::Loaded(d)) => Some(d.as_ref()),
        _ => None,
    };
    if let Some(crate::lazy::Lazy::Error(e)) = state {
        if matches!(section, S::Security | S::Operations | S::Website | S::Advanced) {
            return error_rows(e);
        }
    }
    let loading = !matches!(state, Some(crate::lazy::Lazy::Loaded(_)));

    match section {
        S::Overview => render_default_details(bucket),
        S::Security => render_security_details(bucket, cfg, loading),
        S::Operations => render_operations_details(bucket, cfg, loading),
        S::Website => render_website_details(bucket, cfg, loading),
        S::Advanced => render_advanced_details(bucket, cfg, loading),
        S::Metadata => render_bucket_metadata(bucket, account_id, metrics),
        S::FileSystems => match filesystems {
            None | Some(crate::lazy::Lazy::Loading) => {
                vec![("".to_string(), "Loading file systems…".to_string())]
            }
            Some(crate::lazy::Lazy::Error(e)) => error_rows(e),
            Some(crate::lazy::Lazy::Loaded(list)) => {
                if list.is_empty() {
                    return vec![
                        ("".to_string(), "No S3 file systems linked to this bucket".to_string()),
                        (String::new(), String::new()),
                        (
                            "  · S3 Files exposes a bucket as an NFS file system — @s3files lists them all"
                                .to_string(),
                            String::new(),
                        ),
                    ];
                }
                let mut rows = vec![(format!("File Systems ({})", list.len()), String::new())];
                rows.push((String::new(), String::new()));
                for fs in list {
                    rows.push((fs.name.clone(), String::new()));
                    rows.push(("  ID".to_string(), fs.id.clone()));
                    rows.push(("  Status".to_string(), fs.status.clone()));
                    if let Some(c) = &fs.created {
                        rows.push(("  Created".to_string(), c.clone()));
                    }
                    // The s3files ARN row is the Enter-jump into @s3files.
                    rows.push(("  ARN".to_string(), fs.arn.clone()));
                    rows.push((String::new(), String::new()));
                }
                rows
            }
        },
        S::Tags => {
            let tags = bucket.tags();
            if tags.is_empty() {
                return vec![("  (no tags)".to_string(), String::new())];
            }
            let mut rows: Vec<(String, String)> =
                tags.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            rows.sort();
            rows
        }
    }
}

/// Metadata view — console-style identity/properties + CloudWatch storage.
fn render_bucket_metadata(
    bucket: &S3Bucket,
    account_id: Option<&str>,
    metrics: Option<&crate::lazy::Lazy<Box<crate::aws::services::s3::S3StorageMetrics>>>,
) -> Vec<(String, String)> {
    use crate::aws::services::s3::fmt_object_size;
    // Identity / properties only — posture (encryption, versioning, public
    // access) lives in the Overview / Security views to avoid duplication.
    let mut rows = vec![
        ("Identity".to_string(), String::new()), // group header
        ("Bucket Name".to_string(), bucket.name.clone()),
        ("Bucket ARN".to_string(), format!("arn:aws:s3:::{}", bucket.name)),
        ("Region".to_string(), bucket.region.clone()),
        (
            "Owner Account".to_string(),
            account_id.unwrap_or("—").to_string(),
        ),
        ("Created".to_string(), bucket.creation_date.clone()),
        (String::new(), String::new()),
        ("Storage (CloudWatch, ~24h)".to_string(), String::new()), // group header
    ];

    match metrics {
        None | Some(crate::lazy::Lazy::Loading) => {
            rows.push(("Objects".to_string(), "Loading…".to_string()));
        }
        Some(crate::lazy::Lazy::Error(e)) => {
            rows.extend(error_rows(e));
        }
        Some(crate::lazy::Lazy::Loaded(m)) => {
            rows.push((
                "Objects".to_string(),
                m.object_count
                    .map(|c| format!("~{}", fmt_count(c)))
                    .unwrap_or_else(|| "n/a".to_string()),
            ));
            rows.push((
                "Total Size".to_string(),
                m.size_bytes
                    .map(fmt_object_size)
                    .unwrap_or_else(|| "n/a".to_string()),
            ));
            if let Some(as_of) = &m.as_of {
                rows.push(("As Of".to_string(), as_of.clone()));
            } else {
                rows.push((
                    "  (note)".to_string(),
                    "no datapoints — new bucket, or needs cloudwatch:GetMetricData".to_string(),
                ));
            }
        }
    }
    rows
}

/// Thousands-separated integer (e.g. 1240 -> "1,240").
fn fmt_count(n: i64) -> String {
    let s = n.abs().to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    if n < 0 {
        format!("-{}", out)
    } else {
        out
    }
}

/// A short placeholder value used while Tier-2 config is still loading.
fn tier2_pending(loading: bool) -> String {
    if loading {
        "Loading…".to_string()
    } else {
        "Not configured".to_string()
    }
}

/// Default view - standard overview with all sections
/// Overview — an at-a-glance posture digest. Identity/storage live in the
/// Metadata view; deep config lives in the Security / Operations views, so this
/// stays a concise one-line-per-area summary.
fn render_default_details(bucket: &S3Bucket) -> Vec<(String, String)> {
    let mut rows = vec![
        ("Bucket".to_string(), bucket.name.clone()),
        ("Region".to_string(), bucket.region.clone()),
        ("Objects".to_string(), "o to browse files →".to_string()),
        (String::new(), String::new()),
        ("Security".to_string(), String::new()), // group header
    ];

    rows.push((
        "Public Access".to_string(),
        match &bucket.public_access_block {
            Some(pab) if pab.all_blocked() => "✓ Fully blocked".to_string(),
            Some(pab) if pab.partially_blocked() => "⚠ Partially blocked".to_string(),
            Some(_) => "✗ Public access allowed".to_string(),
            None => "⚠ Unknown — GetPublicAccessBlock failed".to_string(),
        },
    ));
    rows.push((
        "Encryption".to_string(),
        bucket.encryption_config.format_summary(),
    ));
    rows.push((
        "Bucket Policy".to_string(),
        match &bucket.bucket_policy {
            Some(p) if p.contains("Access Denied") => "Access denied".to_string(),
            Some(_) => "Configured (2 Security)".to_string(),
            None => "None".to_string(),
        },
    ));

    rows.push((String::new(), String::new()));
    rows.push(("Data Management".to_string(), String::new())); // group header
    rows.push((
        "Versioning".to_string(),
        match bucket.versioning {
            crate::aws::services::s3::VersioningStatus::Enabled => "✓ Enabled".to_string(),
            crate::aws::services::s3::VersioningStatus::Suspended => {
                "⚠ Suspended — old versions still retained".to_string()
            }
            crate::aws::services::s3::VersioningStatus::Disabled => "Disabled".to_string(),
        },
    ));
    rows.push((
        "Lifecycle".to_string(),
        if bucket.lifecycle_rules_count > 0 {
            format!(
                "{} rule{}",
                bucket.lifecycle_rules_count,
                if bucket.lifecycle_rules_count == 1 { "" } else { "s" }
            )
        } else {
            "None".to_string()
        },
    ));
    rows.push((
        "Access Logging".to_string(),
        match &bucket.logging_config {
            Some(l) => format!("→ s3://{}/{}", l.target_bucket, l.target_prefix),
            None => "Disabled".to_string(),
        },
    ));

    rows
}

/// Security view - focused on security and compliance
fn render_security_details(
    bucket: &S3Bucket,
    cfg: Option<&S3BucketDetails>,
    loading: bool,
) -> Vec<(String, String)> {
    let mut details = vec![
        ("Bucket Name".to_string(), bucket.name.clone()),
        ("Region".to_string(), bucket.region.clone()),
        ("".to_string(), "".to_string()),
    ];

    // Public Access Block with detailed breakdown
    details.push(("Public Access Block".to_string(), "".to_string()));
    match &bucket.public_access_block {
        Some(pab) => {
            let onoff = |b: bool| if b { "✓ Enabled" } else { "✗ Disabled" }.to_string();
            details.push(("  Block Public ACLs".to_string(), onoff(pab.block_public_acls)));
            details.push(("  Ignore Public ACLs".to_string(), onoff(pab.ignore_public_acls)));
            details.push(("  Block Public Policy".to_string(), onoff(pab.block_public_policy)));
            details.push((
                "  Restrict Public Buckets".to_string(),
                onoff(pab.restrict_public_buckets),
            ));
            let overall_status = if pab.all_blocked() {
                "✓ Fully Blocked (Secure)"
            } else if pab.partially_blocked() {
                "⚠ Partially Blocked"
            } else {
                "✗ Public Access Allowed"
            };
            details.push(("  Overall Status".to_string(), overall_status.to_string()));
        }
        None => {
            details.push(("  Overall Status".to_string(), "⚠ Unknown".to_string()));
            details.push((
                "  · GetPublicAccessBlock failed — needs s3:GetBucketPublicAccessBlock"
                    .to_string(),
                String::new(),
            ));
        }
    }
    details.push(("".to_string(), "".to_string()));

    // Bucket Policy — an Enter-able link when one is configured.
    if let Some(policy) = &bucket.bucket_policy {
        if policy.contains("Access Denied") {
            details.push(("Bucket Policy".to_string(), policy.clone()));
        } else {
            details.push((
                "Bucket Policy".to_string(),
                "Configured  ·  ⏎ open  ·  e edit".to_string(),
            ));
        }
    } else {
        details.push(("Bucket Policy".to_string(), "Not configured".to_string()));
    }
    // GetBucketPolicyStatus — AWS's own public/not-public verdict (Tier 2).
    match cfg.map(|c| c.policy_status_public) {
        Some(Some(true)) => {
            details.push(("  Policy Status".to_string(), "⚠ PUBLIC".to_string()));
        }
        Some(Some(false)) => {
            details.push(("  Policy Status".to_string(), "✓ Not public".to_string()));
        }
        // No verdict (no policy / call failed) or Tier-2 still loading — the
        // Bucket Policy row above already covers the interesting part.
        Some(None) | None => {}
    }
    details.push(("".to_string(), "".to_string()));

    // Encryption
    details.push(("Encryption".to_string(), "".to_string()));
    let enc = &bucket.encryption_config;
    details.push((
        "  Status".to_string(),
        if enc.enabled { "Enabled" } else { "Disabled" }.to_string(),
    ));

    if enc.enabled {
        if let Some(algo) = &enc.algorithm {
            if algo.contains("kms") {
                details.push(("  Algorithm".to_string(), "SSE-KMS".to_string()));
                if let Some(key_id) = &enc.kms_master_key_id {
                    let key_display = if key_id.len() > 40 {
                        format!("{}...", &key_id[..40])
                    } else {
                        key_id.clone()
                    };
                    details.push(("  KMS Key ID".to_string(), key_display));
                }
                details.push((
                    "  S3 Bucket Key".to_string(),
                    if enc.bucket_key_enabled {
                        "Enabled"
                    } else {
                        "Disabled"
                    }
                    .to_string(),
                ));
            } else {
                details.push(("  Algorithm".to_string(), format!("SSE-S3 ({})", algo)));
            }
        }
    }
    details.push(("".to_string(), "".to_string()));

    // Object Lock (Tier 2 data)
    match cfg.and_then(|c| c.object_lock_config.as_ref()) {
        Some(obj_lock) => {
            details.push(("Object Lock".to_string(), "".to_string()));
            details.push((
                "  Status".to_string(),
                if obj_lock.enabled {
                    "Enabled"
                } else {
                    "Disabled"
                }
                .to_string(),
            ));
            if obj_lock.enabled {
                if let Some(mode) = &obj_lock.mode {
                    details.push(("  Mode".to_string(), mode.clone()));
                }
                if let Some(days) = obj_lock.retention_days {
                    details.push(("  Default Retention".to_string(), format!("{} days", days)));
                }
            }
        }
        None => {
            details.push(("Object Lock".to_string(), tier2_pending(loading)));
        }
    }
    details.push(("".to_string(), "".to_string()));

    // Access Control (Tier 2 data)
    details.push(("Access Control".to_string(), "".to_string()));
    match cfg {
        Some(c) => {
            details.push((
                "  Ownership Controls".to_string(),
                c.ownership_controls.clone().unwrap_or_else(|| "—".to_string()),
            ));
            match &c.acl_grants {
                Some(grants) if !grants.is_empty() => {
                    details.push(("  ACL Grants".to_string(), grants.len().to_string()));
                    for g in grants {
                        details.push((
                            "    Grant".to_string(),
                            if g.public {
                                format!("⚠ {} — {}", g.grantee, g.permission)
                            } else {
                                format!("{} — {}", g.grantee, g.permission)
                            },
                        ));
                    }
                }
                Some(_) => {
                    details.push(("  ACL Grants".to_string(), "None".to_string()));
                }
                None => {
                    details.push(("  ACL Grants".to_string(), "—".to_string()));
                }
            }
        }
        None => {
            details.push(("  Status".to_string(), tier2_pending(loading)));
        }
    }

    details
}

/// Operations view - focused on operations and management
fn render_operations_details(
    bucket: &S3Bucket,
    cfg: Option<&S3BucketDetails>,
    loading: bool,
) -> Vec<(String, String)> {
    let mut details = vec![
        ("Bucket Name".to_string(), bucket.name.clone()),
        ("Region".to_string(), bucket.region.clone()),
        ("".to_string(), "".to_string()),
    ];

    // Versioning
    details.push(("Versioning".to_string(), "".to_string()));
    details.push((
        "  Status".to_string(),
        match bucket.versioning {
            crate::aws::services::s3::VersioningStatus::Enabled => "✓ Enabled".to_string(),
            crate::aws::services::s3::VersioningStatus::Suspended => {
                "⚠ Suspended — existing versions still retained (and billed)".to_string()
            }
            crate::aws::services::s3::VersioningStatus::Disabled => "Disabled".to_string(),
        },
    ));
    if !matches!(
        bucket.versioning,
        crate::aws::services::s3::VersioningStatus::Disabled
    ) {
        details.push((
            "  MFA Delete".to_string(),
            if bucket.mfa_delete { "✓ Enabled" } else { "Disabled" }.to_string(),
        ));
    }
    details.push(("".to_string(), "".to_string()));

    // Lifecycle Rules
    if bucket.lifecycle_rules_count > 0 {
        details.push((
            "Lifecycle Rules".to_string(),
            format!(
                "{} rule{} configured",
                bucket.lifecycle_rules_count,
                if bucket.lifecycle_rules_count == 1 {
                    ""
                } else {
                    "s"
                }
            ),
        ));
        if let Some(summary) = &bucket.lifecycle_summary {
            details.push(("  Summary".to_string(), summary.clone()));
        }
    } else {
        details.push(("Lifecycle Rules".to_string(), "Not configured".to_string()));
    }

    details.push(("".to_string(), "".to_string()));
    details.push(("Logging & Monitoring".to_string(), "".to_string()));
    details.push(("".to_string(), "".to_string()));

    // Access Logging
    if let Some(logging) = &bucket.logging_config {
        details.push(("Access Logging".to_string(), "Enabled".to_string()));
        details.push(("  Target Bucket".to_string(), logging.target_bucket.clone()));
        details.push(("  Target Prefix".to_string(), logging.target_prefix.clone()));
    } else {
        details.push(("Access Logging".to_string(), "Not configured".to_string()));
    }
    details.push(("".to_string(), "".to_string()));

    // Transfer Acceleration (Tier 2 data)
    let accel = cfg.and_then(|c| c.acceleration_status.clone());
    details.push((
        "Transfer Acceleration".to_string(),
        accel.unwrap_or_else(|| tier2_pending(loading)),
    ));
    details.push(("".to_string(), "".to_string()));

    // Replication (Tier 2 data)
    details.push(("Replication".to_string(), "".to_string()));
    match cfg.and_then(|c| c.replication_config.as_ref()) {
        Some(replication) => {
            details.push((
                "  Status".to_string(),
                if replication.enabled {
                    "Enabled"
                } else {
                    "Disabled"
                }
                .to_string(),
            ));
            if replication.enabled {
                details.push((
                    "  Total Rules".to_string(),
                    replication.rules_count.to_string(),
                ));
                if !replication.destinations.is_empty() {
                    details.push(("  Destinations".to_string(), "".to_string()));
                    for (idx, dest) in replication.destinations.iter().enumerate().take(3) {
                        details.push((format!("    Dest {}", idx + 1), dest.clone()));
                    }
                    if replication.destinations.len() > 3 {
                        details.push((
                            "    ...".to_string(),
                            format!("and {} more", replication.destinations.len() - 3),
                        ));
                    }
                }
            }
        }
        None => {
            details.push(("  Status".to_string(), tier2_pending(loading)));
        }
    }

    details
}

/// Website view - focused on static website hosting
fn render_website_details(
    bucket: &S3Bucket,
    cfg: Option<&S3BucketDetails>,
    loading: bool,
) -> Vec<(String, String)> {
    let mut details = vec![
        ("Bucket Name".to_string(), bucket.name.clone()),
        ("Region".to_string(), bucket.region.clone()),
        ("".to_string(), "".to_string()),
    ];

    let cfg = match cfg {
        Some(c) => c,
        None => {
            details.push(("Status".to_string(), tier2_pending(loading)));
            return details;
        }
    };

    if let Some(website) = &cfg.website_config {
        if website.enabled {
            details.push(("Status".to_string(), "Enabled".to_string()));
            details.push(("".to_string(), "".to_string()));

            details.push(("Index Document".to_string(), website.index_document.clone()));
            if let Some(error_doc) = &website.error_document {
                details.push(("Error Document".to_string(), error_doc.clone()));
            }
            if let Some(redirect) = &website.redirect_all_requests_to {
                details.push(("Redirect All To".to_string(), redirect.clone()));
            }

            details.push(("".to_string(), "".to_string()));
            let endpoint = format!(
                "http://{}.s3-website-{}.amazonaws.com",
                bucket.name, bucket.region
            );
            details.push(("Website Endpoint".to_string(), endpoint));
        } else {
            details.push(("Status".to_string(), "Not enabled".to_string()));
        }
    } else {
        details.push(("Status".to_string(), "Not configured".to_string()));
        details.push(("".to_string(), "".to_string()));
        details.push((
            "Info".to_string(),
            "Static website hosting is not configured for this bucket.".to_string(),
        ));
    }

    details
}

/// Advanced view - focused on CORS, notifications, analytics
fn render_advanced_details(
    bucket: &S3Bucket,
    cfg: Option<&S3BucketDetails>,
    loading: bool,
) -> Vec<(String, String)> {
    let mut details = vec![
        ("Bucket Name".to_string(), bucket.name.clone()),
        ("Region".to_string(), bucket.region.clone()),
        ("".to_string(), "".to_string()),
    ];

    let cfg = match cfg {
        Some(c) => c,
        None => {
            details.push(("CORS".to_string(), tier2_pending(loading)));
            details.push(("Event Notifications".to_string(), tier2_pending(loading)));
            details.push(("Analytics & Metrics".to_string(), tier2_pending(loading)));
            return details;
        }
    };

    // CORS Configuration
    if let Some(cors_rules) = &cfg.cors_rules {
        if !cors_rules.is_empty() {
            details.push((
                "CORS".to_string(),
                format!(
                    "{} rule{} configured",
                    cors_rules.len(),
                    if cors_rules.len() == 1 { "" } else { "s" }
                ),
            ));
            for (idx, rule) in cors_rules.iter().enumerate().take(2) {
                details.push(("".to_string(), "".to_string()));
                details.push((format!("Rule {}", idx + 1), "".to_string()));
                details.push((
                    "  Allowed Origins".to_string(),
                    rule.allowed_origins.join(", "),
                ));
                details.push((
                    "  Allowed Methods".to_string(),
                    rule.allowed_methods.join(", "),
                ));
                if let Some(headers) = &rule.allowed_headers {
                    details.push(("  Allowed Headers".to_string(), headers.join(", ")));
                }
                if let Some(max_age) = rule.max_age_seconds {
                    details.push(("  Max Age".to_string(), format!("{} seconds", max_age)));
                }
            }
            if cors_rules.len() > 2 {
                details.push((
                    "  ...".to_string(),
                    format!(
                        "and {} more rule{}",
                        cors_rules.len() - 2,
                        if cors_rules.len() - 2 == 1 { "" } else { "s" }
                    ),
                ));
            }
        } else {
            details.push(("CORS".to_string(), "No rules configured".to_string()));
        }
    } else {
        details.push(("CORS".to_string(), "Not configured".to_string()));
    }
    details.push(("".to_string(), "".to_string()));

    // Event Notifications — the target ARNs are the payload (and each ARN row
    // is an Enter-jump to its SNS topic / SQS queue / Lambda function).
    if let Some(notif) = &cfg.notification_config {
        let total = notif.topics.len() + notif.queues.len() + notif.lambda_functions.len();
        if total > 0 || notif.eventbridge {
            details.push((
                "Event Notifications".to_string(),
                if total > 0 {
                    format!("{} configured", total)
                } else {
                    String::new()
                },
            ));
            if notif.eventbridge {
                details.push(("  EventBridge".to_string(), "✓ Enabled".to_string()));
            }
            for arn in &notif.topics {
                details.push(("  SNS Topic".to_string(), arn.clone()));
            }
            for arn in &notif.queues {
                details.push(("  SQS Queue".to_string(), arn.clone()));
            }
            for arn in &notif.lambda_functions {
                details.push(("  Lambda".to_string(), arn.clone()));
            }
        } else {
            details.push((
                "Event Notifications".to_string(),
                "Not configured".to_string(),
            ));
        }
    } else {
        details.push((
            "Event Notifications".to_string(),
            "Not configured".to_string(),
        ));
    }
    details.push(("".to_string(), "".to_string()));

    // Analytics & Metrics
    details.push(("Analytics & Metrics".to_string(), "".to_string()));
    details.push(("".to_string(), "".to_string()));

    if let Some(count) = cfg.inventory_configs_count {
        details.push(("Inventory Configurations".to_string(), count.to_string()));
    } else {
        details.push((
            "Inventory Configurations".to_string(),
            "Not configured".to_string(),
        ));
    }

    if let Some(count) = cfg.analytics_configs_count {
        details.push(("Analytics Configurations".to_string(), count.to_string()));
    } else {
        details.push((
            "Analytics Configurations".to_string(),
            "Not configured".to_string(),
        ));
    }

    if let Some(count) = cfg.metrics_configs_count {
        details.push(("Metrics Configurations".to_string(), count.to_string()));
    } else {
        details.push((
            "Metrics Configurations".to_string(),
            "Not configured".to_string(),
        ));
    }

    if let Some(count) = cfg.intelligent_tiering_count {
        details.push((
            "Intelligent-Tiering".to_string(),
            format!(
                "{} configuration{}",
                count,
                if count == 1 { "" } else { "s" }
            ),
        ));
    } else {
        details.push((
            "Intelligent-Tiering".to_string(),
            "Not configured".to_string(),
        ));
    }

    details
}

// ── Jump classifiers (`Enter` → resource_jump_target) ────────────────────────

const HEADER_LABEL_W: usize = 14; // fixed label column width in the header

/// Returns the EC2 sub-tab view and resource ID to jump to when Enter is pressed
/// on the given row key in the given section, or `None` if the row is not jumpable.
///
/// SG rows:  key = "  sg-0abc123" (trimmed starts with "sg-")
/// ENI rows: key = "eth0  eni-0abc123def456" (contains "eni-")
/// Unified "go to resource" detector. Given a detail row (and the service it's
/// shown in), return the resource it references — which service, which sub-tab,
/// and the id to select. `Enter` follows targets in the *current* service; `gd`
/// follows any target (including cross-service, e.g. an EC2 instance's IAM role
/// or its subnet/VPC). Recognizes IAM role/policy ARNs and the common id
/// prefixes; `sg-` resolves to EC2 or VPC's Security Groups tab based on where
/// the row is shown.
pub fn resource_jump_target(
    key: &str,
    value: &str,
    current: crate::aws::service::ServiceType,
) -> Option<crate::app::JumpTarget> {
    use crate::app::{CfnView, DxView, Ec2View, IamView, JumpTarget, JumpView, OrgView, ResolverView, TgwView, VpcView};
    use crate::aws::service::ServiceType;

    let mk = |service, view, id: &str| {
        Some(JumpTarget {
            service,
            view,
            id: id.to_string(),
        })
    };

    // 0. Nested-stack references: parent/root stack ARNs (arn:…:stack/name/guid).
    // Jump by the stack *name* (segment after ":stack/") so the fuzzy match is
    // clean — feeding the full colon-bearing ARN to the query parser misfires.
    for field in [value, key] {
        if let Some(idx) = field.find(":stack/") {
            let name = field[idx + ":stack/".len()..]
                .split('/')
                .next()
                .unwrap_or("");
            if !name.is_empty() {
                return mk(
                    ServiceType::CloudFormation,
                    JumpView::Cfn(CfnView::Stacks),
                    name,
                );
            }
        }
    }

    // 0a. A "Jump To" row whose value is `@prefix id` (X-Ray service-map
    // nodes naming a Lambda / table / bucket): switch service, resolve by
    // exact id then name.
    if key.trim() == "Jump To" {
        if let Some((prefix, id)) = value.trim().strip_prefix('@').and_then(|v| v.split_once(' ')) {
            if let Some(service) = ServiceType::from_prefix(prefix) {
                return mk(service, JumpView::None, id.trim());
            }
        }
    }

    // 0b. Ownership: the CFN stack-name tag row (bare name, no ARN — the
    // `:stack-id` tag's ARN value is caught by rule 0 above) jumps to the
    // owning stack. Key may arrive indented (`map_tags_lines` shape).
    if key.trim().eq_ignore_ascii_case("aws:cloudformation:stack-name") {
        let name = value.trim();
        if !name.is_empty() {
            return mk(ServiceType::CloudFormation, JumpView::Cfn(CfnView::Stacks), name);
        }
    }

    // 1. IAM role / policy / identity-provider ARNs (cross-service from
    // anywhere they appear — e.g. a trust policy's Federated principal).
    for field in [value, key] {
        for (marker, view, strip_path) in [
            (":role/", JumpView::Iam(IamView::Roles), true),
            (":policy/", JumpView::Iam(IamView::Policies), true),
            // Provider names keep their slashes: an OIDC provider's name IS
            // the issuer host + path (e.g. an EKS "…/id/HEX" issuer).
            (":saml-provider/", JumpView::Iam(IamView::IdentityProviders), false),
            (":oidc-provider/", JumpView::Iam(IamView::IdentityProviders), false),
        ] {
            if let Some(idx) = field.find(marker) {
                let rest = &field[idx + marker.len()..];
                let raw = rest
                    .split(|c: char| c.is_whitespace() || c == '"' || c == ',')
                    .next()
                    .unwrap_or("");
                // Strip any IAM path; the role/policy *name* is the last segment.
                let name = if strip_path {
                    raw.rsplit('/').next().unwrap_or(raw)
                } else {
                    raw
                };
                if !name.is_empty() {
                    return mk(ServiceType::IAM, view, name);
                }
            }
        }
    }

    // 1b. Bare KMS alias references (e.g. SNS's KmsMasterKeyId = "alias/aws/sns").
    // Full alias ARNs are handled by the generic ARN classifier below; this
    // catches the prefix-only form. Jump by the alias name (KmsKey stores
    // aliases with "alias/" stripped, so the fuzzy match lands cleanly).
    for field in [value, key] {
        if let Some(rest) = field.trim().strip_prefix("alias/") {
            if !rest.is_empty() {
                return mk(ServiceType::Kms, JumpView::None, rest);
            }
        }
    }

    // 2. Resource-id prefixes. Order longest-first so e.g. "igw-"/"i-" don't clash.
    for field in [value, key] {
        for tok in field.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-')) {
            let after = |pfx: &str| tok.len() > pfx.len() && tok.starts_with(pfx);

            // Resolver rule ids (rslvr-rr-…): checked before the bare
            // "rslvr-" endpoint fallback below, else a rule id would
            // misroute to the Endpoints sub-tab and never resolve.
            if after("rslvr-rr-") {
                return mk(
                    ServiceType::Route53Resolver,
                    JumpView::Resolver(ResolverView::Rules),
                    tok,
                );
            }
            // Resolver endpoint ids (rslvr-in-… / rslvr-out-…): jump to the
            // Resolver Endpoints sub-tab and search the id.
            if after("rslvr-") {
                return mk(
                    ServiceType::Route53Resolver,
                    JumpView::Resolver(ResolverView::Endpoints),
                    tok,
                );
            }
            // Direct Connect ids: connection / VIF / LAG each has its own sub-tab.
            if after("dxcon-") {
                return mk(
                    ServiceType::DirectConnect,
                    JumpView::Dx(DxView::Connections),
                    tok,
                );
            }
            if after("dxvif-") {
                return mk(
                    ServiceType::DirectConnect,
                    JumpView::Dx(DxView::VirtualInterfaces),
                    tok,
                );
            }
            if after("dxlag-") {
                return mk(ServiceType::DirectConnect, JumpView::Dx(DxView::Lags), tok);
            }
            // Transit Gateway ids — longest-first so tgw-attach-/tgw-rtb- don't
            // fall through to the bare tgw- gateway rule.
            if after("tgw-attach-") {
                return mk(
                    ServiceType::TransitGateway,
                    JumpView::Tgw(TgwView::Attachments),
                    tok,
                );
            }
            if after("tgw-rtb-") {
                return mk(
                    ServiceType::TransitGateway,
                    JumpView::Tgw(TgwView::RouteTables),
                    tok,
                );
            }
            if after("tgw-") {
                return mk(
                    ServiceType::TransitGateway,
                    JumpView::Tgw(TgwView::TransitGateways),
                    tok,
                );
            }
            if after("vol-") {
                return mk(ServiceType::EC2, JumpView::Ec2(Ec2View::EbsVolumes), tok);
            }
            // snap- → the account-wide Snapshots sub-tab (this view *is* the
            // snapshots list AMI block-device / volume source ids jump to).
            if after("snap-") {
                return mk(ServiceType::EC2, JumpView::Ec2(Ec2View::Snapshots), tok);
            }
            // EC2 instance id (i-…). `after` matches the exact "i-" prefix, so
            // "igw-"/"ami-"/other tokens that merely start with 'i' don't hit this.
            if after("i-") {
                return mk(ServiceType::EC2, JumpView::Ec2(Ec2View::Instances), tok);
            }
            if after("eni-") {
                return mk(
                    ServiceType::EC2,
                    JumpView::Ec2(Ec2View::NetworkInterfaces),
                    tok,
                );
            }
            if after("subnet-") {
                return mk(ServiceType::VPC, JumpView::Vpc(VpcView::Subnets), tok);
            }
            // Also matches endpoint-service ids (vpce-svc-…) — same tab.
            if after("vpce-") {
                return mk(ServiceType::VPC, JumpView::Vpc(VpcView::Endpoints), tok);
            }
            if after("vpc-") {
                return mk(ServiceType::VPC, JumpView::Vpc(VpcView::Vpcs), tok);
            }
            if after("rtb-") {
                return mk(ServiceType::VPC, JumpView::Vpc(VpcView::Routing), tok);
            }
            if after("igw-") {
                return mk(ServiceType::VPC, JumpView::Vpc(VpcView::Gateways), tok);
            }
            if after("nat-") {
                return mk(ServiceType::VPC, JumpView::Vpc(VpcView::Gateways), tok);
            }
            if after("acl-") {
                return mk(ServiceType::VPC, JumpView::Vpc(VpcView::NetworkAcls), tok);
            }
            if after("dopt-") {
                return mk(ServiceType::VPC, JumpView::Vpc(VpcView::DhcpOptions), tok);
            }
            if after("pcx-") {
                return mk(ServiceType::VPC, JumpView::Vpc(VpcView::Peering), tok);
            }
            if after("pl-") {
                return mk(ServiceType::VPC, JumpView::Vpc(VpcView::Routing), tok);
            }
            if after("eigw-") {
                return mk(ServiceType::VPC, JumpView::Vpc(VpcView::Gateways), tok);
            }
            if after("vgw-") {
                return mk(
                    ServiceType::VPC,
                    JumpView::Vpc(VpcView::VpnConnections),
                    tok,
                );
            }
            if after("cgw-") {
                return mk(
                    ServiceType::VPC,
                    JumpView::Vpc(VpcView::VpnConnections),
                    tok,
                );
            }
            if after("sg-") {
                // Both EC2 and VPC have a Security Groups tab — stay in the
                // service the row is being viewed in (default to EC2).
                return if current == ServiceType::VPC {
                    mk(ServiceType::VPC, JumpView::Vpc(VpcView::SecurityGroups), tok)
                } else {
                    mk(ServiceType::EC2, JumpView::Ec2(Ec2View::SecurityGroups), tok)
                };
            }
            // EC2 instance id: require a hex body to avoid matching stray "i-…".
            if after("i-") && tok[2..].chars().all(|c| c.is_ascii_hexdigit()) {
                return mk(ServiceType::EC2, JumpView::Ec2(Ec2View::Instances), tok);
            }
            // Organizations SCP policy id (p-…)
            if after("p-") && tok.len() > 2 {
                return mk(
                    ServiceType::Organizations,
                    JumpView::Org(OrgView::Policies),
                    tok,
                );
            }
        }
    }

    // 3. AWS Config non-compliant resources: key is an AWS resource type
    //    (e.g. "AWS::S3::Bucket"), value is the resource id/name.
    if key.starts_with("AWS::") && !value.is_empty() {
        if let Some(target) = cfn_type_jump_target(key, value) {
            return Some(target);
        }
    }

    // 3b. ECR image URIs (<acct>.dkr.ecr.<region>.amazonaws.com/<repo>[:tag]) —
    //     not ARNs, so handle before the ARN fallback. Covers Inspector
    //     container-image findings, ECS task defs, Lambda container images.
    for field in [value, key] {
        if let Some(name) = ecr_repo_from_image_uri(field) {
            // Pinned by digest → the image row itself (`repo@digest` is its
            // id); if that row isn't loaded (older than the per-repo cut),
            // `resolve_pending_jump` falls back to the repository. A tag can't
            // be resolved to a digest without a call, so a tagged reference
            // lands on the repository directly.
            if let Some(digest) = ecr_digest_from_image_uri(field) {
                return mk(
                    ServiceType::Ecr,
                    JumpView::Ecr(crate::app::EcrView::Images),
                    &format!("{}@{}", name, digest),
                );
            }
            return mk(ServiceType::Ecr, JumpView::None, name);
        }
    }

    // 3c. `s3://bucket/prefix` URIs (Glue table locations, crawler S3 targets,
    //     job script locations, etc.) → the S3 bucket list, searched by bucket
    //     name. The bucket is the authority segment between `s3://` and the
    //     first `/`.
    for field in [value, key] {
        if let Some(bucket) = s3_uri_bucket(field) {
            return mk(ServiceType::S3, JumpView::None, bucket);
        }
    }

    // 3d. S3 REST / website endpoint hostnames (CloudFront origin domains:
    //     `bucket.s3.region.amazonaws.com`, `bucket.s3.amazonaws.com`,
    //     `bucket.s3-website-region.amazonaws.com`) → the bucket.
    if let Some(bucket) = s3_origin_domain_bucket(value) {
        return mk(ServiceType::S3, JumpView::None, bucket);
    }

    // 4. Full service ARNs (EventBridge rule targets, SNS subscription endpoints,
    //    etc.) → that service's list, when we have a browsable view for it.
    for field in [value, key] {
        if let Some(target) = arn_jump_target(field) {
            return Some(target);
        }
    }

    None
}

/// Extract the bucket name from an `s3://bucket/key…` (or `s3a://`/`s3n://`,
/// used by Spark/Hadoop-flavoured Glue jobs) URI. Returns `None` for anything
/// that isn't an S3 URI or has an empty bucket.
fn s3_uri_bucket(s: &str) -> Option<&str> {
    let rest = s
        .strip_prefix("s3://")
        .or_else(|| s.strip_prefix("s3a://"))
        .or_else(|| s.strip_prefix("s3n://"))?;
    let bucket = rest.split('/').next().unwrap_or(rest).trim();
    (!bucket.is_empty()).then_some(bucket)
}

/// True for a hostname label that marks the start of an S3 endpoint: `s3`,
/// `s3.<region>` legacy dash forms (`s3-us-west-2`, `s3-fips`), and website
/// endpoints (`s3-website-us-east-1` / `s3-website`). Excludes the non-bucket
/// S3 control planes (`s3-control`, `s3-accesspoint`, `s3-object-lambda`,
/// `s3-outposts`) whose leading label isn't a bucket name.
fn is_s3_endpoint_label(label: &str) -> bool {
    if label == "s3" {
        return true;
    }
    let Some(rest) = label.strip_prefix("s3-") else {
        return false;
    };
    !(rest.starts_with("control")
        || rest.starts_with("accesspoint")
        || rest.starts_with("object-lambda")
        || rest.starts_with("outposts"))
}

/// Extract the bucket name from an S3 REST or website endpoint hostname —
/// the domain form CloudFront origins use. Matches every endpoint generation:
/// `<bucket>.s3.<region>.amazonaws.com`, legacy `<bucket>.s3.amazonaws.com`
/// and dash-region `<bucket>.s3-us-west-2.amazonaws.com`, dualstack/fips, and
/// website endpoints `<bucket>.s3-website[-.]<region>.amazonaws.com`. Bucket
/// names may contain dots, so the split happens at the first S3 endpoint
/// label, not the first dot. Deliberately strict (bare hostname with an
/// `.amazonaws.com` suffix) so ordinary values can't false-positive.
fn s3_origin_domain_bucket(s: &str) -> Option<&str> {
    let host = s.trim();
    if host.contains('/') || host.contains(' ') {
        return None;
    }
    let rest = host.strip_suffix(".amazonaws.com")?;
    let labels: Vec<&str> = rest.split('.').collect();
    // The bucket is everything before the first S3 endpoint label (at least
    // one label must precede it).
    for i in 1..labels.len() {
        if is_s3_endpoint_label(labels[i]) {
            // Byte offset of label i: the preceding labels plus their dots.
            let end = labels[..i].iter().map(|l| l.len()).sum::<usize>() + i - 1;
            let bucket = &rest[..end];
            if !bucket.is_empty() {
                return Some(bucket);
            }
        }
    }
    None
}

/// Map a full service ARN (`arn:aws:<service>:…:<resource>`) to a jump target.
/// Covers the common EventBridge-target services that neboto can browse; returns
/// `None` for services with no list (Step Functions, Kinesis, …) so no marker
/// shows. IAM role/policy ARNs are handled earlier in `resource_jump_target`.
fn arn_jump_target(arn: &str) -> Option<crate::app::JumpTarget> {
    use crate::app::{CloudFrontView, CwView, Ec2View, ElbView, EventBridgeView, JumpView, MsgView, NfwView, RdsView, WafScope, WafView};
    use crate::aws::service::ServiceType;

    if !arn.starts_with("arn:") {
        return None;
    }
    // arn : partition : service : region : account : resource(+possibly more colons)
    let parts: Vec<&str> = arn.splitn(6, ':').collect();
    if parts.len() < 6 {
        return None;
    }
    let (service, resource) = (parts[2], parts[5]);
    let mk = |service, view, id: &str| {
        let id = id.trim();
        (!id.is_empty()).then(|| crate::app::JumpTarget {
            service,
            view,
            id: id.to_string(),
        })
    };
    // Last path/colon segment of the resource part (handles `name`, `type/name`).
    let last_seg = || resource.rsplit(['/', ':']).next().unwrap_or(resource);

    match service {
        // Batch rows are keyed by name (queue / compute environment),
        // `name:revision` (job definition) and job id — the ARN's tail.
        "batch" => {
            let (kind, rest) = resource.split_once('/')?;
            let view = match kind {
                "job-queue" => crate::app::BatchView::Queues,
                "compute-environment" => crate::app::BatchView::ComputeEnvironments,
                "job" => crate::app::BatchView::Jobs,
                "job-definition" => crate::app::BatchView::JobDefinitions,
                _ => return None,
            };
            mk(ServiceType::Batch, JumpView::Batch(view), rest)
        }
        // DMS rows are keyed by ARN (`id()`), so the ARN itself is the
        // target. The view has to travel with it: a same-service jump keeps
        // the current sub-tab otherwise, and its type filter hides the row.
        "dms" => {
            use crate::app::DmsView;
            let view = match resource.split(':').next()? {
                "task" => DmsView::Tasks,
                "rep" => DmsView::Instances,
                "endpoint" => DmsView::Endpoints,
                "replication-config" => DmsView::Serverless,
                _ => return None,
            };
            mk(ServiceType::Dms, JumpView::Dms(view), arn)
        }
        // Beanstalk: environment/<app>/<env>, application/<app>,
        // applicationversion/<app>/<label>. Environments resolve by name
        // (ids are `e-…`, which the ARN doesn't carry); versions are keyed
        // `app@label` because a label is only unique within its app.
        "elasticbeanstalk" => {
            use crate::app::BeanstalkView;
            let (kind, rest) = resource.split_once('/')?;
            let (view, id) = match kind {
                "environment" => (BeanstalkView::Environments, rest.split_once('/')?.1.to_string()),
                "application" => (BeanstalkView::Applications, rest.to_string()),
                "applicationversion" => {
                    let (app, label) = rest.split_once('/')?;
                    (BeanstalkView::Versions, format!("{app}@{label}"))
                }
                _ => return None,
            };
            mk(ServiceType::Beanstalk, JumpView::Beanstalk(view), &id)
        }
        "route53" => {
            // resource = "hostedzone/Z123…". R53HostedZone::id() stores the
            // full "/hostedzone/ID" form, so reconstruct it for an exact-id
            // match rather than jumping with the bare zone id.
            resource.strip_prefix("hostedzone/").and_then(|zid| {
                mk(
                    ServiceType::Route53,
                    JumpView::R53(crate::app::R53View::Zones),
                    &format!("/hostedzone/{zid}"),
                )
            })
        }
        // Layer ARNs are resolved by `lambda_row_jump_target` (own-account
        // layers only); read as a function they'd jump to a function named
        // "layer".
        "lambda" if resource.starts_with("layer:") => None,
        "lambda" => {
            // resource = "function:NAME" (may carry ":VERSION"/":ALIAS")
            let name = resource.strip_prefix("function:").unwrap_or(resource);
            mk(
                ServiceType::Lambda,
                JumpView::None,
                name.split(':').next().unwrap_or(name),
            )
        }
        "cloudwatch" => {
            // resource = "alarm:NAME" → the Alarms sub-tab (resolved by ARN).
            resource
                .starts_with("alarm:")
                .then(|| mk(ServiceType::CloudWatch, JumpView::Cw(CwView::Alarms), arn))
                .flatten()
        }
        "cloudformation" => {
            // resource = "stack/NAME/UUID" → the Stacks sub-tab by name (CFN
            // stacks jump-resolve by exact name). Covers any stack ARN in a
            // detail row — e.g. a Service Catalog provisioned product's
            // physical id.
            resource.strip_prefix("stack/").and_then(|rest| {
                mk(
                    ServiceType::CloudFormation,
                    JumpView::Cfn(crate::app::CfnView::Stacks),
                    rest.split('/').next().unwrap_or(rest),
                )
            })
        }
        // AgentCore ARNs: `runtime/<id>`, `gateway/<id>`, `memory/<id>`,
        // `browser/<id>`, `code-interpreter/<id>`, and the nested
        // `workload-identity-directory/default/workload-identity/<name>`.
        // Each lands on the sub-tab that lists that family; the id/name is the
        // last segment, which is what the resource's `id()` holds.
        "bedrock-agentcore" => {
            use crate::app::AgentCoreView;
            let view = if resource.starts_with("runtime/") {
                Some(AgentCoreView::Runtimes)
            } else if resource.starts_with("gateway/") {
                Some(AgentCoreView::Gateways)
            } else if resource.starts_with("memory/") {
                Some(AgentCoreView::Memory)
            } else if resource.starts_with("harness/") {
                Some(AgentCoreView::Harness)
            } else if resource.starts_with("workload-identity-directory/") {
                Some(AgentCoreView::Identity)
            } else if resource.starts_with("browser/")
                || resource.starts_with("browser-custom/")
                || resource.starts_with("code-interpreter/")
                || resource.starts_with("code-interpreter-custom/")
            {
                Some(AgentCoreView::Tools)
            } else {
                None
            };
            view.and_then(|v| {
                mk(
                    ServiceType::AgentCore,
                    JumpView::AgentCore(v),
                    last_seg(),
                )
            })
        }
        "sqs" => mk(ServiceType::Messaging, JumpView::Msg(MsgView::Queues), resource),
        "sns" => mk(ServiceType::Messaging, JumpView::Msg(MsgView::Topics), resource),
        "cloudfront" => {
            // resource = "distribution/EID" / "function/NAME" /
            // "origin-access-control/EID" → the matching sub-tab.
            if let Some(id) = resource.strip_prefix("distribution/") {
                mk(
                    ServiceType::CloudFront,
                    JumpView::Cf(CloudFrontView::Distributions),
                    id,
                )
            } else if let Some(name) = resource.strip_prefix("function/") {
                mk(
                    ServiceType::CloudFront,
                    JumpView::Cf(CloudFrontView::Functions),
                    name,
                )
            } else if let Some(id) = resource.strip_prefix("origin-access-control/") {
                mk(ServiceType::CloudFront, JumpView::Cf(CloudFrontView::Oacs), id)
            } else {
                None
            }
        }
        // KMS: resource = "key/UUID" (or an alias) → the key list (single-view).
        "kms" => mk(ServiceType::Kms, JumpView::None, last_seg()),
        // A bare bucket ARN (`arn:aws:s3:::name`) → the bucket list. Only the
        // bucket form: access-point/object-lambda ARNs carry region+account.
        "s3" if parts[3].is_empty() && parts[4].is_empty() => mk(
            ServiceType::S3,
            JumpView::None,
            resource.split('/').next().unwrap_or(resource),
        ),
        // S3 Files: resource = "file-system/fs-…" → the @s3files list by id.
        "s3files" => resource
            .strip_prefix("file-system/")
            .and_then(|id| mk(ServiceType::S3Files, JumpView::None, id)),
        // S3 Tables: resource = "bucket/<name>[/table/<uuid>]". Both types use
        // the full ARN as their id, so jump with the whole ARN. The view has
        // to travel with it: a same-service jump (a table's Bucket row) keeps
        // the current sub-tab otherwise, whose type filter hides the target.
        "s3tables" if resource.starts_with("bucket/") => {
            use crate::app::S3TablesView;
            let view = if resource.contains("/table/") {
                S3TablesView::Tables
            } else {
                S3TablesView::Buckets
            };
            mk(ServiceType::S3Tables, JumpView::S3Tables(view), arn)
        }
        "organizations" => {
            // resource = "ou/o-…/ou-…", "account/o-…/<12 digits>" or
            // "root/o-…/r-…" (Control Tower targets, FMS scope rows) — jump by
            // the bare trailing id. A root id may not resolve exactly and just
            // lands in the filtered OU list.
            if resource.starts_with("ou/") || resource.starts_with("root/") {
                mk(
                    ServiceType::Organizations,
                    JumpView::Org(crate::app::OrgView::Ous),
                    last_seg(),
                )
            } else if resource.starts_with("account/") {
                mk(
                    ServiceType::Organizations,
                    JumpView::Org(crate::app::OrgView::Accounts),
                    last_seg(),
                )
            } else {
                None
            }
        }
        "states" => {
            // stateMachine:<name>, execution:<machine>:<name>, or
            // mapRun:<machine>/<uuid>. State machines resolve by name, an
            // execution by its full ARN (its `id()`), which the caller already
            // holds — so pass the ARN straight through for those.
            if resource.starts_with("execution:") {
                mk(
                    ServiceType::StepFunctions,
                    JumpView::Sfn(crate::app::SfnView::Executions),
                    arn,
                )
            } else if let Some(n) = resource.strip_prefix("stateMachine:") {
                mk(
                    ServiceType::StepFunctions,
                    JumpView::Sfn(crate::app::SfnView::StateMachines),
                    // `stateMachine:<name>:<version-or-alias>` — the machine's
                    // own id is the unqualified ARN, so match on the name.
                    n.split(':').next().unwrap_or(n),
                )
            } else {
                None
            }
        }
        // CodeSuite. Which form to emit is decided by each type's `id()`:
        // projects / repos are keyed by ARN, pipelines by name, and a deploy
        // group by an opaque UUID (so that one matches on `name()` instead).
        "codebuild" => mk(
            ServiceType::Code,
            JumpView::Code(crate::app::CodeView::BuildProjects),
            arn,
        ),
        "codecommit" => mk(
            ServiceType::Code,
            JumpView::Code(crate::app::CodeView::Repositories),
            arn,
        ),
        "codeartifact" => mk(
            ServiceType::Code,
            JumpView::Code(crate::app::CodeView::Artifacts),
            arn,
        ),
        "codepipeline" => mk(
            ServiceType::Code,
            JumpView::Code(crate::app::CodeView::Pipelines),
            // `<name>` or `<name>/<stage>/<action>`.
            resource.split('/').next().unwrap_or(resource),
        ),
        "codedeploy" => {
            // `deploymentgroup:<app>/<group>` — `CodeDeployGroup::id()` is an
            // opaque UUID, so aim at its `name()` ("<app> / <group>").
            if let Some(rest) = resource.strip_prefix("deploymentgroup:") {
                match rest.split_once('/') {
                    Some((app, group)) => mk(
                        ServiceType::Code,
                        JumpView::Code(crate::app::CodeView::Deployments),
                        &format!("{} / {}", app, group),
                    ),
                    None => None,
                }
            } else if let Some(app) = resource.strip_prefix("application:") {
                // No group named — land on the tab filtered to the app.
                mk(
                    ServiceType::Code,
                    JumpView::Code(crate::app::CodeView::Deployments),
                    app,
                )
            } else {
                None
            }
        }
        "logs" => {
            // resource = "log-group:NAME:*"
            let name = resource.strip_prefix("log-group:").unwrap_or(resource);
            mk(
                ServiceType::CloudWatch,
                JumpView::Cw(CwView::LogGroups),
                name.strip_suffix(":*").unwrap_or(name),
            )
        }
        // EKS is a single-list service keyed by cluster name (Batch's
        // EKS-backed compute environments name their cluster by ARN).
        "eks" => resource
            .strip_prefix("cluster/")
            .and_then(|n| mk(ServiceType::Eks, JumpView::None, n)),
        "ecs" => {
            // service/<cluster>/<name> or cluster/<name>
            if resource.starts_with("service/") || resource.starts_with("cluster/") {
                mk(ServiceType::ECS, JumpView::None, last_seg())
            } else {
                None
            }
        }
        "rds" => {
            // db:NAME or cluster:NAME
            if let Some(n) = resource.strip_prefix("db:") {
                mk(ServiceType::RDS, JumpView::Rds(RdsView::Instances), n)
            } else if let Some(n) = resource.strip_prefix("cluster:") {
                mk(ServiceType::RDS, JumpView::Rds(RdsView::Clusters), n)
            } else {
                None
            }
        }
        "ec2" => {
            // EventBridge can target EC2 instances: instance/i-xxxx
            resource
                .strip_prefix("instance/")
                .and_then(|id| mk(ServiceType::EC2, JumpView::Ec2(Ec2View::Instances), id))
        }
        "elasticloadbalancing" => {
            // resource = "targetgroup/NAME/id" or "loadbalancer/app|net|gwy/NAME/id".
            // Jump by the *name* segment (searchable), which is the second path
            // component for a target group and the third for a load balancer.
            let segs: Vec<&str> = resource.split('/').collect();
            match segs.first().copied() {
                Some("targetgroup") => segs
                    .get(1)
                    .and_then(|name| mk(ServiceType::Elb, JumpView::Elb(ElbView::TargetGroups), name)),
                Some("loadbalancer") => segs
                    .get(2)
                    .and_then(|name| mk(ServiceType::Elb, JumpView::Elb(ElbView::LoadBalancers), name)),
                _ => None,
            }
        }
        "network-firewall" => {
            // resource = "firewall/NAME" / "firewall-policy/NAME" /
            // "stateful-rulegroup/NAME" / "stateless-rulegroup/NAME".
            let (kind, name) = match resource.split_once('/') {
                Some((k, n)) => (k, n),
                None => return None,
            };
            let view = match kind {
                "firewall" => JumpView::Nfw(NfwView::Firewalls),
                "firewall-policy" => JumpView::Nfw(NfwView::Policies),
                "stateful-rulegroup" | "stateless-rulegroup" => {
                    JumpView::Nfw(NfwView::RuleGroups)
                }
                _ => return None,
            };
            mk(ServiceType::NetworkFirewall, view, name)
        }
        "wafv2" => {
            // resource = "<scope>/webacl/<name>/<id>" or
            // "<scope>/rulegroup/<name>/<id>" (FMS WAF policies reference rule
            // groups) where scope is "global" (CLOUDFRONT) or "regional". Jump
            // by the trailing id and carry the scope so the jump lands on the
            // right (us-east-1 vs regional) WAF variant.
            let segs: Vec<&str> = resource.split('/').collect();
            let view = match segs.get(1).copied() {
                Some("webacl") => WafView::WebAcls,
                Some("rulegroup") => WafView::RuleGroups,
                _ => return None,
            };
            let scope = match segs.first().copied() {
                Some("global") => WafScope::CloudFront,
                _ => WafScope::Regional,
            };
            let id = segs.get(3).copied().unwrap_or("");
            mk(ServiceType::Waf, JumpView::Waf(view, scope), id)
        }
        "acm" => {
            // resource = "certificate/<uuid>"; ACM rows key on the full ARN, so
            // jump by the ARN itself (resolves in-region — CloudFront certs are
            // us-east-1).
            resource
                .starts_with("certificate/")
                .then(|| mk(ServiceType::Acm, JumpView::None, arn))
                .flatten()
        }
        "ecr" => {
            // resource = "repository/NAME". NAME may contain slashes (namespaced
            // repos like team/app), so strip the prefix rather than taking the
            // last path segment. ECR is single-view; jump by name (== the repo's
            // id()).
            resource
                .strip_prefix("repository/")
                .and_then(|name| mk(ServiceType::Ecr, JumpView::None, name))
        }
        "events" => {
            // event-bus/NAME, rule/[BUS/]NAME, archive/NAME — each lands on its
            // EventBridge sub-tab. Other events resources (api-destination,
            // connection) have no list here.
            if resource.starts_with("event-bus/") {
                mk(
                    ServiceType::EventBridge,
                    JumpView::Eb(EventBridgeView::EventBuses),
                    last_seg(),
                )
            } else if resource.starts_with("rule/") {
                mk(
                    ServiceType::EventBridge,
                    JumpView::Eb(EventBridgeView::Rules),
                    last_seg(),
                )
            } else if resource.starts_with("archive/") {
                mk(
                    ServiceType::EventBridge,
                    JumpView::Eb(EventBridgeView::Archives),
                    last_seg(),
                )
            } else {
                None
            }
        }
        "scheduler" => {
            // resource = "schedule/GROUP/NAME"
            resource.strip_prefix("schedule/").and_then(|_| {
                mk(
                    ServiceType::EventBridge,
                    JumpView::Eb(EventBridgeView::Schedules),
                    last_seg(),
                )
            })
        }
        "pipes" => {
            // resource = "pipe/NAME"
            resource.strip_prefix("pipe/").and_then(|name| {
                mk(
                    ServiceType::EventBridge,
                    JumpView::Eb(EventBridgeView::Pipes),
                    name,
                )
            })
        }
        _ => None,
    }
}

/// The `sha256:…` digest of a digest-pinned ECR image URI, if it has one.
fn ecr_digest_from_image_uri(s: &str) -> Option<&str> {
    ecr_repo_from_image_uri(s)?;
    let d = s.split_once('@')?.1.trim();
    d.starts_with("sha256:").then_some(d)
}

/// Extract the repository name from an ECR image URI
/// (`<acct>.dkr.ecr.<region>.amazonaws.com/<repo>[:tag][@sha256:…]`). Returns
/// `None` for anything that isn't an ECR image reference. The repo name may
/// contain slashes; the `:tag` / `@digest` suffix is stripped.
fn ecr_repo_from_image_uri(s: &str) -> Option<&str> {
    if !s.contains(".dkr.ecr.") {
        return None;
    }
    let after_host = s.split_once(".amazonaws.com/")?.1;
    // Strip the digest first (@sha256:…), then the tag (:tag). Repo names can
    // contain '/' but never ':' or '@'.
    let repo = after_host.split('@').next().unwrap_or(after_host);
    let repo = repo.split(':').next().unwrap_or(repo).trim();
    (!repo.is_empty()).then_some(repo)
}

/// Console-style "jump to the physical resource" for a CloudFormation stack's
/// Resources section. Routes by the CloudFormation `Type` (e.g. `AWS::S3::Bucket`)
/// — far more reliable than sniffing the physical id — and derives a searchable
/// key from the physical id (extracting the name from ARNs / queue URLs where
/// needed). Returns `None` for types we can't route to a searchable list.
pub fn cfn_type_jump_target(
    resource_type: &str,
    physical_id: &str,
) -> Option<crate::app::JumpTarget> {
    use crate::app::{ApiGatewayView, CfnView, CloudFrontView, CwView, Ec2View, EcsView, ElbView, EventBridgeView, IamView, JumpTarget, JumpView, MsgView, R53View, RdsView, SfnView, SsmView, TgwView, VpcView};
    use crate::aws::service::ServiceType::*;
    use crate::aws::services::code::CodeView;

    // Name = last '/'-segment (queue URLs, ECS/IAM ARNs, R53 paths).
    let after_slash = || physical_id.rsplit('/').next().unwrap_or(physical_id);
    // Name = last ':'-segment (SNS topic ARNs).
    let after_colon = || physical_id.rsplit(':').next().unwrap_or(physical_id);
    // ELB/TargetGroup ARN: arn:…:loadbalancer/app/NAME/hash → NAME (2nd-from-last).
    let elb_name = || {
        physical_id
            .rsplit('/')
            .nth(1)
            .filter(|s| !s.is_empty())
            .unwrap_or(physical_id)
    };

    let (service, view, id) = match resource_type {
        "AWS::EC2::Instance" => (EC2, JumpView::Ec2(Ec2View::Instances), physical_id),
        "AWS::EC2::Volume" => (EC2, JumpView::Ec2(Ec2View::EbsVolumes), physical_id),
        "AWS::EC2::SecurityGroup" => (EC2, JumpView::Ec2(Ec2View::SecurityGroups), physical_id),
        "AWS::EC2::NetworkInterface" => {
            (EC2, JumpView::Ec2(Ec2View::NetworkInterfaces), physical_id)
        }
        "AWS::EC2::Subnet" => (VPC, JumpView::Vpc(VpcView::Subnets), physical_id),
        "AWS::EC2::VPC" => (VPC, JumpView::Vpc(VpcView::Vpcs), physical_id),
        "AWS::EC2::DHCPOptions" => (VPC, JumpView::Vpc(VpcView::DhcpOptions), physical_id),
        "AWS::EC2::RouteTable" => (VPC, JumpView::Vpc(VpcView::Routing), physical_id),
        "AWS::EC2::InternetGateway" => (VPC, JumpView::Vpc(VpcView::Gateways), physical_id),
        "AWS::EC2::NatGateway" => (VPC, JumpView::Vpc(VpcView::Gateways), physical_id),
        "AWS::EC2::NetworkAcl" => (VPC, JumpView::Vpc(VpcView::NetworkAcls), physical_id),
        "AWS::EC2::VPCPeeringConnection" => {
            (VPC, JumpView::Vpc(VpcView::Peering), physical_id)
        }
        "AWS::EC2::PrefixList" => (VPC, JumpView::Vpc(VpcView::Routing), physical_id),
        "AWS::EC2::EgressOnlyInternetGateway" => {
            (VPC, JumpView::Vpc(VpcView::Gateways), physical_id)
        }
        // VPC-side and TGW-side EC2 types — without these the service-token
        // fallback would misroute them to the EC2 instances screen.
        "AWS::EC2::VPCEndpoint" | "AWS::EC2::VPCEndpointService" => {
            (VPC, JumpView::Vpc(VpcView::Endpoints), physical_id)
        }
        "AWS::EC2::TransitGateway" => {
            (TransitGateway, JumpView::Tgw(TgwView::TransitGateways), physical_id)
        }
        "AWS::EC2::TransitGatewayAttachment" | "AWS::EC2::TransitGatewayVpcAttachment" => {
            (TransitGateway, JumpView::Tgw(TgwView::Attachments), physical_id)
        }
        "AWS::EC2::TransitGatewayRouteTable" => {
            (TransitGateway, JumpView::Tgw(TgwView::RouteTables), physical_id)
        }
        "AWS::EC2::VPNGateway" => (VPC, JumpView::Vpc(VpcView::VpnConnections), physical_id),
        "AWS::EC2::CustomerGateway" => {
            (VPC, JumpView::Vpc(VpcView::VpnConnections), physical_id)
        }
        "AWS::EC2::VPNConnection" => {
            (VPC, JumpView::Vpc(VpcView::VpnConnections), physical_id)
        }
        "AWS::IAM::Role" => (IAM, JumpView::Iam(IamView::Roles), after_slash()),
        "AWS::IAM::User" => (IAM, JumpView::Iam(IamView::Users), after_slash()),
        "AWS::IAM::Group" => (IAM, JumpView::Iam(IamView::Groups), after_slash()),
        "AWS::IAM::ManagedPolicy" => (IAM, JumpView::Iam(IamView::Policies), after_slash()),
        "AWS::S3::Bucket" => (S3, JumpView::None, physical_id),
        "AWS::Lambda::Function" => (Lambda, JumpView::None, physical_id),
        "AWS::AutoScaling::AutoScalingGroup" => (Asg, JumpView::None, physical_id),
        "AWS::RDS::DBInstance" => (RDS, JumpView::Rds(RdsView::Instances), physical_id),
        "AWS::RDS::DBCluster" => (RDS, JumpView::Rds(RdsView::Clusters), physical_id),
        "AWS::ECS::Cluster" => (ECS, JumpView::Ecs(EcsView::Clusters), after_slash()),
        "AWS::ECS::Service" => (ECS, JumpView::Ecs(EcsView::Services), after_slash()),
        // Physical id is the repository name (may be namespaced with slashes).
        "AWS::ECR::Repository" => (Ecr, JumpView::None, physical_id),
        "AWS::ElasticLoadBalancingV2::LoadBalancer" => {
            (Elb, JumpView::Elb(ElbView::LoadBalancers), elb_name())
        }
        "AWS::ElasticLoadBalancingV2::TargetGroup" => {
            (Elb, JumpView::Elb(ElbView::TargetGroups), elb_name())
        }
        "AWS::SNS::Topic" => (Messaging, JumpView::Msg(MsgView::Topics), after_colon()),
        "AWS::SQS::Queue" => (Messaging, JumpView::Msg(MsgView::Queues), after_slash()),
        "AWS::CloudWatch::Alarm" => (CloudWatch, JumpView::Cw(CwView::Alarms), physical_id),
        "AWS::Logs::LogGroup" => (CloudWatch, JumpView::Cw(CwView::LogGroups), physical_id),
        "AWS::SSM::Parameter" => (Ssm, JumpView::Ssm(SsmView::Parameters), physical_id),
        "AWS::Route53::HostedZone" => (Route53, JumpView::R53(R53View::Zones), after_slash()),
        "AWS::CloudFormation::Stack" => {
            // Nested stack: physical id is the stack ARN; jump by name.
            let name = physical_id
                .find(":stack/")
                .map(|i| &physical_id[i + ":stack/".len()..])
                .and_then(|s| s.split('/').next())
                .unwrap_or(physical_id);
            (CloudFormation, JumpView::Cfn(CfnView::Stacks), name)
        }
        "AWS::DynamoDB::Table" => (DynamoDb, JumpView::None, physical_id),
        "AWS::KMS::Key" => (Kms, JumpView::None, physical_id),
        "AWS::EFS::FileSystem" => (Efs, JumpView::None, physical_id),
        "AWS::EKS::Cluster" => (Eks, JumpView::None, physical_id),
        "AWS::Kinesis::Stream" => (Kinesis, JumpView::None, physical_id),
        // arn:…:stateMachine:Name → Name.
        "AWS::StepFunctions::StateMachine" => {
            (StepFunctions, JumpView::Sfn(SfnView::StateMachines), after_colon())
        }
        "AWS::ApiGateway::RestApi" => {
            (ApiGateway, JumpView::ApiGw(ApiGatewayView::RestApis), physical_id)
        }
        "AWS::ApiGatewayV2::Api" => {
            (ApiGateway, JumpView::ApiGw(ApiGatewayView::HttpApis), physical_id)
        }
        "AWS::Events::Rule" => (EventBridge, JumpView::Eb(EventBridgeView::Rules), after_slash()),
        "AWS::CloudFront::Distribution" => {
            (CloudFront, JumpView::Cf(CloudFrontView::Distributions), physical_id)
        }
        "AWS::CodeBuild::Project" => (Code, JumpView::Code(CodeView::BuildProjects), physical_id),
        // Secret ARN's last ':'-segment is "name-SUFFIX" — lands as a fuzzy
        // search that shows the secret even though exact-id resolution misses.
        "AWS::SecretsManager::Secret" => (Secrets, JumpView::Secrets, after_colon()),
        // arn:…:task-definition/family:rev → "family:rev" ("family" isn't a
        // known service prefix, so the colon stays a literal fuzzy term).
        "AWS::ECS::TaskDefinition" => {
            (ECS, JumpView::Ecs(EcsView::TaskDefinitions), after_slash())
        }
        "AWS::Cognito::UserPool" => (Cognito, JumpView::None, physical_id),
        "AWS::ElastiCache::CacheCluster" | "AWS::ElastiCache::ReplicationGroup" => {
            (ElastiCache, JumpView::None, physical_id)
        }
        "AWS::OpenSearchService::Domain" => (OpenSearch, JumpView::None, physical_id),
        "AWS::Athena::WorkGroup" => (Athena, JumpView::None, physical_id),
        "AWS::Glue::Job" => (Glue, JumpView::None, physical_id),
        "AWS::Redshift::Cluster" => (Redshift, JumpView::None, physical_id),
        // arn:…:cluster/name/uuid → name (2nd-from-last, the ELB-ARN shape).
        "AWS::MSK::Cluster" => (Msk, JumpView::None, elb_name()),
        "AWS::Transfer::Server" => (Transfer, JumpView::None, physical_id),
        // Anything else: route by the type's service token — lands in the
        // service with the physical id seeded as a search (the query-style
        // jump), rather than silently not jumping.
        _ => return cfn_service_fallback_jump_target(resource_type, physical_id),
    };

    if id.is_empty() {
        return None;
    }
    Some(JumpTarget {
        service,
        view,
        id: id.to_string(),
    })
}

/// Fallback for CFN resource types with no explicit routing arm: map the
/// type's middle token ("AWS::DynamoDB::GlobalTable" → DynamoDB) to the
/// service that browses it and jump there with a searchable form of the
/// physical id — ARNs reduce to their last `:`/`/` segment. The landing is
/// query-style (filtered list, maybe no exact row), which still beats no
/// jump: sub-resources like subscriptions, stages, and record sets at least
/// land next to their parent. Unknown tokens (Custom::, unbrowsed services)
/// stay non-jumpable.
fn cfn_service_fallback_jump_target(
    resource_type: &str,
    physical_id: &str,
) -> Option<crate::app::JumpTarget> {
    use crate::app::{JumpTarget, JumpView};
    use crate::aws::service::ServiceType::*;
    let mut parts = resource_type.split("::");
    if parts.next() != Some("AWS") {
        return None;
    }
    let service = match parts.next()? {
        "EC2" => EC2,
        "S3" => S3,
        "Lambda" => Lambda,
        "DynamoDB" => DynamoDb,
        "IAM" => IAM,
        "RDS" => RDS,
        "ECS" => ECS,
        "ECR" => Ecr,
        "SNS" | "SQS" => Messaging,
        "CloudWatch" | "Logs" => CloudWatch,
        "KMS" => Kms,
        "EFS" => Efs,
        "EKS" => Eks,
        "Kinesis" => Kinesis,
        "StepFunctions" | "States" => StepFunctions,
        "ApiGateway" | "ApiGatewayV2" => ApiGateway,
        "Events" | "Scheduler" | "Pipes" => EventBridge,
        "CloudFront" => CloudFront,
        "CodeBuild" | "CodePipeline" | "CodeDeploy" | "CodeCommit" | "CodeArtifact" => Code,
        "SecretsManager" => Secrets,
        "SSM" => Ssm,
        "Route53" => Route53,
        "Route53Resolver" => Route53Resolver,
        "ElastiCache" => ElastiCache,
        "OpenSearchService" | "Elasticsearch" => OpenSearch,
        "Athena" => Athena,
        "Glue" => Glue,
        "Redshift" | "RedshiftServerless" => Redshift,
        "MSK" => Msk,
        "Transfer" => Transfer,
        "Cognito" => Cognito,
        "CloudFormation" => CloudFormation,
        "ElasticLoadBalancingV2" | "ElasticLoadBalancing" => Elb,
        "AutoScaling" => Asg,
        "CertificateManager" => Acm,
        "Backup" => Backup,
        "FSx" => Fsx,
        "SES" => Ses,
        "NetworkFirewall" => NetworkFirewall,
        "Bedrock" => Bedrock,
        "WorkSpaces" => Workspaces,
        _ => return None,
    };
    // Searchable token: an ARN's last ':'-segment, then its last '/'-segment
    // (covers name, path/name, and resource/name shapes).
    let id = if physical_id.starts_with("arn:") {
        let tail = physical_id.rsplit(':').next().unwrap_or(physical_id);
        tail.rsplit('/').next().unwrap_or(tail)
    } else {
        physical_id
    };
    if id.is_empty() {
        return None;
    }
    Some(JumpTarget {
        service,
        view: JumpView::None,
        id: id.to_string(),
    })
}

/// Columns a wrapped body keeps free at the right edge for a row's `  →`
/// jump arrow, so the arrow lands on the row's last screen line instead of
/// spilling onto a continuation of its own.
const WRAP_ARROW_RESERVE: usize = 3;

/// Hanging indent for a wrapped row's continuation lines: the value column
/// for a `key: value` pair, the text's own leading indent for anything else
/// (capped so a deeply indented line still keeps `WRAP_MIN_COLS` to wrap
/// into — the key column itself always leaves 20, so this only bites on
/// indented content lines in very narrow panes).
fn wrap_indent(key: &str, value: &str, key_w: usize, avail: usize) -> usize {
    let indent = if is_key_value_row(key, value) {
        key_w + 2
    } else if key.is_empty() {
        2 // empty-key notes render as `"  {value}"`
    } else {
        key.chars().take_while(|c| *c == ' ').count()
    };
    indent.min(avail.saturating_sub(WRAP_MIN_COLS))
}

/// Narrowest text column a continuation row is ever given.
const WRAP_MIN_COLS: usize = 12;

/// Split one styled line into screen rows of at most `avail` display
/// columns: the first row as is, every following row prefixed with `indent`
/// spaces. Splits by display width (`unicode-width`), never bytes or chars,
/// so a wide glyph is never cut in half; span styles and the line's own
/// style carry over to every row.
fn wrap_styled_line(line: Line<'static>, avail: usize, indent: usize) -> Vec<Line<'static>> {
    use unicode_width::UnicodeWidthChar;
    let avail = avail.max(1);
    if line.width() <= avail {
        return vec![line];
    }
    let line_style = line.style;
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut cur: Vec<Span<'static>> = Vec::new();
    let mut used = 0usize;
    let mut cap = avail;
    for span in line.spans {
        let mut buf = String::new();
        for ch in span.content.chars() {
            let cw = ch.width().unwrap_or(0);
            if used + cw > cap && used > 0 {
                if !buf.is_empty() {
                    cur.push(Span::styled(std::mem::take(&mut buf), span.style));
                }
                out.push(Line::from(std::mem::take(&mut cur)).style(line_style));
                // Continuation rows hang under the value column.
                cur.push(Span::raw(" ".repeat(indent)));
                used = 0;
                cap = avail.saturating_sub(indent).max(1);
                if ch == ' ' {
                    continue; // don't start a continuation with the break space
                }
            }
            buf.push(ch);
            used += cw;
        }
        if !buf.is_empty() {
            cur.push(Span::styled(buf, span.style));
        }
    }
    if !cur.is_empty() {
        out.push(Line::from(cur).style(line_style));
    }
    out
}

/// Lay out a detail body into exactly the screen lines that fit `area`,
/// keeping the logical `cursor` row on screen, and record the geometry the
/// mouse needs. `style_row(idx, key, value, key_w)` renders one logical row
/// (selection / search / jump-arrow styling — the per-renderer part); the
/// loading spinner is applied before it's called.
///
/// Clip mode (the default) is one screen line per row, as it always was.
/// With `App.detail_wrap` on, long rows continue onto hanging-indent lines
/// under the value column. Every consumer that indexes the body (`j`/`k`,
/// copy, visual selection, `/`, `[[`/`]]`) still works on logical rows; only
/// the screen shape changes, so scrolling is computed here in **screen**
/// rows, bottom-up from the cursor like the log tail's follow window, so a
/// wrapped cursor row is never cut off (`Paragraph::wrap` would anchor to
/// the top and clip). Widths are recomputed every frame — `Z` and terminal
/// resizes change them.
fn layout_detail_body<F>(
    app: &App,
    rows: &[(String, String)],
    area: Rect,
    cursor: usize,
    style_row: F,
) -> Vec<Line<'static>>
where
    F: Fn(usize, &str, &str, usize) -> Line<'static>,
{
    let focused = app.details_focused;
    let visible_height = area.height as usize;
    let width = area.width as usize;
    let key_widths = key_col_widths(rows, area.width);
    let spun = |idx: usize| -> (String, String) {
        let (k, v) = &rows[idx];
        spin_loading_row(k, v, app.tick_count, focused).unwrap_or_else(|| (k.clone(), v.clone()))
    };

    if !app.detail_wrap {
        let scroll_offset = cursor.saturating_sub(visible_height.saturating_sub(1));
        let end = rows.len().min(scroll_offset + visible_height);
        let row_map: Vec<usize> = (scroll_offset..end).collect();
        let lines: Vec<Line<'static>> = row_map
            .iter()
            .map(|&idx| {
                let (k, v) = spun(idx);
                style_row(idx, &k, &v, key_widths[idx])
            })
            .collect();
        record_jump_arrows(app, area, &row_map, &lines);
        app.record_detail_body_geometry(area, row_map);
        return lines;
    }

    if rows.is_empty() {
        app.record_detail_body_geometry(area, Vec::new());
        return Vec::new();
    }
    let avail = width.saturating_sub(WRAP_ARROW_RESERVE).max(1);
    let wrapped = |idx: usize| -> Vec<Line<'static>> {
        let (k, v) = spun(idx);
        let mut line = style_row(idx, &k, &v, key_widths[idx]);
        let arrow = if line.spans.last().is_some_and(|s| s.content == "  →") {
            line.spans.pop()
        } else {
            None
        };
        let indent = wrap_indent(&k, &v, key_widths[idx], avail);
        let mut out = wrap_styled_line(line, avail, indent);
        if let (Some(a), Some(last)) = (arrow, out.last_mut()) {
            last.spans.push(a);
        }
        out
    };
    // Height of a row without the per-renderer styling (which only restyles
    // spans and adds the reserved arrow), so measuring never runs the jump
    // classifiers for rows that won't be drawn. Measured by actually
    // wrapping: a wide glyph pushed to the next row makes arithmetic drift.
    let height = |idx: usize| -> usize {
        let (k, v) = spun(idx);
        let line = style_detail_row(&k, &v, None, key_widths[idx]);
        let indent = wrap_indent(&k, &v, key_widths[idx], avail);
        if line.width() <= avail {
            1
        } else {
            wrap_styled_line(line, avail, indent).len()
        }
    };

    let cursor = cursor.min(rows.len().saturating_sub(1));
    let first = first_visible_row(cursor, visible_height, height);
    let mut lines: Vec<Line<'static>> = Vec::with_capacity(visible_height);
    let mut row_map: Vec<usize> = Vec::with_capacity(visible_height);
    for idx in first..rows.len() {
        if lines.len() >= visible_height {
            break;
        }
        for l in wrapped(idx) {
            if lines.len() >= visible_height {
                break;
            }
            lines.push(l);
            row_map.push(idx);
        }
    }
    record_jump_arrows(app, area, &row_map, &lines);
    app.record_detail_body_geometry(area, row_map);
    lines
}

/// First logical row to draw so the cursor row ends up fully visible:
/// walk back from the cursor adding row heights while they still fit. A
/// cursor row taller than the pane starts at its own top.
fn first_visible_row(cursor: usize, visible_height: usize, height: impl Fn(usize) -> usize) -> usize {
    let mut used = height(cursor);
    let mut first = cursor;
    while first > 0 {
        let h = height(first - 1);
        if used + h > visible_height {
            break;
        }
        used += h;
        first -= 1;
    }
    first
}

/// Indicator state for a jumpable row: `Some(true)` = cross-service (needs
/// `gd`), `Some(false)` = same-service (`Enter` or `gd`), `None` = not jumpable.
/// Make each drawn `→` jump arrow a click target that follows its row's link,
/// as a double-click (or `⏎` on the row) does. Only while the pane has focus:
/// the unfocused preview may show another section than focusing would, so a
/// row index recorded there could follow the wrong link.
fn record_jump_arrows(app: &App, area: Rect, row_map: &[usize], lines: &[Line]) {
    if !app.details_focused {
        return;
    }
    for ((i, line), &logical) in lines.iter().enumerate().zip(row_map) {
        if line.spans.last().is_none_or(|s| s.content != "  →") {
            continue;
        }
        let w = line.width() as u16;
        if w > area.width {
            continue; // clipped: the arrow isn't on screen
        }
        app.push_click_region(
            Rect { x: area.x + w - 2, y: area.y + i as u16, width: 2, height: 1 },
            crate::app::ClickAction::FollowJump(logical),
        );
    }
}

fn jump_indicator(app: &App, key: &str, value: &str) -> Option<bool> {
    let current = app.current_service?;
    let target = app
        .ic_jump_target(key, value)
        .or_else(|| app.ecs_service_task_row_jump_target(key, value))
        .or_else(|| app.ecs_service_tasks_jump_target(key, value))
        .or_else(|| app.ecs_service_lb_jump_target(key, value))
        .or_else(|| app.cfn_resource_jump_target(key, value))
        .or_else(|| app.cfn_export_jump_target(key, value))
        .or_else(|| app.code_row_jump_target(key, value))
        .or_else(|| app.ecr_row_jump_target(key, value))
        .or_else(|| app.lambda_row_jump_target(key, value))
        .or_else(|| app.cc_repo_pr_row_jump_target(key, value))
        .or_else(|| app.cw_composite_alarm_child_jump_target(key, value))
        .or_else(|| app.insp_resource_finding_jump_target(key, value))
        .or_else(|| app.apigw_row_jump_target(key, value))
        .or_else(|| app.cf_row_jump_target(key, value))
        .or_else(|| app.ssm_row_jump_target(key, value))
        .or_else(|| app.r53_row_jump_target(key, value))
        .or_else(|| app.rds_row_jump_target(key, value))
        .or_else(|| app.org_row_jump_target(key, value))
        .or_else(|| app.sh_row_jump_target(key, value))
        .or_else(|| app.eb_row_jump_target(key, value))
        .or_else(|| resource_jump_target(key, value, current))?;
    Some(target.service != current)
}

/// Unified bottom-right footer for a detail pane. Kept deliberately minimal so
/// it doesn't duplicate the global key menu in the status bar, which already
/// carries every *universal* action — `e editor`, `y copy`, `r refresh` — and
/// the conditional `m metrics` / `t tail` (gated by `supports_metrics_overlay`
/// / `supports_log_tail`). The footer therefore auto-includes only the section
/// count, detail-pane `/ search`, and `h back` (plus a "⏎ go to resource" hint
/// when the row references another resource), and `extras` should carry **only
/// genuinely pane-specific** affordances whose key isn't already global — e.g.
/// `o objects` (S3), `i items` (DynamoDB), `d download` (Lambda code), `f
/// search` (log group), `x reveal · Y copy value` (secrets). Do NOT put
/// `e`/`m`/`t` in `extras`; they live in the status bar. Unfocused, the footer
/// just prompts `⏎ focus`.
fn detail_footer(app: &App, focused: bool, sections: usize, extras: &str) -> String {
    if !focused {
        return " ⏎ focus ".to_string();
    }
    let jumpable = app
        .details_selected_index
        .and_then(|i| app.get_detail_lines_filtered().get(i).cloned())
        .and_then(|(k, v)| jump_indicator(app, &k, &v))
        .is_some();

    // In flat view the digits jump to section headers rather than switching
    // the visible section — say so, and advertise the toggle both ways.
    let section_str = if app.detail_flat_mode {
        format!("1–{} jump · \\ tabs", sections)
    } else {
        format!("1–{} section · \\ flat", sections)
    };
    let mut parts: Vec<&str> = Vec::new();
    if jumpable {
        parts.push("⏎ go to resource");
    }
    if sections > 1 {
        parts.push(&section_str);
    }
    parts.push("/ search");
    if !extras.is_empty() {
        parts.push(extras);
    }
    if app.supports_trail_lens() {
        parts.push("W trail");
    }
    parts.push("h back");
    format!(" {} ", parts.join(" · "))
}

/// One aligned key/value row for the header: `  key_padded  value`.
fn header_kv(label: &str, value: &str) -> Line<'static> {
    // Pad to the fixed column width, but always keep at least one space before
    // the value so a label >= HEADER_LABEL_W chars doesn't bleed into it.
    let padded = if label.len() >= HEADER_LABEL_W {
        format!("{} ", label)
    } else {
        format!("{:<HEADER_LABEL_W$}", label)
    };
    Line::from(vec![
        Span::raw("  "),
        Span::styled(padded, Style::default().fg(theme::accent())),
        Span::styled(value.to_string(), Style::default().fg(crate::ui::theme::text_primary())),
    ])
}

/// Full-width `─` rule in TEXT_DIM colour.
fn render_hr(area: Rect, frame: &mut Frame) {
    let rule = "─".repeat(area.width as usize);
    frame.render_widget(
        Paragraph::new(Span::styled(rule, Style::default().fg(theme::text_dim()))),
        area,
    );
}

/// Generic one-row section tab bar from `(key, label, active)` tuples — active
/// tab is orange-on-black, inactive dimmed. Used by panes that don't need a
/// bespoke (adaptive) tab renderer.
fn render_section_tab_bar(app: &App, area: Rect, frame: &mut Frame, tabs: &[(char, &str, bool)]) {
    // Clickable regions: leading "  " (2), separator "   │   " (7).
    app.record_detail_section_regions(
        area,
        2,
        7,
        tabs.iter().map(|(k, l, _)| (*k, l.chars().count())),
    );
    let mut spans: Vec<Span> = vec![Span::raw("  ")];
    for (i, (key, label, active)) in tabs.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("   │   ", Style::default().fg(theme::text_dim())));
        }
        if *active {
            spans.push(Span::styled(
                key.to_string(),
                Style::default()
                    .fg(theme::aws_orange())
                    .add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::styled(
                format!(" {} ", label),
                Style::default()
                    .fg(Color::Black)
                    .bg(theme::aws_orange())
                    .add_modifier(Modifier::BOLD),
            ));
        } else {
            spans.push(Span::styled(
                key.to_string(),
                Style::default().fg(theme::text_dim()),
            ));
            spans.push(Span::styled(
                format!(" {} ", label),
                Style::default().fg(theme::text_dim()),
            ));
        }
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

// ── Shared helpers ────────────────────────────────────────────────────────────

/// Scrollable body for split panes that don't have jump-navigation rows.
fn render_split_section_body(app: &App, area: Rect, frame: &mut Frame) {
    let focused = app.details_focused;
    let has_search = focused && (!app.detail_search_query.is_empty() || app.detail_search_active);

    // Split body area: content rows above, optional 1-row search bar at bottom
    let (content_area, search_area) = if has_search {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(1)])
            .split(area);
        (chunks[0], Some(chunks[1]))
    } else {
        (area, None)
    };

    let all_rows = app.get_detail_lines_filtered();
    let cursor = app.details_selected_index.unwrap_or(0);

    let q_lower = app.detail_search_query.to_lowercase();

    let lines = layout_detail_body(app, &all_rows, content_area, cursor, |idx, key, value, key_w| {
        let line = style_detail_row(key, value, None, key_w);
        let jump = jump_indicator(app, key, value);
        if focused && app.detail_line_in_selection(idx) {
            let sel = theme::selection_style(true);
            let mut spans: Vec<Span> = line
                .spans
                .into_iter()
                .map(|s| Span::styled(s.content, sel))
                .collect();
            // `→` jump hint only on the cursor line, not the whole range.
            if jump.is_some() && Some(idx) == app.details_selected_index {
                spans.push(Span::styled("  →", Style::default().fg(theme::aws_orange())));
            }
            Line::from(spans).style(sel)
        } else if !q_lower.is_empty() {
            let combined = format!("{}{}", key, value);
            if combined.to_lowercase().contains(&q_lower) {
                let spans: Vec<Span> = line
                    .spans
                    .into_iter()
                    .map(|s| Span::styled(s.content, s.style.fg(crate::ui::theme::text_primary())))
                    .collect();
                Line::from(spans)
            } else {
                line
            }
        } else if jump.is_some() && focused {
            let mut spans = line.spans;
            spans.push(Span::styled("  →", Style::default().fg(theme::aws_orange())));
            Line::from(spans)
        } else {
            line
        }
    });

    frame.render_widget(Paragraph::new(lines), content_area);

    // Render inline search bar
    if let Some(sb_area) = search_area {
        let match_count = all_rows.len();
        let match_info = if app.detail_search_query.is_empty() {
            String::new()
        } else {
            format!(" ({} match{})", match_count, if match_count == 1 { "" } else { "es" })
        };

        let search_line = Line::from(vec![
            Span::styled(
                " / ",
                Style::default().fg(theme::aws_orange()).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                app.detail_search_query.clone(),
                Style::default().fg(crate::ui::theme::text_primary()),
            ),
            if app.detail_search_active {
                Span::styled("█", Style::default().fg(theme::aws_orange()))
            } else {
                Span::raw("")
            },
            Span::styled(match_info, Style::default().fg(theme::text_dim())),
            Span::styled("  ⏎ confirm · Esc clear", Style::default().fg(theme::text_dim())),
        ]);
        frame.render_widget(Paragraph::new(search_line), sb_area);
    }
}

/// Flatten a value for a single diff row: newlines to spaces, clipped with an
/// ellipsis (property values can be whole JSON policies — the full text lives
/// behind `e`).
fn clip_inline(s: &str, max: usize) -> String {
    let flat: String = s
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if flat.chars().count() <= max {
        flat
    } else {
        let clipped: String = flat.chars().take(max).collect();
        format!("{}…", clipped)
    }
}

/// Push a multi-line blob as plain content lines, capped. Long single lines are
/// hard-wrapped on a char boundary so a minified JSON cause doesn't render as
/// one unreadable row.
fn push_wrapped_content(rows: &mut Vec<(String, String)>, text: &str, max_lines: usize) {
    const WIDTH: usize = 110;
    let mut emitted = 0usize;
    for line in text.lines() {
        let mut rest = line;
        while !rest.is_empty() {
            if emitted >= max_lines {
                rows.push(("".to_string(), "      · truncated".to_string()));
                return;
            }
            let take = rest
                .char_indices()
                .take_while(|(i, _)| *i < WIDTH)
                .last()
                .map(|(i, c)| i + c.len_utf8())
                .unwrap_or(rest.len());
            let (head, tail) = rest.split_at(take);
            rows.push((format!("      {}", head), String::new()));
            emitted += 1;
            rest = tail;
        }
        if line.is_empty() {
            rows.push(("".to_string(), "".to_string()));
        }
    }
}

/// Shared: sorted key/value tag rows, or "No tags".
fn tag_rows(tags: &std::collections::HashMap<String, String>) -> Vec<(String, String)> {
    if tags.is_empty() {
        return vec![("".to_string(), "No tags".to_string())];
    }
    let mut sorted: Vec<(&String, &String)> = tags.iter().collect();
    sorted.sort_by_key(|(k, _)| k.as_str());
    sorted
        .into_iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

/// Word-wrap plain text to `width` columns (best-effort, splits on whitespace).
fn wrap_plain(text: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if line.is_empty() {
            line = word.to_string();
        } else if line.len() + 1 + word.len() <= width {
            line.push(' ');
            line.push_str(word);
        } else {
            out.push(std::mem::take(&mut line));
            line = word.to_string();
        }
    }
    if !line.is_empty() {
        out.push(line);
    }
    out
}

fn fmt_bytes(b: i64) -> String {
    let b = b as f64;
    if b >= 1_073_741_824.0 {
        format!("{:.2} GB", b / 1_073_741_824.0)
    } else if b >= 1_048_576.0 {
        format!("{:.2} MB", b / 1_048_576.0)
    } else if b >= 1024.0 {
        format!("{:.1} KB", b / 1024.0)
    } else {
        format!("{} B", b as i64)
    }
}

/// The uniform in-section note for a failed lazy fetch (a missing permission,
/// a partial emulator, or an unreachable endpoint) — keeps the error local to
/// the section instead of a scary global "sdk error". `style_detail_row`
/// renders the `⚠`-prefixed content line in the warning color. Used by every
/// lazy-section `*crate::lazy::Lazy::Error` render site.
fn error_rows(err: &str) -> Vec<(String, String)> {
    vec![
        (String::new(), String::new()),
        (format!("  ⚠ {}", err), String::new()),
    ]
}

/// Shared body for the lazy **Optimizer** section (the Compute Optimizer
/// right-sizing lens) on the EC2 instance / EBS volume / Lambda / ASG /
/// ECS-service panes. Pure row-builder: all flattening happens in
/// `aws::services::computeoptimizer`.
pub fn optimizer_lines(
    state: Option<&Lazy<Option<crate::aws::services::computeoptimizer::OptimizerRec>>>,
    enrollment: Option<&crate::aws::services::computeoptimizer::CoEnrollment>,
) -> Vec<(String, String)> {
    use crate::aws::services::computeoptimizer::{display_finding, CoEnrollment};
    // An unenrolled account can never return recommendations — the hint
    // outranks whatever the (doomed) per-resource call reported.
    if let Some(CoEnrollment::Inactive(status)) = enrollment {
        return vec![
            (String::new(), String::new()),
            (
                format!("  ⚠ Compute Optimizer is not enrolled ({})", status),
                String::new(),
            ),
            (
                "  Enroll in the AWS console (free) — recommendations appear ~30h later."
                    .to_string(),
                String::new(),
            ),
        ];
    }
    // Enrollment-check failure (endpoint / permissions): the per-resource
    // call almost certainly failed the same way — show the root cause unless
    // we somehow got data anyway.
    if let Some(CoEnrollment::Error(e)) = enrollment {
        if !matches!(state, Some(Lazy::Loaded(_))) {
            return error_rows(e);
        }
    }
    match state {
        None | Some(Lazy::Loading) => {
            vec![("  Loading recommendation…".to_string(), String::new())]
        }
        Some(Lazy::Error(e)) => error_rows(e),
        Some(Lazy::Loaded(None)) => vec![
            (
                "  No Compute Optimizer recommendation for this resource."
                    .to_string(),
                String::new(),
            ),
            (String::new(), String::new()),
            (
                "  New resources need ~30h and a 14-day metrics window; ECS needs Fargate."
                    .to_string(),
                String::new(),
            ),
        ],
        Some(Lazy::Loaded(Some(rec))) => {
            let mut rows = vec![("Finding".to_string(), display_finding(&rec.finding))];
            if !rec.finding_reasons.is_empty() {
                rows.push(("Reasons".to_string(), rec.finding_reasons.join(", ")));
            }
            if let Some(risk) = &rec.performance_risk {
                rows.push(("Current perf risk".to_string(), risk.clone()));
            }
            rows.push((
                "Look-back".to_string(),
                format!("{:.0} days", rec.lookback_days),
            ));
            if !rec.current.is_empty() {
                rows.push((String::new(), String::new()));
                rows.push(("Current".to_string(), String::new()));
                rows.extend(rec.current.iter().cloned());
            }
            if !rec.utilization.is_empty() {
                rows.push((String::new(), String::new()));
                rows.push(("Observed utilization".to_string(), String::new()));
                rows.extend(rec.utilization.iter().cloned());
            }
            if !rec.options.is_empty() {
                rows.push((String::new(), String::new()));
                rows.push(("Recommended options".to_string(), String::new()));
                for o in &rec.options {
                    let mut extras: Vec<String> = Vec::new();
                    if let Some(r) = o.performance_risk {
                        extras.push(format!("risk {:.1}", r));
                    }
                    if let Some(m) = &o.migration_effort {
                        extras.push(format!("effort {}", m));
                    }
                    let value = match (&o.savings, extras.is_empty()) {
                        (Some(s), true) => s.clone(),
                        (Some(s), false) => format!("{} · {}", s, extras.join(" · ")),
                        (None, false) => extras.join(" · "),
                        (None, true) => "—".to_string(),
                    };
                    rows.push((format!("#{}  {}", o.rank, o.label), value));
                    for p in &o.projected {
                        rows.push((format!("      {}", p), String::new()));
                    }
                }
            }
            rows
        }
    }
}

fn kv(k: &str, v: impl Into<String>) -> (String, String) {
    (k.to_string(), v.into())
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

/// Shared skeleton for the U13 flat-straggler split panes: fixed header
/// (bold name + dim subtitle) | rule | section tab bar | rule | generic body.
#[allow(clippy::too_many_arguments)]
/// Tab-bar triples for a descriptor-backed pane — digit keys, labels, and
/// the active flag all derive from the section table.
fn descriptor_tabs(
    app: &App,
    desc: &'static crate::sections::SectionDescriptor,
) -> Vec<(char, &'static str, bool)> {
    let active = app.detail_section_index();
    desc.sections
        .iter()
        .enumerate()
        .map(|(i, s)| {
            (
                crate::sections::key_for(i).unwrap_or(' '),
                s.label,
                i == active,
            )
        })
        .collect()
}

fn render_simple_split(
    app: &App,
    area: Rect,
    frame: &mut Frame,
    pane_title: &str,
    name: &str,
    subtitle: &str,
    tabs: &[(char, &str, bool)],
) {
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, tabs.len(), "");
    let mut block = theme::pane_block(pane_title, focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let header = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                name.to_string(),
                Style::default()
                    .fg(crate::ui::theme::text_primary())
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(subtitle.to_string(), Style::default().fg(theme::text_dim())),
        ]),
        Line::raw(""),
    ];
    let header_h = header.len() as u16;
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(header_h),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(inner);
    frame.render_widget(Paragraph::new(header), chunks[0]);
    render_hr(chunks[1], frame);
    render_section_tab_bar(app, chunks[2], frame, tabs);
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

// ── Severity colours (Security Hub, Inspector, Control Tower) ────────────────

/// Upper-case severity labels → the GuardDuty colour convention. Shared by
/// Security Hub (ASFF), Inspector, and Control Tower / Control Catalog, which
/// all use the same CRITICAL/HIGH/MEDIUM/LOW vocabulary.
fn severity_color(label: &str) -> Color {
    match label {
        "CRITICAL" | "HIGH" => theme::error(),
        "MEDIUM" => theme::warning(),
        _ => theme::text_dim(),
    }
}

// ── Terraform state detail pane ──────────────────────────────────────────────

fn render_tf_state_pane(app: &App, area: Rect, frame: &mut Frame) {
    let tf = app.terraform_state.as_ref().unwrap();
    let focused = app.details_focused;
    let footer = " Esc close  j/k scroll  y copy  Enter jump ";
    let mut block = theme::pane_block("Terraform State", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let header = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                "Terraform State",
                Style::default().fg(crate::ui::theme::text_primary()).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(format!("v{}", tf.version), Style::default().fg(theme::accent())),
            Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
            Span::styled(format!("serial {}", tf.serial), Style::default().fg(theme::text_dim())),
            Span::styled("  ·  ", Style::default().fg(theme::text_dim())),
            Span::styled(
                {
                    let blocks = tf.resources.len();
                    let instances = tf.instance_count();
                    if instances != blocks {
                        format!("{} resources · {} instances", blocks, instances)
                    } else {
                        format!("{} resources", blocks)
                    }
                },
                Style::default().fg(crate::ui::theme::text_primary()),
            ),
            if !tf.terraform_version.is_empty() {
                Span::styled(
                    format!("  ·  Terraform {}", tf.terraform_version),
                    Style::default().fg(theme::text_dim()),
                )
            } else {
                Span::raw("")
            },
        ]),
        Line::raw(""),
    ];
    let header_h = header.len() as u16;
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(header_h),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(inner);
    frame.render_widget(Paragraph::new(header), chunks[0]);
    render_hr(chunks[1], frame);
    render_split_section_body(app, chunks[2], frame);
}

/// Tags section rows for a lazy bundle that fetches tags best-effort: a
/// failed tag call renders as a warning, never as "(no tags)" (#28).
fn bundle_tag_rows(tags: &[(String, String)], err: Option<&str>) -> Vec<(String, String)> {
    if let Some(err) = err {
        return error_rows(err);
    }
    if tags.is_empty() {
        return vec![("  (no tags)".to_string(), String::new())];
    }
    tags.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
}

fn fmt_secs(secs: i64) -> String {
    if secs >= 86400 && secs % 86400 == 0 {
        format!("{} days", secs / 86400)
    } else if secs >= 3600 && secs % 3600 == 0 {
        format!("{} hours", secs / 3600)
    } else if secs >= 60 && secs % 60 == 0 {
        format!("{} min", secs / 60)
    } else {
        format!("{}s", secs)
    }
}

/// Shared renderer for a Tags section sourced from a resource's tag map.
fn map_tags_lines(tags: &std::collections::HashMap<String, String>) -> Vec<(String, String)> {
    let mut rows = vec![("".to_string(), "".to_string())];
    if tags.is_empty() {
        rows.push(("  No tags".to_string(), "".to_string()));
    } else {
        let mut sorted: Vec<(&String, &String)> = tags.iter().collect();
        sorted.sort_by_key(|(k, _)| k.as_str());
        for (key, value) in sorted {
            rows.push((format!("  {}", key), value.clone()));
        }
    }
    rows.push(("".to_string(), "".to_string()));
    rows
}

#[cfg(test)]
mod ecr_jump_tests {
    use super::*;
    use crate::app::JumpView;
    use crate::aws::service::ServiceType;

    #[test]
    fn cfn_stack_name_tag_row_jumps_to_owning_stack() {
        // Both tag-row shapes: indented (`map_tags_lines`) and plain.
        for key in ["  aws:cloudformation:stack-name", "aws:cloudformation:stack-name"] {
            let t = resource_jump_target(key, "my-stack", ServiceType::EC2)
                .expect("stack-name tag row should jump");
            assert_eq!(t.service, ServiceType::CloudFormation);
            assert!(matches!(t.view, JumpView::Cfn(crate::app::CfnView::Stacks)));
            assert_eq!(t.id, "my-stack");
        }
        // Empty value: no target.
        assert!(resource_jump_target("aws:cloudformation:stack-name", " ", ServiceType::EC2).is_none());
    }

    #[test]
    fn ecr_arn_jumps_to_repository_by_name() {
        let t = arn_jump_target("arn:aws:ecr:us-east-1:123456789012:repository/team/app")
            .expect("ecr arn should jump");
        assert_eq!(t.service, ServiceType::Ecr);
        assert!(matches!(t.view, JumpView::None));
        // Namespaced repo name preserved (not truncated to the last segment).
        assert_eq!(t.id, "team/app");
    }

    #[test]
    fn ecr_image_uri_jumps_to_repository() {
        // Via the generic classifier, with a tag.
        let t = resource_jump_target(
            "Image",
            "123456789012.dkr.ecr.eu-west-1.amazonaws.com/team/app:1.4.2",
            ServiceType::Inspector,
        )
        .expect("ecr image uri should jump");
        assert_eq!(t.service, ServiceType::Ecr);
        assert_eq!(t.id, "team/app");

        // With a digest instead of a tag.
        let repo = ecr_repo_from_image_uri(
            "123456789012.dkr.ecr.eu-west-1.amazonaws.com/svc@sha256:abcdef",
        );
        assert_eq!(repo, Some("svc"));

        // Non-ECR strings don't match.
        assert_eq!(ecr_repo_from_image_uri("docker.io/library/nginx:latest"), None);
    }

    #[test]
    fn digest_pinned_ecr_uri_jumps_to_the_image() {
        let d = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let t = resource_jump_target(
            "Image",
            &format!("123456789012.dkr.ecr.eu-west-1.amazonaws.com/team/app@{d}"),
            ServiceType::ECS,
        )
        .expect("digest uri should jump");
        assert_eq!(t.service, ServiceType::Ecr);
        assert!(matches!(t.view, JumpView::Ecr(crate::app::EcrView::Images)));
        assert_eq!(t.id, format!("team/app@{d}"));
        // A tag can't be resolved offline — still the repository.
        let t = resource_jump_target(
            "Image",
            "123456789012.dkr.ecr.eu-west-1.amazonaws.com/team/app:v1",
            ServiceType::ECS,
        )
        .unwrap();
        assert_eq!(t.id, "team/app");
    }

    #[test]
    fn ecr_image_sections_render() {
        use crate::aws::services::ecr::{
            EcrFinding, EcrImage, EcrImageDetailSection as S, EcrImageUser, EcrScanFindings,
        };
        let img = EcrImage::from_sdk(
            &aws_sdk_ecr::types::ImageDetail::builder()
                .repository_name("web")
                .image_digest("sha256:abc")
                .build(),
            "web",
            "123456789012.dkr.ecr.us-east-1.amazonaws.com/web",
        );
        let ov = ecr_image_section_lines(&img, S::Overview, None, None, &[]);
        assert!(ov.iter().any(|(k, v)| k == "Tags" && v == "<untagged>"));
        assert!(ov.iter().any(|(k, v)| k == "Last Pulled" && v == "Never"));

        let loading = ecr_image_section_lines(&img, S::Findings, None, None, &[]);
        assert!(loading.iter().any(|(_, v)| v.starts_with("Loading")));
        let err = crate::lazy::Lazy::Error("denied".to_string());
        let rows = ecr_image_section_lines(&img, S::Findings, Some(&err), None, &[]);
        assert!(rows.iter().any(|(k, _)| k.contains("denied")));
        let loaded = crate::lazy::Lazy::Loaded(EcrScanFindings {
            scan_status: "COMPLETE".into(),
            scanner: "Enhanced".into(),
            findings: vec![EcrFinding {
                id: "CVE-1".into(),
                severity: "HIGH".into(),
                package: Some("openssl".into()),
                installed: Some("3.0".into()),
                fixed_in: Some("3.1".into()),
                ..Default::default()
            }],
            ..Default::default()
        });
        let rows = ecr_image_section_lines(&img, S::Findings, Some(&loaded), None, &[]);
        assert!(rows.iter().any(|(k, _)| k == "HIGH CVE-1"));
        assert!(rows.iter().any(|(k, v)| k == "  Package" && v == "openssl 3.0"));
        assert!(rows.iter().any(|(k, v)| k == "  Fixed In" && v == "3.1"));

        let users = [EcrImageUser {
            kind: "Task",
            name: "web · 0f1e".into(),
            id: "arn:aws:ecs:us-east-1:1:task/c/0f1e".into(),
            container: "web".into(),
            status: "RUNNING".into(),
        }];
        let rows = ecr_image_section_lines(&img, S::UsedBy, None, Some((&users[..], true)), &[]);
        assert!(rows.iter().any(|(k, v)| k == "  Task" && v.ends_with("/0f1e")));
        let cold = ecr_image_section_lines(&img, S::UsedBy, None, Some((&[][..], false)), &[]);
        assert!(cold.iter().any(|(k, _)| k.contains("ECS not loaded")));
    }

    #[test]
    fn ecr_repo_images_section_reads_sibling_rows() {
        use crate::aws::services::ecr::{EcrImage, EcrRepoDetailSection as S, EcrRepository};
        let repo = EcrRepository {
            name: "web".into(),
            arn: String::new(),
            registry_id: String::new(),
            uri: "123456789012.dkr.ecr.us-east-1.amazonaws.com/web".into(),
            created: None,
            image_tag_mutability: "MUTABLE".into(),
            scan_on_push: false,
            encryption_type: "AES256".into(),
            kms_key: None,
            tags: Default::default(),
        };
        let mut img = EcrImage::from_sdk(
            &aws_sdk_ecr::types::ImageDetail::builder()
                .repository_name("web")
                .image_digest("sha256:abc")
                .image_tags("v1")
                .build(),
            "web",
            &repo.uri,
        );
        img.repo_images_seen = 1;
        let rows = ecr_repo_section_lines(&repo, S::Images, &[&img], false, None, &[], &[]);
        assert_eq!(rows[0].0, "Images (1, newest first)");
        assert!(rows.iter().any(|(k, v)| k == "  Digest" && v == "sha256:abc"));
        // A cut list says so.
        img.repo_images_seen = 250;
        let rows = ecr_repo_section_lines(&repo, S::Images, &[&img], false, None, &[], &[]);
        assert_eq!(rows[0].0, "Images (newest 1 of 250)");
        // Empty: loading while the list streams, "none" once it's done.
        let rows = ecr_repo_section_lines(&repo, S::Images, &[], true, None, &[], &[]);
        assert!(rows.iter().any(|(_, v)| v.starts_with("Loading")));
        let rows = ecr_repo_section_lines(&repo, S::Images, &[], false, None, &[], &[]);
        assert!(rows.iter().any(|(k, _)| k.contains("No images")));
    }

    #[test]
    fn ecr_cfn_type_jumps_to_repository() {
        let t = cfn_type_jump_target("AWS::ECR::Repository", "my-repo")
            .expect("ecr cfn type should jump");
        assert_eq!(t.service, ServiceType::Ecr);
        assert_eq!(t.id, "my-repo");
    }
}

#[cfg(test)]
mod s3_origin_domain_tests {
    use super::*;

    #[test]
    fn s3_rest_endpoints_resolve_to_bucket() {
        assert_eq!(
            s3_origin_domain_bucket("my-bucket.s3.eu-west-1.amazonaws.com"),
            Some("my-bucket")
        );
        // Legacy global endpoint.
        assert_eq!(
            s3_origin_domain_bucket("my-bucket.s3.amazonaws.com"),
            Some("my-bucket")
        );
        // Legacy dash-region endpoint (older distributions commonly carry this).
        assert_eq!(
            s3_origin_domain_bucket("my-bucket.s3-us-west-2.amazonaws.com"),
            Some("my-bucket")
        );
        // FIPS endpoint.
        assert_eq!(
            s3_origin_domain_bucket("b.s3-fips.us-east-1.amazonaws.com"),
            Some("b")
        );
        // Dotted bucket names split at the `.s3.` label, not the first dot.
        assert_eq!(
            s3_origin_domain_bucket("logs.example.com.s3.us-east-1.amazonaws.com"),
            Some("logs.example.com")
        );
        // Dualstack endpoint.
        assert_eq!(
            s3_origin_domain_bucket("b.s3.dualstack.us-east-1.amazonaws.com"),
            Some("b")
        );
    }

    #[test]
    fn cf_origin_domain_row_jumps_to_s3() {
        use crate::aws::service::ServiceType;
        // The exact row shape the CloudFront Origins section emits.
        let t = resource_jump_target(
            "  Domain",
            "my-bucket.s3.eu-west-1.amazonaws.com",
            ServiceType::CloudFront,
        )
        .expect("s3 origin domain should jump");
        assert_eq!(t.service, ServiceType::S3);
        assert_eq!(t.id, "my-bucket");

        let t = resource_jump_target(
            "  Domain",
            "old-bucket.s3-us-west-2.amazonaws.com",
            ServiceType::CloudFront,
        )
        .expect("dash-region origin domain should jump");
        assert_eq!(t.id, "old-bucket");
    }

    #[test]
    fn s3_website_endpoints_resolve_to_bucket() {
        assert_eq!(
            s3_origin_domain_bucket("site.s3-website-us-east-1.amazonaws.com"),
            Some("site")
        );
        // Newer dot-separated website form.
        assert_eq!(
            s3_origin_domain_bucket("www.example.com.s3-website.eu-central-1.amazonaws.com"),
            Some("www.example.com")
        );
    }

    #[test]
    fn non_s3_hostnames_do_not_match() {
        assert_eq!(s3_origin_domain_bucket("api.example.com"), None);
        assert_eq!(
            s3_origin_domain_bucket("vpce-1234.execute-api.us-east-1.amazonaws.com"),
            None
        );
        assert_eq!(s3_origin_domain_bucket("foo.s3import.amazonaws.com"), None);
        assert_eq!(s3_origin_domain_bucket("s3.amazonaws.com"), None);
        // Non-bucket S3 control planes (leading label isn't a bucket).
        assert_eq!(
            s3_origin_domain_bucket("123456789012.s3-control.us-east-1.amazonaws.com"),
            None
        );
        assert_eq!(
            s3_origin_domain_bucket("my-ap-abc123.s3-accesspoint.us-east-1.amazonaws.com"),
            None
        );
        // Paths / spaces disqualify (not a bare hostname).
        assert_eq!(
            s3_origin_domain_bucket("my-bucket.s3.amazonaws.com/key"),
            None
        );
    }
}

#[cfg(test)]
mod key_col_tests {
    use super::*;

    fn rows(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    fn colon_col(line: &Line<'_>) -> Option<usize> {
        use unicode_width::UnicodeWidthStr;
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        text.find(": ").map(|i| text[..i].width())
    }

    #[test]
    fn short_keys_pad_to_the_floor() {
        let r = rows(&[("Name", "a"), ("State", "b")]);
        assert_eq!(key_col_widths(&r, 120), vec![KEY_COL_MIN, KEY_COL_MIN]);
    }

    #[test]
    fn a_long_key_widens_the_whole_section() {
        // A tag key past 24 chars used to push only its own colon out.
        let r = rows(&[("Name", "web"), ("aws:cloudformation:logical-id", "AppServer")]);
        let w = key_col_widths(&r, 120);
        assert_eq!(w, vec![29, 29]);
        let l1 = style_detail_row("Name", "web", None, w[0]);
        let l2 = style_detail_row("aws:cloudformation:logical-id", "AppServer", None, w[1]);
        assert_eq!(colon_col(&l1), colon_col(&l2));
    }

    #[test]
    fn cap_shrinks_with_pane_width_and_never_exceeds_max() {
        let key = "x".repeat(60);
        let r = rows(&[(&key, "v")]);
        assert_eq!(key_col_widths(&r, 200), vec![KEY_COL_MAX]);
        // 50 columns − reserve 20 = 30.
        assert_eq!(key_col_widths(&r, 50), vec![30]);
        // Too narrow: the floor still wins.
        assert_eq!(key_col_widths(&r, 10), vec![KEY_COL_MIN]);
    }

    #[test]
    fn oversize_key_is_ellipsised_to_the_column() {
        use unicode_width::UnicodeWidthStr;
        let padded = pad_key_to_width("kubernetes.io/cluster/eks-prod-cluster-name", 30);
        assert_eq!(padded.width(), 30);
        assert!(padded.ends_with('…'));
        assert!(padded.starts_with("kubernetes.io/cluster/eks-pro"));
    }

    #[test]
    fn padding_uses_display_width_not_char_count() {
        // `⚠` and a CJK key both occupy more or fewer cells than chars.
        let l1 = style_detail_row("⚠ Drift", "yes", None, KEY_COL_MIN);
        let l2 = style_detail_row("名前", "v", None, KEY_COL_MIN);
        let l3 = style_detail_row("Name", "v", None, KEY_COL_MIN);
        assert_eq!(colon_col(&l1), Some(KEY_COL_MIN));
        assert_eq!(colon_col(&l2), Some(KEY_COL_MIN));
        assert_eq!(colon_col(&l3), Some(KEY_COL_MIN));
    }

    #[test]
    fn flat_view_sections_align_independently() {
        let r = rows(&[
            ("━━ Overview ━━━━", ""),
            ("Name", "web"),
            ("━━ Tags ━━━━", ""),
            ("aws:cloudformation:stack-name", "s"),
        ]);
        let w = key_col_widths(&r, 120);
        assert_eq!(w[1], KEY_COL_MIN);
        assert_eq!(w[3], 29);
    }

    #[test]
    fn non_pair_rows_do_not_widen_the_column() {
        // Group headers, content lines and empty-key notes carry no colon,
        // so a long "  No Compute Optimizer recommendation…" line must not
        // move the pairs around it.
        let r = rows(&[
            ("Finding", "Optimized"),
            ("A very long group header that has no value at all", ""),
            ("  a very long plain content line that also has no value", ""),
            ("", "a very long empty-key note that renders as content"),
        ]);
        assert_eq!(key_col_widths(&r, 120), vec![KEY_COL_MIN; 4]);
    }

    #[test]
    fn empty_key_note_renders_as_indented_content_without_a_colon() {
        let l = style_detail_row("", "No tags", None, KEY_COL_MIN);
        let text: String = l.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "  No tags");
        let w = style_detail_row("", "⚠ not enrolled", None, KEY_COL_MIN);
        assert_eq!(w.style.fg, Some(theme::warning()));
    }
}

#[cfg(test)]
mod detail_wrap_tests {
    use super::*;
    use unicode_width::UnicodeWidthStr;

    fn text(l: &Line) -> String {
        l.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn short_line_is_untouched() {
        let rows = wrap_styled_line(Line::raw("short"), 10, 4);
        assert_eq!(rows.len(), 1);
        assert_eq!(text(&rows[0]), "short");
    }

    #[test]
    fn continuation_rows_hang_at_the_indent_and_respect_the_width() {
        let line = Line::from(vec![
            Span::raw("key : "),
            Span::styled("abcdefghijklmnopqrstuvwxyz", Style::default().fg(Color::Red)),
        ]);
        let rows = wrap_styled_line(line, 12, 6);
        let joined: String = rows.iter().map(|r| text(r).trim_start().to_string()).collect();
        assert_eq!(joined, "key : abcdefghijklmnopqrstuvwxyz");
        for r in &rows[1..] {
            assert!(text(r).starts_with("      "), "{:?}", text(r));
        }
        assert!(rows.iter().all(|r| r.width() <= 12));
        // The value's style survives the split.
        assert!(rows[1].spans.iter().any(|s| s.style.fg == Some(Color::Red)));
    }

    #[test]
    fn wide_glyphs_are_never_split_or_overflow() {
        let line = Line::raw("日本語のテキストが長い値です");
        let rows = wrap_styled_line(line, 7, 0);
        assert!(rows.iter().all(|r| text(r).width() <= 7));
        let joined: String = rows.iter().map(text).collect();
        assert_eq!(joined, "日本語のテキストが長い値です");
    }

    #[test]
    fn first_visible_row_keeps_the_cursor_row_whole() {
        // Heights: row 3 is 4 screen rows tall, the rest 1.
        let h = |i: usize| if i == 3 { 4 } else { 1 };
        // 5 rows of screen: cursor row (4) + one row above it.
        assert_eq!(first_visible_row(3, 5, h), 2);
        // Cursor taller than the pane: start at its own top.
        assert_eq!(first_visible_row(3, 2, h), 3);
        assert_eq!(first_visible_row(0, 5, h), 0);
    }
}
