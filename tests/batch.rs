//! Batch execution tests — run command batches from JSON files.
//!
//! Requires headless Chrome.
//!
//! # Running
//!
//! ```sh
//! cargo test --test batch -- --test-threads=1
//! ```

mod common;

use serde_json::{Value, json};
use serial_test::serial;

// ---------------------------------------------------------------------------
// Simple batch file
// ---------------------------------------------------------------------------

/// Execute the test batch file (open → title → wait for h1).
#[test]
#[serial]
fn batch_execution() {
    let (sid, tmp) = match common::setup_headless_session() {
        Some(v) => v,
        None => return,
    };

    let batch_file = common::project_root()
        .join("tests")
        .join("fixtures")
        .join("test-batch.json");
    assert!(batch_file.exists(), "test-batch.json fixture not found");

    let out = common::browsectl(&[
        "--session",
        &sid,
        "batch",
        "--file",
        &batch_file.to_string_lossy(),
    ]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(
        out.status.success(),
        "batch should exit 0:\nstdout: {stdout}\nstderr: {stderr}"
    );

    let j: Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("batch output should be JSON: {e}\nstdout: {stdout}"));
    assert_eq!(j["ok"].as_bool(), Some(true), "batch should report ok: {j}");

    if let Some(batches) = j["batches"].as_array() {
        assert!(!batches.is_empty(), "should have at least one batch report");
        if let Some(report) = batches[0].get("report") {
            if let Some(results) = report["results"].as_array() {
                assert_eq!(results.len(), 3, "batch has 3 commands: {report}");
                assert_eq!(results[0]["command"].as_str(), Some("open"));
                assert_eq!(results[0]["ok"].as_bool(), Some(true));
                assert_eq!(results[1]["command"].as_str(), Some("title"));
                let title = results[1]["result"]["title"].as_str().unwrap_or("");
                assert!(
                    title.contains("Example Domain"),
                    "batch title should contain 'Example Domain': {title}"
                );
            }
        }
    }

    common::teardown_headless(&sid, &tmp);
}

// ---------------------------------------------------------------------------
// Named batch
// ---------------------------------------------------------------------------

/// Execute a named batch from an inline JSON file.
#[test]
#[serial]
fn batch_named() {
    let (sid, tmp) = match common::setup_headless_session() {
        Some(v) => v,
        None => return,
    };

    let batch_content = json!({
        "batches": {
            "nav": {
                "description": "Navigate and get title",
                "commands": [
                    {"type": "open", "url": "https://example.com"},
                    {"type": "title"}
                ]
            },
            "interact": {
                "description": "Wait for heading",
                "commands": [
                    {"type": "wait", "selector": "h1", "condition": "visible", "timeout": 5000}
                ]
            }
        }
    });
    let batch_file = tmp.join("named-batch.json");
    std::fs::write(&batch_file, batch_content.to_string()).expect("write batch file");

    // Run only the "nav" batch
    let out = common::browsectl(&[
        "--session",
        &sid,
        "batch",
        "--file",
        &batch_file.to_string_lossy(),
        "--name",
        "nav",
    ]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "named batch should exit 0:\nstdout: {stdout}\nstderr: {stderr}"
    );

    let j: Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("named batch output should be JSON: {e}\nstdout: {stdout}"));
    assert_eq!(
        j["ok"].as_bool(),
        Some(true),
        "named batch should report ok: {j}"
    );

    // Run "interact" batch
    let out2 = common::browsectl(&[
        "--session",
        &sid,
        "batch",
        "--file",
        &batch_file.to_string_lossy(),
        "--name",
        "interact",
    ]);
    let stdout2 = String::from_utf8_lossy(&out2.stdout);
    assert!(
        out2.status.success(),
        "interact batch should succeed:\nstdout: {stdout2}"
    );

    common::teardown_headless(&sid, &tmp);
}
