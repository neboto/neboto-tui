use crate::aws::client::AwsClients;
use crate::aws::resource::{native_state_label, Resource, ResourceState};
use crate::aws::service::{AwsService, ServiceType};
use crate::error::Result;
use crate::event::{Event, LoadProgress};
use async_trait::async_trait;
use aws_sdk_dynamodb::types::AttributeValue;
use aws_sdk_dynamodb::Client as DdbClient;
use std::any::Any;
use std::collections::HashMap;
use tokio::sync::mpsc;


pub struct DynamoDbService {
    client: DdbClient,
}

impl DynamoDbService {
    pub fn new(aws_clients: &AwsClients) -> Self {
        Self {
            client: aws_clients.dynamodb_client(),
        }
    }
}

#[async_trait]
impl AwsService for DynamoDbService {
    fn service_type(&self) -> ServiceType {
        ServiceType::DynamoDb
    }

    fn name(&self) -> &str {
        "DynamoDB"
    }

    async fn list_resources(&self) -> Result<Vec<Box<dyn Resource>>> {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        self.list_resources_streaming(tx, ServiceType::DynamoDb).await?;
        Ok(vec![])
    }

    async fn list_resources_streaming(
        &self,
        event_tx: mpsc::UnboundedSender<Event>,
        service_type: ServiceType,
    ) -> Result<()> {
        // Phase 1: list_tables is names-only (paginated).
        let mut names: Vec<String> = Vec::new();
        let mut start: Option<String> = None;
        loop {
            let mut req = self.client.list_tables();
            if let Some(s) = &start {
                req = req.exclusive_start_table_name(s);
            }
            match req.send().await {
                Ok(resp) => {
                    names.extend(resp.table_names().iter().cloned());
                    start = crate::aws::pagination::next_page_token(resp.last_evaluated_table_name(), &start);
                    if start.is_none() {
                        break;
                    }
                }
                Err(e) => {
                    let _ = event_tx.send(Event::ResourceLoadError {
                        service: service_type,
                        error: friendly_error(&crate::error::sdk_error_message(&e)),
                    });
                    return Ok(());
                }
            }
        }

        // Phase 2: describe_table per table, bounded concurrency.
        let mut total = 0usize;
        for chunk in names.chunks(8) {
            let futs = chunk.iter().map(|name| {
                let client = self.client.clone();
                let name = name.clone();
                async move {
                    client
                        .describe_table()
                        .table_name(&name)
                        .send()
                        .await
                        .ok()
                        .and_then(|r| r.table)
                        .map(|t| DdbTable::from_sdk(&t))
                }
            });
            let batch: Vec<Box<dyn Resource>> = futures::future::join_all(futs)
                .await
                .into_iter()
                .flatten()
                .map(|t| Box::new(t) as Box<dyn Resource>)
                .collect();
            let count = batch.len();
            if count > 0 {
                total += count;
                let _ = event_tx.send(Event::ResourcesPartiallyLoaded {
                    service: service_type,
                    resources: batch,
                    progress: LoadProgress {
                        loaded_count: total,
                        total_count: Some(names.len()),
                        status_message: None,
                    },
                });
            }
        }

        let _ = event_tx.send(Event::ResourcesFullyLoaded {
            service: service_type,
            total_count: total,
        });
        Ok(())
    }

    async fn get_resource_details(&self, _id: &str) -> Result<Box<dyn Resource>> {
        Err(crate::error::Error::NotImplemented)
    }
}

fn friendly_error(raw: &str) -> String {
    let low = raw.to_lowercase();
    if low.contains("expiredtoken")
        || low.contains("expired")
        || low.contains("unable to locate credentials")
        || low.contains("the security token")
    {
        "AWS credentials are expired or missing — refresh them and reload (r).".to_string()
    } else if low.contains("accessdenied") || low.contains("not authorized") {
        "Access denied — need dynamodb:ListTables and dynamodb:DescribeTable.".to_string()
    } else {
        format!("Failed to load DynamoDB tables: {}", raw)
    }
}

// ── Key-schema helper ─────────────────────────────────────────────────────────

