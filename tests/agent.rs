//! End-to-end tests for the agent intelligence layer (analyze_page, suggest_actions).
//! Tests run headless Chrome via the MCP protocol against the local test fixture.

mod common;

use serde_json::{Value, json};
use serial_test::serial;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

// ---------------------------------------------------------------------------
// MCP test harness
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
            "browsectl-agent-e2e-{}-{}",
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
                "clientInfo": { "name": "agent-test", "version": "0.1.0" }
            }
        }));
        let _ = self.recv();
        self.send(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
    }

    /// Call an MCP tool and return the parsed JSON from the tool result text.
    fn tool(&mut self, name: &str, args: Value) -> Value {
        let id = self.id();
        self.send(&json!({
            "jsonrpc": "2.0", "id": id,
            "method": "tools/call",
            "params": { "name": name, "arguments": args }
        }));
        let resp = self.recv();
        // Extract text from resp.result.content[0].text and parse as JSON
        extract_tool_result(&resp)
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

fn find_slot<'a>(slots: &'a [Value], selector_contains: &str) -> Option<&'a Value> {
    slots.iter().find(|s| {
        s["selector"]
            .as_str()
            .map_or(false, |sel| sel.contains(selector_contains))
    })
}

fn find_slot_by_text<'a>(slots: &'a [Value], text: &str) -> Option<&'a Value> {
    slots.iter().find(|s| {
        s["text"]
            .as_str()
            .map_or(false, |t| t.to_lowercase().contains(&text.to_lowercase()))
    })
}

