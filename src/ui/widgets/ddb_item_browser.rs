use crate::aws::services::dynamodb::{
    av_cell, av_cmp, av_to_json, av_type, item_ddb_json, DdbIndex, DdbItem, SkOp,
};
use crate::ui::theme;
use aws_sdk_dynamodb::types::AttributeValue;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Clear, Paragraph,
    },
    Frame,
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Which half of the browser has keyboard focus.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BrowserFocus {
    Query,
    Results,
}

/// The editable query-bar field that input is routed to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum QueryField {
    Mode,
    Index,
    Pk,
    SkOp,
    Sk,
    Filter,
}

/// How the selected item is drawn (`v` cycles).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ItemView {
    /// One row per attribute with its DynamoDB type, nesting indented.
    Typed,
    /// Plain JSON, as an application would see the item.
    Json,
    /// DynamoDB's typed JSON (`{"S": …}`), as `aws dynamodb get-item` prints it.
    DynamoJson,
}

impl ItemView {
    pub fn next(self) -> Self {
        match self {
            ItemView::Typed => ItemView::Json,
            ItemView::Json => ItemView::DynamoJson,
            ItemView::DynamoJson => ItemView::Typed,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            ItemView::Typed => "typed",
            ItemView::Json => "JSON",
            ItemView::DynamoJson => "DynamoDB JSON",
        }
    }
}

/// Column widths are measured from the data, within these bounds.
const COL_MIN: usize = 4;
const COL_MAX: usize = 40;

/// One row of the typed item view.
pub struct ItemRow {
    pub depth: usize,
    pub name: String,
    pub ty: &'static str,
    /// Scalar text, or a dim `3 keys` / `2 items` summary for a container.
    pub value: String,
    pub container: bool,
    pub key: bool,
}

/// State for the full-screen DynamoDB item browser. Items are kept raw (the
/// renderer turns the selected one into rows or JSON on demand).
pub struct DdbBrowserState {
    pub visible: bool,
    /// Bumped by every read and by open/close; a result stamped with an older
    /// one belongs to a query you've since replaced and is dropped.
    pub generation: u64,
    pub table: String,
    pub indexes: Vec<DdbIndex>,
    pub index_sel: usize,
    pub query_mode: bool, // false = Scan, true = Query
    pub pk_value: String,
    pub sk_op: SkOp,
    pub sk_value: String,
    pub filter: String,
    pub focus: BrowserFocus,
    pub active_field: QueryField,
    pub items: Vec<DdbItem>,
    /// Display order: row `i` shows `items[order[i]]` (load order unless sorted).
    pub order: Vec<usize>,
    pub columns: Vec<String>,
    pub col_widths: Vec<usize>,
    /// The leading key columns, which stay put while the rest scroll.
    pub pinned: usize,
    pub col_cursor: usize,
    /// Sorted column and whether descending.
    pub sort: Option<(String, bool)>,
    pub selected_row: usize,
    pub last_key: Option<DdbItem>,
    pub scanned: usize,
    pub loading: bool,
    pub error: Option<String>,
    pub show_detail: bool,
    pub detail_view: ItemView,
    pub detail_scroll: usize,
    pub show_help: bool,
}

impl DdbBrowserState {
    pub fn new() -> Self {
        Self {
            visible: false,
            generation: 0,
            table: String::new(),
            indexes: Vec::new(),
            index_sel: 0,
            query_mode: false,
            pk_value: String::new(),
            sk_op: SkOp::Eq,
            sk_value: String::new(),
            filter: String::new(),
            focus: BrowserFocus::Query,
            active_field: QueryField::Mode,
            items: Vec::new(),
            order: Vec::new(),
            columns: Vec::new(),
            col_widths: Vec::new(),
            pinned: 0,
            col_cursor: 0,
            sort: None,
            selected_row: 0,
            last_key: None,
            scanned: 0,
            loading: false,
            error: None,
            show_detail: false,
            detail_view: ItemView::Typed,
            detail_scroll: 0,
            show_help: false,
        }
    }

