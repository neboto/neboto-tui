//! What `@audit` fetches beyond the lists (#206, ADR 0006): account facts
//! (one call each) and capped per-resource reads (KMS rotation, IAM
//! credentials, snapshot sharing).
//!
//! This is the loader half of the audit, so it's the only audit code that
//! calls AWS. Every call is a read on the guard's verb list. The checks in
//! `audit.rs` stay pure: they read the `AuditFacts` this returns, exactly as
//! they read cached lists. A failed call becomes a `Gap`, and the check that
//! needed it says "not checked — no permission for …" instead of passing
//! or erroring per call.

use std::collections::HashMap;
use std::future::Future;

use aws_smithy_runtime_api::client::result::SdkError;
use aws_smithy_types::error::metadata::ProvideErrorMetadata;
use futures::stream::{self, StreamExt};

use super::AuditInput;
use crate::aws::client::AwsClients;
use crate::aws::service::ServiceType;
use crate::aws::services::ec2::EbsSnapshot;
use crate::aws::services::iam::IamUser;
use crate::aws::services::kms::KmsKey;
use crate::aws::services::rds::RdsSnapshot;
use crate::aws::services::s3::PublicAccessBlockConfig;

/// Per-resource reads stop at this many of each kind; the rest are a
/// coverage note. Each IAM user is 3 + one per key calls.
pub const MAX_PER_KIND: usize = 100;
/// Calls in flight per kind, on top of the SDK's adaptive retry (#196).
const CONCURRENCY: usize = 4;

/// A fact that couldn't be read. `action` is the IAM action, for the
/// "no permission for …" note.
#[derive(Clone, Debug, PartialEq)]
pub struct Gap {
    pub action: &'static str,
    pub denied: bool,
    pub message: String,
}

