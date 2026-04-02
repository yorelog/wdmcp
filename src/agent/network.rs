//! Network request monitoring via JavaScript interceptors and Chrome DevTools
//! Protocol (CDP) commands.
//!
//! This module injects JavaScript into the browser to intercept `fetch` and
//! `XMLHttpRequest` calls, capturing request/response metadata.  It can also
//! leverage CDP's `Network` domain for richer data when available.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;

use crate::types::now_iso;
use crate::webdriver::WdClient;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A captured network request/response pair.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkEntry {
    /// Unique entry ID ("net-0", "net-1", …).
    pub id: String,
    /// HTTP method (GET, POST, etc.).
    pub method: String,
    /// Request URL.
    pub url: String,
    /// HTTP response status code.
    pub status: Option<u16>,
    /// HTTP status text.
    pub status_text: Option<String>,
    /// Resource type ("document", "script", "xhr", "fetch", "image", …).
    pub resource_type: Option<String>,
    /// Request headers as a JSON object.
    pub request_headers: Option<Value>,
    /// Response headers as a JSON object.
    pub response_headers: Option<Value>,
    /// Request body (POST data).
    pub request_body: Option<String>,
    /// Response body (only populated on demand).
    pub response_body: Option<String>,
    /// Response Content-Type.
    pub content_type: Option<String>,
    /// Response content length in bytes.
    pub content_length: Option<u64>,
    /// Timing information.
    pub timing: Option<NetworkTiming>,
    /// What initiated this request ("script", "user", "parser", …).
    pub initiator: Option<String>,
    /// Whether the response was served from cache.
    pub from_cache: bool,
    /// ISO timestamp of when the request was made.
    pub timestamp: String,
}

/// Timing breakdown for a network request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkTiming {
    /// Timestamp when the request started (ms since navigation start).
    pub started_at: f64,
    /// Total request duration in ms.
    pub duration_ms: f64,
    /// DNS lookup time in ms.
    pub dns_ms: Option<f64>,
    /// TCP connection time in ms.
    pub connect_ms: Option<f64>,
    /// SSL/TLS handshake time in ms.
    pub ssl_ms: Option<f64>,
    /// Time to first byte in ms.
    pub ttfb_ms: Option<f64>,
    /// Content download time in ms.
    pub download_ms: Option<f64>,
}

/// Top-level result from a network capture query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkLog {
    /// Captured network entries (possibly filtered).
    pub entries: Vec<NetworkEntry>,
    /// Number of entries returned.
    pub entry_count: usize,
    /// Aggregate summary statistics.
    pub summary: NetworkSummary,
    /// ISO timestamp of when the log was captured.
    pub captured_at: String,
}

/// Aggregate statistics across captured network entries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkSummary {
    /// Total number of requests.
    pub total_requests: usize,
    /// Counts per resource type, e.g. `{"xhr": 5, "script": 12}`.
    pub by_type: Value,
    /// Counts per status-code range, e.g. `{"2xx": 18, "3xx": 2, "4xx": 1}`.
    pub by_status: Value,
    /// Sum of `content_length` where known.
    pub total_bytes: u64,
    /// Requests with status >= 400 or status == 0.
    pub failed_count: usize,
    /// Requests served from cache.
    pub cached_count: usize,
}

/// Filtering options for network log queries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkFilter {
    /// Filter by URL substring.
    pub url_pattern: Option<String>,
    /// Filter by HTTP method(s).
    pub methods: Option<Vec<String>>,
    /// Filter by resource type(s).
    pub resource_types: Option<Vec<String>>,
    /// Filter by status code range (min, max) inclusive.
    pub status_range: Option<(u16, u16)>,
    /// Only include errors (status >= 400).
    pub has_error: Option<bool>,
    /// Maximum number of entries to return.
    pub limit: Option<usize>,
}

impl Default for NetworkFilter {
    fn default() -> Self {
        Self {
            url_pattern: None,
            methods: None,
            resource_types: None,
            status_range: None,
            has_error: None,
            limit: None,
        }
    }
}

// ---------------------------------------------------------------------------
// JavaScript payloads
// ---------------------------------------------------------------------------

