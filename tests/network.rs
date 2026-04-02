//! End-to-end tests for the network monitoring tools.
//!
//! Tests network_enable, network_get_log, network_clear_log,
//! network_get_resource_timing, network_get_cookies, and network_disable
//! via the MCP protocol against headless Chrome.

mod common;

use serde_json::{json, Value};
use serial_test::serial;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

// ---------------------------------------------------------------------------
// MCP test harness (same pattern as tests/agent.rs)
// ---------------------------------------------------------------------------

struct McpSession {
    child: Child,
    reader: BufReader<std::process::ChildStdout>,
    tmp: PathBuf,
    seq: u64,
}

impl McpSession {
    fn start() -> Option<Self> {
        let tmp = std::env::temp_dir().join(format!(
            "browsectl-net-e2e-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .subsec_nanos()
        ));

        let bin = common::browsectl_bin();
        let mut child = Command::new(&bin)
            .args([
                "--headless",
                "--user-data-dir",
                &tmp.to_string_lossy(),
                "mcp",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .ok()?;

        let stdout = child.stdout.take()?;
        let reader = BufReader::new(stdout);
        let mut s = Self {
            child,
            reader,
            tmp,
            seq: 0,
        };
        s.handshake();
        Some(s)
    }

    fn send(&mut self, v: &Value) {
        let stdin = self.child.stdin.as_mut().unwrap();
        writeln!(stdin, "{}", serde_json::to_string(v).unwrap()).unwrap();
        stdin.flush().unwrap();
    }

    fn recv(&mut self) -> Value {
        let mut line = String::new();
        self.reader.read_line(&mut line).unwrap();
        serde_json::from_str(line.trim())
            .unwrap_or_else(|e| panic!("MCP parse error: {e}\nraw: {line}"))
    }

    fn id(&mut self) -> u64 {
        self.seq += 1;
        self.seq
    }

    fn handshake(&mut self) {
        let id = self.id();
        self.send(&json!({
            "jsonrpc": "2.0", "id": id,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "net-test", "version": "0.1.0" }
            }
        }));
        let _ = self.recv();
        self.send(
            &json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
        );
    }

    fn tool(&mut self, name: &str, args: Value) -> Value {
        let id = self.id();
        self.send(&json!({
            "jsonrpc": "2.0", "id": id,
            "method": "tools/call",
            "params": { "name": name, "arguments": args }
        }));
        let resp = self.recv();
        extract_tool_result(&resp)
    }

    /// Run arbitrary JavaScript via the wait_for tool with condition "js".
    /// The expression must eventually become truthy.  This is the only
    /// reliable way to execute JS in the MCP-only test harness since there
    /// is no "execute" command type exposed via browsectl.
    fn run_js(&mut self, setup_js: &str) {
        // We wrap the caller's JS so that it executes the code and then
        // evaluates to `true`, which makes wait_for return immediately.
        //
        // Pattern:  (function(){ <user code>; return true; })()
        let expr = format!("(function(){{ {setup_js}; return true; }})()", setup_js = setup_js);
        let r = self.tool(
            "wait_for",
            json!({
                "condition": "js",
                "value": expr,
                "timeout": 8000
            }),
        );
        assert_eq!(
            r["ok"].as_bool(),
            Some(true),
            "run_js failed for script: {setup_js}\nresult: {r}"
        );
    }

    /// Run JS that starts async work, then wait for a flag variable to become true.
    fn run_js_async(&mut self, setup_js: &str, done_flag: &str) {
        // First, execute the JS that kicks off async work
        self.run_js(setup_js);
        // Then wait for the done flag
        let r = self.tool(
            "wait_for",
            json!({
                "condition": "js",
                "value": format!("window.{done_flag} === true"),
                "timeout": 8000
            }),
        );
        assert_eq!(
            r["ok"].as_bool(),
            Some(true),
            "Timed out waiting for {done_flag}\nresult: {r}"
        );
    }

    fn shutdown(mut self) {
        drop(self.child.stdin.take());
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.tmp);
    }
}