impl Gap {
    fn from_sdk<E, R>(action: &'static str, err: &SdkError<E, R>) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
        R: std::fmt::Debug,
        SdkError<E, R>: ProvideErrorMetadata,
    {
        let code = err.code().unwrap_or_default();
        let denied = code.contains("AccessDenied")
            || code.starts_with("Unauthorized")
            || code == "AuthorizationError"
            || code == "AuthFailure";
        Self { action, denied, message: crate::error::sdk_error_message(err) }
    }

    /// The list-column note for a check this gap stopped.
    pub fn note(&self) -> String {
        if self.denied {
            format!("not checked — no permission for {}", self.action)
        } else {
            format!("not checked — {} failed", self.action)
        }
    }
}

pub type Fact<T> = Result<T, Gap>;

#[derive(Clone, Debug)]
pub struct TrailFact {
    pub name: String,
    pub multi_region: bool,
    pub organization: bool,
    /// `None`: `GetTrailStatus` failed (often a trail homed in another
    /// region).
    pub logging: Option<bool>,
}

#[derive(Clone, Debug)]
pub struct RecorderFact {
    pub name: String,
    pub recording: bool,
}

#[derive(Clone, Debug)]
pub struct DetectorFact {
    pub id: String,
    pub enabled: bool,
}

#[derive(Clone, Debug)]
pub struct AnalyzerFact {
    pub name: String,
    /// ACCOUNT / ORGANIZATION / …_UNUSED_ACCESS
    pub kind: String,
    pub active: bool,
}

/// One call each.
#[derive(Clone, Debug)]
pub struct AccountFacts {
    /// `Ok(None)`: no account-level block configured.
    pub s3_block: Fact<Option<PublicAccessBlockConfig>>,
    pub ebs_default_encryption: Fact<bool>,
    pub trails: Fact<Vec<TrailFact>>,
    pub config_recorders: Fact<Vec<RecorderFact>>,
    pub guardduty_detectors: Fact<Vec<DetectorFact>>,
    pub analyzers: Fact<Vec<AnalyzerFact>>,
    /// Whether the account is in an AWS Organization. An organization
    /// analyzer lives in the management or delegated-admin account, so a
    /// member account can't see it — and "no analyzer" there isn't a
    /// finding (#227).
    pub in_organization: Fact<bool>,
}

#[derive(Clone, Debug)]
pub struct KeyFact {
    pub id: String,
    pub active: bool,
    pub created_secs: Option<i64>,
    /// `None`: never used — but only when `last_used_gap` is false.
    pub last_used_secs: Option<i64>,
    /// `GetAccessKeyLastUsed` failed (denied, throttled), so `None` above
    /// means *unknown*, not "never used". The difference matters: "old and
    /// never used" reads as "delete it".
    pub last_used_gap: bool,
}

#[derive(Clone, Debug)]
pub struct UserCreds {
    pub console: bool,
    pub mfa_devices: usize,
    pub keys: Vec<KeyFact>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sharing {
    pub public: bool,
    pub accounts: Vec<String>,
}

/// Results by resource id. A target missing from `results` wasn't read:
/// past the cap, or cached after the facts were fetched.
#[derive(Clone, Debug)]
pub struct PerResource<T> {
    pub results: HashMap<String, Fact<T>>,
    pub over_cap: usize,
}

impl<T> Default for PerResource<T> {
    fn default() -> Self {
        Self { results: HashMap::new(), over_cap: 0 }
    }
}

#[derive(Clone, Debug)]
pub struct AuditFacts {
    pub account: AccountFacts,
    pub kms_rotation: PerResource<bool>,
    pub iam_users: PerResource<UserCreds>,
    pub ebs_snapshots: PerResource<Sharing>,
    pub rds_snapshots: PerResource<Sharing>,
}

#[cfg(test)]
impl AuditFacts {
    /// An account where every account fact passes and nothing was read
    /// per resource.
    pub fn clean() -> Self {
        let on = PublicAccessBlockConfig {
            block_public_acls: true,
            ignore_public_acls: true,
            block_public_policy: true,
            restrict_public_buckets: true,
        };
        Self {
            account: AccountFacts {
                s3_block: Ok(Some(on)),
                ebs_default_encryption: Ok(true),
                trails: Ok(vec![TrailFact { name: "org".into(), multi_region: true, organization: true, logging: Some(true) }]),
                config_recorders: Ok(vec![RecorderFact { name: "default".into(), recording: true }]),
                guardduty_detectors: Ok(vec![DetectorFact { id: "d1".into(), enabled: true }]),
                analyzers: Ok(vec![AnalyzerFact { name: "acct".into(), kind: "ACCOUNT".into(), active: true }]),
                in_organization: Ok(false),
            },
            kms_rotation: PerResource::default(),
            iam_users: PerResource::default(),
            ebs_snapshots: PerResource::default(),
            rds_snapshots: PerResource::default(),
        }
    }
}

/// The resources the per-resource reads cover, picked from the cached lists.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FactTargets {
    pub kms_keys: Vec<String>,
    pub iam_users: Vec<String>,
    pub ebs_snapshots: Vec<String>,
    /// `(id, is_cluster)` — the two kinds have different attribute calls.
    pub rds_snapshots: Vec<(String, bool)>,
}

/// Customer-managed symmetric AWS_KMS keys: the only kind rotation applies
/// to (AWS-managed keys rotate on their own).
pub fn kms_key_rotatable(k: &KmsKey) -> bool {
    k.manager == "CUSTOMER"
        && k.key_spec == "SYMMETRIC_DEFAULT"
        && k.origin == "AWS_KMS"
        && k.state == "Enabled"
}

/// Only manual snapshots can be shared; automated ones can't.
pub fn rds_snapshot_shareable(s: &RdsSnapshot) -> bool {
    s.snapshot_type == "manual" && s.status == "available"
}

pub fn ebs_snapshot_shareable(s: &EbsSnapshot) -> bool {
    s.state == "completed"
}

impl FactTargets {
    pub fn from_input(input: &AuditInput) -> Self {
        Self {
            kms_keys: input
                .of::<KmsKey>(ServiceType::Kms)
                .filter(|k| kms_key_rotatable(k))
                .map(|k| k.key_id.clone())
                .collect(),
            iam_users: input.of::<IamUser>(ServiceType::IAM).map(|u| u.user_name.clone()).collect(),
            ebs_snapshots: input
                .of::<EbsSnapshot>(ServiceType::EC2)
                .filter(|s| ebs_snapshot_shareable(s))
                .map(|s| s.snapshot_id.clone())
                .collect(),
            rds_snapshots: input
                .of::<RdsSnapshot>(ServiceType::RDS)
                .filter(|s| rds_snapshot_shareable(s))
                .map(|s| (s.id.clone(), s.is_cluster))
                .collect(),
        }
    }
}

/// Fetch every fact. Clients are built up front so the future owns them
/// (it runs in a spawned task in the TUI).
pub fn fetch(clients: &AwsClients, targets: FactTargets) -> impl Future<Output = AuditFacts> + Send + 'static {
    let ec2 = clients.ec2_client();
    let rds = clients.rds_client();
    let kms = clients.kms_client();
    let iam = clients.iam_client();
    let sts = clients.sts_client();
    let s3control = clients.s3control_client();
    let cloudtrail = clients.cloudtrail_client();
    let config = clients.awsconfig_client();
    let guardduty = clients.guardduty_client();
    let analyzer = clients.accessanalyzer_client();
    let organizations = clients.organizations_client();
    async move {
        let account = async {
            let (s3_block, ebs_default_encryption, trails, config_recorders, guardduty_detectors, analyzers, in_organization) = tokio::join!(
                fetch_s3_account_block(&sts, &s3control),
                fetch_ebs_default_encryption(&ec2),
                fetch_trails(&cloudtrail),
                fetch_recorders(&config),
                fetch_detectors(&guardduty),
                fetch_analyzers(&analyzer),
                fetch_in_organization(&organizations),
            );
            AccountFacts { s3_block, ebs_default_encryption, trails, config_recorders, guardduty_detectors, analyzers, in_organization }
        };
        let (account, kms_rotation, iam_users, ebs_snapshots, rds_snapshots) = tokio::join!(
            account,
            per_resource(targets.kms_keys, |id| fetch_kms_rotation(kms.clone(), id)),
            per_resource(targets.iam_users, |name| fetch_user_creds(iam.clone(), name)),
            per_resource(targets.ebs_snapshots, |id| fetch_ebs_sharing(ec2.clone(), id)),
            per_resource(targets.rds_snapshots, |(id, cluster)| fetch_rds_sharing(rds.clone(), id, cluster)),
        );
        AuditFacts { account, kms_rotation, iam_users, ebs_snapshots, rds_snapshots }
    }
}

