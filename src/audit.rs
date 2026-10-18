//! `@audit` — a virtual service whose rows are security findings computed
//! from other services' **warm caches** (#124, ADR 0006).
//!
//! Every check is a pure function over cached resource lists: it never calls
//! AWS, so the read-only guarantee holds by construction and each check is a
//! unit test over mocks. A check whose inputs aren't cached emits a dim
//! "not checked" coverage row instead, so "no findings" never reads as
//! "didn't look" (the `@all` rule, ADR 0004).

use std::any::Any;
use std::collections::HashMap;

use crate::aws::resource::{Resource, ResourceState};
use crate::aws::service::ServiceType;
use crate::aws::services::ec2::{Ami, EbsSnapshot, EbsVolume, Ec2Instance, SecurityGroup, SgRule};
use crate::aws::services::eks::EksCluster;
use crate::aws::services::iam::{AccessAnalyzerFinding, IamAccountSettings, IamUser};
use crate::aws::services::rds::{RdsCluster, RdsInstance, RdsSnapshot};
use crate::aws::services::kms::KmsKey;
use crate::aws::services::s3::{PublicAccessBlockConfig, S3Bucket};
use crate::aws::services::vpc::VpcSecurityGroup;

mod facts;
pub use facts::{fetch as fetch_facts, AuditFacts, FactTargets};
use facts::{Fact, Gap, PerResource};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    High,
    Medium,
    Low,
}

impl Severity {
    pub fn label(self) -> &'static str {
        match self {
            Severity::High => "high",
            Severity::Medium => "medium",
            Severity::Low => "low",
        }
    }

    /// Parse a `--fail-on` value.
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "high" => Some(Severity::High),
            "medium" | "med" => Some(Severity::Medium),
            "low" => Some(Severity::Low),
            _ => None,
        }
    }
}

/// One built-in check. `sources` are the services whose cached lists it
/// reads — **any** of them is enough to run it (security groups live under
/// both `@ec2` and `@vpc`).
pub struct Check {
    pub id: &'static str,
    pub title: &'static str,
    pub why: &'static str,
    pub sources: &'static [ServiceType],
    /// The detail section `⏎` lands on: the one that shows the problem.
    pub section: Option<&'static str>,
    /// Needs `AuditFacts` (#206) as well as a source list; without them it's
    /// a "not checked — r fetches account facts" row.
    pub facts: bool,
    run: fn(&'static Check, &AuditInput, &mut Vec<AuditFinding>),
}

/// The checks, in display order. The `facts: true` ones (#206) also read
/// `AuditFacts`, which `r` fetches after the source lists land.
static CHECKS: [Check; 24] = [
    Check {
        id: "sg-open-world",
        title: "Security group open to the world",
        why: "Inbound 0.0.0.0/0 or ::/0 lets anyone on the internet reach whatever the group is attached to. Admin and database ports (SSH, RDP, MySQL, Postgres…) are the ones scanned within minutes of exposure; 80/443 is normal for a public load balancer.",
        sources: &[ServiceType::EC2, ServiceType::VPC],
        section: Some("Inbound"),
        facts: false,
        run: check_sg_open_world,
    },
    Check {
        id: "rds-public",
        title: "RDS instance publicly accessible",
        why: "A publicly accessible instance gets a public DNS name resolving to a public IP. Its security groups are then the only thing between the database and the internet.",
        sources: &[ServiceType::RDS],
        section: Some("Config"),
        facts: false,
        run: check_rds_public,
    },
    Check {
        id: "rds-unencrypted",
        title: "RDS storage not encrypted",
        why: "Storage encryption can't be turned on for an existing instance or cluster — it takes a snapshot, an encrypted copy and a restore. Snapshots and read replicas inherit the setting.",
        sources: &[ServiceType::RDS],
        section: Some("Storage"),
        facts: false,
        run: check_rds_unencrypted,
    },
    Check {
        id: "rds-snapshot-unencrypted",
        title: "RDS snapshot not encrypted",
        why: "An unencrypted snapshot restores to an unencrypted database, and can be shared or copied without a KMS key gating it.",
        sources: &[ServiceType::RDS],
        section: None,
        facts: false,
        run: check_rds_snapshot_unencrypted,
    },
    Check {
        id: "ebs-unencrypted",
        title: "EBS volume not encrypted",
        why: "Unencrypted volumes produce unencrypted snapshots. Turning on EBS encryption-by-default covers new volumes only.",
        sources: &[ServiceType::EC2],
        section: Some("Details"),
        facts: false,
        run: check_ebs_unencrypted,
    },
    Check {
        id: "ebs-snapshot-unencrypted",
        title: "EBS snapshot not encrypted",
        why: "An unencrypted snapshot can be shared with another account (or made public) with no KMS key policy in the way.",
        sources: &[ServiceType::EC2],
        section: None,
        facts: false,
        run: check_ebs_snapshot_unencrypted,
    },
    Check {
        id: "ec2-imdsv1",
        title: "EC2 instance allows IMDSv1",
        why: "IMDSv1 answers unauthenticated GETs, so an SSRF bug in anything on the instance can read its role credentials. IMDSv2 (HttpTokens=required) needs a session token first.",
        sources: &[ServiceType::EC2],
        section: Some("Security"),
        facts: false,
        run: check_ec2_imdsv1,
    },
    Check {
        id: "ami-public",
        title: "AMI is public",
        why: "A public AMI can be launched by any AWS account — including whatever is baked into it: keys, config, application code.",
        sources: &[ServiceType::EC2],
        section: Some("Permissions"),
        facts: false,
        run: check_ami_public,
    },
    Check {
        id: "s3-public-access-block",
        title: "S3 bucket public access block not fully on",
        why: "With any of the four Block Public Access settings off, a bucket policy or ACL can make objects public. The account-level block covers every bucket: where it's on, the bucket's own setting is moot and the finding drops to low.",
        sources: &[ServiceType::S3],
        section: Some("Security"),
        facts: false,
        run: check_s3_public_access_block,
    },
    Check {
        id: "eks-public-endpoint",
        title: "EKS API endpoint open to the world",
        why: "The cluster's Kubernetes API is reachable from any IP. Authentication still applies, but the endpoint is exposed to credential stuffing and to any future API-server vulnerability.",
        sources: &[ServiceType::Eks],
        section: Some("Networking"),
        facts: false,
        run: check_eks_public_endpoint,
    },
    // Account-level checks read the IAM Account sub-tab's row, which the IAM
    // load already fetches (GetAccountSummary, GetAccountPasswordPolicy).
    Check {
        id: "iam-root-mfa",
        title: "Root user has no MFA",
        why: "The root user can do anything, including closing the account, and no policy can restrict it. Without MFA, its password is the only thing protecting it.",
        sources: &[ServiceType::IAM],
        section: Some("Overview"),
        facts: false,
        run: check_root_mfa,
    },
    Check {
        id: "iam-root-access-keys",
        title: "Root user has access keys",
        why: "A root access key is unrestricted, long-lived and can't be scoped by any policy. AWS's guidance is to have none; use roles instead.",
        sources: &[ServiceType::IAM],
        section: Some("Overview"),
        facts: false,
        run: check_root_access_keys,
    },
    Check {
        id: "iam-password-policy",
        title: "IAM password policy weak or missing",
        why: "The password policy applies to IAM users' console passwords. CIS asks for at least 14 characters and preventing reuse of the last 24. Accounts where nobody signs in as an IAM user (SSO only) can treat this as low priority.",
        sources: &[ServiceType::IAM],
        section: Some("Password Policy"),
        facts: false,
        run: check_password_policy,
    },
    // ── Account facts (#206): one read each, no list needed. ──
    Check {
        id: "s3-account-public-access-block",
        title: "S3 account-level public access block not fully on",
        why: "The account-level Block Public Access setting overrides every bucket's. With all four on, no bucket policy or ACL in the account can make data public, whatever the bucket says.",
        sources: &[],
        section: None,
        facts: true,
        run: check_s3_account_block,
    },
    Check {
        id: "ebs-default-encryption",
        title: "EBS encryption by default is off",
        why: "With it on, every new volume and snapshot copy in the region is encrypted, so nobody has to remember to tick the box. It's a per-region setting and doesn't touch existing volumes.",
        sources: &[],
        section: None,
        facts: true,
        run: check_ebs_default_encryption,
    },
    Check {
        id: "cloudtrail-multi-region",
        title: "No multi-region CloudTrail trail logging",
        why: "Event history keeps 90 days of management events and nothing else. A multi-region trail is the durable record of who did what, in every region — including the ones nobody watches, where an attacker would start.",
        sources: &[],
        section: None,
        facts: true,
        run: check_cloudtrail_multi_region,
    },
    Check {
        id: "config-recorder-off",
        title: "AWS Config recorder not recording",
        why: "Without a recording Config recorder there's no configuration history in this region, and Config rules, conformance packs and Security Hub controls have nothing to evaluate.",
        sources: &[],
        section: None,
        facts: true,
        run: check_config_recorder,
    },
    Check {
        id: "guardduty-off",
        title: "GuardDuty not enabled",
        why: "GuardDuty watches CloudTrail, VPC flow logs and DNS for compromised credentials, crypto-mining and reconnaissance. It's per-region; a region without it goes unwatched.",
        sources: &[],
        section: None,
        facts: true,
        run: check_guardduty,
    },
    Check {
        id: "access-analyzer-none",
        title: "No IAM Access Analyzer",
        why: "An external-access analyzer proves which resources are shared outside the account or made public. Without one in this region, the Access Analyzer relay below has nothing to relay — a clean result there means nothing.",
        sources: &[],
        section: None,
        facts: true,
        run: check_access_analyzer_present,
    },
    // ── Per-resource reads (#206), capped at facts::MAX_PER_KIND each. ──
    Check {
        id: "kms-rotation-off",
        title: "KMS key rotation off",
        why: "Automatic rotation gives a customer-managed key new key material every year, keeping the old for decryption. It limits how much data one compromised key version protects. Only symmetric AWS_KMS keys support it.",
        sources: &[ServiceType::Kms],
        section: Some("Details"),
        facts: true,
        run: check_kms_rotation,
    },
    Check {
        id: "iam-user-console-no-mfa",
        title: "IAM user has a console password but no MFA",
        why: "A console password with no MFA device can be phished or guessed, and nothing else stands in the way. CIS asks for MFA on every IAM user with a console password.",
        sources: &[ServiceType::IAM],
        section: Some("Access"),
        facts: true,
        run: check_user_console_mfa,
    },
    Check {
        id: "iam-access-key-old",
        title: "IAM access key older than 90 days",
        why: "Long-lived access keys leak through laptops, CI logs and repositories, and work from anywhere until someone deactivates them. CIS asks for rotation every 90 days; a key that's also unused should simply go.",
        sources: &[ServiceType::IAM],
        section: Some("Access"),
        facts: true,
        run: check_access_key_age,
    },
    Check {
        id: "ebs-snapshot-shared",
        title: "EBS snapshot public or shared",
        why: "A public snapshot can be copied by any AWS account, and a volume made from it shows the whole disk. Sharing with specific accounts is often deliberate, so it's reported low — check the list is the one you meant.",
        sources: &[ServiceType::EC2],
        section: None,
        facts: true,
        run: check_ebs_snapshot_sharing,
    },
    Check {
        id: "rds-snapshot-shared",
        title: "RDS snapshot public or shared",
        why: "A public DB snapshot can be restored by any AWS account into a database it controls. Sharing with specific accounts is often deliberate (cross-account backup), so it's reported low.",
        sources: &[ServiceType::RDS],
        section: None,
        facts: true,
        run: check_rds_snapshot_sharing,
    },
];

// The Access Analyzer pass-through isn't a `Check` row of its own design —
// AWS computed it — but it rides the same machinery.
static ACCESS_ANALYZER: Check = Check {
    id: "access-analyzer-public",
    title: "Public access (IAM Access Analyzer)",
    why: "IAM Access Analyzer proved from the resource's policy that anyone can access it. This is AWS's finding, relayed — open it for the principal, actions and conditions.",
    sources: &[ServiceType::IAM],
    section: None,
    facts: false,
    run: check_access_analyzer_public,
};

/// What `r` in `@audit` and `neboto audit` load: every service a check
/// reads. `@vpc` is left out — its security groups are the ones `@ec2` lists.
pub const SOURCES: &[ServiceType] = &[
    ServiceType::EC2,
    ServiceType::RDS,
    ServiceType::S3,
    ServiceType::Eks,
    ServiceType::IAM,
    ServiceType::Kms,
];

/// Every check, the Access Analyzer relay included.
pub fn all_checks() -> impl Iterator<Item = &'static Check> {
    CHECKS.iter().chain(std::iter::once(&ACCESS_ANALYZER))
}

