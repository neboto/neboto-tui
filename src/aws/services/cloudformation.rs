use crate::aws::client::AwsClients;
use crate::aws::resource::{native_state_label, Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::error::Result;
use crate::event::{Event, LoadProgress};
use async_trait::async_trait;
use aws_sdk_cloudformation::Client as CfnClient;
use std::any::Any;
use std::collections::HashMap;
use tokio::sync::mpsc;

pub struct CloudFormationService {
    client: CfnClient,
}

impl CloudFormationService {
    pub fn new(aws_clients: &AwsClients) -> Self {
        Self {
            client: aws_clients.cloudformation_client(),
        }
    }
}

#[async_trait]
impl AwsService for CloudFormationService {
    fn service_type(&self) -> ServiceType {
        ServiceType::CloudFormation
    }

    fn name(&self) -> &str {
        "CloudFormation"
    }

    async fn list_resources(&self) -> Result<Vec<Box<dyn Resource>>> {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        self.list_resources_streaming(tx, ServiceType::CloudFormation)
            .await?;
        Ok(vec![])
    }

    async fn list_resources_streaming(
        &self,
        event_tx: mpsc::UnboundedSender<Event>,
        service_type: ServiceType,
    ) -> Result<()> {
        let mut total = 0usize;

        // Phase 1: Describe all stacks (paginated)
        let mut stack_resources: Vec<Box<dyn Resource>> = Vec::new();
        let mut paginator = self
            .client
            .describe_stacks()
            .into_paginator()
            .send();

        while let Some(result) = paginator.next().await {
            match result {
                Ok(page) => {
                    for stack in page.stacks() {
                        stack_resources.push(Box::new(CfnStack::from_sdk(stack)));
                    }
                }
                Err(e) => {
                    let _ = event_tx.send(Event::ResourceLoadError {
                        service: service_type,
                        error: format!("Failed to describe CloudFormation stacks: {}", e),
                    });
                    return Ok(());
                }
            }
        }

        let stack_count = stack_resources.len();
        total += stack_count;

        if !stack_resources.is_empty() {
            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                service: service_type,
                resources: stack_resources,
                progress: LoadProgress {
                    loaded_count: total,
                    total_count: None,
                    status_message: Some(format!(
                        "Loaded {} stacks. Loading stack sets...",
                        stack_count
                    )),
                },
            });
        }

        // Phase 2: List stack sets
        let mut stackset_resources: Vec<Box<dyn Resource>> = Vec::new();
        let mut ss_paginator = self
            .client
            .list_stack_sets()
            .into_paginator()
            .send();

        while let Some(result) = ss_paginator.next().await {
            match result {
                Ok(page) => {
                    for ss in page.summaries() {
                        stackset_resources.push(Box::new(CfnStackSet::from_summary(ss)));
                    }
                }
                Err(e) => {
                    let _ = event_tx.send(Event::ResourceLoadWarning {
                        service: service_type,
                        warning: format!(
                            "stack sets unavailable: {}",
                            crate::error::sdk_error_message(&e)
                        ),
                    });
                    break;
                }
            }
        }

        total += stackset_resources.len();

        if !stackset_resources.is_empty() {
            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                service: service_type,
                resources: stackset_resources,
                progress: LoadProgress {
                    loaded_count: total,
                    total_count: None,
                    status_message: None,
                },
            });
        }

        // Phase 3: Exports (cross-stack output values). Error-tolerant — a
        // permission gap shouldn't blank the stacks/stacksets already loaded.
        let mut export_resources: Vec<Box<dyn Resource>> = Vec::new();
        let mut ex_paginator = self.client.list_exports().into_paginator().send();
        while let Some(result) = ex_paginator.next().await {
            match result {
                Ok(page) => {
                    for e in page.exports() {
                        export_resources.push(Box::new(CfnExport::from_sdk(e)));
                    }
                }
                Err(e) => {
                    let _ = event_tx.send(Event::ResourceLoadWarning {
                        service: service_type,
                        warning: format!(
                            "exports unavailable: {}",
                            crate::error::sdk_error_message(&e)
                        ),
                    });
                    break;
                }
            }
        }
        total += export_resources.len();
        if !export_resources.is_empty() {
            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                service: service_type,
                resources: export_resources,
                progress: LoadProgress {
                    loaded_count: total,
                    total_count: None,
                    status_message: None,
                },
            });
        }

        // Phase 4: recently deleted stacks (Deleted sub-tab). ListStacks keeps
        // DELETE_COMPLETE stacks for ~90 days; DescribeStacks (phase 1) never
        // returns them. Collected then sorted newest-deletion-first, because
        // the whole point of the tab is "the stack I deleted last week" and
        // 90 days of CI churn arrives in no useful order.
        let mut deleted_summaries: Vec<aws_sdk_cloudformation::types::StackSummary> = Vec::new();
        let mut del_paginator = self
            .client
            .list_stacks()
            .stack_status_filter(aws_sdk_cloudformation::types::StackStatus::DeleteComplete)
            .into_paginator()
            .send();
        while let Some(result) = del_paginator.next().await {
            match result {
                Ok(page) => {
                    deleted_summaries.extend(page.stack_summaries().iter().cloned());
                }
                Err(e) => {
                    let _ = event_tx.send(Event::ResourceLoadWarning {
                        service: service_type,
                        warning: format!(
                            "deleted stacks unavailable: {}",
                            crate::error::sdk_error_message(&e)
                        ),
                    });
                    break;
                }
            }
        }
        deleted_summaries
            .sort_by_key(|s| std::cmp::Reverse(s.deletion_time().map(|t| t.secs()).unwrap_or(0)));
        let deleted_resources: Vec<Box<dyn Resource>> = deleted_summaries
            .iter()
            .map(|s| Box::new(CfnStack::from_deleted_summary(s)) as Box<dyn Resource>)
            .collect();
        total += deleted_resources.len();
        if !deleted_resources.is_empty() {
            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                service: service_type,
                resources: deleted_resources,
                progress: LoadProgress {
                    loaded_count: total,
                    total_count: None,
                    status_message: None,
                },
            });
        }

        let _ = event_tx.send(Event::ResourcesFullyLoaded {
            service: service_type,
            total_count: total,
        });

        Ok(())
    }

    async fn get_resource_details(&self, id: &str) -> Result<Box<dyn Resource>> {
        Err(crate::error::Error::ResourceNotFound(id.to_string()))
    }
}

// ── Exports (CfnView::Exports) ────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct CfnExport {
    pub name: String,
    pub value: String,
    pub exporting_stack_id: String,
    pub tags: HashMap<String, String>,
}

impl CfnExport {
    pub fn from_sdk(e: &aws_sdk_cloudformation::types::Export) -> Self {
        Self {
            name: e.name().unwrap_or("").to_string(),
            value: e.value().unwrap_or("").to_string(),
            exporting_stack_id: e.exporting_stack_id().unwrap_or("").to_string(),
            tags: HashMap::new(),
        }
    }

    /// The exporting stack's short name (last segment of the stack ARN).
    pub fn exporting_stack_name(&self) -> String {
        self.exporting_stack_id
            .split('/')
            .nth(1)
            .unwrap_or(&self.exporting_stack_id)
            .to_string()
    }
}

