//! Session lifecycle manager — create, attach, persist, and tear down browser sessions.
//!
//! This is the central coordination module.  Key responsibilities:
//!
//! - **Session creation** ([`create_session`]): ensures the WebDriver server
//!   is running, copies user data from the real browser profile, creates a
//!   new browser session, and persists it to `.browsectl/sessions.json`.
//!   Reuses cached setup info from `.browsectl/setup.json` when available.
//! - **Session resolution** ([`resolve_session`]): finds an existing session
//!   (in-memory → on-disk store → background job), or auto-creates one.
//! - **Profile management**: [`clone_profile`] clones a locked browser profile
//!   into a temp directory so sessions can start without killing the user's
//!   browser.  [`copy_profile_data`] selectively copies cookies, extensions,
//!   etc. from the real browser profile into the automation profile.
//! - **Tab management**: [`list_tabs`], [`create_tab`], [`switch_tab`],
//!   [`close_tab`].
//! - **Cleanup**: [`delete_session`] quits the browser and removes the session
//!   from the store.

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;
use tokio::time::sleep;

use crate::driver;
use crate::setup;
use crate::store;
use crate::types::*;
use crate::webdriver::WdClient;

// ---------------------------------------------------------------------------
// Capabilities builder — supports Chrome and Edge
// ---------------------------------------------------------------------------

/// If `path` is a bare command name (not an absolute path), resolves it to
/// its absolute path using `which` (Unix) or `where` (Windows).  Returns the
/// original string unchanged if resolution fails or the path is already
/// absolute.
fn resolve_binary_path(path: &str) -> String {
    if std::path::Path::new(path).is_absolute() {
        return path.to_string();
    }

    #[cfg(unix)]
    {
        if let Ok(output) = std::process::Command::new("which")
            .arg(path)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
        {
            if output.status.success() {
                let resolved = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !resolved.is_empty() {
                    return resolved;
                }
            }
        }
    }

    #[cfg(windows)]
    {
        if let Ok(output) = std::process::Command::new("where")
            .arg(path)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
        {
            if output.status.success() {
                // `where` may return multiple lines; take the first
                let resolved = String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .next()
                    .unwrap_or("")
                    .trim()
                    .to_string();
                if !resolved.is_empty() {
                    return resolved;
                }
            }
        }
    }

    path.to_string()
}

fn build_capabilities(
    config: &SessionConfig,
    user_data_dir: &str,
    profile_directory: &str,
) -> Value {
    // Both Chrome and Edge (Chromium-based) accept the same command-line
    // switches.  The only differences are browserName and the vendor
    // capability key.
    let mut args = vec![
        format!("--user-data-dir={}", user_data_dir),
        format!("--profile-directory={}", profile_directory),
        format!(
            "--window-size={},{}",
            config.viewport_width, config.viewport_height
        ),
        "--disable-dev-shm-usage".into(),
        "--no-first-run".into(),
        "--no-default-browser-check".into(),
        "--remote-debugging-port=0".into(),
    ];
    if config.headless {
        args.push("--headless=new".into());
        args.push("--no-sandbox".into());
    }

    let options_obj = json!({
        "binary": resolve_binary_path(&config.chrome_binary),
        "args": args
    });

    match config.browser {
        Browser::Chrome => json!({
            "browserName": "chrome",
            "goog:chromeOptions": options_obj
        }),
        Browser::Edge => json!({
            "browserName": "MicrosoftEdge",
            "ms:edgeOptions": options_obj
        }),
    }
}

// ---------------------------------------------------------------------------
// Profile cloning
// ---------------------------------------------------------------------------