/// JavaScript injected to intercept `fetch` and `XMLHttpRequest` calls, plus
/// observe `PerformanceObserver` resource entries.
pub const NETWORK_INTERCEPTOR_JS: &str = r#"
return (function() {
    try {
        if (window.__browsectl_net && window.__browsectl_net.active) {
            return "interceptor already installed";
        }

        var net = {
            entries: [],
            seq: 0,
            active: true,
            origFetch: window.fetch,
            origXhrOpen: XMLHttpRequest.prototype.open,
            origXhrSend: XMLHttpRequest.prototype.send
        };
        window.__browsectl_net = net;

        // ── Wrap fetch ────────────────────────────────────────────────
        window.fetch = function(resource, init) {
            if (!net.active) return net.origFetch.apply(this, arguments);

            var entry = {
                id: "net-" + (net.seq++),
                method: (init && init.method) ? init.method.toUpperCase() : "GET",
                url: (typeof resource === "string") ? resource : (resource && resource.url ? resource.url : String(resource)),
                type: "fetch",
                startTime: performance.now(),
                timestamp: new Date().toISOString(),
                requestHeaders: null,
                requestBody: null,
                status: null,
                statusText: null,
                responseHeaders: null,
                contentType: null,
                contentLength: null,
                fromCache: false,
                error: null
            };

            try {
                if (init && init.headers) {
                    var h = {};
                    if (init.headers instanceof Headers) {
                        init.headers.forEach(function(v, k) { h[k] = v; });
                    } else if (typeof init.headers === "object") {
                        h = init.headers;
                    }
                    entry.requestHeaders = h;
                }
                if (init && init.body) {
                    entry.requestBody = typeof init.body === "string" ? init.body : "[binary]";
                }
            } catch(e) {}

            return net.origFetch.apply(this, arguments).then(function(response) {
                try {
                    entry.status = response.status;
                    entry.statusText = response.statusText;
                    entry.contentType = response.headers.get("content-type");
                    var cl = response.headers.get("content-length");
                    if (cl) entry.contentLength = parseInt(cl, 10);
                    var rh = {};
                    response.headers.forEach(function(v, k) { rh[k] = v; });
                    entry.responseHeaders = rh;
                    entry.duration = performance.now() - entry.startTime;
                } catch(e) {}
                net.entries.push(entry);
                return response;
            }).catch(function(err) {
                entry.error = err ? err.toString() : "fetch error";
                entry.status = 0;
                entry.duration = performance.now() - entry.startTime;
                net.entries.push(entry);
                throw err;
            });
        };

        // ── Wrap XMLHttpRequest ───────────────────────────────────────
        XMLHttpRequest.prototype.open = function(method, url) {
            this.__browsectl = {
                method: method ? method.toUpperCase() : "GET",
                url: url
            };
            return net.origXhrOpen.apply(this, arguments);
        };

        XMLHttpRequest.prototype.send = function(body) {
            var xhr = this;
            var meta = xhr.__browsectl;
            if (!meta || !net.active) return net.origXhrSend.apply(this, arguments);

            var entry = {
                id: "net-" + (net.seq++),
                method: meta.method,
                url: meta.url,
                type: "xhr",
                startTime: performance.now(),
                timestamp: new Date().toISOString(),
                requestBody: null,
                status: null,
                statusText: null,
                responseHeaders: null,
                contentType: null,
                contentLength: null,
                fromCache: false,
                error: null
            };

            if (body) {
                entry.requestBody = typeof body === "string" ? body : "[binary]";
            }

            xhr.addEventListener("load", function() {
                try {
                    entry.status = xhr.status;
                    entry.statusText = xhr.statusText;
                    entry.contentType = xhr.getResponseHeader("content-type");
                    var cl = xhr.getResponseHeader("content-length");
                    if (cl) entry.contentLength = parseInt(cl, 10);
                    entry.responseHeaders = xhr.getAllResponseHeaders();
                    entry.duration = performance.now() - entry.startTime;
                } catch(e) {}
                net.entries.push(entry);
            });

            xhr.addEventListener("error", function() {
                entry.error = "xhr error";
                entry.status = 0;
                entry.duration = performance.now() - entry.startTime;
                net.entries.push(entry);
            });

            return net.origXhrSend.apply(this, arguments);
        };

        // ── PerformanceObserver for resource timing ───────────────────
        try {
            if (typeof PerformanceObserver !== "undefined") {
                var po = new PerformanceObserver(function(list) {
                    var perfEntries = list.getEntries();
                    for (var i = 0; i < perfEntries.length; i++) {
                        var pe = perfEntries[i];
                        var timing = {
                            name: pe.name,
                            duration: pe.duration,
                            startTime: pe.startTime,
                            dns: pe.domainLookupEnd - pe.domainLookupStart,
                            connect: pe.connectEnd - pe.connectStart,
                            ssl: pe.secureConnectionStart > 0 ? pe.connectEnd - pe.secureConnectionStart : 0,
                            ttfb: pe.responseStart - pe.requestStart,
                            download: pe.responseEnd - pe.responseStart,
                            transferSize: pe.transferSize,
                            decodedBodySize: pe.decodedBodySize
                        };

                        // Try to match with existing entries and merge timing
                        var matched = false;
                        for (var j = net.entries.length - 1; j >= 0; j--) {
                            if (net.entries[j].url === pe.name || pe.name.indexOf(net.entries[j].url) !== -1) {
                                net.entries[j].timing = timing;
                                if (pe.transferSize === 0 && pe.decodedBodySize > 0) {
                                    net.entries[j].fromCache = true;
                                }
                                matched = true;
                                break;
                            }
                        }

                        if (!matched) {
                            net.entries.push({
                                id: "net-" + (net.seq++),
                                method: "GET",
                                url: pe.name,
                                type: pe.initiatorType || "resource",
                                startTime: pe.startTime,
                                timestamp: new Date().toISOString(),
                                status: null,
                                statusText: null,
                                contentType: null,
                                contentLength: pe.decodedBodySize || null,
                                fromCache: pe.transferSize === 0 && pe.decodedBodySize > 0,
                                timing: timing,
                                error: null
                            });
                        }
                    }
                });
                po.observe({ type: "resource", buffered: true });
            }
        } catch(e) {
            // PerformanceObserver not available — ignore
        }

        return "interceptor installed";
    } catch(e) {
        return "interceptor error: " + e.toString();
    }
})();
"#;

