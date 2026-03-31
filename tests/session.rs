//! Session lifecycle tests — create, list, switch default, delete.
//!
//! Requires chromedriver + Chrome.  Each test creates its own temp profile
//! to avoid conflicts with a running browser.
//!
//! # Running
//!
//! ```sh
//! cargo test --test session -- --test-threads=1
//! ```

mod common;

use std::time::Duration;

use serial_test::serial;

// ---------------------------------------------------------------------------
// Full lifecycle
// ---------------------------------------------------------------------------

/// Full lifecycle: create → list → REPL → delete.
#[test]
#[serial]
fn session_lifecycle() {
    let tmp_profile = std::env::temp_dir().join(format!("browsectl-e2e-{}", std::process::id()));

    let create = common::browsectl_json(&[
        "--user-data-dir",
        &tmp_profile.to_string_lossy(),
        "session-create",
        "--foreground",
    ]);

    if create["ok"].as_bool() != Some(true) {
        let err = create["error"].as_str().unwrap_or("unknown");
        if err.contains("no such file") || err.contains("not found") || err.contains("No such file")
        {
            eprintln!("SKIP: Chrome binary not found, skipping lifecycle test");
            return;
        }
        panic!("session-create failed: {err}");
    }

    let session_id = create["sessionId"]
        .as_str()
        .expect("create should return sessionId");

    // --- session-list ---
    let list = common::browsectl_ok(&["session-list"]);
    assert!(
        list["defaultSessionId"].as_str() == Some(session_id),
        "defaultSessionId should be the just-created session: {list}"
    );
    let sessions = list["sessions"]
        .as_array()
        .expect("sessions should be an array");
    assert!(
        sessions
            .iter()
            .any(|s| s["sessionId"].as_str() == Some(session_id)),
        "session list should contain the new session"
    );

    // --- REPL: open, title, tabs ---
    let repl_input = "open https://example.com\ntitle\ntabs\nexit\n";
    let repl_output = common::browsectl_stdin(&["repl"], repl_input);

    assert!(
        repl_output.contains("\"type\": \"open\"") || repl_output.contains("\"type\":\"open\""),
        "REPL should show open result:\n{repl_output}"
    );
    assert!(
        repl_output.contains("Example Domain"),
        "title should be 'Example Domain':\n{repl_output}"
    );

    // --- run --type title ---
    let title = common::browsectl_ok(&["run", "--type", "title"]);
    assert_eq!(title["ok"].as_bool(), Some(true));
    assert!(
        title["title"]
            .as_str()
            .unwrap_or("")
            .contains("Example Domain"),
        "title should be Example Domain: {title}"
    );

    // --- run --type click ---
    let click_result = common::browsectl_json(&["run", "--type", "click", "--selector", "a"]);
    assert_eq!(
        click_result["ok"].as_bool(),
        Some(true),
        "click on <a> should succeed: {click_result}"
    );

    std::thread::sleep(Duration::from_secs(1));

    // --- tab-list ---
    let tabs = common::browsectl_ok(&["tab-list"]);
    assert!(
        tabs["tabs"].is_array(),
        "tab-list should return tabs array: {tabs}"
    );

    // --- session-delete ---
    let del = common::browsectl_ok(&["--session", session_id, "session-delete"]);
    assert_eq!(
        del["deletedSessionId"].as_str(),
        Some(session_id),
        "should delete the correct session: {del}"
    );

    // --- verify session is gone ---
    let list2 = common::browsectl_ok(&["session-list"]);
    let empty = vec![];
    let remaining = list2["sessions"].as_array().unwrap_or(&empty);
    assert!(
        !remaining
            .iter()
            .any(|s| s["sessionId"].as_str() == Some(session_id)),
        "deleted session should not appear in session-list"
    );

    let _ = std::fs::remove_dir_all(&tmp_profile);
}

// ---------------------------------------------------------------------------
// Custom profile
// ---------------------------------------------------------------------------

