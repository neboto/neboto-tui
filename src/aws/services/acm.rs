use crate::aws::client::AwsClients;
use crate::aws::resource::{native_state_label, Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::error::Result;
use crate::event::{Event, LoadProgress};
use async_trait::async_trait;
use aws_sdk_acm::Client as AcmClient;
use std::any::Any;
use std::collections::HashMap;
use tokio::sync::mpsc;

pub struct AcmService {
    client: AcmClient,
}

impl AcmService {
    pub fn new(aws_clients: &AwsClients) -> Self {
        Self {
            client: aws_clients.acm_client(),
        }
    }
}

#[async_trait]
impl AwsService for AcmService {
    fn service_type(&self) -> ServiceType {
        ServiceType::Acm
    }

    fn name(&self) -> &str {
        "ACM"
    }

    async fn list_resources(&self) -> Result<Vec<Box<dyn Resource>>> {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        self.list_resources_streaming(tx, ServiceType::Acm).await?;
        Ok(vec![])
    }

    async fn list_resources_streaming(
        &self,
        event_tx: mpsc::UnboundedSender<Event>,
        service_type: ServiceType,
    ) -> Result<()> {
        let mut total = 0usize;
        let mut paginator = self.client.list_certificates().into_paginator().send();

        while let Some(result) = paginator.next().await {
            match result {
                Ok(page) => {
                    let batch: Vec<Box<dyn Resource>> = page
                        .certificate_summary_list()
                        .iter()
                        .map(|c| {
                            Box::new(AcmCertificate::from_summary(c)) as Box<dyn Resource>
                        })
                        .collect();

                    let count = batch.len();
                    if count == 0 {
                        continue;
                    }
                    total += count;

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
                    let _ = event_tx.send(Event::ResourceLoadError {
                        service: service_type,
                        error: format!("Failed to list ACM certificates: {}", e),
                    });
                    return Ok(());
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

// ── AcmCertificate ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct AcmCertificate {
    pub certificate_arn: String,
    pub domain_name: String,
    pub status: String,
    pub cert_type: String,
    pub not_after: Option<String>,
    pub days_until_expiry: Option<i64>,
    pub tags: HashMap<String, String>,
}

impl AcmCertificate {
    pub fn from_summary(c: &aws_sdk_acm::types::CertificateSummary) -> Self {
        let status = c
            .status()
            .map(|s| s.as_str().to_string())
            .unwrap_or_default();
        let cert_type = c
            .r#type()
            .map(|t| t.as_str().to_string())
            .unwrap_or_default();

        let (not_after, days_until_expiry) = if let Some(dt) = c.not_after() {
            let epoch_secs = dt.secs();
            let formatted = fmt_epoch_secs(epoch_secs);
            let now_secs = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            let days = (epoch_secs - now_secs) / 86400;
            (Some(formatted), Some(days))
        } else {
            (None, None)
        };

        Self {
            certificate_arn: c
                .certificate_arn()
                .unwrap_or_default()
                .to_string(),
            domain_name: c.domain_name().unwrap_or_default().to_string(),
            status,
            cert_type,
            not_after,
            days_until_expiry,
            tags: HashMap::new(),
        }
    }
}

crate::sections! {
    pub enum AcmCertDetailSection,
    pub static ACM_CERT_SECTIONS = [
        Details "Details" => crate::app::App::trigger_acm_details_load,
        Domains "Domains" => crate::app::App::trigger_acm_details_load,
        Validation "Validation" => crate::app::App::trigger_acm_details_load,
        Tags "Tags" => crate::app::App::trigger_acm_details_load,
    ]
}

impl Resource for AcmCertificate {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&ACM_CERT_SECTIONS)
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws acm describe-certificate --certificate-arn {}",
            crate::aws::resource::shell_quote(self.id())
        ))
    }

    fn id(&self) -> &str {
        &self.certificate_arn
    }

    fn name(&self) -> &str {
        &self.domain_name
    }

    fn resource_type(&self) -> &str {
        "ACM Certificate"
    }

    fn state(&self) -> ResourceState {
        match self.status.as_str() {
            "EXPIRED" | "FAILED" => ResourceState::Unavailable,
            "PENDING_VALIDATION" => ResourceState::Pending,
            "REVOKED" => ResourceState::Terminated,
            "ISSUED" => match self.days_until_expiry {
                Some(d) if d < 0 => ResourceState::Unavailable,
                Some(d) if d < 30 => ResourceState::Deleting,
                Some(d) if d < 90 => ResourceState::Creating,
                _ => ResourceState::Available,
            },
            other => ResourceState::Unknown(other.to_string()),
        }
    }

    fn state_label(&self) -> String {
        if self.status == "ISSUED" {
            match self.days_until_expiry {
                Some(d) if d < 0 => return "expired".to_string(),
                Some(d) if d < 30 => return "expires <30d".to_string(),
                Some(d) if d < 90 => return "expires <90d".to_string(),
                _ => {}
            }
        }
        native_state_label(&self.status, || self.state())
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!(
            "{} {} {} {}",
            self.certificate_arn,
            self.domain_name,
            self.status,
            self.cert_type
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("ARN".to_string(), self.certificate_arn.clone()),
            ("Domain".to_string(), self.domain_name.clone()),
            ("Status".to_string(), self.status.clone()),
            ("Type".to_string(), self.cert_type.clone()),
            (
                "Expires".to_string(),
                self.not_after.clone().unwrap_or_else(|| "—".to_string()),
            ),
        ]
    }

    fn console_url(&self, _region: &str) -> Option<String> {
        // Extract region from ARN: arn:aws:acm:<region>:<account>:certificate/<id>
        let parts: Vec<&str> = self.certificate_arn.split(':').collect();
        let region = parts.get(3).copied().unwrap_or("us-east-1");
        let cert_id = self
            .certificate_arn
            .rsplit('/')
            .next()
            .unwrap_or_default();
        Some(format!(
            "https://{}.console.aws.amazon.com/acm/home?region={}#/certificates/{}",
            region, region, cert_id
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── AcmCertDetails (lazy-loaded via describe_certificate) ─────────────────────

#[derive(Debug, Clone)]
pub struct AcmCertDetails {
    pub subject_alternative_names: Vec<String>,
    pub key_algorithm: String,
    pub renewal_eligibility: String,
    pub in_use_by: Vec<String>,
    pub issued_at: Option<String>,
    pub not_before: Option<String>,
    pub validation_options: Vec<AcmDomainValidation>,
    pub tags: HashMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct AcmDomainValidation {
    pub domain_name: String,
    pub validation_method: String,
    pub validation_status: String,
    pub resource_record: Option<AcmResourceRecord>,
}

#[derive(Debug, Clone)]
pub struct AcmResourceRecord {
    pub name: String,
    pub record_type: String,
    pub value: String,
}

pub async fn fetch_cert_details(client: AcmClient, arn: String) -> Result<AcmCertDetails> {
    let resp = client
        .describe_certificate()
        .certificate_arn(&arn)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;

    let cert = resp
        .certificate()
        .ok_or_else(|| crate::error::Error::AwsSdk("No certificate in response".to_string()))?;

    let subject_alternative_names = cert
        .subject_alternative_names()
        .iter()
        .map(|s| s.to_string())
        .collect();

    let key_algorithm = cert
        .key_algorithm()
        .map(|k| k.as_str().to_string())
        .unwrap_or_default();

    let renewal_eligibility = cert
        .renewal_eligibility()
        .map(|r| r.as_str().to_string())
        .unwrap_or_default();

    let in_use_by = cert.in_use_by().iter().map(|s| s.to_string()).collect();

    let issued_at = cert.issued_at().map(|d| fmt_epoch_secs(d.secs()));
    let not_before = cert.not_before().map(|d| fmt_epoch_secs(d.secs()));

    let validation_options = cert
        .domain_validation_options()
        .iter()
        .map(|opt| AcmDomainValidation {
            domain_name: opt.domain_name().to_string(),
            validation_method: opt
                .validation_method()
                .map(|m| m.as_str().to_string())
                .unwrap_or_default(),
            validation_status: opt
                .validation_status()
                .map(|s| s.as_str().to_string())
                .unwrap_or_default(),
            resource_record: opt.resource_record().map(|r| AcmResourceRecord {
                name: r.name().to_string(),
                record_type: r.r#type().as_str().to_string(),
                value: r.value().to_string(),
            }),
        })
        .collect();

    // Fetch tags
    let tags = client
        .list_tags_for_certificate()
        .certificate_arn(&arn)
        .send()
        .await
        .map(|r| {
            r.tags()
                .iter()
                .map(|t| {
                    (
                        t.key().to_string(),
                        t.value().unwrap_or_default().to_string(),
                    )
                })
                .collect::<HashMap<_, _>>()
        })
        .unwrap_or_default();

    Ok(AcmCertDetails {
        subject_alternative_names,
        key_algorithm,
        renewal_eligibility,
        in_use_by,
        issued_at,
        not_before,
        validation_options,
        tags,
    })
}

// ── Timestamp helpers ─────────────────────────────────────────────────────────

fn fmt_epoch_secs(secs: i64) -> String {
    let days = secs / 86400;
    let time_rem = secs % 86400;
    let h = time_rem / 3600;
    let m = (time_rem % 3600) / 60;
    let (y, mo, d) = epoch_days_to_ymd(days);
    format!("{:04}-{:02}-{:02} {:02}:{:02} UTC", y, mo, d, h, m)
}

fn epoch_days_to_ymd(mut days: i64) -> (i32, u8, u8) {
    let mut year = 1970i32;
    loop {
        let dy = if is_leap(year) { 366 } else { 365 };
        if days < dy {
            break;
        }
        days -= dy;
        year += 1;
    }
    let dm = [
        31u8,
        if is_leap(year) { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut month = 1u8;
    for &d in &dm {
        if days < d as i64 {
            break;
        }
        days -= d as i64;
        month += 1;
    }
    (year, month, (days + 1) as u8)
}

fn is_leap(y: i32) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}
