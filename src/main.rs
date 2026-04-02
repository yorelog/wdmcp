//! browsectl — WebDriver automation CLI & MCP server.
//!
//! This binary provides:
//! - A **CLI** with subcommands for browser session management, tab control,
//!   batch execution, an interactive REPL, and environment setup.
//! - An **MCP server** mode (`browsectl mcp`) that exposes the same functionality
//!   over JSON-RPC 2.0 / NDJSON for AI-driven browser automation.
//!
//! Session state is persisted to `.browsectl/sessions.json` so browser sessions
//! survive across CLI invocations.  The `setup` command detects the platform,
//! installed browsers, and auto-downloads the matching WebDriver binary,
//! caching results in `.browsectl/setup.json`.

mod agent;
mod batch;
mod commands;
mod driver;
mod manager;
mod mcp;
mod setup;
mod store;
mod types;
mod webdriver;

use std::borrow::Cow;
use std::collections::HashMap;
use std::io::IsTerminal;
use std::path::PathBuf;
use std::time::Duration;

use rustyline::completion::{Completer, Pair};
use rustyline::error::ReadlineError;
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::validate::Validator;
use rustyline::{Config, Context as RlContext, Editor, Helper};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use serde_json::{Value, json};

use crate::types::*;

// ---------------------------------------------------------------------------
// CLI definitions
// ---------------------------------------------------------------------------

#[derive(Parser, Debug)]
#[command(name = "browsectl", about = "WebDriver automation CLI & MCP server")]
struct Cli {
    /// Browser to automate: "chrome" or "edge"
    #[arg(long, default_value = "chrome")]
    browser: String,

    /// WebDriver server URL
    #[arg(long, default_value = "http://127.0.0.1:9515")]
    server: String,

    /// Path to the WebDriver server binary (chromedriver / msedgedriver).
    /// Auto-detected from --browser when omitted.
    #[arg(long)]
    chromedriver: Option<String>,

    /// Path to the browser binary.
    /// Auto-detected from --browser when omitted.
    #[arg(long)]
    chrome_binary: Option<String>,

    /// Browser user-data-dir (default: ~/.browsectl/{chrome,edge}-profile).
    /// Uses a dedicated automation profile so it never conflicts with
    /// your running browser.  Pass your real profile path here if you
    /// want to reuse bookmarks/cookies (browser must be closed first).
    #[arg(long)]
    user_data_dir: Option<String>,

    /// Browser profile directory name
    #[arg(long, default_value = "Default")]
    profile_directory: String,

    /// Run browser in headless mode
    #[arg(long, default_value_t = false)]
    headless: bool,

    /// Viewport as "width,height"
    #[arg(long, default_value = "1024,768")]
    viewport: String,

