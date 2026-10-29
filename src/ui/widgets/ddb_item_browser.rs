use crate::aws::services::dynamodb::query::{self, Plan, QueryError};
use crate::aws::services::dynamodb::{
    av_cell, av_cmp, av_to_json, av_type, estimate_rcu, item_ddb_json, DdbIndex, DdbItem, READ_BUDGET,
    READ_TARGET,
};
use crate::ui::theme;
use aws_sdk_dynamodb::types::AttributeValue;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
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
    /// What `y` copies: the scalar in full, or the container as JSON.
    pub copy: String,
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
    /// The table's ARN — what query history is keyed by.
    pub table_arn: String,
    /// `ItemCount` / `TableSizeBytes` from `DescribeTable` (updated by AWS
    /// about every six hours), for the plan line's cost estimate.
    pub table_items: i64,
    pub table_bytes: i64,
    /// The base table first, then GSIs, then LSIs.
    pub indexes: Vec<DdbIndex>,
    /// `Ctrl-N`: read this index instead of letting the compiler pick.
    pub pinned_index: Option<usize>,
    /// The query line and the cursor in it (a char index).
    pub query: String,
    pub cursor: usize,
    /// The plan that produced the rows on screen; `n` continues it, whatever
    /// the query line says now.
    pub ran: Option<Plan>,
    /// Recent queries for this table, newest first; `history_pos` is where
    /// `↑`/`↓` are in it, `history_draft` the unsent text they replaced.
    pub history: Vec<String>,
    pub history_pos: Option<usize>,
    pub history_draft: String,
    pub focus: BrowserFocus,
    /// `f`: a substring filter over the loaded rows (no API call).
    pub local_filter: String,
    pub local_editing: bool,
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
    /// Read capacity consumed by the reads behind the rows on screen.
    pub consumed: f64,
    pub loading: bool,
    pub error: Option<String>,
    pub show_detail: bool,
    pub detail_view: ItemView,
    /// The item view's top line, cursor line, linewise-selection anchor
    /// (`V`/`J`/`K`/drag) and the line a mouse drag started on.
    pub detail_scroll: usize,
    pub detail_cursor: usize,
    pub detail_anchor: Option<usize>,
    pub detail_drag: Option<usize>,
    /// `/` in the item view: the text, and whether it's still being typed.
    /// Kept across items, so the same attribute is found in the next one.
    pub item_search: String,
    pub item_searching: bool,
    pub show_help: bool,
}

impl DdbBrowserState {
    pub fn new() -> Self {
        Self {
            visible: false,
            generation: 0,
            table: String::new(),
            table_arn: String::new(),
            table_items: 0,
            table_bytes: 0,
            indexes: Vec::new(),
            pinned_index: None,
            query: String::new(),
            cursor: 0,
            ran: None,
            history: Vec::new(),
            history_pos: None,
            history_draft: String::new(),
            focus: BrowserFocus::Query,
            local_filter: String::new(),
            local_editing: false,
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
            consumed: 0.0,
            loading: false,
            error: None,
            show_detail: false,
            detail_view: ItemView::Typed,
            detail_scroll: 0,
            detail_cursor: 0,
            detail_anchor: None,
            detail_drag: None,
            item_search: String::new(),
            item_searching: false,
            show_help: false,
        }
    }

    /// Open the browser for a table with its queryable indexes (base table +
    /// GSIs/LSIs) and its recent queries.
    pub fn open(&mut self, table: &crate::aws::services::dynamodb::DdbTable) {
        let generation = self.generation.wrapping_add(1);
        let detail_view = self.detail_view;
        *self = Self::new();
        self.generation = generation;
        self.detail_view = detail_view;
        self.visible = true;
        self.table = table.name.clone();
        self.table_arn = table.arn.clone();
        self.table_items = table.item_count;
        self.table_bytes = table.size_bytes;
        self.indexes = table.queryable_indexes();
        self.history = crate::ddb_history::load(&table.arn);
    }

    pub fn close(&mut self) {
        self.visible = false;
        self.generation = self.generation.wrapping_add(1);
    }

    /// What the query line compiles to right now.
    pub fn plan(&self) -> Result<Plan, QueryError> {
        query::plan(&self.query, &self.indexes, self.pinned_index)
    }

    /// The index the rows on screen were read from (the base table before
    /// anything has run).
    pub fn current_index(&self) -> Option<&DdbIndex> {
        self.indexes.get(self.ran.as_ref().map_or(0, |p| p.index))
    }

    /// `Ctrl-N`: auto → each index in turn → auto.
    pub fn cycle_pinned_index(&mut self) {
        self.pinned_index = match self.pinned_index {
            None if !self.indexes.is_empty() => Some(0),
            Some(i) if i + 1 < self.indexes.len() => Some(i + 1),
            _ => None,
        };
    }

    /// Record the query line as run (history, saved to disk).
    pub fn remember_query(&mut self) {
        crate::ddb_history::remember(&mut self.history, &self.query);
        crate::ddb_history::save(&self.table_arn, &self.history);
        self.history_pos = None;
    }

