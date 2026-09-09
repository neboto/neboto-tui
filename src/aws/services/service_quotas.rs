use crate::aws::client::AwsClients;
use crate::aws::resource::{Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::error::Result;
use crate::event::{Event, LoadProgress};
use async_trait::async_trait;
use aws_sdk_servicequotas::Client as SqClient;
use std::any::Any;
use std::collections::HashMap;
use tokio::sync::mpsc;

/// Curated service codes for the quick `t`-cycle toggle (the picker reaches the
/// long tail). Order is the cycle order.
pub const CURATED_SERVICE_CODES: &[&str] = &[
    "ec2",
    "vpc",
    "lambda",
    "rds",
    "s3",
    "iam",
    "ecs",
    "elasticloadbalancing",
    "cloudformation",
    "dynamodb",
    "kms",
];

/// Service Quotas — the API is per-service (`ListServiceQuotas` needs a
/// `ServiceCode`), so the service is scoped to one `service_code` at a time,
/// variant-cached so switching back is instant.
pub struct ServiceQuotasService {
    client: SqClient,
    service_code: String,
}

impl ServiceQuotasService {
    pub fn with_code(aws_clients: &AwsClients, service_code: String) -> Self {
        Self {
            client: aws_clients.service_quotas_client(),
            service_code,
        }
    }
}

#[async_trait]
impl AwsService for ServiceQuotasService {
    fn service_type(&self) -> ServiceType {
        ServiceType::ServiceQuotas
    }

    fn name(&self) -> &str {
        "Service Quotas"
    }

    async fn list_resources(&self) -> Result<Vec<Box<dyn Resource>>> {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        self.list_resources_streaming(tx, ServiceType::ServiceQuotas)
            .await?;
        Ok(vec![])
    }

    async fn list_resources_streaming(
        &self,
        event_tx: mpsc::UnboundedSender<Event>,
        service_type: ServiceType,
    ) -> Result<()> {
        let code = self.service_code.clone();

        // Default quotas first (join key = quota_code) so applied > default can
        // be flagged as a granted increase.
        let mut defaults: HashMap<String, f64> = HashMap::new();
        let mut next: Option<String> = None;
        loop {
            let mut req = self
                .client
                .list_aws_default_service_quotas()
                .service_code(&code);
            if let Some(t) = &next {
                req = req.next_token(t);
            }
            match req.send().await {
                Ok(resp) => {
                    for q in resp.quotas() {
                        if let (Some(qc), Some(v)) = (q.quota_code(), q.value()) {
                            defaults.insert(qc.to_string(), v);
                        }
                    }
                    next = crate::aws::pagination::next_page_token(resp.next_token(), &next);
                    if next.is_none() {
                        break;
                    }
                }
                // Defaults are a best-effort enrichment — don't fail the load.
                Err(_) => break,
            }
        }

        // Applied quotas (paginated). Collect, join defaults, sort by name,
        // emit one batch — a single service's quota count is small.
        let mut quotas: Vec<ServiceQuota> = Vec::new();
        let mut next: Option<String> = None;
        loop {
            let mut req = self.client.list_service_quotas().service_code(&code);
            if let Some(t) = &next {
                req = req.next_token(t);
            }
            match req.send().await {
                Ok(resp) => {
                    for q in resp.quotas() {
                        let mut sq = ServiceQuota::from_sdk(q);
                        sq.default_value = defaults.get(&sq.quota_code).copied();
                        quotas.push(sq);
                    }
                    next = crate::aws::pagination::next_page_token(resp.next_token(), &next);
                    if next.is_none() {
                        break;
                    }
                }
                Err(e) => {
                    let _ = event_tx.send(Event::ResourceLoadError {
                        service: service_type,
                        error: friendly_error(&code, &crate::error::sdk_error_message(&e)),
                    });
                    return Ok(());
                }
            }
        }

        quotas.sort_by(|a, b| a.quota_name.to_lowercase().cmp(&b.quota_name.to_lowercase()));
        let total = quotas.len();
        if total > 0 {
            let batch: Vec<Box<dyn Resource>> = quotas
                .into_iter()
                .map(|q| Box::new(q) as Box<dyn Resource>)
                .collect();
            let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                service: service_type,
                resources: batch,
                progress: LoadProgress {
                    loaded_count: total,
                    total_count: Some(total),
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

    async fn get_resource_details(&self, _id: &str) -> Result<Box<dyn Resource>> {
        Err(crate::error::Error::NotImplemented)
    }
}

fn friendly_error(code: &str, raw: &str) -> String {
    let low = raw.to_lowercase();
    if low.contains("expiredtoken")
        || low.contains("expired")
        || low.contains("unable to locate credentials")
        || low.contains("the security token")
    {
        "AWS credentials are expired or missing — refresh them and reload (r).".to_string()
    } else if low.contains("nosuchresource") || (low.contains("invalid") && low.contains("service"))
    {
        format!("No quotas for service code \"{}\" (try `c` to pick a valid one).", code)
    } else if low.contains("accessdenied") || low.contains("not authorized") {
        "Access denied — need servicequotas:ListServiceQuotas.".to_string()
    } else {
        format!("Failed to load quotas for \"{}\": {}", code, raw)
    }
}

/// Fetch the full list of services that have quotas (for the picker modal).
pub async fn fetch_quota_services(client: SqClient) -> Result<Vec<(String, String)>> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut next: Option<String> = None;
    loop {
        let mut req = client.list_services();
        if let Some(t) = &next {
            req = req.next_token(t);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
        for s in resp.services() {
            out.push((
                s.service_code().unwrap_or_default().to_string(),
                s.service_name().unwrap_or_default().to_string(),
            ));
        }
        next = crate::aws::pagination::next_page_token(resp.next_token(), &next);
        if next.is_none() {
            break;
        }
    }
    out.sort_by(|a, b| a.1.to_lowercase().cmp(&b.1.to_lowercase()));
    Ok(out)
}

// ── ServiceQuota ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ServiceQuota {
    pub service_code: String,
    pub service_name: String,
    pub quota_code: String,
    pub quota_name: String,
    pub value: f64,
    pub unit: String,
    pub adjustable: bool,
    pub global: bool,
    pub default_value: Option<f64>,
    pub arn: String,
    pub tags: HashMap<String, String>,
}

impl ServiceQuota {
    pub fn from_sdk(q: &aws_sdk_servicequotas::types::ServiceQuota) -> Self {
        Self {
            service_code: q.service_code().unwrap_or_default().to_string(),
            service_name: q.service_name().unwrap_or_default().to_string(),
            quota_code: q.quota_code().unwrap_or_default().to_string(),
            quota_name: q.quota_name().unwrap_or_default().to_string(),
            value: q.value().unwrap_or(0.0),
            unit: q.unit().unwrap_or_default().to_string(),
            adjustable: q.adjustable(),
            global: q.global_quota(),
            default_value: None,
            arn: q.quota_arn().unwrap_or_default().to_string(),
            tags: HashMap::new(),
        }
    }

    /// True when the applied value exceeds the AWS default — i.e. a granted
    /// quota increase.
    pub fn raised(&self) -> bool {
        self.default_value.map(|d| self.value > d).unwrap_or(false)
    }
}

/// Format a quota value without trailing `.0` for whole numbers.
fn fmt_num(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        format!("{}", v)
    }
}

impl Resource for ServiceQuota {
    fn id(&self) -> &str {
        &self.quota_code
    }
    fn name(&self) -> &str {
        &self.quota_name
    }
    fn resource_type(&self) -> &str {
        "Service Quota"
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
            self.service_code, self.service_name, self.quota_name, self.quota_code
        )
    }
    fn details(&self) -> Vec<(String, String)> {
        let unit = if self.unit.is_empty() || self.unit == "None" {
            String::new()
        } else {
            format!(" {}", self.unit)
        };
        let applied = if self.raised() {
            format!("{}{}  ↑ raised above default", fmt_num(self.value), unit)
        } else {
            format!("{}{}", fmt_num(self.value), unit)
        };
        vec![
            ("Quota".to_string(), self.quota_name.clone()),
            ("Service".to_string(), self.service_name.clone()),
            ("Applied Value".to_string(), applied),
            (
                "Default Value".to_string(),
                self.default_value.map(fmt_num).unwrap_or_else(|| "—".to_string()),
            ),
            (
                "Adjustable".to_string(),
                if self.adjustable { "Yes" } else { "No" }.to_string(),
            ),
            (
                "Global".to_string(),
                if self.global { "Yes" } else { "No" }.to_string(),
            ),
            ("Quota Code".to_string(), self.quota_code.clone()),
            ("ARN".to_string(), self.arn.clone()),
        ]
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{region}.console.aws.amazon.com/servicequotas/home/services/{}/quotas/{}",
            self.service_code, self.quota_code
        ))
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}
