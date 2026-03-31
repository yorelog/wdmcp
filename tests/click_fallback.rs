//! Click fallback tests — verify that `--fallback sibling|parent|<selector>`
//! is honored when native clicks are intercepted by overlays.
//!
//! Uses a local fixture page with an overlay element that intercepts
//! clicks on `div[data-testid="qrcode_image"]`.
//!
//! # Running
//!
//! ```sh
//! cargo test --test click_fallback -- --test-threads=1
//! ```

mod common;

use serial_test::serial;

// ---------------------------------------------------------------------------
// CLI: --fallback sibling
// ---------------------------------------------------------------------------

/// Explicit `--fallback sibling` should be honored (not auto parent).
#[test]
#[serial]
fn fallback_sibling_via_run() {
    let (sid, tmp) = match common::setup_headless_session() {
        Some(v) => v,
        None => return,
    };

    let url = common::test_page_url();
    let open = common::browsectl_json(&["--session", &sid, "run", "--type", "open", "--url", &url]);
    assert_eq!(open["ok"].as_bool(), Some(true), "open failed: {open}");

    let click = common::browsectl_json(&[
        "--session",
        &sid,
        "run",
        "--type",
        "click",
        "--selector",
        "div[data-testid=\"qrcode_image\"]",
        "--fallback",
        "sibling",
    ]);

    assert_eq!(
        click["ok"].as_bool(),
        Some(true),
        "click should succeed: {click}"
    );
    assert_eq!(click["type"].as_str(), Some("click"));
    assert_eq!(
        click["fallback"].as_str(),
        Some("sibling"),
        "explicit fallback must be honored (not auto parent): {click}"
    );
    assert_eq!(
        click["jsClick"].as_bool(),
        Some(true),
        "intercepted click should have used JS fallback: {click}"
    );

    common::teardown_headless(&sid, &tmp);
}

// ---------------------------------------------------------------------------
// CLI: --fallback parent
// ---------------------------------------------------------------------------

/// Explicit `--fallback parent` should be honored.
#[test]
#[serial]
fn fallback_parent_via_run() {
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
        "div[data-testid=\"qrcode_image\"]",
        "--fallback",
        "parent",
    ]);

    assert_eq!(
        click["ok"].as_bool(),
        Some(true),
        "click should succeed: {click}"
    );
    assert_eq!(
        click["fallback"].as_str(),
        Some("parent"),
        "explicit parent fallback must be honored: {click}"
    );
    assert_eq!(click["jsClick"].as_bool(), Some(true));

    common::teardown_headless(&sid, &tmp);
}

// ---------------------------------------------------------------------------
// CLI: auto fallback (no --fallback flag)
// ---------------------------------------------------------------------------

/// Without `--fallback`, auto smart fallback should kick in.
#[test]
#[serial]
fn fallback_auto_smart() {
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
        "div[data-testid=\"qrcode_image\"]",
    ]);

    assert_eq!(
        click["ok"].as_bool(),
        Some(true),
        "click should succeed: {click}"
    );
    // Auto mode returns whichever strategy worked (parent/sibling/self).
    assert!(
        click["jsClick"].as_bool() == Some(true),
        "should use JS fallback: {click}"
    );
    assert!(
        click["fallback"].is_string(),
        "should report fallback strategy: {click}"
    );

    common::teardown_headless(&sid, &tmp);
}

// ---------------------------------------------------------------------------
// REPL: click <selector> --fallback sibling
// ---------------------------------------------------------------------------

/// REPL parser should pass `--fallback sibling` to CommandSpec.
#[test]
#[serial]
fn fallback_sibling_via_repl() {
    let (sid, tmp) = match common::setup_headless_session() {
        Some(v) => v,
        None => return,
    };

    let url = common::test_page_url();
    common::browsectl_ok(&["--session", &sid, "run", "--type", "open", "--url", &url]);

    let out = common::browsectl_with_input(
        &["--session", &sid, "repl"],
        "click div[data-testid=\"qrcode_image\"] --fallback sibling\nexit\n",
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(
        out.status.success(),
        "repl click fallback failed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("\"type\": \"click\"") || stdout.contains("\"type\":\"click\""),
        "REPL output should contain click result:\n{stdout}"
    );
    assert!(
        stdout.contains("\"fallback\": \"sibling\"") || stdout.contains("\"fallback\":\"sibling\""),
        "REPL output should preserve explicit sibling fallback:\n{stdout}"
    );

    common::teardown_headless(&sid, &tmp);
}

// ---------------------------------------------------------------------------
// Scope + regex selector e2e
// ---------------------------------------------------------------------------

/// Without `--scope`, lookup is global and should hit the first matching
/// scoped block in DOM order (`#scope-a`).
#[test]
#[serial]
fn scope_default_global_first_match() {
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
        "div[data-testid=\"scope_qrcode_image\"]::text(/^QR Code$/)",
        "--fallback",
        "div[data-testid=\"scope_qrcode_sibling\"]",
    ]);

    assert_eq!(click["ok"].as_bool(), Some(true), "click failed: {click}");
    assert_eq!(click["jsClick"].as_bool(), Some(true));
    assert_eq!(
        click["fallback"].as_str(),
        Some("selector:div[data-testid=\"scope_qrcode_sibling\"]")
    );
    assert!(
        click["scope"].is_null(),
        "default scope should be global: {click}"
    );

    let waited = common::browsectl_json(&[
        "--session",
        &sid,
        "run",
        "--type",
        "wait",
        "--selector",
        "#result",
        "--condition",
        "text-equals",
        "--value",
        "scope-a-fallback",
        "--timeout",
        "5000",
    ]);
    assert_eq!(
        waited["ok"].as_bool(),
        Some(true),
        "result wait failed: {waited}"
    );

    common::teardown_headless(&sid, &tmp);
}

/// With `--scope #scope-b`, both primary regex selector and fallback selector
/// should resolve inside scope-b only.
#[test]
#[serial]
fn scope_regex_and_fallback_are_scoped() {
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
        "--scope",
        "#scope-b",
        "--selector",
        "div[data-testid=\"scope_qrcode_image\"]::text(/^QR Code$/)",
        "--fallback",
        "div[data-testid=\"scope_qrcode_sibling\"]",
    ]);

    assert_eq!(click["ok"].as_bool(), Some(true), "click failed: {click}");
    assert_eq!(click["jsClick"].as_bool(), Some(true));
    assert_eq!(
        click["scope"].as_str(),
        Some("#scope-b"),
        "scope lost: {click}"
    );
    assert_eq!(
        click["fallback"].as_str(),
        Some("selector:div[data-testid=\"scope_qrcode_sibling\"]")
    );

    let waited = common::browsectl_json(&[
        "--session",
        &sid,
        "run",
        "--type",
        "wait",
        "--selector",
        "#result",
        "--condition",
        "text-equals",
        "--value",
        "scope-b-fallback",
        "--timeout",
        "5000",
    ]);
    assert_eq!(
        waited["ok"].as_bool(),
        Some(true),
        "result wait failed: {waited}"
    );

    common::teardown_headless(&sid, &tmp);
}
