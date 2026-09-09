use crate::aws::services::s3::{fmt_object_size, S3Entry};
use crate::ui::theme;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        block::{Position, Title},
        Clear, Paragraph,
    },
    Frame,
};

/// Column the object listing is sorted by (folders always stay grouped on top).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum S3SortField {
    Name,
    Size,
    Modified,
}

impl S3SortField {
    pub fn label(&self) -> &'static str {
        match self {
            S3SortField::Name => "name",
            S3SortField::Size => "size",
            S3SortField::Modified => "modified",
        }
    }

    pub fn next(&self) -> Self {
        match self {
            S3SortField::Name => S3SortField::Size,
            S3SortField::Size => S3SortField::Modified,
            S3SortField::Modified => S3SortField::Name,
        }
    }
}

/// State for the full-screen S3 object browser (a file-manager-style view of a
/// bucket using `/` as the folder delimiter).
pub struct S3ObjectBrowserState {
    pub visible: bool,
    pub bucket: String,
    pub region: String,
    /// Current "folder" — `""` is the root, otherwise ends with `/`.
    pub prefix: String,
    /// All entries loaded so far for the current prefix (folders then objects).
    pub entries: Vec<S3Entry>,
    /// Selection index into the *filtered* view.
    pub selected: usize,
    pub filter: String,
    pub filtering: bool,
    pub next_token: Option<String>,
    /// Recursive (flat, no-delimiter) listing of everything under the prefix.
    pub recursive: bool,
    /// Versions mode: list every object version + delete marker
    /// (`ListObjectVersions`) instead of the current objects.
    pub versions: bool,
    pub loading: bool,
    pub error: Option<String>,
    /// Transient one-line notice (download done, copied, too large, …) shown in
    /// the status line — the real status bar is hidden under this overlay.
    pub message: Option<String>,
    pub sort_field: S3SortField,
    pub sort_desc: bool,
    /// Whether the object metadata (info) panel is open.
    pub show_detail: bool,
    /// Whether keyboard focus is in the metadata panel (vs the listing).
    pub detail_focus: bool,
    /// Selected metadata row within the panel (when focused).
    pub detail_selected: usize,
    pub show_help: bool,
}

impl S3ObjectBrowserState {
    pub fn new() -> Self {
        Self {
            visible: false,
            bucket: String::new(),
            region: String::new(),
            prefix: String::new(),
            entries: Vec::new(),
            selected: 0,
            filter: String::new(),
            filtering: false,
            next_token: None,
            recursive: false,
            versions: false,
            loading: false,
            error: None,
            message: None,
            sort_field: S3SortField::Name,
            sort_desc: false,
            show_detail: false,
            detail_focus: false,
            detail_selected: 0,
            show_help: false,
        }
    }

    pub fn open(&mut self, bucket: String, region: String) {
        self.visible = true;
        self.bucket = bucket;
        self.region = region;
        self.prefix = String::new();
        self.entries.clear();
        self.selected = 0;
        self.filter.clear();
        self.filtering = false;
        self.next_token = None;
        self.recursive = false;
        self.versions = false;
        self.loading = false;
        self.error = None;
        self.message = None;
        self.show_detail = false;
        self.detail_focus = false;
        self.detail_selected = 0;
        self.show_help = false;
    }

    /// Cache key for an object's metadata: `bucket/key`, with the version id
    /// appended when the row is a specific version.
    pub fn meta_key(bucket: &str, key: &str, version: Option<&str>) -> String {
        match version {
            Some(v) => format!("{}/{}@{}", bucket, key, v),
            None => format!("{}/{}", bucket, key),
        }
    }

    pub fn close(&mut self) {
        self.visible = false;
    }

    /// Reset the listing state when navigating to a new prefix.
    pub fn set_prefix(&mut self, prefix: String) {
        self.prefix = prefix;
        self.entries.clear();
        self.selected = 0;
        self.next_token = None;
        self.error = None;
        self.message = None;
        self.filter.clear();
        self.filtering = false;
        self.detail_focus = false;
        self.detail_selected = 0;
    }

    /// Parent prefix of the current one (`logs/2026/` -> `logs/`, `logs/` -> ``).
    pub fn parent_prefix(&self) -> Option<String> {
        if self.prefix.is_empty() {
            return None;
        }
        let trimmed = self.prefix.trim_end_matches('/');
        match trimmed.rfind('/') {
            Some(i) => Some(self.prefix[..=i].to_string()),
            None => Some(String::new()),
        }
    }