/// JavaScript to read captured network entries.
pub const NETWORK_READ_JS: &str = r#"
return (function() {
    var net = window.__browsectl_net;
    if (!net) return JSON.stringify({ entries: [], error: "interceptor not installed" });
    var result = JSON.stringify({ entries: net.entries });
    return result;
})();
"#;

/// JavaScript to clear captured network entries.
pub const NETWORK_CLEAR_JS: &str = r#"
return (function() {
    var net = window.__browsectl_net;
    if (!net) return JSON.stringify({ cleared: 0 });
    var count = net.entries.length;
    net.entries = [];
    return JSON.stringify({ cleared: count });
})();
"#;

/// JavaScript to disable the interceptor and restore originals.
pub const NETWORK_DISABLE_JS: &str = r#"
return (function() {
    var net = window.__browsectl_net;
    if (!net) return JSON.stringify({ ok: false, reason: "not installed" });
    if (net.origFetch) window.fetch = net.origFetch;
    if (net.origXhrOpen) XMLHttpRequest.prototype.open = net.origXhrOpen;
    if (net.origXhrSend) XMLHttpRequest.prototype.send = net.origXhrSend;
    net.active = false;
    return JSON.stringify({ ok: true });
})();
"#;

/// JavaScript to read `performance.getEntriesByType('resource')` directly.
const RESOURCE_TIMING_JS: &str = r#"
return (function() {
    var entries = performance.getEntriesByType('resource');
    var result = entries.map(function(e) {
        return {
            name: e.name,
            type: e.initiatorType,
            startTime: e.startTime,
            duration: e.duration,
            transferSize: e.transferSize,
            decodedBodySize: e.decodedBodySize,
            dns: e.domainLookupEnd - e.domainLookupStart,
            connect: e.connectEnd - e.connectStart,
            ttfb: e.responseStart - e.requestStart,
            download: e.responseEnd - e.responseStart,
            protocol: e.nextHopProtocol
        };
    });
    return JSON.stringify(result);
})();
"#;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Inject the network interceptor and optionally enable the CDP Network
/// domain.  Returns a JSON value indicating success.
pub async fn enable_network_capture(driver: &WdClient) -> Result<Value> {
    let result = driver
        .execute(NETWORK_INTERCEPTOR_JS, vec![])
        .await
        .context("failed to inject network interceptor")?;

    // Best-effort: enable CDP Network domain for extra data.  Ignore errors
    // because CDP may not be available (e.g. non-Chrome browsers).
    let _ = driver.cdp_execute_quiet("Network.enable", json!({})).await;

    Ok(json!({
        "ok": true,
        "interceptor": result,
    }))
}

