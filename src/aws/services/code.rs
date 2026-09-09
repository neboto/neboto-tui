use std::any::Any;
use std::collections::HashMap;

use async_trait::async_trait;
use futures::stream::StreamExt;
use tokio::sync::mpsc;

use crate::aws::client::AwsClients;
use crate::aws::resource::{native_state_label, Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::error::Result;
use crate::event::{Event, LoadProgress};

/// Pipelines whose recent executions are listed. Beyond this the Executions
/// sub-tab is incomplete and says so, rather than paying an unbounded N+1.
const MAX_EXECUTION_PIPELINES: usize = 100;
/// Recent executions kept per pipeline (`ListPipelineExecutions` page size).
const MAX_EXECUTIONS_PER_PIPELINE: i32 = 20;

type CcClient = aws_sdk_codecommit::Client;
type CbClient = aws_sdk_codebuild::Client;
type CpClient = aws_sdk_codepipeline::Client;
type CdClient = aws_sdk_codedeploy::Client;
type CaClient = aws_sdk_codeartifact::Client;

// ═══════════════════════════════════════════════════════════════════════════════
// Service
// ═══════════════════════════════════════════════════════════════════════════════

pub struct CodeService {
    cc_client: CcClient,
    cb_client: CbClient,
    cp_client: CpClient,
    cd_client: CdClient,
    ca_client: CaClient,
}

impl CodeService {
    pub fn new(clients: &AwsClients) -> Self {
        Self {
            cc_client: clients.codecommit_client(),
            cb_client: clients.codebuild_client(),
            cp_client: clients.codepipeline_client(),
            cd_client: clients.codedeploy_client(),
            ca_client: clients.codeartifact_client(),
        }
    }
}

#[async_trait]
impl AwsService for CodeService {
    fn service_type(&self) -> ServiceType {
        ServiceType::Code
    }

    fn name(&self) -> &str {
        "CodeSuite"
    }

    async fn list_resources(&self) -> Result<Vec<Box<dyn Resource>>> {
        Ok(vec![])
    }

    async fn list_resources_streaming(
        &self,
        event_tx: mpsc::UnboundedSender<Event>,
        service_type: ServiceType,
    ) -> Result<()> {
        let mut total = 0usize;
        let mut errors: Vec<String> = Vec::new();
        // Repo names feed the pull-requests phase at the end.
        let mut repo_names: Vec<String> = Vec::new();

        // ── CodeCommit Repositories ──────────────────────────────────────────
        let mut paginator = self.cc_client.list_repositories().into_paginator().send();
        while let Some(result) = paginator.next().await {
            match result {
                Ok(page) => {
                    let names: Vec<String> = page
                        .repositories()
                        .iter()
                        .filter_map(|r| r.repository_name().map(|s| s.to_string()))
                        .collect();
                    repo_names.extend(names.iter().cloned());

                    // batch_get_repositories supports max 25 per call
                    for chunk in names.chunks(25) {
                        if chunk.is_empty() {
                            continue;
                        }
                        match self
                            .cc_client
                            .batch_get_repositories()
                            .set_repository_names(Some(chunk.to_vec()))
                            .send()
                            .await
                        {
                            Ok(details) => {
                                let batch: Vec<Box<dyn Resource>> = details
                                    .repositories()
                                    .iter()
                                    .map(|r| {
                                        Box::new(CodeCommitRepo::from_sdk(r)) as Box<dyn Resource>
                                    })
                                    .collect();
                                total += batch.len();
                                let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                                    service: service_type,
                                    resources: batch,
                                    progress: LoadProgress {
                                        loaded_count: total,
                                        total_count: None,
                                        status_message: None,
                                    },
                                });
                            }
                            Err(e) => {
                                errors.push(format!("CodeCommit: {}", crate::error::sdk_error_message(&e)));
                                break;
                            }
                        }
                    }
                }
                Err(e) => {
                    errors.push(format!("CodeCommit: {}", crate::error::sdk_error_message(&e)));
                    break;
                }
            }
        }

        // ── CodeBuild Projects ───────────────────────────────────────────────
        let mut paginator = self.cb_client.list_projects().into_paginator().send();
        while let Some(result) = paginator.next().await {
            match result {
                Ok(page) => {
                    let names: Vec<String> = page.projects().to_vec();
                    if !names.is_empty() {
                        match self
                            .cb_client
                            .batch_get_projects()
                            .set_names(Some(names))
                            .send()
                            .await
                        {
                            Ok(details) => {
                                let batch: Vec<Box<dyn Resource>> = details
                                    .projects()
                                    .iter()
                                    .map(|p| {
                                        Box::new(CodeBuildProject::from_sdk(p))
                                            as Box<dyn Resource>
                                    })
                                    .collect();
                                total += batch.len();
                                let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                                    service: service_type,
                                    resources: batch,
                                    progress: LoadProgress {
                                        loaded_count: total,
                                        total_count: None,
                                        status_message: None,
                                    },
                                });
                            }
                            Err(e) => {
                                errors.push(format!("CodeBuild: {}", crate::error::sdk_error_message(&e)));
                                break;
                            }
                        }
                    }
                }
                Err(e) => {
                    errors.push(format!("CodeBuild: {}", crate::error::sdk_error_message(&e)));
                    break;
                }
            }
        }

        // ── CodePipeline Pipelines ───────────────────────────────────────────
        let mut pipeline_names: Vec<String> = Vec::new();
        let mut paginator = self.cp_client.list_pipelines().into_paginator().send();
        while let Some(result) = paginator.next().await {
            match result {
                Ok(page) => {
                    pipeline_names.extend(
                        page.pipelines()
                            .iter()
                            .filter_map(|p| p.name().map(|s| s.to_string())),
                    );
                    let batch: Vec<Box<dyn Resource>> = page
                        .pipelines()
                        .iter()
                        .map(|p| Box::new(CodePipeline::from_summary(p)) as Box<dyn Resource>)
                        .collect();
                    total += batch.len();
                    if !batch.is_empty() {
                        let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                            service: service_type,
                            resources: batch,
                            progress: LoadProgress {
                                loaded_count: total,
                                total_count: None,
                                status_message: None,
                            },
                        });
                    }
                }
                Err(e) => {
                    errors.push(format!("CodePipeline: {}", crate::error::sdk_error_message(&e)));
                    break;
                }
            }
        }

        // ── CodeDeploy Deployment Groups (applications → deployment groups) ───
        let mut app_paginator = self.cd_client.list_applications().into_paginator().send();
        while let Some(result) = app_paginator.next().await {
            let apps = match result {
                Ok(page) => page.applications().to_vec(),
                Err(e) => {
                    errors.push(format!("CodeDeploy: {}", crate::error::sdk_error_message(&e)));
                    break;
                }
            };
            for app in apps {
                // Deployment group names for this application.
                let mut group_names: Vec<String> = Vec::new();
                let mut grp_paginator = self
                    .cd_client
                    .list_deployment_groups()
                    .application_name(&app)
                    .into_paginator()
                    .send();
                let mut grp_err = false;
                while let Some(gr) = grp_paginator.next().await {
                    match gr {
                        Ok(page) => group_names.extend(page.deployment_groups().iter().cloned()),
                        Err(e) => {
                            errors.push(format!(
                                "CodeDeploy {}: {}",
                                app,
                                crate::error::sdk_error_message(&e)
                            ));
                            grp_err = true;
                            break;
                        }
                    }
                }
                if grp_err || group_names.is_empty() {
                    continue;
                }
                // batch_get_deployment_groups accepts up to 100 names per call.
                for chunk in group_names.chunks(100) {
                    match self
                        .cd_client
                        .batch_get_deployment_groups()
                        .application_name(&app)
                        .set_deployment_group_names(Some(chunk.to_vec()))
                        .send()
                        .await
                    {
                        Ok(resp) => {
                            let batch: Vec<Box<dyn Resource>> = resp
                                .deployment_groups_info()
                                .iter()
                                .map(|g| Box::new(CodeDeployGroup::from_sdk(g)) as Box<dyn Resource>)
                                .collect();
                            total += batch.len();
                            if !batch.is_empty() {
                                let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                                    service: service_type,
                                    resources: batch,
                                    progress: LoadProgress {
                                        loaded_count: total,
                                        total_count: None,
                                        status_message: None,
                                    },
                                });
                            }
                        }
                        Err(e) => {
                            errors.push(format!(
                                "CodeDeploy {}: {}",
                                app,
                                crate::error::sdk_error_message(&e)
                            ));
                            break;
                        }
                    }
                }
            }
        }

        // ── CodeArtifact Repositories (across all domains) ───────────────────
        let mut repo_paginator = self.ca_client.list_repositories().into_paginator().send();
        while let Some(result) = repo_paginator.next().await {
            match result {
                Ok(page) => {
                    let batch: Vec<Box<dyn Resource>> = page
                        .repositories()
                        .iter()
                        .map(|r| Box::new(CodeArtifactRepo::from_sdk(r)) as Box<dyn Resource>)
                        .collect();
                    total += batch.len();
                    if !batch.is_empty() {
                        let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                            service: service_type,
                            resources: batch,
                            progress: LoadProgress {
                                loaded_count: total,
                                total_count: None,
                                status_message: None,
                            },
                        });
                    }
                }
                Err(e) => {
                    errors.push(format!(
                        "CodeArtifact: {}",
                        crate::error::sdk_error_message(&e)
                    ));
                    break;
                }
            }
        }

        // ── Pipeline executions (best effort — a permission gap on
        // ListPipelineExecutions must not cost us the five resource lists). ──
        let overflow = pipeline_names.len().saturating_sub(MAX_EXECUTION_PIPELINES);
        pipeline_names.truncate(MAX_EXECUTION_PIPELINES);
        if overflow > 0 {
            let _ = event_tx.send(Event::ResourceLoadWarning {
                service: service_type,
                warning: format!(
                    "executions listed for the first {} pipelines ({} not scanned)",
                    MAX_EXECUTION_PIPELINES, overflow
                ),
            });
        }

        let mut exec_failures = 0usize;
        let mut stream = futures::stream::iter(pipeline_names.into_iter().map(|name| {
            let client = self.cp_client.clone();
            async move { fetch_pipeline_executions(client, name).await }
        }))
        .buffer_unordered(8);

        while let Some(res) = stream.next().await {
            match res {
                Ok(execs) if !execs.is_empty() => {
                    total += execs.len();
                    let batch: Vec<Box<dyn Resource>> = execs
                        .into_iter()
                        .map(|e| Box::new(e) as Box<dyn Resource>)
                        .collect();
                    let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                        service: service_type,
                        resources: batch,
                        progress: LoadProgress {
                            loaded_count: total,
                            total_count: None,
                            status_message: None,
                        },
                    });
                }
                Ok(_) => {}
                Err(_) => exec_failures += 1,
            }
        }

        if exec_failures > 0 {
            errors.push(format!(
                "CodePipeline: failed to list executions for {} pipeline(s)",
                exec_failures
            ));
        }

        // ── Pull requests (best effort — a permission gap on
        // ListPullRequests must not cost us the resource lists). ──
        let pr_overflow = repo_names.len().saturating_sub(MAX_PR_REPOS);
        repo_names.truncate(MAX_PR_REPOS);
        if pr_overflow > 0 {
            let _ = event_tx.send(Event::ResourceLoadWarning {
                service: service_type,
                warning: format!(
                    "pull requests listed for the first {} repos ({} not scanned)",
                    MAX_PR_REPOS, pr_overflow
                ),
            });
        }
        let budget = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(MAX_PR_LOOKUPS));
        let mut pr_failures = 0usize;
        let mut first_pr_error: Option<String> = None;
        let mut budget_hit = false;
        // Concurrency deliberately below the other phases' 8: CodeCommit's
        // control-plane TPS is low and this phase runs while everything else
        // has already streamed — 8 wide it throttled into partial-load
        // warnings on a ~50-repo account.
        let mut stream = futures::stream::iter(repo_names.into_iter().map(|repo| {
            let client = self.cc_client.clone();
            let budget = budget.clone();
            async move { fetch_repo_pull_requests(client, repo, budget).await }
        }))
        .buffer_unordered(4);
        while let Some(res) = stream.next().await {
            match res {
                Ok((prs, hit)) => {
                    budget_hit |= hit;
                    if !prs.is_empty() {
                        total += prs.len();
                        let batch: Vec<Box<dyn Resource>> = prs
                            .into_iter()
                            .map(|p| Box::new(p) as Box<dyn Resource>)
                            .collect();
                        let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                            service: service_type,
                            resources: batch,
                            progress: LoadProgress {
                                loaded_count: total,
                                total_count: None,
                                status_message: None,
                            },
                        });
                    }
                }
                Err(e) => {
                    pr_failures += 1;
                    first_pr_error.get_or_insert(e);
                }
            }
        }
        if budget_hit {
            let _ = event_tx.send(Event::ResourceLoadWarning {
                service: service_type,
                warning: format!(
                    "pull-request detail lookups capped at {} — some repos' PRs are missing",
                    MAX_PR_LOOKUPS
                ),
            });
        }
        if pr_failures > 0 {
            // Name the underlying error — "failed for 24 repos" alone can't
            // distinguish throttling from a permission gap.
            errors.push(format!(
                "CodeCommit: failed to list pull requests for {} repo(s) — e.g. {}",
                pr_failures,
                first_pr_error.unwrap_or_default()
            ));
        }

        // A sub-service failure is partial, not fatal — four of the five lists
        // loading fine must not read as a hard error (and `ResourceLoadError`
        // would clear `loading` and drop anything still streaming). Only a
        // total wipe-out is an error.
        if !errors.is_empty() {
            if total == 0 {
                let _ = event_tx.send(Event::ResourceLoadError {
                    service: service_type,
                    error: errors.join("; "),
                });
            } else {
                for warning in errors {
                    let _ = event_tx.send(Event::ResourceLoadWarning {
                        service: service_type,
                        warning,
                    });
                }
            }
        }

        let _ = event_tx.send(Event::ResourcesFullyLoaded {
            service: service_type,
            total_count: total,
        });
        Ok(())
    }

    async fn get_resource_details(&self, _id: &str) -> Result<Box<dyn Resource>> {
        Err(crate::error::Error::NotImplemented)
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// CodeCommit Repository
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct CodeCommitRepo {
    pub name: String,
    pub arn: String,
    pub description: String,
    pub clone_url_http: String,
    pub clone_url_ssh: String,
    pub clone_url_grc: String,
    pub default_branch: String,
    pub last_modified: String,
    pub account_id: String,
    pub tags: HashMap<String, String>,
}

impl CodeCommitRepo {
    fn from_sdk(r: &aws_sdk_codecommit::types::RepositoryMetadata) -> Self {
        let name = r.repository_name().unwrap_or_default().to_string();
        // Derive GRC URL from the HTTPS URL region: codecommit::<region>://<repo>
        let region = r
            .clone_url_http()
            .and_then(|u| u.strip_prefix("https://git-codecommit."))
            .and_then(|u| u.split('.').next())
            .unwrap_or("us-east-1");
        let clone_url_grc = format!("codecommit::{}://{}", region, name);
        Self {
            name,
            arn: r.arn().unwrap_or_default().to_string(),
            description: r.repository_description().unwrap_or_default().to_string(),
            clone_url_http: r.clone_url_http().unwrap_or_default().to_string(),
            clone_url_ssh: r.clone_url_ssh().unwrap_or_default().to_string(),
            clone_url_grc,
            default_branch: r.default_branch().unwrap_or_default().to_string(),
            last_modified: r
                .last_modified_date()
                .map(|d| {
                    d.fmt(aws_smithy_types::date_time::Format::DateTime)
                        .unwrap_or_default()
                })
                .unwrap_or_default(),
            account_id: r.account_id().unwrap_or_default().to_string(),
            tags: HashMap::new(),
        }
    }
}

crate::sections! {
    pub enum CodeCommitRepoDetailSection,
    pub static CODE_COMMIT_SECTIONS = [
        Details "Details" => crate::app::App::trigger_cc_extras_load,
        Branches "Branches" => crate::app::App::trigger_cc_branches_load,
        Commits "Commits" => crate::app::App::trigger_cc_commits_load,
        // Open PRs are sibling rows (zero fetch); the hook lazily fetches
        // this one repo's recently-closed PRs (the eager phase is open-only).
        PullRequests "Pull Requests" => crate::app::App::trigger_cc_repo_closed_prs_load,
        Triggers "Triggers" => crate::app::App::trigger_cc_extras_load,
        Readme "README" => crate::app::App::trigger_code_commit_readme_load,
    ]
}

impl Resource for CodeCommitRepo {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&CODE_COMMIT_SECTIONS)
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws codecommit get-repository --repository-name {}",
            crate::aws::resource::shell_quote(&self.name)
        ))
    }

    fn id(&self) -> &str {
        &self.arn
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn resource_type(&self) -> &str {
        "CodeCommit Repository"
    }
    fn state(&self) -> ResourceState {
        ResourceState::stateless()
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        format!(
            "{} {} {} {}",
            self.name, self.arn, self.description, self.default_branch
        )
    }
    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Name".to_string(), self.name.clone()),
            ("ARN".to_string(), self.arn.clone()),
            ("Description".to_string(), self.description.clone()),
            ("Default Branch".to_string(), self.default_branch.clone()),
            ("Clone URL (HTTPS)".to_string(), self.clone_url_http.clone()),
            ("Clone URL (GRC)".to_string(), self.clone_url_grc.clone()),
            ("Clone URL (SSH)".to_string(), self.clone_url_ssh.clone()),
            ("Last Modified".to_string(), self.last_modified.clone()),
        ]
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/codesuite/codecommit/repositories/{}/browse",
            region, self.name
        ))
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// CodeBuild Project
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct CodeBuildEnvVar {
    pub name: String,
    pub value: String,
    /// PLAINTEXT / PARAMETER_STORE / SECRETS_MANAGER. For the latter two the
    /// `value` is a *reference* (param name / secret id), never the secret.
    pub var_type: String,
}

