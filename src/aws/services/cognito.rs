use crate::aws::client::AwsClients;
use crate::aws::pagination::next_page_token;
use crate::aws::resource::{native_state_label, Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::error::Result;
use crate::event::{Event, LoadProgress};
use async_trait::async_trait;
use aws_sdk_cognitoidentity::Client as IdentityClient;
use aws_sdk_cognitoidentityprovider::Client as IdpClient;
use std::any::Any;
use std::collections::HashMap;
use tokio::sync::mpsc;

/// Cognito — a single service over two unrelated APIs: user pools
/// (cognito-idp) and identity/federated pools (cognito-identity). Sub-tabs
/// separate the two resource types. User pools get a split pane (App Clients
/// lazy); identity pools are flat.
pub struct CognitoService {
    idp: IdpClient,
    identity: IdentityClient,
}

impl CognitoService {
    pub fn new(aws_clients: &AwsClients) -> Self {
        Self {
            idp: aws_clients.cognito_idp_client(),
            identity: aws_clients.cognito_identity_client(),
        }
    }
}

#[async_trait]
impl AwsService for CognitoService {
    fn service_type(&self) -> ServiceType {
        ServiceType::Cognito
    }

    fn name(&self) -> &str {
        "Cognito"
    }

    async fn list_resources(&self) -> Result<Vec<Box<dyn Resource>>> {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        self.list_resources_streaming(tx, ServiceType::Cognito).await?;
        Ok(vec![])
    }

    async fn list_resources_streaming(
        &self,
        event_tx: mpsc::UnboundedSender<Event>,
        service_type: ServiceType,
    ) -> Result<()> {
        // Both pool types fetched concurrently — they share nothing.
        let (user_pools, identity_pools) =
            tokio::join!(self.fetch_user_pools(), self.fetch_identity_pools());

        let mut total = 0usize;
        let mut pool_ids: Vec<String> = Vec::new();

        match user_pools {
            Ok(pools) if !pools.is_empty() => {
                pool_ids = pools.iter().map(|p| p.id.clone()).collect();
                let batch: Vec<Box<dyn Resource>> = pools
                    .into_iter()
                    .map(|p| Box::new(p) as Box<dyn Resource>)
                    .collect();
                total += batch.len();
                let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                    service: service_type,
                    resources: batch,
                    progress: LoadProgress {
                        loaded_count: total,
                        total_count: None,
                        status_message: Some("Loading identity pools…".to_string()),
                    },
                });
            }
            Err(e) => {
                let _ = event_tx.send(Event::ResourceLoadWarning {
                    service: service_type,
                    warning: format!("user pools: {}", e),
                });
            }
            _ => {}
        }

        match identity_pools {
            Ok(pools) if !pools.is_empty() => {
                let batch: Vec<Box<dyn Resource>> = pools
                    .into_iter()
                    .map(|p| Box::new(p) as Box<dyn Resource>)
                    .collect();
                total += batch.len();
                let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                    service: service_type,
                    resources: batch,
                    progress: LoadProgress {
                        loaded_count: total,
                        total_count: None,
                        status_message: Some("Loading users…".to_string()),
                    },
                });
            }
            Err(e) => {
                let _ = event_tx.send(Event::ResourceLoadWarning {
                    service: service_type,
                    warning: format!("identity pools: {}", e),
                });
            }
            _ => {}
        }

        // Users, up to MAX_COGNITO_USERS across all pools.
        if !pool_ids.is_empty() {
            let (users, failures) = self.fetch_users(&pool_ids).await;
            for (pool, e) in failures {
                let _ = event_tx.send(Event::ResourceLoadWarning {
                    service: service_type,
                    warning: format!("users in {}: {}", pool, e),
                });
            }
            if !users.is_empty() {
                let batch: Vec<Box<dyn Resource>> = users
                    .into_iter()
                    .map(|u| Box::new(u) as Box<dyn Resource>)
                    .collect();
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

impl CognitoService {
    async fn fetch_user_pools(&self) -> Result<Vec<CognitoUserPool>> {
        let mut ids: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut req = self.idp.list_user_pools().max_results(60);
            if let Some(t) = &token {
                req = req.next_token(t);
            }
            let page = req
                .send()
                .await
                .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
            for p in page.user_pools() {
                if let Some(id) = p.id() {
                    ids.push(id.to_string());
                }
            }
            token = next_page_token(page.next_token(), &token);
            if token.is_none() {
                break;
            }
        }

        // describe each pool concurrently
        let futs = ids.into_iter().map(|id| {
            let idp = self.idp.clone();
            async move {
                idp.describe_user_pool()
                    .user_pool_id(&id)
                    .send()
                    .await
                    .ok()
                    .and_then(|r| r.user_pool().cloned())
                    .map(CognitoUserPool::from_sdk)
            }
        });
        let pools = futures::future::join_all(futs)
            .await
            .into_iter()
            .flatten()
            .collect();
        Ok(pools)
    }

    async fn fetch_identity_pools(&self) -> Result<Vec<CognitoIdentityPool>> {
        let mut ids: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut req = self.identity.list_identity_pools().max_results(60);
            if let Some(t) = &token {
                req = req.next_token(t);
            }
            let page = req
                .send()
                .await
                .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
            for p in page.identity_pools() {
                if let Some(id) = p.identity_pool_id() {
                    ids.push(id.to_string());
                }
            }
            token = next_page_token(page.next_token(), &token);
            if token.is_none() {
                break;
            }
        }

        let futs = ids.into_iter().map(|id| {
            let client = self.identity.clone();
            async move {
                client
                    .describe_identity_pool()
                    .identity_pool_id(&id)
                    .send()
                    .await
                    .ok()
                    .map(|r| CognitoIdentityPool::from_sdk(&r))
            }
        });
        let pools = futures::future::join_all(futs)
            .await
            .into_iter()
            .flatten()
            .collect();
        Ok(pools)
    }

    /// Users from every pool, paginated, up to `MAX_COGNITO_USERS` in total
    /// (a consumer pool can hold millions; this is a browser, not an export
    /// job). A pool whose `ListUsers` fails is reported, not skipped silently
    /// — an empty Users tab would otherwise read as "no users".
    async fn fetch_users(&self, pool_ids: &[String]) -> (Vec<CognitoUser>, Vec<(String, String)>) {
        let mut users: Vec<CognitoUser> = Vec::new();
        let mut failures = Vec::new();
        'pools: for pool_id in pool_ids {
            let mut token: Option<String> = None;
            loop {
                let room = MAX_COGNITO_USERS - users.len();
                if room == 0 {
                    break 'pools;
                }
                let mut req = self
                    .idp
                    .list_users()
                    .user_pool_id(pool_id)
                    .limit(room.min(60) as i32);
                if let Some(t) = &token {
                    req = req.pagination_token(t);
                }
                match req.send().await {
                    Ok(page) => {
                        users.extend(page.users().iter().map(|u| CognitoUser::from_sdk(u, pool_id)));
                        token = next_page_token(page.pagination_token(), &token);
                        if token.is_none() {
                            break;
                        }
                    }
                    Err(e) => {
                        failures.push((pool_id.clone(), crate::error::sdk_error_message(&e)));
                        break;
                    }
                }
            }
        }
        users.truncate(MAX_COGNITO_USERS);
        (users, failures)
    }
}

