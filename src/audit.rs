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
use crate::aws::services::iam::{AccessAnalyzerFinding, IamAccountSettings};
use crate::aws::services::rds::{RdsCluster, RdsInstance, RdsSnapshot};
use crate::aws::services::s3::S3Bucket;
use crate::aws::services::vpc::VpcSecurityGroup;

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
    run: fn(&AuditInput, &mut Vec<AuditFinding>),
}

/// The checks, in display order. Stage 1 is list-time data only.
static CHECKS: [Check; 13] = [
    Check {
        id: "sg-open-world",
        title: "Security group open to the world",
        why: "Inbound 0.0.0.0/0 or ::/0 lets anyone on the internet reach whatever the group is attached to. Admin and database ports (SSH, RDP, MySQL, Postgres…) are the ones scanned within minutes of exposure; 80/443 is normal for a public load balancer.",
        sources: &[ServiceType::EC2, ServiceType::VPC],
        section: Some("Inbound"),
        run: check_sg_open_world,
    },
    Check {
        id: "rds-public",
        title: "RDS instance publicly accessible",
        why: "A publicly accessible instance gets a public DNS name resolving to a public IP. Its security groups are then the only thing between the database and the internet.",
        sources: &[ServiceType::RDS],
        section: Some("Config"),
        run: check_rds_public,
    },
    Check {
        id: "rds-unencrypted",
        title: "RDS storage not encrypted",
        why: "Storage encryption can't be turned on for an existing instance or cluster — it takes a snapshot, an encrypted copy and a restore. Snapshots and read replicas inherit the setting.",
        sources: &[ServiceType::RDS],
        section: Some("Storage"),
        run: check_rds_unencrypted,
    },
    Check {
        id: "rds-snapshot-unencrypted",
        title: "RDS snapshot not encrypted",
        why: "An unencrypted snapshot restores to an unencrypted database, and can be shared or copied without a KMS key gating it.",
        sources: &[ServiceType::RDS],
        section: None,
        run: check_rds_snapshot_unencrypted,
    },
    Check {
        id: "ebs-unencrypted",
        title: "EBS volume not encrypted",
        why: "Unencrypted volumes produce unencrypted snapshots. Turning on EBS encryption-by-default covers new volumes only.",
        sources: &[ServiceType::EC2],
        section: Some("Details"),
        run: check_ebs_unencrypted,
    },
    Check {
        id: "ebs-snapshot-unencrypted",
        title: "EBS snapshot not encrypted",
        why: "An unencrypted snapshot can be shared with another account (or made public) with no KMS key policy in the way.",
        sources: &[ServiceType::EC2],
        section: None,
        run: check_ebs_snapshot_unencrypted,
    },
    Check {
        id: "ec2-imdsv1",
        title: "EC2 instance allows IMDSv1",
        why: "IMDSv1 answers unauthenticated GETs, so an SSRF bug in anything on the instance can read its role credentials. IMDSv2 (HttpTokens=required) needs a session token first.",
        sources: &[ServiceType::EC2],
        section: Some("Security"),
        run: check_ec2_imdsv1,
    },
    Check {
        id: "ami-public",
        title: "AMI is public",
        why: "A public AMI can be launched by any AWS account — including whatever is baked into it: keys, config, application code.",
        sources: &[ServiceType::EC2],
        section: Some("Permissions"),
        run: check_ami_public,
    },
    Check {
        id: "s3-public-access-block",
        title: "S3 bucket public access block not fully on",
        why: "With any of the four Block Public Access settings off, a bucket policy or ACL can make objects public. The account-level block may still cover this bucket — that setting isn't checked yet (#206).",
        sources: &[ServiceType::S3],
        section: Some("Security"),
        run: check_s3_public_access_block,
    },
    Check {
        id: "eks-public-endpoint",
        title: "EKS API endpoint open to the world",
        why: "The cluster's Kubernetes API is reachable from any IP. Authentication still applies, but the endpoint is exposed to credential stuffing and to any future API-server vulnerability.",
        sources: &[ServiceType::Eks],
        section: Some("Networking"),
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
        run: check_root_mfa,
    },
    Check {
        id: "iam-root-access-keys",
        title: "Root user has access keys",
        why: "A root access key is unrestricted, long-lived and can't be scoped by any policy. AWS's guidance is to have none; use roles instead.",
        sources: &[ServiceType::IAM],
        section: Some("Overview"),
        run: check_root_access_keys,
    },
    Check {
        id: "iam-password-policy",
        title: "IAM password policy weak or missing",
        why: "The password policy applies to IAM users' console passwords. CIS asks for at least 14 characters and preventing reuse of the last 24. Accounts where nobody signs in as an IAM user (SSO only) can treat this as low priority.",
        sources: &[ServiceType::IAM],
        section: Some("Password Policy"),
        run: check_password_policy,
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
];

/// Every check, the Access Analyzer relay included.
pub fn all_checks() -> impl Iterator<Item = &'static Check> {
    CHECKS.iter().chain(std::iter::once(&ACCESS_ANALYZER))
}

