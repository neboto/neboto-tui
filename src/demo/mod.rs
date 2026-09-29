//! Demo mode (`neboto --demo`, issue #47): a canned, offline AWS account.
//!
//! Every SDK client gets [`http_client`] as its HTTP client, which answers
//! each request from the embedded fixtures in [`fixtures`] instead of the
//! network. Nothing above the HTTP layer knows — list loads, lazy sections,
//! charts and jumps all run their real SDK calls — so a new feature gets demo
//! coverage by adding fixtures, not code.
//!
//! Lookup is by **service** (the endpoint host's first label: `ec2`, `ecs`,
//! `sts`) and **operation** (the `X-Amz-Target` suffix for JSON protocols, the
//! `Action` form field for query protocols), optionally narrowed by a
//! substring of the request body so per-resource calls
//! (`DescribeTaskDefinition` for `web:15`) get their own response. An
//! operation with no fixture answers with a protocol-correct **empty**
//! success, so an uncovered service shows an empty list, never an error.

pub mod fixtures;
mod generate;

use aws_smithy_runtime_api::client::http::{
    http_client_fn, HttpConnector, HttpConnectorFuture, SharedHttpClient, SharedHttpConnector,
};
use aws_smithy_runtime_api::client::orchestrator::{HttpRequest, HttpResponse};
use aws_smithy_types::body::SdkBody;
use std::sync::atomic::{AtomicBool, Ordering};

/// The demo account id every fixture's `{{account}}` renders as.
pub const ACCOUNT: &str = "123456789012";
/// The region demo mode starts in.
pub const REGION: &str = "us-east-1";

static ENABLED: AtomicBool = AtomicBool::new(false);

/// Turn demo mode on for the process. Called once from `App::new` before the
/// clients are built; there is no way back to real AWS in the same run.
pub fn enable() {
    ENABLED.store(true, Ordering::Relaxed);
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// The replay client installed on every `SdkConfig` in demo mode.
pub fn http_client() -> SharedHttpClient {
    http_client_fn(|_, _| SharedHttpConnector::new(DemoConnector))
}

#[derive(Debug)]
struct DemoConnector;

/// The wire protocol a request speaks, which decides both how the operation
/// is named and what an empty response looks like.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    /// awsJson1.0 / 1.1 — `X-Amz-Target: Prefix.Operation`.
    Json,
    /// EC2's query dialect — form body, XML response with no Result wrapper.
    Ec2Query,
    /// awsQuery (STS, IAM, …) — form body, `<OpResponse><OpResult>` XML.
    Query,
    /// Smithy RPC v2 CBOR (CloudWatch metrics) — not covered yet.
    Cbor,
    /// restJson / restXml (Lambda, S3, …) — method + path. Not covered yet.
    Rest,
}

/// Services whose REST protocol is restXml rather than restJson, by endpoint
/// host label.
fn is_rest_xml(service: &str) -> bool {
    matches!(service, "s3" | "route53" | "cloudfront")
}

/// What the connector learned from a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DemoRequest {
    pub service: String,
    pub region: String,
    pub operation: String,
    pub protocol: Protocol,
    /// The full request URI — REST calls carry their parameters here.
    pub uri: String,
    pub body: String,
}

impl DemoRequest {
    pub fn parse(req: &HttpRequest) -> Self {
        let host = req
            .uri()
            .split("://")
            .nth(1)
            .unwrap_or_default()
            .split(['/', ':', '?'])
            .next()
            .unwrap_or_default()
            .to_string();
        let mut labels = host.split('.');
        let service = labels.next().unwrap_or_default().to_string();
        let region = labels
            .next()
            .filter(|l| l.contains('-'))
            .unwrap_or(REGION)
            .to_string();
        let body = req
            .body()
            .bytes()
            .map(|b| String::from_utf8_lossy(b).into_owned())
            .unwrap_or_default();
        let headers = req.headers();
        let (operation, protocol) = if let Some(target) = headers.get("x-amz-target") {
            (target.rsplit('.').next().unwrap_or(target).to_string(), Protocol::Json)
        } else if headers.get("smithy-protocol").is_some() {
            let op = req.uri().rsplit('/').next().unwrap_or_default().to_string();
            (op, Protocol::Cbor)
        } else if let Some(action) = form_field(&body, "Action") {
            let protocol = if service == "ec2" { Protocol::Ec2Query } else { Protocol::Query };
            (action, protocol)
        } else {
            let path = req.uri().splitn(4, '/').nth(3).unwrap_or_default();
            (format!("{} /{}", req.method(), path.split('?').next().unwrap_or_default()), Protocol::Rest)
        };
        DemoRequest {
            service,
            region,
            operation,
            protocol,
            uri: req.uri().to_string(),
            body,
        }
    }

    /// What a fixture's `when` substrings are matched against: the URI (for
    /// REST path / query parameters) and the body (for everything else).
    pub fn haystack(&self) -> String {
        format!("{}\n{}", self.uri, self.body)
    }

