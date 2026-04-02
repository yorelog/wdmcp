//! Slot extraction types, safety classification, and task suggestion logic.
//!
//! This module defines the core types used by the agent layer to represent
//! interactive DOM elements ("slots"), classify their safety level, and
//! group them into higher-level task suggestions for AI-driven automation.

use std::cmp::Ordering;
use std::fmt;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// SafetyLevel
// ---------------------------------------------------------------------------

/// Graduated safety level for a DOM interaction.
///
/// The ordering is intentional: `Submit` is the most dangerous and
/// `Observe` is the safest.  This lets escalation logic use `max()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SafetyLevel {
    /// Read-only observation (green).
    Observe,
    /// Link clicks / URL changes (yellow).
    Navigate,
    /// Form input, typing (orange).
    Interact,
    /// Form submission, purchase, delete (red).
    Submit,
}

impl SafetyLevel {
    /// Numeric rank used for ordering — higher is more dangerous.
    fn rank(self) -> u8 {
        match self {
            Self::Observe => 0,
            Self::Navigate => 1,
            Self::Interact => 2,
            Self::Submit => 3,
        }
    }

    /// Whether this level requires explicit user confirmation before execution.
    pub fn requires_confirmation(&self) -> bool {
        matches!(self, Self::Submit)
    }
}

impl fmt::Display for SafetyLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Observe => write!(f, "🟢 Observe"),
            Self::Navigate => write!(f, "🟡 Navigate"),
            Self::Interact => write!(f, "🟠 Interact"),
            Self::Submit => write!(f, "🔴 Submit"),
        }
    }
}

impl PartialOrd for SafetyLevel {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for SafetyLevel {
    fn cmp(&self, other: &Self) -> Ordering {
        self.rank().cmp(&other.rank())
    }
}

// ---------------------------------------------------------------------------
// SlotCategory
// ---------------------------------------------------------------------------

/// Semantic category for an interactive DOM element.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SlotCategory {
    Link,
    Button,
    TextInput,
    PasswordInput,
    Checkbox,
    Radio,
    Select,
    Textarea,
    FileUpload,
    FormSubmit,
    ContentEditable,
    Other,
}

// ---------------------------------------------------------------------------
// SlotRect
// ---------------------------------------------------------------------------

/// Bounding-box rectangle for a slot, in CSS pixels.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SlotRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

// ---------------------------------------------------------------------------
// PageSlot
// ---------------------------------------------------------------------------

/// A single interactive element extracted from the page DOM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageSlot {
    /// Synthetic identifier, e.g. `"s-0"`, `"s-1"`.
    pub slot_id: String,
    /// HTML tag name (lowercase).
    pub tag: String,
    /// The `type` attribute for `<input>` elements.
    pub input_type: Option<String>,
    /// Visible text content, truncated to 100 characters.
    pub text: Option<String>,
    /// Best CSS selector that uniquely identifies this element.
    pub selector: String,
    /// Semantic category.
    pub category: SlotCategory,
    /// Safety classification.
    pub safety_level: SafetyLevel,
    /// ID of the parent `<form>`, if any.
    pub form_id: Option<String>,
    /// `aria-label` attribute.
    pub aria_label: Option<String>,
    /// `placeholder` attribute.
    pub placeholder: Option<String>,
    /// `href` attribute (for links).
    pub href: Option<String>,
    /// `name` attribute.
    pub name: Option<String>,
    /// `data-testid` attribute.
    pub data_testid: Option<String>,
    /// Whether the element is currently visible in the viewport.
    pub visible: bool,
    /// Whether the element is disabled.
    pub disabled: bool,
    /// Bounding box in CSS pixels.
    pub rect: Option<SlotRect>,
}

// ---------------------------------------------------------------------------
// PageAnalysis
// ---------------------------------------------------------------------------