crate::sections! {
    pub enum CfnExportDetailSection,
    pub static CFN_EXPORT_SECTIONS = [
        Details "Details",
        Imports "Imports" => crate::app::App::trigger_cfn_imports_load,
    ]
}

impl Resource for CfnExport {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&CFN_EXPORT_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.name // export names are unique account/region-wide
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn resource_type(&self) -> &str {
        "CFN Export"
    }

    fn state(&self) -> ResourceState {
        ResourceState::stateless()
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!(
            "{} {} {}",
            self.name,
            self.value,
            self.exporting_stack_name()
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Name".to_string(), self.name.clone()),
            ("Value".to_string(), self.value.clone()),
            ("Exporting stack".to_string(), self.exporting_stack_name()),
        ]
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{region}.console.aws.amazon.com/cloudformation/home?region={region}#/exports"
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}


/// The stacks that import a given export name (`Fn::ImportValue`). AWS returns
/// an error rather than an empty list when nothing imports it — map that to
/// `None` so the UI can say "not imported" cleanly.
/// Stacks importing an export. AWS *errors* (not empty) when nothing imports
/// the export, so any failure maps to the empty "not imported" result.
pub async fn fetch_cfn_imports(client: &CfnClient, export_name: &str) -> Vec<String> {
    let mut imports: Vec<String> = Vec::new();
    let mut paginator = client
        .list_imports()
        .export_name(export_name)
        .into_paginator()
        .send();
    while let Some(result) = paginator.next().await {
        match result {
            Ok(page) => imports.extend(page.imports().iter().cloned()),
            Err(_) => break,
        }
    }
    imports
}

// ── Lazy-load state types ─────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct CfnStackEvent {
    pub logical_id: String,
    pub resource_type: String,
    pub timestamp: String,
    /// Epoch seconds of `timestamp` — drives the progress rollup's elapsed
    /// times without re-parsing the display string.
    pub ts_secs: Option<i64>,
    pub status: String,
    pub status_reason: Option<String>,
}

impl CfnStackEvent {
    pub fn from_sdk(e: &aws_sdk_cloudformation::types::StackEvent) -> Self {
        Self {
            logical_id: e.logical_resource_id().unwrap_or_default().to_string(),
            resource_type: e.resource_type().unwrap_or_default().to_string(),
            timestamp: e
                .timestamp()
                .map(|t| t.to_string())
                .unwrap_or_default(),
            ts_secs: e.timestamp().map(|t| t.secs()),
            status: e
                .resource_status()
                .map(|s| s.as_str().to_string())
                .unwrap_or_else(|| "UNKNOWN".to_string()),
            status_reason: e.resource_status_reason().map(|s| s.to_string()),
        }
    }
}

/// Fetch the stack's event stream (newest-first, capped at
/// [`CFN_EVENTS_CAP`]). Shared by the lazy Events section trigger and the
/// progress rollup's auto-poll. Mid-pagination failure keeps the partial list
/// when anything already arrived — the recent window is intact — and only
/// errors when nothing did.
pub async fn fetch_stack_events(
    client: aws_sdk_cloudformation::Client,
    stack_id: String,
) -> std::result::Result<Vec<CfnStackEvent>, String> {
    let mut events = Vec::new();
    let mut paginator = client
        .describe_stack_events()
        .stack_name(&stack_id)
        .into_paginator()
        .send();
    'pages: while let Some(result) = paginator.next().await {
        match result {
            Ok(page) => {
                for e in page.stack_events() {
                    if events.len() >= CFN_EVENTS_CAP {
                        break 'pages;
                    }
                    events.push(CfnStackEvent::from_sdk(e));
                }
            }
            Err(e) => {
                if events.is_empty() {
                    return Err(crate::error::sdk_error_message(&e));
                }
                break;
            }
        }
    }
    Ok(events)
}

// ── Progress rollup ("what's left" view of the Events section) ───────────────

/// Latest status of one resource within the current stack operation.
#[derive(Debug, Clone)]
pub struct CfnResourceProgress {
    pub logical_id: String,
    pub resource_type: String,
    pub status: String,
    pub ts_secs: Option<i64>,
    pub status_reason: Option<String>,
}

/// Per-resource rollup of the **current (most recent) operation**, derived
/// from the newest-first event stream.
#[derive(Debug, Clone, Default)]
pub struct CfnProgressRollup {
    /// The newest stack-level event's status — it names the operation
    /// ("CREATE_IN_PROGRESS", "UPDATE_ROLLBACK_COMPLETE", …).
    pub op_status: Option<String>,
    /// When the operation started (its "User Initiated" stack event), if the
    /// fetched window reaches back that far.
    pub op_start_secs: Option<i64>,
    /// True when the walk ran out of events before finding the operation's
    /// start — the fetch cap truncated the window, so the rollup may miss
    /// resources the operation already touched.
    pub window_truncated: bool,
    /// Latest status per resource, most-recently-active first. The stack's
    /// own events are excluded (nested stacks, whose logical ID differs from
    /// the stack name, stay in).
    pub resources: Vec<CfnResourceProgress>,
}

impl CfnProgressRollup {
    /// True while the operation is still converging — drives the auto-poll.
    pub fn in_progress(&self) -> bool {
        self.op_status
            .as_deref()
            .is_some_and(|s| s.ends_with("_IN_PROGRESS"))
    }
}

/// Fold the newest-first event stream into the current operation's rollup.
///
/// The operation window is bounded by walking newest-first until either the
/// operation-start marker — a stack-level `*_IN_PROGRESS` event whose reason
/// contains "User Initiated" (create/update/delete all emit one; rollback
/// phases of the same operation don't, so a rollback chain stays in the
/// window) — or a stack-level terminal event that isn't the newest event
/// overall (the previous operation's end, for streams whose newest operation
/// has already settled).
pub fn cfn_progress_rollup(stack_name: &str, events: &[CfnStackEvent]) -> CfnProgressRollup {
    let mut rollup = CfnProgressRollup {
        window_truncated: true,
        ..Default::default()
    };
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for (i, e) in events.iter().enumerate() {
        let stack_level =
            e.logical_id == stack_name && e.resource_type == "AWS::CloudFormation::Stack";
        if stack_level {
            let terminal = e.status.ends_with("_COMPLETE") || e.status.ends_with("_FAILED");
            if terminal && i > 0 {
                // The previous operation's end — window complete.
                rollup.window_truncated = false;
                break;
            }
            if rollup.op_status.is_none() {
                rollup.op_status = Some(e.status.clone());
            }
            if e.status.ends_with("_IN_PROGRESS")
                && e.status_reason
                    .as_deref()
                    .is_some_and(|r| r.contains("User Initiated"))
            {
                rollup.op_start_secs = e.ts_secs;
                rollup.window_truncated = false;
                break;
            }
            continue;
        }
        if seen.insert(&e.logical_id) {
            rollup.resources.push(CfnResourceProgress {
                logical_id: e.logical_id.clone(),
                resource_type: e.resource_type.clone(),
                status: e.status.clone(),
                ts_secs: e.ts_secs,
                status_reason: e.status_reason.clone(),
            });
        }
    }
    rollup
}

