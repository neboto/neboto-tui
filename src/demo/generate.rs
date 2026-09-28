//! Responses computed per request instead of stored: log events.
//!
//! A stored `FilterLogEvents` body can't drive a live tail — the tail asks
//! for everything after its cursor, and a fixed body either repeats or never
//! moves. So each demo log group is a **script** played on a fixed clock:
//! batch `i` (one Lambda invocation, one HTTP request) happens at
//! `i × period` ms since the epoch, its lines land a few ms apart, and their
//! text depends only on `i`. Any time window therefore has a definite,
//! repeatable set of events, a tail sees a new batch every `period`, and a
//! search over the last hour finds the same lines the tail showed.

use super::DemoRequest;

/// Most events one response carries, like the real API's page cap.
const MAX_EVENTS: usize = 1000;

pub fn respond(req: &DemoRequest, now_ms: i64) -> Option<String> {
    match (req.service.as_str(), req.operation.as_str()) {
        ("logs", "FilterLogEvents") => filter_log_events(&req.body, now_ms),
        _ => None,
    }
}

struct Script {
    group: &'static str,
    period_ms: i64,
    stream: fn(i: i64) -> String,
    /// Batch `i`'s lines as `(ms after the batch starts, text)`; every
    /// offset is below `period_ms`.
    batch: fn(i: i64) -> Vec<(i64, String)>,
}

static SCRIPTS: &[Script] = &[
    Script {
        group: "/aws/lambda/orders-api",
        period_ms: 2000,
        stream: lambda_stream,
        batch: orders_api_invocation,
    },
    Script {
        group: "/ecs/storefront-web",
        period_ms: 700,
        stream: storefront_stream,
        batch: storefront_request,
    },
];

fn filter_log_events(body: &str, now_ms: i64) -> Option<String> {
    let req: serde_json::Value = serde_json::from_str(body).ok()?;
    let group = req.get("logGroupName")?.as_str()?;
    let script = SCRIPTS.iter().find(|s| s.group == group)?;
    let start = req.get("startTime").and_then(|v| v.as_i64()).unwrap_or(now_ms - 15 * 60_000);
    let end = req
        .get("endTime")
        .and_then(|v| v.as_i64())
        .unwrap_or(now_ms)
        .min(now_ms);
    let terms = pattern_terms(req.get("filterPattern").and_then(|v| v.as_str()).unwrap_or(""));

    // A batch that started before `start` can still have lines after it.
    let first = start.div_euclid(script.period_ms);
    let last = end.div_euclid(script.period_ms);
    let mut events: Vec<serde_json::Value> = Vec::new();
    for i in first..=last {
        let stream = (script.stream)(i);
        for (n, (offset, message)) in (script.batch)(i).into_iter().enumerate() {
            let ts = i * script.period_ms + offset;
            if ts < start || ts > end || !terms.iter().all(|t| message.contains(t.as_str())) {
                continue;
            }
            events.push(serde_json::json!({
                "logStreamName": stream,
                "timestamp": ts,
                "message": message,
                "ingestionTime": ts + 180,
                "eventId": format!("{i}-{n}"),
            }));
        }
    }
    // Keep the newest page: a tail seeding its lookback wants the latest lines.
    if events.len() > MAX_EVENTS {
        events.drain(..events.len() - MAX_EVENTS);
    }
    Some(serde_json::json!({ "events": events, "searchedLogStreams": [] }).to_string())
}

/// `"ERROR" timeout` → `["ERROR", "timeout"]`. A JSON (`{ … }`) or
/// space-delimited (`[ … ]`) pattern matches everything — the demo only
/// understands plain terms.
fn pattern_terms(pattern: &str) -> Vec<String> {
    let p = pattern.trim();
    if p.starts_with('{') || p.starts_with('[') {
        return Vec::new();
    }
    p.split_whitespace()
        .map(|t| t.trim_matches('"').to_string())
        .filter(|t| !t.is_empty())
        .collect()
}

