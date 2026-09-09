//! Terraform state file (`.tfstate`) parser. Extracts resources, outputs, and
//! metadata for display in the detail pane with jump-to-resource support.

use serde::Deserialize;
use std::collections::HashMap;

/// Parsed Terraform state — the subset we display.
#[derive(Debug, Clone)]
pub struct TfState {
    pub version: u32,
    pub terraform_version: String,
    pub serial: u64,
    pub resources: Vec<TfResource>,
    pub outputs: Vec<(String, String)>,
}

impl TfState {
    /// Total number of *instances* across all resource blocks (a `count` /
    /// `for_each` resource contributes more than one). Falls back to counting
    /// the block itself when a block records no instances.
    pub fn instance_count(&self) -> usize {
        self.resources
            .iter()
            .map(|r| r.instances.len().max(1))
            .sum()
    }
}

/// A single managed or data resource *block* from the state (one HCL block;
/// `count` / `for_each` expand to multiple `instances`).
#[derive(Debug, Clone)]
pub struct TfResource {
    /// Module path: `""` for root, else `module.x` (possibly nested).
    pub module: String,
    /// `"managed"` or `"data"`.
    pub mode: String,
    pub r#type: String,
    pub name: String,
    /// Short provider label, e.g. `hashicorp/aws` (+ `.alias` when set).
    pub provider: String,
    pub instances: Vec<TfInstance>,
}