    /// Open the browser for a table with its queryable indexes (base table + GSIs/LSIs).
    pub fn open(&mut self, table: String, indexes: Vec<DdbIndex>) {
        let generation = self.generation.wrapping_add(1);
        let detail_view = self.detail_view;
        *self = Self::new();
        self.generation = generation;
        self.detail_view = detail_view;
        self.visible = true;
        self.table = table;
        self.indexes = indexes;
    }

    pub fn close(&mut self) {
        self.visible = false;
        self.generation = self.generation.wrapping_add(1);
    }

    pub fn current_index(&self) -> Option<&DdbIndex> {
        self.indexes.get(self.index_sel)
    }

    pub fn has_sort_key(&self) -> bool {
        self.current_index().and_then(|i| i.sk_name.as_ref()).is_some()
    }

    pub fn next_field(&mut self) {
        // Skip SK fields when the current index has no sort key.
        let has_sk = self.has_sort_key();
        self.active_field = match self.active_field {
            QueryField::Mode => QueryField::Index,
            QueryField::Index => {
                if self.query_mode {
                    QueryField::Pk
                } else {
                    QueryField::Filter
                }
            }
            QueryField::Pk if has_sk => QueryField::SkOp,
            QueryField::Pk => QueryField::Filter,
            QueryField::SkOp => QueryField::Sk,
            QueryField::Sk => QueryField::Filter,
            QueryField::Filter => QueryField::Mode,
        };
    }

    /// Take in a page of results: replace the list, or append to it for `n`.
    pub fn apply_page(&mut self, items: Vec<DdbItem>, last_key: Option<DdbItem>, scanned: usize, append: bool) {
        if append {
            self.items.extend(items);
            self.scanned += scanned;
        } else {
            self.items = items;
            self.scanned = scanned;
            self.selected_row = 0;
            self.detail_scroll = 0;
        }
        self.last_key = last_key;
        self.loading = false;
        self.error = None;
        self.recompute_columns();
        self.apply_sort();
    }

    /// The key attribute names, queried index first, then the base table's.
    fn key_names(&self) -> Vec<String> {
        let mut keys: Vec<String> = Vec::new();
        let idx = self.current_index().into_iter();
        for i in idx.chain(self.indexes.first()) {
            for k in std::iter::once(&i.pk_name).chain(i.sk_name.as_ref()) {
                if !k.is_empty() && !keys.contains(k) {
                    keys.push(k.clone());
                }
            }
        }
        keys
    }