pub async fn clone_profile(user_data_dir: &str, profile_directory: &str) -> Result<PathBuf> {
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();

    let temp_root = std::env::temp_dir().join(format!(
        "browsectl-profile-{}-{}",
        std::process::id(),
        timestamp
    ));
    tokio::fs::create_dir_all(&temp_root).await?;

    let src_profile = Path::new(user_data_dir).join(profile_directory);
    let dst_profile = temp_root.join(profile_directory);

    // Copy the profile directory — platform-specific command.
    #[cfg(unix)]
    {
        let status = Command::new("cp")
            .arg("-R")
            .arg(&src_profile)
            .arg(&dst_profile)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .context("failed to run cp for profile clone")?;
        if !status.success() {
            bail!(
                "cp -R failed copying {} -> {}",
                src_profile.display(),
                dst_profile.display()
            );
        }
    }

    #[cfg(windows)]
    {
        // robocopy returns 0-7 on success, ≥8 on error.
        let status = Command::new("robocopy")
            .args([
                &src_profile.to_string_lossy().to_string(),
                &dst_profile.to_string_lossy().to_string(),
                "/E", // recurse
                "/NFL",
                "/NDL",
                "/NJH",
                "/NJS", // quiet
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .context("failed to run robocopy for profile clone")?;
        let code = status.code().unwrap_or(99);
        if code >= 8 {
            bail!(
                "robocopy failed (exit {code}) copying {} -> {}",
                src_profile.display(),
                dst_profile.display()
            );
        }
    }

    // Try to copy "Local State" (ignore failure – it may not exist).
    let src_local_state = Path::new(user_data_dir).join("Local State");
    let dst_local_state = temp_root.join("Local State");

    #[cfg(unix)]
    {
        let _ = Command::new("cp")
            .arg("-R")
            .arg(&src_local_state)
            .arg(&dst_local_state)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;
    }

    #[cfg(windows)]
    {
        if src_local_state.exists() {
            let _ = tokio::fs::copy(&src_local_state, &dst_local_state).await;
        }
    }

    // Remove Chromium-based browser lock / singleton files from the clone
    // so the browser doesn't think the profile is already in use.
    // These lock files are used by both Chrome and Edge.
    let lock_files = ["SingletonLock", "SingletonCookie", "SingletonSocket"];
    for name in &lock_files {
        let _ = tokio::fs::remove_file(temp_root.join(name)).await;
    }
    // Also remove LevelDB LOCK files inside the profile directory itself.
    let _ = tokio::fs::remove_file(dst_profile.join("LOCK")).await;

    Ok(temp_root)
}

// ---------------------------------------------------------------------------
// Selective profile data copying (cookies, extensions, etc.)
// ---------------------------------------------------------------------------

/// Copies selected user data items from the real browser profile into the
/// automation user-data directory.  This runs BEFORE the session is created
/// so the browser picks up the copied data on startup.
///
/// - `real_user_data_dir`:  e.g. `~/Library/Application Support/Google/Chrome`
/// - `dest_user_data_dir`:  e.g. `~/.browsectl/chrome-profile`
/// - `profile_directory`:   e.g. `Default`
/// - `copy`:                which items to copy
pub async fn copy_profile_data(
    real_user_data_dir: &str,
    dest_user_data_dir: &str,
    profile_directory: &str,
    copy: &CopyDataConfig,
) -> Result<()> {
    if !copy.any() {
        return Ok(());
    }

    let src_profile = Path::new(real_user_data_dir).join(profile_directory);
    let dst_profile = Path::new(dest_user_data_dir).join(profile_directory);

    if !src_profile.exists() {
        eprintln!(
            "info: real browser profile not found at {}, skipping data copy",
            src_profile.display()
        );
        return Ok(());
    }

    // Ensure destination profile directory exists.
    tokio::fs::create_dir_all(&dst_profile).await?;

    let mut copied = Vec::new();

    // --- Cookies ---
    if copy.cookies {
        // Chromium stores cookies in a file called "Cookies" (SQLite db).
        // There may also be a "Cookies-journal" file.
        for name in &["Cookies", "Cookies-journal"] {
            let src = src_profile.join(name);
            let dst = dst_profile.join(name);
            if src.exists() {
                if let Err(e) = tokio::fs::copy(&src, &dst).await {
                    eprintln!("warning: failed to copy {}: {e}", src.display());
                } else {
                    copied.push(name.to_string());
                }
            }
        }
    }

    // --- Extensions ---
    if copy.extensions {
        let src_ext = src_profile.join("Extensions");
        let dst_ext = dst_profile.join("Extensions");
        if src_ext.exists() {
            if let Err(e) = copy_dir_recursive(&src_ext, &dst_ext).await {
                eprintln!("warning: failed to copy Extensions: {e}");
            } else {
                copied.push("Extensions".to_string());
            }
        }
        // Also copy extension-related preference keys by copying
        // "Secure Preferences" and "Preferences" if extensions are
        // requested (Chromium needs these to recognise installed exts).
        for name in &["Preferences", "Secure Preferences"] {
            let src = src_profile.join(name);
            let dst = dst_profile.join(name);
            if src.exists() && !dst.exists() {
                let _ = tokio::fs::copy(&src, &dst).await;
            }
        }
    }

    // --- Local Storage ---
    if copy.local_storage {
        let src_ls = src_profile.join("Local Storage");
        let dst_ls = dst_profile.join("Local Storage");
        if src_ls.exists() {
            if let Err(e) = copy_dir_recursive(&src_ls, &dst_ls).await {
                eprintln!("warning: failed to copy Local Storage: {e}");
            } else {
                copied.push("Local Storage".to_string());
            }
        }
    }

    // --- Bookmarks ---
    if copy.bookmarks {
        let src = src_profile.join("Bookmarks");
        let dst = dst_profile.join("Bookmarks");
        if src.exists() {
            if let Err(e) = tokio::fs::copy(&src, &dst).await {
                eprintln!("warning: failed to copy Bookmarks: {e}");
            } else {
                copied.push("Bookmarks".to_string());
            }
        }
    }

    // Also copy "Local State" at the user-data-dir level (not profile level)
    // — Chromium needs this for cookie encryption keys, extension registry, etc.
    let src_local_state = Path::new(real_user_data_dir).join("Local State");
    let dst_local_state = Path::new(dest_user_data_dir).join("Local State");
    if src_local_state.exists() && !dst_local_state.exists() {
        let _ = tokio::fs::copy(&src_local_state, &dst_local_state).await;
    }

    if copied.is_empty() {
        eprintln!("info: no profile data found to copy");
    } else {
        eprintln!("info: copied from real profile: {}", copied.join(", "));
    }

    Ok(())
}

/// Recursively copy a directory tree.
async fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    tokio::fs::create_dir_all(dst).await?;
    let mut entries = tokio::fs::read_dir(src).await?;
    while let Some(entry) = entries.next_entry().await? {
        let file_type = entry.file_type().await?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if file_type.is_dir() {
            Box::pin(copy_dir_recursive(&src_path, &dst_path)).await?;
        } else {
            tokio::fs::copy(&src_path, &dst_path).await?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Attach to an existing browser session
// ---------------------------------------------------------------------------

pub async fn attach_existing_session(record: &StoredSession) -> Result<WdClient> {
    let server_url = record.server.effective_url();
    let client = WdClient::attach(&server_url, &record.session_id);

    // Validate the session is actually alive (with timeout to avoid hangs).
    match tokio::time::timeout(Duration::from_secs(10), client.windows()).await {
        Ok(Ok(_handles)) => Ok(client),
        Ok(Err(e)) => {
            eprintln!(
                "warning: session {} validation failed: {e}",
                record.session_id
            );
            // Try to clean up the dead session from chromedriver so it
            // doesn't block future requests.
            let _ = client.delete_session().await;
            Err(anyhow::anyhow!("session validation failed: {e}"))
        }
        Err(_) => {
            eprintln!(
                "warning: session {} validation timed out (10 s) — \
                 the browser may be hung or showing a dialog",
                record.session_id
            );
            // The session is stuck; try to delete it from chromedriver
            // (best-effort, may also hang — use a short timeout).
            let delete_client = client.clone();
            let _ =
                tokio::time::timeout(Duration::from_secs(5), delete_client.delete_session()).await;
            Err(anyhow::anyhow!("session validation timed out (10s)"))
        }
    }
}

// ---------------------------------------------------------------------------
// Tab helpers
// ---------------------------------------------------------------------------

pub async fn collect_tabs(
    driver: &WdClient,
    aliases: &HashMap<String, String>,
) -> Result<StoredTabs> {
    let handles = driver.windows().await?;
    let current_handle = driver.window().await.ok();

    // Keep only aliases whose handle still exists.
    let filtered_aliases: HashMap<String, String> = aliases
        .iter()
        .filter(|(_, handle_val)| handles.contains(handle_val))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    Ok(StoredTabs {
        handles,
        current_handle,
        aliases: filtered_aliases,
    })
}

// ---------------------------------------------------------------------------
// Persist runtime state to the store
// ---------------------------------------------------------------------------

pub async fn upsert_runtime(ctx: &RuntimeCtx, set_default: bool) -> Result<()> {
    let tabs = collect_tabs(&ctx.driver, &ctx.tab_aliases).await?;
    let server = parse_server_url(&ctx.server_url);
    let now = now_iso();

    let session = StoredSession {
        session_id: ctx.session_id.clone(),
        server,
        capabilities: Value::Null,
        temp_profile_dir: ctx
            .temp_profile_dir
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned()),
        tabs,
        created_at: now.clone(),
        updated_at: now,
    };

    store::upsert(&ctx.session_id, session, set_default).await
}

// ---------------------------------------------------------------------------
// Helper: try to create a session with a timeout
// ---------------------------------------------------------------------------

async fn try_create_session(server_url: &str, caps: Value) -> Result<WdClient> {
    match tokio::time::timeout(
        Duration::from_secs(30),
        WdClient::create_session(server_url, caps),
    )
    .await
    {
        Ok(Ok((client, _caps))) => Ok(client),
        Ok(Err(e)) => Err(e),
        Err(_) => {
            bail!("session creation timed out (30s): possible profile lock or Chrome startup hang")
        }
    }
}

/// Returns `true` when the error looks like it could be caused by a locked
/// Chrome user-data directory (explicit error **or** a timeout that likely
/// means Chrome is stuck trying to acquire the profile lock).
fn is_profile_lock_error(msg: &str) -> bool {
    msg.contains("user data directory is already in use")
        || msg.contains("timed out")
        || msg.contains("DevToolsActivePort")
}

// ---------------------------------------------------------------------------
// Create a brand-new browser session
// ---------------------------------------------------------------------------

pub async fn create_session(config: &SessionConfig) -> Result<RuntimeCtx> {
    let user_data_dir = config
        .user_data_dir
        .clone()
        .unwrap_or_else(|| default_user_data_dir(config.browser));

    // ── Reuse setup info if available ─────────────────────────────────
    let setup_info = store::read_setup_info().await.ok();

    let (browser_version, driver_path) = match &setup_info {
        Some(info)
            if info.ready_browser.as_deref() == Some(&config.browser.to_string())
                && info.ready_driver_path.is_some() =>
        {
            // Setup has already confirmed the driver is ready for this browser.
            let cached_version = info
                .browsers
                .iter()
                .find(|b| b.browser == config.browser.to_string() && b.installed)
                .and_then(|b| b.version.clone());

            let cached_driver = info.ready_driver_path.clone().unwrap();

            // Quick sanity check: does the driver binary still exist?
            if Path::new(&cached_driver).exists() {
                eprintln!(
                    "info: reusing setup info — {} driver at {}",
                    config.browser, cached_driver
                );
                (cached_version, cached_driver)
            } else {
                eprintln!("info: cached driver path gone, re-detecting…");
                detect_and_ensure_driver(config).await?
            }
        }
        _ => detect_and_ensure_driver(config).await?,
    };

    let _ = browser_version; // may be used in the future

    let child = driver::ensure_running(&config.server_url, &driver_path).await?;

    // ── Copy user data from real browser profile ──────────────────────
    if config.copy_data.any() {
        if let Some(real_dir) = real_browser_user_data_dir(config.browser) {
            eprintln!(
                "info: copying user data ({}) from real {} profile…",
                config.copy_data.summary(),
                config.browser
            );
            if let Err(e) = copy_profile_data(
                &real_dir,
                &user_data_dir,
                &config.profile_directory,
                &config.copy_data,
            )
            .await
            {
                eprintln!("warning: profile data copy failed: {e:#}");
            }
        } else {
            eprintln!(
                "info: real {} browser profile not found, skipping data copy",
                config.browser
            );
        }
    }

    let caps = build_capabilities(config, &user_data_dir, &config.profile_directory);

    let mut temp_profile_dir: Option<PathBuf> = None;

    let driver = match try_create_session(&config.server_url, caps).await {
        Ok(d) => d,
        Err(e) => {
            let err_msg = e.to_string();

            if is_profile_lock_error(&err_msg) {
                eprintln!(
                    "info: profile appears locked ({err_msg}), \
                     cloning profile for a clean session…"
                );

                let _ = driver::ensure_running(&config.server_url, &driver_path).await;

                let temp_dir = clone_profile(&user_data_dir, &config.profile_directory).await?;
                let caps2 = build_capabilities(
                    config,
                    &temp_dir.to_string_lossy(),
                    &config.profile_directory,
                );
                temp_profile_dir = Some(temp_dir);
                try_create_session(&config.server_url, caps2)
                    .await
                    .map_err(|e2| {
                        anyhow::anyhow!("failed to create session even with cloned profile: {e2}")
                    })?
            } else {
                return Err(e);
            }
        }
    };

    let session_id = driver.session_id().to_string();

    let ctx = RuntimeCtx {
        driver,
        session_id,
        server_url: config.server_url.clone(),
        tab_aliases: HashMap::new(),
        temp_profile_dir,
        chromedriver_child: child,
    };

    upsert_runtime(&ctx, true).await?;

    Ok(ctx)
}

/// Fallback: detect browser version and ensure driver the traditional way.
async fn detect_and_ensure_driver(config: &SessionConfig) -> Result<(Option<String>, String)> {
    let browser_version = setup::detect_browser_version(&config.chrome_binary);
    let driver_path = match setup::ensure_driver(
        config.browser,
        browser_version.as_deref(),
        &config.chromedriver_path,
    )
    .await
    {
        Ok(path) => path,
        Err(e) => {
            eprintln!("warning: driver auto-setup failed: {e:#}");
            config.chromedriver_path.clone()
        }
    };
    Ok((browser_version, driver_path))
}

// ---------------------------------------------------------------------------
// Resolve (find-or-create) a session
// ---------------------------------------------------------------------------

pub async fn resolve_session(
    sessions: &mut HashMap<String, RuntimeCtx>,
    config: &SessionConfig,
    session_id: Option<&str>,
    auto_create: bool,
) -> Result<String> {
    let store_data = store::read_store().await?;

    let effective_id = session_id
        .map(|s| s.to_string())
        .or_else(|| store_data.default_session_id.clone());

    // Already live in memory?
    if let Some(ref id) = effective_id {
        if sessions.contains_key(id) {
            return Ok(id.clone());
        }
    }

    // Track whether we found a stored session that turned out to be stale,
    // so we can clean up Chrome before auto-creating.
    let mut had_stale_session = false;

    // Present in the persisted store?
    if let Some(ref id) = effective_id {
        if let Some(record) = store_data.sessions.get(id) {
            match attach_existing_session(record).await {
                Ok(driver) => {
                    let tab_aliases = record.tabs.aliases.clone();
                    let temp_profile_dir = record.temp_profile_dir.as_ref().map(PathBuf::from);
                    let ctx = RuntimeCtx {
                        driver,
                        session_id: id.clone(),
                        server_url: record.server.effective_url(),
                        tab_aliases,
                        temp_profile_dir,
                        chromedriver_child: None,
                    };
                    sessions.insert(id.clone(), ctx);
                    return Ok(id.clone());
                }
                Err(e) => {
                    eprintln!("info: stored session {id} is stale ({e}), removing");
                    had_stale_session = true;
                    // Remove from store.
                    let _ = store::remove(id).await;
                }
            }
        }
    }

    // Check if a background session-create job is still in progress.
    // If so, wait for it instead of creating yet another session.
    if let Some(result) = wait_for_pending_background_job().await {
        // Re-read the store — the background worker should have persisted
        // the session by now.
        let store_data = store::read_store().await?;
        if let Some(ref id) = store_data.default_session_id {
            if let Some(record) = store_data.sessions.get(id) {
                if let Ok(driver) = attach_existing_session(record).await {
                    let tab_aliases = record.tabs.aliases.clone();
                    let temp_profile_dir = record.temp_profile_dir.as_ref().map(PathBuf::from);
                    let ctx = RuntimeCtx {
                        driver,
                        session_id: id.clone(),
                        server_url: record.server.effective_url(),
                        tab_aliases,
                        temp_profile_dir,
                        chromedriver_child: None,
                    };
                    sessions.insert(id.clone(), ctx);
                    return Ok(id.clone());
                }
            }
        }
        // If the background job reported an error, surface it.
        if let Some(err) = result["error"].as_str() {
            bail!("background session-create failed: {err}");
        }
    }

    // Auto-create a fresh session.
    if auto_create {
        if had_stale_session {
            // The stale session has been removed from the store.
            // We do NOT kill Chrome — that would close the user's
            // unrelated browser windows.  Just make sure the driver
            // server is alive for session creation.
            let _ = driver::ensure_running(&config.server_url, &config.chromedriver_path).await;
        }

        let ctx = create_session(config).await?;
        let id = ctx.session_id.clone();
        upsert_runtime(&ctx, true).await?;
        sessions.insert(id.clone(), ctx);
        return Ok(id);
    }

    bail!("no active session found — run `session-create` first")
}

// ---------------------------------------------------------------------------
// Wait for a pending background session-create job
// ---------------------------------------------------------------------------

/// Scans `.browsectl/jobs/` for a recent output file (created within the last
/// 120 seconds) that does not yet exist or is still empty.  If found, polls
/// until it contains valid JSON (up to 90 s).  Returns `Some(result_json)` if
/// a pending job was found and completed, `None` if there was no pending job.
async fn wait_for_pending_background_job() -> Option<Value> {
    let jobs_dir = PathBuf::from(".browsectl/jobs");
    let mut entries = match tokio::fs::read_dir(&jobs_dir).await {
        Ok(e) => e,
        Err(_) => return None,
    };

    let now = std::time::SystemTime::now();
    let recency_window = Duration::from_secs(120);

    // Collect .json job files that are recent (by filename timestamp prefix).
    let mut candidates: Vec<PathBuf> = Vec::new();
    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) if n.ends_with(".json") => n.to_string(),
            _ => continue,
        };
        // Filename format: "<millis_since_epoch>-<pid>.json"
        if let Some(ts_str) = name.split('-').next() {
            if let Ok(ts_ms) = ts_str.parse::<u128>() {
                let file_time = std::time::UNIX_EPOCH + Duration::from_millis(ts_ms as u64);
                if let Ok(age) = now.duration_since(file_time) {
                    if age < recency_window {
                        candidates.push(path);
                    }
                }
            }
        }
    }

    if candidates.is_empty() {
        return None;
    }

    // Sort descending by name (most recent first) and pick the newest.
    candidates.sort();
    let target = candidates.last().unwrap().clone();

    // If the file already has valid JSON content, the job is done.
    if let Ok(raw) = tokio::fs::read_to_string(&target).await {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
                eprintln!("info: found completed background job: {}", target.display());
                return Some(v);
            }
        }
    }

    // File exists but is empty / incomplete → background worker is still
    // running.  Poll until it finishes.
    eprintln!(
        "info: background session-create in progress ({}), waiting…",
        target.display()
    );

    let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
    let poll_interval = Duration::from_millis(500);

    loop {
        if tokio::time::Instant::now() >= deadline {
            eprintln!("warning: timed out waiting for background job");
            return None;
        }

        if let Ok(raw) = tokio::fs::read_to_string(&target).await {
            let trimmed = raw.trim();
            if !trimmed.is_empty() {
                if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
                    eprintln!("info: background session-create finished");
                    return Some(v);
                }
            }
        }

        sleep(poll_interval).await;
    }
}

