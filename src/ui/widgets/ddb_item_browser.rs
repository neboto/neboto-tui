use crate::aws::services::dynamodb::{av_cell, DdbIndex, SkOp};
use crate::ui::theme;
use aws_sdk_dynamodb::types::AttributeValue;
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
use std::collections::HashMap;

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

/// State for the full-screen DynamoDB item browser. Items are kept raw (the
/// renderer turns the selected one into pretty JSON on demand).
pub struct DdbBrowserState {
    pub visible: bool,
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
    pub items: Vec<HashMap<String, AttributeValue>>,
    pub columns: Vec<String>,
    pub selected_row: usize,
    pub last_key: Option<HashMap<String, AttributeValue>>,
    pub scanned: usize,
    pub loading: bool,
    pub error: Option<String>,
    pub show_detail: bool,
    pub detail_scroll: usize,
    pub show_help: bool,
}

impl DdbBrowserState {
    pub fn new() -> Self {
        Self {
            visible: false,
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
            columns: Vec::new(),
            selected_row: 0,
            last_key: None,
            scanned: 0,
            loading: false,
            error: None,
            show_detail: false,
            detail_scroll: 0,
            show_help: false,
        }
    }

    /// Open the browser for a table with its queryable indexes (base table + GSIs/LSIs).
    pub fn open(&mut self, table: String, indexes: Vec<DdbIndex>) {
        self.visible = true;
        self.table = table;
        self.indexes = indexes;
        self.index_sel = 0;
        self.query_mode = false;
        self.pk_value.clear();
        self.sk_op = SkOp::Eq;
        self.sk_value.clear();
        self.filter.clear();
        self.focus = BrowserFocus::Query;
        self.active_field = QueryField::Mode;
        self.items.clear();
        self.columns.clear();
        self.selected_row = 0;
        self.last_key = None;
        self.scanned = 0;
        self.loading = false;
        self.error = None;
        self.show_detail = false;
        self.detail_scroll = 0;
        self.show_help = false;
    }