// ── App Clients (lazy, per user pool) ────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct CognitoAppClient {
    pub id: String,
    pub name: String,
    pub explicit_auth_flows: Vec<String>,
    pub callback_urls: Vec<String>,
    pub has_secret: bool,
}

/// Fetch a user pool's app clients (list → describe each). Keyed by pool id.
pub async fn fetch_cognito_clients(
    idp: IdpClient,
    pool_id: String,
) -> Result<Vec<CognitoAppClient>> {
    let mut client_ids: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut req = idp
            .list_user_pool_clients()
            .user_pool_id(&pool_id)
            .max_results(60);
        if let Some(t) = &token {
            req = req.next_token(t);
        }
        let page = req
            .send()
            .await
            .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
        for c in page.user_pool_clients() {
            if let Some(id) = c.client_id() {
                client_ids.push(id.to_string());
            }
        }
        token = next_page_token(page.next_token(), &token);
        if token.is_none() {
            break;
        }
    }

    let futs = client_ids.into_iter().map(|cid| {
        let idp = idp.clone();
        let pool_id = pool_id.clone();
        async move {
            idp.describe_user_pool_client()
                .user_pool_id(&pool_id)
                .client_id(&cid)
                .send()
                .await
                .ok()
                .and_then(|r| r.user_pool_client().cloned())
                .map(|c| CognitoAppClient {
                    id: c.client_id().unwrap_or_default().to_string(),
                    name: c.client_name().unwrap_or_default().to_string(),
                    explicit_auth_flows: c
                        .explicit_auth_flows()
                        .iter()
                        .map(|f| f.as_str().to_string())
                        .collect(),
                    callback_urls: c.callback_urls().to_vec(),
                    has_secret: c.client_secret().is_some(),
                })
        }
    });
    Ok(futures::future::join_all(futs)
        .await
        .into_iter()
        .flatten()
        .collect())
}

