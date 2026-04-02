# Agent Intelligence

> DOM slot extraction, safety classification, task recommendation, and network monitoring for AI-driven browser automation.

The agent layer sits between the AI model (via MCP) and the WebDriver execution layer. It provides two complementary capabilities:

1. **DOM Intelligence** — Parses the current page's DOM to extract structured "slot" data — interactive elements with metadata, CSS selectors, and safety classifications — enabling the AI to understand what actions are possible and plan tasks accordingly.
2. **Network Intelligence** — Captures, filters, and inspects HTTP traffic (requests, responses, headers, timing, cookies) just like the browser DevTools Network panel, giving the AI deep visibility into page behavior and API interactions.

## Tools

### DOM & Task Tools

| Tool | Description |
|---|---|
| `analyze_page` | Extract all interactive slots from the current page with safety classification |
| `suggest_actions` | Get recommended actions combining DOM analysis with task memory |

### Network Tools

| Tool | Description |
|---|---|
| `network_enable` | Start capturing network traffic (injects fetch/XHR interceptor + CDP) |
| `network_disable` | Stop capturing and restore original fetch/XHR |
| `network_get_log` | Retrieve captured requests with filtering (URL, method, type, status) |
| `network_get_response_body` | Get the response body of a specific captured request |
| `network_clear_log` | Clear captured entries (reset before a new action) |
| `network_get_resource_timing` | Get Performance API resource timing (no interceptor needed) |
| `network_get_cookies` | Get all cookies via CDP or JavaScript fallback |

---

## `analyze_page`

Parse the current page DOM and return structured data about every interactive element (slot).

### Parameters

| Parameter | Type | Required | Default | Description |
|---|---|---|---|---|
| `sessionId` | string | No | default session | Target browser session |
| `include_hidden` | boolean | No | `false` | Include hidden/invisible elements |
| `include_suggestions` | boolean | No | `true` | Include grouped task suggestions |

### Example

```sh
# Via MCP tool call
analyze_page({ "include_suggestions": true })
```

### Response Schema

```json
{
  "url": "https://example.com/login",
  "title": "Login — Example",
  "slots": [
    {
      "slot_id": "s-0",
      "tag": "input",
      "input_type": "text",
      "text": null,
      "selector": "input[name=\"username\"]",
      "category": "TextInput",
      "safety_level": "Interact",
      "form_id": "login-form",
      "aria_label": "Username",
      "placeholder": "Enter your username",
      "href": null,
      "name": "username",
      "data_testid": null,
      "visible": true,
      "disabled": false,
      "rect": { "x": 100, "y": 200, "width": 300, "height": 40 }
    },
    {
      "slot_id": "s-1",
      "tag": "input",
      "input_type": "password",
      "text": null,
      "selector": "input[name=\"password\"]",
      "category": "PasswordInput",
      "safety_level": "Interact",
      "form_id": "login-form",
      "aria_label": "Password",
      "placeholder": "Enter your password",
      "href": null,
      "name": "password",
      "data_testid": null,
      "visible": true,
      "disabled": false,
      "rect": { "x": 100, "y": 260, "width": 300, "height": 40 }
    },
    {
      "slot_id": "s-2",
      "tag": "button",
      "input_type": "submit",
      "text": "Sign In",
      "selector": "button[data-testid=\"login-btn\"]",
      "category": "FormSubmit",
      "safety_level": "Submit",
      "form_id": "login-form",
      "aria_label": null,
      "placeholder": null,
      "href": null,
      "name": null,
      "data_testid": "login-btn",
      "visible": true,
      "disabled": false,
      "rect": { "x": 100, "y": 320, "width": 300, "height": 44 }
    }
  ],
  "slot_count": 3,
  "safety_summary": {
    "observe": 0,
    "navigate": 0,
    "interact": 2,
    "submit": 1
  },
  "forms": [
    {
      "form_id": "login-form",
      "action": "/api/login",
      "method": "POST",
      "slot_ids": ["s-0", "s-1", "s-2"]
    }
  ],
  "timestamp": "2025-01-15T10:30:00Z",
  "suggestions": [
    {
      "title": "Fill and submit login-form",
      "description": "Complete 3 field(s) in <form action=\"/api/login\"> and submit.",
      "safety_level": "Submit",
      "slot_ids": ["s-0", "s-1", "s-2"],
      "commands": [
        { "type": "fill", "selector": "input[name=\"username\"]", "text": "" },
        { "type": "fill", "selector": "input[name=\"password\"]", "text": "" },
        { "type": "click", "selector": "button[data-testid=\"login-btn\"]" }
      ]
    }
  ]
}
```

---

