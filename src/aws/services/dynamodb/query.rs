//! The item browser's one-line query language, and the compiler that turns it
//! into a DynamoDB read.
//!
//! ```text
//! pk = "USER#42" and sk begins_with "ORDER#" and total > 100
//! status in (pending, shipped) or not coupon exists
//! address.city = Perth and tags contains gift
//! ```
//!
//! You write conditions; the compiler decides how to read. If the top-level
//! `and` pins an index's partition key with `=`, it's a **Query** on that
//! index (the base table first, then GSIs/LSIs), with at most one sort-key
//! condition as part of the key; everything else becomes the
//! `FilterExpression`. Otherwise it's a **Scan** with the whole thing as the
//! filter, and the plan says so — a Scan is the expensive one.
//!
//! Values: quoted is always a string (`"007"`); bare is a number when it looks
//! like one, `true`/`false`/`null` are what they say, anything else is a
//! string (`status = shipped`). A key attribute's declared type wins, so
//! `year = 2024` on an `S` partition key sends `"2024"`.

use super::DdbIndex;
use aws_sdk_dynamodb::types::AttributeValue;
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub struct QueryError {
    pub message: String,
    /// Character offset in the input the error points at, if any.
    pub pos: Option<usize>,
}

impl QueryError {
    fn at(pos: usize, message: impl Into<String>) -> Self {
        Self { message: message.into(), pos: Some(pos) }
    }
    fn plain(message: impl Into<String>) -> Self {
        Self { message: message.into(), pos: None }
    }
}

// ── Lexer ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    /// Unquoted text: a path, a keyword or a bare value.
    Word(String),
    /// `"…"` or `'…'`.
    Str(String),
    /// `` `odd name` `` — an attribute name taken literally.
    Name(String),
    Op(CmpOp),
    LParen,
    RParen,
    Comma,
}

fn lex(input: &str) -> Result<Vec<(usize, Tok)>, QueryError> {
    let chars: Vec<char> = input.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let start = i;
        match c {
            c if c.is_whitespace() => i += 1,
            '(' => {
                out.push((start, Tok::LParen));
                i += 1;
            }
            ')' => {
                out.push((start, Tok::RParen));
                i += 1;
            }
            ',' => {
                out.push((start, Tok::Comma));
                i += 1;
            }
            '=' | '<' | '>' | '!' => {
                let next = chars.get(i + 1).copied();
                let (op, len) = match (c, next) {
                    ('=', Some('=')) => (CmpOp::Eq, 2),
                    ('=', _) => (CmpOp::Eq, 1),
                    ('!', Some('=')) => (CmpOp::Ne, 2),
                    ('<', Some('>')) => (CmpOp::Ne, 2),
                    ('<', Some('=')) => (CmpOp::Le, 2),
                    ('<', _) => (CmpOp::Lt, 1),
                    ('>', Some('=')) => (CmpOp::Ge, 2),
                    ('>', _) => (CmpOp::Gt, 1),
                    _ => return Err(QueryError::at(start, "`!` on its own — did you mean `!=`?")),
                };
                out.push((start, Tok::Op(op)));
                i += len;
            }
            '"' | '\'' | '`' => {
                let quote = c;
                let mut s = String::new();
                i += 1;
                loop {
                    match chars.get(i) {
                        None => return Err(QueryError::at(start, "unclosed quote")),
                        Some('\\') if chars.get(i + 1).is_some() => {
                            s.push(chars[i + 1]);
                            i += 2;
                        }
                        Some(&ch) if ch == quote => {
                            i += 1;
                            break;
                        }
                        Some(&ch) => {
                            s.push(ch);
                            i += 1;
                        }
                    }
                }
                out.push((start, if quote == '`' { Tok::Name(s) } else { Tok::Str(s) }));
            }
            _ => {
                let mut s = String::new();
                while let Some(&ch) = chars.get(i) {
                    if ch.is_whitespace() || "()=<>!,\"'`".contains(ch) {
                        break;
                    }
                    s.push(ch);
                    i += 1;
                }
                out.push((start, Tok::Word(s)));
            }
        }
    }
    Ok(out)
}

