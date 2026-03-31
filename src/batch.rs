//! Batch execution engine — runs sequences and parallel groups of browser commands.
//!
//! This module handles:
//! - **Plan normalization** ([`normalize_batch_plan`]): accepts multiple JSON
//!   formats (array, `{commands}`, `{batches}`) and normalises them into a
//!   uniform `Vec<NamedBatch>`.
//! - **Sequential execution** ([`execute_batch`]): runs commands one-by-one,
//!   collecting results and stopping on first error (unless `continueOnError`).
//! - **Single-command dispatch** ([`execute_command`]): routes a [`CommandSpec`]
//!   to the correct handler — tab management commands go to [`crate::manager`],
//!   everything else to [`crate::commands::execute_single_command`].
//! - **Parallel groups** ([`execute_parallel_groups`]): spawns each group as
//!   an independent `tokio::spawn` task sharing the same browser session via
//!   cloned [`WdClient`](crate::webdriver::WdClient) handles.

use std::future::Future;
use std::pin::Pin;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::commands;
use crate::manager;
use crate::types::*;

// ---------------------------------------------------------------------------
// Batch plan normalization
// ---------------------------------------------------------------------------

/// Normalize a parsed JSON value into a vector of [`NamedBatch`] entries.
///
/// Accepted formats:
/// - A plain JSON array of command objects → single batch named `"default"`.
/// - An object with a `"commands"` key → single batch (optional `name`,
///   `description`, `continueOnError`).
/// - An object with `"batches"` as an array of batch descriptors.
/// - An object with `"batches"` as a map (`{ name1: {…}, name2: {…} }`).
pub fn normalize_batch_plan(parsed: Value) -> Result<Vec<NamedBatch>> {
    // ── Plain array ────────────────────────────────────────────────────
    if parsed.is_array() {
        let commands: Vec<CommandSpec> =
            serde_json::from_value(parsed).context("failed to parse top-level command array")?;
        return Ok(vec![NamedBatch {
            name: "default".to_string(),
            description: String::new(),
            continue_on_error: false,
            commands,
        }]);
    }

    // ── Must be an object from here on ─────────────────────────────────
    if !parsed.is_object() {
        bail!("Batch file must be an array, {{ commands: [] }}, or {{ batches: [] | {{...}} }}");
    }

    // ── Object with "commands" ─────────────────────────────────────────
    if parsed.get("commands").is_some() {
        let name = parsed
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("default")
            .to_string();
        let description = parsed
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let continue_on_error = parsed
            .get("continueOnError")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let commands: Vec<CommandSpec> =
            serde_json::from_value(parsed.get("commands").unwrap().clone())
                .context("failed to parse commands array")?;

        return Ok(vec![NamedBatch {
            name,
            description,
            continue_on_error,
            commands,
        }]);
    }

    // ── Object with "batches" ──────────────────────────────────────────
    if let Some(batches_val) = parsed.get("batches") {
        // batches as array
        if batches_val.is_array() {
            let arr = batches_val.as_array().unwrap();
            let mut result = Vec::with_capacity(arr.len());
            for (i, entry) in arr.iter().enumerate() {
                let name = entry
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or(&format!("batch-{i}"))
                    .to_string();
                let description = entry
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let continue_on_error = entry
                    .get("continueOnError")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let commands: Vec<CommandSpec> = serde_json::from_value(
                    entry.get("commands").cloned().unwrap_or_else(|| json!([])),
                )
                .with_context(|| format!("failed to parse commands for batch index {i}"))?;

                result.push(NamedBatch {
                    name,
                    description,
                    continue_on_error,
                    commands,
                });
            }
            return Ok(result);
        }

        // batches as object/map
        if batches_val.is_object() {
            let map = batches_val.as_object().unwrap();
            let mut result = Vec::with_capacity(map.len());
            for (key, entry) in map.iter() {
                let description = entry
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let continue_on_error = entry
                    .get("continueOnError")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let commands: Vec<CommandSpec> = serde_json::from_value(
                    entry.get("commands").cloned().unwrap_or_else(|| json!([])),
                )
                .with_context(|| format!("failed to parse commands for batch '{key}'"))?;

                result.push(NamedBatch {
                    name: key.clone(),
                    description,
                    continue_on_error,
                    commands,
                });
            }
            return Ok(result);
        }
    }

    bail!("Batch file must be an array, {{ commands: [] }}, or {{ batches: [] | {{...}} }}");
}

// ---------------------------------------------------------------------------
// Batch execution
// ---------------------------------------------------------------------------