/// Cached lists, one per service that had a warm cache.
pub struct AuditInput<'a> {
    lists: HashMap<ServiceType, &'a [Box<dyn Resource>]>,
    /// Sources whose load `r` has in flight — their checks say "loading…".
    loading: Vec<ServiceType>,
    /// For telling current-region S3 buckets from other-region stubs.
    region: String,
}

impl<'a> AuditInput<'a> {
    pub fn new(region: &str) -> Self {
        Self {
            lists: HashMap::new(),
            loading: Vec::new(),
            region: region.to_string(),
        }
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
        if check.sources.iter().any(|s| input.has(*s)) {
            ran += 1;
            (check.run)(input, &mut findings);
        } else {
            let loading = check.sources.iter().any(|s| input.loading.contains(s));
            notes.push(AuditFinding::not_checked(check, loading));
        }
    }
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

fn check_sg_open_world(input: &AuditInput, out: &mut Vec<AuditFinding>) {
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
            &CHECKS[0],
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

fn check_rds_public(input: &AuditInput, out: &mut Vec<AuditFinding>) {
    for db in input.of::<RdsInstance>(ServiceType::RDS) {
        if db.publicly_accessible {
            out.push(AuditFinding::new(
                &CHECKS[1],
                Severity::High,
                ServiceType::RDS,
                db,
                "PubliclyAccessible = true".into(),
            ));
        }
    }
}

fn check_rds_unencrypted(input: &AuditInput, out: &mut Vec<AuditFinding>) {
    for db in input.of::<RdsInstance>(ServiceType::RDS) {
        // A cluster member's storage is the cluster's — reported once there.
        if !db.storage_encrypted && db.cluster_identifier.is_none() {
            out.push(AuditFinding::new(
                &CHECKS[2],
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
                &CHECKS[2],
                Severity::Medium,
                ServiceType::RDS,
                c,
                "StorageEncrypted = false (cluster)".into(),
            ));
        }
    }
}

fn check_rds_snapshot_unencrypted(input: &AuditInput, out: &mut Vec<AuditFinding>) {
    for s in input.of::<RdsSnapshot>(ServiceType::RDS) {
        if !s.encrypted {
            out.push(AuditFinding::new(
                &CHECKS[3],
                Severity::Low,
                ServiceType::RDS,
                s,
                "Encrypted = false".into(),
            ));
        }
    }
}

fn check_ebs_unencrypted(input: &AuditInput, out: &mut Vec<AuditFinding>) {
    for v in input.of::<EbsVolume>(ServiceType::EC2) {
        if !v.encrypted {
            let reason = match &v.attached_instance_id {
                Some(i) => format!("Encrypted = false (attached to {i})"),
                None => "Encrypted = false (unattached)".to_string(),
            };
            out.push(AuditFinding::new(&CHECKS[4], Severity::Medium, ServiceType::EC2, v, reason));
        }
    }
}

fn check_ebs_snapshot_unencrypted(input: &AuditInput, out: &mut Vec<AuditFinding>) {
    for s in input.of::<EbsSnapshot>(ServiceType::EC2) {
        if !s.encrypted {
            out.push(AuditFinding::new(
                &CHECKS[5],
                Severity::Low,
                ServiceType::EC2,
                s,
                "Encrypted = false".into(),
            ));
        }
    }
}

fn check_ec2_imdsv1(input: &AuditInput, out: &mut Vec<AuditFinding>) {
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
                &CHECKS[6],
                Severity::Medium,
                ServiceType::EC2,
                i,
                "HttpTokens = optional (IMDSv1 allowed)".into(),
            ));
        }
    }
}

