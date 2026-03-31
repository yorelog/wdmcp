//! driver.rs — WebDriver server management and browser process helpers.
//!
//! This module has two sections:
//!
//! **Active API** — used by the session lifecycle:
//! - [`fetch_status`]: GET /status from the WebDriver server.
//! - [`is_ready`]: quick connectivity check.
//! - [`ensure_running`]: start chromedriver / msedgedriver if not already alive.
//!
//! **Dormant utilities** — browser process management functions that are
//! intentionally kept but **not currently called**.  The session manager
//! switched from a "kill-the-browser-to-release-the-profile-lock" strategy
//! to a safer "clone the profile" approach (see [`crate::manager::clone_profile`]).
//! These utilities are retained because:
//! - They may be needed for a future `--force` / `--kill-existing` flag.
//! - They document how to interact with browser processes cross-platform.
//! - They serve as reference implementations for macOS `pgrep`/`pkill` and
//!   Windows `tasklist`/`taskkill` patterns.
//!
//! The dormant section is gated with `#[allow(dead_code)]` at the item level
//! to silence compiler warnings while keeping the code visible for review.

use anyhow::{Context, Result, bail};
use reqwest::Client;
use serde_json::Value;
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::process::{Child, Command};
use tokio::time::sleep;

use crate::types::Browser;

// ---------------------------------------------------------------------------
// WebDriver server status
// ---------------------------------------------------------------------------

/// GET /status from the WebDriver server and return the JSON response.
pub async fn fetch_status(server_url: &str) -> Result<Value> {
    let client = Client::new();
    let url = format!("{}/status", server_url.trim_end_matches('/'));
    let response = client.get(&url).send().await?;
    if !response.status().is_success() {
        bail!("status failed: {}", response.status());
    }
    Ok(response.json::<Value>().await?)
}

/// Returns true if the WebDriver server at `server_url` is ready.
pub async fn is_ready(server_url: &str) -> bool {
    fetch_status(server_url).await.is_ok()
}

/// Start the WebDriver server (chromedriver / msedgedriver) if not already
/// running.  Returns the `Child` process if started, `None` if already
/// running.
pub async fn ensure_running(server_url: &str, driver_path: &str) -> Result<Option<Child>> {
    if is_ready(server_url).await {
        return Ok(None);
    }
    // Parse port from server_url (default 9515)
    let port = reqwest::Url::parse(server_url)
        .ok()
        .and_then(|u| u.port())
        .unwrap_or(9515);

    let mut cmd = Command::new(driver_path);
    cmd.arg(format!("--port={port}"));
    cmd.stdout(Stdio::null()).stderr(Stdio::null());

    // On Windows, create the process in a new process group so it can
    // outlive the parent without holding the console.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;
        cmd.creation_flags(CREATE_NEW_PROCESS_GROUP);
    }

    let child = cmd
        .spawn()
        .with_context(|| format!("failed to start driver: {driver_path}"))?;

    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        if is_ready(server_url).await {
            return Ok(Some(child));
        }
        sleep(Duration::from_millis(250)).await;
    }
    bail!("driver startup timeout (15 s) for {driver_path}")
}

// ---------------------------------------------------------------------------
// Dormant: browser process management (kept for future use; see module docs)
// ---------------------------------------------------------------------------

/// Process names / patterns used by `is_browser_running` and
/// `stop_local_browser` on each platform.
#[allow(dead_code)]
struct BrowserProcessInfo {
    /// Exact process name for `pgrep -x` (macOS) or `tasklist` (Windows).
    exact_name: &'static str,
    /// Pattern that matches helper processes (renderers, GPU, network, etc.)
    /// for `pgrep -f` (macOS) or `taskkill /im` (Windows).
    helper_pattern: &'static str,
    /// Image name used on Windows with `taskkill /im`.
    #[allow(dead_code)]
    win_image_name: &'static str,
}

/// Returns platform-specific process names and patterns for the given browser.
#[allow(dead_code)]
fn browser_process_info(browser: Browser) -> BrowserProcessInfo {
    match browser {
        Browser::Chrome => BrowserProcessInfo {
            exact_name: "Google Chrome",
            helper_pattern: "Google Chrome.app",
            win_image_name: "chrome.exe",
        },
        Browser::Edge => BrowserProcessInfo {
            exact_name: "Microsoft Edge",
            helper_pattern: "Microsoft Edge.app",
            win_image_name: "msedge.exe",
        },
    }
}

/// Check whether the given browser is currently running locally.
///
/// On macOS/Linux uses `pgrep`; on Windows uses `tasklist`.
#[allow(dead_code)]
pub async fn is_browser_running(browser: Browser) -> bool {
    let info = browser_process_info(browser);

    #[cfg(unix)]
    {
        let status = Command::new("pgrep")
            .args(["-x", info.exact_name])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;
        matches!(status, Ok(s) if s.success())
    }

    #[cfg(windows)]
    {
        let output = Command::new("tasklist")
            .args(["/FI", &format!("IMAGENAME eq {}", info.win_image_name)])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .await;
        match output {
            Ok(o) => {
                let text = String::from_utf8_lossy(&o.stdout);
                text.contains(info.win_image_name)
            }
            Err(_) => false,
        }
    }

    #[cfg(not(any(unix, windows)))]
    {
        let _ = info;
        false
    }
}

/// Kill **all** local processes of the given browser (main + helpers) so that
/// profile locks are released.
///
/// On macOS/Linux uses `pkill`; on Windows uses `taskkill`.
/// The patterns are specific enough to never accidentally kill the WebDriver
/// server process (chromedriver / msedgedriver).
#[allow(dead_code)]
pub async fn stop_local_browser(browser: Browser) {
    let info = browser_process_info(browser);

    #[cfg(unix)]
    {
        // Graceful SIGTERM on the main browser process.
        let _ = Command::new("pkill")
            .args(["-x", info.exact_name])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;

        sleep(Duration::from_millis(500)).await;

        // SIGTERM all helper processes (renderers, GPU, network, etc.)
        // Pattern matches the .app bundle path — won't hit chromedriver
        // or msedgedriver.
        let _ = Command::new("pkill")
            .args(["-f", info.helper_pattern])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;

        sleep(Duration::from_millis(500)).await;

        // SIGKILL any stragglers.
        let _ = Command::new("pkill")
            .args(["-9", "-f", info.helper_pattern])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;

        sleep(Duration::from_millis(300)).await;
    }

    #[cfg(windows)]
    {
        // On Windows, taskkill /F /IM kills all processes with that image
        // name.  /T also kills child processes.
        let _ = Command::new("taskkill")
            .args(["/F", "/IM", info.win_image_name, "/T"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;

        sleep(Duration::from_millis(1000)).await;
    }

    #[cfg(not(any(unix, windows)))]
    {
        let _ = info;
    }
}
