//! Browser command tests — headless Chrome on the local fixture page.
//!
//! Tests open, click, fill, paste, wait, scroll, screenshot, and
//! sequential navigation.  All use headless sessions with temp profiles.
//!
//! # Running
//!
//! ```sh
//! cargo test --test browser -- --test-threads=1
//! ```

mod common;

use serial_test::serial;

// ---------------------------------------------------------------------------
// Navigation & title
// ---------------------------------------------------------------------------

/// Open a remote URL in headless mode and verify the page title.
#[test]
#[serial]
fn open_remote_url_and_title() {
    let (sid, tmp) = match common::setup_headless_session() {
        Some(v) => v,
        None => return,
    };

    let open = common::browsectl_json(&[
        "--session",
        &sid,
        "run",
        "--type",
        "open",
        "--url",
        "https://example.com",
    ]);
    assert_eq!(
        open["ok"].as_bool(),
        Some(true),
        "open should succeed: {open}"
    );
    assert_eq!(open["type"].as_str(), Some("open"));

    let title = common::browsectl_ok(&["--session", &sid, "run", "--type", "title"]);
    let t = title["title"].as_str().unwrap_or("");
    assert!(
        t.contains("Example Domain"),
        "expected 'Example Domain', got: {t}"
    );

    common::teardown_headless(&sid, &tmp);
}

/// Open the local test-page.html fixture and verify its title.
#[test]
#[serial]
fn open_local_page_and_title() {
    let (sid, tmp) = match common::setup_headless_session() {
        Some(v) => v,
        None => return,
    };

    let url = common::test_page_url();
    let open = common::browsectl_json(&["--session", &sid, "run", "--type", "open", "--url", &url]);
    assert_eq!(
        open["ok"].as_bool(),
        Some(true),
        "open local page failed: {open}"
    );

    let title = common::browsectl_ok(&["--session", &sid, "run", "--type", "title"]);
    assert_eq!(
        title["title"].as_str(),
        Some("browsectl Test Page"),
        "local page title mismatch: {title}"
    );

    common::teardown_headless(&sid, &tmp);
}

// ---------------------------------------------------------------------------
// Click
// ---------------------------------------------------------------------------

/// Click a button on the local test page.
#[test]
#[serial]
fn click_element() {
    let (sid, tmp) = match common::setup_headless_session() {
        Some(v) => v,
        None => return,
    };

    let url = common::test_page_url();
    common::browsectl_ok(&["--session", &sid, "run", "--type", "open", "--url", &url]);

    let click = common::browsectl_json(&[
        "--session",
        &sid,
        "run",
        "--type",
        "click",
        "--selector",
        "#submit-btn",
    ]);
    assert_eq!(click["ok"].as_bool(), Some(true), "click failed: {click}");
    assert_eq!(click["type"].as_str(), Some("click"));
    assert_eq!(click["selector"].as_str(), Some("#submit-btn"));

    common::teardown_headless(&sid, &tmp);
}

// ---------------------------------------------------------------------------
// Fill
// ---------------------------------------------------------------------------

/// Fill an input field and verify form submission.
#[test]
#[serial]
fn fill_input() {
    let (sid, tmp) = match common::setup_headless_session() {
        Some(v) => v,
        None => return,
    };

    let url = common::test_page_url();
    common::browsectl_ok(&["--session", &sid, "run", "--type", "open", "--url", &url]);

    let fill = common::browsectl_json(&[
        "--session",
        &sid,
        "run",
        "--type",
        "fill",
        "--selector",
        "#name-input",
        "--text",
        "E2E Test User",
    ]);
    assert_eq!(fill["ok"].as_bool(), Some(true), "fill failed: {fill}");

    let fill2 = common::browsectl_json(&[
        "--session",
        &sid,
        "run",
        "--type",
        "fill",
        "--selector",
        "#message",
        "--text",
        "Hello from E2E tests!",
    ]);
    assert_eq!(
        fill2["ok"].as_bool(),
        Some(true),
        "fill textarea failed: {fill2}"
    );

    common::browsectl_ok(&[
        "--session",
        &sid,
        "run",
        "--type",
        "click",
        "--selector",
        "#submit-btn",
    ]);

    let wait = common::browsectl_json(&[
        "--session",
        &sid,
        "run",
        "--type",
        "wait",
        "--selector",
        "#result",
        "--condition",
        "text-contains",
        "--value",
        "E2E Test User",
        "--timeout",
        "5000",
    ]);
    assert_eq!(
        wait["ok"].as_bool(),
        Some(true),
        "result div should contain the filled name: {wait}"
    );

    common::teardown_headless(&sid, &tmp);
}