fn check_ami_public(input: &AuditInput, out: &mut Vec<AuditFinding>) {
    for a in input.of::<Ami>(ServiceType::EC2) {
        if a.public {
            out.push(AuditFinding::new(
                &CHECKS[7],
                Severity::High,
                ServiceType::EC2,
                a,
                "Public = true — launchable by any AWS account".into(),
            ));
        }
    }
}

fn check_s3_public_access_block(input: &AuditInput, out: &mut Vec<AuditFinding>) {
    for b in input.of::<S3Bucket>(ServiceType::S3) {
        if b.region != input.region {
            continue; // other-region stub: no Tier-1 data, see s3_coverage_notes
        }
        let Some(pab) = &b.public_access_block else { continue };
        if pab.all_blocked() {
            continue;
        }
        let off: Vec<&str> = [
            (pab.block_public_acls, "BlockPublicAcls"),
            (pab.ignore_public_acls, "IgnorePublicAcls"),
            (pab.block_public_policy, "BlockPublicPolicy"),
            (pab.restrict_public_buckets, "RestrictPublicBuckets"),
        ]
        .iter()
        .filter(|(on, _)| !on)
        .map(|(_, n)| *n)
        .collect();
        let reason = if off.len() == 4 {
            "no bucket-level public access block".to_string()
        } else {
            format!("off: {}", off.join(", "))
        };
        out.push(AuditFinding::new(&CHECKS[8], Severity::Medium, ServiceType::S3, b, reason));
    }
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
    let check = &CHECKS[8];
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

fn check_eks_public_endpoint(input: &AuditInput, out: &mut Vec<AuditFinding>) {
    for c in input.of::<EksCluster>(ServiceType::Eks) {
        if c.endpoint_public
            && (c.public_access_cidrs.is_empty()
                || c.public_access_cidrs.iter().any(|x| x == "0.0.0.0/0" || x == "::/0"))
        {
            out.push(AuditFinding::new(
                &CHECKS[9],
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

fn check_root_mfa(input: &AuditInput, out: &mut Vec<AuditFinding>) {
    if let Some(a) = account_settings(input) {
        if a.summary_value("AccountMFAEnabled") == Some(0) {
            out.push(AuditFinding::new(
                &CHECKS[10],
                Severity::High,
                ServiceType::IAM,
                a,
                "AccountMFAEnabled = 0".into(),
            ));
        }
    }
}

fn check_root_access_keys(input: &AuditInput, out: &mut Vec<AuditFinding>) {
    if let Some(a) = account_settings(input) {
        if let Some(n) = a.summary_value("AccountAccessKeysPresent").filter(|n| *n > 0) {
            out.push(AuditFinding::new(
                &CHECKS[11],
                Severity::High,
                ServiceType::IAM,
                a,
                format!("AccountAccessKeysPresent = {n}"),
            ));
        }
    }
}

fn check_password_policy(input: &AuditInput, out: &mut Vec<AuditFinding>) {
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
    out.push(AuditFinding::new(&CHECKS[12], severity, ServiceType::IAM, a, reason));
}

fn check_access_analyzer_public(input: &AuditInput, out: &mut Vec<AuditFinding>) {
    for f in input.of::<AccessAnalyzerFinding>(ServiceType::IAM) {
        if f.status == "ACTIVE" && f.is_public {
            let mut row = AuditFinding::new(
                &ACCESS_ANALYZER,
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

    #[test]
    fn wrap_respects_width() {
        let w = wrap("aaa bbb ccc ddd", 7);
        assert_eq!(w, vec!["aaa bbb", "ccc ddd"]);
    }
}