/// Pull (pk_name, pk_type, sk_name, sk_type) out of a key schema + attribute
/// type map. Types are the DynamoDB scalar codes: S / N / B.
fn key_pair(
    schema: &[aws_sdk_dynamodb::types::KeySchemaElement],
    types: &HashMap<String, String>,
) -> (String, String, Option<String>, Option<String>) {
    let mut pk = String::new();
    let mut pk_t = "S".to_string();
    let mut sk = None;
    let mut sk_t = None;
    for k in schema {
        let name = k.attribute_name().to_string();
        let ty = types.get(&name).cloned().unwrap_or_else(|| "S".to_string());
        match k.key_type() {
            aws_sdk_dynamodb::types::KeyType::Hash => {
                pk = name;
                pk_t = ty;
            }
            aws_sdk_dynamodb::types::KeyType::Range => {
                sk = Some(name);
                sk_t = Some(ty);
            }
            _ => {}
        }
    }
    (pk, pk_t, sk, sk_t)
}

// ── DdbTable ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct DdbIndex {
    pub name: String,
    pub keys: String,       // "pk (HASH) / sk (RANGE)" display
    pub projection: String, // ALL / KEYS_ONLY / INCLUDE(...)
    pub status: String,
    pub read_capacity: Option<i64>,
    pub write_capacity: Option<i64>,
    // For querying the index:
    pub pk_name: String,
    pub pk_type: String,
    pub sk_name: Option<String>,
    pub sk_type: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DdbTable {
    pub name: String,
    pub arn: String,
    pub status: String,
    pub item_count: i64,
    pub size_bytes: i64,
    pub billing_mode: String, // PROVISIONED / PAY_PER_REQUEST
    pub read_capacity: Option<i64>,
    pub write_capacity: Option<i64>,
    pub partition_key: String,
    pub partition_key_type: String,
    pub sort_key: Option<String>,
    pub sort_key_type: Option<String>,
    pub gsis: Vec<DdbIndex>,
    pub lsis: Vec<DdbIndex>,
    pub stream_enabled: bool,
    pub stream_view_type: Option<String>,
    pub created: Option<String>,
    pub tags: HashMap<String, String>,
}

impl DdbTable {
    pub fn from_sdk(t: &aws_sdk_dynamodb::types::TableDescription) -> Self {
        let types: HashMap<String, String> = t
            .attribute_definitions()
            .iter()
            .map(|a| (a.attribute_name().to_string(), a.attribute_type().as_str().to_string()))
            .collect();

        let (partition_key, partition_key_type, sort_key, sort_key_type) =
            key_pair(t.key_schema(), &types);

        let billing_mode = t
            .billing_mode_summary()
            .and_then(|b| b.billing_mode())
            .map(|m| m.as_str().to_string())
            .unwrap_or_else(|| "PROVISIONED".to_string());

        let pt = t.provisioned_throughput();
        let read_capacity = pt.and_then(|p| p.read_capacity_units()).filter(|v| *v > 0);
        let write_capacity = pt.and_then(|p| p.write_capacity_units()).filter(|v| *v > 0);

        let mk_index = |name: String,
                        schema: &[aws_sdk_dynamodb::types::KeySchemaElement],
                        proj: Option<&aws_sdk_dynamodb::types::Projection>,
                        status: String,
                        rcu: Option<i64>,
                        wcu: Option<i64>|
         -> DdbIndex {
            let (pk_name, pk_type, sk_name, sk_type) = key_pair(schema, &types);
            let keys = match &sk_name {
                Some(sk) => format!("{} (HASH) / {} (RANGE)", pk_name, sk),
                None => format!("{} (HASH)", pk_name),
            };
            let projection = proj
                .map(|p| {
                    let kind = p
                        .projection_type()
                        .map(|t| t.as_str().to_string())
                        .unwrap_or_default();
                    if kind == "INCLUDE" {
                        format!("INCLUDE({})", p.non_key_attributes().join(", "))
                    } else {
                        kind
                    }
                })
                .unwrap_or_default();
            DdbIndex {
                name,
                keys,
                projection,
                status,
                read_capacity: rcu,
                write_capacity: wcu,
                pk_name,
                pk_type,
                sk_name,
                sk_type,
            }
        };

        let gsis = t
            .global_secondary_indexes()
            .iter()
            .map(|g| {
                let pt = g.provisioned_throughput();
                mk_index(
                    g.index_name().unwrap_or_default().to_string(),
                    g.key_schema(),
                    g.projection(),
                    g.index_status().map(|s| s.as_str().to_string()).unwrap_or_default(),
                    pt.and_then(|p| p.read_capacity_units()).filter(|v| *v > 0),
                    pt.and_then(|p| p.write_capacity_units()).filter(|v| *v > 0),
                )
            })
            .collect();

        let lsis = t
            .local_secondary_indexes()
            .iter()
            .map(|l| {
                mk_index(
                    l.index_name().unwrap_or_default().to_string(),
                    l.key_schema(),
                    l.projection(),
                    String::new(),
                    None,
                    None,
                )
            })
            .collect();

        let stream = t.stream_specification();
        Self {
            name: t.table_name().unwrap_or_default().to_string(),
            arn: t.table_arn().unwrap_or_default().to_string(),
            status: t.table_status().map(|s| s.as_str().to_string()).unwrap_or_default(),
            item_count: t.item_count().unwrap_or(0),
            size_bytes: t.table_size_bytes().unwrap_or(0),
            billing_mode,
            read_capacity,
            write_capacity,
            partition_key,
            partition_key_type,
            sort_key,
            sort_key_type,
            gsis,
            lsis,
            stream_enabled: stream.map(|s| s.stream_enabled()).unwrap_or(false),
            stream_view_type: stream
                .and_then(|s| s.stream_view_type())
                .map(|v| v.as_str().to_string()),
            created: t.creation_date_time().map(|d| fmt_epoch_secs(d.secs())),
            tags: HashMap::new(),
        }
    }