/// Top-level result from a full DOM analysis pass.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageAnalysis {
    /// Current page URL.
    pub url: String,
    /// Current page title.
    pub title: String,
    /// All interactive slots found on the page.
    pub slots: Vec<PageSlot>,
    /// Total number of slots (convenience; equals `slots.len()`).
    pub slot_count: usize,
    /// Aggregate counts per safety level.
    pub safety_summary: SafetySummary,
    /// Detected forms and the slots that belong to each.
    pub forms: Vec<FormInfo>,
    /// ISO-8601 timestamp of when the analysis was performed.
    pub timestamp: String,
}

// ---------------------------------------------------------------------------
// SafetySummary
// ---------------------------------------------------------------------------

/// Counts of slots at each safety level.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SafetySummary {
    pub observe: usize,
    pub navigate: usize,
    pub interact: usize,
    pub submit: usize,
}

// ---------------------------------------------------------------------------
// FormInfo
// ---------------------------------------------------------------------------

/// Metadata about a `<form>` element and the slots it contains.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FormInfo {
    /// The `id` attribute of the form, if present.
    pub form_id: Option<String>,
    /// The `action` attribute.
    pub action: Option<String>,
    /// The `method` attribute (GET / POST / …).
    pub method: Option<String>,
    /// IDs of [`PageSlot`]s that are children of this form.
    pub slot_ids: Vec<String>,
}

// ---------------------------------------------------------------------------
// TaskSuggestion
// ---------------------------------------------------------------------------

/// A higher-level task that the agent can propose to the user.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskSuggestion {
    /// Human-readable title, e.g. "登录", "Search".
    pub title: String,
    /// Longer description of what the task does.
    pub description: String,
    /// Worst-case safety level of the task.
    pub safety_level: SafetyLevel,
    /// Slot IDs involved in this task.
    pub slot_ids: Vec<String>,
    /// Pre-built `CommandSpec` JSON array ready for batch execution.
    pub commands: Vec<serde_json::Value>,
}

// ---------------------------------------------------------------------------
// classify_slot
// ---------------------------------------------------------------------------

/// Classify a single slot into a [`SafetyLevel`] based on its category and
/// textual cues.
///
/// # Rules
///
/// 1. `FormSubmit` → inspect text for dangerous keywords → [`SafetyLevel::Submit`].
/// 2. `Link` → [`SafetyLevel::Navigate`].
/// 3. Input-like categories (`TextInput`, `PasswordInput`, `Textarea`, `Select`,
///    `Checkbox`, `Radio`, `FileUpload`, `ContentEditable`) → [`SafetyLevel::Interact`].
/// 4. `Button` → heuristic based on visible text / aria-label:
///    - Submit-like patterns → `Submit`
///    - Navigate-like patterns → `Navigate`
///    - Otherwise → `Interact`
/// 5. Everything else → [`SafetyLevel::Observe`].
pub fn classify_slot(slot: &PageSlot) -> SafetyLevel {
    match slot.category {
        // 1. Explicit form submit
        SlotCategory::FormSubmit => SafetyLevel::Submit,

        // 2. Links
        SlotCategory::Link => SafetyLevel::Navigate,

        // 3. Input-like elements
        SlotCategory::TextInput
        | SlotCategory::PasswordInput
        | SlotCategory::Textarea
        | SlotCategory::Select
        | SlotCategory::Checkbox
        | SlotCategory::Radio
        | SlotCategory::FileUpload
        | SlotCategory::ContentEditable => SafetyLevel::Interact,

        // 4. Buttons — need deeper inspection
        SlotCategory::Button => classify_button(slot),

        // 5. Anything else
        _ => SafetyLevel::Observe,
    }
}

/// Classify a button by scanning its visible text and aria-label for known
/// patterns.
fn classify_button(slot: &PageSlot) -> SafetyLevel {
    let haystack = combined_text(slot);

    if is_submit_like(&haystack) {
        return SafetyLevel::Submit;
    }

    if is_navigate_like(&haystack) {
        return SafetyLevel::Navigate;
    }

    SafetyLevel::Interact
}

