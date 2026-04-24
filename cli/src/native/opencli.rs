//! opencli integration — bridges agent-browser (Rust daemon) to opencli's 90+ site adapters.
//!
//! Architecture:
//!   Rust daemon  →  opencli-host (Node.js subprocess)  →  opencli CLI  →  Site APIs
//!
//! The opencli-host process is spawned once and kept alive as a long-lived child,
//! communicating over stdin/stdout JSON-RPC 2.0.
//!
//! Key benefits:
//!   - 90+ pre-built site adapters (bilibili, xiaohongshu, reddit, etc.)
//!   - PUBLIC strategy: direct JSON API calls, no browser needed
//!   - COOKIE strategy: reuse browser's logged-in session via CDP cookies
//!   - EXPLORE: auto-discover a site's API capabilities
//!   - GENERATE: create new adapters on-demand from browser behavior

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, ChildStdout};
use tokio::sync::{Mutex, RwLock};
use tokio::time::timeout;

// ── Types ─────────────────────────────────────────────────────────────────────

/// Result of running an opencli command.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenCliResult {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub site: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
}

/// Detected capability for a URL.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SiteCapability {
    pub url: String,
    pub matched: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub site: Option<String>,
    pub strategy: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commands: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recommendation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_logged_in_required: Option<bool>,
}

/// Explore result for a URL.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExploreResult {
    pub url: String,
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggested_commands: Option<Vec<SuggestedCommand>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SuggestedCommand {
    pub site: String,
    pub command: String,
}

/// Generate result.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerateResult {
    pub url: String,
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub adapter_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub adapter_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggested_commands: Option<Vec<SuggestedCommand>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