    /// The table + each GSI as a queryable index. The first entry (`(table)`)
    /// has an empty `name` meaning the base table (no IndexName).
    pub fn queryable_indexes(&self) -> Vec<DdbIndex> {
        let mut out = vec![DdbIndex {
            name: String::new(),
            keys: String::new(),
            projection: String::new(),
            status: String::new(),
            read_capacity: None,
            write_capacity: None,
            pk_name: self.partition_key.clone(),
            pk_type: self.partition_key_type.clone(),
            sk_name: self.sort_key.clone(),
            sk_type: self.sort_key_type.clone(),
        }];
        out.extend(self.gsis.iter().cloned());
        out.extend(self.lsis.iter().cloned());
        out
    }
}

crate::sections! {
    pub enum DdbTableDetailSection,
    pub static DDB_TABLE_SECTIONS = [
        Overview "Overview" => crate::app::App::trigger_ddb_ttl_load,
        Indexes "Indexes",
        Capacity "Capacity",
        Tags "Tags" => crate::app::App::trigger_ddb_tags_load,
    ]
}

impl Resource for DdbTable {
    fn detail_sections(&self) -> Option<&'static crate::sections::SectionDescriptor> {
        Some(&DDB_TABLE_SECTIONS)
    }
    fn cli_command(&self) -> Option<String> {
        Some(format!(
            "aws dynamodb describe-table --table-name {}",
            crate::aws::resource::shell_quote(self.id())
        ))
    }

    fn id(&self) -> &str {
        &self.name
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn resource_type(&self) -> &str {
        "DynamoDB Table"
    }
    fn state(&self) -> ResourceState {
        match self.status.as_str() {
            "ACTIVE" => ResourceState::Available,
            "CREATING" | "UPDATING" => ResourceState::Pending,
            "DELETING" => ResourceState::Deleting,
            "INACCESSIBLE_ENCRYPTION_CREDENTIALS" | "ARCHIVING" | "ARCHIVED" => {
                ResourceState::Unavailable
            }
            other => ResourceState::Unknown(other.to_string()),
        }
    }

    fn state_label(&self) -> String {
        native_state_label(&self.status, || self.state())
    }
    fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }
    fn search_text(&self) -> String {
        format!("{} {} {}", self.name, self.partition_key, self.billing_mode)
    }
    fn details(&self) -> Vec<(String, String)> {
        vec![
            ("Name".to_string(), self.name.clone()),
            ("Status".to_string(), self.status.clone()),
            ("Billing Mode".to_string(), self.billing_mode.clone()),
            ("Items (approx)".to_string(), self.item_count.to_string()),
            ("Partition Key".to_string(), self.partition_key.clone()),
            (
                "Sort Key".to_string(),
                self.sort_key.clone().unwrap_or_else(|| "—".to_string()),
            ),
        ]
    }
    fn console_url(&self, region: &str) -> Option<String> {
        Some(format!(
            "https://{region}.console.aws.amazon.com/dynamodbv2/home?region={region}#table?name={}",
            self.name
        ))
    }
    fn clone_box(&self) -> Box<dyn Resource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── Lazy TTL + tags ───────────────────────────────────────────────────────────

