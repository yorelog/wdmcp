//! Tab management tests — create, list, switch, close tabs.
//!
//! Requires headless Chrome.
//!
//! # Running
//!
//! ```sh
//! cargo test --test tabs -- --test-threads=1
//! ```

mod common;

use std::time::Duration;

use serial_test::serial;

/// Full tab lifecycle: create with alias, list, switch, close.
#[test]
#[serial]
fn tab_management_lifecycle() {
    let (sid, tmp) = match common::setup_headless_session() {
        Some(v) => v,
        None => return,
    };

    // Open initial page
    common::browsectl_ok(&[
        "--session",
        &sid,
        "run",
        "--type",
        "open",
        "--url",
        "https://example.com",
    ]);

    // List tabs — should have exactly 1
    let tabs1 = common::browsectl_ok(&["--session", &sid, "tab-list"]);
    let handles1 = tabs1["tabs"]
        .as_array()
        .or_else(|| tabs1["handles"].as_array());
    let initial_count = handles1.map(|a| a.len()).unwrap_or(1);
    assert!(
        initial_count >= 1,
        "should start with at least 1 tab: {tabs1}"
    );

    // Create tab "alpha"
    let tc1 = common::browsectl_json(&[
        "--session",
        &sid,
        "tab-create",
        "--url",
        "https://example.org",
        "--alias",
        "alpha",
    ]);
    assert!(
        tc1["ok"].as_bool() == Some(true) || tc1.get("handle").is_some(),
        "tab-create alpha should succeed: {tc1}"
    );

    // Create tab "beta"
    let tc2 = common::browsectl_json(&[
        "--session",
        &sid,
        "tab-create",
        "--url",
        "https://www.iana.org",
        "--alias",
        "beta",
    ]);
    assert!(
        tc2["ok"].as_bool() == Some(true) || tc2.get("handle").is_some(),
        "tab-create beta should succeed: {tc2}"
    );

    // List tabs — should now have initial_count + 2
    let tabs2 = common::browsectl_ok(&["--session", &sid, "tab-list"]);
    let handles2 = tabs2["tabs"]
        .as_array()
        .or_else(|| tabs2["handles"].as_array());
    if let Some(arr) = handles2 {
        assert_eq!(
            arr.len(),
            initial_count + 2,
            "should have {} tabs: {tabs2}",
            initial_count + 2
        );
    }

    // Switch to first tab by index
    let sw1 = common::browsectl_json(&["--session", &sid, "tab-switch", "--tab", "0"]);
    assert!(
        sw1.get("currentHandle").is_some(),
        "tab-switch 0 failed: {sw1}"
    );

    // Verify we're on example.com
    std::thread::sleep(Duration::from_millis(500));
    let title0 = common::browsectl_ok(&["--session", &sid, "run", "--type", "title"]);
    let t0 = title0["title"].as_str().unwrap_or("");
    assert!(
        t0.contains("Example Domain"),
        "tab 0 should be example.com, got: {t0}"
    );

    // Switch to tab by alias "alpha"
    let sw2 = common::browsectl_json(&["--session", &sid, "tab-switch", "--tab", "alpha"]);
    assert!(
        sw2.get("currentHandle").is_some(),
        "tab-switch alpha failed: {sw2}"
    );

    // Close "beta"
    let cl1 = common::browsectl_json(&["--session", &sid, "tab-close", "--tab", "beta"]);
    assert!(
        cl1.get("closedHandle").is_some(),
        "tab-close beta failed: {cl1}"
    );

    // List tabs — should be initial_count + 1
    let tabs3 = common::browsectl_ok(&["--session", &sid, "tab-list"]);
    let handles3 = tabs3["tabs"]
        .as_array()
        .or_else(|| tabs3["handles"].as_array());
    if let Some(arr) = handles3 {
        assert_eq!(
            arr.len(),
            initial_count + 1,
            "should have {} tabs after closing beta: {tabs3}",
            initial_count + 1
        );
    }

    // Close "alpha"
    let cl2 = common::browsectl_json(&["--session", &sid, "tab-close", "--tab", "alpha"]);
    assert!(
        cl2.get("closedHandle").is_some(),
        "tab-close alpha failed: {cl2}"
    );

    // Back to original count
    let tabs4 = common::browsectl_ok(&["--session", &sid, "tab-list"]);
    let handles4 = tabs4["tabs"]
        .as_array()
        .or_else(|| tabs4["handles"].as_array());
    if let Some(arr) = handles4 {
        assert_eq!(
            arr.len(),
            initial_count,
            "should be back to {initial_count} tab(s): {tabs4}"
        );
    }

    common::teardown_headless(&sid, &tmp);
}