fn by_id(id: &str) -> &'static Check {
    all_checks().find(|c| c.id == id).expect("a known check id")
}

/// Cached lists, one per service that had a warm cache.
pub struct AuditInput<'a> {
    lists: HashMap<ServiceType, &'a [Box<dyn Resource>]>,
    /// Sources whose load `r` has in flight — their checks say "loading…".
    loading: Vec<ServiceType>,
    /// For telling current-region S3 buckets from other-region stubs.
    region: String,
    /// Account facts and per-resource reads, once `r` has fetched them.
    facts: Option<&'a AuditFacts>,
    facts_loading: bool,
    /// For key ages. A field, not a clock read, so checks stay pure.
    now_secs: i64,
}

impl<'a> AuditInput<'a> {
    pub fn new(region: &str) -> Self {
        Self {
            lists: HashMap::new(),
            loading: Vec::new(),
            region: region.to_string(),
            facts: None,
            facts_loading: false,
            now_secs: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0),
        }
    }

    pub fn set_facts(&mut self, facts: Option<&'a AuditFacts>, loading: bool) {
        self.facts = facts;
        self.facts_loading = loading;
    }

    pub fn set_loading(&mut self, loading: &[ServiceType]) {
        self.loading = loading.to_vec();
    }

    pub fn add(&mut self, svc: ServiceType, list: &'a [Box<dyn Resource>]) {
        self.lists.insert(svc, list);
    }

    fn has(&self, svc: ServiceType) -> bool {
        self.lists.contains_key(&svc)
    }

    /// Every cached resource of type `T` from `svc`, with the service it
    /// came from (the jump target).
    fn of<T: 'static>(&self, svc: ServiceType) -> impl Iterator<Item = &'a T> + '_ {
        self.lists
            .get(&svc)
            .into_iter()
            .flat_map(|l| l.iter())
            .filter_map(|r| r.as_any().downcast_ref::<T>())
    }
}

/// What the audit produced: findings (worst first), then coverage notes.
pub fn run(input: &AuditInput) -> Vec<AuditFinding> {
    let mut findings = Vec::new();
    let mut notes = Vec::new();
    let mut ran = 0;
    for check in all_checks() {
        let listed = check.sources.is_empty() || check.sources.iter().any(|s| input.has(*s));
        if !listed {
            let loading = check.sources.iter().any(|s| input.loading.contains(s));
            notes.push(AuditFinding::not_checked(check, loading));
        } else if check.facts && input.facts.is_none() {
            notes.push(AuditFinding::facts_not_fetched(check, input.facts_loading));
        } else {
            ran += 1;
            (check.run)(check, input, &mut findings);
        }
    }
    // Fact checks report their own gaps (a denied call, past the cap) as
    // notes alongside their findings.
    let (gaps, real): (Vec<_>, Vec<_>) =
        findings.into_iter().partition(|f: &AuditFinding| f.kind == RowKind::NotChecked);
    let mut findings = real;
    notes.extend(gaps);
    notes.extend(s3_coverage_notes(input));
    findings.sort_by(|a, b| {
        a.severity
            .cmp(&b.severity)
            .then_with(|| a.check_id.cmp(b.check_id))
            .then_with(|| a.resource_name.cmp(&b.resource_name))
    });
    if findings.is_empty() && ran > 0 {
        findings.push(AuditFinding::all_clear(ran));
    }
    findings.extend(notes);
    findings
}

// ── Finding rows ──────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RowKind {
    Finding,
    /// A check that couldn't run, or ran on part of its input.
    NotChecked,
    /// Every check that ran came back clean.
    AllClear,
}

#[derive(Clone, Debug)]
pub struct AuditFinding {
    pub kind: RowKind,
    pub severity: Severity,
    pub check_id: &'static str,
    pub check_title: &'static str,
    pub why: &'static str,
    /// Where `⏎` goes: the resource's service, id and list type.
    pub service: Option<ServiceType>,
    pub resource_id: String,
    pub resource_name: String,
    pub resource_type: String,
    pub reason: String,
    /// See `Check::section`.
    pub section: Option<&'static str>,
    /// The list's second column: the reason for a finding, why not for a
    /// not-checked row.
    pub list_note: String,
    /// Extra content lines for the detail pane (e.g. the offending rules).
    pub evidence: Vec<String>,
    /// The resource's own tags, so `tag:team=x` narrows findings.
    pub tags: HashMap<String, String>,
    row_id: String,
}

