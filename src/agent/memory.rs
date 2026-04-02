//! Agent memory — persistence layer for cross-session agent state.
//!
//! Stores page visit history, successful task patterns, and user preferences
//! in `.browsectl/memory.json` so the agent can recall previous interactions
//! and learned behaviours across browser sessions.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

use crate::types::now_iso;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of page visits to keep in memory.
const MAX_PAGE_VISITS: usize = 200;

/// Maximum number of task patterns to keep in memory.
const MAX_TASK_PATTERNS: usize = 100;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Top-level store for all agent memory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMemory {
    /// Recent page visit records.
    pub page_visits: Vec<PageVisit>,
    /// Successful task execution patterns.
    pub task_patterns: Vec<TaskPattern>,
    /// User preference signals.
    pub preferences: UserPreferences,
}

impl Default for AgentMemory {
    fn default() -> Self {
        Self {
            page_visits: Vec::new(),
            task_patterns: Vec::new(),
            preferences: UserPreferences::default(),
        }
    }
}

/// A single page visit record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageVisit {
    /// Full URL that was visited.
    pub url: String,
    /// Normalized URL pattern (e.g. `"example.com/product/*"`).
    pub url_pattern: String,
    /// Page title.
    pub title: String,
    /// Number of interactive slots found on the page.
    pub slot_count: usize,
    /// Actions that were performed during the visit.
    pub actions_taken: Vec<String>,
    /// ISO 8601 timestamp of when the page was visited.
    pub visited_at: String,
}

/// A reusable task pattern that has been observed to succeed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskPattern {
    /// The user's original intent (e.g. `"login"`, `"search"`).
    pub intent: String,
    /// URL pattern where this task works.
    pub url_pattern: String,
    /// The `CommandSpec` JSON objects that worked.
    pub commands: Vec<Value>,
    /// How many times this pattern has succeeded.
    pub success_count: u32,
    /// ISO 8601 timestamp of the last successful use.
    pub last_used: String,
}

/// User preference signals that influence agent behaviour.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserPreferences {
    /// Whether to always confirm submit-level actions.
    pub confirm_submit: bool,
    /// Auto-scroll to elements before interaction.
    pub auto_scroll: bool,
    /// Maximum number of task suggestions to show.
    pub max_suggestions: usize,
    /// Preferred language for suggestions (`"auto"` = detect).
    pub language: String,
}