    /// Recompute the displayed columns: key attributes first (pinned), then
    /// every other attribute, the most common first and alphabetically among
    /// equals — so the order is the same every time, and nothing is cut.
    /// Widths fit the header and the widest cell, within `COL_MIN..=COL_MAX`.
    pub fn recompute_columns(&mut self) {
        let cursor_name = self.columns.get(self.col_cursor).cloned();
        let keys: Vec<String> = self
            .key_names()
            .into_iter()
            .filter(|k| self.items.is_empty() || self.items.iter().any(|it| it.contains_key(k)))
            .collect();
        let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
        for item in &self.items {
            for k in item.keys() {
                if !keys.contains(k) {
                    *counts.entry(k.as_str()).or_default() += 1;
                }
            }
        }
        let mut rest: Vec<(&str, usize)> = counts.into_iter().collect();
        rest.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));

        self.pinned = keys.len();
        self.columns = keys;
        self.columns.extend(rest.into_iter().map(|(k, _)| k.to_string()));
        self.col_widths = self
            .columns
            .iter()
            .map(|c| {
                let cells = self
                    .items
                    .iter()
                    .filter_map(|it| it.get(c))
                    .map(|av| av_cell(av).width())
                    .max()
                    .unwrap_or(0);
                // +2 leaves room for the sort arrow on the header.
                (c.width() + 2).max(cells).clamp(COL_MIN, COL_MAX)
            })
            .collect();
        self.col_cursor = cursor_name
            .and_then(|n| self.columns.iter().position(|c| *c == n))
            .unwrap_or(0)
            .min(self.columns.len().saturating_sub(1));
    }

    /// Rebuild `order` from `sort`, keeping the cursor on the same item.
    pub fn apply_sort(&mut self) {
        let keep = self.order.get(self.selected_row).copied();
        self.order = (0..self.items.len()).collect();
        if let Some((col, desc)) = &self.sort {
            let items = &self.items;
            self.order.sort_by(|&a, &b| {
                match (items[a].get(col), items[b].get(col)) {
                    (Some(x), Some(y)) => {
                        let o = av_cmp(x, y);
                        if *desc { o.reverse() } else { o }
                    }
                    // Items without the attribute sink to the bottom both ways.
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (None, None) => std::cmp::Ordering::Equal,
                }
            });
        }
        self.selected_row = keep
            .and_then(|k| self.order.iter().position(|&i| i == k))
            .unwrap_or(self.selected_row)
            .min(self.order.len().saturating_sub(1));
    }

    /// `z`: the cursor column goes unsorted → ascending → descending → unsorted.
    pub fn cycle_sort(&mut self) {
        let Some(col) = self.columns.get(self.col_cursor).cloned() else {
            return;
        };
        self.sort = match &self.sort {
            Some((c, false)) if *c == col => Some((col, true)),
            Some((c, true)) if *c == col => None,
            _ => Some((col, false)),
        };
        self.apply_sort();
    }

    pub fn row_item(&self, row: usize) -> Option<&DdbItem> {
        self.order.get(row).and_then(|&i| self.items.get(i))
    }

    pub fn selected_item(&self) -> Option<&DdbItem> {
        self.row_item(self.selected_row)
    }

    /// The selected item as pretty JSON in the current view's flavour (the
    /// typed view copies as plain JSON). What `y` copies and `e` opens.
    pub fn selected_item_json(&self) -> Option<String> {
        let item = self.selected_item()?;
        let value = match self.detail_view {
            ItemView::DynamoJson => item_ddb_json(item),
            ItemView::Typed | ItemView::Json => item_json(item, &self.key_names()),
        };
        serde_json::to_string_pretty(&value).ok()
    }

    /// Rows of the typed view for the selected item, keys first.
    pub fn item_rows(&self) -> Vec<ItemRow> {
        let Some(item) = self.selected_item() else {
            return Vec::new();
        };
        let keys = self.key_names();
        let mut out = Vec::new();
        for (name, av) in ordered_attrs(item, &keys) {
            push_rows(&mut out, 0, name.clone(), av, keys.contains(name));
        }
        out
    }

    /// How many lines the item view has, for bounding its scroll.
    pub fn detail_len(&self) -> usize {
        match self.detail_view {
            ItemView::Typed => self.item_rows().len(),
            _ => self.selected_item_json().map(|j| j.lines().count()).unwrap_or(0),
        }
    }
}

/// An item's attributes, the given keys first, then alphabetical.
fn ordered_attrs<'a>(item: &'a DdbItem, keys: &[String]) -> Vec<(&'a String, &'a AttributeValue)> {
    let mut attrs: Vec<_> = item.iter().collect();
    let rank = |k: &String| keys.iter().position(|x| x == k).unwrap_or(usize::MAX);
    attrs.sort_by(|a, b| rank(a.0).cmp(&rank(b.0)).then(a.0.cmp(b.0)));
    attrs
}

/// Plain JSON for an item, in the same order as the typed view.
fn item_json(item: &DdbItem, keys: &[String]) -> serde_json::Value {
    serde_json::Value::Object(
        ordered_attrs(item, keys)
            .into_iter()
            .map(|(k, v)| (k.clone(), av_to_json(v)))
            .collect(),
    )
}

fn push_rows(out: &mut Vec<ItemRow>, depth: usize, name: String, av: &AttributeValue, key: bool) {
    let ty = av_type(av);
    let plural = |n: usize, one: &str, many: &str| format!("{} {}", n, if n == 1 { one } else { many });
    match av {
        AttributeValue::M(m) => {
            out.push(ItemRow { depth, name, ty, value: plural(m.len(), "key", "keys"), container: true, key });
            let mut entries: Vec<_> = m.iter().collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            for (k, v) in entries {
                push_rows(out, depth + 1, k.clone(), v, false);
            }
        }
        AttributeValue::L(l) => {
            out.push(ItemRow { depth, name, ty, value: plural(l.len(), "item", "items"), container: true, key });
            for (i, v) in l.iter().enumerate() {
                push_rows(out, depth + 1, format!("[{}]", i), v, false);
            }
        }
        _ => out.push(ItemRow { depth, name, ty, value: av_cell(av), container: false, key }),
    }
}

