//! List-time tags.
//!
//! **Failed per-resource tag calls (#28).** Services that fill `tags` during
//! the list load fan out one tag call per resource. Mapping a failure to an
//! empty map makes a permission gap or throttle read as "No tags" —
//! indistinguishable from an untagged resource. [`take_tags`] keeps the load
//! best-effort but sends one `ResourceLoadWarning`, so the `Partial load —`
//! line says what happened.
//!
//! **Tags from the Tagging API (#29).** Most services' tag APIs are one call
//! per resource, so their rows had no tags until a pane was opened — or
//! never, for services with no tag call at all — and the ownership ribbon,
//! `tag:` filters, `U` and exports saw nothing. [`ListTimeTags`] wraps such a
//! service: one paginated `tag:GetResources` (filtered to the service's
//! resource types) runs alongside the list load, and its result goes out as
//! `Event::ListTagsLoaded` after the last batch and before
//! `ResourcesFullyLoaded`, so the first rows still paint at list speed and the
//! cache stamps rows that already carry their tags. A row opts in with
//! `Resource::list_tags_slot`. Untagged resources are simply absent from the
//! response — that is "no tags", not an error.

use crate::aws::resource::Resource;
use crate::aws::service::{AwsService, ServiceType};
use crate::event::Event;
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;

type Tags = HashMap<String, String>;

/// `tag:GetResources` results for one load: ARN → tags, plus the ARN's
/// resource part (`job/NAME`) → ARN for row types that carry no ARN.
#[derive(Debug, Default)]
pub struct TagIndex {
    by_arn: HashMap<String, Tags>,
    by_resource: HashMap<String, String>,
}

impl TagIndex {
    pub fn insert(&mut self, arn: &str, tags: Tags) {
        if let Some(part) = arn.splitn(6, ':').nth(5) {
            self.by_resource.insert(part.to_string(), arn.to_string());
        }
        self.by_arn.insert(arn.to_string(), tags);
    }

    /// Tags for a row's `list_tags_slot` key: a full ARN, or a resource part.
    pub fn get(&self, key: &str) -> Option<&Tags> {
        self.by_arn
            .get(key)
            .or_else(|| self.by_resource.get(key).and_then(|arn| self.by_arn.get(arn)))
    }

    /// Fill every opted-in row this index has tags for. Returns how many
    /// rows changed.
    pub fn apply(&self, rows: &mut [Box<dyn Resource>]) -> usize {
        let mut n = 0;
        for row in rows.iter_mut() {
            if let Some((key, slot)) = row.list_tags_slot() {
                if let Some(tags) = self.get(&key) {
                    *slot = tags.clone();
                    n += 1;
                }
            }
        }
        n
    }
}

/// Every tagged resource of `types` (`"cloudwatch"`, `"logs"` — a service
/// prefix, optionally `:resource-type`) in the client's region.
pub async fn fetch_tag_index(
    client: aws_sdk_resourcegroupstagging::Client,
    types: &[&str],
) -> Result<TagIndex, String> {
    let mut req = client.get_resources().resources_per_page(100);
    for t in types {
        req = req.resource_type_filters(*t);
    }
    let mut index = TagIndex::default();
    let mut pages = req.into_paginator().send();
    while let Some(page) = pages.next().await {
        let page = page.map_err(|e| crate::error::sdk_error_message(&e))?;
        for mapping in page.resource_tag_mapping_list() {
            let Some(arn) = mapping.resource_arn() else { continue };
            let tags = mapping
                .tags()
                .iter()
                .map(|t| (t.key().to_string(), t.value().to_string()))
                .collect();
            index.insert(arn, tags);
        }
    }
    Ok(index)
}

/// A service whose rows get their tags at list time from the Tagging API.
/// Everything but the list load passes straight through.
pub struct ListTimeTags {
    inner: Arc<dyn AwsService>,
    client: aws_sdk_resourcegroupstagging::Client,
    types: &'static [&'static str],
    /// Set once `tag:GetResources` is denied: it stays denied for these
    /// credentials (the service is rebuilt on a profile/role switch), so
    /// later loads skip the call instead of warning on every refresh.
    denied: AtomicBool,
}

impl ListTimeTags {
    pub fn wrap(
        inner: Arc<dyn AwsService>,
        aws_clients: &crate::aws::client::AwsClients,
        types: &'static [&'static str],
    ) -> Arc<dyn AwsService> {
        Arc::new(Self {
            inner,
            client: aws_clients.tagging_client(),
            types,
            denied: AtomicBool::new(false),
        })
    }

    /// The tag fetch, started before the list so the two run side by side.
    /// `None` once the call has been denied.
    fn spawn_fetch(&self) -> Option<tokio::task::JoinHandle<Result<TagIndex, String>>> {
        if self.denied.load(Ordering::Relaxed) {
            return None;
        }
        Some(tokio::spawn(fetch_tag_index(self.client.clone(), self.types)))
    }

    /// Await the fetch. A failure warns once per load (and, when it was a
    /// denial, never again for this service), never fails the list.
    async fn finish_fetch(
        &self,
        fetch: Option<tokio::task::JoinHandle<Result<TagIndex, String>>>,
        service: ServiceType,
        event_tx: &mpsc::UnboundedSender<Event>,
    ) -> Option<TagIndex> {
        let result = match fetch?.await {
            Ok(r) => r,
            Err(e) => Err(format!("tag fetch failed: {e}")),
        };
        match result {
            Ok(index) => Some(index),
            Err(e) => {
                if e.contains("AccessDenied") {
                    self.denied.store(true, Ordering::Relaxed);
                }
                let _ = event_tx.send(Event::ResourceLoadWarning {
                    service,
                    warning: format!("tags unavailable (tag:GetResources): {e}"),
                });
                None
            }
        }
    }
}