pub async fn fetch_ddb_ttl(client: DdbClient, table: String) -> Result<(bool, Option<String>)> {
    let resp = client
        .describe_time_to_live()
        .table_name(&table)
        .send()
        .await
        .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
    let d = resp.time_to_live_description();
    let status = d
        .and_then(|d| d.time_to_live_status())
        .map(|s| s.as_str().to_string())
        .unwrap_or_default();
    let enabled = status == "ENABLED" || status == "ENABLING";
    let attribute = d.and_then(|d| d.attribute_name()).map(|s| s.to_string());
    Ok((enabled, attribute))
}

pub async fn fetch_ddb_tags(client: DdbClient, arn: String) -> Result<HashMap<String, String>> {
    let mut tags = HashMap::new();
    let mut next: Option<String> = None;
    loop {
        let mut req = client.list_tags_of_resource().resource_arn(&arn);
        if let Some(t) = &next {
            req = req.next_token(t);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| crate::error::Error::AwsSdk(crate::error::sdk_error_message(&e)))?;
        for t in resp.tags() {
            tags.insert(t.key().to_string(), t.value().to_string());
        }
        next = crate::aws::pagination::next_page_token(resp.next_token(), &next);
        if next.is_none() {
            break;
        }
    }
    Ok(tags)
}

// ── AttributeValue → JSON (item browser) ──────────────────────────────────────

/// An item as the browser holds it.
pub type DdbItem = HashMap<String, AttributeValue>;

/// A DynamoDB number as JSON: a real number when it survives the round trip
/// (`42`, `1.5`), else the original string, so `12345678901234567890.123`
/// keeps every digit instead of being rounded through an `f64`.
fn number_json(n: &str) -> serde_json::Value {
    use serde_json::Value;
    if let Ok(i) = n.parse::<i64>() {
        return Value::from(i);
    }
    if let Ok(u) = n.parse::<u64>() {
        return Value::from(u);
    }
    match n.parse::<f64>().ok().and_then(serde_json::Number::from_f64) {
        Some(num) if num.to_string() == n => Value::Number(num),
        _ => Value::String(n.to_string()),
    }
}

/// Map entries in a stable order (DynamoDB returns them unordered).
fn sorted_entries(m: &HashMap<String, AttributeValue>) -> Vec<(&String, &AttributeValue)> {
    let mut v: Vec<_> = m.iter().collect();
    v.sort_by(|a, b| a.0.cmp(b.0));
    v
}

/// Convert a DynamoDB `AttributeValue` into a `serde_json::Value` for display.
/// Numbers become JSON numbers where that loses nothing (see `number_json`);
/// binary is shown as a `<binary N bytes>` placeholder.
pub fn av_to_json(av: &AttributeValue) -> serde_json::Value {
    use serde_json::Value;
    match av {
        AttributeValue::S(s) => Value::String(s.clone()),
        AttributeValue::N(n) => number_json(n),
        AttributeValue::Bool(b) => Value::Bool(*b),
        AttributeValue::Null(_) => Value::Null,
        AttributeValue::M(m) => Value::Object(
            sorted_entries(m)
                .into_iter()
                .map(|(k, v)| (k.clone(), av_to_json(v)))
                .collect(),
        ),
        AttributeValue::L(l) => Value::Array(l.iter().map(av_to_json).collect()),
        AttributeValue::Ss(ss) => Value::Array(ss.iter().map(|s| Value::String(s.clone())).collect()),
        AttributeValue::Ns(ns) => Value::Array(ns.iter().map(|n| number_json(n)).collect()),
        AttributeValue::B(b) => Value::String(format!("<binary {} bytes>", b.as_ref().len())),
        AttributeValue::Bs(bs) => Value::Array(
            bs.iter()
                .map(|b| Value::String(format!("<binary {} bytes>", b.as_ref().len())))
                .collect(),
        ),
        _ => Value::String("<unknown>".to_string()),
    }
}

