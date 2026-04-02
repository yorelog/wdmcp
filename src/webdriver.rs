//! webdriver.rs — lightweight W3C WebDriver client built on reqwest.
//!
//! Supports both **creating** new browser sessions and **attaching** to
//!
//! `WdClient` is `Clone + Send + Sync` and does **not** close the session on
//! drop — callers must explicitly call `delete_session()` when they want to
//! end a session.

use anyhow::{Context, Result, bail};
use base64::Engine;
use reqwest::Client;
use serde_json::{Value, json};

// ───────────────────────────────────────────────────────────────────────────
// WdClient
// ───────────────────────────────────────────────────────────────────────────

/// A thin, `Clone`-able WebDriver client.
///
/// Internally wraps a `reqwest::Client` (connection-pooled, cheap to clone)
/// together with the server base URL and the session id.
#[derive(Clone, Debug)]
pub struct WdClient {
    http: Client,
    base_url: String,
    session_id: String,
}

impl WdClient {
    // ── constructors ───────────────────────────────────────────────────

    /// Attach to an **existing** WebDriver session.
    ///
    /// This is instant — no HTTP request is made. Call [`WdClient::windows`]
    /// or similar to validate liveness.
    pub fn attach(base_url: &str, session_id: &str) -> Self {
        Self {
            http: Client::new(),
            base_url: base_url.trim_end_matches('/').to_string(),
            session_id: session_id.to_string(),
        }
    }

    /// Create a **new** WebDriver session via `POST /session`.
    ///
    /// Returns the client **and** the capabilities object returned by the
    /// server.
    pub async fn create_session(base_url: &str, capabilities: Value) -> Result<(Self, Value)> {
        let http = Client::new();
        let url = format!("{}/session", base_url.trim_end_matches('/'));
        let body = json!({
            "capabilities": {
                "alwaysMatch": capabilities
            }
        });

        let resp = http
            .post(&url)
            .json(&body)
            .send()
            .await
            .context("POST /session: request failed")?;

        let success = resp.status().is_success();
        let data: Value = resp
            .json()
            .await
            .context("POST /session: failed to parse response")?;

        if !success {
            let error = data["value"]["error"].as_str().unwrap_or("unknown error");
            let message = data["value"]["message"]
                .as_str()
                .unwrap_or("session creation failed");
            bail!("{error}: {message}");
        }

        let session_id = data["value"]["sessionId"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("no sessionId in /session response"))?
            .to_string();

        let caps = data["value"]["capabilities"].clone();

        Ok((
            Self {
                http,
                base_url: base_url.trim_end_matches('/').to_string(),
                session_id,
            },
            caps,
        ))
    }

    // ── accessors ──────────────────────────────────────────────────────

    #[allow(dead_code)]
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    #[allow(dead_code)]
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    // ── internal HTTP helpers ──────────────────────────────────────────

    fn session_url(&self) -> String {
        format!("{}/session/{}", self.base_url, self.session_id)
    }

    async fn parse_response(&self, resp: reqwest::Response) -> Result<Value> {
        let success = resp.status().is_success();
        let data: Value = resp.json().await?;
        if !success {
            let error = data["value"]["error"].as_str().unwrap_or("unknown error");
            let message = data["value"]["message"].as_str().unwrap_or("");
            bail!("{error}: {message}");
        }
        Ok(data.get("value").cloned().unwrap_or(Value::Null))
    }

    async fn get_value(&self, path: &str) -> Result<Value> {
        let url = format!("{}{}", self.session_url(), path);
        let resp = self.http.get(&url).send().await?;
        self.parse_response(resp).await
    }

    async fn post_value(&self, path: &str, body: &Value) -> Result<Value> {
        let url = format!("{}{}", self.session_url(), path);
        let resp = self.http.post(&url).json(body).send().await?;
        self.parse_response(resp).await
    }

    async fn delete_value(&self, path: &str) -> Result<Value> {
        let url = format!("{}{}", self.session_url(), path);
        let resp = self.http.delete(&url).send().await?;
        // Tolerate errors on delete (session may already be gone).
        let data: Value = resp.json().await.unwrap_or(Value::Null);
        Ok(data.get("value").cloned().unwrap_or(Value::Null))
    }

    // ── navigation ─────────────────────────────────────────────────────

    pub async fn goto(&self, url: &str) -> Result<()> {
        self.post_value("/url", &json!({ "url": url })).await?;
        Ok(())
    }

    #[allow(dead_code)]
    pub async fn current_url(&self) -> Result<String> {
        let v = self.get_value("/url").await?;
        Ok(v.as_str().unwrap_or("").to_string())
    }