#[derive(Debug, Clone)]
pub struct CodeBuildProject {
    pub name: String,
    pub arn: String,
    pub description: String,
    pub source_type: String,
    pub source_location: String,
    pub source_version: String,
    /// Inline buildspec YAML/JSON, a `buildspec.yml` path, or empty (default).
    pub buildspec: String,
    pub environment_type: String,
    pub compute_type: String,
    pub image: String,
    pub privileged_mode: bool,
    pub env_vars: Vec<CodeBuildEnvVar>,
    pub artifacts_type: String,
    pub artifacts_location: String,
    pub artifacts_name: String,
    pub cache_type: String,
    pub cache_location: String,
    pub vpc_id: String,
    pub vpc_subnets: Vec<String>,
    pub vpc_security_groups: Vec<String>,
    pub log_cw_group: String,
    pub log_cw_stream: String,
    pub log_cw_status: String,
    pub log_s3_location: String,
    pub log_s3_status: String,
    pub badge_enabled: bool,
    pub queued_timeout_minutes: i32,
    pub concurrent_build_limit: Option<i32>,
    pub created: String,
    pub last_modified: String,
    pub service_role: String,
    pub timeout_minutes: i32,
    pub tags: HashMap<String, String>,
}

impl CodeBuildProject {
    fn from_sdk(p: &aws_sdk_codebuild::types::Project) -> Self {
        let source = p.source();
        let env = p.environment();
        let artifacts = p.artifacts();
        let cache = p.cache();
        let vpc = p.vpc_config();
        let logs = p.logs_config();
        let cw = logs.and_then(|l| l.cloud_watch_logs());
        let s3 = logs.and_then(|l| l.s3_logs());
        Self {
            name: p.name().unwrap_or_default().to_string(),
            arn: p.arn().unwrap_or_default().to_string(),
            description: p.description().unwrap_or_default().to_string(),
            source_type: source
                .map(|s| s.r#type().as_str().to_string())
                .unwrap_or_default(),
            source_location: source
                .and_then(|s| s.location())
                .unwrap_or_default()
                .to_string(),
            source_version: p.source_version().unwrap_or_default().to_string(),
            buildspec: source
                .and_then(|s| s.buildspec())
                .unwrap_or_default()
                .to_string(),
            environment_type: env
                .map(|e| e.r#type().as_str().to_string())
                .unwrap_or_default(),
            compute_type: env
                .map(|e| e.compute_type().as_str().to_string())
                .unwrap_or_default(),
            image: env.map(|e| e.image().to_string()).unwrap_or_default(),
            privileged_mode: env.and_then(|e| e.privileged_mode()).unwrap_or(false),
            env_vars: env
                .map(|e| {
                    e.environment_variables()
                        .iter()
                        .map(|v| CodeBuildEnvVar {
                            name: v.name().to_string(),
                            value: v.value().to_string(),
                            var_type: v
                                .r#type()
                                .map(|t| t.as_str().to_string())
                                .unwrap_or_else(|| "PLAINTEXT".to_string()),
                        })
                        .collect()
                })
                .unwrap_or_default(),
            artifacts_type: artifacts
                .map(|a| a.r#type().as_str().to_string())
                .unwrap_or_default(),
            artifacts_location: artifacts
                .and_then(|a| a.location())
                .unwrap_or_default()
                .to_string(),
            artifacts_name: artifacts
                .and_then(|a| a.name())
                .unwrap_or_default()
                .to_string(),
            cache_type: cache
                .map(|c| c.r#type().as_str().to_string())
                .unwrap_or_default(),
            cache_location: cache
                .and_then(|c| c.location())
                .unwrap_or_default()
                .to_string(),
            vpc_id: vpc.and_then(|v| v.vpc_id()).unwrap_or_default().to_string(),
            vpc_subnets: vpc
                .map(|v| v.subnets().to_vec())
                .unwrap_or_default(),
            vpc_security_groups: vpc
                .map(|v| v.security_group_ids().to_vec())
                .unwrap_or_default(),
            log_cw_group: cw
                .and_then(|c| c.group_name())
                .unwrap_or_default()
                .to_string(),
            log_cw_stream: cw
                .and_then(|c| c.stream_name())
                .unwrap_or_default()
                .to_string(),
            log_cw_status: cw
                .map(|c| c.status().as_str().to_string())
                .unwrap_or_default(),
            log_s3_location: s3
                .and_then(|c| c.location())
                .unwrap_or_default()
                .to_string(),
            log_s3_status: s3
                .map(|c| c.status().as_str().to_string())
                .unwrap_or_default(),
            badge_enabled: p.badge().map(|b| b.badge_enabled()).unwrap_or(false),
            queued_timeout_minutes: p.queued_timeout_in_minutes().unwrap_or(0),
            concurrent_build_limit: p.concurrent_build_limit(),
            created: p
                .created()
                .map(|d| {
                    d.fmt(aws_smithy_types::date_time::Format::DateTime)
                        .unwrap_or_default()
                })
                .unwrap_or_default(),
            last_modified: p
                .last_modified()
                .map(|d| {
                    d.fmt(aws_smithy_types::date_time::Format::DateTime)
                        .unwrap_or_default()
                })
                .unwrap_or_default(),
            service_role: p.service_role().unwrap_or_default().to_string(),
            timeout_minutes: p.timeout_in_minutes().unwrap_or(0),
            tags: p
                .tags()
                .iter()
                .filter_map(|t| Some((t.key()?.to_string(), t.value().unwrap_or_default().to_string())))
                .collect(),
        }
    }

    /// True when the buildspec is inline content (YAML/JSON) rather than a file
    /// path reference like `buildspec.yml` (or empty → the source-root default).
    pub fn buildspec_is_inline(&self) -> bool {
        let bs = self.buildspec.trim();
        !bs.is_empty()
            && (bs.contains('\n') || bs.starts_with("version") || bs.starts_with('{'))
    }
}

crate::sections! {
    pub enum CodeBuildDetailSection,
    pub static CODE_BUILD_SECTIONS = [
        Details "Details",
        Buildspec "Buildspec" => crate::app::App::trigger_code_build_buildspec_load,
        Builds "Builds" => crate::app::App::trigger_code_build_builds_load,
        Environment "Environment",
        Tags "Tags",
    ]
}

impl Resource for CodeBuildProject {
    fn security_group_ids(&self) -> Vec<String> {
        self.vpc_security_groups.clone()
    }

    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&CODE_BUILD_SECTIONS)
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws codebuild batch-get-projects --names {}",
            crate::aws::resource::shell_quote(&self.name)
        ))
    }

    fn id(&self) -> &str {
        &self.arn
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn resource_type(&self) -> &str {
        "CodeBuild Project"
    }
    fn state(&self) -> ResourceState {
        ResourceState::stateless()
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        format!(
            "{} {} {} {} {} {}",
            self.name, self.arn, self.description, self.source_type, self.source_location, self.image
        )
    }
    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Name".to_string(), self.name.clone()),
            ("ARN".to_string(), self.arn.clone()),
            ("Description".to_string(), self.description.clone()),
            ("Source Type".to_string(), self.source_type.clone()),
            ("Source Location".to_string(), self.source_location.clone()),
            ("Environment".to_string(), self.environment_type.clone()),
            ("Compute".to_string(), self.compute_type.clone()),
            ("Image".to_string(), self.image.clone()),
            ("Last Modified".to_string(), self.last_modified.clone()),
            ("Service Role".to_string(), self.service_role.clone()),
            ("Timeout (min)".to_string(), self.timeout_minutes.to_string()),
        ]
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/codesuite/codebuild/projects/{}/history",
            region, self.name
        ))
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// CodePipeline
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct CodePipeline {
    pub name: String,
    pub version: i32,
    pub pipeline_type: String,
    pub created: String,
    pub updated: String,
    pub tags: HashMap<String, String>,
}

impl CodePipeline {
    fn from_summary(p: &aws_sdk_codepipeline::types::PipelineSummary) -> Self {
        Self {
            name: p.name().unwrap_or_default().to_string(),
            version: p.version().unwrap_or(0),
            pipeline_type: p
                .pipeline_type()
                .map(|t| t.as_str().to_string())
                .unwrap_or_default(),
            created: p
                .created()
                .map(|d| {
                    d.fmt(aws_smithy_types::date_time::Format::DateTime)
                        .unwrap_or_default()
                })
                .unwrap_or_default(),
            updated: p
                .updated()
                .map(|d| {
                    d.fmt(aws_smithy_types::date_time::Format::DateTime)
                        .unwrap_or_default()
                })
                .unwrap_or_default(),
            tags: HashMap::new(),
        }
    }
}

crate::sections! {
    pub enum CodePipelineDetailSection,
    pub static CODE_PIPELINE_SECTIONS = [
        Stages "Stages" => crate::app::App::trigger_code_pipeline_details_load,
        // Zero fetch: the executions are already sibling rows on the
        // Executions sub-tab, so this section filters them.
        Executions "Executions",
        // Tags ride the details fetch — `ListPipelines` returns none.
        Tags "Tags" => crate::app::App::trigger_code_pipeline_details_load,
    ]
}

impl Resource for CodePipeline {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&CODE_PIPELINE_SECTIONS)
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws codepipeline get-pipeline --name {}",
            crate::aws::resource::shell_quote(self.id())
        ))
    }

    fn id(&self) -> &str {
        &self.name
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn resource_type(&self) -> &str {
        "CodePipeline"
    }
    fn state(&self) -> ResourceState {
        ResourceState::stateless()
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        format!("{} {} {}", self.name, self.pipeline_type, self.version)
    }
    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Name".to_string(), self.name.clone()),
            ("Type".to_string(), self.pipeline_type.clone()),
            ("Version".to_string(), self.version.to_string()),
            ("Created".to_string(), self.created.clone()),
            ("Updated".to_string(), self.updated.clone()),
        ]
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/codesuite/codepipeline/pipelines/{}/view",
            region, self.name
        ))
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// View enum for sub-tabs
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum CodeView {
    Repositories,
    BuildProjects,
    Pipelines,
    Deployments,
    Artifacts,
    Executions,
    PullRequests,
}

