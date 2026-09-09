use std::any::Any;
use std::collections::HashMap;

use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::aws::client::AwsClients;
use crate::aws::resource::{native_state_label, Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::error::Result;
use crate::event::{Event, LoadProgress};

type BedrockClient = aws_sdk_bedrock::Client;
type BedrockAgentClient = aws_sdk_bedrockagent::Client;

/// Trim an `aws_smithy_types::DateTime` to a readable `YYYY-MM-DD HH:MM:SS`.
fn fmt_dt(dt: Option<&aws_smithy_types::DateTime>) -> String {
    dt.map(|t| {
        let s = t.to_string();
        s.split('.').next().unwrap_or(&s).replace('T', " ").replace('Z', "")
    })
    .unwrap_or_default()
}

// ═══════════════════════════════════════════════════════════════════════════════
// Service
// ═══════════════════════════════════════════════════════════════════════════════

pub struct BedrockService {
    client: BedrockClient,
    agent_client: BedrockAgentClient,
}

impl BedrockService {
    pub fn new(clients: &AwsClients) -> Self {
        Self {
            client: clients.bedrock_client(),
            agent_client: clients.bedrockagent_client(),
        }
    }
}

#[async_trait]
impl AwsService for BedrockService {
    fn service_type(&self) -> ServiceType {
        ServiceType::Bedrock
    }

    fn name(&self) -> &str {
        "Bedrock"
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

        let emit = |batch: Vec<Box<dyn Resource>>, total: &mut usize| {
            if batch.is_empty() {
                return;
            }
            *total += batch.len();
            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                service: service_type,
                resources: batch,
                progress: LoadProgress {
                    loaded_count: *total,
                    total_count: None,
                    status_message: None,
                },
            });
        };

        // ── Foundation Models (single call, no paginator) ────────────────────
        match self.client.list_foundation_models().send().await {
            Ok(resp) => {
                let batch: Vec<Box<dyn Resource>> = resp
                    .model_summaries()
                    .iter()
                    .map(|m| Box::new(BedrockModel::from_sdk(m)) as Box<dyn Resource>)
                    .collect();
                emit(batch, &mut total);
            }
            Err(e) => errors.push(format!("Foundation Models: {}", crate::error::sdk_error_message(&e))),
        }

        // ── Inference Profiles ───────────────────────────────────────────────
        let mut paginator = self.client.list_inference_profiles().into_paginator().send();
        loop {
            match paginator.next().await {
                Some(Ok(page)) => {
                    let batch: Vec<Box<dyn Resource>> = page
                        .inference_profile_summaries()
                        .iter()
                        .map(|p| Box::new(BedrockInferenceProfile::from_sdk(p)) as Box<dyn Resource>)
                        .collect();
                    emit(batch, &mut total);
                }
                Some(Err(e)) => {
                    errors.push(format!(
                        "Inference Profiles: {}",
                        crate::error::sdk_error_message(&e)
                    ));
                    break;
                }
                None => break,
            }
        }

        // ── Guardrails ───────────────────────────────────────────────────────
        let mut paginator = self.client.list_guardrails().into_paginator().send();
        loop {
            match paginator.next().await {
                Some(Ok(page)) => {
                    let batch: Vec<Box<dyn Resource>> = page
                        .guardrails()
                        .iter()
                        .map(|g| Box::new(BedrockGuardrail::from_sdk(g)) as Box<dyn Resource>)
                        .collect();
                    emit(batch, &mut total);
                }
                Some(Err(e)) => {
                    errors.push(format!("Guardrails: {}", crate::error::sdk_error_message(&e)));
                    break;
                }
                None => break,
            }
        }

        // ── Knowledge Bases (bedrock-agent control plane) ────────────────────
        let mut paginator = self.agent_client.list_knowledge_bases().into_paginator().send();
        loop {
            match paginator.next().await {
                Some(Ok(page)) => {
                    let batch: Vec<Box<dyn Resource>> = page
                        .knowledge_base_summaries()
                        .iter()
                        .map(|k| Box::new(BedrockKnowledgeBase::from_sdk(k)) as Box<dyn Resource>)
                        .collect();
                    emit(batch, &mut total);
                }
                Some(Err(e)) => {
                    errors.push(format!("Knowledge Bases: {}", crate::error::sdk_error_message(&e)));
                    break;
                }
                None => break,
            }
        }

        // ── Agents (bedrock-agent control plane) ─────────────────────────────
        let mut paginator = self.agent_client.list_agents().into_paginator().send();
        loop {
            match paginator.next().await {
                Some(Ok(page)) => {
                    let batch: Vec<Box<dyn Resource>> = page
                        .agent_summaries()
                        .iter()
                        .map(|a| Box::new(BedrockAgent::from_sdk(a)) as Box<dyn Resource>)
                        .collect();
                    emit(batch, &mut total);
                }
                Some(Err(e)) => {
                    errors.push(format!("Agents: {}", crate::error::sdk_error_message(&e)));
                    break;
                }
                None => break,
            }
        }

        // ── Prompts (bedrock-agent control plane) ────────────────────────────
        let mut paginator = self.agent_client.list_prompts().into_paginator().send();
        loop {
            match paginator.next().await {
                Some(Ok(page)) => {
                    let batch: Vec<Box<dyn Resource>> = page
                        .prompt_summaries()
                        .iter()
                        .map(|p| Box::new(BedrockPrompt::from_sdk(p)) as Box<dyn Resource>)
                        .collect();
                    emit(batch, &mut total);
                }
                Some(Err(e)) => {
                    errors.push(format!("Prompts: {}", crate::error::sdk_error_message(&e)));
                    break;
                }
                None => break,
            }
        }

        // ── Flows (bedrock-agent control plane) ──────────────────────────────
        let mut paginator = self.agent_client.list_flows().into_paginator().send();
        loop {
            match paginator.next().await {
                Some(Ok(page)) => {
                    let batch: Vec<Box<dyn Resource>> = page
                        .flow_summaries()
                        .iter()
                        .map(|f| Box::new(BedrockFlow::from_sdk(f)) as Box<dyn Resource>)
                        .collect();
                    emit(batch, &mut total);
                }
                Some(Err(e)) => {
                    errors.push(format!("Flows: {}", crate::error::sdk_error_message(&e)));
                    break;
                }
                None => break,
            }
        }

        // ── Custom Models (self-owned) ───────────────────────────────────────
        let mut paginator = self.client.list_custom_models().into_paginator().send();
        loop {
            match paginator.next().await {
                Some(Ok(page)) => {
                    let batch: Vec<Box<dyn Resource>> = page
                        .model_summaries()
                        .iter()
                        .map(|m| Box::new(BedrockCustomModel::from_sdk(m)) as Box<dyn Resource>)
                        .collect();
                    emit(batch, &mut total);
                }
                Some(Err(e)) => {
                    errors.push(format!("Custom Models: {}", crate::error::sdk_error_message(&e)));
                    break;
                }
                None => break,
            }
        }

        // ── Imported Models ──────────────────────────────────────────────────
        let mut paginator = self.client.list_imported_models().into_paginator().send();
        loop {
            match paginator.next().await {
                Some(Ok(page)) => {
                    let batch: Vec<Box<dyn Resource>> = page
                        .model_summaries()
                        .iter()
                        .map(|m| Box::new(BedrockImportedModel::from_sdk(m)) as Box<dyn Resource>)
                        .collect();
                    emit(batch, &mut total);
                }
                Some(Err(e)) => {
                    errors.push(format!("Imported Models: {}", crate::error::sdk_error_message(&e)));
                    break;
                }
                None => break,
            }
        }

        // Surface a consolidated error if any sub-API failed (the others still
        // streamed their partial batches).
        if !errors.is_empty() {
            let _ = event_tx.send(Event::ResourceLoadError {
                service: service_type,
                error: errors.join("; "),
            });
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
// Foundation Model
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct BedrockModel {
    pub model_id: String,
    pub model_arn: String,
    pub model_name: String,
    pub provider_name: String,
    pub input_modalities: Vec<String>,
    pub output_modalities: Vec<String>,
    pub inference_types: Vec<String>,
    pub customizations: Vec<String>,
    pub streaming_supported: bool,
    pub lifecycle: String,
    tags: HashMap<String, String>,
}

impl BedrockModel {
    pub fn from_sdk(m: &aws_sdk_bedrock::types::FoundationModelSummary) -> Self {
        Self {
            model_id: m.model_id().to_string(),
            model_arn: m.model_arn().to_string(),
            model_name: m.model_name().unwrap_or("").to_string(),
            provider_name: m.provider_name().unwrap_or("").to_string(),
            input_modalities: m.input_modalities().iter().map(|x| x.as_str().to_string()).collect(),
            output_modalities: m.output_modalities().iter().map(|x| x.as_str().to_string()).collect(),
            inference_types: m
                .inference_types_supported()
                .iter()
                .map(|x| x.as_str().to_string())
                .collect(),
            customizations: m
                .customizations_supported()
                .iter()
                .map(|x| x.as_str().to_string())
                .collect(),
            streaming_supported: m.response_streaming_supported().unwrap_or(false),
            lifecycle: m
                .model_lifecycle()
                .map(|l| l.status().as_str().to_string())
                .unwrap_or_default(),
            tags: HashMap::new(),
        }
    }

    fn is_legacy(&self) -> bool {
        self.lifecycle.eq_ignore_ascii_case("LEGACY")
    }
}

impl Resource for BedrockModel {
    fn id(&self) -> &str {
        &self.model_id
    }
    fn name(&self) -> &str {
        if self.model_name.is_empty() {
            &self.model_id
        } else {
            &self.model_name
        }
    }
    fn resource_type(&self) -> &str {
        "Bedrock Foundation Model"
    }
    fn state(&self) -> ResourceState {
        if self.is_legacy() {
            ResourceState::Unknown("legacy".to_string())
        } else {
            ResourceState::Available
        }
    }

    fn state_label(&self) -> String {
        native_state_label(&self.lifecycle, || self.state())
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        format!("{} {} {}", self.model_id, self.model_name, self.provider_name)
    }
    fn is_noise(&self) -> bool {
        // Legacy models are low-signal — hide unless `a` is toggled.
        self.is_legacy()
    }
    fn details(&self) -> Vec<(String, String)> {
        let mut d = vec![
            ("Model ID".to_string(), self.model_id.clone()),
            ("Model Name".to_string(), self.model_name.clone()),
            ("Provider".to_string(), self.provider_name.clone()),
            ("ARN".to_string(), self.model_arn.clone()),
            (
                "Lifecycle".to_string(),
                if self.lifecycle.is_empty() { "-".to_string() } else { self.lifecycle.clone() },
            ),
            ("Input".to_string(), join_or_dash(&self.input_modalities)),
            ("Output".to_string(), join_or_dash(&self.output_modalities)),
            ("Inference Types".to_string(), join_or_dash(&self.inference_types)),
            ("Customizations".to_string(), join_or_dash(&self.customizations)),
            (
                "Streaming".to_string(),
                if self.streaming_supported { "Yes".to_string() } else { "No".to_string() },
            ),
        ];
        d.retain(|(_, v)| !v.is_empty());
        d
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{r}.console.aws.amazon.com/bedrock/home?region={r}#/foundation-models",
            r = region
        ))
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Inference Profile
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct BedrockInferenceProfile {
    pub profile_id: String,
    pub profile_arn: String,
    pub profile_name: String,
    pub description: String,
    pub status: String,
    pub profile_type: String,
    pub model_arns: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
    tags: HashMap<String, String>,
}

impl BedrockInferenceProfile {
    pub fn from_sdk(p: &aws_sdk_bedrock::types::InferenceProfileSummary) -> Self {
        Self {
            profile_id: p.inference_profile_id().to_string(),
            profile_arn: p.inference_profile_arn().to_string(),
            profile_name: p.inference_profile_name().to_string(),
            description: p.description().unwrap_or("").to_string(),
            status: p.status().as_str().to_string(),
            profile_type: p.r#type().as_str().to_string(),
            model_arns: p
                .models()
                .iter()
                .filter_map(|m| m.model_arn().map(|s| s.to_string()))
                .collect(),
            created_at: fmt_dt(p.created_at()),
            updated_at: fmt_dt(p.updated_at()),
            tags: HashMap::new(),
        }
    }
}

impl Resource for BedrockInferenceProfile {
    fn id(&self) -> &str {
        &self.profile_id
    }
    fn name(&self) -> &str {
        &self.profile_name
    }
    fn resource_type(&self) -> &str {
        "Bedrock Inference Profile"
    }
    fn state(&self) -> ResourceState {
        match self.status.to_uppercase().as_str() {
            "ACTIVE" => ResourceState::Available,
            other => ResourceState::Unknown(other.to_lowercase()),
        }
    }

    fn state_label(&self) -> String {
        native_state_label(&self.status, || self.state())
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        format!("{} {} {}", self.profile_id, self.profile_name, self.profile_type)
    }
    fn details(&self) -> Vec<(String, String)> {
        let mut d = vec![
            ("Name".to_string(), self.profile_name.clone()),
            ("Profile ID".to_string(), self.profile_id.clone()),
            ("ARN".to_string(), self.profile_arn.clone()),
            ("Type".to_string(), self.profile_type.clone()),
            ("Status".to_string(), self.status.clone()),
            ("Description".to_string(), self.description.clone()),
            ("Models".to_string(), self.model_arns.len().to_string()),
            ("Created".to_string(), self.created_at.clone()),
            ("Updated".to_string(), self.updated_at.clone()),
        ];
        for (i, arn) in self.model_arns.iter().enumerate() {
            d.push((format!("  Model {}", i + 1), arn.clone()));
        }
        d.retain(|(_, v)| !v.is_empty());
        d
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{r}.console.aws.amazon.com/bedrock/home?region={r}#/inference-profiles",
            r = region
        ))
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Guardrail
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct BedrockGuardrail {
    pub guardrail_id: String,
    pub guardrail_arn: String,
    pub guardrail_name: String,
    pub description: String,
    pub status: String,
    pub version: String,
    pub created_at: String,
    pub updated_at: String,
    tags: HashMap<String, String>,
}

impl BedrockGuardrail {
    pub fn from_sdk(g: &aws_sdk_bedrock::types::GuardrailSummary) -> Self {
        Self {
            guardrail_id: g.id().to_string(),
            guardrail_arn: g.arn().to_string(),
            guardrail_name: g.name().to_string(),
            description: g.description().unwrap_or("").to_string(),
            status: g.status().as_str().to_string(),
            version: g.version().to_string(),
            created_at: fmt_dt(Some(g.created_at())),
            updated_at: fmt_dt(Some(g.updated_at())),
            tags: HashMap::new(),
        }
    }
}

crate::sections! {
    pub enum BedrockGuardrailDetailSection,
    pub static BEDROCK_GUARDRAIL_SECTIONS = [
        Overview "Overview" => crate::app::App::trigger_guardrail_detail_load,
        ContentFilters "Content" => crate::app::App::trigger_guardrail_detail_load,
        DeniedTopics "Topics" => crate::app::App::trigger_guardrail_detail_load,
        WordFilters "Words" => crate::app::App::trigger_guardrail_detail_load,
        SensitiveInfo "Sensitive" => crate::app::App::trigger_guardrail_detail_load,
        Grounding "Grounding" => crate::app::App::trigger_guardrail_detail_load,
        Advanced "Advanced" => crate::app::App::trigger_guardrail_detail_load,
    ]
}

impl Resource for BedrockGuardrail {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&BEDROCK_GUARDRAIL_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.guardrail_id
    }
    fn name(&self) -> &str {
        &self.guardrail_name
    }
    fn resource_type(&self) -> &str {
        "Bedrock Guardrail"
    }
    fn state(&self) -> ResourceState {
        match self.status.to_uppercase().as_str() {
            "READY" => ResourceState::Available,
            "FAILED" => ResourceState::Unavailable,
            "CREATING" => ResourceState::Creating,
            "DELETING" => ResourceState::Deleting,
            "UPDATING" => ResourceState::Pending,
            other => ResourceState::Unknown(other.to_lowercase()),
        }
    }

    fn state_label(&self) -> String {
        native_state_label(&self.status, || self.state())
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        format!("{} {}", self.guardrail_id, self.guardrail_name)
    }
    fn details(&self) -> Vec<(String, String)> {
        let mut d = vec![
            ("Name".to_string(), self.guardrail_name.clone()),
            ("Guardrail ID".to_string(), self.guardrail_id.clone()),
            ("ARN".to_string(), self.guardrail_arn.clone()),
            ("Status".to_string(), self.status.clone()),
            ("Version".to_string(), self.version.clone()),
            ("Description".to_string(), self.description.clone()),
            ("Created".to_string(), self.created_at.clone()),
            ("Updated".to_string(), self.updated_at.clone()),
        ];
        d.retain(|(_, v)| !v.is_empty());
        d
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{r}.console.aws.amazon.com/bedrock/home?region={r}#/guardrails",
            r = region
        ))
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Knowledge Base
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct BedrockKnowledgeBase {
    pub kb_id: String,
    pub kb_name: String,
    pub description: String,
    pub status: String,
    pub updated_at: String,
    tags: HashMap<String, String>,
}

impl BedrockKnowledgeBase {
    pub fn from_sdk(k: &aws_sdk_bedrockagent::types::KnowledgeBaseSummary) -> Self {
        Self {
            kb_id: k.knowledge_base_id().to_string(),
            kb_name: k.name().to_string(),
            description: k.description().unwrap_or("").to_string(),
            status: k.status().as_str().to_string(),
            updated_at: fmt_dt(Some(k.updated_at())),
            tags: HashMap::new(),
        }
    }
}

crate::sections! {
    pub enum BedrockKbDetailSection,
    pub static BEDROCK_KB_SECTIONS = [
        Overview "Overview" => crate::app::App::trigger_kb_detail_load,
        Configuration "Config" => crate::app::App::trigger_kb_detail_load,
        VectorStore "Vector Store" => crate::app::App::trigger_kb_detail_load,
        DataSources "Data Sources" => crate::app::App::trigger_kb_data_sources_load,
        Ingestion "Ingestion" => crate::app::App::trigger_kb_ingestion_load,
    ]
}

impl Resource for BedrockKnowledgeBase {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&BEDROCK_KB_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.kb_id
    }
    fn name(&self) -> &str {
        &self.kb_name
    }
    fn resource_type(&self) -> &str {
        "Bedrock Knowledge Base"
    }
    fn state(&self) -> ResourceState {
        match self.status.to_uppercase().as_str() {
            "ACTIVE" => ResourceState::Available,
            "FAILED" => ResourceState::Unavailable,
            "CREATING" => ResourceState::Creating,
            "DELETING" => ResourceState::Deleting,
            "UPDATING" => ResourceState::Pending,
            other => ResourceState::Unknown(other.to_lowercase()),
        }
    }

    fn state_label(&self) -> String {
        native_state_label(&self.status, || self.state())
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        format!("{} {}", self.kb_id, self.kb_name)
    }
    fn details(&self) -> Vec<(String, String)> {
        let mut d = vec![
            ("Name".to_string(), self.kb_name.clone()),
            ("Knowledge Base ID".to_string(), self.kb_id.clone()),
            ("Status".to_string(), self.status.clone()),
            ("Description".to_string(), self.description.clone()),
            ("Updated".to_string(), self.updated_at.clone()),
        ];
        d.retain(|(_, v)| !v.is_empty());
        d
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{r}.console.aws.amazon.com/bedrock/home?region={r}#/knowledge-bases/{id}",
            r = region,
            id = self.kb_id
        ))
    }
}