/// One resource declared by a template's `Resources` block.
#[derive(Debug, Clone, PartialEq)]
pub struct CfnTemplateResource {
    pub logical_id: String,
    pub resource_type: String,
    /// The entry carries a `Condition:` — when it evaluates false the
    /// resource is never instantiated and never emits an event, so "no
    /// events yet" doesn't mean "pending" for it. Conditions can't be
    /// evaluated client-side (they need intrinsic-function evaluation), so
    /// the rollup annotates instead of guessing.
    pub conditional: bool,
}

/// Extract the resources a template declares — the full set behind the
/// rollup's "not started" group. JSON parses exactly (`serde_json`); YAML
/// gets a structural indent scan of the top-level `Resources:` block (no
/// YAML dependency): logical IDs are the keys at the block's first-child
/// indent, and each entry's `Type:` / `Condition:` are read only at the
/// entry's own first-child indent, so `Type:` keys nested inside
/// `Properties:` (listener actions, attribute definitions) can't shadow
/// them. Returns `None` when the body can't be read — callers render the
/// rollup without the group.
pub fn parse_template_resource_ids(body: &str) -> Option<Vec<CfnTemplateResource>> {
    if body.trim_start().starts_with('{') {
        let v: serde_json::Value = serde_json::from_str(body).ok()?;
        let resources = v.get("Resources")?.as_object()?;
        return Some(
            resources
                .iter()
                .map(|(id, r)| CfnTemplateResource {
                    logical_id: id.clone(),
                    resource_type: r
                        .get("Type")
                        .and_then(|t| t.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    conditional: r.get("Condition").is_some(),
                })
                .collect(),
        );
    }
    let mut out: Vec<CfnTemplateResource> = Vec::new();
    let mut in_resources = false;
    let mut child_indent: Option<usize> = None;
    let mut sub_indent: Option<usize> = None;
    let mut current: Option<usize> = None;
    for raw in body.lines() {
        let line = raw.trim_end();
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = line.len() - trimmed.len();
        if indent == 0 {
            in_resources = line == "Resources:";
            child_indent = None;
            sub_indent = None;
            current = None;
            continue;
        }
        if !in_resources {
            continue;
        }
        let ci = *child_indent.get_or_insert(indent);
        if indent == ci {
            sub_indent = None;
            current = trimmed
                .strip_suffix(':')
                .filter(|id| !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric()))
                .map(|id| {
                    out.push(CfnTemplateResource {
                        logical_id: id.to_string(),
                        resource_type: String::new(),
                        conditional: false,
                    });
                    out.len() - 1
                });
        } else if indent > ci {
            if let Some(cur) = current {
                let si = *sub_indent.get_or_insert(indent);
                if indent == si {
                    if let Some(ty) = trimmed.strip_prefix("Type:") {
                        if out[cur].resource_type.is_empty() {
                            out[cur].resource_type =
                                ty.trim().trim_matches('"').trim_matches('\'').to_string();
                        }
                    } else if trimmed.strip_prefix("Condition:").is_some() {
                        out[cur].conditional = true;
                    }
                }
            }
        }
    }
    if out.is_empty() { None } else { Some(out) }
}

// ── Resource drift (lazy Drift section) ───────────────────────────────────────

#[derive(Debug, Clone)]
pub struct CfnPropertyDiff {
    pub path: String,
    pub expected: String,
    pub actual: String,
    pub diff_type: String,
}

#[derive(Debug, Clone)]
pub struct CfnResourceDrift {
    pub logical_id: String,
    pub physical_id: Option<String>,
    pub resource_type: String,
    pub drift_status: String,
    pub differences: Vec<CfnPropertyDiff>,
}

impl CfnResourceDrift {
    pub fn from_sdk(d: &aws_sdk_cloudformation::types::StackResourceDrift) -> Self {
        let differences = d
            .property_differences()
            .iter()
            .map(|p| CfnPropertyDiff {
                path: p.property_path().unwrap_or_default().to_string(),
                expected: p.expected_value().unwrap_or_default().to_string(),
                actual: p.actual_value().unwrap_or_default().to_string(),
                diff_type: p
                    .difference_type()
                    .map(|t| t.as_str().to_string())
                    .unwrap_or_default(),
            })
            .collect();
        Self {
            logical_id: d.logical_resource_id().unwrap_or_default().to_string(),
            physical_id: d.physical_resource_id().map(|s| s.to_string()),
            resource_type: d.resource_type().unwrap_or_default().to_string(),
            drift_status: d
                .stack_resource_drift_status()
                .map(|s| s.as_str().to_string())
                .unwrap_or_else(|| "UNKNOWN".to_string()),
            differences,
        }
    }
}

// ── Change sets (lazy Changes section) ────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct CfnChange {
    pub action: String,
    pub logical_id: String,
    pub resource_type: String,
    /// The live resource the change will touch (Modify/Remove/Import; Adds
    /// have none yet). Rendered as a "Physical ID" row — that label is
    /// load-bearing, `cfn_resource_jump_target` keys on it for the
    /// Enter-jump to the resource.
    pub physical_id: Option<String>,
    pub replacement: Option<String>,
    /// What happens to the old resource on replace/remove (Delete / Retain /
    /// Snapshot / ReplaceAndDelete / …) — the DeletionPolicy outcome.
    pub policy_action: Option<String>,
    /// Per-property modifications (`IncludePropertyValues` diff rows).
    pub details: Vec<CfnChangeDetail>,
    /// The change's full before/after resource JSON (the console's "JSON
    /// changes" view) — kept for the `e` editor export, not rendered inline.
    pub before_context: Option<String>,
    pub after_context: Option<String>,
}

/// One property-level modification within a resource change.
#[derive(Debug, Clone)]
pub struct CfnChangeDetail {
    /// Property path when the API provides it (newer), else the target name.
    pub label: String,
    pub attribute: String,
    pub before: Option<String>,
    pub after: Option<String>,
    /// Static (known now) vs Dynamic (resolved only at execution).
    pub evaluation: Option<String>,
    /// Never / Conditionally / Always.
    pub requires_recreation: Option<String>,
    /// What triggered it (DirectModification, ParameterReference, …) + the
    /// entity (parameter name, ref target).
    pub change_source: Option<String>,
    pub causing_entity: Option<String>,
}

/// A CloudFormation Hook that will run (or ran) against a change set — the
/// console's "validations".
#[derive(Debug, Clone)]
pub struct CfnChangeSetHook {
    pub type_name: String,
    pub invocation_point: Option<String>,
    pub failure_mode: Option<String>,
    /// "LogicalId (AWS::Type) Action" when the hook targets one resource.
    pub target: Option<String>,
}