// ---------------------------------------------------------------------------
// Delete a session
// ---------------------------------------------------------------------------

pub async fn delete_session(
    sessions: &mut HashMap<String, RuntimeCtx>,
    session_id: Option<&str>,
) -> Result<Value> {
    let store_data = store::read_store().await?;

    let effective_id = session_id
        .map(|s| s.to_string())
        .or_else(|| store_data.default_session_id.clone())
        .ok_or_else(|| anyhow::anyhow!("no session to delete"))?;

    // If the session is live in memory, quit and remove it.
    if let Some(ctx) = sessions.remove(&effective_id) {
        let _ = ctx.driver.delete_session().await;
        if let Some(ref dir) = ctx.temp_profile_dir {
            let _ = tokio::fs::remove_dir_all(dir).await;
        }
    } else if let Some(record) = store_data.sessions.get(&effective_id) {
        // Try to attach so we can cleanly quit the browser.
        if let Ok(driver) = attach_existing_session(record).await {
            let _ = driver.delete_session().await;
        }
        if let Some(ref dir) = record.temp_profile_dir {
            let _ = tokio::fs::remove_dir_all(Path::new(dir)).await;
        }
    }

    // Remove from the persisted store.
    let updated_store = store::remove(&effective_id).await?;

    Ok(json!({
        "deletedSessionId": effective_id,
        "defaultSessionId": updated_store.default_session_id,
    }))
}