/// The item in DynamoDB's own typed JSON (`{"S": "x"}`, `{"N": "1"}`), as
/// `aws dynamodb get-item` prints it — what you paste into a `--key` or
/// `--item`. Binary is base64, as on the wire.
pub fn av_to_ddb_json(av: &AttributeValue) -> serde_json::Value {
    use serde_json::{json, Value};
    let b64 = |b: &aws_sdk_dynamodb::primitives::Blob| aws_smithy_types::base64::encode(b.as_ref());
    match av {
        AttributeValue::S(s) => json!({ "S": s }),
        AttributeValue::N(n) => json!({ "N": n }),
        AttributeValue::Bool(b) => json!({ "BOOL": b }),
        AttributeValue::Null(_) => json!({ "NULL": true }),
        AttributeValue::M(m) => json!({ "M": item_ddb_json(m) }),
        AttributeValue::L(l) => json!({ "L": l.iter().map(av_to_ddb_json).collect::<Vec<_>>() }),
        AttributeValue::Ss(ss) => json!({ "SS": ss }),
        AttributeValue::Ns(ns) => json!({ "NS": ns }),
        AttributeValue::B(b) => json!({ "B": b64(b) }),
        AttributeValue::Bs(bs) => json!({ "BS": bs.iter().map(b64).collect::<Vec<_>>() }),
        _ => Value::Null,
    }
}

/// `av_to_ddb_json` over a whole item, keys in stable order.
pub fn item_ddb_json(m: &HashMap<String, AttributeValue>) -> serde_json::Value {
    serde_json::Value::Object(
        sorted_entries(m)
            .into_iter()
            .map(|(k, v)| (k.clone(), av_to_ddb_json(v)))
            .collect(),
    )
}

/// The DynamoDB type descriptor (`S`, `N`, `M`, …) the item view labels a row with.
pub fn av_type(av: &AttributeValue) -> &'static str {
    match av {
        AttributeValue::S(_) => "S",
        AttributeValue::N(_) => "N",
        AttributeValue::Bool(_) => "BOOL",
        AttributeValue::Null(_) => "NULL",
        AttributeValue::M(_) => "M",
        AttributeValue::L(_) => "L",
        AttributeValue::Ss(_) => "SS",
        AttributeValue::Ns(_) => "NS",
        AttributeValue::B(_) => "B",
        AttributeValue::Bs(_) => "BS",
        _ => "?",
    }
}

/// Past this many characters a nested preview stops descending; the cell
/// truncates to its column anyway, this just bounds the work per cell.
const PREVIEW_BUDGET: usize = 120;

/// Compact single-cell rendering of an attribute for the results table.
/// Nested values preview their contents (`{city: Perth, zip: 6000}`,
/// `[a, b, c]`) rather than just a count.
pub fn av_cell(av: &AttributeValue) -> String {
    let mut out = String::new();
    preview(av, &mut out);
    out
}

fn preview(av: &AttributeValue, out: &mut String) {
    fn seq<'a, T: 'a>(
        out: &mut String,
        open: char,
        close: char,
        items: impl ExactSizeIterator<Item = T>,
        mut each: impl FnMut(T, &mut String),
    ) {
        out.push(open);
        for (i, it) in items.enumerate() {
            if out.len() >= PREVIEW_BUDGET {
                out.push('…');
                break;
            }
            if i > 0 {
                out.push_str(", ");
            }
            each(it, out);
        }
        out.push(close);
    }
    match av {
        AttributeValue::S(s) => out.push_str(s),
        AttributeValue::N(n) => out.push_str(n),
        AttributeValue::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        AttributeValue::Null(_) => out.push_str("null"),
        AttributeValue::M(m) => {
            let entries = sorted_entries(m);
            seq(out, '{', '}', entries.into_iter(), |(k, v), out| {
                out.push_str(k);
                out.push_str(": ");
                preview(v, out);
            })
        }
        AttributeValue::L(l) => seq(out, '[', ']', l.iter(), preview),
        AttributeValue::Ss(s) => seq(out, '[', ']', s.iter(), |s, out| out.push_str(s)),
        AttributeValue::Ns(n) => seq(out, '[', ']', n.iter(), |n, out| out.push_str(n)),
        AttributeValue::B(b) => out.push_str(&format!("<{} bytes>", b.as_ref().len())),
        AttributeValue::Bs(bs) => out.push_str(&format!("<{} binaries>", bs.len())),
        _ => {}
    }
}

