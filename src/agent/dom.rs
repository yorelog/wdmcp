//! DOM analysis utilities for extracting interactive elements from a page.
//!
//! This module injects JavaScript into the browser via WebDriver's `execute`
//! method to traverse the page DOM and produce [`PageSlot`] entries for every
//! interactive element found.  The heavy lifting happens inside
//! [`DOM_EXTRACT_JS`]; the Rust side ([`extract_slots`]) simply parses the
//! resulting JSON and maps it into the typed slot / analysis structures
//! defined in [`super::slots`].

use anyhow::{Context, Result};
use serde_json::Value;

use crate::agent::slots::*;
use crate::types::now_iso;
use crate::webdriver::WdClient;

// ---------------------------------------------------------------------------
// JavaScript payload
// ---------------------------------------------------------------------------

/// JavaScript executed inside the browser to discover all interactive elements.
///
/// The script returns a JSON **string** (via `JSON.stringify`) with the shape:
///
/// ```json
/// {
///   "url": "...",
///   "title": "...",
///   "elements": [ { tag, inputType, text, selector, … }, … ],
///   "forms": [ { id, action, method }, … ]
/// }
/// ```
pub const DOM_EXTRACT_JS: &str = r#"
return (function() {
    // ---- helper: generate a stable CSS selector for an element ----
    function generateSelector(el) {
        // 1. data-testid
        var testId = el.getAttribute('data-testid');
        if (testId) {
            return el.tagName.toLowerCase() + '[data-testid="' + testId + '"]';
        }
        // 2. id
        if (el.id) {
            return '#' + CSS.escape(el.id);
        }
        // 3. name attribute
        var nameAttr = el.getAttribute('name');
        if (nameAttr) {
            return el.tagName.toLowerCase() + '[name="' + nameAttr + '"]';
        }
        // 4. unique aria-label
        var ariaLabel = el.getAttribute('aria-label');
        if (ariaLabel) {
            var tag = el.tagName.toLowerCase();
            var matches = document.querySelectorAll(tag + '[aria-label="' + ariaLabel + '"]');
            if (matches.length === 1) {
                return tag + '[aria-label="' + ariaLabel + '"]';
            }
        }
        // 5. path using nth-of-type (up to 3 levels)
        var parts = [];
        var cur = el;
        for (var depth = 0; depth < 3 && cur && cur !== document.documentElement; depth++) {
            var tag = cur.tagName.toLowerCase();
            var parent = cur.parentElement;
            if (parent) {
                var siblings = Array.from(parent.children).filter(function(c) {
                    return c.tagName === cur.tagName;
                });
                if (siblings.length > 1) {
                    var idx = siblings.indexOf(cur) + 1;
                    parts.unshift(tag + ':nth-of-type(' + idx + ')');
                } else {
                    parts.unshift(tag);
                }
            } else {
                parts.unshift(tag);
            }
            cur = parent;
        }
        return parts.join(' > ');
    }

    // ---- helper: visibility check ----
    function isVisible(el) {
        return el.offsetWidth > 0 && el.offsetHeight > 0 &&
               window.getComputedStyle(el).visibility !== 'hidden';
    }

    // ---- collect interactive elements ----
    var selectors = [
        'a[href]', 'button', 'input', 'select', 'textarea',
        '[role="button"]', '[role="link"]', '[role="tab"]', '[role="menuitem"]',
        '[onclick]', '[contenteditable="true"]'
    ];
    var seen = new Set();
    var elements = [];

    selectors.forEach(function(sel) {
        var nodes;
        try { nodes = document.querySelectorAll(sel); } catch(e) { return; }
        for (var i = 0; i < nodes.length; i++) {
            var el = nodes[i];
            if (seen.has(el)) continue;
            // skip form elements themselves — they are captured separately
            if (el.tagName.toLowerCase() === 'form') continue;
            seen.add(el);

            var rect = el.getBoundingClientRect();
            var text = (el.innerText || '').trim();
            if (text.length > 100) text = text.substring(0, 100);

            var closestForm = el.closest('form');

            elements.push({
                tag: el.tagName.toLowerCase(),
                inputType: el.getAttribute('type') || null,
                text: text || null,
                selector: generateSelector(el),
                formId: closestForm ? (closestForm.id || null) : null,
                ariaLabel: el.getAttribute('aria-label') || null,
                placeholder: el.getAttribute('placeholder') || null,
                href: el.getAttribute('href') || null,
                name: el.getAttribute('name') || null,
                dataTestId: el.getAttribute('data-testid') || null,
                role: el.getAttribute('role') || null,
                contentEditable: el.getAttribute('contenteditable') || null,
                visible: isVisible(el),
                disabled: !!el.disabled,
                rect: { x: rect.x, y: rect.y, width: rect.width, height: rect.height }
            });
        }
    });

    // ---- collect form metadata ----
    var formNodes = document.querySelectorAll('form');
    var forms = [];
    for (var f = 0; f < formNodes.length; f++) {
        var fm = formNodes[f];
        forms.push({
            id: fm.id || null,
            action: fm.getAttribute('action') || null,
            method: fm.getAttribute('method') || null
        });
    }

    var result = {
        url: window.location.href,
        title: document.title,
        elements: elements,
        forms: forms
    };

    return JSON.stringify(result);
})();
"#;