impl AuditFinding {
    fn new(
        check: &Check,
        severity: Severity,
        service: ServiceType,
        r: &dyn Resource,
        reason: String,
    ) -> Self {
        Self {
            kind: RowKind::Finding,
            severity,
            check_id: check.id,
            check_title: check.title,
            why: check.why,
            section: check.section,
            service: Some(service),
            resource_id: r.id().to_string(),
            resource_name: r.name().to_string(),
            resource_type: r.resource_type().to_string(),
            list_note: reason.clone(),
            reason,
            evidence: Vec::new(),
            tags: r.tags().clone(),
            row_id: format!("{}:{}", check.id, r.id()),
        }
    }

    fn not_checked(check: &Check, loading: bool) -> Self {
        let load = check
            .sources
            .iter()
            .map(|s| s.prefix())
            .collect::<Vec<_>>()
            .join(" or ");
        if loading {
            return Self::note(
                check,
                check.title.to_string(),
                "loading…".to_string(),
                format!("{load} is loading; this check runs when it lands."),
            );
        }
        Self::note(
            check,
            check.title.to_string(),
            format!("not checked — {load} not loaded"),
            format!("{load} isn't cached, so this check didn't run. Press r here to load it (or visit {load})."),
        )
    }

    /// A fact check before `r` has fetched the facts.
    fn facts_not_fetched(check: &Check, loading: bool) -> Self {
        if loading {
            return Self::note(
                check,
                check.title.to_string(),
                "loading…".to_string(),
                "Account facts are being fetched; this check runs when they land.".to_string(),
            );
        }
        Self::note(
            check,
            check.title.to_string(),
            "not checked — press r to fetch account facts".to_string(),
            "This check reads data no list carries (an account setting, or one call per resource). Press r here to fetch it — read-only, capped calls.".to_string(),
        )
    }

    /// A fact the check needed couldn't be read.
    fn gap(check: &Check, subject: String, gap: &Gap) -> Self {
        let why = if gap.denied {
            format!("{} was denied, so the posture is unknown — not assumed bad or good. ({})", gap.action, gap.message)
        } else {
            format!("{} failed: {}", gap.action, gap.message)
        };
        Self::note(check, subject, gap.note(), why)
    }

    /// A finding about the account or region rather than one resource:
    /// nothing to jump to, so `⏎` reads it.
    fn account(check: &'static Check, severity: Severity, subject: String, reason: String) -> Self {
        Self {
            kind: RowKind::Finding,
            severity,
            check_id: check.id,
            check_title: check.title,
            why: check.why,
            section: None,
            service: None,
            resource_id: String::new(),
            row_id: format!("{}:{}", check.id, subject),
            resource_name: subject,
            resource_type: "Account".to_string(),
            list_note: reason.clone(),
            reason,
            evidence: Vec::new(),
            tags: HashMap::new(),
        }
    }

    fn note(check: &Check, name: String, list_note: String, reason: String) -> Self {
        Self {
            row_id: format!("not-checked:{}:{}", check.id, name),
            kind: RowKind::NotChecked,
            severity: Severity::Low,
            check_id: check.id,
            check_title: check.title,
            why: check.why,
            section: None,
            service: None,
            resource_id: String::new(),
            resource_name: name,
            resource_type: "Not checked".to_string(),
            list_note,
            reason,
            evidence: Vec::new(),
            tags: HashMap::new(),
        }
    }

    fn all_clear(ran: usize) -> Self {
        Self {
            kind: RowKind::AllClear,
            severity: Severity::Low,
            check_id: "all-clear",
            check_title: "No findings",
            why: "",
            section: None,
            service: None,
            resource_id: String::new(),
            resource_name: format!("✓ no findings — {ran} check{} ran clean", if ran == 1 { "" } else { "s" }),
            resource_type: "Summary".to_string(),
            list_note: String::new(),
            reason: String::new(),
            evidence: Vec::new(),
            tags: HashMap::new(),
            row_id: "all-clear".to_string(),
        }
    }

    pub fn is_finding(&self) -> bool {
        self.kind == RowKind::Finding
    }
}

impl Resource for AuditFinding {
    fn id(&self) -> &str {
        &self.row_id
    }

    fn name(&self) -> &str {
        &self.resource_name
    }

    fn resource_type(&self) -> &str {
        match self.kind {
            RowKind::Finding => self.check_title,
            RowKind::NotChecked => "Not checked",
            RowKind::AllClear => "Summary",
        }
    }

    /// Severity is the state: `F` filters by it, `z`'s state sort ranks it.
    fn state(&self) -> ResourceState {
        match (&self.kind, self.severity) {
            (RowKind::Finding, Severity::High) => ResourceState::Unavailable,
            (RowKind::Finding, Severity::Medium) => ResourceState::Pending,
            (RowKind::Finding, Severity::Low) => ResourceState::Unknown("low".into()),
            (RowKind::AllClear, _) => ResourceState::Available,
            (RowKind::NotChecked, _) => ResourceState::stateless(),
        }
    }

    fn state_label(&self) -> String {
        match self.kind {
            RowKind::Finding => self.severity.label().to_string(),
            _ => String::new(),
        }
    }

    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    fn search_text(&self) -> String {
        format!(
            "{} {} {} {} {} {} {} {}",
            self.check_id,
            self.check_title,
            self.resource_id,
            self.resource_name,
            self.resource_type,
            self.reason,
            self.state_label(),
            self.service.as_ref().map(|s| s.prefix()).unwrap_or_default(),
        )
    }