impl CodeView {
    pub fn resource_type_filter(&self) -> &'static str {
        match self {
            CodeView::Repositories => "CodeCommit Repository",
            CodeView::BuildProjects => "CodeBuild Project",
            CodeView::Pipelines => "CodePipeline",
            CodeView::Deployments => "CodeDeploy Group",
            CodeView::Artifacts => "CodeArtifact Repository",
            CodeView::Executions => "Pipeline Execution",
            CodeView::PullRequests => "Pull Request",
        }
    }

    pub fn next(&self) -> Self {
        match self {
            CodeView::Repositories => CodeView::BuildProjects,
            CodeView::BuildProjects => CodeView::Pipelines,
            CodeView::Pipelines => CodeView::Deployments,
            CodeView::Deployments => CodeView::Artifacts,
            CodeView::Artifacts => CodeView::Executions,
            CodeView::Executions => CodeView::PullRequests,
            CodeView::PullRequests => CodeView::Repositories,
        }
    }

    pub fn prev(&self) -> Self {
        match self {
            CodeView::Repositories => CodeView::PullRequests,
            CodeView::BuildProjects => CodeView::Repositories,
            CodeView::Pipelines => CodeView::BuildProjects,
            CodeView::Deployments => CodeView::Pipelines,
            CodeView::Artifacts => CodeView::Deployments,
            CodeView::Executions => CodeView::Artifacts,
            CodeView::PullRequests => CodeView::Executions,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Lazy-loaded pipeline details (Stages + State)
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct CodePipelineDetails {
    pub stages: Vec<PipelineStage>,
    pub role_arn: String,
    pub execution_mode: String,
    pub artifact_store_type: String,
    pub artifact_store_location: String,
    pub artifact_store_kms: String,
    /// `ListTagsForResource` — `ListPipelines` returns no tags and no ARN, so
    /// the Tags section rides this fetch rather than the list row.
    pub tags: HashMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct PipelineStage {
    pub name: String,
    pub actions: Vec<PipelineAction>,
    /// Current status from get_pipeline_state (if available).
    pub status: String,
}

#[derive(Debug, Clone)]
pub struct PipelineAction {
    pub name: String,
    pub category: String,
    pub owner: String,
    pub provider: String,
    pub version: String,
    pub run_order: Option<i32>,
    /// Cross-region action (empty = the pipeline's own region).
    pub region: String,
    pub role_arn: String,
    pub namespace: String,
    pub input_artifacts: Vec<String>,
    pub output_artifacts: Vec<String>,
    /// Row label + value for the primary resource this action acts on
    /// (`("Project", "my-build")`). The label is what
    /// `App::code_row_jump_target` keys on, so it must stay stable.
    pub target: Option<(&'static str, String)>,
    /// Everything else in the action configuration, sorted, with
    /// credential-shaped keys dropped.
    pub configuration: Vec<(String, String)>,
}

/// The action's primary target — the row label to render it under, and the
/// value — resolved from the action configuration by provider. Returns `None`
/// for providers with nothing worth pointing at (manual approval).
///
/// The label doubles as the jump key, so `Project` / `Repository` /
/// `Application` are reserved for resources this app actually lists; a
/// third-party source repo gets `Source` instead so the classifier leaves it
/// alone.
pub fn action_target(
    provider: &str,
    cfg: &HashMap<String, String>,
) -> Option<(&'static str, String)> {
    let get = |k: &str| cfg.get(k).cloned().unwrap_or_default();
    let pick = |label: &'static str, v: String| (!v.is_empty()).then_some((label, v));
    match provider {
        "CodeBuild" => pick("Project", get("ProjectName")),
        "CodeCommit" => pick("Repository", get("RepositoryName")),
        "CloudFormation" => pick("Stack", get("StackName")),
        "CodeDeploy" => pick("Application", get("ApplicationName")),
        "ECS" | "ECSBlueGreen" => pick("Service", get("ServiceName")),
        "Lambda" => pick("Function", get("FunctionName")),
        "StepFunctions" => pick("State Machine", get("StateMachineArn")),
        "ECR" => pick("ECR Repository", get("RepositoryName")),
        "CodeStarSourceConnection" => pick("Source", get("FullRepositoryId")),
        "S3" => {
            let b = get("S3Bucket");
            pick("Bucket", if b.is_empty() { get("BucketName") } else { b })
        }
        _ => None,
    }
}

/// The rest of the configuration map, sorted and stripped of the target key
/// (already rendered) and of anything credential-shaped. `GetPipeline` masks
/// `OAuthToken`, but nothing masks a third-party action's equivalent.
fn other_config(cfg: &HashMap<String, String>, target: Option<&str>) -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = cfg
        .iter()
        .filter(|(k, v)| {
            let lower = k.to_lowercase();
            !v.is_empty()
                && !lower.contains("token")
                && !lower.contains("secret")
                && !lower.contains("password")
                && Some(k.as_str()) != target
        })
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    rows
}

/// The configuration key `action_target` consumed for this provider, so
/// `other_config` doesn't render it twice.
fn target_config_key(provider: &str, cfg: &HashMap<String, String>) -> Option<&'static str> {
    Some(match provider {
        "CodeBuild" => "ProjectName",
        "CodeCommit" | "ECR" => "RepositoryName",
        "CloudFormation" => "StackName",
        "CodeDeploy" => "ApplicationName",
        "ECS" | "ECSBlueGreen" => "ServiceName",
        "Lambda" => "FunctionName",
        "StepFunctions" => "StateMachineArn",
        "CodeStarSourceConnection" => "FullRepositoryId",
        "S3" => {
            if cfg.contains_key("S3Bucket") {
                "S3Bucket"
            } else {
                "BucketName"
            }
        }
        _ => return None,
    })
}

/// `arn` is `arn:aws:codepipeline:{region}:{account}:{name}`, built by the
/// caller since neither `ListPipelines` nor `GetPipeline` returns one; empty
/// skips the tag fetch.
pub async fn fetch_pipeline_details(
    client: CpClient,
    name: String,
    arn: String,
) -> std::result::Result<CodePipelineDetails, String> {
    // Get pipeline definition (stages + actions)
    let pipeline_resp = client
        .get_pipeline()
        .name(&name)
        .send()
        .await
        .map_err(|e| crate::error::sdk_error_message(&e))?;

    let pipeline = pipeline_resp.pipeline();
    let role_arn = pipeline
        .map(|p| p.role_arn().to_string())
        .unwrap_or_default();
    let execution_mode = pipeline
        .and_then(|p| p.execution_mode())
        .map(|m| m.as_str().to_string())
        .unwrap_or_default();

    let store = pipeline.and_then(|p| p.artifact_store());
    let artifact_store_type = store
        .map(|s| s.r#type().as_str().to_string())
        .unwrap_or_default();
    let artifact_store_location = store.map(|s| s.location().to_string()).unwrap_or_default();
    let artifact_store_kms = store
        .and_then(|s| s.encryption_key())
        .map(|k| k.id().to_string())
        .unwrap_or_default();

    let mut stages: Vec<PipelineStage> = pipeline
        .map(|p| {
            p.stages()
                .iter()
                .map(|s| PipelineStage {
                    name: s.name().to_string(),
                    actions: s
                        .actions()
                        .iter()
                        .map(|a| {
                            let type_id = a.action_type_id();
                            let provider = type_id
                                .map(|t| t.provider().to_string())
                                .unwrap_or_default();
                            let cfg = a.configuration().cloned().unwrap_or_default();
                            let target = action_target(&provider, &cfg);
                            let other =
                                other_config(&cfg, target_config_key(&provider, &cfg));
                            PipelineAction {
                                name: a.name().to_string(),
                                category: type_id
                                    .map(|t| t.category().as_str().to_string())
                                    .unwrap_or_default(),
                                owner: type_id
                                    .map(|t| t.owner().as_str().to_string())
                                    .unwrap_or_default(),
                                provider,
                                version: type_id
                                    .map(|t| t.version().to_string())
                                    .unwrap_or_default(),
                                run_order: a.run_order(),
                                region: a.region().unwrap_or_default().to_string(),
                                role_arn: a.role_arn().unwrap_or_default().to_string(),
                                namespace: a.namespace().unwrap_or_default().to_string(),
                                input_artifacts: a
                                    .input_artifacts()
                                    .iter()
                                    .map(|x| x.name().to_string())
                                    .collect(),
                                output_artifacts: a
                                    .output_artifacts()
                                    .iter()
                                    .map(|x| x.name().to_string())
                                    .collect(),
                                target,
                                configuration: other,
                            }
                        })
                        .collect(),
                    status: String::new(),
                })
                .collect()
        })
        .unwrap_or_default();

    // Get current pipeline state (status per stage)
    if let Ok(state_resp) = client.get_pipeline_state().name(&name).send().await {
        for ss in state_resp.stage_states() {
            if let Some(stage) = stages.iter_mut().find(|s| {
                ss.stage_name().map(|n| n == s.name).unwrap_or(false)
            }) {
                stage.status = ss
                    .latest_execution()
                    .map(|e| e.status().as_str().to_string())
                    .unwrap_or_default();
            }
        }
    }

    // Tags (best effort — a missing `ListTagsForResource` permission must not
    // cost us the stage graph, which is the point of this fetch).
    let mut tags = HashMap::new();
    if !arn.is_empty() {
        let mut token: Option<String> = None;
        loop {
            let mut req = client.list_tags_for_resource().resource_arn(&arn);
            if let Some(t) = &token {
                req = req.next_token(t);
            }
            let Ok(resp) = req.send().await else { break };
            for t in resp.tags() {
                tags.insert(t.key().to_string(), t.value().to_string());
            }
            match crate::aws::pagination::next_page_token(resp.next_token(), &token) {
                Some(t) => token = Some(t),
                None => break,
            }
        }
    }

    Ok(CodePipelineDetails {
        stages,
        role_arn,
        execution_mode,
        artifact_store_type,
        artifact_store_location,
        artifact_store_kms,
        tags,
    })
}

// ═══════════════════════════════════════════════════════════════════════════════
// CodePipeline executions (first-class rows on the Executions sub-tab)
// ═══════════════════════════════════════════════════════════════════════════════

/// One pipeline run. First-class so "which pipeline is running / what broke"
/// is a list operation, rather than text buried in the pipeline's detail pane.
#[derive(Debug, Clone)]
pub struct CodePipelineExecution {
    pub execution_id: String,
    pub pipeline_name: String,
    /// `<pipeline> / <execution id>` — the list row, since the Executions
    /// sub-tab spans every pipeline.
    pub display_name: String,
    pub status: String,
    pub status_summary: String,
    pub start_time: String,
    pub start_ms: i64,
    pub duration_secs: i64,
    pub trigger: String,
    pub trigger_detail: String,
    pub execution_mode: String,
    pub execution_type: String,
    /// Set when this run is a rollback — the execution it rolled back to.
    pub rollback_target: String,
    pub stop_reason: String,
    /// "action: revision — summary" per source revision.
    pub source_revisions: Vec<String>,
    pub tags: HashMap<String, String>,
}

impl CodePipelineExecution {
    pub fn from_summary(
        e: &aws_sdk_codepipeline::types::PipelineExecutionSummary,
        pipeline_name: &str,
    ) -> Self {
        let start = e.start_time().and_then(|t| t.to_millis().ok()).unwrap_or(0);
        let end = e
            .last_update_time()
            .and_then(|t| t.to_millis().ok())
            .unwrap_or(0);
        let execution_id = e.pipeline_execution_id().unwrap_or_default().to_string();
        Self {
            display_name: format!("{} / {}", pipeline_name, execution_id),
            pipeline_name: pipeline_name.to_string(),
            execution_id,
            status: e
                .status()
                .map(|s| s.as_str().to_string())
                .unwrap_or_default(),
            status_summary: e.status_summary().unwrap_or_default().to_string(),
            start_time: e
                .start_time()
                .map(|d| {
                    d.fmt(aws_smithy_types::date_time::Format::DateTime)
                        .unwrap_or_default()
                })
                .unwrap_or_default(),
            start_ms: start,
            duration_secs: if end > start { (end - start) / 1000 } else { 0 },
            trigger: e
                .trigger()
                .and_then(|t| t.trigger_type())
                .map(|t| t.as_str().to_string())
                .unwrap_or_default(),
            trigger_detail: e
                .trigger()
                .and_then(|t| t.trigger_detail())
                .unwrap_or_default()
                .to_string(),
            execution_mode: e
                .execution_mode()
                .map(|m| m.as_str().to_string())
                .unwrap_or_default(),
            execution_type: e
                .execution_type()
                .map(|t| t.as_str().to_string())
                .unwrap_or_default(),
            rollback_target: e
                .rollback_metadata()
                .and_then(|m| m.rollback_target_pipeline_execution_id())
                .unwrap_or_default()
                .to_string(),
            stop_reason: e
                .stop_trigger()
                .and_then(|t| t.reason())
                .unwrap_or_default()
                .to_string(),
            source_revisions: e
                .source_revisions()
                .iter()
                .map(|r| {
                    let mut s =
                        format!("{}: {}", r.action_name(), r.revision_id().unwrap_or("?"));
                    if let Some(summary) = r.revision_summary().filter(|s| !s.is_empty()) {
                        // Commit messages can be multi-line — keep the first line.
                        let first = summary.lines().next().unwrap_or(summary);
                        s.push_str(" — ");
                        s.push_str(first);
                    }
                    s
                })
                .collect(),
            tags: HashMap::new(),
        }
    }

    pub fn is_running(&self) -> bool {
        matches!(self.status.as_str(), "InProgress" | "Stopping")
    }

    pub fn failed(&self) -> bool {
        matches!(self.status.as_str(), "Failed" | "Cancelled")
    }

    pub fn duration_label(&self) -> String {
        if self.duration_secs <= 0 {
            return "—".to_string();
        }
        let d = self.duration_secs;
        let s = if d >= 3600 {
            format!("{}h {}m", d / 3600, (d % 3600) / 60)
        } else {
            format!("{}m {}s", d / 60, d % 60)
        };
        if self.is_running() {
            format!("{} (running)", s)
        } else {
            s
        }
    }
}

crate::sections! {
    pub enum CodeExecDetailSection,
    pub static CODE_EXEC_SECTIONS = [
        Overview "Overview" => crate::app::App::trigger_code_action_executions_load,
        Actions "Actions" => crate::app::App::trigger_code_action_executions_load,
        Artifacts "Artifacts" => crate::app::App::trigger_code_action_executions_load,
    ]
}

impl Resource for CodePipelineExecution {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&CODE_EXEC_SECTIONS)
    }

    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws codepipeline get-pipeline-execution --pipeline-name {} --pipeline-execution-id {}",
            crate::aws::resource::shell_quote(&self.pipeline_name),
            crate::aws::resource::shell_quote(&self.execution_id)
        ))
    }

    fn id(&self) -> &str {
        &self.execution_id
    }

    // Deliberately not a `name` field: the Executions sub-tab spans every
    // pipeline, so a bare execution UUID doesn't identify the row.
    #[allow(clippy::misnamed_getters)]
    fn name(&self) -> &str {
        &self.display_name
    }

    fn resource_type(&self) -> &str {
        "Pipeline Execution"
    }

    fn state(&self) -> ResourceState {
        pipeline_execution_state(&self.status)
    }

    fn state_label(&self) -> String {
        native_state_label(&self.status, || self.state())
    }

    /// A succeeded run is the routine case, and a superseded one never got
    /// past the source stage — `a` hides both so what's left needs looking at.
    fn is_noise(&self) -> bool {
        matches!(self.status.as_str(), "Succeeded" | "Superseded")
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!(
            "{} {} {} {} {} {}",
            self.pipeline_name,
            self.execution_id,
            self.status,
            self.trigger,
            self.trigger_detail,
            self.source_revisions.join(" ")
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Pipeline".to_string(), self.pipeline_name.clone()),
            ("Execution".to_string(), self.execution_id.clone()),
            ("Status".to_string(), self.status.clone()),
        ]
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/codesuite/codepipeline/pipelines/{}/executions/{}/timeline",
            region, self.pipeline_name, self.execution_id
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Map a pipeline-execution status to a `ResourceState` so the row dot and the
/// `F` state filter read the same way they do everywhere else.
pub fn pipeline_execution_state(status: &str) -> ResourceState {
    match status {
        "InProgress" => ResourceState::Running,
        "Succeeded" => ResourceState::Available,
        "Failed" | "Cancelled" => ResourceState::Unavailable,
        "Stopped" | "Stopping" => ResourceState::Stopped,
        other => ResourceState::Unknown(other.to_lowercase()),
    }
}

/// Newest first. The Executions sub-tab re-sorts the whole list the same way
/// (batches land in fetch-completion order), so keeping the two in step means
/// a pipeline's own Executions section reads identically.
fn sort_executions(execs: &mut [CodePipelineExecution]) {
    execs.sort_by_key(|e| std::cmp::Reverse(e.start_ms));
}

pub async fn fetch_pipeline_executions(
    client: CpClient,
    name: String,
) -> std::result::Result<Vec<CodePipelineExecution>, String> {
    let resp = client
        .list_pipeline_executions()
        .pipeline_name(&name)
        .max_results(MAX_EXECUTIONS_PER_PIPELINE)
        .send()
        .await
        .map_err(|e| crate::error::sdk_error_message(&e))?;

    let mut execs: Vec<CodePipelineExecution> = resp
        .pipeline_execution_summaries()
        .iter()
        .map(|e| CodePipelineExecution::from_summary(e, &name))
        .collect();
    sort_executions(&mut execs);
    Ok(execs)
}

// ── Action executions (what a run actually did, stage by stage) ────────────────

/// One action within a pipeline run. This is where the drill-down lives: for a
/// CodeBuild action, `external_execution_id` is the build id and
/// `log_stream_arn` is the build's CloudWatch stream.
#[derive(Debug, Clone)]
pub struct PipelineActionExecution {
    pub stage_name: String,
    pub action_name: String,
    pub status: String,
    pub start_time: String,
    pub start_ms: i64,
    pub duration_secs: i64,
    pub updated_by: String,
    pub category: String,
    pub owner: String,
    pub provider: String,
    pub region: String,
    pub role_arn: String,
    pub namespace: String,
    /// Resolved (variable-substituted) configuration where the API gave us
    /// one, else the declared configuration.
    pub target: Option<(&'static str, String)>,
    pub configuration: Vec<(String, String)>,
    pub input_artifacts: Vec<String>,
    pub output_artifacts: Vec<String>,
    pub output_variables: Vec<(String, String)>,
    /// The provider-side execution: a CodeBuild build id, a CFN change set, …
    pub external_execution_id: String,
    pub external_execution_summary: String,
    pub external_execution_url: String,
    /// Full CloudWatch Logs *stream* ARN, when the provider emitted one.
    pub log_stream_arn: String,
    pub error_code: String,
    pub error_message: String,
}

impl PipelineActionExecution {
    fn from_sdk(d: &aws_sdk_codepipeline::types::ActionExecutionDetail) -> Self {
        let input = d.input();
        // `resolved_configuration` has pipeline variables substituted, which is
        // what actually ran — prefer it over the declared configuration.
        let cfg: HashMap<String, String> = input
            .and_then(|i| {
                i.resolved_configuration()
                    .filter(|c| !c.is_empty())
                    .or_else(|| i.configuration())
            })
            .cloned()
            .unwrap_or_default();
        let type_id = input.and_then(|i| i.action_type_id());
        let provider = type_id
            .map(|t| t.provider().to_string())
            .unwrap_or_default();
        let target = action_target(&provider, &cfg);
        let output = d.output();
        let result = output.and_then(|o| o.execution_result());
        let start = d.start_time().and_then(|t| t.to_millis().ok()).unwrap_or(0);
        let end = d
            .last_update_time()
            .and_then(|t| t.to_millis().ok())
            .unwrap_or(0);
        let mut output_variables: Vec<(String, String)> = output
            .and_then(|o| o.output_variables())
            .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default();
        output_variables.sort_by(|a, b| a.0.cmp(&b.0));

        Self {
            stage_name: d.stage_name().unwrap_or_default().to_string(),
            action_name: d.action_name().unwrap_or_default().to_string(),
            status: d
                .status()
                .map(|s| s.as_str().to_string())
                .unwrap_or_default(),
            start_time: d
                .start_time()
                .map(|t| {
                    t.fmt(aws_smithy_types::date_time::Format::DateTime)
                        .unwrap_or_default()
                })
                .unwrap_or_default(),
            start_ms: start,
            duration_secs: if end > start { (end - start) / 1000 } else { 0 },
            updated_by: d.updated_by().unwrap_or_default().to_string(),
            category: type_id
                .map(|t| t.category().as_str().to_string())
                .unwrap_or_default(),
            owner: type_id.map(|t| t.owner().as_str().to_string()).unwrap_or_default(),
            configuration: other_config(&cfg, target_config_key(&provider, &cfg)),
            provider,
            region: input.and_then(|i| i.region()).unwrap_or_default().to_string(),
            role_arn: input
                .and_then(|i| i.role_arn())
                .unwrap_or_default()
                .to_string(),
            namespace: input
                .and_then(|i| i.namespace())
                .unwrap_or_default()
                .to_string(),
            target,
            input_artifacts: input
                .map(|i| {
                    i.input_artifacts()
                        .iter()
                        .filter_map(|a| a.name().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default(),
            output_artifacts: output
                .map(|o| {
                    o.output_artifacts()
                        .iter()
                        .filter_map(|a| a.name().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default(),
            output_variables,
            external_execution_id: result
                .and_then(|r| r.external_execution_id())
                .unwrap_or_default()
                .to_string(),
            external_execution_summary: result
                .and_then(|r| r.external_execution_summary())
                .unwrap_or_default()
                .to_string(),
            external_execution_url: result
                .and_then(|r| r.external_execution_url())
                .unwrap_or_default()
                .to_string(),
            log_stream_arn: result
                .and_then(|r| r.log_stream_arn())
                .unwrap_or_default()
                .to_string(),
            error_code: result
                .and_then(|r| r.error_details())
                .and_then(|e| e.code())
                .unwrap_or_default()
                .to_string(),
            error_message: result
                .and_then(|r| r.error_details())
                .and_then(|e| e.message())
                .unwrap_or_default()
                .to_string(),
        }
    }

    pub fn failed(&self) -> bool {
        matches!(self.status.as_str(), "Failed" | "Abandoned")
    }

    /// A CodeBuild action's `external_execution_id` is `<project>:<build-uuid>`
    /// — the project name is the jump target, since builds themselves live
    /// inside the project's own detail pane.
    pub fn build_project(&self) -> Option<&str> {
        (self.provider == "CodeBuild")
            .then(|| self.external_execution_id.split(':').next())
            .flatten()
            .filter(|s| !s.is_empty())
    }

    /// Split a CloudWatch Logs *stream* ARN into `(group, stream)` so `t` can
    /// tail it without a resolve call. The ARN form is
    /// `arn:aws:logs:<region>:<acct>:log-group:<group>:log-stream:<stream>`.
    pub fn log_group_and_stream(&self) -> Option<(String, String)> {
        let (_, rest) = self.log_stream_arn.split_once(":log-group:")?;
        let (group, stream) = rest.split_once(":log-stream:")?;
        (!group.is_empty()).then(|| (group.to_string(), stream.to_string()))
    }
}

pub async fn fetch_action_executions(
    client: CpClient,
    pipeline_name: String,
    execution_id: String,
) -> std::result::Result<Vec<PipelineActionExecution>, String> {
    let mut out: Vec<PipelineActionExecution> = Vec::new();
    let mut paginator = client
        .list_action_executions()
        .pipeline_name(&pipeline_name)
        .filter(
            aws_sdk_codepipeline::types::ActionExecutionFilter::builder()
                .pipeline_execution_id(&execution_id)
                .build(),
        )
        .into_paginator()
        .send();
    while let Some(page) = paginator.next().await {
        let page = page.map_err(|e| crate::error::sdk_error_message(&e))?;
        out.extend(
            page.action_execution_details()
                .iter()
                .map(PipelineActionExecution::from_sdk),
        );
    }
    // The API returns newest-first across the whole run; reading a run means
    // walking it forwards, so order by when each action started.
    out.sort_by_key(|a| a.start_ms);
    Ok(out)
}

// ═══════════════════════════════════════════════════════════════════════════════
// Lazy-loaded CodeCommit repo README
// ═══════════════════════════════════════════════════════════════════════════════

/// The repo's README content, or `None` when no README exists on the branch.
pub async fn fetch_repo_readme(
    client: CcClient,
    repo_name: String,
    branch: String,
) -> Option<String> {
    // Try common README filenames
    for filename in &["README.md", "README", "README.txt", "readme.md"] {
        let branch_ref = if branch.is_empty() { None } else { Some(branch.as_str()) };
        let mut req = client.get_file().repository_name(&repo_name).file_path(*filename);
        if let Some(b) = branch_ref {
            req = req.commit_specifier(b);
        }
        match req.send().await {
            Ok(resp) => {
                let content = resp.file_content();
                if let Ok(text) = String::from_utf8(content.as_ref().to_vec()) {
                    return Some(text);
                }
            }
            Err(_) => continue,
        }
    }
    None
}

// ═══════════════════════════════════════════════════════════════════════════════
// Lazy-loaded CodeCommit branches / commits / triggers+tags
// ═══════════════════════════════════════════════════════════════════════════════

/// Branches whose tips are resolved (`GetBranch` is one call per branch).
/// Beyond this the Branches section is incomplete and says so.
pub const MAX_CC_BRANCHES: usize = 100;
/// Commit-history walk depth per branch. There is no git-log API — each
/// first-parent step is one `GetCommit`, so the walk is sequential and capped.
pub const MAX_CC_COMMITS: usize = 50;

#[derive(Debug, Clone)]
pub struct CcBranch {
    pub name: String,
    pub tip_commit_id: String,
    /// First line of the tip commit's message.
    pub subject: String,
    pub author: String,
    pub date: String,
    /// Epoch seconds of the tip commit, for sorting.
    pub date_secs: i64,
    pub is_default: bool,
}

#[derive(Debug, Clone)]
pub struct CcBranches {
    pub branches: Vec<CcBranch>,
    /// Branch names listed beyond `MAX_CC_BRANCHES`, left unresolved.
    pub unresolved: usize,
}

#[derive(Debug, Clone)]
pub struct CcCommit {
    pub id: String,
    /// First line of the message.
    pub subject: String,
    /// The whole message, body included (`e` opens a git-log view).
    pub message: String,
    pub author: String,
    pub email: String,
    pub date: String,
    pub date_secs: i64,
    pub parents: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct CcCommitWalk {
    pub branch: String,
    pub commits: Vec<CcCommit>,
    /// The walk hit `MAX_CC_COMMITS` with history remaining.
    pub truncated: bool,
}

#[derive(Debug, Clone)]
pub struct CcTrigger {
    pub name: String,
    pub destination_arn: String,
    pub custom_data: String,
    pub branches: Vec<String>,
    pub events: Vec<String>,
}

/// Tags + triggers — one fetch feeding both the Details and Triggers sections
/// (the CodeSuite one-fetch-N-sections shape). Each call is independently
/// best-effort so a denied `ListTagsForResource` doesn't blank the triggers.
#[derive(Debug, Clone)]
pub struct CcRepoExtras {
    pub tags: Vec<(String, String)>,
    pub triggers: Vec<CcTrigger>,
    /// Per-call denials/failures, rendered inline in the owning section.
    pub tags_error: Option<String>,
    pub triggers_error: Option<String>,
}

/// CodeCommit `UserInfo.date` is git-style: `"<epoch-seconds> <±zone>"`.
fn cc_user_date(date: &str) -> (String, i64) {
    let secs = date
        .split_whitespace()
        .next()
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(0);
    if secs == 0 {
        return (String::new(), 0);
    }
    let formatted = aws_smithy_types::DateTime::from_secs(secs)
        .fmt(aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default();
    (formatted, secs)
}

fn cc_commit_from_sdk(c: &aws_sdk_codecommit::types::Commit) -> CcCommit {
    let message = c.message().unwrap_or_default().to_string();
    let subject = message.lines().next().unwrap_or_default().trim().to_string();
    let (author, email, date, date_secs) = c
        .author()
        .map(|a| {
            let (date, secs) = cc_user_date(a.date().unwrap_or_default());
            (
                a.name().unwrap_or_default().to_string(),
                a.email().unwrap_or_default().to_string(),
                date,
                secs,
            )
        })
        .unwrap_or_default();
    CcCommit {
        id: c.commit_id().unwrap_or_default().to_string(),
        subject,
        message,
        author,
        email,
        date,
        date_secs,
        parents: c.parents().iter().map(|s| s.to_string()).collect(),
    }
}

pub async fn fetch_repo_branches(
    client: CcClient,
    repo: String,
    default_branch: String,
) -> std::result::Result<CcBranches, String> {
    let mut names: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut req = client.list_branches().repository_name(&repo);
        if let Some(t) = &token {
            req = req.next_token(t);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| crate::error::sdk_error_message(&e))?;
        names.extend(resp.branches().iter().map(|s| s.to_string()));
        token = crate::aws::pagination::next_page_token(resp.next_token(), &token);
        if token.is_none() {
            break;
        }
    }
    let unresolved = names.len().saturating_sub(MAX_CC_BRANCHES);
    names.truncate(MAX_CC_BRANCHES);

    // Tip commit id per branch (GetBranch), then one BatchGetCommits pass for
    // the tip metadata (≤100 ids per call).
    let tips: Vec<(String, String)> = futures::stream::iter(names.into_iter().map(|name| {
        let client = client.clone();
        let repo = repo.clone();
        async move {
            let tip = client
                .get_branch()
                .repository_name(&repo)
                .branch_name(&name)
                .send()
                .await
                .ok()
                .and_then(|r| r.branch().and_then(|b| b.commit_id().map(|s| s.to_string())))
                .unwrap_or_default();
            (name, tip)
        }
    }))
    .buffer_unordered(8)
    .collect()
    .await;

    let mut ids: Vec<String> = tips
        .iter()
        .map(|(_, t)| t.clone())
        .filter(|t| !t.is_empty())
        .collect();
    ids.sort();
    ids.dedup();
    let mut meta: HashMap<String, CcCommit> = HashMap::new();
    for chunk in ids.chunks(100) {
        if let Ok(resp) = client
            .batch_get_commits()
            .repository_name(&repo)
            .set_commit_ids(Some(chunk.to_vec()))
            .send()
            .await
        {
            for c in resp.commits() {
                let cc = cc_commit_from_sdk(c);
                meta.insert(cc.id.clone(), cc);
            }
        }
    }

    let mut branches: Vec<CcBranch> = tips
        .into_iter()
        .map(|(name, tip)| {
            let m = meta.get(&tip);
            CcBranch {
                is_default: name == default_branch,
                subject: m.map(|c| c.subject.clone()).unwrap_or_default(),
                author: m.map(|c| c.author.clone()).unwrap_or_default(),
                date: m.map(|c| c.date.clone()).unwrap_or_default(),
                date_secs: m.map(|c| c.date_secs).unwrap_or(0),
                tip_commit_id: tip,
                name,
            }
        })
        .collect();
    branches.sort_by(|a, b| {
        b.is_default
            .cmp(&a.is_default)
            .then(b.date_secs.cmp(&a.date_secs))
            .then(a.name.cmp(&b.name))
    });
    Ok(CcBranches { branches, unresolved })
}

pub async fn fetch_branch_commits(
    client: CcClient,
    repo: String,
    branch: String,
) -> std::result::Result<CcCommitWalk, String> {
    let tip = client
        .get_branch()
        .repository_name(&repo)
        .branch_name(&branch)
        .send()
        .await
        .map_err(|e| crate::error::sdk_error_message(&e))?
        .branch()
        .and_then(|b| b.commit_id().map(|s| s.to_string()))
        .filter(|t| !t.is_empty())
        .ok_or_else(|| format!("branch {} has no tip commit", branch))?;

    let mut commits: Vec<CcCommit> = Vec::new();
    let mut next = Some(tip);
    let mut truncated = false;
    while let Some(id) = next.take() {
        if commits.len() >= MAX_CC_COMMITS {
            truncated = true;
            break;
        }
        let resp = client
            .get_commit()
            .repository_name(&repo)
            .commit_id(&id)
            .send()
            .await
            .map_err(|e| crate::error::sdk_error_message(&e))?;
        let Some(c) = resp.commit() else { break };
        let cc = cc_commit_from_sdk(c);
        next = cc.parents.first().cloned(); // first-parent walk
        commits.push(cc);
    }
    Ok(CcCommitWalk {
        branch,
        commits,
        truncated,
    })
}

pub async fn fetch_repo_extras(
    client: CcClient,
    repo: String,
    arn: String,
) -> std::result::Result<CcRepoExtras, String> {
    let mut tags: Vec<(String, String)> = Vec::new();
    let mut tags_error = None;
    match client.list_tags_for_resource().resource_arn(&arn).send().await {
        Ok(r) => {
            if let Some(map) = r.tags() {
                tags = map.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                tags.sort();
            }
        }
        Err(e) => tags_error = Some(crate::error::sdk_error_message(&e)),
    }

    let mut triggers: Vec<CcTrigger> = Vec::new();
    let mut triggers_error = None;
    match client
        .get_repository_triggers()
        .repository_name(&repo)
        .send()
        .await
    {
        Ok(r) => {
            triggers = r
                .triggers()
                .iter()
                .map(|t| CcTrigger {
                    name: t.name().to_string(),
                    destination_arn: t.destination_arn().to_string(),
                    custom_data: t.custom_data().unwrap_or_default().to_string(),
                    branches: t.branches().iter().map(|s| s.to_string()).collect(),
                    events: t.events().iter().map(|e| e.as_str().to_string()).collect(),
                })
                .collect();
        }
        Err(e) => triggers_error = Some(crate::error::sdk_error_message(&e)),
    }
    Ok(CcRepoExtras {
        tags,
        triggers,
        tags_error,
        triggers_error,
    })
}

// ═══════════════════════════════════════════════════════════════════════════════
// CodeCommit Pull Requests (first-class rows, the Executions precedent)
// ═══════════════════════════════════════════════════════════════════════════════

/// Repos whose pull requests are listed. Beyond this the Pull Requests
/// sub-tab is incomplete and says so.
const MAX_PR_REPOS: usize = 50;
/// Open PRs fetched per repo (`GetPullRequest` is one call per PR — there is
/// no batch variant).
const MAX_OPEN_PRS_PER_REPO: usize = 20;
/// Recently-closed PRs fetched per repo — lazily, from the repo pane's Pull
/// Requests section, never during the list load (the eager phase is
/// open-only: an eager closed pass doubled the list calls and tripled the
/// gets, and throttled to partial-load warnings on real accounts).
pub const MAX_CLOSED_PRS_PER_REPO: usize = 10;
/// Global `GetPullRequest` budget for one load — the hard ceiling on the
/// phase's N+1, shared across repos.
const MAX_PR_LOOKUPS: usize = 300;

/// Strip `refs/heads/` so refs read as branch names.
fn cc_short_ref(r: &str) -> String {
    r.strip_prefix("refs/heads/").unwrap_or(r).to_string()
}

/// Last path segment of an ARN — the human name of an IAM user/role ARN.
fn arn_short(arn: &str) -> String {
    arn.rsplit('/').next().unwrap_or(arn).to_string()
}

fn cc_fmt_dt(d: Option<&aws_smithy_types::DateTime>) -> (String, i64) {
    match d {
        Some(d) => (
            d.fmt(aws_smithy_types::date_time::Format::DateTime)
                .unwrap_or_default(),
            d.to_millis().unwrap_or(0),
        ),
        None => (String::new(), 0),
    }
}

#[derive(Debug, Clone)]
pub struct CodeCommitPullRequest {
    pub pr_id: String,
    pub title: String,
    pub description: String,
    pub repo: String,
    /// `<repo> #<id> <title>` — the list row, since the sub-tab spans repos.
    pub display_name: String,
    pub author: String,
    pub author_arn: String,
    /// OPEN / CLOSED.
    pub status: String,
    pub is_merged: bool,
    pub merged_by: String,
    pub merge_commit_id: String,
    pub merge_option: String,
    pub source_ref: String,
    pub dest_ref: String,
    pub source_commit: String,
    pub dest_commit: String,
    pub merge_base: String,
    pub created: String,
    pub created_ms: i64,
    pub last_activity: String,
    pub revision_id: String,
    pub approval_rule_names: Vec<String>,
    pub tags: HashMap<String, String>,
}

impl CodeCommitPullRequest {
    pub fn from_sdk(pr: &aws_sdk_codecommit::types::PullRequest) -> Self {
        let t = pr.pull_request_targets().first();
        let merge = t.and_then(|t| t.merge_metadata());
        let pr_id = pr.pull_request_id().unwrap_or_default().to_string();
        let title = pr.title().unwrap_or_default().to_string();
        let repo = t
            .and_then(|t| t.repository_name())
            .unwrap_or_default()
            .to_string();
        let author_arn = pr.author_arn().unwrap_or_default().to_string();
        let (created, created_ms) = cc_fmt_dt(pr.creation_date());
        let (last_activity, _) = cc_fmt_dt(pr.last_activity_date());
        Self {
            display_name: format!("{} #{} {}", repo, pr_id, title),
            pr_id,
            title,
            description: pr.description().unwrap_or_default().to_string(),
            repo,
            author: arn_short(&author_arn),
            author_arn,
            status: pr
                .pull_request_status()
                .map(|s| s.as_str().to_string())
                .unwrap_or_default(),
            is_merged: merge.map(|m| m.is_merged()).unwrap_or(false),
            merged_by: merge
                .and_then(|m| m.merged_by())
                .map(arn_short)
                .unwrap_or_default(),
            merge_commit_id: merge
                .and_then(|m| m.merge_commit_id())
                .unwrap_or_default()
                .to_string(),
            merge_option: merge
                .and_then(|m| m.merge_option())
                .map(|o| o.as_str().to_string())
                .unwrap_or_default(),
            source_ref: t
                .and_then(|t| t.source_reference())
                .map(cc_short_ref)
                .unwrap_or_default(),
            dest_ref: t
                .and_then(|t| t.destination_reference())
                .map(cc_short_ref)
                .unwrap_or_default(),
            source_commit: t
                .and_then(|t| t.source_commit())
                .unwrap_or_default()
                .to_string(),
            dest_commit: t
                .and_then(|t| t.destination_commit())
                .unwrap_or_default()
                .to_string(),
            merge_base: t
                .and_then(|t| t.merge_base())
                .unwrap_or_default()
                .to_string(),
            created,
            created_ms,
            last_activity,
            revision_id: pr.revision_id().unwrap_or_default().to_string(),
            approval_rule_names: pr
                .approval_rules()
                .iter()
                .filter_map(|r| r.approval_rule_name().map(|s| s.to_string()))
                .collect(),
            tags: HashMap::new(),
        }
    }

    /// OPEN / MERGED / CLOSED — the status chip the pane and rows show
    /// (CodeCommit's own status enum collapses merged into CLOSED).
    pub fn status_label(&self) -> &'static str {
        if self.status == "OPEN" {
            "OPEN"
        } else if self.is_merged {
            "MERGED"
        } else {
            "CLOSED"
        }
    }
}

crate::sections! {
    pub enum CcPrDetailSection,
    pub static CC_PR_SECTIONS = [
        Overview "Overview" => crate::app::App::trigger_cc_pr_approvals_load,
        Activity "Activity" => crate::app::App::trigger_cc_pr_events_load,
        Comments "Comments" => crate::app::App::trigger_cc_pr_comments_load,
        Changes "Changes" => crate::app::App::trigger_cc_pr_diff_load,
    ]
}

impl Resource for CodeCommitPullRequest {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&CC_PR_SECTIONS)
    }

    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws codecommit get-pull-request --pull-request-id {}",
            crate::aws::resource::shell_quote(&self.pr_id)
        ))
    }

    fn id(&self) -> &str {
        &self.pr_id
    }

    // Deliberately not a `name` field: the sub-tab spans every repo, so a
    // bare PR number doesn't identify the row (the Executions precedent).
    #[allow(clippy::misnamed_getters)]
    fn name(&self) -> &str {
        &self.display_name
    }

    fn resource_type(&self) -> &str {
        "Pull Request"
    }

    fn state(&self) -> ResourceState {
        if self.status == "OPEN" {
            ResourceState::Running
        } else if self.is_merged {
            ResourceState::Available
        } else {
            ResourceState::Stopped
        }
    }

    fn state_label(&self) -> String {
        if self.status == "OPEN" {
            "open".to_string()
        } else if self.is_merged {
            "merged".to_string()
        } else {
            "closed".to_string()
        }
    }

    /// A closed PR is history — `a` hides them so what's left needs review.
    fn is_noise(&self) -> bool {
        self.status == "CLOSED"
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!(
            "{} {} {} {} {} {} {} {} {}",
            self.repo,
            self.pr_id,
            self.title,
            self.author,
            self.author_arn,
            self.status_label(),
            self.source_ref,
            self.dest_ref,
            self.description.chars().take(200).collect::<String>()
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Repository".to_string(), self.repo.clone()),
            ("PR".to_string(), format!("#{}", self.pr_id)),
            ("Title".to_string(), self.title.clone()),
            ("Status".to_string(), self.status_label().to_string()),
        ]
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/codesuite/codecommit/repositories/{}/pull-requests/{}/details",
            region, self.repo, self.pr_id
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// One repo's **open** PRs, newest first. Open-only by design: most repos
/// return an empty id list (one cheap call, zero gets), which is what keeps
/// the phase viable at 50 repos. Closed PRs are a per-repo lazy fetch
/// (`fetch_repo_closed_prs`). The second tuple element reports the shared
/// `GetPullRequest` budget running out.
async fn fetch_repo_pull_requests(
    client: CcClient,
    repo: String,
    budget: std::sync::Arc<std::sync::atomic::AtomicUsize>,
) -> std::result::Result<(Vec<CodeCommitPullRequest>, bool), String> {
    use aws_sdk_codecommit::types::PullRequestStatusEnum;
    let resp = client
        .list_pull_requests()
        .repository_name(&repo)
        .pull_request_status(PullRequestStatusEnum::Open)
        .max_results(MAX_OPEN_PRS_PER_REPO as i32)
        .send()
        .await
        .map_err(|e| crate::error::sdk_error_message(&e))?;
    let ids: Vec<String> = resp
        .pull_request_ids()
        .iter()
        .take(MAX_OPEN_PRS_PER_REPO)
        .map(|s| s.to_string())
        .collect();

    let mut prs = Vec::new();
    let mut budget_hit = false;
    for id in ids {
        let allowed = budget
            .fetch_update(
                std::sync::atomic::Ordering::Relaxed,
                std::sync::atomic::Ordering::Relaxed,
                |v| v.checked_sub(1),
            )
            .is_ok();
        if !allowed {
            budget_hit = true;
            break;
        }
        if let Ok(resp) = client.get_pull_request().pull_request_id(&id).send().await {
            if let Some(pr) = resp.pull_request() {
                prs.push(CodeCommitPullRequest::from_sdk(pr));
            }
        }
    }
    prs.sort_by_key(|p| std::cmp::Reverse(p.created_ms));
    Ok((prs, budget_hit))
}

/// A repo's recently-closed PRs — the lazy half of PR listing, fired from
/// the repo pane's Pull Requests section for one repo at a time.
pub async fn fetch_repo_closed_prs(
    client: CcClient,
    repo: String,
) -> std::result::Result<Vec<CodeCommitPullRequest>, String> {
    use aws_sdk_codecommit::types::PullRequestStatusEnum;
    let resp = client
        .list_pull_requests()
        .repository_name(&repo)
        .pull_request_status(PullRequestStatusEnum::Closed)
        .max_results(MAX_CLOSED_PRS_PER_REPO as i32)
        .send()
        .await
        .map_err(|e| crate::error::sdk_error_message(&e))?;
    let ids: Vec<String> = resp
        .pull_request_ids()
        .iter()
        .take(MAX_CLOSED_PRS_PER_REPO)
        .map(|s| s.to_string())
        .collect();
    let mut prs = Vec::new();
    for id in ids {
        if let Ok(resp) = client.get_pull_request().pull_request_id(&id).send().await {
            if let Some(pr) = resp.pull_request() {
                prs.push(CodeCommitPullRequest::from_sdk(pr));
            }
        }
    }
    prs.sort_by_key(|p| std::cmp::Reverse(p.created_ms));
    Ok(prs)
}

// ── PR detail sections (lazy) ─────────────────────────────────────────────────

/// Approval-rule evaluation + who approved — the Overview section's
/// Approvals group. Each call is independently best-effort.
#[derive(Debug, Clone)]
pub struct CcPrApprovals {
    pub approved: bool,
    pub overridden: bool,
    pub satisfied: Vec<String>,
    pub not_satisfied: Vec<String>,
    /// `(user, state)` — users who approved / revoked on this revision.
    pub approvals: Vec<(String, String)>,
    pub eval_error: Option<String>,
    pub states_error: Option<String>,
}

pub async fn fetch_pr_approvals(
    client: CcClient,
    pr_id: String,
    revision_id: String,
) -> std::result::Result<CcPrApprovals, String> {
    let mut out = CcPrApprovals {
        approved: false,
        overridden: false,
        satisfied: vec![],
        not_satisfied: vec![],
        approvals: vec![],
        eval_error: None,
        states_error: None,
    };
    match client
        .evaluate_pull_request_approval_rules()
        .pull_request_id(&pr_id)
        .revision_id(&revision_id)
        .send()
        .await
    {
        Ok(r) => {
            if let Some(e) = r.evaluation() {
                out.approved = e.approved();
                out.overridden = e.overridden();
                out.satisfied = e
                    .approval_rules_satisfied()
                    .iter()
                    .map(|s| s.to_string())
                    .collect();
                out.not_satisfied = e
                    .approval_rules_not_satisfied()
                    .iter()
                    .map(|s| s.to_string())
                    .collect();
            }
        }
        Err(e) => out.eval_error = Some(crate::error::sdk_error_message(&e)),
    }
    match client
        .get_pull_request_approval_states()
        .pull_request_id(&pr_id)
        .revision_id(&revision_id)
        .send()
        .await
    {
        Ok(r) => {
            out.approvals = r
                .approvals()
                .iter()
                .map(|a| {
                    (
                        a.user_arn().map(arn_short).unwrap_or_default(),
                        a.approval_state()
                            .map(|s| s.as_str().to_string())
                            .unwrap_or_default(),
                    )
                })
                .collect();
        }
        Err(e) => out.states_error = Some(crate::error::sdk_error_message(&e)),
    }
    Ok(out)
}

/// Activity-timeline rows per `DescribePullRequestEvents` page, capped.
pub const MAX_PR_EVENTS: usize = 200;

#[derive(Debug, Clone)]
pub struct CcPrEvent {
    pub date: String,
    pub kind: String,
    pub actor: String,
    pub detail: String,
}

pub async fn fetch_pr_events(
    client: CcClient,
    pr_id: String,
) -> std::result::Result<Vec<CcPrEvent>, String> {
    let mut events = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut req = client.describe_pull_request_events().pull_request_id(&pr_id);
        if let Some(t) = &token {
            req = req.next_token(t);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| crate::error::sdk_error_message(&e))?;
        for e in resp.pull_request_events() {
            let (date, _) = cc_fmt_dt(e.event_date());
            let kind = e
                .pull_request_event_type()
                .map(|t| prettify_pr_event(t.as_str()))
                .unwrap_or_default();
            // Flatten every populated metadata field into the detail cell —
            // exactly one of these metadata structs is set per event type.
            let mut parts: Vec<String> = Vec::new();
            if let Some(m) = e.pull_request_status_changed_event_metadata() {
                if let Some(s) = m.pull_request_status() {
                    parts.push(s.as_str().to_string());
                }
            }
            if let Some(m) = e.pull_request_source_reference_updated_event_metadata() {
                if let Some(b) = m.before_commit_id() {
                    parts.push(format!(
                        "{} → {}",
                        b.chars().take(8).collect::<String>(),
                        m.after_commit_id()
                            .unwrap_or_default()
                            .chars()
                            .take(8)
                            .collect::<String>()
                    ));
                }
            }
            if let Some(m) = e.pull_request_merged_state_changed_event_metadata() {
                if let Some(mm) = m.merge_metadata() {
                    if let Some(o) = mm.merge_option() {
                        parts.push(o.as_str().to_string());
                    }
                    if let Some(c) = mm.merge_commit_id() {
                        parts.push(c.chars().take(8).collect::<String>());
                    }
                }
            }
            if let Some(m) = e.approval_state_changed_event_metadata() {
                if let Some(s) = m.approval_status() {
                    parts.push(s.as_str().to_string());
                }
            }
            if let Some(m) = e.approval_rule_event_metadata() {
                if let Some(n) = m.approval_rule_name() {
                    parts.push(n.to_string());
                }
            }
            if let Some(m) = e.approval_rule_overridden_event_metadata() {
                if let Some(s) = m.override_status() {
                    parts.push(s.as_str().to_string());
                }
            }
            events.push(CcPrEvent {
                date,
                kind,
                actor: e.actor_arn().map(arn_short).unwrap_or_default(),
                detail: parts.join(" · "),
            });
            if events.len() >= MAX_PR_EVENTS {
                return Ok(events);
            }
        }
        token = crate::aws::pagination::next_page_token(resp.next_token(), &token);
        if token.is_none() {
            break;
        }
    }
    Ok(events)
}

/// `PULL_REQUEST_STATUS_CHANGED` → `Status changed`.
fn prettify_pr_event(raw: &str) -> String {
    let s = raw
        .strip_prefix("PULL_REQUEST_")
        .unwrap_or(raw)
        .replace('_', " ")
        .to_lowercase();
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => s,
    }
}

pub const MAX_PR_COMMENT_THREADS: usize = 100;

#[derive(Debug, Clone)]
pub struct CcPrComment {
    pub author: String,
    pub date: String,
    pub content: String,
    pub deleted: bool,
}

#[derive(Debug, Clone)]
pub struct CcPrThread {
    /// Empty for a PR-level (non-inline) thread.
    pub path: String,
    pub line: String,
    pub comments: Vec<CcPrComment>,
}

#[derive(Debug, Clone)]
pub struct CcPrComments {
    pub threads: Vec<CcPrThread>,
    pub total_comments: usize,
    pub truncated: bool,
}

pub async fn fetch_pr_comments(
    client: CcClient,
    pr_id: String,
) -> std::result::Result<CcPrComments, String> {
    let mut threads = Vec::new();
    let mut total = 0usize;
    let mut truncated = false;
    let mut token: Option<String> = None;
    'pages: loop {
        let mut req = client.get_comments_for_pull_request().pull_request_id(&pr_id);
        if let Some(t) = &token {
            req = req.next_token(t);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| crate::error::sdk_error_message(&e))?;
        for t in resp.comments_for_pull_request_data() {
            if threads.len() >= MAX_PR_COMMENT_THREADS {
                truncated = true;
                break 'pages;
            }
            let (path, line) = t
                .location()
                .map(|l| {
                    (
                        l.file_path().unwrap_or_default().to_string(),
                        l.file_position()
                            .map(|p| p.to_string())
                            .unwrap_or_default(),
                    )
                })
                .unwrap_or_default();
            let comments: Vec<CcPrComment> = t
                .comments()
                .iter()
                .map(|c| {
                    let (date, _) = cc_fmt_dt(c.creation_date());
                    CcPrComment {
                        author: c.author_arn().map(arn_short).unwrap_or_default(),
                        date,
                        content: c.content().unwrap_or_default().to_string(),
                        deleted: c.deleted(),
                    }
                })
                .collect();
            total += comments.len();
            if !comments.is_empty() {
                threads.push(CcPrThread { path, line, comments });
            }
        }
        token = crate::aws::pagination::next_page_token(resp.next_token(), &token);
        if token.is_none() {
            break;
        }
    }
    Ok(CcPrComments {
        threads,
        total_comments: total,
        truncated,
    })
}

pub const MAX_PR_DIFF_FILES: usize = 500;
/// Files the `e` patch renders (each is up to two `GetBlob` calls).
pub const MAX_PATCH_FILES: usize = 50;
/// A blob bigger than this is reported, not diffed.
pub const MAX_PATCH_BLOB_BYTES: usize = 1_000_000;

#[derive(Debug, Clone)]
pub struct CcPrDiffEntry {
    /// A / M / D.
    pub change: String,
    pub path: String,
    /// Differs from `path` on renames.
    pub old_path: String,
    pub before_blob: String,
    pub after_blob: String,
}

#[derive(Debug, Clone)]
pub struct CcPrDiff {
    pub entries: Vec<CcPrDiffEntry>,
    pub truncated: bool,
    /// The compared commit specifiers (merge base → source tip).
    pub before: String,
    pub after: String,
    pub repo: String,
}

pub async fn fetch_pr_diff(
    client: CcClient,
    repo: String,
    before: String,
    after: String,
) -> std::result::Result<CcPrDiff, String> {
    let mut entries = Vec::new();
    let mut truncated = false;
    let mut token: Option<String> = None;
    'pages: loop {
        let mut req = client
            .get_differences()
            .repository_name(&repo)
            .before_commit_specifier(&before)
            .after_commit_specifier(&after);
        if let Some(t) = &token {
            req = req.next_token(t);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| crate::error::sdk_error_message(&e))?;
        for d in resp.differences() {
            if entries.len() >= MAX_PR_DIFF_FILES {
                truncated = true;
                break 'pages;
            }
            entries.push(CcPrDiffEntry {
                change: d
                    .change_type()
                    .map(|c| c.as_str().to_string())
                    .unwrap_or_default(),
                path: d
                    .after_blob()
                    .and_then(|b| b.path())
                    .or_else(|| d.before_blob().and_then(|b| b.path()))
                    .unwrap_or_default()
                    .to_string(),
                old_path: d
                    .before_blob()
                    .and_then(|b| b.path())
                    .unwrap_or_default()
                    .to_string(),
                before_blob: d
                    .before_blob()
                    .and_then(|b| b.blob_id())
                    .unwrap_or_default()
                    .to_string(),
                after_blob: d
                    .after_blob()
                    .and_then(|b| b.blob_id())
                    .unwrap_or_default()
                    .to_string(),
            });
        }
        token = crate::aws::pagination::next_page_token(resp.next_token(), &token);
        if token.is_none() {
            break;
        }
    }
    Ok(CcPrDiff {
        entries,
        truncated,
        before,
        after,
        repo,
    })
}

/// Fetch a blob's text, or `None` for binary/oversize content (with a note).
async fn cc_blob_text(client: &CcClient, repo: &str, blob_id: &str) -> Result2<Option<String>> {
    if blob_id.is_empty() {
        return Ok(None);
    }
    let resp = client
        .get_blob()
        .repository_name(repo)
        .blob_id(blob_id)
        .send()
        .await
        .map_err(|e| crate::error::sdk_error_message(&e))?;
    let bytes = resp.content().as_ref();
    if bytes.len() > MAX_PATCH_BLOB_BYTES {
        return Err(format!("file larger than {} bytes", MAX_PATCH_BLOB_BYTES));
    }
    Ok(String::from_utf8(bytes.to_vec()).ok())
}

type Result2<T> = std::result::Result<T, String>;

/// Build a unified-diff patch for the whole PR (`e` on the Changes section):
/// both blobs per file, diffed locally — CodeCommit has no patch API.
pub async fn fetch_pr_patch(client: CcClient, diff: CcPrDiff) -> String {
    let mut out = format!(
        "# {} — {} → {}\n# {} changed file(s)\n\n",
        diff.repo,
        diff.before.chars().take(8).collect::<String>(),
        diff.after.chars().take(8).collect::<String>(),
        diff.entries.len()
    );
    for entry in diff.entries.iter().take(MAX_PATCH_FILES) {
        let old_label = if entry.before_blob.is_empty() {
            "/dev/null".to_string()
        } else {
            format!("a/{}", entry.old_path)
        };
        let new_label = if entry.after_blob.is_empty() {
            "/dev/null".to_string()
        } else {
            format!("b/{}", entry.path)
        };
        out.push_str(&format!("diff --git a/{} b/{}\n", entry.old_path, entry.path));
        let before = cc_blob_text(&client, &diff.repo, &entry.before_blob).await;
        let after = cc_blob_text(&client, &diff.repo, &entry.after_blob).await;
        match (before, after) {
            (Ok(b), Ok(a)) => {
                if (b.is_none() && !entry.before_blob.is_empty())
                    || (a.is_none() && !entry.after_blob.is_empty())
                {
                    out.push_str("Binary files differ\n\n");
                    continue;
                }
                let b = b.unwrap_or_default();
                let a = a.unwrap_or_default();
                let td = similar::TextDiff::from_lines(&b, &a);
                out.push_str(
                    &td.unified_diff()
                        .context_radius(3)
                        .header(&old_label, &new_label)
                        .to_string(),
                );
                out.push('\n');
            }
            (Err(e), _) | (_, Err(e)) => {
                out.push_str(&format!("# skipped: {}\n\n", e));
            }
        }
    }
    if diff.entries.len() > MAX_PATCH_FILES {
        out.push_str(&format!(
            "# … {} more file(s) not rendered (patch capped at {} files)\n",
            diff.entries.len() - MAX_PATCH_FILES,
            MAX_PATCH_FILES
        ));
    }
    if diff.truncated {
        out.push_str(&format!(
            "# … file list itself truncated at {} entries\n",
            MAX_PR_DIFF_FILES
        ));
    }
    out
}

/// git-log-style text of a walked branch, behind `e` on the Commits section.
pub fn cc_walk_git_log(walk: &CcCommitWalk) -> String {
    let mut out = format!("# {} — most recent {} commits (first-parent)\n\n", walk.branch, walk.commits.len());
    for c in &walk.commits {
        out.push_str(&format!(
            "commit {}\nAuthor: {} <{}>\nDate:   {}\n\n",
            c.id, c.author, c.email, c.date
        ));
        for line in c.message.lines() {
            out.push_str(&format!("    {}\n", line));
        }
        out.push('\n');
    }
    if walk.truncated {
        out.push_str(&format!("… older history truncated (walk capped at {})\n", MAX_CC_COMMITS));
    }
    out
}

// ═══════════════════════════════════════════════════════════════════════════════
// Lazy-loaded CodeBuild build history
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct CodeBuildBuild {
    pub build_number: i64,
    pub status: String,
    pub start_time: String,
    /// Epoch-millis of the build start, for seeding the log-tail cursor.
    pub start_ms: i64,
    pub end_time: String,
    pub duration_secs: i64,
    pub initiator: String,
    pub source_version: String,
    pub resolved_source_version: String,
    pub phases: Vec<CodeBuildPhase>,
    pub log_group: String,
    pub log_stream: String,
}

#[derive(Debug, Clone)]
pub struct CodeBuildPhase {
    pub phase_type: String,
    pub status: String,
    pub duration_secs: i64,
}


pub async fn fetch_project_builds(
    client: CbClient,
    project_name: String,
) -> std::result::Result<Vec<CodeBuildBuild>, String> {
    // Get recent build IDs for this project (max 100)
    let list_resp = client
        .list_builds_for_project()
        .project_name(&project_name)
        .sort_order(aws_sdk_codebuild::types::SortOrderType::Descending)
        .send()
        .await
        .map_err(|e| crate::error::sdk_error_message(&e))?;

    let ids: Vec<String> = list_resp.ids().iter().take(20).map(|s| s.to_string()).collect();
    if ids.is_empty() {
        return Ok(vec![]);
    }

    let builds_resp = client
        .batch_get_builds()
        .set_ids(Some(ids))
        .send()
        .await
        .map_err(|e| crate::error::sdk_error_message(&e))?;

    Ok(map_builds(builds_resp.builds()))
}

fn map_builds(builds: &[aws_sdk_codebuild::types::Build]) -> Vec<CodeBuildBuild> {
    builds
        .iter()
        .map(|b| {
            let logs = b.logs();
            CodeBuildBuild {
                build_number: b.build_number().unwrap_or(0),
                status: b
                    .build_status()
                    .map(|s| s.as_str().to_string())
                    .unwrap_or_default(),
                start_time: b
                    .start_time()
                    .map(|d| {
                        d.fmt(aws_smithy_types::date_time::Format::DateTime)
                            .unwrap_or_default()
                    })
                    .unwrap_or_default(),
                start_ms: b.start_time().and_then(|t| t.to_millis().ok()).unwrap_or(0),
                end_time: b
                    .end_time()
                    .map(|d| {
                        d.fmt(aws_smithy_types::date_time::Format::DateTime)
                            .unwrap_or_default()
                    })
                    .unwrap_or_default(),
                duration_secs: {
                    let start = b.start_time().and_then(|t| t.to_millis().ok()).unwrap_or(0);
                    let end = b.end_time().and_then(|t| t.to_millis().ok()).unwrap_or(0);
                    if end > start { (end - start) / 1000 } else { 0 }
                },
                initiator: b.initiator().unwrap_or_default().to_string(),
                source_version: b.source_version().unwrap_or_default().to_string(),
                resolved_source_version: b
                    .resolved_source_version()
                    .unwrap_or_default()
                    .to_string(),
                phases: b
                    .phases()
                    .iter()
                    .map(|p| CodeBuildPhase {
                        phase_type: p
                            .phase_type()
                            .map(|t| t.as_str().to_string())
                            .unwrap_or_default(),
                        status: p
                            .phase_status()
                            .map(|s| s.as_str().to_string())
                            .unwrap_or_default(),
                        duration_secs: p.duration_in_seconds().unwrap_or(0),
                    })
                    .collect(),
                log_group: logs
                    .and_then(|l| l.group_name())
                    .unwrap_or_default()
                    .to_string(),
                log_stream: logs
                    .and_then(|l| l.stream_name())
                    .unwrap_or_default()
                    .to_string(),
            }
        })
        .collect()
}

/// Resolve the latest build's CloudWatch log group + stream (and its start time
/// in epoch-millis, to seed the tail cursor) so `t` can tail a project's logs.
pub async fn resolve_codebuild_latest_log(
    client: CbClient,
    project_name: String,
) -> std::result::Result<(String, String, i64), String> {
    let list_resp = client
        .list_builds_for_project()
        .project_name(&project_name)
        .sort_order(aws_sdk_codebuild::types::SortOrderType::Descending)
        .send()
        .await
        .map_err(|e| crate::error::sdk_error_message(&e))?;

    let latest = list_resp
        .ids()
        .first()
        .map(|s| s.to_string())
        .ok_or_else(|| "No builds yet for this project".to_string())?;

    let builds_resp = client
        .batch_get_builds()
        .ids(latest)
        .send()
        .await
        .map_err(|e| crate::error::sdk_error_message(&e))?;

    let build = builds_resp
        .builds()
        .first()
        .ok_or_else(|| "Could not load the latest build".to_string())?;

    let group = build
        .logs()
        .and_then(|l| l.group_name())
        .unwrap_or_default()
        .to_string();
    let stream = build
        .logs()
        .and_then(|l| l.stream_name())
        .unwrap_or_default()
        .to_string();
    if group.is_empty() {
        return Err("The latest build has no CloudWatch logs configured".to_string());
    }
    let start_ms = build
        .start_time()
        .and_then(|t| t.to_millis().ok())
        .unwrap_or(0);

    Ok((group, stream, start_ms))
}

// ═══════════════════════════════════════════════════════════════════════════════
// Lazy-loaded CodeBuild buildspec fetched from the source repo
// ═══════════════════════════════════════════════════════════════════════════════

/// Successful outcome of resolving a project's buildspec (the payload of the
/// lazy Buildspec section; fetch failures are the `Lazy::Error` arm).
#[derive(Debug, Clone)]
pub enum BuildspecResult {
    /// The buildspec file content.
    Content(String),
    /// The buildspec lives in a non-CodeCommit source we can't read here
    /// (GitHub/Bitbucket/S3/…) — carries a human note.
    Unsupported(String),
    /// The buildspec file doesn't exist in the repo.
    NotFound,
}

/// Extract the CodeCommit repository name from a CodeBuild `source.location`
/// (the HTTPS clone URL, `…/v1/repos/<name>`), or the bare name as a fallback.
pub fn codecommit_repo_from_location(location: &str) -> Option<String> {
    let loc = location.trim_end_matches('/');
    if loc.is_empty() {
        return None;
    }
    Some(loc.rsplit('/').next().unwrap_or(loc).to_string())
}

/// Fetch the buildspec file from a CodeCommit repo. `path` is the buildspec path
/// (defaulting to `buildspec.yml`); `commit` is the source version (branch / tag
/// / commit id) or None for the repo default branch.
pub async fn fetch_codebuild_buildspec(
    client: CcClient,
    repo_name: String,
    path: String,
    commit: Option<String>,
) -> std::result::Result<BuildspecResult, String> {
    let file_path = if path.trim().is_empty() {
        "buildspec.yml".to_string()
    } else {
        path
    };
    let mut req = client
        .get_file()
        .repository_name(&repo_name)
        .file_path(&file_path);
    if let Some(c) = commit.as_deref().filter(|c| !c.is_empty()) {
        req = req.commit_specifier(c);
    }
    match req.send().await {
        Ok(resp) => match String::from_utf8(resp.file_content().as_ref().to_vec()) {
            Ok(text) => Ok(BuildspecResult::Content(text)),
            Err(_) => Err("buildspec is not valid UTF-8".to_string()),
        },
        Err(e) => {
            let msg = crate::error::sdk_error_message(&e);
            // FileDoesNotExist / PathDoesNotExist → treat as not found.
            if msg.contains("DoesNotExist") || msg.contains("not exist") {
                Ok(BuildspecResult::NotFound)
            } else {
                Err(msg)
            }
        }
    }
}

/// Where a CodePipeline-driven build gets its source.
enum PipelineSource {
    CodeCommit { repo: String, branch: String },
    Other(String),
}

/// For a project whose source is `CODEPIPELINE`, the buildspec lives in the
/// pipeline's source artifact, not on the project. Find the pipeline whose
/// CodeBuild action runs `project_name`, read its CodeCommit Source action, and
/// fetch the buildspec from that repo/branch.
pub async fn fetch_codebuild_buildspec_via_pipeline(
    cp: CpClient,
    cc: CcClient,
    project_name: String,
    buildspec_path: String,
) -> std::result::Result<BuildspecResult, String> {
    match find_pipeline_source_for_project(&cp, &project_name).await {
        Ok(Some(PipelineSource::CodeCommit { repo, branch })) => {
            let commit = (!branch.is_empty()).then_some(branch);
            fetch_codebuild_buildspec(cc, repo, buildspec_path, commit).await
        }
        Ok(Some(PipelineSource::Other(note))) => Ok(BuildspecResult::Unsupported(note)),
        Ok(None) => Ok(BuildspecResult::Unsupported(
            "No CodePipeline builds this project — can't locate the buildspec source".to_string(),
        )),
        Err(e) => Err(e),
    }
}

async fn find_pipeline_source_for_project(
    cp: &CpClient,
    project_name: &str,
) -> std::result::Result<Option<PipelineSource>, String> {
    let mut token: Option<String> = None;
    loop {
        let mut req = cp.list_pipelines();
        if let Some(t) = &token {
            req = req.next_token(t);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| crate::error::sdk_error_message(&e))?;

        for summary in resp.pipelines() {
            let Some(name) = summary.name() else { continue };
            let Ok(def) = cp.get_pipeline().name(name).send().await else {
                continue;
            };
            let Some(pipeline) = def.pipeline() else { continue };

            let mut builds_project = false;
            let mut source: Option<PipelineSource> = None;
            for stage in pipeline.stages() {
                for action in stage.actions() {
                    let category = action
                        .action_type_id()
                        .map(|t| t.category().as_str().to_string())
                        .unwrap_or_default();
                    let provider = action
                        .action_type_id()
                        .map(|t| t.provider().to_string())
                        .unwrap_or_default();
                    let config = action.configuration();
                    match category.as_str() {
                        "Build" | "Test" if provider == "CodeBuild" => {
                            if config
                                .and_then(|c| c.get("ProjectName"))
                                .map(|p| p == project_name)
                                .unwrap_or(false)
                            {
                                builds_project = true;
                            }
                        }
                        "Source" => {
                            if provider == "CodeCommit" {
                                let repo = config
                                    .and_then(|c| c.get("RepositoryName"))
                                    .cloned()
                                    .unwrap_or_default();
                                let branch = config
                                    .and_then(|c| c.get("BranchName"))
                                    .cloned()
                                    .unwrap_or_default();
                                if !repo.is_empty() {
                                    source = Some(PipelineSource::CodeCommit { repo, branch });
                                }
                            } else if source.is_none() {
                                source = Some(PipelineSource::Other(format!(
                                    "Pipeline source is {} (not CodeCommit) — can't read the buildspec here",
                                    provider
                                )));
                            }
                        }
                        _ => {}
                    }
                }
            }

            if builds_project {
                return Ok(Some(source.unwrap_or_else(|| {
                    PipelineSource::Other(
                        "Found the pipeline but no source stage to read the buildspec from"
                            .to_string(),
                    )
                })));
            }
        }

        token = crate::aws::pagination::next_page_token(resp.next_token(), &token);
        if token.is_none() {
            break;
        }
    }
    Ok(None)
}

fn fmt_dt(dt: Option<&aws_smithy_types::DateTime>) -> String {
    dt.map(|d| {
        d.fmt(aws_smithy_types::date_time::Format::DateTime)
            .unwrap_or_default()
    })
    .unwrap_or_default()
}

// ═══════════════════════════════════════════════════════════════════════════════
// CodeDeploy Deployment Group
// ═══════════════════════════════════════════════════════════════════════════════

/// A pointer to a group's last-attempted / last-successful deployment.
#[derive(Debug, Clone)]
pub struct DeploymentRef {
    pub id: String,
    pub status: String,
    pub create_time: String,
    pub end_time: String,
}

#[derive(Debug, Clone)]
pub struct CodeDeployGroup {
    /// "application / group" — the list row label (group names repeat across apps).
    pub label: String,
    pub app_name: String,
    pub group_name: String,
    pub group_id: String,
    pub deployment_config: String,
    pub compute_platform: String,
    pub service_role: String,
    /// IN_PLACE / BLUE_GREEN (empty when no explicit style).
    pub deployment_type: String,
    /// WITH_TRAFFIC_CONTROL / WITHOUT_TRAFFIC_CONTROL.
    pub deployment_option: String,
    pub auto_rollback_enabled: bool,
    pub auto_rollback_events: Vec<String>,
    /// "Key=Value (TYPE)" rows from the EC2 tag filters.
    pub ec2_tag_filters: Vec<String>,
    pub asg_names: Vec<String>,
    /// "cluster/service" rows from the ECS targets.
    pub ecs_services: Vec<String>,
    pub last_attempted: Option<DeploymentRef>,
    pub last_successful: Option<DeploymentRef>,
    pub tags: HashMap<String, String>,
}

impl CodeDeployGroup {
    fn from_sdk(g: &aws_sdk_codedeploy::types::DeploymentGroupInfo) -> Self {
        let app_name = g.application_name().unwrap_or_default().to_string();
        let group_name = g.deployment_group_name().unwrap_or_default().to_string();
        let style = g.deployment_style();
        let rollback = g.auto_rollback_configuration();
        let to_ref = |d: &aws_sdk_codedeploy::types::LastDeploymentInfo| DeploymentRef {
            id: d.deployment_id().unwrap_or_default().to_string(),
            status: d
                .status()
                .map(|s| s.as_str().to_string())
                .unwrap_or_default(),
            create_time: fmt_dt(d.create_time()),
            end_time: fmt_dt(d.end_time()),
        };
        Self {
            label: format!("{} / {}", app_name, group_name),
            app_name,
            group_name,
            group_id: g.deployment_group_id().unwrap_or_default().to_string(),
            deployment_config: g.deployment_config_name().unwrap_or_default().to_string(),
            compute_platform: g
                .compute_platform()
                .map(|c| c.as_str().to_string())
                .unwrap_or_default(),
            service_role: g.service_role_arn().unwrap_or_default().to_string(),
            deployment_type: style
                .and_then(|s| s.deployment_type())
                .map(|t| t.as_str().to_string())
                .unwrap_or_default(),
            deployment_option: style
                .and_then(|s| s.deployment_option())
                .map(|o| o.as_str().to_string())
                .unwrap_or_default(),
            auto_rollback_enabled: rollback.map(|r| r.enabled()).unwrap_or(false),
            auto_rollback_events: rollback
                .map(|r| r.events().iter().map(|e| e.as_str().to_string()).collect())
                .unwrap_or_default(),
            ec2_tag_filters: g
                .ec2_tag_filters()
                .iter()
                .map(|f| {
                    let key = f.key().unwrap_or_default();
                    let val = f.value().unwrap_or_default();
                    let ty = f
                        .r#type()
                        .map(|t| t.as_str())
                        .unwrap_or("");
                    format!("{}={} ({})", key, val, ty)
                })
                .collect(),
            asg_names: g
                .auto_scaling_groups()
                .iter()
                .filter_map(|a| a.name().map(|s| s.to_string()))
                .collect(),
            ecs_services: g
                .ecs_services()
                .iter()
                .map(|s| {
                    format!(
                        "{}/{}",
                        s.cluster_name().unwrap_or_default(),
                        s.service_name().unwrap_or_default()
                    )
                })
                .collect(),
            last_attempted: g.last_attempted_deployment().map(to_ref),
            last_successful: g.last_successful_deployment().map(to_ref),
            tags: HashMap::new(),
        }
    }
}

/// Map a CodeDeploy deployment/last-deployment status to a resource state colour.
fn codedeploy_state(status: &str) -> ResourceState {
    match status {
        "Succeeded" => ResourceState::Available,
        "Failed" | "Stopped" => ResourceState::Unavailable,
        "Created" | "Queued" | "InProgress" | "Ready" | "Baking" => ResourceState::Pending,
        other => ResourceState::Unknown(other.to_string()),
    }
}

crate::sections! {
    pub enum CodeDeployGroupDetailSection,
    pub static CODE_DEPLOY_SECTIONS = [
        Overview "Overview",
        Targets "Targets",
        Deployments "Deployments" => crate::app::App::trigger_code_deploy_deployments_load,
    ]
}

impl Resource for CodeDeployGroup {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&CODE_DEPLOY_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.group_id
    }
    fn name(&self) -> &str {
        &self.label
    }
    fn resource_type(&self) -> &str {
        "CodeDeploy Group"
    }
    fn state(&self) -> ResourceState {
        self.last_attempted
            .as_ref()
            .map(|d| codedeploy_state(&d.status))
            .unwrap_or_else(|| ResourceState::Unknown(String::new()))
    }
    fn state_label(&self) -> String {
        // The group's colour is its last attempted deployment's status.
        native_state_label(
            self.last_attempted.as_ref().map_or("", |d| d.status.as_str()),
            || self.state(),
        )
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        format!(
            "{} {} {} {} {}",
            self.label, self.app_name, self.group_name, self.compute_platform, self.deployment_config
        )
    }
    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Application".to_string(), self.app_name.clone()),
            ("Deployment Group".to_string(), self.group_name.clone()),
            ("Group ID".to_string(), self.group_id.clone()),
            ("Compute Platform".to_string(), self.compute_platform.clone()),
            ("Deployment Config".to_string(), self.deployment_config.clone()),
        ]
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/codesuite/codedeploy/applications/{}/deployment-groups/{}?region={}",
            region, self.app_name, self.group_name, region
        ))
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// CodeArtifact Repository
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct CodeArtifactRepo {
    pub name: String,
    pub domain_name: String,
    pub domain_owner: String,
    pub admin_account: String,
    pub arn: String,
    pub description: String,
    pub created: String,
    pub tags: HashMap<String, String>,
}