/// `s` cut to `w` display columns (with `…` when cut), padded to exactly `w`.
fn fit(s: &str, w: usize) -> String {
    let s: String = s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    if s.width() <= w {
        return format!("{}{}", s, " ".repeat(w - s.width()));
    }
    let mut out = String::new();
    let mut used = 0;
    for c in s.chars() {
        let cw = c.width().unwrap_or(0);
        if used + cw + 1 > w {
            break;
        }
        out.push(c);
        used += cw;
    }
    out.push('…');
    used += 1;
    out.push_str(&" ".repeat(w.saturating_sub(used)));
    out
}

/// Which columns fit in `width`: the pinned key columns, then a run of the
/// others ending at (or starting from) the cursor. Stateless, so the
/// renderer can work it out from `&App` alone.
fn visible_columns(st: &DdbBrowserState, width: usize) -> Vec<usize> {
    // Each column is followed by a one-space gap, or the two-cell `│ ` rule
    // after the last pinned one.
    let w = |i: usize| st.col_widths.get(i).copied().unwrap_or(COL_MIN) + if i + 1 == st.pinned { 2 } else { 1 };
    let pinned: Vec<usize> = (0..st.pinned.min(st.columns.len())).collect();
    let pinned_w: usize = pinned.iter().map(|&i| w(i)).sum();
    let budget = width.saturating_sub(pinned_w);
    let n = st.columns.len();
    // Start far enough right that the cursor column is on screen.
    let mut start = st.pinned.min(n);
    if st.col_cursor >= st.pinned {
        let mut used = 0;
        start = st.col_cursor;
        for i in (st.pinned..=st.col_cursor).rev() {
            if used + w(i) > budget && i != st.col_cursor {
                break;
            }
            used += w(i);
            start = i;
        }
    }
    let mut out = pinned;
    let mut used = 0;
    for i in start..n {
        if used + w(i) > budget && out.len() > st.pinned {
            break;
        }
        used += w(i);
        out.push(i);
    }
    out
}

pub fn render_ddb_item_browser(app: &crate::app::App, area: Rect, frame: &mut Frame) {
    let st = &app.ddb_browser;
    if !st.visible {
        return;
    }
    app.popup_hits.borrow_mut().record_area(area);

    let hint = if st.show_detail {
        " j/k scroll · v typed/JSON/DynamoDB JSON · y copy · e editor · ⏎/Esc back to items "
    } else {
        " Tab query · ⏎ item · ←/→ columns · z sort · n next page · y copy · ? help · Esc close "
    };
    let block = theme::popup_block(&format!("Items — {}", st.table)).title_bottom(
        Line::from(Span::styled(hint, Style::default().fg(theme::text_dim()))).centered(),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // query bar line 1
            Constraint::Length(1), // query bar line 2
            Constraint::Length(1), // rule
            Constraint::Min(0),    // results (+ optional detail)
            Constraint::Length(1), // status
        ])
        .split(inner);

    render_query_bar(st, chunks[0], chunks[1], frame);
    render_hr(chunks[2], frame);

    if st.show_detail {
        let split = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Percentage(40),
                Constraint::Length(1),
                Constraint::Min(0),
            ])
            .split(chunks[3]);
        render_results(st, split[0], &app.popup_hits, frame);
        render_hr(split[1], frame);
        render_item_detail(st, split[2], frame);
    } else {
        render_results(st, chunks[3], &app.popup_hits, frame);
    }

    render_status(st, chunks[4], frame);

    if st.show_help {
        render_help(area, frame);
    }
}

