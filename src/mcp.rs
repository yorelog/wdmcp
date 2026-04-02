//! MCP (Model Context Protocol) server — JSON-RPC 2.0 over NDJSON on stdin/stdout.
//!
//! Exposes the full browsectl feature set as MCP tools so AI clients (e.g. Claude,
//! Cursor) can drive a browser through the standard MCP transport.
//!
//! **Tool categories:**
//! - Session management: `create_session`, `list_sessions`, `use_session`,
//!   `delete_session`, `driver_status`.
//! - Browser commands: `open`, `click`, `fill`, `paste`, `screenshot`,
//!   `scroll`, `get_title`, `get_last_message`, `wait_for`.
//! - Tab management: `list_tabs`, `create_tab`, `switch_tab`, `close_tab`.
//! - Power-user: `run_command` (arbitrary [`CommandSpec`]), `run_batch`.
//!
//! The server handles the MCP lifecycle (`initialize` → tool calls → shutdown)
//! and maintains an in-memory map of live [`RuntimeCtx`] sessions.

use std::collections::HashMap;

use anyhow::{Context, Result, bail};
use base64::Engine;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::agent;
use crate::batch;
use crate::driver;
use crate::manager;
use crate::store;
use crate::types::*;

// ---------------------------------------------------------------------------
// Tool definitions
// ---------------------------------------------------------------------------

fn tool_definitions() -> Value {
    let mut tools = Vec::new();
    tools.extend(session_tool_defs());
    tools.extend(browser_command_tool_defs());
    tools.extend(tab_tool_defs());
    tools.extend(power_user_tool_defs());
    tools.extend(agent_tool_defs());
    tools.extend(network_tool_defs());
    Value::Array(tools)
}

/// Session / driver management tools.
fn session_tool_defs() -> Vec<Value> {
    vec![
        json!({
            "name": "driver_status",
            "description": "Check whether chromedriver is running and return its /status response.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "required": [],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "create_session",
            "description": "Create a new browser session (launches Chrome via chromedriver).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "headless": {
                        "type": "boolean",
                        "description": "Run Chrome in headless mode."
                    },
                    "viewport": {
                        "type": "object",
                        "description": "Initial viewport size.",
                        "properties": {
                            "width":  { "type": "integer" },
                            "height": { "type": "integer" }
                        }
                    }
                },
                "required": [],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "list_sessions",
            "description": "List all persisted browser sessions and the current default.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "required": [],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "use_session",
            "description": "Set a session as the default active session.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "sessionId": {
                        "type": "string",
                        "description": "The session ID to set as default."
                    }
                },
                "required": ["sessionId"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "delete_session",
            "description": "Delete a browser session (quits the browser).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "sessionId": {
                        "type": "string",
                        "description": "Session to delete. Defaults to the current default session."
                    }
                },
                "required": [],
                "additionalProperties": false
            }
        }),
    ]
}

