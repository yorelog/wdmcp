//! Driver tests — verify chromedriver can be started / detected.
//!
//! Requires chromedriver binary but does NOT create browser sessions.
//!
//! # Running
//!
//! ```sh
//! cargo test --test driver -- --test-threads=1
//! ```

mod common;

use serial_test::serial;

// ---------------------------------------------------------------------------
// Driver start
// ---------------------------------------------------------------------------

/// `driver-start` brings up chromedriver (or reports reused).
#[test]
#[serial]
fn driver_start() {
    let val = common::browsectl_json(&["driver-start"]);
    assert_eq!(
        val["ok"].as_bool(),
        Some(true),
        "driver-start should succeed: {val}"
    );
    assert!(
        val.get("reused").is_some(),
        "driver-start should report 'reused' field: {val}"
    );
}