impl Default for UserPreferences {
    fn default() -> Self {
        Self {
            confirm_submit: true,
            auto_scroll: true,
            max_suggestions: 5,
            language: "auto".to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// Path helpers
// ---------------------------------------------------------------------------

/// Returns the path to the memory JSON file (`~/.browsectl/memory.json`).
fn memory_path() -> PathBuf {
    match dirs::home_dir() {
        Some(home) => home.join(".browsectl/memory.json"),
        None => PathBuf::from(".browsectl/memory.json"),
    }
}

// ---------------------------------------------------------------------------
// Read / Write
// ---------------------------------------------------------------------------

/// Read the agent memory from disk.
///
/// Returns a default (empty) [`AgentMemory`] if the file does not exist or
/// cannot be parsed.
pub async fn read_memory() -> Result<AgentMemory> {
    let path = memory_path();
    if !path.exists() {
        return Ok(AgentMemory::default());
    }
    let raw = tokio::fs::read_to_string(&path)
        .await
        .context("reading memory.json")?;
    match serde_json::from_str::<AgentMemory>(&raw) {
        Ok(memory) => Ok(memory),
        Err(e) => {
            eprintln!(
                "warning: failed to parse {}: {e} — starting with empty memory",
                path.display()
            );
            Ok(AgentMemory::default())
        }
    }
}

/// Persist the agent memory to disk.
pub async fn write_memory(memory: &AgentMemory) -> Result<()> {
    let path = memory_path();
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let raw = serde_json::to_string_pretty(memory)?;
    tokio::fs::write(path, format!("{raw}\n"))
        .await
        .context("writing memory.json")?;
    Ok(())
}

// ---------------------------------------------------------------------------
// High-level operations
// ---------------------------------------------------------------------------

/// Record a page visit in agent memory.
///
/// Keeps at most [`MAX_PAGE_VISITS`] entries, dropping the oldest when the
/// limit is exceeded.
pub async fn record_visit(
    url: &str,
    title: &str,
    slot_count: usize,
    actions: Vec<String>,
) -> Result<()> {
    let mut memory = read_memory().await?;

    let visit = PageVisit {
        url: url.to_string(),
        url_pattern: normalize_url(url),
        title: title.to_string(),
        slot_count,
        actions_taken: actions,
        visited_at: now_iso(),
    };

    memory.page_visits.push(visit);

    // Trim to the most recent MAX_PAGE_VISITS entries.
    if memory.page_visits.len() > MAX_PAGE_VISITS {
        let excess = memory.page_visits.len() - MAX_PAGE_VISITS;
        memory.page_visits.drain(..excess);
    }

    write_memory(&memory).await
}

/// Record a successful task pattern (or update an existing one).
///
/// If a pattern with the same `intent` and `url_pattern` already exists its
/// `success_count` is incremented and `last_used` is refreshed.  Otherwise a
/// new entry is created.  At most [`MAX_TASK_PATTERNS`] patterns are kept;
/// when the limit is exceeded the least-recently-used pattern is evicted.
pub async fn record_task(intent: &str, url: &str, commands: Vec<Value>) -> Result<()> {
    let mut memory = read_memory().await?;
    let pattern = normalize_url(url);
    let now = now_iso();

    // Look for an existing pattern with the same intent + url_pattern.
    let existing = memory
        .task_patterns
        .iter_mut()
        .find(|p| p.intent == intent && p.url_pattern == pattern);

    if let Some(tp) = existing {
        tp.success_count += 1;
        tp.last_used = now;
        tp.commands = commands;
    } else {
        let tp = TaskPattern {
            intent: intent.to_string(),
            url_pattern: pattern,
            commands,
            success_count: 1,
            last_used: now,
        };
        memory.task_patterns.push(tp);

        // Evict the least-recently-used pattern when over limit.
        if memory.task_patterns.len() > MAX_TASK_PATTERNS {
            // Find the index of the pattern with the oldest `last_used`.
            if let Some((idx, _)) = memory
                .task_patterns
                .iter()
                .enumerate()
                .min_by(|(_, a), (_, b)| a.last_used.cmp(&b.last_used))
            {
                memory.task_patterns.remove(idx);
            }
        }
    }

    write_memory(&memory).await
}

/// Find all task patterns whose `url_pattern` matches the given URL.
///
/// Results are sorted by `success_count` descending (most successful first).
pub async fn find_patterns(url: &str) -> Result<Vec<TaskPattern>> {
    let memory = read_memory().await?;
    let pattern = normalize_url(url);

    let mut matches: Vec<TaskPattern> = memory
        .task_patterns
        .into_iter()
        .filter(|tp| tp.url_pattern == pattern)
        .collect();

    matches.sort_by(|a, b| b.success_count.cmp(&a.success_count));
    Ok(matches)
}

// ---------------------------------------------------------------------------
// URL normalization
// ---------------------------------------------------------------------------

/// Simple URL normalization that produces a pattern string suitable for
/// matching across visits.
///
/// 1. Remove protocol (`http://`, `https://`).
/// 2. Remove query string and fragment.
/// 3. Replace path segments that look like IDs (numeric, UUID-like) with `"*"`.
///
/// # Examples
///
/// ```text
/// "https://example.com/product/12345?ref=abc" → "example.com/product/*"
/// ```
fn normalize_url(url: &str) -> String {
    // 1. Strip protocol.
    let without_proto = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);

    // 2. Strip query string and fragment.
    let without_qs = without_proto.split('?').next().unwrap_or(without_proto);
    let without_frag = without_qs.split('#').next().unwrap_or(without_qs);

    // 3. Split into host and path, then normalise path segments.
    let trimmed = without_frag.trim_end_matches('/');

    if let Some(slash_idx) = trimmed.find('/') {
        let (host, path) = trimmed.split_at(slash_idx);
        let normalised_segments: Vec<&str> = path
            .split('/')
            .filter(|s| !s.is_empty())
            .map(|seg| if looks_like_id(seg) { "*" } else { seg })
            .collect();

        if normalised_segments.is_empty() {
            host.to_string()
        } else {
            format!("{}/{}", host, normalised_segments.join("/"))
        }
    } else {
        // No path — just a bare host.
        trimmed.to_string()
    }
}

/// Returns `true` if a path segment looks like an opaque identifier that
/// should be replaced with `"*"` during normalisation.
///
/// Matches:
/// - Pure numeric strings (`"12345"`)
/// - UUID-like strings (`"550e8400-e29b-41d4-a716-446655440000"`)
/// - Hex strings of 8+ characters (`"a3f9b2c1d4"`)
fn looks_like_id(segment: &str) -> bool {
    if segment.is_empty() {
        return false;
    }

    // Pure numeric.
    if segment.chars().all(|c| c.is_ascii_digit()) {
        return true;
    }

    // UUID-like: 8-4-4-4-12 hex pattern.
    if segment.len() == 36 {
        let parts: Vec<&str> = segment.split('-').collect();
        if parts.len() == 5
            && parts[0].len() == 8
            && parts[1].len() == 4
            && parts[2].len() == 4
            && parts[3].len() == 4
            && parts[4].len() == 12
            && parts
                .iter()
                .all(|p| p.chars().all(|c| c.is_ascii_hexdigit()))
        {
            return true;
        }
    }

    // Long hex string (≥8 chars, all hex digits).
    if segment.len() >= 8 && segment.chars().all(|c| c.is_ascii_hexdigit()) {
        return true;
    }

    false
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_url_basic() {
        assert_eq!(
            normalize_url("https://example.com/product/12345?ref=abc"),
            "example.com/product/*"
        );
    }

    #[test]
    fn test_normalize_url_uuid() {
        assert_eq!(
            normalize_url("https://app.io/users/550e8400-e29b-41d4-a716-446655440000/profile"),
            "app.io/users/*/profile"
        );
    }

    #[test]
    fn test_normalize_url_no_path() {
        assert_eq!(normalize_url("https://example.com"), "example.com");
    }

    #[test]
    fn test_normalize_url_no_protocol() {
        assert_eq!(normalize_url("example.com/page/42"), "example.com/page/*");
    }

    #[test]
    fn test_normalize_url_fragment() {
        assert_eq!(
            normalize_url("https://example.com/docs#section"),
            "example.com/docs"
        );
    }

    #[test]
    fn test_normalize_url_hex_id() {
        assert_eq!(
            normalize_url("https://cdn.example.com/assets/a3f9b2c1d4/style.css"),
            "cdn.example.com/assets/*/style.css"
        );
    }

    #[test]
    fn test_normalize_url_preserves_named_segments() {
        assert_eq!(
            normalize_url("https://example.com/blog/my-cool-post"),
            "example.com/blog/my-cool-post"
        );
    }

    #[test]
    fn test_looks_like_id_numeric() {
        assert!(looks_like_id("12345"));
        assert!(looks_like_id("0"));
    }

    #[test]
    fn test_looks_like_id_uuid() {
        assert!(looks_like_id("550e8400-e29b-41d4-a716-446655440000"));
    }

    #[test]
    fn test_looks_like_id_short_hex() {
        // 6 hex chars should NOT match (< 8).
        assert!(!looks_like_id("a3f9b2"));
    }

    #[test]
    fn test_looks_like_id_long_hex() {
        assert!(looks_like_id("a3f9b2c1d4"));
    }

    #[test]
    fn test_looks_like_id_word() {
        assert!(!looks_like_id("product"));
        assert!(!looks_like_id("my-cool-post"));
    }

    #[test]
    fn test_default_preferences() {
        let prefs = UserPreferences::default();
        assert!(prefs.confirm_submit);
        assert!(prefs.auto_scroll);
        assert_eq!(prefs.max_suggestions, 5);
        assert_eq!(prefs.language, "auto");
    }

    #[test]
    fn test_default_agent_memory() {
        let mem = AgentMemory::default();
        assert!(mem.page_visits.is_empty());
        assert!(mem.task_patterns.is_empty());
        assert!(mem.preferences.confirm_submit);
    }
}