    fn matches(&self, e: &S3Entry) -> bool {
        if self.filter.is_empty() {
            return true;
        }
        e.display_name(&self.prefix)
            .to_lowercase()
            .contains(&self.filter.to_lowercase())
    }

    /// Indices into `entries` that pass the current filter, in display order.
    pub fn filtered_indices(&self) -> Vec<usize> {
        self.entries
            .iter()
            .enumerate()
            .filter(|(_, e)| self.matches(e))
            .map(|(i, _)| i)
            .collect()
    }

    pub fn selected_entry(&self) -> Option<&S3Entry> {
        let idxs = self.filtered_indices();
        idxs.get(self.selected).and_then(|&i| self.entries.get(i))
    }

    pub fn clamp_selection(&mut self) {
        let n = self.filtered_indices().len();
        if n == 0 {
            self.selected = 0;
        } else if self.selected >= n {
            self.selected = n - 1;
        }
    }

    /// Move the selection by a signed delta, clamped to the filtered list.
    pub fn move_selection(&mut self, delta: isize) {
        self.message = None;
        let n = self.filtered_indices().len();
        if n == 0 {
            self.selected = 0;
            return;
        }
        let max = (n - 1) as isize;
        let next = (self.selected as isize + delta).clamp(0, max);
        self.selected = next as usize;
    }

    /// Re-sort the loaded entries: folders stay grouped on top (by name); objects
    /// sort by the active field + direction. Note this sorts only what's loaded —
    /// S3 returns keys lexicographically and there's no server-side sort, so for
    /// a truncated listing it covers the pages fetched so far.
    pub fn apply_sort(&mut self) {
        use std::cmp::Ordering;
        let prefix = self.prefix.clone();
        let field = self.sort_field;
        let desc = self.sort_desc;
        let name_of = |e: &S3Entry| e.display_name(&prefix).to_lowercase();
        self.entries.sort_by(|a, b| {
            match (a.is_folder(), b.is_folder()) {
                (true, false) => return Ordering::Less,
                (false, true) => return Ordering::Greater,
                _ => {}
            }
            let ord = if a.is_folder() {
                name_of(a).cmp(&name_of(b)) // folders: always name-ascending
            } else {
                let o = match field {
                    S3SortField::Name => name_of(a).cmp(&name_of(b)),
                    S3SortField::Size => entry_size(a).cmp(&entry_size(b)),
                    S3SortField::Modified => entry_modified(a).cmp(entry_modified(b)),
                };
                if desc {
                    o.reverse()
                } else {
                    o
                }
            };
            ord.then_with(|| name_of(a).cmp(&name_of(b)))
        });
        self.clamp_selection();
    }

    /// Compact sort indicator for the header, e.g. `size ↓`.
    pub fn sort_indicator(&self) -> String {
        format!(
            "{} {}",
            self.sort_field.label(),
            if self.sort_desc { "↓" } else { "↑" }
        )
    }
}

fn entry_size(e: &S3Entry) -> i64 {
    match e {
        S3Entry::Object { size, .. } | S3Entry::Version { size, .. } => *size,
        _ => 0,
    }
}

fn entry_modified(e: &S3Entry) -> &str {
    match e {
        S3Entry::Object { last_modified, .. } | S3Entry::Version { last_modified, .. } => {
            last_modified.as_deref().unwrap_or("")
        }
        _ => "",
    }
}

pub fn render_s3_object_browser(app: &crate::app::App, area: Rect, frame: &mut Frame) {
    let st = &app.s3_object_browser;
    if !st.visible {
        return;
    }

    let block = theme::popup_block(&format!("Objects — {}", st.bucket)).title(
        Title::from(Span::styled(
            " ⏎/l open · h up · i info · / filter · f recurse · V versions · s sort · n next · d dl · e edit · p url · y copy · Z width · ? help · Esc close ",
            Style::default().fg(theme::text_dim()),
        ))
        .position(Position::Bottom)
        .alignment(Alignment::Center),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // breadcrumb + filter
            Constraint::Length(1), // column header (sortable)
            Constraint::Length(1), // rule
            Constraint::Min(0),    // listing
            Constraint::Length(1), // status
        ])
        .split(inner);

    render_breadcrumb(st, chunks[0], frame);
    render_header(st, chunks[1], frame);
    render_hr(chunks[2], frame);

    // When the info panel is open, split the listing area: list on top, the
    // selected object's metadata below.
    if st.show_detail {
        let split = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Percentage(55),
                Constraint::Length(1),
                Constraint::Min(0),
            ])
            .split(chunks[3]);
        render_listing(st, split[0], frame);
        render_hr(split[1], frame);
        render_detail_panel(app, split[2], frame);
    } else {
        render_listing(st, chunks[3], frame);
    }

    render_status(st, chunks[4], frame);

    if st.show_help {
        render_help(area, frame);
    }
}

