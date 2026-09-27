//! Export the current resource list or a single resource's detail to JSON, CSV,
//! and Markdown files in the working directory. Used by the `X` key.
//!
//! Fidelity: the JSON/CSV outputs each carry the resource's core fields
//! (type/id/name/state), the real key:value pairs from `details()` (group
//! headers, table rows, and spacers are skipped), and its tags. The Markdown
//! output instead mirrors the on-screen layout: a list becomes a table, and a
//! single resource becomes a document with one `##` heading per detail-pane
//! section (passed in by the caller via `get_detail_lines`), so the split-pane
//! richness the UI shows is preserved rather than the thin `details()` fallback.

use crate::aws::resource::Resource;
use crate::error::Result;
use serde_json::{json, Map, Value};
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// Real key:value detail rows only — skips group headers (`("X", "")`), plain
/// content lines (`("  …", "")`), and blank spacers (`("", "")`).
fn resource_fields(resource: &dyn Resource) -> Vec<(String, String)> {
    resource
        .details()
        .into_iter()
        .filter_map(|(k, v)| {
            let key = k.trim().to_string();
            if key.is_empty() || v.is_empty() {
                None
            } else {
                Some((key, v))
            }
        })
        .collect()
}

fn tags_sorted(resource: &dyn Resource) -> Vec<(String, String)> {
    let mut tags: Vec<(String, String)> = resource
        .tags()
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    tags.sort_by(|a, b| a.0.cmp(&b.0));
    tags
}

/// JSON object for one resource: core fields + details + nested tags.
fn resource_object(resource: &dyn Resource) -> Value {
    let mut obj = Map::new();
    obj.insert("type".into(), json!(resource.resource_type()));
    obj.insert("id".into(), json!(resource.id()));
    obj.insert("name".into(), json!(resource.name()));
    obj.insert("state".into(), json!(resource.state_label()));
    for (k, v) in resource_fields(resource) {
        obj.insert(k, json!(v));
    }
    let mut tags = Map::new();
    for (k, v) in tags_sorted(resource) {
        tags.insert(k, json!(v));
    }
    obj.insert("tags".into(), Value::Object(tags));
    Value::Object(obj)
}

