//! Shared types, constants, and pure helper functions for the browsectl project.
//!
//! This is a leaf module imported by every other module via `use crate::types::*`.
//! It intentionally has **no** dependencies on other crate modules to avoid
//! circular imports.
//!
//! Contents:
//! - **Constants**: default hostname, port, profile name.
//! - **`Browser` enum**: Chrome / Edge with `Display`, `FromStr`, serde support.
//! - **Path helpers**: [`default_browser_binary`], [`default_driver_path`],
//!   [`default_user_data_dir`], [`real_browser_user_data_dir`].
//! - **Setup types**: [`SetupInfo`], [`SetupPlatform`], [`SetupBrowser`],
//!   [`SetupDriver`], [`CopyDataConfig`].
//! - **Session types**: [`SessionConfig`], [`SessionStoreData`],
//!   [`StoredSession`], [`StoredServer`], [`StoredTabs`].
//! - **Runtime types**: [`RuntimeCtx`], [`CommandSpec`], [`NamedBatch`],
//!   [`ParallelGroup`], [`ViewportSpec`].
//! - **Utilities**: [`now_iso`], [`parse_server_url`], [`is_leap`].

use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::process::Child;

use crate::webdriver::WdClient;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

pub const DEFAULT_HOSTNAME: &str = "127.0.0.1";
pub const DEFAULT_PORT: u16 = 9515;
pub const DEFAULT_PATH: &str = "/";
pub const DEFAULT_PROFILE: &str = "Default";

// ---------------------------------------------------------------------------
// Browser enum
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Browser {
    Chrome,
    Edge,
}

impl Default for Browser {
    fn default() -> Self {
        Browser::Chrome
    }
}

impl fmt::Display for Browser {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Browser::Chrome => write!(f, "chrome"),
            Browser::Edge => write!(f, "edge"),
        }
    }
}