/// Merge `text` and `aria_label` into a single lowercase string for matching.
fn combined_text(slot: &PageSlot) -> String {
    let mut buf = String::new();
    if let Some(ref t) = slot.text {
        buf.push_str(t);
    }
    buf.push(' ');
    if let Some(ref a) = slot.aria_label {
        buf.push_str(a);
    }
    buf.to_lowercase()
}

/// Submit / destructive action patterns.
///
/// Equivalent regex:
/// `(?i)(submit|purchase|buy|delete|remove|confirm|checkout|pay|post|发布|购买|删除|确认|提交|付款|下单|支付)`
const SUBMIT_KEYWORDS: &[&str] = &[
    "submit", "purchase", "buy", "delete", "remove", "confirm", "checkout", "pay", "post", "发布",
    "购买", "删除", "确认", "提交", "付款", "下单", "支付",
];

/// Navigation patterns.
///
/// Equivalent regex:
/// `(?i)(back|next|more|details|view|查看|返回|下一步|更多)`
const NAVIGATE_KEYWORDS: &[&str] = &[
    "back",
    "next",
    "more",
    "details",
    "view",
    "查看",
    "返回",
    "下一步",
    "更多",
];

fn is_submit_like(haystack: &str) -> bool {
    SUBMIT_KEYWORDS.iter().any(|kw| haystack.contains(kw))
}

fn is_navigate_like(haystack: &str) -> bool {
    NAVIGATE_KEYWORDS.iter().any(|kw| haystack.contains(kw))
}

// ---------------------------------------------------------------------------
// group_suggestions
// ---------------------------------------------------------------------------

