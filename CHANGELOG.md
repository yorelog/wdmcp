# Changelog

All notable changes to this project will be documented in this file.

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