// ── AST ───────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl CmpOp {
    fn symbol(self) -> &'static str {
        match self {
            CmpOp::Eq => "=",
            CmpOp::Ne => "<>",
            CmpOp::Lt => "<",
            CmpOp::Le => "<=",
            CmpOp::Gt => ">",
            CmpOp::Ge => ">=",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Seg {
    Name(String),
    Index(usize),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Path(pub Vec<Seg>);

impl Path {
    /// The attribute name when the path is a plain top-level attribute.
    fn top_level(&self) -> Option<&str> {
        match self.0.as_slice() {
            [Seg::Name(n)] => Some(n),
            _ => None,
        }
    }
    fn root(&self) -> Option<&str> {
        match self.0.first() {
            Some(Seg::Name(n)) => Some(n),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// Quoted — always a string.
    Str(String),
    /// Unquoted — typed by `typed_value`.
    Bare(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Cond {
    Cmp(Path, CmpOp, Value),
    Between(Path, Value, Value),
    BeginsWith(Path, Value),
    Contains(Path, Value),
    Exists(Path, bool),
    In(Path, Vec<Value>),
    And(Vec<Cond>),
    Or(Vec<Cond>),
    Not(Box<Cond>),
}

// ── Parser ────────────────────────────────────────────────────────────────────

struct Parser {
    toks: Vec<(usize, Tok)>,
    i: usize,
    end: usize,
}

fn is_kw(t: Option<&Tok>, kw: &str) -> bool {
    matches!(t, Some(Tok::Word(w)) if w.eq_ignore_ascii_case(kw))
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.i).map(|(_, t)| t)
    }
    fn peek_at(&self, n: usize) -> Option<&Tok> {
        self.toks.get(self.i + n).map(|(_, t)| t)
    }
    fn pos(&self) -> usize {
        self.toks.get(self.i).map(|(p, _)| *p).unwrap_or(self.end)
    }
    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.i).map(|(_, t)| t.clone());
        self.i += 1;
        t
    }
    fn eat_kw(&mut self, kw: &str) -> bool {
        if is_kw(self.peek(), kw) {
            self.i += 1;
            true
        } else {
            false
        }
    }
    fn expect(&mut self, want: Tok, what: &str) -> Result<(), QueryError> {
        let pos = self.pos();
        match self.next() {
            Some(t) if t == want => Ok(()),
            _ => Err(QueryError::at(pos, format!("expected {}", what))),
        }
    }

    fn or_expr(&mut self) -> Result<Cond, QueryError> {
        let mut parts = vec![self.and_expr()?];
        while self.eat_kw("or") || self.eat_sym("||") {
            parts.push(self.and_expr()?);
        }
        Ok(if parts.len() == 1 { parts.pop().unwrap() } else { Cond::Or(parts) })
    }

    fn eat_sym(&mut self, sym: &str) -> bool {
        if matches!(self.peek(), Some(Tok::Word(w)) if w == sym) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    fn and_expr(&mut self) -> Result<Cond, QueryError> {
        let mut parts = vec![self.unary()?];
        while self.eat_kw("and") || self.eat_sym("&&") {
            parts.push(self.unary()?);
        }
        Ok(if parts.len() == 1 { parts.pop().unwrap() } else { Cond::And(parts) })
    }

    fn unary(&mut self) -> Result<Cond, QueryError> {
        if self.eat_kw("not") {
            return Ok(Cond::Not(Box::new(self.unary()?)));
        }
        if self.peek() == Some(&Tok::LParen) {
            self.i += 1;
            let c = self.or_expr()?;
            self.expect(Tok::RParen, "`)`")?;
            return Ok(c);
        }
        // Function form: begins_with(path, v), contains(path, v), exists(path)…
        if let (Some(Tok::Word(w)), Some(Tok::LParen)) = (self.peek(), self.peek_at(1)) {
            let f = w.to_ascii_lowercase();
            if matches!(
                f.as_str(),
                "begins_with" | "contains" | "exists" | "attribute_exists" | "not_exists" | "attribute_not_exists"
            ) {
                self.i += 2;
                let path = self.path()?;
                let c = match f.as_str() {
                    "begins_with" | "contains" => {
                        self.expect(Tok::Comma, "`,` and a value")?;
                        let v = self.value()?;
                        if f == "contains" { Cond::Contains(path, v) } else { Cond::BeginsWith(path, v) }
                    }
                    "exists" | "attribute_exists" => Cond::Exists(path, true),
                    _ => Cond::Exists(path, false),
                };
                self.expect(Tok::RParen, "`)`")?;
                return Ok(c);
            }
        }
        self.clause()
    }

    fn clause(&mut self) -> Result<Cond, QueryError> {
        let path = self.path()?;
        let pos = self.pos();
        match self.peek().cloned() {
            Some(Tok::Op(op)) => {
                self.i += 1;
                Ok(Cond::Cmp(path, op, self.value()?))
            }
            Some(Tok::Word(w)) => {
                self.i += 1;
                match w.to_ascii_lowercase().as_str() {
                    "between" => {
                        let lo = self.value()?;
                        if !self.eat_kw("and") {
                            return Err(QueryError::at(self.pos(), "expected `and` in `between A and B`"));
                        }
                        Ok(Cond::Between(path, lo, self.value()?))
                    }
                    "begins_with" | "startswith" | "starts_with" => Ok(Cond::BeginsWith(path, self.value()?)),
                    "contains" => Ok(Cond::Contains(path, self.value()?)),
                    "exists" => Ok(Cond::Exists(path, true)),
                    "not" => {
                        if self.eat_kw("exists") {
                            Ok(Cond::Exists(path, false))
                        } else if self.eat_kw("contains") {
                            Ok(Cond::Not(Box::new(Cond::Contains(path, self.value()?))))
                        } else if self.eat_kw("begins_with") {
                            Ok(Cond::Not(Box::new(Cond::BeginsWith(path, self.value()?))))
                        } else if self.eat_kw("in") {
                            Ok(Cond::Not(Box::new(Cond::In(path, self.list()?))))
                        } else {
                            Err(QueryError::at(self.pos(), "expected `exists`, `contains`, `begins_with` or `in` after `not`"))
                        }
                    }
                    "in" => Ok(Cond::In(path, self.list()?)),
                    _ => Err(QueryError::at(
                        pos,
                        format!("`{}` isn't an operator — use = <> < <= > >= between begins_with contains exists in", w),
                    )),
                }
            }
            None => Err(QueryError::at(pos, "expected an operator, e.g. `= value`")),
            _ => Err(QueryError::at(pos, "expected an operator")),
        }
    }

    fn list(&mut self) -> Result<Vec<Value>, QueryError> {
        self.expect(Tok::LParen, "`(` to start the list")?;
        let mut vals = vec![self.value()?];
        while self.peek() == Some(&Tok::Comma) {
            self.i += 1;
            vals.push(self.value()?);
        }
        self.expect(Tok::RParen, "`)` to close the list")?;
        if vals.len() > 100 {
            return Err(QueryError::plain("`in` takes at most 100 values"));
        }
        Ok(vals)
    }

    fn path(&mut self) -> Result<Path, QueryError> {
        let pos = self.pos();
        match self.next() {
            Some(Tok::Name(n)) => Ok(Path(vec![Seg::Name(n)])),
            Some(Tok::Word(w)) if !is_reserved(&w) => parse_path(&w).map_err(|m| QueryError::at(pos, m)),
            Some(Tok::Word(w)) => Err(QueryError::at(pos, format!("expected an attribute name, found `{}`", w))),
            Some(Tok::Str(_)) => Err(QueryError::at(
                pos,
                "expected an attribute name — quote odd names with backticks: `my-attr`",
            )),
            None => Err(QueryError::at(pos, "expected a condition, e.g. `status = shipped`")),
            _ => Err(QueryError::at(pos, "expected an attribute name")),
        }
    }

    fn value(&mut self) -> Result<Value, QueryError> {
        let pos = self.pos();
        match self.next() {
            Some(Tok::Str(s)) => Ok(Value::Str(s)),
            Some(Tok::Word(w)) if !is_reserved(&w) => Ok(Value::Bare(w)),
            _ => Err(QueryError::at(pos, "expected a value")),
        }
    }
}

fn is_reserved(w: &str) -> bool {
    ["and", "or", "not", "between", "in", "&&", "||"].iter().any(|k| w.eq_ignore_ascii_case(k))
}

/// `address.city`, `items[0].sku` → path segments.
fn parse_path(w: &str) -> Result<Path, String> {
    let mut segs = Vec::new();
    for part in w.split('.') {
        let (name, mut rest) = match part.find('[') {
            Some(i) => (&part[..i], &part[i..]),
            None => (part, ""),
        };
        if name.is_empty() {
            return Err(format!("`{}` has an empty name segment", w));
        }
        segs.push(Seg::Name(name.to_string()));
        while let Some(r) = rest.strip_prefix('[') {
            let Some(close) = r.find(']') else {
                return Err(format!("`{}` is missing a `]`", w));
            };
            let n: usize = r[..close].parse().map_err(|_| format!("`{}`: list index must be a number", w))?;
            segs.push(Seg::Index(n));
            rest = &r[close + 1..];
        }
        if !rest.is_empty() {
            return Err(format!("unexpected `{}` in `{}`", rest, w));
        }
    }
    Ok(Path(segs))
}

/// Parse a query. An empty (or all-whitespace) query is `None`: read everything.
pub fn parse(input: &str) -> Result<Option<Cond>, QueryError> {
    let toks = lex(input)?;
    if toks.is_empty() {
        return Ok(None);
    }
    let mut p = Parser { toks, i: 0, end: input.chars().count() };
    let c = p.or_expr()?;
    if p.i < p.toks.len() {
        return Err(QueryError::at(p.pos(), "expected `and`, `or` or the end of the query"));
    }
    Ok(Some(c))
}

// ── Compiler ──────────────────────────────────────────────────────────────────

/// What a query compiles to: one Query or Scan, ready to send.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    /// Position in the browser's index list (0 = the base table).
    pub index: usize,
    /// `IndexName` to send; `None` for the base table.
    pub index_name: Option<String>,
    pub query: bool,
    pub key_condition: Option<String>,
    pub filter: Option<String>,
    pub names: HashMap<String, String>,
    pub values: HashMap<String, AttributeValue>,
    /// The key condition and filter written out with values inline, for the plan line.
    pub key_text: Option<String>,
    pub filter_text: Option<String>,
    /// For a Scan: why it couldn't be a Query.
    pub why_scan: Option<String>,
}

/// The type a bare value is sent as, given what we know about the attribute.
fn typed_value(v: &Value, key_type: Option<&str>) -> AttributeValue {
    match (v, key_type) {
        (Value::Str(s), Some("N")) | (Value::Bare(s), Some("N")) => AttributeValue::N(s.clone()),
        (Value::Str(s), _) | (Value::Bare(s), Some(_)) => AttributeValue::S(s.clone()),
        (Value::Bare(s), None) => {
            if s.eq_ignore_ascii_case("true") {
                AttributeValue::Bool(true)
            } else if s.eq_ignore_ascii_case("false") {
                AttributeValue::Bool(false)
            } else if s.eq_ignore_ascii_case("null") {
                AttributeValue::Null(true)
            } else if s.parse::<f64>().is_ok_and(|f| f.is_finite()) {
                AttributeValue::N(s.clone())
            } else {
                AttributeValue::S(s.clone())
            }
        }
    }
}

fn value_text(v: &Value, key_type: Option<&str>) -> String {
    match typed_value(v, key_type) {
        AttributeValue::S(s) => format!("{:?}", s),
        AttributeValue::N(n) => n,
        AttributeValue::Bool(b) => b.to_string(),
        _ => "null".to_string(),
    }
}

/// Renders conditions twice over: as the expression DynamoDB gets
/// (`#n0 = :v0`) and as text for the plan line (`status = "shipped"`).
struct Emit<'a> {
    key_types: &'a HashMap<String, String>,
    names: HashMap<String, String>,
    values: HashMap<String, AttributeValue>,
}

impl Emit<'_> {
    fn name(&mut self, n: &str) -> String {
        if let Some((k, _)) = self.names.iter().find(|(_, v)| v.as_str() == n) {
            return k.clone();
        }
        let k = format!("#n{}", self.names.len());
        self.names.insert(k.clone(), n.to_string());
        k
    }

    fn path(&mut self, p: &Path) -> (String, String) {
        let (mut expr, mut text) = (String::new(), String::new());
        for seg in &p.0 {
            match seg {
                Seg::Name(n) => {
                    if !expr.is_empty() {
                        expr.push('.');
                        text.push('.');
                    }
                    expr.push_str(&self.name(n));
                    text.push_str(n);
                }
                Seg::Index(i) => {
                    expr.push_str(&format!("[{}]", i));
                    text.push_str(&format!("[{}]", i));
                }
            }
        }
        (expr, text)
    }

    fn value(&mut self, p: &Path, v: &Value) -> (String, String) {
        let kt = p.top_level().and_then(|n| self.key_types.get(n)).map(String::as_str);
        let k = format!(":v{}", self.values.len());
        self.values.insert(k.clone(), typed_value(v, kt));
        (k, value_text(v, kt))
    }

    /// `(expression, text)` for one condition. `nested` parenthesises `or`.
    fn cond(&mut self, c: &Cond, nested: bool) -> (String, String) {
        let wrap = |(e, t): (String, String), yes: bool| if yes { (format!("({})", e), format!("({})", t)) } else { (e, t) };
        match c {
            Cond::Cmp(p, op, v) => {
                let (pe, pt) = self.path(p);
                let (ve, vt) = self.value(p, v);
                (format!("{} {} {}", pe, op.symbol(), ve), format!("{} {} {}", pt, op.symbol(), vt))
            }
            Cond::Between(p, lo, hi) => {
                let (pe, pt) = self.path(p);
                let (le, lt) = self.value(p, lo);
                let (he, ht) = self.value(p, hi);
                (format!("{} BETWEEN {} AND {}", pe, le, he), format!("{} between {} and {}", pt, lt, ht))
            }
            Cond::BeginsWith(p, v) => {
                let (pe, pt) = self.path(p);
                let (ve, vt) = self.value(p, v);
                (format!("begins_with({}, {})", pe, ve), format!("{} begins_with {}", pt, vt))
            }
            Cond::Contains(p, v) => {
                let (pe, pt) = self.path(p);
                // `contains` on a set or list matches an element, so the value
                // is typed on its own, not as the (container) attribute.
                let k = format!(":v{}", self.values.len());
                self.values.insert(k.clone(), typed_value(v, None));
                (format!("contains({}, {})", pe, k), format!("{} contains {}", pt, value_text(v, None)))
            }
            Cond::Exists(p, yes) => {
                let (pe, pt) = self.path(p);
                if *yes {
                    (format!("attribute_exists({})", pe), format!("{} exists", pt))
                } else {
                    (format!("attribute_not_exists({})", pe), format!("{} not exists", pt))
                }
            }
            Cond::In(p, vals) => {
                let (pe, pt) = self.path(p);
                let (es, ts): (Vec<_>, Vec<_>) = vals.iter().map(|v| self.value(p, v)).unzip();
                (format!("{} IN ({})", pe, es.join(", ")), format!("{} in ({})", pt, ts.join(", ")))
            }
            Cond::And(parts) if parts.len() == 1 => self.cond(&parts[0], nested),
            Cond::And(parts) => {
                let (es, ts): (Vec<_>, Vec<_>) = parts.iter().map(|c| self.cond(c, true)).unzip();
                wrap((es.join(" AND "), ts.join(" and ")), false)
            }
            Cond::Or(parts) => {
                let (es, ts): (Vec<_>, Vec<_>) = parts.iter().map(|c| self.cond(c, true)).unzip();
                wrap((es.join(" OR "), ts.join(" or ")), nested)
            }
            Cond::Not(inner) => {
                let (e, t) = self.cond(inner, true);
                (format!("NOT {}", e), format!("not {}", t))
            }
        }
    }
}