/// Disable the network interceptor and restore the original `fetch` /
/// `XMLHttpRequest` implementations.
pub async fn disable_network_capture(driver: &WdClient) -> Result<Value> {
    let raw = driver
        .execute(NETWORK_DISABLE_JS, vec![])
        .await
        .context("failed to disable network interceptor")?;

    // Best-effort CDP cleanup.
    let _ = driver.cdp_execute_quiet("Network.disable", json!({})).await;

    let result: Value = if let Some(s) = raw.as_str() {
        serde_json::from_str(s).unwrap_or(raw)
    } else {
        raw
    };

    Ok(result)
}

/// Retrieve the network log, optionally filtered.
pub async fn get_network_log(driver: &WdClient, filter: &NetworkFilter) -> Result<NetworkLog> {
    // 1. Execute JS to read captured entries.
    let raw = driver
        .execute(NETWORK_READ_JS, vec![])
        .await
        .context("failed to read network entries")?;

    let raw_str = match raw.as_str() {
        Some(s) => s.to_string(),
        None => serde_json::to_string(&raw).unwrap_or_default(),
    };

    let parsed: Value = serde_json::from_str(&raw_str).unwrap_or_else(|_| json!({ "entries": [] }));

    let raw_entries = parsed["entries"].as_array().cloned().unwrap_or_default();

    // 2. Parse raw JS entries into typed NetworkEntry values.
    let mut entries: Vec<NetworkEntry> = Vec::with_capacity(raw_entries.len());
    for (idx, raw_entry) in raw_entries.iter().enumerate() {
        let entry = parse_raw_entry(raw_entry, idx);
        entries.push(entry);
    }

    // 3. Apply filters.
    let filtered = apply_filter(entries, filter);

    // 4. Build summary.
    let summary = build_summary(&filtered);

    let entry_count = filtered.len();
    Ok(NetworkLog {
        entries: filtered,
        entry_count,
        summary,
        captured_at: now_iso(),
    })
}

/// Attempt to retrieve the response body for a given request ID.
///
/// Looks in the JS-captured entries first.  CDP-based body retrieval requires
/// request IDs from the Network domain event stream, which is not yet
/// implemented; this function returns an appropriate message in that case.
pub async fn get_response_body(driver: &WdClient, request_id: &str) -> Result<Value> {
    // Check JS entries for a matching request body.
    let raw = driver
        .execute(NETWORK_READ_JS, vec![])
        .await
        .context("failed to read network entries for body lookup")?;

    let raw_str = match raw.as_str() {
        Some(s) => s.to_string(),
        None => serde_json::to_string(&raw).unwrap_or_default(),
    };

    let parsed: Value = serde_json::from_str(&raw_str).unwrap_or_else(|_| json!({ "entries": [] }));

    if let Some(entries) = parsed["entries"].as_array() {
        for entry in entries {
            let id = entry["id"].as_str().unwrap_or("");
            if id == request_id {
                if let Some(body) = entry.get("responseBody") {
                    if !body.is_null() {
                        return Ok(json!({
                            "request_id": request_id,
                            "body": body,
                            "source": "js_interceptor",
                        }));
                    }
                }
                // Entry found but no body stored.
                return Ok(json!({
                    "request_id": request_id,
                    "body": null,
                    "source": "js_interceptor",
                    "note": "response body was not captured for this entry; \
                             JS interceptors do not automatically store bodies \
                             to avoid memory overhead",
                }));
            }
        }
    }

    Ok(json!({
        "request_id": request_id,
        "body": null,
        "error": "entry not found",
        "note": "no entry with the given ID exists in the JS-captured log",
    }))
}

/// Clear all captured network entries.
pub async fn clear_network_log(driver: &WdClient) -> Result<Value> {
    let raw = driver
        .execute(NETWORK_CLEAR_JS, vec![])
        .await
        .context("failed to clear network log")?;

    let result: Value = if let Some(s) = raw.as_str() {
        serde_json::from_str(s).unwrap_or(raw)
    } else {
        raw
    };

    Ok(result)
}