    /// Session ID to use (omit for default session)
    #[arg(long)]
    session: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Show WebDriver server status
    Status,
    /// Start WebDriver server if not running
    DriverStart,
    /// Create a new browser session
    SessionCreate {
        /// Run session creation in the current process (no background worker).
        #[arg(long, default_value_t = false)]
        foreground: bool,
        /// Spawn the worker and return immediately without waiting for
        /// completion (fire-and-forget).  By default the CLI now waits for
        /// the background worker to finish.
        #[arg(long, default_value_t = false)]
        detach: bool,
        /// Copy user data (cookies, extensions) from real browser profile.
        /// Skips the interactive prompt.
        #[arg(long)]
        copy_data: bool,
        /// Do not copy any user data from real browser profile.
        /// Skips the interactive prompt.
        #[arg(long, conflicts_with = "copy_data")]
        no_copy_data: bool,
    },
    /// List all stored sessions
    SessionList,
    /// Switch default session
    SessionUse {
        #[arg(long = "session")]
        session_id: String,
    },
    /// Delete a session
    SessionDelete {
        #[arg(long = "session")]
        session_id: Option<String>,
    },
    /// List tabs in current session
    TabList,
    /// Create a new tab
    TabCreate {
        #[arg(long)]
        url: Option<String>,
        #[arg(long)]
        alias: Option<String>,
        #[arg(long, default_value_t = true)]
        activate: bool,
    },
    /// Switch to a tab by alias/handle/index
    TabSwitch {
        #[arg(long)]
        tab: String,
    },
    /// Close a tab by alias/handle/index
    TabClose {
        #[arg(long)]
        tab: String,
    },
    /// Execute a batch file
    Batch {
        #[arg(long)]
        file: PathBuf,
        #[arg(long)]
        name: Option<String>,
    },
    /// Interactive REPL
    Repl,
    /// Start MCP server (JSON-RPC over stdio)
    Mcp,
    /// Detect platform, installed browsers & versions, and auto-download WebDriver.
    Setup {
        /// Browser to set up: "chrome" or "edge". Overrides the global --browser flag.
        #[arg(long)]
        browser: Option<String>,
        /// Only report detected info without downloading drivers.
        #[arg(long, default_value_t = false)]
        check_only: bool,
    },
    /// Run a single command
    Run {
        #[arg(long = "type")]
        command_type: String,
        #[arg(long)]
        selector: Option<String>,
        #[arg(long)]
        scope: Option<String>,
        #[arg(long)]
        fallback: Option<String>,
        #[arg(long)]
        url: Option<String>,
        #[arg(long)]
        text: Option<String>,
        #[arg(long)]
        path: Option<String>,
        #[arg(long)]
        ms: Option<u64>,
        #[arg(long)]
        condition: Option<String>,
        #[arg(long)]
        attribute: Option<String>,
        #[arg(long)]
        value: Option<String>,
        #[arg(long)]
        timeout: Option<u64>,
        #[arg(long)]
        interval: Option<u64>,
        #[arg(long)]
        direction: Option<String>,
        #[arg(long)]
        amount: Option<i64>,
        #[arg(long)]
        behavior: Option<String>,
        #[arg(long)]
        viewport: Option<String>,
        #[arg(long)]
        tab: Option<String>,
    },
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn parse_browser(raw: &str) -> Browser {
    raw.parse::<Browser>().unwrap_or_else(|e| {
        eprintln!("warning: {e}, defaulting to chrome");
        Browser::Chrome
    })
}

fn config_from_cli(cli: &Cli) -> SessionConfig {
    let browser = parse_browser(&cli.browser);
    let (vw, vh) = parse_viewport_str(&cli.viewport);
    SessionConfig {
        browser,
        server_url: cli.server.clone(),
        chromedriver_path: cli
            .chromedriver
            .clone()
            .unwrap_or_else(|| default_driver_path(browser)),
        chrome_binary: cli
            .chrome_binary
            .clone()
            .unwrap_or_else(|| default_browser_binary(browser)),
        user_data_dir: cli.user_data_dir.clone(),
        profile_directory: cli.profile_directory.clone(),
        headless: cli.headless,
        viewport_width: vw,
        viewport_height: vh,
        copy_data: CopyDataConfig::default(),
    }
}

fn parse_viewport_str(raw: &str) -> (i64, i64) {
    let parts: Vec<&str> = raw.split(',').collect();
    if parts.len() == 2 {
        if let (Ok(w), Ok(h)) = (
            parts[0].trim().parse::<i64>(),
            parts[1].trim().parse::<i64>(),
        ) {
            return (w, h);
        }
    }
    (1024, 768)
}

fn print_json(value: &Value) {
    if let Ok(s) = serde_json::to_string_pretty(value) {
        println!("{s}");
    }
}

fn print_help() {
    println!("browsectl CLI");
    println!("  status                                  — chromedriver status");
    println!("  driver-start                            — start chromedriver");
    println!("  session-create [--foreground]            — create browser session");
    println!("  session-list                            — list sessions");
    println!("  session-use --session <id>              — switch default session");
    println!("  session-delete [--session <id>]         — delete session");
    println!("  tab-list                                — list tabs");
    println!("  tab-create [--url X] [--alias X]        — create tab");
    println!("  tab-switch --tab <ref>                  — switch tab");
    println!("  tab-close  --tab <ref>                  — close tab");
    println!("  run --type <cmd> [options]               — single command");
    println!("  batch --file X [--name X]               — batch execution");
    println!("  repl                                    — interactive REPL");
    println!("  mcp                                     — MCP server (stdio)");
    println!(
        "  setup [--check-only]                    — detect platform, browsers & auto-download driver"
    );
    println!();
    println!("Run types: open, click, fill, paste, scroll, screenshot, title,");
    println!("           last-message-content, wait, tab-list, tab-create,");
    println!("           tab-switch, tab-close, parallel");
}

// ---------------------------------------------------------------------------
// Interactive copy-data prompt
// ---------------------------------------------------------------------------

/// Shows an interactive menu letting the user choose which data to copy from
/// their real browser profile.  Returns the selected [`CopyDataConfig`].
fn prompt_copy_data(browser: Browser) -> CopyDataConfig {
    let real_dir = real_browser_user_data_dir(browser);
    if real_dir.is_none() {
        eprintln!("No real {} profile found, skipping data copy.", browser);
        return CopyDataConfig::none();
    }

    eprintln!("Real {} profile: {}", browser, real_dir.as_ref().unwrap());
    eprintln!();
    eprintln!("Copy user data from your {} profile?", browser);
    eprintln!("  [1] Cookies           (login sessions, etc.)");
    eprintln!("  [2] Extensions        (installed plugins)");
    eprintln!("  [3] Local Storage     (site data)");
    eprintln!("  [4] Bookmarks");
    eprintln!();
    eprint!("Enter items to copy (e.g. 1,2) or 'all'/'none' [default: 1,2]: ");

    let mut input = String::new();
    if std::io::stdin().read_line(&mut input).is_err() {
        return CopyDataConfig::default();
    }
    let input = input.trim();

    if input.is_empty() {
        return CopyDataConfig::default(); // cookies + extensions
    }
    if input == "none" || input == "n" || input == "no" {
        return CopyDataConfig::none();
    }
    if input == "all" || input == "a" || input == "yes" || input == "y" {
        return CopyDataConfig {
            cookies: true,
            extensions: true,
            local_storage: true,
            bookmarks: true,
        };
    }

    // Parse comma-separated numbers
    let mut cfg = CopyDataConfig::none();
    for part in input.split(',') {
        match part.trim() {
            "1" => cfg.cookies = true,
            "2" => cfg.extensions = true,
            "3" => cfg.local_storage = true,
            "4" => cfg.bookmarks = true,
            _ => {}
        }
    }
    cfg
}

/// Resolve a session: try --session, then default from store, or auto-create.
async fn resolve_or_create(
    sessions: &mut HashMap<String, RuntimeCtx>,
    config: &SessionConfig,
    explicit_session: Option<&str>,
) -> Result<String> {
    manager::resolve_session(
        sessions,
        config,
        explicit_session,
        explicit_session.is_none(),
    )
    .await
}

// ---------------------------------------------------------------------------
// Background session-create (non-blocking)
// ---------------------------------------------------------------------------

/// Spawn a **detached** child process that creates a browser session and
/// writes the result to a JSON file.  Returns immediately so the caller is
/// never blocked.
/// Spawn the background worker and return metadata (output file path, pid, etc.).
async fn spawn_background_worker(config: &SessionConfig) -> Result<(PathBuf, PathBuf, u32)> {
    let run_id = format!(
        "{}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
        std::process::id(),
    );

    let base_dir = PathBuf::from(".browsectl/jobs");
    tokio::fs::create_dir_all(&base_dir).await?;

    let output_file = base_dir.join(format!("{run_id}.json"));
    let log_file = base_dir.join(format!("{run_id}.log"));

    let exe = std::env::current_exe().context("cannot determine own executable path")?;
    let options_json = serde_json::to_string(config)?;

    let mut cmd = std::process::Command::new(&exe);
    cmd.arg("session-create")
        .arg("--foreground")
        .env("BROWSECTL_SESSION_OPTIONS", &options_json)
        .env(
            "BROWSECTL_SESSION_OUTPUT",
            output_file.to_string_lossy().as_ref(),
        )
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());