/// Every top-level attribute a condition reads.
fn roots<'a>(c: &'a Cond, out: &mut Vec<&'a str>) {
    match c {
        Cond::Cmp(p, ..) | Cond::Between(p, ..) | Cond::BeginsWith(p, _) | Cond::Contains(p, _)
        | Cond::Exists(p, _) | Cond::In(p, _) => out.extend(p.root()),
        Cond::And(cs) | Cond::Or(cs) => cs.iter().for_each(|c| roots(c, out)),
        Cond::Not(c) => roots(c, out),
    }
}

fn is_sk_cond(c: &Cond, sk: &str) -> bool {
    match c {
        Cond::Cmp(p, op, _) => p.top_level() == Some(sk) && *op != CmpOp::Ne,
        Cond::Between(p, ..) | Cond::BeginsWith(p, _) => p.top_level() == Some(sk),
        _ => false,
    }
}

/// The attributes an index carries: its keys, the table's keys, and its
/// `INCLUDE` list. `None` means everything (`ALL`, or the base table).
fn projected(idx: &DdbIndex, base: &DdbIndex) -> Option<Vec<String>> {
    let p = idx.projection.as_str();
    if p.is_empty() || p == "ALL" {
        return None;
    }
    let mut attrs: Vec<String> = [&idx.pk_name, &base.pk_name]
        .into_iter()
        .cloned()
        .chain(idx.sk_name.clone())
        .chain(base.sk_name.clone())
        .collect();
    if let Some(list) = p.strip_prefix("INCLUDE(").and_then(|r| r.strip_suffix(')')) {
        attrs.extend(list.split(',').map(|s| s.trim().to_string()));
    }
    Some(attrs)
}

