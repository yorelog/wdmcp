# Changelog

All notable changes to this project will be documented in this file.

## [0.2.0] - 2026-04-02

### Added

- **Agent intelligence layer** (`src/agent/`) — a new module sitting between the AI model and WebDriver, providing structured DOM understanding and network visibility.
- **DOM slot extraction** (`analyze_page` MCP tool) — parses the current page DOM to discover all interactive elements ("slots") with metadata, CSS selectors, element categories, and form membership.
- **Safety classification** — four-tier graduated safety levels (🟢 Observe → 🟡 Navigate → 🟠 Interact → 🔴 Submit) applied to every slot and suggested action; uses multilingual keyword detection (EN/ZH).
- **Task suggestion** (`suggest_actions` MCP tool) — recommends possible actions by combining DOM analysis with locally stored task memory; ranks suggestions by relevance and safety.
- **Agent memory** (`~/.browsectl/memory.json`) — persists page visit history and successful task patterns across sessions so the agent can learn and recall previously effective actions.
- **Network monitoring** — captures HTTP traffic like the browser DevTools Network panel:
  - `network_enable` / `network_disable` — start/stop capturing via injected fetch/XHR interceptor + CDP Network domain.
  - `network_get_log` — retrieve captured requests with filtering by URL pattern, HTTP method, resource type, status code range, or errors only.
  - `network_get_response_body` — inspect the response body of a specific captured request.
  - `network_clear_log` — clear captured entries before a new action.
  - `network_get_resource_timing` — get Performance API resource timing data (DNS, connect, SSL, TTFB, download) without needing the interceptor.
  - `network_get_cookies` — get all cookies via CDP with JavaScript fallback, including httpOnly and secure flags.
- **Chrome DevTools Protocol (CDP) support** — `cdp_execute` and `cdp_execute_quiet` methods on `WdClient` with automatic Chrome/Edge endpoint detection.
- **WebDriver logging** — `get_log_types` and `get_log` methods for retrieving browser log entries.
- `AGENT.md` design document describing the agent layer architecture, flow examples, and security considerations.
- `skills/agent.md` skill documentation covering all agent and network MCP tools with schemas and examples.
- Agent and network integration tests (`tests/agent.rs`, `tests/network.rs`).
- Expanded test fixture (`tests/fixtures/test-page.html`) with additional interactive elements for agent testing.

### Changed

- Updated `skills/README.md` with agent layer description, tool table entry, and detailed feature summary.
- Registered `agent` and `network` tool definition groups in MCP server tool surface (`src/mcp.rs`).

## [0.1.2] - 2026-03-31

### Added

- Helpful error message in `scripts/run.js` when binary is missing (guides pnpm users to approve build scripts).

### Changed

- Replaced `skill.json` with `SKILL.md` markdown skill manifest (npm package validation requirement).
- Removed `LICENSE` from npm package `files` (non-text file filtering).

### Fixed

- Fixed BSD `sed` compatibility in `scripts/release.sh` for macOS (replaced GNU-only `\s` and multi-line syntax with portable `awk`).

## [0.1.1] - 2026-03-31

### Added

- README for npm package and GitHub repo.
- MIT LICENSE file.
- Release script (`scripts/release.sh`) that reads version from `package.json`, syncs `Cargo.toml` and `skill.json`, extracts CHANGELOG notes, and creates annotated git tags.
- `npm run release` convenience script.
- GitHub Release notes now sourced from CHANGELOG instead of auto-generated.

### Changed

- Upgraded GitHub Actions to v5 (`checkout`, `upload-artifact`, `download-artifact`, `setup-node`).
- Switched npm publish to Trusted Publishing (OIDC) — removed `NPM_TOKEN` secret.
- Updated macOS x64 CI runner from `macos-13` to `macos-latest`.
- Opted into Node.js 24 runtime for GitHub Actions.
- Improved `package.json` description.

### Fixed

- Fixed `E403` / Node.js 20 deprecation warnings in CI pipeline.

## [0.1.0] - 2026-03-31

### Added

- **CLI** with subcommands for browser session management, tab control, batch execution, interactive REPL, and environment setup.
- **MCP server** mode (`browsectl mcp`) exposing browser automation over JSON-RPC 2.0 / NDJSON for AI-driven workflows.
- **Session management**: create, list, switch, and delete persistent browser sessions.
- **Tab management**: create, list, switch, and close browser tabs with alias support.
- **Browser commands**: `open`, `click`, `fill`, `paste`, `screenshot`, `scroll`, `get_title`, `get_last_message`, `wait_for`.
- **Batch execution**: run sequences of commands from JSON files with parallel group support.
- **Interactive REPL** for ad-hoc browser automation.
- **Setup wizard** (`browsectl setup`): auto-detects platform, installed browsers, and downloads the matching WebDriver binary.
- **Smart click fallback**: automatically retries with parent/sibling/JS strategies when native click is intercepted.
- **CSS selector extensions**: `CSS::text(/pattern/flags)` syntax for filtering elements by text content.
- Cross-platform support: macOS (x64/arm64), Linux (x64/arm64), Windows (x64/arm64).
- npm distribution with automatic binary download via `postinstall` script.