/// Centered examples/cheat-sheet popup (`?` toggles it).
fn render_help(area: Rect, frame: &mut Frame) {
    let w = 64u16.min(area.width.saturating_sub(4));
    let h = 24u16.min(area.height.saturating_sub(2));
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    let popup = Rect { x, y, width: w, height: h };
    frame.render_widget(Clear, popup);

    let block = theme::popup_block("Item browser — examples").title_bottom(
        Line::from(Span::styled(
            " ? / Esc close ",
            Style::default().fg(theme::text_dim()),
        ))
        .centered(),
    );
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let head = |s: &str| {
        Line::styled(
            format!("  {}", s),
            Style::default().fg(theme::accent()).add_modifier(Modifier::BOLD),
        )
    };
    let item = |s: &str| Line::styled(format!("    {}", s), Style::default().fg(crate::ui::theme::text_primary()));
    let dim = |s: &str| Line::styled(format!("    {}", s), Style::default().fg(theme::text_dim()));

    let lines = vec![
        Line::raw(""),
        head("Scan (default)"),
        dim("press ⏎ to list items · n loads the next page"),
        Line::raw(""),
        head("Filter (Scan or Query)"),
        item("status=active        attribute equals value"),
        item("email~@example.com   attribute contains text"),
        Line::raw(""),
        head("Query (by key) — set Mode to Query"),
        dim("set PK to the partition-key value, then optionally:"),
        item("Op = begins_with   SK = 2024-     prefix match"),
        item("Op = >             SK = 100       range on sort key"),
        dim("switch Index to a GSI/LSI to query its keys"),
        Line::raw(""),
        head("Results"),
        dim("←/→ move between columns (key columns stay put)"),
        dim("z sorts the loaded rows by the column: ▲ → ▼ → off"),
        dim("⏎ opens the item · v typed / JSON / DynamoDB JSON"),
        dim("y copies the item · e opens it in $EDITOR"),
    ];
    frame.render_widget(Paragraph::new(lines), inner);
}

fn field_span(label: &str, value: String, active: bool) -> Vec<Span<'static>> {
    let val_style = if active {
        Style::default()
            .fg(Color::Black)
            .bg(theme::aws_orange())
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(crate::ui::theme::text_primary())
    };
    vec![
        Span::styled(format!("{}: ", label), Style::default().fg(theme::text_dim())),
        Span::styled(format!(" {} ", value), val_style),
        Span::raw("   "),
    ]
}

fn render_query_bar(st: &DdbBrowserState, line1: Rect, line2: Rect, frame: &mut Frame) {
    let q = st.focus == BrowserFocus::Query;
    let mode = if st.query_mode { "Query" } else { "Scan" };
    let index = st
        .current_index()
        .map(|i| if i.name.is_empty() { "(table)".to_string() } else { i.name.clone() })
        .unwrap_or_else(|| "(table)".to_string());

    let mut spans1 = vec![Span::raw(" ")];
    spans1.extend(field_span("Mode", mode.to_string(), q && st.active_field == QueryField::Mode));
    spans1.extend(field_span("Index", index, q && st.active_field == QueryField::Index));
    frame.render_widget(Paragraph::new(Line::from(spans1)), line1);

    let mut spans2 = vec![Span::raw(" ")];
    if st.query_mode {
        let pk_label = st.current_index().map(|i| i.pk_name.clone()).unwrap_or_default();
        spans2.extend(field_span(
            &format!("PK[{}]", pk_label),
            if st.pk_value.is_empty() { "—".to_string() } else { st.pk_value.clone() },
            q && st.active_field == QueryField::Pk,
        ));
        if st.has_sort_key() {
            spans2.extend(field_span("Op", st.sk_op.label().to_string(), q && st.active_field == QueryField::SkOp));
            spans2.extend(field_span(
                "SK",
                if st.sk_value.is_empty() { "—".to_string() } else { st.sk_value.clone() },
                q && st.active_field == QueryField::Sk,
            ));
        }
    }
    spans2.extend(field_span(
        "Filter",
        if st.filter.is_empty() { "attr=val / attr~val".to_string() } else { st.filter.clone() },
        q && st.active_field == QueryField::Filter,
    ));
    frame.render_widget(Paragraph::new(Line::from(spans2)), line2);
}

