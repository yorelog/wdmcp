//! Low-level browser command implementations built on [`WdClient`](crate::webdriver::WdClient).
//!
//! Each command corresponds to a user-facing action (open, click, fill, paste,
//! scroll, screenshot, wait, etc.).  The public entry point is
//! [`execute_single_command`], which dispatches based on `CommandSpec.command_type`.
//!
//! **Selector syntax** — supports an extended CSS selector with optional text
//! regex matching: `CSS_SELECTOR::text(/regex/flags)`.  Parsed by
//! [`parse_selector`].

use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use tokio::time::sleep;

use crate::types::CommandSpec;
use crate::webdriver::{WdClient, WdElement};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct ParsedSelector {
    css: String,
    text_regex: Option<(String, String)>,
}

/// Custom selector syntax:
///   CSS::text(/pattern/flags)
/// Examples:
///   div.tab::text(/^视频$/)
///   .menu-item::text(/video/i)
fn parse_selector(selector: &str) -> Result<ParsedSelector> {
    const MARKER: &str = "::text(";

    let raw = selector.trim();
    if raw.is_empty() {
        bail!("selector cannot be empty");
    }

    // Plain CSS selector path.
    let Some(idx) = raw.rfind(MARKER) else {
        return Ok(ParsedSelector {
            css: raw.to_string(),
            text_regex: None,
        });
    };

    if !raw.ends_with(')') {
        bail!(
            "invalid selector syntax: expected ')' at end for {}",
            MARKER
        );
    }

    let css = raw[..idx].trim();
    if css.is_empty() {
        bail!("invalid selector syntax: CSS part is empty before {MARKER}");
    }

    let regex_expr = raw[idx + MARKER.len()..raw.len() - 1].trim();
    let (pattern, flags) = parse_js_regex_literal(regex_expr)?;

    Ok(ParsedSelector {
        css: css.to_string(),
        text_regex: Some((pattern, flags)),
    })
}

/// Parse a JavaScript regex literal body like `/video/i`.
fn parse_js_regex_literal(expr: &str) -> Result<(String, String)> {
    if !expr.starts_with('/') {
        bail!("invalid regex syntax: expected /pattern/flags");
    }

    let bytes = expr.as_bytes();
    let mut escaped = false;
    let mut close_idx: Option<usize> = None;

    for (i, b) in bytes.iter().enumerate().skip(1) {
        if escaped {
            escaped = false;
            continue;
        }
        match *b {
            b'\\' => escaped = true,
            b'/' => {
                close_idx = Some(i);
                break;
            }
            _ => {}
        }
    }

    let end = close_idx.ok_or_else(|| {
        anyhow::anyhow!("invalid regex syntax: missing closing '/' in /pattern/flags")
    })?;

    let pattern = &expr[1..end];
    if pattern.is_empty() {
        bail!("invalid regex syntax: pattern cannot be empty");
    }

    let flags = expr[end + 1..].trim();
    if !flags.chars().all(|c| c.is_ascii_alphabetic()) {
        bail!("invalid regex syntax: flags must be alphabetic");
    }

    Ok((pattern.to_string(), flags.to_string()))
}