    fn details(&self) -> Vec<(String, String)> {
        let mut rows = Vec::new();
        match self.kind {
            RowKind::Finding => {
                // The generic header above already shows the name, the check
                // title (Type) and the severity (State).
                rows.push(("Check".into(), self.check_id.into()));
                // The jump row: ⏎ opens the resource in its service.
                rows.push(("Resource ID".into(), self.resource_id.clone()));
                if let Some(svc) = self.service {
                    rows.push(("Resource type".into(), format!("{} ({})", self.resource_type, svc.prefix())));
                }
                rows.push(("Finding".into(), self.reason.clone()));
                if !self.evidence.is_empty() {
                    rows.push((String::new(), String::new()));
                    rows.push(("Evidence".into(), String::new()));
                    for e in &self.evidence {
                        rows.push((format!("  {e}"), String::new()));
                    }
                }
            }
            RowKind::NotChecked => {
                rows.push(("Check".into(), format!("{} ({})", self.check_title, self.check_id)));
                rows.push((String::new(), String::new()));
                rows.push((format!("  {}", self.reason), String::new()));
            }
            RowKind::AllClear => {
                rows.push(("Result".into(), self.resource_name.clone()));
                rows.push((String::new(), String::new()));
                rows.push((
                    "  · Clean over what's cached. Rows below say which checks didn't run.".into(),
                    String::new(),
                ));
            }
        }
        if !self.why.is_empty() {
            rows.push((String::new(), String::new()));
            rows.push(("Why it matters".into(), String::new()));
            for line in wrap(self.why, 72) {
                rows.push((format!("  {line}"), String::new()));
            }
        }
        rows
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
}

/// Greedy word wrap for the "why" paragraphs (content lines don't wrap
/// unless `Ctrl-W` is on).
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut cur = String::new();
    for word in text.split_whitespace() {
        if !cur.is_empty() && cur.len() + 1 + word.len() > width {
            lines.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(word);
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines
}

// ── Checks ────────────────────────────────────────────────────────────────────

/// Ports where world exposure is High: remote admin, databases, caches and
/// unauthenticated-by-default control planes.
const RISKY_PORTS: &[u16] = &[
    21, 22, 23, 135, 139, 445, 1433, 1521, 2375, 2379, 3306, 3389, 5432, 5601, 5900, 5985,
    5986, 6379, 9200, 11211, 27017,
];

fn port_range(ports: &str) -> Option<(u16, u16)> {
    if ports.eq_ignore_ascii_case("all") {
        return Some((0, u16::MAX));
    }
    match ports.split_once('-') {
        Some((a, b)) => Some((a.trim().parse().ok()?, b.trim().parse().ok()?)),
        None => {
            let p = ports.trim().parse().ok()?;
            Some((p, p))
        }
    }
}

/// How bad one world-open rule is.
fn sg_rule_severity(rule: &SgRule) -> Severity {
    if rule.protocol.eq_ignore_ascii_case("icmp") || rule.protocol.eq_ignore_ascii_case("icmpv6") {
        return Severity::Low;
    }
    let Some((from, to)) = port_range(&rule.ports) else {
        return Severity::Medium;
    };
    if from == 0 && to == u16::MAX {
        return Severity::High;
    }
    if RISKY_PORTS.iter().any(|p| (from..=to).contains(p)) {
        Severity::High
    } else if (from == 80 || from == 443) && from == to {
        Severity::Low
    } else {
        Severity::Medium
    }
}

fn describe_rule(rule: &SgRule) -> String {
    if rule.protocol.eq_ignore_ascii_case("all") {
        "all traffic".to_string()
    } else if rule.ports.eq_ignore_ascii_case("all") {
        format!("all {}", rule.protocol)
    } else {
        format!("{} {}", rule.protocol, rule.ports)
    }
}

fn check_sg_open_world(check: &'static Check, input: &AuditInput, out: &mut Vec<AuditFinding>) {
    // The same group can be cached under @ec2 and @vpc; report it once,
    // preferring @ec2 as the jump target.
    let mut seen = std::collections::HashSet::new();
    let ec2 = input
        .of::<SecurityGroup>(ServiceType::EC2)
        .map(|g| (ServiceType::EC2, g as &dyn Resource, &g.inbound_rules));
    let vpc = input
        .of::<VpcSecurityGroup>(ServiceType::VPC)
        .map(|g| (ServiceType::VPC, g as &dyn Resource, &g.inbound_rules));
    for (svc, res, inbound) in ec2.chain(vpc) {
        if !seen.insert(res.id().to_string()) {
            continue;
        }
        let world: Vec<&SgRule> = inbound.iter().filter(|r| r.is_world()).collect();
        let Some(severity) = world.iter().map(|r| sg_rule_severity(r)).min() else {
            continue;
        };
        // v4 + v6 rules on the same port read as one.
        let mut what: Vec<String> = Vec::new();
        for w in world.iter().map(|r| describe_rule(r)) {
            if !what.contains(&w) {
                what.push(w);
            }
        }
        let mut f = AuditFinding::new(
            check,
            severity,
            svc,
            res,
            format!("inbound from anywhere on {}", what.join(", ")),
        );
        f.evidence = world
            .iter()
            .map(|r| {
                let rule = format!("{:<5}  {:<11}  ←  {}", r.protocol, r.ports, r.source);
                match &r.description {
                    Some(d) => format!("{rule}\t{d}"),
                    None => rule,
                }
            })
            .collect();
        f.evidence.push(format!("· press N on {} for the effective access across groups", res.id()));
        out.push(f);
    }
}

fn check_rds_public(check: &'static Check, input: &AuditInput, out: &mut Vec<AuditFinding>) {
    for db in input.of::<RdsInstance>(ServiceType::RDS) {
        if db.publicly_accessible {
            out.push(AuditFinding::new(
                check,
                Severity::High,
                ServiceType::RDS,
                db,
                "PubliclyAccessible = true".into(),
            ));
        }
    }
}

fn check_rds_unencrypted(check: &'static Check, input: &AuditInput, out: &mut Vec<AuditFinding>) {
    for db in input.of::<RdsInstance>(ServiceType::RDS) {
        // A cluster member's storage is the cluster's — reported once there.
        if !db.storage_encrypted && db.cluster_identifier.is_none() {
            out.push(AuditFinding::new(
                check,
                Severity::Medium,
                ServiceType::RDS,
                db,
                "StorageEncrypted = false".into(),
            ));
        }
    }
    for c in input.of::<RdsCluster>(ServiceType::RDS) {
        if !c.storage_encrypted {
            out.push(AuditFinding::new(
                check,
                Severity::Medium,
                ServiceType::RDS,
                c,
                "StorageEncrypted = false (cluster)".into(),
            ));
        }
    }
}

fn check_rds_snapshot_unencrypted(check: &'static Check, input: &AuditInput, out: &mut Vec<AuditFinding>) {
    for s in input.of::<RdsSnapshot>(ServiceType::RDS) {
        if !s.encrypted {
            out.push(AuditFinding::new(
                check,
                Severity::Low,
                ServiceType::RDS,
                s,
                "Encrypted = false".into(),
            ));
        }
    }
}

fn check_ebs_unencrypted(check: &'static Check, input: &AuditInput, out: &mut Vec<AuditFinding>) {
    for v in input.of::<EbsVolume>(ServiceType::EC2) {
        if !v.encrypted {
            let reason = match &v.attached_instance_id {
                Some(i) => format!("Encrypted = false (attached to {i})"),
                None => "Encrypted = false (unattached)".to_string(),
            };
            out.push(AuditFinding::new(check, Severity::Medium, ServiceType::EC2, v, reason));
        }
    }
}

fn check_ebs_snapshot_unencrypted(check: &'static Check, input: &AuditInput, out: &mut Vec<AuditFinding>) {
    for s in input.of::<EbsSnapshot>(ServiceType::EC2) {
        if !s.encrypted {
            out.push(AuditFinding::new(
                check,
                Severity::Low,
                ServiceType::EC2,
                s,
                "Encrypted = false".into(),
            ));
        }
    }
}

fn check_ec2_imdsv1(check: &'static Check, input: &AuditInput, out: &mut Vec<AuditFinding>) {
    for i in input.of::<Ec2Instance>(ServiceType::EC2) {
        if matches!(i.state.as_str(), "terminated" | "shutting-down") {
            continue;
        }
        let Some(md) = &i.metadata else { continue };
        if md.http_endpoint.as_deref() == Some("disabled") {
            continue;
        }
        if md.http_tokens.as_deref() == Some("optional") {
            out.push(AuditFinding::new(
                check,
                Severity::Medium,
                ServiceType::EC2,
                i,
                "HttpTokens = optional (IMDSv1 allowed)".into(),
            ));
        }
    }
}

fn check_ami_public(check: &'static Check, input: &AuditInput, out: &mut Vec<AuditFinding>) {
    for a in input.of::<Ami>(ServiceType::EC2) {
        if a.public {
            out.push(AuditFinding::new(
                check,
                Severity::High,
                ServiceType::EC2,
                a,
                "Public = true — launchable by any AWS account".into(),
            ));
        }
    }
}

fn check_s3_public_access_block(check: &'static Check, input: &AuditInput, out: &mut Vec<AuditFinding>) {
    for b in input.of::<S3Bucket>(ServiceType::S3) {
        if b.region != input.region {
            continue; // other-region stub: no Tier-1 data, see s3_coverage_notes
        }
        let Some(pab) = &b.public_access_block else { continue };
        if pab.all_blocked() {
            continue;
        }
        let off: Vec<&str> = block_flags(pab).iter().filter(|(on, _)| !on).map(|(_, n)| *n).collect();
        let mut reason = if off.len() == 4 {
            "no bucket-level public access block".to_string()
        } else {
            format!("off: {}", off.join(", "))
        };
        // The account-level block is OR'd with the bucket's, per setting.
        let mut severity = Severity::Medium;
        if let Some(Ok(account)) = input.facts.map(|f| &f.account.s3_block) {
            let account_on: Vec<&str> = account
                .as_ref()
                .map(|a| block_flags(a).iter().filter(|(on, _)| *on).map(|(_, n)| *n).collect())
                .unwrap_or_default();
            if off.iter().all(|n| account_on.contains(n)) {
                severity = Severity::Low;
                reason.push_str(" — covered by the account-level block");
            } else if off.iter().any(|n| account_on.contains(n)) {
                let still: Vec<&str> = off.iter().copied().filter(|n| !account_on.contains(n)).collect();
                reason.push_str(&format!(" — account block leaves {} off", still.join(", ")));
            }
        }
        out.push(AuditFinding::new(check, severity, ServiceType::S3, b, reason));
    }
}

fn block_flags(pab: &PublicAccessBlockConfig) -> [(bool, &'static str); 4] {
    [
        (pab.block_public_acls, "BlockPublicAcls"),
        (pab.ignore_public_acls, "IgnorePublicAcls"),
        (pab.block_public_policy, "BlockPublicPolicy"),
        (pab.restrict_public_buckets, "RestrictPublicBuckets"),
    ]
}

/// S3 rows the check had to skip: other-region stubs (no Tier-1 fetch) and
/// buckets whose public-access-block read failed.
fn s3_coverage_notes(input: &AuditInput) -> Vec<AuditFinding> {
    if !input.has(ServiceType::S3) {
        return Vec::new();
    }
    let (mut other_region, mut unknown) = (0, 0);
    for b in input.of::<S3Bucket>(ServiceType::S3) {
        if b.region != input.region {
            other_region += 1;
        } else if b.public_access_block.is_none() {
            unknown += 1;
        }
    }
    let mut notes = Vec::new();
    let check = by_id("s3-public-access-block");
    if other_region > 0 {
        notes.push(AuditFinding::note(
            check,
            format!("{other_region} bucket{} in other regions", plural(other_region)),
            "not checked — switch region (R)".into(),
            "Buckets outside the current region are listed without their settings. Switch region (R) to audit them.".into(),
        ));
    }
    if unknown > 0 {
        notes.push(AuditFinding::note(
            check,
            format!("{unknown} bucket{}: public access block unreadable", plural(unknown)),
            "not checked — no permission?".into(),
            "GetPublicAccessBlock failed (usually AccessDenied), so the posture is unknown — not assumed open.".into(),
        ));
    }
    notes
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

fn check_eks_public_endpoint(check: &'static Check, input: &AuditInput, out: &mut Vec<AuditFinding>) {
    for c in input.of::<EksCluster>(ServiceType::Eks) {
        if c.endpoint_public
            && (c.public_access_cidrs.is_empty()
                || c.public_access_cidrs.iter().any(|x| x == "0.0.0.0/0" || x == "::/0"))
        {
            out.push(AuditFinding::new(
                check,
                Severity::Medium,
                ServiceType::Eks,
                c,
                "public endpoint, allowed from 0.0.0.0/0".into(),
            ));
        }
    }
}

/// The IAM Account sub-tab's one synthetic row, with a readable summary.
fn account_settings<'a>(input: &AuditInput<'a>) -> Option<&'a IamAccountSettings> {
    input
        .of::<IamAccountSettings>(ServiceType::IAM)
        .find(|a| a.summary_error.is_none())
}

fn check_root_mfa(check: &'static Check, input: &AuditInput, out: &mut Vec<AuditFinding>) {
    if let Some(a) = account_settings(input) {
        if a.summary_value("AccountMFAEnabled") == Some(0) {
            out.push(AuditFinding::new(
                check,
                Severity::High,
                ServiceType::IAM,
                a,
                "AccountMFAEnabled = 0".into(),
            ));
        }
    }
}

fn check_root_access_keys(check: &'static Check, input: &AuditInput, out: &mut Vec<AuditFinding>) {
    if let Some(a) = account_settings(input) {
        if let Some(n) = a.summary_value("AccountAccessKeysPresent").filter(|n| *n > 0) {
            out.push(AuditFinding::new(
                check,
                Severity::High,
                ServiceType::IAM,
                a,
                format!("AccountAccessKeysPresent = {n}"),
            ));
        }
    }
}

fn check_password_policy(check: &'static Check, input: &AuditInput, out: &mut Vec<AuditFinding>) {
    let Some(a) = input
        .of::<IamAccountSettings>(ServiceType::IAM)
        .find(|a| a.pw_policy_error.is_none())
    else {
        return;
    };
    let (severity, reason) = match &a.password_policy {
        None => (Severity::Medium, "no password policy (AWS default: 8 characters, no reuse limit)".to_string()),
        Some(p) => {
            let mut short = Vec::new();
            if p.minimum_length.unwrap_or(0) < 14 {
                short.push(format!("minimum length {}", p.minimum_length.unwrap_or(0)));
            }
            if p.reuse_prevention.unwrap_or(0) < 24 {
                short.push(format!("reuse prevention {}", p.reuse_prevention.unwrap_or(0)));
            }
            if short.is_empty() {
                return;
            }
            (Severity::Low, format!("{} — CIS asks for length ≥ 14, reuse ≥ 24", short.join(", ")))
        }
    };
    out.push(AuditFinding::new(check, severity, ServiceType::IAM, a, reason));
}

fn check_access_analyzer_public(check: &'static Check, input: &AuditInput, out: &mut Vec<AuditFinding>) {
    for f in input.of::<AccessAnalyzerFinding>(ServiceType::IAM) {
        if f.status == "ACTIVE" && f.is_public {
            let mut row = AuditFinding::new(
                check,
                Severity::High,
                ServiceType::IAM,
                f,
                format!("{} {} is public", f.resource_type, f.resource),
            );
            row.resource_name = f.resource.clone();
            out.push(row);
        }
    }
}

// ── Fact checks (#206) ────────────────────────────────────────────────────────

/// The fact a check reads; `run` only calls a `facts: true` check once
/// they're present.
fn facts<'a>(input: &AuditInput<'a>) -> &'a AuditFacts {
    input.facts.expect("run() gates facts checks")
}