/// Analyse a [`PageAnalysis`] and produce a list of [`TaskSuggestion`]s
/// grouped by logical task (forms, navigation clusters, search, etc.).
///
/// Results are sorted safest-first so the agent can present low-risk actions
/// before high-risk ones.
pub fn group_suggestions(analysis: &PageAnalysis) -> Vec<TaskSuggestion> {
    let mut suggestions: Vec<TaskSuggestion> = Vec::new();

    // ---- 1. One suggestion per form -----------------------------------------
    for form in &analysis.forms {
        let form_slots: Vec<&PageSlot> = analysis
            .slots
            .iter()
            .filter(|s| form.slot_ids.contains(&s.slot_id))
            .collect();

        if form_slots.is_empty() {
            continue;
        }

        let worst_safety = form_slots
            .iter()
            .map(|s| s.safety_level)
            .max()
            .unwrap_or(SafetyLevel::Observe);

        let form_label = form.form_id.as_deref().unwrap_or("form");

        let title = format!("Fill and submit {}", form_label);
        let description = format!(
            "Complete {} field(s) in <form{}> and submit.",
            form_slots.len(),
            form.action
                .as_deref()
                .map(|a| format!(" action=\"{}\"", a))
                .unwrap_or_default(),
        );

        let slot_ids: Vec<String> = form.slot_ids.clone();

        // Build a minimal command sequence: fill each input, then click submit.
        let mut commands: Vec<serde_json::Value> = Vec::new();
        for fs in &form_slots {
            match fs.category {
                SlotCategory::TextInput | SlotCategory::PasswordInput | SlotCategory::Textarea => {
                    commands.push(serde_json::json!({
                        "type": "fill",
                        "selector": fs.selector,
                        "text": ""
                    }));
                }
                SlotCategory::Select => {
                    commands.push(serde_json::json!({
                        "type": "click",
                        "selector": fs.selector
                    }));
                }
                SlotCategory::Checkbox | SlotCategory::Radio => {
                    commands.push(serde_json::json!({
                        "type": "click",
                        "selector": fs.selector
                    }));
                }
                SlotCategory::FormSubmit | SlotCategory::Button => {
                    commands.push(serde_json::json!({
                        "type": "click",
                        "selector": fs.selector
                    }));
                }
                _ => {}
            }
        }

        suggestions.push(TaskSuggestion {
            title,
            description,
            safety_level: worst_safety,
            slot_ids,
            commands,
        });
    }

    // ---- 2. Group navigation links ------------------------------------------
    let nav_links: Vec<&PageSlot> = analysis
        .slots
        .iter()
        .filter(|s| s.category == SlotCategory::Link && s.visible)
        .collect();

    if !nav_links.is_empty() {
        let slot_ids: Vec<String> = nav_links.iter().map(|s| s.slot_id.clone()).collect();
        let commands: Vec<serde_json::Value> = nav_links
            .iter()
            .map(|s| {
                serde_json::json!({
                    "type": "click",
                    "selector": s.selector
                })
            })
            .collect();

        let description = format!(
            "{} navigation link(s) available on the page.",
            nav_links.len()
        );

        suggestions.push(TaskSuggestion {
            title: "Navigate".to_string(),
            description,
            safety_level: SafetyLevel::Navigate,
            slot_ids,
            commands,
        });
    }

    // ---- 3. Search suggestion (text input near a button) --------------------
    //
    // Heuristic: if a TextInput and a Button are adjacent (slot-index distance
    // ≤ 3) and neither belongs to a form already covered, suggest "Search".
    let form_slot_ids: std::collections::HashSet<&String> = analysis
        .forms
        .iter()
        .flat_map(|f| f.slot_ids.iter())
        .collect();

    let non_form_text_inputs: Vec<(usize, &PageSlot)> = analysis
        .slots
        .iter()
        .enumerate()
        .filter(|(_, s)| {
            s.category == SlotCategory::TextInput && !form_slot_ids.contains(&s.slot_id)
        })
        .collect();

    let non_form_buttons: Vec<(usize, &PageSlot)> = analysis
        .slots
        .iter()
        .enumerate()
        .filter(|(_, s)| s.category == SlotCategory::Button && !form_slot_ids.contains(&s.slot_id))
        .collect();

    for (ti_idx, ti_slot) in &non_form_text_inputs {
        for (btn_idx, btn_slot) in &non_form_buttons {
            let distance = if btn_idx > ti_idx {
                btn_idx - ti_idx
            } else {
                ti_idx - btn_idx
            };

            if distance <= 3 {
                let slot_ids = vec![ti_slot.slot_id.clone(), btn_slot.slot_id.clone()];
                let commands = vec![
                    serde_json::json!({
                        "type": "fill",
                        "selector": ti_slot.selector,
                        "text": ""
                    }),
                    serde_json::json!({
                        "type": "click",
                        "selector": btn_slot.selector
                    }),
                ];

                suggestions.push(TaskSuggestion {
                    title: "Search".to_string(),
                    description: format!(
                        "Type into \"{}\" and press \"{}\".",
                        ti_slot
                            .placeholder
                            .as_deref()
                            .or(ti_slot.aria_label.as_deref())
                            .or(ti_slot.name.as_deref())
                            .unwrap_or(&ti_slot.selector),
                        btn_slot
                            .text
                            .as_deref()
                            .or(btn_slot.aria_label.as_deref())
                            .unwrap_or("button"),
                    ),
                    safety_level: SafetyLevel::Interact,
                    slot_ids,
                    commands,
                });

                // Only generate one search suggestion per input.
                break;
            }
        }
    }

    // ---- Sort safest-first --------------------------------------------------
    suggestions.sort_by(|a, b| a.safety_level.cmp(&b.safety_level));

    suggestions
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: minimal slot with only the fields we care about filled in.
    fn make_slot(category: SlotCategory, text: Option<&str>) -> PageSlot {
        PageSlot {
            slot_id: "s-0".into(),
            tag: "button".into(),
            input_type: None,
            text: text.map(|t| t.to_string()),
            selector: "button".into(),
            category,
            safety_level: SafetyLevel::Observe, // will be reclassified
            form_id: None,
            aria_label: None,
            placeholder: None,
            href: None,
            name: None,
            data_testid: None,
            visible: true,
            disabled: false,
            rect: None,
        }
    }

    #[test]
    fn safety_level_ordering() {
        assert!(SafetyLevel::Submit > SafetyLevel::Interact);
        assert!(SafetyLevel::Interact > SafetyLevel::Navigate);
        assert!(SafetyLevel::Navigate > SafetyLevel::Observe);
    }

    #[test]
    fn safety_level_display() {
        assert_eq!(format!("{}", SafetyLevel::Observe), "🟢 Observe");
        assert_eq!(format!("{}", SafetyLevel::Submit), "🔴 Submit");
    }

    #[test]
    fn classify_form_submit() {
        let slot = make_slot(SlotCategory::FormSubmit, Some("Go"));
        assert_eq!(classify_slot(&slot), SafetyLevel::Submit);
    }

    #[test]
    fn classify_link() {
        let slot = make_slot(SlotCategory::Link, Some("Home"));
        assert_eq!(classify_slot(&slot), SafetyLevel::Navigate);
    }

    #[test]
    fn classify_text_input() {
        let slot = make_slot(SlotCategory::TextInput, None);
        assert_eq!(classify_slot(&slot), SafetyLevel::Interact);
    }

    #[test]
    fn classify_button_submit_keyword() {
        let slot = make_slot(SlotCategory::Button, Some("Purchase Now"));
        assert_eq!(classify_slot(&slot), SafetyLevel::Submit);
    }

    #[test]
    fn classify_button_chinese_submit() {
        let slot = make_slot(SlotCategory::Button, Some("确认支付"));
        assert_eq!(classify_slot(&slot), SafetyLevel::Submit);
    }

    #[test]
    fn classify_button_navigate_keyword() {
        let slot = make_slot(SlotCategory::Button, Some("View Details"));
        assert_eq!(classify_slot(&slot), SafetyLevel::Navigate);
    }

    #[test]
    fn classify_button_generic() {
        let slot = make_slot(SlotCategory::Button, Some("OK"));
        assert_eq!(classify_slot(&slot), SafetyLevel::Interact);
    }

    #[test]
    fn classify_other() {
        let slot = make_slot(SlotCategory::Other, None);
        assert_eq!(classify_slot(&slot), SafetyLevel::Observe);
    }

    #[test]
    fn requires_confirmation_only_for_submit() {
        assert!(!SafetyLevel::Observe.requires_confirmation());
        assert!(!SafetyLevel::Navigate.requires_confirmation());
        assert!(!SafetyLevel::Interact.requires_confirmation());
        assert!(SafetyLevel::Submit.requires_confirmation());
    }

    #[test]
    fn group_suggestions_sorts_safest_first() {
        let analysis = PageAnalysis {
            url: "https://example.com".into(),
            title: "Example".into(),
            slots: vec![
                {
                    let mut s = make_slot(SlotCategory::Link, Some("About"));
                    s.slot_id = "s-0".into();
                    s.tag = "a".into();
                    s.selector = "a.about".into();
                    s.safety_level = SafetyLevel::Navigate;
                    s
                },
                {
                    let mut s = make_slot(SlotCategory::TextInput, None);
                    s.slot_id = "s-1".into();
                    s.tag = "input".into();
                    s.selector = "input.search".into();
                    s.safety_level = SafetyLevel::Interact;
                    s
                },
                {
                    let mut s = make_slot(SlotCategory::Button, Some("Go"));
                    s.slot_id = "s-2".into();
                    s.selector = "button.go".into();
                    s.safety_level = SafetyLevel::Interact;
                    s
                },
            ],
            slot_count: 3,
            safety_summary: SafetySummary {
                observe: 0,
                navigate: 1,
                interact: 2,
                submit: 0,
            },
            forms: vec![],
            timestamp: "2025-01-01T00:00:00Z".into(),
        };

        let suggestions = group_suggestions(&analysis);
        assert!(!suggestions.is_empty());

        // Should be sorted safest first.
        for pair in suggestions.windows(2) {
            assert!(pair[0].safety_level <= pair[1].safety_level);
        }
    }
}