/// Metadata panel for the selected object (lazy `head_object`, cached on `App`).
fn render_detail_panel(app: &crate::app::App, area: Rect, frame: &mut Frame) {
    use crate::aws::services::s3::S3Entry;
    let st = &app.s3_object_browser;
    let Some((key, version)) = st.selected_entry().and_then(|e| e.content_ref()) else {
        let hint = match st.selected_entry() {
            Some(S3Entry::Version { delete_marker: true, .. }) => {
                "  Delete marker — no content or metadata"
            }
            _ => "  Select an object for metadata",
        };
        frame.render_widget(
            Paragraph::new(Line::styled(hint, Style::default().fg(theme::text_dim()))),
            area,
        );
        return;
    };

    let mk = S3ObjectBrowserState::meta_key(&st.bucket, key, version);
    let header = Line::from(vec![
        Span::raw("  "),
        Span::styled(
            basename(key),
            Style::default().fg(crate::ui::theme::text_primary()).add_modifier(Modifier::BOLD),
        ),
    ]);

    let mut lines: Vec<Line> = vec![header, Line::raw("")];
    match app.lazy.s3_object_meta.get(&mk) {
        None | Some(crate::lazy::Lazy::Loading) => {
            lines.push(Line::styled(
                "  Loading metadata…",
                Style::default().fg(theme::text_dim()),
            ));
        }
        Some(crate::lazy::Lazy::Error(e)) => {
            lines.push(Line::styled(format!("  ✗ {}", e), Style::default().fg(theme::error())));
        }
        Some(crate::lazy::Lazy::Loaded(meta)) => {
            for (i, (k, v)) in meta.rows().into_iter().enumerate() {
                let focused = st.detail_focus && i == st.detail_selected;
                let marker = if focused { "▸ " } else { "  " };
                let val_style = if focused {
                    theme::selection_style(true)
                } else {
                    Style::default().fg(crate::ui::theme::text_primary())
                };
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("{}{:<18}", marker, k),
                        Style::default().fg(theme::accent()),
                    ),
                    Span::styled(v, val_style),
                ]));
            }
            if st.detail_focus {
                lines.push(Line::raw(""));
                lines.push(Line::styled(
                    "  j/k row · y copy value · Y copy all · Tab/Esc back to list",
                    Style::default().fg(theme::text_dim()),
                ));
            } else {
                lines.push(Line::raw(""));
                lines.push(Line::styled(
                    "  Tab to focus & copy",
                    Style::default().fg(theme::text_dim()),
                ));
            }
        }
    }
    frame.render_widget(Paragraph::new(lines), area);
}

fn basename(key: &str) -> String {
    key.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(key)
        .to_string()
}