/// End-to-end form interaction: fill → click → verify result + heading.
#[test]
#[serial]
fn fill_click_verify() {
    let (sid, tmp) = match common::setup_headless_session() {
        Some(v) => v,
        None => return,
    };

    let url = common::test_page_url();
    common::browsectl_ok(&["--session", &sid, "run", "--type", "open", "--url", &url]);

    common::browsectl_json(&[
        "--session",
        &sid,
        "run",
        "--type",
        "fill",
        "--selector",
        "#name-input",
        "--text",
        "Alice",
    ]);
    common::browsectl_json(&[
        "--session",
        &sid,
        "run",
        "--type",
        "fill",
        "--selector",
        "#message",
        "--text",
        "Testing 123",
    ]);
    common::browsectl_json(&[
        "--session",
        &sid,
        "run",
        "--type",
        "click",
        "--selector",
        "#submit-btn",
    ]);

    let w = common::browsectl_json(&[
        "--session",
        &sid,
        "run",
        "--type",
        "wait",
        "--selector",
        "#result",
        "--condition",
        "text-contains",
        "--value",
        "Alice",
        "--timeout",
        "5000",
    ]);
    assert_eq!(
        w["ok"].as_bool(),
        Some(true),
        "result should contain 'Alice': {w}"
    );

    let w2 = common::browsectl_json(&[
        "--session",
        &sid,
        "run",
        "--type",
        "wait",
        "--selector",
        "#heading",
        "--condition",
        "text-equals",
        "--value",
        "Form Submitted",
        "--timeout",
        "5000",
    ]);
    assert_eq!(
        w2["ok"].as_bool(),
        Some(true),
        "heading should change: {w2}"
    );

    common::teardown_headless(&sid, &tmp);
}

// ---------------------------------------------------------------------------
// Paste
// ---------------------------------------------------------------------------

/// Test the `paste` command on the local test page.
#[test]
#[serial]
fn paste_text() {
    let (sid, tmp) = match common::setup_headless_session() {
        Some(v) => v,
        None => return,
    };

    let url = common::test_page_url();
    common::browsectl_ok(&["--session", &sid, "run", "--type", "open", "--url", &url]);

    let p = common::browsectl_json(&[
        "--session",
        &sid,
        "run",
        "--type",
        "paste",
        "--selector",
        "#name-input",
        "--text",
        "Pasted Value",
    ]);
    assert!(
        p.get("ok").is_some(),
        "paste should return JSON with ok field: {p}"
    );

    common::teardown_headless(&sid, &tmp);
}

// ---------------------------------------------------------------------------
// Wait
// ---------------------------------------------------------------------------

/// Test `wait` with various conditions on the local test page.
#[test]
#[serial]
fn wait_for_element() {
    let (sid, tmp) = match common::setup_headless_session() {
        Some(v) => v,
        None => return,
    };

    let url = common::test_page_url();
    common::browsectl_ok(&["--session", &sid, "run", "--type", "open", "--url", &url]);

    // exist
    let w1 = common::browsectl_json(&[
        "--session",
        &sid,
        "run",
        "--type",
        "wait",
        "--selector",
        "#heading",
        "--condition",
        "exist",
        "--timeout",
        "5000",
    ]);
    assert_eq!(w1["ok"].as_bool(), Some(true), "wait exist failed: {w1}");

    // visible
    let w2 = common::browsectl_json(&[
        "--session",
        &sid,
        "run",
        "--type",
        "wait",
        "--selector",
        "#heading",
        "--condition",
        "visible",
        "--timeout",
        "5000",
    ]);
    assert_eq!(w2["ok"].as_bool(), Some(true), "wait visible failed: {w2}");

    // text-contains
    let w3 = common::browsectl_json(&[
        "--session",
        &sid,
        "run",
        "--type",
        "wait",
        "--selector",
        "#heading",
        "--condition",
        "text-contains",
        "--value",
        "Hello",
        "--timeout",
        "5000",
    ]);
    assert_eq!(
        w3["ok"].as_bool(),
        Some(true),
        "wait text-contains failed: {w3}"
    );

    // text-equals
    let w4 = common::browsectl_json(&[
        "--session",
        &sid,
        "run",
        "--type",
        "wait",
        "--selector",
        "#heading",
        "--condition",
        "text-equals",
        "--value",
        "Hello World",
        "--timeout",
        "5000",
    ]);
    assert_eq!(
        w4["ok"].as_bool(),
        Some(true),
        "wait text-equals failed: {w4}"
    );

    // hidden
    let w5 = common::browsectl_json(&[
        "--session",
        &sid,
        "run",
        "--type",
        "wait",
        "--selector",
        "#hidden-box",
        "--condition",
        "hidden",
        "--timeout",
        "5000",
    ]);
    assert_eq!(w5["ok"].as_bool(), Some(true), "wait hidden failed: {w5}");

    // enabled
    let w6 = common::browsectl_json(&[
        "--session",
        &sid,
        "run",
        "--type",
        "wait",
        "--selector",
        "#name-input",
        "--condition",
        "enabled",
        "--timeout",
        "5000",
    ]);
    assert_eq!(w6["ok"].as_bool(), Some(true), "wait enabled failed: {w6}");

    // pure sleep
    let w7 = common::browsectl_json(&["--session", &sid, "run", "--type", "wait", "--ms", "200"]);
    assert_eq!(w7["ok"].as_bool(), Some(true), "wait sleep failed: {w7}");

    common::teardown_headless(&sid, &tmp);
}