/// A deterministic hash (splitmix64) so "random" details repeat for the same
/// `k`, while neighbouring `k`s look unrelated.
fn mix(k: i64) -> u64 {
    let mut x = (k as u64).wrapping_add(0x9e37_79b9_7f4a_7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

fn request_id(n: i64) -> String {
    let h = mix(n);
    format!(
        "{:08x}-{:04x}-4{:03x}-a{:03x}-{:012x}",
        h >> 32,
        (h >> 16) & 0xffff,
        h & 0xfff,
        (h >> 20) & 0xfff,
        mix(n + 7) & 0xffff_ffff_ffff
    )
}

// ── /aws/lambda/orders-api ──────────────────────────────────────────────────
// One invocation per batch. Roughly one in nine hits the DynamoDB throttle
// the function has no retry for — the thing to spot.

fn lambda_stream(i: i64) -> String {
    // A fresh execution environment every ~20 minutes, as Lambda does.
    let env = i / 600;
    format!("2026/09/28/[$LATEST]{:016x}{:016x}", mix(env), mix(env + 1))
}

fn orders_api_invocation(i: i64) -> Vec<(i64, String)> {
    let id = request_id(i);
    let failed = mix(i).is_multiple_of(9);
    let order = 48_000 + i.rem_euclid(9_000);
    let items = 1 + mix(i + 3) % 4;
    let ms = 38 + (mix(i) % 90) as i64;
    let result = if failed {
        format!(
            "{{\"level\":\"ERROR\",\"requestId\":\"{id}\",\"msg\":\"PutItem failed\",\"table\":\"orders\",\"error\":\"ProvisionedThroughputExceededException: The level of configured provisioned throughput for the table was exceeded.\"}}"
        )
    } else {
        format!("{{\"level\":\"INFO\",\"requestId\":\"{id}\",\"msg\":\"order stored\",\"orderId\":\"ord_{order}\"}}")
    };
    vec![
        (0, format!("START RequestId: {id} Version: $LATEST")),
        (
            2,
            format!(
                "{{\"level\":\"INFO\",\"requestId\":\"{id}\",\"msg\":\"POST /orders\",\"orderId\":\"ord_{order}\",\"items\":{items}}}"
            ),
        ),
        (ms - 3, result),
        (ms, format!("END RequestId: {id}")),
        (
            ms,
            format!(
                "REPORT RequestId: {id}\tDuration: {ms}.{:02} ms\tBilled Duration: {} ms\tMemory Size: 512 MB\tMax Memory Used: {} MB",
                mix(i + 5) % 100,
                ms + 1,
                91 + mix(i + 9) % 12
            ),
        ),
    ]
}

// ── /ecs/storefront-web ─────────────────────────────────────────────────────
// One access-log line per batch; the odd checkout times out upstream.

const STOREFRONT_TASKS: [&str; 3] = [
    "0f1e2d3c4b5a69788796a5b4c3d2e1f0",
    "1a2b3c4d5e6f70819283a4b5c6d7e8f9",
    "2b3c4d5e6f7081929384b5c6d7e8f9a0",
];

fn storefront_stream(i: i64) -> String {
    format!("ecs/web/{}", STOREFRONT_TASKS[(mix(i) % 3) as usize])
}

fn storefront_request(i: i64) -> Vec<(i64, String)> {
    const PATHS: [&str; 6] = ["/", "/products", "/products/42", "/cart", "/checkout", "/healthz"];
    let h = mix(i);
    let path = PATHS[(h % PATHS.len() as u64) as usize];
    let (status, ms) = if path == "/checkout" && h.is_multiple_of(7) {
        (502, 3000 + h % 500)
    } else {
        (200, 4 + h % 60)
    };
    let line = format!(
        "10.20.{}.{} - - \"GET {path} HTTP/1.1\" {status} {} {ms}ms",
        1 + h % 2,
        10 + (h >> 8) % 200,
        512 + (h >> 16) % 20_000
    );
    vec![((h >> 24) as i64 % 300, line)]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(start: i64, end: Option<i64>, pattern: &str) -> String {
        let mut v = serde_json::json!({"logGroupName": "/aws/lambda/orders-api", "startTime": start});
        if let Some(end) = end {
            v["endTime"] = end.into();
        }
        if !pattern.is_empty() {
            v["filterPattern"] = pattern.into();
        }
        v.to_string()
    }

    fn events(resp: &str) -> Vec<serde_json::Value> {
        let v: serde_json::Value = serde_json::from_str(resp).unwrap();
        v["events"].as_array().unwrap().clone()
    }

    #[test]
    fn a_tail_sees_only_lines_after_its_cursor_and_they_repeat() {
        let now = 1_790_000_000_000;
        let first = events(&filter_log_events(&body(now - 60_000, None, ""), now).unwrap());
        assert!((145..=155).contains(&first.len()), "~30 invocations of 5 lines: {}", first.len());
        let cursor = first.last().unwrap()["timestamp"].as_i64().unwrap() + 1;
        let later = events(&filter_log_events(&body(cursor, None, ""), now + 4_000).unwrap());
        assert!(!later.is_empty() && later.len() <= 10, "only the new lines: {}", later.len());
        assert!(later.iter().all(|e| e["timestamp"].as_i64().unwrap() >= cursor));
        let again = events(&filter_log_events(&body(now - 60_000, None, ""), now).unwrap());
        assert_eq!(first, again, "the same window gives the same lines");
    }

    #[test]
    fn a_search_pattern_filters_to_matching_lines() {
        let now = 1_790_000_000_000;
        let errors = events(&filter_log_events(&body(now - 3_600_000, Some(now), "\"ERROR\""), now).unwrap());
        assert!(!errors.is_empty(), "an hour of traffic has some throttles");
        assert!(errors.iter().all(|e| e["message"].as_str().unwrap().contains("ERROR")));
    }

    #[test]
    fn unknown_groups_fall_through_to_fixtures() {
        let b = serde_json::json!({"logGroupName": "/nope", "startTime": 0}).to_string();
        assert!(filter_log_events(&b, 1).is_none());
    }
}