fn render_results(st: &DdbBrowserState, area: Rect, hits: &std::cell::RefCell<crate::app::PopupHits>, frame: &mut Frame) {
    if st.loading && st.items.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::styled("  Loading items…", Style::default().fg(theme::text_dim()))),
            area,
        );
        return;
    }
    if let Some(err) = &st.error {
        frame.render_widget(
            Paragraph::new(Line::styled(format!("  {}", err), Style::default().fg(theme::error()))),
            area,
        );
        return;
    }
    if st.items.is_empty() {
        frame.render_widget(
            Paragraph::new(vec![
                Line::styled(
                    "  No items. Tab to the query bar, ⏎ to run a Scan or Query.",
                    Style::default().fg(theme::text_dim()),
                ),
                Line::styled(
                    "  Press ? for query examples (filters, key conditions, GSIs).",
                    Style::default().fg(theme::text_dim()),
                ),
            ]),
            area,
        );
        return;
    }

    let focused = st.focus == BrowserFocus::Results;
    let cols = visible_columns(st, (area.width as usize).saturating_sub(1));
    let sep = |i: usize| -> Span<'static> {
        // A rule after the last pinned key column marks where scrolling starts.
        if i + 1 == st.pinned && st.pinned < st.columns.len() {
            Span::styled("│ ", Style::default().fg(theme::border_dim()))
        } else {
            Span::raw(" ")
        }
    };

    let mut lines: Vec<Line> = Vec::new();
    let mut header = vec![Span::raw(" ")];
    for &i in &cols {
        let name = &st.columns[i];
        let arrow = match &st.sort {
            Some((c, desc)) if c == name => if *desc { " ▼" } else { " ▲" },
            _ => "",
        };
        let label = fit(&format!("{}{}", name, arrow), st.col_widths[i]);
        let mut style = Style::default().fg(theme::accent()).add_modifier(Modifier::BOLD);
        if focused && i == st.col_cursor {
            style = style.add_modifier(Modifier::REVERSED);
        }
        header.push(Span::styled(label, style));
        header.push(sep(i));
    }
    lines.push(Line::from(header));

    let height = area.height as usize;
    let visible = height.saturating_sub(1);
    let offset = st.selected_row.saturating_sub(visible.saturating_sub(1));
    for row in offset..st.order.len().min(offset + visible) {
        let Some(item) = st.row_item(row) else { break };
        let selected = row == st.selected_row && focused;
        let base = if selected {
            theme::selection_style(true)
        } else {
            Style::default().fg(crate::ui::theme::text_primary())
        };
        let mut spans = vec![Span::styled(" ", base)];
        for &i in &cols {
            let cell = match item.get(&st.columns[i]) {
                Some(av) => fit(&av_cell(av), st.col_widths[i]),
                None => fit("", st.col_widths[i]),
            };
            let mut style = base;
            if i < st.pinned && !selected {
                style = style.fg(theme::heading());
            }
            if selected && i == st.col_cursor {
                style = style.add_modifier(Modifier::UNDERLINED);
            }
            spans.push(Span::styled(cell, style));
            spans.push(sep(i).patch_style(base));
        }
        lines.push(Line::from(spans));
    }

    frame.render_widget(Paragraph::new(lines), area);
    // Data rows start under the column-header row.
    let rows = Rect { y: area.y + 1, height: area.height.saturating_sub(1), ..area };
    hits.borrow_mut().record_rows(rows, offset, st.order.len());
}