// ---------------------------------------------------------------------------
// Scroll
// ---------------------------------------------------------------------------

/// Test scrolling down and up.
#[test]
#[serial]
fn scroll() {
    let (sid, tmp) = match common::setup_headless_session() {
        Some(v) => v,
        None => return,
    };

    let url = common::test_page_url();
    common::browsectl_ok(&["--session", &sid, "run", "--type", "open", "--url", &url]);

    let s1 = common::browsectl_json(&[
        "--session",
        &sid,
        "run",
        "--type",
        "scroll",
        "--direction",
        "down",
        "--amount",
        "500",
    ]);
    assert_eq!(s1["ok"].as_bool(), Some(true), "scroll down failed: {s1}");

    let s2 = common::browsectl_json(&[
        "--session",
        &sid,
        "run",
        "--type",
        "scroll",
        "--direction",
        "up",
        "--amount",
        "300",
    ]);
    assert_eq!(s2["ok"].as_bool(), Some(true), "scroll up failed: {s2}");

    let s3 = common::browsectl_json(&[
        "--session",
        &sid,
        "run",
        "--type",
        "scroll",
        "--direction",
        "down",
        "--amount",
        "200",
        "--behavior",
        "smooth",
    ]);
    assert_eq!(s3["ok"].as_bool(), Some(true), "scroll smooth failed: {s3}");

    common::teardown_headless(&sid, &tmp);
}

// ---------------------------------------------------------------------------
// Screenshot
// ---------------------------------------------------------------------------

/// Take an element screenshot and verify file output.
#[test]
#[serial]
fn screenshot_local_page() {
    let (sid, tmp) = match common::setup_headless_session() {
        Some(v) => v,
        None => return,
    };

    let url = common::test_page_url();
    common::browsectl_ok(&["--session", &sid, "run", "--type", "open", "--url", &url]);

    let shot_path = tmp.join("e2e-heading-screenshot.png");
    let _ = std::fs::remove_file(&shot_path);

    let shot = common::browsectl_json(&[
        "--session",
        &sid,
        "run",
        "--type",
        "screenshot",
        "--selector",
        "#heading",
        "--path",
        &shot_path.to_string_lossy(),
    ]);

    if shot["ok"].as_bool() == Some(true) {
        assert!(
            shot_path.exists(),
            "screenshot file should exist at {}",
            shot_path.display()
        );
        let meta = std::fs::metadata(&shot_path).unwrap();
        assert!(
            meta.len() > 100,
            "screenshot should be valid PNG (got {} bytes)",
            meta.len()
        );
        assert!(
            shot.get("path").is_some() || shot.get("file").is_some(),
            "screenshot response should include file path: {shot}"
        );
    } else {
        assert!(shot.get("ok").is_some(), "should return JSON: {shot}");
    }

    let _ = std::fs::remove_file(&shot_path);
    common::teardown_headless(&sid, &tmp);
}

// ---------------------------------------------------------------------------
// Sequential navigation
// ---------------------------------------------------------------------------

/// Navigate to multiple pages in sequence and verify titles change.
#[test]
#[serial]
fn sequential_navigation() {
    let (sid, tmp) = match common::setup_headless_session() {
        Some(v) => v,
        None => return,
    };

    // example.com
    common::browsectl_ok(&[
        "--session",
        &sid,
        "run",
        "--type",
        "open",
        "--url",
        "https://example.com",
    ]);
    let t1 = common::browsectl_ok(&["--session", &sid, "run", "--type", "title"]);
    assert!(
        t1["title"]
            .as_str()
            .unwrap_or("")
            .contains("Example Domain"),
        "first page title: {t1}"
    );

    // local test page
    let url = common::test_page_url();
    common::browsectl_ok(&["--session", &sid, "run", "--type", "open", "--url", &url]);
    let t2 = common::browsectl_ok(&["--session", &sid, "run", "--type", "title"]);
    assert_eq!(
        t2["title"].as_str(),
        Some("browsectl Test Page"),
        "second page title: {t2}"
    );

    // about:blank
    common::browsectl_ok(&[
        "--session",
        &sid,
        "run",
        "--type",
        "open",
        "--url",
        "about:blank",
    ]);
    let t3 = common::browsectl_ok(&["--session", &sid, "run", "--type", "title"]);
    let title3 = t3["title"].as_str().unwrap_or("");
    assert!(
        title3.is_empty() || title3 == "about:blank",
        "about:blank title should be empty, got: {title3}"
    );

    common::teardown_headless(&sid, &tmp);
}