fn csv_escape(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') || s.contains('\r') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

fn flatten_tags(resource: &dyn Resource) -> String {
    tags_sorted(resource)
        .iter()
        .map(|(k, v)| format!("{}={}", k, v))
        .collect::<Vec<_>>()
        .join(";")
}

/// Sanitize a label for use in a filename.
fn slug(label: &str) -> String {
    let s: String = label
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
        .collect();
    let s = s.trim_matches('-').to_string();
    if s.is_empty() { "export".to_string() } else { s }
}

/// Compact UTC timestamp `YYYYMMDD-HHMMSS` for filenames.
fn timestamp() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let days = secs / 86_400;
    let rem = secs % 86_400;
    let (h, mi, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let (y, mo, d) = epoch_days_to_ymd(days);
    format!("{:04}{:02}{:02}-{:02}{:02}{:02}", y, mo, d, h, mi, s)
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
    for &len in &dm {
        if days < len as i64 {
            break;
        }
        days -= len as i64;
        month += 1;
    }
    (year, month, (days + 1) as u8)
}

fn is_leap(y: i32) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

// ── Markdown rendering (mirrors the on-screen list / detail layout) ───────────

/// Escape a value for a Markdown table cell: pipes and backslashes, and fold
/// newlines so a multi-line value stays on one row.
fn md_cell(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('|', "\\|")
        .replace('\r', "")
        .replace('\n', "<br>")
}

/// True when a section's lines are nothing but a lazy-load placeholder — i.e.
/// the same "Loading…" text the UI shows before the section has been fetched.
/// Such sections are exported as a "not loaded" note rather than the spinner.
fn is_unloaded(lines: &[(String, String)]) -> bool {
    let mut saw_content = false;
    for (k, v) in lines {
        if k.trim().is_empty() && v.trim().is_empty() {
            continue;
        }
        saw_content = true;
        if !format!("{} {}", k, v).to_lowercase().contains("loading") {
            return false;
        }
    }
    saw_content
}

/// Render detail (key, value) rows — the same tuples the UI styles — into
/// Markdown, honouring the conventions in `style_detail_row`:
/// - group header (`("X", "")`)            → bold line
/// - key/value run                         → a `| Field | Value |` table
/// - fixed-width plain-content run (`(" …", "")`) → a fenced code block (keeps alignment)
/// - blank spacer (`("", "")`)             → flush the current block
fn lines_to_markdown(lines: &[(String, String)]) -> String {
    let mut out = String::new();
    let mut kv: Vec<(String, String)> = Vec::new();
    let mut code: Vec<String> = Vec::new();

    fn flush_kv(kv: &mut Vec<(String, String)>, out: &mut String) {
        if kv.is_empty() {
            return;
        }
        out.push_str("| Field | Value |\n| --- | --- |\n");
        for (k, v) in kv.drain(..) {
            out.push_str(&format!("| {} | {} |\n", md_cell(k.trim()), md_cell(&v)));
        }
        out.push('\n');
    }
    fn flush_code(code: &mut Vec<String>, out: &mut String) {
        if code.is_empty() {
            return;
        }
        out.push_str("```\n");
        for line in code.drain(..) {
            out.push_str(&line);
            out.push('\n');
        }
        out.push_str("```\n\n");
    }

    for (k, v) in lines {
        let blank = k.is_empty() && v.is_empty();
        let plain = v.is_empty() && k.starts_with(' ');
        let header = v.is_empty() && !k.is_empty() && !k.starts_with(' ');
        if blank {
            flush_kv(&mut kv, &mut out);
            flush_code(&mut code, &mut out);
        } else if header {
            flush_kv(&mut kv, &mut out);
            flush_code(&mut code, &mut out);
            out.push_str(&format!("**{}**\n\n", k.trim()));
        } else if plain {
            flush_kv(&mut kv, &mut out);
            // Keep verbatim to preserve table alignment; render the SG-rule tab
            // marker as the same inline note the UI shows.
            code.push(k.replace('\t', "  — "));
        } else {
            flush_code(&mut code, &mut out);
            kv.push((k.clone(), v.clone()));
        }
    }
    flush_kv(&mut kv, &mut out);
    flush_code(&mut code, &mut out);
    out
}

/// Insert into `map` under `key`, suffixing `" (2)"`, `" (3)"`, … on a
/// collision so repeated detail keys (e.g. several `Rule` rows) all survive.
fn insert_unique(map: &mut Map<String, Value>, key: &str, value: Value) {
    if !map.contains_key(key) {
        map.insert(key.to_string(), value);
        return;
    }
    for n in 2.. {
        let candidate = format!("{} ({})", key, n);
        if !map.contains_key(&candidate) {
            map.insert(candidate, value);
            return;
        }
    }
}

/// One detail-pane section's rows as a JSON value, honouring the
/// `style_detail_row` conventions:
/// - group header (`("X", "")`)    → a nested object under that name
/// - key/value row                 → a string field (duplicates suffixed)
/// - plain content line (`(" …", "")`) → collected into a `"content"` array
///   (verbatim, so fixed-width tables keep their alignment)
/// - blank spacer                  → dropped
fn lines_to_json(lines: &[(String, String)]) -> Value {
    let mut root = Map::new();
    let mut group: Option<(String, Map<String, Value>)> = None;

    fn flush(group: &mut Option<(String, Map<String, Value>)>, root: &mut Map<String, Value>) {
        if let Some((name, map)) = group.take() {
            insert_unique(root, &name, Value::Object(map));
        }
    }
    fn push_content(map: &mut Map<String, Value>, line: &str) {
        // Render the SG-rule tab marker as the same inline note the UI shows.
        let line = line.replace('\t', "  — ");
        match map.get_mut("content") {
            Some(Value::Array(arr)) => arr.push(json!(line)),
            _ => {
                map.insert("content".into(), json!([line]));
            }
        }
    }

    for (k, v) in lines {
        let blank = k.trim().is_empty() && v.is_empty();
        let plain = v.is_empty() && k.starts_with(' ');
        let header = v.is_empty() && !k.is_empty() && !k.starts_with(' ');
        if blank {
            continue;
        }
        if header {
            flush(&mut group, &mut root);
            group = Some((k.trim().to_string(), Map::new()));
        } else if plain {
            let target = group.as_mut().map(|(_, m)| m).unwrap_or(&mut root);
            push_content(target, k.trim_end());
        } else {
            let target = group.as_mut().map(|(_, m)| m).unwrap_or(&mut root);
            insert_unique(target, k.trim(), json!(v));
        }
    }
    flush(&mut group, &mut root);
    Value::Object(root)
}

/// The whole detail pane as pretty-printed JSON: identity + one entry per
/// section (as captured from `get_detail_lines`) + tags. This is what `e`
/// opens in `$EDITOR` when no richer content (template, policy, raw finding
/// JSON…) applies — full split-pane fidelity, unlike the thin `details()`
/// fallback. Lazy sections not yet fetched become a "not loaded" note.
pub fn detail_json(
    resource: &dyn Resource,
    sections: &[(String, Vec<(String, String)>)],
) -> String {
    serde_json::to_string_pretty(&detail_value(resource, sections))
        .unwrap_or_else(|_| "{}".to_string())
}

/// The `detail_json` object as a `Value` — shared by the single-resource
/// string form above and the multi-resource array export.
fn detail_value(
    resource: &dyn Resource,
    sections: &[(String, Vec<(String, String)>)],
) -> Value {
    let mut root = Map::new();
    let mut ident = Map::new();
    ident.insert("type".into(), json!(resource.resource_type()));
    ident.insert("id".into(), json!(resource.id()));
    ident.insert("name".into(), json!(resource.name()));
    ident.insert("state".into(), json!(resource.state_label()));
    root.insert("resource".into(), Value::Object(ident));

    let mut secs = Map::new();
    for (name, lines) in sections {
        let value = if is_unloaded(lines) {
            json!("(not loaded — open this section in the UI to capture it)")
        } else {
            lines_to_json(lines)
        };
        insert_unique(&mut secs, name.trim(), value);
    }
    root.insert("sections".into(), Value::Object(secs));

    let mut tags = Map::new();
    for (k, v) in tags_sorted(resource) {
        tags.insert(k, json!(v));
    }
    root.insert("tags".into(), Value::Object(tags));

    Value::Object(root)
}

/// True when any captured section is still a lazy-load placeholder — the
/// signal that a deep export fired triggers and should be pressed again once
/// they land, rather than exporting "not loaded" notes.
pub fn any_section_unloaded(sections: &[(String, Vec<(String, String)>)]) -> bool {
    sections.iter().any(|(_, lines)| is_unloaded(lines))
}

/// One resource as a Markdown document: a title + identity line, then a
/// section heading per detail-pane section (as captured from
/// `get_detail_lines`). `level` is the title's heading depth (1 for a
/// standalone document; 2 inside a multi-resource export, where sections
/// then nest at 3).
fn detail_markdown_at(
    resource: &dyn Resource,
    sections: &[(String, Vec<(String, String)>)],
    level: usize,
) -> String {
    let h_title = "#".repeat(level);
    let h_section = "#".repeat(level + 1);
    let mut md = format!(
        "{} {}: {}\n\n`{}` · {}\n\n",
        h_title,
        resource.resource_type(),
        resource.name(),
        resource.id(),
        resource.state_label()
    );
    for (name, lines) in sections {
        md.push_str(&format!("{} {}\n\n", h_section, name));
        if is_unloaded(lines) {
            md.push_str("_Not loaded — open this section in the UI to capture it._\n\n");
            continue;
        }
        let body = lines_to_markdown(lines);
        if body.trim().is_empty() {
            md.push_str("_None._\n\n");
        } else {
            md.push_str(&body);
        }
    }
    // Types without a dedicated Tags section (CwAlarm, IamGroup, the Athena/
    // Glue panes, flat resources whose tags sit inline under Details) still
    // get one, so every .md carries tags uniformly like the .json/.csv do.
    let has_tags_section = sections
        .iter()
        .any(|(name, _)| name.trim().eq_ignore_ascii_case("tags"));
    if !has_tags_section {
        md.push_str(&format!("{} Tags\n\n", h_section));
        let tags = tags_sorted(resource);
        if tags.is_empty() {
            md.push_str("_None._\n\n");
        } else {
            md.push_str("| Tag | Value |\n| --- | --- |\n");
            for (k, v) in tags {
                md.push_str(&format!("| {} | {} |\n", md_cell(&k), md_cell(&v)));
            }
            md.push('\n');
        }
    }
    md
}

fn detail_markdown(resource: &dyn Resource, sections: &[(String, Vec<(String, String)>)]) -> String {
    detail_markdown_at(resource, sections, 1)
}

/// A homogeneous list as a Markdown table over the same columns as the CSV.
/// The list Markdown is a *readable* summary, so it sticks to core columns
/// (the full per-field union lives in the `.csv` / `.json`). A wide
/// union-of-all-detail-keys table is unreadable for rich resources like S3.
pub fn list_markdown(resources: &[&dyn Resource], label: &str) -> String {
    const COLUMNS: [&str; 5] = ["Type", "ID", "Name", "State", "Tags"];
    let mut md = format!("# {} ({})\n\n", label, resources.len());
    md.push_str(&format!(
        "| {} |\n|{}\n",
        COLUMNS.join(" | "),
        " --- |".repeat(COLUMNS.len())
    ));
    for r in resources {
        let row = [
            r.resource_type().to_string(),
            r.id().to_string(),
            r.name().to_string(),
            r.state_label(),
            flatten_tags(*r),
        ];
        let cells: Vec<String> = row.iter().map(|c| md_cell(c)).collect();
        md.push_str(&format!("| {} |\n", cells.join(" | ")));
    }
    md
}

/// Which files an export writes (config `export_formats`, default all
/// three). Applies to `X`, `Ctrl-X` and the deep export alike.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExportFormats {
    pub json: bool,
    pub csv: bool,
    pub md: bool,
}