    pub async fn title(&self) -> Result<String> {
        let v = self.get_value("/title").await?;
        Ok(v.as_str().unwrap_or("").to_string())
    }

    // ── window / tab management ────────────────────────────────────────

    /// Current window handle.
    pub async fn window(&self) -> Result<String> {
        let v = self.get_value("/window").await?;
        Ok(v.as_str().unwrap_or("").to_string())
    }

    /// All window handles.
    pub async fn windows(&self) -> Result<Vec<String>> {
        let v = self.get_value("/window/handles").await?;
        Ok(v.as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|h| h.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default())
    }

    pub async fn switch_to_window(&self, handle: &str) -> Result<()> {
        self.post_value("/window", &json!({ "handle": handle }))
            .await?;
        Ok(())
    }

    /// Open a new window / tab.  `type_hint` is `"tab"` or `"window"`.
    #[allow(dead_code)]
    pub async fn new_window(&self, type_hint: &str) -> Result<String> {
        let v = self
            .post_value("/window/new", &json!({ "type": type_hint }))
            .await?;
        Ok(v.get("handle")
            .and_then(|h| h.as_str())
            .unwrap_or("")
            .to_string())
    }

    pub async fn close_window(&self) -> Result<()> {
        self.delete_value("/window").await?;
        Ok(())
    }

    // ── elements ───────────────────────────────────────────────────────

    pub async fn find_css(&self, selector: &str) -> Result<WdElement> {
        let v = self
            .post_value(
                "/element",
                &json!({
                    "using": "css selector",
                    "value": selector
                }),
            )
            .await?;

        let element_id = extract_element_id(&v)?;
        Ok(WdElement {
            client: self.clone(),
            element_id,
        })
    }

    /// Find the first descendant element matching `selector` within the
    /// first element matched by `scope_selector`.
    pub async fn find_css_scoped(&self, scope_selector: &str, selector: &str) -> Result<WdElement> {
        let script = r#"
const scopeSelector = arguments[0];
const selector = arguments[1];
const scope = document.querySelector(scopeSelector);
if (!scope) return { __scopeMissing: true };
return scope.querySelector(selector);
"#;

        let v = self
            .execute(script, vec![json!(scope_selector), json!(selector)])
            .await?;

        if v.get("__scopeMissing").and_then(|x| x.as_bool()) == Some(true) {
            bail!("scope element not found: {scope_selector}");
        }
        if v.is_null() {
            bail!("no element matched selector in scope: {scope_selector} >> {selector}");
        }

        let element_id = extract_element_id(&v)?;
        Ok(WdElement {
            client: self.clone(),
            element_id,
        })
    }

    /// Find the first element matching `css` whose textContent matches
    /// JavaScript regex `/pattern/flags`.
    pub async fn find_css_with_text_regex(
        &self,
        css: &str,
        pattern: &str,
        flags: &str,
    ) -> Result<WdElement> {
        let script = r#"
const css = arguments[0];
const pattern = arguments[1];
const flags = arguments[2] || '';

let re;
try {
  re = new RegExp(pattern, flags);
} catch (e) {
  throw new Error(`invalid regex /${pattern}/${flags}: ${e.message}`);
}

const nodes = Array.from(document.querySelectorAll(css));
for (const el of nodes) {
  const text = (el.textContent || '').trim();
  if (re.test(text)) {
    return el;
  }
}
return null;
"#;

        let v = self
            .execute(script, vec![json!(css), json!(pattern), json!(flags)])
            .await?;

        if v.is_null() {
            bail!("no element matched selector with text regex: {css}::text(/{pattern}/{flags})");
        }

        let element_id = extract_element_id(&v)?;
        Ok(WdElement {
            client: self.clone(),
            element_id,
        })
    }