/// Retrieve resource timing entries via `performance.getEntriesByType`.
///
/// This is a lightweight alternative that does **not** require the interceptor
/// to be installed.
pub async fn get_resource_timing(driver: &WdClient) -> Result<Value> {
    let raw = driver
        .execute(RESOURCE_TIMING_JS, vec![])
        .await
        .context("failed to read resource timing")?;

    let result: Value = if let Some(s) = raw.as_str() {
        serde_json::from_str(s).unwrap_or(raw)
    } else {
        raw
    };

    Ok(result)
}

/// Retrieve all cookies via CDP `Network.getAllCookies`.  Falls back to
/// `document.cookie` when CDP is unavailable.
pub async fn get_cookies(driver: &WdClient) -> Result<Value> {
    // Try CDP first.
    match driver.cdp_execute("Network.getAllCookies", json!({})).await {
        Ok(val) => return Ok(val),
        Err(_) => {
            // Fall back to JS.
            let raw = driver
                .execute("return document.cookie;", vec![])
                .await
                .context("failed to read cookies via JS")?;

            let cookie_str = raw.as_str().unwrap_or("");
            let cookies: Vec<Value> = cookie_str
                .split(';')
                .filter(|s| !s.trim().is_empty())
                .map(|s| {
                    let s = s.trim();
                    if let Some((name, value)) = s.split_once('=') {
                        json!({ "name": name.trim(), "value": value.trim() })
                    } else {
                        json!({ "name": s, "value": "" })
                    }
                })
                .collect();

            Ok(json!({
                "cookies": cookies,
                "source": "document.cookie",
            }))
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Parse a single raw JS entry object into a typed [`NetworkEntry`].
fn parse_raw_entry(raw: &Value, idx: usize) -> NetworkEntry {
    let id = raw["id"]
        .as_str()
        .map(String::from)
        .unwrap_or_else(|| format!("net-{}", idx));

    let method = raw["method"].as_str().unwrap_or("GET").to_uppercase();

    let url = raw["url"].as_str().unwrap_or("").to_string();

    let status = raw["status"].as_u64().map(|s| s as u16);

    let status_text = raw["statusText"].as_str().map(String::from);

    let resource_type = raw["type"].as_str().map(String::from);

    let request_headers = raw.get("requestHeaders").cloned().filter(|v| !v.is_null());
    let response_headers = raw.get("responseHeaders").cloned().filter(|v| !v.is_null());
    let request_body = raw["requestBody"].as_str().map(String::from);
    let response_body = raw["responseBody"].as_str().map(String::from);

    let content_type = raw["contentType"].as_str().map(String::from);
    let content_length = raw["contentLength"].as_u64();

    let from_cache = raw["fromCache"].as_bool().unwrap_or(false);

    let timestamp = raw["timestamp"].as_str().unwrap_or("").to_string();

    let initiator = raw["initiator"].as_str().map(String::from);

    // Parse timing if present.
    let timing = raw.get("timing").and_then(|t| {
        if t.is_null() {
            return None;
        }
        Some(NetworkTiming {
            started_at: t["startTime"].as_f64().unwrap_or(0.0),
            duration_ms: t["duration"].as_f64().unwrap_or(0.0),
            dns_ms: t["dns"].as_f64(),
            connect_ms: t["connect"].as_f64(),
            ssl_ms: t["ssl"].as_f64(),
            ttfb_ms: t["ttfb"].as_f64(),
            download_ms: t["download"].as_f64(),
        })
    });

    NetworkEntry {
        id,
        method,
        url,
        status,
        status_text,
        resource_type,
        request_headers,
        response_headers,
        request_body,
        response_body,
        content_type,
        content_length,
        timing,
        initiator,
        from_cache,
        timestamp,
    }
}

/// Apply a [`NetworkFilter`] to a list of entries.
fn apply_filter(entries: Vec<NetworkEntry>, filter: &NetworkFilter) -> Vec<NetworkEntry> {
    let mut result: Vec<NetworkEntry> = entries
        .into_iter()
        .filter(|e| {
            // URL pattern
            if let Some(ref pat) = filter.url_pattern {
                if !e.url.contains(pat.as_str()) {
                    return false;
                }
            }

            // HTTP methods
            if let Some(ref methods) = filter.methods {
                if !methods.iter().any(|m| m.eq_ignore_ascii_case(&e.method)) {
                    return false;
                }
            }

            // Resource types
            if let Some(ref types) = filter.resource_types {
                match &e.resource_type {
                    Some(rt) => {
                        if !types.iter().any(|t| t.eq_ignore_ascii_case(rt)) {
                            return false;
                        }
                    }
                    None => return false,
                }
            }

            // Status range
            if let Some((min, max)) = filter.status_range {
                match e.status {
                    Some(s) => {
                        if s < min || s > max {
                            return false;
                        }
                    }
                    None => return false,
                }
            }

            // Has error
            if let Some(errors_only) = filter.has_error {
                if errors_only {
                    match e.status {
                        Some(s) => {
                            if s < 400 && s != 0 {
                                return false;
                            }
                        }
                        None => return false,
                    }
                }
            }

            true
        })
        .collect();

    // Limit
    if let Some(limit) = filter.limit {
        result.truncate(limit);
    }

    result
}

/// Build aggregate summary statistics from a slice of entries.
fn build_summary(entries: &[NetworkEntry]) -> NetworkSummary {
    let total_requests = entries.len();

    let mut by_type: HashMap<String, usize> = HashMap::new();
    let mut by_status: HashMap<String, usize> = HashMap::new();
    let mut total_bytes: u64 = 0;
    let mut failed_count: usize = 0;
    let mut cached_count: usize = 0;

    for entry in entries {
        // By type
        let rtype = entry
            .resource_type
            .as_deref()
            .unwrap_or("unknown")
            .to_string();
        *by_type.entry(rtype).or_insert(0) += 1;

        // By status range
        if let Some(status) = entry.status {
            let range_key = match status {
                0 => "0xx".to_string(),
                100..=199 => "1xx".to_string(),
                200..=299 => "2xx".to_string(),
                300..=399 => "3xx".to_string(),
                400..=499 => "4xx".to_string(),
                500..=599 => "5xx".to_string(),
                _ => "other".to_string(),
            };
            *by_status.entry(range_key).or_insert(0) += 1;

            if status >= 400 || status == 0 {
                failed_count += 1;
            }
        } else {
            *by_status.entry("unknown".to_string()).or_insert(0) += 1;
        }

        // Total bytes
        if let Some(cl) = entry.content_length {
            total_bytes += cl;
        }

        // Cached
        if entry.from_cache {
            cached_count += 1;
        }
    }

    NetworkSummary {
        total_requests,
        by_type: serde_json::to_value(by_type).unwrap_or(json!({})),
        by_status: serde_json::to_value(by_status).unwrap_or(json!({})),
        total_bytes,
        failed_count,
        cached_count,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_network_filter_default() {
        let f = NetworkFilter::default();
        assert!(f.url_pattern.is_none());
        assert!(f.methods.is_none());
        assert!(f.resource_types.is_none());
        assert!(f.status_range.is_none());
        assert!(f.has_error.is_none());
        assert!(f.limit.is_none());
    }

    fn mock_entries() -> Vec<NetworkEntry> {
        vec![
            NetworkEntry {
                id: "net-0".into(),
                method: "GET".into(),
                url: "https://example.com/api/users".into(),
                status: Some(200),
                status_text: Some("OK".into()),
                resource_type: Some("xhr".into()),
                request_headers: None,
                response_headers: None,
                request_body: None,
                response_body: None,
                content_type: Some("application/json".into()),
                content_length: Some(512),
                timing: None,
                initiator: Some("script".into()),
                from_cache: false,
                timestamp: "2025-01-01T00:00:00Z".into(),
            },
            NetworkEntry {
                id: "net-1".into(),
                method: "POST".into(),
                url: "https://example.com/api/login".into(),
                status: Some(401),
                status_text: Some("Unauthorized".into()),
                resource_type: Some("xhr".into()),
                request_headers: None,
                response_headers: None,
                request_body: Some(r#"{"user":"a"}"#.into()),
                response_body: None,
                content_type: Some("application/json".into()),
                content_length: Some(64),
                timing: None,
                initiator: Some("script".into()),
                from_cache: false,
                timestamp: "2025-01-01T00:00:01Z".into(),
            },
            NetworkEntry {
                id: "net-2".into(),
                method: "GET".into(),
                url: "https://cdn.example.com/style.css".into(),
                status: Some(200),
                status_text: Some("OK".into()),
                resource_type: Some("stylesheet".into()),
                request_headers: None,
                response_headers: None,
                request_body: None,
                response_body: None,
                content_type: Some("text/css".into()),
                content_length: Some(2048),
                timing: None,
                initiator: Some("parser".into()),
                from_cache: true,
                timestamp: "2025-01-01T00:00:02Z".into(),
            },
            NetworkEntry {
                id: "net-3".into(),
                method: "GET".into(),
                url: "https://example.com/script.js".into(),
                status: Some(304),
                status_text: Some("Not Modified".into()),
                resource_type: Some("script".into()),
                request_headers: None,
                response_headers: None,
                request_body: None,
                response_body: None,
                content_type: Some("application/javascript".into()),
                content_length: None,
                timing: None,
                initiator: Some("parser".into()),
                from_cache: false,
                timestamp: "2025-01-01T00:00:03Z".into(),
            },
            NetworkEntry {
                id: "net-4".into(),
                method: "GET".into(),
                url: "https://example.com/missing".into(),
                status: Some(500),
                status_text: Some("Internal Server Error".into()),
                resource_type: Some("fetch".into()),
                request_headers: None,
                response_headers: None,
                request_body: None,
                response_body: None,
                content_type: None,
                content_length: Some(128),
                timing: None,
                initiator: Some("script".into()),
                from_cache: false,
                timestamp: "2025-01-01T00:00:04Z".into(),
            },
        ]
    }

    #[test]
    fn test_build_summary() {
        let entries = mock_entries();
        let summary = build_summary(&entries);

        assert_eq!(summary.total_requests, 5);
        assert_eq!(summary.total_bytes, 512 + 64 + 2048 + 128);
        assert_eq!(summary.failed_count, 2); // 401 and 500
        assert_eq!(summary.cached_count, 1); // style.css

        // Check by_type
        let by_type = summary.by_type.as_object().unwrap();
        assert_eq!(by_type.get("xhr").and_then(|v| v.as_u64()), Some(2));
        assert_eq!(by_type.get("stylesheet").and_then(|v| v.as_u64()), Some(1));
        assert_eq!(by_type.get("script").and_then(|v| v.as_u64()), Some(1));
        assert_eq!(by_type.get("fetch").and_then(|v| v.as_u64()), Some(1));

        // Check by_status
        let by_status = summary.by_status.as_object().unwrap();
        assert_eq!(by_status.get("2xx").and_then(|v| v.as_u64()), Some(2));
        assert_eq!(by_status.get("3xx").and_then(|v| v.as_u64()), Some(1));
        assert_eq!(by_status.get("4xx").and_then(|v| v.as_u64()), Some(1));
        assert_eq!(by_status.get("5xx").and_then(|v| v.as_u64()), Some(1));
    }

    #[test]
    fn test_filter_url_pattern() {
        let entries = mock_entries();
        let filter = NetworkFilter {
            url_pattern: Some("/api/".into()),
            ..Default::default()
        };
        let result = apply_filter(entries, &filter);
        assert_eq!(result.len(), 2);
        assert!(result.iter().all(|e| e.url.contains("/api/")));
    }

    #[test]
    fn test_filter_methods() {
        let entries = mock_entries();
        let filter = NetworkFilter {
            methods: Some(vec!["POST".into()]),
            ..Default::default()
        };
        let result = apply_filter(entries, &filter);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].method, "POST");
    }

    #[test]
    fn test_filter_resource_types() {
        let entries = mock_entries();
        let filter = NetworkFilter {
            resource_types: Some(vec!["xhr".into(), "fetch".into()]),
            ..Default::default()
        };
        let result = apply_filter(entries, &filter);
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn test_filter_status_range() {
        let entries = mock_entries();
        let filter = NetworkFilter {
            status_range: Some((200, 299)),
            ..Default::default()
        };
        let result = apply_filter(entries, &filter);
        assert_eq!(result.len(), 2);
        assert!(result.iter().all(|e| e.status == Some(200)));
    }

    #[test]
    fn test_filter_has_error() {
        let entries = mock_entries();
        let filter = NetworkFilter {
            has_error: Some(true),
            ..Default::default()
        };
        let result = apply_filter(entries, &filter);
        assert_eq!(result.len(), 2);
        assert!(result.iter().all(|e| e.status.map_or(false, |s| s >= 400)));
    }

    #[test]
    fn test_filter_limit() {
        let entries = mock_entries();
        let filter = NetworkFilter {
            limit: Some(2),
            ..Default::default()
        };
        let result = apply_filter(entries, &filter);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_filter_combined() {
        let entries = mock_entries();
        let filter = NetworkFilter {
            url_pattern: Some("example.com".into()),
            methods: Some(vec!["GET".into()]),
            limit: Some(2),
            ..Default::default()
        };
        let result = apply_filter(entries, &filter);
        assert_eq!(result.len(), 2);
        assert!(result.iter().all(|e| e.method == "GET"));
        assert!(result.iter().all(|e| e.url.contains("example.com")));
    }

    #[test]
    fn test_parse_raw_entry_minimal() {
        let raw = json!({
            "method": "GET",
            "url": "https://test.com/",
            "status": 200,
            "type": "fetch",
            "timestamp": "2025-01-01T00:00:00Z",
        });
        let entry = parse_raw_entry(&raw, 7);
        assert_eq!(entry.id, "net-7");
        assert_eq!(entry.method, "GET");
        assert_eq!(entry.url, "https://test.com/");
        assert_eq!(entry.status, Some(200));
        assert_eq!(entry.resource_type.as_deref(), Some("fetch"));
        assert!(!entry.from_cache);
    }

    #[test]
    fn test_parse_raw_entry_with_timing() {
        let raw = json!({
            "id": "net-42",
            "method": "POST",
            "url": "https://test.com/submit",
            "status": 201,
            "statusText": "Created",
            "type": "xhr",
            "contentType": "application/json",
            "contentLength": 1024,
            "fromCache": false,
            "timestamp": "2025-06-01T12:00:00Z",
            "timing": {
                "startTime": 100.5,
                "duration": 250.3,
                "dns": 5.2,
                "connect": 12.0,
                "ssl": 8.1,
                "ttfb": 120.0,
                "download": 30.5,
            },
        });
        let entry = parse_raw_entry(&raw, 0);
        assert_eq!(entry.id, "net-42");
        assert_eq!(entry.method, "POST");
        assert_eq!(entry.content_length, Some(1024));

        let timing = entry.timing.unwrap();
        assert!((timing.started_at - 100.5).abs() < f64::EPSILON);
        assert!((timing.duration_ms - 250.3).abs() < f64::EPSILON);
        assert!((timing.dns_ms.unwrap() - 5.2).abs() < f64::EPSILON);
        assert!((timing.ttfb_ms.unwrap() - 120.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_build_summary_empty() {
        let summary = build_summary(&[]);
        assert_eq!(summary.total_requests, 0);
        assert_eq!(summary.total_bytes, 0);
        assert_eq!(summary.failed_count, 0);
        assert_eq!(summary.cached_count, 0);
    }

    #[test]
    fn test_network_entry_serialization_roundtrip() {
        let entry = NetworkEntry {
            id: "net-0".into(),
            method: "GET".into(),
            url: "https://example.com".into(),
            status: Some(200),
            status_text: Some("OK".into()),
            resource_type: Some("document".into()),
            request_headers: Some(json!({"Accept": "text/html"})),
            response_headers: None,
            request_body: None,
            response_body: None,
            content_type: Some("text/html".into()),
            content_length: Some(4096),
            timing: Some(NetworkTiming {
                started_at: 0.0,
                duration_ms: 100.0,
                dns_ms: Some(5.0),
                connect_ms: Some(10.0),
                ssl_ms: Some(8.0),
                ttfb_ms: Some(50.0),
                download_ms: Some(27.0),
            }),
            initiator: Some("user".into()),
            from_cache: false,
            timestamp: "2025-01-01T00:00:00Z".into(),
        };

        let json_str = serde_json::to_string(&entry).unwrap();
        let deserialized: NetworkEntry = serde_json::from_str(&json_str).unwrap();
        assert_eq!(deserialized.id, entry.id);
        assert_eq!(deserialized.status, entry.status);
        assert_eq!(deserialized.content_length, entry.content_length);
        assert!(deserialized.timing.is_some());
    }
}