/// A fact's value, or its gap pushed as a note.
fn read_fact<'f, T>(
    check: &'static Check,
    subject: &str,
    fact: &'f Fact<T>,
    out: &mut Vec<AuditFinding>,
) -> Option<&'f T> {
    match fact {
        Ok(v) => Some(v),
        Err(gap) => {
            out.push(AuditFinding::gap(check, subject.to_string(), gap));
            None
        }
    }
}

fn check_s3_account_block(check: &'static Check, input: &AuditInput, out: &mut Vec<AuditFinding>) {
    let subject = "Account S3 Block Public Access";
    let Some(block) = read_fact(check, subject, &facts(input).account.s3_block, out) else { return };
    let reason = match block {
        None => "no account-level public access block".to_string(),
        Some(b) if b.all_blocked() => return,
        Some(b) => {
            let off: Vec<&str> = block_flags(b).iter().filter(|(on, _)| !on).map(|(_, n)| *n).collect();
            format!("off: {}", off.join(", "))
        }
    };
    out.push(AuditFinding::account(check, Severity::Medium, subject.to_string(), reason));
}

fn check_ebs_default_encryption(check: &'static Check, input: &AuditInput, out: &mut Vec<AuditFinding>) {
    let subject = format!("EBS default encryption ({})", input.region);
    let Some(on) = read_fact(check, &subject, &facts(input).account.ebs_default_encryption, out) else { return };
    if !on {
        out.push(AuditFinding::account(check, Severity::Low, subject, "EbsEncryptionByDefault = false".into()));
    }
}

fn check_cloudtrail_multi_region(check: &'static Check, input: &AuditInput, out: &mut Vec<AuditFinding>) {
    let subject = "CloudTrail";
    let Some(trails) = read_fact(check, subject, &facts(input).account.trails, out) else { return };
    let multi: Vec<_> = trails.iter().filter(|t| t.multi_region).collect();
    if multi.iter().any(|t| t.logging == Some(true)) {
        return;
    }
    // A multi-region trail whose status couldn't be read may well be
    // logging: unknown, not a finding.
    if let Some(t) = multi.iter().find(|t| t.logging.is_none()) {
        out.push(AuditFinding::note(
            check,
            format!("trail {}", t.name),
            "not checked — trail status unreadable".into(),
            format!("GetTrailStatus failed for the multi-region trail {} (no permission, or it's homed in another region), so whether it's logging is unknown.", t.name),
        ));
        return;
    }
    let reason = if multi.is_empty() {
        match trails.len() {
            0 => "no trails".to_string(),
            n => format!("{n} single-region trail{}, none multi-region", plural(n)),
        }
    } else {
        let names: Vec<&str> = multi.iter().map(|t| t.name.as_str()).collect();
        format!("multi-region trail{} not logging: {}", plural(multi.len()), names.join(", "))
    };
    let mut f = AuditFinding::account(check, Severity::High, subject.to_string(), reason);
    f.evidence = trails
        .iter()
        .map(|t| {
            let scope = if t.organization { "organization" } else if t.multi_region { "multi-region" } else { "single-region" };
            let logging = match t.logging {
                Some(true) => "logging",
                Some(false) => "stopped",
                None => "status not read",
            };
            format!("{:<32}  {scope:<13}  {logging}", t.name)
        })
        .collect();
    out.push(f);
}

fn check_config_recorder(check: &'static Check, input: &AuditInput, out: &mut Vec<AuditFinding>) {
    let subject = format!("AWS Config ({})", input.region);
    let Some(recorders) = read_fact(check, &subject, &facts(input).account.config_recorders, out) else { return };
    if recorders.iter().any(|r| r.recording) {
        return;
    }
    let reason = if recorders.is_empty() {
        "no configuration recorder".to_string()
    } else {
        let names: Vec<&str> = recorders.iter().map(|r| r.name.as_str()).collect();
        format!("recorder stopped: {}", names.join(", "))
    };
    out.push(AuditFinding::account(check, Severity::Medium, subject, reason));
}