/// First-class browser command tools.
fn browser_command_tool_defs() -> Vec<Value> {
    vec![
        json!({
            "name": "open",
            "description": "Navigate the current tab to a URL. Bare domains like \"doubao.com\" are auto-prefixed with https://.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "The URL to navigate to (e.g. \"https://example.com\" or \"example.com\")."
                    },
                    "viewport": {
                        "type": "object",
                        "description": "Optional: resize the viewport after navigation.",
                        "properties": {
                            "width":  { "type": "integer" },
                            "height": { "type": "integer" }
                        }
                    },
                    "sessionId": {
                        "type": "string",
                        "description": "Target session. Defaults to the current default session."
                    }
                },
                "required": ["url"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "click",
            "description": "Click a DOM element by CSS selector. If the click is intercepted by an overlay, automatically falls back to JS click strategies (parent > sibling > self).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "selector": {
                        "type": "string",
                        "description": "CSS selector of the element to click. Supports custom syntax: CSS::text(/pattern/flags)."
                    },
                    "scope": {
                        "type": "string",
                        "description": "Optional CSS selector of a scope root. Element lookup and fallback selectors are restricted to descendants of this scope."
                    },
                    "fallback": {
                        "type": "string",
                        "description": "Fallback strategy when native click is intercepted. Values: \"parent\", \"sibling\", or a CSS selector string. Omit for automatic smart fallback."
                    },
                    "timeout": {
                        "type": "integer",
                        "description": "Max time in ms to wait for the element to appear. Default: 20000."
                    },
                    "sessionId": {
                        "type": "string",
                        "description": "Target session. Defaults to the current default session."
                    }
                },
                "required": ["selector"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "fill",
            "description": "Clear an input field and type text into it character by character. If no selector is given, targets the currently focused element.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "text": {
                        "type": "string",
                        "description": "The text to type into the field."
                    },
                    "selector": {
                        "type": "string",
                        "description": "CSS selector of the input element. Supports CSS::text(/pattern/flags). Omit to use the focused element."
                    },
                    "scope": {
                        "type": "string",
                        "description": "Optional CSS selector of a scope root. Selector lookup is limited to descendants of this scope."
                    },
                    "timeout": {
                        "type": "integer",
                        "description": "Max time in ms to wait for the element. Default: 20000."
                    },
                    "sessionId": {
                        "type": "string",
                        "description": "Target session. Defaults to the current default session."
                    }
                },
                "required": ["text"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "paste",
            "description": "Paste text into an input field via clipboard simulation (faster than fill for large text). If no selector is given, targets the currently focused element.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "text": {
                        "type": "string",
                        "description": "The text to paste."
                    },
                    "selector": {
                        "type": "string",
                        "description": "CSS selector of the input element. Supports CSS::text(/pattern/flags). Omit to use the focused element."
                    },
                    "scope": {
                        "type": "string",
                        "description": "Optional CSS selector of a scope root. Selector lookup is limited to descendants of this scope."
                    },
                    "timeout": {
                        "type": "integer",
                        "description": "Max time in ms to wait for the element. Default: 20000."
                    },
                    "sessionId": {
                        "type": "string",
                        "description": "Target session. Defaults to the current default session."
                    }
                },
                "required": ["text"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "screenshot",
            "description": "Take a screenshot of a DOM element and save it to a file. Optionally include base64 inline content for MCP clients that cannot access local files.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "selector": {
                        "type": "string",
                        "description": "CSS selector of the element to capture. Supports CSS::text(/pattern/flags)."
                    },
                    "scope": {
                        "type": "string",
                        "description": "Optional CSS selector of a scope root. Selector lookup is limited to descendants of this scope."
                    },
                    "path": {
                        "type": "string",
                        "description": "File path to save the PNG screenshot. Default: \"outputs/screenshot.png\"."
                    },
                    "inline": {
                        "type": "boolean",
                        "description": "If true, read the saved PNG and return base64 content inline in the MCP result."
                    },
                    "timeout": {
                        "type": "integer",
                        "description": "Max time in ms to wait for the element. Default: 20000."
                    },
                    "sessionId": {
                        "type": "string",
                        "description": "Target session. Defaults to the current default session."
                    }
                },
                "required": ["selector"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "scroll",
            "description": "Scroll the page or a specific element.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "direction": {
                        "type": "string",
                        "description": "Scroll direction. Default: \"down\".",
                        "enum": ["up", "down", "left", "right"]
                    },
                    "amount": {
                        "type": "integer",
                        "description": "Scroll distance in pixels. Default: 800."
                    },
                    "selector": {
                        "type": "string",
                        "description": "CSS selector of the element to scroll. Omit to scroll the window."
                    },
                    "behavior": {
                        "type": "string",
                        "description": "Scroll behavior. Default: \"smooth\".",
                        "enum": ["smooth", "auto"]
                    },
                    "sessionId": {
                        "type": "string",
                        "description": "Target session. Defaults to the current default session."
                    }
                },
                "required": [],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "get_title",
            "description": "Get the title of the current page.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "sessionId": {
                        "type": "string",
                        "description": "Target session. Defaults to the current default session."
                    }
                },
                "required": [],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "get_last_message",
            "description": "Extract the content (text, HTML, images, links) of the last message block on the page. Useful for chat interfaces.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "selector": {
                        "type": "string",
                        "description": "CSS selector for message block containers. Default: [data-testid=\"message-block-container\"]."
                    },
                    "sessionId": {
                        "type": "string",
                        "description": "Target session. Defaults to the current default session."
                    }
                },
                "required": [],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "wait_for",
            "description": "Wait for a condition to be met before continuing.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "condition": {
                        "type": "string",
                        "description": "What to wait for.",
                        "enum": ["visible", "hidden", "url", "title", "js"]
                    },
                    "selector": {
                        "type": "string",
                        "description": "CSS selector (for \"visible\" and \"hidden\" conditions)."
                    },
                    "value": {
                        "type": "string",
                        "description": "Expected value: a substring for url/title, or a JS expression that returns truthy for \"js\"."
                    },
                    "timeout": {
                        "type": "integer",
                        "description": "Max time in ms to wait. Default: 20000."
                    },
                    "interval": {
                        "type": "integer",
                        "description": "Polling interval in ms. Default: 250."
                    },
                    "sessionId": {
                        "type": "string",
                        "description": "Target session. Defaults to the current default session."
                    }
                },
                "required": ["condition"],
                "additionalProperties": false
            }
        }),
    ]
}

/// Tab management tools.
fn tab_tool_defs() -> Vec<Value> {
    vec![
        json!({
            "name": "list_tabs",
            "description": "List all open tabs/windows in a browser session.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "sessionId": {
                        "type": "string",
                        "description": "Target session. Defaults to the current default session."
                    }
                },
                "required": [],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "create_tab",
            "description": "Open a new browser tab, optionally navigating to a URL.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "URL to open in the new tab."
                    },
                    "alias": {
                        "type": "string",
                        "description": "Human-friendly alias for the new tab."
                    },
                    "activate": {
                        "type": "boolean",
                        "description": "Whether to switch focus to the new tab."
                    },
                    "sessionId": {
                        "type": "string",
                        "description": "Target session. Defaults to the current default session."
                    }
                },
                "required": [],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "switch_tab",
            "description": "Switch to a different tab by numeric index (0-based), alias string, or window handle id.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "tab": {
                        "description": "Tab reference: numeric index (0-based), alias string, or window handle id."
                    },
                    "sessionId": {
                        "type": "string",
                        "description": "Target session. Defaults to the current default session."
                    }
                },
                "required": ["tab"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "close_tab",
            "description": "Close a tab by numeric index (0-based), alias string, or window handle id.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "tab": {
                        "description": "Tab reference: numeric index (0-based), alias string, or window handle id."
                    },
                    "sessionId": {
                        "type": "string",
                        "description": "Target session. Defaults to the current default session."
                    }
                },
                "required": ["tab"],
                "additionalProperties": false
            }
        }),
    ]
}

/// Power-user escape-hatch tools (run_command, run_batch).
/// Agent intelligence tools — DOM analysis, slot extraction, task recommendation.
fn agent_tool_defs() -> Vec<Value> {
    vec![
        json!({
            "name": "analyze_page",
            "description": "Extract all interactive elements (slots) from the current page with safety classification. Returns structured slot data including selectors, categories, safety levels, form membership, and task suggestions. Use this to understand what actions are possible on the current page.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "sessionId": {
                        "type": "string",
                        "description": "Target session. Defaults to the current default session."
                    },
                    "include_hidden": {
                        "type": "boolean",
                        "description": "Include hidden/invisible elements in the analysis. Default: false."
                    },
                    "include_suggestions": {
                        "type": "boolean",
                        "description": "Include task suggestions grouped from the extracted slots. Default: true."
                    }
                },
                "required": [],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "suggest_actions",
            "description": "Get recommended actions for the current page by combining DOM analysis with task memory. Returns a list of suggested tasks the user can perform, ranked by relevance and safety. Useful when the user doesn't know what to do on a page.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "sessionId": {
                        "type": "string",
                        "description": "Target session. Defaults to the current default session."
                    },
                    "max_suggestions": {
                        "type": "integer",
                        "description": "Maximum number of suggestions to return. Default: 5."
                    }
                },
                "required": [],
                "additionalProperties": false
            }
        }),
    ]
}