/// How a query reads a given index: the conjunct indexes forming its key
/// condition (partition key, then the optional sort-key one).
struct KeyUse {
    pk: usize,
    sk: Option<usize>,
}

fn key_use(conjuncts: &[Cond], idx: &DdbIndex) -> Option<KeyUse> {
    let pk = conjuncts.iter().position(|c| {
        matches!(c, Cond::Cmp(p, CmpOp::Eq, _) if p.top_level() == Some(idx.pk_name.as_str()))
    })?;
    let sk = idx.sk_name.as_deref().and_then(|sk| conjuncts.iter().position(|c| is_sk_cond(c, sk)));
    Some(KeyUse { pk, sk })
}

/// `sk >= a and sk <= b` → `sk between a and b`, so a range fits in the one
/// sort-key condition a Query allows.
fn merge_sk_range(conjuncts: &mut Vec<Cond>, sk: &str) {
    let ge = conjuncts.iter().position(|c| matches!(c, Cond::Cmp(p, CmpOp::Ge, _) if p.top_level() == Some(sk)));
    let le = conjuncts.iter().position(|c| matches!(c, Cond::Cmp(p, CmpOp::Le, _) if p.top_level() == Some(sk)));
    if let (Some(a), Some(b)) = (ge, le) {
        let (Cond::Cmp(p, _, lo), Cond::Cmp(_, _, hi)) = (conjuncts[a].clone(), conjuncts[b].clone()) else {
            return;
        };
        let (first, second) = (a.min(b), a.max(b));
        conjuncts.remove(second);
        conjuncts[first] = Cond::Between(p, lo, hi);
    }
}