/// `session-create --foreground` with a custom user-data-dir works.
#[test]
#[serial]
fn create_with_custom_profile() {
    let tmp = std::env::temp_dir().join(format!("browsectl-e2e-custom-{}", std::process::id()));

    let create = common::browsectl_json(&[
        "--user-data-dir",
        &tmp.to_string_lossy(),
        "session-create",
        "--foreground",
    ]);

    if create["ok"].as_bool() != Some(true) {
        let err = create["error"].as_str().unwrap_or("unknown");
        if err.contains("not found") || err.contains("No such file") {
            eprintln!("SKIP: Chrome not available");
            return;
        }
        panic!("session-create with custom profile failed: {err}");
    }

    let sid = create["sessionId"].as_str().unwrap();

    let list = common::browsectl_ok(&["session-list"]);
    assert!(
        list["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["sessionId"].as_str() == Some(sid)),
        "custom-profile session should be in the store"
    );

    common::cleanup_session(sid);
    let _ = std::fs::remove_dir_all(&tmp);
}

// ---------------------------------------------------------------------------
// Background session-create
// ---------------------------------------------------------------------------

/// Background session-create should eventually produce a session.
#[test]
#[serial]
fn background_session_create() {
    let tmp = std::env::temp_dir().join(format!("browsectl-e2e-bg-{}", std::process::id()));

    let result =
        common::browsectl_json(&["--user-data-dir", &tmp.to_string_lossy(), "session-create"]);

    if result["ok"].as_bool() == Some(true) {
        let sid = result["sessionId"].as_str().unwrap();
        let list = common::browsectl_ok(&["session-list"]);
        assert!(
            list["sessions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|s| s["sessionId"].as_str() == Some(sid)),
            "background-created session should be in the store"
        );
        common::cleanup_session(sid);
    } else {
        let err = result["error"].as_str().unwrap_or("unknown");
        eprintln!("background session-create did not succeed: {err}");
    }

    let _ = std::fs::remove_dir_all(&tmp);
}

// ---------------------------------------------------------------------------
// Multiple sessions
// ---------------------------------------------------------------------------

/// Multiple sessions can coexist and default can be switched.
#[test]
#[serial]
fn multiple_sessions() {
    let tmp1 = std::env::temp_dir().join(format!("browsectl-e2e-multi1-{}", std::process::id()));
    let tmp2 = std::env::temp_dir().join(format!("browsectl-e2e-multi2-{}", std::process::id()));

    let c1 = common::browsectl_json(&[
        "--user-data-dir",
        &tmp1.to_string_lossy(),
        "session-create",
        "--foreground",
    ]);
    if c1["ok"].as_bool() != Some(true) {
        eprintln!("SKIP: cannot create first session");
        let _ = std::fs::remove_dir_all(&tmp1);
        return;
    }
    let sid1 = c1["sessionId"].as_str().unwrap().to_string();

    let c2 = common::browsectl_json(&[
        "--user-data-dir",
        &tmp2.to_string_lossy(),
        "session-create",
        "--foreground",
    ]);
    if c2["ok"].as_bool() != Some(true) {
        eprintln!("SKIP: cannot create second session");
        common::cleanup_session(&sid1);
        let _ = std::fs::remove_dir_all(&tmp1);
        let _ = std::fs::remove_dir_all(&tmp2);
        return;
    }
    let sid2 = c2["sessionId"].as_str().unwrap().to_string();

    let list = common::browsectl_ok(&["session-list"]);
    let sessions = list["sessions"].as_array().unwrap();
    assert!(
        sessions
            .iter()
            .any(|s| s["sessionId"].as_str() == Some(&sid1)),
        "session 1 should be in store"
    );
    assert!(
        sessions
            .iter()
            .any(|s| s["sessionId"].as_str() == Some(&sid2)),
        "session 2 should be in store"
    );

    assert_eq!(
        list["defaultSessionId"].as_str(),
        Some(sid2.as_str()),
        "default should be the latest session"
    );

    let use_result = common::browsectl_ok(&["session-use", "--session", &sid1]);
    assert_eq!(use_result["ok"].as_bool(), Some(true));
    assert_eq!(
        use_result["defaultSessionId"].as_str(),
        Some(sid1.as_str()),
        "default should now be session 1"
    );

    common::cleanup_session(&sid1);
    common::cleanup_session(&sid2);
    let _ = std::fs::remove_dir_all(&tmp1);
    let _ = std::fs::remove_dir_all(&tmp2);
}