    /// The response for this request: the first matching fixture, rendered,
    /// or a protocol-correct empty success.
    pub fn respond(&self) -> (String, &'static str) {
        let content_type = match self.protocol {
            Protocol::Json => "application/x-amz-json-1.1",
            Protocol::Cbor => "application/cbor",
            Protocol::Rest if is_rest_xml(&self.service) => "application/xml",
            Protocol::Rest => "application/json",
            Protocol::Ec2Query | Protocol::Query => "text/xml",
        };
        let now = chrono::Utc::now().timestamp();
        let (body, source) = if let Some(body) = generate::respond(self, now * 1000) {
            (body, "generated")
        } else if let Some(body) = fixtures::find(&self.service, &self.operation, &self.haystack()) {
            (narrow(self, render(body, &self.region, now)), "fixture")
        } else {
            (self.empty(), "EMPTY")
        };
        trace(self, source);
        (body, content_type)
    }

    fn empty(&self) -> String {
        let op = &self.operation;
        match self.protocol {
            // restXml (S3, Route 53, CloudFront) can't parse `{}`; an empty
            // body deserializes as an output with nothing in it.
            Protocol::Rest if is_rest_xml(&self.service) => String::new(),
            Protocol::Json | Protocol::Rest => "{}".to_string(),
            // An empty CBOR map.
            Protocol::Cbor => "\u{a0}".to_string(),
            Protocol::Ec2Query => format!(
                "<{op}Response xmlns=\"http://ec2.amazonaws.com/doc/2016-11-15/\"><requestId>demo</requestId></{op}Response>"
            ),
            Protocol::Query => format!(
                "<{op}Response><{op}Result></{op}Result><ResponseMetadata><RequestId>demo</RequestId></ResponseMetadata></{op}Response>"
            ),
        }
    }
}

impl HttpConnector for DemoConnector {
    fn call(&self, request: HttpRequest) -> HttpConnectorFuture {
        let req = DemoRequest::parse(&request);
        let (body, content_type) = req.respond();
        let (status, body) = split_status(body);
        let error_type = (status >= 400).then(|| error_type(&body)).flatten();
        let body = if req.protocol == Protocol::Cbor && body == "\u{a0}" {
            SdkBody::from(vec![0xa0u8])
        } else {
            SdkBody::from(body)
        };
        let mut resp = HttpResponse::new(status.try_into().expect("fixture status is valid"), body);
        resp.headers_mut().insert("content-type", content_type);
        if let Some(code) = error_type {
            resp.headers_mut().insert("x-amzn-errortype", code);
        }
        if req.protocol == Protocol::Cbor {
            resp.headers_mut().insert("smithy-protocol", "rpc-v2-cbor");
        }
        resp.headers_mut().insert("x-amzn-requestid", "neboto-demo");
        HttpConnectorFuture::ready(Ok(resp))
    }
}

/// A fixture that starts `!status 404` answers with that status — how a
/// "this resource has none" call (Lambda's `GetFunctionEventInvokeConfig`)
/// fails the way the real API does, instead of an empty success the app
/// would read as "configured, all defaults".
fn split_status(body: String) -> (u16, String) {
    if let Some(rest) = body.strip_prefix("!status ") {
        let (code, rest) = rest.split_once('\n').unwrap_or((rest, ""));
        if let Ok(code) = code.trim().parse() {
            return (code, rest.to_string());
        }
    }
    (200, body)
}

/// A JSON error body's `__type`, which REST-JSON clients also want in the
/// `x-amzn-errortype` header.
fn error_type(body: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    v.get("__type")?.as_str().map(str::to_string)
}

/// `NEBOTO_DEMO_TRACE=<file>` appends one line per request — how a new
/// fixture's author finds out which calls a view makes and which of them
/// are still answered empty.
/// Every request the demo connector answered in this test process, as
/// `(service, operation, body)` — how a test proves a call never happened
/// (`NEBOTO_DEMO_TRACE` is a file, shared by every test running at once).
#[cfg(test)]
pub(crate) static TEST_LOG: std::sync::Mutex<Vec<(String, String, String)>> =
    std::sync::Mutex::new(Vec::new());

fn trace(req: &DemoRequest, source: &str) {
    use std::io::Write;
    #[cfg(test)]
    if let Ok(mut log) = TEST_LOG.lock() {
        log.push((req.service.clone(), req.operation.clone(), req.body.clone()));
    }
    let Some(path) = std::env::var_os("NEBOTO_DEMO_TRACE") else {
        return;
    };
    let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) else {
        return;
    };
    let detail: String = match req.protocol {
        Protocol::Rest => req.uri.splitn(4, '/').nth(3).unwrap_or_default().to_string(),
        _ => req.body.chars().take(200).collect(),
    };
    // One write per line: requests trace from many tasks at once.
    let line = format!("{:9} {:22} {:40} {}\n", source, req.service, req.operation, detail);
    let _ = f.write_all(line.as_bytes());
}