/// Run `f` over the first `MAX_PER_KIND` targets, `CONCURRENCY` at a time.
async fn per_resource<K, T, F, Fut>(targets: Vec<K>, f: F) -> PerResource<T>
where
    F: Fn(K) -> Fut,
    Fut: Future<Output = (String, Fact<T>)>,
{
    let over_cap = targets.len().saturating_sub(MAX_PER_KIND);
    let results = stream::iter(targets.into_iter().take(MAX_PER_KIND).map(f))
        .buffer_unordered(CONCURRENCY)
        .collect::<HashMap<_, _>>()
        .await;
    PerResource { results, over_cap }
}

// ── Account facts ─────────────────────────────────────────────────────────────

async fn fetch_s3_account_block(
    sts: &aws_sdk_sts::Client,
    s3control: &aws_sdk_s3control::Client,
) -> Fact<Option<PublicAccessBlockConfig>> {
    let ident = sts
        .get_caller_identity()
        .send()
        .await
        .map_err(|e| Gap::from_sdk("sts:GetCallerIdentity", &e))?;
    let account = ident.account().unwrap_or_default();
    match s3control.get_public_access_block().account_id(account).send().await {
        Ok(resp) => Ok(resp.public_access_block_configuration().map(|c| PublicAccessBlockConfig {
            block_public_acls: c.block_public_acls().unwrap_or(false),
            ignore_public_acls: c.ignore_public_acls().unwrap_or(false),
            block_public_policy: c.block_public_policy().unwrap_or(false),
            restrict_public_buckets: c.restrict_public_buckets().unwrap_or(false),
        })),
        Err(e) if e.as_service_error().is_some_and(|s| s.is_no_such_public_access_block_configuration()) => Ok(None),
        Err(e) => Err(Gap::from_sdk("s3:GetAccountPublicAccessBlock", &e)),
    }
}

async fn fetch_ebs_default_encryption(ec2: &aws_sdk_ec2::Client) -> Fact<bool> {
    ec2.get_ebs_encryption_by_default()
        .send()
        .await
        .map(|r| r.ebs_encryption_by_default().unwrap_or(false))
        .map_err(|e| Gap::from_sdk("ec2:GetEbsEncryptionByDefault", &e))
}

async fn fetch_trails(ct: &aws_sdk_cloudtrail::Client) -> Fact<Vec<TrailFact>> {
    let resp = ct
        .describe_trails()
        .send()
        .await
        .map_err(|e| Gap::from_sdk("cloudtrail:DescribeTrails", &e))?;
    let mut out = Vec::new();
    for t in resp.trail_list() {
        let multi_region = t.is_multi_region_trail().unwrap_or(false);
        // Only multi-region trails decide the check; skip the status call
        // for the rest.
        let logging = if multi_region {
            let arn = t.trail_arn().or(t.name()).unwrap_or_default();
            ct.get_trail_status().name(arn).send().await.ok().and_then(|s| s.is_logging())
        } else {
            None
        };
        out.push(TrailFact {
            name: t.name().unwrap_or_default().to_string(),
            multi_region,
            organization: t.is_organization_trail().unwrap_or(false),
            logging,
        });
    }
    Ok(out)
}