// ── Groups (lazy, per user pool) ─────────────────────────────────────────────

/// Groups listed per pool — past this the section says it's truncated.
pub const MAX_COGNITO_GROUPS: usize = 50;
/// Members listed per group: one `ListUsersInGroup` page (60 is the API max).
pub const MAX_COGNITO_GROUP_MEMBERS: i32 = 60;

#[derive(Debug, Clone)]
pub struct CognitoGroupMember {
    pub username: String,
    /// What the member row shows — see `member_label`.
    pub label: String,
    pub enabled: bool,
    pub status: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CognitoGroup {
    pub name: String,
    pub description: Option<String>,
    pub precedence: Option<i32>,
    pub role_arn: Option<String>,
    /// `Err` = this group's `ListUsersInGroup` failed; the others still render.
    pub members: std::result::Result<Vec<CognitoGroupMember>, String>,
    /// More members exist than the one page fetched.
    pub members_truncated: bool,
}

#[derive(Debug, Clone)]
pub struct CognitoGroups {
    pub groups: Vec<CognitoGroup>,
    /// More groups exist than `MAX_COGNITO_GROUPS`.
    pub truncated: bool,
}

/// The member row's text. A pool that signs in by email or phone sets each
/// `Username` to the user's `sub` (a UUID), which says nothing about who is in
/// the group, so those rows show the contact instead. A chosen username is
/// kept, with the contact beside it.
pub(crate) fn member_label(
    username: &str,
    sub: Option<&str>,
    email: Option<&str>,
    phone: Option<&str>,
) -> String {
    match email.or(phone).filter(|c| !c.is_empty()) {
        None => username.to_string(),
        Some(c) if c == username || sub == Some(username) => c.to_string(),
        Some(c) => format!("{username} <{c}>"),
    }
}

/// Whether the group list stops at the cap: more than `MAX_COGNITO_GROUPS`
/// arrived, or exactly that many with another page still to come.
pub(crate) fn groups_truncated(len: usize, more_pages: bool) -> bool {
    len > MAX_COGNITO_GROUPS || (len == MAX_COGNITO_GROUPS && more_pages)
}

/// A user pool's groups with their members, inverted from the group side:
/// `ListGroups` + one `ListUsersInGroup` page per group (4 at a time). The
/// per-user `AdminListGroupsForUser` is never called — it's one call per user,
/// and an `Admin*` action.
pub async fn fetch_cognito_groups(idp: IdpClient, pool_id: String) -> Result<CognitoGroups> {
    use futures::StreamExt;
    let mut raw = Vec::new();
    let mut token: Option<String> = None;
    let mut truncated = false;
    loop {
        let page = idp
            .list_groups()
            .user_pool_id(&pool_id)
            .limit(60)
            .set_next_token(token.clone())
            .send()
            .await
            .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
        raw.extend(page.groups().iter().cloned());
        token = next_page_token(page.next_token(), &token);
        if groups_truncated(raw.len(), token.is_some()) {
            raw.truncate(MAX_COGNITO_GROUPS);
            truncated = true;
            break;
        }
        if token.is_none() {
            break;
        }
    }

    let mut groups: Vec<CognitoGroup> = futures::stream::iter(raw.into_iter().map(|g| {
        let idp = idp.clone();
        let pool_id = pool_id.clone();
        async move {
            let name = g.group_name().unwrap_or_default().to_string();
            let resp = idp
                .list_users_in_group()
                .user_pool_id(&pool_id)
                .group_name(&name)
                .limit(MAX_COGNITO_GROUP_MEMBERS)
                .send()
                .await;
            let (members, members_truncated) = match resp {
                Ok(r) => (
                    Ok(r.users()
                        .iter()
                        .map(|u| {
                            let attr = |name: &str| {
                                u.attributes().iter().find(|a| a.name() == name).and_then(|a| a.value())
                            };
                            let username = u.username().unwrap_or_default().to_string();
                            CognitoGroupMember {
                                label: member_label(
                                    &username,
                                    attr("sub"),
                                    attr("email"),
                                    attr("phone_number"),
                                ),
                                username,
                                enabled: u.enabled(),
                                status: u.user_status().map(|s| s.as_str().to_string()),
                            }
                        })
                        .collect()),
                    r.next_token().is_some_and(|t| !t.is_empty()),
                ),
                Err(e) => (Err(crate::error::sdk_error_message(&e)), false),
            };
            CognitoGroup {
                name,
                description: g.description().filter(|d| !d.is_empty()).map(str::to_string),
                precedence: g.precedence(),
                role_arn: g.role_arn().map(str::to_string),
                members,
                members_truncated,
            }
        }
    }))
    .buffered(4)
    .collect()
    .await;
    // Precedence order (lower wins when a user's groups carry roles), then name.
    groups.sort_by(|a, b| {
        a.precedence
            .unwrap_or(i32::MAX)
            .cmp(&b.precedence.unwrap_or(i32::MAX))
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(CognitoGroups { groups, truncated })
}

// ── CognitoUserPool ──────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct CognitoUserPool {
    pub id: String,
    pub name: String,
    pub arn: String,
    pub mfa_config: String,
    pub estimated_users: i32,
    pub username_attributes: Vec<String>,
    pub domain: Option<String>,
    pub created: Option<String>,
    pub policies_summary: String,
    pub lambda_triggers: Vec<(String, String)>,
    pub tags: HashMap<String, String>,
}

impl CognitoUserPool {
    fn from_sdk(p: aws_sdk_cognitoidentityprovider::types::UserPoolType) -> Self {
        let policies_summary = p
            .policies()
            .and_then(|pol| pol.password_policy())
            .map(|pp| {
                let mut parts = vec![format!("min {}", pp.minimum_length().unwrap_or(0))];
                if pp.require_uppercase() {
                    parts.push("upper".to_string());
                }
                if pp.require_lowercase() {
                    parts.push("lower".to_string());
                }
                if pp.require_numbers() {
                    parts.push("number".to_string());
                }
                if pp.require_symbols() {
                    parts.push("symbol".to_string());
                }
                parts.join(", ")
            })
            .unwrap_or_else(|| "—".to_string());

        let lambda_triggers = extract_lambda_triggers(p.lambda_config());

        Self {
            id: p.id().unwrap_or_default().to_string(),
            name: p.name().unwrap_or_default().to_string(),
            arn: p.arn().unwrap_or_default().to_string(),
            mfa_config: p
                .mfa_configuration()
                .map(|m| m.as_str().to_string())
                .unwrap_or_else(|| "OFF".to_string()),
            estimated_users: p.estimated_number_of_users(),
            username_attributes: p
                .username_attributes()
                .iter()
                .map(|a| a.as_str().to_string())
                .collect(),
            domain: p
                .domain()
                .or(p.custom_domain())
                .map(|s| s.to_string())
                .filter(|s| !s.is_empty()),
            created: p.creation_date().map(|d| fmt_epoch_secs(d.secs())),
            policies_summary,
            lambda_triggers,
            tags: p
                .user_pool_tags()
                .map(|t| t.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                .unwrap_or_default(),
        }
    }
}

crate::sections! {
    pub enum CognitoUserPoolDetailSection,
    pub static COGNITO_USER_POOL_SECTIONS = [
        Details "Details",
        AppClients "App Clients" => crate::app::App::trigger_cognito_clients_load,
        Policies "Policies",
        Tags "Tags",
        Groups "Groups" => crate::app::App::trigger_cognito_groups_load,
    ]
}

impl Resource for CognitoUserPool {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&COGNITO_USER_POOL_SECTIONS)
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws cognito-idp describe-user-pool --user-pool-id {}",
            crate::aws::resource::shell_quote(self.id())
        ))
    }

    fn id(&self) -> &str {
        &self.id
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn resource_type(&self) -> &str {
        "Cognito User Pool"
    }
    fn state(&self) -> ResourceState {
        // Cognito no longer reports a user-pool status; treat all as available.
        ResourceState::stateless()
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        format!(
            "{} {} {} {}",
            self.name,
            self.id,
            self.domain.as_deref().unwrap_or(""),
            self.username_attributes.join(" "),
        )
    }
    fn details(&self) -> Vec<(String, String)> {
        // Fallback only — the split pane is the real view.
        vec![
            ("Name".to_string(), self.name.clone()),
            ("ID".to_string(), self.id.clone()),
            ("MFA".to_string(), self.mfa_config.clone()),
        ]
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/cognito/v2/idp/user-pools/{}/users?region={}",
            region, self.id, region
        ))
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── CognitoIdentityPool ──────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct CognitoIdentityPool {
    pub id: String,
    pub name: String,
    pub allow_unauthenticated: bool,
    pub providers: Vec<String>,
    pub tags: HashMap<String, String>,
}

