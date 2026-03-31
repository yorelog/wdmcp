//! setup.rs — platform detection, browser discovery, and WebDriver auto-download.

use anyhow::{Context, Result, bail};
use futures_util::StreamExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::process::Command;

use serde::{Deserialize, Serialize};

use crate::types::Browser;

// ---------------------------------------------------------------------------
// Platform detection
// ---------------------------------------------------------------------------

/// Detected platform (OS + architecture) information.
///
/// Returned by [`detect_platform`] and used to select the correct driver
/// download for the current system.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlatformInfo {
    pub os: String,
    pub arch: String,
    pub display: String,
}

pub fn detect_platform() -> PlatformInfo {
    let os = if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        "linux"
    };

    let arch = if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "x64"
    };

    let display = match (os, arch) {
        ("macos", "arm64") => "macOS arm64 (Apple Silicon)".to_string(),
        ("macos", _) => "macOS x64 (Intel)".to_string(),
        ("windows", "arm64") => "Windows arm64".to_string(),
        ("windows", _) => "Windows x64".to_string(),
        ("linux", "arm64") => "Linux arm64".to_string(),
        ("linux", _) => "Linux x64".to_string(),
        _ => format!("{os} {arch}"),
    };

    PlatformInfo {
        os: os.to_string(),
        arch: arch.to_string(),
        display,
    }
}

// ---------------------------------------------------------------------------
// Version helpers
// ---------------------------------------------------------------------------

/// Extracts a version string like "136.0.7103.113" from arbitrary text.
/// Looks for a pattern of digits separated by dots (at least X.Y.Z).
fn extract_version(raw: &str) -> Option<String> {
    // Scan for the first occurrence of a digit sequence that forms X.Y.Z or X.Y.Z.W
    let chars: Vec<char> = raw.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        // Find the start of a digit run that is either at the beginning
        // or preceded by a non-alphanumeric character (word boundary).
        if chars[i].is_ascii_digit() {
            let at_boundary = i == 0 || !chars[i - 1].is_ascii_alphanumeric();
            if at_boundary {
                // Try to parse X.Y.Z(.W)? starting at position i
                if let Some(ver) = try_parse_version(&chars, i) {
                    return Some(ver);
                }
            }
        }
        i += 1;
    }
    None
}

/// Attempts to parse a version like X.Y.Z or X.Y.Z.W starting at `start` in `chars`.
/// Returns `Some(version_string)` if at least three dot-separated digit groups are found.
fn try_parse_version(chars: &[char], start: usize) -> Option<String> {
    let len = chars.len();
    let mut parts: Vec<String> = Vec::new();
    let mut i = start;

    loop {
        // Consume a run of digits
        let digit_start = i;
        while i < len && chars[i].is_ascii_digit() {
            i += 1;
        }
        if i == digit_start {
            // No digits found — stop
            break;
        }
        let part: String = chars[digit_start..i].iter().collect();
        parts.push(part);

        // After digits, we expect a dot to continue; otherwise stop
        if i < len && chars[i] == '.' {
            // Peek ahead: the dot must be followed by a digit to be part of the version
            if i + 1 < len && chars[i + 1].is_ascii_digit() {
                i += 1; // skip the dot
            } else {
                break;
            }
        } else {
            break;
        }

        // Cap at 4 parts (X.Y.Z.W)
        if parts.len() == 4 {
            break;
        }
    }

    // Check trailing boundary: the character after the version must not be
    // alphanumeric (or we must be at end-of-string).
    if i < len && chars[i].is_ascii_alphanumeric() {
        return None;
    }

    if parts.len() >= 3 {
        Some(parts.join("."))
    } else {
        None
    }
}

/// Returns the major version number from a version string like "136.0.7103.113" → 136.
fn major_version(version: &str) -> Option<u32> {
    version.split('.').next()?.parse::<u32>().ok()
}

// ---------------------------------------------------------------------------
// Browser discovery
// ---------------------------------------------------------------------------

/// Information about an installed browser binary (Chrome / Edge).
///
/// Returned by [`detect_browsers`] and consumed by the `setup` command.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserInfo {
    pub browser: Browser,
    pub path: String,
    pub version: Option<String>,
    pub major_version: Option<u32>,
    pub installed: bool,
}

