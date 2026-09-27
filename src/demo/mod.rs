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

/// What the connector learned from a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DemoRequest {
    pub service: String,
    pub region: String,
    pub operation: String,
    pub protocol: Protocol,
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
            body,
        }
    }

    /// The response for this request: the first matching fixture, rendered,
    /// or a protocol-correct empty success.
    pub fn respond(&self) -> (String, &'static str) {
        let content_type = match self.protocol {
            Protocol::Json => "application/x-amz-json-1.1",
            Protocol::Cbor => "application/cbor",
            Protocol::Rest => "application/json",
            Protocol::Ec2Query | Protocol::Query => "text/xml",
        };
        let body = match fixtures::find(&self.service, &self.operation, &self.body) {
            Some(body) => render(body, &self.region, chrono::Utc::now().timestamp()),
            None => self.empty(),
        };
        (body, content_type)
    }

    fn empty(&self) -> String {
        let op = &self.operation;
        match self.protocol {
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
        let body = if req.protocol == Protocol::Cbor && body == "\u{a0}" {
            SdkBody::from(vec![0xa0u8])
        } else {
            SdkBody::from(body)
        };
        let mut resp = HttpResponse::new(200u16.try_into().expect("200 is a status"), body);
        resp.headers_mut().insert("content-type", content_type);
        if req.protocol == Protocol::Cbor {
            resp.headers_mut().insert("smithy-protocol", "rpc-v2-cbor");
        }
        resp.headers_mut().insert("x-amzn-requestid", "neboto-demo");
        HttpConnectorFuture::ready(Ok(resp))
    }
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
///
/// Offsets take `s`/`m`/`h`/`d`; a bare `now` is allowed. Times are relative
/// so the demo never looks stale.
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
        "iso" => chrono::DateTime::from_timestamp(secs, 0)
            .map(|t| t.format("%Y-%m-%dT%H:%M:%S.000Z").to_string()),
        _ => None,
    }
}

/// `now` → 0, `now-15m` → 900.
fn offset_secs(when: &str) -> Option<i64> {
    let rest = when.strip_prefix("now")?;
    if rest.is_empty() {
        return Some(0);
    }
    let rest = rest.strip_prefix('-')?;
    let (num, unit) = rest.split_at(rest.len().checked_sub(1)?);
    let n: i64 = num.parse().ok()?;
    let mult = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3600,
        "d" => 86400,
        _ => return None,
    };
    Some(n * mult)
}

#[cfg(test)]
mod tests;
