//! Headless subcommands (`neboto services`, `neboto ls …`): run once, print,
//! exit — no terminal UI. They build the same service objects and make the
//! same read-only calls the TUI does, so `scripts/check-readonly.py` covers
//! them like everything else under `src/`.
//!
//! Output goes to stdout in the chosen format; warnings (partial loads,
//! config problems) go to stderr, so `-o json | jq` stays parseable. Exit
//! codes: 0 success (an empty list included), 1 an AWS or runtime error,
//! 2 bad usage (unknown service or region).

use crate::app::App;
use crate::aws::client::AwsClients;
use crate::aws::region::Region;
use crate::aws::resource::Resource;
use crate::aws::service::{AwsService, ServiceType};
use crate::cli::{Cli, Command, LsArgs, OutputFormat};
use crate::config::Config;
use crate::event::Event;
use serde_json::json;
use std::io::{IsTerminal, Write};
use std::sync::Arc;
use tokio::sync::mpsc;

/// Version tag carried in every JSON document, so a consumer can tell when
/// the shape changes.
pub const SCHEMA: &str = "neboto/v1";

/// A failure and the exit code it maps to.
#[derive(Debug)]
pub enum Failure {
    /// The request itself is wrong (unknown service / region): exit 2.
    Usage(String),
    /// AWS or the runtime failed: exit 1.
    Error(String),
}

impl Failure {
    fn code(&self) -> i32 {
        match self {
            Failure::Usage(_) => 2,
            Failure::Error(_) => 1,
        }
    }
    fn message(&self) -> &str {
        match self {
            Failure::Usage(m) | Failure::Error(m) => m,
        }
    }
}

/// Run the subcommand in `cli.command` and return the process exit code.
pub async fn run(cli: Cli) -> i32 {
    let format = cli.output.unwrap_or_else(default_format);
    let result = match &cli.command {
        Some(Command::Services) => services(format),
        Some(Command::Ls(args)) => match setup(&cli).await {
            Ok((config, clients)) => ls(&config, &clients, args, format).await,
            Err(f) => Err(f),
        },
        None => Ok(String::new()),
    };
    match result {
        Ok(out) => {
            emit(&out);
            0
        }
        Err(f) => {
            eprintln!("neboto: {}", f.message());
            f.code()
        }
    }
}

/// A terminal gets the readable table; a pipe (an agent, `jq`) gets JSON.
fn default_format() -> OutputFormat {
    if std::io::stdout().is_terminal() {
        OutputFormat::Table
    } else {
        OutputFormat::Json
    }
}

/// Write to stdout, tolerating a closed pipe (`neboto ls … | head`).
fn emit(out: &str) {
    let mut stdout = std::io::stdout().lock();
    let _ = stdout.write_all(out.as_bytes());
    if !out.ends_with('\n') {
        let _ = stdout.write_all(b"\n");
    }
    let _ = stdout.flush();
}

/// Config + clients for a one-shot run. Unlike the TUI, which shrugs off a
/// bad `--region` / `--profile` and carries on with the defaults, this fails:
/// a script that asked for one account must never be answered from another.
async fn setup(cli: &Cli) -> Result<(Config, AwsClients), Failure> {
    let mut config = Config::load();
    cli.apply_to(&mut config);
    if let Some(w) = &config.load_warning {
        eprintln!("neboto: warning: config: {w}");
    }
    if cli.demo {
        crate::demo::enable();
    }
    let mut clients = AwsClients::new_with_endpoint(config.endpoint_url.clone())
        .await
        .map_err(|e| Failure::Error(format!("building AWS clients: {e}")))?;
    if let Some(name) = &config.default_region {
        let region = Region::from_str(name)
            .ok_or_else(|| Failure::Usage(format!("unknown region {name}")))?;
        clients = clients
            .switch_region(region)
            .await
            .map_err(|e| Failure::Error(format!("region {name}: {e}")))?;
    }
    if let Some(profile) = &config.default_profile {
        clients = clients
            .switch_profile(profile.clone())
            .await
            .map_err(|e| Failure::Error(format!("profile {profile}: {e}")))?;
    }
    Ok((config, clients))
}

/// `@ec2`, `ec2` and every alias the TUI's `@` search accepts.
pub fn resolve_service(name: &str) -> Result<ServiceType, Failure> {
    let bare = name.trim().trim_start_matches('@');
    ServiceType::from_prefix(bare).ok_or_else(|| {
        Failure::Usage(format!(
            "unknown service {name} — `neboto services` lists them"
        ))
    })
}

// ── services ────────────────────────────────────────────────────────────────

