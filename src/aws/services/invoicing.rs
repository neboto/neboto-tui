use crate::aws::client::AwsClients;
use crate::aws::resource::{Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::error::Result;
use crate::event::{Event, LoadProgress};
use async_trait::async_trait;
use aws_sdk_invoicing::primitives::DateTime as InvDateTime;
use aws_sdk_invoicing::Client as InvoicingClient;
use std::any::Any;
use std::collections::HashMap;
use tokio::sync::mpsc;

/// Invoices — a browse-only, single-list view of `ListInvoiceSummaries`
/// (amounts, dates, entity, PO number). Global (`us-east-1`) like Cost
/// Explorer / Budgets.
///
/// This is the closest thing to "payment details" AWS exposes over an API:
/// there is no read API for stored payment methods (card on file — that's
/// console-only, for good reason), and invoice summaries carry no
/// paid/unpaid flag either (most accounts auto-pay on the card on file), so
/// this is deliberately informational only — no fabricated "overdue" state.
///
/// `d` (detail pane) downloads the invoice PDF via a fresh presigned
/// `GetInvoicePdf` URL — the same download-a-presigned-URL pattern Lambda's
/// Code section uses for its deployment package.
///
/// `ListInvoiceSummaries` requires a `selector` — despite the SDK typing it
/// `Option`, the service rejects a request without one
/// (`ValidationException: Value at 'selector' failed to satisfy constraint:
/// Member must not be null`) — so the list load resolves the caller's
/// account id via `sts:GetCallerIdentity` (like Budgets) and selects
/// `ACCOUNT_ID = <that account>`, same as Budgets resolving its own required
/// `AccountId` param.
pub struct InvoicingService {
    client: InvoicingClient,
    sts_client: aws_sdk_sts::Client,
}

impl InvoicingService {
    pub fn new(aws_clients: &AwsClients) -> Self {
        Self {
            client: aws_clients.invoicing_client(),
            sts_client: aws_clients.sts_client(),
        }
    }
}

#[async_trait]
impl AwsService for InvoicingService {
    fn service_type(&self) -> ServiceType {
        ServiceType::Invoices
    }

    fn name(&self) -> &str {
        "Invoices"
    }

    async fn list_resources(&self) -> Result<Vec<Box<dyn Resource>>> {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        self.list_resources_streaming(tx, ServiceType::Invoices).await?;
        Ok(vec![])
    }

    async fn list_resources_streaming(
        &self,
        event_tx: mpsc::UnboundedSender<Event>,
        service_type: ServiceType,
    ) -> Result<()> {
        let account_id = match self.sts_client.get_caller_identity().send().await {
            Ok(o) => o.account().unwrap_or_default().to_string(),
            Err(e) => {
                let _ = event_tx.send(Event::ResourceLoadError {
                    service: service_type,
                    error: format!(
                        "Failed to resolve account id: {}",
                        crate::error::sdk_error_message(&e)
                    ),
                });
                return Ok(());
            }
        };
        let selector = match aws_sdk_invoicing::types::InvoiceSummariesSelector::builder()
            .resource_type(aws_sdk_invoicing::types::ListInvoiceSummariesResourceType::AccountId)
            .value(&account_id)
            .build()
        {
            Ok(s) => s,
            Err(e) => {
                let _ = event_tx.send(Event::ResourceLoadError {
                    service: service_type,
                    error: format!("Failed to build invoice selector: {}", e),
                });
                return Ok(());
            }
        };

        // Without an explicit time interval `ListInvoiceSummaries` only
        // covers the current billing period, and without `receiver_role` the
        // interval itself is capped at one month — so default to the
        // trailing 12 months (`Buyer`: the account paying for AWS usage,
        // the overwhelmingly common case for a browsed account) for a
        // useful history.
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let start = now - 365 * 24 * 3600;
        let interval = match aws_sdk_invoicing::types::DateInterval::builder()
            .start_date(InvDateTime::from_secs(start))
            .end_date(InvDateTime::from_secs(now))
            .build()
        {
            Ok(i) => i,
            Err(e) => {
                let _ = event_tx.send(Event::ResourceLoadError {
                    service: service_type,
                    error: format!("Failed to build invoice date range: {}", e),
                });
                return Ok(());
            }
        };
        let filter = aws_sdk_invoicing::types::InvoiceSummariesFilter::builder()
            .time_interval(interval)
            .receiver_role(aws_sdk_invoicing::types::ReceiverRole::Buyer)
            .build();

        let mut invoices: Vec<Invoice> = Vec::new();
        let mut next_token: Option<String> = None;
        loop {
            let mut req = self
                .client
                .list_invoice_summaries()
                .selector(selector.clone())
                .filter(filter.clone());
            if let Some(t) = &next_token {
                req = req.next_token(t);
            }
            let resp = match req.send().await {
                Ok(r) => r,
                Err(e) => {
                    let _ = event_tx.send(Event::ResourceLoadError {
                        service: service_type,
                        error: friendly_error(&crate::error::sdk_error_message(&e)),
                    });
                    return Ok(());
                }
            };
            invoices.extend(resp.invoice_summaries().iter().map(Invoice::from_sdk));
            next_token = crate::aws::pagination::next_page_token(resp.next_token(), &next_token);
            if next_token.is_none() {
                break;
            }
        }

        invoices.sort_by(|a, b| b.issued_date.cmp(&a.issued_date));

        let total = invoices.len();
        if total > 0 {
            let batch: Vec<Box<dyn Resource>> = invoices
                .into_iter()
                .map(|i| Box::new(i) as Box<dyn Resource>)
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

/// Map raw Invoicing SDK errors to actionable hints.
fn friendly_error(raw: &str) -> String {
    let low = raw.to_lowercase();
    if low.contains("accessdenied") || low.contains("access denied") || low.contains("not authorized") {
        "Access denied — need invoicing:ListInvoiceSummaries.".to_string()
    } else {
        format!("Failed to load invoices: {}", raw)
    }
}

// ── Invoice ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Invoice {
    pub invoice_id: String,
    pub account_id: String,
    pub entity: String,
    /// `"2026-06"`.
    pub billing_period: String,
    pub issued_date: Option<String>,
    pub due_date: Option<String>,
    pub invoice_type: String,
    pub bill_type: String,
    pub total_amount: Option<f64>,
    pub currency_code: String,
    pub po_number: Option<String>,
    tags: HashMap<String, String>,
}

impl Invoice {
    pub fn from_sdk(s: &aws_sdk_invoicing::types::InvoiceSummary) -> Self {
        let invoice_id = s.invoice_id().unwrap_or_default().to_string();
        let account_id = s.account_id().unwrap_or_default().to_string();
        let entity = s
            .entity()
            .and_then(|e| e.invoicing_entity())
            .unwrap_or_default()
            .to_string();
        let billing_period = s
            .billing_period()
            .map(|p| format!("{:04}-{:02}", p.year(), p.month()))
            .unwrap_or_default();
        let issued_date = s
            .issued_date()
            .map(|t| crate::aws::services::cloudwatch::fmt_epoch_secs(t.secs()));
        let due_date = s
            .due_date()
            .map(|t| crate::aws::services::cloudwatch::fmt_epoch_secs(t.secs()));
        let invoice_type = s
            .invoice_type()
            .map(|t| t.as_str().to_string())
            .unwrap_or_default();
        let bill_type = s.bill_type().map(|t| t.as_str().to_string()).unwrap_or_default();
        let (total_amount, currency_code) = s
            .base_currency_amount()
            .map(|a| {
                (
                    a.total_amount().and_then(|v| v.parse::<f64>().ok()),
                    a.currency_code().unwrap_or_default().to_string(),
                )
            })
            .unwrap_or((None, String::new()));
        let po_number = s
            .purchase_order_number()
            .filter(|p| !p.is_empty())
            .map(|p| p.to_string());

        Self {
            invoice_id,
            account_id,
            entity,
            billing_period,
            issued_date,
            due_date,
            invoice_type,
            bill_type,
            total_amount,
            currency_code,
            po_number,
            tags: HashMap::new(),
        }
    }
}

impl Resource for Invoice {
    fn id(&self) -> &str {
        &self.invoice_id
    }

    fn name(&self) -> &str {
        &self.invoice_id
    }

    fn resource_type(&self) -> &str {
        "Invoice"
    }

    fn state(&self) -> ResourceState {
        // No paid/unpaid signal from the API — deliberately neutral rather
        // than guessing "overdue" from `due_date` alone (most accounts
        // auto-pay on the card on file).
        ResourceState::Unknown(String::new())
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!(
            "{} {} {} {} {}",
            self.invoice_id,
            self.account_id,
            self.entity,
            self.billing_period,
            self.po_number.as_deref().unwrap_or(""),
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        let mut rows = vec![
            ("Invoice ID".to_string(), self.invoice_id.clone()),
            ("Account".to_string(), self.account_id.clone()),
            ("Entity".to_string(), self.entity.clone()),
            ("Billing Period".to_string(), self.billing_period.clone()),
        ];
        if let Some(d) = &self.issued_date {
            rows.push(("Issued".to_string(), d.clone()));
        }
        if let Some(d) = &self.due_date {
            rows.push(("Due".to_string(), d.clone()));
        }
        rows.push(("Type".to_string(), self.invoice_type.clone()));
        rows.push(("Bill Type".to_string(), self.bill_type.clone()));
        if let Some(a) = self.total_amount {
            rows.push((
                "Total".to_string(),
                format!("{} {}", crate::aws::services::cost::fmt_money(a), self.currency_code),
            ));
        }
        if let Some(po) = &self.po_number {
            rows.push(("PO Number".to_string(), po.clone()));
        }
        rows
    }

    fn console_url(&self, _region: &str) -> Option<String> {
        Some("https://console.aws.amazon.com/billing/home#/bills".to_string())
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn estimated_monthly_cost(&self) -> Option<f64> {
        self.total_amount
    }
}

// ── PDF download (`d`, detail pane) ────────────────────────────────────────────

/// Download the invoice PDF via a fresh presigned URL (each `GetInvoicePdf`
/// call returns a new one, so it can't be stale) to `dest_path`. Returns
/// `(dest_path, bytes_written)`.
pub async fn download_invoice_pdf(
    client: InvoicingClient,
    invoice_id: String,
    dest_path: std::path::PathBuf,
) -> Result<(std::path::PathBuf, u64)> {
    let resp = client
        .get_invoice_pdf()
        .invoice_id(&invoice_id)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
    let url = resp
        .invoice_pdf()
        .and_then(|p| p.document_url())
        .ok_or_else(|| crate::error::Error::AwsSdk("GetInvoicePdf returned no document URL".to_string()))?
        .to_string();

    // Network download is blocking — keep it off the async runtime.
    let dest_for_return = dest_path.clone();
    let bytes_written = tokio::task::spawn_blocking(move || -> std::io::Result<u64> {
        use std::io::Read;
        let io_err = std::io::Error::other;
        let resp = ureq::get(&url).call().map_err(|e| io_err(e.to_string()))?;
        let mut bytes: Vec<u8> = Vec::new();
        resp.into_reader().read_to_end(&mut bytes)?;
        if let Some(parent) = dest_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&dest_path, &bytes)?;
        Ok(bytes.len() as u64)
    })
    .await
    .map_err(|e| crate::error::Error::AwsSdk(format!("download task failed: {e}")))??;

    Ok((dest_for_return, bytes_written))
}