// ---------------------------------------------------------------------------
// Rust extraction
// ---------------------------------------------------------------------------

/// Execute [`DOM_EXTRACT_JS`] in the browser, parse the result, and return a
/// fully typed [`PageAnalysis`].
pub async fn extract_slots(driver: &WdClient) -> Result<PageAnalysis> {
    // 1. Run the extraction script.
    let raw = driver
        .execute(DOM_EXTRACT_JS, vec![])
        .await
        .context("DOM extraction script failed")?;

    // The WebDriver value is a JSON string – unwrap it.
    let json_str = raw
        .as_str()
        .context("DOM extraction did not return a string")?;

    let root: Value =
        serde_json::from_str(json_str).context("Failed to parse DOM extraction JSON")?;

    let url = root
        .get("url")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let title = root
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    // 2. Convert raw element objects → PageSlot.
    let raw_elements = root
        .get("elements")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let mut slots: Vec<PageSlot> = Vec::with_capacity(raw_elements.len());

    for (idx, elem) in raw_elements.iter().enumerate() {
        let tag = str_field(elem, "tag");
        let input_type = opt_str(elem, "inputType");
        let text = opt_str(elem, "text");
        let selector = str_field(elem, "selector");
        let form_id = opt_str(elem, "formId");
        let aria_label = opt_str(elem, "ariaLabel");
        let placeholder = opt_str(elem, "placeholder");
        let href = opt_str(elem, "href");
        let name = opt_str(elem, "name");
        let data_testid = opt_str(elem, "dataTestId");
        let visible = elem
            .get("visible")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let disabled = elem
            .get("disabled")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        let rect = elem.get("rect").and_then(|r| {
            Some(SlotRect {
                x: r.get("x")?.as_f64()?,
                y: r.get("y")?.as_f64()?,
                width: r.get("width")?.as_f64()?,
                height: r.get("height")?.as_f64()?,
            })
        });

        let category = determine_category(&tag, input_type.as_deref(), elem);

        // Build a temporary slot so we can call `classify_slot`.
        let mut slot = PageSlot {
            slot_id: format!("s-{}", idx),
            tag,
            input_type,
            text,
            selector,
            category,
            safety_level: SafetyLevel::Observe, // placeholder
            form_id,
            aria_label,
            placeholder,
            href,
            name,
            data_testid,
            visible,
            disabled,
            rect,
        };

        slot.safety_level = classify_slot(&slot);
        slots.push(slot);
    }

    // 3. Build form info, associating each form with its child slot_ids.
    let raw_forms = root
        .get("forms")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let forms: Vec<FormInfo> = raw_forms
        .iter()
        .map(|f| {
            let fid = opt_str(f, "id");
            let action = opt_str(f, "action");
            let method = opt_str(f, "method");

            // Collect slot_ids whose form_id matches this form.
            let slot_ids: Vec<String> = slots
                .iter()
                .filter(|s| match (&s.form_id, &fid) {
                    (Some(sid), Some(fid_val)) => sid == fid_val,
                    // If the form has no id, we cannot reliably match by id
                    // alone, so we skip. A future enhancement could use DOM
                    // ordering, but for now unnamed forms get an empty list.
                    _ => false,
                })
                .map(|s| s.slot_id.clone())
                .collect();

            FormInfo {
                form_id: fid,
                action,
                method,
                slot_ids,
            }
        })
        .collect();

    // 4. Safety summary.
    let safety_summary = build_safety_summary(&slots);

    let slot_count = slots.len();

    Ok(PageAnalysis {
        url,
        title,
        slots,
        slot_count,
        safety_summary,
        forms,
        timestamp: now_iso(),
    })
}