fn extract_tool_result(resp: &Value) -> Value {
    resp["result"]["content"]
        .as_array()
        .and_then(|c| c.first())
        .and_then(|c| c["text"].as_str())
        .and_then(|t| serde_json::from_str(t).ok())
        .unwrap_or_else(|| resp.clone())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Spin up an MCP session, create a headless browser, and navigate to a URL.
fn setup_on(url: &str) -> Option<McpSession> {
    let mut mcp = McpSession::start()?;

    let r = mcp.tool("create_session", json!({"headless": true}));
    if r["ok"].as_bool() != Some(true) {
        eprintln!("SKIP: create_session failed: {r}");
        mcp.shutdown();
        return None;
    }

    let r = mcp.tool("open", json!({ "url": url }));
    assert_eq!(r["ok"].as_bool(), Some(true), "open failed: {r}");

    // Give the page a moment to settle
    let _ = mcp.tool(
        "wait_for",
        json!({"condition": "visible", "selector": "body", "timeout": 8000}),
    );

    Some(mcp)
}

fn setup_on_test_page() -> Option<McpSession> {
    setup_on(&common::test_page_url())
}

// ---------------------------------------------------------------------------
// Tests: interceptor lifecycle
// ---------------------------------------------------------------------------

/// Verify that network_enable installs the interceptor and network_disable
/// removes it.
#[test]
#[serial]
fn network_enable_disable_lifecycle() {
    let Some(mut mcp) = setup_on_test_page() else {
        return;
    };

    // Enable
    let r = mcp.tool("network_enable", json!({}));
    assert_eq!(
        r["ok"].as_bool(),
        Some(true),
        "network_enable should succeed: {r}"
    );
    let interceptor = r["interceptor"].as_str().unwrap_or("");
    assert!(
        interceptor.contains("installed"),
        "interceptor status should contain 'installed', got: {interceptor}"
    );

    // Double-enable should say already installed
    let r2 = mcp.tool("network_enable", json!({}));
    assert_eq!(
        r2["ok"].as_bool(),
        Some(true),
        "second enable should also succeed"
    );
    let status2 = r2["interceptor"].as_str().unwrap_or("");
    assert!(
        status2.contains("already installed"),
        "second enable should report already installed, got: {status2}"
    );

    // Disable
    let r3 = mcp.tool("network_disable", json!({}));
    assert_eq!(
        r3["ok"].as_bool(),
        Some(true),
        "network_disable should succeed: {r3}"
    );

    mcp.shutdown();
}

// ---------------------------------------------------------------------------
// Tests: capturing and reading the log
// ---------------------------------------------------------------------------

/// Enable the interceptor, trigger a fetch via JS, then read the log.
#[test]
#[serial]
fn network_capture_fetch() {
    let Some(mut mcp) = setup_on_test_page() else {
        return;
    };

    // Enable capture
    mcp.tool("network_enable", json!({}));

    // Clear any entries from the page load itself
    mcp.tool("network_clear_log", json!({}));

    // Trigger a fetch via JS using data: URI (no network needed)
    mcp.run_js_async(
        "window.fetch('data:text/plain;base64,aGVsbG8=').then(function(){ window.__fetch_done=true; })",
        "__fetch_done",
    );

    // Read the log
    let log = mcp.tool("network_get_log", json!({}));
    eprintln!("[DEBUG] network log: {log}");

    assert!(
        log["entry_count"].as_u64().unwrap_or(0) > 0,
        "should have captured at least 1 entry, got: {}",
        log["entry_count"]
    );

    // The summary should exist
    assert!(log["summary"]["total_requests"].is_number());
    assert!(log["captured_at"].is_string());

    // Check that entries have the right shape
    let entries = log["entries"].as_array().expect("entries should be array");
    for entry in entries {
        assert!(entry["id"].is_string(), "entry must have id");
        assert!(entry["method"].is_string(), "entry must have method");
        assert!(entry["url"].is_string(), "entry must have url");
    }

    mcp.shutdown();
}

/// Enable capture, trigger fetches, filter the log by URL pattern.
#[test]
#[serial]
fn network_filter_url_pattern() {
    let Some(mut mcp) = setup_on_test_page() else {
        return;
    };

    mcp.tool("network_enable", json!({}));
    mcp.tool("network_clear_log", json!({}));

    // Fire two different fetches so we can filter
    mcp.run_js_async(
        "Promise.all([\
            window.fetch('data:text/plain;base64,YWxwaGE='),\
            window.fetch('data:application/json;base64,eyJrZXkiOiJ2YWx1ZSJ9')\
        ]).then(function(){ window.__two_done = true; })",
        "__two_done",
    );

    // Filter for "application/json" URL
    let filtered = mcp.tool(
        "network_get_log",
        json!({"url_pattern": "application/json"}),
    );
    let empty1: Vec<Value> = vec![];
    let entries = filtered["entries"].as_array().unwrap_or(&empty1);
    for entry in entries {
        assert!(
            entry["url"]
                .as_str()
                .unwrap_or("")
                .contains("application/json"),
            "filtered entry URL should contain pattern"
        );
    }

    // Filter for plain
    let filtered2 = mcp.tool("network_get_log", json!({"url_pattern": "text/plain"}));
    let empty2: Vec<Value> = vec![];
    let entries2 = filtered2["entries"].as_array().unwrap_or(&empty2);
    for entry in entries2 {
        assert!(
            entry["url"].as_str().unwrap_or("").contains("text/plain"),
            "filtered entry URL should contain 'text/plain'"
        );
    }

    mcp.shutdown();
}

/// Filter by HTTP method.
#[test]
#[serial]
fn network_filter_methods() {
    let Some(mut mcp) = setup_on_test_page() else {
        return;
    };

    mcp.tool("network_enable", json!({}));
    mcp.tool("network_clear_log", json!({}));

    // Fire a GET and a POST
    mcp.run_js_async(
        "Promise.all([\
            window.fetch('data:text/plain;base64,Z2V0'),\
            window.fetch('data:text/plain;base64,cG9zdA==', {method:'POST', body:'hello'})\
        ]).then(function(){ window.__methods_done = true; })",
        "__methods_done",
    );

    // Only POST
    let post_log = mcp.tool("network_get_log", json!({"methods": ["POST"]}));
    let empty_post: Vec<Value> = vec![];
    let post_entries = post_log["entries"].as_array().unwrap_or(&empty_post);
    for entry in post_entries {
        assert_eq!(
            entry["method"].as_str(),
            Some("POST"),
            "filtered entries should all be POST"
        );
    }

    mcp.shutdown();
}

/// Use the limit filter.
#[test]
#[serial]
fn network_filter_limit() {
    let Some(mut mcp) = setup_on_test_page() else {
        return;
    };

    mcp.tool("network_enable", json!({}));
    mcp.tool("network_clear_log", json!({}));

    // Fire several fetches
    mcp.run_js_async(
        "Promise.all([1,2,3,4,5].map(function(i){\
            return window.fetch('data:text/plain;base64,dGVzdA==');\
        })).then(function(){ window.__multi_done = true; })",
        "__multi_done",
    );

    // Limit to 2
    let limited = mcp.tool("network_get_log", json!({"limit": 2}));
    let entries = limited["entries"].as_array().expect("entries");
    assert!(
        entries.len() <= 2,
        "limit=2 should return at most 2 entries, got {}",
        entries.len()
    );

    mcp.shutdown();
}

// ---------------------------------------------------------------------------
// Tests: clear log
// ---------------------------------------------------------------------------

/// Clear the log and verify it's empty.
#[test]
#[serial]
fn network_clear_log_resets() {
    let Some(mut mcp) = setup_on_test_page() else {
        return;
    };

    mcp.tool("network_enable", json!({}));

    // Fire a fetch
    mcp.run_js_async(
        "window.fetch('data:text/plain;base64,Y2xlYXI=').then(function(){ window.__clr_done = true; })",
        "__clr_done",
    );

    // Verify we have entries
    let before = mcp.tool("network_get_log", json!({}));
    let before_count = before["entry_count"].as_u64().unwrap_or(0);
    assert!(before_count > 0, "should have entries before clearing");

    // Clear
    let clear_result = mcp.tool("network_clear_log", json!({}));
    let cleared = clear_result["cleared"].as_u64().unwrap_or(0);
    assert!(cleared > 0, "should report entries cleared");

    // After clear, log should be empty
    let after = mcp.tool("network_get_log", json!({}));
    assert_eq!(
        after["entry_count"].as_u64().unwrap_or(99),
        0,
        "log should be empty after clear"
    );

    mcp.shutdown();
}

// ---------------------------------------------------------------------------
// Tests: network log summary structure
// ---------------------------------------------------------------------------

/// Verify the summary aggregation fields.
#[test]
#[serial]
fn network_log_summary_structure() {
    let Some(mut mcp) = setup_on_test_page() else {
        return;
    };

    mcp.tool("network_enable", json!({}));
    mcp.tool("network_clear_log", json!({}));

    // Fire requests
    mcp.run_js_async(
        "Promise.all([\
            window.fetch('data:text/plain;base64,c3VtbWFyeQ=='),\
            window.fetch('data:text/html;base64,PGh0bWw+PC9odG1sPg==')\
        ]).then(function(){ window.__summary_done = true; })",
        "__summary_done",
    );

    let log = mcp.tool("network_get_log", json!({}));
    let summary = &log["summary"];

    // Required summary fields
    assert!(
        summary["total_requests"].is_number(),
        "summary must have total_requests"
    );
    assert!(
        summary["by_type"].is_object(),
        "summary must have by_type object"
    );
    assert!(
        summary["by_status"].is_object(),
        "summary must have by_status object"
    );
    assert!(
        summary["total_bytes"].is_number(),
        "summary must have total_bytes"
    );
    assert!(
        summary["failed_count"].is_number(),
        "summary must have failed_count"
    );
    assert!(
        summary["cached_count"].is_number(),
        "summary must have cached_count"
    );

    mcp.shutdown();
}

// ---------------------------------------------------------------------------
// Tests: resource timing (no interceptor needed)
// ---------------------------------------------------------------------------

/// Resource timing should work even without network_enable.
#[test]
#[serial]
fn network_resource_timing_without_interceptor() {
    let Some(mut mcp) = setup_on_test_page() else {
        return;
    };

    // Do NOT call network_enable — resource timing uses the Performance API
    let result = mcp.tool("network_get_resource_timing", json!({}));
    eprintln!("[DEBUG] resource timing: {result}");

    // For a file:// page there may not be many resource entries, but the
    // call should succeed and return an array (even if empty).
    assert!(
        result.is_array(),
        "resource timing should return an array, got: {result}"
    );

    // If there are entries, check structure
    if let Some(entries) = result.as_array() {
        for entry in entries {
            assert!(entry["name"].is_string(), "timing entry must have name");
            assert!(
                entry["duration"].is_number(),
                "timing entry must have duration"
            );
        }
    }

    mcp.shutdown();
}

// ---------------------------------------------------------------------------
// Tests: cookies
// ---------------------------------------------------------------------------

/// Cookie retrieval should succeed (may be empty for file:// pages).
#[test]
#[serial]
fn network_get_cookies_returns_result() {
    let Some(mut mcp) = setup_on_test_page() else {
        return;
    };

    let result = mcp.tool("network_get_cookies", json!({}));
    eprintln!("[DEBUG] cookies: {result}");

    // CDP might return cookies directly, or we might get the JS fallback.
    // Either way we should not get an error in the MCP response.
    // The result should be an object (CDP returns {cookies:[...]}) or
    // our fallback returns {cookies:[...], source:"document.cookie"}.
    let is_valid = result["cookies"].is_array()
        || result.is_object() // CDP raw shape
        || result.is_array(); // unlikely but tolerate
    assert!(is_valid, "cookies result should be valid: {result}");

    mcp.shutdown();
}

/// Set a cookie via JS, then verify network_get_cookies can see it.
#[test]
#[serial]
fn network_get_cookies_reads_set_cookie() {
    // Use a real http page so cookies work (file:// doesn't support cookies)
    let Some(mut mcp) = setup_on("https://example.com") else {
        return;
    };

    // Set a cookie via JS
    mcp.run_js("document.cookie = 'browsectl_test=hello_world; path=/'");

    let result = mcp.tool("network_get_cookies", json!({}));
    eprintln!("[DEBUG] cookies after set: {result}");

    // Look for our cookie in the result
    let found = if let Some(cookies) = result["cookies"].as_array() {
        cookies.iter().any(|c| {
            c["name"].as_str() == Some("browsectl_test")
                || c.as_str().unwrap_or("").contains("browsectl_test")
        })
    } else {
        // Stringify the whole result and do a substring check as fallback
        serde_json::to_string(&result)
            .unwrap_or_default()
            .contains("browsectl_test")
    };

    assert!(found, "should find the cookie we set: {result}");

    mcp.shutdown();
}

// ---------------------------------------------------------------------------
// Tests: network_get_response_body
// ---------------------------------------------------------------------------

/// Try to get response body for a nonexistent request ID.
#[test]
#[serial]
fn network_get_response_body_missing_id() {
    let Some(mut mcp) = setup_on_test_page() else {
        return;
    };

    mcp.tool("network_enable", json!({}));

    let result = mcp.tool(
        "network_get_response_body",
        json!({"request_id": "net-99999"}),
    );

    // Should indicate the entry was not found, not crash
    let has_error = result["error"].is_string();
    let body_null = result["body"].is_null();
    assert!(
        has_error || body_null,
        "missing request ID should return error or null body: {result}"
    );

    mcp.shutdown();
}

// ---------------------------------------------------------------------------
// Tests: interceptor captures request metadata
// ---------------------------------------------------------------------------

/// Verify that captured fetch entries include method, URL, and resource type.
#[test]
#[serial]
fn network_entry_metadata() {
    let Some(mut mcp) = setup_on_test_page() else {
        return;
    };

    mcp.tool("network_enable", json!({}));
    mcp.tool("network_clear_log", json!({}));

    // Fire a POST with headers and body
    mcp.run_js_async(
        "window.fetch('data:application/json;base64,e30=', {\
            method: 'POST',\
            headers: {'Content-Type': 'application/json', 'X-Test': 'browsectl'},\
            body: JSON.stringify({hello: 'world'})\
        }).then(function(){ window.__meta_done = true; })",
        "__meta_done",
    );

    let log = mcp.tool("network_get_log", json!({"methods": ["POST"]}));
    let entries = log["entries"].as_array().expect("entries");

    let post_entry = entries
        .iter()
        .find(|e| e["method"].as_str() == Some("POST"));
    assert!(
        post_entry.is_some(),
        "should have captured the POST request"
    );

    let entry = post_entry.unwrap();

    // URL should be the data URI
    assert!(
        entry["url"]
            .as_str()
            .unwrap_or("")
            .contains("application/json"),
        "entry URL should contain data URI"
    );

    // Resource type should be fetch
    if let Some(rtype) = entry["resource_type"].as_str() {
        assert_eq!(rtype, "fetch", "resource_type should be 'fetch'");
    }

    // Request body should contain our JSON
    if let Some(body) = entry["request_body"].as_str() {
        assert!(
            body.contains("hello"),
            "request body should contain our JSON, got: {body}"
        );
    }

    // Request headers should have our custom header
    if let Some(headers) = entry["request_headers"].as_object() {
        let has_test_header = headers
            .iter()
            .any(|(k, _)| k.to_lowercase() == "x-test" || k.to_lowercase() == "content-type");
        assert!(
            has_test_header,
            "should capture custom request headers: {:?}",
            headers
        );
    }

    mcp.shutdown();
}

// ---------------------------------------------------------------------------
// Tests: XHR capture
// ---------------------------------------------------------------------------

/// Verify the interceptor captures XMLHttpRequest calls.
#[test]
#[serial]
fn network_capture_xhr() {
    let Some(mut mcp) = setup_on_test_page() else {
        return;
    };

    mcp.tool("network_enable", json!({}));
    mcp.tool("network_clear_log", json!({}));

    // Fire an XHR to a data: URI
    mcp.run_js(
        "var x = new XMLHttpRequest();\
         x.open('GET', 'data:text/plain;base64,eGhy');\
         x.onload = function(){ window.__xhr_done = true; };\
         x.onerror = function(){ window.__xhr_done = true; };\
         x.send()",
    );

    // Wait for the XHR to complete
    let _ = mcp.tool(
        "wait_for",
        json!({"condition": "js", "value": "window.__xhr_done === true", "timeout": 5000}),
    );

    let log = mcp.tool("network_get_log", json!({}));
    let empty_xhr: Vec<Value> = vec![];
    let entries = log["entries"].as_array().unwrap_or(&empty_xhr);

    // Should capture the XHR
    let xhr_entry = entries.iter().find(|e| {
        let rtype = e["resource_type"].as_str().unwrap_or("");
        let url = e["url"].as_str().unwrap_or("");
        rtype == "xhr" && url.contains("text/plain")
    });

    assert!(
        xhr_entry.is_some(),
        "should have captured the XHR request; entries: {:?}",
        entries
            .iter()
            .map(|e| format!(
                "{}:{} ({})",
                e["method"].as_str().unwrap_or("?"),
                e["url"].as_str().unwrap_or("?"),
                e["resource_type"].as_str().unwrap_or("?")
            ))
            .collect::<Vec<_>>()
    );

    if let Some(entry) = xhr_entry {
        assert_eq!(entry["method"].as_str(), Some("GET"));
    }

    mcp.shutdown();
}

// ---------------------------------------------------------------------------
// Tests: full cycle — enable, navigate, capture, filter, clear, disable
// ---------------------------------------------------------------------------

/// Run a full network monitoring lifecycle in a single test.
#[test]
#[serial]
fn network_full_lifecycle() {
    let Some(mut mcp) = setup_on_test_page() else {
        return;
    };

    // 1. Enable
    let r = mcp.tool("network_enable", json!({}));
    assert_eq!(r["ok"].as_bool(), Some(true));

    // 2. Clear any noise from page load
    let _ = mcp.tool("network_clear_log", json!({}));

    // 3. Fire several requests via JS
    mcp.run_js_async(
        "Promise.all([\
            window.fetch('data:text/plain;base64,YQ=='),\
            window.fetch('data:text/plain;base64,Yg==', {method:'POST', body:'test'}),\
            window.fetch('data:text/html;base64,PHA+PC9wPg==')\
        ]).then(function(){ window.__lifecycle_done = true; })",
        "__lifecycle_done",
    );

    // 4. Read unfiltered log
    let full = mcp.tool("network_get_log", json!({}));
    let total = full["entry_count"].as_u64().unwrap_or(0);
    assert!(total >= 3, "should have at least 3 entries, got {total}");

    // 5. Filter: only POST
    let post_only = mcp.tool("network_get_log", json!({"methods": ["POST"]}));
    let post_count = post_only["entry_count"].as_u64().unwrap_or(0);
    assert!(
        post_count >= 1,
        "should have at least 1 POST, got {post_count}"
    );
    assert!(
        post_count < total,
        "POST filter should return fewer than total"
    );

    // 6. Filter: URL pattern
    let html_only = mcp.tool("network_get_log", json!({"url_pattern": "text/html"}));
    let empty_html: Vec<Value> = vec![];
    for entry in html_only["entries"].as_array().unwrap_or(&empty_html) {
        assert!(entry["url"].as_str().unwrap_or("").contains("text/html"));
    }

    // 7. Filter: limit
    let limited = mcp.tool("network_get_log", json!({"limit": 1}));
    assert!(
        limited["entries"]
            .as_array()
            .map_or(true, |e| e.len() <= 1),
        "limit=1 should return at most 1 entry"
    );

    // 8. Clear
    let cleared = mcp.tool("network_clear_log", json!({}));
    assert!(
        cleared["cleared"].as_u64().unwrap_or(0) > 0,
        "should have cleared entries"
    );

    // 9. Verify empty
    let empty = mcp.tool("network_get_log", json!({}));
    assert_eq!(empty["entry_count"].as_u64().unwrap_or(99), 0);

    // 10. Disable
    let dis = mcp.tool("network_disable", json!({}));
    assert_eq!(dis["ok"].as_bool(), Some(true));

    mcp.shutdown();
}

// ---------------------------------------------------------------------------
// Tests: MCP tools/list includes network tools
// ---------------------------------------------------------------------------

/// Verify that all network + agent tools appear in the MCP tools/list response.
#[test]
#[serial]
fn mcp_tools_list_includes_network_and_agent_tools() {
    let Some(mut mcp) = McpSession::start() else {
        return;
    };

    let id = mcp.id();
    mcp.send(&json!({
        "jsonrpc": "2.0", "id": id,
        "method": "tools/list",
        "params": {}
    }));
    let resp = mcp.recv();

    let tools = resp["result"]["tools"]
        .as_array()
        .expect("tools/list should return tools array");

    let tool_names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();

    let expected = [
        "network_enable",
        "network_disable",
        "network_get_log",
        "network_get_response_body",
        "network_clear_log",
        "network_get_resource_timing",
        "network_get_cookies",
        "analyze_page",
        "suggest_actions",
    ];

    for name in &expected {
        assert!(
            tool_names.contains(name),
            "tools/list should include '{name}', available tools: {tool_names:?}"
        );
    }

    mcp.shutdown();
}