impl Default for ExportFormats {
    fn default() -> Self {
        ExportFormats {
            json: true,
            csv: true,
            md: true,
        }
    }
}

impl ExportFormats {
    /// Parse config `export_formats`. Case-insensitive; `markdown` aliases
    /// `md`. Unknown values warn and are ignored, and a list left empty
    /// falls back to all three with a warning — a bad value is never fatal
    /// (the `[theme_colors]` precedent).
    pub fn from_config(list: Option<&[String]>) -> (Self, Vec<String>) {
        let Some(list) = list else {
            return (Self::default(), Vec::new());
        };
        let mut f = ExportFormats {
            json: false,
            csv: false,
            md: false,
        };
        let mut warnings = Vec::new();
        for v in list {
            match v.trim().to_ascii_lowercase().as_str() {
                "json" => f.json = true,
                "csv" => f.csv = true,
                "md" | "markdown" => f.md = true,
                other => warnings.push(format!(
                    "export_formats: unknown format {other:?} (use json, csv, md)"
                )),
            }
        }
        if !(f.json || f.csv || f.md) {
            warnings.push("export_formats: no valid format — writing all three".to_string());
            f = Self::default();
        }
        (f, warnings)
    }
}

/// Where and what an export writes — resolved once from config at startup.
#[derive(Debug, Clone, Default)]
pub struct ExportOptions {
    pub formats: ExportFormats,
    /// Config `export_dir` (`~` expanded). `NEBOTO_EXPORT_DIR` still wins.
    pub dir: Option<PathBuf>,
}