    // Detach the child so it survives the parent exiting / terminal closing.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;
        const DETACHED_PROCESS: u32 = 0x00000008;
        cmd.creation_flags(CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS);
    }

    let child = cmd.spawn().context("failed to spawn background worker")?;
    let pid = child.id();

    Ok((output_file, log_file, pid))
}

/// Spawn the background worker and **wait** for it to finish, polling the
/// output file until it appears (or a timeout is reached).  Returns the
/// worker's result JSON.
async fn start_background_session_create(config: &SessionConfig) -> Result<Value> {
    let (output_file, log_file, pid) = spawn_background_worker(config).await?;

    eprintln!("info: session creation started in background (pid {pid}), waiting for completion…");

    // Poll the output file every 500 ms, up to 90 seconds.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
    let poll_interval = Duration::from_millis(500);

    loop {
        if tokio::time::Instant::now() >= deadline {
            return Ok(json!({
                "ok": false,
                "error": format!(
                    "background worker (pid {pid}) did not finish within 90 s.\n\
                     Check output file: {}",
                    output_file.display()
                ),
                "pid": pid,
                "outputFile": output_file.to_string_lossy(),
                "logFile": log_file.to_string_lossy(),
            }));
        }

        if output_file.exists() {
            // The file exists — read it.  The worker writes atomically
            // (single tokio::fs::write call), so if the file is present
            // it should be complete.
            if let Ok(raw) = tokio::fs::read_to_string(&output_file).await {
                let raw = raw.trim();
                if !raw.is_empty() {
                    if let Ok(result) = serde_json::from_str::<Value>(raw) {
                        return Ok(result);
                    }
                }
            }
        }

        tokio::time::sleep(poll_interval).await;
    }
}