impl FromStr for Browser {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "chrome" => Ok(Browser::Chrome),
            "edge" => Ok(Browser::Edge),
            other => Err(format!(
                "unknown browser: {other:?} (expected \"chrome\" or \"edge\")"
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// Platform detection
// ---------------------------------------------------------------------------

pub fn current_platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        "linux"
    }
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// Returns the default browser binary path for the given browser on the
/// current platform.
pub fn default_browser_binary(browser: Browser) -> String {
    let platform = current_platform();
    match (browser, platform) {
        (Browser::Chrome, "macos") => {
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome".to_string()
        }
        (Browser::Chrome, "windows") => {
            r"C:\Program Files\Google\Chrome\Application\chrome.exe".to_string()
        }
        (Browser::Chrome, _) => "google-chrome".to_string(),
        (Browser::Edge, "macos") => {
            "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge".to_string()
        }
        (Browser::Edge, "windows") => {
            r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe".to_string()
        }
        (Browser::Edge, _) => "microsoft-edge".to_string(),
    }
}

/// Returns the default driver binary path for the given browser on the
/// current platform.
pub fn default_driver_path(browser: Browser) -> String {
    let platform = current_platform();
    match (browser, platform) {
        (Browser::Chrome, "windows") => "./chromedriver.exe".to_string(),
        (Browser::Chrome, _) => "./chromedriver".to_string(),
        (Browser::Edge, "windows") => "./msedgedriver.exe".to_string(),
        (Browser::Edge, _) => "./msedgedriver".to_string(),
    }
}

/// Returns the default user-data directory for automation for the given
/// browser.
///
/// Instead of using the real browser profile — which is almost always locked
/// by a running browser instance — we default to a dedicated directory that
/// won't conflict with the user's normal browser.
///
/// Users can still override this with `--user-data-dir` if they want to
/// reuse their real profile (after closing the browser).
pub fn default_user_data_dir(browser: Browser) -> String {
    let subdir = match browser {
        Browser::Chrome => "chrome-profile",
        Browser::Edge => "edge-profile",
    };
    match dirs::home_dir() {
        Some(home) => home
            .join(format!(".browsectl/{subdir}"))
            .to_string_lossy()
            .into_owned(),
        None => format!(".browsectl/{subdir}"),
    }
}

/// Returns the path to the user's real (day-to-day) browser user-data
/// directory, if it exists.  This is the profile that contains their
/// actual cookies, extensions, bookmarks, etc.
pub fn real_browser_user_data_dir(browser: Browser) -> Option<String> {
    let platform = current_platform();
    let path = match (browser, platform) {
        (Browser::Chrome, "macos") => {
            dirs::home_dir()?.join("Library/Application Support/Google/Chrome")
        }
        (Browser::Chrome, "windows") => dirs::data_local_dir()?.join(r"Google\Chrome\User Data"),
        (Browser::Chrome, _) => {
            // Linux
            dirs::config_dir()?.join("google-chrome")
        }
        (Browser::Edge, "macos") => {
            dirs::home_dir()?.join("Library/Application Support/Microsoft Edge")
        }
        (Browser::Edge, "windows") => dirs::data_local_dir()?.join(r"Microsoft\Edge\User Data"),
        (Browser::Edge, _) => dirs::config_dir()?.join("microsoft-edge"),
    };
    if path.exists() {
        Some(path.to_string_lossy().into_owned())
    } else {
        None
    }
}

/// Returns the path used to persist session information between runs.
pub fn session_store_path() -> PathBuf {
    PathBuf::from(".browsectl/sessions.json")
}

/// Information detected by the `setup` command, persisted so that other
/// commands can reuse it without re-detecting the environment.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SetupInfo {
    pub platform: SetupPlatform,
    pub browsers: Vec<SetupBrowser>,
    pub drivers: Vec<SetupDriver>,
    /// The browser that `setup` confirmed is ready (driver downloaded & matched).
    #[serde(default, rename = "readyBrowser")]
    pub ready_browser: Option<String>,
    #[serde(default, rename = "readyDriverPath")]
    pub ready_driver_path: Option<String>,
    #[serde(default, rename = "updatedAt")]
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SetupPlatform {
    pub os: String,
    pub arch: String,
    pub display: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SetupBrowser {
    pub browser: String,
    pub path: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default, rename = "majorVersion")]
    pub major_version: Option<u32>,
    pub installed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SetupDriver {
    pub browser: String,
    pub path: String,
    #[serde(default)]
    pub version: Option<String>,
    pub exists: bool,
    /// Whether the driver major version matches the browser major version.
    #[serde(default, rename = "versionMatch")]
    pub matched: bool,
}

/// Controls what user data to copy from the real browser profile into the
/// automation profile before starting the session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CopyDataConfig {
    /// Copy the Cookies database.
    #[serde(default = "default_true")]
    pub cookies: bool,
    /// Copy installed extensions / plugins.
    #[serde(default = "default_true")]
    pub extensions: bool,
    /// Copy Local Storage data.
    #[serde(default)]
    pub local_storage: bool,
    /// Copy bookmarks.
    #[serde(default)]
    pub bookmarks: bool,
}

fn default_true() -> bool {
    true
}

impl Default for CopyDataConfig {
    fn default() -> Self {
        Self {
            cookies: true,
            extensions: true,
            local_storage: false,
            bookmarks: false,
        }
    }
}

impl CopyDataConfig {
    /// A config that copies nothing.
    pub fn none() -> Self {
        Self {
            cookies: false,
            extensions: false,
            local_storage: false,
            bookmarks: false,
        }
    }

    /// Returns `true` if at least one item is selected for copying.
    pub fn any(&self) -> bool {
        self.cookies || self.extensions || self.local_storage || self.bookmarks
    }

    /// Returns a human-readable summary of what will be copied.
    pub fn summary(&self) -> String {
        let mut items = Vec::new();
        if self.cookies {
            items.push("cookies");
        }
        if self.extensions {
            items.push("extensions");
        }
        if self.local_storage {
            items.push("local_storage");
        }
        if self.bookmarks {
            items.push("bookmarks");
        }
        if items.is_empty() {
            "nothing".to_string()
        } else {
            items.join(", ")
        }
    }
}

/// Returns the path used to persist setup information between runs.
pub fn setup_info_path() -> PathBuf {
    PathBuf::from(".browsectl/setup.json")
}

/// Builds the default WebDriver server URL from the default hostname and port.
pub fn default_server_url() -> String {
    format!("http://{}:{}", DEFAULT_HOSTNAME, DEFAULT_PORT)
}

/// Returns the current time as an ISO-like timestamp string
/// (e.g. `"2025-01-15T08:30:00Z"`).
pub fn now_iso() -> String {
    use std::time::SystemTime;

    let duration = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();

    let total_secs = duration.as_secs();

    // Manual calendar decomposition (no chrono dependency).
    let secs_per_minute: u64 = 60;
    let secs_per_hour: u64 = 3600;
    let secs_per_day: u64 = 86400;

    let days = total_secs / secs_per_day;
    let remaining = total_secs % secs_per_day;
    let hour = remaining / secs_per_hour;
    let minute = (remaining % secs_per_hour) / secs_per_minute;
    let second = remaining % secs_per_minute;

    // Days since 1970-01-01 → year/month/day.
    let mut y: i64 = 1970;
    let mut remaining_days = days as i64;

    loop {
        let days_in_year: i64 = if is_leap(y) { 366 } else { 365 };
        if remaining_days < days_in_year {
            break;
        }
        remaining_days -= days_in_year;
        y += 1;
    }

    let leap = is_leap(y);
    let month_days: [i64; 12] = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];

