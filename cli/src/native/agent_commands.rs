//! Agent-native built-in command registry.
//!
//! These are high-level natural language commands that the Rust layer handles
//! internally, returning structured results to the LLM. This prevents the agent
//! from having to orchestrate multiple CDP calls manually for common tasks.
//!
//! Each command returns a structured JSON result that the LLM can reason about.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

// ---------------------------------------------------------------------------
// Command registry
// ---------------------------------------------------------------------------

/// A built-in agent command.
#[derive(Debug, Clone)]
pub struct AgentCommand {
    /// Unique command identifier.
    pub id: &'static str,
    /// Human-readable name.
    pub name: &'static str,
    /// One-line description.
    pub description: &'static str,
    /// Parameters schema (JSON Schema style).
    pub parameters: Value,
    /// Whether this command is currently available.
    pub available: bool,
}

/// All registered built-in commands.
pub fn get_builtin_commands() -> Vec<AgentCommand> {
    vec![
        AgentCommand {
            id: "search_github",
            name: "search_github",
            description: "Search GitHub for repositories matching a query",
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "GitHub search query (e.g. 'rust browser automation stars:>100')"
                    },
                    "sort": {
                        "type": "string",
                        "enum": ["stars", "forks", "updated"],
                        "description": "Sort order"
                    },
                    "limit": {
                        "type": "integer",
                        "default": 5,
                        "description": "Maximum number of results"
                    }
                },
                "required": ["query"]
            }),
            available: true,
        },
        AgentCommand {
            id: "find_best_repo",
            name: "find_best_repo",
            description: "Find the best repository for a given task by comparing options",
            parameters: json!({
                "type": "object",
                "properties": {
                    "task": {
                        "type": "string",
                        "description": "What you need the repo to do"
                    },
                    "language": {
                        "type": "string",
                        "description": "Preferred language (e.g. 'rust', 'python')"
                    },
                    "min_stars": {
                        "type": "integer",
                        "default": 100,
                        "description": "Minimum star count"
                    }
                },
                "required": ["task"]
            }),
            available: true,
        },
        AgentCommand {
            id: "best_github_repo",
            name: "best_github_repo",
            description: "Find the single best GitHub repo for a task using GitHub API search",
            parameters: json!({
                "type": "object",
                "properties": {
                    "task": {
                        "type": "string",
                        "description": "Task description (e.g. 'ONNX runtime inference server')"
                    },
                    "language": {
                        "type": "string",
                        "description": "Programming language"
                    }
                },
                "required": ["task"]
            }),
            available: true,
        },
        AgentCommand {
            id: "extract_structured_data",
            name: "extract_structured_data",
            description: "Extract structured data from the current page using a schema",
            parameters: json!({
                "type": "object",
                "properties": {
                    "schema": {
                        "type": "object",
                        "description": "JSON Schema for the data to extract"
                    },
                    "selector": {
                        "type": "string",
                        "description": "CSS selector for the container element (default: body)"
                    }
                },
                "required": ["schema"]
            }),
            available: true,
        },
        AgentCommand {
            id: "find_all_forms",
            name: "find_all_forms",
            description: "Find all forms on the current page with their inputs and submit buttons",
            parameters: json!({
                "type": "object",
                "properties": {
                    "filter": {
                        "type": "string",
                        "description": "Filter forms by action URL pattern"
                    }
                }
            }),
            available: true,
        },
        AgentCommand {
            id: "audit_accessibility",
            name: "audit_accessibility",
            description: "Run a basic accessibility audit on the current page",
            parameters: json!({
                "type": "object",
                "properties": {
                    "severity": {
                        "type": "string",
                        "enum": ["all", "critical", "serious"],
                        "default": "all"
                    }
                }
            }),
            available: true,
        },
        AgentCommand {
            id: "extract_breadcrumbs",
            name: "extract_breadcrumbs",
            description: "Extract breadcrumb navigation from the current page",
            parameters: json!({}),
            available: true,
        },
        AgentCommand {
            id: "compare_elements",
            name: "compare_elements",
            description: "Compare multiple elements by selector and return their differences",
            parameters: json!({
                "type": "object",
                "properties": {
                    "selectors": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "List of CSS selectors to compare"
                    },
                    "properties": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Properties to compare (text, href, src, style, etc.)"
                    }
                },
                "required": ["selectors"]
            }),
            available: true,
        },
    ]
}

// ---------------------------------------------------------------------------
// GitHub API integration
// ---------------------------------------------------------------------------

const GITHUB_API_BASE: &str = "https://api.github.com";