## `suggest_actions`

Get recommended actions for the current page by combining DOM analysis with task memory. Useful when a user doesn't know what to do on a page.

### Parameters

| Parameter | Type | Required | Default | Description |
|---|---|---|---|---|
| `sessionId` | string | No | default session | Target browser session |
| `max_suggestions` | integer | No | `5` | Maximum number of suggestions to return |

### Example

```sh
# Via MCP tool call
suggest_actions({ "max_suggestions": 3 })
```

### Response Schema

```json
{
  "url": "https://example.com",
  "title": "Example Domain",
  "slot_count": 12,
  "suggestions": [
    {
      "title": "Navigate",
      "description": "5 navigation link(s) available on the page.",
      "safety_level": "Navigate",
      "slot_ids": ["s-0", "s-1", "s-2", "s-3", "s-4"],
      "commands": [
        { "type": "click", "selector": "a[href=\"/about\"]" },
        { "type": "click", "selector": "a[href=\"/products\"]" }
      ]
    },
    {
      "title": "Search",
      "description": "Type into \"Search products...\" and press \"Search\".",
      "safety_level": "Interact",
      "slot_ids": ["s-5", "s-6"],
      "commands": [
        { "type": "fill", "selector": "input[name=\"q\"]", "text": "" },
        { "type": "click", "selector": "button[aria-label=\"Search\"]" }
      ]
    },
    {
      "title": "login (from memory)",
      "description": "Previously successful task (used 3 times)",
      "safety_level": "Interact",
      "slot_ids": [],
      "commands": [
        { "type": "click", "selector": "a[href=\"/login\"]" }
      ]
    }
  ],
  "memory_patterns_found": 1
}
```

---

## Concepts

### Slots

A **slot** is a single interactive DOM element on the page. Each slot has:

- **`slot_id`** — Stable identifier for this analysis run (`s-0`, `s-1`, …)
- **`selector`** — Best CSS selector for targeting via WebDriver, chosen by priority:
  1. `data-testid` attribute (most stable)
  2. `id` attribute
  3. `name` attribute
  4. Unique `aria-label`
  5. Structural path (`tag:nth-of-type(n)` up to 3 levels)
- **`category`** — What kind of element: `Link`, `Button`, `TextInput`, `PasswordInput`, `Checkbox`, `Radio`, `Select`, `Textarea`, `FileUpload`, `FormSubmit`, `ContentEditable`, `Other`
- **`safety_level`** — How risky it is to interact with this element (see below)

### Slot Categories

| Category | Element types |
|---|---|
| `Link` | `<a href="…">` |
| `Button` | `<button>`, `[role="button"]` (not type=submit) |
| `TextInput` | `<input type="text\|email\|tel\|url\|search\|number\|date\|…">` |
| `PasswordInput` | `<input type="password">` |
| `Checkbox` | `<input type="checkbox">` |
| `Radio` | `<input type="radio">` |
| `Select` | `<select>` |
| `Textarea` | `<textarea>` |
| `FileUpload` | `<input type="file">` |
| `FormSubmit` | `<input type="submit">`, `<button type="submit">` |
| `ContentEditable` | `[contenteditable="true"]` |
| `Other` | Anything interactive not matching above |

### Safety Levels

Every slot and suggestion is classified into one of four safety tiers:

| Level | Emoji | Description | Confirmation? |
|---|---|---|---|
| **Observe** | 🟢 | Read-only: screenshots, title, scroll, read text | No |
| **Navigate** | 🟡 | Link clicks, URL changes, tab switches | No |
| **Interact** | 🟠 | Form filling, typing, selecting, toggling | No |
| **Submit** | 🔴 | Form submission, purchase, delete, irreversible actions | **Yes** |

#### Classification Rules

1. **FormSubmit** category → always `Submit`
2. **Link** category → always `Navigate`
3. **Input-like** categories (TextInput, PasswordInput, Textarea, Select, Checkbox, Radio, FileUpload, ContentEditable) → always `Interact`
4. **Button** category → heuristic based on text content:
   - **Submit-like keywords** → `Submit`: `submit`, `purchase`, `buy`, `delete`, `remove`, `confirm`, `checkout`, `pay`, `post`, `发布`, `购买`, `删除`, `确认`, `提交`, `付款`, `下单`, `支付`
   - **Navigate-like keywords** → `Navigate`: `back`, `next`, `more`, `details`, `view`, `查看`, `返回`, `下一步`, `更多`
   - **Otherwise** → `Interact`
5. **Everything else** → `Observe`

### Task Suggestions

Suggestions are higher-level groupings of slots into logical tasks:

- **Form tasks** — One suggestion per `<form>` element, grouping all its child slots into a fill-and-submit sequence
- **Navigation** — All visible links grouped as navigation options
- **Search** — When a text input and button are adjacent (within 3 slots distance) outside a form, suggests a search action
- **Memory patterns** — Previously successful task patterns from `~/.browsectl/memory.json` that match the current URL pattern

Suggestions are sorted safest-first (Observe → Navigate → Interact → Submit).

### Memory

The agent stores data locally at `~/.browsectl/memory.json`:

| Data | Description | Max entries |
|---|---|---|
| **Page visits** | URL, title, slot count, actions taken, timestamp | 200 |
| **Task patterns** | Intent, URL pattern, commands, success count | 100 |
| **User preferences** | Confirmation settings, max suggestions, language | — |

Memory enables the agent to:
- Recommend previously successful tasks when revisiting similar pages
- Learn which URL patterns correspond to which actions
- Rank suggestions by past success

URL patterns are normalized by stripping protocols, query strings, and replacing ID-like path segments with `*` (e.g., `example.com/product/12345` → `example.com/product/*`).

---

## Workflow Examples

### 1. Page Discovery

User opens an unfamiliar page and wants to know what's available.

```
User: "I just opened this page. What can I do here?"

Agent flow:
  1. analyze_page({ "include_suggestions": true })
  2. Receives: 15 slots, 3 forms, 8 links
  3. Suggestions: "Fill and submit search", "Navigate (8 links)", "Fill and submit login-form"
  4. Presents to user: "You can search for content, browse 8 navigation links, or log in."
```

### 2. Intent-Driven Login

User expresses a high-level intent.

```
User: "帮我登录"

Agent flow:
  1. analyze_page() → finds login form with username, password, submit button
  2. Identifies slots: s-3 (username input), s-4 (password input), s-5 (submit button)
  3. Safety check: s-5 is Submit-level → requires confirmation
  4. Asks user: "I found a login form. Please provide your username and password."
  5. User provides credentials
  6. Executes batch:
     - fill({ selector: "input[name='username']", text: "user@example.com" })
     - fill({ selector: "input[name='password']", text: "••••••••" })
     - ⚠️ Confirms with user before: click({ selector: "button[type='submit']" })
  7. record_task(intent: "login", url: current_url, commands: [...])
```

### 3. Search with Memory

User searches on a previously visited site.

```
User: "搜索最新手机"

Agent flow:
  1. suggest_actions() → finds "Search" suggestion + memory pattern (used 5 times)
  2. Memory confirms: search input is input[name="q"], button is button.search-btn
  3. Executes batch:
     - fill({ selector: "input[name='q']", text: "最新手机" })
     - click({ selector: "button.search-btn" })
  4. Updates memory: success_count → 6
```

### 4. Safety Escalation

User triggers a dangerous action.

```
User: "Delete my account"

Agent flow:
  1. analyze_page() → finds "Delete Account" button (s-12)
  2. Classification: text contains "delete" → Submit (🔴)
  3. Agent: "⚠️ This is a destructive action (Submit-level). Are you sure you want to delete your account? This cannot be undone."
  4. User confirms → executes click
  5. User declines → action cancelled
```

---

## Network Monitoring

The network tools give the agent the same visibility as the browser DevTools Network panel — captured HTTP traffic with full request/response details, timing breakdown, and filtering.

### `network_enable`

Start capturing network traffic on the current page. Must be called **before** navigating or performing actions whose network activity you want to inspect.

Internally this:
1. Injects a JavaScript interceptor that wraps `window.fetch` and `XMLHttpRequest` to record all HTTP activity
2. Installs a `PerformanceObserver` for resource timing data
3. Optionally enables the Chrome DevTools Protocol (CDP) `Network` domain for deeper introspection

| Parameter | Type | Required | Default | Description |
|---|---|---|---|---|
| `sessionId` | string | No | default session | Target browser session |

```
network_enable()
→ { "ok": true, "interceptor": "installed", "cdp": "enabled" }
```

> **Note:** The interceptor is per-page. If you navigate to a new page, call `network_enable` again.

### `network_disable`

Stop capturing and restore original `fetch`/`XHR` functions.

| Parameter | Type | Required | Default | Description |
|---|---|---|---|---|
| `sessionId` | string | No | default session | Target browser session |

### `network_get_log`

Retrieve captured network requests with optional filtering.