impl ExportOptions {
    /// The directory export files land in: `NEBOTO_EXPORT_DIR` if set (the
    /// test harness points it at a temp dir so test runs never litter the
    /// repo), else config `export_dir` (created if missing), else the
    /// working directory.
    fn dir(&self) -> Result<PathBuf> {
        if let Some(env) = std::env::var_os("NEBOTO_EXPORT_DIR") {
            return Ok(PathBuf::from(env));
        }
        match &self.dir {
            Some(d) => {
                fs::create_dir_all(d)?;
                Ok(d.clone())
            }
            None => Ok(PathBuf::new()),
        }
    }
}

/// Expand a leading `~/` in a configured path.
pub fn expand_home(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join(rest))
            .unwrap_or_else(|| PathBuf::from(path)),
        None => PathBuf::from(path),
    }
}

/// Write the enabled formats, each built lazily so a skipped format costs
/// nothing. Returns the written paths in json/csv/md order.
fn write_formats(
    base: &std::path::Path,
    formats: ExportFormats,
    json: impl FnOnce() -> Result<String>,
    csv: impl FnOnce() -> String,
    md: impl FnOnce() -> String,
) -> Result<Vec<PathBuf>> {
    let mut written = Vec::new();
    if formats.json {
        let path = base.with_extension("json");
        fs::write(&path, json()?)?;
        written.push(path);
    }
    if formats.csv {
        let path = base.with_extension("csv");
        fs::write(&path, csv())?;
        written.push(path);
    }
    if formats.md {
        let path = base.with_extension("md");
        fs::write(&path, md())?;
        written.push(path);
    }
    Ok(written)
}