/// Detects the version of a browser binary.
///
/// On Windows, reads the file's embedded product version via PowerShell
/// because `browser.exe --version` is unreliable. On other platforms,
/// runs the binary with `--version` and parses the output.
pub fn detect_browser_version(binary_path: &str) -> Option<String> {
    #[cfg(windows)]
    {
        if Path::new(binary_path).exists() {
            let ps_cmd = format!(
                "(Get-Item '{}').VersionInfo.ProductVersion",
                binary_path.replace('\'', "''")
            );
            let output = std::process::Command::new("powershell")
                .args(["-NoProfile", "-NoLogo", "-Command", &ps_cmd])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output()
                .ok()?;

            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                if let Some(ver) = extract_version(stdout.trim()) {
                    return Some(ver);
                }
            }
        }
    }

    #[cfg(not(windows))]
    {
        let output = std::process::Command::new(binary_path)
            .arg("--version")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .ok()?;

        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            return extract_version(&stdout);
        }
    }

    None
}

/// Checks whether a browser binary exists at the given path.
///
/// First checks if the path exists on disk; on Unix also tries `which`,
/// on Windows tries `where`.
pub async fn browser_exists(binary_path: &str) -> bool {
    if Path::new(binary_path).exists() {
        return true;
    }

    // Fall back to which / where for bare command names
    #[cfg(unix)]
    {
        Command::new("which")
            .arg(binary_path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .map(|s| s.success())
            .unwrap_or(false)
    }

    #[cfg(windows)]
    {
        Command::new("where")
            .arg(binary_path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .map(|s| s.success())
            .unwrap_or(false)
    }

    #[cfg(not(any(unix, windows)))]
    {
        false
    }
}

/// Discovers installed Chrome and Edge browsers on the current system.
pub async fn detect_browsers() -> Vec<BrowserInfo> {
    let browsers = [Browser::Chrome, Browser::Edge];
    let mut results = Vec::new();

    for &browser in &browsers {
        let path = crate::types::default_browser_binary(browser);
        let installed = browser_exists(&path).await;

        let (version, maj) = if installed {
            let ver = detect_browser_version(&path);
            let maj = ver.as_deref().and_then(major_version);
            (ver, maj)
        } else {
            (None, None)
        };

        results.push(BrowserInfo {
            browser,
            path,
            version,
            major_version: maj,
            installed,
        });
    }

    results
}

// ---------------------------------------------------------------------------
// Driver discovery
// ---------------------------------------------------------------------------

/// Information about an installed WebDriver binary (chromedriver / msedgedriver).
///
/// Returned by [`detect_driver`] and consumed by the `setup` command to
/// populate [`crate::types::SetupDriver`] in `.browsectl/setup.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriverInfo {
    pub browser: Browser,
    pub path: String,
    pub version: Option<String>,
    pub exists: bool,
}

/// Runs the driver binary with `--version` and extracts the version string.
pub async fn detect_driver_version(driver_path: &str) -> Option<String> {
    let output = Command::new(driver_path)
        .arg("--version")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    extract_version(&stdout)
}

/// Detects an existing driver for the given browser at its default path.
pub async fn detect_driver(browser: Browser) -> DriverInfo {
    let path = crate::types::default_driver_path(browser);
    let exists = Path::new(&path).exists();

    let version = if exists {
        detect_driver_version(&path).await
    } else {
        None
    };

    DriverInfo {
        browser,
        path,
        version,
        exists,
    }
}

// ---------------------------------------------------------------------------
// Driver download — helpers
// ---------------------------------------------------------------------------

/// Maps (os, arch) to the Chrome for Testing platform string.
fn cft_platform(os: &str, arch: &str) -> &'static str {
    match (os, arch) {
        ("macos", "arm64") => "mac-arm64",
        ("macos", _) => "mac-x64",
        ("linux", _) => "linux64",
        ("windows", _) => "win64",
        _ => "linux64",
    }
}

/// Maps (os, arch) to the Edge driver platform string.
fn edge_platform(os: &str, arch: &str) -> &'static str {
    match (os, arch) {
        ("macos", "arm64") => "mac64_m1",
        ("macos", _) => "mac64",
        ("linux", _) => "linux64",
        ("windows", _) => "win64",
        _ => "linux64",
    }
}

/// Recursively searches `dir` for a file named `filename`.
fn find_file_recursive(dir: &Path, filename: &str) -> Option<PathBuf> {
    if !dir.is_dir() {
        return None;
    }

    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() {
            if let Some(name) = path.file_name() {
                if name == filename {
                    return Some(path);
                }
            }
        } else if path.is_dir() {
            if let Some(found) = find_file_recursive(&path, filename) {
                return Some(found);
            }
        }
    }

    None
}