/// Poll `driver.find_css(selector)` until a displayed element is found
/// or the timeout expires.
pub async fn wait_find(
    driver: &WdClient,
    selector: &str,
    scope: Option<&str>,
    timeout_ms: u64,
    interval_ms: u64,
) -> Result<WdElement> {
    let parsed =
        parse_selector(selector).with_context(|| format!("invalid selector syntax: {selector}"))?;

    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    let interval = Duration::from_millis(interval_ms);
    let scope = scope.map(str::trim).filter(|s| !s.is_empty());

    loop {
        let found = match parsed.text_regex.as_ref() {
            Some((pattern, flags)) => match scope {
                Some(scope_selector) => {
                    driver
                        .find_css_with_text_regex_scoped(
                            scope_selector,
                            &parsed.css,
                            pattern,
                            flags,
                        )
                        .await
                }
                None => {
                    driver
                        .find_css_with_text_regex(&parsed.css, pattern, flags)
                        .await
                }
            },
            None => match scope {
                Some(scope_selector) => driver.find_css_scoped(scope_selector, &parsed.css).await,
                None => driver.find_css(&parsed.css).await,
            },
        };

        match found {
            Ok(el) => {
                if el.is_displayed().await.unwrap_or(false) {
                    return Ok(el);
                }
            }
            Err(_) => {}
        }

        if Instant::now() >= deadline {
            bail!(
                "wait_find timed out after {}ms waiting for selector: {} (scope: {})",
                timeout_ms,
                selector,
                scope.unwrap_or("<document>")
            );
        }

        sleep(interval).await;
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_js_regex_literal, parse_selector};

    #[test]
    fn parse_plain_css_selector() {
        let parsed = parse_selector("div[data-testid=\"x\"]").expect("parse should succeed");
        assert_eq!(parsed.css, "div[data-testid=\"x\"]");
        assert!(parsed.text_regex.is_none());
    }

    #[test]
    fn parse_css_with_text_regex() {
        let parsed = parse_selector("div.item::text(/^视频$/i)").expect("parse should succeed");
        assert_eq!(parsed.css, "div.item");
        assert_eq!(
            parsed.text_regex,
            Some(("^视频$".to_string(), "i".to_string()))
        );
    }

    #[test]
    fn parse_js_regex_literal_with_escaped_slash() {
        let (pattern, flags) =
            parse_js_regex_literal(r"/foo\/bar/gi").expect("regex parse should succeed");
        assert_eq!(pattern, r"foo\/bar");
        assert_eq!(flags, "gi");
    }

    #[test]
    fn reject_invalid_regex_literal() {
        let err = parse_selector("div.item::text(foo)").expect_err("must fail");
        let msg = format!("{err:#}");
        assert!(msg.contains("expected /pattern/flags"));
    }
}

// ---------------------------------------------------------------------------
// fill_like
// ---------------------------------------------------------------------------

pub async fn fill_like(driver: &WdClient, selector: Option<&str>, text: &str) -> Result<()> {
    let js = r#"
const selector = arguments[0];
const content = arguments[1];
const target = selector ? document.querySelector(selector) : document.activeElement;
if (!target) throw new Error('target not found');
const emitInput = () => { target.dispatchEvent(new InputEvent('input', { bubbles: true })); };
if (target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement) {
  target.focus(); target.value = ''; emitInput(); target.value = content; emitInput();
} else {
  target.focus(); target.textContent = content; emitInput();
}
"#;

    let sel_arg: Value = selector.unwrap_or("").into();
    let text_arg: Value = text.into();

    driver
        .execute(js, vec![sel_arg, text_arg])
        .await
        .context("fill_like: JS execution failed")?;

    Ok(())
}

// ---------------------------------------------------------------------------
// paste_like
// ---------------------------------------------------------------------------

