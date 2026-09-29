//! Command-line argument parsing. Every switch is optional and **overrides the
//! config file** (`~/.config/neboto/config.toml` etc.) for this run only — so you
//! can jump straight to a service/region/profile without editing config.

use crate::config::Config;
use clap::{Parser, Subcommand, ValueEnum};

/// neboto — a read-only AWS resource browser TUI.
///
/// With no arguments it honours your config file (or shows the welcome splash).
/// Flags below override the config for this run, e.g. `neboto -s ec2 -r eu-west-1`.
/// With a subcommand it runs once and prints instead of opening the TUI —
/// the same read-only calls, for scripts and agents.
#[derive(Debug, Parser)]
#[command(name = "neboto", version, about, long_about = None)]
pub struct Cli {
    /// Run once and print instead of opening the TUI.
    #[command(subcommand)]
    pub command: Option<Command>,

    /// Output format for subcommands (default: table on a terminal, json
    /// when piped).
    #[arg(short = 'o', long, value_enum, global = true, value_name = "FORMAT")]
    pub output: Option<OutputFormat>,

    /// Service to open on startup (prefix/alias, e.g. ec2, s3, iam, @cw).
    #[arg(short = 's', long, value_name = "SERVICE")]
    pub service: Option<String>,

    /// Region to start in (e.g. us-west-2, eu-west-1).
    #[arg(short = 'r', long, global = true, value_name = "REGION")]
    pub region: Option<String>,

    /// Named AWS profile to use.
    #[arg(short = 'p', long, global = true, value_name = "PROFILE")]
    pub profile: Option<String>,

    /// Custom AWS endpoint URL (local emulator, e.g. http://localhost:4566).
    #[arg(long, global = true, value_name = "URL")]
    pub endpoint_url: Option<String>,

    /// Show the ASCII banner on startup.
    #[arg(long, conflicts_with = "no_banner")]
    pub banner: bool,

    /// Hide the ASCII banner on startup.
    #[arg(long)]
    pub no_banner: bool,

    /// Start in watch mode: auto-refresh the current view (default every 10s;
    /// `w` toggles, `+`/`-` tune the interval once running).
    #[arg(short = 'w', long)]
    pub watch: bool,

    /// Show each key as it's pressed, and what it did, in a corner box —
    /// for screen recordings, screen shares and demos.
    #[arg(long)]
    pub show_keys: bool,

    /// Run a saved macro on startup, by name (see `,` in the TUI).
    #[arg(short = 'm', long = "macro", value_name = "NAME")]
    pub macro_name: Option<String>,

    /// Try neboto without an AWS account: browse a made-up account from
    /// canned responses, fully offline. Ignores profiles and endpoints.
    #[arg(long, global = true)]
    pub demo: bool,

    /// Color theme preset: dark (default), light, solarized-dark,
    /// solarized-light, gruvbox-dark, gruvbox-light, dracula, nord,
    /// catppuccin-mocha, catppuccin-latte.
    #[arg(long, value_name = "THEME")]
    pub theme: Option<String>,
}

/// The one-shot subcommands. Each makes the same read-only calls the TUI
/// makes and prints the result — nothing here can change an account.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// List every service: its @prefix, name, and whether it's global.
    Services,
    /// List a service's resources, filtered like the TUI's search.
    #[command(visible_alias = "list")]
    Ls(LsArgs),
    /// Show resources' full detail — every detail-pane section, lazy ones
    /// included.
    Get(GetArgs),
}

#[derive(Debug, clap::Args)]
pub struct GetArgs {
    /// Service prefix or alias, with or without the @.
    #[arg(value_name = "SERVICE")]
    pub service: String,

    /// Resource ids (or ARNs, where that is the id) or exact names — up to
    /// 50. An id wins over a same-named resource.
    #[arg(value_name = "ID", required = true, num_args = 1..)]
    pub ids: Vec<String>,

    /// Pick among resources that share a name, like `ls --type`: the type or
    /// its last word(s), any case, e.g. service, "task definition".
    #[arg(short = 't', long = "type", value_name = "TYPE")]
    pub resource_type: Option<String>,

    /// Only this detail section (repeatable, any case), e.g. --section
    /// permissions. Only its data is fetched.
    #[arg(long = "section", value_name = "NAME")]
    pub sections: Vec<String>,

    /// Seconds to wait for lazily-loaded sections; anything still loading
    /// after that prints as "not loaded".
    #[arg(long, value_name = "SECS", default_value_t = 60)]
    pub wait: u64,
}

#[derive(Debug, clap::Args)]
pub struct LsArgs {
    /// Service prefix or alias, with or without the @ (ec2, @lambda, iam, …;
    /// `neboto services` lists them).
    #[arg(value_name = "SERVICE")]
    pub service: String,