impl CognitoIdentityPool {
    fn from_sdk(
        r: &aws_sdk_cognitoidentity::operation::describe_identity_pool::DescribeIdentityPoolOutput,
    ) -> Self {
        let mut providers: Vec<String> = Vec::new();
        for p in r.cognito_identity_providers() {
            if let Some(n) = p.provider_name() {
                providers.push(format!("cognito: {}", n));
            }
        }
        if let Some(login) = r.supported_login_providers() {
            for k in login.keys() {
                providers.push(format!("social: {}", k));
            }
        }
        for arn in r.saml_provider_arns() {
            providers.push(format!("saml: {}", arn));
        }
        for arn in r.open_id_connect_provider_arns() {
            providers.push(format!("oidc: {}", arn));
        }
        Self {
            id: r.identity_pool_id().to_string(),
            name: r.identity_pool_name().to_string(),
            allow_unauthenticated: r.allow_unauthenticated_identities(),
            providers,
            tags: r
                .identity_pool_tags()
                .map(|t| t.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                .unwrap_or_default(),
        }
    }
}

impl Resource for CognitoIdentityPool {
    fn id(&self) -> &str {
        &self.id
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn resource_type(&self) -> &str {
        "Cognito Identity Pool"
    }
    fn state(&self) -> ResourceState {
        ResourceState::stateless()
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        format!("{} {} {}", self.name, self.id, self.providers.join(" "))
    }
    fn details(&self) -> Vec<(String, String)> {
        let mut rows = vec![
            ("Name".to_string(), self.name.clone()),
            ("ID".to_string(), self.id.clone()),
            (
                "Unauthenticated Access".to_string(),
                if self.allow_unauthenticated { "allowed".to_string() } else { "denied".to_string() },
            ),
        ];
        rows.push((String::new(), String::new()));
        if self.providers.is_empty() {
            rows.push(("Identity Providers".to_string(), "none".to_string()));
        } else {
            rows.push(("Identity Providers".to_string(), String::new())); // group header
            for p in &self.providers {
                rows.push((format!("  {}", p), String::new()));
            }
        }
        if !self.tags.is_empty() {
            rows.push((String::new(), String::new()));
            rows.push(("Tags".to_string(), String::new()));
            let mut sorted: Vec<_> = self.tags.iter().collect();
            sorted.sort_by_key(|(k, _)| k.as_str());
            for (k, v) in sorted {
                rows.push((format!("  {}", k), v.clone()));
            }
        }
        rows
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/cognito/v2/identity/identity-pools/{}/user-statistics?region={}",
            region, self.id, region
        ))
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── CognitoUser (capped at MAX_COGNITO_USERS) ─────────────────────────────────

/// Users listed across all pools before the load stops (~9 `ListUsers`
/// pages of 60).
pub const MAX_COGNITO_USERS: usize = 500;

#[derive(Debug, Clone)]
pub struct CognitoUser {
    /// `pool_id/username` — a username is only unique within its pool.
    pub key: String,
    pub username: String,
    pub pool_id: String,
    pub status: String,
    pub enabled: bool,
    pub created: Option<String>,
    pub last_modified: Option<String>,
    pub email: Option<String>,
    pub phone: Option<String>,
    pub email_verified: bool,
    pub phone_verified: bool,
    /// Every other attribute `ListUsers` returned (`sub`, `name`,
    /// `custom:*`, …), sorted; the ones shown above as fields are left out.
    pub attributes: std::collections::BTreeMap<String, String>,
}

/// Attributes the pane already shows as named fields.
const COGNITO_FIELD_ATTRS: [&str; 4] = ["email", "phone_number", "email_verified", "phone_number_verified"];

impl CognitoUser {
    fn from_sdk(
        u: &aws_sdk_cognitoidentityprovider::types::UserType,
        pool_id: &str,
    ) -> Self {
        let attrs: HashMap<String, String> = u
            .attributes()
            .iter()
            .map(|a| {
                (a.name().to_string(), a.value().unwrap_or_default().to_string())
            })
            .collect();

        let username = u.username().unwrap_or_default().to_string();
        Self {
            key: format!("{}/{}", pool_id, username),
            attributes: attrs
                .iter()
                .filter(|(k, _)| !COGNITO_FIELD_ATTRS.contains(&k.as_str()))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            username,
            pool_id: pool_id.to_string(),
            status: u
                .user_status()
                .map(|s| s.as_str().to_string())
                .unwrap_or_default(),
            enabled: u.enabled(),
            created: u.user_create_date().map(|d| fmt_epoch_secs(d.secs())),
            last_modified: u.user_last_modified_date().map(|d| fmt_epoch_secs(d.secs())),
            email: attrs.get("email").cloned(),
            phone: attrs.get("phone_number").cloned(),
            email_verified: attrs.get("email_verified").map(|v| v == "true").unwrap_or(false),
            phone_verified: attrs.get("phone_number_verified").map(|v| v == "true").unwrap_or(false),
        }
    }
}

impl Resource for CognitoUser {
    fn id(&self) -> &str {
        &self.key
    }
    fn name(&self) -> &str {
        // Show email if available, else username
        self.email.as_deref().unwrap_or(&self.username)
    }
    fn resource_type(&self) -> &str {
        "Cognito User"
    }
    fn state(&self) -> ResourceState {
        if !self.enabled {
            return ResourceState::Unavailable;
        }
        match self.status.as_str() {
            "CONFIRMED" => ResourceState::Available,
            "UNCONFIRMED" | "FORCE_CHANGE_PASSWORD" | "RESET_REQUIRED" => ResourceState::Pending,
            "COMPROMISED" | "DISABLED" => ResourceState::Unavailable,
            _ => ResourceState::Unknown(self.status.clone()),
        }
    }