/// Fire-and-forget variant: spawn the worker and return immediately.
async fn start_detached_session_create(config: &SessionConfig) -> Result<Value> {
    let (output_file, log_file, pid) = spawn_background_worker(config).await?;

    Ok(json!({
        "started": true,
        "mode": "background",
        "pid": pid,
        "outputFile": output_file.to_string_lossy(),
        "logFile": log_file.to_string_lossy(),
    }))
}

// ---------------------------------------------------------------------------
// REPL — helper types
// ---------------------------------------------------------------------------

/// All commands the REPL understands, used for Tab completion.
const REPL_COMMANDS: &[&str] = &[
    "click",
    "exit",
    "fill",
    "help",
    "last-message",
    "last-message-content",
    "open",
    "paste",
    "quit",
    "screenshot",
    "scroll",
    "tab-close",
    "tab-create",
    "tab-switch",
    "tabs",
    "title",
    "wait",
];

/// Returns the path used to persist REPL history across sessions.
fn repl_history_path() -> PathBuf {
    PathBuf::from(".browsectl/repl_history")
}

/// Rustyline helper that provides Tab-completion for REPL commands.
struct ReplHelper;

impl Completer for ReplHelper {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &RlContext<'_>,
    ) -> rustyline::Result<(usize, Vec<Pair>)> {
        // Only complete the first token (command name).
        let prefix = &line[..pos];
        if prefix.contains(' ') {
            // Cursor is past the first word — no completion.
            return Ok((pos, vec![]));
        }

        let matches: Vec<Pair> = REPL_COMMANDS
            .iter()
            .filter(|cmd| cmd.starts_with(prefix))
            .map(|cmd| Pair {
                display: cmd.to_string(),
                replacement: cmd.to_string(),
            })
            .collect();

        Ok((0, matches))
    }
}

impl Hinter for ReplHelper {
    type Hint = String;

    fn hint(&self, line: &str, pos: usize, _ctx: &RlContext<'_>) -> Option<String> {
        if line.is_empty() || line.contains(' ') || pos != line.len() {
            return None;
        }
        // Show the first matching command as a dim hint.
        REPL_COMMANDS
            .iter()
            .find(|cmd| cmd.starts_with(line) && **cmd != line)
            .map(|cmd| cmd[line.len()..].to_string())
    }
}

impl Highlighter for ReplHelper {
    fn highlight_hint<'h>(&self, hint: &'h str) -> Cow<'h, str> {
        // Dim grey hint text.
        Cow::Owned(format!("\x1b[90m{hint}\x1b[0m"))
    }
}

impl Validator for ReplHelper {}
impl Helper for ReplHelper {}

// ---------------------------------------------------------------------------
// REPL — main loop
// ---------------------------------------------------------------------------

async fn run_repl(ctx: &mut RuntimeCtx) -> Result<()> {
    println!("repl session: {}", ctx.session_id);
    println!("type \"help\" for commands, Tab to complete, \"exit\" to quit");

    let config = Config::builder()
        .auto_add_history(true)
        .max_history_size(1000)
        .expect("valid history size")
        .build();

    let helper = ReplHelper;
    let mut rl = Editor::with_config(config)?;
    rl.set_helper(Some(helper));

    // Load history from previous sessions (ignore errors).
    let history_path = repl_history_path();
    if let Some(parent) = history_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let _ = rl.load_history(&history_path);

    loop {
        match rl.readline("browsectl> ") {
            Ok(line) => {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                if line == "exit" || line == "quit" {
                    break;
                }
                if line == "help" {
                    print_help();
                    continue;
                }

                let result = repl_dispatch(ctx, line).await;
                match result {
                    Ok(value) => print_json(&value),
                    Err(e) => eprintln!("error: {e:#}"),
                }
            }
            Err(ReadlineError::Interrupted) => {
                // Ctrl-C — cancel current line, keep REPL running.
                continue;
            }
            Err(ReadlineError::Eof) => {
                // Ctrl-D — exit.
                break;
            }
            Err(e) => {
                eprintln!("readline error: {e}");
                break;
            }
        }
    }

    // Persist history for next session.
    let _ = rl.save_history(&history_path);
    Ok(())
}

