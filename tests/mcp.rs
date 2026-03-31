//! MCP (Model Context Protocol) integration tests.
//!
//! These tests start `browsectl mcp` as a subprocess and communicate over
//! JSON-RPC / NDJSON via stdin/stdout.  They are the heaviest tests and
//! require chromedriver + Chrome.
//!
//! # Running
//!
//! ```sh
//! cargo test --test mcp -- --test-threads=1
//! ```

mod common;

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

use serde_json::Value;
use serial_test::serial;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

struct McpChild {
    stdin: std::process::ChildStdin,
    reader: BufReader<std::process::ChildStdout>,
    child: std::process::Child,
    tmp: std::path::PathBuf,
}

impl McpChild {
    /// Spawn `browsectl mcp` in headless mode with a fresh temp profile.
    fn spawn() -> Self {
        let tmp =
            std::env::temp_dir().join(format!("browsectl-e2e-mcp-{}-{}", std::process::id(), {
                use std::time::SystemTime;
                SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap_or_default()
                    .subsec_nanos()
            }));
        let _ = std::fs::create_dir_all(&tmp);

        let bin = common::browsectl_bin();
        let mut child = Command::new(&bin)
            .arg("--headless")
            .arg("--user-data-dir")
            .arg(tmp.to_string_lossy().to_string())
            .arg("mcp")
            .current_dir(common::project_root())
            .env("NO_COLOR", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to spawn mcp process");

        let stdin = child.stdin.take().expect("missing mcp stdin");
        let stdout = child.stdout.take().expect("missing mcp stdout");
        let reader = BufReader::new(stdout);

        McpChild {
            stdin,
            reader,
            child,
            tmp,
        }
    }

    /// Send a JSON-RPC message (appends newline).
    fn send(&mut self, msg: &str) {
        self.stdin
            .write_all(msg.as_bytes())
            .expect("write mcp request failed");
        self.stdin.write_all(b"\n").expect("write newline failed");
        self.stdin.flush().expect("flush failed");
    }

    /// Read one JSON line from stdout.
    fn read_json(&mut self) -> Value {
        let mut line = String::new();
        self.reader
            .read_line(&mut line)
            .expect("read mcp line failed");
        serde_json::from_str(line.trim())
            .unwrap_or_else(|e| panic!("invalid mcp json line: {e}\nline={line}"))
    }

    /// Extract the parsed JSON from a tools/call result content[0].text.
    fn extract_tool_result(&mut self) -> Value {
        let resp = self.read_json();
        let text = resp["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or("{}");
        serde_json::from_str(text).unwrap_or(Value::Null)
    }

    /// Send initialize and return the response.
    fn initialize(&mut self) -> Value {
        self.send(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"e2e","version":"1.0"}}}"#,
        );
        self.read_json()
    }

    /// Shut down: close stdin, wait for child, remove temp dir.
    fn shutdown(self) {
        drop(self.stdin);
        let mut child = self.child;
        let _ = child.wait();
        let _ = std::fs::remove_dir_all(&self.tmp);
    }
}

// ---------------------------------------------------------------------------
// tools/list
// ---------------------------------------------------------------------------