impl CodeArtifactRepo {
    fn from_sdk(r: &aws_sdk_codeartifact::types::RepositorySummary) -> Self {
        Self {
            name: r.name().unwrap_or_default().to_string(),
            domain_name: r.domain_name().unwrap_or_default().to_string(),
            domain_owner: r.domain_owner().unwrap_or_default().to_string(),
            admin_account: r.administrator_account().unwrap_or_default().to_string(),
            arn: r.arn().unwrap_or_default().to_string(),
            description: r.description().unwrap_or_default().to_string(),
            created: fmt_dt(r.created_time()),
            tags: HashMap::new(),
        }
    }
}

crate::sections! {
    pub enum CodeArtifactRepoDetailSection,
    pub static CODE_ARTIFACT_SECTIONS = [
        Overview "Overview",
        Packages "Packages" => crate::app::App::trigger_code_artifact_packages_load,
    ]
}

impl Resource for CodeArtifactRepo {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&CODE_ARTIFACT_SECTIONS)
    }
    fn id(&self) -> &str {
        // ARN is unique; repo names can repeat across domains.
        &self.arn
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn resource_type(&self) -> &str {
        "CodeArtifact Repository"
    }
    fn state(&self) -> ResourceState {
        ResourceState::stateless()
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        format!(
            "{} {} {} {}",
            self.name, self.domain_name, self.arn, self.description
        )
    }
    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Name".to_string(), self.name.clone()),
            ("Domain".to_string(), self.domain_name.clone()),
            ("ARN".to_string(), self.arn.clone()),
            ("Description".to_string(), self.description.clone()),
            ("Created".to_string(), self.created.clone()),
        ]
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/codesuite/codeartifact/d/{}/{}/r/{}",
            region, self.domain_owner, self.domain_name, self.name
        ))
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Lazy-loaded CodeDeploy recent deployments (per deployment group)
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct CodeDeployDeployment {
    pub deployment_id: String,
    pub status: String,
    pub create_time: String,
    pub complete_time: String,
    pub description: String,
    pub error_code: String,
    pub error_message: String,
    /// Instance/target summary counts from the deployment overview.
    pub pending: i64,
    pub in_progress: i64,
    pub succeeded: i64,
    pub failed: i64,
    pub skipped: i64,
    pub ready: i64,
    pub rollback_message: String,
    pub rollback_deployment_id: String,
}