/// A hook invocation result for the change set (exists once hooks have run).
#[derive(Debug, Clone)]
pub struct CfnHookResult {
    pub type_name: String,
    pub status: String,
    pub reason: Option<String>,
    pub failure_mode: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CfnChangeSet {
    pub name: String,
    pub id: String,
    pub status: String,
    pub execution_status: String,
    pub status_reason: Option<String>,
    pub creation_time: String,
    pub description: Option<String>,
    pub changes: Vec<CfnChange>,
    pub hooks: Vec<CfnChangeSetHook>,
    pub hook_results: Vec<CfnHookResult>,
}

impl CfnChangeSet {
    /// Any change carries a before/after JSON context (drives the `e` hint
    /// and the editor export).
    pub fn has_json_context(&self) -> bool {
        self.changes
            .iter()
            .any(|c| c.before_context.is_some() || c.after_context.is_some())
    }
}

/// List a stack's change sets and resolve each one's resource changes
/// (property-value diffs included), planned hook invocations, and hook
/// results. Change sets are usually few (0-2), so describing each is cheap;
/// the hook calls are best-effort — accounts without hooks (or the IAM
/// actions) just render without the Validations rows.
pub async fn fetch_cfn_change_sets(client: &CfnClient, stack_name: &str) -> Vec<CfnChangeSet> {
    let summaries = match client.list_change_sets().stack_name(stack_name).send().await {
        Ok(resp) => resp.summaries().to_vec(),
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    for s in summaries {
        let id = s.change_set_id().unwrap_or_default().to_string();
        let name = s.change_set_name().unwrap_or_default().to_string();
        // Resolve the per-resource changes for this set, with property values
        // (the console's per-property diff + JSON changes need them).
        let mut changes = Vec::new();
        if let Ok(resp) = client
            .describe_change_set()
            .change_set_name(&id)
            .include_property_values(true)
            .send()
            .await
        {
            for ch in resp.changes() {
                if let Some(rc) = ch.resource_change() {
                    changes.push(cfn_change_from_sdk(rc));
                }
            }
        }
        // Planned hook invocations ("validations").
        let mut hooks = Vec::new();
        if let Ok(resp) = client
            .describe_change_set_hooks()
            .change_set_name(&id)
            .send()
            .await
        {
            for h in resp.hooks() {
                let target = h
                    .target_details()
                    .and_then(|t| t.resource_target_details())
                    .map(|r| {
                        format!(
                            "{} ({}) {}",
                            r.logical_resource_id().unwrap_or_default(),
                            r.resource_type().unwrap_or_default(),
                            r.resource_action().map(|a| a.as_str()).unwrap_or_default()
                        )
                        .trim()
                        .to_string()
                    });
                hooks.push(CfnChangeSetHook {
                    type_name: h.type_name().unwrap_or_default().to_string(),
                    invocation_point: h.invocation_point().map(|p| p.as_str().to_string()),
                    failure_mode: h.failure_mode().map(|m| m.as_str().to_string()),
                    target,
                });
            }
        }
        // Hook results (populated once the hooks have actually run).
        let mut hook_results = Vec::new();
        if let Ok(resp) = client
            .list_hook_results()
            .target_type(aws_sdk_cloudformation::types::ListHookResultsTargetType::ChangeSet)
            .target_id(&id)
            .send()
            .await
        {
            for r in resp.hook_results() {
                hook_results.push(CfnHookResult {
                    type_name: r.type_name().unwrap_or_default().to_string(),
                    status: r
                        .status()
                        .map(|st| st.as_str().to_string())
                        .unwrap_or_default(),
                    reason: r.hook_status_reason().map(|x| x.to_string()),
                    failure_mode: r.failure_mode().map(|m| m.as_str().to_string()),
                });
            }
        }
        out.push(CfnChangeSet {
            name,
            id,
            status: s
                .status()
                .map(|st| st.as_str().to_string())
                .unwrap_or_default(),
            execution_status: s
                .execution_status()
                .map(|e| e.as_str().to_string())
                .unwrap_or_default(),
            status_reason: s.status_reason().map(|r| r.to_string()),
            creation_time: s
                .creation_time()
                .map(|t| t.to_string())
                .unwrap_or_default(),
            description: s.description().map(|d| d.to_string()),
            changes,
            hooks,
            hook_results,
        });
    }
    out
}

/// The change sets as one pretty JSON document — the `e` editor view behind
/// the Changes section. Before/after contexts are embedded as parsed JSON
/// (falling back to the raw string), so the resource diff is directly
/// readable instead of being an escaped blob.
pub fn change_sets_json(sets: &[CfnChangeSet]) -> String {
    use serde_json::{json, Value};
    let parse = |s: &Option<String>| -> Value {
        match s {
            Some(raw) => serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.clone())),
            None => Value::Null,
        }
    };
    let doc: Vec<Value> = sets
        .iter()
        .map(|cs| {
            json!({
                "changeSetName": cs.name,
                "changeSetId": cs.id,
                "status": cs.status,
                "executionStatus": cs.execution_status,
                "statusReason": cs.status_reason,
                "created": cs.creation_time,
                "description": cs.description,
                "changes": cs.changes.iter().map(|ch| json!({
                    "action": ch.action,
                    "logicalResourceId": ch.logical_id,
                    "physicalResourceId": ch.physical_id,
                    "resourceType": ch.resource_type,
                    "replacement": ch.replacement,
                    "policyAction": ch.policy_action,
                    "propertyChanges": ch.details.iter().map(|d| json!({
                        "target": d.label,
                        "attribute": d.attribute,
                        "before": d.before,
                        "after": d.after,
                        "evaluation": d.evaluation,
                        "requiresRecreation": d.requires_recreation,
                        "changeSource": d.change_source,
                        "causingEntity": d.causing_entity,
                    })).collect::<Vec<_>>(),
                    "before": parse(&ch.before_context),
                    "after": parse(&ch.after_context),
                })).collect::<Vec<_>>(),
                "hooks": cs.hooks.iter().map(|h| json!({
                    "typeName": h.type_name,
                    "invocationPoint": h.invocation_point,
                    "failureMode": h.failure_mode,
                    "target": h.target,
                })).collect::<Vec<_>>(),
                "hookResults": cs.hook_results.iter().map(|r| json!({
                    "typeName": r.type_name,
                    "status": r.status,
                    "reason": r.reason,
                    "failureMode": r.failure_mode,
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    serde_json::to_string_pretty(&doc).unwrap_or_default()
}

fn cfn_change_from_sdk(rc: &aws_sdk_cloudformation::types::ResourceChange) -> CfnChange {
    let details = rc
        .details()
        .iter()
        .filter_map(|d| {
            let t = d.target()?;
            // Path is the precise "Properties/MemorySize" form; fall back to
            // the bare name, then the attribute (Tags/Metadata/…).
            let label = t
                .path()
                .map(|p| p.to_string())
                .or_else(|| t.name().map(|n| n.to_string()))
                .or_else(|| t.attribute().map(|a| a.as_str().to_string()))?;
            Some(CfnChangeDetail {
                label,
                attribute: t
                    .attribute()
                    .map(|a| a.as_str().to_string())
                    .unwrap_or_default(),
                before: t.before_value().map(|v| v.to_string()),
                after: t.after_value().map(|v| v.to_string()),
                evaluation: d.evaluation().map(|e| e.as_str().to_string()),
                requires_recreation: t.requires_recreation().map(|r| r.as_str().to_string()),
                change_source: d.change_source().map(|c| c.as_str().to_string()),
                causing_entity: d.causing_entity().map(|c| c.to_string()),
            })
        })
        .collect();
    CfnChange {
        action: rc
            .action()
            .map(|a| a.as_str().to_string())
            .unwrap_or_default(),
        logical_id: rc.logical_resource_id().unwrap_or_default().to_string(),
        resource_type: rc.resource_type().unwrap_or_default().to_string(),
        physical_id: rc
            .physical_resource_id()
            .map(|p| p.to_string())
            .filter(|p| !p.is_empty()),
        replacement: rc.replacement().map(|r| r.as_str().to_string()),
        policy_action: rc.policy_action().map(|p| p.as_str().to_string()),
        details,
        before_context: rc.before_context().map(|c| c.to_string()),
        after_context: rc.after_context().map(|c| c.to_string()),
    }
}

// ── StackSet detail (lazy Config / Instances / Operations sections) ───────────

#[derive(Debug, Clone, Default)]
pub struct CfnStackSetDetailData {
    pub description: Option<String>,
    pub permission_model: Option<String>,
    pub auto_deployment_enabled: Option<bool>,
    pub retain_on_account_removal: Option<bool>,
    pub managed_execution: Option<bool>,
    pub capabilities: Vec<String>,
    pub administration_role_arn: Option<String>,
    pub execution_role_name: Option<String>,
    pub organizational_unit_ids: Vec<String>,
    pub parameters: Vec<(String, String)>,
    pub tags: Vec<(String, String)>,
    pub drift_status: Option<String>,
}

impl CfnStackSetDetailData {
    pub fn from_sdk(ss: &aws_sdk_cloudformation::types::StackSet) -> Self {
        let (auto_deployment_enabled, retain_on_account_removal) = match ss.auto_deployment() {
            Some(ad) => (ad.enabled(), ad.retain_stacks_on_account_removal()),
            None => (None, None),
        };
        let parameters = ss
            .parameters()
            .iter()
            .map(|p| {
                (
                    p.parameter_key().unwrap_or_default().to_string(),
                    p.parameter_value().unwrap_or_default().to_string(),
                )
            })
            .collect();
        let tags = ss
            .tags()
            .iter()
            .filter_map(|t| match (t.key(), t.value()) {
                (Some(k), Some(v)) => Some((k.to_string(), v.to_string())),
                _ => None,
            })
            .collect();
        Self {
            description: ss.description().map(|s| s.to_string()),
            permission_model: ss.permission_model().map(|p| p.as_str().to_string()),
            auto_deployment_enabled,
            retain_on_account_removal,
            managed_execution: ss.managed_execution().and_then(|m| m.active()),
            capabilities: ss.capabilities().iter().map(|c| c.as_str().to_string()).collect(),
            administration_role_arn: ss.administration_role_arn().map(|s| s.to_string()),
            execution_role_name: ss.execution_role_name().map(|s| s.to_string()),
            organizational_unit_ids: ss
                .organizational_unit_ids()
                .iter()
                .map(|s| s.to_string())
                .collect(),
            parameters,
            tags,
            drift_status: ss
                .stack_set_drift_detection_details()
                .and_then(|d| d.drift_status())
                .map(|s| s.as_str().to_string()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CfnStackInstance {
    pub account: String,
    pub region: String,
    pub status: String,
    pub status_reason: Option<String>,
    pub drift_status: Option<String>,
    pub stack_id: Option<String>,
}

impl CfnStackInstance {
    pub fn from_summary(s: &aws_sdk_cloudformation::types::StackInstanceSummary) -> Self {
        Self {
            account: s.account().unwrap_or_default().to_string(),
            region: s.region().unwrap_or_default().to_string(),
            status: s
                .status()
                .map(|st| st.as_str().to_string())
                .unwrap_or_else(|| "UNKNOWN".to_string()),
            status_reason: s.status_reason().map(|r| r.to_string()),
            drift_status: s.drift_status().map(|d| d.as_str().to_string()),
            stack_id: s.stack_id().map(|s| s.to_string()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CfnStackSetOperation {
    pub operation_id: String,
    pub action: String,
    pub status: String,
    pub creation_time: String,
    pub end_time: Option<String>,
    pub status_reason: Option<String>,
}

impl CfnStackSetOperation {
    pub fn from_summary(o: &aws_sdk_cloudformation::types::StackSetOperationSummary) -> Self {
        Self {
            operation_id: o.operation_id().unwrap_or_default().to_string(),
            action: o
                .action()
                .map(|a| a.as_str().to_string())
                .unwrap_or_default(),
            status: o
                .status()
                .map(|s| s.as_str().to_string())
                .unwrap_or_else(|| "UNKNOWN".to_string()),
            creation_time: o
                .creation_timestamp()
                .map(|t| t.to_string())
                .unwrap_or_default(),
            end_time: o.end_timestamp().map(|t| t.to_string()),
            status_reason: o.status_reason().map(|r| r.to_string()),
        }
    }
}

// ── Stack Resource Entry (for lazy-loaded Resources section) ──────────────────

#[derive(Debug, Clone)]
pub struct CfnStackResourceEntry {
    pub logical_id: String,
    pub physical_id: Option<String>,
    pub resource_type: String,
    pub status: String,
    pub status_reason: Option<String>,
}

impl CfnStackResourceEntry {
    pub fn from_sdk(r: &aws_sdk_cloudformation::types::StackResource) -> Self {
        Self {
            logical_id: r.logical_resource_id().unwrap_or_default().to_string(),
            physical_id: r.physical_resource_id().map(|s| s.to_string()),
            resource_type: r.resource_type().unwrap_or_default().to_string(),
            status: r.resource_status().map(|s| s.as_str()).unwrap_or("UNKNOWN").to_string(),
            status_reason: r.resource_status_reason().map(|s| s.to_string()),
        }
    }
}

// ── Output / Parameter helper types ──────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct CfnOutput {
    pub key: String,
    pub value: String,
    pub export_name: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CfnParameter {
    pub key: String,
    pub value: String,
}

// ── CFN Stack ─────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct CfnStack {
    pub stack_id: String,
    pub stack_name: String,
    pub description: Option<String>,
    pub status: String,
    pub status_reason: Option<String>,
    pub creation_time: String,
    pub last_updated: Option<String>,
    pub drift_status: Option<String>,
    pub capabilities: Vec<String>,
    pub outputs: Vec<CfnOutput>,
    pub parameters: Vec<CfnParameter>,
    pub tags: HashMap<String, String>,
    pub parent_id: Option<String>,
    pub root_id: Option<String>,
    pub role_arn: Option<String>,
    pub notification_arns: Vec<String>,
    pub termination_protection: Option<bool>,
    pub disable_rollback: Option<bool>,
    pub timeout_minutes: Option<i32>,
    pub rollback_monitoring_minutes: Option<i32>,
    pub rollback_triggers: Vec<String>,
    /// True for DELETE_COMPLETE stacks surfaced by ListStacks (Deleted
    /// sub-tab). Those rows are built from thin `StackSummary`s — no tags,
    /// outputs, parameters, or capabilities — and get the reduced
    /// `CFN_DELETED_STACK_SECTIONS` pane.
    pub deleted: bool,
    pub deletion_time: Option<String>,
}

impl CfnStack {
    pub fn from_sdk(stack: &aws_sdk_cloudformation::types::Stack) -> Self {
        let mut tags = HashMap::new();
        for tag in stack.tags() {
            if let (Some(k), Some(v)) = (tag.key(), tag.value()) {
                tags.insert(k.to_string(), v.to_string());
            }
        }

        let outputs = stack
            .outputs()
            .iter()
            .map(|o| CfnOutput {
                key: o.output_key().unwrap_or_default().to_string(),
                value: o.output_value().unwrap_or_default().to_string(),
                export_name: o.export_name().map(|s| s.to_string()),
                description: o.description().map(|s| s.to_string()),
            })
            .collect();

        let parameters = stack
            .parameters()
            .iter()
            .map(|p| CfnParameter {
                key: p.parameter_key().unwrap_or_default().to_string(),
                value: p.parameter_value().unwrap_or_default().to_string(),
            })
            .collect();

        let capabilities = stack
            .capabilities()
            .iter()
            .map(|c| c.as_str().to_string())
            .collect();

        let drift_status = stack
            .drift_information()
            .map(|d| d.stack_drift_status().map(|s| s.as_str().to_string()))
            .flatten();

        let notification_arns = stack
            .notification_arns()
            .iter()
            .map(|s| s.to_string())
            .collect();

        let (rollback_monitoring_minutes, rollback_triggers) = match stack.rollback_configuration() {
            Some(rc) => (
                rc.monitoring_time_in_minutes(),
                rc.rollback_triggers()
                    .iter()
                    .map(|t| t.arn().unwrap_or_default().to_string())
                    .collect(),
            ),
            None => (None, Vec::new()),
        };

        Self {
            stack_id: stack.stack_id().unwrap_or_default().to_string(),
            stack_name: stack.stack_name().unwrap_or_default().to_string(),
            description: stack.description().map(|s| s.to_string()),
            status: stack
                .stack_status()
                .map(|s| s.as_str().to_string())
                .unwrap_or_else(|| "UNKNOWN".to_string()),
            status_reason: stack.stack_status_reason().map(|s| s.to_string()),
            creation_time: stack
                .creation_time()
                .map(|t| t.to_string())
                .unwrap_or_default(),
            last_updated: stack.last_updated_time().map(|t| t.to_string()),
            drift_status,
            capabilities,
            outputs,
            parameters,
            tags,
            parent_id: stack.parent_id().map(|s| s.to_string()),
            root_id: stack.root_id().map(|s| s.to_string()),
            role_arn: stack.role_arn().map(|s| s.to_string()),
            notification_arns,
            termination_protection: stack.enable_termination_protection(),
            disable_rollback: stack.disable_rollback(),
            timeout_minutes: stack.timeout_in_minutes(),
            rollback_monitoring_minutes,
            rollback_triggers,
            deleted: false,
            deletion_time: stack.deletion_time().map(|t| t.to_string()),
        }
    }

    /// A deleted stack from a `ListStacks` summary. Every lazy section that
    /// still works post-delete (Resources / Events / Template — ~90 days)
    /// must address the stack by its unique **stack ID**, never the name:
    /// name resolution only works for live stacks, and a delete → recreate →
    /// delete cycle legitimately yields several deleted stacks with the same
    /// name.
    pub fn from_deleted_summary(s: &aws_sdk_cloudformation::types::StackSummary) -> Self {
        Self {
            stack_id: s.stack_id().unwrap_or_default().to_string(),
            stack_name: s.stack_name().unwrap_or_default().to_string(),
            description: s.template_description().map(|d| d.to_string()),
            status: s
                .stack_status()
                .map(|st| st.as_str().to_string())
                .unwrap_or_else(|| "DELETE_COMPLETE".to_string()),
            status_reason: s.stack_status_reason().map(|r| r.to_string()),
            creation_time: s
                .creation_time()
                .map(|t| t.to_string())
                .unwrap_or_default(),
            last_updated: s.last_updated_time().map(|t| t.to_string()),
            drift_status: None,
            capabilities: Vec::new(),
            outputs: Vec::new(),
            parameters: Vec::new(),
            tags: HashMap::new(),
            parent_id: s.parent_id().map(|p| p.to_string()),
            root_id: s.root_id().map(|r| r.to_string()),
            role_arn: None,
            notification_arns: Vec::new(),
            termination_protection: None,
            disable_rollback: None,
            timeout_minutes: None,
            rollback_monitoring_minutes: None,
            rollback_triggers: Vec::new(),
            deleted: true,
            deletion_time: s.deletion_time().map(|t| t.to_string()),
        }
    }
}

crate::sections! {
    pub enum CfnStackDetailSection,
    pub static CFN_STACK_SECTIONS = [
        Overview "Overview" => crate::app::App::hook_cfn_stack_section,
        Resources "Resources" => crate::app::App::hook_cfn_stack_section,
        Outputs "Outputs" => crate::app::App::hook_cfn_stack_section,
        Parameters "Parameters" => crate::app::App::hook_cfn_stack_section,
        Tags "Tags" => crate::app::App::hook_cfn_stack_section,
        Events "Events" => crate::app::App::hook_cfn_stack_section,
        Template "Template" => crate::app::App::hook_cfn_stack_section,
        Drift "Drift" => crate::app::App::hook_cfn_stack_section,
        Changes "Changes" => crate::app::App::hook_cfn_stack_section,
    ]
}

// Deleted stacks get their own reduced descriptor: Outputs/Parameters/Tags
// aren't in the ListStacks summary and are gone from the API, and
// Drift/Changes only error on a deleted stack. Resources/Events/Template
// still work for ~90 days when fetched by stack ID.
crate::sections! {
    pub enum CfnDeletedStackDetailSection,
    pub static CFN_DELETED_STACK_SECTIONS = [
        Overview "Overview",
        Resources "Resources" => crate::app::App::hook_cfn_deleted_stack_section,
        Events "Events" => crate::app::App::hook_cfn_deleted_stack_section,
        Template "Template" => crate::app::App::hook_cfn_deleted_stack_section,
    ]
}

/// Cap on the Events section fetch — DescribeStackEvents retains the full
/// 90-day history and a busy stack has thousands; the recent window is what
/// answers "what went wrong". Events arrive newest-first, so truncation drops
/// the oldest. The renderer keys its "truncated" note on this constant.
pub const CFN_EVENTS_CAP: usize = 500;

impl Resource for CfnStack {
    fn references(&self) -> Vec<(String, String)> {
        let mut v = Vec::new();
        let mut r = |l: &str, x: &str| v.push((l.to_string(), x.to_string()));
        if let Some(x) = &self.role_arn { r("Role", x); }
        if let Some(x) = &self.parent_id { r("Parent Stack", x); }
        for x in &self.notification_arns { r("Notification Topic", x); }
        for out in &self.outputs { r(&format!("Output {}", out.key), &out.value); }
        v
    }

    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        if self.deleted {
            Some(&CFN_DELETED_STACK_SECTIONS)
        } else {
            Some(&CFN_STACK_SECTIONS)
        }
    }
    fn cli_command(&self) -> Option<String> {
        // A deleted stack can only be described by its unique stack ID.
        let target = if self.deleted {
            &self.stack_id
        } else {
            &self.stack_name
        };
        Some(format!(
            "aws cloudformation describe-stacks --stack-name {}",
            crate::aws::resource::shell_quote(target)
        ))
    }

    fn id(&self) -> &str {
        &self.stack_id
    }

    fn name(&self) -> &str {
        &self.stack_name
    }

    fn resource_type(&self) -> &str {
        if self.deleted {
            "CFN Deleted Stack"
        } else {
            "CFN Stack"
        }
    }

    fn state(&self) -> ResourceState {
        match self.status.as_str() {
            "CREATE_COMPLETE" | "UPDATE_COMPLETE" | "IMPORT_COMPLETE" => ResourceState::Available,
            "CREATE_IN_PROGRESS" | "UPDATE_IN_PROGRESS" | "REVIEW_IN_PROGRESS"
            | "IMPORT_IN_PROGRESS" => ResourceState::Creating,
            "DELETE_IN_PROGRESS" => ResourceState::Deleting,
            "ROLLBACK_IN_PROGRESS"
            | "UPDATE_ROLLBACK_IN_PROGRESS"
            | "UPDATE_ROLLBACK_COMPLETE_CLEANUP_IN_PROGRESS"
            | "CREATE_COMPLETE_CLEANUP_IN_PROGRESS"
            | "UPDATE_COMPLETE_CLEANUP_IN_PROGRESS" => ResourceState::Pending,
            "ROLLBACK_COMPLETE"
            | "UPDATE_ROLLBACK_COMPLETE"
            | "IMPORT_ROLLBACK_COMPLETE"
            | "DELETE_COMPLETE" => ResourceState::Stopped,
            "CREATE_FAILED"
            | "ROLLBACK_FAILED"
            | "DELETE_FAILED"
            | "UPDATE_ROLLBACK_FAILED"
            | "IMPORT_ROLLBACK_FAILED" => ResourceState::Unavailable,
            _ => ResourceState::Unknown(self.status.clone()),
        }
    }

    fn state_label(&self) -> String {
        native_state_label(&self.status, || self.state())
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        let mut parts = vec![
            self.stack_id.clone(),
            self.stack_name.clone(),
            self.status.clone(),
        ];
        if let Some(desc) = &self.description {
            parts.push(desc.clone());
        }
        for (k, v) in &self.tags {
            parts.push(format!("{}:{}", k, v));
        }
        parts.join(" ")
    }

    fn details(&self) -> Vec<(String, String)> {
        let mut d = vec![
            ("Stack Name".to_string(), self.stack_name.clone()),
            ("Stack ID".to_string(), self.stack_id.clone()),
            ("Status".to_string(), self.status.clone()),
            ("Created".to_string(), self.creation_time.clone()),
        ];
        if let Some(updated) = &self.last_updated {
            d.push(("Last Updated".to_string(), updated.clone()));
        }
        if let Some(deleted) = &self.deletion_time {
            d.push(("Deleted".to_string(), deleted.clone()));
        }
        if let Some(desc) = &self.description {
            d.push(("Description".to_string(), desc.clone()));
        }
        if let Some(drift) = &self.drift_status {
            d.push(("Drift Status".to_string(), drift.clone()));
        }
        if !self.capabilities.is_empty() {
            d.push(("Capabilities".to_string(), self.capabilities.join(", ")));
        }
        d
    }

    fn console_url(&self, region: &str) -> Option<String> {
        // The console only resolves a deleted stack's ID with the Deleted
        // status filter in the URL.
        let filter = if self.deleted {
            "&filteringStatus=deleted"
        } else {
            ""
        };
        Some(format!(
            "https://{region}.console.aws.amazon.com/cloudformation/home?region={region}#/stacks/stackinfo?stackId={}{filter}",
            self.stack_id
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── CFN StackSet ──────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct CfnStackSet {
    pub stack_set_id: String,
    pub stack_set_name: String,
    pub description: Option<String>,
    pub status: String,
    pub permission_model: Option<String>,
    pub tags: HashMap<String, String>,
}

impl CfnStackSet {
    pub fn from_summary(ss: &aws_sdk_cloudformation::types::StackSetSummary) -> Self {
        Self {
            stack_set_id: ss.stack_set_id().unwrap_or_default().to_string(),
            stack_set_name: ss.stack_set_name().unwrap_or_default().to_string(),
            description: ss.description().map(|s| s.to_string()),
            status: ss
                .status()
                .map(|s| s.as_str().to_string())
                .unwrap_or_else(|| "UNKNOWN".to_string()),
            permission_model: ss
                .permission_model()
                .map(|p| p.as_str().to_string()),
            tags: HashMap::new(),
        }
    }
}

crate::sections! {
    pub enum CfnStackSetDetailSection,
    pub static CFN_STACKSET_SECTIONS = [
        Config "Config" => crate::app::App::hook_cfn_stackset_section,
        Instances "Instances" => crate::app::App::hook_cfn_stackset_section,
        Operations "Operations" => crate::app::App::hook_cfn_stackset_section,
        Tags "Tags" => crate::app::App::hook_cfn_stackset_section,
    ]
}

impl Resource for CfnStackSet {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&CFN_STACKSET_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.stack_set_id
    }

    fn name(&self) -> &str {
        &self.stack_set_name
    }

    fn resource_type(&self) -> &str {
        "CFN StackSet"
    }

    fn state(&self) -> ResourceState {
        match self.status.as_str() {
            "ACTIVE" => ResourceState::Available,
            "DELETED" => ResourceState::Stopped,
            _ => ResourceState::Unknown(self.status.clone()),
        }
    }

    fn state_label(&self) -> String {
        native_state_label(&self.status, || self.state())
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        let mut parts = vec![
            self.stack_set_id.clone(),
            self.stack_set_name.clone(),
            self.status.clone(),
        ];
        if let Some(desc) = &self.description {
            parts.push(desc.clone());
        }
        parts.join(" ")
    }

    fn details(&self) -> Vec<(String, String)> {
        let mut d = vec![
            ("Stack Set Name".to_string(), self.stack_set_name.clone()),
            ("Stack Set ID".to_string(), self.stack_set_id.clone()),
            ("Status".to_string(), self.status.clone()),
        ];
        if let Some(desc) = &self.description {
            d.push(("Description".to_string(), desc.clone()));
        }
        if let Some(model) = &self.permission_model {
            d.push(("Permission Model".to_string(), model.clone()));
        }
        d
    }

    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{region}.console.aws.amazon.com/cloudformation/home?region={region}#/stacksets/{}/stacks",
            self.stack_set_name
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(logical: &str, ty: &str, status: &str, reason: Option<&str>, ts: i64) -> CfnStackEvent {
        CfnStackEvent {
            logical_id: logical.to_string(),
            resource_type: ty.to_string(),
            timestamp: String::new(),
            ts_secs: Some(ts),
            status: status.to_string(),
            status_reason: reason.map(|r| r.to_string()),
        }
    }

    fn stack_ev(status: &str, reason: Option<&str>, ts: i64) -> CfnStackEvent {
        ev("my-stack", "AWS::CloudFormation::Stack", status, reason, ts)
    }

    #[test]
    fn rollup_bounds_window_at_user_initiated_and_dedupes_latest_status() {
        // Newest-first: an in-flight create on top of a previous completed
        // update — the update's events must not leak into the rollup.
        let events = vec![
            ev("TableB", "AWS::DynamoDB::Table", "CREATE_IN_PROGRESS", None, 200),
            ev("TableA", "AWS::DynamoDB::Table", "CREATE_COMPLETE", None, 190),
            ev("TableA", "AWS::DynamoDB::Table", "CREATE_IN_PROGRESS", None, 150),
            stack_ev("CREATE_IN_PROGRESS", Some("User Initiated"), 100),
            stack_ev("UPDATE_COMPLETE", None, 50),
            ev("OldRes", "AWS::SNS::Topic", "UPDATE_COMPLETE", None, 40),
        ];
        let r = cfn_progress_rollup("my-stack", &events);
        assert_eq!(r.op_status.as_deref(), Some("CREATE_IN_PROGRESS"));
        assert!(r.in_progress());
        assert_eq!(r.op_start_secs, Some(100));
        assert!(!r.window_truncated);
        // TableA's latest status wins; OldRes (previous op) excluded.
        assert_eq!(r.resources.len(), 2);
        assert_eq!(r.resources[0].logical_id, "TableB");
        assert_eq!(r.resources[1].logical_id, "TableA");
        assert_eq!(r.resources[1].status, "CREATE_COMPLETE");
    }

    #[test]
    fn rollup_settled_operation_stops_at_previous_ops_end() {
        // Newest-first: a finished create — the newest event is the stack's
        // own terminal event and must set op_status, not end the window.
        let events = vec![
            stack_ev("CREATE_COMPLETE", None, 300),
            ev("TableA", "AWS::DynamoDB::Table", "CREATE_COMPLETE", None, 250),
            stack_ev("CREATE_IN_PROGRESS", Some("User Initiated"), 100),
        ];
        let r = cfn_progress_rollup("my-stack", &events);
        assert_eq!(r.op_status.as_deref(), Some("CREATE_COMPLETE"));
        assert!(!r.in_progress());
        assert!(!r.window_truncated);
        assert_eq!(r.resources.len(), 1);
    }

    #[test]
    fn rollup_rollback_chain_stays_in_one_window() {
        // A failed create rolling back: ROLLBACK_IN_PROGRESS has no "User
        // Initiated" reason, so the walk continues to the create's start.
        let events = vec![
            stack_ev("ROLLBACK_IN_PROGRESS", Some("The following resource(s) failed to create"), 400),
            ev("Vpc", "AWS::EC2::VPC", "CREATE_FAILED", Some("boom"), 350),
            stack_ev("CREATE_IN_PROGRESS", Some("User Initiated"), 100),
        ];
        let r = cfn_progress_rollup("my-stack", &events);
        assert_eq!(r.op_status.as_deref(), Some("ROLLBACK_IN_PROGRESS"));
        assert_eq!(r.resources.len(), 1);
        assert_eq!(r.resources[0].status, "CREATE_FAILED");
        assert!(!r.window_truncated);
    }

    #[test]
    fn rollup_flags_truncated_window() {
        // No stack-level start marker in the fetched window (cap truncation).
        let events = vec![ev("TableA", "AWS::DynamoDB::Table", "CREATE_COMPLETE", None, 10)];
        let r = cfn_progress_rollup("my-stack", &events);
        assert!(r.window_truncated);
        assert_eq!(r.op_status, None);
        assert_eq!(r.resources.len(), 1);
    }

    fn tr(id: &str, ty: &str, conditional: bool) -> CfnTemplateResource {
        CfnTemplateResource {
            logical_id: id.to_string(),
            resource_type: ty.to_string(),
            conditional,
        }
    }

    #[test]
    fn template_ids_from_json_with_conditions() {
        let body = r#"{"AWSTemplateFormatVersion":"2010-09-09","Resources":{
            "Vpc":{"Type":"AWS::EC2::VPC","Properties":{}},
            "AdminPolicy":{"Type":"AWS::IAM::Policy","Condition":"IsProd"},
            "Table":{"Type":"AWS::DynamoDB::Table"}}}"#;
        let ids = parse_template_resource_ids(body).unwrap();
        assert_eq!(ids.len(), 3);
        assert!(ids.contains(&tr("Vpc", "AWS::EC2::VPC", false)));
        assert!(ids.contains(&tr("AdminPolicy", "AWS::IAM::Policy", true)));
        assert!(ids.contains(&tr("Table", "AWS::DynamoDB::Table", false)));
    }

    #[test]
    fn template_ids_from_yaml_ignores_nested_type_keys() {
        let body = "AWSTemplateFormatVersion: '2010-09-09'\n\
                    # a comment\n\
                    Parameters:\n\
                    \x20 Env:\n\
                    \x20   Type: String\n\
                    Resources:\n\
                    \x20 Listener:\n\
                    \x20   Type: AWS::ElasticLoadBalancingV2::Listener\n\
                    \x20   Properties:\n\
                    \x20     DefaultActions:\n\
                    \x20       - Type: forward\n\
                    \x20 AdminPolicy:\n\
                    \x20   Condition: IsProd\n\
                    \x20   Type: AWS::IAM::Policy\n\
                    \x20 Table:\n\
                    \x20   Type: \"AWS::DynamoDB::Table\"\n\
                    Outputs:\n\
                    \x20 TableName:\n\
                    \x20   Value: !Ref Table\n";
        let ids = parse_template_resource_ids(body).unwrap();
        assert_eq!(
            ids,
            vec![
                tr("Listener", "AWS::ElasticLoadBalancingV2::Listener", false),
                tr("AdminPolicy", "AWS::IAM::Policy", true),
                tr("Table", "AWS::DynamoDB::Table", false),
            ]
        );
    }

    #[test]
    fn template_ids_unparseable_is_none() {
        assert_eq!(parse_template_resource_ids("not a template"), None);
        assert_eq!(parse_template_resource_ids("{\"Resources\": 4}"), None);
    }

    #[test]
    fn change_sets_json_embeds_contexts_as_parsed_json() {
        let cs = CfnChangeSet {
            name: "cs-1".to_string(),
            id: "arn:aws:cloudformation:...:changeSet/cs-1".to_string(),
            status: "CREATE_COMPLETE".to_string(),
            execution_status: "AVAILABLE".to_string(),
            status_reason: None,
            creation_time: String::new(),
            description: None,
            changes: vec![CfnChange {
                action: "Modify".to_string(),
                logical_id: "Fn".to_string(),
                resource_type: "AWS::Lambda::Function".to_string(),
                physical_id: Some("my-fn".to_string()),
                replacement: Some("False".to_string()),
                policy_action: None,
                details: vec![],
                before_context: Some(r#"{"Properties":{"MemorySize":128}}"#.to_string()),
                after_context: Some("not-json".to_string()),
            }],
            hooks: vec![],
            hook_results: vec![],
        };
        let doc = change_sets_json(&[cs]);
        let parsed: serde_json::Value = serde_json::from_str(&doc).unwrap();
        // Valid context embeds as structured JSON, not an escaped string…
        assert_eq!(
            parsed[0]["changes"][0]["before"]["Properties"]["MemorySize"],
            serde_json::json!(128)
        );
        // …and an unparseable one falls back to the raw string.
        assert_eq!(
            parsed[0]["changes"][0]["after"],
            serde_json::json!("not-json")
        );
    }
}