/// Extracts a zip archive to `extract_dir`, locates `binary_name` inside it,
/// copies it to `dest_dir`, makes it executable on Unix, and cleans up.
async fn extract_and_install(
    zip_path: &Path,
    extract_dir: &Path,
    dest_dir: &Path,
    binary_name: &str,
) -> Result<PathBuf> {
    // Create the extract directory
    tokio::fs::create_dir_all(extract_dir)
        .await
        .context("failed to create extract directory")?;

    // Run extraction command and record diagnostics, but only fail hard if the
    // expected driver binary is still missing afterward.
    let mut extract_error: Option<String> = None;

    #[cfg(unix)]
    {
        let output = Command::new("unzip")
            .arg("-o")
            .arg(zip_path.as_os_str())
            .arg("-d")
            .arg(extract_dir.as_os_str())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .context("failed to run unzip")?;

        if !output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            extract_error = Some(format!(
                "unzip exited with {:?}; stdout: {}; stderr: {}",
                output.status.code(),
                stdout.trim(),
                stderr.trim()
            ));
        }
    }

    #[cfg(windows)]
    {
        let output = Command::new("tar")
            .arg("-xf")
            .arg(zip_path.as_os_str())
            .arg("-C")
            .arg(extract_dir.as_os_str())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .context("failed to run tar")?;

        if !output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            extract_error = Some(format!(
                "tar exited with {:?}; stdout: {}; stderr: {}",
                output.status.code(),
                stdout.trim(),
                stderr.trim()
            ));
        }
    }

    // Find the binary in the extracted content (may be in a subdirectory)
    let found = match find_file_recursive(extract_dir, binary_name) {
        Some(path) => path,
        None => {
            if let Some(err) = extract_error {
                bail!("failed to extract driver archive and could not find {binary_name}: {err}");
            }
            bail!("could not find {binary_name} in extracted archive");
        }
    };

    if let Some(err) = extract_error.as_deref() {
        eprintln!(
            "warning: archive tool reported a non-zero exit status, but {} was found and will be installed ({})",
            binary_name, err
        );
    }

    // Copy to destination
    let dest_path = dest_dir.join(binary_name);
    tokio::fs::create_dir_all(dest_dir)
        .await
        .context("failed to create destination directory")?;

    tokio::fs::copy(&found, &dest_path).await.with_context(|| {
        format!(
            "failed to copy {} → {}",
            found.display(),
            dest_path.display()
        )
    })?;

    // chmod +x on unix
    #[cfg(unix)]
    {
        let status = Command::new("chmod")
            .arg("+x")
            .arg(dest_path.as_os_str())
            .status()
            .await
            .context("failed to chmod +x")?;

        if !status.success() {
            eprintln!("warning: chmod +x exited with non-zero status");
        }
    }

    // Clean up temp files
    let _ = tokio::fs::remove_dir_all(extract_dir).await;
    let _ = tokio::fs::remove_file(zip_path).await;

    Ok(dest_path)
}

// ---------------------------------------------------------------------------
// Download with progress
// ---------------------------------------------------------------------------

/// Downloads a URL with a progress indicator showing downloaded/total MB.
/// Prints progress to stderr so it doesn't interfere with stdout output.
async fn download_with_progress(client: &reqwest::Client, url: &str) -> Result<Vec<u8>> {
    let resp = client
        .get(url)
        .send()
        .await
        .context("failed to send download request")?;

    if !resp.status().is_success() {
        bail!("download returned HTTP {} (url: {})", resp.status(), url);
    }

    let total = resp.content_length();
    let mut stream = resp.bytes_stream();
    let mut downloaded: u64 = 0;
    let mut buf = Vec::new();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("error reading download stream")?;
        downloaded += chunk.len() as u64;
        buf.extend_from_slice(&chunk);

        let dl_mb = downloaded as f64 / 1_048_576.0;
        match total {
            Some(t) => {
                let total_mb = t as f64 / 1_048_576.0;
                eprint!("\r  {:.1} / {:.1} MB", dl_mb, total_mb);
            }
            None => {
                eprint!("\r  {:.1} MB", dl_mb);
            }
        }
    }
    eprintln!(); // finish the progress line

    Ok(buf)
}

// ---------------------------------------------------------------------------
// Driver download — Chrome
// ---------------------------------------------------------------------------

