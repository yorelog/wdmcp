//! Smoke tests — verify CLI binary, flag parsing, and error handling.
//!
//! These tests need NO browser, NO chromedriver — they only check that the
//! binary exists, parses flags correctly, and returns valid JSON.
//!
//! # Running
//!
//! ```sh
//! cargo test --test smoke
//! ```

mod common;

use serde_json::Value;

// ---------------------------------------------------------------------------
// Binary & help
// ---------------------------------------------------------------------------

/// Verify the binary exists and prints help.
#[test]
fn cli_help() {
    let out = common::browsectl(&["--help"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("WebDriver automation CLI"),
        "help output should contain tool description"
    );
    assert!(stdout.contains("session-create"), "should list commands");
    assert!(stdout.contains("--browser"), "should list --browser flag");
}

// ---------------------------------------------------------------------------
// Status (no browser required — just returns ok/error JSON)
// ---------------------------------------------------------------------------

/// `status` should return JSON (ok or error) without crashing.
#[test]
fn cli_status_returns_json() {
    let val = common::browsectl_json(&["status"]);
    assert!(
        val.get("ok").is_some(),
        "status output should have an 'ok' field: {val}"
    );
}

// ---------------------------------------------------------------------------
// Session list (works even with empty store)
// ---------------------------------------------------------------------------

/// `session-list` returns valid JSON with the right structure.
#[test]
fn session_list_structure() {
    let list = common::browsectl_ok(&["session-list"]);
    assert!(
        list.get("defaultSessionId").is_some(),
        "should have defaultSessionId key"
    );
    assert!(list.get("sessions").is_some(), "should have sessions key");
    assert!(list["sessions"].is_array(), "sessions should be an array");
}

// ---------------------------------------------------------------------------
// Flag parsing
// ---------------------------------------------------------------------------

/// `--browser chrome` is accepted and produces valid output.
#[test]
fn browser_flag_chrome() {
    let out = common::browsectl(&["--browser", "chrome", "status"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("ok") || stdout.contains("error"),
        "chrome status should return JSON: {stdout}"
    );
}

/// `--browser edge` is accepted (even if driver isn't running).
#[test]
fn browser_flag_edge() {
    let out = common::browsectl(&["--browser", "edge", "status"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("ok") || stdout.contains("error"),
        "edge status should return JSON, not a parse error: {stdout}"
    );
}

/// Invalid browser name should produce a warning but not crash.
#[test]
fn invalid_browser_warns() {
    let out = common::browsectl(&["--browser", "firefox", "status"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("unknown browser") || stderr.contains("warning"),
        "invalid browser should warn: stderr={stderr}"
    );
}

// ---------------------------------------------------------------------------
// Error paths
// ---------------------------------------------------------------------------

/// Deleting a non-existent session should not crash.
#[test]
fn delete_nonexistent_session() {
    let out = common::browsectl(&[
        "--session",
        "nonexistent-session-id-12345",
        "session-delete",
    ]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let combined = format!("{stdout}{stderr}");

    let is_json = serde_json::from_str::<Value>(stdout.trim()).is_ok();
    let is_error_msg = combined.contains("error")
        || combined.contains("not found")
        || combined.contains("no session");
    let is_idempotent = combined.contains("deletedSessionId");
    assert!(
        is_json || is_error_msg || is_idempotent,
        "delete of nonexistent session should return valid JSON or an error, got: {combined}"
    );
}

// ---------------------------------------------------------------------------
// Setup command
// ---------------------------------------------------------------------------

/// `setup --check-only` should return JSON with platform, browsers, drivers.
#[test]
fn setup_check_only_returns_json() {
    let val = common::browsectl_json(&["setup", "--check-only"]);
    assert!(
        val.get("platform").is_some(),
        "should have platform key: {val}"
    );
    assert!(
        val.get("browsers").is_some(),
        "should have browsers key: {val}"
    );
    assert!(
        val.get("drivers").is_some(),
        "should have drivers key: {val}"
    );

    let platform = &val["platform"];
    assert!(platform["os"].is_string(), "platform.os should be a string");
    assert!(
        platform["arch"].is_string(),
        "platform.arch should be a string"
    );
    assert!(
        platform["display"].is_string(),
        "platform.display should be a string"
    );

    let browsers = val["browsers"]
        .as_array()
        .expect("browsers should be an array");
    assert!(
        !browsers.is_empty(),
        "should detect at least one browser entry"
    );
    for b in browsers {
        assert!(
            b.get("browser").is_some(),
            "each browser should have a 'browser' field"
        );
        assert!(
            b.get("path").is_some(),
            "each browser should have a 'path' field"
        );
        assert!(
            b.get("installed").is_some(),
            "each browser should have an 'installed' field"
        );
    }

    let drivers = val["drivers"]
        .as_array()
        .expect("drivers should be an array");
    assert!(!drivers.is_empty(), "should have at least one driver entry");
    for d in drivers {
        assert!(
            d.get("browser").is_some(),
            "each driver should have a 'browser' field"
        );
        assert!(
            d.get("path").is_some(),
            "each driver should have a 'path' field"
        );
        assert!(
            d.get("exists").is_some(),
            "each driver should have an 'exists' field"
        );
        assert!(
            d.get("versionMatch").is_some(),
            "each driver should have a 'versionMatch' field"
        );
    }
}

/// `session-use` with a bad session id should fail gracefully.
#[test]
fn session_use_bad_id() {
    let out = common::browsectl(&["session-use", "--session", "does-not-exist"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let combined = format!("{stdout}{stderr}");
    assert!(
        combined.contains("not found") || combined.contains("error") || combined.contains("Error"),
        "session-use with bad id should fail: {combined}"
    );
}