    fn state_label(&self) -> String {
        if !self.enabled {
            return "disabled".to_string();
        }
        native_state_label(&self.status, || self.state())
    }
    fn tags(&self) -> &HashMap<String, String> {
        static EMPTY: std::sync::OnceLock<HashMap<String, String>> = std::sync::OnceLock::new();
        EMPTY.get_or_init(HashMap::new)
    }
    fn search_text(&self) -> String {
        format!(
            "{} {} {} {} {}",
            self.username,
            self.email.as_deref().unwrap_or(""),
            self.phone.as_deref().unwrap_or(""),
            self.status,
            self.attributes.get("sub").map(String::as_str).unwrap_or(""),
        )
    }
    fn details(&self) -> Vec<(String, String)> {
        let mut rows = vec![
            ("Username".to_string(), self.username.clone()),
            ("Pool ID".to_string(), self.pool_id.clone()),
            ("Status".to_string(), self.status.clone()),
            ("Enabled".to_string(), if self.enabled { "✓".to_string() } else { "✗".to_string() }),
        ];
        if let Some(e) = &self.email {
            rows.push(("Email".to_string(), e.clone()));
            rows.push(("Email Verified".to_string(), if self.email_verified { "✓" } else { "✗" }.to_string()));
        }
        if let Some(p) = &self.phone {
            rows.push(("Phone".to_string(), p.clone()));
            rows.push(("Phone Verified".to_string(), if self.phone_verified { "✓" } else { "✗" }.to_string()));
        }
        if let Some(c) = &self.created {
            rows.push(("Created".to_string(), c.clone()));
        }
        if let Some(m) = &self.last_modified {
            rows.push(("Last Modified".to_string(), m.clone()));
        }
        // `sub` (the stable id tokens carry) first, then the rest A–Z
        // (`custom:*` sort after the standard ones by their prefix).
        let mut attrs: Vec<(&String, &String)> = self.attributes.iter().collect();
        attrs.sort_by_key(|(k, _)| (k.as_str() != "sub", k.starts_with("custom:"), k.as_str()));
        for (k, v) in attrs {
            rows.push((k.clone(), v.clone()));
        }
        rows
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{}.console.aws.amazon.com/cognito/v2/idp/user-pools/{}/users/{}?region={}",
            region, self.pool_id, self.username, region
        ))
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── Lambda trigger extraction ────────────────────────────────────────────────

fn extract_lambda_triggers(
    config: Option<&aws_sdk_cognitoidentityprovider::types::LambdaConfigType>,
) -> Vec<(String, String)> {
    let Some(c) = config else {
        return Vec::new();
    };
    let mut triggers: Vec<(String, String)> = Vec::new();
    let add = |t: &mut Vec<(String, String)>, name: &str, val: Option<&str>| {
        if let Some(v) = val {
            if !v.is_empty() {
                // Extract just the function name from the ARN for readability
                let short = v.rsplit(':').next().unwrap_or(v);
                t.push((name.to_string(), short.to_string()));
            }
        }
    };
    add(&mut triggers, "Pre Sign-up", c.pre_sign_up());
    add(&mut triggers, "Pre Authentication", c.pre_authentication());
    add(&mut triggers, "Post Authentication", c.post_authentication());
    add(&mut triggers, "Post Confirmation", c.post_confirmation());
    add(&mut triggers, "Pre Token Generation", c.pre_token_generation());
    add(&mut triggers, "Custom Message", c.custom_message());
    add(&mut triggers, "User Migration", c.user_migration());
    add(&mut triggers, "Define Auth Challenge", c.define_auth_challenge());
    add(&mut triggers, "Create Auth Challenge", c.create_auth_challenge());
    add(&mut triggers, "Verify Auth Challenge", c.verify_auth_challenge_response());
    triggers
}

// ── Timestamp helper ─────────────────────────────────────────────────────────

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
        31, 30, 31, 30, 31, 31, 30, 31, 30, 31,
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

// ── CloudWatch metrics (`m` overlay) — AWS/Cognito, dims UserPool+UserPoolClient

pub use crate::aws::services::ec2::MetricsTimeRange;

#[derive(Debug, Clone)]
pub struct CognitoMetricsData {
    pub time_range: MetricsTimeRange,
    pub sign_in_ok: Vec<(f64, f64)>,
    pub sign_up_ok: Vec<(f64, f64)>,
    pub token_refresh_ok: Vec<(f64, f64)>,
    pub sign_in_throttles: Vec<(f64, f64)>,
    pub sign_up_throttles: Vec<(f64, f64)>,
    pub federation_ok: Vec<(f64, f64)>,
    pub x_max: f64,
}

#[derive(Debug, Clone)]
pub enum CognitoMetricsState {
    Loading,
    Loaded(CognitoMetricsData),
}

/// Pull `AWS/Cognito` auth-flow metrics for one user pool. Cognito publishes
/// per `(UserPool, UserPoolClient)` pair, so each series is aggregated across
/// all app clients with the SEARCH/SUM `GetMetricData` pattern (same trick as
/// MSK's per-broker series) — the schema form pins the dimension set so
/// nothing else matches.
pub async fn fetch_cognito_metrics(
    cw: aws_sdk_cloudwatch::Client,
    pool_id: String,
    time_range: MetricsTimeRange,
) -> crate::error::Result<CognitoMetricsData> {
    use aws_sdk_cloudwatch::types::MetricDataQuery;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let start = now - time_range.duration_secs();
    let period = time_range.period_secs();

    let specs = [
        ("m0", "SignInSuccesses"),
        ("m1", "SignUpSuccesses"),
        ("m2", "TokenRefreshSuccesses"),
        ("m3", "SignInThrottles"),
        ("m4", "SignUpThrottles"),
        ("m5", "FederationSuccesses"),
    ];

    let queries: Vec<MetricDataQuery> = specs
        .iter()
        .map(|(id, metric)| {
            let expr = format!(
                "SUM(SEARCH('{{AWS/Cognito,UserPool,UserPoolClient}} MetricName=\"{}\" UserPool=\"{}\"', 'Sum', {}))",
                metric, pool_id, period
            );
            MetricDataQuery::builder().id(*id).expression(expr).build()
        })
        .collect();

    let resp = cw
        .get_metric_data()
        .set_metric_data_queries(Some(queries))
        .start_time(aws_sdk_cloudwatch::primitives::DateTime::from_secs(start))
        .end_time(aws_sdk_cloudwatch::primitives::DateTime::from_secs(now))
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;

    let mut series: HashMap<String, Vec<(f64, f64)>> = HashMap::new();
    for r in resp.metric_data_results() {
        let id = r.id().unwrap_or_default().to_string();
        let mut pts: Vec<(f64, f64)> = r
            .timestamps()
            .iter()
            .zip(r.values().iter())
            .map(|(t, v)| (t.secs() as f64 - start as f64, *v))
            .collect();
        pts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        series.insert(id, pts);
    }

    let take = |id: &str| series.get(id).cloned().unwrap_or_default();
    Ok(CognitoMetricsData {
        time_range,
        sign_in_ok: take("m0"),
        sign_up_ok: take("m1"),
        token_refresh_ok: take("m2"),
        sign_in_throttles: take("m3"),
        sign_up_throttles: take("m4"),
        federation_ok: take("m5"),
        x_max: time_range.duration_secs() as f64,
    })
}

#[cfg(test)]
mod user_tests {
    use super::*;
    use aws_sdk_cognitoidentityprovider::types::{AttributeType, UserType};