/// Downloads the matching chromedriver for the given Chrome version into `dest_dir`.
///
/// Uses the Chrome for Testing (CfT) JSON endpoint to resolve the download URL.
pub async fn download_chromedriver(chrome_version: &str, dest_dir: &Path) -> Result<PathBuf> {
    let platform = detect_platform();
    let cft_plat = cft_platform(&platform.os, &platform.arch);

    let maj = major_version(chrome_version)
        .ok_or_else(|| anyhow::anyhow!("cannot parse major version from {chrome_version}"))?;
    let major_str = maj.to_string();

    // Fetch the CfT versions JSON
    let url = "https://googlechromelabs.github.io/chrome-for-testing/latest-versions-per-milestone-with-downloads.json";
    let client = reqwest::Client::new();
    let resp = client
        .get(url)
        .send()
        .await
        .context("failed to fetch CfT versions JSON")?;

    if !resp.status().is_success() {
        bail!("CfT versions endpoint returned HTTP {}", resp.status());
    }

    let data: serde_json::Value = resp
        .json()
        .await
        .context("failed to parse CfT versions JSON")?;

    // Navigate: milestones → <major> → version
    let milestone = &data["milestones"][&major_str];
    if milestone.is_null() {
        bail!("no CfT data found for Chrome milestone {major_str}");
    }

    let resolved_version = milestone["version"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing version in CfT milestone {major_str}"))?;

    // Find the chromedriver download for our platform
    let downloads = milestone["downloads"]["chromedriver"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("no chromedriver downloads for milestone {major_str}"))?;

    let download_url = downloads
        .iter()
        .find(|entry| entry["platform"].as_str() == Some(cft_plat))
        .and_then(|entry| entry["url"].as_str())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "no chromedriver download for platform {cft_plat} in milestone {major_str}"
            )
        })?;

    eprintln!(
        "Downloading chromedriver {} for {} …",
        resolved_version, cft_plat
    );

    // Download the zip with progress
    let zip_bytes = download_with_progress(&client, download_url)
        .await
        .context("failed to download chromedriver zip")?;

    let zip_path = dest_dir.join(".chromedriver-download.zip");
    tokio::fs::create_dir_all(dest_dir)
        .await
        .context("failed to create destination directory")?;
    tokio::fs::write(&zip_path, &zip_bytes)
        .await
        .context("failed to write chromedriver zip")?;

    let extract_dir = dest_dir.join(".driver-extract-tmp");

    let binary_name = if cfg!(target_os = "windows") {
        "chromedriver.exe"
    } else {
        "chromedriver"
    };

    let dest_path = extract_and_install(&zip_path, &extract_dir, dest_dir, binary_name)
        .await
        .context("failed to extract and install chromedriver")?;

    eprintln!(
        "chromedriver {} installed → {}",
        resolved_version,
        dest_path.display()
    );

    Ok(dest_path)
}

// ---------------------------------------------------------------------------
// Driver download — Edge
// ---------------------------------------------------------------------------

/// Downloads the matching msedgedriver for the given Edge version into `dest_dir`.
pub async fn download_edgedriver(edge_version: &str, dest_dir: &Path) -> Result<PathBuf> {
    let platform = detect_platform();
    let edge_plat = edge_platform(&platform.os, &platform.arch);

    let download_url = format!(
        "https://msedgedriver.microsoft.com/{}/edgedriver_{}.zip",
        edge_version, edge_plat
    );

    eprintln!(
        "Downloading msedgedriver {} for {} …",
        edge_version, edge_plat
    );

    let client = reqwest::Client::new();
    let zip_bytes = download_with_progress(&client, &download_url)
        .await
        .context("failed to download edgedriver zip")?;

    let zip_path = dest_dir.join(".edgedriver-download.zip");
    tokio::fs::create_dir_all(dest_dir)
        .await
        .context("failed to create destination directory")?;
    tokio::fs::write(&zip_path, &zip_bytes)
        .await
        .context("failed to write edgedriver zip")?;

    let extract_dir = dest_dir.join(".driver-extract-tmp");

    let binary_name = if cfg!(target_os = "windows") {
        "msedgedriver.exe"
    } else {
        "msedgedriver"
    };

    let dest_path = extract_and_install(&zip_path, &extract_dir, dest_dir, binary_name)
        .await
        .context("failed to extract and install msedgedriver")?;

    eprintln!(
        "msedgedriver {} installed → {}",
        edge_version,
        dest_path.display()
    );

    Ok(dest_path)
}

// ---------------------------------------------------------------------------
// Driver download — unified entry point
// ---------------------------------------------------------------------------

/// Downloads the appropriate WebDriver binary for the given browser and version.
pub async fn download_driver(
    browser: Browser,
    browser_version: &str,
    dest_dir: &Path,
) -> Result<PathBuf> {
    match browser {
        Browser::Chrome => download_chromedriver(browser_version, dest_dir).await,
        Browser::Edge => download_edgedriver(browser_version, dest_dir).await,
    }
}