    // ── Query-line editing ──

    fn byte_at(&self, char_idx: usize) -> usize {
        self.query.char_indices().nth(char_idx).map_or(self.query.len(), |(b, _)| b)
    }
    fn len_chars(&self) -> usize {
        self.query.chars().count()
    }
    fn set_query(&mut self, q: String) {
        self.query = q;
        self.cursor = self.len_chars();
    }
    pub fn insert_char(&mut self, c: char) {
        let at = self.byte_at(self.cursor);
        self.query.insert(at, c);
        self.cursor += 1;
        self.history_pos = None;
    }
    pub fn backspace(&mut self) {
        if self.cursor > 0 {
            let at = self.byte_at(self.cursor - 1);
            self.query.remove(at);
            self.cursor -= 1;
        }
    }
    pub fn delete(&mut self) {
        if self.cursor < self.len_chars() {
            let at = self.byte_at(self.cursor);
            self.query.remove(at);
        }
    }
    pub fn cursor_left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }
    pub fn cursor_right(&mut self) {
        self.cursor = (self.cursor + 1).min(self.len_chars());
    }
    pub fn cursor_home(&mut self) {
        self.cursor = 0;
    }
    pub fn cursor_end(&mut self) {
        self.cursor = self.len_chars();
    }
    /// `Ctrl-W`: delete the word before the cursor (and the spaces after it).
    pub fn delete_word(&mut self) {
        let chars: Vec<char> = self.query.chars().collect();
        let mut i = self.cursor;
        while i > 0 && chars[i - 1].is_whitespace() {
            i -= 1;
        }
        while i > 0 && !chars[i - 1].is_whitespace() {
            i -= 1;
        }
        let (from, to) = (self.byte_at(i), self.byte_at(self.cursor));
        self.query.replace_range(from..to, "");
        self.cursor = i;
    }
    /// `Ctrl-U`: delete everything before the cursor.
    pub fn delete_to_start(&mut self) {
        let to = self.byte_at(self.cursor);
        self.query.replace_range(..to, "");
        self.cursor = 0;
    }
    /// `Ctrl-K`: delete everything from the cursor on.
    pub fn delete_to_end(&mut self) {
        let from = self.byte_at(self.cursor);
        self.query.truncate(from);
    }
    /// `↑`: the next-older query in this table's history.
    pub fn history_prev(&mut self) {
        let next = self.history_pos.map_or(0, |p| p + 1);
        if next < self.history.len() {
            if self.history_pos.is_none() {
                self.history_draft = self.query.clone();
            }
            self.history_pos = Some(next);
            self.set_query(self.history[next].clone());
        }
    }
    /// `↓`: back toward the draft you were typing.
    pub fn history_next(&mut self) {
        match self.history_pos {
            Some(0) => {
                self.history_pos = None;
                let draft = std::mem::take(&mut self.history_draft);
                self.set_query(draft);
            }
            Some(p) => {
                self.history_pos = Some(p - 1);
                self.set_query(self.history[p - 1].clone());
            }
            None => {}
        }
    }

    /// Take in a page of results: replace the list, or append to it for `n`.
    /// `done` is false while a read is still fetching pages toward its target.
    pub fn apply_page(&mut self, page: Page, append: bool, done: bool) {
        if append {
            self.items.extend(page.items);
            self.scanned += page.scanned;
            self.consumed += page.consumed;
        } else {
            self.items = page.items;
            self.scanned = page.scanned;
            self.consumed = page.consumed;
            self.selected_row = 0;
            self.reset_detail();
        }
        self.last_key = page.last_key;
        self.loading = !done;
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

    /// Rebuild `order` from the local filter and `sort`, keeping the cursor
    /// on the same item.
    pub fn apply_sort(&mut self) {
        let keep = self.order.get(self.selected_row).copied();
        let needle = self.local_filter.to_lowercase();
        self.order = (0..self.items.len())
            .filter(|&i| needle.is_empty() || item_matches(&self.items[i], &needle))
            .collect();
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

    /// Back to the top of the item view, nothing selected (another item, or
    /// another view of it — line numbers don't carry over).
    pub fn reset_detail(&mut self) {
        self.detail_scroll = 0;
        self.detail_cursor = 0;
        self.detail_anchor = None;
        self.detail_drag = None;
    }

    /// Move the cursor by `delta` lines (saturating at both ends), scrolling
    /// a `height`-line window so it stays on screen.
    pub fn detail_move(&mut self, delta: isize, height: usize) {
        let last = self.detail_len().saturating_sub(1);
        self.detail_cursor = self.detail_cursor.saturating_add_signed(delta).min(last);
        self.detail_scroll = follow(self.detail_scroll, self.detail_cursor, height);
    }

    /// The wheel: scroll the window, dragging the cursor along only when it
    /// would fall off the screen.
    pub fn detail_scroll_by(&mut self, delta: isize, height: usize) {
        let len = self.detail_len();
        let max_top = len.saturating_sub(height);
        self.detail_scroll = self.detail_scroll.saturating_add_signed(delta).min(max_top);
        let bottom = (self.detail_scroll + height).min(len).saturating_sub(1);
        self.detail_cursor = self.detail_cursor.clamp(self.detail_scroll, bottom.max(self.detail_scroll));
    }

    /// The selected line range, inclusive (just the cursor line without one).
    pub fn detail_range(&self) -> (usize, usize) {
        let a = self.detail_anchor.unwrap_or(self.detail_cursor);
        (a.min(self.detail_cursor), a.max(self.detail_cursor))
    }

    /// The item view as copyable lines: `(line text, what copying just that
    /// line gives)`. A typed row copies its value (a map or list as JSON);
    /// a JSON line copies its value too, so `y` on `"email": "a@b.com",`
    /// gives `a@b.com`.
    fn detail_texts(&self) -> Vec<(String, String)> {
        match self.detail_view {
            ItemView::Typed => self
                .item_rows()
                .into_iter()
                .map(|r| (format!("{}{}: {}", "  ".repeat(r.depth), r.name, r.copy), r.copy))
                .collect(),
            _ => self
                .selected_item_json()
                .unwrap_or_default()
                .lines()
                .map(|l| (l.to_string(), json_line_value(l)))
                .collect(),
        }
    }

    /// Lines of the item view containing the `/` text (case-insensitive).
    pub fn item_matches(&self) -> Vec<usize> {
        let needle = self.item_search.to_lowercase();
        if needle.is_empty() {
            return Vec::new();
        }
        self.detail_texts()
            .iter()
            .enumerate()
            .filter(|(_, (line, _))| line.to_lowercase().contains(&needle))
            .map(|(i, _)| i)
            .collect()
    }

    /// Put the cursor on a match: `0` the first at or after the cursor (as
    /// you type), `1` the next, `-1` the previous — wrapping both ways.
    pub fn item_search_jump(&mut self, dir: isize, height: usize) {
        let m = self.item_matches();
        let c = self.detail_cursor;
        let target = match dir {
            0 => m.iter().find(|&&i| i >= c).or(m.first()),
            d if d > 0 => m.iter().find(|&&i| i > c).or(m.first()),
            _ => m.iter().rev().find(|&&i| i < c).or(m.last()),
        };
        if let Some(&t) = target {
            self.detail_anchor = None;
            self.detail_move(t as isize - c as isize, height);
        }
    }

    /// What `y` copies, with the toast's label: the selected lines (common
    /// indent removed), or the cursor line's value.
    pub fn detail_copy(&self) -> Option<(String, String)> {
        let texts = self.detail_texts();
        if texts.is_empty() {
            return None;
        }
        let (lo, hi) = self.detail_range();
        let hi = hi.min(texts.len() - 1);
        if self.detail_anchor.is_none() {
            return texts.get(lo).map(|(_, v)| (v.clone(), "value".to_string()));
        }
        let lines: Vec<&str> = texts[lo..=hi].iter().map(|(l, _)| l.as_str()).collect();
        let indent = lines
            .iter()
            .filter(|l| !l.trim().is_empty())
            .map(|l| l.len() - l.trim_start().len())
            .min()
            .unwrap_or(0);
        let text = lines.iter().map(|l| l.get(indent..).unwrap_or("")).collect::<Vec<_>>().join("\n");
        Some((text, format!("{} lines", hi - lo + 1)))
    }
}

/// A page as the browser takes it in (the event's payload, minus routing).
pub struct Page {
    pub items: Vec<DdbItem>,
    pub last_key: Option<DdbItem>,
    pub scanned: usize,
    pub consumed: f64,
}

/// Whether any attribute's cell text contains `needle` (already lowercased).
fn item_matches(item: &DdbItem, needle: &str) -> bool {
    item.values().any(|v| av_cell(v).to_lowercase().contains(needle))
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
    let copy = match av {
        AttributeValue::S(s) => s.clone(),
        AttributeValue::N(n) => n.clone(),
        AttributeValue::B(_) => av_cell(av),
        _ => av_to_json(av).to_string(),
    };
    let plural = |n: usize, one: &str, many: &str| format!("{} {}", n, if n == 1 { one } else { many });
    match av {
        AttributeValue::M(m) => {
            out.push(ItemRow { depth, name, ty, value: plural(m.len(), "key", "keys"), copy, container: true, key });
            let mut entries: Vec<_> = m.iter().collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            for (k, v) in entries {
                push_rows(out, depth + 1, k.clone(), v, false);
            }
        }
        AttributeValue::L(l) => {
            out.push(ItemRow { depth, name, ty, value: plural(l.len(), "item", "items"), copy, container: true, key });
            for (i, v) in l.iter().enumerate() {
                push_rows(out, depth + 1, format!("[{}]", i), v, false);
            }
        }
        _ => out.push(ItemRow { depth, name, ty, value: av_cell(av), copy, container: false, key }),
    }
}

/// The top line of a `height`-line window that keeps `cursor` on screen,
/// moving the window as little as possible from `top`.
fn follow(top: usize, cursor: usize, height: usize) -> usize {
    let height = height.max(1);
    if cursor < top {
        cursor
    } else if cursor >= top + height {
        cursor + 1 - height
    } else {
        top
    }
}

/// The value on one pretty-printed JSON line: `"email": "a@b.com",` →
/// `a@b.com`, `"n": 42` → `42`. A line that opens or closes a container
/// copies as itself (trimmed).
fn json_line_value(line: &str) -> String {
    let t = line.trim().trim_end_matches(',');
    let v = match t.strip_prefix('"').and_then(|r| r.find("\": ").map(|i| &r[i + 3..])) {
        Some(v) => v,
        None => t,
    };
    if v.ends_with('{') || v.ends_with('[') {
        return t.to_string();
    }
    match serde_json::from_str::<serde_json::Value>(v) {
        Ok(serde_json::Value::String(s)) => s,
        _ => v.to_string(),
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

    let hint = if st.focus == BrowserFocus::Query {
        " ⏎ run · ↑/↓ history · Ctrl-N index · Tab results · ? help · Esc close "
    } else if st.show_detail {
        " j/k · Ctrl-D/U · / search · V select · y copy · Y whole item · C CLI · v view · e editor · ⏎/Esc back "
    } else if st.loading {
        " Esc stop reading · ←/→ columns · z sort · / filter rows · ? help "
    } else {
        " Tab query · ⏎ item · / filter rows · ←/→ columns · z sort · n more · y copy · C CLI · ? help · Esc close "
    };
    let block = theme::popup_block(&format!("Items — {}", st.table)).title_bottom(
        Line::from(Span::styled(hint, Style::default().fg(theme::text_dim()))).centered(),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // query line
            Constraint::Length(1), // plan
            Constraint::Length(1), // index strip
            Constraint::Length(1), // rule
            Constraint::Min(0),    // results (+ optional detail)
            Constraint::Length(1), // status
        ])
        .split(inner);

    render_query_line(st, chunks[0], frame);
    render_plan_line(st, chunks[1], frame);
    render_index_strip(st, chunks[2], frame);
    render_hr(chunks[3], frame);
    let chunks = &chunks[1..];

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
        render_item_detail(st, split[2], &app.popup_hits, frame);
    } else {
        render_results(st, chunks[3], &app.popup_hits, frame);
    }

    render_status(st, chunks[4], frame);
    if st.focus == BrowserFocus::Query {
        // The terminal cursor sits in the query line, after the prompt.
        let before: String = st.query.chars().take(st.cursor).collect();
        let x = inner.x + PROMPT.width() as u16 + before.width() as u16;
        if x < inner.x + inner.width {
            frame.set_cursor_position((x, inner.y));
        }
    }

    if st.show_help {
        render_help(area, frame);
    }
}