/// Extract the visible text content of the page body.
pub async fn extract_page_text(driver: &WdClient) -> Result<String> {
    let raw = driver
        .execute(
            "return document.body ? document.body.innerText : '';",
            vec![],
        )
        .await
        .context("Failed to extract page text")?;

    Ok(raw.as_str().unwrap_or("").to_string())
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

/// Read a required string field, falling back to an empty string.
fn str_field(val: &Value, key: &str) -> String {
    val.get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

/// Read an optional string field — returns `None` for absent / null / empty.
fn opt_str(val: &Value, key: &str) -> Option<String> {
    val.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(String::from)
}

/// Map a tag name + optional input type to a [`SlotCategory`].
fn determine_category(tag: &str, input_type: Option<&str>, elem: &Value) -> SlotCategory {
    match tag {
        "a" => SlotCategory::Link,

        "button" => {
            let btn_type = input_type.unwrap_or("button");
            if btn_type == "submit" {
                SlotCategory::FormSubmit
            } else {
                SlotCategory::Button
            }
        }

        "input" => match input_type.unwrap_or("text") {
            "text" | "email" | "tel" | "url" | "search" | "number" | "date" | "datetime-local"
            | "month" | "week" | "time" | "color" => SlotCategory::TextInput,
            "password" => SlotCategory::PasswordInput,
            "checkbox" => SlotCategory::Checkbox,
            "radio" => SlotCategory::Radio,
            "file" => SlotCategory::FileUpload,
            "submit" | "image" => SlotCategory::FormSubmit,
            "hidden" | "reset" | "button" => SlotCategory::Button,
            _ => SlotCategory::TextInput,
        },

        "select" => SlotCategory::Select,
        "textarea" => SlotCategory::Textarea,

        _ => {
            // Elements matched via [contenteditable="true"]
            if is_contenteditable(elem) {
                SlotCategory::ContentEditable
            } else {
                // role-based fallback
                let role = elem_role(elem);
                match role.as_deref() {
                    Some("button") => SlotCategory::Button,
                    Some("link") => SlotCategory::Link,
                    Some("tab") | Some("menuitem") => SlotCategory::Button,
                    _ => SlotCategory::Other,
                }
            }
        }
    }
}

/// Check whether the element has `contenteditable="true"` set.
fn is_contenteditable(elem: &Value) -> bool {
    elem.get("contentEditable")
        .and_then(Value::as_str)
        .map(|v| v == "true")
        .unwrap_or(false)
}

/// Extract a `role` attribute value from the raw JS element object.
fn elem_role(elem: &Value) -> Option<String> {
    elem.get("role")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(String::from)
}

/// Count slots by safety level.
pub fn build_safety_summary(slots: &[PageSlot]) -> SafetySummary {
    let mut observe: usize = 0;
    let mut navigate: usize = 0;
    let mut interact: usize = 0;
    let mut submit: usize = 0;

    for slot in slots {
        match slot.safety_level {
            SafetyLevel::Observe => observe += 1,
            SafetyLevel::Navigate => navigate += 1,
            SafetyLevel::Interact => interact += 1,
            SafetyLevel::Submit => submit += 1,
        }
    }

    SafetySummary {
        observe,
        navigate,
        interact,
        submit,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn category_link() {
        let elem = json!({ "tag": "a" });
        assert_eq!(determine_category("a", None, &elem), SlotCategory::Link);
    }

    #[test]
    fn category_button_submit() {
        let elem = json!({ "tag": "button" });
        assert_eq!(
            determine_category("button", Some("submit"), &elem),
            SlotCategory::FormSubmit
        );
    }

    #[test]
    fn category_button_default() {
        let elem = json!({ "tag": "button" });
        assert_eq!(
            determine_category("button", None, &elem),
            SlotCategory::Button
        );
    }

    #[test]
    fn category_input_text() {
        let elem = json!({ "tag": "input" });
        assert_eq!(
            determine_category("input", Some("text"), &elem),
            SlotCategory::TextInput
        );
    }

    #[test]
    fn category_input_password() {
        let elem = json!({ "tag": "input" });
        assert_eq!(
            determine_category("input", Some("password"), &elem),
            SlotCategory::PasswordInput
        );
    }

    #[test]
    fn category_input_checkbox() {
        let elem = json!({ "tag": "input" });
        assert_eq!(
            determine_category("input", Some("checkbox"), &elem),
            SlotCategory::Checkbox
        );
    }

    #[test]
    fn category_input_radio() {
        let elem = json!({ "tag": "input" });
        assert_eq!(
            determine_category("input", Some("radio"), &elem),
            SlotCategory::Radio
        );
    }

    #[test]
    fn category_input_file() {
        let elem = json!({ "tag": "input" });
        assert_eq!(
            determine_category("input", Some("file"), &elem),
            SlotCategory::FileUpload
        );
    }

    #[test]
    fn category_input_submit() {
        let elem = json!({ "tag": "input" });
        assert_eq!(
            determine_category("input", Some("submit"), &elem),
            SlotCategory::FormSubmit
        );
    }

    #[test]
    fn category_select() {
        let elem = json!({ "tag": "select" });
        assert_eq!(
            determine_category("select", None, &elem),
            SlotCategory::Select
        );
    }

    #[test]
    fn category_textarea() {
        let elem = json!({ "tag": "textarea" });
        assert_eq!(
            determine_category("textarea", None, &elem),
            SlotCategory::Textarea
        );
    }

    #[test]
    fn category_unknown_tag_falls_to_other() {
        let elem = json!({ "tag": "div" });
        assert_eq!(determine_category("div", None, &elem), SlotCategory::Other);
    }

    #[test]
    fn category_role_button() {
        let elem = json!({ "tag": "div", "role": "button" });
        assert_eq!(determine_category("div", None, &elem), SlotCategory::Button);
    }

    #[test]
    fn category_role_link() {
        let elem = json!({ "tag": "span", "role": "link" });
        assert_eq!(determine_category("span", None, &elem), SlotCategory::Link);
    }

    #[test]
    fn category_contenteditable() {
        let elem = json!({ "tag": "div", "contentEditable": "true" });
        assert_eq!(
            determine_category("div", None, &elem),
            SlotCategory::ContentEditable
        );
    }

    #[test]
    fn opt_str_returns_none_for_empty() {
        let v = json!({ "key": "" });
        assert_eq!(opt_str(&v, "key"), None);
    }

    #[test]
    fn opt_str_returns_some_for_value() {
        let v = json!({ "key": "hello" });
        assert_eq!(opt_str(&v, "key"), Some("hello".to_string()));
    }

    #[test]
    fn opt_str_returns_none_for_missing() {
        let v = json!({});
        assert_eq!(opt_str(&v, "key"), None);
    }

    #[test]
    fn str_field_returns_empty_for_missing() {
        let v = json!({});
        assert_eq!(str_field(&v, "key"), "");
    }

    #[test]
    fn safety_summary_counts() {
        let make = |cat: SlotCategory, safety: SafetyLevel| PageSlot {
            slot_id: "s-0".into(),
            tag: "a".into(),
            input_type: None,
            text: None,
            selector: "a".into(),
            category: cat,
            safety_level: safety,
            form_id: None,
            aria_label: None,
            placeholder: None,
            href: None,
            name: None,
            data_testid: None,
            visible: true,
            disabled: false,
            rect: None,
        };

        let slots = vec![
            make(SlotCategory::Link, SafetyLevel::Navigate),
            make(SlotCategory::Link, SafetyLevel::Navigate),
            make(SlotCategory::Button, SafetyLevel::Interact),
            make(SlotCategory::FormSubmit, SafetyLevel::Submit),
        ];

        let summary = build_safety_summary(&slots);
        assert_eq!(summary.observe, 0);
        assert_eq!(summary.navigate, 2);
        assert_eq!(summary.interact, 1);
        assert_eq!(summary.submit, 1);
    }
}