/// EC2 describe calls filter by id (`GroupId.1=sg-…&GroupId.2=…`), and
/// callers trust the answer to contain only those — the network-access lens
/// merges every group it gets back. One fixture holds the whole list, so a
/// request that names ids gets it cut down to them.
fn narrow(req: &DemoRequest, body: String) -> String {
    let (param, list, id_tag) = match (req.service.as_str(), req.operation.as_str()) {
        ("ec2", "DescribeSecurityGroups") => ("GroupId", "securityGroupInfo", "groupId"),
        ("ec2", "DescribeVolumes") => ("VolumeId", "volumeSet", "volumeId"),
        _ => return body,
    };
    let wanted: Vec<String> = (1..)
        .map_while(|n| form_field(&req.body, &format!("{param}.{n}")))
        .collect();
    if wanted.is_empty() {
        return body;
    }
    keep_items(&body, list, id_tag, &wanted).unwrap_or(body)
}

/// Keep the top-level `<item>`s of `<list>` whose first `<id_tag>` is in
/// `wanted`. Items nest (a security group's rules are items too), so the
/// split counts depth rather than trusting indentation.
fn keep_items(xml: &str, list: &str, id_tag: &str, wanted: &[String]) -> Option<String> {
    let open = format!("<{list}>");
    let start = xml.find(&open)? + open.len();
    let end = start + xml[start..].find(&format!("</{list}>"))?;
    let inner = &xml[start..end];

    let mut kept = String::new();
    let (mut depth, mut item_start, mut i) = (0usize, 0usize, 0usize);
    while i < inner.len() {
        let rest = &inner[i..];
        if rest.starts_with("<item>") {
            if depth == 0 {
                item_start = i;
            }
            depth += 1;
            i += "<item>".len();
        } else if rest.starts_with("</item>") {
            depth = depth.checked_sub(1)?;
            i += "</item>".len();
            if depth == 0 {
                let item = &inner[item_start..i];
                let id = item
                    .split(&format!("<{id_tag}>"))
                    .nth(1)
                    .and_then(|r| r.split('<').next())
                    .unwrap_or_default();
                if wanted.iter().any(|w| w == id) {
                    kept.push_str(item);
                }
            }
        } else {
            i += rest.chars().next().map_or(1, char::len_utf8);
        }
    }
    Some(format!("{}{kept}{}", &xml[..start], &xml[end..]))
}

/// `Action=DescribeInstances&Version=…` → `Some("DescribeInstances")`.
fn form_field(body: &str, name: &str) -> Option<String> {
    body.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        (k == name).then(|| v.to_string())
    })
}

/// Expand a fixture's placeholders:
/// - `{{account}}`, `{{region}}`
/// - `{{iso:now-15m}}` → `2026-09-27T04:12:00.000Z` (XML timestamps)
/// - `{{epoch:now-2h}}` → `1790000000` (JSON timestamps)
/// - `{{epochms:now-2h}}` → `1790000000000` (CloudWatch Logs' millisecond fields)
///
/// Offsets take `s`/`m`/`h`/`d` and chain (`now-26h+31s`); a bare `now` is
/// allowed. Times are relative so the demo never looks stale.
pub fn render(template: &str, region: &str, now: i64) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let Some(len) = rest[start + 2..].find("}}") else {
            out.push_str(&rest[start..]);
            return out;
        };
        let token = &rest[start + 2..start + 2 + len];
        out.push_str(&expand(token, region, now).unwrap_or_else(|| format!("{{{{{token}}}}}")));
        rest = &rest[start + 2 + len + 2..];
    }
    out.push_str(rest);
    out
}

fn expand(token: &str, region: &str, now: i64) -> Option<String> {
    match token {
        "account" => return Some(ACCOUNT.to_string()),
        "region" => return Some(region.to_string()),
        _ => {}
    }
    let (kind, when) = token.split_once(':')?;
    let secs = now - offset_secs(when)?;
    match kind {
        "epoch" => Some(secs.to_string()),
        "epochms" => Some((secs * 1000).to_string()),
        "iso" => chrono::DateTime::from_timestamp(secs, 0)
            .map(|t| t.format("%Y-%m-%dT%H:%M:%S.000Z").to_string()),
        _ => None,
    }
}

/// How long ago a `now…` expression is: `now` → 0, `now-15m` → 900, and
/// terms chain, so `now-26h+31s` is 31 seconds after "26 hours ago" — how a
/// burst of CloudFormation events gets realistic spacing.
fn offset_secs(when: &str) -> Option<i64> {
    let mut rest = when.strip_prefix("now")?;
    let mut ago = 0;
    while !rest.is_empty() {
        let sign = match rest.as_bytes()[0] {
            b'-' => 1,
            b'+' => -1,
            _ => return None,
        };
        rest = &rest[1..];
        let digits = rest.find(|c: char| !c.is_ascii_digit())?;
        let n: i64 = rest[..digits].parse().ok()?;
        let mult = match rest[digits..].chars().next()? {
            's' => 1,
            'm' => 60,
            'h' => 3600,
            'd' => 86400,
            _ => return None,
        };
        ago += sign * n * mult;
        rest = &rest[digits + 1..];
    }
    Some(ago)
}

#[cfg(test)]
mod tests;
