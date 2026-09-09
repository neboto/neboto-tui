use crate::aws::client::AwsClients;
use crate::aws::resource::{Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::error::Result;
use crate::event::{Event, LoadProgress};
use async_trait::async_trait;
use aws_sdk_resourceexplorer2::config::Region;
use aws_sdk_resourceexplorer2::types::IndexType;
use aws_sdk_resourceexplorer2::Client;
use std::any::Any;
use std::collections::HashMap;
use tokio::sync::mpsc;

/// Cap on `Search` pages for the broad `"*"` query. Busy accounts can return
/// thousands of rows; the index is server-side and cheap to page, but we bound
/// the load for v1 (each page is up to 1000 results).
const MAX_SEARCH_PAGES: usize = 20;

/// One cross-region search result from AWS Resource Explorer. The payoff columns
/// are `service` and `region` — Resource Explorer is the only neboto view that
/// surfaces resources living in regions other than the selected one.
#[derive(Debug, Clone)]
pub struct ReResult {
    pub arn: String,
    /// Resource Explorer's own type token, e.g. "ec2:instance", "s3:bucket".
    pub resource_type: String,
    /// Owning service token, e.g. "ec2", "s3".
    pub service: String,
    /// The region the resource lives in (the cross-region payoff).
    pub region: String,
    pub owning_account_id: String,
    pub last_reported_at: Option<String>,
    /// Flattened property names/values for fuzzy search (tag values, names, etc.)
    pub properties_text: String,
    /// Always empty — Resource Explorer search results carry no tags. The trait
    /// requires a `&HashMap`, so we hold one.
    pub tags: HashMap<String, String>,
}

impl ReResult {
    fn from_sdk(r: &aws_sdk_resourceexplorer2::types::Resource) -> Self {
        // Extract searchable text from resource properties (tag values, names)
        let properties_text = r
            .properties()
            .iter()
            .filter_map(|p| {
                let data = p.data()?;
                // Serialize the whole document to capture all searchable values
                Some(format!("{:?}", data))
            })
            .collect::<Vec<_>>()
            .join(" ");

        Self {
            arn: r.arn().unwrap_or_default().to_string(),
            resource_type: r.resource_type().unwrap_or_default().to_string(),
            service: r.service().unwrap_or_default().to_string(),
            region: r.region().unwrap_or_default().to_string(),
            owning_account_id: r.owning_account_id().unwrap_or_default().to_string(),
            last_reported_at: r.last_reported_at().map(|d| {
                d.fmt(aws_smithy_types::date_time::Format::DateTime)
                    .unwrap_or_default()
            }),
            properties_text,
            tags: HashMap::new(),
        }
    }

    /// Display name = the last meaningful ARN segment (instance id, bucket name,
    /// function name, …). Falls back to the whole ARN.
    fn display_name(&self) -> &str {
        self.arn
            .rsplit(['/', ':'])
            .find(|s| !s.is_empty())
            .unwrap_or(&self.arn)
    }
}

pub struct ResourceExplorerService {
    /// The base SDK config — clients are built per call, pinned to the
    /// aggregator-index region discovered at runtime.
    config: aws_config::SdkConfig,
}

impl ResourceExplorerService {
    pub fn new(aws_clients: &AwsClients) -> Self {
        Self {
            config: aws_clients.sdk_config().clone(),
        }
    }
}

#[async_trait]
impl AwsService for ResourceExplorerService {
    fn service_type(&self) -> ServiceType {
        ServiceType::ResourceExplorer
    }

    fn name(&self) -> &str {
        "Resource Explorer"
    }

    async fn list_resources(&self) -> Result<Vec<Box<dyn Resource>>> {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        self.list_resources_streaming(tx, ServiceType::ResourceExplorer)
            .await?;
        Ok(vec![])
    }

    async fn list_resources_streaming(
        &self,
        event_tx: mpsc::UnboundedSender<Event>,
        service_type: ServiceType,
    ) -> Result<()> {
        // Phase 1 — discover the aggregator index. It lives in one region but
        // indexes every region in the account; that region is where we must run
        // `Search`. No aggregator → Resource Explorer isn't set up for
        // cross-region search → friendly gate.
        let bootstrap = Client::new(&self.config);
        let agg_region = match bootstrap.list_indexes().r#type(IndexType::Aggregator).send().await {
            Ok(resp) => {
                resp.indexes().iter().find_map(index_region)
            }
            Err(_) => None,
        };

        // If no aggregator, try finding any local index
        let agg_region = match agg_region {
            Some(r) => Some(r),
            None => match bootstrap.list_indexes().send().await {
                Ok(resp) => resp.indexes().iter().find_map(index_region),
                Err(e) => {
                    let _ = event_tx.send(Event::ResourceLoadError {
                        service: service_type,
                        error: friendly_error(&crate::error::sdk_error_message(&e)),
                    });
                    return Ok(());
                }
            },
        };
        let agg_region = match agg_region {
            Some(r) => r,
            None => {
                let _ = event_tx.send(Event::ResourceLoadError {
                    service: service_type,
                    error: not_set_up_message(),
                });
                return Ok(());
            }
        };

        // Phase 2 — run the broad search. Try the user's current region first
        // (the console uses it), then fall back to the discovered index region.
        // SCPs may block access in some regions but allow it in the user's own.
        let current_region = self
            .config
            .region()
            .map(|r| r.to_string())
            .unwrap_or_else(|| agg_region.clone());

        let regions_to_try = if current_region == agg_region {
            vec![agg_region]
        } else {
            vec![current_region, agg_region]
        };

        let mut client = None;
        for region in &regions_to_try {
            let cfg = aws_sdk_resourceexplorer2::config::Builder::from(&self.config)
                .region(Region::new(region.clone()))
                .build();
            let c = Client::from_conf(cfg);
            // Test with a minimal search
            match c.search().query_string("*").max_results(1).send().await {
                Ok(_) => {
                    client = Some(c);
                    break;
                }
                Err(_) => continue,
            }
        }

        let client = match client {
            Some(c) => c,
            None => {
                let _ = event_tx.send(Event::ResourceLoadError {
                    service: service_type,
                    error: "Resource Explorer Search blocked in all available regions (check SCPs).".to_string(),
                });
                return Ok(());
            }
        };

        let mut token: Option<String> = None;
        let mut total = 0usize;
        for _ in 0..MAX_SEARCH_PAGES {
            let mut req = client.search().query_string("*");
            if let Some(t) = &token {
                req = req.next_token(t.clone());
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

            let batch: Vec<Box<dyn Resource>> = resp
                .resources()
                .iter()
                .map(|r| Box::new(ReResult::from_sdk(r)) as Box<dyn Resource>)
                .collect();

            if !batch.is_empty() {
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

            // RE's `next_token` is hand-rolled — advance via the shared helper so
            // an empty / non-advancing token stops the loop (the GuardDuty hang).
            token = crate::aws::pagination::next_page_token(resp.next_token(), &token);
            if token.is_none() {
                break;
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

/// The region an index lives in: prefer its explicit `region`, else parse the
/// 4th `:`-segment of the ARN (`arn:aws:resource-explorer-2:REGION:acct:...`).
fn index_region(idx: &aws_sdk_resourceexplorer2::types::Index) -> Option<String> {
    if let Some(r) = idx.region().filter(|s| !s.is_empty()) {
        return Some(r.to_string());
    }
    idx.arn()
        .and_then(|arn| arn.split(':').nth(3))
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

fn not_set_up_message() -> String {
    "AWS Resource Explorer isn't set up. Enable it with an aggregator index and a default view in the Resource Explorer console, then this cross-region view works.".to_string()
}

/// Map raw RE errors to an actionable hint — the dominant cases are
/// "not turned on" (no index/view) and access-denied. Everything else (incl.
/// not-found / no-default-view) falls through to the set-up hint; we never bubble
/// a raw SDK error.
fn friendly_error(raw: &str) -> String {
    let low = raw.to_lowercase();
    if low.contains("accessdenied")
        || low.contains("access denied")
        || low.contains("not authorized")
        || low.contains("unauthorized")
    {
        format!("Access denied — need resource-explorer-2:ListIndexes and resource-explorer-2:Search permissions. ({})", raw)
    } else {
        format!("{} ({})", not_set_up_message(), raw)
    }
}

impl Resource for ReResult {
    fn id(&self) -> &str {
        &self.arn
    }

    fn name(&self) -> &str {
        self.display_name()
    }

    fn resource_type(&self) -> &str {
        "Resource Explorer Result"
    }

    fn state(&self) -> ResourceState {
        // RE reports no lifecycle state — the dot is neutral; region/service
        // carry the signal.
        ResourceState::Unknown(String::new())
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!(
            "{} {} {} {} {} {}",
            self.arn, self.resource_type, self.service, self.region, self.owning_account_id,
            self.properties_text,
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        let mut rows = vec![
            ("ARN".to_string(), self.arn.clone()),
            ("Type".to_string(), self.resource_type.clone()),
            ("Service".to_string(), self.service.clone()),
            // The cross-region payoff — keep it prominent.
            ("Region".to_string(), self.region.clone()),
            ("Owning Account".to_string(), self.owning_account_id.clone()),
        ];
        if let Some(ts) = &self.last_reported_at {
            rows.push(("Last Reported".to_string(), ts.clone()));
        }
        rows
    }

    fn console_url(&self, _region: &str) -> Option<String> {
        // The result's native console is reachable via the ARN jump (Enter);
        // the flat link points at the Resource Explorer search page.
        Some("https://console.aws.amazon.com/resource-explorer/home#/search".to_string())
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

    fn mk(arn: &str, region: &str) -> ReResult {
        ReResult {
            arn: arn.to_string(),
            resource_type: "ec2:instance".to_string(),
            service: "ec2".to_string(),
            region: region.to_string(),
            owning_account_id: "123456789012".to_string(),
            last_reported_at: None,
            tags: HashMap::new(),
            properties_text: String::new(),
        }
    }

    #[test]
    fn display_name_takes_last_arn_segment() {
        let r = mk(
            "arn:aws:ec2:us-west-2:123456789012:instance/i-0abc123",
            "us-west-2",
        );
        assert_eq!(r.name(), "i-0abc123");

        let bucket = ReResult {
            service: "s3".to_string(),
            resource_type: "s3:bucket".to_string(),
            ..mk("arn:aws:s3:::my-bucket", "us-east-1")
        };
        assert_eq!(bucket.name(), "my-bucket");
    }

    #[test]
    fn details_keeps_region_row() {
        let r = mk("arn:aws:ec2:eu-west-1:123:instance/i-1", "eu-west-1");
        let details = r.details();
        assert!(details
            .iter()
            .any(|(k, v)| k == "Region" && v == "eu-west-1"));
    }

    #[test]
    fn friendly_error_gates_not_set_up_and_denied() {
        assert!(friendly_error("AccessDeniedException").contains("Access denied"));
        assert!(friendly_error("UnauthorizedException").contains("Access denied"));
        assert!(friendly_error("ResourceNotFoundException: no default view")
            .contains("isn't set up"));
        // Unknown errors fall through to the set-up hint (never a raw bubble).
        assert!(friendly_error("something weird").contains("isn't set up"));
    }
}