async fn repl_dispatch(ctx: &mut RuntimeCtx, line: &str) -> Result<Value> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.is_empty() {
        bail!("empty command");
    }

    let cmd_type = parts[0];
    match cmd_type {
        "title" => {
            let title = ctx.driver.title().await?;
            Ok(json!({"ok": true, "type": "title", "title": title}))
        }
        "tabs" => manager::list_tabs(ctx).await,
        "last-message" | "last-message-content" => {
            let cmd = CommandSpec {
                command_type: "last-message-content".into(),
                ..Default::default()
            };
            batch::execute_command(ctx, &cmd).await
        }
        "wait" => {
            let cmd = parse_repl_wait(&parts[1..]);
            batch::execute_command(ctx, &cmd).await
        }
        "scroll" => {
            let cmd = CommandSpec {
                command_type: "scroll".into(),
                direction: parts.get(1).map(|s| s.to_string()),
                amount: parts.get(2).and_then(|s| s.parse().ok()),
                selector: parts.get(3).map(|s| s.to_string()),
                ..Default::default()
            };
            batch::execute_command(ctx, &cmd).await
        }
        "tab-create" => {
            manager::create_tab(ctx, parts.get(1).copied(), parts.get(2).copied(), true).await
        }
        "tab-switch" => {
            let tab_ref = parts.get(1).unwrap_or(&"current");
            manager::switch_tab(ctx, &json!(tab_ref)).await
        }
        "tab-close" => {
            let tab_ref = parts.get(1).unwrap_or(&"current");
            manager::close_tab(ctx, &json!(tab_ref)).await
        }
        "open" => {
            let cmd = CommandSpec {
                command_type: "open".into(),
                url: parts.get(1).map(|s| s.to_string()),
                ..Default::default()
            };
            batch::execute_command(ctx, &cmd).await
        }
        "click" => {
            if parts.len() < 2 {
                bail!("click requires a selector");
            }

            let selector = parts[1].to_string();
            let mut fallback: Option<String> = None;
            let mut scope: Option<String> = None;

            // REPL syntax:
            //   click <selector>
            //   click <selector> --fallback <parent|sibling|css-selector>
            //   click <selector> --scope <css-selector>
            if parts.len() >= 4 && parts[2] == "--fallback" {
                fallback = Some(parts[3].to_string());
            }
            if parts.len() >= 4 && parts[2] == "--scope" {
                scope = Some(parts[3].to_string());
            }
            if parts.len() >= 6 {
                if parts[2] == "--scope" && parts[4] == "--fallback" {
                    fallback = Some(parts[5].to_string());
                }
                if parts[2] == "--fallback" && parts[4] == "--scope" {
                    scope = Some(parts[5].to_string());
                }
            }

            let cmd = CommandSpec {
                command_type: "click".into(),
                selector: Some(selector),
                scope,
                fallback,
                ..Default::default()
            };
            batch::execute_command(ctx, &cmd).await
        }
        "fill" | "paste" => {
            let is_no_selector = parts.len() <= 2;
            let cmd = CommandSpec {
                command_type: cmd_type.into(),
                selector: if is_no_selector {
                    None
                } else {
                    parts.get(1).map(|s| s.to_string())
                },
                text: Some(if is_no_selector {
                    parts[1..].join(" ")
                } else {
                    parts[2..].join(" ")
                }),
                ..Default::default()
            };
            batch::execute_command(ctx, &cmd).await
        }
        "screenshot" => {
            let cmd = CommandSpec {
                command_type: "screenshot".into(),
                selector: parts.get(1).map(|s| s.to_string()),
                path: parts.get(2).map(|s| s.to_string()),
                ..Default::default()
            };
            batch::execute_command(ctx, &cmd).await
        }
        _ => {
            // Generic fallback: treat first word as command type
            let cmd = CommandSpec {
                command_type: cmd_type.into(),
                selector: parts.get(1).map(|s| s.to_string()),
                url: parts.get(1).map(|s| s.to_string()),
                text: if parts.len() > 2 {
                    Some(parts[2..].join(" "))
                } else {
                    None
                },
                ..Default::default()
            };
            batch::execute_command(ctx, &cmd).await
        }
    }
}