fn render_item_detail(st: &DdbBrowserState, area: Rect, frame: &mut Frame) {
    let height = area.height as usize;
    let scroll = st.detail_scroll.min(st.detail_len().saturating_sub(height.max(1)));
    let mut lines: Vec<Line> = vec![Line::styled(
        format!(" {} · v to switch", st.detail_view.label()),
        Style::default().fg(theme::text_dim()),
    )];
    let body = height.saturating_sub(1);
    match st.detail_view {
        ItemView::Typed => {
            let rows = st.item_rows();
            let name_w = rows
                .iter()
                .map(|r| r.depth * 2 + r.name.width())
                .max()
                .unwrap_or(0)
                .clamp(12, 36);
            for r in rows.iter().skip(scroll).take(body) {
                let indent = "  ".repeat(r.depth);
                let name = fit(&format!("{}{}", indent, r.name), name_w);
                let name_style = if r.key {
                    Style::default().fg(theme::heading()).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme::accent())
                };
                let value_style = if r.container {
                    Style::default().fg(theme::text_dim())
                } else {
                    Style::default().fg(theme::text_primary())
                };
                lines.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(name, name_style),
                    Span::styled(format!(" {:<5}", r.ty), Style::default().fg(theme::text_muted())),
                    Span::styled(r.value.clone(), value_style),
                ]));
            }
        }
        ItemView::Json | ItemView::DynamoJson => {
            let json = st.selected_item_json().unwrap_or_default();
            let all: Vec<&str> = json.lines().collect();
            for spans in crate::ui::syntax::highlight(crate::ui::syntax::Lang::Json, &all)
                .into_iter()
                .skip(scroll)
                .take(body)
            {
                let mut l = vec![Span::raw("  ")];
                l.extend(spans);
                lines.push(Line::from(l));
            }
        }
    }
    frame.render_widget(Paragraph::new(lines), area);
}