async fn fetch_recorders(config: &aws_sdk_config::Client) -> Fact<Vec<RecorderFact>> {
    let resp = config
        .describe_configuration_recorder_status()
        .send()
        .await
        .map_err(|e| Gap::from_sdk("config:DescribeConfigurationRecorderStatus", &e))?;
    Ok(resp
        .configuration_recorders_status()
        .iter()
        .map(|s| RecorderFact { name: s.name().unwrap_or_default().to_string(), recording: s.recording() })
        .collect())
}

async fn fetch_detectors(gd: &aws_sdk_guardduty::Client) -> Fact<Vec<DetectorFact>> {
    let resp = gd
        .list_detectors()
        .send()
        .await
        .map_err(|e| Gap::from_sdk("guardduty:ListDetectors", &e))?;
    let mut out = Vec::new();
    // A region has at most one detector.
    for id in resp.detector_ids() {
        let d = gd
            .get_detector()
            .detector_id(id)
            .send()
            .await
            .map_err(|e| Gap::from_sdk("guardduty:GetDetector", &e))?;
        let enabled = d.status().is_some_and(|s| s.as_str() == "ENABLED");
        out.push(DetectorFact { id: id.clone(), enabled });
    }
    Ok(out)
}

async fn fetch_analyzers(aa: &aws_sdk_accessanalyzer::Client) -> Fact<Vec<AnalyzerFact>> {
    let mut pages = aa.list_analyzers().into_paginator().send();
    let mut out = Vec::new();
    while let Some(page) = pages.next().await {
        let page = page.map_err(|e| Gap::from_sdk("access-analyzer:ListAnalyzers", &e))?;
        out.extend(page.analyzers().iter().map(|a| AnalyzerFact {
            name: a.name().to_string(),
            kind: a.r#type().as_str().to_string(),
            active: a.status().as_str() == "ACTIVE",
        }));
    }
    Ok(out)
}

/// `DescribeOrganization` answers from any account in an org, management or
/// member; `AWSOrganizationsNotInUseException` means a standalone account.
async fn fetch_in_organization(org: &aws_sdk_organizations::Client) -> Fact<bool> {
    match org.describe_organization().send().await {
        Ok(r) => Ok(r.organization().is_some()),
        Err(e) if e.as_service_error().is_some_and(|s| s.is_aws_organizations_not_in_use_exception()) => Ok(false),
        Err(e) => Err(Gap::from_sdk("organizations:DescribeOrganization", &e)),
    }
}

/// The `rds_snapshots` key: an instance snapshot and a cluster snapshot may
/// share an identifier, and they're different snapshots.
pub fn rds_snapshot_key(id: &str, is_cluster: bool) -> String {
    format!("{}:{id}", if is_cluster { "cluster" } else { "instance" })
}

// ── Per-resource reads ────────────────────────────────────────────────────────

async fn fetch_kms_rotation(kms: aws_sdk_kms::Client, key_id: String) -> (String, Fact<bool>) {
    let r = kms
        .get_key_rotation_status()
        .key_id(&key_id)
        .send()
        .await
        .map(|r| r.key_rotation_enabled())
        .map_err(|e| Gap::from_sdk("kms:GetKeyRotationStatus", &e));
    (key_id, r)
}