/// Export a homogeneous list to `<slug>-<ts>.json` + `.csv` + `.md` (the
/// formats `opts` enables). Returns the written paths.
pub fn export_list(
    resources: &[&dyn Resource],
    label: &str,
    opts: &ExportOptions,
) -> Result<Vec<PathBuf>> {
    let ts = timestamp();
    let base = opts.dir()?.join(format!("neboto-{}-{}", slug(label), ts));
    write_formats(
        &base,
        opts.formats,
        // JSON: array of resource objects.
        || {
            let arr: Vec<Value> = resources.iter().map(|r| resource_object(*r)).collect();
            Ok(serde_json::to_string_pretty(&Value::Array(arr))?)
        },
        || list_csv(resources),
        // Markdown: a concise, readable summary table (core columns only —
        // the full per-field data is in the .csv / .json).
        || list_markdown(resources, label),
    )
}

/// The list export's CSV: core columns + union of detail keys (first-seen
/// order) + Tags.
fn list_csv(resources: &[&dyn Resource]) -> String {
    let mut columns: Vec<String> = vec![
        "Type".into(),
        "ID".into(),
        "Name".into(),
        "State".into(),
    ];
    let mut seen: HashSet<String> = columns.iter().cloned().collect();
    for r in resources {
        for (k, _) in resource_fields(*r) {
            if seen.insert(k.clone()) {
                columns.push(k);
            }
        }
    }
    columns.push("Tags".into());

    let mut out = String::new();
    out.push_str(&columns.iter().map(|c| csv_escape(c)).collect::<Vec<_>>().join(","));
    out.push('\n');
    for r in resources {
        let fields: std::collections::HashMap<String, String> =
            resource_fields(*r).into_iter().collect();
        let mut row: Vec<String> = Vec::with_capacity(columns.len());
        for col in &columns {
            let cell = match col.as_str() {
                "Type" => r.resource_type().to_string(),
                "ID" => r.id().to_string(),
                "Name" => r.name().to_string(),
                "State" => r.state_label(),
                "Tags" => flatten_tags(*r),
                other => fields.get(other).cloned().unwrap_or_default(),
            };
            row.push(csv_escape(&cell));
        }
        out.push_str(&row.join(","));
        out.push('\n');
    }
    out
}

/// Export one resource to `<slug>-<ts>.json` (object) + `.csv` (Key,Value) +
/// `.md` (a document mirroring the detail pane) — the formats `opts`
/// enables. `sections` is the per-section `(name, lines)` captured from
/// `get_detail_lines`; for a flat resource it's a single "Details" section.
pub fn export_detail(
    resource: &dyn Resource,
    sections: &[(String, Vec<(String, String)>)],
    label: &str,
    opts: &ExportOptions,
) -> Result<Vec<PathBuf>> {
    let ts = timestamp();
    let base = opts.dir()?.join(format!("neboto-{}-{}", slug(label), ts));
    write_formats(
        &base,
        opts.formats,
        || Ok(serde_json::to_string_pretty(&resource_object(resource))?),
        || detail_csv(resource),
        // Markdown: the detail pane mirrored as a document.
        || detail_markdown(resource, sections),
    )
}

/// The single-resource export's CSV: `Key,Value` rows.
fn detail_csv(resource: &dyn Resource) -> String {
    let mut out = String::from("Key,Value\n");
    let core = [
        ("Type", resource.resource_type().to_string()),
        ("ID", resource.id().to_string()),
        ("Name", resource.name().to_string()),
        ("State", resource.state_label()),
    ];
    for (k, v) in core {
        out.push_str(&format!("{},{}\n", csv_escape(k), csv_escape(&v)));
    }
    for (k, v) in resource_fields(resource) {
        out.push_str(&format!("{},{}\n", csv_escape(&k), csv_escape(&v)));
    }
    for (k, v) in tags_sorted(resource) {
        out.push_str(&format!("{},{}\n", csv_escape(&format!("tag:{}", k)), csv_escape(&v)));
    }
    out
}

/// Captured detail-pane sections for one resource: `(section name, rows)` as
/// produced by `detail_sections_snapshot`.
pub type DetailSections = Vec<(String, Vec<(String, String)>)>;