| Parameter | Type | Required | Default | Description |
|---|---|---|---|---|
| `sessionId` | string | No | default session | Target browser session |
| `url_pattern` | string | No | — | Filter entries whose URL contains this substring |
| `methods` | string[] | No | — | Filter by HTTP methods, e.g. `["GET", "POST"]` |
| `resource_types` | string[] | No | — | Filter by type: `"xhr"`, `"fetch"`, `"script"`, `"document"`, `"image"`, `"stylesheet"` |
| `status_min` | integer | No | — | Minimum HTTP status code (inclusive) |
| `status_max` | integer | No | — | Maximum HTTP status code (inclusive) |
| `has_error` | boolean | No | — | If true, only return failed requests (status ≥ 400 or 0) |
| `limit` | integer | No | — | Maximum number of entries to return |

#### Example: Get all API calls

```
network_get_log({ "url_pattern": "/api/", "methods": ["GET", "POST"] })
```

#### Example: Find failed requests

```
network_get_log({ "has_error": true })
```

#### Response Schema

```json
{
  "entries": [
    {
      "id": "net-0",
      "method": "GET",
      "url": "https://api.example.com/users?page=1",
      "status": 200,
      "status_text": "OK",
      "resource_type": "fetch",
      "request_headers": { "Authorization": "Bearer ..." },
      "response_headers": { "content-type": "application/json" },
      "request_body": null,
      "response_body": null,
      "content_type": "application/json",
      "content_length": 4523,
      "timing": {
        "started_at": 1705312200000.0,
        "duration_ms": 142.5,
        "dns_ms": 2.1,
        "connect_ms": 15.3,
        "ssl_ms": 12.0,
        "ttfb_ms": 98.7,
        "download_ms": 14.4
      },
      "initiator": "fetch",
      "from_cache": false,
      "timestamp": "2025-01-15T10:30:00Z"
    },
    {
      "id": "net-1",
      "method": "POST",
      "url": "https://api.example.com/login",
      "status": 401,
      "status_text": "Unauthorized",
      "resource_type": "fetch",
      "request_headers": { "content-type": "application/json" },
      "response_headers": { "content-type": "application/json" },
      "request_body": "{\"username\":\"test\"}",
      "response_body": null,
      "content_type": "application/json",
      "content_length": 87,
      "timing": null,
      "initiator": "fetch",
      "from_cache": false,
      "timestamp": "2025-01-15T10:30:01Z"
    }
  ],
  "entry_count": 2,
  "summary": {
    "total_requests": 2,
    "by_type": { "fetch": 2 },
    "by_status": { "2xx": 1, "4xx": 1 },
    "total_bytes": 4610,
    "failed_count": 1,
    "cached_count": 0
  },
  "captured_at": "2025-01-15T10:30:05Z"
}
```

### `network_get_response_body`

Retrieve the response body of a specific captured request by its entry ID.

| Parameter | Type | Required | Default | Description |
|---|---|---|---|---|
| `sessionId` | string | No | default session | Target browser session |
| `request_id` | string | **Yes** | — | The entry ID, e.g. `"net-0"` |

```
network_get_response_body({ "request_id": "net-0" })
→ { "ok": true, "request_id": "net-0", "body": "{\"users\":[...]}", "content_type": "application/json" }
```

### `network_clear_log`

Clear all captured entries. Call this before performing a new action so you only capture fresh traffic.

| Parameter | Type | Required | Default | Description |
|---|---|---|---|---|
| `sessionId` | string | No | default session | Target browser session |

```
network_clear_log()
→ { "ok": true, "cleared": 15 }
```

### `network_get_resource_timing`

Get resource loading performance data from the browser's built-in Performance API. Does **NOT** require `network_enable` — works on any page, any time.

Returns timing breakdown (DNS, connect, SSL, TTFB, download) for all loaded resources.

| Parameter | Type | Required | Default | Description |
|---|---|---|---|---|
| `sessionId` | string | No | default session | Target browser session |

```json
[
  {
    "name": "https://example.com/style.css",
    "type": "link",
    "startTime": 45.2,
    "duration": 120.5,
    "transferSize": 15200,
    "decodedBodySize": 42000,
    "dns": 1.2,
    "connect": 10.5,
    "ttfb": 85.3,
    "download": 23.5,
    "protocol": "h2"
  }
]
```

### `network_get_cookies`

Get all cookies for the current browser context. Uses CDP `Network.getAllCookies` when available (returns full cookie metadata including httpOnly, secure, sameSite), with a `document.cookie` JavaScript fallback.

| Parameter | Type | Required | Default | Description |
|---|---|---|---|---|
| `sessionId` | string | No | default session | Target browser session |

---

## Network Workflow Examples

### 5. API Discovery

Understand what API calls a page makes.