// ---------------------------------------------------------------------------
// ensure_driver — high-level "make sure a driver is available"
// ---------------------------------------------------------------------------

/// Ensures a WebDriver binary is available at `driver_path`, downloading it
/// if necessary.
///
/// Returns the path to the usable driver binary.
///
/// # Logic
///
/// - If the driver already exists at `driver_path`:
///   - If `browser_version` is `Some`, compare major versions. If they match,
///     return the existing path. If they differ, download a fresh copy.
///   - If `browser_version` is `None`, return the existing path as-is.
/// - If the driver does not exist:
///   - If `browser_version` is `Some`, download it into the parent directory
///     of `driver_path`.
///   - If `browser_version` is `None`, bail with a helpful error message.
pub async fn ensure_driver(
    browser: Browser,
    browser_version: Option<&str>,
    driver_path: &str,
) -> Result<String> {
    let driver_exists = Path::new(driver_path).exists();

    if driver_exists {
        match browser_version {
            Some(bver) => {
                // Check major version match
                let driver_ver = detect_driver_version(driver_path).await;
                let driver_major = driver_ver.as_deref().and_then(major_version);
                let browser_major = major_version(bver);

                if driver_major.is_some() && driver_major == browser_major {
                    eprintln!(
                        "Driver at {} matches browser major version ({}), reusing.",
                        driver_path,
                        driver_major.unwrap()
                    );
                    return Ok(driver_path.to_string());
                }

                eprintln!(
                    "Driver version mismatch (driver: {:?}, browser: {:?}). Downloading fresh copy …",
                    driver_ver, bver
                );

                let dest_dir = Path::new(driver_path)
                    .parent()
                    .unwrap_or_else(|| Path::new("."));
                let installed = download_driver(browser, bver, dest_dir).await?;
                Ok(installed.to_string_lossy().into_owned())
            }
            None => {
                // No browser version to compare — just use what we have
                Ok(driver_path.to_string())
            }
        }
    } else {
        match browser_version {
            Some(bver) => {
                let dest_dir = Path::new(driver_path)
                    .parent()
                    .unwrap_or_else(|| Path::new("."));
                let installed = download_driver(browser, bver, dest_dir).await?;
                Ok(installed.to_string_lossy().into_owned())
            }
            None => {
                bail!(
                    "No {} driver found at {:?} and no browser version was provided \
                     to auto-download one. Please either:\n\
                     • Install the driver manually, or\n\
                     • Ensure the browser is installed so the version can be detected.",
                    browser,
                    driver_path
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_version_chrome() {
        let raw = "Google Chrome 136.0.7103.113 ";
        assert_eq!(extract_version(raw), Some("136.0.7103.113".to_string()));
    }

    #[test]
    fn test_extract_version_chromedriver() {
        let raw = "ChromeDriver 136.0.7103.113 (abc123-refs/branch-heads/...)";
        assert_eq!(extract_version(raw), Some("136.0.7103.113".to_string()));
    }

    #[test]
    fn test_extract_version_edge() {
        let raw = "Microsoft Edge 124.0.2478.97 ";
        assert_eq!(extract_version(raw), Some("124.0.2478.97".to_string()));
    }

    #[test]
    fn test_extract_version_three_part() {
        let raw = "SomeTool 12.34.56";
        assert_eq!(extract_version(raw), Some("12.34.56".to_string()));
    }

    #[test]
    fn test_extract_version_none() {
        assert_eq!(extract_version("no version here"), None);
    }

    #[test]
    fn test_major_version() {
        assert_eq!(major_version("136.0.7103.113"), Some(136));
        assert_eq!(major_version("12.34.56"), Some(12));
        assert_eq!(major_version("abc"), None);
    }

    #[test]
    fn test_detect_platform_smoke() {
        let p = detect_platform();
        assert!(!p.os.is_empty());
        assert!(!p.arch.is_empty());
        assert!(!p.display.is_empty());
    }

    #[test]
    fn test_cft_platform_mappings() {
        assert_eq!(cft_platform("macos", "arm64"), "mac-arm64");
        assert_eq!(cft_platform("macos", "x64"), "mac-x64");
        assert_eq!(cft_platform("linux", "x64"), "linux64");
        assert_eq!(cft_platform("windows", "x64"), "win64");
    }

    #[test]
    fn test_edge_platform_mappings() {
        assert_eq!(edge_platform("macos", "arm64"), "mac64_m1");
        assert_eq!(edge_platform("macos", "x64"), "mac64");
        assert_eq!(edge_platform("linux", "x64"), "linux64");
        assert_eq!(edge_platform("windows", "x64"), "win64");
    }
}