fn check_guardduty(check: &'static Check, input: &AuditInput, out: &mut Vec<AuditFinding>) {
    let subject = format!("GuardDuty ({})", input.region);
    let Some(detectors) = read_fact(check, &subject, &facts(input).account.guardduty_detectors, out) else { return };
    if detectors.iter().any(|d| d.enabled) {
        return;
    }
    let reason = match detectors.first() {
        None => "no detector in this region".to_string(),
        Some(d) => format!("detector {} suspended", d.id),
    };
    out.push(AuditFinding::account(check, Severity::Medium, subject, reason));
}

fn check_access_analyzer_present(check: &'static Check, input: &AuditInput, out: &mut Vec<AuditFinding>) {
    let subject = format!("IAM Access Analyzer ({})", input.region);
    let Some(analyzers) = read_fact(check, &subject, &facts(input).account.analyzers, out) else { return };
    // Unused-access analyzers don't find external access.
    let external = |a: &&facts::AnalyzerFact| matches!(a.kind.as_str(), "ACCOUNT" | "ORGANIZATION");
    if analyzers.iter().filter(external).any(|a| a.active) {
        return;
    }
    let reason = match analyzers.iter().find(external) {
        Some(a) => format!("analyzer {} not active", a.name),
        None => "no external-access analyzer in this region".to_string(),
    };
    out.push(AuditFinding::account(check, Severity::Low, subject, reason));
}

/// Coverage for a per-resource read: denied/failed calls grouped by note,
/// plus targets with no result (past the cap, or cached after `r`).
fn per_resource_notes<T>(
    check: &'static Check,
    noun: &str,
    facts: &PerResource<T>,
    targets: impl Iterator<Item = String>,
    out: &mut Vec<AuditFinding>,
) {
    let mut gaps: Vec<(Gap, usize)> = Vec::new();
    let mut missing = 0;
    for id in targets {
        match facts.results.get(&id) {
            Some(Err(g)) => match gaps.iter_mut().find(|(seen, _)| seen.note() == g.note()) {
                Some((_, n)) => *n += 1,
                None => gaps.push((g.clone(), 1)),
            },
            Some(Ok(_)) => {}
            None => missing += 1,
        }
    }
    for (gap, n) in gaps {
        out.push(AuditFinding::gap(check, format!("{n} {noun}{}", plural(n)), &gap));
    }
    if missing > 0 {
        let why = if facts.over_cap > 0 {
            format!("Per-resource reads stop at {} {noun}s, so the rest weren't checked.", facts::MAX_PER_KIND)
        } else {
            format!("These {noun}s were cached after the facts were fetched. Press r to read them.")
        };
        out.push(AuditFinding::note(
            check,
            format!("{missing} {noun}{}", plural(missing)),
            "not checked — not read (cap, or press r)".into(),
            why,
        ));
    }
}

fn check_kms_rotation(check: &'static Check, input: &AuditInput, out: &mut Vec<AuditFinding>) {
    let rotation = &facts(input).kms_rotation;
    let keys: Vec<&KmsKey> = input.of::<KmsKey>(ServiceType::Kms).filter(|k| facts::kms_key_rotatable(k)).collect();
    for k in &keys {
        if let Some(Ok(false)) = rotation.results.get(&k.key_id) {
            let mut f = AuditFinding::new(check, Severity::Medium, ServiceType::Kms, *k, "KeyRotationEnabled = false".into());
            if let Some(alias) = k.aliases.first() {
                f.resource_name = alias.clone();
            }
            out.push(f);
        }
    }
    per_resource_notes(check, "key", rotation, keys.iter().map(|k| k.key_id.clone()), out);
}

const DAY: i64 = 86_400;
const KEY_MAX_AGE_DAYS: i64 = 90;

fn check_user_console_mfa(check: &'static Check, input: &AuditInput, out: &mut Vec<AuditFinding>) {
    let creds = &facts(input).iam_users;
    let users: Vec<&IamUser> = input.of::<IamUser>(ServiceType::IAM).collect();
    for u in &users {
        if let Some(Ok(c)) = creds.results.get(&u.user_name) {
            if c.console && c.mfa_devices == 0 {
                out.push(AuditFinding::new(
                    check,
                    Severity::High,
                    ServiceType::IAM,
                    *u,
                    "console password, no MFA device".into(),
                ));
            }
        }
    }
    per_resource_notes(check, "user", creds, users.iter().map(|u| u.user_name.clone()), out);
}

fn check_access_key_age(check: &'static Check, input: &AuditInput, out: &mut Vec<AuditFinding>) {
    let creds = &facts(input).iam_users;
    let users: Vec<&IamUser> = input.of::<IamUser>(ServiceType::IAM).collect();
    let days = |secs: i64| (input.now_secs - secs) / DAY;
    for u in &users {
        let Some(Ok(c)) = creds.results.get(&u.user_name) else { continue };
        let old: Vec<String> = c
            .keys
            .iter()
            .filter(|k| k.active)
            .filter_map(|k| {
                let age = days(k.created_secs?);
                (age > KEY_MAX_AGE_DAYS).then(|| {
                    let used = match k.last_used_secs {
                        None if k.last_used_gap => {
                            "last use unknown — iam:GetAccessKeyLastUsed failed".to_string()
                        }
                        Some(t) => match days(t) {
                            0 => "last used today".to_string(),
                            1 => "last used yesterday".to_string(),
                            d => format!("last used {d} days ago"),
                        },
                        None => "never used".to_string(),
                    };
                    format!("{} {age} days old, {used}", k.id)
                })
            })
            .collect();
        if old.is_empty() {
            continue;
        }
        let mut f = AuditFinding::new(check, Severity::Medium, ServiceType::IAM, *u, old[0].clone());
        if old.len() > 1 {
            f.list_note = format!("{} active keys older than {KEY_MAX_AGE_DAYS} days", old.len());
            f.reason = f.list_note.clone();
        }
        f.evidence = old;
        out.push(f);
    }
    // Same reads as the MFA check, but a user that couldn't be read is a gap
    // for this check too — skipping it silently would read as "no old keys".
    per_resource_notes(check, "user", creds, users.iter().map(|u| u.user_name.clone()), out);
}

/// High when public, low when shared with named accounts.
fn sharing_finding(
    check: &'static Check,
    svc: ServiceType,
    r: &dyn Resource,
    sharing: &facts::Sharing,
) -> Option<AuditFinding> {
    let (severity, reason) = if sharing.public {
        (Severity::High, "public — restorable by any AWS account".to_string())
    } else if !sharing.accounts.is_empty() {
        let n = sharing.accounts.len();
        (Severity::Low, format!("shared with {n} account{}", plural(n)))
    } else {
        return None;
    };
    let mut f = AuditFinding::new(check, severity, svc, r, reason);
    f.evidence = sharing.accounts.iter().map(|a| format!("shared with {a}")).collect();
    Some(f)
}

fn check_ebs_snapshot_sharing(check: &'static Check, input: &AuditInput, out: &mut Vec<AuditFinding>) {
    let sharing = &facts(input).ebs_snapshots;
    let snaps: Vec<&EbsSnapshot> = input.of::<EbsSnapshot>(ServiceType::EC2).filter(|s| facts::ebs_snapshot_shareable(s)).collect();
    for s in &snaps {
        if let Some(Ok(sh)) = sharing.results.get(&s.snapshot_id) {
            out.extend(sharing_finding(check, ServiceType::EC2, *s, sh));
        }
    }
    per_resource_notes(check, "snapshot", sharing, snaps.iter().map(|s| s.snapshot_id.clone()), out);
}