/// Centered examples/cheat-sheet popup (`?` toggles it).
fn render_help(area: Rect, frame: &mut Frame) {
    let w = 64u16.min(area.width.saturating_sub(4));
    let h = 25u16.min(area.height.saturating_sub(2));
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
        head("Conditions — joined with and / or, grouped with ( )"),
        item("pk = \"USER#42\" and sk begins_with \"ORDER#\""),
        item("total > 100 and status in (pending, shipped)"),
        item("address.city = Perth · items[0].qty >= 2"),
        item("tags contains gift · coupon exists · not x exists"),
        item("createdAt between 1700000000 and 1800000000"),
        Line::raw(""),
        head("Query or Scan?"),
        dim("`=` on an index's partition key runs a Query on that index;"),
        dim("anything else Scans — the plan line says which, and why."),
        dim("Ctrl-N pins an index (Scan a sparse GSI, or force one)."),
        Line::raw(""),
        head("Values"),
        dim("\"007\" is a string · 7 a number · shipped a string · true/null"),
        dim("key attributes use their declared type · `odd-name` for names"),
        Line::raw(""),
        head("Results"),
        dim("reads stop at 50 matches or 5,000 items scanned · n for more"),
        dim("/ filters loaded rows (free) · ←/→ columns · z sort · ⏎ item"),
        dim("in an item, / finds a line · n/N next/previous"),
        dim("v typed / JSON / DynamoDB JSON · y copy · e $EDITOR"),
        dim("C copies the read as `aws dynamodb query|scan`, or get-item"),
    ];
    frame.render_widget(Paragraph::new(lines), inner);
}