```
Agent flow:
  1. network_enable()
  2. open({ url: "https://app.example.com/dashboard" })
  3. wait({ condition: "displayed", selector: ".dashboard" })
  4. network_get_log({ "resource_types": ["xhr", "fetch"] })
  5. Discovers: GET /api/user/profile, GET /api/notifications, POST /api/analytics
  6. Agent: "The dashboard loads your profile from /api/user/profile,
     fetches notifications from /api/notifications, and sends analytics."
```

### 6. Debug Failed Requests

Find out why something isn't working.

```
User: "The page loaded but the data is missing"

Agent flow:
  1. network_get_log({ "has_error": true })
  2. Finds: POST /api/data → 403 Forbidden
  3. network_get_response_body({ "request_id": "net-7" })
  4. Body: {"error": "token expired"}
  5. Agent: "The data request failed with 403 — your authentication token has
     expired. You need to log in again."
```

### 7. Performance Analysis

Identify slow resources.

```
User: "This page is slow"

Agent flow:
  1. network_get_resource_timing()
  2. Finds: main.js (2.1MB, 3200ms), hero.png (4.5MB, 2800ms)
  3. Agent: "Two resources are slow:
     - main.js: 2.1MB taking 3.2s (consider code splitting)
     - hero.png: 4.5MB taking 2.8s (consider compression/WebP)"
```

### 8. Form Submission Inspection

Verify what data is being sent.

```
User: "帮我看看提交的表单发送了什么"

Agent flow:
  1. network_enable()
  2. network_clear_log()  ← reset before the action
  3. click({ selector: "button[type='submit']" })
  4. wait({ ms: 2000 })
  5. network_get_log({ "methods": ["POST", "PUT"] })
  6. Finds: POST /api/order with request body
  7. Agent: "The form sent a POST to /api/order with: {product_id: 123, quantity: 2, address: '...'}"
```

### 9. Cookie Inspection

Check authentication state.

```
Agent flow:
  1. network_get_cookies()
  2. Finds: session_id (httpOnly, secure), csrf_token, preferences
  3. Agent: "You have an active session (session_id cookie present, httpOnly+Secure).
     CSRF token is set. You appear to be logged in."
```

---

## Integration with Existing Tools

The agent tools work alongside all existing browsectl tools:

1. **`analyze_page`** uses `WdClient::execute()` to inject JavaScript for DOM extraction — the same mechanism used by `wait`, `scroll`, and other commands.
2. **Selectors** returned by `analyze_page` are standard CSS selectors compatible with `click`, `fill`, `paste`, `screenshot`, and all other selector-based commands, including the `::text(/regex/)` extension.
3. **Suggestions** include pre-built `commands` arrays that can be passed directly to `run_batch`.
4. **Memory** uses the same `~/.browsectl/` directory and JSON persistence pattern as the session store.
5. **Network tools** use a combination of JavaScript injection (same as DOM analysis) and CDP commands for deeper inspection. The interceptor captures data in `window.__browsectl_net`, while CDP provides access to features JavaScript cannot reach (httpOnly cookies, detailed timing, etc.).
6. **Network + DOM together** — use `analyze_page` to understand what's on the page, then `network_get_log` to understand what happened behind the scenes. This combination gives the agent both the "what you see" and "what's underneath" perspectives.

### Typical Agent Loop

```
┌─────────────┐     ┌──────────────┐     ┌─────────────────┐
│  User says   │────▶│  analyze_page │────▶│  Structured      │
│  intent      │     │  or           │     │  slots + safety  │
│              │     │  suggest_     │     │  + suggestions   │
│              │     │  actions      │     │                  │
└─────────────┘     └──────────────┘     └────────┬────────┘
                                                   │
                                          ┌────────▼────────┐
                                          │  AI plans task   │
                                          │  from slots +    │
                                          │  intent          │
                                          └────────┬────────┘
                                                   │
                                    ┌──────────────▼──────────────┐
                                    │  Safety check               │
                                    │  🟢🟡 → execute immediately  │
                                    │  🟠   → inform user          │
                                    │  🔴   → require confirmation │
                                    └──────────────┬──────────────┘
                                                   │
                                          ┌────────▼────────┐
                                          │  run_batch /     │
                                          │  run_command     │
                                          │  via WebDriver   │
                                          └────────┬────────┘
                                                   │
                              ┌────────────────────┼────────────────────┐
                              │                    │                    │
                     ┌────────▼────────┐  ┌────────▼────────┐ ┌────────▼────────┐
                     │  Record to       │  │  network_get_log │ │  Verify result   │
                     │  memory.json     │  │  (inspect traffic)│ │  (screenshot)    │
                     └─────────────────┘  └─────────────────┘ └─────────────────┘
```
