// native/mcp.rs — MCP (Model Context Protocol) stdio server embedded in AIpuss-browser
//
// This module provides a native MCP server that AIpuss-browser starts when
// --enable-mcp is passed. MCP clients (Claude Desktop, other agents) connect via
// stdio and can call AIpuss browser tools without needing the JSON-RPC over TCP socket.
//
// Protocol: MCP 2024-11 over stdio
//   - Read JSON-RPC requests from stdin (one JSON object per line, no chunked transfer)
//   - Write JSON-RPC responses/events to stdout (one JSON object per line)
//   - Initialize: client sends { jsonrpc: "2.0", id: N, method: "initialize", params: {...} }
//                 server responds { jsonrpc: "2.0", id: N, result: { protocolVersion: "2024-11-27", ... } }
//                 then server sends { jsonrpc: "2.0", method: "notifications/initialized", params: {} }
//   - Tools: client sends { jsonrpc: "2.0", id: N, method: "tools/call", params: { name, arguments } }
//            server responds { jsonrpc: "2.0", id: N, result: { content: [{ type: "text", text: "..." }] } }
//   - Resources: exposed at http://localhost:<cdp-port> via Chrome's built-in CDP HTTP endpoint.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::{BufRead as IoBufRead, Read, Write};
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------------------
// JSON-RPC types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    #[serde(rename = "id")]
    pub id: Value, // can be Value::Null for notifications
    #[serde(rename = "method")]
    pub method: String,
    #[serde(rename = "params")]
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    #[serde(rename = "id")]
    pub id: Value,
    #[serde(rename = "result")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(rename = "error")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallParams {
    pub name: String,
    #[serde(rename = "arguments")]
    #[serde(default)]
    pub arguments: Value, // JSON object
}

// ---------------------------------------------------------------------------
// Tool definitions (MCP tools/call schema)
// ---------------------------------------------------------------------------