    /// Find the first scoped element matching `css` whose textContent matches
    /// JavaScript regex `/pattern/flags`.
    pub async fn find_css_with_text_regex_scoped(
        &self,
        scope_selector: &str,
        css: &str,
        pattern: &str,
        flags: &str,
    ) -> Result<WdElement> {
        let script = r#"
const scopeSelector = arguments[0];
const css = arguments[1];
const pattern = arguments[2];
const flags = arguments[3] || '';

const scope = document.querySelector(scopeSelector);
if (!scope) return { __scopeMissing: true };

let re;
try {
  re = new RegExp(pattern, flags);
} catch (e) {
  throw new Error(`invalid regex /${pattern}/${flags}: ${e.message}`);
}

const nodes = Array.from(scope.querySelectorAll(css));
for (const el of nodes) {
  const text = (el.textContent || '').trim();
  if (re.test(text)) {
    return el;
  }
}
return null;
"#;

        let v = self
            .execute(
                script,
                vec![
                    json!(scope_selector),
                    json!(css),
                    json!(pattern),
                    json!(flags),
                ],
            )
            .await?;

        if v.get("__scopeMissing").and_then(|x| x.as_bool()) == Some(true) {
            bail!("scope element not found: {scope_selector}");
        }
        if v.is_null() {
            bail!(
                "no element matched selector with text regex in scope: {scope_selector} >> {css}::text(/{pattern}/{flags})"
            );
        }

        let element_id = extract_element_id(&v)?;
        Ok(WdElement {
            client: self.clone(),
            element_id,
        })
    }

    // ── script execution ───────────────────────────────────────────────

    /// Execute synchronous JavaScript.  Returns the script's return value.
    pub async fn execute(&self, script: &str, args: Vec<Value>) -> Result<Value> {
        self.post_value(
            "/execute/sync",
            &json!({
                "script": script,
                "args": args
            }),
        )
        .await
    }

    // ── Chrome DevTools Protocol (CDP) ────────────────────────────────

    /// Execute a CDP command via the Chrome-specific WebDriver extension.
    ///
    /// This uses the `/goog/cdp/execute` endpoint supported by chromedriver.
    /// For Edge, the equivalent `/ms/cdp/execute` endpoint is tried as a
    /// fallback.
    ///
    /// Returns the CDP result or an error if the command fails.
    pub async fn cdp_execute(&self, cmd: &str, params: Value) -> Result<Value> {
        let body = json!({ "cmd": cmd, "params": params });

        // Try Chrome endpoint first, then Edge endpoint.
        let chrome_url = format!("{}/goog/cdp/execute", self.session_url());
        let resp = self.http.post(&chrome_url).json(&body).send().await;

        match resp {
            Ok(r) if r.status().is_success() => {
                return self.parse_response(r).await;
            }
            Ok(r) => {
                // If Chrome endpoint returned an error, try Edge endpoint
                let error_data: Value = r.json().await.unwrap_or(Value::Null);
                let error_msg = error_data["value"]["message"].as_str().unwrap_or("");

                // If it's a "unknown command" error, try the Edge endpoint
                if error_msg.contains("unknown command") || error_msg.contains("unrecognized") {
                    let edge_url = format!("{}/ms/cdp/execute", self.session_url());
                    let resp2 = self.http.post(&edge_url).json(&body).send().await?;
                    return self.parse_response(resp2).await;
                }

                bail!("CDP command {cmd} failed: {}", error_data);
            }
            Err(e) => {
                // Network error on Chrome endpoint — try Edge
                let edge_url = format!("{}/ms/cdp/execute", self.session_url());
                match self.http.post(&edge_url).json(&body).send().await {
                    Ok(r2) => return self.parse_response(r2).await,
                    Err(_) => return Err(e.into()),
                }
            }
        }
    }

    /// Send a CDP command and ignore errors (best-effort).
    pub async fn cdp_execute_quiet(&self, cmd: &str, params: Value) -> Value {
        self.cdp_execute(cmd, params).await.unwrap_or(Value::Null)
    }

    // ── WebDriver logging ─────────────────────────────────────────────

    /// Retrieve available log types from the WebDriver server.
    pub async fn get_log_types(&self) -> Result<Vec<String>> {
        let val = self.get_value("/se/log/types").await?;
        Ok(val
            .as_array()
            .unwrap_or(&vec![])
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect())
    }

    /// Retrieve log entries of the specified type.
    ///
    /// Each call drains the buffer — entries are returned only once.
    pub async fn get_log(&self, log_type: &str) -> Result<Value> {
        self.post_value("/se/log", &json!({ "type": log_type }))
            .await
    }

    // ── session lifecycle ──────────────────────────────────────────────

    /// Explicitly delete (quit) the session.
    pub async fn delete_session(&self) -> Result<()> {
        let url = format!("{}/session/{}", self.base_url, self.session_id);
        // Best-effort — ignore network / already-gone errors.
        let _ = self.http.delete(&url).send().await;
        Ok(())
    }
}

// ───────────────────────────────────────────────────────────────────────────
// WdElement
// ───────────────────────────────────────────────────────────────────────────

/// A handle to a DOM element inside a WebDriver session.
pub struct WdElement {
    client: WdClient,
    element_id: String,
}

