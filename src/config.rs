use crate::aws::region::Region;
use crate::aws::service::ServiceType;
use serde::Deserialize;
use std::path::PathBuf;

/// User configuration, loaded once at startup from a TOML file. Every field is
/// optional — a missing file (or missing key) just falls back to defaults.
///
/// Search order: `$NEBOTO_CONFIG`, then `$XDG_CONFIG_HOME/neboto/config.toml`
/// (default `~/.config/neboto/config.toml`), then `~/.neboto.toml`.
///
/// ```toml
/// # ~/.config/neboto/config.toml
/// default_service = "ec2"      # omit to show the welcome screen and load nothing
/// default_region  = "us-west-2"
/// default_profile = "myprofile"
/// show_banner     = true
/// ```
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Service to load on startup (a prefix/alias, e.g. "ec2", "s3", "iam").
    /// `None` shows the welcome/help splash and loads nothing until the user
    /// picks a service.
    pub default_service: Option<String>,
    /// Region code to start in (e.g. "eu-west-1").
    pub default_region: Option<String>,
    /// Named AWS profile to start with.
    pub default_profile: Option<String>,
    /// Whether the ASCII banner is shown at startup (default true).
    pub show_banner: Option<bool>,
    /// Custom AWS endpoint URL for a local emulator (e.g. floci/LocalStack at
    /// `http://localhost:4566`). The `AWS_ENDPOINT_URL` env var overrides this.
    /// When set, S3 uses path-style addressing and dummy credentials are used.
    pub endpoint_url: Option<String>,
    /// Start in watch mode (auto-refresh of the current view) — same as the
    /// `--watch` flag. `w` toggles it at runtime either way.
    pub watch: Option<bool>,
    /// Watch-mode refresh interval in seconds (default 10, clamped 5–300).
    /// `+`/`-` step through presets at runtime.
    pub watch_interval: Option<u64>,
    /// Start with the detail pane in flat view: every section concatenated
    /// into one scroll (section chips become jump anchors) instead of the
    /// tabbed section-at-a-time view. `\` toggles it at runtime either way.
    pub detail_flat: Option<bool>,
    /// Start log tail / log search panes with long lines wrapped (hanging
    /// indent under the message column) instead of clipped at the pane edge.
    /// `w` toggles it at runtime either way; the toggle is sticky for the
    /// session. Default false (clip).
    pub log_wrap: Option<bool>,
    /// IAM role name assumed when switching into a member account from the
    /// Organizations accounts list (`s`). Default:
    /// `OrganizationAccountAccessRole`; Control Tower shops typically want
    /// `AWSControlTowerExecution`. The session is always scoped to the
    /// AWS-managed `ReadOnlyAccess` policy.
    pub org_access_role: Option<String>,
    /// Several candidate roles for the member-account switch (e.g. a deploy
    /// role next to the org default). With more than one entry, `s` opens a
    /// picker (last-used preselected); takes precedence over
    /// `org_access_role` when set.
    pub org_access_roles: Option<Vec<String>>,
    /// Account id holding the Control Tower Config aggregator (the **audit**
    /// account under a landing zone that delegates AWS Config, which v4.0
    /// manifests do). Control Tower's own APIs answer only in the management
    /// account while the aggregator lives only in the audit account, so
    /// without this the Compliance sub-tab is unreachable from the same
    /// session as every other tab. Set it and the compliance phase assumes
    /// that account for its Config calls alone — read-only, like every other
    /// assumed session.
    ///
    /// ```toml
    /// controltower_audit_account = "111122223333"
    /// ```
    pub controltower_audit_account: Option<String>,
    /// Role assumed in `controltower_audit_account`. Defaults to the same
    /// role the member-account switch uses (`org_access_role`, else
    /// `OrganizationAccountAccessRole`).
    pub controltower_audit_role: Option<String>,
    /// Base resource-cache freshness window in seconds (default 300). A list
    /// reload within this window serves cached data instead of refetching.
    pub cache_ttl: Option<u64>,
    /// Per-service TTL overrides in seconds, keyed by service prefix (any
    /// alias `@`-search accepts). Unknown prefixes are ignored (the config
    /// parses; the key just has no effect). Cost keeps its built-in 6h TTL
    /// unless overridden here.
    ///
    /// ```toml
    /// [cache_ttls]
    /// lambda = 60
    /// cost   = 86400
    /// ```
    pub cache_ttls: Option<std::collections::HashMap<String, u64>>,
    /// Tag keys read as "owner" for the ownership ribbon on the detail pane,
    /// tried in order, matched case-insensitively (so `owner` also covers
    /// `Owner`). Default: `["owner", "team"]`.
    ///
    /// ```toml
    /// owner_tags = ["squad", "team", "cost-center"]
    /// ```
    pub owner_tags: Option<Vec<String>>,
    /// Show the ownership ribbon (`⛓ stack … · terraform · team …`) on the
    /// detail pane's bottom border (default true). `false` hides the ribbon
    /// only — the change timeline (`W`) still resolves ownership from tags
    /// to pick which stack's events to merge.
    pub ownership_ribbon: Option<bool>,
    /// Tag keys read as the "managed by" marker (values like `terraform`,
    /// `cdk`) for the ownership ribbon, tried in order, matched
    /// case-insensitively. Default: `["managedby", "managed-by",
    /// "managed_by"]` — setting this replaces the list.
    ///
    /// ```toml
    /// managed_by_tags = ["provisioner", "managed-by"]
    /// ```
    pub managed_by_tags: Option<Vec<String>>,
    /// Color theme preset: "dark" (default — the classic look), "light",
    /// "solarized-dark", "solarized-light", "gruvbox-dark", "gruvbox-light",
    /// "dracula", "nord", "catppuccin-mocha", or "catppuccin-latte"
    /// (separators optional; `mocha`/`latte` also work).
    pub theme: Option<String>,
    /// Per-color overrides applied on top of the preset, keyed by palette
    /// field name (`accent`, `warning`, `text_dim`, `selection_bg`, …).
    /// Values are `#rrggbb` hex or ANSI color names. Unknown keys / unparsable
    /// values warn at startup and are ignored.
    ///
    /// ```toml
    /// theme = "light"
    /// [theme_colors]
    /// accent  = "#005faf"
    /// warning = "yellow"
    /// ```
    pub theme_colors: Option<std::collections::HashMap<String, String>>,
    /// Set when the config file existed but failed to parse — the app starts
    /// on defaults and surfaces this in the status bar. Never silently: a
    /// TOML typo used to wipe every setting (default_service, roles, …) with
    /// no hint why.
    #[serde(skip)]
    pub load_warning: Option<String>,
}