/// Deep-export several resources to one combined
/// `neboto-<slug>-deep-<ts>` trio (`-deep-` keeps a same-second `Ctrl-X`
/// shallow export from colliding):
/// - `.json`: an array of the objects `detail_json` serializes (identity +
///   per-section maps + tags — the full split-pane fidelity)
/// - `.md`: one document — `# <label> (N)`, then a `##` chapter per resource
///   mirroring its detail pane (sections nest at `###`)
/// - `.csv`: long format (`ID, Section, Key, Value`) — deep nested data
///   doesn't fit a wide table, but long format filters in a spreadsheet
pub fn export_detail_multi(
    items: &[(&dyn Resource, DetailSections)],
    label: &str,
    opts: &ExportOptions,
) -> Result<Vec<PathBuf>> {
    let ts = timestamp();
    let base = opts.dir()?.join(format!("neboto-{}-deep-{}", slug(label), ts));
    write_formats(
        &base,
        opts.formats,
        || {
            let arr: Vec<Value> = items
                .iter()
                .map(|(r, sections)| detail_value(*r, sections))
                .collect();
            Ok(serde_json::to_string_pretty(&Value::Array(arr))?)
        },
        || multi_detail_csv(items),
        || {
            let mut md = format!("# {} ({})\n\n", label, items.len());
            for (r, sections) in items {
                md.push_str(&detail_markdown_at(*r, sections, 2));
                md.push('\n');
            }
            md
        },
    )
}