    #[test]
    fn users_keep_every_attribute_and_are_keyed_by_pool() {
        let attr = |n: &str, v: &str| AttributeType::builder().name(n).value(v).build().unwrap();
        let u = CognitoUser::from_sdk(
            &UserType::builder()
                .username("alice")
                .attributes(attr("email", "a@example.com"))
                .attributes(attr("custom:tenant", "acme"))
                .attributes(attr("name", "Alice"))
                .attributes(attr("sub", "1111-2222"))
                .build(),
            "us-east-1_ABC",
        );
        assert_eq!(u.id(), "us-east-1_ABC/alice", "a username is only unique per pool");
        assert_eq!(u.name(), "a@example.com");
        assert!(u.search_text().contains("1111-2222"));
        let keys: Vec<String> = u.details().into_iter().map(|(k, _)| k).collect();
        let tail: Vec<&str> = keys.iter().map(String::as_str).skip_while(|k| *k != "sub").collect();
        assert_eq!(tail, ["sub", "name", "custom:tenant"], "sub first, custom:* last: {keys:?}");
        assert!(!keys.iter().any(|k| k == "email"), "shown as Email, not repeated raw");
    }
}

#[cfg(test)]
mod group_tests {
    use super::*;

    #[test]
    fn email_sign_in_members_show_the_email_not_the_sub() {
        let sub = "a1b2c3d4-0000-4000-8000-000000000001";
        assert_eq!(member_label(sub, Some(sub), Some("ana@example.com"), None), "ana@example.com");
        // Phone sign-in falls back to the number.
        assert_eq!(member_label(sub, Some(sub), None, Some("+15550100")), "+15550100");
    }

    #[test]
    fn a_chosen_username_keeps_the_contact_beside_it() {
        assert_eq!(
            member_label("ana", Some("a1b2c3d4-0000-4000-8000-000000000001"), Some("ana@example.com"), None),
            "ana <ana@example.com>"
        );
        assert_eq!(member_label("ana@example.com", None, Some("ana@example.com"), None), "ana@example.com");
        assert_eq!(member_label("ana", None, None, None), "ana");
        assert_eq!(member_label("ana", None, Some(""), None), "ana");
    }

    #[test]
    fn group_cap_counts_a_further_page_only_at_the_cap() {
        assert!(!groups_truncated(MAX_COGNITO_GROUPS, false));
        assert!(groups_truncated(MAX_COGNITO_GROUPS, true));
        assert!(groups_truncated(MAX_COGNITO_GROUPS + 1, false));
        assert!(!groups_truncated(MAX_COGNITO_GROUPS - 1, true));
    }
}