async fn fetch_user_creds(iam: aws_sdk_iam::Client, user: String) -> (String, Fact<UserCreds>) {
    let r = async {
        let (keys, mfa, login) = tokio::join!(
            iam.list_access_keys().user_name(&user).send(),
            iam.list_mfa_devices().user_name(&user).send(),
            iam.get_login_profile().user_name(&user).send(),
        );
        let keys = keys.map_err(|e| Gap::from_sdk("iam:ListAccessKeys", &e))?;
        let mfa = mfa.map_err(|e| Gap::from_sdk("iam:ListMFADevices", &e))?;
        // NoSuchEntity is the "no console password" answer.
        let console = match login {
            Ok(_) => true,
            Err(e) if e.as_service_error().is_some_and(|s| s.is_no_such_entity_exception()) => false,
            Err(e) => return Err(Gap::from_sdk("iam:GetLoginProfile", &e)),
        };
        let mut out = Vec::new();
        for k in keys.access_key_metadata() {
            let id = k.access_key_id().unwrap_or_default().to_string();
            let active = k.status().is_some_and(|s| s.as_str() == "Active");
            // Last-used matters only for active keys; a failed read marks it
            // unknown rather than failing the user.
            let (last_used_secs, last_used_gap) = if active && !id.is_empty() {
                match iam.get_access_key_last_used().access_key_id(&id).send().await {
                    Ok(r) => (r.access_key_last_used().and_then(|u| u.last_used_date()).map(|d| d.secs()), false),
                    Err(_) => (None, true),
                }
            } else {
                (None, false)
            };
            out.push(KeyFact {
                id,
                active,
                created_secs: k.create_date().map(|d| d.secs()),
                last_used_secs,
                last_used_gap,
            });
        }
        Ok(UserCreds { console, mfa_devices: mfa.mfa_devices().len(), keys: out })
    }
    .await;
    (user, r)
}

async fn fetch_ebs_sharing(ec2: aws_sdk_ec2::Client, id: String) -> (String, Fact<Sharing>) {
    let r = ec2
        .describe_snapshot_attribute()
        .snapshot_id(&id)
        .attribute(aws_sdk_ec2::types::SnapshotAttributeName::CreateVolumePermission)
        .send()
        .await
        .map(|r| {
            let perms = r.create_volume_permissions();
            Sharing {
                public: perms.iter().any(|p| p.group().is_some_and(|g| g.as_str() == "all")),
                accounts: perms.iter().filter_map(|p| p.user_id().map(String::from)).collect(),
            }
        })
        .map_err(|e| Gap::from_sdk("ec2:DescribeSnapshotAttribute", &e));
    (id, r)
}

/// The `restore` attribute lists who may restore: `all` = public.
fn restore_sharing<'a>(attrs: impl Iterator<Item = (Option<&'a str>, &'a [String])>) -> Sharing {
    let mut s = Sharing::default();
    for (name, values) in attrs {
        if name != Some("restore") {
            continue;
        }
        for v in values {
            if v == "all" {
                s.public = true;
            } else {
                s.accounts.push(v.clone());
            }
        }
    }
    s
}

async fn fetch_rds_sharing(rds: aws_sdk_rds::Client, id: String, cluster: bool) -> (String, Fact<Sharing>) {
    let r = if cluster {
        rds.describe_db_cluster_snapshot_attributes()
            .db_cluster_snapshot_identifier(&id)
            .send()
            .await
            .map(|r| {
                let attrs = r
                    .db_cluster_snapshot_attributes_result()
                    .map(|a| a.db_cluster_snapshot_attributes())
                    .unwrap_or_default();
                restore_sharing(attrs.iter().map(|a| (a.attribute_name(), a.attribute_values())))
            })
            .map_err(|e| Gap::from_sdk("rds:DescribeDBClusterSnapshotAttributes", &e))
    } else {
        rds.describe_db_snapshot_attributes()
            .db_snapshot_identifier(&id)
            .send()
            .await
            .map(|r| {
                let attrs = r
                    .db_snapshot_attributes_result()
                    .map(|a| a.db_snapshot_attributes())
                    .unwrap_or_default();
                restore_sharing(attrs.iter().map(|a| (a.attribute_name(), a.attribute_values())))
            })
            .map_err(|e| Gap::from_sdk("rds:DescribeDBSnapshotAttributes", &e))
    };
    (rds_snapshot_key(&id, cluster), r)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restore_attribute_all_is_public_and_ids_are_shares() {
        let values = vec!["all".to_string(), "111122223333".to_string()];
        let other = vec!["x".to_string()];
        let s = restore_sharing(
            [(Some("restore"), values.as_slice()), (Some("other"), other.as_slice())].into_iter(),
        );
        assert_eq!(s, Sharing { public: true, accounts: vec!["111122223333".into()] });
    }

    #[test]
    fn gap_note_names_the_action() {
        let g = Gap { action: "kms:GetKeyRotationStatus", denied: true, message: String::new() };
        assert_eq!(g.note(), "not checked — no permission for kms:GetKeyRotationStatus");
        let g = Gap { denied: false, ..g };
        assert_eq!(g.note(), "not checked — kms:GetKeyRotationStatus failed");
    }
}