impl WdElement {
    /// Helper — builds the sub-path relative to the session URL.
    fn path(&self, suffix: &str) -> String {
        format!("/element/{}{}", self.element_id, suffix)
    }

    pub async fn click(&self) -> Result<()> {
        self.client
            .post_value(&self.path("/click"), &json!({}))
            .await?;
        Ok(())
    }

    /// Click via JavaScript — bypasses the native WebDriver hit-test so it
    /// works even when another element (overlay, tooltip, etc.) would
    /// intercept the click.
    pub async fn js_click(&self) -> Result<()> {
        let w3c_key = "element-6066-11e4-a52e-4f735466cecf";
        let arg = json!({ w3c_key: self.element_id });
        self.client
            .execute("arguments[0].click()", vec![arg])
            .await?;
        Ok(())
    }

    /// Click the **parent** element via JavaScript.
    pub async fn js_click_parent(&self) -> Result<()> {
        let w3c_key = "element-6066-11e4-a52e-4f735466cecf";
        let arg = json!({ w3c_key: self.element_id });
        self.client
            .execute(
                "var p = arguments[0].parentElement; if (p) { p.click(); return true; } return false;",
                vec![arg],
            )
            .await?;
        Ok(())
    }

    /// Click the **next sibling** element via JavaScript (useful when an
    /// overlay sibling intercepts clicks on the target).
    pub async fn js_click_sibling(&self) -> Result<()> {
        let w3c_key = "element-6066-11e4-a52e-4f735466cecf";
        let arg = json!({ w3c_key: self.element_id });
        self.client
            .execute(
                "var el = arguments[0]; \
                 var sib = el.nextElementSibling || el.previousElementSibling; \
                 if (sib) { sib.click(); return true; } return false;",
                vec![arg],
            )
            .await?;
        Ok(())
    }

    /// Smart JS click: try parent → next/prev sibling → self.
    /// Returns a string indicating which strategy worked:
    /// `"parent"`, `"sibling"`, or `"self"`.
    pub async fn js_click_smart(&self) -> Result<String> {
        let w3c_key = "element-6066-11e4-a52e-4f735466cecf";
        let arg = json!({ w3c_key: self.element_id });
        let result = self
            .client
            .execute(
                r#"
                var el = arguments[0];
                // 1) Try parent
                var parent = el.parentElement;
                if (parent && parent !== document.body && parent !== document.documentElement) {
                    parent.click();
                    return "parent";
                }
                // 2) Try next sibling, then previous sibling
                var sib = el.nextElementSibling || el.previousElementSibling;
                if (sib) {
                    sib.click();
                    return "sibling";
                }
                // 3) Fall back to self
                el.click();
                return "self";
                "#,
                vec![arg],
            )
            .await?;
        let strategy = result.as_str().unwrap_or("self").to_string();
        Ok(strategy)
    }

    /// Scroll the element into the visible area of the viewport.
    pub async fn scroll_into_view(&self) -> Result<()> {
        let w3c_key = "element-6066-11e4-a52e-4f735466cecf";
        let arg = json!({ w3c_key: self.element_id });
        self.client
            .execute(
                "arguments[0].scrollIntoView({block:'center',inline:'center'})",
                vec![arg],
            )
            .await?;
        Ok(())
    }

    /// Uses the JSONWP `/displayed` endpoint (supported by chromedriver).
    pub async fn is_displayed(&self) -> Result<bool> {
        let v = self.client.get_value(&self.path("/displayed")).await?;
        Ok(v.as_bool().unwrap_or(false))
    }

    /// Take an element screenshot and return the raw PNG bytes.
    pub async fn screenshot_as_png(&self) -> Result<Vec<u8>> {
        let v = self.client.get_value(&self.path("/screenshot")).await?;
        let b64 = v.as_str().unwrap_or("");
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .context("failed to decode element screenshot base64")?;
        Ok(bytes)
    }
}

// ───────────────────────────────────────────────────────────────────────────
// Helpers
// ───────────────────────────────────────────────────────────────────────────

/// Extract the element identifier from a W3C find-element response.
///
/// The spec uses the key `"element-6066-11e4-a52e-4f735466cecf"`, but some
/// drivers use legacy `"ELEMENT"`.  We simply grab the first string value.
fn extract_element_id(value: &Value) -> Result<String> {
    if let Some(obj) = value.as_object() {
        for val in obj.values() {
            if let Some(s) = val.as_str() {
                return Ok(s.to_string());
            }
        }
    }
    bail!("invalid element response: {value}")
}