// ── JSON-RPC types ─────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct JsonRpcRequest {
    jsonrpc: &'static str,
    id: u64,
    method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct JsonRpcResponse {
    #[serde(rename = "jsonrpc")]
    _version: String,
    #[serde(default)]
    id: Option<u64>,
    #[serde(default)]
    result: Option<serde_json::Value>,
    #[serde(default)]
    error: Option<JsonRpcError>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct JsonRpcError {
    code: i32,
    message: String,
}

// ── OpenCliHost ────────────────────────────────────────────────────────────────

/// A long-lived opencli-host subprocess managed by the daemon.
/// Spawned once, reused across all opencli calls.
pub struct OpenCliHost {
    stdin: Arc<RwLock<ChildStdin>>,
    stdout: Arc<Mutex<BufReader<ChildStdout>>>,
    next_id: Arc<RwLock<u64>>,
    pub ready: Arc<RwLock<bool>>,
}

impl OpenCliHost {
    /// Spawn the opencli-host Node.js subprocess.
    pub async fn spawn(host_script: PathBuf) -> Result<Self, OpenCliError> {
        let mut child = tokio::process::Command::new("node")
            .arg(host_script)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(false)
            .spawn()
            .map_err(|e| OpenCliError::Spawn(e.to_string()))?;

        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();

        // Log stderr from the host process
        let stderr_handle = tokio::spawn(async move {
            let mut reader = BufReader::new(stderr);
            let mut line = String::new();
            while reader.read_line(&mut line).await.unwrap_or(0) > 0 {
                eprintln!("[opencli-host] {}", line.trim());
                line.clear();
            }
        });

        let host = Self {
            stdin: Arc::new(RwLock::new(stdin)),
            stdout: Arc::new(Mutex::new(BufReader::new(stdout))),
            next_id: Arc::new(RwLock::new(1)),
            ready: Arc::new(RwLock::new(false)),
        };

        // Wait for the "ready" notification
        timeout(Duration::from_secs(10), host.wait_ready())
            .await
            .map_err(|_| OpenCliError::Timeout)??;

        Ok(host)
    }

    async fn wait_ready(&self) -> Result<(), OpenCliError> {
        let mut line = String::new();
        loop {
            self.stdout
                .lock()
                .await
                .read_line(&mut line)
                .await
                .map_err(|e| OpenCliError::Communication(format!("read_line failed: {e}")))?;
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            // Look for ready notification: { "jsonrpc": "2.0", "method": "ready", ... }
            if trimmed.contains("\"method\":\"ready\"") {
                *self.ready.write().await = true;
                return Ok(());
            }
            line.clear();
        }
    }

    /// Send a JSON-RPC request and wait for response.
    async fn call(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, OpenCliError> {
        let id = {
            let mut guard = self.next_id.write().await;
            let id = *guard;
            *guard = guard.saturating_add(1);
            id
        };

        let req = JsonRpcRequest {
            jsonrpc: "2.0",
            id,
            method: method.to_string(),
            params,
        };

        let req_str =
            serde_json::to_string(&req).map_err(|e| OpenCliError::Serialize(e.to_string()))?;
        let mut stdin = self.stdin.write().await;
        stdin
            .write_all(req_str.as_bytes())
            .await
            .map_err(|e| OpenCliError::Communication(format!("write failed: {e}")))?;
        stdin
            .write_all(b"\n")
            .await
            .map_err(|e| OpenCliError::Communication(format!("write newline failed: {e}")))?;
        drop(stdin);

        // Read response
        let mut line = String::new();
        timeout(
            Duration::from_secs(120),
            self.stdout.lock().await.read_line(&mut line),
        )
        .await
        .map_err(|_| OpenCliError::Timeout)?
        .map_err(|e| OpenCliError::Communication(format!("read_line failed: {e}")))?;

        let resp: JsonRpcResponse = serde_json::from_str(line.trim())
            .map_err(|e| OpenCliError::Parse(format!("{e}: {line}")))?;

        if let Some(err) = resp.error {
            return Err(OpenCliError::Method(err.message));
        }

        resp.result.ok_or_else(|| OpenCliError::NoResult)
    }

    /// List all available opencli commands.
    pub async fn list_commands(
        &self,
        format: Option<&str>,
    ) -> Result<serde_json::Value, OpenCliError> {
        self.call(
            "opencli.list",
            Some(serde_json::json!({ "format": format.unwrap_or("json") })),
        )
        .await
    }

    /// Run an opencli command: <site> <command> [args]
    pub async fn run_command(
        &self,
        site: &str,
        command: &str,
        args: Option<serde_json::Value>,
        format: Option<&str>,
        timeout_secs: Option<u64>,
    ) -> Result<OpenCliResult, OpenCliError> {
        let result: serde_json::Value = self
            .call(
                "opencli.run",
                Some(serde_json::json!({
                    "site": site,
                    "command": command,
                    "args": args.unwrap_or(serde_json::Value::Null),
                    "format": format.unwrap_or("json"),
                    "timeout": timeout_secs.unwrap_or(60),
                })),
            )
            .await?;

        serde_json::from_value(result)
            .map_err(|e| OpenCliError::Serialize(format!("run result parse failed: {e}")))
    }

    /// Detect opencli adapter for a URL.
    pub async fn detect(&self, url: &str) -> Result<SiteCapability, OpenCliError> {
        let result: serde_json::Value = self
            .call("opencli.detect", Some(serde_json::json!({ "url": url })))
            .await?;

        serde_json::from_value(result)
            .map_err(|e| OpenCliError::Serialize(format!("detect result parse failed: {e}")))
    }

    /// Explore a URL and discover its capabilities.
    pub async fn explore(
        &self,
        url: &str,
        timeout_secs: Option<u64>,
    ) -> Result<ExploreResult, OpenCliError> {
        let result: serde_json::Value = self
            .call(
                "opencli.explore",
                Some(serde_json::json!({ "url": url, "timeout": timeout_secs.unwrap_or(120) })),
            )
            .await?;

        serde_json::from_value(result)
            .map_err(|e| OpenCliError::Serialize(format!("explore result parse failed: {e}")))
    }

    /// Generate a new adapter for a URL.
    pub async fn generate(
        &self,
        url: &str,
        name: Option<&str>,
        force: bool,
        timeout_secs: Option<u64>,
    ) -> Result<GenerateResult, OpenCliError> {
        let result: serde_json::Value = self
            .call(
                "opencli.generate",
                Some(serde_json::json!({
                    "url": url,
                    "name": name,
                    "force": force,
                    "timeout": timeout_secs.unwrap_or(120),
                })),
            )
            .await?;

        serde_json::from_value(result)
            .map_err(|e| OpenCliError::Serialize(format!("generate result parse failed: {e}")))
    }

    /// Health check.
    pub async fn health(&self) -> Result<bool, OpenCliError> {
        let result: serde_json::Value = self.call("opencli.health", None).await?;
        Ok(result.get("ok").and_then(|v| v.as_bool()).unwrap_or(false))
    }
}

// ── Error ─────────────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum OpenCliError {
    #[error("spawn failed: {0}")]
    Spawn(String),
    #[error("communication error: {0}")]
    Communication(String),
    #[error("parse error: {0}")]
    Parse(String),
    #[error("serialize error: {0}")]
    Serialize(String),
    #[error("method error: {0}")]
    Method(String),
    #[error("timeout")]
    Timeout,
    #[error("no result in response")]
    NoResult,
}

// ── Strategy ──────────────────────────────────────────────────────────────────

/// Auth strategy for site adapters.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AdapterStrategy {
    /// Direct public API, no auth needed
    Public,
    /// Requires browser cookies (reuse logged-in session)
    Cookie,
    /// Requires API key injected as header
    Header,
    /// Man-in-the-middle network interception
    Intercept,
    /// Must use browser UI interaction
    Ui,
    /// Unknown / not detected
    Unknown,
}

impl Default for AdapterStrategy {
    fn default() -> Self {
        Self::Unknown
    }
}

impl std::fmt::Display for AdapterStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Public => write!(f, "public"),
            Self::Cookie => write!(f, "cookie"),
            Self::Header => write!(f, "header"),
            Self::Intercept => write!(f, "intercept"),
            Self::Ui => write!(f, "ui"),
            Self::Unknown => write!(f, "unknown"),
        }
    }
}

impl From<&str> for AdapterStrategy {
    fn from(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "public" => Self::Public,
            "cookie" => Self::Cookie,
            "header" => Self::Header,
            "intercept" => Self::Intercept,
            "ui" => Self::Ui,
            _ => Self::Unknown,
        }
    }
}

// ── Tool exposure ──────────────────────────────────────────────────────────────

/// Represents an opencli command exposed as a tool to the AI agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenCliTool {
    /// Full command path, e.g. "bilibili/hot"
    pub name: String,
    pub description: String,
    pub strategy: AdapterStrategy,
    /// Whether this requires a live browser session (cookie/auth commands)
    pub requires_browser: bool,
    pub args: Vec<ToolArg>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolArg {
    pub name: String,
    #[serde(rename = "type")]
    pub arg_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
}