/// Network monitoring tools — capture, inspect, and filter HTTP traffic.
fn network_tool_defs() -> Vec<Value> {
    vec![
        json!({
            "name": "network_enable",
            "description": "Start capturing network traffic on the current page. Injects a JavaScript interceptor that wraps fetch/XHR to record all HTTP requests and responses, and optionally enables CDP Network domain for deeper introspection. Call this before navigating or performing actions whose network activity you want to inspect.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "sessionId": {
                        "type": "string",
                        "description": "Target session. Defaults to the current default session."
                    }
                },
                "required": [],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "network_disable",
            "description": "Stop capturing network traffic and restore original fetch/XHR functions.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "sessionId": {
                        "type": "string",
                        "description": "Target session. Defaults to the current default session."
                    }
                },
                "required": [],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "network_get_log",
            "description": "Retrieve captured network requests with optional filtering. Returns a structured log of HTTP traffic including method, URL, status, headers, timing, and content metadata. Use filters to narrow results by URL pattern, HTTP method, resource type, or status code.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "sessionId": {
                        "type": "string",
                        "description": "Target session. Defaults to the current default session."
                    },
                    "url_pattern": {
                        "type": "string",
                        "description": "Filter entries whose URL contains this substring."
                    },
                    "methods": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Filter by HTTP methods, e.g. [\"GET\", \"POST\"]."
                    },
                    "resource_types": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Filter by resource type, e.g. [\"xhr\", \"fetch\", \"script\", \"document\", \"image\", \"stylesheet\"]."
                    },
                    "status_min": {
                        "type": "integer",
                        "description": "Minimum HTTP status code (inclusive). Use with status_max for range filtering."
                    },
                    "status_max": {
                        "type": "integer",
                        "description": "Maximum HTTP status code (inclusive)."
                    },
                    "has_error": {
                        "type": "boolean",
                        "description": "If true, return only failed requests (status >= 400 or status = 0)."
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of entries to return."
                    }
                },
                "required": [],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "network_get_response_body",
            "description": "Retrieve the response body of a specific captured network request by its entry ID (e.g. \"net-0\"). The request must have been captured by the network interceptor.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "sessionId": {
                        "type": "string",
                        "description": "Target session. Defaults to the current default session."
                    },
                    "request_id": {
                        "type": "string",
                        "description": "The entry ID of the request (e.g. \"net-0\", \"net-5\")."
                    }
                },
                "required": ["request_id"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "network_clear_log",
            "description": "Clear all captured network entries. Useful to reset before a new action so you only capture fresh traffic.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "sessionId": {
                        "type": "string",
                        "description": "Target session. Defaults to the current default session."
                    }
                },
                "required": [],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "network_get_resource_timing",
            "description": "Get resource loading performance timing data from the browser's Performance API. Does NOT require network_enable — works with the browser's built-in performance entries. Returns timing breakdown (DNS, connect, SSL, TTFB, download) for all loaded resources.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "sessionId": {
                        "type": "string",
                        "description": "Target session. Defaults to the current default session."
                    }
                },
                "required": [],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "network_get_cookies",
            "description": "Get all cookies for the current page. Uses CDP Network.getAllCookies when available, with a JavaScript document.cookie fallback.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "sessionId": {
                        "type": "string",
                        "description": "Target session. Defaults to the current default session."
                    }
                },
                "required": [],
                "additionalProperties": false
            }
        }),
    ]
}

fn power_user_tool_defs() -> Vec<Value> {
    let cmd_type_enum = json!([
        "open",
        "click",
        "fill",
        "paste",
        "screenshot",
        "scroll",
        "title",
        "last-message-content",
        "wait",
        "tab-list",
        "tab-create",
        "tab-switch",
        "tab-close",
        "parallel"
    ]);

    vec![
        json!({
            "name": "run_command",
            "description": "Execute a single WebDriver command in a browser session. Use this for advanced commands not covered by the first-class tools.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "command": {
                        "type": "object",
                        "description": "A command spec object. The \"type\" field selects the command. Supported types: open, click, fill, paste, screenshot, scroll, title, last-message-content, wait, tab-list, tab-create, tab-switch, tab-close, parallel.",
                        "properties": {
                            "type": { "type": "string", "enum": cmd_type_enum },
                            "selector": { "type": "string" },
                            "fallback": { "type": "string" },
                            "url": { "type": "string" },
                            "text": { "type": "string" },
                            "path": { "type": "string" },
                            "ms": { "type": "integer" },
                            "condition": { "type": "string" },
                            "attribute": { "type": "string" },
                            "value": { "type": "string" },
                            "timeout": { "type": "integer" },
                            "interval": { "type": "integer" },
                            "direction": { "type": "string" },
                            "amount": { "type": "integer" },
                            "behavior": { "type": "string" },
                            "tab": { "description": "Tab reference." },
                            "continueOnError": { "type": "boolean" },
                            "alias": { "type": "string" },
                            "activate": { "type": "boolean" }
                        },
                        "required": ["type"]
                    },
                    "sessionId": {
                        "type": "string",
                        "description": "Target session. Defaults to the current default session."
                    }
                },
                "required": ["command"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "run_batch",
            "description": "Execute a batch of WebDriver commands sequentially. Stops on first error unless the failing command has continueOnError set.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "commands": {
                        "type": "array",
                        "description": "Array of command spec objects (same schema as run_command's \"command\" parameter).",
                        "items": {
                            "type": "object",
                            "properties": {
                                "type": { "type": "string", "enum": cmd_type_enum }
                            },
                            "required": ["type"]
                        }
                    },
                    "sessionId": {
                        "type": "string",
                        "description": "Target session. Defaults to the current default session."
                    }
                },
                "required": ["commands"],
                "additionalProperties": false
            }
        }),
    ]
}

// ---------------------------------------------------------------------------
// Response helpers
// ---------------------------------------------------------------------------