/// Compile a parsed query against the table's indexes (`indexes[0]` is the
/// base table). `pinned` forces one index: a Query on it if the query names
/// its partition key, else a Scan of it.
pub fn compile(cond: Option<&Cond>, indexes: &[DdbIndex], pinned: Option<usize>) -> Result<Plan, QueryError> {
    let base = indexes.first().ok_or_else(|| QueryError::plain("table has no key schema"))?;
    let mut key_types: HashMap<String, String> = HashMap::new();
    for i in indexes {
        key_types.insert(i.pk_name.clone(), i.pk_type.clone());
        if let (Some(n), Some(t)) = (&i.sk_name, &i.sk_type) {
            key_types.insert(n.clone(), t.clone());
        }
    }
    let mut conjuncts: Vec<Cond> = match cond {
        None => Vec::new(),
        Some(Cond::And(cs)) => cs.clone(),
        Some(c) => vec![c.clone()],
    };

    // Pick the index: the pinned one, or the best that has its partition key
    // pinned with `=` — one with a sort-key condition beats one without, the
    // base table beats a GSI, and an index that doesn't project every
    // attribute the filter reads is out (its items lack them).
    let mut chosen: Option<usize> = None;
    let mut skipped_projection: Option<&str> = None;
    if let Some(p) = pinned {
        if p < indexes.len() {
            if let Some(sk) = indexes[p].sk_name.clone() {
                merge_sk_range(&mut conjuncts, &sk);
            }
            if key_use(&conjuncts, &indexes[p]).is_some() {
                chosen = Some(p);
            }
        }
    } else {
        let mut best: Option<(usize, (bool, bool))> = None;
        for (i, idx) in indexes.iter().enumerate() {
            let mut cs = conjuncts.clone();
            if let Some(sk) = &idx.sk_name {
                merge_sk_range(&mut cs, sk);
            }
            let Some(ku) = key_use(&cs, idx) else { continue };
            if let Some(attrs) = projected(idx, base) {
                let mut read = Vec::new();
                cs.iter().for_each(|c| roots(c, &mut read));
                if read.iter().any(|r| !attrs.iter().any(|a| a == r)) {
                    skipped_projection.get_or_insert(idx.name.as_str());
                    continue;
                }
            }
            let score = (ku.sk.is_some(), i == 0);
            if best.is_none_or(|(_, s)| score > s) {
                best = Some((i, score));
            }
        }
        if let Some((i, _)) = best {
            if let Some(sk) = &indexes[i].sk_name {
                merge_sk_range(&mut conjuncts, sk);
            }
            chosen = Some(i);
        }
    }

    let mut emit = Emit { key_types: &key_types, names: HashMap::new(), values: HashMap::new() };
    let index_at = |i: usize| -> Option<String> {
        let n = &indexes[i].name;
        (!n.is_empty()).then(|| n.clone())
    };

    let Some(ci) = chosen else {
        // Scan: everything is filter.
        let scan_idx = pinned.filter(|&p| p < indexes.len()).unwrap_or(0);
        let why = if conjuncts.is_empty() {
            "no condition — reads every item".to_string()
        } else if let Some(gsi) = skipped_projection {
            format!("index `{}` matches but doesn't project every attribute the filter reads", gsi)
        } else {
            let keys: Vec<String> = match pinned {
                Some(p) if p < indexes.len() => vec![format!("`{} = …`", indexes[p].pk_name)],
                _ => indexes.iter().map(|i| format!("`{} = …`", i.pk_name)).collect::<Vec<_>>(),
            };
            let mut keys = keys;
            keys.dedup();
            format!("a Query needs {}", keys.join(" or "))
        };
        let (filter, filter_text) = if conjuncts.is_empty() {
            (None, None)
        } else {
            let (e, t) = emit.cond(&Cond::And(conjuncts), false);
            (Some(e), Some(t))
        };
        return Ok(Plan {
            index: scan_idx,
            index_name: index_at(scan_idx),
            query: false,
            key_condition: None,
            filter,
            names: emit.names,
            values: emit.values,
            key_text: None,
            filter_text,
            why_scan: Some(why),
        });
    };

    let idx = &indexes[ci];
    let ku = key_use(&conjuncts, idx).expect("chosen index has a key condition");
    let mut key_parts = vec![conjuncts[ku.pk].clone()];
    if let Some(s) = ku.sk {
        key_parts.push(conjuncts[s].clone());
    }
    let rest: Vec<Cond> = conjuncts
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != ku.pk && Some(*i) != ku.sk)
        .map(|(_, c)| c.clone())
        .collect();

    // A Query's filter can't read the index's own key attributes.
    let mut read = Vec::new();
    rest.iter().for_each(|c| roots(c, &mut read));
    for key in std::iter::once(&idx.pk_name).chain(idx.sk_name.as_ref()) {
        if read.contains(&key.as_str()) {
            let msg = if Some(key) == idx.sk_name.as_ref() {
                format!("a Query takes one condition on sort key `{}` — for a range use `{} between A and B`", key, key)
            } else {
                format!("a Query takes one `{} = …`; it can't also be in an `or` or `not`", key)
            };
            return Err(QueryError::plain(msg));
        }
    }

    let (ke, kt) = emit.cond(&Cond::And(key_parts), false);
    let (filter, filter_text) = if rest.is_empty() {
        (None, None)
    } else {
        let (e, t) = emit.cond(&Cond::And(rest), false);
        (Some(e), Some(t))
    };
    Ok(Plan {
        index: ci,
        index_name: index_at(ci),
        query: true,
        key_condition: Some(ke),
        filter,
        names: emit.names,
        values: emit.values,
        key_text: Some(kt),
        filter_text,
        why_scan: None,
    })
}

