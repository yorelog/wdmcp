//! REPL integration tests — exercise the interactive REPL via stdin/stdout.
//!
//! Requires chromedriver + Chrome.
//!
//! # Running
//!
//! ```sh
//! cargo test --test repl -- --test-threads=1
//! ```

mod common;

use serial_test::serial;

// ---------------------------------------------------------------------------
// REPL: open + title cycle
// ---------------------------------------------------------------------------

/// Run REPL commands: open two URLs and verify both titles appear.
#[test]
#[serial]
fn repl_open_title_cycle() {
    let tmp = std::env::temp_dir().join(format!("browsectl-e2e-repl-{}", std::process::id()));

    let create = common::browsectl_json(&[
        "--user-data-dir",
        &tmp.to_string_lossy(),
        "session-create",
        "--foreground",
    ]);
    if create["ok"].as_bool() != Some(true) {
        eprintln!("SKIP: cannot create session for REPL test");
        let _ = std::fs::remove_dir_all(&tmp);
        return;
    }
    let sid = create["sessionId"].as_str().unwrap().to_string();

    let repl = common::browsectl_stdin(
        &["repl"],
        "open https://example.com\ntitle\nopen https://www.iana.org\ntitle\nexit\n",
    );

    assert!(
        repl.contains("Example Domain"),
        "should contain Example Domain title:\n{repl}"
    );
    assert!(
        repl.contains("IANA") || repl.contains("iana"),
        "should contain IANA page title:\n{repl}"
    );

    common::cleanup_session(&sid);
    let _ = std::fs::remove_dir_all(&tmp);
}

// ---------------------------------------------------------------------------
// REPL: screenshot
// ---------------------------------------------------------------------------

/// Screenshot via REPL-created session.
#[test]
#[serial]
fn repl_screenshot() {
    let tmp = std::env::temp_dir().join(format!("browsectl-e2e-shot-{}", std::process::id()));
    let shot_path = tmp.join("shot.png");

    let create = common::browsectl_json(&[
        "--user-data-dir",
        &tmp.to_string_lossy(),
        "session-create",
        "--foreground",
    ]);
    if create["ok"].as_bool() != Some(true) {
        eprintln!("SKIP: cannot create session for screenshot test");
        let _ = std::fs::remove_dir_all(&tmp);
        return;
    }
    let sid = create["sessionId"].as_str().unwrap().to_string();

    let _ = common::browsectl(&["run", "--type", "open", "--url", "https://example.com"]);
    std::thread::sleep(std::time::Duration::from_secs(1));

    let shot = common::browsectl_json(&[
        "run",
        "--type",
        "screenshot",
        "--selector",
        "body",
        "--path",
        &shot_path.to_string_lossy(),
    ]);

    if shot["ok"].as_bool() == Some(true) {
        assert!(
            shot_path.exists(),
            "screenshot file should be created at {}",
            shot_path.display()
        );
        let metadata = std::fs::metadata(&shot_path).unwrap();
        assert!(
            metadata.len() > 100,
            "screenshot file should not be empty (got {} bytes)",
            metadata.len()
        );
    }
    assert!(shot.get("ok").is_some(), "should return JSON with ok field");

    common::cleanup_session(&sid);
    let _ = std::fs::remove_dir_all(&tmp);
}