/// MCP tools/list should return all registered tools.
#[test]
#[serial]
fn mcp_tools_list() {
    let mut mcp = McpChild::spawn();
    mcp.initialize();

    mcp.send(r#"{"jsonrpc":"2.0","id":10,"method":"tools/list","params":{}}"#);
    let resp = mcp.read_json();

    let tools = resp["result"]["tools"]
        .as_array()
        .expect("tools should be an array");
    assert!(
        tools.len() >= 15,
        "should have at least 15 tools, got {}",
        tools.len()
    );

    // Spot-check a few tool names.
    let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    assert!(names.contains(&"open"), "should have 'open' tool");
    assert!(names.contains(&"click"), "should have 'click' tool");
    assert!(
        names.contains(&"screenshot"),
        "should have 'screenshot' tool"
    );
    assert!(
        names.contains(&"create_session"),
        "should have 'create_session' tool"
    );
    assert!(
        names.contains(&"run_command"),
        "should have 'run_command' tool"
    );

    mcp.shutdown();
}

// ---------------------------------------------------------------------------
// ping
// ---------------------------------------------------------------------------

/// MCP ping should return empty result.
#[test]
#[serial]
fn mcp_ping() {
    let mut mcp = McpChild::spawn();
    mcp.initialize();

    mcp.send(r#"{"jsonrpc":"2.0","id":20,"method":"ping","params":{}}"#);
    let resp = mcp.read_json();

    assert!(
        resp["result"].is_object(),
        "ping should return a result object: {resp}"
    );

    mcp.shutdown();
}

// ---------------------------------------------------------------------------
// Screenshot with inline base64
// ---------------------------------------------------------------------------

/// MCP screenshot with `inline:true` returns base64 payload.
#[test]
#[serial]
fn mcp_screenshot_inline_base64() {
    let mut mcp = McpChild::spawn();
    mcp.initialize();

    // create_session
    mcp.send(
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"create_session","arguments":{"headless":true}}}"#,
    );
    let create = mcp.extract_tool_result();
    assert_eq!(
        create["ok"].as_bool(),
        Some(true),
        "mcp create_session failed: {create}"
    );

    // open local fixture page
    let url = common::test_page_url();
    let open_req = format!(
        r#"{{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{{"name":"open","arguments":{{"url":"{}"}}}}}}"#,
        url.replace('\\', "\\\\").replace('"', "\\\"")
    );
    mcp.send(&open_req);
    let open = mcp.extract_tool_result();
    assert_eq!(open["ok"].as_bool(), Some(true), "mcp open failed: {open}");

    // screenshot inline=true
    mcp.send(
        r##"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"screenshot","arguments":{"selector":"#heading","path":"outputs/e2e-inline.png","inline":true}}}"##,
    );
    let shot = mcp.extract_tool_result();

    assert_eq!(
        shot["ok"].as_bool(),
        Some(true),
        "screenshot should succeed: {shot}"
    );
    assert_eq!(shot["type"].as_str(), Some("screenshot"));
    assert_eq!(
        shot["mime"].as_str(),
        Some("image/png"),
        "mime must be image/png: {shot}"
    );
    assert_eq!(
        shot["inline"].as_bool(),
        Some(true),
        "inline flag should be true: {shot}"
    );

    let b64 = shot["base64"].as_str().unwrap_or("");
    assert!(
        !b64.is_empty() && b64.len() > 100,
        "base64 payload should be non-empty and valid: len={}",
        b64.len()
    );

    // Also verify saved=true and path is present
    assert_eq!(
        shot["saved"].as_bool(),
        Some(true),
        "saved should be true: {shot}"
    );
    assert!(
        shot["path"].as_str().is_some(),
        "path should be present: {shot}"
    );
    assert!(
        shot["uri"].as_str().is_some(),
        "uri should be present: {shot}"
    );

    mcp.shutdown();
}

// ---------------------------------------------------------------------------
// Screenshot without inline (default)
// ---------------------------------------------------------------------------

/// MCP screenshot without `inline` should NOT include base64.
#[test]
#[serial]
fn mcp_screenshot_no_inline() {
    let mut mcp = McpChild::spawn();
    mcp.initialize();

    // create_session
    mcp.send(
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"create_session","arguments":{"headless":true}}}"#,
    );
    let create = mcp.extract_tool_result();
    assert_eq!(
        create["ok"].as_bool(),
        Some(true),
        "create_session failed: {create}"
    );

    // open
    let url = common::test_page_url();
    let open_req = format!(
        r#"{{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{{"name":"open","arguments":{{"url":"{}"}}}}}}"#,
        url.replace('\\', "\\\\").replace('"', "\\\"")
    );
    mcp.send(&open_req);
    let open = mcp.extract_tool_result();
    assert_eq!(open["ok"].as_bool(), Some(true), "open failed: {open}");

    // screenshot without inline
    mcp.send(
        r##"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"screenshot","arguments":{"selector":"#heading","path":"outputs/e2e-noinline.png"}}}"##,
    );
    let shot = mcp.extract_tool_result();

    assert_eq!(
        shot["ok"].as_bool(),
        Some(true),
        "screenshot should succeed: {shot}"
    );
    assert_eq!(
        shot["inline"].as_bool(),
        Some(false),
        "inline should be false: {shot}"
    );
    assert!(
        shot.get("base64").is_none(),
        "base64 should NOT be present: {shot}"
    );
    assert!(
        shot["path"].as_str().is_some(),
        "path should be present: {shot}"
    );
    assert!(
        shot["uri"].as_str().is_some(),
        "uri should be present: {shot}"
    );

    mcp.shutdown();
}

// ---------------------------------------------------------------------------
// MCP open + title round-trip
// ---------------------------------------------------------------------------

/// MCP open URL then get_title to verify navigation worked.
#[test]
#[serial]
fn mcp_open_and_title() {
    let mut mcp = McpChild::spawn();
    mcp.initialize();

    // create_session
    mcp.send(
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"create_session","arguments":{"headless":true}}}"#,
    );
    let create = mcp.extract_tool_result();
    assert_eq!(
        create["ok"].as_bool(),
        Some(true),
        "create_session failed: {create}"
    );

    // open
    mcp.send(
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"open","arguments":{"url":"https://example.com"}}}"#,
    );
    let open = mcp.extract_tool_result();
    assert_eq!(open["ok"].as_bool(), Some(true), "open failed: {open}");

    // get_title
    mcp.send(
        r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"get_title","arguments":{}}}"#,
    );
    let title = mcp.extract_tool_result();
    assert_eq!(
        title["ok"].as_bool(),
        Some(true),
        "get_title failed: {title}"
    );
    assert!(
        title["title"]
            .as_str()
            .unwrap_or("")
            .contains("Example Domain"),
        "title should contain 'Example Domain': {title}"
    );

    mcp.shutdown();
}