/// Recent deployments for one deployment group: `ListDeployments` (most recent
/// first, capped) then `BatchGetDeployments` for status + overview counts.
pub async fn fetch_deployment_group_deployments(
    client: CdClient,
    app_name: String,
    group_name: String,
) -> std::result::Result<Vec<CodeDeployDeployment>, String> {
    let list_resp = client
        .list_deployments()
        .application_name(&app_name)
        .deployment_group_name(&group_name)
        .send()
        .await
        .map_err(|e| crate::error::sdk_error_message(&e))?;

    let ids: Vec<String> = list_resp.deployments().iter().take(10).cloned().collect();
    if ids.is_empty() {
        return Ok(vec![]);
    }

    let resp = client
        .batch_get_deployments()
        .set_deployment_ids(Some(ids))
        .send()
        .await
        .map_err(|e| crate::error::sdk_error_message(&e))?;

    let mut deployments: Vec<CodeDeployDeployment> = resp
        .deployments_info()
        .iter()
        .map(|d| {
            let ov = d.deployment_overview();
            let err = d.error_information();
            let rb = d.rollback_info();
            CodeDeployDeployment {
                deployment_id: d.deployment_id().unwrap_or_default().to_string(),
                status: d
                    .status()
                    .map(|s| s.as_str().to_string())
                    .unwrap_or_default(),
                create_time: fmt_dt(d.create_time()),
                complete_time: fmt_dt(d.complete_time()),
                description: d.description().unwrap_or_default().to_string(),
                error_code: err
                    .and_then(|e| e.code())
                    .map(|c| c.as_str().to_string())
                    .unwrap_or_default(),
                error_message: err.and_then(|e| e.message()).unwrap_or_default().to_string(),
                pending: ov.map(|o| o.pending()).unwrap_or(0),
                in_progress: ov.map(|o| o.in_progress()).unwrap_or(0),
                succeeded: ov.map(|o| o.succeeded()).unwrap_or(0),
                failed: ov.map(|o| o.failed()).unwrap_or(0),
                skipped: ov.map(|o| o.skipped()).unwrap_or(0),
                ready: ov.map(|o| o.ready()).unwrap_or(0),
                rollback_message: rb
                    .and_then(|r| r.rollback_message())
                    .unwrap_or_default()
                    .to_string(),
                rollback_deployment_id: rb
                    .and_then(|r| r.rollback_deployment_id())
                    .unwrap_or_default()
                    .to_string(),
            }
        })
        .collect();

    // BatchGetDeployments does not preserve the ListDeployments order; keep most
    // recent first by create time (ISO-8601 strings sort lexicographically).
    deployments.sort_by(|a, b| b.create_time.cmp(&a.create_time));
    Ok(deployments)
}