    pub fn close(&mut self) {
        self.visible = false;
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

    /// Recompute the displayed columns: index keys first, then other attributes
    /// seen across the loaded items (capped), preserving first-seen order.
    pub fn recompute_columns(&mut self) {
        const MAX_COLS: usize = 6;
        let mut cols: Vec<String> = Vec::new();
        if let Some(idx) = self.current_index() {
            cols.push(idx.pk_name.clone());
            if let Some(sk) = &idx.sk_name {
                cols.push(sk.clone());
            }
        }
        for item in &self.items {
            for k in item.keys() {
                if !cols.contains(k) {
                    cols.push(k.clone());
                    if cols.len() >= MAX_COLS {
                        break;
                    }
                }
            }
            if cols.len() >= MAX_COLS {
                break;
            }
        }
        self.columns = cols;
    }
}

pub fn render_ddb_item_browser(app: &crate::app::App, area: Rect, frame: &mut Frame) {
    let st = &app.ddb_browser;
    if !st.visible {
        return;
    }

    let block = theme::popup_block(&format!("Items — {}", st.table)).title(
        Title::from(Span::styled(
            " Tab fields/results · ⏎ run/detail · n next page · Z width · ? help · Esc close ",
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
                Constraint::Percentage(50),
                Constraint::Length(1),
                Constraint::Min(0),
            ])
            .split(chunks[3]);
        render_results(st, split[0], frame);
        render_hr(split[1], frame);
        render_item_detail(st, split[2], frame);
    } else {
        render_results(st, chunks[3], frame);
    }

    render_status(st, chunks[4], frame);

    if st.show_help {
        render_help(area, frame);
    }
}

/// Centered examples/cheat-sheet popup (`?` toggles it).
fn render_help(area: Rect, frame: &mut Frame) {
    let w = 64u16.min(area.width.saturating_sub(4));
    let h = 20u16.min(area.height.saturating_sub(2));
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    let popup = Rect { x, y, width: w, height: h };
    frame.render_widget(Clear, popup);

    let block = theme::popup_block("Item browser — examples").title(
        Title::from(Span::styled(
            " ? / Esc close ",
            Style::default().fg(theme::text_dim()),
        ))
        .position(Position::Bottom)
        .alignment(Alignment::Center),
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
        head("Keys"),
        dim("Tab move field · ←/→ change choice · type to edit"),
        dim("⏎ run · ⏎ on a row = full item JSON · n next page"),
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

fn render_results(st: &DdbBrowserState, area: Rect, frame: &mut Frame) {
    if st.loading {
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

    let col_w = if st.columns.is_empty() {
        20
    } else {
        ((area.width as usize).saturating_sub(2) / st.columns.len()).clamp(8, 28)
    };
    let trunc = |s: &str, w: usize| -> String {
        if s.chars().count() > w {
            let t: String = s.chars().take(w.saturating_sub(1)).collect();
            format!("{}…", t)
        } else {
            format!("{:<w$}", s, w = w)
        }
    };

    let mut lines: Vec<Line> = Vec::new();
    // Header
    let header: String = st
        .columns
        .iter()
        .map(|c| trunc(c, col_w))
        .collect::<Vec<_>>()
        .join(" ");
    lines.push(Line::styled(
        format!(" {}", header),
        Style::default().fg(theme::accent()).add_modifier(Modifier::BOLD),
    ));

    let height = area.height as usize;
    let visible = height.saturating_sub(1);
    let offset = st.selected_row.saturating_sub(visible.saturating_sub(1));
    for (i, item) in st.items.iter().enumerate().skip(offset).take(visible) {
        let row: String = st
            .columns
            .iter()
            .map(|c| trunc(&item.get(c).map(av_cell).unwrap_or_default(), col_w))
            .collect::<Vec<_>>()
            .join(" ");
        let selected = i == st.selected_row && st.focus == BrowserFocus::Results;
        let style = if selected {
            theme::selection_style(true)
        } else {
            Style::default().fg(crate::ui::theme::text_primary())
        };
        lines.push(Line::styled(format!(" {}", row), style));
    }

    frame.render_widget(Paragraph::new(lines), area);
}

fn render_item_detail(st: &DdbBrowserState, area: Rect, frame: &mut Frame) {
    let json = st
        .items
        .get(st.selected_row)
        .map(|item| {
            let v: serde_json::Map<String, serde_json::Value> = item
                .iter()
                .map(|(k, av)| (k.clone(), crate::aws::services::dynamodb::av_to_json(av)))
                .collect();
            serde_json::to_string_pretty(&serde_json::Value::Object(v))
                .unwrap_or_else(|_| "{}".to_string())
        })
        .unwrap_or_default();

    let lines: Vec<Line> = json
        .lines()
        .skip(st.detail_scroll)
        .map(|l| Line::styled(format!(" {}", l), Style::default().fg(crate::ui::theme::text_primary())))
        .collect();
    frame.render_widget(Paragraph::new(lines), area);
}

fn render_status(st: &DdbBrowserState, area: Rect, frame: &mut Frame) {
    let more = if st.last_key.is_some() {
        Span::styled("  · more pages (n)", Style::default().fg(theme::warning()))
    } else {
        Span::raw("")
    };
    let line = Line::from(vec![
        Span::styled(
            format!(" {} items loaded", st.items.len()),
            Style::default().fg(theme::text_dim()),
        ),
        Span::styled(
            format!("  · {} scanned", st.scanned),
            Style::default().fg(theme::text_dim()),
        ),
        more,
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

fn render_hr(area: Rect, frame: &mut Frame) {
    let line = "─".repeat(area.width as usize);
    frame.render_widget(
        Paragraph::new(Line::styled(line, Style::default().fg(theme::border_dim()))),
        area,
    );
}