fn join_or_dash(v: &[String]) -> String {
    if v.is_empty() {
        String::new()
    } else {
        v.join(", ")
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Guardrail — full policy detail (lazy `GetGuardrail`)
// ═══════════════════════════════════════════════════════════════════════════════
//
// `ListGuardrails` returns only a thin summary (id/name/status/version). All the
// actual policy config — content filters, denied topics, word/PII/regex filters,
// contextual grounding, cross-region, KMS, blocked-messaging — lives behind
// `GetGuardrail`, fetched lazily on first focus and cached per guardrail id.

/// A single content filter (HATE / INSULTS / SEXUAL / VIOLENCE / MISCONDUCT /
/// PROMPT_ATTACK) with its per-direction strength + action.
#[derive(Debug, Clone)]
pub struct GContentFilter {
    pub filter_type: String,
    pub input_strength: String,
    pub output_strength: String,
    pub input_action: String,
    pub output_action: String,
    pub input_enabled: bool,
    pub output_enabled: bool,
    pub input_modalities: Vec<String>,
    pub output_modalities: Vec<String>,
}

/// A denied-topic definition.
#[derive(Debug, Clone)]
pub struct GTopic {
    pub name: String,
    pub definition: String,
    pub examples: Vec<String>,
    pub input_action: String,
    pub output_action: String,
}

/// A custom blocked word.
#[derive(Debug, Clone)]
pub struct GWord {
    pub text: String,
}

/// An AWS-managed word list (e.g. PROFANITY) — a "system policy".
#[derive(Debug, Clone)]
pub struct GManagedWords {
    pub word_type: String,
    pub input_action: String,
    pub output_action: String,
}

/// A PII entity filter (managed sensitive-info type).
#[derive(Debug, Clone)]
pub struct GPiiEntity {
    pub entity_type: String,
    pub action: String,
}

/// A custom regex sensitive-info filter.
#[derive(Debug, Clone)]
pub struct GRegex {
    pub name: String,
    pub description: String,
    pub pattern: String,
    pub action: String,
}

/// A contextual-grounding filter (GROUNDING / RELEVANCE).
#[derive(Debug, Clone)]
pub struct GGroundingFilter {
    pub filter_type: String,
    pub threshold: f64,
    pub action: String,
    pub enabled: bool,
}

/// The fully-resolved guardrail — everything `GetGuardrail` exposes.
#[derive(Debug, Clone)]
pub struct GuardrailFull {
    pub id: String,
    pub arn: String,
    pub name: String,
    pub description: String,
    pub status: String,
    pub version: String,
    pub kms_key_arn: String,
    pub blocked_input_messaging: String,
    pub blocked_outputs_messaging: String,
    pub status_reasons: Vec<String>,
    pub failure_recommendations: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
    pub cross_region_profile: String,
    pub content_filters: Vec<GContentFilter>,
    pub content_tier: String,
    pub topics: Vec<GTopic>,
    pub topic_tier: String,
    pub words: Vec<GWord>,
    pub managed_word_lists: Vec<GManagedWords>,
    pub pii_entities: Vec<GPiiEntity>,
    pub regexes: Vec<GRegex>,
    pub grounding_filters: Vec<GGroundingFilter>,
    pub automated_reasoning_policies: Vec<String>,
}

impl GuardrailFull {
    fn from_sdk(g: &aws_sdk_bedrock::operation::get_guardrail::GetGuardrailOutput) -> Self {
        let content_filters = g
            .content_policy()
            .map(|p| {
                p.filters()
                    .iter()
                    .map(|f| GContentFilter {
                        filter_type: f.r#type().as_str().to_string(),
                        input_strength: f.input_strength().as_str().to_string(),
                        output_strength: f.output_strength().as_str().to_string(),
                        input_action: opt_action(f.input_action().map(|a| a.as_str())),
                        output_action: opt_action(f.output_action().map(|a| a.as_str())),
                        input_enabled: f.input_enabled().unwrap_or(true),
                        output_enabled: f.output_enabled().unwrap_or(true),
                        input_modalities: f
                            .input_modalities()
                            .iter()
                            .map(|m| m.as_str().to_string())
                            .collect(),
                        output_modalities: f
                            .output_modalities()
                            .iter()
                            .map(|m| m.as_str().to_string())
                            .collect(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let content_tier = g
            .content_policy()
            .and_then(|p| p.tier())
            .map(|t| t.tier_name().as_str().to_string())
            .unwrap_or_default();

        let topics = g
            .topic_policy()
            .map(|p| {
                p.topics()
                    .iter()
                    .map(|t| GTopic {
                        name: t.name().to_string(),
                        definition: t.definition().to_string(),
                        examples: t.examples().to_vec(),
                        input_action: opt_action(t.input_action().map(|a| a.as_str())),
                        output_action: opt_action(t.output_action().map(|a| a.as_str())),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let topic_tier = g
            .topic_policy()
            .and_then(|p| p.tier())
            .map(|t| t.tier_name().as_str().to_string())
            .unwrap_or_default();

        let words = g
            .word_policy()
            .map(|p| {
                p.words()
                    .iter()
                    .map(|w| GWord {
                        text: w.text().to_string(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let managed_word_lists = g
            .word_policy()
            .map(|p| {
                p.managed_word_lists()
                    .iter()
                    .map(|m| GManagedWords {
                        word_type: m.r#type().as_str().to_string(),
                        input_action: opt_action(m.input_action().map(|a| a.as_str())),
                        output_action: opt_action(m.output_action().map(|a| a.as_str())),
                    })
                    .collect()
            })
            .unwrap_or_default();

        let pii_entities = g
            .sensitive_information_policy()
            .map(|p| {
                p.pii_entities()
                    .iter()
                    .map(|e| GPiiEntity {
                        entity_type: e.r#type().as_str().to_string(),
                        action: e.action().as_str().to_string(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let regexes = g
            .sensitive_information_policy()
            .map(|p| {
                p.regexes()
                    .iter()
                    .map(|r| GRegex {
                        name: r.name().to_string(),
                        description: r.description().unwrap_or("").to_string(),
                        pattern: r.pattern().to_string(),
                        action: r.action().as_str().to_string(),
                    })
                    .collect()
            })
            .unwrap_or_default();

        let grounding_filters = g
            .contextual_grounding_policy()
            .map(|p| {
                p.filters()
                    .iter()
                    .map(|f| GGroundingFilter {
                        filter_type: f.r#type().as_str().to_string(),
                        threshold: f.threshold(),
                        action: opt_action(f.action().map(|a| a.as_str())),
                        enabled: f.enabled().unwrap_or(true),
                    })
                    .collect()
            })
            .unwrap_or_default();

        let cross_region_profile = g
            .cross_region_details()
            .and_then(|c| {
                c.guardrail_profile_arn()
                    .or_else(|| c.guardrail_profile_id())
            })
            .unwrap_or("")
            .to_string();

        let automated_reasoning_policies = g
            .automated_reasoning_policy()
            .map(|p| p.policies().to_vec())
            .unwrap_or_default();

        Self {
            id: g.guardrail_id().to_string(),
            arn: g.guardrail_arn().to_string(),
            name: g.name().to_string(),
            description: g.description().unwrap_or("").to_string(),
            status: g.status().as_str().to_string(),
            version: g.version().to_string(),
            kms_key_arn: g.kms_key_arn().unwrap_or("").to_string(),
            blocked_input_messaging: g.blocked_input_messaging().to_string(),
            blocked_outputs_messaging: g.blocked_outputs_messaging().to_string(),
            status_reasons: g.status_reasons().to_vec(),
            failure_recommendations: g.failure_recommendations().to_vec(),
            created_at: fmt_dt(Some(g.created_at())),
            updated_at: fmt_dt(Some(g.updated_at())),
            cross_region_profile,
            content_filters,
            content_tier,
            topics,
            topic_tier,
            words,
            managed_word_lists,
            pii_entities,
            regexes,
            grounding_filters,
            automated_reasoning_policies,
        }
    }
}

/// Stringify an optional guardrail action enum (BLOCK / ANONYMIZE / NONE / …).
fn opt_action(a: Option<&str>) -> String {
    a.unwrap_or("").to_string()
}

/// Fetch a guardrail's full policy config. `version` may be empty (defaults to
/// the DRAFT working version).
pub async fn fetch_guardrail_detail(
    client: BedrockClient,
    id: String,
    version: String,
) -> Result<GuardrailFull> {
    let mut req = client.get_guardrail().guardrail_identifier(&id);
    if !version.is_empty() {
        req = req.guardrail_version(&version);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
    Ok(GuardrailFull::from_sdk(&resp))
}

// ═══════════════════════════════════════════════════════════════════════════════
// Knowledge Base — full detail (lazy `GetKnowledgeBase` + data sources + ingestion)
// ═══════════════════════════════════════════════════════════════════════════════
//
// `ListKnowledgeBases` returns only a thin summary (id/name/status). The role,
// KB type, embedding model, vector-store wiring, data sources and their
// chunking config, and ingestion-job history all live behind separate
// `GetKnowledgeBase` / `ListDataSources`+`GetDataSource` / `ListIngestionJobs`
// calls, fetched lazily per section and cached by kb id.

/// The fully-resolved knowledge base — everything `GetKnowledgeBase` exposes.
#[derive(Debug, Clone)]
pub struct KnowledgeBaseFull {
    pub id: String,
    pub arn: String,
    pub name: String,
    pub description: String,
    pub status: String,
    pub role_arn: String,
    pub created_at: String,
    pub updated_at: String,
    pub failure_reasons: Vec<String>,
    pub kb_type: String,
    pub embedding_model_arn: String,
    pub embedding_dimensions: Option<i32>,
    pub embedding_data_type: String,
    pub storage_type: String,
    /// Pre-flattened per-store-type connection details (collection ARN, index
    /// name, endpoint, …) — the storage config is an 8-way union, so it's
    /// resolved to rows here rather than in the renderer.
    pub storage_detail: Vec<(String, String)>,
}

impl KnowledgeBaseFull {
    fn from_sdk(kb: &aws_sdk_bedrockagent::types::KnowledgeBase) -> Self {
        let cfg = kb.knowledge_base_configuration();
        let kb_type = cfg.map(|c| c.r#type().as_str().to_string()).unwrap_or_default();

        let vector_cfg = cfg.and_then(|c| c.vector_knowledge_base_configuration());
        let embedding_model_arn = vector_cfg
            .map(|v| v.embedding_model_arn().to_string())
            .unwrap_or_default();
        let bedrock_emb = vector_cfg
            .and_then(|v| v.embedding_model_configuration())
            .and_then(|e| e.bedrock_embedding_model_configuration());
        let embedding_dimensions = bedrock_emb.and_then(|b| b.dimensions());
        let embedding_data_type = bedrock_emb
            .and_then(|b| b.embedding_data_type())
            .map(|d| d.as_str().to_string())
            .unwrap_or_default();

        let (storage_type, storage_detail) = kb
            .storage_configuration()
            .map(storage_rows)
            .unwrap_or_default();

        Self {
            id: kb.knowledge_base_id().to_string(),
            arn: kb.knowledge_base_arn().to_string(),
            name: kb.name().to_string(),
            description: kb.description().unwrap_or("").to_string(),
            status: kb.status().as_str().to_string(),
            role_arn: kb.role_arn().to_string(),
            created_at: fmt_dt(Some(kb.created_at())),
            updated_at: fmt_dt(Some(kb.updated_at())),
            failure_reasons: kb.failure_reasons().to_vec(),
            kb_type,
            embedding_model_arn,
            embedding_dimensions,
            embedding_data_type,
            storage_type,
            storage_detail,
        }
    }
}

/// Flatten the vector-store union into `(store_type, rows)`.
fn storage_rows(
    sc: &aws_sdk_bedrockagent::types::StorageConfiguration,
) -> (String, Vec<(String, String)>) {
    let stype = sc.r#type().as_str().to_string();
    let mut rows: Vec<(String, String)> = Vec::new();
    let kv = |rows: &mut Vec<(String, String)>, k: &str, v: &str| {
        if !v.is_empty() {
            rows.push((k.to_string(), v.to_string()));
        }
    };
    if let Some(c) = sc.opensearch_serverless_configuration() {
        kv(&mut rows, "Collection ARN", c.collection_arn());
        kv(&mut rows, "Vector Index", c.vector_index_name());
    } else if let Some(c) = sc.opensearch_managed_cluster_configuration() {
        kv(&mut rows, "Domain Endpoint", c.domain_endpoint());
        kv(&mut rows, "Domain ARN", c.domain_arn());
        kv(&mut rows, "Vector Index", c.vector_index_name());
    } else if let Some(c) = sc.pinecone_configuration() {
        kv(&mut rows, "Connection", c.connection_string());
        kv(&mut rows, "Namespace", c.namespace().unwrap_or(""));
        kv(&mut rows, "Credentials Secret", c.credentials_secret_arn());
    } else if let Some(c) = sc.rds_configuration() {
        kv(&mut rows, "Cluster ARN", c.resource_arn());
        kv(&mut rows, "Database", c.database_name());
        kv(&mut rows, "Table", c.table_name());
        kv(&mut rows, "Credentials Secret", c.credentials_secret_arn());
    } else if let Some(c) = sc.mongo_db_atlas_configuration() {
        kv(&mut rows, "Endpoint", c.endpoint());
        kv(&mut rows, "Database", c.database_name());
        kv(&mut rows, "Collection", c.collection_name());
        kv(&mut rows, "Vector Index", c.vector_index_name());
    } else if let Some(c) = sc.neptune_analytics_configuration() {
        kv(&mut rows, "Graph ARN", c.graph_arn());
    } else if let Some(c) = sc.redis_enterprise_cloud_configuration() {
        kv(&mut rows, "Endpoint", c.endpoint());
        kv(&mut rows, "Vector Index", c.vector_index_name());
        kv(&mut rows, "Credentials Secret", c.credentials_secret_arn());
    } else if let Some(c) = sc.s3_vectors_configuration() {
        kv(&mut rows, "Vector Bucket ARN", c.vector_bucket_arn().unwrap_or(""));
        kv(&mut rows, "Index ARN", c.index_arn().unwrap_or(""));
        kv(&mut rows, "Index Name", c.index_name().unwrap_or(""));
    }
    (stype, rows)
}

/// A knowledge-base data source with its connector config + chunking strategy.
#[derive(Debug, Clone)]
pub struct KbDataSource {
    pub id: String,
    pub name: String,
    pub status: String,
    pub description: String,
    pub ds_type: String,
    pub data_deletion_policy: String,
    pub updated_at: String,
    pub source_detail: Vec<(String, String)>,
    pub chunking: String,
    pub failure_reasons: Vec<String>,
}

impl KbDataSource {
    fn from_sdk(ds: &aws_sdk_bedrockagent::types::DataSource) -> Self {
        let cfg = ds.data_source_configuration();
        let ds_type = cfg.map(|c| c.r#type().as_str().to_string()).unwrap_or_default();

        let mut source_detail: Vec<(String, String)> = Vec::new();
        if let Some(c) = cfg {
            if let Some(s3) = c.s3_configuration() {
                source_detail.push(("Bucket ARN".to_string(), s3.bucket_arn().to_string()));
                let prefixes = s3.inclusion_prefixes();
                if !prefixes.is_empty() {
                    source_detail.push(("Prefixes".to_string(), prefixes.join(", ")));
                }
                if let Some(owner) = s3.bucket_owner_account_id() {
                    source_detail.push(("Bucket Owner".to_string(), owner.to_string()));
                }
            } else if let Some(web) = c.web_configuration() {
                if let Some(sc) = web.source_configuration() {
                    if let Some(url_cfg) = sc.url_configuration() {
                        let urls: Vec<String> = url_cfg
                            .seed_urls()
                            .iter()
                            .filter_map(|u| u.url().map(|s| s.to_string()))
                            .collect();
                        if !urls.is_empty() {
                            source_detail.push(("Seed URLs".to_string(), urls.join(", ")));
                        }
                    }
                }
            }
        }

        let chunking = ds
            .vector_ingestion_configuration()
            .map(chunking_desc)
            .unwrap_or_default();

        Self {
            id: ds.data_source_id().to_string(),
            name: ds.name().to_string(),
            status: ds.status().as_str().to_string(),
            description: ds.description().unwrap_or("").to_string(),
            ds_type,
            data_deletion_policy: ds
                .data_deletion_policy()
                .map(|p| p.as_str().to_string())
                .unwrap_or_default(),
            updated_at: fmt_dt(Some(ds.updated_at())),
            source_detail,
            chunking,
            failure_reasons: ds.failure_reasons().to_vec(),
        }
    }
}

/// Human-readable one-liner for a data source's chunking strategy.
fn chunking_desc(vic: &aws_sdk_bedrockagent::types::VectorIngestionConfiguration) -> String {
    let Some(cc) = vic.chunking_configuration() else {
        return String::new();
    };
    use aws_sdk_bedrockagent::types::ChunkingStrategy;
    match cc.chunking_strategy() {
        ChunkingStrategy::FixedSize => {
            if let Some(f) = cc.fixed_size_chunking_configuration() {
                format!(
                    "Fixed size · {} tok · {}% overlap",
                    f.max_tokens(),
                    f.overlap_percentage()
                )
            } else {
                "Fixed size".to_string()
            }
        }
        ChunkingStrategy::Hierarchical => {
            if let Some(h) = cc.hierarchical_chunking_configuration() {
                format!("Hierarchical · {} overlap tokens", h.overlap_tokens())
            } else {
                "Hierarchical".to_string()
            }
        }
        ChunkingStrategy::Semantic => {
            if let Some(s) = cc.semantic_chunking_configuration() {
                format!(
                    "Semantic · max {} tok · buffer {} · {}% breakpoint",
                    s.max_tokens(),
                    s.buffer_size(),
                    s.breakpoint_percentile_threshold()
                )
            } else {
                "Semantic".to_string()
            }
        }
        ChunkingStrategy::None => "None (one chunk per file)".to_string(),
        other => other.as_str().to_string(),
    }
}

/// A recent ingestion (sync) job for a data source.
#[derive(Debug, Clone)]
pub struct KbIngestionJob {
    pub job_id: String,
    pub data_source_id: String,
    pub status: String,
    pub started_at: String,
    pub updated_at: String,
    pub docs_scanned: i64,
    pub docs_indexed: i64,
    pub docs_failed: i64,
    pub docs_deleted: i64,
}

impl KbIngestionJob {
    fn from_sdk(j: &aws_sdk_bedrockagent::types::IngestionJobSummary) -> Self {
        let stats = j.statistics();
        Self {
            job_id: j.ingestion_job_id().to_string(),
            data_source_id: j.data_source_id().to_string(),
            status: j.status().as_str().to_string(),
            started_at: fmt_dt(Some(j.started_at())),
            updated_at: fmt_dt(Some(j.updated_at())),
            docs_scanned: stats.map(|s| s.number_of_documents_scanned()).unwrap_or(0),
            docs_indexed: stats
                .map(|s| s.number_of_new_documents_indexed() + s.number_of_modified_documents_indexed())
                .unwrap_or(0),
            docs_failed: stats.map(|s| s.number_of_documents_failed()).unwrap_or(0),
            docs_deleted: stats.map(|s| s.number_of_documents_deleted()).unwrap_or(0),
        }
    }
}

/// Fetch a knowledge base's full config.
pub async fn fetch_kb_detail(
    client: BedrockAgentClient,
    kb_id: String,
) -> Result<KnowledgeBaseFull> {
    let resp = client
        .get_knowledge_base()
        .knowledge_base_id(&kb_id)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
    let kb = resp
        .knowledge_base()
        .ok_or_else(|| crate::error::Error::ResourceNotFound(kb_id.clone()))?;
    Ok(KnowledgeBaseFull::from_sdk(kb))
}

/// Fetch a KB's data sources: `ListDataSources` for the ids, then a
/// `GetDataSource` per source for the full connector + chunking config.
pub async fn fetch_kb_data_sources(
    client: BedrockAgentClient,
    kb_id: String,
) -> Result<Vec<KbDataSource>> {
    let ids = list_data_source_ids(&client, &kb_id).await?;
    let mut sources = Vec::new();
    for id in ids {
        let resp = client
            .get_data_source()
            .knowledge_base_id(&kb_id)
            .data_source_id(&id)
            .send()
            .await
            .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
        if let Some(ds) = resp.data_source() {
            sources.push(KbDataSource::from_sdk(ds));
        }
    }
    Ok(sources)
}

/// Fetch recent ingestion jobs across all of a KB's data sources (capped per
/// source so a chatty auto-sync KB can't flood the pane).
pub async fn fetch_kb_ingestion(
    client: BedrockAgentClient,
    kb_id: String,
) -> Result<Vec<KbIngestionJob>> {
    const MAX_JOBS_PER_SOURCE: usize = 10;
    let ids = list_data_source_ids(&client, &kb_id).await?;
    let mut jobs = Vec::new();
    for id in ids {
        let mut count = 0usize;
        let mut paginator = client
            .list_ingestion_jobs()
            .knowledge_base_id(&kb_id)
            .data_source_id(&id)
            .into_paginator()
            .send();
        'pages: loop {
            match paginator.next().await {
                Some(Ok(page)) => {
                    for j in page.ingestion_job_summaries() {
                        jobs.push(KbIngestionJob::from_sdk(j));
                        count += 1;
                        if count >= MAX_JOBS_PER_SOURCE {
                            break 'pages;
                        }
                    }
                }
                Some(Err(e)) => {
                    return Err(crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))
                }
                None => break,
            }
        }
    }
    Ok(jobs)
}

async fn list_data_source_ids(
    client: &BedrockAgentClient,
    kb_id: &str,
) -> Result<Vec<String>> {
    let mut ids = Vec::new();
    let mut paginator = client
        .list_data_sources()
        .knowledge_base_id(kb_id)
        .into_paginator()
        .send();
    loop {
        match paginator.next().await {
            Some(Ok(page)) => {
                for s in page.data_source_summaries() {
                    ids.push(s.data_source_id().to_string());
                }
            }
            Some(Err(e)) => {
                return Err(crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))
            }
            None => break,
        }
    }
    Ok(ids)
}

// ═══════════════════════════════════════════════════════════════════════════════
// Agent
// ═══════════════════════════════════════════════════════════════════════════════
//
// `ListAgents` returns a thin summary; the foundation model, instruction, role,
// guardrail, memory + orchestration config (Overview), and the agent's action
// groups / aliases / associated knowledge bases each need a separate call,
// fetched lazily per section and keyed by agent id. Sub-lists that need a
// version use `DRAFT` (the working draft).

const AGENT_DRAFT: &str = "DRAFT";

#[derive(Debug, Clone)]
pub struct BedrockAgent {
    pub agent_id: String,
    pub agent_name: String,
    pub status: String,
    pub description: String,
    pub latest_version: String,
    pub updated_at: String,
    tags: HashMap<String, String>,
}

impl BedrockAgent {
    pub fn from_sdk(a: &aws_sdk_bedrockagent::types::AgentSummary) -> Self {
        Self {
            agent_id: a.agent_id().to_string(),
            agent_name: a.agent_name().to_string(),
            status: a.agent_status().as_str().to_string(),
            description: a.description().unwrap_or("").to_string(),
            latest_version: a.latest_agent_version().unwrap_or("").to_string(),
            updated_at: fmt_dt(Some(a.updated_at())),
            tags: HashMap::new(),
        }
    }
}

crate::sections! {
    pub enum BedrockAgentDetailSection,
    pub static BEDROCK_AGENT_SECTIONS = [
        Overview "Overview" => crate::app::App::trigger_agent_detail_load,
        ActionGroups "Action Groups" => crate::app::App::trigger_agent_action_groups_load,
        Aliases "Aliases" => crate::app::App::trigger_agent_aliases_load,
        KnowledgeBases "Knowledge Bases" => crate::app::App::trigger_agent_knowledge_bases_load,
    ]
}

impl Resource for BedrockAgent {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&BEDROCK_AGENT_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.agent_id
    }
    fn name(&self) -> &str {
        &self.agent_name
    }
    fn resource_type(&self) -> &str {
        "Bedrock Agent"
    }
    fn state(&self) -> ResourceState {
        match self.status.to_uppercase().as_str() {
            "PREPARED" => ResourceState::Available,
            "FAILED" => ResourceState::Unavailable,
            "CREATING" | "PREPARING" | "UPDATING" => ResourceState::Pending,
            "DELETING" => ResourceState::Deleting,
            "NOT_PREPARED" | "VERSIONING" => ResourceState::Unknown(self.status.to_lowercase()),
            other => ResourceState::Unknown(other.to_lowercase()),
        }
    }

    fn state_label(&self) -> String {
        native_state_label(&self.status, || self.state())
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        format!("{} {}", self.agent_id, self.agent_name)
    }
    fn details(&self) -> Vec<(String, String)> {
        // Real content is the split pane; this is only the flat fallback.
        let mut d = vec![
            ("Name".to_string(), self.agent_name.clone()),
            ("Agent ID".to_string(), self.agent_id.clone()),
            ("Status".to_string(), self.status.clone()),
            ("Latest Version".to_string(), self.latest_version.clone()),
            ("Description".to_string(), self.description.clone()),
            ("Updated".to_string(), self.updated_at.clone()),
        ];
        d.retain(|(_, v)| !v.is_empty());
        d
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{r}.console.aws.amazon.com/bedrock/home?region={r}#/agents/{id}",
            r = region,
            id = self.agent_id
        ))
    }
}

/// The fully-resolved agent — everything `GetAgent` exposes.
#[derive(Debug, Clone)]
pub struct AgentFull {
    pub id: String,
    pub arn: String,
    pub name: String,
    pub version: String,
    pub status: String,
    pub description: String,
    pub instruction: String,
    pub foundation_model: String,
    pub role_arn: String,
    pub idle_ttl_secs: i32,
    pub orchestration_type: String,
    pub guardrail_id: String,
    pub guardrail_version: String,
    pub kms_key_arn: String,
    pub memory: String,
    pub created_at: String,
    pub updated_at: String,
    pub prepared_at: String,
    pub failure_reasons: Vec<String>,
}

impl AgentFull {
    fn from_sdk(a: &aws_sdk_bedrockagent::types::Agent) -> Self {
        let (guardrail_id, guardrail_version) = a
            .guardrail_configuration()
            .map(|g| {
                (
                    g.guardrail_identifier().unwrap_or("").to_string(),
                    g.guardrail_version().unwrap_or("").to_string(),
                )
            })
            .unwrap_or_default();

        let memory = a
            .memory_configuration()
            .map(|m| {
                let types: Vec<String> =
                    m.enabled_memory_types().iter().map(|t| t.as_str().to_string()).collect();
                if types.is_empty() {
                    format!("{} day retention", m.storage_days())
                } else {
                    format!("{} · {} day retention", types.join(", "), m.storage_days())
                }
            })
            .unwrap_or_default();

        Self {
            id: a.agent_id().to_string(),
            arn: a.agent_arn().to_string(),
            name: a.agent_name().to_string(),
            version: a.agent_version().to_string(),
            status: a.agent_status().as_str().to_string(),
            description: a.description().unwrap_or("").to_string(),
            instruction: a.instruction().unwrap_or("").to_string(),
            foundation_model: a.foundation_model().unwrap_or("").to_string(),
            role_arn: a.agent_resource_role_arn().to_string(),
            idle_ttl_secs: a.idle_session_ttl_in_seconds(),
            orchestration_type: a
                .orchestration_type()
                .map(|o| o.as_str().to_string())
                .unwrap_or_default(),
            guardrail_id,
            guardrail_version,
            kms_key_arn: a.customer_encryption_key_arn().unwrap_or("").to_string(),
            memory,
            created_at: fmt_dt(Some(a.created_at())),
            updated_at: fmt_dt(Some(a.updated_at())),
            prepared_at: fmt_dt(a.prepared_at()),
            failure_reasons: a.failure_reasons().to_vec(),
        }
    }
}

/// An agent action group (a tool/function surface).
#[derive(Debug, Clone)]
pub struct AgentActionGroup {
    pub id: String,
    pub name: String,
    pub state: String,
    pub description: String,
    pub updated_at: String,
}

/// An agent alias (a deployable pointer to a version).
#[derive(Debug, Clone)]
pub struct AgentAlias {
    pub id: String,
    pub name: String,
    pub status: String,
    pub description: String,
    pub routed_versions: Vec<String>,
    pub updated_at: String,
}

/// A knowledge base associated with an agent.
#[derive(Debug, Clone)]
pub struct AgentKb {
    pub kb_id: String,
    pub state: String,
    pub description: String,
    pub updated_at: String,
}

pub async fn fetch_agent_detail(client: BedrockAgentClient, agent_id: String) -> Result<AgentFull> {
    let resp = client
        .get_agent()
        .agent_id(&agent_id)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
    let agent = resp
        .agent()
        .ok_or_else(|| crate::error::Error::ResourceNotFound(agent_id.clone()))?;
    Ok(AgentFull::from_sdk(agent))
}

pub async fn fetch_agent_action_groups(
    client: BedrockAgentClient,
    agent_id: String,
) -> Result<Vec<AgentActionGroup>> {
    let mut out = Vec::new();
    let mut paginator = client
        .list_agent_action_groups()
        .agent_id(&agent_id)
        .agent_version(AGENT_DRAFT)
        .into_paginator()
        .send();
    loop {
        match paginator.next().await {
            Some(Ok(page)) => {
                for g in page.action_group_summaries() {
                    out.push(AgentActionGroup {
                        id: g.action_group_id().to_string(),
                        name: g.action_group_name().to_string(),
                        state: g.action_group_state().as_str().to_string(),
                        description: g.description().unwrap_or("").to_string(),
                        updated_at: fmt_dt(Some(g.updated_at())),
                    });
                }
            }
            Some(Err(e)) => {
                return Err(crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))
            }
            None => break,
        }
    }
    Ok(out)
}

pub async fn fetch_agent_aliases(
    client: BedrockAgentClient,
    agent_id: String,
) -> Result<Vec<AgentAlias>> {
    let mut out = Vec::new();
    let mut paginator = client
        .list_agent_aliases()
        .agent_id(&agent_id)
        .into_paginator()
        .send();
    loop {
        match paginator.next().await {
            Some(Ok(page)) => {
                for a in page.agent_alias_summaries() {
                    let routed_versions: Vec<String> = a
                        .routing_configuration()
                        .iter()
                        .filter_map(|r| r.agent_version().map(|s| s.to_string()))
                        .collect();
                    out.push(AgentAlias {
                        id: a.agent_alias_id().to_string(),
                        name: a.agent_alias_name().to_string(),
                        status: a.agent_alias_status().as_str().to_string(),
                        description: a.description().unwrap_or("").to_string(),
                        routed_versions,
                        updated_at: fmt_dt(Some(a.updated_at())),
                    });
                }
            }
            Some(Err(e)) => {
                return Err(crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))
            }
            None => break,
        }
    }
    Ok(out)
}

pub async fn fetch_agent_knowledge_bases(
    client: BedrockAgentClient,
    agent_id: String,
) -> Result<Vec<AgentKb>> {
    let mut out = Vec::new();
    let mut paginator = client
        .list_agent_knowledge_bases()
        .agent_id(&agent_id)
        .agent_version(AGENT_DRAFT)
        .into_paginator()
        .send();
    loop {
        match paginator.next().await {
            Some(Ok(page)) => {
                for k in page.agent_knowledge_base_summaries() {
                    out.push(AgentKb {
                        kb_id: k.knowledge_base_id().to_string(),
                        state: k.knowledge_base_state().as_str().to_string(),
                        description: k.description().unwrap_or("").to_string(),
                        updated_at: fmt_dt(Some(k.updated_at())),
                    });
                }
            }
            Some(Err(e)) => {
                return Err(crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))
            }
            None => break,
        }
    }
    Ok(out)
}

// ═══════════════════════════════════════════════════════════════════════════════
// Prompt (Prompt Management) — flat detail
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct BedrockPrompt {
    pub id: String,
    pub name: String,
    pub arn: String,
    pub version: String,
    pub description: String,
    pub updated_at: String,
    tags: HashMap<String, String>,
}

impl BedrockPrompt {
    pub fn from_sdk(p: &aws_sdk_bedrockagent::types::PromptSummary) -> Self {
        Self {
            id: p.id().to_string(),
            name: p.name().to_string(),
            arn: p.arn().to_string(),
            version: p.version().to_string(),
            description: p.description().unwrap_or("").to_string(),
            updated_at: fmt_dt(Some(p.updated_at())),
            tags: HashMap::new(),
        }
    }
}

impl Resource for BedrockPrompt {
    fn id(&self) -> &str {
        &self.id
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn resource_type(&self) -> &str {
        "Bedrock Prompt"
    }
    fn state(&self) -> ResourceState {
        ResourceState::stateless()
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        format!("{} {}", self.id, self.name)
    }
    fn details(&self) -> Vec<(String, String)> {
        let mut d = vec![
            ("Name".to_string(), self.name.clone()),
            ("Prompt ID".to_string(), self.id.clone()),
            ("ARN".to_string(), self.arn.clone()),
            ("Version".to_string(), self.version.clone()),
            ("Description".to_string(), self.description.clone()),
            ("Updated".to_string(), self.updated_at.clone()),
        ];
        d.retain(|(_, v)| !v.is_empty());
        d
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{r}.console.aws.amazon.com/bedrock/home?region={r}#/prompt-management",
            r = region
        ))
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Flow (Prompt Flows) — flat detail
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct BedrockFlow {
    pub id: String,
    pub name: String,
    pub arn: String,
    pub status: String,
    pub version: String,
    pub description: String,
    pub updated_at: String,
    tags: HashMap<String, String>,
}

impl BedrockFlow {
    pub fn from_sdk(f: &aws_sdk_bedrockagent::types::FlowSummary) -> Self {
        Self {
            id: f.id().to_string(),
            name: f.name().to_string(),
            arn: f.arn().to_string(),
            status: f.status().as_str().to_string(),
            version: f.version().to_string(),
            description: f.description().unwrap_or("").to_string(),
            updated_at: fmt_dt(Some(f.updated_at())),
            tags: HashMap::new(),
        }
    }
}

impl Resource for BedrockFlow {
    fn id(&self) -> &str {
        &self.id
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn resource_type(&self) -> &str {
        "Bedrock Flow"
    }
    fn state(&self) -> ResourceState {
        match self.status.to_uppercase().as_str() {
            "PREPARED" => ResourceState::Available,
            "FAILED" => ResourceState::Unavailable,
            "PREPARING" => ResourceState::Pending,
            "NOT_PREPARED" => ResourceState::Unknown("not prepared".to_string()),
            other => ResourceState::Unknown(other.to_lowercase()),
        }
    }

    fn state_label(&self) -> String {
        native_state_label(&self.status, || self.state())
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        format!("{} {}", self.id, self.name)
    }
    fn details(&self) -> Vec<(String, String)> {
        let mut d = vec![
            ("Name".to_string(), self.name.clone()),
            ("Flow ID".to_string(), self.id.clone()),
            ("ARN".to_string(), self.arn.clone()),
            ("Status".to_string(), self.status.clone()),
            ("Version".to_string(), self.version.clone()),
            ("Description".to_string(), self.description.clone()),
            ("Updated".to_string(), self.updated_at.clone()),
        ];
        d.retain(|(_, v)| !v.is_empty());
        d
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{r}.console.aws.amazon.com/bedrock/home?region={r}#/flows",
            r = region
        ))
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Custom Model — flat detail
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct BedrockCustomModel {
    pub model_arn: String,
    pub model_name: String,
    pub base_model_name: String,
    pub customization_type: String,
    pub status: String,
    pub created_at: String,
    tags: HashMap<String, String>,
}

impl BedrockCustomModel {
    pub fn from_sdk(m: &aws_sdk_bedrock::types::CustomModelSummary) -> Self {
        Self {
            model_arn: m.model_arn().to_string(),
            model_name: m.model_name().to_string(),
            base_model_name: m.base_model_name().to_string(),
            customization_type: m
                .customization_type()
                .map(|c| c.as_str().to_string())
                .unwrap_or_default(),
            status: m
                .model_status()
                .map(|s| s.as_str().to_string())
                .unwrap_or_default(),
            created_at: fmt_dt(Some(m.creation_time())),
            tags: HashMap::new(),
        }
    }
}

impl Resource for BedrockCustomModel {
    fn id(&self) -> &str {
        &self.model_arn
    }
    fn name(&self) -> &str {
        &self.model_name
    }
    fn resource_type(&self) -> &str {
        "Bedrock Custom Model"
    }
    fn state(&self) -> ResourceState {
        match self.status.to_uppercase().as_str() {
            "ACTIVE" | "" => ResourceState::Available,
            "FAILED" => ResourceState::Unavailable,
            "CREATING" => ResourceState::Creating,
            other => ResourceState::Unknown(other.to_lowercase()),
        }
    }

    fn state_label(&self) -> String {
        native_state_label(&self.status, || self.state())
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        format!("{} {}", self.model_name, self.base_model_name)
    }
    fn details(&self) -> Vec<(String, String)> {
        let mut d = vec![
            ("Name".to_string(), self.model_name.clone()),
            ("ARN".to_string(), self.model_arn.clone()),
            ("Base Model".to_string(), self.base_model_name.clone()),
            ("Customization".to_string(), self.customization_type.clone()),
            ("Status".to_string(), self.status.clone()),
            ("Created".to_string(), self.created_at.clone()),
        ];
        d.retain(|(_, v)| !v.is_empty());
        d
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{r}.console.aws.amazon.com/bedrock/home?region={r}#/custom-models",
            r = region
        ))
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Imported Model — flat detail
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct BedrockImportedModel {
    pub model_arn: String,
    pub model_name: String,
    pub architecture: String,
    pub instruct_supported: bool,
    pub created_at: String,
    tags: HashMap<String, String>,
}

impl BedrockImportedModel {
    pub fn from_sdk(m: &aws_sdk_bedrock::types::ImportedModelSummary) -> Self {
        Self {
            model_arn: m.model_arn().to_string(),
            model_name: m.model_name().to_string(),
            architecture: m.model_architecture().unwrap_or("").to_string(),
            instruct_supported: m.instruct_supported().unwrap_or(false),
            created_at: fmt_dt(Some(m.creation_time())),
            tags: HashMap::new(),
        }
    }
}

impl Resource for BedrockImportedModel {
    fn id(&self) -> &str {
        &self.model_arn
    }
    fn name(&self) -> &str {
        &self.model_name
    }
    fn resource_type(&self) -> &str {
        "Bedrock Imported Model"
    }
    fn state(&self) -> ResourceState {
        ResourceState::stateless()
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        format!("{} {}", self.model_name, self.architecture)
    }
    fn details(&self) -> Vec<(String, String)> {
        let mut d = vec![
            ("Name".to_string(), self.model_name.clone()),
            ("ARN".to_string(), self.model_arn.clone()),
            ("Architecture".to_string(), self.architecture.clone()),
            (
                "Instruct Supported".to_string(),
                if self.instruct_supported { "Yes" } else { "No" }.to_string(),
            ),
            ("Created".to_string(), self.created_at.clone()),
        ];
        d.retain(|(_, v)| !v.is_empty());
        d
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{r}.console.aws.amazon.com/bedrock/home?region={r}#/imported-models",
            r = region
        ))
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// CloudWatch metrics (`m` overlay) — AWS/Bedrock, dimension ModelId
// ═══════════════════════════════════════════════════════════════════════════════
//
// Works for both foundation models (ModelId = model id) and inference profiles
// (ModelId = profile id). No data for a given id just renders empty charts.

pub use crate::aws::services::ec2::MetricsTimeRange;

#[derive(Debug, Clone)]
pub struct BedrockMetricsData {
    pub time_range: MetricsTimeRange,
    pub invocations: Vec<(f64, f64)>,
    pub latency_ms: Vec<(f64, f64)>,
    pub input_tokens: Vec<(f64, f64)>,
    pub output_tokens: Vec<(f64, f64)>,
    pub throttles: Vec<(f64, f64)>,
    pub errors: Vec<(f64, f64)>,
    pub x_max: f64,
}

#[derive(Debug, Clone)]
pub enum BedrockMetricsState {
    Loading,
    Loaded(BedrockMetricsData),
}

/// Pull `AWS/Bedrock` invocation metrics for one model / inference profile.
/// `InvocationLatency` (avg, ms) and token counts are the headline usage
/// signals; throttles + client/server errors surface capacity/permission pain.
pub async fn fetch_bedrock_model_metrics(
    cw: aws_sdk_cloudwatch::Client,
    model_id: String,
    time_range: MetricsTimeRange,
) -> Result<BedrockMetricsData> {
    use aws_sdk_cloudwatch::types::{Dimension, Statistic};

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let start = now - time_range.duration_secs();
    let period = time_range.period_secs();
    let start_dt = aws_sdk_cloudwatch::primitives::DateTime::from_secs(start);
    let end_dt = aws_sdk_cloudwatch::primitives::DateTime::from_secs(now);

    let dim = || Dimension::builder().name("ModelId").value(&model_id).build();
    let metric = |name: &'static str, stat: Statistic| {
        cw.get_metric_statistics()
            .namespace("AWS/Bedrock")
            .metric_name(name)
            .dimensions(dim())
            .start_time(start_dt)
            .end_time(end_dt)
            .period(period)
            .set_statistics(Some(vec![stat]))
            .send()
    };

    let (invocations, latency, input_tokens, output_tokens, throttles, client_err, server_err) = tokio::join!(
        metric("Invocations", Statistic::Sum),
        metric("InvocationLatency", Statistic::Average),
        metric("InputTokenCount", Statistic::Sum),
        metric("OutputTokenCount", Statistic::Sum),
        metric("InvocationThrottles", Statistic::Sum),
        metric("InvocationClientErrors", Statistic::Sum),
        metric("InvocationServerErrors", Statistic::Sum),
    );

    let client_pts = parse_points(client_err, start, |dp| dp.sum());
    let server_pts = parse_points(server_err, start, |dp| dp.sum());

    Ok(BedrockMetricsData {
        time_range,
        invocations: parse_points(invocations, start, |dp| dp.sum()),
        latency_ms: parse_points(latency, start, |dp| dp.average()),
        input_tokens: parse_points(input_tokens, start, |dp| dp.sum()),
        output_tokens: parse_points(output_tokens, start, |dp| dp.sum()),
        throttles: parse_points(throttles, start, |dp| dp.sum()),
        errors: sum_series(&client_pts, &server_pts),
        x_max: time_range.duration_secs() as f64,
    })
}

/// Flatten a `GetMetricStatistics` response to `(x_offset_secs, value)` points,
/// sorted by time. Errors → empty series (the chart just renders blank).
fn parse_points<E>(
    resp: std::result::Result<
        aws_sdk_cloudwatch::operation::get_metric_statistics::GetMetricStatisticsOutput,
        E,
    >,
    start: i64,
    pick: impl Fn(&aws_sdk_cloudwatch::types::Datapoint) -> Option<f64>,
) -> Vec<(f64, f64)> {
    let dps = match resp {
        Ok(r) => r.datapoints().to_vec(),
        Err(_) => vec![],
    };
    let mut pts: Vec<(f64, f64)> = dps
        .iter()
        .filter_map(|dp| Some((dp.timestamp()?.secs() as f64 - start as f64, pick(dp)?)))
        .collect();
    pts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    pts
}

/// Sum two aligned point-series by x (client + server errors → total errors).
fn sum_series(a: &[(f64, f64)], b: &[(f64, f64)]) -> Vec<(f64, f64)> {
    use std::collections::BTreeMap;
    let mut merged: BTreeMap<i64, f64> = BTreeMap::new();
    for (x, y) in a.iter().chain(b.iter()) {
        *merged.entry(*x as i64).or_insert(0.0) += *y;
    }
    merged.into_iter().map(|(x, y)| (x as f64, y)).collect()
}