/// Order two attribute values for the browser's column sort: numbers
/// numerically, everything else by its cell text. A missing value sorts
/// last either way (the caller handles direction).
pub fn av_cmp(a: &AttributeValue, b: &AttributeValue) -> std::cmp::Ordering {
    if let (AttributeValue::N(x), AttributeValue::N(y)) = (a, b) {
        if let (Ok(x), Ok(y)) = (x.parse::<f64>(), y.parse::<f64>()) {
            return x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal);
        }
    }
    av_cell(a).cmp(&av_cell(b))
}

// ── Query / Scan execution ────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkOp {
    Eq,
    BeginsWith,
    Lt,
    Le,
    Gt,
    Ge,
}

impl SkOp {
    pub fn label(self) -> &'static str {
        match self {
            SkOp::Eq => "=",
            SkOp::BeginsWith => "begins_with",
            SkOp::Lt => "<",
            SkOp::Le => "<=",
            SkOp::Gt => ">",
            SkOp::Ge => ">=",
        }
    }
    pub fn next(self) -> Self {
        match self {
            SkOp::Eq => SkOp::BeginsWith,
            SkOp::BeginsWith => SkOp::Lt,
            SkOp::Lt => SkOp::Le,
            SkOp::Le => SkOp::Gt,
            SkOp::Gt => SkOp::Ge,
            SkOp::Ge => SkOp::Eq,
        }
    }
}

/// One page of an item read, plus the cursor to fetch the next page.
pub struct DdbPage {
    pub items: Vec<HashMap<String, AttributeValue>>,
    pub last_key: Option<HashMap<String, AttributeValue>>,
    pub scanned: i32,
}

/// Build a typed `AttributeValue` from user text + the DynamoDB scalar type.
fn make_av(value: &str, ty: &str) -> AttributeValue {
    match ty {
        "N" => AttributeValue::N(value.to_string()),
        "B" => AttributeValue::S(value.to_string()), // binary literals unsupported; treat as string
        _ => AttributeValue::S(value.to_string()),
    }
}

/// Parse a simple filter `attr=value` (equals) or `attr~value` (contains) into
/// a FilterExpression + names/values. Numeric values use `N`.
fn build_filter(
    filter: &str,
    names: &mut HashMap<String, String>,
    values: &mut HashMap<String, AttributeValue>,
) -> Option<String> {
    let filter = filter.trim();
    if filter.is_empty() {
        return None;
    }
    let (attr, op_contains, val) = if let Some((a, v)) = filter.split_once('~') {
        (a.trim(), true, v.trim())
    } else if let Some((a, v)) = filter.split_once('=') {
        (a.trim(), false, v.trim())
    } else {
        return None;
    };
    if attr.is_empty() {
        return None;
    }
    names.insert("#f".to_string(), attr.to_string());
    // Numeric literal → N, else S.
    let av = if val.parse::<f64>().is_ok() {
        AttributeValue::N(val.to_string())
    } else {
        AttributeValue::S(val.to_string())
    };
    values.insert(":fval".to_string(), av);
    if op_contains {
        Some("contains(#f, :fval)".to_string())
    } else {
        Some("#f = :fval".to_string())
    }
}

/// Full spec for one item read (built from the browser state on `App`).
pub struct DdbQuerySpec {
    pub table: String,
    pub index_name: Option<String>, // None = base table
    pub query: bool,                // true = Query, false = Scan
    pub pk_name: String,
    pub pk_type: String,
    pub pk_value: String,
    pub sk_name: Option<String>,
    pub sk_type: Option<String>,
    pub sk_op: SkOp,
    pub sk_value: String,
    pub filter: String,
    pub start_key: Option<HashMap<String, AttributeValue>>,
}

const PAGE_LIMIT: i32 = 50;

