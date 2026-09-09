use std::any::Any;
use std::collections::HashMap;
use std::fmt::Debug;

#[derive(Debug, Clone, PartialEq)]
pub enum ResourceState {
    Running,
    Stopped,
    Pending,
    Terminated,
    Available,
    Unavailable,
    Creating,
    Deleting,
    Unknown(String),
}

impl ResourceState {
    /// The state of a resource kind that has **no lifecycle state** in AWS —
    /// an IAM role, a security group, an SQS queue, a log group. Renders as a
    /// dim `○` with a blank label, so the wide list column stays empty and
    /// the `F` filter offers nothing. Use this instead of a constant
    /// `Available`: a column of green "available" on rows that can't be
    /// anything else reads as a state and is just noise.
    pub fn stateless() -> Self {
        ResourceState::Unknown(String::new())
    }
}

impl std::fmt::Display for ResourceState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResourceState::Running => write!(f, "running"),
            ResourceState::Stopped => write!(f, "stopped"),
            ResourceState::Pending => write!(f, "pending"),
            ResourceState::Terminated => write!(f, "terminated"),
            ResourceState::Available => write!(f, "available"),
            ResourceState::Unavailable => write!(f, "unavailable"),
            ResourceState::Creating => write!(f, "creating"),
            ResourceState::Deleting => write!(f, "deleting"),
            ResourceState::Unknown(s) => write!(f, "{}", s),
        }
    }
}

/// The `state_label()` for a type whose `state()` is a mapping of a native
/// status string: the native word, lowercased (so the `F` chips read
/// `non_compliant` / `action recommended` / `create_complete`, not the
/// coarse bucket's `unavailable`). An empty native status falls back to the
/// coarse word so the column never goes blank.
pub fn native_state_label(native: &str, fallback: impl FnOnce() -> ResourceState) -> String {
    let native = native.trim();
    if native.is_empty() {
        fallback().to_string()
    } else {
        native.to_lowercase()
    }
}

// Future extensibility for resource actions
#[derive(Debug, Clone)]
pub enum ResourceAction {
    Start,
    Stop,
    Restart,
    Terminate,
    Custom(String),
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ActionResult {
    pub success: bool,
    pub message: String,
}

pub trait Resource: Send + Sync + Debug {
    /// Unique identifier
    fn id(&self) -> &str;

    /// Display name
    fn name(&self) -> &str;

    /// Resource type (e.g., "EC2 Instance", "RDS Database")
    fn resource_type(&self) -> &str;

    /// Current state (running, stopped, etc.)
    fn state(&self) -> ResourceState;

    /// The word shown wherever `state()` renders as text — the wide list
    /// column, the `F` filter chips, exports. `state()` is a *coarse*
    /// colour/sort bucket, so nearly every type maps its own status onto it
    /// (a Config rule's NON_COMPLIANT → Unavailable, an ELB's `active` →
    /// Running); the label must say the resource's **own** word, never the
    /// bucket's — otherwise the `F` chip reads "unavailable" for a Trusted
    /// Advisor "action recommended" check. Override on every type whose
    /// mapping changes the word (`native_state_label` covers the common
    /// status-string case); the default is only right when the enum word IS
    /// the native one. The dot colour always comes from `state()` itself.
    fn state_label(&self) -> String {
        self.state().to_string()
    }

    /// The split pane's section table, when this type has one — the single
    /// source of truth its section wiring derives from (`src/sections.rs`).
    /// `None` (the default) renders the flat `details()` view.
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        None
    }

    /// Tags
    fn tags(&self) -> &HashMap<String, String>;

    /// Searchable text (for fuzzy search)
    fn search_text(&self) -> String {
        format!(
            "{} {} {} {}",
            self.id(),
            self.name(),
            self.resource_type(),
            self.tags()
                .iter()
                .map(|(k, v)| format!("{}:{}", k, v))
                .collect::<Vec<_>>()
                .join(" ")
        )
    }

    /// Details for display pane (key-value pairs)
    fn details(&self) -> Vec<(String, String)>;

    /// Whether this resource is "noise" — a low-signal, everything-is-fine row
    /// (a passing check, an OK alarm, a compliant rule) that the list pane hides
    /// by default so problems stand out. Toggled with `a`. Default: not noise.
    fn is_noise(&self) -> bool {
        false
    }

    /// Available actions (for future use)
    fn available_actions(&self) -> Vec<ResourceAction> {
        vec![]
    }

    /// Clone as boxed trait object
    fn clone_box(&self) -> Box<dyn Resource>;

    fn as_any(&self) -> &dyn Any;

    /// The resource's **raw AWS JSON**, when the API hands us one verbatim —
    /// CloudTrail events, GuardDuty / Inspector / Security Hub findings. `e`
    /// opens it in `$EDITOR` in preference to the detail-snapshot JSON.
    /// Not for policies or hand-picked field summaries: policies belong in
    /// `App::editor_override_content` (section-gated), and summaries are
    /// superseded by the snapshot.
    fn raw_content(&self) -> Option<String> {
        None
    }