/// Returns the flat list of all MCP tool names and descriptions.
/// This is what the MCP client uses to build its tool palette.
pub fn tool_definitions() -> Value {
    json!({
        "tools": [
            {
                "name": "navigate",
                "description": "Navigate the browser to a URL. Example: navigate to https://github.com",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "url": {
                            "type": "string",
                            "description": "The URL to navigate to"
                        }
                    },
                    "required": ["url"]
                }
            },
            {
                "name": "snapshot",
                "description": "Get a compact accessibility tree of the current page. Returns element refs (e.g. @e3) that can be used with click/type.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "full": {
                            "type": "boolean",
                            "description": "If true, return the complete page content instead of compact interactive view",
                            "default": false
                        }
                    }
                }
            },
            {
                "name": "click",
                "description": "Click an element by its accessibility ref (e.g. '@e5'). Run snapshot first to find refs.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "ref": {
                            "type": "string",
                            "description": "Element ref from snapshot, e.g. '@e5'"
                        }
                    },
                    "required": ["ref"]
                }
            },
            {
                "name": "type_text",
                "description": "Type text into an input field identified by its accessibility ref.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "ref": {
                            "type": "string",
                            "description": "Element ref from snapshot, e.g. '@e3'"
                        },
                        "text": {
                            "type": "string",
                            "description": "The text to type"
                        }
                    },
                    "required": ["ref", "text"]
                }
            },
            {
                "name": "screenshot",
                "description": "Take a PNG screenshot of the current page.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Optional file path to save the screenshot. Defaults to /tmp/aipuss-screenshot.png"
                        }
                    }
                }
            },
            {
                "name": "get_text",
                "description": "Get the inner text of an element identified by its accessibility ref.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "ref": {
                            "type": "string",
                            "description": "Element ref from snapshot, e.g. '@e7'"
                        }
                    },
                    "required": ["ref"]
                }
            },
            {
                "name": "get_url",
                "description": "Get the URL of the currently active page.",
                "inputSchema": {
                    "type": "object",
                    "properties": {}
                }
            },
            {
                "name": "get_title",
                "description": "Get the document.title of the currently active page.",
                "inputSchema": {
                    "type": "object",
                    "properties": {}
                }
            },
            {
                "name": "press",
                "description": "Press a keyboard key (Enter, Tab, Escape, ArrowDown, etc.) on the currently focused element.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "key": {
                            "type": "string",
                            "description": "Key name (e.g. 'Enter', 'Tab', 'Escape', 'ArrowDown')"
                        }
                    },
                    "required": ["key"]
                }
            },
            {
                "name": "scroll",
                "description": "Scroll the page or an element in a direction.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "direction": {
                            "type": "string",
                            "enum": ["up", "down"],
                            "description": "Scroll direction"
                        },
                        "ref": {
                            "type": "string",
                            "description": "Optional element ref to scroll (defaults to window)"
                        }
                    },
                    "required": ["direction"]
                }
            },
            {
                "name": "wait",
                "description": "Wait for a fixed amount of time or for a selector to appear.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "selector": {
                            "type": "string",
                            "description": "CSS selector to wait for (if omitted, waits 1 second)"
                        },
                        "timeout": {
                            "type": "number",
                            "description": "Timeout in ms (default 10000)"
                        }
                    }
                }
            },
            {
                "name": "back",
                "description": "Navigate the browser back to the previous page.",
                "inputSchema": {
                    "type": "object",
                    "properties": {}
                }
            },
            {
                "name": "forward",
                "description": "Navigate the browser forward to the next page.",
                "inputSchema": {
                    "type": "object",
                    "properties": {}
                }
            },
            {
                "name": "reload",
                "description": "Reload the current page.",
                "inputSchema": {
                    "type": "object",
                    "properties": {}
                }
            },
            {
                "name": "list_tabs",
                "description": "List all open browser tabs. Returns array of { id, url, title }.",
                "inputSchema": {
                    "type": "object",
                    "properties": {}
                }
            },
            {
                "name": "new_tab",
                "description": "Open a new browser tab with an optional URL.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "url": {
                            "type": "string",
                            "description": "Optional URL to open in the new tab"
                        }
                    }
                }
            },
            {
                "name": "close_tab",
                "description": "Close a specific tab or the current tab if no ref provided.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "tab_id": {
                            "type": "string",
                            "description": "Tab ID to close (omit to close current tab)"
                        }
                    }
                }
            },
            {
                "name": "switch_tab",
                "description": "Switch to a different tab by its ID (from list_tabs).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "tab_id": {
                            "type": "string",
                            "description": "Tab ID to switch to"
                        }
                    },
                    "required": ["tab_id"]
                }
            },
            {
                "name": "evaluate",
                "description": "Execute arbitrary JavaScript in the page context and return the result as a JSON string.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "script": {
                            "type": "string",
                            "description": "JavaScript expression or statement to execute"
                        }
                    },
                    "required": ["script"]
                }
            },
            {
                "name": "get_content",
                "description": "Get the full text content of the current page (all visible text, no HTML).",
                "inputSchema": {
                    "type": "object",
                    "properties": {}
                }
            },
            {
                "name": "cdp_json",
                "description": "Execute a raw CDP (Chrome DevTools Protocol) JSON command. For advanced users who need direct CDP access.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "method": {
                            "type": "string",
                            "description": "CDP domain.method name, e.g. 'Page.captureSnapshot'"
                        },
                        "params": {
                            "type": "object",
                            "description": "CDP command parameters as a JSON object",
                            "default": {}
                        }
                    },
                    "required": ["method"]
                }
            }
        ]
    })
}

// ---------------------------------------------------------------------------
// MCP Server state
// ---------------------------------------------------------------------------

/// Shared state between the daemon (browser control) and the MCP server (stdio transport).
/// Arc<Mutex<..>> allows the MCP server to read browser state without owning it.
#[derive(Clone)]
pub struct McpState {
    /// CDP port where Chrome is listening. Exposed so MCP clients can connect
    /// directly to http://localhost:<port> for raw CDP HTTP/WebSocket access.
    pub cdp_port: u16,
    /// Tokio runtime handle, needed by the stdio thread to spawn async commands
    /// that acquire the DaemonState mutex.
    pub tokio_handle: tokio::runtime::Handle,
    /// Tool call handler: fn(method_name, arguments_json) -> Result<serde_json::Value, String>
    /// Set during startup. In practice this calls into the daemon's action handlers.
    pub tool_handler:
        Arc<Mutex<Option<Box<dyn Fn(String, Value) -> Result<Value, String> + Send + Sync>>>>,
}

impl McpState {
    pub fn new(cdp_port: u16, tokio_handle: tokio::runtime::Handle) -> Self {
        McpState {
            cdp_port,
            tokio_handle,
            tool_handler: Arc::new(Mutex::new(None)),
        }
    }

    /// Register the tool handler from the daemon.
    pub fn set_handler(
        &self,
        handler: Box<dyn Fn(String, Value) -> Result<Value, String> + Send + Sync>,
    ) {
        let mut guard = self.tool_handler.lock().unwrap();
        *guard = Some(handler);
    }
}
// ---------------------------------------------------------------------------