fn render_status(st: &DdbBrowserState, area: Rect, frame: &mut Frame) {
    let dim = Style::default().fg(theme::text_dim());
    let mut spans = vec![
        Span::styled(format!(" {} items loaded", st.items.len()), dim),
        Span::styled(format!("  · {} scanned", st.scanned), dim),
    ];
    if !st.columns.is_empty() {
        spans.push(Span::styled(
            format!("  · column {}/{}", st.col_cursor + 1, st.columns.len()),
            dim,
        ));
    }
    if let Some((col, desc)) = &st.sort {
        spans.push(Span::styled(
            format!("  · sorted by {} {}", col, if *desc { "▼" } else { "▲" }),
            dim,
        ));
    }
    if st.loading {
        spans.push(Span::styled("  · loading…", Style::default().fg(theme::warning())));
    } else if st.last_key.is_some() {
        spans.push(Span::styled("  · more pages (n)", Style::default().fg(theme::warning())));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn render_hr(area: Rect, frame: &mut Frame) {
    let line = "─".repeat(area.width as usize);
    frame.render_widget(
        Paragraph::new(Line::styled(line, Style::default().fg(theme::border_dim()))),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &str) -> AttributeValue {
        AttributeValue::S(v.into())
    }
    fn n(v: &str) -> AttributeValue {
        AttributeValue::N(v.into())
    }

    fn browser(items: Vec<DdbItem>) -> DdbBrowserState {
        let mut st = DdbBrowserState::new();
        st.open(
            "orders".into(),
            vec![DdbIndex {
                name: String::new(),
                keys: String::new(),
                projection: String::new(),
                status: String::new(),
                read_capacity: None,
                write_capacity: None,
                pk_name: "pk".into(),
                pk_type: "S".into(),
                sk_name: Some("sk".into()),
                sk_type: Some("S".into()),
            }],
        );
        st.apply_page(items, None, 0, false);
        st
    }

    fn item(pairs: &[(&str, AttributeValue)]) -> DdbItem {
        pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
    }

    #[test]
    fn columns_are_keys_then_most_common_then_alphabetical() {
        let st = browser(vec![
            item(&[("pk", s("a")), ("sk", s("1")), ("zeta", s("x")), ("beta", s("y")), ("rare", s("r"))]),
            item(&[("pk", s("b")), ("sk", s("2")), ("zeta", s("x")), ("beta", s("y"))]),
        ]);
        assert_eq!(st.columns, ["pk", "sk", "beta", "zeta", "rare"]);
        assert_eq!(st.pinned, 2);
    }

    #[test]
    fn no_column_is_dropped() {
        let attrs: Vec<(String, AttributeValue)> = (0..20).map(|i| (format!("a{:02}", i), s("v"))).collect();
        let mut it: DdbItem = attrs.into_iter().collect();
        it.insert("pk".into(), s("p"));
        let st = browser(vec![it]);
        assert_eq!(st.columns.len(), 21);
    }

    #[test]
    fn widths_fit_the_data_within_bounds() {
        let long = "x".repeat(200);
        let st = browser(vec![item(&[("pk", s("ab")), ("note", s(&long))])]);
        let w = |c: &str| st.col_widths[st.columns.iter().position(|x| x == c).unwrap()];
        assert_eq!(w("pk"), COL_MIN);
        assert_eq!(w("note"), COL_MAX);
    }

    #[test]
    fn sort_is_numeric_keeps_missing_last_and_cycles_off() {
        let mut st = browser(vec![
            item(&[("pk", s("a")), ("total", n("100"))]),
            item(&[("pk", s("b"))]),
            item(&[("pk", s("c")), ("total", n("9"))]),
        ]);
        st.col_cursor = st.columns.iter().position(|c| c == "total").unwrap();
        let pks = |st: &DdbBrowserState| -> Vec<String> {
            (0..st.order.len()).map(|r| av_cell(&st.row_item(r).unwrap()["pk"])).collect()
        };
        st.cycle_sort();
        assert_eq!(pks(&st), ["c", "a", "b"]);
        st.cycle_sort();
        assert_eq!(pks(&st), ["a", "c", "b"]);
        st.cycle_sort();
        assert_eq!(st.sort, None);
        assert_eq!(pks(&st), ["a", "b", "c"]);
    }

    #[test]
    fn sort_keeps_the_cursor_on_the_same_item() {
        let mut st = browser(vec![
            item(&[("pk", s("b"))]),
            item(&[("pk", s("a"))]),
        ]);
        st.selected_row = 0; // "b"
        st.cycle_sort();
        assert_eq!(av_cell(&st.selected_item().unwrap()["pk"]), "b");
        assert_eq!(st.selected_row, 1);
    }

    #[test]
    fn typed_rows_put_keys_first_and_indent_nesting() {
        let addr: DdbItem = item(&[("city", s("Perth")), ("zip", s("6000"))]);
        let st = browser(vec![item(&[
            ("addr", AttributeValue::M(addr)),
            ("sk", s("1")),
            ("pk", s("p")),
        ])]);
        let rows = st.item_rows();
        let names: Vec<(usize, &str, &str)> = rows.iter().map(|r| (r.depth, r.name.as_str(), r.ty)).collect();
        assert_eq!(
            names,
            [(0, "pk", "S"), (0, "sk", "S"), (0, "addr", "M"), (1, "city", "S"), (1, "zip", "S")]
        );
        assert_eq!(rows[2].value, "2 keys");
    }

    #[test]
    fn json_views_differ_in_typing() {
        let mut st = browser(vec![item(&[("pk", s("p")), ("n", n("42"))])]);
        st.detail_view = ItemView::Json;
        assert!(st.selected_item_json().unwrap().contains("\"n\": 42"));
        st.detail_view = ItemView::DynamoJson;
        assert!(st.selected_item_json().unwrap().contains("\"N\": \"42\""));
    }

    #[test]
    fn visible_columns_pin_keys_and_follow_the_cursor() {
        let attrs: Vec<(String, AttributeValue)> = (0..10).map(|i| (format!("attr{}", i), s("value"))).collect();
        let mut it: DdbItem = attrs.into_iter().collect();
        it.insert("pk".into(), s("p"));
        let mut st = browser(vec![it]);
        st.col_cursor = st.columns.len() - 1;
        let cols = visible_columns(&st, 30);
        assert_eq!(cols[0], 0, "key column stays pinned");
        assert!(cols.contains(&st.col_cursor), "cursor column is on screen");
        assert!(cols.len() < st.columns.len());
    }

    #[test]
    fn fit_truncates_by_display_width() {
        assert_eq!(fit("abc", 5), "abc  ");
        assert_eq!(fit("abcdef", 4), "abc…");
        assert_eq!(fit("日本語", 4).width(), 4);
    }

    #[test]
    fn open_and_close_bump_the_generation() {
        let mut st = DdbBrowserState::new();
        let g0 = st.generation;
        st.open("t".into(), vec![]);
        assert_ne!(st.generation, g0);
        let g1 = st.generation;
        st.close();
        assert_ne!(st.generation, g1);
    }
}
