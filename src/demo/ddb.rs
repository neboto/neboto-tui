//! DynamoDB for the demo account: two tables generated from a fixed seed,
//! answered per request — `DescribeTable` from the same data, so its item
//! count and size can't disagree with what a Scan finds, and `Query` /
//! `Scan` by **evaluating the request's expressions** against the items.
//!
//! A stored Query response would answer every query with the same items, so
//! `total > 100` would come back with a `total` of 40 — the item browser's
//! query language would look broken to exactly the people trying it. The
//! expressions are parsed with the browser's own `query.rs` parser (the wire
//! form is a subset of its language: `#n0` / `:v0` are just words to it),
//! with placeholders resolved from the request.
//!
//! The story (see `fixtures.rs`' header for the rest of the account):
//! - `orders` is a single-table design — `USER#n` partitions holding a
//!   `PROFILE` item and `ORDER#<date>#<id>` items — provisioned at 5 RCU /
//!   5 WCU, which is why `orders-api`'s `PutItem`s throttle (its log tail
//!   and the table's `m` Write throttles chart say the same)
//! - GSIs: `byStatus` (status, createdAt; ALL) over orders only — sparse —
//!   and `byEmail` (KEYS_ONLY) over profiles, so a filter on anything else
//!   can't be served from it and the plan line says so
//! - three orders from a legacy import store `total` as a **string**, so
//!   `total > 100` silently misses them, as it would in AWS
//! - cancelled orders carry `expiresAt`, the table's TTL attribute
//! - `catalog` is the AppSync `CatalogTable` data source: products by `sku`

use super::DemoRequest;
use crate::aws::services::dynamodb::query::{self, CmpOp, Cond, Path, Seg, Value};
use crate::aws::services::dynamodb::{av_cmp, av_from_ddb_json, av_to_ddb_json, item_ddb_json, item_from_ddb_json, DdbItem};
use aws_sdk_dynamodb::types::AttributeValue;
use serde_json::json;
use std::cmp::Ordering;

pub fn respond(req: &DemoRequest, now: i64) -> Option<String> {
    if req.service != "dynamodb" {
        return None;
    }
    let body: serde_json::Value = serde_json::from_str(&req.body).ok()?;
    match req.operation.as_str() {
        "DescribeTable" => Some(describe_table(&body, now)),
        "Query" => Some(read(&body, now, true)),
        "Scan" => Some(read(&body, now, false)),
        _ => None,
    }
}

// ── The dataset ───────────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
enum Projection {
    All,
    KeysOnly,
}

struct Index {
    name: &'static str,
    pk: &'static str,
    sk: Option<&'static str>,
    projection: Projection,
}

pub(super) struct Table {
    pub name: &'static str,
    pk: &'static str,
    sk: Option<&'static str>,
    /// `AttributeDefinitions`: every key attribute of the table and its indexes.
    attrs: &'static [(&'static str, &'static str)],
    gsis: Vec<Index>,
    read_capacity: i64,
    write_capacity: i64,
    created_days_ago: i64,
    pub items: Vec<DdbItem>,
}