/// Search GitHub using the REST API.
pub async fn search_github(
    query: &str,
    sort: Option<&str>,
    limit: usize,
    token: Option<&str>,
) -> Result<Value, String> {
    let client = reqwest::Client::new();

    let mut url = format!("{}/search/repositories?q={}", GITHUB_API_BASE, urlencoding::encode(query));
    if let Some(s) = sort {
        url.push_str(&format!("&sort={}", s));
    }
    url.push_str("&per_page=30");

    let mut req = client.get(&url).header("User-Agent", "AIpuss-browser");
    if let Some(t) = token {
        req = req.header("Authorization", format!("Bearer {}", t));
    }

    let resp = req.send().await.map_err(|e| e.to_string())?;
    let status = resp.status();

    if status.as_u16() == 403 {
        return Err("GitHub API rate limit exceeded. Use a GitHub token for higher limits.".to_string());
    }
    if status.as_u16() == 422 {
        return Err("GitHub API: invalid search query".to_string());
    }
    if !status.is_success() {
        return Err(format!("GitHub API error: {}", status));
    }

    let data: Value = resp.json().await.map_err(|e| e.to_string())?;

    let items = data
        .get("items")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .take(limit)
                .map(|repo| {
                    json!({
                        "name": repo.get("full_name").and_then(|v| v.as_str()).unwrap_or(""),
                        "description": repo.get("description").and_then(|v| v.as_str()).unwrap_or(""),
                        "stars": repo.get("stargazers_count").and_then(|v| v.as_i64()).unwrap_or(0),
                        "forks": repo.get("forks_count").and_then(|v| v.as_i64()).unwrap_or(0),
                        "language": repo.get("language").and_then(|v| v.as_str()).unwrap_or(""),
                        "url": repo.get("html_url").and_then(|v| v.as_str()).unwrap_or(""),
                        "topics": repo.get("topics").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>()).unwrap_or_default(),
                        "last_updated": repo.get("updated_at").and_then(|v| v.as_str()).unwrap_or(""),
                        "license": repo.get("license").and_then(|v| v.get("spdx_id")).and_then(|v| v.as_str()).unwrap_or(""),
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    Ok(json!({
        "query": query,
        "total": data.get("total_count").and_then(|v| v.as_i64()).unwrap_or(0),
        "results": items,
    }))
}

/// Find the single best repo for a task using GitHub API.
pub async fn find_best_repo(
    task: &str,
    language: Option<&str>,
    min_stars: usize,
    token: Option<&str>,
) -> Result<Value, String> {
    let mut query_parts = vec![task.to_string()];
    query_parts.push(format!("stars:>={}", min_stars));
    if let Some(lang) = language {
        query_parts.push(format!("language:{}", lang));
    }
    query_parts.push("stars:>1000".to_string()); // Only consider popular repos

    let query = query_parts.join("+");
    let results = search_github(&query, Some("stars"), 5, token).await?;

    let repos = results.get("results").and_then(|v| v.as_array()).unwrap_or(&[]);

    if repos.is_empty() {
        return Ok(json!({
            "task": task,
            "best": null,
            "message": "No matching repositories found"
        }));
    }

    // Score repos: stars * 1 + forks * 0.5 + recency boost
    let scored: Vec<(usize, &Value)> = repos
        .iter()
        .enumerate()
        .map(|(i, repo)| {
            let stars = repo.get("stars").and_then(|v| v.as_i64()).unwrap_or(0) as usize;
            let forks = repo.get("forks").and_then(|v| v.as_i64()).unwrap_or(0) as usize;
            let score = stars + forks / 2 + (1000 - i * 100).max(0);
            (score, repo)
        })
        .collect();

    let best = scored
        .iter()
        .max_by_key(|(s, _)| *s)
        .map(|(_, repo)| repo.clone());

    Ok(json!({
        "task": task,
        "language": language,
        "best": best,
        "alternatives": repos.get(1..).unwrap_or(&[]),
        "total_found": repos.len(),
    }))
}

// ---------------------------------------------------------------------------
// Page analysis commands
// ---------------------------------------------------------------------------

/// Execute a built-in page analysis command via CDP.
pub async fn execute_page_command(
    cmd: &AgentCommand,
    params: &Value,
    cdp_client: &super::cdp::client::CdpClient,
    session_id: &str,
) -> Result<Value, String> {
    match cmd.id {
        "find_all_forms" => {
            let filter = params.get("filter").and_then(|v| v.as_str());
            let js = format!(
                r#"(function() {{
                    var forms = Array.from(document.querySelectorAll('form'));
                    return forms.map(function(f) {{
                        var inputs = Array.from(f.querySelectorAll('input, select, textarea'));
                        var submit = f.querySelector('[type="submit"], button[type="submit"], button:not([type])');
                        return {{
                            action: f.action,
                            method: f.method,
                            id: f.id || null,
                            class: f.className || null,
                            inputs: inputs.map(function(i) {{
                                return {{
                                    type: i.type || i.tagName.toLowerCase(),
                                    name: i.name || null,
                                    id: i.id || null,
                                    placeholder: i.placeholder || null,
                                    required: i.required || false,
                                    autocomplete: i.autocomplete || null
                                }};
                            }}),
                            submitText: submit ? (submit.textContent || submit.value || '').trim() : null,
                            submitType: submit ? submit.type : null
                        }};
                    }});
                }})()"#
            );
            let result: super::cdp::types::EvaluateResult = cdp_client
                .send_command_typed(
                    "Runtime.evaluate",
                    &super::cdp::types::EvaluateParams {
                        expression: js,
                        return_by_value: Some(true),
                        await_promise: Some(false),
                    },
                    Some(session_id),
                )
                .await
                .map_err(|e| e.to_string())?;

            let forms = result
                .result
                .value
                .and_then(|v| serde_json::from_value::<Vec<Value>>(v).ok())
                .unwrap_or_default();

            // Apply filter if provided
            let forms: Vec<Value> = if let Some(filt) = filter {
                forms
                    .into_iter()
                    .filter(|form| {
                        form.get("action")
                            .and_then(|v| v.as_str())
                            .map(|a| a.contains(filt))
                            .unwrap_or(false)
                    })
                    .collect()
            } else {
                forms
            };

            Ok(json!({ "forms": forms, "count": forms.len() }))
        }

        "extract_breadcrumbs" => {
            let js = r#"(function() {
                var selectors = ['[role="navigation"] nav', '.breadcrumbs', '.breadcrumb', '[aria-label="Breadcrumb"]', 'nav ol', 'nav ul'];
                for (var s of selectors) {
                    var el = document.querySelector(s);
                    if (!el) continue;
                    var items = Array.from(el.querySelectorAll('a, span')).filter(function(e) { return e.textContent.trim().length > 0; });
                    if (items.length < 2) continue;
                    return {
                        items: items.map(function(e) { return { text: e.textContent.trim(), href: e.href || null }; }),
                        selector: s
                    };
                }
                return null;
            })()"#;

            let result: super::cdp::types::EvaluateResult = cdp_client
                .send_command_typed(
                    "Runtime.evaluate",
                    &super::cdp::types::EvaluateParams {
                        expression: js.to_string(),
                        return_by_value: Some(true),
                        await_promise: Some(false),
                    },
                    Some(session_id),
                )
                .await
                .map_err(|e| e.to_string())?;

            let breadcrumbs = result.result.value;
            Ok(json!({ "breadcrumbs": breadcrumbs }))
        }

        "compare_elements" => {
            let selectors = params
                .get("selectors")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>())
                .unwrap_or_default();
            let properties = params
                .get("properties")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>())
                .unwrap_or_else(|| vec!["textContent".to_string(), "href".to_string(), "src".to_string()]);

            let selectors_json = serde_json::to_string(&selectors).unwrap_or_default();
            let props_json = serde_json::to_string(&properties).unwrap_or_default();

            let js = format!(
                r#"(function() {{
                    var selectors = JSON.parse('{}');
                    var props = JSON.parse('{}');
                    return selectors.map(function(sel) {{
                        var el = document.querySelector(sel);
                        if (!el) return {{ selector: sel, error: 'Not found' }};
                        var result = {{ selector: sel, found: true }};
                        props.forEach(function(p) {{
                            try {{
                                result[p] = p === 'textContent' ? el.textContent.trim() :
                                            p === 'href' ? el.href :
                                            p === 'src' ? el.src :
                                            p === 'value' ? el.value :
                                            el.getAttribute(p) || null;
                            }} catch(e) {{ result[p] = null; }}
                        }});
                        return result;
                    }});
                }})()"#,
                selectors_json, props_json
            );

            let result: super::cdp::types::EvaluateResult = cdp_client
                .send_command_typed(
                    "Runtime.evaluate",
                    &super::cdp::types::EvaluateParams {
                        expression: js,
                        return_by_value: Some(true),
                        await_promise: Some(false),
                    },
                    Some(session_id),
                )
                .await
                .map_err(|e| e.to_string())?;

            let comparison = result.result.value;
            Ok(json!({ "comparison": comparison }))
        }

        "audit_accessibility" => {
            let severity = params.get("severity").and_then(|v| v.as_str()).unwrap_or("all");

            let js = format!(
                r#"(function() {{
                    var issues = [];
                    var all = document.querySelectorAll('img, a, button, input, select, textarea');
                    all.forEach(function(el) {{
                        if (el.tagName === 'IMG' && !el.alt) issues.push({{
                            severity: 'serious',
                            tag: 'IMG', selector: getSelector(el),
                            message: 'Image missing alt attribute'
                        }});
                        if ((el.tagName === 'A') && !el.textContent.trim() && !el.querySelector('img')) issues.push({{
                            severity: 'critical',
                            tag: 'A', selector: getSelector(el),
                            message: 'Link has no accessible name'
                        }});
                        if ((el.tagName === 'INPUT' || el.tagName === 'SELECT' || el.tagName === 'TEXTAREA') && !el.id && !el.closest('label')) issues.push({{
                            severity: 'serious',
                            tag: el.tagName, selector: getSelector(el),
                            message: 'Form control has no associated label'
                        }});
                    }});
                    return issues.filter(function(i) {{
                        if ('{}' === 'all') return true;
                        return i.severity === '{}';
                    }});
                }})()"#,
                severity, severity
            );

            let result: super::cdp::types::EvaluateResult = cdp_client
                .send_command_typed(
                    "Runtime.evaluate",
                    &super::cdp::types::EvaluateParams {
                        expression: js,
                        return_by_value: Some(true),
                        await_promise: Some(false),
                    },
                    Some(session_id),
                )
                .await
                .map_err(|e| e.to_string())?;

            let issues = result.result.value;
            Ok(json!({ "issues": issues, "count": issues.as_array().map(|a| a.len()).unwrap_or(0) }))
        }

        _ => Err(format!("Command '{}' requires CDP integration and is not yet implemented", cmd.id)),
    }
}