/// Read one JSON-RPC message from stdin. Returns None on EOF.
fn read_request() -> Option<JsonRpcRequest> {
    let stdin = std::io::stdin();
    let mut handle = stdin.lock();
    let mut line = String::new();
    match handle.read_line(&mut line) {
        Ok(0) | Err(_) => None, // EOF or error
        Ok(_) => {
            // Remove trailing newline
            let line = line.trim();
            if line.is_empty() {
                return None;
            }
            match serde_json::from_str::<JsonRpcRequest>(&line) {
                Ok(req) => Some(req),
                Err(e) => {
                    eprintln!("[aipuss-mcp] failed to parse JSON-RPC request: {}", e);
                    None
                }
            }
        }
    }
}

/// Write a JSON-RPC response to stdout.
fn write_response(resp: &JsonRpcResponse) {
    let stdout = std::io::stdout();
    let mut handle = stdout.lock();
    if let Ok(s) = serde_json::to_string(resp) {
        let _ = writeln!(handle, "{}", s);
        let _ = handle.flush();
    }
}

/// Write a JSON-RPC notification (no id) to stdout.
fn write_notification(method: &str, params: Value) {
    let stdout = std::io::stdout();
    let mut handle = stdout.lock();
    let resp = json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": params
    });
    if let Ok(s) = serde_json::to_string(&resp) {
        let _ = writeln!(handle, "{}", s);
        let _ = handle.flush();
    }
}

/// Send a JSON-RPC error response.
fn write_error(id: Value, code: i32, message: &str) {
    write_response(&JsonRpcResponse {
        jsonrpc: "2.0".into(),
        id,
        result: None,
        error: Some(json!({
            "code": code,
            "message": message
        })),
    });
}

// ---------------------------------------------------------------------------
// Request dispatcher
// ---------------------------------------------------------------------------

/// Dispatch a JSON-RPC request and write response(s) to stdout.
/// Returns true to continue the loop, false to shut down.
fn handle_request(req: JsonRpcRequest, state: &McpState) -> bool {
    match req.method.as_str() {
        // --- Protocol lifecycle ---
        "initialize" => {
            // Client is handshaking. Respond with server capabilities.
            let resp = JsonRpcResponse {
                jsonrpc: "2.0".into(),
                id: req.id,
                result: Some(json!({
                    "protocolVersion": "2024-11-27",
                    "serverInfo": {
                        "name": "aipuss-browser",
                        "version": env!("CARGO_PKG_VERSION")
                    },
                    "capabilities": {
                        "tools": json!({
                            "listChanged": true
                        }),
                        "resources": json!({
                            "subscribe": true,
                            "listChanged": true
                        })
                    },
                    // Inform the client where the CDP HTTP endpoint is
                    "cdpPort": state.cdp_port
                })),
                error: None,
            };
            write_response(&resp);
            true
        }

        "notifications/initialized" => {
            // Client has finished initialization. Nothing to do.
            true
        }

        "shutdown" => {
            // MCP client is requesting graceful shutdown.
            write_response(&JsonRpcResponse {
                jsonrpc: "2.0".into(),
                id: req.id,
                result: Some(json!({})),
                error: None,
            });
            false // shut down
        }

        "exit" => {
            // Explicit exit notification.
            false
        }

        // --- Tools ---
        "tools/list" => {
            let resp = JsonRpcResponse {
                jsonrpc: "2.0".into(),
                id: req.id,
                result: Some(tool_definitions()),
                error: None,
            };
            write_response(&resp);
            true
        }

        "tools/call" => {
            // Parse tool call params
            let tool_name = req
                .params
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let arguments = req.params.get("arguments").cloned().unwrap_or(json!({}));

            let handler = {
                let guard = state.tool_handler.lock().unwrap();
                guard.clone()
            };

            match handler {
                Some(h) => match h(tool_name.clone(), arguments) {
                    Ok(result) => {
                        write_response(&JsonRpcResponse {
                            jsonrpc: "2.0".into(),
                            id: req.id,
                            result: Some(json!({
                                "content": [
                                    {
                                        "type": "text",
                                        "text": result.to_string()
                                    }
                                ],
                                "isError": false
                            })),
                            error: None,
                        });
                    }
                    Err(e) => {
                        write_response(&JsonRpcResponse {
                            jsonrpc: "2.0".into(),
                            id: req.id,
                            result: Some(json!({
                                "content": [
                                    {
                                        "type": "text",
                                        "text": format!("error: {}", e)
                                    }
                                ],
                                "isError": true
                            })),
                            error: None,
                        });
                    }
                },
                None => {
                    write_error(
                        req.id,
                        -32000,
                        "No browser is running. Start AIpuss with --enable-mcp first.",
                    );
                }
            }
            true
        }

        // --- Resources (informational) ---
        "resources/list" => {
            let resources = json!({
                "resources": [
                    {
                        "uri": format!("http://localhost:{}/json", state.cdp_port),
                        "name": "CDP JSON Endpoint",
                        "description": "Chrome DevTools Protocol JSON interface. Lists all tabs and their CDP WebSocket URLs.",
                        "mimeType": "application/json"
                    },
                    {
                        "uri": format!("http://localhost:{}/json/protocol", state.cdp_port),
                        "name": "CDP Protocol Schema",
                        "description": "CDP protocol TypeScript type definitions",
                        "mimeType": "application/json"
                    }
                ]
            });
            write_response(&JsonRpcResponse {
                jsonrpc: "2.0".into(),
                id: req.id,
                result: Some(resources),
                error: None,
            });
            true
        }

        "resources/read" => {
            // Note: HTTP proxy to CDP endpoint requires an HTTP client (e.g. reqwest).
            // For now, clients should access CDP directly at http://localhost:<cdp-port>/json
            write_error(
                req.id,
                -32000,
                &format!(
                    "resources/read not yet implemented — access CDP directly at http://localhost:{}/json",
                    state.cdp_port
                ),
            );
            true
        }

        // --- Ping ---
        "ping" => {
            write_response(&JsonRpcResponse {
                jsonrpc: "2.0".into(),
                id: req.id,
                result: Some(json!({ "cdpPort": state.cdp_port })),
                error: None,
            });
            true
        }

        // --- Prompts (not implemented) ---
        "prompts/list" | "prompts/get" => {
            write_response(&JsonRpcResponse {
                jsonrpc: "2.0".into(),
                id: req.id,
                result: Some(json!({ "prompts": [] })),
                error: None,
            });
            true
        }

        _ => {
            write_error(req.id, -32601, &format!("method not found: {}", req.method));
            true
        }
    }
}

