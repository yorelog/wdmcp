---
name: browsectl
version: 0.1.2
description: "WebDriver automation CLI for AI-driven browser control. Provides session management, tab control, element interaction, screenshots, batch execution, an interactive REPL, and an intelligent agent layer with DOM slot extraction, safety classification, and task recommendation."
metadata:
  requires:
    bins: ["browsectl"]
  cliHelp: "browsectl --help"
---

# browsectl (0.1.2)

> **CRITICAL:** Before doing anything, run `browsectl setup` to detect installed browsers and auto-download the matching WebDriver binary. Sessions persist to `.browsectl/sessions.json` across CLI invocations — you do not need to create a new session every time.

A WebDriver automation CLI for AI-driven browser control. `browsectl` provides subcommands for browser session management, tab control, element interaction, screenshots, batch execution, an interactive REPL, and an intelligent agent layer that combines NLP slot extraction with WebDriver automation for safe, intent-driven page interaction.

## Installation

```sh
npm install -g @yorelog/browsectl
browsectl setup
```

`browsectl setup` detects your platform, finds installed browsers (Chrome / Edge), and automatically downloads the matching WebDriver binary (chromedriver / msedgedriver).

## Skills Index

| Skill file | Category | Commands |
|---|---|---|
| [session.md](session.md) | Session & driver lifecycle | `setup`, `status`, `driver-start`, `session-create`, `session-list`, `session-use`, `session-delete` |
| [browser.md](browser.md) | Browser commands | `run --type open`, `click`, `fill`, `paste`, `screenshot`, `scroll`, `title`, `last-message-content`, `wait` |
| [tabs.md](tabs.md) | Tab management | `tab-list`, `tab-create`, `tab-switch`, `tab-close` |
| [batch.md](batch.md) | Batch execution | `run`, `batch` — single commands, batch files, parallel groups |
| [selectors.md](selectors.md) | Selector syntax reference | CSS selectors + `::text(/regex/flags)` extension |
| [repl.md](repl.md) | Interactive REPL | Live command entry, history, tab-completion |
| [agent.md](agent.md) | Agent intelligence | `analyze_page`, `suggest_actions`, `network_*` — DOM slot extraction, safety classification, task recommendation, network monitoring |

## Global CLI Flags

| Flag | Default | Description |
|---|---|---|
| `--browser` | `chrome` | Browser to automate: `chrome` or `edge` |
| `--server` | `http://127.0.0.1:9515` | WebDriver server URL |
| `--chromedriver` | *(auto-detected)* | Path to WebDriver binary (chromedriver / msedgedriver) |
| `--chrome-binary` | *(auto-detected)* | Path to browser binary |
| `--user-data-dir` | `~/.browsectl/<browser>-profile` | Browser user-data directory |
| `--profile-directory` | `Default` | Browser profile directory name |
| `--headless` | `false` | Run browser in headless mode |
| `--viewport` | `1024,768` | Viewport size as `width,height` |
| `--session` | *(default session)* | Session ID to operate on (omit for default) |

## Agent Intelligence Layer

The agent layer sits between the AI model (via MCP) and the WebDriver execution layer, providing structured DOM understanding:

- **`analyze_page`** — Parses the current page DOM to extract all interactive "slots" (buttons, inputs, links, forms) with metadata, CSS selectors, and a four-tier safety classification (🟢 observe → 🟡 navigate → 🟠 interact → 🔴 submit).
- **`suggest_actions`** — Recommends possible tasks by combining DOM analysis with local memory of previously successful task patterns. Useful when a user doesn't know what actions are available on a page.
- **Safety classification** — Every slot and suggested action is tagged with a safety level. Submit-level actions (purchase, delete, form post) are flagged for confirmation. Classification uses multilingual keyword detection (EN/ZH).
- **Memory** — Task history and page visit patterns are stored locally in `~/.browsectl/memory.json`, enabling the agent to learn which actions work on which URL patterns and recommend them on future visits.
- **Network monitoring** — Captures HTTP traffic like the browser DevTools Network panel:
  - **`network_enable`** / **`network_disable`** — Start/stop capturing by injecting a fetch/XHR interceptor + CDP Network domain.
  - **`network_get_log`** — Retrieve captured requests with filtering by URL pattern, HTTP method, resource type, status code range, or errors only.
  - **`network_get_response_body`** — Inspect the response body of a specific captured request.
  - **`network_get_resource_timing`** — Get Performance API timing data (DNS, connect, SSL, TTFB, download) without needing the interceptor.
  - **`network_get_cookies`** — Get all cookies via CDP (with JS fallback), including httpOnly and secure flags.

See [agent.md](agent.md) for full details, schemas, and flow examples.

## Platform Support

| OS | x64 | arm64 |
|---|---|---|
| macOS | ✅ | ✅ |
| Linux | ✅ | ✅ |
| Windows | ✅ | ✅ |