async fn write_response(stdout: &mut tokio::io::Stdout, value: &Value) -> Result<()> {
    let json = serde_json::to_string(value)?;
    stdout.write_all(json.as_bytes()).await?;
    stdout.write_all(b"\n").await?;
    stdout.flush().await?;
    Ok(())
}

fn make_success(id: &Value, result: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result,
    })
}

fn make_error(id: &Value, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": code,
            "message": message,
        },
    })
}

fn make_tool_result(id: &Value, data: &Value, is_error: bool) -> Value {
    let text = serde_json::to_string_pretty(data).unwrap_or_else(|_| data.to_string());
    let content = json!({
        "type": "text",
        "text": text,
    });
    let mut result = json!({
        "content": [content],
    });
    if is_error {
        result["isError"] = json!(true);
    }
    make_success(id, result)
}

// ---------------------------------------------------------------------------
// Helper: resolve session + get mutable RuntimeCtx
// ---------------------------------------------------------------------------

/// Resolve a session id from the arguments and return it.
async fn resolve_sid(
    sessions: &mut HashMap<String, RuntimeCtx>,
    config: &SessionConfig,
    args: &Value,
) -> Result<String> {
    let session_id_arg = args.get("sessionId").and_then(|v| v.as_str());
    manager::resolve_session(sessions, config, session_id_arg, true).await
}

/// Build a `CommandSpec` from flat tool arguments, filling in the `command_type`
/// and any relevant fields.
fn build_command_spec(command_type: &str, args: &Value) -> CommandSpec {
    eprintln!(
        "[mcp] build_command_spec type={} args_keys={}",
        command_type,
        args.as_object()
            .map(|o| o.keys().cloned().collect::<Vec<_>>().join(","))
            .unwrap_or_else(|| "<non-object>".to_string())
    );
    CommandSpec {
        command_type: command_type.to_string(),
        selector: args
            .get("selector")
            .and_then(|v| v.as_str())
            .map(String::from),
        scope: args.get("scope").and_then(|v| v.as_str()).map(String::from),
        fallback: args
            .get("fallback")
            .and_then(|v| v.as_str())
            .map(String::from),
        url: args.get("url").and_then(|v| v.as_str()).map(String::from),
        text: args.get("text").and_then(|v| v.as_str()).map(String::from),
        path: args.get("path").and_then(|v| v.as_str()).map(String::from),
        ms: args.get("ms").and_then(|v| v.as_u64()),
        condition: args
            .get("condition")
            .and_then(|v| v.as_str())
            .map(String::from),
        attribute: args
            .get("attribute")
            .and_then(|v| v.as_str())
            .map(String::from),
        value: args.get("value").and_then(|v| v.as_str()).map(String::from),
        timeout: args.get("timeout").and_then(|v| v.as_u64()),
        interval: args.get("interval").and_then(|v| v.as_u64()),
        direction: args
            .get("direction")
            .and_then(|v| v.as_str())
            .map(String::from),
        amount: args.get("amount").and_then(|v| v.as_i64()),
        behavior: args
            .get("behavior")
            .and_then(|v| v.as_str())
            .map(String::from),
        tab: args.get("tab").cloned(),
        viewport: args
            .get("viewport")
            .and_then(|v| serde_json::from_value::<ViewportSpec>(v.clone()).ok()),
        continue_on_error: args
            .get("continueOnError")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        alias: args.get("alias").and_then(|v| v.as_str()).map(String::from),
        activate: args.get("activate").and_then(|v| v.as_bool()),
        groups: None,
    }
}

// ---------------------------------------------------------------------------
// Extracted helpers for agent / network tools
// ---------------------------------------------------------------------------

/// Build a [`NetworkFilter`] from the flat JSON args of the `network_get_log` tool.
fn parse_network_filter(args: &Value) -> agent::network::NetworkFilter {
    let status_range = match (
        args.get("status_min").and_then(|v| v.as_u64()),
        args.get("status_max").and_then(|v| v.as_u64()),
    ) {
        (Some(min), Some(max)) => Some((min as u16, max as u16)),
        (Some(min), None) => Some((min as u16, 599)),
        (None, Some(max)) => Some((100, max as u16)),
        (None, None) => None,
    };

    agent::network::NetworkFilter {
        url_pattern: args
            .get("url_pattern")
            .and_then(|v| v.as_str())
            .map(String::from),
        methods: args.get("methods").and_then(|v| {
            v.as_array().map(|arr| {
                arr.iter()
                    .filter_map(|item| item.as_str().map(|s| s.to_uppercase()))
                    .collect()
            })
        }),
        resource_types: args.get("resource_types").and_then(|v| {
            v.as_array().map(|arr| {
                arr.iter()
                    .filter_map(|item| item.as_str().map(|s| s.to_lowercase()))
                    .collect()
            })
        }),
        status_range,
        has_error: args.get("has_error").and_then(|v| v.as_bool()),
        limit: args
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize),
    }
}

/// Handle the `analyze_page` tool: extract DOM slots, optionally filter hidden,
/// attach suggestions, and record the visit in memory.
async fn handle_analyze_page(driver: &crate::webdriver::WdClient, args: &Value) -> Result<Value> {
    let include_hidden = args
        .get("include_hidden")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let include_suggestions = args
        .get("include_suggestions")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);

    let mut analysis = agent::dom::extract_slots(driver).await?;

    // Filter out hidden slots unless requested
    if !include_hidden {
        analysis.slots.retain(|s| s.visible);
        analysis.safety_summary = agent::dom::build_safety_summary(&analysis.slots);
        analysis.slot_count = analysis.slots.len();
    }

    let mut result =
        serde_json::to_value(&analysis).context("failed to serialize page analysis")?;

    if include_suggestions {
        let suggestions = agent::slots::group_suggestions(&analysis);
        result["suggestions"] = serde_json::to_value(&suggestions).unwrap_or(json!([]));
    }

    // Record visit in memory (best-effort, don't fail the tool call)
    let _ =
        agent::memory::record_visit(&analysis.url, &analysis.title, analysis.slot_count, vec![])
            .await;

    Ok(result)
}