/// Set up an MCP session, create a browser session, and navigate to the test page.
/// Returns the McpSession ready for tool calls against the loaded test page.
fn setup_on_test_page() -> Option<McpSession> {
    let mut mcp = McpSession::start()?;

    // Create session
    let r = mcp.tool("create_session", json!({"headless": true}));
    if r["ok"].as_bool() != Some(true) {
        eprintln!("SKIP: create_session failed: {r}");
        mcp.shutdown();
        return None;
    }

    // Navigate to test page
    let url = common::test_page_url();
    let r = mcp.tool("open", json!({"url": url}));
    assert_eq!(r["ok"].as_bool(), Some(true), "open failed: {r}");

    // Wait for agent test section to be visible
    let r = mcp.tool(
        "wait_for",
        json!({
            "condition": "visible",
            "selector": "#agent-test-section",
            "timeout": 10000
        }),
    );
    assert_eq!(r["ok"].as_bool(), Some(true), "wait_for failed: {r}");

    Some(mcp)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn analyze_page_extracts_slots() {
    let Some(mut mcp) = setup_on_test_page() else {
        return;
    };

    let result = mcp.tool("analyze_page", json!({}));
    eprintln!("[DEBUG] analyze_page result: {result}");

    // Basic structure
    assert!(
        result["url"]
            .as_str()
            .unwrap_or("")
            .contains("test-page.html"),
        "url should contain test-page.html, got: {}",
        result["url"]
    );
    assert_eq!(result["title"].as_str(), Some("browsectl Test Page"));

    let slot_count = result["slot_count"].as_u64().unwrap_or(0);
    assert!(slot_count > 10, "expected >10 slots, got {slot_count}");

    let slots = result["slots"]
        .as_array()
        .expect("slots should be an array");
    assert!(!slots.is_empty());

    // Safety summary should have entries in multiple categories
    let ss = &result["safety_summary"];
    let navigate = ss["navigate"].as_u64().unwrap_or(0);
    let interact = ss["interact"].as_u64().unwrap_or(0);
    let submit = ss["submit"].as_u64().unwrap_or(0);
    assert!(navigate > 0, "expected navigate > 0, got {navigate}");
    assert!(interact > 0, "expected interact > 0, got {interact}");
    assert!(submit > 0, "expected submit > 0, got {submit}");

    mcp.shutdown();
}

#[test]
#[serial]
fn analyze_page_slot_categories() {
    let Some(mut mcp) = setup_on_test_page() else {
        return;
    };
    let result = mcp.tool("analyze_page", json!({"include_hidden": true}));
    let slots = result["slots"].as_array().expect("slots array");

    // TextInput: username field (id="login-user", name="username")
    let username = find_slot(slots, "login-user").expect("should find login-user slot");
    assert_eq!(
        username["category"].as_str(),
        Some("TextInput"),
        "login-user should be TextInput, got: {}",
        username["category"]
    );

    // PasswordInput: password field (id="login-pass")
    let password = find_slot(slots, "login-pass").expect("should find login-pass slot");
    assert_eq!(
        password["category"].as_str(),
        Some("PasswordInput"),
        "login-pass should be PasswordInput"
    );

    // Link: nav links (id="nav-about")
    let about = find_slot(slots, "nav-about").expect("should find nav-about link slot");
    assert_eq!(about["category"].as_str(), Some("Link"));

    // Checkbox (id="agree-cb")
    let checkbox = find_slot(slots, "agree-cb").expect("should find agree-cb checkbox");
    assert_eq!(checkbox["category"].as_str(), Some("Checkbox"));

    // Select (id="country-select")
    let select = find_slot(slots, "country-select").expect("should find country-select");
    assert_eq!(select["category"].as_str(), Some("Select"));

    // Search input: TextInput (id="search-input")
    let search = find_slot(slots, "search-input").expect("should find search-input");
    assert_eq!(search["category"].as_str(), Some("TextInput"));

    mcp.shutdown();
}

#[test]
#[serial]
fn analyze_page_safety_levels() {
    let Some(mut mcp) = setup_on_test_page() else {
        return;
    };
    let result = mcp.tool("analyze_page", json!({}));
    let slots = result["slots"].as_array().expect("slots array");

    // Login inputs → Interact
    let username = find_slot(slots, "login-user").expect("login-user slot");
    assert_eq!(username["safety_level"].as_str(), Some("Interact"));

    // Sign In (type=submit, id="login-submit") → Submit
    let signin = find_slot(slots, "login-submit")
        .or_else(|| find_slot_by_text(slots, "Sign In"))
        .expect("should find Sign In button");
    assert_eq!(
        signin["safety_level"].as_str(),
        Some("Submit"),
        "Sign In should be Submit, got: {}",
        signin["safety_level"]
    );

    // Nav links → Navigate
    let about = find_slot(slots, "nav-about").expect("nav-about link");
    assert_eq!(about["safety_level"].as_str(), Some("Navigate"));

    // Delete Account → Submit (contains "delete")
    let delete = find_slot_by_text(slots, "Delete Account")
        .or_else(|| find_slot(slots, "delete-btn"))
        .expect("should find Delete Account button");
    assert_eq!(
        delete["safety_level"].as_str(),
        Some("Submit"),
        "Delete Account should be Submit, got: {}",
        delete["safety_level"]
    );

    // 购买 → Submit (Chinese purchase keyword)
    let purchase = find_slot_by_text(slots, "购买")
        .or_else(|| find_slot(slots, "purchase-btn"))
        .expect("should find 购买 button");
    assert_eq!(
        purchase["safety_level"].as_str(),
        Some("Submit"),
        "购买 should be Submit, got: {}",
        purchase["safety_level"]
    );

    // View Details → Navigate (contains "view")
    let view = find_slot_by_text(slots, "View Details")
        .or_else(|| find_slot(slots, "view-details-btn"))
        .expect("should find View Details button");
    assert_eq!(
        view["safety_level"].as_str(),
        Some("Navigate"),
        "View Details should be Navigate, got: {}",
        view["safety_level"]
    );

    mcp.shutdown();
}

#[test]
#[serial]
fn analyze_page_forms() {
    let Some(mut mcp) = setup_on_test_page() else {
        return;
    };
    let result = mcp.tool("analyze_page", json!({}));

    let forms = result["forms"].as_array().expect("forms array");
    assert!(!forms.is_empty(), "should find at least one form");

    // Find the login form
    let login_form = forms
        .iter()
        .find(|f| f["form_id"].as_str() == Some("login-form"))
        .expect("should find login-form");

    assert!(
        login_form["action"]
            .as_str()
            .unwrap_or("")
            .contains("/api/login"),
        "login form action should contain /api/login"
    );
    assert_eq!(
        login_form["method"]
            .as_str()
            .map(|m| m.to_uppercase())
            .as_deref(),
        Some("POST")
    );

    let slot_ids = login_form["slot_ids"].as_array().expect("slot_ids");
    assert!(
        slot_ids.len() >= 2,
        "login form should have at least 2 slot_ids (username + password), got {}",
        slot_ids.len()
    );

    mcp.shutdown();
}

#[test]
#[serial]
fn analyze_page_suggestions() {
    let Some(mut mcp) = setup_on_test_page() else {
        return;
    };
    let result = mcp.tool("analyze_page", json!({"include_suggestions": true}));

    let suggestions = result["suggestions"].as_array().expect("suggestions array");
    assert!(
        !suggestions.is_empty(),
        "should have at least one suggestion"
    );

    // Should have a Navigate suggestion
    let has_navigate = suggestions
        .iter()
        .any(|s| s["safety_level"].as_str() == Some("Navigate"));
    assert!(has_navigate, "should have a Navigate suggestion");

    // Should have a suggestion involving the login form
    let has_form = suggestions.iter().any(|s| {
        let title = s["title"].as_str().unwrap_or("");
        title.to_lowercase().contains("form") || title.to_lowercase().contains("login")
    });
    assert!(has_form, "should have a form-related suggestion");

    // Verify suggestions are sorted safest-first
    let levels: Vec<&str> = suggestions
        .iter()
        .filter_map(|s| s["safety_level"].as_str())
        .collect();
    let level_order = |l: &str| -> u8 {
        match l {
            "Observe" => 0,
            "Navigate" => 1,
            "Interact" => 2,
            "Submit" => 3,
            _ => 4,
        }
    };
    for window in levels.windows(2) {
        assert!(
            level_order(window[0]) <= level_order(window[1]),
            "suggestions should be sorted safest-first, but got {:?}",
            levels
        );
    }

    mcp.shutdown();
}

#[test]
#[serial]
fn analyze_page_hidden_filter() {
    let Some(mut mcp) = setup_on_test_page() else {
        return;
    };

    // Default: exclude hidden
    let visible = mcp.tool("analyze_page", json!({"include_hidden": false}));
    let visible_count = visible["slot_count"].as_u64().unwrap_or(0);

    // Include hidden
    let all = mcp.tool("analyze_page", json!({"include_hidden": true}));
    let all_count = all["slot_count"].as_u64().unwrap_or(0);

    assert!(
        all_count >= visible_count,
        "include_hidden=true should return >= visible-only count: all={all_count} visible={visible_count}"
    );

    // The hidden CSRF input (id="csrf-hidden") should appear in the full set
    let all_slots = all["slots"].as_array().unwrap();
    let has_hidden = all_slots
        .iter()
        .any(|s| s["visible"].as_bool() == Some(false));
    assert!(
        has_hidden,
        "include_hidden=true should include at least one invisible slot"
    );

    mcp.shutdown();
}

#[test]
#[serial]
fn suggest_actions_returns_results() {
    let Some(mut mcp) = setup_on_test_page() else {
        return;
    };

    let result = mcp.tool("suggest_actions", json!({"max_suggestions": 10}));

    assert!(
        result["url"]
            .as_str()
            .unwrap_or("")
            .contains("test-page.html")
    );
    assert!(result["slot_count"].as_u64().unwrap_or(0) > 0);

    let suggestions = result["suggestions"].as_array().expect("suggestions");
    assert!(!suggestions.is_empty(), "should return suggestions");

    // Each suggestion should have required fields
    for s in suggestions {
        assert!(s["title"].is_string(), "suggestion must have title");
        assert!(
            s["description"].is_string(),
            "suggestion must have description"
        );
        assert!(
            s["safety_level"].is_string(),
            "suggestion must have safety_level"
        );
        assert!(
            s["commands"].is_array(),
            "suggestion must have commands array"
        );
    }

    assert!(
        result.get("memory_patterns_found").is_some(),
        "should have memory_patterns_found field"
    );

    mcp.shutdown();
}