// ═══════════════════════════════════════════════════════════════════════════════
// Lazy-loaded CodeArtifact packages (per repository)
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct CodeArtifactPackage {
    pub format: String,
    pub namespace: String,
    pub package: String,
    /// The repo's default display version for this package (enriched via
    /// `ListPackageVersions`; empty when not fetched / none published).
    pub latest_version: String,
}

/// Max packages to enrich with a `ListPackageVersions` call (bounds the N+1).
const MAX_PACKAGE_VERSION_LOOKUPS: usize = 40;
/// Max packages listed for a repo (bounds huge repos).
const MAX_PACKAGES: usize = 300;

/// Packages in a repo via `ListPackages`, each enriched (up to a cap) with its
/// default display version via `ListPackageVersions`.
pub async fn fetch_repo_packages(
    client: CaClient,
    domain: String,
    domain_owner: String,
    repository: String,
) -> std::result::Result<Vec<CodeArtifactPackage>, String> {
    let mut packages: Vec<(aws_sdk_codeartifact::types::PackageFormat, String, String)> = Vec::new();
    let mut paginator = client
        .list_packages()
        .domain(&domain)
        .repository(&repository)
        .into_paginator()
        .send();
    'outer: while let Some(result) = paginator.next().await {
        let page = result.map_err(|e| crate::error::sdk_error_message(&e))?;
        for p in page.packages() {
            let Some(fmt) = p.format().cloned() else {
                continue;
            };
            packages.push((
                fmt,
                p.namespace().unwrap_or_default().to_string(),
                p.package().unwrap_or_default().to_string(),
            ));
            if packages.len() >= MAX_PACKAGES {
                break 'outer;
            }
        }
    }

    let mut out: Vec<CodeArtifactPackage> = Vec::with_capacity(packages.len());
    for (i, (fmt, namespace, package)) in packages.into_iter().enumerate() {
        let mut latest_version = String::new();
        if i < MAX_PACKAGE_VERSION_LOOKUPS {
            let mut req = client
                .list_package_versions()
                .domain(&domain)
                .repository(&repository)
                .format(fmt.clone())
                .package(&package)
                .max_results(1);
            if !domain_owner.is_empty() {
                req = req.domain_owner(&domain_owner);
            }
            if !namespace.is_empty() {
                req = req.namespace(&namespace);
            }
            if let Ok(resp) = req.send().await {
                latest_version = resp.default_display_version().unwrap_or_default().to_string();
            }
        }
        out.push(CodeArtifactPackage {
            format: fmt.as_str().to_string(),
            namespace,
            package,
            latest_version,
        });
    }
    Ok(out)
}