/// Handle the `suggest_actions` tool: analyse the page, enrich with memory
/// patterns, and return capped suggestions.
async fn handle_suggest_actions(
    driver: &crate::webdriver::WdClient,
    args: &Value,
) -> Result<Value> {
    let max_suggestions = args
        .get("max_suggestions")
        .and_then(|v| v.as_u64())
        .unwrap_or(5) as usize;

    // 1. Analyze the current page
    let mut analysis = agent::dom::extract_slots(driver).await?;
    analysis.slots.retain(|s| s.visible);
    analysis.slot_count = analysis.slots.len();

    // 2. Get DOM-based suggestions
    let mut suggestions = agent::slots::group_suggestions(&analysis);

    // 3. Enrich with memory patterns
    let memory_patterns = agent::memory::find_patterns(&analysis.url)
        .await
        .unwrap_or_default();

    for pattern in &memory_patterns {
        // Check if this pattern's intent already exists in suggestions
        let already_exists = suggestions.iter().any(|s| {
            s.title
                .to_lowercase()
                .contains(&pattern.intent.to_lowercase())
        });

        if !already_exists {
            suggestions.push(agent::slots::TaskSuggestion {
                title: format!("{} (from memory)", pattern.intent),
                description: format!(
                    "Previously successful task (used {} time{})",
                    pattern.success_count,
                    if pattern.success_count == 1 { "" } else { "s" }
                ),
                safety_level: agent::slots::SafetyLevel::Interact,
                slot_ids: vec![],
                commands: pattern.commands.clone(),
            });
        }
    }

    // 4. Truncate to max
    suggestions.truncate(max_suggestions);

    // 5. Record visit
    let _ = agent::memory::record_visit(
        &analysis.url,
        &analysis.title,
        analysis.slot_count,
        vec!["suggest_actions".to_string()],
    )
    .await;

    Ok(json!({
        "url": analysis.url,
        "title": analysis.title,
        "slot_count": analysis.slot_count,
        "suggestions": suggestions,
        "memory_patterns_found": memory_patterns.len(),
    }))
}

// ---------------------------------------------------------------------------
// Main match dispatcher
// ---------------------------------------------------------------------------