fn render_breadcrumb(st: &S3ObjectBrowserState, area: Rect, frame: &mut Frame) {
    let path = format!("s3://{}/{}", st.bucket, st.prefix);
    let mut spans = vec![
        Span::raw(" "),
        Span::styled(path, Style::default().fg(theme::aws_orange()).add_modifier(Modifier::BOLD)),
    ];
    if st.recursive {
        spans.push(Span::styled(
            "  ↳ recursive",
            Style::default().fg(theme::warning()).add_modifier(Modifier::BOLD),
        ));
    }
    if st.versions {
        spans.push(Span::styled(
            "  ⧉ versions",
            Style::default().fg(theme::warning()).add_modifier(Modifier::BOLD),
        ));
    }
    if st.filtering || !st.filter.is_empty() {
        let style = if st.filtering {
            Style::default().fg(Color::Black).bg(theme::aws_orange()).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(crate::ui::theme::text_primary())
        };
        spans.push(Span::raw("   "));
        spans.push(Span::styled("filter: ", Style::default().fg(theme::text_dim())));
        spans.push(Span::styled(
            format!(" {} ", if st.filter.is_empty() { "_" } else { &st.filter }),
            style,
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// Width of the name column, shared by the header and the rows so they align.
fn name_col_width(width: usize) -> usize {
    width.saturating_sub(40).clamp(16, 70)
}

fn render_header(st: &S3ObjectBrowserState, area: Rect, frame: &mut Frame) {
    let name_w = name_col_width(area.width as usize);
    let arrow = if st.sort_desc { " ↓" } else { " ↑" };
    let label = |f: S3SortField, base: &str| -> String {
        if st.sort_field == f {
            format!("{}{}", base, arrow)
        } else {
            base.to_string()
        }
    };
    let style = |f: S3SortField| {
        if st.sort_field == f {
            Style::default().fg(theme::accent()).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme::text_dim())
        }
    };
    let spans = vec![
        Span::raw("   "),
        Span::styled(
            format!("{:<w$}", trunc(&label(S3SortField::Name, "Name"), name_w), w = name_w),
            style(S3SortField::Name),
        ),
        Span::raw("  "),
        Span::styled(format!("{:>10}", label(S3SortField::Size, "Size")), style(S3SortField::Size)),
        Span::raw("  "),
        Span::styled(
            format!("{:<16}", label(S3SortField::Modified, "Modified")),
            style(S3SortField::Modified),
        ),
        Span::raw("  "),
        Span::styled("Storage", Style::default().fg(theme::text_dim())),
    ];
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn render_listing(st: &S3ObjectBrowserState, area: Rect, frame: &mut Frame) {
    if st.loading && st.entries.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::styled("  Loading…", Style::default().fg(theme::text_dim()))),
            area,
        );
        return;
    }
    if let Some(err) = &st.error {
        frame.render_widget(
            Paragraph::new(vec![
                Line::styled(format!("  ✗ {}", err), Style::default().fg(theme::error())),
                Line::styled("  press y to copy · Esc to close", Style::default().fg(theme::text_dim())),
            ]),
            area,
        );
        return;
    }

    let idxs = st.filtered_indices();
    if idxs.is_empty() {
        let msg = if st.entries.is_empty() {
            "  (empty)"
        } else {
            "  no matches for filter"
        };
        frame.render_widget(
            Paragraph::new(Line::styled(msg, Style::default().fg(theme::text_dim()))),
            area,
        );
        return;
    }

    let width = area.width as usize;
    let height = area.height as usize;
    // Keep the selected row in view.
    let offset = st.selected.saturating_sub(height.saturating_sub(1));

    let name_w = name_col_width(width);
    let mut lines: Vec<Line> = Vec::new();
    for (row, &ei) in idxs.iter().enumerate().skip(offset).take(height) {
        let e = &st.entries[ei];
        let selected = row == st.selected;
        let name = e.display_name(&st.prefix);
        let line = match e {
            S3Entry::Folder { .. } => {
                let nm = trunc(&name, name_w);
                Line::from(vec![
                    Span::styled(" ▸ ", Style::default().fg(theme::aws_orange())),
                    Span::styled(
                        format!("{:<w$}", nm, w = name_w),
                        Style::default().fg(theme::accent()).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled("  <folder>", Style::default().fg(theme::text_dim())),
                ])
            }
            S3Entry::Object { size, last_modified, storage_class, .. } => {
                let nm = trunc(&name, name_w);
                Line::from(vec![
                    Span::raw("   "),
                    Span::styled(format!("{:<w$}", nm, w = name_w), Style::default().fg(crate::ui::theme::text_primary())),
                    Span::styled(format!("  {:>10}", fmt_object_size(*size)), Style::default().fg(theme::text_dim())),
                    Span::styled(
                        format!("  {:<16}", last_modified.clone().unwrap_or_default()),
                        Style::default().fg(theme::text_dim()),
                    ),
                    Span::styled(format!("  {}", storage_class), Style::default().fg(theme::text_dim())),
                ])
            }
            S3Entry::Version {
                version_id,
                is_latest,
                delete_marker,
                size,
                last_modified,
                storage_class,
                ..
            } => {
                let nm = trunc(&name, name_w);
                // Latest version reads like a normal row; older ones are
                // dimmed; delete markers are the "this key was deleted" tell.
                let name_style = if *delete_marker {
                    Style::default().fg(theme::error())
                } else if *is_latest {
                    Style::default().fg(crate::ui::theme::text_primary())
                } else {
                    Style::default().fg(theme::text_dim())
                };
                let vid_short: String = version_id.chars().take(8).collect();
                let tail = if *delete_marker {
                    format!("  ⌫ delete marker · v {}", vid_short)
                } else if *is_latest {
                    format!("  {} · v {} · latest", storage_class, vid_short)
                } else {
                    format!("  {} · v {}", storage_class, vid_short)
                };
                let tail_style = if *delete_marker {
                    Style::default().fg(theme::error())
                } else {
                    Style::default().fg(theme::text_dim())
                };
                Line::from(vec![
                    Span::raw("   "),
                    Span::styled(format!("{:<w$}", nm, w = name_w), name_style),
                    Span::styled(
                        format!(
                            "  {:>10}",
                            if *delete_marker {
                                "—".to_string()
                            } else {
                                fmt_object_size(*size)
                            }
                        ),
                        Style::default().fg(theme::text_dim()),
                    ),
                    Span::styled(
                        format!("  {:<16}", last_modified.clone().unwrap_or_default()),
                        Style::default().fg(theme::text_dim()),
                    ),
                    Span::styled(tail, tail_style),
                ])
            }
        };
        let line = if selected {
            line.patch_style(theme::selection_style(true))
        } else {
            line
        };
        lines.push(line);
    }
    frame.render_widget(Paragraph::new(lines), area);
}

fn render_status(st: &S3ObjectBrowserState, area: Rect, frame: &mut Frame) {
    if let Some(msg) = &st.message {
        frame.render_widget(
            Paragraph::new(Line::styled(
                format!(" {}", msg),
                Style::default().fg(theme::success()),
            )),
            area,
        );
        return;
    }
    let idxs = st.filtered_indices();
    let folders = idxs.iter().filter(|&&i| st.entries[i].is_folder()).count();
    let summary = if st.versions {
        let markers = idxs
            .iter()
            .filter(|&&i| {
                matches!(
                    st.entries[i],
                    crate::aws::services::s3::S3Entry::Version { delete_marker: true, .. }
                )
            })
            .count();
        let versions = idxs.len() - folders - markers;
        format!(
            " {} folders · {} versions · {} delete markers",
            folders, versions, markers
        )
    } else {
        format!(" {} folders · {} objects", folders, idxs.len() - folders)
    };
    let mut spans = vec![Span::styled(summary, Style::default().fg(theme::text_dim()))];
    if st.next_token.is_some() {
        spans.push(Span::styled("  · more (n)", Style::default().fg(theme::warning())));
    }
    if st.loading && !st.entries.is_empty() {
        spans.push(Span::styled("  · loading…", Style::default().fg(theme::text_dim())));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn render_help(area: Rect, frame: &mut Frame) {
    let w = 60u16.min(area.width.saturating_sub(4));
    let h = 24u16.min(area.height.saturating_sub(2));
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    let popup = Rect { x, y, width: w, height: h };
    frame.render_widget(Clear, popup);

    let block = theme::popup_block("Object browser — keys").title(
        Title::from(Span::styled(" ? / Esc close ", Style::default().fg(theme::text_dim())))
            .position(Position::Bottom)
            .alignment(Alignment::Center),
    );
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let item = |k: &str, d: &str| {
        Line::from(vec![
            Span::styled(format!("  {:<10}", k), Style::default().fg(theme::accent()).add_modifier(Modifier::BOLD)),
            Span::styled(d.to_string(), Style::default().fg(crate::ui::theme::text_primary())),
        ])
    };
    let lines = vec![
        Line::raw(""),
        item("j / k", "move up / down"),
        item("⏎ / l", "open folder · preview object"),
        item("h / ⌫", "up one level"),
        item("i", "toggle object metadata panel"),
        item("Tab", "focus panel → y copy value · Y copy all"),
        item("g / G", "top / bottom"),
        item("/", "filter the current listing"),
        item("f", "toggle recursive (all objects under prefix)"),
        item("V", "toggle version history (incl. delete markers)"),
        item("s / S", "cycle sort column / toggle direction"),
        item("Ctrl-d/u", "half-page down / up"),
        item("n", "load the next page"),
        item("d", "download object to working dir"),
        item("v", "view object text (head of large files)"),
        item("e", "open object in $EDITOR (≤ 1 MiB)"),
        item("p", "copy presigned GET URL (1h)"),
        item("y", "copy s3:// URI"),
        item("Z", "toggle full-width / split (show bucket list)"),
        item("Esc / q", "close"),
    ];
    frame.render_widget(Paragraph::new(lines), inner);
}

fn trunc(s: &str, w: usize) -> String {
    if s.chars().count() > w {
        let t: String = s.chars().take(w.saturating_sub(1)).collect();
        format!("{}…", t)
    } else {
        s.to_string()
    }
}

fn render_hr(area: Rect, frame: &mut Frame) {
    let line = "─".repeat(area.width as usize);
    frame.render_widget(
        Paragraph::new(Line::styled(line, Style::default().fg(theme::border_dim()))),
        area,
    );
}