impl TfResource {
    /// The resource address without module prefix: `aws_instance.web`, or
    /// `data.aws_ami.ubuntu` for data sources (Terraform's own convention).
    pub fn type_name(&self) -> String {
        if self.mode == "data" {
            format!("data.{}.{}", self.r#type, self.name)
        } else {
            format!("{}.{}", self.r#type, self.name)
        }
    }
}

/// A single instance of a resource block (one row per `count`/`for_each` key).
#[derive(Debug, Clone)]
pub struct TfInstance {
    /// The `count`/`for_each` index, pre-formatted as `[0]` or `["key"]`;
    /// `None` for a plain (single-instance) resource.
    pub index: Option<String>,
    /// The primary identifier (ARN preferred, else the `id` attribute).
    pub identifier: String,
}

/// Parse a tfstate JSON string into our display model.
pub fn parse_tfstate(json: &str) -> Result<TfState, String> {
    let raw: RawState =
        serde_json::from_str(json).map_err(|e| format!("Invalid tfstate JSON: {}", e))?;

    let mut resources = Vec::new();
    for r in &raw.resources {
        let module = r.module.as_deref().unwrap_or("").to_string();
        let provider = parse_provider(r.provider.as_deref().unwrap_or(""));

        let mut instances: Vec<TfInstance> = r
            .instances
            .iter()
            .map(|inst| {
                let identifier = inst
                    .attributes
                    .get("arn")
                    .or_else(|| inst.attributes.get("id"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                TfInstance {
                    index: inst.index_key.as_ref().map(format_index_key),
                    identifier,
                }
            })
            .collect();
        // Stable, human order for count/for_each rows.
        instances.sort_by(|a, b| a.index.cmp(&b.index));

        resources.push(TfResource {
            module,
            mode: r.mode.clone(),
            r#type: r.r#type.clone(),
            name: r.name.clone(),
            provider,
            instances,
        });
    }

    let outputs: Vec<(String, String)> = raw
        .outputs
        .iter()
        .map(|(k, v)| {
            let display = if v.sensitive {
                "(sensitive)".to_string()
            } else {
                match &v.value {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                }
            };
            (k.clone(), display)
        })
        .collect();

    Ok(TfState {
        version: raw.version,
        terraform_version: raw.terraform_version.unwrap_or_default(),
        serial: raw.serial,
        resources,
        outputs,
    })
}

/// Extract a short provider label from the state's fully-qualified provider
/// string, e.g. `provider["registry.terraform.io/hashicorp/aws"]` →
/// `hashicorp/aws`, preserving any `.alias` suffix
/// (`…/aws"].us_east_1` → `hashicorp/aws.us_east_1`).
fn parse_provider(raw: &str) -> String {
    let Some((_, after)) = raw.split_once("provider[\"") else {
        return String::new();
    };
    let (fqn, alias) = match after.split_once("\"]") {
        Some((fqn, alias)) => (fqn, alias),
        None => (after.trim_end_matches("\"]"), ""),
    };
    // Keep the last two path segments (namespace/type), dropping the registry
    // host — `registry.terraform.io/hashicorp/aws` → `hashicorp/aws`.
    let short = {
        let mut segs: Vec<&str> = fqn.split('/').collect();
        while segs.len() > 2 {
            segs.remove(0);
        }
        segs.join("/")
    };
    format!("{}{}", short, alias)
}

/// Format a `count`/`for_each` index key for display: numbers → `[0]`,
/// strings → `["key"]`.
fn format_index_key(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => format!("[\"{}\"]", s),
        serde_json::Value::Number(n) => format!("[{}]", n),
        other => format!("[{}]", other),
    }
}

// ── Raw serde models (just enough to extract what we need) ───────────────────

#[derive(Deserialize)]
struct RawState {
    version: u32,
    terraform_version: Option<String>,
    serial: u64,
    #[serde(default)]
    resources: Vec<RawResource>,
    #[serde(default)]
    outputs: HashMap<String, RawOutput>,
}

#[derive(Deserialize)]
struct RawResource {
    #[serde(default)]
    module: Option<String>,
    #[serde(default)]
    mode: String,
    r#type: String,
    name: String,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    instances: Vec<RawInstance>,
}

#[derive(Deserialize)]
struct RawInstance {
    #[serde(default)]
    attributes: HashMap<String, serde_json::Value>,
    /// `count` index (number) or `for_each` key (string); absent otherwise.
    #[serde(default)]
    index_key: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct RawOutput {
    value: serde_json::Value,
    #[serde(default)]
    sensitive: bool,
}

/// Produce detail-pane rows for a terraform state. Uses the `style_detail_row`
/// conventions: group headers (key non-empty, value empty), key-value rows
/// (for jumpable identifiers — the value is the ARN/ID which `resource_jump_target`
/// recognizes), and plain content lines (space-prefixed key, empty value).
pub fn tf_state_detail_lines(state: &TfState) -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = Vec::new();

    // Sort by (module, type, name) — root ("") sorts first, and same-type
    // resources cluster together within a module.
    let mut sorted: Vec<&TfResource> = state.resources.iter().collect();
    sorted.sort_by(|a, b| {
        a.module
            .cmp(&b.module)
            .then(a.r#type.cmp(&b.r#type))
            .then(a.name.cmp(&b.name))
    });

    // Per-module instance counts for the group headers.
    let mut mod_counts: HashMap<&str, usize> = HashMap::new();
    for r in &sorted {
        let m = if r.module.is_empty() { "root" } else { &r.module };
        *mod_counts.entry(m).or_default() += r.instances.len().max(1);
    }

    let mut current_module: Option<&str> = None;
    for r in &sorted {
        let mod_name = if r.module.is_empty() { "root" } else { &r.module };
        if current_module != Some(mod_name) {
            if current_module.is_some() {
                rows.push((String::new(), String::new())); // spacer
            }
            let n = mod_counts.get(mod_name).copied().unwrap_or(0);
            rows.push((
                format!("{}  ·  {} resource{}", mod_name, n, if n == 1 { "" } else { "s" }),
                String::new(),
            ));
            rows.push((String::new(), String::new()));
            current_module = Some(mod_name);
        }

        let base = r.type_name();
        if r.instances.is_empty() {
            // No recorded instances — show the address alone (plain line).
            rows.push((format!("  {}", base), String::new()));
            continue;
        }
        // One row per instance: address as the key (accent), the jumpable
        // id/ARN as the value. Instances with no identifier render as a plain
        // indented line so they still show.
        for inst in &r.instances {
            let addr = match &inst.index {
                Some(ix) => format!("  {}{}", base, ix),
                None => format!("  {}", base),
            };
            rows.push((addr, inst.identifier.clone()));
        }
    }

    // Providers summary — replaces the per-resource `provider:` noise. Distinct
    // providers with the number of instances each backs, as dim-noted lines.
    let mut prov_counts: Vec<(&str, usize)> = Vec::new();
    for r in &sorted {
        if r.provider.is_empty() {
            continue;
        }
        let n = r.instances.len().max(1);
        match prov_counts.iter_mut().find(|(p, _)| *p == r.provider) {
            Some((_, c)) => *c += n,
            None => prov_counts.push((&r.provider, n)),
        }
    }
    if !prov_counts.is_empty() {
        prov_counts.sort_by(|a, b| a.0.cmp(b.0));
        rows.push((String::new(), String::new()));
        rows.push(("Providers".to_string(), String::new())); // group header
        rows.push((String::new(), String::new()));
        for (p, n) in &prov_counts {
            // `\t` splits fixed content from a dim trailing note (see
            // `style_detail_row`), so the count reads as a comment.
            rows.push((
                format!("  {}\t{} resource{}", p, n, if *n == 1 { "" } else { "s" }),
                String::new(),
            ));
        }
    }

    // Outputs section
    if !state.outputs.is_empty() {
        rows.push((String::new(), String::new()));
        rows.push(("Outputs".to_string(), String::new())); // group header
        rows.push((String::new(), String::new()));
        let mut sorted_outputs = state.outputs.clone();
        sorted_outputs.sort_by(|a, b| a.0.cmp(&b.0));
        for (k, v) in &sorted_outputs {
            rows.push((format!("  {}", k), v.clone()));
        }
    }

    rows
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_minimal_tfstate() {
        let json = r#"{
            "version": 4,
            "terraform_version": "1.5.7",
            "serial": 42,
            "lineage": "abc-123",
            "outputs": {
                "vpc_id": { "value": "vpc-0abc123" }
            },
            "resources": [
                {
                    "mode": "managed",
                    "type": "aws_vpc",
                    "name": "main",
                    "provider": "provider[\"registry.terraform.io/hashicorp/aws\"]",
                    "instances": [
                        { "attributes": { "id": "vpc-0abc123", "arn": "arn:aws:ec2:us-east-1:123:vpc/vpc-0abc123" } }
                    ]
                },
                {
                    "module": "module.network",
                    "mode": "managed",
                    "type": "aws_subnet",
                    "name": "private",
                    "provider": "provider[\"registry.terraform.io/hashicorp/aws\"]",
                    "instances": [
                        { "attributes": { "id": "subnet-0def456" } }
                    ]
                }
            ]
        }"#;

        let state = parse_tfstate(json).unwrap();
        assert_eq!(state.version, 4);
        assert_eq!(state.serial, 42);
        assert_eq!(state.terraform_version, "1.5.7");
        assert_eq!(state.resources.len(), 2);
        assert_eq!(state.instance_count(), 2);
        assert_eq!(state.outputs.len(), 1);

        let vpc = &state.resources[0];
        assert_eq!(vpc.type_name(), "aws_vpc.main");
        assert_eq!(vpc.provider, "hashicorp/aws");
        assert_eq!(vpc.instances.len(), 1);
        assert_eq!(
            vpc.instances[0].identifier,
            "arn:aws:ec2:us-east-1:123:vpc/vpc-0abc123"
        );
        assert_eq!(vpc.instances[0].index, None);

        let subnet = &state.resources[1];
        assert_eq!(subnet.module, "module.network");
        assert_eq!(subnet.type_name(), "aws_subnet.private");
        assert_eq!(subnet.instances[0].identifier, "subnet-0def456");

        assert_eq!(state.outputs[0], ("vpc_id".to_string(), "vpc-0abc123".to_string()));
    }

    #[test]
    fn test_count_instances_and_data_and_sensitive() {
        let json = r#"{
            "version": 4,
            "serial": 1,
            "outputs": {
                "db_password": { "value": "hunter2", "sensitive": true },
                "region": { "value": "us-east-1" }
            },
            "resources": [
                {
                    "mode": "managed",
                    "type": "aws_subnet",
                    "name": "web",
                    "provider": "provider[\"registry.terraform.io/hashicorp/aws\"]",
                    "instances": [
                        { "index_key": 1, "attributes": { "id": "subnet-1" } },
                        { "index_key": 0, "attributes": { "id": "subnet-0" } }
                    ]
                },
                {
                    "mode": "data",
                    "type": "aws_ami",
                    "name": "ubuntu",
                    "provider": "provider[\"registry.terraform.io/hashicorp/aws\"].us_east_1",
                    "instances": [ { "attributes": { "id": "ami-abc" } } ]
                }
            ]
        }"#;

        let state = parse_tfstate(json).unwrap();
        assert_eq!(state.resources.len(), 2);
        assert_eq!(state.instance_count(), 3);

        let subnet = &state.resources[0];
        // Instances sorted by index key.
        assert_eq!(subnet.instances[0].index.as_deref(), Some("[0]"));
        assert_eq!(subnet.instances[1].index.as_deref(), Some("[1]"));

        let ami = &state.resources[1];
        assert_eq!(ami.type_name(), "data.aws_ami.ubuntu");
        // Alias preserved on the short provider label.
        assert_eq!(ami.provider, "hashicorp/aws.us_east_1");

        // Sensitive output is redacted.
        let pw = state.outputs.iter().find(|(k, _)| k == "db_password").unwrap();
        assert_eq!(pw.1, "(sensitive)");

        // A count resource renders one row per instance with its own index.
        let lines = tf_state_detail_lines(&state);
        assert!(lines.iter().any(|(k, _)| k == "  aws_subnet.web[0]"));
        assert!(lines.iter().any(|(k, _)| k == "  aws_subnet.web[1]"));
        assert!(lines.iter().any(|(k, _)| k == "  data.aws_ami.ubuntu"));
        // Providers summary is present.
        assert!(lines.iter().any(|(k, _)| k == "Providers"));
    }

    #[test]
    fn test_parse_empty_state() {
        let json = r#"{"version": 4, "serial": 0, "resources": []}"#;
        let state = parse_tfstate(json).unwrap();
        assert_eq!(state.resources.len(), 0);
        assert_eq!(state.outputs.len(), 0);
    }
}