const PROMPT: &str = " ❯ ";

fn render_query_line(st: &DdbBrowserState, area: Rect, frame: &mut Frame) {
    let focused = st.focus == BrowserFocus::Query;
    let prompt_style = if focused {
        Style::default().fg(theme::accent()).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme::text_dim())
    };
    let mut spans = vec![Span::styled(PROMPT, prompt_style)];
    if st.query.is_empty() {
        spans.push(Span::styled(
            "e.g. pk = \"USER#42\" and total > 100 · ? for examples",
            Style::default().fg(theme::text_dim()),
        ));
    } else {
        // Mark where a parse error points, so `status is 5` shows which word.
        let err_at = match st.plan() {
            Err(QueryError { pos: Some(p), .. }) => Some(p),
            _ => None,
        };
        let text = Style::default().fg(theme::text_primary());
        for (i, c) in st.query.chars().enumerate() {
            let style = if Some(i) == err_at {
                text.fg(theme::error()).add_modifier(Modifier::UNDERLINED)
            } else {
                text
            };
            spans.push(Span::styled(c.to_string(), style));
        }
        if err_at.is_some_and(|p| p >= st.query.chars().count()) {
            spans.push(Span::styled("␣", Style::default().fg(theme::error())));
        }
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// `≈ 1.2 GB` / `≈ 3.4M` — table stats are approximate (refreshed by AWS
/// about every six hours), so they read that way.
fn approx(n: f64, units: &[&str], step: f64) -> String {
    let mut v = n;
    let mut u = 0;
    while v >= step && u + 1 < units.len() {
        v /= step;
        u += 1;
    }
    if u == 0 {
        format!("{}{}", v as i64, units[0])
    } else {
        format!("{:.1}{}", v, units[u])
    }
}

fn render_plan_line(st: &DdbBrowserState, area: Rect, frame: &mut Frame) {
    let dim = Style::default().fg(theme::text_dim());
    let index_name = |i: usize| -> String {
        st.indexes
            .get(i)
            .map(|x| if x.name.is_empty() { "table".to_string() } else { x.name.clone() })
            .unwrap_or_default()
    };
    let spans: Vec<Span> = match st.plan() {
        Err(e) => vec![Span::styled(format!("   ✗ {}", e.message), Style::default().fg(theme::error()))],
        Ok(plan) if plan.query => {
            let mut v = vec![
                Span::styled("   ▶ Query ", Style::default().fg(theme::success()).add_modifier(Modifier::BOLD)),
                Span::styled(index_name(plan.index), Style::default().fg(theme::heading())),
                Span::styled("  key ", dim),
                Span::styled(plan.key_text.clone().unwrap_or_default(), Style::default().fg(theme::text_primary())),
            ];
            if let Some(f) = plan.filter_text {
                v.push(Span::styled("  filter ", dim));
                v.push(Span::styled(f, Style::default().fg(theme::text_primary())));
            }
            v
        }
        // No condition: a read stops at the first READ_TARGET items, so it's
        // cheap and shouldn't shout like a filtered Scan does.
        Ok(plan) if plan.filter.is_none() && st.pinned_index.is_none() => vec![Span::styled(
            format!("   Scan table · first {} items, n for more", READ_TARGET),
            dim,
        )],
        Ok(plan) => {
            let mut v = vec![
                Span::styled("   ⚠ Scan ", Style::default().fg(theme::warning()).add_modifier(Modifier::BOLD)),
                Span::styled(index_name(plan.index), Style::default().fg(theme::heading())),
            ];
            if let Some(f) = plan.filter_text.clone() {
                v.push(Span::styled("  filter ", dim));
                v.push(Span::styled(f, Style::default().fg(theme::text_primary())));
            }
            if let Some(why) = plan.why_scan.clone() {
                v.push(Span::styled(format!("  · {}", why), dim));
            }
            if st.table_items > 0 {
                v.push(Span::styled(
                    format!(
                        "  · table ≈ {} items, {}",
                        approx(st.table_items as f64, &["", "k", "M", "B"], 1000.0),
                        approx(st.table_bytes as f64, &[" B", " KB", " MB", " GB", " TB"], 1024.0)
                    ),
                    dim,
                ));
                // Unfiltered, a read stops at READ_TARGET items; filtered, it
                // may evaluate up to READ_BUDGET looking for matches.
                let reads = if plan.filter.is_some() { READ_BUDGET } else { READ_TARGET };
                if let Some(rcu) = estimate_rcu(reads, st.table_items, st.table_bytes) {
                    v.push(Span::styled(format!(" · ≤ {} RCU per read", approx(rcu, &["", "k", "M"], 1000.0)), dim));
                }
            }
            v
        }
    };
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// Every index with its key names — what you can write `=` on for a Query.
/// The one the plan reads is highlighted; a pinned one is marked.
fn render_index_strip(st: &DdbBrowserState, area: Rect, frame: &mut Frame) {
    let planned = st.plan().ok().map(|p| p.index);
    let dim = Style::default().fg(theme::text_dim());
    let mut spans = vec![Span::styled(
        if st.pinned_index.is_some() { "   index (pinned, Ctrl-N) " } else { "   index (auto, Ctrl-N) " },
        dim,
    )];
    for (i, idx) in st.indexes.iter().enumerate() {
        let name = if idx.name.is_empty() { "table" } else { idx.name.as_str() };
        let keys = match &idx.sk_name {
            Some(sk) => format!("{}, {}", idx.pk_name, sk),
            None => idx.pk_name.clone(),
        };
        let style = if Some(i) == planned {
            Style::default().fg(theme::accent()).add_modifier(Modifier::BOLD)
        } else {
            dim
        };
        let pin = if st.pinned_index == Some(i) { "📌" } else { "" };
        spans.push(Span::styled(format!(" {}{}({}) ", pin, name, keys), style));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
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
                    if st.ran.is_some() {
                        "  No matching items in what was read."
                    } else {
                        "  Type a condition and press ⏎ — or ⏎ on an empty line to read everything."
                    },
                    Style::default().fg(theme::text_dim()),
                ),
                Line::styled(
                    if st.last_key.is_some() {
                        "  The table has more — n reads further."
                    } else {
                        "  Press ? for query examples (key conditions, filters, GSIs)."
                    },
                    Style::default().fg(theme::text_dim()),
                ),
            ]),
            area,
        );
        return;
    }

    if st.order.is_empty() {
        // The local filter hid every row. Say that it only looked at what's
        // loaded — a miss here isn't a miss in the table.
        let more = if st.last_key.is_some() { " — n reads more, or Tab to query the table" } else { "" };
        frame.render_widget(
            Paragraph::new(vec![
                Line::styled(
                    format!("  No match for \"{}\" in the {} loaded rows{}.", st.local_filter, st.items.len(), more),
                    Style::default().fg(theme::text_dim()),
                ),
                Line::styled("  Esc clears the filter.", Style::default().fg(theme::text_dim())),
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

fn render_item_detail(
    st: &DdbBrowserState,
    area: Rect,
    hits: &std::cell::RefCell<crate::app::PopupHits>,
    frame: &mut Frame,
) {
    let header = Rect { height: 1.min(area.height), ..area };
    let body = Rect { y: area.y + header.height, height: area.height.saturating_sub(header.height), ..area };
    let height = body.height as usize;
    let len = st.detail_len();
    let scroll = follow(st.detail_scroll.min(len.saturating_sub(height)), st.detail_cursor, height);
    let (lo, hi) = st.detail_range();
    let selecting = st.detail_anchor.is_some();
    let pos = if len > height { format!(" · {}/{}", st.detail_cursor + 1, len) } else { String::new() };
    let sel = if selecting { format!(" · {} lines selected (y copies)", hi - lo + 1) } else { String::new() };
    let matches = st.item_matches();
    let search = if st.item_searching || !st.item_search.is_empty() {
        let at = matches.iter().position(|&m| m == st.detail_cursor).map_or("–".to_string(), |p| (p + 1).to_string());
        let caret = if st.item_searching { "▏" } else { "" };
        if matches.is_empty() && !st.item_search.is_empty() {
            format!(" · /{}{} no match", st.item_search, caret)
        } else {
            format!(" · /{}{} {}/{} (n/N)", st.item_search, caret, at, matches.len())
        }
    } else {
        String::new()
    };
    frame.render_widget(
        Paragraph::new(Line::styled(
            format!(" {} · v to switch{}{}{}", st.detail_view.label(), pos, sel, search),
            Style::default().fg(theme::text_dim()),
        )),
        header,
    );

    // Each line as spans, then the cursor / selection painted over the ones
    // on screen.
    let rows: Vec<Vec<Span>> = match st.detail_view {
        ItemView::Typed => {
            let rows = st.item_rows();
            let name_w = rows
                .iter()
                .map(|r| r.depth * 2 + r.name.width())
                .max()
                .unwrap_or(0)
                .clamp(12, 36);
            rows.iter()
                .skip(scroll)
                .take(height)
                .map(|r| {
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
                    vec![
                        Span::styled(name, name_style),
                        Span::styled(format!(" {:<5}", r.ty), Style::default().fg(theme::text_dim())),
                        Span::styled(r.value.clone(), value_style),
                    ]
                })
                .collect()
        }
        ItemView::Json | ItemView::DynamoJson => {
            let json = st.selected_item_json().unwrap_or_default();
            let all: Vec<&str> = json.lines().collect();
            crate::ui::syntax::highlight(crate::ui::syntax::Lang::Json, &all)
                .into_iter()
                .skip(scroll)
                .take(height)
                .collect()
        }
    };
    let focused = st.focus == BrowserFocus::Results;
    let lines: Vec<Line> = rows
        .into_iter()
        .enumerate()
        .map(|(i, spans)| {
            let n = scroll + i;
            let marked = if selecting { n >= lo && n <= hi } else { n == st.detail_cursor };
            let gutter = if matches.contains(&n) {
                Span::styled("› ", Style::default().fg(theme::accent()).add_modifier(Modifier::BOLD))
            } else {
                Span::raw("  ")
            };
            let mut l = vec![gutter];
            l.extend(spans);
            let line = Line::from(l);
            if marked && focused {
                // Pad to the full width so the bar reads as a row, not a word.
                let w = line.width();
                let mut line = line.patch_style(theme::selection_style(true));
                line.spans.push(Span::styled(
                    " ".repeat((area.width as usize).saturating_sub(w)),
                    theme::selection_style(true),
                ));
                line
            } else {
                line
            }
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), body);
    hits.borrow_mut().record_body(body, scroll, len);
}

fn render_status(st: &DdbBrowserState, area: Rect, frame: &mut Frame) {
    let dim = Style::default().fg(theme::text_dim());
    let thousands = |n: usize| {
        let s = n.to_string();
        let mut out = String::new();
        for (i, c) in s.chars().enumerate() {
            if i > 0 && (s.len() - i).is_multiple_of(3) {
                out.push(',');
            }
            out.push(c);
        }
        out
    };
    let mut spans = Vec::new();
    if st.local_editing || !st.local_filter.is_empty() {
        spans.push(Span::styled(" / ", Style::default().fg(theme::accent()).add_modifier(Modifier::BOLD)));
        spans.push(Span::styled(
            format!("{}{}", st.local_filter, if st.local_editing { "▏" } else { "" }),
            Style::default().fg(theme::text_primary()),
        ));
        spans.push(Span::styled(format!("  {} of {} rows ·", st.order.len(), st.items.len()), dim));
    }
    let filtered = st.ran.as_ref().is_some_and(|p| p.filter.is_some());
    spans.push(Span::styled(
        if filtered {
            format!(" {} matched · {} scanned", thousands(st.items.len()), thousands(st.scanned))
        } else {
            format!(" {} items", thousands(st.items.len()))
        },
        dim,
    ));
    if st.consumed > 0.0 {
        spans.push(Span::styled(format!(" · {:.1} RCU", st.consumed), dim));
    }
    if !st.columns.is_empty() {
        spans.push(Span::styled(format!(" · column {}/{}", st.col_cursor + 1, st.columns.len()), dim));
    }
    if let Some((col, desc)) = &st.sort {
        spans.push(Span::styled(format!(" · sorted by {} {}", col, if *desc { "▼" } else { "▲" }), dim));
    }
    let warn = Style::default().fg(theme::warning());
    if st.loading {
        spans.push(Span::styled(" · reading… (Esc stops)", warn));
    } else if st.last_key.is_some() {
        spans.push(Span::styled(" · more (n)", warn));
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
    use crate::aws::services::dynamodb::DdbTable;

    fn s(v: &str) -> AttributeValue {
        AttributeValue::S(v.into())
    }
    fn n(v: &str) -> AttributeValue {
        AttributeValue::N(v.into())
    }

    fn browser(items: Vec<DdbItem>) -> DdbBrowserState {
        let mut st = DdbBrowserState::new();
        st.open(&DdbTable::test_table("orders"));
        st.apply_page(page(items), false, true);
        st
    }

    fn page(items: Vec<DdbItem>) -> Page {
        Page { items, last_key: None, scanned: 0, consumed: 0.0 }
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
        st.open(&DdbTable::test_table("t"));
        assert_ne!(st.generation, g0);
        let g1 = st.generation;
        st.close();
        assert_ne!(st.generation, g1);
    }

    #[test]
    fn query_line_editing() {
        let mut st = browser(vec![]);
        for c in "pk = a and x".chars() {
            st.insert_char(c);
        }
        st.delete_word();
        assert_eq!(st.query, "pk = a and ");
        st.cursor_home();
        st.insert_char('(');
        st.cursor_end();
        st.backspace();
        assert_eq!(st.query, "(pk = a and");
        st.cursor_left();
        st.cursor_left();
        st.cursor_left();
        st.delete_to_end();
        assert_eq!(st.query, "(pk = a ");
        st.delete_to_start();
        assert_eq!((st.query.as_str(), st.cursor), ("", 0));
        // Multi-byte characters edit by character, not byte.
        st.insert_char('é');
        st.insert_char('x');
        st.cursor_left();
        st.backspace();
        assert_eq!(st.query, "x");
    }

    #[test]
    fn history_walks_back_and_restores_the_draft() {
        let mut st = browser(vec![]);
        st.history = vec!["newer".into(), "older".into()];
        st.query = "draft".into();
        st.history_prev();
        assert_eq!(st.query, "newer");
        st.history_prev();
        assert_eq!(st.query, "older");
        st.history_prev(); // past the end: stays
        assert_eq!(st.query, "older");
        st.history_next();
        st.history_next();
        assert_eq!(st.query, "draft");
        assert_eq!(st.cursor, 5);
    }

    #[test]
    fn local_filter_narrows_rows_without_dropping_items() {
        let mut st = browser(vec![
            item(&[("pk", s("a")), ("status", s("Shipped"))]),
            item(&[("pk", s("b")), ("status", s("pending"))]),
        ]);
        st.local_filter = "ship".into();
        st.apply_sort();
        assert_eq!(st.order.len(), 1);
        assert_eq!(st.items.len(), 2);
        assert_eq!(av_cell(&st.selected_item().unwrap()["pk"]), "a");
    }

    #[test]
    fn pages_accumulate_until_done() {
        let mut st = browser(vec![]);
        st.loading = true;
        let p = |pk: &str, scanned| Page {
            items: vec![item(&[("pk", s(pk))])],
            last_key: None,
            scanned,
            consumed: 2.5,
        };
        st.apply_page(p("a", 1000), false, false);
        assert!(st.loading);
        st.apply_page(p("b", 1000), true, true);
        assert!(!st.loading);
        assert_eq!((st.items.len(), st.scanned, st.consumed), (2, 2000, 5.0));
    }

    #[test]
    fn index_pin_cycles_through_auto() {
        let mut st = browser(vec![]);
        st.indexes.push(st.indexes[0].clone());
        st.cycle_pinned_index();
        assert_eq!(st.pinned_index, Some(0));
        st.cycle_pinned_index();
        assert_eq!(st.pinned_index, Some(1));
        st.cycle_pinned_index();
        assert_eq!(st.pinned_index, None);
    }

    #[test]
    fn item_view_cursor_moves_scrolls_and_copies() {
        let addr: DdbItem = item(&[("city", s("Perth")), ("zip", s("6000"))]);
        let mut st = browser(vec![item(&[
            ("pk", s("p")),
            ("sk", s("1")),
            ("addr", AttributeValue::M(addr)),
            ("email", s("a@b.com")),
        ])]);
        st.show_detail = true;
        // pk, sk, addr, city, zip, email — a 2-line window.
        st.detail_move(3, 2);
        assert_eq!((st.detail_cursor, st.detail_scroll), (3, 2));
        assert_eq!(st.detail_copy().unwrap().0, "Perth");
        st.detail_move(-1, 2);
        assert_eq!(st.detail_copy().unwrap().0, r#"{"city":"Perth","zip":"6000"}"#);
        st.detail_move(isize::MAX, 2);
        assert_eq!((st.detail_cursor, st.detail_scroll), (5, 4));
        st.detail_move(isize::MIN, 2);
        assert_eq!((st.detail_cursor, st.detail_scroll), (0, 0));
        // A selection copies its lines, indent kept relative.
        st.detail_cursor = 2;
        st.detail_anchor = Some(4);
        let (text, label) = st.detail_copy().unwrap();
        assert_eq!(text, "addr: {\"city\":\"Perth\",\"zip\":\"6000\"}\n  city: Perth\n  zip: 6000");
        assert_eq!(label, "3 lines");
    }

    #[test]
    fn json_view_lines_copy_their_values() {
        assert_eq!(json_line_value(r#"  "email": "a@b.com","#), "a@b.com");
        assert_eq!(json_line_value(r#"  "n": 42"#), "42");
        assert_eq!(json_line_value(r#"  "addr": {"#), r#""addr": {"#);
        assert_eq!(json_line_value("  },"), "}");
        assert_eq!(json_line_value(r#"    "x","#), "x");
    }

    #[test]
    fn wheel_scrolls_the_window_and_keeps_the_cursor_on_it() {
        let attrs: Vec<(String, AttributeValue)> = (0..30).map(|i| (format!("a{:02}", i), s("v"))).collect();
        let mut st = browser(vec![attrs.into_iter().collect()]);
        st.show_detail = true;
        st.detail_scroll_by(3, 10);
        assert_eq!((st.detail_scroll, st.detail_cursor), (3, 3));
        st.detail_scroll_by(100, 10);
        assert_eq!(st.detail_scroll, 20);
        st.detail_scroll_by(-100, 10);
        assert_eq!(st.detail_scroll, 0);
        assert!(st.detail_cursor < 10);
    }

    #[test]
    fn item_search_jumps_between_matching_lines_and_wraps() {
        let st_items = vec![item(&[
            ("pk", s("p")),
            ("sk", s("1")),
            ("email", s("a@b.com")),
            ("note", s("no")),
            ("alt_email", s("c@d.com")),
        ])];
        let mut st = browser(st_items);
        st.show_detail = true;
        // Typed rows: pk, sk, alt_email, email, note.
        st.item_search = "EMAIL".into();
        assert_eq!(st.item_matches(), [2, 3]);
        st.item_search_jump(0, 10);
        assert_eq!(st.detail_cursor, 2);
        st.item_search_jump(1, 10);
        assert_eq!(st.detail_cursor, 3);
        st.item_search_jump(1, 10);
        assert_eq!(st.detail_cursor, 2, "wraps forward");
        st.item_search_jump(-1, 10);
        assert_eq!(st.detail_cursor, 3, "wraps back");
        st.item_search = "zzz".into();
        st.item_search_jump(1, 10);
        assert_eq!(st.detail_cursor, 3, "no match leaves the cursor");
    }
}