// ---------------------------------------------------------------------------
// Command dispatcher
// ---------------------------------------------------------------------------

/// Execute a built-in command by ID.
pub async fn dispatch_command(
    cmd_id: &str,
    params: Value,
    cdp_client: Option<&super::cdp::client::CdpClient>,
    session_id: Option<&str>,
    github_token: Option<&str>,
) -> Result<Value, String> {
    let commands = get_builtin_commands();
    let cmd = commands
        .iter()
        .find(|c| c.id == cmd_id)
        .ok_or_else(|| format!("Unknown command: {}", cmd_id))?;

    match cmd.id {
        "search_github" => {
            let query = params
                .get("query")
                .and_then(|v| v.as_str())
                .ok_or("Missing required field: query")?;
            let sort = params.get("sort").and_then(|v| v.as_str());
            let limit = params
                .get("limit")
                .and_then(|v| v.as_u64())
                .unwrap_or(5) as usize;
            search_github(query, sort, limit, github_token).await
        }

        "find_best_repo" | "best_github_repo" => {
            let task = params
                .get("task")
                .and_then(|v| v.as_str())
                .ok_or("Missing required field: task")?;
            let language = params.get("language").and_then(|v| v.as_str());
            let min_stars = params
                .get("min_stars")
                .and_then(|v| v.as_u64())
                .unwrap_or(100) as usize;
            find_best_repo(task, language, min_stars, github_token).await
        }

        "extract_structured_data" => {
            let cdp = cdp_client.ok_or("CDP client required for this command")?;
            let sid = session_id.ok_or("Session ID required for this command")?;
            let schema = params.get("schema").ok_or("Missing required field: schema")?;
            let selector = params
                .get("selector")
                .and_then(|v| v.as_str())
                .unwrap_or("body");

            let schema_json = serde_json::to_string(schema).unwrap_or_default();
            let js = format!(
                r#"(function() {{
                    var schema = {};
                    var el = document.querySelector('{}');
                    if (!el) return {{ error: 'Selector not found' }};
                    // Basic extraction based on schema properties
                    var result = {{}};
                    Object.keys(schema.properties || {{}}).forEach(function(key) {{
                        var sel = schema.properties[key].selector || '[data-{}]';
                        var match = el.querySelector(sel);
                        result[key] = match ? match.textContent.trim() : null;
                    }});
                    return result;
                }})()"#,
                schema_json, selector, selector.replace('\'', "\\'")
            );

            let result: super::cdp::types::EvaluateResult = cdp
                .send_command_typed(
                    "Runtime.evaluate",
                    &super::cdp::types::EvaluateParams {
                        expression: js,
                        return_by_value: Some(true),
                        await_promise: Some(false),
                    },
                    Some(sid),
                )
                .await
                .map_err(|e| e.to_string())?;

            Ok(json!({ "data": result.result.value }))
        }

        cmd_id => {
            // CDP-based page commands
            let cdp = cdp_client.ok_or("CDP client required for this command")?;
            let sid = session_id.ok_or("Session ID required for this command")?;
            execute_page_command(cmd, &params, cdp, sid).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_builtin_commands() {
        let cmds = get_builtin_commands();
        assert!(cmds.len() >= 8);
        assert!(cmds.iter().any(|c| c.id == "search_github"));
        assert!(cmds.iter().any(|c| c.id == "find_best_repo"));
        assert!(cmds.iter().any(|c| c.id == "audit_accessibility"));
    }
}