async fn handle_tool_call(
    sessions: &mut HashMap<String, RuntimeCtx>,
    config: &SessionConfig,
    tool_name: &str,
    args: &Value,
) -> Result<Value> {
    eprintln!(
        "[mcp] tools/call name={} args={}",
        tool_name,
        serde_json::to_string(args).unwrap_or_else(|_| "<args-serialize-error>".to_string())
    );
    match tool_name {
        // ── driver_status ──────────────────────────────────────────────
        "driver_status" => match driver::fetch_status(&config.server_url).await {
            Ok(status) => Ok(json!({ "ok": true, "status": status })),
            Err(e) => Ok(json!({ "ok": false, "error": format!("{e:#}") })),
        },

        // ── create_session ─────────────────────────────────────────────
        "create_session" => {
            let headless = args
                .get("headless")
                .and_then(|v| v.as_bool())
                .unwrap_or(config.headless);

            let viewport_width = args
                .get("viewport")
                .and_then(|v| v.get("width"))
                .and_then(|v| v.as_i64())
                .unwrap_or(config.viewport_width);

            let viewport_height = args
                .get("viewport")
                .and_then(|v| v.get("height"))
                .and_then(|v| v.as_i64())
                .unwrap_or(config.viewport_height);

            let session_config = SessionConfig {
                browser: config.browser,
                server_url: config.server_url.clone(),
                chromedriver_path: config.chromedriver_path.clone(),
                chrome_binary: config.chrome_binary.clone(),
                user_data_dir: config.user_data_dir.clone(),
                profile_directory: config.profile_directory.clone(),
                headless,
                viewport_width,
                viewport_height,
                copy_data: config.copy_data.clone(),
            };

            let ctx = manager::create_session(&session_config).await?;
            let session_id = ctx.session_id.clone();
            sessions.insert(session_id.clone(), ctx);

            Ok(json!({ "ok": true, "sessionId": session_id }))
        }

        // ── list_sessions ──────────────────────────────────────────────
        "list_sessions" => {
            let store_data = store::read_store().await?;
            Ok(json!({
                "defaultSessionId": store_data.default_session_id,
                "sessions": store_data.sessions,
            }))
        }

        // ── use_session ────────────────────────────────────────────────
        "use_session" => {
            let session_id = args
                .get("sessionId")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("missing required parameter: sessionId"))?;

            store::set_default(session_id).await?;

            Ok(json!({ "ok": true, "defaultSessionId": session_id }))
        }

        // ── delete_session ─────────────────────────────────────────────
        "delete_session" => {
            let session_id = args.get("sessionId").and_then(|v| v.as_str());
            let result = manager::delete_session(sessions, session_id).await?;
            Ok(result)
        }

        // ================================================================
        // First-class browser command tools
        // ================================================================

        // ── open ───────────────────────────────────────────────────────
        "open" => {
            let sid = resolve_sid(sessions, config, args).await?;
            let cmd = build_command_spec("open", args);
            let ctx = sessions
                .get_mut(&sid)
                .ok_or_else(|| anyhow::anyhow!("session not found: {sid}"))?;
            let result = batch::execute_command(ctx, &cmd).await?;
            manager::upsert_runtime(ctx, false).await?;
            Ok(result)
        }

        // ── click ──────────────────────────────────────────────────────
        "click" => {
            let sid = resolve_sid(sessions, config, args).await?;
            let cmd = build_command_spec("click", args);
            let ctx = sessions
                .get_mut(&sid)
                .ok_or_else(|| anyhow::anyhow!("session not found: {sid}"))?;
            let result = batch::execute_command(ctx, &cmd).await?;
            manager::upsert_runtime(ctx, false).await?;
            Ok(result)
        }

        // ── fill ───────────────────────────────────────────────────────
        "fill" => {
            let sid = resolve_sid(sessions, config, args).await?;
            let cmd = build_command_spec("fill", args);
            let ctx = sessions
                .get_mut(&sid)
                .ok_or_else(|| anyhow::anyhow!("session not found: {sid}"))?;
            let result = batch::execute_command(ctx, &cmd).await?;
            manager::upsert_runtime(ctx, false).await?;
            Ok(result)
        }

        // ── paste ──────────────────────────────────────────────────────
        "paste" => {
            let sid = resolve_sid(sessions, config, args).await?;
            let cmd = build_command_spec("paste", args);
            let ctx = sessions
                .get_mut(&sid)
                .ok_or_else(|| anyhow::anyhow!("session not found: {sid}"))?;
            let result = batch::execute_command(ctx, &cmd).await?;
            manager::upsert_runtime(ctx, false).await?;
            Ok(result)
        }

        // ── screenshot ─────────────────────────────────────────────────
        "screenshot" => {
            let sid = resolve_sid(sessions, config, args).await?;
            eprintln!("[mcp] screenshot resolved session={sid}");

            let inline = args
                .get("inline")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let cmd = build_command_spec("screenshot", args);
            let ctx = sessions
                .get_mut(&sid)
                .ok_or_else(|| anyhow::anyhow!("session not found: {sid}"))?;

            eprintln!(
                "[mcp] screenshot executing selector={} path={} inline={}",
                cmd.selector.as_deref().unwrap_or("<none>"),
                cmd.path.as_deref().unwrap_or("outputs/screenshot.png"),
                inline
            );

            let result = batch::execute_command(ctx, &cmd).await?;
            manager::upsert_runtime(ctx, false).await?;

            let path = result
                .get("path")
                .and_then(|v| v.as_str())
                .map(String::from)
                .or_else(|| cmd.path.clone())
                .unwrap_or_else(|| "outputs/screenshot.png".to_string());

            let uri = if path.starts_with('/') {
                format!("file://{path}")
            } else {
                format!("file://{}/{}", std::env::current_dir()?.display(), path)
            };

            let mut merged = json!({
                "ok": true,
                "type": "screenshot",
                "saved": true,
                "path": path,
                "uri": uri,
                "mime": "image/png"
            });

            if inline {
                let image_path = merged
                    .get("path")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("screenshot: missing output path"))?;

                eprintln!("[mcp] screenshot inline read path={image_path}");
                let bytes = tokio::fs::read(image_path).await.with_context(|| {
                    format!("screenshot: failed to read file for inline payload: {image_path}")
                })?;
                let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);

                merged["inline"] = json!(true);
                merged["base64"] = json!(b64);
                eprintln!("[mcp] screenshot inline payload attached");
            } else {
                merged["inline"] = json!(false);
            }

            Ok(merged)
        }

        // ── scroll ─────────────────────────────────────────────────────
        "scroll" => {
            let sid = resolve_sid(sessions, config, args).await?;
            let cmd = build_command_spec("scroll", args);
            let ctx = sessions
                .get_mut(&sid)
                .ok_or_else(|| anyhow::anyhow!("session not found: {sid}"))?;
            let result = batch::execute_command(ctx, &cmd).await?;
            manager::upsert_runtime(ctx, false).await?;
            Ok(result)
        }

        // ── get_title ──────────────────────────────────────────────────
        "get_title" => {
            let sid = resolve_sid(sessions, config, args).await?;
            let cmd = build_command_spec("title", args);
            let ctx = sessions
                .get_mut(&sid)
                .ok_or_else(|| anyhow::anyhow!("session not found: {sid}"))?;
            let result = batch::execute_command(ctx, &cmd).await?;
            Ok(result)
        }

        // ── get_last_message ───────────────────────────────────────────
        "get_last_message" => {
            let sid = resolve_sid(sessions, config, args).await?;
            let cmd = build_command_spec("last-message-content", args);
            let ctx = sessions
                .get_mut(&sid)
                .ok_or_else(|| anyhow::anyhow!("session not found: {sid}"))?;
            let result = batch::execute_command(ctx, &cmd).await?;
            Ok(result)
        }

        // ── wait_for ───────────────────────────────────────────────────
        "wait_for" => {
            let sid = resolve_sid(sessions, config, args).await?;
            let cmd = build_command_spec("wait", args);
            let ctx = sessions
                .get_mut(&sid)
                .ok_or_else(|| anyhow::anyhow!("session not found: {sid}"))?;
            let result = batch::execute_command(ctx, &cmd).await?;
            Ok(result)
        }

        // ================================================================
        // Tab management tools
        // ================================================================

        // ── list_tabs ──────────────────────────────────────────────────
        "list_tabs" => {
            let sid = resolve_sid(sessions, config, args).await?;
            let ctx = sessions
                .get(&sid)
                .ok_or_else(|| anyhow::anyhow!("session not found: {sid}"))?;
            let result = manager::list_tabs(ctx).await?;
            Ok(result)
        }

        // ── create_tab ─────────────────────────────────────────────────
        "create_tab" => {
            let sid = resolve_sid(sessions, config, args).await?;

            let url = args.get("url").and_then(|v| v.as_str());
            let alias = args.get("alias").and_then(|v| v.as_str());
            let activate = args
                .get("activate")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);

            let ctx = sessions
                .get_mut(&sid)
                .ok_or_else(|| anyhow::anyhow!("session not found: {sid}"))?;

            let result = manager::create_tab(ctx, url, alias, activate).await?;

            manager::upsert_runtime(ctx, false).await?;

            Ok(result)
        }

        // ── switch_tab ─────────────────────────────────────────────────
        "switch_tab" => {
            let sid = resolve_sid(sessions, config, args).await?;

            let tab_ref = args
                .get("tab")
                .ok_or_else(|| anyhow::anyhow!("missing required parameter: tab"))?;

            let ctx = sessions
                .get(&sid)
                .ok_or_else(|| anyhow::anyhow!("session not found: {sid}"))?;

            let result = manager::switch_tab(ctx, tab_ref).await?;

            manager::upsert_runtime(ctx, false).await?;

            Ok(result)
        }

        // ── close_tab ──────────────────────────────────────────────────
        "close_tab" => {
            let sid = resolve_sid(sessions, config, args).await?;

            let tab_ref = args
                .get("tab")
                .ok_or_else(|| anyhow::anyhow!("missing required parameter: tab"))?;

            let ctx = sessions
                .get_mut(&sid)
                .ok_or_else(|| anyhow::anyhow!("session not found: {sid}"))?;

            let result = manager::close_tab(ctx, tab_ref).await?;

            manager::upsert_runtime(ctx, false).await?;

            Ok(result)
        }

        // ================================================================
        // Power-user escape hatches
        // ================================================================

        // ── run_command ────────────────────────────────────────────────
        "run_command" => {
            let sid = resolve_sid(sessions, config, args).await?;

            let cmd_value = args
                .get("command")
                .ok_or_else(|| anyhow::anyhow!("missing required parameter: command"))?;

            let cmd: CommandSpec = serde_json::from_value(cmd_value.clone())
                .context("failed to parse command object")?;

            let ctx = sessions
                .get_mut(&sid)
                .ok_or_else(|| anyhow::anyhow!("session not found: {sid}"))?;

            let result = batch::execute_command(ctx, &cmd).await?;

            manager::upsert_runtime(ctx, false).await?;

            Ok(result)
        }

        // ── run_batch ──────────────────────────────────────────────────
        "run_batch" => {
            let sid = resolve_sid(sessions, config, args).await?;

            let cmds_value = args
                .get("commands")
                .ok_or_else(|| anyhow::anyhow!("missing required parameter: commands"))?;

            let cmds: Vec<CommandSpec> = serde_json::from_value(cmds_value.clone())
                .context("failed to parse commands array")?;

            let ctx = sessions
                .get_mut(&sid)
                .ok_or_else(|| anyhow::anyhow!("session not found: {sid}"))?;

            let result = batch::execute_batch(ctx, &cmds).await;

            manager::upsert_runtime(ctx, false).await?;

            Ok(result)
        }

        // ================================================================
        // Agent intelligence tools
        // ================================================================

        // ── analyze_page ───────────────────────────────────────────────
        "analyze_page" => {
            let sid = resolve_sid(sessions, config, args).await?;
            let ctx = sessions
                .get(&sid)
                .ok_or_else(|| anyhow::anyhow!("session not found: {sid}"))?;
            handle_analyze_page(&ctx.driver, args).await
        }

        // ── suggest_actions ────────────────────────────────────────────
        "suggest_actions" => {
            let sid = resolve_sid(sessions, config, args).await?;
            let ctx = sessions
                .get(&sid)
                .ok_or_else(|| anyhow::anyhow!("session not found: {sid}"))?;
            handle_suggest_actions(&ctx.driver, args).await
        }

        // ================================================================
        // Network monitoring tools
        // ================================================================

        // ── network_enable ─────────────────────────────────────────────
        "network_enable" => {
            let sid = resolve_sid(sessions, config, args).await?;
            let ctx = sessions
                .get(&sid)
                .ok_or_else(|| anyhow::anyhow!("session not found: {sid}"))?;

            let result = agent::network::enable_network_capture(&ctx.driver).await?;
            Ok(result)
        }

        // ── network_disable ────────────────────────────────────────────
        "network_disable" => {
            let sid = resolve_sid(sessions, config, args).await?;
            let ctx = sessions
                .get(&sid)
                .ok_or_else(|| anyhow::anyhow!("session not found: {sid}"))?;

            let result = agent::network::disable_network_capture(&ctx.driver).await?;
            Ok(result)
        }

        // ── network_get_log ────────────────────────────────────────────
        "network_get_log" => {
            let sid = resolve_sid(sessions, config, args).await?;
            let ctx = sessions
                .get(&sid)
                .ok_or_else(|| anyhow::anyhow!("session not found: {sid}"))?;

            let filter = parse_network_filter(args);
            let log = agent::network::get_network_log(&ctx.driver, &filter).await?;
            let result = serde_json::to_value(&log).context("failed to serialize network log")?;
            Ok(result)
        }

        // ── network_get_response_body ──────────────────────────────────
        "network_get_response_body" => {
            let sid = resolve_sid(sessions, config, args).await?;
            let ctx = sessions
                .get(&sid)
                .ok_or_else(|| anyhow::anyhow!("session not found: {sid}"))?;

            let request_id = args
                .get("request_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("missing required parameter: request_id"))?;

            let result = agent::network::get_response_body(&ctx.driver, request_id).await?;
            Ok(result)
        }

        // ── network_clear_log ──────────────────────────────────────────
        "network_clear_log" => {
            let sid = resolve_sid(sessions, config, args).await?;
            let ctx = sessions
                .get(&sid)
                .ok_or_else(|| anyhow::anyhow!("session not found: {sid}"))?;

            let result = agent::network::clear_network_log(&ctx.driver).await?;
            Ok(result)
        }

        // ── network_get_resource_timing ────────────────────────────────
        "network_get_resource_timing" => {
            let sid = resolve_sid(sessions, config, args).await?;
            let ctx = sessions
                .get(&sid)
                .ok_or_else(|| anyhow::anyhow!("session not found: {sid}"))?;

            let result = agent::network::get_resource_timing(&ctx.driver).await?;
            Ok(result)
        }

        // ── network_get_cookies ────────────────────────────────────────
        "network_get_cookies" => {
            let sid = resolve_sid(sessions, config, args).await?;
            let ctx = sessions
                .get(&sid)
                .ok_or_else(|| anyhow::anyhow!("session not found: {sid}"))?;

            let result = agent::network::get_cookies(&ctx.driver).await?;
            Ok(result)
        }

        _ => bail!("unknown tool: {tool_name}"),
    }
}