fn check_rds_snapshot_sharing(check: &'static Check, input: &AuditInput, out: &mut Vec<AuditFinding>) {
    let sharing = &facts(input).rds_snapshots;
    let snaps: Vec<&RdsSnapshot> = input.of::<RdsSnapshot>(ServiceType::RDS).filter(|s| facts::rds_snapshot_shareable(s)).collect();
    for s in &snaps {
        if let Some(Ok(sh)) = sharing.results.get(&s.id) {
            out.extend(sharing_finding(check, ServiceType::RDS, *s, sh));
        }
    }
    per_resource_notes(check, "snapshot", sharing, snaps.iter().map(|s| s.id.clone()), out);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(proto: &str, ports: &str) -> SgRule {
        SgRule {
            protocol: proto.into(),
            ports: ports.into(),
            source: "0.0.0.0/0".into(),
            source_label: None,
            description: None,
        }
    }

    #[test]
    fn world_rule_severity_by_port() {
        assert_eq!(sg_rule_severity(&rule("TCP", "22")), Severity::High);
        assert_eq!(sg_rule_severity(&rule("All", "All")), Severity::High);
        assert_eq!(sg_rule_severity(&rule("TCP", "3000-3400")), Severity::High); // spans 3306
        assert_eq!(sg_rule_severity(&rule("TCP", "8080")), Severity::Medium);
        assert_eq!(sg_rule_severity(&rule("TCP", "443")), Severity::Low);
        assert_eq!(sg_rule_severity(&rule("TCP", "8000-8100")), Severity::Medium);
        assert_eq!(sg_rule_severity(&rule("TCP", "80-443")), Severity::High); // spans 135/139
        assert_eq!(sg_rule_severity(&rule("ICMP", "All")), Severity::Low);
    }

    #[test]
    fn empty_input_reports_every_check_as_not_checked() {
        let rows = run(&AuditInput::new("us-east-1"));
        assert!(rows.iter().all(|r| r.kind == RowKind::NotChecked));
        assert_eq!(rows.len(), CHECKS.len() + 1);
        assert_eq!(rows[0].resource_name, "Security group open to the world");
        assert!(rows[0].list_note.contains("@ec2 or @vpc not loaded"));
        // Not-checked rows carry no severity — `F` must not offer them.
        assert!(rows.iter().all(|r| r.state_label().is_empty()));
    }

    fn account(summary: &[(&str, i32)], policy: Option<crate::aws::services::iam::IamPasswordPolicy>) -> Vec<Box<dyn Resource>> {
        vec![Box::new(IamAccountSettings {
            display_name: "acct".into(),
            aliases: vec![],
            summary: summary.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
            summary_error: None,
            password_policy: policy,
            pw_policy_error: None,
            tags: HashMap::new(),
        })]
    }

    fn ids(list: &[Box<dyn Resource>]) -> Vec<(&'static str, Severity)> {
        let mut input = AuditInput::new("us-east-1");
        input.add(ServiceType::IAM, list);
        run(&input)
            .into_iter()
            .filter(|f| f.is_finding())
            .map(|f| (f.check_id, f.severity))
            .collect()
    }

    #[test]
    fn account_checks_read_the_iam_account_row() {
        let bad = account(&[("AccountMFAEnabled", 0), ("AccountAccessKeysPresent", 1)], None);
        assert_eq!(
            ids(&bad),
            vec![
                ("iam-root-access-keys", Severity::High),
                ("iam-root-mfa", Severity::High),
                ("iam-password-policy", Severity::Medium),
            ]
        );
        let strong = crate::aws::services::iam::IamPasswordPolicy {
            minimum_length: Some(14),
            require_symbols: true,
            require_numbers: true,
            require_uppercase: true,
            require_lowercase: true,
            allow_users_to_change: true,
            expire_passwords: false,
            max_age: None,
            reuse_prevention: Some(24),
            hard_expiry: false,
        };
        let good = account(&[("AccountMFAEnabled", 1), ("AccountAccessKeysPresent", 0)], Some(strong));
        // Clean: one all-clear summary row, no findings.
        assert!(ids(&good).is_empty());
    }

    // ── Fact checks (#206) ──

    use facts::{KeyFact, Sharing, UserCreds};

    fn rows_with(lists: &[(ServiceType, &[Box<dyn Resource>])], facts: Option<&AuditFacts>) -> Vec<AuditFinding> {
        let mut input = AuditInput::new("us-east-1");
        for (svc, l) in lists {
            input.add(*svc, l);
        }
        input.now_secs = 1_000 * DAY;
        input.set_facts(facts, false);
        run(&input)
    }

    fn findings(rows: &[AuditFinding]) -> Vec<(&'static str, Severity, String)> {
        rows.iter().filter(|f| f.is_finding()).map(|f| (f.check_id, f.severity, f.reason.clone())).collect()
    }

    fn note(rows: &[AuditFinding], check: &str) -> Vec<String> {
        rows.iter()
            .filter(|f| f.kind == RowKind::NotChecked && f.check_id == check)
            .map(|f| format!("{}: {}", f.resource_name, f.list_note))
            .collect()
    }

    fn denied(action: &'static str) -> Gap {
        Gap { action, denied: true, message: "AccessDenied: no".into() }
    }

    #[test]
    fn fact_checks_wait_for_r_and_say_so() {
        let rows = rows_with(&[], None);
        assert_eq!(
            note(&rows, "guardduty-off"),
            vec!["GuardDuty not enabled: not checked — press r to fetch account facts"]
        );
        // A list-backed fact check still names its missing list first.
        assert!(note(&rows, "kms-rotation-off")[0].contains("@kms not loaded"));
        // A clean account with no lists: every account fact ran, nothing found.
        let clean = AuditFacts::clean();
        let rows = rows_with(&[], Some(&clean));
        assert!(findings(&rows).is_empty());
        assert_eq!(rows[0].kind, RowKind::AllClear);
        assert!(note(&rows, "guardduty-off").is_empty());
    }

    #[test]
    fn account_facts_produce_account_rows() {
        let mut f = AuditFacts::clean();
        f.account.s3_block = Ok(None);
        f.account.ebs_default_encryption = Ok(false);
        f.account.trails = Ok(vec![facts::TrailFact { name: "local".into(), multi_region: false, organization: false, logging: Some(true) }]);
        f.account.config_recorders = Ok(vec![facts::RecorderFact { name: "default".into(), recording: false }]);
        f.account.guardduty_detectors = Ok(vec![]);
        f.account.analyzers = Ok(vec![facts::AnalyzerFact { name: "unused".into(), kind: "ACCOUNT_UNUSED_ACCESS".into(), active: true }]);
        let rows = rows_with(&[], Some(&f));
        let got = findings(&rows);
        assert_eq!(got[0], ("cloudtrail-multi-region", Severity::High, "1 single-region trail, none multi-region".into()));
        let ids: Vec<_> = got.iter().map(|(id, s, _)| (*id, *s)).collect();
        assert_eq!(
            ids[1..],
            [
                ("config-recorder-off", Severity::Medium),
                ("guardduty-off", Severity::Medium),
                ("s3-account-public-access-block", Severity::Medium),
                ("access-analyzer-none", Severity::Low),
                ("ebs-default-encryption", Severity::Low),
            ]
        );
        // Account rows have nothing to jump to.
        assert!(rows.iter().filter(|r| r.is_finding()).all(|r| r.service.is_none()));
    }

    #[test]
    fn a_denied_fact_is_coverage_not_a_finding() {
        let mut f = AuditFacts::clean();
        f.account.guardduty_detectors = Err(denied("guardduty:ListDetectors"));
        let rows = rows_with(&[], Some(&f));
        assert!(findings(&rows).is_empty());
        assert_eq!(
            note(&rows, "guardduty-off"),
            vec!["GuardDuty (us-east-1): not checked — no permission for guardduty:ListDetectors"]
        );
        // An unreadable multi-region trail status is unknown, not "not logging".
        f.account.trails = Ok(vec![facts::TrailFact { name: "org".into(), multi_region: true, organization: true, logging: None }]);
        let rows = rows_with(&[], Some(&f));
        assert!(findings(&rows).is_empty());
        assert_eq!(note(&rows, "cloudtrail-multi-region"), vec!["trail org: not checked — trail status unreadable"]);
    }

    fn bucket(name: &str, pab: PublicAccessBlockConfig) -> Box<dyn Resource> {
        let mut b = S3Bucket::stub(name.into(), String::new(), "us-east-1".into());
        b.public_access_block = Some(pab);
        Box::new(b)
    }

    fn pab(acls: bool, policy: bool) -> PublicAccessBlockConfig {
        PublicAccessBlockConfig {
            block_public_acls: acls,
            ignore_public_acls: acls,
            block_public_policy: policy,
            restrict_public_buckets: policy,
        }
    }

    #[test]
    fn the_account_block_downgrades_bucket_findings() {
        let buckets = vec![bucket("open", pab(false, false))];
        let lists: &[(ServiceType, &[Box<dyn Resource>])] = &[(ServiceType::S3, &buckets)];
        // No facts: as before.
        assert_eq!(findings(&rows_with(lists, None))[0].1, Severity::Medium);
        // Account block fully on: the bucket's setting is moot.
        let mut f = AuditFacts::clean();
        let got = findings(&rows_with(lists, Some(&f)));
        assert_eq!(got[0].1, Severity::Low);
        assert!(got[0].2.ends_with("covered by the account-level block"), "{}", got[0].2);
        // Account block covers only the ACL half: still medium, says what's left.
        f.account.s3_block = Ok(Some(pab(true, false)));
        let got = findings(&rows_with(lists, Some(&f)));
        let bucket_row = got.iter().find(|g| g.0 == "s3-public-access-block").unwrap();
        assert_eq!(bucket_row.1, Severity::Medium);
        assert!(bucket_row.2.ends_with("account block leaves BlockPublicPolicy, RestrictPublicBuckets off"), "{}", bucket_row.2);
    }

    fn kms_key(id: &str, manager: &str) -> Box<dyn Resource> {
        Box::new(KmsKey {
            key_id: id.into(),
            arn: format!("arn:aws:kms:us-east-1:1:key/{id}"),
            aliases: vec![format!("alias/{id}")],
            description: String::new(),
            state: "Enabled".into(),
            key_usage: "ENCRYPT_DECRYPT".into(),
            key_spec: "SYMMETRIC_DEFAULT".into(),
            origin: "AWS_KMS".into(),
            manager: manager.into(),
            multi_region: false,
            rotation_enabled: None,
            creation_date: None,
            deletion_date: None,
            tags: HashMap::new(),
        })
    }

    #[test]
    fn kms_rotation_reads_per_key_results_and_reports_gaps() {
        let keys = vec![kms_key("k1", "CUSTOMER"), kms_key("k2", "CUSTOMER"), kms_key("k3", "CUSTOMER"), kms_key("aws", "AWS")];
        let lists: &[(ServiceType, &[Box<dyn Resource>])] = &[(ServiceType::Kms, &keys)];
        let input = {
            let mut i = AuditInput::new("us-east-1");
            i.add(ServiceType::Kms, &keys);
            FactTargets::from_input(&i)
        };
        assert_eq!(input.kms_keys, vec!["k1", "k2", "k3"], "AWS-managed keys rotate on their own");

        let mut f = AuditFacts::clean();
        f.kms_rotation.results.insert("k1".into(), Ok(false));
        f.kms_rotation.results.insert("k2".into(), Err(denied("kms:GetKeyRotationStatus")));
        // k3 has no result: cached after the facts were fetched.
        let rows = rows_with(lists, Some(&f));
        assert_eq!(findings(&rows), vec![("kms-rotation-off", Severity::Medium, "KeyRotationEnabled = false".into())]);
        assert_eq!(rows[0].resource_name, "alias/k1", "the alias reads better than a key id");
        assert_eq!(
            note(&rows, "kms-rotation-off"),
            vec![
                "1 key: not checked — no permission for kms:GetKeyRotationStatus",
                "1 key: not checked — not read (cap, or press r)",
            ]
        );
    }

    fn user(name: &str) -> Box<dyn Resource> {
        Box::new(IamUser {
            user_name: name.into(),
            arn: format!("arn:aws:iam::1:user/{name}"),
            user_id: name.to_uppercase(),
            path: "/".into(),
            create_date: None,
            password_last_used: None,
            tags: HashMap::new(),
        })
    }

    #[test]
    fn iam_users_console_without_mfa_and_old_keys() {
        let users = vec![user("alice"), user("bot")];
        let lists: &[(ServiceType, &[Box<dyn Resource>])] = &[(ServiceType::IAM, &users)];
        let now = 1_000 * DAY;
        let mut f = AuditFacts::clean();
        f.iam_users.results.insert(
            "alice".into(),
            Ok(UserCreds { console: true, mfa_devices: 0, keys: vec![] }),
        );
        f.iam_users.results.insert(
            "bot".into(),
            Ok(UserCreds {
                console: false,
                mfa_devices: 0,
                keys: vec![
                    KeyFact { id: "AKIAOLD".into(), active: true, created_secs: Some(now - 200 * DAY), last_used_secs: None, last_used_gap: false },
                    KeyFact { id: "AKIANEW".into(), active: true, created_secs: Some(now - 10 * DAY), last_used_secs: Some(now), last_used_gap: false },
                    KeyFact { id: "AKIAOFF".into(), active: false, created_secs: Some(now - 900 * DAY), last_used_secs: None, last_used_gap: false },
                ],
            }),
        );
        let rows = rows_with(lists, Some(&f));
        assert_eq!(
            findings(&rows),
            vec![
                ("iam-user-console-no-mfa", Severity::High, "console password, no MFA device".into()),
                ("iam-access-key-old", Severity::Medium, "AKIAOLD 200 days old, never used".into()),
            ]
        );
    }

    #[test]
    fn a_failed_last_used_read_is_unknown_not_never_used() {
        let users = vec![user("bot")];
        let lists: &[(ServiceType, &[Box<dyn Resource>])] = &[(ServiceType::IAM, &users)];
        let now = 1_000 * DAY;
        let mut f = AuditFacts::clean();
        f.iam_users.results.insert(
            "bot".into(),
            Ok(UserCreds {
                console: false,
                mfa_devices: 0,
                keys: vec![KeyFact {
                    id: "AKIAOLD".into(),
                    active: true,
                    created_secs: Some(now - 200 * DAY),
                    last_used_secs: None,
                    last_used_gap: true,
                }],
            }),
        );
        assert_eq!(
            findings(&rows_with(lists, Some(&f))),
            vec![(
                "iam-access-key-old",
                Severity::Medium,
                "AKIAOLD 200 days old, last use unknown — iam:GetAccessKeyLastUsed failed".into()
            )]
        );
    }

    #[test]
    fn an_unreadable_user_is_not_checked_for_old_keys_too() {
        let users = vec![user("bot")];
        let lists: &[(ServiceType, &[Box<dyn Resource>])] = &[(ServiceType::IAM, &users)];
        let mut f = AuditFacts::clean();
        f.iam_users.results.insert("bot".into(), Err(denied("iam:ListAccessKeys")));
        let rows = rows_with(lists, Some(&f));
        assert_eq!(note(&rows, "iam-access-key-old").len(), 1, "{:?}", note(&rows, "iam-access-key-old"));
        assert_eq!(note(&rows, "iam-user-console-no-mfa").len(), 1);
    }

    #[test]
    fn snapshot_sharing_public_is_high_shared_is_low() {
        let snap = |id: &str| -> Box<dyn Resource> {
            Box::new(EbsSnapshot::from_sdk(
                &aws_sdk_ec2::types::Snapshot::builder()
                    .snapshot_id(id)
                    .state(aws_sdk_ec2::types::SnapshotState::Completed)
                    .encrypted(true)
                    .build(),
            ))
        };
        let snaps = vec![snap("snap-pub"), snap("snap-shared"), snap("snap-private")];
        let lists: &[(ServiceType, &[Box<dyn Resource>])] = &[(ServiceType::EC2, &snaps)];
        let mut f = AuditFacts::clean();
        let r = &mut f.ebs_snapshots.results;
        r.insert("snap-pub".into(), Ok(Sharing { public: true, accounts: vec![] }));
        r.insert("snap-shared".into(), Ok(Sharing { public: false, accounts: vec!["111122223333".into()] }));
        r.insert("snap-private".into(), Ok(Sharing::default()));
        let got: Vec<_> = findings(&rows_with(lists, Some(&f)))
            .into_iter()
            .filter(|g| g.0 == "ebs-snapshot-shared")
            .map(|g| (g.1, g.2))
            .collect();
        assert_eq!(
            got,
            vec![
                (Severity::High, "public — restorable by any AWS account".into()),
                (Severity::Low, "shared with 1 account".into()),
            ]
        );
    }

    #[test]
    fn over_the_cap_is_a_coverage_note() {
        let keys: Vec<Box<dyn Resource>> = (0..3).map(|i| kms_key(&format!("k{i}"), "CUSTOMER")).collect();
        let lists: &[(ServiceType, &[Box<dyn Resource>])] = &[(ServiceType::Kms, &keys)];
        let mut f = AuditFacts::clean();
        f.kms_rotation.results.insert("k0".into(), Ok(true));
        f.kms_rotation.over_cap = 2;
        let rows = rows_with(lists, Some(&f));
        let n: Vec<_> = rows.iter().filter(|r| r.check_id == "kms-rotation-off" && r.kind == RowKind::NotChecked).collect();
        assert_eq!(n.len(), 1);
        assert!(n[0].reason.contains("stop at 100 keys"), "{}", n[0].reason);
    }

    #[test]
    fn wrap_respects_width() {
        let w = wrap("aaa bbb ccc ddd", 7);
        assert_eq!(w, vec!["aaa bbb", "ccc ddd"]);
    }
}