fn parse_repl_wait(rest: &[&str]) -> CommandSpec {
    if rest.is_empty() {
        return CommandSpec {
            command_type: "wait".into(),
            ms: Some(500),
            ..Default::default()
        };
    }

    if let Ok(ms) = rest[0].parse::<u64>() {
        return CommandSpec {
            command_type: "wait".into(),
            ms: Some(ms),
            ..Default::default()
        };
    }

    let selector = rest[0].to_string();
    let condition = rest.get(1).unwrap_or(&"displayed").to_string();

    if condition == "attribute-equals" || condition == "attribute-contains" {
        return CommandSpec {
            command_type: "wait".into(),
            selector: Some(selector),
            condition: Some(condition),
            attribute: rest.get(2).map(|s| s.to_string()),
            value: Some(rest[3..].join(" ")),
            ..Default::default()
        };
    }

    if condition == "text-contains"
        || condition == "text-equals"
        || condition == "value-contains"
        || condition == "value-equals"
    {
        return CommandSpec {
            command_type: "wait".into(),
            selector: Some(selector),
            condition: Some(condition),
            value: Some(rest[2..].join(" ")),
            ..Default::default()
        };
    }

    CommandSpec {
        command_type: "wait".into(),
        selector: Some(selector),
        condition: Some(condition),
        ..Default::default()
    }
}

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let config = config_from_cli(&cli);

    match &cli.command {
        // ---- commands that do NOT need a session ----
        Commands::Status => {
            match driver::fetch_status(&cli.server).await {
                Ok(status) => print_json(&json!({"ok": true, "status": status})),
                Err(e) => print_json(&json!({"ok": false, "error": format!("{e:#}")})),
            }
            return Ok(());
        }

        Commands::DriverStart => {
            let driver_path = cli
                .chromedriver
                .clone()
                .unwrap_or_else(|| default_driver_path(parse_browser(&cli.browser)));
            let result = driver::ensure_running(&cli.server, &driver_path).await?;
            let reused = result.is_none();
            print_json(&json!({"ok": true, "reused": reused, "server": cli.server}));
            // Let the driver child continue running after we exit.
            // (Dropping tokio::process::Child does NOT kill the child.)
            return Ok(());
        }

        Commands::SessionCreate {
            foreground,
            detach,
            copy_data,
            no_copy_data,
        } => {
            // Check if we are running as a background worker (spawned by a
            // previous invocation).  The worker receives the output-file path
            // via an environment variable.
            let output_file = std::env::var("BROWSECTL_SESSION_OUTPUT").ok();

            // ── Determine copy-data config ────────────────────────────
            // Priority: --no-copy-data > --copy-data > interactive prompt > default
            let copy_cfg = if *no_copy_data {
                CopyDataConfig::none()
            } else if *copy_data {
                CopyDataConfig::default()
            } else if output_file.is_none() && std::io::stdin().is_terminal() {
                // Interactive terminal — ask the user what to copy.
                prompt_copy_data(config.browser)
            } else {
                // Background worker or non-interactive — use default
                // (cookies + extensions).
                CopyDataConfig::default()
            };

            let mut config = config.clone();
            config.copy_data = copy_cfg;

            if *foreground || output_file.is_some() {
                // ── Foreground / worker mode ───────────────────────────
                // If BROWSECTL_SESSION_OPTIONS is set, prefer it over CLI
                // args (the parent serialised the config there).
                let effective_config = match std::env::var("BROWSECTL_SESSION_OPTIONS") {
                    Ok(json_str) => {
                        serde_json::from_str(&json_str).unwrap_or_else(|_| config.clone())
                    }
                    Err(_) => config.clone(),
                };

                let result = match manager::create_session(&effective_config).await {
                    Ok(ctx) => {
                        let sid = ctx.session_id.clone();
                        // WdClient does NOT close the session on drop, so we
                        // can just let `ctx` drop normally.
                        json!({"ok": true, "sessionId": sid})
                    }
                    Err(e) => {
                        json!({"ok": false, "error": format!("{e:#}")})
                    }
                };

                if let Some(ref file) = output_file {
                    // Worker mode → write result to the output file.
                    if let Some(parent) = PathBuf::from(file).parent() {
                        tokio::fs::create_dir_all(parent).await.ok();
                    }
                    let pretty =
                        serde_json::to_string_pretty(&result).unwrap_or_else(|_| "{}".into());
                    tokio::fs::write(file, format!("{pretty}\n")).await?;
                } else {
                    // Interactive foreground mode → print to stdout.
                    print_json(&result);
                }
            } else if *detach {
                // ── Detached mode (--detach) ──────────────────────────
                // Spawn a background worker and return immediately.
                let result = start_detached_session_create(&config).await?;
                print_json(&result);
            } else {
                // ── Background mode (default) ─────────────────────────
                // Spawn a background worker and poll until it finishes.
                let result = start_background_session_create(&config).await?;
                print_json(&result);
            }
            return Ok(());
        }

        Commands::SessionList => {
            let s = store::read_store().await?;
            print_json(&json!({
                "defaultSessionId": s.default_session_id,
                "sessions": s.sessions.values().collect::<Vec<_>>(),
            }));
            return Ok(());
        }

        Commands::SessionUse { session_id } => {
            store::set_default(session_id).await?;
            print_json(&json!({"ok": true, "defaultSessionId": session_id}));
            return Ok(());
        }

        Commands::SessionDelete { session_id } => {
            let mut sessions: HashMap<String, RuntimeCtx> = HashMap::new();
            let effective = session_id.clone().or_else(|| cli.session.clone());
            let result = manager::delete_session(&mut sessions, effective.as_deref()).await?;
            print_json(&result);
            return Ok(());
        }

        Commands::Mcp => {
            return mcp::run(config).await;
        }

        Commands::Setup {
            browser: setup_browser,
            check_only,
        } => {
            let platform = setup::detect_platform();
            let browsers = setup::detect_browsers().await;

            // -- Platform --
            eprintln!("Platform: {}", platform.display);
            eprintln!();

            // -- Browsers (only installed) --
            let installed: Vec<_> = browsers.iter().filter(|b| b.installed).collect();
            if installed.is_empty() {
                eprintln!("No supported browsers detected.");
                eprintln!("  Install Chrome or Edge, then run this command again.");
            } else {
                eprintln!("Browsers:");
                for b in &installed {
                    let ver = b.version.as_deref().unwrap_or("unknown version");
                    eprintln!("  {} {} — {}", b.browser, ver, b.path);
                }
            }
            eprintln!();

            // -- Drivers (only for installed browsers) --
            let mut setup_drivers = Vec::new();
            eprintln!("Drivers:");
            for b in &installed {
                let info = setup::detect_driver(b.browser).await;
                let matched;
                if info.exists {
                    let dver = info.version.as_deref().unwrap_or("unknown version");
                    matched = match (&info.version, &b.version) {
                        (Some(dv), Some(bv)) => {
                            let dm: Option<u32> = dv.split('.').next().and_then(|s| s.parse().ok());
                            let bm: Option<u32> = bv.split('.').next().and_then(|s| s.parse().ok());
                            dm.is_some() && dm == bm
                        }
                        _ => false,
                    };
                    if matched {
                        eprintln!("  {} driver {} — OK ({})", b.browser, dver, info.path);
                    } else {
                        eprintln!(
                            "  {} driver {} — version mismatch with browser ({})",
                            b.browser, dver, info.path
                        );
                    }
                } else {
                    matched = false;
                    eprintln!("  {} driver — not found ({})", b.browser, info.path);
                }
                setup_drivers.push(SetupDriver {
                    browser: info.browser.to_string(),
                    path: info.path.clone(),
                    version: info.version.clone(),
                    exists: info.exists,
                    matched,
                });
            }
            eprintln!();

            let mut ready_browser: Option<String> = None;
            let mut ready_driver_path: Option<String> = None;

            if !check_only {
                let mut browser = parse_browser(setup_browser.as_deref().unwrap_or(&cli.browser));
                let mut browser_info = installed.iter().find(|b| b.browser == browser);

                // If the selected browser isn't installed but exactly one other is, use it automatically
                if browser_info.is_none() && installed.len() == 1 {
                    let alt = installed[0].browser;
                    eprintln!(
                        "{} is not installed. Auto-selecting {} (the only installed browser).",
                        browser, alt
                    );
                    eprintln!();
                    browser = alt;
                    browser_info = installed.iter().find(|b| b.browser == browser);
                }

                if let Some(bi) = browser_info {
                    let driver_path = cli
                        .chromedriver
                        .clone()
                        .unwrap_or_else(|| default_driver_path(browser));
                    match setup::ensure_driver(browser, bi.version.as_deref(), &driver_path).await {
                        Ok(path) => {
                            eprintln!("Ready! {} driver is available at {}", browser, path);
                            ready_browser = Some(browser.to_string());
                            ready_driver_path = Some(path);
                        }
                        Err(e) => {
                            eprintln!("Could not set up {} driver: {:#}", browser, e);
                            eprintln!();
                            eprintln!(
                                "Tip: make sure {} is installed so the version can be detected.",
                                browser
                            );
                        }
                    }
                } else if installed.is_empty() {
                    eprintln!("No installed browser found. Install Chrome or Edge first.");
                } else {
                    // Multiple browsers installed but selected one isn't among them (unlikely)
                    let alt = installed[0].browser;
                    eprintln!(
                        "{} is not installed. Try: browsectl --browser {} setup",
                        browser, alt
                    );
                }
            }

            // -- Persist setup info to .browsectl/setup.json --
            let setup_info = SetupInfo {
                platform: SetupPlatform {
                    os: platform.os.clone(),
                    arch: platform.arch.clone(),
                    display: platform.display.clone(),
                },
                browsers: browsers
                    .iter()
                    .map(|b| SetupBrowser {
                        browser: b.browser.to_string(),
                        path: b.path.clone(),
                        version: b.version.clone(),
                        major_version: b.major_version,
                        installed: b.installed,
                    })
                    .collect(),
                drivers: setup_drivers,
                ready_browser,
                ready_driver_path,
                updated_at: now_iso(),
            };
            if let Err(e) = store::write_setup_info(&setup_info).await {
                eprintln!("warning: failed to write setup info: {e}");
            } else {
                eprintln!("Setup info saved to {}", setup_info_path().display());
            }

            // -- Output JSON to stdout (consistent with all other commands) --
            print_json(&json!(setup_info));

            return Ok(());
        }

        _ => {} // handled below with a session
    }

    // ---- commands that need a live session ----
    let mut sessions: HashMap<String, RuntimeCtx> = HashMap::new();
    let session_id = resolve_or_create(&mut sessions, &config, cli.session.as_deref()).await?;

    // Take the RuntimeCtx out of the map for mutable access
    let mut ctx = sessions
        .remove(&session_id)
        .context("session resolved but not in memory")?;

    let result: Result<()> = async {
        match &cli.command {
            Commands::TabList => {
                let tabs = manager::list_tabs(&ctx).await?;
                print_json(&tabs);
            }

            Commands::TabCreate {
                url,
                alias,
                activate,
            } => {
                let result =
                    manager::create_tab(&mut ctx, url.as_deref(), alias.as_deref(), *activate)
                        .await?;
                manager::upsert_runtime(&ctx, false).await.ok();
                print_json(&result);
            }

            Commands::TabSwitch { tab } => {
                let result = manager::switch_tab(&ctx, &json!(tab)).await?;
                manager::upsert_runtime(&ctx, false).await.ok();
                print_json(&result);
            }

            Commands::TabClose { tab } => {
                let result = manager::close_tab(&mut ctx, &json!(tab)).await?;
                manager::upsert_runtime(&ctx, false).await.ok();
                print_json(&result);
            }

            Commands::Batch { file, name } => {
                let raw = tokio::fs::read_to_string(file)
                    .await
                    .with_context(|| format!("failed to read: {}", file.display()))?;
                let parsed: Value = serde_json::from_str(&raw)
                    .with_context(|| format!("invalid JSON: {}", file.display()))?;

                let batches = batch::normalize_batch_plan(parsed)?;

                if batches.len() > 1 && name.is_none() {
                    bail!("Multiple batches found; specify --name");
                }

                let selected: Vec<NamedBatch> = if let Some(target) = name.as_deref() {
                    batches.into_iter().filter(|b| b.name == target).collect()
                } else {
                    batches
                };

                if selected.is_empty() {
                    bail!("batch not found");
                }

                let mut all_ok = true;
                let mut reports = Vec::new();
                for b in selected {
                    let report = batch::execute_batch(&mut ctx, &b.commands).await;
                    let ok = report["ok"].as_bool().unwrap_or(false);
                    all_ok &= ok;
                    reports.push(json!({
                        "name": b.name,
                        "description": b.description,
                        "report": report,
                    }));
                    if !ok && !b.continue_on_error {
                        break;
                    }
                }

                print_json(&json!({"ok": all_ok, "batches": reports}));
                if !all_ok {
                    std::process::exit(1);
                }
            }

            Commands::Repl => {
                run_repl(&mut ctx).await?;
            }

            Commands::Run {
                command_type,
                selector,
                scope,
                fallback,
                url,
                text,
                path,
                ms,
                condition,
                attribute,
                value,
                timeout,
                interval,
                direction,
                amount,
                behavior,
                viewport,
                tab,
            } => {
                let cmd = CommandSpec {
                    command_type: command_type.clone(),
                    selector: selector.clone(),
                    scope: scope.clone(),
                    fallback: fallback.clone(),
                    url: url.clone(),
                    text: text.clone(),
                    path: path.clone(),
                    ms: *ms,
                    condition: condition.clone(),
                    attribute: attribute.clone(),
                    value: value.clone(),
                    timeout: *timeout,
                    interval: *interval,
                    direction: direction.clone(),
                    amount: *amount,
                    behavior: behavior.clone(),
                    viewport: viewport.as_ref().map(|raw| {
                        let (w, h) = parse_viewport_str(raw);
                        ViewportSpec {
                            width: w,
                            height: h,
                        }
                    }),
                    tab: tab.clone().map(Value::String),
                    ..Default::default()
                };
                let result = batch::execute_command(&mut ctx, &cmd).await?;
                print_json(&result);
            }

            // These were already handled above; keep match exhaustive
            Commands::Status
            | Commands::DriverStart
            | Commands::SessionCreate { .. }
            | Commands::SessionList
            | Commands::SessionUse { .. }
            | Commands::SessionDelete { .. }
            | Commands::Setup { .. }
            | Commands::Mcp => {}
        }
        Ok(())
    }
    .await;

    // Always persist session state before exiting.
    manager::upsert_runtime(&ctx, cli.session.is_none())
        .await
        .ok();

    // WdClient does NOT close the session on drop — the browser stays open.

    result
}