// ── CodeBuild project CloudWatch metrics (`m`) — AWS/CodeBuild, dim ProjectName

use crate::aws::services::ec2::{parse_metric_datapoints, MetricsTimeRange};

#[derive(Debug, Clone)]
pub struct CodeBuildMetricsData {
    pub time_range: MetricsTimeRange,
    pub builds: Vec<(f64, f64)>,
    pub succeeded: Vec<(f64, f64)>,
    pub failed: Vec<(f64, f64)>,
    pub duration_avg: Vec<(f64, f64)>,
    pub x_max: f64,
}

#[derive(Debug, Clone)]
pub enum CodeBuildMetricsState {
    Loading,
    Loaded(CodeBuildMetricsData),
}

/// Build cadence + health for one project: counts (total / succeeded /
/// failed) and the average build duration (a creeping duration is the
/// cache-regression signal).
pub async fn fetch_codebuild_metrics(
    cw: aws_sdk_cloudwatch::Client,
    project_name: String,
    time_range: MetricsTimeRange,
) -> crate::error::Result<CodeBuildMetricsData> {
    use aws_sdk_cloudwatch::types::{Dimension, Statistic};

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let start = now - time_range.duration_secs();
    let period = time_range.period_secs();
    let start_dt = aws_sdk_cloudwatch::primitives::DateTime::from_secs(start);
    let end_dt = aws_sdk_cloudwatch::primitives::DateTime::from_secs(now);

    let metric = |name: &'static str, stat: Statistic| {
        cw.get_metric_statistics()
            .namespace("AWS/CodeBuild")
            .metric_name(name)
            .dimensions(Dimension::builder().name("ProjectName").value(&project_name).build())
            .start_time(start_dt)
            .end_time(end_dt)
            .period(period)
            .set_statistics(Some(vec![stat]))
            .send()
    };

    let (builds, succeeded, failed, duration) = tokio::join!(
        metric("Builds", Statistic::Sum),
        metric("SucceededBuilds", Statistic::Sum),
        metric("FailedBuilds", Statistic::Sum),
        metric("Duration", Statistic::Average),
    );

    Ok(CodeBuildMetricsData {
        time_range,
        builds: parse_metric_datapoints(builds, start),
        succeeded: parse_metric_datapoints(succeeded, start),
        failed: parse_metric_datapoints(failed, start),
        duration_avg: parse_metric_datapoints(duration, start),
        x_max: time_range.duration_secs() as f64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn action(provider: &str, external_id: &str, log_stream_arn: &str) -> PipelineActionExecution {
        PipelineActionExecution {
            stage_name: "Build".to_string(),
            action_name: "Build".to_string(),
            status: "Failed".to_string(),
            start_time: String::new(),
            start_ms: 0,
            duration_secs: 0,
            updated_by: String::new(),
            category: "Build".to_string(),
            owner: "AWS".to_string(),
            provider: provider.to_string(),
            region: String::new(),
            role_arn: String::new(),
            namespace: String::new(),
            target: None,
            configuration: Vec::new(),
            input_artifacts: Vec::new(),
            output_artifacts: Vec::new(),
            output_variables: Vec::new(),
            external_execution_id: external_id.to_string(),
            external_execution_summary: String::new(),
            external_execution_url: String::new(),
            log_stream_arn: log_stream_arn.to_string(),
            error_code: String::new(),
            error_message: String::new(),
        }
    }

    #[test]
    fn codebuild_external_id_yields_the_project_name() {
        // `<project>:<build-uuid>` — the jump target is the project, since
        // builds live inside the project's own pane.
        let a = action("CodeBuild", "infra-build:7c2f-aaaa", "");
        assert_eq!(a.build_project(), Some("infra-build"));
        // Any other provider's external id is not a build id.
        let a = action("CloudFormation", "arn:aws:cloudformation:…", "");
        assert_eq!(a.build_project(), None);
        // A CodeBuild action that never started has no id to split.
        let a = action("CodeBuild", "", "");
        assert_eq!(a.build_project(), None);
    }

    #[test]
    fn log_stream_arn_splits_into_group_and_stream() {
        let a = action(
            "CodeBuild",
            "p:1",
            "arn:aws:logs:us-east-1:123456789012:log-group:/aws/codebuild/p:log-stream:abc-def",
        );
        assert_eq!(
            a.log_group_and_stream(),
            Some(("/aws/codebuild/p".to_string(), "abc-def".to_string()))
        );
        // No stream ARN at all (a Source or Approval action) — nothing to tail.
        assert_eq!(action("CodeBuild", "p:1", "").log_group_and_stream(), None);
    }

    #[test]
    fn action_target_labels_are_the_jump_keys() {
        let cfg: HashMap<String, String> =
            [("ProjectName".to_string(), "my-build".to_string())].into();
        assert_eq!(
            action_target("CodeBuild", &cfg),
            Some(("Project", "my-build".to_string()))
        );
        // A provider we don't map, and a mapped provider with no value, both
        // yield no row rather than an empty one.
        assert_eq!(action_target("ThirdParty", &cfg), None);
        assert_eq!(action_target("CodeBuild", &HashMap::new()), None);
    }

    #[test]
    fn config_rows_drop_the_target_key_and_anything_credential_shaped() {
        let cfg: HashMap<String, String> = [
            ("ProjectName".to_string(), "my-build".to_string()),
            ("OAuthToken".to_string(), "hunter2".to_string()),
            ("PrimarySource".to_string(), "src".to_string()),
            ("Empty".to_string(), String::new()),
        ]
        .into();
        let rows = other_config(&cfg, target_config_key("CodeBuild", &cfg));
        assert_eq!(rows, vec![("PrimarySource".to_string(), "src".to_string())]);
    }

    #[test]
    fn executions_sort_newest_first() {
        let mk = |id: &str, status: &str, start_ms: i64| CodePipelineExecution {
            execution_id: id.to_string(),
            pipeline_name: "p".to_string(),
            display_name: format!("p / {}", id),
            status: status.to_string(),
            status_summary: String::new(),
            start_time: String::new(),
            start_ms,
            duration_secs: 0,
            trigger: String::new(),
            trigger_detail: String::new(),
            execution_mode: String::new(),
            execution_type: String::new(),
            rollback_target: String::new(),
            stop_reason: String::new(),
            source_revisions: Vec::new(),
            tags: HashMap::new(),
        };
        let mut execs = vec![
            mk("old", "Succeeded", 100),
            mk("new", "Failed", 300),
            mk("live", "InProgress", 200),
        ];
        sort_executions(&mut execs);
        // Strictly by start time — a still-running run gets no special place.
        let order: Vec<&str> = execs.iter().map(|e| e.execution_id.as_str()).collect();
        assert_eq!(order, vec!["new", "live", "old"]);
    }
}
