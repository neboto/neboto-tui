use std::any::Any;
use std::collections::HashMap;

use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::aws::client::AwsClients;
use crate::aws::resource::{native_state_label, Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::error::Result;
use crate::event::{Event, LoadProgress};

type HealthClient = aws_sdk_health::Client;

// ═══════════════════════════════════════════════════════════════════════════════
// Service
// ═══════════════════════════════════════════════════════════════════════════════

pub struct HealthService {
    client: HealthClient,
    status_filter: HealthStatusFilter,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthStatusFilter {
    OpenUpcoming,
    /// Closed/resolved events — reserved for a future filter toggle; the
    /// status-code mapping already handles it.
    #[allow(dead_code)]
    Closed,
}

impl HealthService {
    pub fn new(clients: &AwsClients, filter: HealthStatusFilter) -> Self {
        Self {
            client: clients.health_client(),
            status_filter: filter,
        }
    }
}

fn friendly_error(e: &str) -> Option<String> {
    if e.contains("SubscriptionRequired") || e.contains("subscription") {
        Some(
            "AWS Health requires a Business or Enterprise Support plan. \
             Upgrade in the Support console to use this view."
                .to_string(),
        )
    } else {
        None
    }
}

#[async_trait]
impl AwsService for HealthService {
    fn service_type(&self) -> ServiceType {
        ServiceType::Health
    }

    fn name(&self) -> &str {
        "AWS Health"
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

        let status_codes = match self.status_filter {
            HealthStatusFilter::OpenUpcoming => vec![
                aws_sdk_health::types::EventStatusCode::Open,
                aws_sdk_health::types::EventStatusCode::Upcoming,
            ],
            HealthStatusFilter::Closed => vec![
                aws_sdk_health::types::EventStatusCode::Closed,
            ],
        };

        let filter = aws_sdk_health::types::EventFilter::builder()
            .set_event_status_codes(Some(status_codes))
            .build();

        let mut paginator = self
            .client
            .describe_events()
            .filter(filter)
            .into_paginator()
            .send();

        while let Some(result) = paginator.next().await {
            match result {
                Ok(page) => {
                    let mut batch: Vec<Box<dyn Resource>> = page
                        .events()
                        .iter()
                        .map(|e| Box::new(HealthEvent::from_sdk(e)) as Box<dyn Resource>)
                        .collect();
                    // Sort newest first
                    batch.sort_by(|a, b| {
                        let ha = a.as_any().downcast_ref::<HealthEvent>().unwrap();
                        let hb = b.as_any().downcast_ref::<HealthEvent>().unwrap();
                        hb.last_updated_time.cmp(&ha.last_updated_time)
                    });
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
                    let msg = crate::error::sdk_error_message(&e);
                    let display = friendly_error(&msg).unwrap_or(msg);
                    let _ = event_tx.send(Event::ResourceLoadError {
                        service: service_type,
                        error: display,
                    });
                    break;
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
// Resource
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct HealthEvent {
    pub arn: String,
    pub service: String,
    pub region: String,
    pub availability_zone: String,
    pub event_type_code: String,
    pub category: String,
    pub status_code: String,
    pub scope: String,
    pub start_time: String,
    pub end_time: String,
    pub last_updated_time: String,
}

impl HealthEvent {
    fn from_sdk(e: &aws_sdk_health::types::Event) -> Self {
        Self {
            arn: e.arn().unwrap_or_default().to_string(),
            service: e.service().unwrap_or_default().to_string(),
            region: e.region().unwrap_or_default().to_string(),
            availability_zone: e.availability_zone().unwrap_or_default().to_string(),
            event_type_code: e.event_type_code().unwrap_or_default().to_string(),
            category: e
                .event_type_category()
                .map(|c| c.as_str().to_string())
                .unwrap_or_default(),
            status_code: e
                .status_code()
                .map(|s| s.as_str().to_string())
                .unwrap_or_default(),
            scope: e
                .event_scope_code()
                .map(|s| s.as_str().to_string())
                .unwrap_or_default(),
            start_time: e
                .start_time()
                .map(|d| {
                    d.fmt(aws_smithy_types::date_time::Format::DateTime)
                        .unwrap_or_default()
                })
                .unwrap_or_default(),
            end_time: e
                .end_time()
                .map(|d| {
                    d.fmt(aws_smithy_types::date_time::Format::DateTime)
                        .unwrap_or_default()
                })
                .unwrap_or_default(),
            last_updated_time: e
                .last_updated_time()
                .map(|d| {
                    d.fmt(aws_smithy_types::date_time::Format::DateTime)
                        .unwrap_or_default()
                })
                .unwrap_or_default(),
        }
    }

    pub fn display_name(&self) -> String {
        if self.service.is_empty() {
            self.event_type_code.clone()
        } else {
            format!("{}: {}", self.service, self.event_type_code)
        }
    }
}

crate::sections! {
    pub enum HealthEventDetailSection,
    pub static HEALTH_EVENT_SECTIONS = [
        Details "Details",
        Description "Description" => crate::app::App::trigger_health_event_details_load,
        AffectedEntities "Affected" => crate::app::App::trigger_health_event_details_load,
    ]
}

impl Resource for HealthEvent {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&HEALTH_EVENT_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.arn
    }
    fn name(&self) -> &str {
        &self.event_type_code
    }
    fn resource_type(&self) -> &str {
        "Health Event"
    }
    fn state(&self) -> ResourceState {
        match (self.category.as_str(), self.status_code.as_str()) {
            ("issue", "open") => ResourceState::Unavailable,
            ("scheduledChange", "upcoming") => ResourceState::Pending,
            ("investigation", "open") => ResourceState::Pending,
            (_, "closed") => ResourceState::Available,
            _ => ResourceState::Available,
        }
    }

    fn state_label(&self) -> String {
        native_state_label(&self.status_code, || self.state())
    }
    fn tags(&self) -> &HashMap<String, String> {
        static EMPTY: std::sync::LazyLock<HashMap<String, String>> =
            std::sync::LazyLock::new(HashMap::new);
        &EMPTY
    }
    fn search_text(&self) -> String {
        format!(
            "{} {} {} {} {} {}",
            self.service, self.region, self.event_type_code,
            self.category, self.status_code, self.scope
        )
    }
    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Event ARN".to_string(), self.arn.clone()),
            ("Service".to_string(), self.service.clone()),
            ("Type Code".to_string(), self.event_type_code.clone()),
            ("Category".to_string(), self.category.clone()),
            ("Status".to_string(), self.status_code.clone()),
            ("Scope".to_string(), self.scope.clone()),
            ("Region".to_string(), self.region.clone()),
            ("AZ".to_string(), self.availability_zone.clone()),
            ("Start Time".to_string(), self.start_time.clone()),
            ("End Time".to_string(), self.end_time.clone()),
            ("Last Updated".to_string(), self.last_updated_time.clone()),
        ]
    }
    fn console_url(&self, _region: &str) -> Option<String> {
        Some("https://health.aws.amazon.com/health/home".to_string())
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Lazy-loaded event details (description + affected entities)
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct HealthEventDetails {
    pub description: String,
    pub affected_entities: Vec<HealthAffectedEntity>,
}

#[derive(Debug, Clone)]
pub struct HealthAffectedEntity {
    pub entity_value: String,
    pub status_code: String,
    pub last_updated_time: String,
}

pub async fn fetch_event_details(
    client: HealthClient,
    event_arn: String,
) -> std::result::Result<HealthEventDetails, String> {
    // Get description
    let desc_resp = client
        .describe_event_details()
        .event_arns(&event_arn)
        .send()
        .await
        .map_err(|e| crate::error::sdk_error_message(&e))?;

    let description = desc_resp
        .successful_set()
        .first()
        .and_then(|d| d.event_description())
        .and_then(|d| d.latest_description())
        .unwrap_or_default()
        .to_string();

    // Get affected entities
    let mut entities = Vec::new();
    let filter = aws_sdk_health::types::EntityFilter::builder()
        .event_arns(&event_arn)
        .build()
        .map_err(|e| e.to_string())?;

    let mut paginator = client
        .describe_affected_entities()
        .filter(filter)
        .into_paginator()
        .send();

    while let Some(result) = paginator.next().await {
        match result {
            Ok(page) => {
                for entity in page.entities() {
                    entities.push(HealthAffectedEntity {
                        entity_value: entity
                            .entity_value()
                            .unwrap_or_default()
                            .to_string(),
                        status_code: entity
                            .status_code()
                            .map(|s| s.as_str().to_string())
                            .unwrap_or_default(),
                        last_updated_time: entity
                            .last_updated_time()
                            .map(|d| {
                                d.fmt(aws_smithy_types::date_time::Format::DateTime)
                                    .unwrap_or_default()
                            })
                            .unwrap_or_default(),
                    });
                }
            }
            Err(_) => break,
        }
    }

    Ok(HealthEventDetails {
        description,
        affected_entities: entities,
    })
}