    /// Get the estimated monthly cost for this resource
    /// Returns None if cost data is not available
    fn estimated_monthly_cost(&self) -> Option<f64> {
        None
    }

    /// Get the cost trend indicator
    fn cost_trend(&self) -> Option<&str> {
        None
    }

    /// Return the AWS Console URL for this resource, given the current region string.
    /// Returns None for resource types that don't have a direct console page.
    fn console_url(&self, _region: &str) -> Option<String> {
        None
    }

    /// The AWS CLI command that fetches this resource — the service part only
    /// (`aws ec2 describe-instances --instance-ids i-…`); the app appends
    /// `--region` / `--profile` from the current context and `C` copies it.
    /// **Read commands only**: never a mutation, and never one that reveals a
    /// secret value (Secrets Manager maps to `describe-secret`, not
    /// `get-secret-value`). Quote values with [`shell_quote`].
    fn cli_command(&self) -> Option<String> {
        None
    }

    /// Identifiers to search CloudTrail by for the "who changed this?" lens
    /// (`W`), tried in order until one yields events. CloudTrail's
    /// `ResourceName` attribute indexes by the value that appears in an event's
    /// `resources` list — usually the resource id, but some types are recorded
    /// by name (e.g. an IAM role by role name, an S3 bucket by bucket name).
    /// Defaults to `[id()]`; override where the CloudTrail-indexed identifier
    /// differs from `id()`.
    fn trail_lookup_keys(&self) -> Vec<String> {
        vec![self.id().to_string()]
    }

    /// Security groups attached to this resource, for the network-access
    /// lens (`N`), which merges every group's rules into one effective
    /// ingress/egress table. Default: none — override on every type that
    /// carries a `security_groups`-shaped field (instances, ENIs, RDS,
    /// Lambda, ECS services, load balancers, …). A security group returns
    /// its own id. Order is preserved; duplicates are fine (the lens
    /// dedupes).
    fn security_group_ids(&self) -> Vec<String> {
        Vec::new()
    }

    /// Identifiers this resource **points at** — `(label, id-or-ARN)` pairs
    /// (`("Subnet", "subnet-…")`, `("Role", "arn:…")`) — for the
    /// "referenced by" lens (`U`), which walks every loaded resource's
    /// references looking for the selected one. The flat `details()` view
    /// is the fallback, but rich split-pane types keep most of their links
    /// in section renderers the lens can't reach, so **override this on any
    /// type whose links matter** — the label becomes the lens's `via`
    /// column. Default: the security groups from `security_group_ids()`;
    /// overrides should start from [`sg_refs`] to keep them.
    fn references(&self) -> Vec<(String, String)> {
        sg_refs(self.security_group_ids())
    }
}

/// `security_group_ids()` as labelled references — the seed every
/// `references()` override starts from.
pub fn sg_refs(ids: Vec<String>) -> Vec<(String, String)> {
    ids.into_iter()
        .map(|id| ("Security Group".to_string(), id))
        .collect()
}

// Enable cloning of Box<dyn Resource>
impl Clone for Box<dyn Resource> {
    fn clone(&self) -> Self {
        self.clone_box()
    }
}

/// Single-quote a value for a copied shell command when it contains anything
/// beyond the characters safe in every POSIX shell; identifiers (ARNs, ids,
/// most names) pass through untouched so commands stay readable.
pub fn shell_quote(s: &str) -> String {
    let safe = |c: char| c.is_ascii_alphanumeric() || "-_./:=@,+".contains(c);
    if !s.is_empty() && s.chars().all(safe) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::{native_state_label, shell_quote, ResourceState};

    #[test]
    fn native_state_label_prefers_the_resource_word_over_the_bucket() {
        // The coarse bucket says "unavailable"; the chip must say the
        // resource's own word.
        assert_eq!(
            native_state_label("NON_COMPLIANT", || ResourceState::Unavailable),
            "non_compliant"
        );
        assert_eq!(native_state_label(" Active ", || ResourceState::Running), "active");
        // Empty native status: fall back to the bucket so the column isn't blank.
        assert_eq!(native_state_label("", || ResourceState::Pending), "pending");
    }

    #[test]
    fn shell_quote_passes_identifiers_and_quotes_the_rest() {
        assert_eq!(shell_quote("i-0abc123"), "i-0abc123");
        assert_eq!(
            shell_quote("arn:aws:iam::123456789012:role/My-Role"),
            "arn:aws:iam::123456789012:role/My-Role"
        );
        assert_eq!(shell_quote("my name"), "'my name'");
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
        assert_eq!(shell_quote(""), "''");
    }
}