/// splitmix64, as in `generate.rs`: repeatable "random" details.
fn mix(k: i64) -> u64 {
    let mut x = (k as u64).wrapping_add(0x9e37_79b9_7f4a_7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

fn pick<T: Copy>(list: &[T], k: i64) -> T {
    list[(mix(k) % list.len() as u64) as usize]
}

fn s(v: impl Into<String>) -> AttributeValue {
    AttributeValue::S(v.into())
}
fn n(v: impl ToString) -> AttributeValue {
    AttributeValue::N(v.to_string())
}

const PRODUCTS: &[(&str, &str, f64, &str)] = &[
    ("SKU-1001", "Linen tea towel", 14.0, "kitchen"),
    ("SKU-1002", "Stoneware mug", 22.0, "kitchen"),
    ("SKU-1003", "Cast iron skillet", 89.0, "kitchen"),
    ("SKU-1004", "Chef's knife", 129.0, "kitchen"),
    ("SKU-1005", "Pour-over kettle", 74.5, "kitchen"),
    ("SKU-2001", "Wool throw", 159.0, "living"),
    ("SKU-2002", "Ceramic vase", 48.0, "living"),
    ("SKU-2003", "Floor cushion", 65.0, "living"),
    ("SKU-2004", "Reading lamp", 112.0, "living"),
    ("SKU-3001", "Cotton sheet set", 139.0, "bedroom"),
    ("SKU-3002", "Down pillow", 79.0, "bedroom"),
    ("SKU-3003", "Bedside carafe", 36.0, "bedroom"),
    ("SKU-4001", "Canvas tote", 29.0, "outdoor"),
    ("SKU-4002", "Enamel camp mug", 18.5, "outdoor"),
    ("SKU-4003", "Picnic blanket", 69.0, "outdoor"),
    ("SKU-5001", "Gift card $50", 50.0, "gift"),
];
const FIRST: &[&str] = &["Ava", "Noah", "Mia", "Leo", "Zoe", "Kai", "Ivy", "Eli", "Ruby", "Finn", "Isla", "Theo"];
const LAST: &[&str] = &["Nguyen", "Smith", "Patel", "Kowalski", "Okafor", "Garcia", "Chen", "Murphy", "Rossi", "Haddad"];
const CITIES: &[(&str, &str)] = &[
    ("Perth", "6000"),
    ("Sydney", "2000"),
    ("Melbourne", "3000"),
    ("Brisbane", "4000"),
    ("Adelaide", "5000"),
    ("Hobart", "7000"),
];
const USERS: i64 = 48;
/// Orders (by their running number across the table) whose `total` a
/// legacy importer wrote as a string.
const LEGACY_ORDERS: &[usize] = &[9, 58, 121];

fn ymd(epoch: i64) -> String {
    chrono::DateTime::from_timestamp(epoch, 0).map(|t| t.format("%Y-%m-%d").to_string()).unwrap_or_default()
}

fn orders_items(now: i64) -> Vec<DdbItem> {
    // Whole days, so keys (which carry the date) only move at UTC midnight
    // and a paging cursor stays valid between requests.
    let today = now.div_euclid(86_400) * 86_400;
    let mut items = Vec::new();
    let mut order_no = 0usize;
    for u in 1..=USERS {
        let first = pick(FIRST, u);
        let last = pick(LAST, u * 7 + 1);
        let (city, zip) = pick(CITIES, u * 3);
        let pk = format!("USER#{}", u);
        let joined = today - (30 + (mix(u * 11) % 600) as i64) * 86_400;
        let mut profile: DdbItem = [
            ("pk", s(&pk)),
            ("sk", s("PROFILE")),
            ("type", s("USER")),
            ("name", s(format!("{} {}", first, last))),
            ("email", s(format!("{}.{}{}@example.com", first.to_lowercase(), last.to_lowercase(), u))),
            ("tier", s(pick(&["basic", "basic", "silver", "gold"], u * 13))),
            ("createdAt", n(joined)),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
        if mix(u * 17).is_multiple_of(4) {
            profile.insert("marketingOptIn".into(), AttributeValue::Bool(true));
        }
        items.push(profile);

        // USER#42 is the query line's example, so it has orders to find.
        let count = if u == 42 { 6 } else { (mix(u * 5) % 7) as i64 };
        for j in 0..count {
            let id = u * 10 + j;
            let days = (mix(id * 3) % 90) as i64;
            let created = today - days * 86_400 + (mix(id * 7) % 86_400) as i64;
            let status = if mix(id * 19).is_multiple_of(12) {
                "cancelled"
            } else if days < 2 {
                "pending"
            } else if days < 6 {
                "shipped"
            } else {
                "delivered"
            };
            let lines: Vec<AttributeValue> = (0..1 + (mix(id * 23) % 3) as i64)
                .map(|l| {
                    let (sku, _, price, _) = pick(PRODUCTS, id * 31 + l);
                    let qty = 1 + (mix(id * 37 + l) % 3) as i64;
                    AttributeValue::M(
                        [("sku", s(sku)), ("qty", n(qty)), ("price", n(format!("{:.2}", price)))]
                            .into_iter()
                            .map(|(k, v)| (k.to_string(), v))
                            .collect(),
                    )
                })
                .collect();
            let total: f64 = lines
                .iter()
                .filter_map(|l| match l {
                    AttributeValue::M(m) => {
                        let num = |k: &str| match m.get(k) {
                            Some(AttributeValue::N(x)) => x.parse::<f64>().ok(),
                            _ => None,
                        };
                        Some(num("qty")? * num("price")?)
                    }
                    _ => None,
                })
                .sum();
            let legacy = LEGACY_ORDERS.contains(&order_no);
            order_no += 1;
            let mut order: DdbItem = [
                ("pk", s(&pk)),
                ("sk", s(format!("ORDER#{}#{:04}", ymd(created), id))),
                ("type", s("ORDER")),
                ("status", s(status)),
                ("createdAt", n(created)),
                ("items", AttributeValue::L(lines)),
                (
                    "total",
                    if legacy { s(format!("{:.2}", total)) } else { n(format!("{:.2}", total)) },
                ),
                (
                    "address",
                    AttributeValue::M(
                        [("city", s(city)), ("zip", s(zip)), ("country", s("AU"))]
                            .into_iter()
                            .map(|(k, v)| (k.to_string(), v))
                            .collect(),
                    ),
                ),
            ]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect();
            if legacy {
                order.insert("source".into(), s("legacy-import"));
            }
            if mix(id * 41).is_multiple_of(5) {
                order.insert("coupon".into(), s(pick(&["SPRING10", "WELCOME5", "VIP20"], id)));
            }
            if mix(id * 43).is_multiple_of(6) {
                order.insert("giftWrap".into(), AttributeValue::Bool(true));
            }
            if status == "cancelled" {
                order.insert("expiresAt".into(), n(created + 30 * 86_400));
            }
            items.push(order);
        }
    }
    items
}

fn catalog_items() -> Vec<DdbItem> {
    PRODUCTS
        .iter()
        .enumerate()
        .map(|(i, (sku, name, price, category))| {
            let mut tags = vec![category.to_string()];
            if price > &100.0 {
                tags.push("premium".into());
            }
            if i % 4 == 0 {
                tags.push("bestseller".into());
            }
            [
                ("sku", s(*sku)),
                ("name", s(*name)),
                ("price", n(format!("{:.2}", price))),
                // Two products are out of stock.
                ("stock", n(if i == 3 || i == 9 { 0 } else { 5 + mix(i as i64) % 120 })),
                ("category", s(*category)),
                ("tags", AttributeValue::Ss(tags)),
            ]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect()
        })
        .collect()
}

pub(super) fn tables(now: i64) -> Vec<Table> {
    vec![
        Table {
            name: "orders",
            pk: "pk",
            sk: Some("sk"),
            attrs: &[("pk", "S"), ("sk", "S"), ("status", "S"), ("createdAt", "N"), ("email", "S")],
            gsis: vec![
                Index { name: "byStatus", pk: "status", sk: Some("createdAt"), projection: Projection::All },
                Index { name: "byEmail", pk: "email", sk: None, projection: Projection::KeysOnly },
            ],
            read_capacity: 5,
            write_capacity: 5,
            created_days_ago: 412,
            items: orders_items(now),
        },
        Table {
            name: "catalog",
            pk: "sku",
            sk: None,
            attrs: &[("sku", "S")],
            gsis: Vec::new(),
            read_capacity: 10,
            write_capacity: 2,
            created_days_ago: 380,
            items: catalog_items(),
        },
    ]
}

fn item_size(item: &DdbItem) -> usize {
    serde_json::to_string(&item_ddb_json(item)).map_or(0, |j| j.len())
}

// ── DescribeTable ─────────────────────────────────────────────────────────────

fn not_found() -> String {
    "!status 400\n{\"__type\":\"com.amazonaws.dynamodb.v20120810#ResourceNotFoundException\",\"message\":\"Requested resource not found\"}".to_string()
}

fn validation(msg: &str) -> String {
    format!(
        "!status 400\n{}",
        json!({ "__type": "com.amazon.coral.validate#ValidationException", "message": msg })
    )
}

fn describe_table(body: &serde_json::Value, now: i64) -> String {
    let Some(name) = body.get("TableName").and_then(|v| v.as_str()) else {
        return validation("TableName is required");
    };
    let Some(t) = tables(now).into_iter().find(|t| t.name == name) else {
        return not_found();
    };
    let arn = format!("arn:aws:dynamodb:us-east-1:{}:table/{}", super::ACCOUNT, t.name);
    let schema = |pk: &str, sk: Option<&str>| {
        let mut k = vec![json!({ "AttributeName": pk, "KeyType": "HASH" })];
        k.extend(sk.map(|sk| json!({ "AttributeName": sk, "KeyType": "RANGE" })));
        k
    };
    let size: usize = t.items.iter().map(item_size).sum();
    let gsis: Vec<serde_json::Value> = t
        .gsis
        .iter()
        .map(|g| {
            let in_index: Vec<&DdbItem> = t.items.iter().filter(|i| i.contains_key(g.pk)).collect();
            json!({
                "IndexName": g.name,
                "KeySchema": schema(g.pk, g.sk),
                "Projection": { "ProjectionType": match g.projection { Projection::All => "ALL", Projection::KeysOnly => "KEYS_ONLY" } },
                "IndexStatus": "ACTIVE",
                "ProvisionedThroughput": { "ReadCapacityUnits": t.read_capacity, "WriteCapacityUnits": t.write_capacity, "NumberOfDecreasesToday": 0 },
                "IndexSizeBytes": in_index.iter().map(|i| item_size(i)).sum::<usize>(),
                "ItemCount": in_index.len(),
                "IndexArn": format!("{}/index/{}", arn, g.name),
            })
        })
        .collect();
    let mut table = json!({
        "TableName": t.name,
        "TableArn": arn,
        "TableId": format!("{:08x}-demo-4000-8000-{:012x}", mix(t.created_days_ago) as u32, mix(t.created_days_ago + 1) & 0xffff_ffff_ffff),
        "TableStatus": "ACTIVE",
        "CreationDateTime": now - t.created_days_ago * 86_400,
        "KeySchema": schema(t.pk, t.sk),
        "AttributeDefinitions": t.attrs.iter().map(|(n, ty)| json!({ "AttributeName": n, "AttributeType": ty })).collect::<Vec<_>>(),
        "ProvisionedThroughput": { "ReadCapacityUnits": t.read_capacity, "WriteCapacityUnits": t.write_capacity, "NumberOfDecreasesToday": 0 },
        "ItemCount": t.items.len(),
        "TableSizeBytes": size,
        "TableClassSummary": { "TableClass": "STANDARD" },
        "DeletionProtectionEnabled": true,
    });
    if !gsis.is_empty() {
        table["GlobalSecondaryIndexes"] = json!(gsis);
    }
    json!({ "Table": table }).to_string()
}

// ── Query / Scan ──────────────────────────────────────────────────────────────

/// What an expression's placeholders stand for, from the request.
struct Ctx {
    names: std::collections::HashMap<String, String>,
    values: std::collections::HashMap<String, AttributeValue>,
}

impl Ctx {
    fn from(body: &serde_json::Value) -> Self {
        let names = body
            .get("ExpressionAttributeNames")
            .and_then(|v| v.as_object())
            .map(|m| m.iter().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string()))).collect())
            .unwrap_or_default();
        let values = body
            .get("ExpressionAttributeValues")
            .and_then(|v| v.as_object())
            .map(|m| m.iter().filter_map(|(k, v)| Some((k.clone(), av_from_ddb_json(v)?))).collect())
            .unwrap_or_default();
        Ctx { names, values }
    }

    fn name<'a>(&'a self, n: &'a str) -> &'a str {
        if n.starts_with('#') {
            self.names.get(n).map(String::as_str).unwrap_or(n)
        } else {
            n
        }
    }

    fn resolve<'a>(&self, path: &Path, item: &'a DdbItem) -> Option<&'a AttributeValue> {
        let mut segs = path.0.iter();
        let Some(Seg::Name(first)) = segs.next() else { return None };
        let mut cur = item.get(self.name(first))?;
        for seg in segs {
            cur = match (seg, cur) {
                (Seg::Name(k), AttributeValue::M(m)) => m.get(self.name(k))?,
                (Seg::Index(i), AttributeValue::L(l)) => l.get(*i)?,
                _ => return None,
            };
        }
        Some(cur)
    }

    /// A value operand: a `:placeholder`, or (for hand-written requests) a
    /// literal the browser's parser would type the same way.
    fn value(&self, v: &Value) -> Option<AttributeValue> {
        match v {
            Value::Bare(b) if b.starts_with(':') => self.values.get(b).cloned(),
            Value::Bare(b) => Some(AttributeValue::S(b.clone())),
            Value::Str(s) => Some(AttributeValue::S(s.clone())),
        }
    }

    /// DynamoDB's comparison rules: only like types compare (a number never
    /// equals a string — that's the legacy `total` story), and a missing
    /// attribute makes every comparison false, `<>` included.
    fn cmp(&self, a: &AttributeValue, b: &AttributeValue) -> Option<Ordering> {
        match (a, b) {
            (AttributeValue::N(_), AttributeValue::N(_))
            | (AttributeValue::S(_), AttributeValue::S(_))
            | (AttributeValue::B(_), AttributeValue::B(_)) => Some(av_cmp(a, b)),
            _ if a == b => Some(Ordering::Equal),
            _ => None,
        }
    }

    fn eval(&self, c: &Cond, item: &DdbItem) -> bool {
        match c {
            Cond::Cmp(p, op, v) => {
                let (Some(a), Some(b)) = (self.resolve(p, item), self.value(v)) else { return false };
                match (self.cmp(a, &b), op) {
                    (None, CmpOp::Ne) => true,
                    (None, _) => false,
                    (Some(o), CmpOp::Eq) => o == Ordering::Equal,
                    (Some(o), CmpOp::Ne) => o != Ordering::Equal,
                    (Some(o), CmpOp::Lt) => o == Ordering::Less,
                    (Some(o), CmpOp::Le) => o != Ordering::Greater,
                    (Some(o), CmpOp::Gt) => o == Ordering::Greater,
                    (Some(o), CmpOp::Ge) => o != Ordering::Less,
                }
            }
            Cond::Between(p, lo, hi) => {
                let (Some(a), Some(lo), Some(hi)) = (self.resolve(p, item), self.value(lo), self.value(hi)) else {
                    return false;
                };
                matches!(self.cmp(a, &lo), Some(Ordering::Greater | Ordering::Equal))
                    && matches!(self.cmp(a, &hi), Some(Ordering::Less | Ordering::Equal))
            }
            Cond::BeginsWith(p, v) => match (self.resolve(p, item), self.value(v)) {
                (Some(AttributeValue::S(a)), Some(AttributeValue::S(b))) => a.starts_with(&b),
                _ => false,
            },
            Cond::Contains(p, v) => match (self.resolve(p, item), self.value(v)) {
                (Some(AttributeValue::S(a)), Some(AttributeValue::S(b))) => a.contains(&b),
                (Some(AttributeValue::Ss(set)), Some(AttributeValue::S(b))) => set.contains(&b),
                (Some(AttributeValue::Ns(set)), Some(AttributeValue::N(b))) => set.contains(&b),
                (Some(AttributeValue::L(l)), Some(b)) => l.contains(&b),
                _ => false,
            },
            Cond::Exists(p, yes) => self.resolve(p, item).is_some() == *yes,
            Cond::In(p, vals) => self.resolve(p, item).is_some_and(|a| {
                vals.iter()
                    .filter_map(|v| self.value(v))
                    .any(|b| self.cmp(a, &b) == Some(Ordering::Equal))
            }),
            Cond::And(cs) => cs.iter().all(|c| self.eval(c, item)),
            Cond::Or(cs) => cs.iter().any(|c| self.eval(c, item)),
            Cond::Not(c) => !self.eval(c, item),
        }
    }
}

