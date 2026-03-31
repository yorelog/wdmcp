//! Shared test helpers for browsectl integration tests.
//!
//! Include in any test file with:
//!   `mod common;`
//!
//! Each integration test crate is compiled independently, so not every test
//! file uses every helper.  The module-level `#![allow(dead_code)]` prevents
//! spurious warnings for helpers that only some test files call.
#![allow(dead_code)]

use std::path::PathBuf;
use std::process::{Command, Output};

use serde_json::Value;

// ---------------------------------------------------------------------------
// Binary / paths
// ---------------------------------------------------------------------------

/// Returns the path to the compiled `browsectl` binary.
pub fn browsectl_bin() -> PathBuf {
    let mut path = std::env::current_exe()
        .expect("cannot determine test exe path")
        .parent()
        .expect("no parent dir")
        .parent()
        .expect("no grandparent dir")
        .to_path_buf();
    path.push("browsectl");
    if cfg!(windows) {
        path.set_extension("exe");
    }
    assert!(
        path.exists(),
        "browsectl binary not found at {}. Run `cargo build` first.",
        path.display()
    );
    path
}

/// Returns the project root directory (where Cargo.toml lives).
pub fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Returns the `file://` URL for the local test page fixture.
pub fn test_page_url() -> String {
    let path = project_root()
        .join("tests")
        .join("fixtures")
        .join("test-page.html");
    assert!(path.exists(), "test page not found at {}", path.display());
    format!("file://{}", path.display())
}

// ---------------------------------------------------------------------------
// Run helpers
// ---------------------------------------------------------------------------

/// Run `browsectl <args>` in the project root directory and return the output.
pub fn browsectl(args: &[&str]) -> Output {
    let bin = browsectl_bin();
    Command::new(&bin)
        .args(args)
        .current_dir(project_root())
        .env("NO_COLOR", "1")
        .output()
        .unwrap_or_else(|e| panic!("failed to run {}: {e}", bin.display()))
}

/// Run `browsectl <args>` with stdin input and return full Output.
pub fn browsectl_with_input(args: &[&str], input: &str) -> Output {
    use std::io::Write;
    use std::process::Stdio;

    let bin = browsectl_bin();
    let mut child = Command::new(&bin)
        .args(args)
        .current_dir(project_root())
        .env("NO_COLOR", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("failed to spawn {}: {e}", bin.display()));

    if let Some(ref mut stdin) = child.stdin {
        stdin
            .write_all(input.as_bytes())
            .unwrap_or_else(|e| panic!("failed writing to child stdin: {e}"));
    }
    drop(child.stdin.take());

    child
        .wait_with_output()
        .unwrap_or_else(|e| panic!("failed waiting for {}: {e}", bin.display()))
}

/// Run `browsectl <args>` and parse stdout as JSON.
pub fn browsectl_json(args: &[&str]) -> Value {
    let out = browsectl(args);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    serde_json::from_str(stdout.trim()).unwrap_or_else(|e| {
        panic!(
            "failed to parse JSON from `browsectl {}`:\n\
             parse error: {e}\n\
             --- stdout ---\n{stdout}\n\
             --- stderr ---\n{stderr}",
            args.join(" ")
        )
    })
}

/// Run `browsectl <args>`, assert exit code 0, and return parsed JSON from stdout.
pub fn browsectl_ok(args: &[&str]) -> Value {
    let out = browsectl(args);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "`browsectl {}` exited with {:?}\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}",
        args.join(" "),
        out.status.code()
    );
    serde_json::from_str(stdout.trim()).unwrap_or_else(|e| {
        panic!(
            "JSON parse error from `browsectl {}`: {e}\n--- stdout ---\n{stdout}",
            args.join(" ")
        )
    })
}

/// Feed `input` to stdin of `browsectl <args>` and return stdout as a String.
pub fn browsectl_stdin(args: &[&str], input: &str) -> String {
    use std::io::Write;
    use std::process::Stdio;

    let bin = browsectl_bin();
    let mut child = Command::new(&bin)
        .args(args)
        .current_dir(project_root())
        .env("NO_COLOR", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("failed to spawn browsectl: {e}"));

    if let Some(ref mut stdin) = child.stdin {
        stdin.write_all(input.as_bytes()).ok();
    }
    drop(child.stdin.take());

    let out = child
        .wait_with_output()
        .expect("failed to wait for browsectl child");
    String::from_utf8_lossy(&out.stdout).to_string()
}

// ---------------------------------------------------------------------------
// Session helpers
// ---------------------------------------------------------------------------

/// Delete session by id, ignoring errors.
pub fn cleanup_session(session_id: &str) {
    let _ = browsectl(&["--session", session_id, "session-delete"]);
}

/// Create a headless session with a fresh temp profile.
/// Returns (session_id, tmp_dir).  Returns None if Chrome is unavailable.
pub fn setup_headless_session() -> Option<(String, PathBuf)> {
    let tmp = std::env::temp_dir().join(format!("browsectl-e2e-hl-{}-{}", std::process::id(), {
        use std::time::SystemTime;
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos()
    }));

    let drv = browsectl_json(&["driver-start"]);
    if drv["ok"].as_bool() != Some(true) {
        eprintln!("SKIP: driver-start failed");
        return None;
    }

    let create = browsectl_json(&[
        "--headless",
        "--user-data-dir",
        &tmp.to_string_lossy(),
        "session-create",
        "--foreground",
    ]);

    if create["ok"].as_bool() != Some(true) {
        let err = create["error"].as_str().unwrap_or("unknown");
        eprintln!("SKIP: headless session-create failed: {err}");
        let _ = std::fs::remove_dir_all(&tmp);
        return None;
    }

    let sid = create["sessionId"]
        .as_str()
        .expect("sessionId missing from create response")
        .to_string();

    Some((sid, tmp))
}

/// Tear down a headless session and clean up the temp profile directory.
pub fn teardown_headless(session_id: &str, tmp: &PathBuf) {
    cleanup_session(session_id);
    let _ = std::fs::remove_dir_all(tmp);
}