fn services(format: OutputFormat) -> Result<String, Failure> {
    let mut all = ServiceType::all();
    all.sort_by_key(|s| s.prefix().to_string());
    let rows: Vec<[String; 3]> = all
        .iter()
        .map(|s| {
            [
                s.prefix().to_string(),
                s.name().to_string(),
                if s.is_global() { "global" } else { "regional" }.to_string(),
            ]
        })
        .collect();
    Ok(match format {
        OutputFormat::Json => {
            let items: Vec<_> = all
                .iter()
                .map(|s| json!({"prefix": s.prefix(), "name": s.name(), "global": s.is_global()}))
                .collect();
            pretty(&json!({"schema": SCHEMA, "count": items.len(), "services": items}))
        }
        OutputFormat::Md => markdown_table(&["Prefix", "Service", "Scope"], &rows),
        OutputFormat::Csv => {
            let mut out = String::from("Prefix,Service,Scope\n");
            for r in &rows {
                out.push_str(&r.iter().map(|c| csv_cell(c)).collect::<Vec<_>>().join(","));
                out.push('\n');
            }
            out
        }
        OutputFormat::Table => table(&["PREFIX", "SERVICE", "SCOPE"], &rows),
    })
}

// ── ls ──────────────────────────────────────────────────────────────────────

async fn ls(
    config: &Config,
    clients: &AwsClients,
    args: &LsArgs,
    format: OutputFormat,
) -> Result<String, Failure> {
    let svc = resolve_service(&args.service)?;
    let roles = config.org_access_roles_resolved();
    let services = App::build_services(clients, config.controltower_audit_target(&roles), &roles);
    let service = services
        .get(&svc)
        .cloned()
        .ok_or_else(|| Failure::Error(format!("{} isn't available", svc.name())))?;

    let (resources, warnings) = list_all(service, svc).await?;
    for w in &warnings {
        eprintln!("neboto: warning: {w}");
    }
    let types_present = distinct_types(&resources);
    let rows = filter(resources, args);
    if rows.is_empty() && args.resource_type.is_some() && !types_present.is_empty() {
        eprintln!(
            "neboto: no {} rows of type {:?} — types here: {}",
            svc.name(),
            args.resource_type.as_deref().unwrap_or_default(),
            types_present.join(", ")
        );
    }
    let region = if svc.is_global() { "global".to_string() } else { clients.current_region().as_str().to_string() };
    Ok(render_list(svc, &region, &rows, format))
}

/// Run one service's list load to completion, collecting every streamed
/// batch. A fatal load error fails the command; a phase warning (one part of
/// a multi-phase load failed) is returned alongside the rows, the way the TUI
/// shows `Partial load — …` rather than an empty list.
pub(crate) async fn list_all(
    service: Arc<dyn AwsService>,
    svc: ServiceType,
) -> Result<(Vec<Box<dyn Resource>>, Vec<String>), Failure> {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let task = tokio::spawn(async move { service.list_resources_streaming(tx, svc).await });

    let mut rows: Vec<Box<dyn Resource>> = Vec::new();
    let mut warnings = Vec::new();
    let mut done = false;
    while let Some(event) = rx.recv().await {
        match event {
            Event::ResourcesPartiallyLoaded { resources, .. } => rows.extend(resources),
            // The non-streaming default sends everything in one event.
            Event::ResourcesLoaded { resources, .. } => {
                rows.extend(resources);
                done = true;
            }
            Event::ResourcesFullyLoaded { .. } => done = true,
            Event::ResourceLoadWarning { warning, .. } => warnings.push(warning),
            Event::ResourceLoadError { error, .. } => return Err(Failure::Error(error)),
            _ => {}
        }
        if done {
            break;
        }
    }
    // A warning can race the completion event onto the channel.
    while let Ok(event) = rx.try_recv() {
        if let Event::ResourceLoadWarning { warning, .. } = event {
            warnings.push(warning);
        }
    }
    match task.await {
        Ok(Err(e)) if !done => Err(Failure::Error(e.to_string())),
        Err(e) if !done => Err(Failure::Error(format!("load task failed: {e}"))),
        _ => Ok((rows, warnings)),
    }
}

/// Resource types in load order, each once.
fn distinct_types(resources: &[Box<dyn Resource>]) -> Vec<String> {
    let mut seen = Vec::new();
    for r in resources {
        if !seen.iter().any(|t: &String| t == r.resource_type()) {
            seen.push(r.resource_type().to_string());
        }
    }
    seen
}