/// Sort key order: numbers numerically, strings lexically (as DynamoDB
/// stores them), a missing value first.
fn key_order(a: Option<&AttributeValue>, b: Option<&AttributeValue>) -> Ordering {
    match (a, b) {
        (Some(a), Some(b)) => av_cmp(a, b),
        (a, b) => a.is_some().cmp(&b.is_some()),
    }
}

/// A stand-in for DynamoDB's partition hash, so a Scan comes back in an
/// order that isn't the keys' — as it does in AWS.
fn partition_hash(v: Option<&AttributeValue>) -> u64 {
    let text = match v {
        Some(AttributeValue::S(s)) | Some(AttributeValue::N(s)) => s.as_str(),
        _ => "",
    };
    mix(text.bytes().fold(7i64, |h, b| h.wrapping_mul(131).wrapping_add(b as i64)))
}

fn read(body: &serde_json::Value, now: i64, is_query: bool) -> String {
    let Some(name) = body.get("TableName").and_then(|v| v.as_str()) else {
        return validation("TableName is required");
    };
    let all = tables(now);
    let Some(t) = all.iter().find(|t| t.name == name) else {
        return not_found();
    };
    let index_name = body.get("IndexName").and_then(|v| v.as_str());
    let index = match index_name {
        Some(i) => match t.gsis.iter().find(|g| g.name == i) {
            Some(g) => Some(g),
            None => return validation(&format!("The table does not have the specified index: {}", i)),
        },
        None => None,
    };
    let (pk, sk) = index.map_or((t.pk, t.sk), |g| (g.pk, g.sk));
    // The key attributes a cursor carries: the read's own, then the table's.
    let mut cursor_keys = vec![pk];
    cursor_keys.extend(sk);
    for k in std::iter::once(t.pk).chain(t.sk) {
        if !cursor_keys.contains(&k) {
            cursor_keys.push(k);
        }
    }

    let ctx = Ctx::from(body);
    let parse = |field: &str| -> Result<Option<Cond>, String> {
        match body.get(field).and_then(|v| v.as_str()) {
            None => Ok(None),
            Some(e) => query::parse(e).map_err(|err| format!("Invalid {}: {}", field, err.message)),
        }
    };
    let key_cond = match parse("KeyConditionExpression") {
        Ok(c) => c,
        Err(m) => return validation(&m),
    };
    let filter = match parse("FilterExpression") {
        Ok(c) => c,
        Err(m) => return validation(&m),
    };
    if is_query && key_cond.is_none() {
        return validation("Either the KeyConditions or KeyConditionExpression parameter must be specified in the request.");
    }

    // The index's items (a sparse index holds only items with its key), in
    // storage order: partition by hash, then sort key.
    let mut rows: Vec<&DdbItem> = t
        .items
        .iter()
        .filter(|i| i.contains_key(pk) && sk.is_none_or(|sk| i.contains_key(sk)))
        .collect();
    rows.sort_by(|a, b| {
        partition_hash(a.get(pk))
            .cmp(&partition_hash(b.get(pk)))
            .then_with(|| key_order(sk.and_then(|k| a.get(k)), sk.and_then(|k| b.get(k))))
            .then_with(|| key_order(a.get(t.pk), b.get(t.pk)))
            .then_with(|| key_order(t.sk.and_then(|k| a.get(k)), t.sk.and_then(|k| b.get(k))))
    });
    if let Some(kc) = &key_cond {
        rows.retain(|i| ctx.eval(kc, i));
        if body.get("ScanIndexForward").and_then(|v| v.as_bool()) == Some(false) {
            rows.reverse();
        }
    }

    // Resume after the cursor's item.
    let start = body
        .get("ExclusiveStartKey")
        .and_then(item_from_ddb_json)
        .and_then(|key| rows.iter().position(|i| key.iter().all(|(k, v)| i.get(k) == Some(v))))
        .map_or(0, |p| p + 1);
    let limit = body.get("Limit").and_then(|v| v.as_u64()).unwrap_or(1_000).max(1) as usize;
    let evaluated: Vec<&DdbItem> = rows.iter().skip(start).take(limit).copied().collect();
    let more = start + evaluated.len() < rows.len();

    let project = |i: &DdbItem| -> DdbItem {
        match index.map(|g| g.projection) {
            Some(Projection::KeysOnly) => i
                .iter()
                .filter(|(k, _)| cursor_keys.contains(&k.as_str()))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            _ => i.clone(),
        }
    };
    let matched: Vec<serde_json::Value> = evaluated
        .iter()
        .filter(|i| filter.as_ref().is_none_or(|f| ctx.eval(f, i)))
        .map(|i| item_ddb_json(&project(i)))
        .collect();

    let mut out = json!({
        "Items": matched,
        "Count": matched.len(),
        "ScannedCount": evaluated.len(),
    });
    if more {
        if let Some(last) = evaluated.last() {
            let key: serde_json::Map<String, serde_json::Value> = cursor_keys
                .iter()
                .filter_map(|k| Some((k.to_string(), av_to_ddb_json(last.get(*k)?))))
                .collect();
            out["LastEvaluatedKey"] = serde_json::Value::Object(key);
        }
    }
    if body.get("ReturnConsumedCapacity").and_then(|v| v.as_str()).is_some_and(|v| v != "NONE") {
        // Eventually consistent: 0.5 per 4 KB evaluated, rounded up per call.
        let bytes: usize = evaluated.iter().map(|i| item_size(i)).sum();
        let units = (bytes as f64 / 4096.0).ceil().max(1.0) * 0.5;
        out["ConsumedCapacity"] = json!({ "TableName": t.name, "CapacityUnits": units });
    }
    out.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_790_000_000;

    fn run(op: &str, body: serde_json::Value) -> serde_json::Value {
        let out = if op == "Query" { read(&body, NOW, true) } else { read(&body, NOW, false) };
        serde_json::from_str(&out).unwrap_or_else(|_| panic!("not JSON: {out}"))
    }

    fn items(v: &serde_json::Value) -> Vec<DdbItem> {
        v["Items"].as_array().unwrap().iter().map(|i| item_from_ddb_json(i).unwrap()).collect()
    }

    fn text(i: &DdbItem, k: &str) -> String {
        match i.get(k) {
            Some(AttributeValue::S(s)) | Some(AttributeValue::N(s)) => s.clone(),
            _ => String::new(),
        }
    }

    #[test]
    fn the_dataset_is_repeatable_and_tells_its_story() {
        let a = tables(NOW);
        let b = tables(NOW);
        assert_eq!(a[0].items, b[0].items);
        let orders = &a[0].items;
        assert!(orders.len() > 150, "{} items", orders.len());
        let strings = orders.iter().filter(|i| matches!(i.get("total"), Some(AttributeValue::S(_)))).count();
        assert_eq!(strings, LEGACY_ORDERS.len(), "every legacy id exists and is planted");
        assert!(orders.iter().any(|i| i.contains_key("expiresAt")), "some cancelled orders");
        let user42 = orders.iter().filter(|i| text(i, "pk") == "USER#42").count();
        assert_eq!(user42, 7, "the example user: a profile and six orders");
    }

    #[test]
    fn query_reads_one_partition_in_sort_key_order_and_filters_after() {
        let v = run(
            "Query",
            json!({
                "TableName": "orders",
                "KeyConditionExpression": "#n0 = :v0 AND begins_with(#n1, :v1)",
                "FilterExpression": "#n2 > :v2",
                "ExpressionAttributeNames": {"#n0": "pk", "#n1": "sk", "#n2": "total"},
                "ExpressionAttributeValues": {":v0": {"S": "USER#42"}, ":v1": {"S": "ORDER#"}, ":v2": {"N": "100"}},
            }),
        );
        assert_eq!(v["ScannedCount"], 6, "the key condition picks six orders; the filter runs after");
        let got = items(&v);
        assert!(got.iter().all(|i| text(i, "pk") == "USER#42"));
        assert!(got.iter().all(|i| text(i, "total").parse::<f64>().unwrap() > 100.0));
        let sks: Vec<String> = got.iter().map(|i| text(i, "sk")).collect();
        let mut sorted = sks.clone();
        sorted.sort();
        assert_eq!(sks, sorted);
    }

    #[test]
    fn a_number_never_matches_a_string_total() {
        let v = run(
            "Scan",
            json!({
                "TableName": "orders",
                "FilterExpression": "#n0 = :v0",
                "ExpressionAttributeNames": {"#n0": "source"},
                "ExpressionAttributeValues": {":v0": {"S": "legacy-import"}},
            }),
        );
        let legacy = items(&v);
        assert_eq!(legacy.len(), 3);
        let v = run(
            "Scan",
            json!({
                "TableName": "orders",
                "FilterExpression": "#n0 >= :v0 AND #n1 = :v1",
                "ExpressionAttributeNames": {"#n0": "total", "#n1": "source"},
                "ExpressionAttributeValues": {":v0": {"N": "0"}, ":v1": {"S": "legacy-import"}},
            }),
        );
        assert!(items(&v).is_empty(), "`total >= 0` misses the string totals, as in AWS");
    }

    #[test]
    fn pages_resume_from_the_cursor_without_gaps_or_repeats() {
        let mut seen = Vec::new();
        let mut cursor: Option<serde_json::Value> = None;
        loop {
            let mut body = json!({ "TableName": "orders", "Limit": 50 });
            if let Some(c) = cursor.take() {
                body["ExclusiveStartKey"] = c;
            }
            let v = run("Scan", body);
            assert!(v["ScannedCount"].as_u64().unwrap() <= 50);
            seen.extend(items(&v).into_iter().map(|i| (text(&i, "pk"), text(&i, "sk"))));
            match v.get("LastEvaluatedKey") {
                Some(k) => cursor = Some(k.clone()),
                None => break,
            }
        }
        let total = tables(NOW)[0].items.len();
        assert_eq!(seen.len(), total);
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), total);
    }

    #[test]
    fn a_sparse_keys_only_gsi_returns_keys_only() {
        let v = run(
            "Query",
            json!({
                "TableName": "orders",
                "IndexName": "byEmail",
                "KeyConditionExpression": "#n0 = :v0",
                "ExpressionAttributeNames": {"#n0": "email"},
                "ExpressionAttributeValues": {":v0": {"S": "nope@example.com"}},
            }),
        );
        assert_eq!(v["Count"], 0);
        let v = run("Scan", json!({ "TableName": "orders", "IndexName": "byEmail", "Limit": 5 }));
        let got = items(&v);
        assert_eq!(got.len(), 5);
        let mut keys: Vec<&String> = got[0].keys().collect();
        keys.sort();
        assert_eq!(keys, ["email", "pk", "sk"]);
        // byStatus holds orders only.
        let v = run("Scan", json!({ "TableName": "orders", "IndexName": "byStatus" }));
        assert!(items(&v).iter().all(|i| text(i, "type") == "ORDER"));
        assert!(v.get("LastEvaluatedKey").is_none());
    }

    #[test]
    fn errors_look_like_dynamodb_errors() {
        let missing = read(&json!({ "TableName": "nope" }), NOW, false);
        assert!(missing.starts_with("!status 400") && missing.contains("ResourceNotFoundException"));
        let bad = read(&json!({ "TableName": "orders", "IndexName": "x" }), NOW, false);
        assert!(bad.contains("ValidationException") && bad.contains("specified index"));
        let no_key = read(&json!({ "TableName": "orders" }), NOW, true);
        assert!(no_key.contains("KeyConditionExpression"));
    }

    #[test]
    fn consumed_capacity_only_when_asked() {
        let v = run("Scan", json!({ "TableName": "catalog" }));
        assert!(v.get("ConsumedCapacity").is_none());
        let v = run("Scan", json!({ "TableName": "catalog", "ReturnConsumedCapacity": "TOTAL" }));
        assert!(v["ConsumedCapacity"]["CapacityUnits"].as_f64().unwrap() >= 0.5);
    }

    #[test]
    fn describe_table_counts_what_a_scan_finds() {
        let v: serde_json::Value = serde_json::from_str(&describe_table(&json!({ "TableName": "orders" }), NOW)).unwrap();
        let t = &v["Table"];
        assert_eq!(t["ItemCount"].as_u64().unwrap() as usize, tables(NOW)[0].items.len());
        assert_eq!(t["GlobalSecondaryIndexes"][1]["Projection"]["ProjectionType"], "KEYS_ONLY");
        assert_eq!(t["ProvisionedThroughput"]["WriteCapacityUnits"], 5);
    }
}