pub async fn run_ddb_read(client: DdbClient, spec: DdbQuerySpec) -> Result<DdbPage> {
    let mut names: HashMap<String, String> = HashMap::new();
    let mut values: HashMap<String, AttributeValue> = HashMap::new();

    if spec.query {
        // Key condition: #pk = :pkval [AND <sk op>]
        names.insert("#pk".to_string(), spec.pk_name.clone());
        values.insert(":pkval".to_string(), make_av(&spec.pk_value, &spec.pk_type));
        let mut cond = "#pk = :pkval".to_string();

        if let (Some(sk_name), Some(sk_type)) = (&spec.sk_name, &spec.sk_type) {
            if !spec.sk_value.trim().is_empty() {
                names.insert("#sk".to_string(), sk_name.clone());
                values.insert(":skval".to_string(), make_av(spec.sk_value.trim(), sk_type));
                let clause = match spec.sk_op {
                    SkOp::Eq => "#sk = :skval",
                    SkOp::BeginsWith => "begins_with(#sk, :skval)",
                    SkOp::Lt => "#sk < :skval",
                    SkOp::Le => "#sk <= :skval",
                    SkOp::Gt => "#sk > :skval",
                    SkOp::Ge => "#sk >= :skval",
                };
                cond.push_str(" AND ");
                cond.push_str(clause);
            }
        }

        let filter = build_filter(&spec.filter, &mut names, &mut values);

        let mut req = client
            .query()
            .table_name(&spec.table)
            .key_condition_expression(cond)
            .limit(PAGE_LIMIT)
            .set_expression_attribute_names(Some(names))
            .set_expression_attribute_values(Some(values));
        if let Some(idx) = &spec.index_name {
            req = req.index_name(idx);
        }
        if let Some(f) = filter {
            req = req.filter_expression(f);
        }
        if let Some(sk) = spec.start_key {
            req = req.set_exclusive_start_key(Some(sk));
        }

        let resp = req
            .send()
            .await
            .map_err(|e| crate::error::Error::AwsSdk(friendly_query_error(&crate::error::sdk_error_message(&e))))?;
        Ok(DdbPage {
            items: resp.items().to_vec(),
            last_key: resp.last_evaluated_key().cloned(),
            scanned: resp.scanned_count(),
        })
    } else {
        let filter = build_filter(&spec.filter, &mut names, &mut values);
        let mut req = client.scan().table_name(&spec.table).limit(PAGE_LIMIT);
        if let Some(idx) = &spec.index_name {
            req = req.index_name(idx);
        }
        if let Some(f) = filter {
            req = req
                .filter_expression(f)
                .set_expression_attribute_names(Some(names))
                .set_expression_attribute_values(Some(values));
        }
        if let Some(sk) = spec.start_key {
            req = req.set_exclusive_start_key(Some(sk));
        }
        let resp = req
            .send()
            .await
            .map_err(|e| crate::error::Error::AwsSdk(friendly_query_error(&crate::error::sdk_error_message(&e))))?;
        Ok(DdbPage {
            items: resp.items().to_vec(),
            last_key: resp.last_evaluated_key().cloned(),
            scanned: resp.scanned_count(),
        })
    }
}

fn friendly_query_error(raw: &str) -> String {
    let low = raw.to_lowercase();
    if low.contains("validationexception") && low.contains("key condition") {
        "Query needs a partition-key value (or switch to Scan).".to_string()
    } else if low.contains("accessdenied") || low.contains("not authorized") {
        "Access denied — need dynamodb:Query / dynamodb:Scan.".to_string()
    } else {
        raw.to_string()
    }
}

// ── Metrics (AWS/DynamoDB) ────────────────────────────────────────────────────

// ── Timestamp helpers ─────────────────────────────────────────────────────────