pub async fn paste_like(driver: &WdClient, selector: Option<&str>, text: &str) -> Result<()> {
    let js = r#"
const selector = arguments[0];
const content = arguments[1];
const target = selector ? document.querySelector(selector) : document.activeElement;
if (!target) throw new Error('target not found');
target.focus && target.focus();
try {
  const dt = new DataTransfer(); dt.setData('text/plain', content);
  const pe = new ClipboardEvent('paste', { bubbles: true, cancelable: true, clipboardData: dt });
  const prevented = target.dispatchEvent(pe) === false || pe.defaultPrevented;
  if (prevented) return;
} catch(e) {}
const emitInput = () => { target.dispatchEvent(new InputEvent('input', { bubbles: true, inputType: 'insertFromPaste', data: content })); };
if (target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement) {
  const s = target.selectionStart ?? target.value.length;
  const e = target.selectionEnd ?? target.value.length;
  target.setRangeText(content, s, e, 'end'); emitInput();
  target.dispatchEvent(new Event('change', { bubbles: true }));
} else if (target.isContentEditable) {
  const sel = window.getSelection();
  if (sel && sel.rangeCount > 0) {
    const r = sel.getRangeAt(0); r.deleteContents();
    const tn = document.createTextNode(content); r.insertNode(tn);
    r.setStartAfter(tn); r.collapse(true); sel.removeAllRanges(); sel.addRange(r);
  } else { target.textContent = (target.textContent || '') + content; }
  emitInput();
} else { target.textContent = (target.textContent || '') + content; emitInput(); }
"#;

    let sel_arg: Value = selector.unwrap_or("").into();
    let text_arg: Value = text.into();

    driver
        .execute(js, vec![sel_arg, text_arg])
        .await
        .context("paste_like: JS execution failed")?;

    Ok(())
}

// ---------------------------------------------------------------------------
// wait_command
// ---------------------------------------------------------------------------

pub async fn wait_command(driver: &WdClient, cmd: &CommandSpec) -> Result<Value> {
    // Pure sleep variant
    if let Some(ms) = cmd.ms {
        sleep(Duration::from_millis(ms)).await;
        return Ok(json!({"ok": true, "type": "wait", "ms": ms}));
    }

    // Selector + condition variant
    let selector = cmd.selector.as_deref().unwrap_or("");
    let condition = cmd.condition.as_deref().unwrap_or(if !selector.is_empty() {
        "displayed"
    } else {
        "displayed"
    });
    let attribute = cmd.attribute.as_deref().unwrap_or("");
    let expected = cmd.value.as_deref().unwrap_or("");
    let timeout_ms = cmd.timeout.unwrap_or(20000);
    let interval_ms = cmd.interval.unwrap_or(250);

    if selector.is_empty() {
        bail!("wait command requires either 'ms' or 'selector'");
    }

    let js = r#"
const selector = arguments[0];
const condition = (arguments[1] || '').toLowerCase();
const attribute = arguments[2] || '';
const expected = arguments[3] || '';
const el = document.querySelector(selector);
if (condition === 'exist') return !!el;
if (condition === 'not-exist' || condition === 'gone') return !el;
if (!el) return false;
const style = window.getComputedStyle(el);
const displayed = style.display !== 'none' && style.visibility !== 'hidden' && style.opacity !== '0';
if (condition === 'displayed' || condition === 'visible') return displayed;
if (condition === 'hidden' || condition === 'not-displayed') return !displayed;
if (condition === 'enabled') return !el.disabled;
if (condition === 'disabled') return !!el.disabled;
if (condition === 'text-contains') return (el.textContent || '').includes(expected);
if (condition === 'text-equals') return (el.textContent || '').trim() === expected;
if (condition === 'value-contains') return String(el.value || '').includes(expected);
if (condition === 'value-equals') return String(el.value || '') === expected;
if (condition === 'attribute-equals') return String(el.getAttribute(attribute) || '') === expected;
if (condition === 'attribute-contains') return String(el.getAttribute(attribute) || '').includes(expected);
if (condition === 'clickable') { const rect = el.getBoundingClientRect(); return displayed && rect.width > 0 && rect.height > 0 && !el.disabled; }
if (condition === 'not-clickable') { const rect = el.getBoundingClientRect(); return !displayed || rect.width <= 0 || rect.height <= 0 || !!el.disabled; }
return false;
"#;

    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    let interval = Duration::from_millis(interval_ms);

    loop {
        let result = driver
            .execute(
                js,
                vec![
                    json!(selector),
                    json!(condition),
                    json!(attribute),
                    json!(expected),
                ],
            )
            .await;

        if let Ok(val) = result {
            if val.as_bool().unwrap_or(false) {
                return Ok(json!({
                    "ok": true,
                    "type": "wait",
                    "selector": selector,
                    "condition": condition,
                }));
            }
        }

        if Instant::now() >= deadline {
            bail!(
                "wait_command timed out after {}ms: selector={}, condition={}",
                timeout_ms,
                selector,
                condition
            );
        }

        sleep(interval).await;
    }
}

