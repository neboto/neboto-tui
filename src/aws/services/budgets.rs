use crate::aws::client::AwsClients;
use crate::aws::resource::{Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::error::Result;
use crate::event::{Event, LoadProgress};
use async_trait::async_trait;
use aws_sdk_budgets::types::Expression;
use aws_sdk_budgets::Client as BudgetsClient;
use std::any::Any;
use std::collections::HashMap;
use tokio::sync::mpsc;

/// AWS Budgets — single-list of cost/usage/RI/Savings-Plans budgets with a
/// split detail pane (Overview / Filters / Notifications / Tags).
///
/// A global (`us-east-1`) service like Cost Explorer. `DescribeBudgets`
/// requires the caller's account id explicitly (budgets are designed for a
/// payer account to manage budgets on behalf of linked accounts), so the
/// list load resolves it once via `sts:GetCallerIdentity` rather than
/// depending on `App.account_id` having already resolved — that field is
/// set asynchronously and may not be ready by the time this service's first
/// load fires.
pub struct BudgetsService {
    client: BudgetsClient,
    sts_client: aws_sdk_sts::Client,
}

impl BudgetsService {
    pub fn new(aws_clients: &AwsClients) -> Self {
        Self {
            client: aws_clients.budgets_client(),
            sts_client: aws_clients.sts_client(),
        }
    }
}

#[async_trait]
impl AwsService for BudgetsService {
    fn service_type(&self) -> ServiceType {
        ServiceType::Budgets
    }

    fn name(&self) -> &str {
        "Budgets"
    }

    async fn list_resources(&self) -> Result<Vec<Box<dyn Resource>>> {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        self.list_resources_streaming(tx, ServiceType::Budgets).await?;
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

        let mut pager = self
            .client
            .describe_budgets()
            .account_id(&account_id)
            .into_paginator()
            .items()
            .send();

        let mut budgets: Vec<BudgetItem> = Vec::new();
        while let Some(item) = pager.next().await {
            match item {
                Ok(b) => budgets.push(BudgetItem::from_sdk(&account_id, &b)),
                Err(e) => {
                    let _ = event_tx.send(Event::ResourceLoadError {
                        service: service_type,
                        error: friendly_error(&crate::error::sdk_error_message(&e)),
                    });
                    return Ok(());
                }
            }
        }

        let total = budgets.len();
        if total > 0 {
            let batch: Vec<Box<dyn Resource>> = budgets
                .into_iter()
                .map(|b| Box::new(b) as Box<dyn Resource>)
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

/// Map raw Budgets SDK errors to actionable hints.
fn friendly_error(raw: &str) -> String {
    let low = raw.to_lowercase();
    if low.contains("accessdenied") || low.contains("access denied") || low.contains("not authorized") {
        "Access denied — need budgets:ViewBudget (or the granular DescribeBudgets action)."
            .to_string()
    } else {
        format!("Failed to load budgets: {}", raw)
    }
}

// ── BudgetItem ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct BudgetItem {
    pub account_id: String,
    pub name: String,
    pub budget_type: String,
    pub time_unit: String,
    pub period_start: Option<String>,
    pub period_end: Option<String>,
    pub limit_amount: Option<f64>,
    pub limit_unit: String,
    pub actual_amount: Option<f64>,
    pub actual_unit: String,
    pub forecasted_amount: Option<f64>,
    pub last_updated: Option<String>,
    pub auto_adjust: Option<String>,
    /// `filter_expression` pretty-printed (preferred), falling back to the
    /// deprecated `cost_filters` map when a budget predates the expression
    /// field.
    pub filter_rows: Vec<(String, String)>,
    tags: HashMap<String, String>,
}

fn parse_spend(s: Option<&aws_sdk_budgets::types::Spend>) -> (Option<f64>, String) {
    match s {
        Some(spend) => (spend.amount().parse::<f64>().ok(), spend.unit().to_string()),
        None => (None, String::new()),
    }
}

/// Flatten `filter_expression`'s recursive AND/OR/NOT tree, falling back to
/// the deprecated flat `cost_filters` map for older budgets that predate it.
fn flatten_filters(
    cost_filters: Option<&HashMap<String, Vec<String>>>,
    expr: Option<&Expression>,
) -> Vec<(String, String)> {
    let mut rows = Vec::new();
    match expr {
        Some(e) => flatten_expression(e, 0, &mut rows),
        None => {
            if let Some(cf) = cost_filters {
                for (k, vals) in cf {
                    if vals.is_empty() {
                        continue;
                    }
                    rows.push((k.clone(), vals.join(", ")));
                }
            }
        }
    }
    rows
}

fn flatten_expression(expr: &Expression, depth: usize, rows: &mut Vec<(String, String)>) {
    let indent = "  ".repeat(depth);
    if !expr.and().is_empty() {
        rows.push((format!("{}AND", indent), String::new()));
        for e in expr.and() {
            flatten_expression(e, depth + 1, rows);
        }
    }
    if !expr.or().is_empty() {
        rows.push((format!("{}OR", indent), String::new()));
        for e in expr.or() {
            flatten_expression(e, depth + 1, rows);
        }
    }
    if let Some(not) = expr.not() {
        rows.push((format!("{}NOT", indent), String::new()));
        flatten_expression(not, depth + 1, rows);
    }
    if let Some(dim) = expr.dimensions() {
        rows.push((
            format!("{}{}", indent, dim.key().as_str()),
            dim.values().join(", "),
        ));
    }
    if let Some(tags) = expr.tags() {
        rows.push((
            format!("{}Tag: {}", indent, tags.key().unwrap_or_default()),
            tags.values().join(", "),
        ));
    }
    if let Some(cc) = expr.cost_categories() {
        rows.push((
            format!("{}Cost Category: {}", indent, cc.key().unwrap_or_default()),
            cc.values().join(", "),
        ));
    }
}

impl BudgetItem {
    pub fn from_sdk(account_id: &str, b: &aws_sdk_budgets::types::Budget) -> Self {
        let name = b.budget_name().to_string();
        let budget_type = b.budget_type().as_str().to_string();
        let time_unit = b.time_unit().as_str().to_string();

        let (period_start, period_end) = b
            .time_period()
            .map(|p| {
                (
                    p.start().map(|t| crate::aws::services::cloudwatch::fmt_epoch_secs(t.secs())),
                    p.end().map(|t| crate::aws::services::cloudwatch::fmt_epoch_secs(t.secs())),
                )
            })
            .unwrap_or((None, None));

        let (limit_amount, limit_unit) = parse_spend(b.budget_limit());
        let (actual_amount, actual_unit) =
            parse_spend(b.calculated_spend().and_then(|c| c.actual_spend()));
        let (forecasted_amount, _) =
            parse_spend(b.calculated_spend().and_then(|c| c.forecasted_spend()));

        let last_updated = b
            .last_updated_time()
            .map(|t| crate::aws::services::cloudwatch::fmt_epoch_secs(t.secs()));

        let auto_adjust = b.auto_adjust_data().map(|a| {
            let kind = a.auto_adjust_type().as_str();
            match a.historical_options().map(|h| h.budget_adjustment_period()) {
                Some(days) => format!("{} (last {} periods)", kind, days),
                None => kind.to_string(),
            }
        });

        // `cost_filters` is deprecated in favor of `filter_expression`, but
        // still populated for budgets created before that field existed —
        // `flatten_filters` prefers the expression and only falls back to it.
        #[allow(deprecated)]
        let filter_rows = flatten_filters(b.cost_filters(), b.filter_expression());

        Self {
            account_id: account_id.to_string(),
            name,
            budget_type,
            time_unit,
            period_start,
            period_end,
            limit_amount,
            limit_unit,
            actual_amount,
            actual_unit,
            forecasted_amount,
            last_updated,
            auto_adjust,
            filter_rows,
            tags: HashMap::new(),
        }
    }

    /// Percent of the limit currently spent (None without a positive limit).
    pub fn pct_used(&self) -> Option<f64> {
        match (self.actual_amount, self.limit_amount) {
            (Some(a), Some(l)) if l > 0.0 => Some(a / l * 100.0),
            _ => None,
        }
    }

    /// Percent of the limit the budget is forecasted to reach by period end.
    pub fn pct_forecasted(&self) -> Option<f64> {
        match (self.forecasted_amount, self.limit_amount) {
            (Some(f), Some(l)) if l > 0.0 => Some(f / l * 100.0),
            _ => None,
        }
    }
}

crate::sections! {
    pub enum BudgetDetailSection,
    pub static BUDGET_SECTIONS = [
        Overview "Overview",
        Filters "Filters",
        Notifications "Notifications" => crate::app::App::trigger_budget_notifications_load,
        Tags "Tags" => crate::app::App::trigger_budget_tags_load,
    ]
}

impl Resource for BudgetItem {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&BUDGET_SECTIONS)
    }

    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws budgets describe-budget --account-id {} --budget-name {}",
            crate::aws::resource::shell_quote(&self.account_id),
            crate::aws::resource::shell_quote(&self.name),
        ))
    }

    fn id(&self) -> &str {
        &self.name
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn resource_type(&self) -> &str {
        "Budget"
    }

    fn state(&self) -> ResourceState {
        let Some(limit) = self.limit_amount.filter(|l| *l > 0.0) else {
            return ResourceState::Unknown(String::new());
        };
        let Some(actual) = self.actual_amount else {
            return ResourceState::Unknown(String::new());
        };
        if actual >= limit {
            ResourceState::Unavailable
        } else if self.forecasted_amount.map(|f| f >= limit).unwrap_or(false) {
            ResourceState::Pending
        } else {
            ResourceState::Available
        }
    }

    fn state_label(&self) -> String {
        match self.state() {
            ResourceState::Unavailable => "over budget".to_string(),
            ResourceState::Pending => "forecast over".to_string(),
            ResourceState::Available => "under budget".to_string(),
            other => other.to_string(),
        }
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!("{} {} {}", self.name, self.budget_type, self.time_unit)
    }

    fn details(&self) -> Vec<(String, String)> {
        let mut rows = vec![
            ("Name".to_string(), self.name.clone()),
            ("Type".to_string(), self.budget_type.clone()),
            ("Time Unit".to_string(), self.time_unit.clone()),
        ];
        if let Some(l) = self.limit_amount {
            rows.push((
                "Limit".to_string(),
                format!("{} {}", crate::aws::services::cost::fmt_money(l), self.limit_unit),
            ));
        }
        if let Some(a) = self.actual_amount {
            rows.push((
                "Actual".to_string(),
                format!("{} {}", crate::aws::services::cost::fmt_money(a), self.actual_unit),
            ));
        }
        if let Some(pct) = self.pct_used() {
            rows.push(("% Used".to_string(), format!("{:.0}%", pct)));
        }
        rows
    }

    fn console_url(&self, _region: &str) -> Option<String> {
        Some("https://console.aws.amazon.com/billing/home#/budgets".to_string())
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn estimated_monthly_cost(&self) -> Option<f64> {
        self.actual_amount
    }
}

// ── Notifications (lazy: DescribeNotificationsForBudget + subscribers) ────────

#[derive(Debug, Clone)]
pub struct BudgetNotification {
    pub notification_type: String, // ACTUAL / FORECASTED
    pub comparison_operator: String,
    pub threshold: f64,
    pub threshold_type: Option<String>, // PERCENTAGE / ABSOLUTE_VALUE
    pub alarm: bool,
    /// (subscription type, address) — email or SNS topic ARN.
    pub subscribers: Vec<(String, String)>,
}

pub async fn fetch_budget_notifications(
    client: BudgetsClient,
    account_id: String,
    budget_name: String,
) -> Result<Vec<BudgetNotification>> {
    let mut notifications = Vec::new();
    let mut next_token: Option<String> = None;
    loop {
        let mut req = client
            .describe_notifications_for_budget()
            .account_id(&account_id)
            .budget_name(&budget_name);
        if let Some(t) = &next_token {
            req = req.next_token(t);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
        notifications.extend(resp.notifications().to_vec());
        next_token = crate::aws::pagination::next_page_token(resp.next_token(), &next_token);
        if next_token.is_none() {
            break;
        }
    }

    let mut out = Vec::with_capacity(notifications.len());
    for n in notifications {
        let mut subscribers = Vec::new();
        let mut next_token: Option<String> = None;
        loop {
            let mut req = client
                .describe_subscribers_for_notification()
                .account_id(&account_id)
                .budget_name(&budget_name)
                .notification(n.clone());
            if let Some(t) = &next_token {
                req = req.next_token(t);
            }
            let resp = req
                .send()
                .await
                .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
            for s in resp.subscribers() {
                subscribers.push((
                    s.subscription_type().as_str().to_string(),
                    s.address().to_string(),
                ));
            }
            next_token = crate::aws::pagination::next_page_token(resp.next_token(), &next_token);
            if next_token.is_none() {
                break;
            }
        }

        out.push(BudgetNotification {
            notification_type: n.notification_type().as_str().to_string(),
            comparison_operator: n.comparison_operator().as_str().to_string(),
            threshold: n.threshold(),
            threshold_type: n.threshold_type().map(|t| t.as_str().to_string()),
            alarm: n
                .notification_state()
                .map(|s| s.as_str() == "ALARM")
                .unwrap_or(false),
            subscribers,
        });
    }

    Ok(out)
}

// ── Tags (lazy: ListTagsForResource) ───────────────────────────────────────────

/// `ListTagsForResource` on the budget ARN, built by the caller from the
/// budget's own `account_id` + name (`arn:aws:budgets::{account}:budget/{name}`
/// — a global, region-less ARN like IAM's).
pub async fn fetch_budget_tags(
    client: BudgetsClient,
    resource_arn: String,
) -> Result<Vec<(String, String)>> {
    let resp = client
        .list_tags_for_resource()
        .resource_arn(&resource_arn)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
    Ok(resp
        .resource_tags()
        .iter()
        .map(|t| (t.key().to_string(), t.value().to_string()))
        .collect())
}