// ---------------------------------------------------------------------------
// Tab management
// ---------------------------------------------------------------------------

pub async fn list_tabs(ctx: &RuntimeCtx) -> Result<Value> {
    let tabs = collect_tabs(&ctx.driver, &ctx.tab_aliases).await?;

    let tab_list: Vec<Value> = tabs
        .handles
        .iter()
        .map(|h| {
            let aliases_for_handle: Vec<&String> = tabs
                .aliases
                .iter()
                .filter(|(_, v)| *v == h)
                .map(|(k, _)| k)
                .collect();

            json!({
                "handle": h,
                "active": tabs.current_handle.as_deref() == Some(h.as_str()),
                "aliases": aliases_for_handle,
            })
        })
        .collect();

    Ok(json!({
        "tabs": tab_list,
        "currentHandle": tabs.current_handle,
        "totalTabs": tabs.handles.len(),
    }))
}

pub async fn create_tab(
    ctx: &mut RuntimeCtx,
    url: Option<&str>,
    alias: Option<&str>,
    activate: bool,
) -> Result<Value> {
    let previous_handle = ctx.driver.window().await?;

    let open_url = url.unwrap_or("about:blank");
    let script = format!("window.open('{}', '_blank')", open_url);
    ctx.driver
        .execute(&script, vec![])
        .await
        .context("execute window.open failed")?;

    let handles = ctx.driver.windows().await?;

    let new_handle = handles
        .last()
        .ok_or_else(|| anyhow::anyhow!("no window handle found after creating tab"))?
        .clone();

    if activate {
        ctx.driver.switch_to_window(&new_handle).await?;
    } else {
        ctx.driver.switch_to_window(&previous_handle).await?;
    }

    if let Some(alias_name) = alias {
        ctx.tab_aliases
            .insert(alias_name.to_string(), new_handle.clone());
    }

    Ok(json!({
        "handle": new_handle,
        "alias": alias,
        "activated": activate,
        "totalTabs": handles.len(),
    }))
}

pub async fn switch_tab(ctx: &RuntimeCtx, tab_ref: &Value) -> Result<Value> {
    ctx.switch_to_tab(tab_ref).await?;

    let current = ctx.driver.window().await?;

    Ok(json!({
        "currentHandle": current,
    }))
}

pub async fn close_tab(ctx: &mut RuntimeCtx, tab_ref: &Value) -> Result<Value> {
    // Switch to the target tab.
    ctx.switch_to_tab(tab_ref).await?;

    let closed_handle = ctx.driver.window().await?;

    ctx.driver.close_window().await?;

    // Switch to the first remaining window.
    let remaining = ctx.driver.windows().await?;

    if let Some(first) = remaining.first() {
        ctx.driver.switch_to_window(first).await?;
    }

    // Remove any alias that pointed at the closed handle.
    ctx.tab_aliases.retain(|_, v| *v != closed_handle);

    let current = ctx.driver.window().await.ok();

    Ok(json!({
        "closedHandle": closed_handle,
        "currentHandle": current,
        "totalTabs": remaining.len(),
    }))
}