// ---------------------------------------------------------------------------
// execute_single_command
// ---------------------------------------------------------------------------

pub async fn execute_single_command(driver: &WdClient, cmd: &CommandSpec) -> Result<Value> {
    let command_type = cmd.command_type.as_str();

    match command_type {
        // -----------------------------------------------------------------
        // open
        // -----------------------------------------------------------------
        "open" => {
            let raw_url = cmd.url.as_deref().unwrap_or("about:blank");

            // Auto-prepend https:// for bare domains (e.g. "doubao.com")
            let url_owned;
            let url = if !raw_url.contains("://") && raw_url != "about:blank" {
                url_owned = format!("https://{}", raw_url);
                &url_owned
            } else {
                raw_url
            };

            driver
                .goto(url)
                .await
                .with_context(|| format!("open: failed to navigate to {url}"))?;

            if let Some(ref vp) = cmd.viewport {
                let js = format!("window.resizeTo({}, {})", vp.width, vp.height);
                driver
                    .execute(&js, vec![])
                    .await
                    .context("open: failed to resize viewport")?;
            }

            Ok(json!({"ok": true, "type": "open", "url": url}))
        }

        // -----------------------------------------------------------------
        // click
        // -----------------------------------------------------------------
        "click" => {
            let selector = cmd
                .selector
                .as_deref()
                .context("click: 'selector' is required")?;
            let timeout = cmd.timeout.unwrap_or(20000);
            let interval = cmd.interval.unwrap_or(250);
            let scope = cmd.scope.as_deref();

            let ele = wait_find(driver, selector, scope, timeout, interval)
                .await
                .with_context(|| format!("click: element not found: {selector}"))?;

            // Try native WebDriver click first.  If another element
            // intercepts the click (overlay, tooltip, expired-QR cover,
            // etc.) fall back to:
            //   1. --fallback <selector>  → find & click that element
            //   2. --fallback parent      → JS click on parentElement
            //   3. --fallback sibling     → JS click on next/prev sibling
            //   4. (no fallback)          → smart JS: parent → sibling → self
            let mut used_js = false;
            let mut fallback_strategy: Option<String> = None;
            match ele.click().await {
                Ok(()) => {}
                Err(e) => {
                    let msg = format!("{e:#}");
                    if msg.contains("element click intercepted") {
                        let fallback = cmd.fallback.as_deref();
                        eprintln!(
                            "info: native click intercepted on {selector}, \
                             retrying with fallback={fb}",
                            fb = fallback.unwrap_or("auto(parent→sibling→self)")
                        );
                        ele.scroll_into_view().await.ok();

                        match fallback {
                            // Explicit "parent" keyword
                            Some("parent") => {
                                ele.js_click_parent().await.with_context(|| {
                                    format!("click: parent JS click failed: {selector}")
                                })?;
                                fallback_strategy = Some("parent".into());
                            }
                            // Explicit "sibling" keyword
                            Some("sibling") => {
                                ele.js_click_sibling().await.with_context(|| {
                                    format!("click: sibling JS click failed: {selector}")
                                })?;
                                fallback_strategy = Some("sibling".into());
                            }
                            // Custom CSS selector → find that element and click it
                            Some(fb_sel) => {
                                let fb_ele = wait_find(driver, fb_sel, scope, timeout, interval)
                                    .await
                                    .with_context(|| {
                                        format!("click: fallback element not found: {fb_sel}")
                                    })?;
                                fb_ele.scroll_into_view().await.ok();
                                // Try native click on fallback, then JS click
                                if let Err(_) = fb_ele.click().await {
                                    fb_ele.js_click().await.with_context(|| {
                                        format!("click: fallback JS click also failed: {fb_sel}")
                                    })?;
                                }
                                fallback_strategy = Some(format!("selector:{fb_sel}"));
                            }
                            // No fallback specified → smart auto: parent → sibling → self
                            None => {
                                let strategy = ele.js_click_smart().await.with_context(|| {
                                    format!("click: smart JS click failed: {selector}")
                                })?;
                                fallback_strategy = Some(strategy);
                            }
                        }
                        used_js = true;
                    } else {
                        return Err(e)
                            .with_context(|| format!("click: failed to click: {selector}"));
                    }
                }
            }

            Ok(json!({
                "ok": true,
                "type": "click",
                "selector": selector,
                "scope": cmd.scope,
                "jsClick": used_js,
                "fallback": fallback_strategy,
            }))
        }

        // -----------------------------------------------------------------
        // fill
        // -----------------------------------------------------------------
        "fill" => {
            let text = cmd.text.as_deref().unwrap_or("");
            // Treat empty/blank selectors as None → use focused element
            let selector = cmd.selector.as_deref().filter(|s| !s.trim().is_empty());

            match selector {
                Some(selector) => {
                    let timeout = cmd.timeout.unwrap_or(20000);
                    let interval = cmd.interval.unwrap_or(250);
                    let scope = cmd.scope.as_deref();

                    let ele = wait_find(driver, selector, scope, timeout, interval)
                        .await
                        .with_context(|| format!("fill: element not found: {selector}"))?;

                    ele.click()
                        .await
                        .with_context(|| format!("fill: failed to click: {selector}"))?;

                    fill_like(driver, Some(selector), text)
                        .await
                        .context("fill: fill_like failed")?;
                }
                None => {
                    fill_like(driver, None, text)
                        .await
                        .context("fill: fill_like failed (no selector, using focused element)")?;
                }
            }

            Ok(json!({
                "ok": true,
                "type": "fill",
                "text": text,
                "selector": selector,
                "scope": cmd.scope,
            }))
        }

        // -----------------------------------------------------------------
        // paste
        // -----------------------------------------------------------------
        "paste" => {
            let text = cmd.text.as_deref().unwrap_or("");
            // Treat empty/blank selectors as None → use focused element
            let selector = cmd.selector.as_deref().filter(|s| !s.trim().is_empty());

            match selector {
                Some(selector) => {
                    let timeout = cmd.timeout.unwrap_or(20000);
                    let interval = cmd.interval.unwrap_or(250);
                    let scope = cmd.scope.as_deref();

                    let ele = wait_find(driver, selector, scope, timeout, interval)
                        .await
                        .with_context(|| format!("paste: element not found: {selector}"))?;

                    ele.click()
                        .await
                        .with_context(|| format!("paste: failed to click: {selector}"))?;

                    paste_like(driver, Some(selector), text)
                        .await
                        .context("paste: paste_like failed")?;
                }
                None => {
                    paste_like(driver, None, text)
                        .await
                        .context("paste: paste_like failed (no selector, using focused element)")?;
                }
            }

            Ok(json!({
                "ok": true,
                "type": "paste",
                "text": text,
                "selector": selector,
                "scope": cmd.scope,
            }))
        }

        // -----------------------------------------------------------------
        // screenshot
        // -----------------------------------------------------------------
        "screenshot" => {
            let selector = cmd
                .selector
                .as_deref()
                .context("screenshot: 'selector' is required")?;
            let timeout = cmd.timeout.unwrap_or(20000);
            let interval = cmd.interval.unwrap_or(250);
            let path_str = cmd.path.as_deref().unwrap_or("outputs/screenshot.png");
            let scope = cmd.scope.as_deref();

            let ele = wait_find(driver, selector, scope, timeout, interval)
                .await
                .with_context(|| format!("screenshot: element not found: {selector}"))?;

            let png_bytes = ele
                .screenshot_as_png()
                .await
                .context("screenshot: failed to capture element screenshot")?;

            let dest = Path::new(path_str);
            if let Some(parent) = dest.parent() {
                tokio::fs::create_dir_all(parent).await.with_context(|| {
                    format!(
                        "screenshot: failed to create directory: {}",
                        parent.display()
                    )
                })?;
            }

            tokio::fs::write(dest, &png_bytes)
                .await
                .with_context(|| format!("screenshot: failed to write file: {path_str}"))?;

            Ok(json!({
                "ok": true,
                "type": "screenshot",
                "path": path_str,
                "selector": selector,
                "scope": cmd.scope,
            }))
        }

        // -----------------------------------------------------------------
        // scroll
        // -----------------------------------------------------------------
        "scroll" => {
            let direction = cmd.direction.as_deref().unwrap_or("down");
            let amount = cmd.amount.unwrap_or(800);
            let behavior = cmd.behavior.as_deref().unwrap_or("smooth");

            let (x, y): (i64, i64) = match direction {
                "up" => (0, -amount),
                "down" => (0, amount),
                "left" => (-amount, 0),
                "right" => (amount, 0),
                other => bail!("scroll: unsupported direction: {other}"),
            };

            match cmd.selector.as_deref() {
                Some(selector) => {
                    let js = format!(
                        "document.querySelector('{}').scrollBy({{left:{},top:{},behavior:'{}'}})",
                        selector.replace('\'', "\\'"),
                        x,
                        y,
                        behavior,
                    );
                    driver
                        .execute(&js, vec![])
                        .await
                        .with_context(|| format!("scroll: failed to scroll element: {selector}"))?;
                }
                None => {
                    let js = format!(
                        "window.scrollBy({{left:{},top:{},behavior:'{}'}})",
                        x, y, behavior,
                    );
                    driver
                        .execute(&js, vec![])
                        .await
                        .context("scroll: failed to scroll window")?;
                }
            }

            Ok(json!({
                "ok": true,
                "type": "scroll",
                "direction": direction,
                "amount": amount,
                "x": x,
                "y": y,
                "behavior": behavior,
            }))
        }

        // -----------------------------------------------------------------
        // title
        // -----------------------------------------------------------------
        "title" => {
            let title = driver
                .title()
                .await
                .context("title: failed to get page title")?;

            Ok(json!({"ok": true, "type": "title", "title": title}))
        }

        // -----------------------------------------------------------------
        // last-message-content
        // -----------------------------------------------------------------
        "last-message-content" => {
            let selector = cmd
                .selector
                .as_deref()
                .unwrap_or("[data-testid=\"message-block-container\"]");

            let js = r#"
const css = arguments[0];
const nodes = Array.from(document.querySelectorAll(css));
const target = nodes[nodes.length - 1];
if (!target) return { found: false, selector: css };
return {
  found: true, selector: css,
  html: target.innerHTML,
  text: (target.textContent || '').trim(),
  images: Array.from(target.querySelectorAll('img')).map((img, i) => ({
    index: i, src: img.getAttribute('src') || '', alt: img.getAttribute('alt') || '', title: img.getAttribute('title') || ''
  })),
  links: Array.from(target.querySelectorAll('a')).map((a, i) => ({
    index: i, href: a.getAttribute('href') || '', text: (a.textContent || '').trim()
  }))
};
"#;

            let data = driver
                .execute(js, vec![json!(selector)])
                .await
                .context("last-message-content: JS execution failed")?;

            Ok(json!({"ok": true, "type": "last-message-content", "data": data}))
        }

        // -----------------------------------------------------------------
        // wait
        // -----------------------------------------------------------------
        "wait" => wait_command(driver, cmd).await,

        // -----------------------------------------------------------------
        // unsupported
        // -----------------------------------------------------------------
        other => {
            bail!("unsupported command type: {other}");
        }
    }
}