#[async_trait]
impl AwsService for ListTimeTags {
    fn service_type(&self) -> ServiceType {
        self.inner.service_type()
    }

    fn name(&self) -> &str {
        self.inner.name()
    }

    async fn list_resources(&self) -> crate::error::Result<Vec<Box<dyn Resource>>> {
        let fetch = self.spawn_fetch();
        let mut rows = self.inner.list_resources().await?;
        if let Some(Ok(Ok(index))) = match fetch {
            Some(f) => Some(f.await),
            None => None,
        } {
            index.apply(&mut rows);
        }
        Ok(rows)
    }

    async fn list_resources_streaming(
        &self,
        event_tx: mpsc::UnboundedSender<Event>,
        service_type: ServiceType,
    ) -> crate::error::Result<()> {
        let fetch = self.spawn_fetch();
        let (inner_tx, mut inner_rx) = mpsc::unbounded_channel();
        let inner = Arc::clone(&self.inner);
        let list =
            tokio::spawn(async move { inner.list_resources_streaming(inner_tx, service_type).await });

        // Pass the stream through, holding back the completion event: the
        // tags have to land between the last batch and it.
        let mut completion = None;
        while let Some(event) = inner_rx.recv().await {
            match event {
                Event::ResourcesFullyLoaded { .. } | Event::ResourcesLoaded { .. } => {
                    completion = Some(event)
                }
                Event::ResourceLoadError { .. } => {
                    if let Some(f) = &fetch {
                        f.abort();
                    }
                    let _ = event_tx.send(event);
                    return Ok(());
                }
                other => {
                    let _ = event_tx.send(other);
                }
            }
        }
        let listed = match list.await {
            Ok(r) => r,
            Err(e) => Err(crate::error::Error::AwsSdk(format!("load task failed: {e}"))),
        };

        let index = self.finish_fetch(fetch, service_type, &event_tx).await;
        match completion {
            // The non-streaming default sends its rows with the completion.
            Some(Event::ResourcesLoaded { service, mut resources }) => {
                if let Some(index) = &index {
                    index.apply(&mut resources);
                }
                let _ = event_tx.send(Event::ResourcesLoaded { service, resources });
            }
            Some(done) => {
                if let Some(index) = index {
                    let _ = event_tx.send(Event::ListTagsLoaded {
                        service: service_type,
                        tags: Arc::new(index),
                    });
                }
                let _ = event_tx.send(done);
            }
            None => {}
        }
        listed
    }

    async fn get_resource_details(&self, id: &str) -> crate::error::Result<Box<dyn Resource>> {
        self.inner.get_resource_details(id).await
    }
}

/// Unwrap one phase's tag results (failures become `T::default()`), sending
/// a single warning for the phase when any call failed. `warned` holds the
/// warning to once per load for loops that fan out phase after phase (a
/// denied call stays denied, so repeating it per batch is noise).
pub fn take_tags<T: Default>(
    results: Vec<Result<T, String>>,
    service: ServiceType,
    what: &str,
    warned: &mut bool,
    event_tx: &mpsc::UnboundedSender<Event>,
) -> Vec<T> {
    let failed = results.iter().filter(|r| r.is_err()).count();
    if !*warned {
        if let Some(Err(e)) = results.iter().find(|r| r.is_err()) {
            *warned = true;
            let _ = event_tx.send(Event::ResourceLoadWarning {
                service,
                warning: format!("{what} tags ({failed} of {} failed): {e}", results.len()),
            });
        }
    }
    results.into_iter().map(|r| r.unwrap_or_default()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failure_warns_once_and_defaults_the_row() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut warned = false;
        let tagged: Tags = [("team".to_string(), "x".to_string())].into();
        let out = take_tags(
            vec![Ok(tagged.clone()), Err("AccessDenied: no".to_string())],
            ServiceType::Backup,
            "vault",
            &mut warned,
            &tx,
        );
        assert_eq!(out, vec![tagged, Tags::new()]);
        match rx.try_recv() {
            Ok(Event::ResourceLoadWarning { warning, .. }) => {
                assert_eq!(warning, "vault tags (1 of 2 failed): AccessDenied: no")
            }
            _ => panic!("expected one warning"),
        }
        // A second failing phase under the same flag stays quiet.
        take_tags::<Tags>(vec![Err("again".into())], ServiceType::Backup, "plan", &mut warned, &tx);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn index_matches_by_arn_or_resource_part() {
        let mut idx = TagIndex::default();
        let team: Tags = [("team".to_string(), "data".to_string())].into();
        idx.insert("arn:aws:glue:us-east-1:111111111111:job/nightly", team.clone());
        idx.insert("arn:aws:logs:us-east-1:111111111111:log-group:/aws/lambda/x", Tags::new());
        assert_eq!(idx.get("arn:aws:glue:us-east-1:111111111111:job/nightly"), Some(&team));
        assert_eq!(idx.get("job/nightly"), Some(&team));
        assert_eq!(idx.get("job/night"), None, "a prefix is not a match");
        // The part is everything after the account, prefix and all.
        assert!(idx.get("log-group:/aws/lambda/x").is_some());
    }

    #[test]
    fn all_ok_sends_nothing() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut warned = false;
        take_tags::<Tags>(vec![Ok(Tags::new())], ServiceType::Backup, "vault", &mut warned, &tx);
        assert!(!warned);
        assert!(rx.try_recv().is_err());
    }
}