/// Execute a sequence of commands, collecting results into a JSON summary.
///
/// Execution stops at the first error unless the failing command has
/// `continue_on_error` set.
pub async fn execute_batch(ctx: &mut RuntimeCtx, commands: &[CommandSpec]) -> Value {
    let mut results: Vec<Value> = Vec::with_capacity(commands.len());
    let mut all_ok = true;

    for (i, cmd) in commands.iter().enumerate() {
        match execute_command(ctx, cmd).await {
            Ok(result) => {
                results.push(json!({
                    "index": i,
                    "ok": true,
                    "command": cmd.command_type,
                    "result": result,
                }));
            }
            Err(err) => {
                all_ok = false;
                results.push(json!({
                    "index": i,
                    "ok": false,
                    "command": cmd.command_type,
                    "error": format!("{err:#}"),
                }));
                if !cmd.continue_on_error {
                    break;
                }
            }
        }
    }

    json!({
        "ok": all_ok,
        "results": results,
    })
}

// ---------------------------------------------------------------------------
// Single command dispatch
// ---------------------------------------------------------------------------

/// Execute a single [`CommandSpec`], dispatching to the appropriate handler.
///
/// Returns a boxed future instead of being `async fn` in order to break the
/// recursive async type chain (`execute_command → execute_parallel_groups →
/// tokio::spawn → execute_batch → execute_command`).  Without the explicit
/// `Pin<Box<…>>`, the compiler cannot prove the future is `Send`.
pub fn execute_command<'a>(
    ctx: &'a mut RuntimeCtx,
    cmd: &'a CommandSpec,
) -> Pin<Box<dyn Future<Output = Result<Value>> + Send + 'a>> {
    Box::pin(async move {
        // Optional tab switch before executing the command.
        if let Some(ref tab_ref) = cmd.tab {
            ctx.switch_to_tab(tab_ref).await?;
        }

        match cmd.command_type.as_str() {
            "tab-list" => manager::list_tabs(ctx).await,

            "tab-create" => {
                manager::create_tab(
                    ctx,
                    cmd.url.as_deref(),
                    cmd.alias.as_deref(),
                    cmd.activate.unwrap_or(true),
                )
                .await
            }

            "tab-switch" => {
                manager::switch_tab(ctx, &cmd.tab.clone().unwrap_or(json!("current"))).await
            }

            "tab-close" => {
                manager::close_tab(ctx, &cmd.tab.clone().unwrap_or(json!("current"))).await
            }

            "parallel" => {
                execute_parallel_groups(ctx, &cmd.groups.clone().unwrap_or_default()).await
            }

            _ => commands::execute_single_command(&ctx.driver, cmd).await,
        }
    })
}

// ---------------------------------------------------------------------------
// Parallel group execution
// ---------------------------------------------------------------------------

/// Execute several [`ParallelGroup`]s concurrently.
///
/// Each group gets a **cloned** `WdClient` that shares the same underlying
/// browser session.  `WdClient` is cheap to clone (it wraps an
/// `Arc`-backed `reqwest::Client` plus two `String` fields) and does **not**
/// close the session on drop, so there is no need for `std::mem::forget` or
/// attach-from-store tricks.
pub async fn execute_parallel_groups(
    ctx: &mut RuntimeCtx,
    groups: &[ParallelGroup],
) -> Result<Value> {
    let mut handles = Vec::with_capacity(groups.len());

    for (idx, group) in groups.iter().enumerate() {
        let group_name = group.name.clone().unwrap_or_else(|| format!("group-{idx}"));
        let group_tab = group.tab.clone();
        let group_commands = group.commands.clone();

        // Clone the lightweight WdClient — same session, same connection pool.
        let driver = ctx.driver.clone();
        let session_id = ctx.session_id.clone();
        let server_url = ctx.server_url.clone();
        let tab_aliases = ctx.tab_aliases.clone();

        let task = tokio::spawn(async move {
            let mut worker_ctx = RuntimeCtx {
                driver,
                session_id,
                server_url,
                tab_aliases,
                temp_profile_dir: None,
                chromedriver_child: None,
            };

            // Switch to the group-level tab if specified.
            if let Some(ref tab_ref) = group_tab {
                if let Err(e) = worker_ctx.switch_to_tab(tab_ref).await {
                    return json!({
                        "name": group_name,
                        "ok": false,
                        "error": format!("failed to switch tab for parallel group: {e:#}"),
                    });
                }
            }

            let batch_result = execute_batch(&mut worker_ctx, &group_commands).await;

            // No need to forget the driver — WdClient does not close on drop.

            json!({
                "name": group_name,
                "ok": batch_result.get("ok").and_then(|v| v.as_bool()).unwrap_or(false),
                "results": batch_result.get("results").cloned().unwrap_or(json!([])),
            })
        });

        handles.push(task);
    }

    // Await all spawned tasks and collect results.
    let mut group_results = Vec::with_capacity(handles.len());
    let mut all_ok = true;

    for task in handles {
        match task.await {
            Ok(result) => {
                if !result.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
                    all_ok = false;
                }
                group_results.push(result);
            }
            Err(e) => {
                all_ok = false;
                group_results.push(json!({
                    "ok": false,
                    "error": format!("task join error: {e:#}"),
                }));
            }
        }
    }

    Ok(json!({
        "ok": all_ok,
        "type": "parallel",
        "mode": "optimistic-parallel",
        "groups": group_results,
    }))
}