fn fmt_epoch_secs(secs: i64) -> String {
    let days = secs / 86400;
    let rem = secs % 86400;
    let (h, mi) = (rem / 3600, (rem % 3600) / 60);
    let (y, mo, d) = epoch_days_to_ymd(days);
    format!("{:04}-{:02}-{:02} {:02}:{:02} UTC", y, mo, d, h, mi)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_filter_equals_and_contains() {
        let mut names = HashMap::new();
        let mut values = HashMap::new();
        let expr = build_filter("status=active", &mut names, &mut values).unwrap();
        assert_eq!(expr, "#f = :fval");
        assert_eq!(names.get("#f").unwrap(), "status");
        assert!(matches!(values.get(":fval"), Some(AttributeValue::S(s)) if s == "active"));

        let mut names = HashMap::new();
        let mut values = HashMap::new();
        let expr = build_filter("name~smith", &mut names, &mut values).unwrap();
        assert_eq!(expr, "contains(#f, :fval)");
    }

    #[test]
    fn build_filter_numeric_value_uses_n() {
        let mut names = HashMap::new();
        let mut values = HashMap::new();
        build_filter("age=42", &mut names, &mut values).unwrap();
        assert!(matches!(values.get(":fval"), Some(AttributeValue::N(n)) if n == "42"));
    }

    #[test]
    fn build_filter_empty_or_malformed_is_none() {
        let mut n = HashMap::new();
        let mut v = HashMap::new();
        assert!(build_filter("", &mut n, &mut v).is_none());
        assert!(build_filter("noseparator", &mut n, &mut v).is_none());
    }

    #[test]
    fn av_to_json_handles_scalars_and_nesting() {
        use serde_json::json;
        let mut m = HashMap::new();
        m.insert("s".to_string(), AttributeValue::S("hi".into()));
        m.insert("n".to_string(), AttributeValue::N("3".into()));
        m.insert("b".to_string(), AttributeValue::Bool(true));
        m.insert(
            "list".to_string(),
            AttributeValue::L(vec![AttributeValue::S("x".into())]),
        );
        let v = av_to_json(&AttributeValue::M(m));
        assert_eq!(v["n"], json!(3));
        assert_eq!(v["s"], json!("hi"));
        assert_eq!(v["b"], json!(true));
        assert_eq!(v["list"], json!(["x"]));
    }

    #[test]
    fn numbers_keep_their_precision() {
        use serde_json::json;
        assert_eq!(av_to_json(&AttributeValue::N("1.5".into())), json!(1.5));
        assert_eq!(av_to_json(&AttributeValue::N("-7".into())), json!(-7));
        // Too precise for an f64: stays the exact string.
        let big = "12345678901234567890.123";
        assert_eq!(av_to_json(&AttributeValue::N(big.into())), json!(big));
    }

    #[test]
    fn cells_preview_nested_values_in_stable_order() {
        let mut m = HashMap::new();
        m.insert("zip".to_string(), AttributeValue::S("6000".into()));
        m.insert("city".to_string(), AttributeValue::S("Perth".into()));
        assert_eq!(av_cell(&AttributeValue::M(m)), "{city: Perth, zip: 6000}");
        let l = AttributeValue::L(vec![AttributeValue::N("1".into()), AttributeValue::Bool(true)]);
        assert_eq!(av_cell(&l), "[1, true]");
        let long = AttributeValue::L((0..200).map(|i| AttributeValue::N(i.to_string())).collect());
        let cell = av_cell(&long);
        assert!(cell.len() < 200 && cell.ends_with("…]"), "{cell}");
    }

    #[test]
    fn ddb_json_is_typed_like_the_cli() {
        use serde_json::json;
        let mut item = HashMap::new();
        item.insert("pk".to_string(), AttributeValue::S("p".into()));
        item.insert("n".to_string(), AttributeValue::N("3".into()));
        item.insert("tags".to_string(), AttributeValue::Ss(vec!["a".into()]));
        item.insert("bin".to_string(), AttributeValue::B(aws_sdk_dynamodb::primitives::Blob::new(b"hi".to_vec())));
        assert_eq!(
            item_ddb_json(&item),
            json!({"bin": {"B": "aGk="}, "n": {"N": "3"}, "pk": {"S": "p"}, "tags": {"SS": ["a"]}})
        );
    }

    #[test]
    fn sort_compares_numbers_numerically() {
        let n = |v: &str| AttributeValue::N(v.into());
        assert_eq!(av_cmp(&n("9"), &n("100")), std::cmp::Ordering::Less);
        let s = |v: &str| AttributeValue::S(v.into());
        assert_eq!(av_cmp(&s("9"), &s("100")), std::cmp::Ordering::Greater);
    }

    #[test]
    fn sk_op_cycles_and_labels() {
        assert_eq!(SkOp::Eq.label(), "=");
        assert_eq!(SkOp::BeginsWith.label(), "begins_with");
        // Cycling visits every variant and returns to start.
        let mut op = SkOp::Eq;
        for _ in 0..6 {
            op = op.next();
        }
        assert_eq!(op, SkOp::Eq);
    }
}