// ---------------------------------------------------------------------------
// Main entry point
// ---------------------------------------------------------------------------

pub async fn run(config: SessionConfig) -> Result<()> {
    eprintln!(
        "[mcp] starting server browser={:?} server_url={} headless={} viewport={}x{}",
        config.browser,
        config.server_url,
        config.headless,
        config.viewport_width,
        config.viewport_height
    );
    let mut sessions: HashMap<String, RuntimeCtx> = HashMap::new();

    let stdin = tokio::io::stdin();
    let reader = BufReader::new(stdin);
    let mut lines = reader.lines();

    let mut stdout = tokio::io::stdout();

    // Pre-load existing sessions from the store so that tools like
    // `open`, `click` etc. can resolve the default session without
    // requiring `create_session` first.
    if let Ok(store_data) = store::read_store().await {
        eprintln!(
            "[mcp] preload sessions from store count={} default={:?}",
            store_data.sessions.len(),
            store_data.default_session_id
        );
        for (sid, stored) in &store_data.sessions {
            if sessions.contains_key(sid) {
                eprintln!("[mcp] preload skip duplicate session={sid}");
                continue;
            }
            match manager::attach_existing_session(stored).await {
                Ok(client) => {
                    let ctx = RuntimeCtx {
                        driver: client,
                        session_id: sid.clone(),
                        server_url: stored.server.effective_url(),
                        tab_aliases: stored.tabs.aliases.clone(),
                        temp_profile_dir: stored
                            .temp_profile_dir
                            .as_ref()
                            .map(std::path::PathBuf::from),
                        chromedriver_child: None,
                    };
                    sessions.insert(sid.clone(), ctx);
                    eprintln!("[mcp] preload attached session={sid}");
                }
                Err(e) => {
                    eprintln!("[mcp] preload failed session={sid} err={e:#}");
                }
            }
        }
    } else {
        eprintln!("[mcp] preload store read failed, continuing with empty runtime map");
    }

    while let Some(line) = lines.next_line().await? {
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }

        eprintln!("[mcp] recv raw={line}");

        // Parse the incoming JSON-RPC request.
        let request: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("[mcp] parse error err={e}");
                let resp = make_error(&Value::Null, -32700, "Parse error");
                write_response(&mut stdout, &resp).await?;
                continue;
            }
        };

        let id = request.get("id").cloned().unwrap_or(Value::Null);
        let method = request.get("method").and_then(|v| v.as_str()).unwrap_or("");
        eprintln!(
            "[mcp] dispatch method={} id={}",
            method,
            if id.is_null() {
                "null".to_string()
            } else {
                id.to_string()
            }
        );

        match method {
            // ── initialize ─────────────────────────────────────────────
            "initialize" => {
                let resp = make_success(
                    &id,
                    json!({
                        "protocolVersion": "2024-11-05",
                        "capabilities": {
                            "tools": {
                                "listChanged": false
                            }
                        },
                        "serverInfo": {
                            "name": "browsectl-mcp-service",
                            "version": "0.2.0"
                        }
                    }),
                );
                write_response(&mut stdout, &resp).await?;
            }

            // ── notifications (no response) ────────────────────────────
            "notifications/initialized" | "initialized" => {
                // Notification — no response expected.
            }

            // ── ping ───────────────────────────────────────────────────
            "ping" => {
                let resp = make_success(&id, json!({}));
                write_response(&mut stdout, &resp).await?;
            }

            // ── tools/list ─────────────────────────────────────────────
            "tools/list" => {
                let resp = make_success(&id, json!({ "tools": tool_definitions() }));
                write_response(&mut stdout, &resp).await?;
            }

            // ── tools/call ─────────────────────────────────────────────
            "tools/call" => {
                let params = request.get("params").cloned().unwrap_or(json!({}));
                let tool_name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let arguments = params.get("arguments").cloned().unwrap_or(json!({}));

                eprintln!(
                    "[mcp] tools/call begin name={} args={}",
                    tool_name,
                    serde_json::to_string(&arguments)
                        .unwrap_or_else(|_| "<args-serialize-error>".to_string())
                );

                let resp =
                    match handle_tool_call(&mut sessions, &config, tool_name, &arguments).await {
                        Ok(result) => {
                            eprintln!(
                                "[mcp] tools/call ok name={} result={}",
                                tool_name,
                                serde_json::to_string(&result)
                                    .unwrap_or_else(|_| "<result-serialize-error>".to_string())
                            );
                            make_tool_result(&id, &result, false)
                        }
                        Err(e) => {
                            eprintln!("[mcp] tools/call err name={} err={e:#}", tool_name);
                            make_tool_result(&id, &json!({ "error": format!("{e:#}") }), true)
                        }
                    };
                write_response(&mut stdout, &resp).await?;
            }

            // ── resources/list (empty — we don't expose resources) ─────
            "resources/list" => {
                let resp = make_success(&id, json!({ "resources": [] }));
                write_response(&mut stdout, &resp).await?;
            }

            // ── resources/read ─────────────────────────────────────────
            "resources/read" => {
                let resp = make_error(&id, -32602, "No resources available");
                write_response(&mut stdout, &resp).await?;
            }

            // ── prompts/list (empty — we don't expose prompts) ─────────
            "prompts/list" => {
                let resp = make_success(&id, json!({ "prompts": [] }));
                write_response(&mut stdout, &resp).await?;
            }

            // ── prompts/get ────────────────────────────────────────────
            "prompts/get" => {
                let resp = make_error(&id, -32602, "No prompts available");
                write_response(&mut stdout, &resp).await?;
            }

            // ── unknown method ─────────────────────────────────────────
            _ => {
                if !id.is_null() {
                    let resp = make_error(&id, -32601, &format!("Method not found: {method}"));
                    write_response(&mut stdout, &resp).await?;
                }
                // If there's no id it's a notification for an unknown
                // method — silently ignore per JSON-RPC 2.0.
            }
        }
    }

    // ── stdin closed — let all drivers drop normally ───────────────────
    // WdClient does NOT close the session on drop, so browsers stay open.
    // We only need to prevent tokio from killing chromedriver child
    // processes (dropping tokio::process::Child does NOT kill by default,
    // so this is safe — but we explicitly drop to be clear).
    eprintln!(
        "[mcp] stdin closed, draining sessions count={}",
        sessions.len()
    );
    for (_sid, ctx) in sessions.drain() {
        // driver (WdClient) drops silently — no session deletion.
        drop(ctx.driver);

        // Let chromedriver child process continue running.
        // Dropping tokio::process::Child does NOT kill the child.
        if let Some(child) = ctx.chromedriver_child {
            drop(child);
        }
    }

    Ok(())
}