/// Parse and compile in one step — what the browser calls on every keystroke.
pub fn plan(input: &str, indexes: &[DdbIndex], pinned: Option<usize>) -> Result<Plan, QueryError> {
    let cond = parse(input)?;
    compile(cond.as_ref(), indexes, pinned)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index(name: &str, pk: &str, pk_type: &str, sk: Option<(&str, &str)>, projection: &str) -> DdbIndex {
        DdbIndex {
            name: name.into(),
            keys: String::new(),
            projection: projection.into(),
            status: String::new(),
            read_capacity: None,
            write_capacity: None,
            pk_name: pk.into(),
            pk_type: pk_type.into(),
            sk_name: sk.map(|s| s.0.into()),
            sk_type: sk.map(|s| s.1.into()),
        }
    }

    fn table() -> Vec<DdbIndex> {
        vec![
            index("", "pk", "S", Some(("sk", "S")), ""),
            index("byStatus", "status", "S", Some(("createdAt", "N")), "ALL"),
            index("byEmail", "email", "S", None, "KEYS_ONLY"),
        ]
    }

    /// The expression with placeholders swapped back for names/values, so
    /// assertions read like the query.
    fn expand(expr: &str, plan: &Plan) -> String {
        let mut out = expr.to_string();
        let mut names: Vec<_> = plan.names.iter().collect();
        names.sort_by_key(|(k, _)| std::cmp::Reverse(k.len()));
        for (k, v) in names {
            out = out.replace(k.as_str(), v);
        }
        let mut values: Vec<_> = plan.values.iter().collect();
        values.sort_by_key(|(k, _)| std::cmp::Reverse(k.len()));
        for (k, v) in values {
            let lit = match v {
                AttributeValue::S(s) => format!("S:{}", s),
                AttributeValue::N(n) => format!("N:{}", n),
                AttributeValue::Bool(b) => format!("BOOL:{}", b),
                _ => "NULL".into(),
            };
            out = out.replace(k.as_str(), &lit);
        }
        out
    }

    fn p(q: &str) -> Plan {
        plan(q, &table(), None).unwrap_or_else(|e| panic!("{q}: {e:?}"))
    }

    #[test]
    fn empty_query_scans_everything() {
        let pl = p("  ");
        assert!(!pl.query);
        assert_eq!(pl.filter, None);
        assert!(pl.why_scan.unwrap().contains("every item"));
    }

    #[test]
    fn partition_key_equality_becomes_a_query() {
        let pl = p(r#"pk = "USER#42""#);
        assert!(pl.query);
        assert_eq!(pl.index_name, None);
        assert_eq!(expand(pl.key_condition.as_ref().unwrap(), &pl), "pk = S:USER#42");
        assert_eq!(pl.filter, None);
    }

    #[test]
    fn sort_key_condition_joins_the_key_and_the_rest_filters() {
        let pl = p(r#"pk = USER#42 and sk begins_with "ORDER#" and total > 100"#);
        assert_eq!(
            expand(pl.key_condition.as_ref().unwrap(), &pl),
            "pk = S:USER#42 AND begins_with(sk, S:ORDER#)"
        );
        assert_eq!(expand(pl.filter.as_ref().unwrap(), &pl), "total > N:100");
        assert_eq!(pl.key_text.as_deref(), Some(r#"pk = "USER#42" and sk begins_with "ORDER#""#));
    }

    #[test]
    fn a_gsi_is_chosen_when_it_has_the_partition_key() {
        let pl = p("status = shipped and createdAt > 1700000000");
        assert!(pl.query);
        assert_eq!(pl.index_name.as_deref(), Some("byStatus"));
        assert_eq!(pl.index, 1);
        assert_eq!(
            expand(pl.key_condition.as_ref().unwrap(), &pl),
            "status = S:shipped AND createdAt > N:1700000000"
        );
    }

    #[test]
    fn a_keys_only_index_is_skipped_when_the_filter_needs_more() {
        // email is byEmail's key, but `total` isn't projected there.
        let pl = p("email = a@b.com and total > 5");
        assert!(!pl.query);
        assert!(pl.why_scan.unwrap().contains("byEmail"));
        // Reading only projected attributes is fine.
        let pl = p("email = a@b.com");
        assert_eq!(pl.index_name.as_deref(), Some("byEmail"));
    }

    #[test]
    fn no_partition_key_scans_and_says_why() {
        let pl = p("total > 100 and coupon exists");
        assert!(!pl.query);
        assert_eq!(
            expand(pl.filter.as_ref().unwrap(), &pl),
            "total > N:100 AND attribute_exists(coupon)"
        );
        let why = pl.why_scan.unwrap();
        assert!(why.contains("`pk = …`") && why.contains("`status = …`"), "{why}");
    }

    #[test]
    fn key_types_override_bare_value_guessing() {
        // pk is S: a bare 2024 stays a string. createdAt is N: quotes don't matter.
        let pl = p(r#"pk = 2024"#);
        assert_eq!(expand(pl.key_condition.as_ref().unwrap(), &pl), "pk = S:2024");
        let pl = p(r#"status = x and createdAt = "5""#);
        assert!(expand(pl.key_condition.as_ref().unwrap(), &pl).ends_with("createdAt = N:5"));
        // A non-key bare number is a number; quoted is a string.
        let pl = p(r#"zip = "007" and total = 7 and gift = true"#);
        assert_eq!(
            expand(pl.filter.as_ref().unwrap(), &pl),
            "zip = S:007 AND total = N:7 AND gift = BOOL:true"
        );
    }

    #[test]
    fn a_range_on_the_sort_key_merges_into_between() {
        let pl = p("pk = u and sk >= a and sk <= m");
        assert_eq!(expand(pl.key_condition.as_ref().unwrap(), &pl), "pk = S:u AND sk BETWEEN S:a AND S:m");
        assert_eq!(pl.filter, None);
    }

    #[test]
    fn two_sort_key_conditions_that_cannot_merge_are_an_error() {
        let e = plan("pk = u and sk > a and sk < m", &table(), None).unwrap_err();
        assert!(e.message.contains("between"), "{}", e.message);
    }

    #[test]
    fn or_and_not_and_nesting_render_with_parentheses() {
        let pl = p("pk = u and (status = a or not tags contains gift) and address.city = Perth and items[0].qty >= 2");
        assert_eq!(
            expand(pl.filter.as_ref().unwrap(), &pl),
            "(status = S:a OR NOT contains(tags, S:gift)) AND address.city = S:Perth AND items[0].qty >= N:2"
        );
    }

    #[test]
    fn in_lists_and_function_forms() {
        let pl = p("status in (pending, shipped) or begins_with(sku, AB) or attribute_not_exists(x)");
        assert_eq!(
            expand(pl.filter.as_ref().unwrap(), &pl),
            "status IN (S:pending, S:shipped) OR begins_with(sku, S:AB) OR attribute_not_exists(x)"
        );
    }

    #[test]
    fn an_or_at_the_top_is_a_scan() {
        let pl = p("pk = a or pk = b");
        assert!(!pl.query);
    }

    #[test]
    fn pinning_an_index_without_its_key_scans_that_index() {
        let pl = plan("total > 1", &table(), Some(1)).unwrap();
        assert!(!pl.query);
        assert_eq!(pl.index_name.as_deref(), Some("byStatus"));
        assert!(pl.why_scan.unwrap().contains("`status = …`"));
        // …and a pinned index does override the base-table preference.
        let pl = plan("pk = u and status = s", &table(), Some(1)).unwrap();
        assert!(pl.query);
        assert_eq!(pl.index_name.as_deref(), Some("byStatus"));
    }

    #[test]
    fn backtick_names_and_repeated_names_share_a_placeholder() {
        let pl = p("`my-attr` = 1 or `my-attr` = 2");
        assert_eq!(pl.names.len(), 1);
        assert_eq!(expand(pl.filter.as_ref().unwrap(), &pl), "my-attr = N:1 OR my-attr = N:2");
    }

    #[test]
    fn errors_point_at_the_problem() {
        let e = parse("status =").unwrap_err();
        assert_eq!(e.pos, Some(8));
        assert!(e.message.contains("value"));
        let e = parse("status is 5").unwrap_err();
        assert_eq!(e.pos, Some(7));
        assert!(parse(r#"x = "open"#).unwrap_err().message.contains("unclosed"));
        assert!(parse("a = 1 b = 2").unwrap_err().message.contains("`and`"));
        assert!(parse("(a = 1").unwrap_err().message.contains(")"));
        assert!(parse("= 5").is_err());
    }

    #[test]
    fn keywords_are_case_insensitive_and_symbols_work() {
        let pl = p("pk = u AND sk BEGINS_WITH x && total != 3");
        assert_eq!(expand(pl.filter.as_ref().unwrap(), &pl), "total <> N:3");
    }

    #[test]
    fn escaped_quotes_survive() {
        let pl = p(r#"note = "say \"hi\"""#);
        assert!(matches!(pl.values.values().next(), Some(AttributeValue::S(s)) if s == r#"say "hi""#));
    }
}