    let mut m: usize = 0;
    for (i, &md) in month_days.iter().enumerate() {
        if remaining_days < md {
            m = i;
            break;
        }
        remaining_days -= md;
    }

    let d = remaining_days + 1; // 1-based day

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        y,
        m + 1,
        d,
        hour,
        minute,
        second,
    )
}

fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || (y % 400 == 0)
}

/// Parses a WebDriver server URL string into a [`StoredServer`].
/// On parse failure the returned struct uses the default values.
pub fn parse_server_url(url: &str) -> StoredServer {
    match reqwest::Url::parse(url) {
        Ok(parsed) => {
            let hostname = parsed.host_str().unwrap_or(DEFAULT_HOSTNAME).to_string();
            let port = parsed.port().unwrap_or(DEFAULT_PORT);
            let path = parsed.path().to_string();
            let reconstructed =
                format!("http://{}:{}{}", hostname, port, path.trim_end_matches('/'));
            StoredServer {
                hostname,
                port,
                path,
                url: reconstructed,
            }
        }
        Err(_) => StoredServer::default(),
    }
}

// ---------------------------------------------------------------------------
// Structs
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct ViewportSpec {
    pub width: i64,
    pub height: i64,
}

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct CommandSpec {
    #[serde(rename = "type", default)]
    pub command_type: String,
    #[serde(default)]
    pub selector: Option<String>,
    /// Optional CSS selector that limits element lookups to within the
    /// first matched scope element. If omitted, searches the full document.
    #[serde(default)]
    pub scope: Option<String>,
    /// Fallback selector used when a native click is intercepted.
    /// If provided, this element will be clicked instead of the original.
    /// Special values: "parent" clicks the parent element, "sibling" clicks
    /// the next sibling element.
    #[serde(default)]
    pub fallback: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub ms: Option<u64>,
    #[serde(default)]
    pub condition: Option<String>,
    #[serde(default)]
    pub attribute: Option<String>,
    #[serde(default)]
    pub value: Option<String>,
    #[serde(default)]
    pub timeout: Option<u64>,
    #[serde(default)]
    pub interval: Option<u64>,
    #[serde(default)]
    pub direction: Option<String>,
    #[serde(default)]
    pub amount: Option<i64>,
    #[serde(default)]
    pub behavior: Option<String>,
    #[serde(default)]
    pub tab: Option<Value>,
    #[serde(default)]
    pub viewport: Option<ViewportSpec>,
    #[serde(default, rename = "continueOnError")]
    pub continue_on_error: bool,
    #[serde(default)]
    pub alias: Option<String>,
    #[serde(default)]
    pub activate: Option<bool>,
    #[serde(default)]
    pub groups: Option<Vec<ParallelGroup>>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ParallelGroup {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub tab: Option<Value>,
    #[serde(default)]
    pub commands: Vec<CommandSpec>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct NamedBatch {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default, rename = "continueOnError")]
    pub continue_on_error: bool,
    #[serde(default)]
    pub commands: Vec<CommandSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SessionStoreData {
    #[serde(default, rename = "defaultSessionId")]
    pub default_session_id: Option<String>,
    #[serde(default)]
    pub sessions: HashMap<String, StoredSession>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StoredSession {
    #[serde(rename = "sessionId")]
    pub session_id: String,
    pub server: StoredServer,
    #[serde(default)]
    pub capabilities: Value,
    #[serde(default, rename = "tempProfileDir")]
    pub temp_profile_dir: Option<String>,
    #[serde(default)]
    pub tabs: StoredTabs,
    #[serde(default, rename = "createdAt")]
    pub created_at: String,
    #[serde(default, rename = "updatedAt")]
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct StoredServer {
    pub hostname: String,
    pub port: u16,
    pub path: String,
    pub url: String,
}

impl Default for StoredServer {
    fn default() -> Self {
        Self {
            hostname: DEFAULT_HOSTNAME.to_string(),
            port: DEFAULT_PORT,
            path: DEFAULT_PATH.to_string(),
            url: format!("http://{}:{}", DEFAULT_HOSTNAME, DEFAULT_PORT),
        }
    }
}

impl StoredServer {
    /// Returns the effective server URL.
    ///
    /// JS-created sessions store `hostname`/`port`/`path` but no `url` field,
    /// so we reconstruct it when missing.
    pub fn effective_url(&self) -> String {
        if !self.url.is_empty() {
            self.url.clone()
        } else {
            format!(
                "http://{}:{}{}",
                self.hostname,
                self.port,
                self.path.trim_end_matches('/')
            )
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StoredTabs {
    #[serde(default)]
    pub handles: Vec<String>,
    #[serde(default, rename = "currentHandle")]
    pub current_handle: Option<String>,
    #[serde(default)]
    pub aliases: HashMap<String, String>,
}

/// Configuration used when creating or resolving a browser session.
///
/// Derives `Serialize` / `Deserialize` so it can be passed to background
/// worker processes via an environment variable.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionConfig {
    #[serde(default)]
    pub browser: Browser,
    pub server_url: String,
    pub chromedriver_path: String,
    pub chrome_binary: String,
    pub user_data_dir: Option<String>,
    pub profile_directory: String,
    pub headless: bool,
    pub viewport_width: i64,
    pub viewport_height: i64,
    #[serde(default)]
    pub copy_data: CopyDataConfig,
}

impl Default for SessionConfig {
    fn default() -> Self {
        let browser = Browser::default();
        Self {
            browser,
            server_url: default_server_url(),
            chromedriver_path: default_driver_path(browser),
            chrome_binary: default_browser_binary(browser),
            user_data_dir: None,
            profile_directory: DEFAULT_PROFILE.to_string(),
            headless: false,
            viewport_width: 1024,
            viewport_height: 768,
            copy_data: CopyDataConfig::default(),
        }
    }
}

/// Holds a live browser session together with associated runtime state.
///
/// The `driver` field is a lightweight [`WdClient`] which is `Clone` and does
/// **not** close the session on drop.
pub struct RuntimeCtx {
    pub driver: WdClient,
    pub session_id: String,
    pub server_url: String,
    pub tab_aliases: HashMap<String, String>,
    pub temp_profile_dir: Option<PathBuf>,
    pub chromedriver_child: Option<Child>,
}

impl RuntimeCtx {
    /// Switch the active browser window/tab.
    ///
    /// `tab_ref` may be:
    /// - a numeric index (JSON number) into the window-handles list,
    /// - the string `"current"` (no-op),
    /// - a previously-registered alias,
    /// - a stringified numeric index, or
    /// - a raw window handle id.
    pub async fn switch_to_tab(&self, tab_ref: &Value) -> anyhow::Result<()> {
        let handles = self.driver.windows().await?;

        // --- numeric index (e.g. 0, 1, 2 …) ---
        if let Some(index) = tab_ref.as_u64() {
            let handle = handles
                .get(index as usize)
                .ok_or_else(|| anyhow::anyhow!("tab index not found: {index}"))?;
            self.driver.switch_to_window(handle).await?;
            return Ok(());
        }

        // --- string-based lookup ---
        if let Some(text) = tab_ref.as_str() {
            // "current" → no-op
            if text == "current" {
                return Ok(());
            }

            // alias lookup
            if let Some(handle) = self.tab_aliases.get(text) {
                self.driver.switch_to_window(handle).await?;
                return Ok(());
            }

            // stringified numeric index
            if let Ok(idx) = text.parse::<usize>() {
                let handle = handles
                    .get(idx)
                    .ok_or_else(|| anyhow::anyhow!("tab index not found: {idx}"))?;
                self.driver.switch_to_window(handle).await?;
                return Ok(());
            }

            // raw handle id
            if handles.iter().any(|h| h == text) {
                self.driver.switch_to_window(text).await?;
                return Ok(());
            }
        }

        anyhow::bail!("tab not found: {tab_ref}")
    }
}
