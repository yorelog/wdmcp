# browsectl Agent Layer — Design Document

> Intelligence layer bridging natural language intent and WebDriver automation.

**Status:** Draft  
**Version:** 0.1.0  
**Applies to:** browsectl ≥ 0.2.0

---

## Table of Contents

1. [Motivation](#motivation)
2. [Architecture Overview](#architecture-overview)
3. [DOM Slot Extraction](#dom-slot-extraction)
4. [Safety Classification](#safety-classification)
5. [Task Recommendation](#task-recommendation)
6. [Intent → Plan Compilation](#intent--plan-compilation)
7. [Memory Store](#memory-store)
8. [MCP Tool Surface](#mcp-tool-surface)
9. [Rust Implementation Sketch](#rust-implementation-sketch)
10. [Integration Points](#integration-points)
11. [Flow Examples](#flow-examples)
12. [Security Considerations](#security-considerations)
13. [Open Questions](#open-questions)

---

## Motivation

Today browsectl exposes low-level WebDriver primitives — `click`, `fill`,
`screenshot`, `scroll`, etc. — as MCP tools. An LLM calling these tools must
already know **which** CSS selector to target and **what** sequence of
commands to issue. This works, but it pushes all the burden of page
understanding onto the model.

The Agent layer solves three problems:

1. **Discovery** — "What can I do on this page?" requires parsing the DOM for
   interactive elements, not guessing selectors.
2. **Safety** — A purchase button and a scroll action carry vastly different
   risk. The agent must classify actions and gate irreversible ones.
3. **Planning** — Converting "帮我登录" into a concrete `CommandSpec` batch
   requires structural understanding of the page, not just selector knowledge.

```
/dev/null/diagram.txt#L1-3
  Before:   LLM ──raw selectors──▶ browsectl ──WebDriver──▶ Browser
  After:    LLM ──intent──▶ Agent ──plan──▶ browsectl ──WebDriver──▶ Browser
```

---

## Architecture Overview

```
/dev/null/architecture.txt#L1-34
┌─────────────────────────────────────────────────────────────────────┐
│                          LLM (via MCP)                              │
│                                                                     │
│  "搜索最新手机"          "帮我登录"          "这个页面能做什么？"       │
└──────────┬───────────────────┬──────────────────────┬───────────────┘
           │                   │                      │
           ▼                   ▼                      ▼
┌─────────────────────────────────────────────────────────────────────┐
│                        AGENT LAYER (new)                            │
│                                                                     │
│  ┌──────────────┐  ┌──────────────┐  ┌───────────────────────────┐  │
│  │  Slot        │  │  Safety      │  │  Intent → Plan            │  │
│  │  Extractor   │  │  Classifier  │  │  Compiler                 │  │
│  │              │  │              │  │                           │  │
│  │  DOM → Slots │  │  Slot →      │  │  NL + Slots → CommandSpec │  │
│  │  (JS inject) │  │  SafetyLevel │  │  batch                   │  │
│  └──────┬───────┘  └──────┬───────┘  └─────────┬─────────────────┘  │
│         │                 │                    │                    │
│  ┌──────┴─────────────────┴────────────────────┴──────────────┐     │
│  │                    Memory Store                             │     │
│  │            .browsectl/memory.json                           │     │
│  └─────────────────────────────────────────────────────────────┘     │
└──────────┬──────────────────────────────────────────────────────────┘
           │
           ▼
┌─────────────────────────────────────────────────────────────────────┐
│                   EXECUTION LAYER (existing)                        │
│                                                                     │
│  ┌────────────┐  ┌────────────┐  ┌────────────┐  ┌──────────────┐  │
│  │  batch.rs   │  │ commands.rs│  │ manager.rs │  │ webdriver.rs │  │
│  │            │  │            │  │            │  │              │  │
│  │ execute_   │  │ execute_   │  │ tab mgmt   │  │ WdClient     │  │
│  │ batch()    │  │ single_    │  │            │  │ WdElement    │  │
│  │            │  │ command()  │  │            │  │              │  │
│  └────────────┘  └────────────┘  └────────────┘  └──────────────┘  │
└──────────┬──────────────────────────────────────────────────────────┘
           │
           ▼
┌─────────────────────────────────────────────────────────────────────┐
│                   WebDriver Protocol (ChromeDriver)                 │
└─────────────────────────────────────────────────────────────────────┘
```

### Component Responsibilities

| Component | Input | Output | Rust Module |
|---|---|---|---|
| **Slot Extractor** | Live DOM (via JS injection) | `Vec<PageSlot>` | `src/agent/slots.rs` |
| **Safety Classifier** | `PageSlot` | `SafetyLevel` enum | `src/agent/safety.rs` |
| **Task Recommender** | `Vec<PageSlot>` + Memory | Suggested actions (NL) | `src/agent/recommend.rs` |
| **Plan Compiler** | NL intent + `Vec<PageSlot>` | `Vec<CommandSpec>` | `src/agent/plan.rs` |
| **Memory Store** | Task outcomes, page visits | Persisted patterns | `src/agent/memory.rs` |

---

## DOM Slot Extraction

A **slot** is an interactive element on the page that a user (or agent) can
act upon. The extractor injects JavaScript via `WdClient::execute()` to walk
the DOM and produce a structured inventory.

### PageSlot Schema

```
/dev/null/slot-schema.json#L1-27
{
  "slotId": "s-0",
  "tag": "button",
  "type": "submit",
  "role": "button",
  "text": "购买",
  "placeholder": null,
  "selector": "button[data-testid='buy-btn']",
  "xpath": "/html/body/form/button[2]",
  "safetyLevel": "submit",
  "category": "form-action",
  "visible": true,
  "enabled": true,
  "rect": {
    "x": 320,
    "y": 580,
    "width": 120,
    "height": 40
  },
  "metadata": {
    "form": "checkout-form",
    "ariaLabel": "确认购买",
    "name": "buy",
    "value": "confirm",
    "action": "https://shop.example.com/checkout"
  }
}
```

### Slot Categories

| Category | Tags / Patterns | Examples |
|---|---|---|
| `text-input` | `input[type=text\|email\|password\|search\|tel\|url]`, `textarea` | Search box, login fields |
| `select` | `select`, `[role=listbox]`, `[role=combobox]` | Dropdown menus |
| `toggle` | `input[type=checkbox\|radio]`, `[role=switch]` | Preferences, filters |
| `button` | `button`, `input[type=button]`, `[role=button]` | Generic clickable actions |
| `form-action` | `input[type=submit]`, `button[type=submit]` | Form submission triggers |
| `link` | `a[href]`, `[role=link]` | Navigation links |
| `media-control` | `video`, `audio`, `[role=slider]` | Play/pause, volume, seek |
| `file-input` | `input[type=file]` | Upload fields |
| `rich-editor` | `[contenteditable=true]`, `[role=textbox]` | WYSIWYG editors, chat inputs |

### Extraction JavaScript

The injected script targets all interactive elements and builds the slot
array. It runs in the page context via the WebDriver `/execute/sync` endpoint
(`WdClient::execute()`).

```
/dev/null/extract-slots.js#L1-102
(function () {
  const INTERACTIVE = [
    'a[href]',
    'button',
    'input:not([type=hidden])',
    'textarea',
    'select',
    '[role=button]',
    '[role=link]',
    '[role=textbox]',
    '[role=combobox]',
    '[role=listbox]',
    '[role=switch]',
    '[role=slider]',
    '[contenteditable=true]',
    '[tabindex]:not([tabindex="-1"])',
  ].join(',');

  const els = document.querySelectorAll(INTERACTIVE);
  const slots = [];

  for (let i = 0; i < els.length; i++) {
    const el = els[i];
    const rect = el.getBoundingClientRect();

    // Skip invisible / zero-size elements
    if (rect.width === 0 && rect.height === 0) continue;
    const style = window.getComputedStyle(el);
    if (style.display === 'none' || style.visibility === 'hidden') continue;

    const tag = el.tagName.toLowerCase();
    const type = el.getAttribute('type') || null;
    const role = el.getAttribute('role') || null;
    const text = (el.textContent || '').trim().substring(0, 120);
    const placeholder = el.getAttribute('placeholder') || null;
    const ariaLabel = el.getAttribute('aria-label') || null;
    const name = el.getAttribute('name') || null;
    const value = el.value !== undefined ? el.value : null;
    const href = el.getAttribute('href') || null;

    // Build a reasonably unique CSS selector
    let selector = tag;
    const id = el.getAttribute('id');
    const testId = el.getAttribute('data-testid');
    if (testId) {
      selector = tag + "[data-testid='" + testId + "']";
    } else if (id) {
      selector = '#' + CSS.escape(id);
    } else if (name) {
      selector = tag + "[name='" + CSS.escape(name) + "']";
    } else if (ariaLabel) {
      selector = tag + "[aria-label='" + CSS.escape(ariaLabel) + "']";
    }

    // Determine category
    let category = 'button';
    if (tag === 'a') category = 'link';
    else if (tag === 'textarea' || (tag === 'input' && /^(text|email|password|search|tel|url)$/i.test(type || 'text')))
      category = 'text-input';
    else if (tag === 'select' || role === 'listbox' || role === 'combobox')
      category = 'select';
    else if ((tag === 'input' && /^(checkbox|radio)$/i.test(type)) || role === 'switch')
      category = 'toggle';
    else if (type === 'submit' || (tag === 'button' && el.closest('form') && type !== 'button'))
      category = 'form-action';
    else if (type === 'file')
      category = 'file-input';
    else if (el.getAttribute('contenteditable') === 'true' || role === 'textbox')
      category = 'rich-editor';

    // Determine the closest form (if any)
    const form = el.closest('form');
    const formId = form ? (form.getAttribute('id') || form.getAttribute('name') || null) : null;
    const formAction = form ? form.getAttribute('action') || null : null;

    const metadata = {};
    if (formId) metadata.form = formId;
    if (formAction) metadata.action = formAction;
    if (ariaLabel) metadata.ariaLabel = ariaLabel;
    if (name) metadata.name = name;
    if (value) metadata.value = value;
    if (href) metadata.href = href;
    if (placeholder) metadata.placeholder = placeholder;

    slots.push({
      slotId: 's-' + i,
      tag: tag,
      type: type,
      role: role,
      text: text,
      placeholder: placeholder,
      selector: selector,
      safetyLevel: null, // classified server-side
      category: category,
      visible: true,
      enabled: !el.disabled,
      rect: { x: rect.x, y: rect.y, width: rect.width, height: rect.height },
      metadata: metadata,
    });
  }

  return slots;
})();
```

### Selector Robustness Strategy

The extractor produces selectors with a priority chain for stability:

```
/dev/null/selector-priority.txt#L1-6
1. data-testid  →  button[data-testid='buy-btn']        (most stable)
2. id           →  #checkout-submit                      (usually stable)
3. name         →  input[name='username']                (form-stable)
4. aria-label   →  button[aria-label='确认购买']          (a11y-stable)
5. tag + index  →  form > button:nth-of-type(2)          (fragile, last resort)
6. ::text()     →  button::text(/购买/)                   (browsectl extension)
```

When the top-priority attributes are unavailable, the extractor falls back to
browsectl's `::text(/regex/)` custom selector syntax for text-based matching.

---

## Safety Classification

Every slot — and every action targeting that slot — receives a **safety
level**. This is the core guardrail that prevents the agent from performing
destructive operations without explicit confirmation.

### Safety Levels

```
/dev/null/safety-levels.txt#L1-18
Level        Emoji   Gate            Description
──────────── ─────── ─────────────── ─────────────────────────────────────
observe      🟢      auto-approve    Read-only. No page mutation.
                                     screenshot, title, scroll, read text

navigate     🟡      auto-approve    Reversible page transitions.
                                     click links, open URLs, switch tabs,
                                     back/forward

interact     🟠      auto-approve*   Modifiable state. Can be undone by
                                     clearing/retyping.
                                     fill inputs, select options, toggle
                                     checkboxes
                                     *first interaction auto; bulk needs ack

submit       🔴      CONFIRM         Irreversible or costly side-effects.
                                     form submit, purchase, delete account,
                                     post content, send message
```

### Classification Rules

Safety level is determined by a combination of slot category, element
attributes, and textual signals:

```
/dev/null/safety-rules.txt#L1-38
RULE 1: Command-type override (highest priority)
  screenshot, title, scroll, last-message-content  →  observe
  open, tab-create, tab-switch, tab-close           →  navigate
  fill, paste                                       →  interact

RULE 2: Slot-category mapping
  link                →  navigate
  text-input          →  interact
  select              →  interact
  toggle              →  interact
  rich-editor         →  interact
  file-input          →  interact
  media-control       →  interact
  button              →  navigate   (default, unless escalated)
  form-action         →  submit     (default for submit buttons)

RULE 3: Text-signal escalation (case-insensitive, multilingual)
  Escalate to submit if text matches any of:
    EN: buy, purchase, order, pay, checkout, delete, remove,
        confirm, submit, send, post, publish, subscribe, unsubscribe
    ZH: 购买, 下单, 付款, 支付, 删除, 移除, 确认, 提交,
        发送, 发布, 订阅, 取消订阅
    JA: 購入, 注文, 支払い, 削除, 確認, 送信, 投稿

RULE 4: Attribute-signal escalation
  Escalate to submit if:
    - <form> has action containing /checkout|payment|delete|order/
    - Element has data-destructive="true"
    - Element has aria-label matching Rule 3 patterns

RULE 5: URL-context escalation
  Escalate to submit if current URL contains:
    /checkout|payment|order|settings.*delete/
```

### Confirmation Protocol

When a plan includes `submit`-level actions, the agent **must** pause and
present a confirmation prompt to the LLM (which relays it to the user):

```
/dev/null/confirmation.json#L1-16
{
  "confirmationRequired": true,
  "safetyLevel": "submit",
  "summary": "即将提交结账表单",
  "actions": [
    {
      "step": 3,
      "command": "click",
      "selector": "button[data-testid='buy-btn']",
      "description": "点击"购买"按钮 — 将完成订单并扣款",
      "safetyLevel": "submit"
    }
  ],
  "prompt": "This plan includes irreversible actions. Proceed? [y/N]"
}
```

---

## Task Recommendation

When the user hasn't expressed a specific intent — or explicitly asks "what
can I do here?" — the agent analyzes extracted slots and memory to suggest
meaningful tasks.

### Recommendation Pipeline

```
/dev/null/recommend-pipeline.txt#L1-15
                    ┌─────────────┐
                    │ Extracted   │
                    │ Slots       │
                    └──────┬──────┘
                           │
                    ┌──────▼──────┐     ┌─────────────┐
                    │  Cluster    │◄────│  Memory     │
                    │  & Rank     │     │  (patterns) │
                    └──────┬──────┘     └─────────────┘
                           │
                    ┌──────▼──────┐
                    │  Generate   │
                    │  NL summary │
                    └──────┬──────┘
                           │
                           ▼
              "You can: login, search products,
               view cart (3 items), change language"
```

### Clustering Strategy

Slots are clustered into **task groups** by proximity and semantic role:

| Signal | Grouping Logic |
|---|---|
| **Form membership** | Slots sharing the same `<form>` ancestor form one group |
| **Spatial proximity** | Slots within 100px vertical distance are candidates |
| **ARIA landmarks** | `<nav>`, `<header>`, `<main>`, `<aside>`, `<footer>` |
| **Semantic role** | Login fields + submit = "login task" |
| **Memory match** | URL pattern seen before → recall what user did last time |

### Recommendation Output

```
/dev/null/recommendation.json#L1-33
{
  "url": "https://shop.example.com/",
  "title": "Example Shop — Home",
  "slotCount": 24,
  "taskGroups": [
    {
      "id": "tg-0",
      "label": "Search products",
      "confidence": 0.95,
      "slots": ["s-3"],
      "suggestedIntent": "搜索商品"
    },
    {
      "id": "tg-1",
      "label": "Login / Register",
      "confidence": 0.90,
      "slots": ["s-7", "s-8"],
      "suggestedIntent": "登录或注册账户"
    },
    {
      "id": "tg-2",
      "label": "Browse categories",
      "confidence": 0.85,
      "slots": ["s-10", "s-11", "s-12", "s-13", "s-14"],
      "suggestedIntent": "浏览商品分类"
    },
    {
      "id": "tg-3",
      "label": "View cart",
      "confidence": 0.80,
      "slots": ["s-20"],
      "suggestedIntent": "查看购物车"
    }
  ]
}
```

---

## Intent → Plan Compilation

The plan compiler converts a natural-language intent plus the current page's
slot inventory into an executable `Vec<CommandSpec>` batch.

### Compilation Pipeline

```
/dev/null/plan-pipeline.txt#L1-25
   ┌──────────────────┐
   │  NL Intent       │   "帮我登录"
   └────────┬─────────┘
            │
   ┌────────▼─────────┐
   │  Slot Matching    │   Find slots relevant to "login":
   │                   │   s-7 (input[name='username'])
   │                   │   s-8 (input[name='password'])
   │                   │   s-9 (button "登录")
   └────────┬─────────┘
            │
   ┌────────▼─────────┐
   │  Plan Assembly    │   Sequence the matched slots into
   │                   │   CommandSpec objects with correct
   │                   │   types, waits, and dependencies
   └────────┬─────────┘
            │
   ┌────────▼─────────┐
   │  Safety Gate      │   Classify each step, insert
   │                   │   confirmation if needed
   └────────┬─────────┘
            │
            ▼
   Vec<CommandSpec>  (ready for batch::execute_batch)
```

### Plan Output Example: Login

Intent: `"帮我登录"` (Help me log in)

```
/dev/null/plan-login.json#L1-43
{
  "intent": "帮我登录",
  "plan": {
    "name": "login",
    "description": "Log into the website using provided credentials",
    "continueOnError": false,
    "commands": [
      {
        "type": "click",
        "selector": "input[name='username']",
        "continueOnError": false,
        "_safety": "interact",
        "_slotId": "s-7",
        "_description": "Focus the username field"
      },
      {
        "type": "fill",
        "selector": "input[name='username']",
        "text": "{{username}}",
        "_safety": "interact",
        "_slotId": "s-7",
        "_description": "Type username"
      },
      {
        "type": "fill",
        "selector": "input[name='password']",
        "text": "{{password}}",
        "_safety": "interact",
        "_slotId": "s-8",
        "_description": "Type password"
      },
      {
        "type": "click",
        "selector": "button::text(/登录/i)",
        "_safety": "submit",
        "_slotId": "s-9",
        "_description": "Click the login button"
      },
      {
        "type": "wait",
        "condition": "url",
        "value": "/dashboard|/home|/account/",
        "timeout": 10000,
        "_safety": "observe",
        "_description": "Wait for redirect after login"
      }
    ]
  },
  "confirmationRequired": true,
  "pendingInputs": ["username", "password"]
}
```

> **Note:** Fields prefixed with `_` are metadata annotations stripped before
> execution. They exist for traceability and LLM context. The `pendingInputs`
> array tells the LLM which `{{template}}` variables need user values.

### Plan Output Example: Search

Intent: `"搜索最新手机"` (Search for latest phones)

```
/dev/null/plan-search.json#L1-29
{
  "intent": "搜索最新手机",
  "plan": {
    "name": "search",
    "description": "Search for '最新手机' using the site search",
    "continueOnError": false,
    "commands": [
      {
        "type": "click",
        "selector": "input[name='q']",
        "_safety": "interact",
        "_slotId": "s-3",
        "_description": "Focus the search input"
      },
      {
        "type": "paste",
        "selector": "input[name='q']",
        "text": "最新手机",
        "_safety": "interact",
        "_slotId": "s-3",
        "_description": "Enter search query"
      },
      {
        "type": "click",
        "selector": "button[aria-label='Search']",
        "_safety": "navigate",
        "_slotId": "s-4",
        "_description": "Submit search"
      },
      {
        "type": "wait",
        "condition": "visible",
        "selector": ".search-results",
        "timeout": 10000,
        "_safety": "observe",
        "_description": "Wait for results to load"
      }
    ]
  },
  "confirmationRequired": false,
  "pendingInputs": []
}
```

### Template Variables

Plans may contain `{{variable}}` placeholders when user input is needed.
The LLM is responsible for collecting these from the user and substituting
them before the plan is dispatched to `execute_batch`.

---

## Memory Store

The memory system enables the agent to learn from past interactions,
recommend actions based on history, and avoid repeating mistakes.

### Storage Location

```
/dev/null/memory-path.txt#L1-5
~/.browsectl/
├── sessions.json        (existing — session persistence)
├── setup.json           (existing — browser/driver config)
└── memory.json          (new — agent memory store)
```

This follows the established `.browsectl/` persistence pattern used by
`store.rs` for session data and `setup.rs` for platform config.

### Memory Schema

```
/dev/null/memory-schema.json#L1-62
{
  "version": 1,
  "updatedAt": "2025-07-18T12:00:00Z",
  "pageVisits": [
    {
      "urlPattern": "https://shop.example.com/**",
      "lastVisit": "2025-07-18T11:55:00Z",
      "visitCount": 12,
      "title": "Example Shop",
      "knownSlots": ["search-input", "login-link", "cart-icon"],
      "commonTasks": ["search", "login", "view-cart"]
    }
  ],
  "taskPatterns": [
    {
      "patternId": "tp-0",
      "intent": "login",
      "urlPattern": "https://shop.example.com/login",
      "steps": [
        { "type": "fill", "slotCategory": "text-input", "slotName": "username" },
        { "type": "fill", "slotCategory": "text-input", "slotName": "password" },
        { "type": "click", "slotCategory": "form-action", "text": "/登录|login|sign.?in/i" }
      ],
      "successCount": 5,
      "failCount": 0,
      "lastUsed": "2025-07-18T11:50:00Z",
      "averageDurationMs": 3200
    }
  ],
  "userPreferences": {
    "language": "zh-CN",
    "defaultConfirmSubmit": false,
    "autoFillPatterns": {
      "username": "user@example.com"
    },
    "trustedDomains": [
      "shop.example.com",
      "mail.example.com"
    ],
    "blockedDomains": []
  },
  "errors": [
    {
      "timestamp": "2025-07-18T10:30:00Z",
      "url": "https://shop.example.com/checkout",
      "intent": "checkout",
      "failedStep": 2,
      "error": "element not interactable: button[data-testid='pay-btn']",
      "resolution": "Added scroll_into_view before click"
    }
  ]
}
```

### Memory Operations

| Operation | Trigger | Effect |
|---|---|---|
| **Record visit** | `analyze_page` called | Upsert URL pattern in `pageVisits` |
| **Record task** | Plan executed successfully | Upsert pattern in `taskPatterns`, increment `successCount` |
| **Record failure** | Plan step fails | Append to `errors`, increment `failCount` on pattern |
| **Recall patterns** | `suggest_actions` called | Query `taskPatterns` matching current URL |
| **Preference update** | User explicitly sets preference | Update `userPreferences` |
| **Prune** | On write, if entries > 500 | Drop least-recent entries to keep file bounded |

### Memory Size Bounds

To keep the JSON file manageable:

- `pageVisits`: max 200 entries, LRU eviction
- `taskPatterns`: max 100 entries, ranked by `successCount`
- `errors`: max 50 entries, FIFO

---

## MCP Tool Surface

Two new MCP tools are added alongside the existing tool definitions in
`mcp.rs`. They integrate with the same `tool_definitions()` registry.

### `analyze_page`

Extracts all interactive slots from the current page with safety classification.

```
/dev/null/analyze-page-tool.json#L1-34
{
  "name": "analyze_page",
  "description": "Extract all interactive slots from current page with safety classification. Returns structured data about buttons, inputs, links, forms, and other interactive elements.",
  "inputSchema": {
    "type": "object",
    "properties": {
      "sessionId": {
        "type": "string",
        "description": "Target session. Defaults to the current default session."
      },
      "categories": {
        "type": "array",
        "items": {
          "type": "string",
          "enum": [
            "text-input", "select", "toggle", "button",
            "form-action", "link", "media-control",
            "file-input", "rich-editor"
          ]
        },
        "description": "Filter slots by category. Omit to return all categories."
      },
      "visibleOnly": {
        "type": "boolean",
        "description": "Only return slots currently visible in viewport. Default: true."
      },
      "limit": {
        "type": "integer",
        "description": "Maximum number of slots to return. Default: 50."
      }
    },
    "required": [],
    "additionalProperties": false
  }
}
```

**Response:**

```
/dev/null/analyze-page-response.json#L1-26
{
  "url": "https://shop.example.com/",
  "title": "Example Shop — Home",
  "slotCount": 24,
  "slots": [
    {
      "slotId": "s-0",
      "tag": "input",
      "type": "search",
      "text": "",
      "placeholder": "搜索商品...",
      "selector": "input[name='q']",
      "safetyLevel": "interact",
      "category": "text-input",
      "visible": true,
      "enabled": true,
      "metadata": { "name": "q", "ariaLabel": "搜索", "form": "search-form" }
    }
  ],
  "safetySummary": {
    "observe": 0,
    "navigate": 12,
    "interact": 8,
    "submit": 4
  }
}
```

### `suggest_actions`

Combines DOM analysis with memory to recommend actions for the current page.

```
/dev/null/suggest-actions-tool.json#L1-22
{
  "name": "suggest_actions",
  "description": "Get recommended actions for the current page. Analyzes DOM slots and combines with memory of past interactions to suggest what the user can do.",
  "inputSchema": {
    "type": "object",
    "properties": {
      "sessionId": {
        "type": "string",
        "description": "Target session. Defaults to the current default session."
      },
      "maxSuggestions": {
        "type": "integer",
        "description": "Maximum number of task suggestions. Default: 5."
      },
      "includeSlots": {
        "type": "boolean",
        "description": "Include raw slot data alongside suggestions. Default: false."
      }
    },
    "required": [],
    "additionalProperties": false
  }
}
```

**Response:**

```
/dev/null/suggest-actions-response.json#L1-42
{
  "url": "https://shop.example.com/",
  "title": "Example Shop — Home",
  "suggestions": [
    {
      "id": "tg-0",
      "label": "Search products",
      "description": "Use the search bar to find products",
      "confidence": 0.95,
      "slots": ["s-3"],
      "exampleIntent": "搜索最新手机",
      "fromMemory": false
    },
    {
      "id": "tg-1",
      "label": "Login",
      "description": "Log into your account (last login: 2 days ago)",
      "confidence": 0.90,
      "slots": ["s-7", "s-8"],
      "exampleIntent": "帮我登录",
      "fromMemory": true
    },
    {
      "id": "tg-2",
      "label": "Browse categories",
      "description": "Navigate product categories: Electronics, Clothing, Home",
      "confidence": 0.85,
      "slots": ["s-10", "s-11", "s-12", "s-13", "s-14"],
      "exampleIntent": "浏览电子产品分类",
      "fromMemory": false
    }
  ],
  "pageContext": {
    "hasLoginForm": false,
    "hasSearchBar": true,
    "hasCart": true,
    "formCount": 1,
    "linkCount": 18,
    "inputCount": 3
  }
}
```

---

## Rust Implementation Sketch

### Module Structure

```
/dev/null/module-tree.txt#L1-11
src/
├── agent/
│   ├── mod.rs           // pub mod declarations
│   ├── slots.rs         // PageSlot, SlotCategory, extract_slots()
│   ├── safety.rs        // SafetyLevel, classify_slot(), classify_command()
│   ├── recommend.rs     // TaskGroup, suggest_actions()
│   ├── plan.rs          // compile_plan(), PlanOutput
│   └── memory.rs        // MemoryStore, read_memory(), write_memory()
├── main.rs              // add `mod agent;`
├── mcp.rs               // register analyze_page, suggest_actions tools
└── ...existing modules...
```

### Core Types

```
/dev/null/agent-types.rs#L1-97
use serde::{Deserialize, Serialize};

// ── Safety ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SafetyLevel {
    Observe,
    Navigate,
    Interact,
    Submit,
}

impl SafetyLevel {
    pub fn emoji(&self) -> &'static str {
        match self {
            Self::Observe  => "🟢",
            Self::Navigate => "🟡",
            Self::Interact => "🟠",
            Self::Submit   => "🔴",
        }
    }

    pub fn requires_confirmation(&self) -> bool {
        matches!(self, Self::Submit)
    }
}

// ── Slots ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SlotCategory {
    TextInput,
    Select,
    Toggle,
    Button,
    FormAction,
    Link,
    MediaControl,
    FileInput,
    RichEditor,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SlotRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageSlot {
    pub slot_id: String,
    pub tag: String,
    #[serde(rename = "type")]
    pub input_type: Option<String>,
    pub role: Option<String>,
    pub text: String,
    pub placeholder: Option<String>,
    pub selector: String,
    pub safety_level: SafetyLevel,
    pub category: SlotCategory,
    pub visible: bool,
    pub enabled: bool,
    pub rect: SlotRect,
    pub metadata: serde_json::Value,
}

// ── Recommendations ────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskGroup {
    pub id: String,
    pub label: String,
    pub description: String,
    pub confidence: f64,
    pub slots: Vec<String>,
    pub example_intent: String,
    pub from_memory: bool,
}

// ── Plan Output ────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanOutput {
    pub intent: String,
    pub plan: crate::types::NamedBatch,
    pub confirmation_required: bool,
    pub pending_inputs: Vec<String>,
    pub safety_summary: std::collections::HashMap<String, usize>,
}
```

### Slot Extraction Function

```
/dev/null/slots-rs.rs#L1-30
use anyhow::Result;
use crate::webdriver::WdClient;
use super::types::{PageSlot, SafetyLevel};
use super::safety::classify_slot;

const EXTRACT_SLOTS_JS: &str = include_str!("extract_slots.js");

/// Inject JavaScript into the page to extract all interactive slots,
/// then classify each slot's safety level server-side.
pub async fn extract_slots(
    client: &WdClient,
    visible_only: bool,
    limit: usize,
) -> Result<Vec<PageSlot>> {
    let raw = client.execute(EXTRACT_SLOTS_JS, vec![]).await?;

    let mut slots: Vec<PageSlot> = serde_json::from_value(raw)?;

    // Server-side safety classification
    for slot in &mut slots {
        slot.safety_level = classify_slot(slot);
    }

    if visible_only {
        slots.retain(|s| s.visible);
    }

    slots.truncate(limit);
    Ok(slots)
}
```

### Safety Classification Function

```
/dev/null/safety-rs.rs#L1-56
use super::types::{PageSlot, SafetyLevel, SlotCategory};
use regex::Regex;
use std::sync::LazyLock;

static SUBMIT_TEXT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(buy|purchase|order|pay|checkout|delete|remove|confirm|submit|send|post|publish)\b|购买|下单|付款|支付|删除|移除|确认|提交|发送|发布|購入|注文|支払い|削除|確認|送信|投稿"
    ).unwrap()
});

static SUBMIT_ACTION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(checkout|payment|delete|order|purchase)").unwrap()
});

/// Classify a single slot's safety level based on category, text,
/// and metadata signals.
pub fn classify_slot(slot: &PageSlot) -> SafetyLevel {
    // Rule 2: Category-based default
    let base = match slot.category {
        SlotCategory::Link         => SafetyLevel::Navigate,
        SlotCategory::TextInput    => SafetyLevel::Interact,
        SlotCategory::Select       => SafetyLevel::Interact,
        SlotCategory::Toggle       => SafetyLevel::Interact,
        SlotCategory::RichEditor   => SafetyLevel::Interact,
        SlotCategory::FileInput    => SafetyLevel::Interact,
        SlotCategory::MediaControl => SafetyLevel::Interact,
        SlotCategory::Button       => SafetyLevel::Navigate,
        SlotCategory::FormAction   => SafetyLevel::Submit,
    };

    // Rule 3: Text-signal escalation
    if SUBMIT_TEXT.is_match(&slot.text) {
        return SafetyLevel::Submit;
    }

    // Check aria-label too
    if let Some(aria) = slot.metadata.get("ariaLabel").and_then(|v| v.as_str()) {
        if SUBMIT_TEXT.is_match(aria) {
            return SafetyLevel::Submit;
        }
    }

    // Rule 4: Form action escalation
    if let Some(action) = slot.metadata.get("action").and_then(|v| v.as_str()) {
        if SUBMIT_ACTION.is_match(action) {
            return SafetyLevel::Submit;
        }
    }

    base
}
```

---

## Integration Points

### How This Connects to Existing Code

```
/dev/null/integration.txt#L1-30
┌─────────────────────────────────────────────────────────────────────┐
│                    Existing browsectl Code                           │
│                                                                     │
│  mcp.rs                                                             │
│  ├── tool_definitions()  ← ADD agent_tool_defs()                    │
│  ├── handle_tool_call()  ← ADD match arms for analyze_page,         │
│  │                         suggest_actions                          │
│  └── build_command_spec()  (unchanged — plans produce CommandSpec)   │
│                                                                     │
│  webdriver.rs                                                       │
│  └── WdClient::execute()  ← Used by slot extractor to inject JS    │
│                                                                     │
│  batch.rs                                                           │
│  └── execute_batch()  ← Plans compile to Vec<CommandSpec>,          │
│                         executed through existing batch engine       │
│                                                                     │
│  types.rs                                                           │
│  └── CommandSpec  ← Plans produce these directly; no changes needed │
│                                                                     │
│  store.rs                                                           │
│  └── read_store() / write_store()  ← Memory store follows same      │
│      pattern with read_memory() / write_memory()                    │
│                                                                     │
│  types.rs                                                           │
│  └── session_store_path()  ← memory_store_path() follows same       │
│      convention: .browsectl/memory.json                             │
└─────────────────────────────────────────────────────────────────────┘
```

### Key Integration Details

1. **JS Injection** — `extract_slots()` calls `WdClient::execute()` with the
   extraction script. This is the same mechanism used by `commands.rs` for
   `wait` conditions and `js_click` fallbacks.

2. **CommandSpec Compatibility** — Plans compile down to `Vec<CommandSpec>`,
   the same struct used by `batch.rs`. No new execution infrastructure is
   needed. The `_`-prefixed metadata fields are ignored by serde
   deserialization (unknown fields are skipped with `#[serde(deny_unknown_fields)]`
   not being set).

3. **MCP Registration** — New tools are added via an `agent_tool_defs()`
   function called from `tool_definitions()`, following the pattern of
   `session_tool_defs()`, `browser_command_tool_defs()`, etc.

4. **Memory Persistence** — `memory.rs` mirrors the pattern in `store.rs`:
   - `read_memory() -> Result<MemoryData>`
   - `write_memory(&MemoryData) -> Result<()>`
   - File path: `.browsectl/memory.json`
   - Graceful fallback to empty on parse error

5. **Session Resolution** — Both new tools accept an optional `sessionId`,
   resolved through the existing `resolve_sid()` helper in `mcp.rs`.

---

## Flow Examples

### Flow 1: Page Discovery

```
/dev/null/flow-discovery.txt#L1-22
User: "这个页面能做什么？"
  │
  ▼
LLM calls: suggest_actions { includeSlots: true }
  │
  ▼
Agent:
  1. resolve_sid() → get active session
  2. WdClient::execute(EXTRACT_SLOTS_JS) → raw slots JSON
  3. Deserialize into Vec<PageSlot>
  4. classify_slot() on each → safety levels assigned
  5. Cluster slots into TaskGroups
  6. Query memory for URL pattern matches
  7. Rank by confidence (memory-backed patterns rank higher)
  8. Return structured response
  │
  ▼
LLM → User: "当前页面可以：
  🔍 搜索商品 — 页面顶部有搜索栏
  👤 登录账户 — 右上角有登录链接
  🛒 查看购物车 — 购物车图标显示有 3 件商品
  📂 浏览分类 — 电子产品、服装、家居等分类导航"
```

### Flow 2: Login Task

```
/dev/null/flow-login.txt#L1-33
User: "帮我登录"
  │
  ▼
LLM calls: analyze_page { categories: ["text-input", "form-action"] }
  │
  ▼
Agent returns:
  s-7: input[name='username']   (text-input, 🟠 interact)
  s-8: input[name='password']   (text-input, 🟠 interact)
  s-9: button "登录"             (form-action, 🔴 submit)
  │
  ▼
LLM recognizes login form, asks user for credentials
  │
  ▼
User: "用户名 alice，密码 hunter2"
  │
  ▼
LLM calls: run_batch { commands: [
  { type: "click",  selector: "input[name='username']" },
  { type: "fill",   selector: "input[name='username']", text: "alice" },
  { type: "fill",   selector: "input[name='password']", text: "hunter2" },
  { type: "click",  selector: "button::text(/登录/i)" },
  { type: "wait",   condition: "url", value: "/dashboard" }
]}
  │
  ▼
batch::execute_batch() runs each CommandSpec sequentially
  │
  ▼
Agent records successful login pattern in memory.json
```

### Flow 3: Search Task

```
/dev/null/flow-search.txt#L1-23
User: "搜索最新手机"
  │
  ▼
LLM calls: analyze_page { categories: ["text-input"] }
  │
  ▼
Agent returns:
  s-3: input[name='q']  (text-input, 🟠 interact, placeholder: "搜索商品...")
  │
  ▼
LLM calls: run_batch { commands: [
  { type: "click",  selector: "input[name='q']" },
  { type: "paste",  selector: "input[name='q']", text: "最新手机" },
  { type: "click",  selector: "button[aria-label='Search']" },
  { type: "wait",   condition: "visible", selector: ".search-results" }
]}
  │
  ▼
Results page loads
  │
  ▼
LLM calls: analyze_page → extracts product listing slots
LLM → User: "找到以下手机：1. iPhone 16... 2. Galaxy S25..."
```

---

## Security Considerations

### Threat Model

| Threat | Mitigation |
|---|---|
| **Injection via slot text** | Slot text is truncated to 120 chars, HTML-escaped in JSON serialization |
| **Malicious JS in page overriding DOM APIs** | Extraction script uses `querySelectorAll` in page context — inherent risk of untrusted pages. Future: run in isolated world via CDP |
| **Credential leakage in memory** | Memory store **never** records `fill`/`paste` text values. Only structural patterns (slot categories, step types) are stored |
| **Submit without consent** | Safety gate requires LLM to relay confirmation to user for all `🔴 submit` actions |
| **Memory poisoning** | Memory entries are keyed by URL pattern, bounded in size, and pruned automatically |
| **Trusted domain bypass** | `trustedDomains` in preferences only affect recommendation ranking, never bypass safety gates |

### Credential Handling Rules

1. Passwords and sensitive text are **never** written to `memory.json`
2. The `text` field of `fill`/`paste` commands is **not** recorded in task patterns
3. Task patterns store only structural metadata: `{ type, slotCategory, slotName }`
4. The LLM is responsible for collecting and injecting credentials per-session

---

## Open Questions

- [ ] **CDP vs WebDriver for extraction** — Should we use Chrome DevTools
  Protocol's `DOM.getDocument` for richer extraction (shadow DOM, iframes)
  instead of `execute/sync` JavaScript injection?

- [ ] **Slot stability across navigations** — When should we invalidate the
  slot cache? On every navigation? On a timer? Via MutationObserver?

- [ ] **Plan compilation: agent-side vs LLM-side** — Should the plan compiler
  be a Rust function that takes structured intent, or should the LLM
  assemble `CommandSpec` arrays directly using `analyze_page` output as
  context? (Current design supports both — the plan compiler is optional
  infrastructure for when we want to reduce LLM token usage.)

- [ ] **Iframe support** — Slots inside iframes require `switchToFrame`
  before interaction. The extractor needs to recursively enter iframes
  and tag slots with their frame context.

- [ ] **Shadow DOM** — Elements inside closed shadow roots are invisible
  to `querySelectorAll`. CDP's `DOM.getFlattenedDocument` with
  `pierce: true` would solve this but requires a protocol switch.

- [ ] **Memory sync across sessions** — If multiple browser sessions are
  active simultaneously, memory writes could race. A simple file lock
  or last-write-wins policy is needed.

- [ ] **Slot limit scaling** — Pages with hundreds of interactive elements
  (e.g., spreadsheets, dashboards) need smarter filtering. Viewport-based
  extraction or landmark-scoped extraction may be necessary.