    /// Only this resource type: the type column's text or its last word(s),
    /// any case, `|` for several — e.g. "Security Group", role, "role|policy".
    #[arg(short = 't', long = "type", value_name = "TYPE")]
    pub resource_type: Option<String>,

    /// Filter like the TUI search: fuzzy text plus exact `tag:key[=value]`
    /// terms, e.g. 'web tag:env=prod'.
    #[arg(short = 'f', long, value_name = "QUERY")]
    pub filter: Option<String>,

    /// Only rows in this state, as the state column shows it
    /// (case-insensitive), e.g. running.
    #[arg(long, value_name = "STATE")]
    pub state: Option<String>,

    /// Drop noise rows (AWS-managed defaults, automated snapshots, …) — the
    /// TUI's `a`.
    #[arg(long)]
    pub hide_noise: bool,

    /// Print at most N rows.
    #[arg(long, value_name = "N")]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    /// Aligned text for reading in a terminal.
    Table,
    /// One JSON document carrying `"schema": "neboto/v1"`.
    Json,
    /// Markdown: a table for lists, a document for `get`.
    Md,
    /// CSV: one row per resource for `ls`, one per detail row for `get`.
    Csv,
}

impl Cli {
    /// Banner preference, if either flag was given (`--banner` → on, `--no-banner`
    /// → off, neither → `None` to defer to config).
    fn banner_pref(&self) -> Option<bool> {
        if self.no_banner {
            Some(false)
        } else if self.banner {
            Some(true)
        } else {
            None
        }
    }

    /// Overlay these CLI switches onto a loaded `Config` (CLI wins where set).
    ///
    /// `--macro` is deliberately absent: it names an *action* for this run, not
    /// a setting, and it's applied by `App::arm_startup_macro` after the app is
    /// built (the saved macros have to be loaded before the name can resolve).
    pub fn apply_to(&self, config: &mut Config) {
        if self.service.is_some() {
            config.default_service = self.service.clone();
        }
        if self.region.is_some() {
            config.default_region = self.region.clone();
        }
        if self.profile.is_some() {
            config.default_profile = self.profile.clone();
        }
        if self.endpoint_url.is_some() {
            config.endpoint_url = self.endpoint_url.clone();
        }
        if let Some(b) = self.banner_pref() {
            config.show_banner = Some(b);
        }
        if self.watch {
            config.watch = Some(true);
        }
        if self.show_keys {
            config.show_keys = Some(true);
        }
        if self.theme.is_some() {
            config.theme = self.theme.clone();
        }
        // Demo mode never touches real credentials or a configured emulator,
        // and opens on a service with data rather than the splash.
        if self.demo {
            config.endpoint_url = None;
            config.default_profile = None;
            config.default_region = Some(crate::demo::REGION.to_string());
            if self.service.is_none() {
                config.default_service = Some("ecs".to_string());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty() -> Cli {
        Cli {
            command: None,
            output: None,
            service: None,
            region: None,
            profile: None,
            endpoint_url: None,
            banner: false,
            no_banner: false,
            watch: false,
            show_keys: false,
            macro_name: None,
            theme: None,
            demo: false,
        }
    }

    #[test]
    fn watch_flag_overlays_config() {
        let mut cfg = Config::default();
        Cli { watch: true, ..empty() }.apply_to(&mut cfg);
        assert_eq!(cfg.watch, Some(true));
        // Absent flag leaves a config-file `watch = false` untouched.
        let mut cfg = Config { watch: Some(false), ..Config::default() };
        empty().apply_to(&mut cfg);
        assert_eq!(cfg.watch, Some(false));
    }

    #[test]
    fn cli_overrides_config_where_set() {
        let mut cfg = Config {
            default_service: Some("s3".into()),
            default_region: Some("us-east-1".into()),
            ..Config::default()
        };
        let cli = Cli {
            service: Some("ec2".into()),
            no_banner: true,
            ..empty()
        };
        cli.apply_to(&mut cfg);
        assert_eq!(cfg.default_service.as_deref(), Some("ec2")); // overridden
        assert_eq!(cfg.default_region.as_deref(), Some("us-east-1")); // untouched
        assert_eq!(cfg.show_banner, Some(false));
    }

    #[test]
    fn banner_flags_resolve() {
        assert_eq!(empty().banner_pref(), None);
        assert_eq!(Cli { banner: true, ..empty() }.banner_pref(), Some(true));
        assert_eq!(Cli { no_banner: true, ..empty() }.banner_pref(), Some(false));
    }
}