/// `--type` matching: the full type or its trailing word(s), any case, so
/// `role` finds "IAM Role" and `group` finds "Security Group". `|` separates
/// alternatives.
pub(crate) fn type_matches(filter: &str, resource_type: &str) -> bool {
    let have = resource_type.to_lowercase();
    filter.split('|').map(|t| t.trim().to_lowercase()).any(|want| {
        !want.is_empty() && (have == want || have.ends_with(&format!(" {want}")))
    })
}

/// The TUI's list filters, as flags: resource type, state, noise, then the
/// search query (exact `tag:` terms + fuzzy text, best match first).
pub(crate) fn filter(resources: Vec<Box<dyn Resource>>, args: &LsArgs) -> Vec<Box<dyn Resource>> {
    let (tag_filters, text) = args
        .filter
        .as_deref()
        .map(crate::search::query_parser::split_tag_filters)
        .unwrap_or_default();
    let kept: Vec<Box<dyn Resource>> = resources
        .into_iter()
        .filter(|r| {
            args.resource_type
                .as_deref()
                .is_none_or(|t| type_matches(t, r.resource_type()))
        })
        .filter(|r| {
            args.state
                .as_deref()
                .is_none_or(|s| r.state_label().eq_ignore_ascii_case(s))
        })
        .filter(|r| !(args.hide_noise && r.is_noise()))
        .filter(|r| tag_filters.iter().all(|f| f.matches(r.tags())))
        .collect();

    let mut kept = if text.trim().is_empty() {
        kept
    } else {
        let order = crate::search::fuzzy::FuzzyMatcher::new().filter_resources(text.trim(), &kept);
        let mut slots: Vec<Option<Box<dyn Resource>>> = kept.into_iter().map(Some).collect();
        order.into_iter().filter_map(|(i, _)| slots[i].take()).collect()
    };
    if let Some(n) = args.limit {
        kept.truncate(n);
    }
    kept
}

fn render_list(svc: ServiceType, region: &str, rows: &[Box<dyn Resource>], format: OutputFormat) -> String {
    let refs: Vec<&dyn Resource> = rows.iter().map(|r| r.as_ref()).collect();
    match format {
        OutputFormat::Json => {
            let items: Vec<_> = refs.iter().map(|r| crate::export::resource_object(*r)).collect();
            pretty(&json!({
                "schema": SCHEMA,
                "service": svc.prefix().trim_start_matches('@'),
                "region": region,
                "count": items.len(),
                "resources": items,
            }))
        }
        OutputFormat::Md => crate::export::list_markdown(&refs, svc.name()),
        OutputFormat::Csv => crate::export::list_csv(&refs),
        OutputFormat::Table => {
            let body: Vec<[String; 4]> = refs
                .iter()
                .map(|r| {
                    [
                        r.resource_type().to_string(),
                        r.id().to_string(),
                        r.name().to_string(),
                        r.state_label(),
                    ]
                })
                .collect();
            table(&["TYPE", "ID", "NAME", "STATE"], &body)
        }
    }
}

// ── formatting ──────────────────────────────────────────────────────────────

fn pretty(v: &serde_json::Value) -> String {
    serde_json::to_string_pretty(v).unwrap_or_default()
}

/// Space-aligned columns by display width (CJK / emoji names line up).
fn table<const N: usize>(headers: &[&str; N], rows: &[[String; N]]) -> String {
    use unicode_width::UnicodeWidthStr;
    let mut widths: [usize; N] = headers.map(|h| h.width());
    for row in rows {
        for (w, cell) in widths.iter_mut().zip(row) {
            *w = (*w).max(cell.width());
        }
    }
    let line = |cells: Vec<&str>| {
        let mut s = String::new();
        for (i, (cell, w)) in cells.iter().zip(widths).enumerate() {
            s.push_str(cell);
            if i + 1 < N {
                s.push_str(&" ".repeat(w - cell.width() + 2));
            }
        }
        s.trim_end().to_string()
    };
    let mut out = vec![line(headers.to_vec())];
    out.extend(rows.iter().map(|r| line(r.iter().map(String::as_str).collect())));
    out.join("\n")
}

fn markdown_table<const N: usize>(headers: &[&str; N], rows: &[[String; N]]) -> String {
    let esc = |s: &str| s.replace('|', "\\|").replace('\n', " ");
    let mut out = format!("| {} |\n|{}|\n", headers.join(" | "), " --- |".repeat(N));
    for r in rows {
        out.push_str(&format!("| {} |\n", r.iter().map(|c| esc(c)).collect::<Vec<_>>().join(" | ")));
    }
    out
}

fn csv_cell(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests;
