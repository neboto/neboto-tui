use crate::aws::client::AwsClients;
use crate::aws::resource::{native_state_label, Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::error::Result;
use crate::event::{Event, LoadProgress};
use async_trait::async_trait;
use aws_sdk_cloudfront::Client as CloudFrontClient;
use std::any::Any;
use std::collections::HashMap;
use tokio::sync::mpsc;

pub struct CloudFrontService {
    client: CloudFrontClient,
}

impl CloudFrontService {
    pub fn new(aws_clients: &AwsClients) -> Self {
        Self {
            client: aws_clients.cloudfront_client(),
        }
    }
}

#[async_trait]
impl AwsService for CloudFrontService {
    fn service_type(&self) -> ServiceType {
        ServiceType::CloudFront
    }

    fn name(&self) -> &str {
        "CloudFront"
    }

    async fn list_resources(&self) -> Result<Vec<Box<dyn Resource>>> {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        self.list_resources_streaming(tx, ServiceType::CloudFront)
            .await?;
        Ok(vec![])
    }

    async fn list_resources_streaming(
        &self,
        event_tx: mpsc::UnboundedSender<Event>,
        service_type: ServiceType,
    ) -> Result<()> {
        // Phase 0: cache / origin-request / response-headers policies. Fetched
        // first because distributions need the id→name maps (KMS-alias
        // pattern) to render behaviors by policy name. The policies themselves
        // stream as Policies sub-tab rows in phase 2. Best-effort: a
        // permission gap warns and behaviors fall back to raw ids.
        let (policies, policy_names) =
            fetch_policies(&self.client, &event_tx, service_type).await;

        let mut total = 0usize;
        let mut marker: Option<String> = None;

        // Phase 1: distributions (core — a total failure here is fatal).
        loop {
            let mut req = self.client.list_distributions();
            if let Some(m) = &marker {
                req = req.marker(m);
            }

            match req.send().await {
                Ok(resp) => {
                    if let Some(list) = resp.distribution_list() {
                        let items = list.items();
                        let batch: Vec<Box<dyn Resource>> = items
                            .iter()
                            .map(|s| {
                                Box::new(CfDistribution::from_summary(s, &policy_names))
                                    as Box<dyn Resource>
                            })
                            .collect();

                        let count = batch.len();
                        if count > 0 {
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

                        if list.is_truncated() {
                            marker = crate::aws::pagination::next_page_token(list.next_marker(), &marker);
                        } else {
                            break;
                        }
                    } else {
                        break;
                    }
                }
                Err(e) => {
                    let _ = event_tx.send(Event::ResourceLoadError {
                        service: service_type,
                        error: format!("Failed to list CloudFront distributions: {}", e),
                    });
                    return Ok(());
                }
            }
        }

        // Phase 2: the policies fetched in phase 0, as browsable rows.
        if !policies.is_empty() {
            total += policies.len();
            let batch: Vec<Box<dyn Resource>> = policies
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

        // Phase 3: CloudFront Functions (error-tolerant).
        match fetch_functions(&self.client).await {
            Ok(functions) if !functions.is_empty() => {
                total += functions.len();
                let batch: Vec<Box<dyn Resource>> = functions
                    .into_iter()
                    .map(|f| Box::new(f) as Box<dyn Resource>)
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
            Err(e) => {
                let _ = event_tx.send(Event::ResourceLoadWarning {
                    service: service_type,
                    warning: format!("functions: {}", e),
                });
            }
        }

        // Phase 4: Origin Access Controls (error-tolerant).
        match fetch_oacs(&self.client).await {
            Ok(oacs) if !oacs.is_empty() => {
                total += oacs.len();
                let batch: Vec<Box<dyn Resource>> = oacs
                    .into_iter()
                    .map(|o| Box::new(o) as Box<dyn Resource>)
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
            Err(e) => {
                let _ = event_tx.send(Event::ResourceLoadWarning {
                    service: service_type,
                    warning: format!("origin access controls: {}", e),
                });
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

// ── Policy id→name resolution ─────────────────────────────────────────────────

/// id → name maps for the three behavior policy types, resolved once per load.
#[derive(Debug, Clone, Default)]
pub struct CfPolicyNames {
    pub cache: HashMap<String, String>,
    pub origin_request: HashMap<String, String>,
    pub response_headers: HashMap<String, String>,
}

impl CfPolicyNames {
    fn resolve(map: &HashMap<String, String>, id: Option<&str>) -> String {
        let id = id.unwrap_or_default();
        if id.is_empty() {
            return String::new();
        }
        map.get(id).cloned().unwrap_or_else(|| id.to_string())
    }
}

/// Fetch all cache / origin-request / response-headers policies (managed +
/// custom): the browsable `CfPolicy` rows for the Policies sub-tab plus the
/// id→name maps behaviors resolve against. Each list is best-effort: a failure
/// sends a `ResourceLoadWarning`, behaviors render the raw policy id, and that
/// kind is simply absent from the sub-tab.
async fn fetch_policies(
    client: &CloudFrontClient,
    event_tx: &mpsc::UnboundedSender<Event>,
    service_type: ServiceType,
) -> (Vec<CfPolicy>, CfPolicyNames) {
    let mut policies = Vec::new();
    let mut names = CfPolicyNames::default();
    let warn = |what: &str, e: String| {
        let _ = event_tx.send(Event::ResourceLoadWarning {
            service: service_type,
            warning: format!("{}: {}", what, e),
        });
    };

    // Cache policies
    let mut marker: Option<String> = None;
    loop {
        let mut req = client.list_cache_policies();
        if let Some(m) = &marker {
            req = req.marker(m);
        }
        match req.send().await {
            Ok(resp) => {
                let Some(list) = resp.cache_policy_list() else { break };
                for s in list.items() {
                    let managed =
                        *s.r#type() == aws_sdk_cloudfront::types::CachePolicyType::Managed;
                    if let Some(p) = s.cache_policy() {
                        if let Some(cfg) = p.cache_policy_config() {
                            names.cache.insert(p.id().to_string(), cfg.name().to_string());
                            policies.push(CfPolicy::from_cache_policy(p.id(), cfg, managed));
                        }
                    }
                }
                marker = crate::aws::pagination::next_page_token(list.next_marker(), &marker);
                if marker.is_none() {
                    break;
                }
            }
            Err(e) => {
                warn("cache policies", crate::error::sdk_error_message(&e));
                break;
            }
        }
    }

    // Origin request policies
    let mut marker: Option<String> = None;
    loop {
        let mut req = client.list_origin_request_policies();
        if let Some(m) = &marker {
            req = req.marker(m);
        }
        match req.send().await {
            Ok(resp) => {
                let Some(list) = resp.origin_request_policy_list() else { break };
                for s in list.items() {
                    let managed = *s.r#type()
                        == aws_sdk_cloudfront::types::OriginRequestPolicyType::Managed;
                    if let Some(p) = s.origin_request_policy() {
                        if let Some(cfg) = p.origin_request_policy_config() {
                            names
                                .origin_request
                                .insert(p.id().to_string(), cfg.name().to_string());
                            policies.push(CfPolicy::from_origin_request_policy(
                                p.id(),
                                cfg,
                                managed,
                            ));
                        }
                    }
                }
                marker = crate::aws::pagination::next_page_token(list.next_marker(), &marker);
                if marker.is_none() {
                    break;
                }
            }
            Err(e) => {
                warn("origin request policies", crate::error::sdk_error_message(&e));
                break;
            }
        }
    }

    // Response headers policies
    let mut marker: Option<String> = None;
    loop {
        let mut req = client.list_response_headers_policies();
        if let Some(m) = &marker {
            req = req.marker(m);
        }
        match req.send().await {
            Ok(resp) => {
                let Some(list) = resp.response_headers_policy_list() else { break };
                for s in list.items() {
                    let managed = *s.r#type()
                        == aws_sdk_cloudfront::types::ResponseHeadersPolicyType::Managed;
                    if let Some(p) = s.response_headers_policy() {
                        if let Some(cfg) = p.response_headers_policy_config() {
                            names
                                .response_headers
                                .insert(p.id().to_string(), cfg.name().to_string());
                            policies.push(CfPolicy::from_response_headers_policy(
                                p.id(),
                                cfg,
                                managed,
                            ));
                        }
                    }
                }
                marker = crate::aws::pagination::next_page_token(list.next_marker(), &marker);
                if marker.is_none() {
                    break;
                }
            }
            Err(e) => {
                warn("response headers policies", crate::error::sdk_error_message(&e));
                break;
            }
        }
    }

    (policies, names)
}

// ── CfDistribution ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct CfDistribution {
    pub id: String,
    pub arn: String,
    pub domain_name: String,
    pub status: String,
    pub enabled: bool,
    pub aliases: Vec<String>,
    pub comment: String,
    pub price_class: String,
    pub http_version: String,
    pub is_ipv6_enabled: bool,
    pub web_acl_id: String,
    pub last_modified: Option<String>,
    pub origins: Vec<CfOrigin>,
    pub origin_groups: Vec<CfOriginGroup>,
    pub behaviors: Vec<CfBehavior>,
    pub error_responses: Vec<CfErrorResponse>,
    pub geo_restriction: CfGeoRestriction,
    pub viewer_cert: String,
    pub min_tls: String,
    pub ssl_method: String,
    pub staging: bool,
    pub anycast_ip_list_id: String,
    pub tags: HashMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct CfOrigin {
    pub id: String,
    pub domain_name: String,
    pub origin_path: String,
    pub kind: String,
    pub protocol_policy: String,
    pub oac_id: String,
    /// Origin Shield region; empty = disabled.
    pub shield_region: String,
    pub custom_headers: usize,
    pub connection: String,
    /// Custom-origin extras: "read Ns · keepalive Ns · TLSv1.2" — empty for S3.
    pub custom_timeouts: String,
}

/// Failover origin group (primary → secondary on the listed status codes).
#[derive(Debug, Clone)]
pub struct CfOriginGroup {
    pub id: String,
    pub members: Vec<String>,
    pub status_codes: Vec<i32>,
}

#[derive(Debug, Clone)]
pub struct CfBehavior {
    pub path_pattern: String,
    pub target_origin_id: String,
    pub viewer_protocol_policy: String,
    pub allowed_methods: String,
    /// Resolved policy names (raw id fallback); empty = not set.
    pub cache_policy: String,
    pub origin_request_policy: String,
    pub response_headers_policy: String,
    pub compress: bool,
    pub smooth_streaming: bool,
    /// (event type, function ARN) pairs.
    pub lambda_edge: Vec<(String, String)>,
    pub functions: Vec<(String, String)>,
    pub realtime_log_arn: String,
    pub field_level_encryption: String,
    pub trusted_key_groups: Vec<String>,
    pub trusted_signers: Vec<String>,
    /// Legacy (no cache policy): (min, default, max) TTL seconds.
    pub legacy_ttls: Option<(i64, i64, i64)>,
    /// Legacy forwarded-values summary; empty when a cache policy is in use.
    pub legacy_forwarded: String,
}

#[derive(Debug, Clone)]
pub struct CfErrorResponse {
    pub error_code: i32,
    pub response_code: String,
    pub response_page: String,
    pub min_ttl: Option<i64>,
}

#[derive(Debug, Clone, Default)]
pub struct CfGeoRestriction {
    pub kind: String,
    pub locations: Vec<String>,
}

/// Shared field extraction for `DefaultCacheBehavior` / `CacheBehavior` — two
/// SDK types with identical accessors but no common trait.
macro_rules! parse_behavior {
    ($b:expr, $path:expr, $names:expr) => {{
        let b = $b;
        let names: &CfPolicyNames = $names;
        let lambda_edge: Vec<(String, String)> = b
            .lambda_function_associations()
            .map(|l| l.items())
            .unwrap_or_default()
            .iter()
            .map(|a| {
                let mut ev = a.event_type().as_str().to_string();
                if a.include_body().unwrap_or(false) {
                    ev.push_str(" +body");
                }
                (ev, a.lambda_function_arn().to_string())
            })
            .collect();
        let functions: Vec<(String, String)> = b
            .function_associations()
            .map(|l| l.items())
            .unwrap_or_default()
            .iter()
            .map(|a| (a.event_type().as_str().to_string(), a.function_arn().to_string()))
            .collect();
        let trusted_key_groups: Vec<String> = b
            .trusted_key_groups()
            .filter(|t| t.enabled())
            .map(|t| t.items().to_vec())
            .unwrap_or_default();
        let trusted_signers: Vec<String> = b
            .trusted_signers()
            .filter(|t| t.enabled())
            .map(|t| t.items().to_vec())
            .unwrap_or_default();
        // Legacy mode: forwarded-values + per-behavior TTLs (a cache policy
        // supersedes both; the console shows the same split). The accessors are
        // SDK-deprecated, but old distributions still carry their config here.
        #[allow(deprecated)]
        let (legacy_ttls, legacy_forwarded) = if let Some(fv) = b.forwarded_values() {
            let mut parts = vec![format!(
                "query strings: {}",
                if fv.query_string() { "yes" } else { "no" }
            )];
            if let Some(c) = fv.cookies() {
                parts.push(format!("cookies: {}", c.forward().as_str()));
            }
            if let Some(h) = fv.headers() {
                if h.quantity() > 0 {
                    parts.push(format!("headers: {}", h.items().join(", ")));
                }
            }
            (
                Some((
                    b.min_ttl().unwrap_or(0),
                    b.default_ttl().unwrap_or(0),
                    b.max_ttl().unwrap_or(0),
                )),
                parts.join(" · "),
            )
        } else {
            (None, String::new())
        };
        CfBehavior {
            path_pattern: $path,
            target_origin_id: b.target_origin_id().to_string(),
            viewer_protocol_policy: b.viewer_protocol_policy().as_str().to_string(),
            allowed_methods: b
                .allowed_methods()
                .map(|m| {
                    m.items()
                        .iter()
                        .map(|x| x.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default(),
            cache_policy: CfPolicyNames::resolve(&names.cache, b.cache_policy_id()),
            origin_request_policy: CfPolicyNames::resolve(
                &names.origin_request,
                b.origin_request_policy_id(),
            ),
            response_headers_policy: CfPolicyNames::resolve(
                &names.response_headers,
                b.response_headers_policy_id(),
            ),
            compress: b.compress().unwrap_or(false),
            smooth_streaming: b.smooth_streaming().unwrap_or(false),
            lambda_edge,
            functions,
            realtime_log_arn: b.realtime_log_config_arn().unwrap_or_default().to_string(),
            field_level_encryption: b
                .field_level_encryption_id()
                .unwrap_or_default()
                .to_string(),
            trusted_key_groups,
            trusted_signers,
            legacy_ttls,
            legacy_forwarded,
        }
    }};
}

impl CfDistribution {
    pub fn from_summary(
        s: &aws_sdk_cloudfront::types::DistributionSummary,
        policy_names: &CfPolicyNames,
    ) -> Self {
        let id = s.id().to_string();
        let arn = s.arn().to_string();
        let domain_name = s.domain_name().to_string();
        let status = s.status().to_string();
        let enabled = s.enabled();
        let comment = s.comment().to_string();
        let price_class = s.price_class().as_str().to_string();
        let http_version = s.http_version().as_str().to_string();
        let is_ipv6_enabled = s.is_ipv6_enabled();
        let web_acl_id = s.web_acl_id().to_string();
        let last_modified = {
            let dt = s.last_modified_time();
            fmt_epoch_secs(dt.secs())
        };

        // Aliases (CNAMEs)
        let aliases = s
            .aliases()
            .map(|a| a.items().iter().map(|i| i.to_string()).collect())
            .unwrap_or_default();

        // Origins
        let origins = s
            .origins()
            .map(|o| o.items())
            .unwrap_or_default()
            .iter()
            .map(|o| {
                let oac_id = o.origin_access_control_id().unwrap_or_default().to_string();
                let kind = if o.s3_origin_config().is_some() {
                    if oac_id.is_empty() {
                        "S3".to_string()
                    } else {
                        "S3 (OAC)".to_string()
                    }
                } else if let Some(v) = o.vpc_origin_config() {
                    format!("VPC ({})", v.vpc_origin_id())
                } else {
                    "Custom".to_string()
                };
                let protocol_policy = o
                    .custom_origin_config()
                    .map(|c| c.origin_protocol_policy().as_str().to_string())
                    .unwrap_or_default();
                let shield_region = o
                    .origin_shield()
                    .filter(|sh| sh.enabled())
                    .and_then(|sh| sh.origin_shield_region())
                    .unwrap_or_default()
                    .to_string();
                let custom_headers = o
                    .custom_headers()
                    .map(|h| h.quantity() as usize)
                    .unwrap_or(0);
                let connection = match (o.connection_attempts(), o.connection_timeout()) {
                    (Some(a), Some(t)) => format!("{} attempts · {}s timeout", a, t),
                    (Some(a), None) => format!("{} attempts", a),
                    (None, Some(t)) => format!("{}s timeout", t),
                    (None, None) => String::new(),
                };
                let custom_timeouts = o
                    .custom_origin_config()
                    .map(|c| {
                        let mut parts = vec![];
                        if let Some(rt) = c.origin_read_timeout() {
                            parts.push(format!("read {}s", rt));
                        }
                        if let Some(ka) = c.origin_keepalive_timeout() {
                            parts.push(format!("keepalive {}s", ka));
                        }
                        if let Some(sp) = c.origin_ssl_protocols() {
                            let protos: Vec<&str> =
                                sp.items().iter().map(|p| p.as_str()).collect();
                            if !protos.is_empty() {
                                parts.push(protos.join("/"));
                            }
                        }
                        parts.join(" · ")
                    })
                    .unwrap_or_default();
                CfOrigin {
                    id: o.id().to_string(),
                    domain_name: o.domain_name().to_string(),
                    origin_path: o.origin_path().unwrap_or_default().to_string(),
                    kind,
                    protocol_policy,
                    oac_id,
                    shield_region,
                    custom_headers,
                    connection,
                    custom_timeouts,
                }
            })
            .collect();

        // Origin groups (failover)
        let origin_groups = s
            .origin_groups()
            .map(|g| g.items())
            .unwrap_or_default()
            .iter()
            .map(|g| CfOriginGroup {
                id: g.id().to_string(),
                members: g
                    .members()
                    .map(|m| m.items().iter().map(|i| i.origin_id().to_string()).collect())
                    .unwrap_or_default(),
                status_codes: g
                    .failover_criteria()
                    .and_then(|f| f.status_codes())
                    .map(|sc| sc.items().to_vec())
                    .unwrap_or_default(),
            })
            .collect();

        // Custom error responses
        let error_responses = s
            .custom_error_responses()
            .map(|c| c.items())
            .unwrap_or_default()
            .iter()
            .map(|er| CfErrorResponse {
                error_code: er.error_code(),
                response_code: er.response_code().unwrap_or_default().to_string(),
                response_page: er.response_page_path().unwrap_or_default().to_string(),
                min_ttl: er.error_caching_min_ttl(),
            })
            .collect();

        // Behaviors: default first, then cache behaviors
        let mut behaviors = Vec::new();
        if let Some(db) = s.default_cache_behavior() {
            behaviors.push(parse_behavior!(db, "Default (*)".to_string(), policy_names));
        }
        if let Some(cbs) = s.cache_behaviors() {
            for cb in cbs.items() {
                behaviors.push(parse_behavior!(
                    cb,
                    cb.path_pattern().to_string(),
                    policy_names
                ));
            }
        }

        // Geo restriction
        let geo_restriction = s
            .restrictions()
            .and_then(|r| r.geo_restriction())
            .map(|g| CfGeoRestriction {
                kind: g.restriction_type().as_str().to_string(),
                locations: g.items().iter().map(|l| l.to_string()).collect(),
            })
            .unwrap_or_default();

        // Viewer certificate + TLS posture
        let viewer_cert = s
            .viewer_certificate()
            .map(|vc| {
                if let Some(acm_arn) = vc.acm_certificate_arn() {
                    if !acm_arn.is_empty() {
                        return acm_arn.to_string();
                    }
                }
                if let Some(iam_id) = vc.iam_certificate_id() {
                    if !iam_id.is_empty() {
                        return format!("IAM: {}", iam_id);
                    }
                }
                if vc.cloud_front_default_certificate().unwrap_or(false) {
                    return "CloudFront default certificate".to_string();
                }
                "Unknown".to_string()
            })
            .unwrap_or_default();
        let min_tls = s
            .viewer_certificate()
            .and_then(|vc| vc.minimum_protocol_version())
            .map(|v| v.as_str().to_string())
            .unwrap_or_default();
        let ssl_method = s
            .viewer_certificate()
            .and_then(|vc| vc.ssl_support_method())
            .map(|v| v.as_str().to_string())
            .unwrap_or_default();

        Self {
            id,
            arn,
            domain_name,
            status,
            enabled,
            aliases,
            comment,
            price_class,
            http_version,
            is_ipv6_enabled,
            web_acl_id,
            last_modified: Some(last_modified),
            origins,
            origin_groups,
            behaviors,
            error_responses,
            geo_restriction,
            viewer_cert,
            min_tls,
            ssl_method,
            staging: s.staging(),
            anycast_ip_list_id: s.anycast_ip_list_id().unwrap_or_default().to_string(),
            tags: HashMap::new(),
        }
    }
}

crate::sections! {
    pub enum CfDistributionDetailSection,
    pub static CF_DISTRIBUTION_SECTIONS = [
        Details "Details",
        Origins "Origins",
        Behaviors "Behaviors",
        Errors "Errors",
        Restrictions "Restrictions",
        Invalidations "Invalidations" => crate::app::App::trigger_cf_invalidations_load,
        Tags "Tags" => crate::app::App::trigger_cf_tags_load,
    ]
}

impl Resource for CfDistribution {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&CF_DISTRIBUTION_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        if let Some(first_alias) = self.aliases.first() {
            first_alias
        } else {
            &self.domain_name
        }
    }

    fn resource_type(&self) -> &str {
        "CloudFront Distribution"
    }

    fn state(&self) -> ResourceState {
        if self.status == "InProgress" {
            return ResourceState::Pending;
        }
        if !self.enabled {
            return ResourceState::Stopped;
        }
        ResourceState::Available
    }

    fn state_label(&self) -> String {
        if self.status == "InProgress" {
            "in progress".to_string()
        } else if !self.enabled {
            "disabled".to_string()
        } else {
            native_state_label(&self.status, || self.state())
        }
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        let mut text = format!(
            "{} {} {} {}",
            self.id,
            self.domain_name,
            self.aliases.join(" "),
            self.comment,
        );
        if !self.web_acl_id.is_empty() {
            text.push(' ');
            text.push_str(&self.web_acl_id);
        }
        text
    }

    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("ID".to_string(), self.id.clone()),
            ("Domain".to_string(), self.domain_name.clone()),
            ("Status".to_string(), self.status.clone()),
            ("Enabled".to_string(), self.enabled.to_string()),
            ("Price Class".to_string(), self.price_class.clone()),
            ("HTTP Version".to_string(), self.http_version.clone()),
            ("IPv6".to_string(), self.is_ipv6_enabled.to_string()),
            ("Origins".to_string(), self.origins.len().to_string()),
            ("Behaviors".to_string(), self.behaviors.len().to_string()),
            (
                "Last Modified".to_string(),
                self.last_modified
                    .clone()
                    .unwrap_or_else(|| "—".to_string()),
            ),
        ]
    }

    fn console_url(&self, _region: &str) -> Option<String> {
        Some(format!(
            "https://us-east-1.console.aws.amazon.com/cloudfront/v4/home#/distributions/{}",
            self.id
        ))
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── CfFunction (CloudFront Functions) ─────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct CfFunction {
    pub name: String,
    pub arn: String,
    pub status: String,
    pub runtime: String,
    pub comment: String,
    /// True when the function has a LIVE (published) stage.
    pub published: bool,
    pub created: String,
    pub last_modified: String,
    pub kv_stores: Vec<String>,
    pub tags: HashMap<String, String>,
}

impl CfFunction {
    fn from_summary(s: &aws_sdk_cloudfront::types::FunctionSummary) -> Self {
        let (arn, created, last_modified) = s
            .function_metadata()
            .map(|m| {
                (
                    m.function_arn().to_string(),
                    m.created_time()
                        .map(|t| fmt_epoch_secs(t.secs()))
                        .unwrap_or_default(),
                    fmt_epoch_secs(m.last_modified_time().secs()),
                )
            })
            .unwrap_or_default();
        let (comment, runtime, kv_stores) = s
            .function_config()
            .map(|c| {
                (
                    c.comment().to_string(),
                    c.runtime().as_str().to_string(),
                    c.key_value_store_associations()
                        .map(|k| {
                            k.items()
                                .iter()
                                .map(|a| a.key_value_store_arn().to_string())
                                .collect()
                        })
                        .unwrap_or_default(),
                )
            })
            .unwrap_or_default();
        Self {
            name: s.name().to_string(),
            arn,
            status: s.status().unwrap_or_default().to_string(),
            runtime,
            comment,
            published: false,
            created,
            last_modified,
            kv_stores,
            tags: HashMap::new(),
        }
    }
}

crate::sections! {
    pub enum CfFunctionDetailSection,
    pub static CF_FUNCTION_SECTIONS = [
        Overview "Overview",
        Code "Code" => crate::app::App::trigger_cf_function_code_load,
    ]
}

impl Resource for CfFunction {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&CF_FUNCTION_SECTIONS)
    }
    fn id(&self) -> &str {
        &self.name
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn resource_type(&self) -> &str {
        "CloudFront Function"
    }
    fn state(&self) -> ResourceState {
        // Unpublished (DEVELOPMENT-only) functions read as pending so the
        // "edited but never published" state pops in the list.
        if self.published {
            ResourceState::Available
        } else {
            ResourceState::Pending
        }
    }

    fn state_label(&self) -> String {
        if self.published { "published" } else { "unpublished" }.to_string()
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        format!("{} {} {}", self.name, self.comment, self.runtime)
    }
    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Name".to_string(), self.name.clone()),
            ("ARN".to_string(), self.arn.clone()),
            ("Status".to_string(), self.status.clone()),
            ("Runtime".to_string(), self.runtime.clone()),
            (
                "Stage".to_string(),
                if self.published {
                    "LIVE (published)".to_string()
                } else {
                    "DEVELOPMENT only (never published)".to_string()
                },
            ),
        ]
    }
    fn console_url(&self, _region: &str) -> Option<String> {
        Some(format!(
            "https://us-east-1.console.aws.amazon.com/cloudfront/v4/home#/functions/{}",
            self.name
        ))
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// List CloudFront functions: the DEVELOPMENT stage lists every function, the
/// LIVE stage marks which are published.
async fn fetch_functions(client: &CloudFrontClient) -> Result<Vec<CfFunction>> {
    use aws_sdk_cloudfront::types::FunctionStage;

    let list_stage = |stage: FunctionStage| {
        let client = client.clone();
        async move {
            let mut items = Vec::new();
            let mut marker: Option<String> = None;
            loop {
                let mut req = client.list_functions().stage(stage.clone());
                if let Some(m) = &marker {
                    req = req.marker(m);
                }
                let resp = req.send().await.map_err(|e| {
                    crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e))
                })?;
                let Some(list) = resp.function_list() else { break };
                items.extend(list.items().to_vec());
                marker = crate::aws::pagination::next_page_token(list.next_marker(), &marker);
                if marker.is_none() {
                    break;
                }
            }
            Ok::<_, crate::error::Error>(items)
        }
    };

    let (dev, live) = tokio::join!(
        list_stage(FunctionStage::Development),
        list_stage(FunctionStage::Live)
    );
    let dev = dev?;
    // LIVE failing after DEVELOPMENT succeeded shouldn't drop the list — the
    // published marker just stays off.
    let live_names: std::collections::HashSet<String> = live
        .unwrap_or_default()
        .iter()
        .map(|s| s.name().to_string())
        .collect();

    Ok(dev
        .iter()
        .map(|s| {
            let mut f = CfFunction::from_summary(s);
            f.published = live_names.contains(&f.name);
            f
        })
        .collect())
}


/// Fetch a function's source (GetFunction returns the code itself). Reads the
/// LIVE stage when published, DEVELOPMENT otherwise.
pub async fn fetch_function_code(
    client: CloudFrontClient,
    name: String,
    published: bool,
) -> Result<String> {
    use aws_sdk_cloudfront::types::FunctionStage;
    let stage = if published {
        FunctionStage::Live
    } else {
        FunctionStage::Development
    };
    let resp = client
        .get_function()
        .name(&name)
        .stage(stage)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
    let code = resp
        .function_code()
        .map(|b| String::from_utf8_lossy(b.as_ref()).to_string())
        .unwrap_or_default();
    Ok(code)
}

// ── CfPolicy (cache / origin-request / response-headers policies) ────────────

#[derive(Debug, Clone)]
pub struct CfPolicy {
    pub id: String,
    pub name: String,
    /// "Cache" / "Origin Request" / "Response Headers".
    pub kind: &'static str,
    pub managed: bool,
    pub comment: String,
    /// Pre-flattened config rows appended to `details()`.
    pub config_rows: Vec<(String, String)>,
    pub tags: HashMap<String, String>,
}

impl CfPolicy {
    fn base(
        id: &str,
        name: &str,
        kind: &'static str,
        managed: bool,
        comment: Option<&str>,
    ) -> Self {
        Self {
            id: id.to_string(),
            name: name.to_string(),
            kind,
            managed,
            comment: comment.unwrap_or_default().to_string(),
            config_rows: Vec::new(),
            tags: HashMap::new(),
        }
    }

    fn from_cache_policy(
        id: &str,
        cfg: &aws_sdk_cloudfront::types::CachePolicyConfig,
        managed: bool,
    ) -> Self {
        let mut p = Self::base(id, cfg.name(), "Cache", managed, cfg.comment());
        let rows = &mut p.config_rows;
        rows.push((
            "TTL".to_string(),
            format!(
                "min {}s · default {}s · max {}s",
                cfg.min_ttl(),
                cfg.default_ttl().unwrap_or(0),
                cfg.max_ttl().unwrap_or(0)
            ),
        ));
        if let Some(k) = cfg.parameters_in_cache_key_and_forwarded_to_origin() {
            let mut enc = Vec::new();
            if k.enable_accept_encoding_gzip() {
                enc.push("gzip");
            }
            if k.enable_accept_encoding_brotli().unwrap_or(false) {
                enc.push("brotli");
            }
            rows.push((
                "Accept-Encoding".to_string(),
                if enc.is_empty() {
                    "none".to_string()
                } else {
                    enc.join(" + ")
                },
            ));
            rows.push(("".to_string(), "".to_string()));
            rows.push(("Cache Key".to_string(), "".to_string()));
            if let Some(h) = k.headers_config() {
                rows.push((
                    "  Headers".to_string(),
                    behavior_with_items(
                        h.header_behavior().as_str(),
                        h.headers().map(|x| x.items()),
                    ),
                ));
            }
            if let Some(c) = k.cookies_config() {
                rows.push((
                    "  Cookies".to_string(),
                    behavior_with_items(
                        c.cookie_behavior().as_str(),
                        c.cookies().map(|x| x.items()),
                    ),
                ));
            }
            if let Some(q) = k.query_strings_config() {
                rows.push((
                    "  Query Strings".to_string(),
                    behavior_with_items(
                        q.query_string_behavior().as_str(),
                        q.query_strings().map(|x| x.items()),
                    ),
                ));
            }
        }
        p
    }

    fn from_origin_request_policy(
        id: &str,
        cfg: &aws_sdk_cloudfront::types::OriginRequestPolicyConfig,
        managed: bool,
    ) -> Self {
        let mut p = Self::base(id, cfg.name(), "Origin Request", managed, cfg.comment());
        let rows = &mut p.config_rows;
        rows.push(("Forwarded to Origin".to_string(), "".to_string()));
        if let Some(h) = cfg.headers_config() {
            rows.push((
                "  Headers".to_string(),
                behavior_with_items(h.header_behavior().as_str(), h.headers().map(|x| x.items())),
            ));
        }
        if let Some(c) = cfg.cookies_config() {
            rows.push((
                "  Cookies".to_string(),
                behavior_with_items(c.cookie_behavior().as_str(), c.cookies().map(|x| x.items())),
            ));
        }
        if let Some(q) = cfg.query_strings_config() {
            rows.push((
                "  Query Strings".to_string(),
                behavior_with_items(
                    q.query_string_behavior().as_str(),
                    q.query_strings().map(|x| x.items()),
                ),
            ));
        }
        p
    }

    fn from_response_headers_policy(
        id: &str,
        cfg: &aws_sdk_cloudfront::types::ResponseHeadersPolicyConfig,
        managed: bool,
    ) -> Self {
        let mut p = Self::base(id, cfg.name(), "Response Headers", managed, cfg.comment());
        let rows = &mut p.config_rows;
        if let Some(cors) = cfg.cors_config() {
            rows.push(("CORS".to_string(), "".to_string()));
            if let Some(o) = cors.access_control_allow_origins() {
                let origins = o.items().join(", ");
                let val = if origins.contains('*') {
                    format!("⚠ {}", origins)
                } else {
                    origins
                };
                rows.push(("  Allow Origins".to_string(), val));
            }
            if let Some(m) = cors.access_control_allow_methods() {
                rows.push((
                    "  Allow Methods".to_string(),
                    m.items()
                        .iter()
                        .map(|x| x.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                ));
            }
            rows.push((
                "  Allow Credentials".to_string(),
                if cors.access_control_allow_credentials() {
                    "Yes".to_string()
                } else {
                    "No".to_string()
                },
            ));
            if let Some(age) = cors.access_control_max_age_sec() {
                rows.push(("  Max Age".to_string(), format!("{}s", age)));
            }
            rows.push((
                "  Origin Override".to_string(),
                if cors.origin_override() {
                    "Yes".to_string()
                } else {
                    "No".to_string()
                },
            ));
            rows.push(("".to_string(), "".to_string()));
        }
        if let Some(sec) = cfg.security_headers_config() {
            rows.push(("Security Headers".to_string(), "".to_string()));
            if let Some(h) = sec.strict_transport_security() {
                let mut v = format!("max-age={}", h.access_control_max_age_sec());
                if h.include_subdomains().unwrap_or(false) {
                    v.push_str("; includeSubDomains");
                }
                if h.preload().unwrap_or(false) {
                    v.push_str("; preload");
                }
                rows.push(("  HSTS".to_string(), v));
            }
            if let Some(h) = sec.frame_options() {
                rows.push(("  X-Frame-Options".to_string(), h.frame_option().as_str().to_string()));
            }
            if let Some(h) = sec.referrer_policy() {
                rows.push((
                    "  Referrer-Policy".to_string(),
                    h.referrer_policy().as_str().to_string(),
                ));
            }
            if let Some(h) = sec.content_security_policy() {
                rows.push((
                    "  CSP".to_string(),
                    h.content_security_policy().to_string(),
                ));
            }
            if sec.content_type_options().is_some() {
                rows.push(("  X-Content-Type-Options".to_string(), "nosniff".to_string()));
            }
            if let Some(x) = sec.xss_protection() {
                rows.push((
                    "  X-XSS-Protection".to_string(),
                    format!(
                        "{}{}",
                        if x.protection() { "1" } else { "0" },
                        if x.mode_block().unwrap_or(false) {
                            "; mode=block"
                        } else {
                            ""
                        }
                    ),
                ));
            }
            rows.push(("".to_string(), "".to_string()));
        }
        if let Some(st) = cfg.server_timing_headers_config() {
            if st.enabled() {
                rows.push((
                    "Server-Timing".to_string(),
                    format!(
                        "enabled (sampling {}%)",
                        st.sampling_rate().unwrap_or(0.0)
                    ),
                ));
            }
        }
        if let Some(ch) = cfg.custom_headers_config() {
            if !ch.items().is_empty() {
                rows.push(("Custom Headers".to_string(), "".to_string()));
                for h in ch.items() {
                    rows.push((
                        format!("  {}", h.header()),
                        format!(
                            "{}{}",
                            h.value(),
                            if h.r#override() { " (override)" } else { "" }
                        ),
                    ));
                }
            }
        }
        if let Some(rh) = cfg.remove_headers_config() {
            if !rh.items().is_empty() {
                rows.push((
                    "Removed Headers".to_string(),
                    rh.items()
                        .iter()
                        .map(|h| h.header())
                        .collect::<Vec<_>>()
                        .join(", "),
                ));
            }
        }
        p
    }
}

/// "whitelist: Host, Origin" / "all" / "none" — the behavior enum plus the
/// item list when one applies.
fn behavior_with_items(behavior: &str, items: Option<&[String]>) -> String {
    let list = items.unwrap_or_default();
    if list.is_empty() {
        behavior.to_string()
    } else {
        format!("{}: {}", behavior, list.join(", "))
    }
}

impl Resource for CfPolicy {
    fn id(&self) -> &str {
        &self.id
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn resource_type(&self) -> &str {
        "CloudFront Policy"
    }
    fn state(&self) -> ResourceState {
        ResourceState::stateless()
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn is_noise(&self) -> bool {
        // AWS-managed policies are reference material; `a` narrows to your own.
        self.managed
    }
    fn search_text(&self) -> String {
        format!("{} {} {} {}", self.name, self.id, self.kind, self.comment)
    }
    fn details(&self) -> Vec<(String, String)> {
        let mut rows = vec![
            ("Name".to_string(), self.name.clone()),
            ("ID".to_string(), self.id.clone()),
            ("Kind".to_string(), format!("{} Policy", self.kind)),
            (
                "Type".to_string(),
                if self.managed {
                    "AWS Managed".to_string()
                } else {
                    "Custom".to_string()
                },
            ),
        ];
        if !self.comment.is_empty() {
            rows.push(("Comment".to_string(), self.comment.clone()));
        }
        rows.push(("".to_string(), "".to_string()));
        rows.extend(self.config_rows.iter().cloned());
        rows
    }
    fn console_url(&self, _region: &str) -> Option<String> {
        Some(
            "https://us-east-1.console.aws.amazon.com/cloudfront/v4/home#/policies".to_string(),
        )
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── CfOac (Origin Access Controls) ────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct CfOac {
    pub id: String,
    pub name: String,
    pub description: String,
    pub signing_behavior: String,
    pub signing_protocol: String,
    pub origin_type: String,
    pub tags: HashMap<String, String>,
}

impl Resource for CfOac {
    fn id(&self) -> &str {
        &self.id
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn resource_type(&self) -> &str {
        "CloudFront OAC"
    }
    fn state(&self) -> ResourceState {
        ResourceState::stateless()
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        format!("{} {} {} {}", self.name, self.id, self.origin_type, self.description)
    }
    fn details(&self) -> Vec<(String, String)> {
        let mut rows = vec![
            ("Name".to_string(), self.name.clone()),
            ("ID".to_string(), self.id.clone()),
            ("Origin Type".to_string(), self.origin_type.clone()),
            ("Signing Behavior".to_string(), self.signing_behavior.clone()),
            ("Signing Protocol".to_string(), self.signing_protocol.clone()),
        ];
        if !self.description.is_empty() {
            rows.push(("Description".to_string(), self.description.clone()));
        }
        rows
    }
    fn console_url(&self, _region: &str) -> Option<String> {
        Some(format!(
            "https://us-east-1.console.aws.amazon.com/cloudfront/v4/home#/originAccess/controlSettings/{}",
            self.id
        ))
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

async fn fetch_oacs(client: &CloudFrontClient) -> Result<Vec<CfOac>> {
    let mut oacs = Vec::new();
    let mut marker: Option<String> = None;
    loop {
        let mut req = client.list_origin_access_controls();
        if let Some(m) = &marker {
            req = req.marker(m);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
        let Some(list) = resp.origin_access_control_list() else { break };
        for s in list.items() {
            oacs.push(CfOac {
                id: s.id().to_string(),
                name: s.name().to_string(),
                description: s.description().to_string(),
                signing_behavior: s.signing_behavior().as_str().to_string(),
                signing_protocol: s.signing_protocol().as_str().to_string(),
                origin_type: s.origin_access_control_origin_type().as_str().to_string(),
                tags: HashMap::new(),
            });
        }
        if !list.is_truncated() {
            break;
        }
        marker = crate::aws::pagination::next_page_token(list.next_marker(), &marker);
        if marker.is_none() {
            break;
        }
    }
    Ok(oacs)
}

// ── Lazy tags fetch ───────────────────────────────────────────────────────────

pub async fn fetch_distribution_tags(
    client: CloudFrontClient,
    arn: String,
) -> Result<HashMap<String, String>> {
    let resp = client
        .list_tags_for_resource()
        .resource(&arn)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;

    let tags = resp
        .tags()
        .map(|t| {
            t.items()
                .iter()
                .map(|tag| {
                    (
                        tag.key().to_string(),
                        tag.value().unwrap_or_default().to_string(),
                    )
                })
                .collect::<HashMap<_, _>>()
        })
        .unwrap_or_default();

    Ok(tags)
}

// ── Invalidations (lazy distribution section) ─────────────────────────────────

/// Cap on invalidations shown (newest-first); the section header reads
/// "latest N of M" when the history is longer.
pub const MAX_INVALIDATIONS: usize = 20;
/// Cap on paths listed per invalidation ("+N more" row when exceeded).
const MAX_INVALIDATION_PATHS: usize = 8;

#[derive(Debug, Clone)]
pub struct CfInvalidation {
    pub id: String,
    pub status: String,
    pub created: String,
    /// First `MAX_INVALIDATION_PATHS` paths (from `GetInvalidation`).
    pub paths: Vec<String>,
    pub paths_total: usize,
}

#[derive(Debug, Clone)]
pub struct CfInvalidationsData {
    pub items: Vec<CfInvalidation>,
    /// Total invalidations on the distribution (≥ items.len()).
    pub total: usize,
}


/// Fetch a distribution's invalidation history: all summaries (paginated),
/// newest `MAX_INVALIDATIONS` kept, each deepened with its paths via
/// `GetInvalidation` (best-effort — a path fetch failure leaves the row
/// path-less rather than failing the section).
pub async fn fetch_invalidations(
    client: CloudFrontClient,
    distribution_id: String,
) -> Result<CfInvalidationsData> {
    let mut summaries: Vec<(i64, String, String)> = Vec::new(); // (epoch, id, status)
    let mut marker: Option<String> = None;
    loop {
        let mut req = client.list_invalidations().distribution_id(&distribution_id);
        if let Some(m) = &marker {
            req = req.marker(m);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
        let Some(list) = resp.invalidation_list() else { break };
        for s in list.items() {
            summaries.push((
                s.create_time().secs(),
                s.id().to_string(),
                s.status().to_string(),
            ));
        }
        if !list.is_truncated() {
            break;
        }
        marker = crate::aws::pagination::next_page_token(list.next_marker(), &marker);
        if marker.is_none() {
            break;
        }
    }

    let total = summaries.len();
    summaries.sort_by_key(|(epoch, _, _)| std::cmp::Reverse(*epoch));
    summaries.truncate(MAX_INVALIDATIONS);

    let mut items = Vec::with_capacity(summaries.len());
    for (epoch, id, status) in summaries {
        let (paths, paths_total) = match client
            .get_invalidation()
            .distribution_id(&distribution_id)
            .id(&id)
            .send()
            .await
        {
            Ok(resp) => {
                let all: Vec<String> = resp
                    .invalidation()
                    .and_then(|i| i.invalidation_batch())
                    .and_then(|b| b.paths())
                    .map(|p| p.items().to_vec())
                    .unwrap_or_default();
                let n = all.len();
                (all.into_iter().take(MAX_INVALIDATION_PATHS).collect(), n)
            }
            Err(_) => (Vec::new(), 0),
        };
        items.push(CfInvalidation {
            id,
            status,
            created: fmt_epoch_secs(epoch),
            paths,
            paths_total,
        });
    }

    Ok(CfInvalidationsData { items, total })
}

// ── Metrics (AWS/CloudFront) ──────────────────────────────────────────────────

pub use crate::aws::services::ec2::MetricsTimeRange;

#[derive(Debug, Clone)]
pub struct CfMetricsData {
    pub time_range: MetricsTimeRange,
    pub requests: Vec<(f64, f64)>,       // Requests (Sum)
    pub bytes_down: Vec<(f64, f64)>,     // BytesDownloaded (Sum)
    pub bytes_up: Vec<(f64, f64)>,       // BytesUploaded (Sum)
    pub error_4xx: Vec<(f64, f64)>,      // 4xxErrorRate (Average, %)
    pub error_5xx: Vec<(f64, f64)>,      // 5xxErrorRate (Average, %)
    pub error_total: Vec<(f64, f64)>,    // TotalErrorRate (Average, %)
    pub cache_hit: Vec<(f64, f64)>,      // CacheHitRate (Average, %) — additional metric
    pub origin_latency: Vec<(f64, f64)>, // OriginLatency (Average, ms) — additional metric
    pub x_max: f64,
}

#[derive(Debug, Clone)]
pub enum CfMetricsState {
    Loading,
    Loaded(Box<CfMetricsData>),
}

/// Fetch the `AWS/CloudFront` dashboard metrics. CloudFront publishes to
/// us-east-1 only (global service), keyed on `DistributionId` + `Region=Global`.
/// `CacheHitRate` / `TotalErrorRate` / `OriginLatency` are "additional"
/// metrics — they only have datapoints when additional monitoring is enabled
/// on the distribution, so an empty series there is normal, not an error.
pub async fn fetch_cloudfront_metrics(
    cw_client: aws_sdk_cloudwatch::Client,
    distribution_id: String,
    time_range: MetricsTimeRange,
) -> Result<CfMetricsData> {
    use aws_sdk_cloudwatch::primitives::DateTime as CwDateTime;
    use aws_sdk_cloudwatch::types::{Dimension, Statistic};

    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let start_secs = now_secs - time_range.duration_secs();
    let period = time_range.period_secs();
    let start_dt = CwDateTime::from_secs(start_secs);
    let end_dt = CwDateTime::from_secs(now_secs);

    let make_dims = || {
        vec![
            Dimension::builder()
                .name("DistributionId")
                .value(&distribution_id)
                .build(),
            Dimension::builder().name("Region").value("Global").build(),
        ]
    };

    let metric = |name: &'static str, stat: Statistic| {
        cw_client
            .get_metric_statistics()
            .namespace("AWS/CloudFront")
            .metric_name(name)
            .set_dimensions(Some(make_dims()))
            .start_time(start_dt)
            .end_time(end_dt)
            .period(period)
            .set_statistics(Some(vec![stat]))
            .send()
    };

    let (req_r, down_r, up_r, e4_r, e5_r, et_r, hit_r, lat_r) = tokio::join!(
        metric("Requests", Statistic::Sum),
        metric("BytesDownloaded", Statistic::Sum),
        metric("BytesUploaded", Statistic::Sum),
        metric("4xxErrorRate", Statistic::Average),
        metric("5xxErrorRate", Statistic::Average),
        metric("TotalErrorRate", Statistic::Average),
        metric("CacheHitRate", Statistic::Average),
        metric("OriginLatency", Statistic::Average),
    );

    let parse = |resp: std::result::Result<
        aws_sdk_cloudwatch::operation::get_metric_statistics::GetMetricStatisticsOutput,
        _,
    >,
                 sum: bool|
     -> Vec<(f64, f64)> {
        let dps = match resp {
            Ok(r) => r.datapoints().to_vec(),
            Err(_) => vec![],
        };
        let mut pts: Vec<(f64, f64)> = dps
            .iter()
            .filter_map(|dp| {
                let t = dp.timestamp()?.secs() as f64 - start_secs as f64;
                let v = if sum { dp.sum()? } else { dp.average()? };
                Some((t, v))
            })
            .collect();
        pts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        pts
    };

    Ok(CfMetricsData {
        time_range,
        requests: parse(req_r, true),
        bytes_down: parse(down_r, true),
        bytes_up: parse(up_r, true),
        error_4xx: parse(e4_r, false),
        error_5xx: parse(e5_r, false),
        error_total: parse(et_r, false),
        cache_hit: parse(hit_r, false),
        origin_latency: parse(lat_r, false),
        x_max: time_range.duration_secs() as f64,
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