/// The multi-resource deep export's CSV: long format (`ID, Section, Key,
/// Value`) — one row per detail line, filterable in a spreadsheet. Plain
/// content lines (leading-space key, empty value) carry their text in the
/// Value column so fixed-width tables and code previews survive; unloaded
/// lazy sections are skipped (the export gate means there normally are none).
fn multi_detail_csv(items: &[(&dyn Resource, DetailSections)]) -> String {
    let mut csv = String::from("ID,Section,Key,Value\n");
    for (r, sections) in items {
        for (section, lines) in sections {
            if is_unloaded(lines) {
                continue;
            }
            for (k, v) in lines {
                if k.trim().is_empty() && v.is_empty() {
                    continue;
                }
                let (key, value) = if v.is_empty() && k.starts_with(' ') {
                    ("", k.trim())
                } else {
                    (k.trim(), v.as_str())
                };
                csv.push_str(&format!(
                    "{},{},{},{}\n",
                    csv_escape(r.id()),
                    csv_escape(section.trim()),
                    csv_escape(key),
                    csv_escape(value)
                ));
            }
        }
    }
    csv
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_to_markdown_maps_row_conventions() {
        let lines = vec![
            ("Identity".into(), "".into()),          // group header -> bold
            ("Instance ID".into(), "i-123".into()),  // kv -> table
            ("State".into(), "running".into()),
            ("".into(), "".into()),                  // spacer -> flush
            ("  TCP    443    → 0.0.0.0/0".into(), "".into()), // plain -> code block
            ("  desc\tmy home ip".into(), "".into()),          // tab -> inline note
        ];
        let md = lines_to_markdown(&lines);
        assert!(md.contains("**Identity**"));
        assert!(md.contains("| Field | Value |"));
        assert!(md.contains("| Instance ID | i-123 |"));
        assert!(md.contains("```"));
        assert!(md.contains("TCP    443"));
        assert!(md.contains("desc  — my home ip")); // tab rendered as the UI note
    }

    #[test]
    fn md_cell_escapes_pipes_and_newlines() {
        assert_eq!(md_cell("a|b"), "a\\|b");
        assert_eq!(md_cell("line1\nline2"), "line1<br>line2");
    }

    #[derive(Debug, Clone)]
    struct MockResource {
        tags: std::collections::HashMap<String, String>,
    }
    impl Resource for MockResource {
        fn id(&self) -> &str {
            "mock-1"
        }
        fn name(&self) -> &str {
            "mock"
        }
        fn resource_type(&self) -> &str {
            "Mock"
        }
        fn state(&self) -> crate::aws::resource::ResourceState {
            crate::aws::resource::ResourceState::Available
        }
        fn tags(&self) -> &std::collections::HashMap<String, String> {
            &self.tags
        }
        fn details(&self) -> Vec<(String, String)> {
            vec![]
        }
        fn clone_box(&self) -> Box<dyn Resource> {
            Box::new(self.clone())
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    #[test]
    fn detail_markdown_synthesizes_tags_section_when_missing() {
        let r = MockResource {
            tags: [("env".to_string(), "prod".to_string())].into_iter().collect(),
        };
        let sections = vec![(
            "Overview".to_string(),
            vec![("Name".to_string(), "mock".to_string())],
        )];
        let md = detail_markdown(&r, &sections);
        assert!(md.contains("## Tags"));
        assert!(md.contains("| env | prod |"));

        // A resource whose sections already include Tags gets no duplicate.
        let sections_with_tags = vec![
            ("Overview".to_string(), vec![("Name".to_string(), "mock".to_string())]),
            ("Tags".to_string(), vec![("env".to_string(), "prod".to_string())]),
        ];
        let md = detail_markdown(&r, &sections_with_tags);
        assert_eq!(md.matches("## Tags").count(), 1);

        // No tags at all → an explicit empty marker, not a bare heading.
        let untagged = MockResource { tags: Default::default() };
        let md = detail_markdown(&untagged, &sections);
        assert!(md.contains("## Tags\n\n_None._"));
    }

    #[test]
    fn lines_to_json_maps_row_conventions() {
        let lines = vec![
            ("State".into(), "running".into()),               // kv at section root
            ("Identity".into(), "".into()),                   // group header -> nested
            ("Instance ID".into(), "i-123".into()),
            ("Rule".into(), "allow 80".into()),               // duplicate keys
            ("Rule".into(), "allow 443".into()),
            ("".into(), "".into()),                           // spacer -> dropped
            ("Ports".into(), "".into()),                      // second group
            ("  TCP    443    → 0.0.0.0/0".into(), "".into()), // plain -> content array
            ("  desc\tmy home ip".into(), "".into()),          // tab -> inline note
        ];
        let v = lines_to_json(&lines);
        assert_eq!(v["State"], "running");
        assert_eq!(v["Identity"]["Instance ID"], "i-123");
        assert_eq!(v["Identity"]["Rule"], "allow 80");
        assert_eq!(v["Identity"]["Rule (2)"], "allow 443");
        let content = v["Ports"]["content"].as_array().unwrap();
        assert!(content[0].as_str().unwrap().contains("TCP    443"));
        assert!(content[1].as_str().unwrap().contains("desc  — my home ip"));
    }

    #[test]
    fn detail_json_carries_identity_sections_and_tags() {
        let r = MockResource {
            tags: [("env".to_string(), "prod".to_string())].into_iter().collect(),
        };
        let sections = vec![
            (
                "Overview".to_string(),
                vec![("Name".to_string(), "mock".to_string())],
            ),
            (
                "Breakdown".to_string(),
                vec![("  Loading breakdown…".to_string(), "".to_string())],
            ),
        ];
        let out = detail_json(&r, &sections);
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["resource"]["id"], "mock-1");
        assert_eq!(v["resource"]["type"], "Mock");
        assert_eq!(v["sections"]["Overview"]["Name"], "mock");
        // Unloaded lazy section becomes a note, not a spinner dump.
        assert!(v["sections"]["Breakdown"].as_str().unwrap().contains("not loaded"));
        assert_eq!(v["tags"]["env"], "prod");
        // preserve_order: sections and fields appear in on-screen order.
        assert!(out.find("Overview").unwrap() < out.find("Breakdown").unwrap());
    }

    #[test]
    fn is_unloaded_detects_loading_placeholder_only() {
        assert!(is_unloaded(&[("  Loading breakdown…".into(), "".into())]));
        assert!(is_unloaded(&[
            ("".into(), "".into()),
            ("  Loading forecast…".into(), "".into()),
        ]));
        // Real content is not "unloaded".
        assert!(!is_unloaded(&[("Cost".into(), "$10.00".into())]));
        // Genuinely empty section is not flagged as unloaded.
        assert!(!is_unloaded(&[("".into(), "".into())]));
    }

    #[test]
    fn any_section_unloaded_gates_the_press_again_export() {
        let loaded = ("Overview".to_string(), vec![("Name".to_string(), "x".to_string())]);
        let loading = (
            "Rules".to_string(),
            vec![("Loading…".to_string(), "".to_string())],
        );
        assert!(any_section_unloaded(&[loaded.clone(), loading]));
        assert!(!any_section_unloaded(&[loaded]));
    }

    #[test]
    fn detail_markdown_at_nests_headings_for_multi_export() {
        let r = MockResource { tags: Default::default() };
        let sections = vec![(
            "Overview".to_string(),
            vec![("Name".to_string(), "mock".to_string())],
        )];
        let md = detail_markdown_at(&r, &sections, 2);
        assert!(md.starts_with("## Mock: mock"), "chapter title at ##: {md}");
        assert!(md.contains("\n### Overview\n"), "sections nest at ###: {md}");
        assert!(md.contains("\n### Tags\n"), "synthesized Tags follows depth: {md}");
        // The standalone document is unchanged at level 1.
        let solo = detail_markdown(&r, &sections);
        assert!(solo.starts_with("# Mock: mock"));
        assert!(solo.contains("\n## Overview\n"));
    }

    #[test]
    fn multi_detail_csv_is_long_format() {
        let a = MockResource { tags: Default::default() };
        let b = MockResource { tags: Default::default() };
        let items: Vec<(&dyn Resource, Vec<(String, Vec<(String, String)>)>)> = vec![
            (
                &a,
                vec![(
                    "Overview".to_string(),
                    vec![
                        ("Name".to_string(), "mock".to_string()),
                        ("".to_string(), "".to_string()), // spacer dropped
                        ("  fixed-width row".to_string(), "".to_string()),
                    ],
                )],
            ),
            (
                &b,
                vec![(
                    "Rules".to_string(),
                    vec![("Loading…".to_string(), "".to_string())], // unloaded: skipped
                )],
            ),
        ];
        let csv = multi_detail_csv(&items);
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines[0], "ID,Section,Key,Value");
        assert_eq!(lines[1], "mock-1,Overview,Name,mock");
        // Plain content line: empty key, text in the Value column.
        assert_eq!(lines[2], "mock-1,Overview,,fixed-width row");
        // The unloaded section contributed nothing.
        assert_eq!(lines.len(), 3);
    }

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn export_formats_default_is_all_three() {
        let (f, w) = ExportFormats::from_config(None);
        assert_eq!(f, ExportFormats::default());
        assert!(f.json && f.csv && f.md);
        assert!(w.is_empty());
    }

    #[test]
    fn export_formats_parse_alias_case_and_unknowns() {
        let (f, w) = ExportFormats::from_config(Some(&strings(&["JSON", "Markdown", "xlsx"])));
        assert_eq!(
            f,
            ExportFormats {
                json: true,
                csv: false,
                md: true
            }
        );
        assert_eq!(w.len(), 1);
        assert!(w[0].contains("xlsx"), "{w:?}");
    }

    #[test]
    fn export_formats_empty_or_all_invalid_falls_back_to_all() {
        for list in [strings(&[]), strings(&["pdf"])] {
            let (f, w) = ExportFormats::from_config(Some(&list));
            assert_eq!(f, ExportFormats::default());
            assert!(w.iter().any(|m| m.contains("writing all three")), "{w:?}");
        }
    }

    #[test]
    fn exports_write_only_the_configured_formats() {
        let r = MockResource {
            tags: Default::default(),
        };
        let only = |json, csv, md| ExportOptions {
            formats: ExportFormats { json, csv, md },
            dir: Some(std::env::temp_dir().join("neboto-test-exports")),
        };
        let exts = |paths: &[PathBuf]| -> Vec<String> {
            paths
                .iter()
                .inspect(|p| assert!(p.exists(), "{} not written", p.display()))
                .map(|p| p.extension().unwrap().to_string_lossy().into_owned())
                .collect()
        };

        let paths = export_list(&[&r], "FmtList", &only(true, false, false)).unwrap();
        assert_eq!(exts(&paths), ["json"]);

        let sections = vec![("Details".to_string(), vec![("K".to_string(), "v".to_string())])];
        let paths = export_detail(&r, &sections, "FmtDetail", &only(false, true, true)).unwrap();
        assert_eq!(exts(&paths), ["csv", "md"]);

        let items: Vec<(&dyn Resource, DetailSections)> = vec![(&r, sections)];
        let paths = export_detail_multi(&items, "FmtMulti", &only(false, false, true)).unwrap();
        assert_eq!(exts(&paths), ["md"]);
    }

    #[test]
    fn expand_home_only_touches_a_leading_tilde() {
        assert_eq!(expand_home("/tmp/x"), PathBuf::from("/tmp/x"));
        if let Some(home) = std::env::var_os("HOME") {
            assert_eq!(expand_home("~/exports"), PathBuf::from(home).join("exports"));
        }
    }
}