impl Config {
    /// Load the config from the first existing candidate path, or defaults.
    /// A file that exists but fails to parse falls back to defaults **with
    /// `load_warning` set** so the failure is visible at startup.
    pub fn load() -> Self {
        for path in Self::candidate_paths() {
            if let Ok(contents) = std::fs::read_to_string(&path) {
                return match toml::from_str(&contents) {
                    Ok(config) => config,
                    Err(e) => {
                        let mut config = Config::default();
                        // First line of the TOML error carries the message;
                        // the rest is a source snippet too wide for the bar.
                        let msg = e.to_string().lines().next().unwrap_or("parse error").to_string();
                        config.load_warning = Some(format!(
                            "Config ignored — {} failed to parse: {}",
                            path.display(),
                            msg
                        ));
                        config
                    }
                };
            }
        }
        Config::default()
    }

    fn candidate_paths() -> Vec<PathBuf> {
        let mut paths = Vec::new();
        if let Ok(p) = std::env::var("NEBOTO_CONFIG") {
            paths.push(PathBuf::from(p));
        }
        if let Ok(home) = std::env::var("HOME") {
            let xdg = std::env::var("XDG_CONFIG_HOME")
                .unwrap_or_else(|_| format!("{}/.config", home));
            paths.push(PathBuf::from(format!("{}/neboto/config.toml", xdg)));
            paths.push(PathBuf::from(format!("{}/.neboto.toml", home)));
        }
        paths
    }

    /// Resolve `default_service` to a `ServiceType` (None if unset/unknown).
    pub fn default_service_type(&self) -> Option<ServiceType> {
        self.default_service
            .as_deref()
            .and_then(ServiceType::from_prefix)
    }

    /// Resolve `default_region` to a `Region` (None if unset/unknown).
    pub fn default_region_typed(&self) -> Option<Region> {
        self.default_region.as_deref().and_then(Region::from_str)
    }

    /// Resolve `cache_ttls` prefixes to typed per-service overrides. Unknown
    /// prefixes are dropped silently — a stale key shouldn't break startup.
    pub fn cache_ttl_overrides(
        &self,
    ) -> std::collections::HashMap<ServiceType, std::time::Duration> {
        self.cache_ttls
            .iter()
            .flatten()
            .filter_map(|(prefix, secs)| {
                ServiceType::from_prefix(prefix)
                    .map(|s| (s, std::time::Duration::from_secs(*secs)))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_org_access_roles_and_default_service() {
        let config: Config = toml::from_str(
            r#"
default_service = "ec2"
org_access_role = "SingleRole"
org_access_roles = ["OrganizationAccountAccessRole", "DeployRole"]
"#,
        )
        .unwrap();
        assert_eq!(config.default_service_type(), Some(ServiceType::EC2));
        assert_eq!(config.org_access_role.as_deref(), Some("SingleRole"));
        assert_eq!(
            config.org_access_roles.as_deref(),
            Some(&["OrganizationAccountAccessRole".to_string(), "DeployRole".to_string()][..])
        );
    }

    #[test]
    fn parses_cache_ttls_and_drops_unknown_prefixes() {
        let config: Config = toml::from_str(
            r#"
cache_ttl = 120

[cache_ttls]
lambda = 60
cost   = 86400
nosuchservice = 5
"#,
        )
        .unwrap();
        assert_eq!(config.cache_ttl, Some(120));
        let overrides = config.cache_ttl_overrides();
        assert_eq!(
            overrides.get(&ServiceType::Lambda),
            Some(&std::time::Duration::from_secs(60))
        );
        assert_eq!(
            overrides.get(&ServiceType::Cost),
            Some(&std::time::Duration::from_secs(86400))
        );
        // The unknown prefix parses but resolves to nothing.
        assert_eq!(overrides.len(), 2);
    }

    #[test]
    fn parses_ownership_keys() {
        let config: Config = toml::from_str(
            r#"
ownership_ribbon = false
owner_tags = ["squad"]
managed_by_tags = ["provisioner"]
"#,
        )
        .unwrap();
        assert_eq!(config.ownership_ribbon, Some(false));
        assert_eq!(config.owner_tags.as_deref(), Some(&["squad".to_string()][..]));
        assert_eq!(
            config.managed_by_tags.as_deref(),
            Some(&["provisioner".to_string()][..])
        );
    }

    #[test]
    fn broken_toml_sets_load_warning_semantics() {
        // Mirrors Config::load's parse arm: a bad file must produce a default
        // config, never a panic — load_warning carries the reason.
        let parsed: std::result::Result<Config, _> = toml::from_str("org_access_roles = [unclosed");
        assert!(parsed.is_err());
    }
}