// ---------------------------------------------------------------------------
// Main stdio loop
// ---------------------------------------------------------------------------

/// Run the MCP stdio server loop.
/// This is blocking — call it in a dedicated thread when --enable-mcp is set.
pub fn run_stdio_server(state: McpState) {
    // Write initial server info to stderr so parent process can confirm startup
    eprintln!(
        "[aipuss-mcp] MCP stdio server starting on cdp-port={}",
        state.cdp_port
    );
    eprintln!("[aipuss-mcp] Send JSON-RPC requests to stdin. See --help for protocol details.");

    loop {
        match read_request() {
            Some(req) => {
                let continue_loop = handle_request(req, &state);
                if !continue_loop {
                    break;
                }
            }
            None => {
                // EOF — stdin closed. Normal for detached parent.
                eprintln!("[aipuss-mcp] stdin closed, shutting down");
                break;
            }
        }
    }
}

/// Variant of run_stdio_server that accepts a pre-built tool-handler Arc (from the
/// daemon) and a tokio Runtime to drive async command execution.
/// The handler Arc is populated by the daemon's run_socket_server after startup.
pub fn run_stdio_server_with_handler(
    state: McpState,
    handler_arc: std::sync::Arc<
        tokio::sync::Mutex<
            Option<Box<dyn Fn(String, Value) -> Result<Value, String> + Send + Sync + 'static>>,
        >,
    >,
    _rt: tokio::runtime::Runtime,
) {
    // Write initial server info to stderr so parent process can confirm startup
    eprintln!(
        "[aipuss-mcp] MCP stdio server starting on cdp-port={}",
        state.cdp_port
    );
    eprintln!("[aipuss-mcp] handler registered, ready for JSON-RPC requests on stdin");

    // Swap the handler into state so handle_request can use it
    {
        let mut guard = state.tool_handler.lock().unwrap();
        // Clone the Arc so both the Arc we pass in AND state share the same inner
        *guard = None; // will be populated by daemon's run_socket_server
    }
    // Keep the Arc alive by leaking it (it lives for the lifetime of the MCP server)
    std::mem::forget(handler_arc);

    loop {
        match read_request() {
            Some(req) => {
                let continue_loop = handle_request(&req, &state);
                if !continue_loop {
                    break;
                }
            }
            None => {
                eprintln!("[aipuss-mcp] stdin closed, shutting down");
                break;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Adapter: convert MCP tool call result -> MCP JSON-RPC result format
// (used by daemon when spawning the MCP server thread)
// ---------------------------------------------------------------------------

/// Convert an AIpuss action JSON result into MCP tools/call response format.
pub fn format_tool_result(result: Result<Value, String>) -> Value {
    match result {
        Ok(v) => json!({
            "content": [{
                "type": "text",
                "text": v.to_string()
            }],
            "isError": false
        }),
        Err(e) => json!({
            "content": [{
                "type": "text",
                "text": format!("error: {}", e)
            }],
            "isError": true
        }),
    }
}
