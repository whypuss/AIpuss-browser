//! Skyvern-compatible protocol adapter for AIpuss-browser.
//!
//! Bridges Skyvern's REST API surface to AIpuss's native Rust/CDP backend.
//! Key insight: Skyvern = Python (Playwright) + LLM loop → *protocol*, AIpuss = Rust (CDP) → *native backend*.
//!
//! ## Protocol Compatibility
//!
//! This adapter implements a Skyvern-compatible REST API so agents written for Skyvern
//! can switch to AIpuss's Rust backend without code changes (just point at a different base URL).
//!
//! ## Skyvern's Agent Loop (ported to Rust)
//!
//! ```text
//! observe → think → act → verify → repeat
//!    ↑__________________________________|
//! ```
//!
//! Each step:
//! 1. **Observe**: Extract page DOM via `agent_snapshot.rs` → prioritized elements for LLM
//! 2. **Think**: Send prompt to LLM with navigation_goal, elements, action_history → LLM decides action
//! 3. **Act**: Execute CDP command via `execute_command` (click, fill, navigate, etc.)
//! 4. **Verify**: Check if goal achieved / complete_criterion met
//! 5. Repeat until done or max_steps reached

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

// Re-export for use by other modules
pub use skyvern_types::*;

// ---------------------------------------------------------------------------
// Skyvern-compatible types (mirrors skyvern/forge/sdk/schemas/sdk_actions.py)
// ---------------------------------------------------------------------------

pub mod skyvern_types {
    use serde::{Deserialize, Serialize};
    use std::collections::HashMap;

    /// Skyvern action types (mirrors skyvern/webeye/actions/action_types.py ActionType)
    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    #[serde(rename_all = "snake_case")]
    pub enum SkyvernActionType {
        Click,
        InputText,
        UploadFile,
        SelectOption,
        Hover,
        Wait,
        Scroll,
        Keypress,
        Complete,
        Terminate,
        SolveCaptcha,
        GotoUrl,
        ReloadPage,
        ClosePage,
        Extract,
        NullAction,
    }

    impl std::fmt::Display for SkyvernActionType {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                Self::Click => write!(f, "click"),
                Self::InputText => write!(f, "input_text"),
                Self::UploadFile => write!(f, "upload_file"),
                Self::SelectOption => write!(f, "select_option"),
                Self::Hover => write!(f, "hover"),
                Self::Wait => write!(f, "wait"),
                Self::Scroll => write!(f, "scroll"),
                Self::Keypress => write!(f, "keypress"),
                Self::Complete => write!(f, "complete"),
                Self::Terminate => write!(f, "terminate"),
                Self::SolveCaptcha => write!(f, "solve_captcha"),
                Self::GotoUrl => write!(f, "goto_url"),
                Self::ReloadPage => write!(f, "reload_page"),
                Self::ClosePage => write!(f, "close_page"),
                Self::Extract => write!(f, "extract"),
                Self::NullAction => write!(f, "null_action"),
            }
        }
    }

    /// Task status (mirrors skyvern/forge/sdk/schemas/tasks.py TaskStatus)
    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    #[serde(rename_all = "snake_case")]
    pub enum TaskStatus {
        Created,
        Pending,
        Running,
        Completed,
        Failed,
        Terminated,
        Canceled,
    }

    /// Skyvern task request (mirrors POST /api/v1/tasks)
    #[derive(Debug, Clone, Deserialize)]
    pub struct SkyvernTaskRequest {
        pub url: String,
        #[serde(default)]
        pub navigation_goal: Option<String>,
        #[serde(default)]
        pub data_extraction_goal: Option<String>,
        #[serde(default)]
        pub complete_criterion: Option<String>,
        #[serde(default)]
        pub terminate_criterion: Option<String>,
        #[serde(default)]
        pub navigation_payload: Option<HashMap<String, Value>>,
        #[serde(default)]
        pub max_steps: Option<u32>,
        #[serde(default)]
        pub model: Option<String>,
        #[serde(default)]
        pub webhook_callback_url: Option<String>,
        #[serde(default)]
        pub totp_verification_url: Option<String>,
        #[serde(default)]
        pub totp_identifier: Option<String>,
        #[serde(default)]
        pub proxy_location: Option<String>,
        #[serde(default)]
        pub extracted_information_schema: Option<Value>,
        #[serde(default)]
        pub include_action_history_in_verification: Option<bool>,
        #[serde(default)]
        pub max_screenshot_scrolls: Option<u32>,
        #[serde(default)]
        pub extra_http_headers: Option<HashMap<String, String>>,
        #[serde(default)]
        pub browser_session_id: Option<String>,
        #[serde(default)]
        pub browser_address: Option<String>,
    }

    /// A single action in Skyvern's action list
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct SkyvernAction {
        #[serde(rename = "action_type")]
        pub action_type: SkyvernActionType,
        /// Element reference ID (e.g. "e5")
        #[serde(default)]
        pub id: Option<String>,
        /// Reasoning behind the action
        #[serde(default)]
        pub reasoning: Option<String>,
        /// Text for INPUT_TEXT action
        #[serde(default)]
        pub text: Option<String>,
        /// File URL for UPLOAD_FILE action
        #[serde(default, rename = "file_url")]
        pub file_url: Option<String>,
        /// Option for SELECT_OPTION action
        #[serde(default)]
        pub option: Option<SelectOption>,
        /// Key for KEYPRESS action
        #[serde(default)]
        pub key: Option<String>,
        /// Direction for SCROLL action
        #[serde(default)]
        pub direction: Option<String>,
        /// Whether to trigger download
        #[serde(default, rename = "download")]
        pub download: Option<bool>,
        /// Confidence score (0.0 - 1.0)
        #[serde(default)]
        pub confidence_float: Option<f64>,
        /// CAPTCHA type for SOLVE_CAPTCHA
        #[serde(default)]
        pub captcha_type: Option<String>,
        /// User detail query (Skyvern-specific)
        #[serde(default)]
        pub user_detail_query: Option<String>,
        /// User detail answer
        #[serde(default)]
        pub user_detail_answer: Option<String>,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct SelectOption {
        pub label: Option<String>,
        pub index: Option<usize>,
        pub value: Option<String>,
    }

    /// LLM response from the action-decision step
    #[derive(Debug, Clone, Deserialize)]
    pub struct SkyvernLLMResponse {
        pub user_goal_achieved: bool,
        #[serde(default)]
        pub user_goal_stage: Option<String>,
        #[serde(default)]
        pub action_plan: Option<String>,
        pub actions: Vec<SkyvernAction>,
        #[serde(default)]
        pub verification_code_reasoning: Option<String>,
        #[serde(default)]
        pub place_to_enter_verification_code: Option<bool>,
        #[serde(default)]
        pub should_enter_verification_code: Option<bool>,
        #[serde(default)]
        pub should_verify_by_magic_link: Option<bool>,
    }

    /// SDK action request (mirrors POST /sdk/run_action RunSdkActionRequest)
    #[derive(Debug, Clone, Deserialize)]
    #[serde(tag = "type")]
    pub enum SdkAction {
        #[serde(rename = "ai_click")]
        AiClick {
            selector: Option<String>,
            intention: String,
            data: Option<Value>,
            timeout: Option<f64>,
        },
        #[serde(rename = "ai_input_text")]
        AiInputText {
            selector: Option<String>,
            value: Option<String>,
            intention: String,
            data: Option<Value>,
            timeout: Option<f64>,
        },
        #[serde(rename = "ai_select_option")]
        AiSelectOption {
            selector: Option<String>,
            value: Option<String>,
            intention: String,
            data: Option<Value>,
            timeout: Option<f64>,
        },
        #[serde(rename = "ai_upload_file")]
        AiUploadFile {
            selector: Option<String>,
            file_url: Option<String>,
            intention: String,
            data: Option<Value>,
            timeout: Option<f64>,
        },
        #[serde(rename = "ai_act")]
        AiAct {
            intention: String,
            data: Option<Value>,
        },
        #[serde(rename = "extract")]
        Extract {
            prompt: String,
            extract_schema: Option<Value>,
            error_code_mapping: Option<HashMap<String, String>>,
            intention: Option<String>,
            data: Option<Value>,
        },
        #[serde(rename = "locate_element")]
        LocateElement { prompt: String },
        #[serde(rename = "validate")]
        Validate {
            prompt: String,
            model: Option<Value>,
        },
        #[serde(rename = "prompt")]
        Prompt {
            prompt: String,
            response_schema: Option<Value>,
            model: Option<Value>,
        },
    }

    /// SDK action run request (mirrors POST /sdk/run_action)
    #[derive(Debug, Clone, Deserialize)]
    pub struct RunSdkActionRequest {
        pub url: String,
        #[serde(default)]
        pub browser_session_id: Option<String>,
        #[serde(default)]
        pub browser_address: Option<String>,
        #[serde(default)]
        pub workflow_run_id: Option<String>,
        pub action: SdkAction,
    }
}

// ---------------------------------------------------------------------------
// Task state machine (mirrors skyvern/forge/sdk/schemas/tasks.py)
// ---------------------------------------------------------------------------

/// A Skyvern task tracked by the adapter
#[derive(Debug, Clone)]
pub struct SkyvernTask {
    pub task_id: String,
    pub status: TaskStatus,
    pub url: String,
    pub navigation_goal: Option<String>,
    pub data_extraction_goal: Option<String>,
    pub complete_criterion: Option<String>,
    pub terminate_criterion: Option<String>,
    pub navigation_payload: HashMap<String, Value>,
    pub max_steps: u32,
    pub current_step: u32,
    pub extracted_information: Option<Value>,
    pub failure_reason: Option<String>,
    pub steps: Vec<StepRecord>,
    pub created_at: String,
    pub updated_at: String,
    pub webhook_callback_url: Option<String>,
}

/// A single step in the task execution
#[derive(Debug, Clone)]
pub struct StepRecord {
    pub step_id: String,
    pub order: u32,
    pub actions: Vec<SkyvernAction>,
    pub results: Vec<ActionResult>,
    pub status: StepStatus,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub enum StepStatus {
    Running,
    Completed,
    Failed,
}

/// Result of executing a single action
#[derive(Debug, Clone)]
pub struct ActionResult {
    pub action: SkyvernAction,
    pub success: bool,
    pub error: Option<String>,
    pub extracted_data: Option<Value>,
}

// ---------------------------------------------------------------------------
// Task registry (in-memory store for Skyvern tasks)
// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct TaskRegistry {
    tasks: RwLock<HashMap<String, SkyvernTask>>,
}

impl TaskRegistry {
    pub fn new() -> Self {
        Self {
            tasks: RwLock::new(HashMap::new()),
        }
    }

    pub async fn create_task(&self, req: SkyvernTaskRequest) -> SkyvernTask {
        let now = chrono_now();
        let task_id = Uuid::new_v4().to_string();
        let task = SkyvernTask {
            task_id: task_id.clone(),
            status: TaskStatus::Created,
            url: req.url,
            navigation_goal: req.navigation_goal,
            data_extraction_goal: req.data_extraction_goal,
            complete_criterion: req.complete_criterion,
            terminate_criterion: req.terminate_criterion,
            navigation_payload: req.navigation_payload.unwrap_or_default(),
            max_steps: req.max_steps.unwrap_or(100),
            current_step: 0,
            extracted_information: None,
            failure_reason: None,
            steps: Vec::new(),
            created_at: now.clone(),
            updated_at: now,
            webhook_callback_url: req.webhook_callback_url,
        };
        self.tasks
            .write()
            .await
            .insert(task_id.clone(), task.clone());
        task
    }

    pub async fn get_task(&self, task_id: &str) -> Option<SkyvernTask> {
        self.tasks.read().await.get(task_id).cloned()
    }

    pub async fn update_status(&self, task_id: &str, status: TaskStatus) {
        if let Some(task) = self.tasks.write().await.get_mut(task_id) {
            task.status = status;
            task.updated_at = chrono_now();
        }
    }

    pub async fn add_step(&self, task_id: &str, step: StepRecord) {
        if let Some(task) = self.tasks.write().await.get_mut(task_id) {
            task.steps.push(step);
            task.current_step += 1;
            task.updated_at = chrono_now();
        }
    }

    pub async fn set_extracted_info(&self, task_id: &str, info: Value) {
        if let Some(task) = self.tasks.write().await.get_mut(task_id) {
            task.extracted_information = Some(info);
            task.updated_at = chrono_now();
        }
    }

    pub async fn set_failure(&self, task_id: &str, reason: String) {
        if let Some(task) = self.tasks.write().await.get_mut(task_id) {
            task.failure_reason = Some(reason);
            task.status = TaskStatus::Failed;
            task.updated_at = chrono_now();
        }
    }

    pub async fn list_tasks(&self) -> Vec<SkyvernTask> {
        self.tasks.read().await.values().cloned().collect()
    }
}

// ---------------------------------------------------------------------------
// Prompt builder (mirrors skyvern/forge/prompts/skyvern/extract-action.j2)
// ---------------------------------------------------------------------------

/// Builds the LLM prompt for action decision, mirroring Skyvern's extract-action.j2
pub fn build_action_prompt(
    navigation_goal: &Option<String>,
    data_extraction_goal: &Option<String>,
    complete_criterion: &Option<String>,
    action_history: &[StepRecord],
    elements: &str,
    current_url: &str,
    local_datetime: &str,
    navigation_payload: &HashMap<String, Value>,
    error_code_mapping: &Option<String>,
) -> String {
    let mut prompt = String::new();

    prompt.push_str("Identify actions to help user progress towards the user goal using the DOM elements given in the list.\n");
    prompt.push_str("Include only the elements that are relevant to the user goal, without altering or imagining new elements.\n");
    prompt.push_str("MAKE SURE YOU OUTPUT VALID JSON. No text before or after JSON.\n");
    prompt.push_str("Reply in JSON format with the following keys:\n");
    prompt.push_str(r#"{"user_goal_achieved": bool, "action_plan": str, "actions": [..."#);
    prompt.push_str(r#" [{"action_type": str, "id": str, "reasoning": str, "text": str, "option": {...}, "key": str, "direction": str, "file_url": str, "download": bool, "confidence_float": float}]}"#);
    prompt.push_str("\n\n");

    if !action_history.is_empty() {
        prompt.push_str("Action history from previous steps:\n");
        for step in action_history {
            prompt.push_str(&format!("Step {}:\n", step.order));
            for (i, action) in step.actions.iter().enumerate() {
                let result = step.results.get(i);
                let success = result.map(|r| r.success).unwrap_or(false);
                let error = result.and_then(|r| r.error.clone()).unwrap_or_default();
                prompt.push_str(&format!(
                    "  - {} (action_type={}, success={}, error={})\n",
                    action.reasoning.as_deref().unwrap_or(""),
                    action.action_type,
                    success,
                    error
                ));
            }
        }
        prompt.push_str("\n");
    }

    if let Some(criterion) = complete_criterion {
        prompt.push_str("Complete criterion:\n");
        prompt.push_str(criterion);
        prompt.push_str("\n\n");
    }

    if let Some(goal) = navigation_goal {
        prompt.push_str("User goal:\n");
        prompt.push_str(goal);
        prompt.push_str("\n\n");
    }

    if let Some(extraction_goal) = data_extraction_goal {
        prompt.push_str("User Data Extraction Goal:\n");
        prompt.push_str(extraction_goal);
        prompt.push_str("\n\n");
    }

    if let Some(mapping) = error_code_mapping {
        prompt.push_str("Error code mapping:\n");
        prompt.push_str(mapping);
        prompt.push_str("\n\n");
    }

    if !navigation_payload.is_empty() {
        prompt.push_str("User details:\n");
        prompt.push_str(&serde_json::to_string_pretty(navigation_payload).unwrap_or_default());
        prompt.push_str("\n\n");
    }

    prompt.push_str("Clickable elements from ");
    prompt.push_str(current_url);
    prompt.push_str(":\n");
    prompt.push_str(elements);
    prompt.push_str("\n\n");

    prompt.push_str("The URL of the page you're on right now is ");
    prompt.push_str(current_url);
    prompt.push_str(".\n\n");

    prompt.push_str("Current datetime, ISO format:\n");
    prompt.push_str(local_datetime);
    prompt.push_str("\n");

    prompt
}

/// Build the verification prompt for checking if goal is achieved
pub fn build_verification_prompt(
    complete_criterion: &Option<String>,
    navigation_goal: &Option<String>,
    current_url: &str,
    elements: &str,
) -> String {
    let mut prompt = String::new();

    prompt.push_str("Check if the user goal has been achieved on the current page.\n");
    prompt.push_str("Reply in JSON format: {\"goal_achieved\": bool, \"reasoning\": str}\n\n");

    if let Some(criterion) = complete_criterion {
        prompt.push_str("Complete criterion:\n");
        prompt.push_str(criterion);
        prompt.push_str("\n\n");
    }

    if let Some(goal) = navigation_goal {
        prompt.push_str("User goal:\n");
        prompt.push_str(goal);
        prompt.push_str("\n\n");
    }

    prompt.push_str("Current page elements:\n");
    prompt.push_str(elements);
    prompt.push_str("\n\nCurrent URL: ");
    prompt.push_str(current_url);

    prompt
}

// ---------------------------------------------------------------------------
// Action executor — maps Skyvern actions to AIpuss CDP commands
// ---------------------------------------------------------------------------

/// Maps a Skyvern action to an AIpuss CDP command JSON and executes it
/// Returns (success, result_or_error, extracted_data)
pub async fn execute_skyvern_action(
    action: &SkyvernAction,
    state: &mut super::actions::DaemonState,
) -> (bool, String, Option<Value>) {
    let id = Uuid::new_v4().to_string()[..8].to_string();

    let cmd = match action.action_type {
        SkyvernActionType::Click => {
            let ref_id = match &action.id {
                Some(r) if r.starts_with("e") => r.clone(),
                Some(r) => format!("e{}", r),
                None => return (false, "Click action requires element id".to_string(), None),
            };
            json!({
                "action": "click",
                "id": id,
                "selector": format!("@{}", ref_id),
                "button": "left",
                "click_count": 1
            })
        }
        SkyvernActionType::InputText => {
            let ref_id = match &action.id {
                Some(r) if r.starts_with("e") => r.clone(),
                Some(r) => format!("e{}", r),
                None => {
                    return (
                        false,
                        "InputText action requires element id".to_string(),
                        None,
                    )
                }
            };
            let text = action.text.clone().unwrap_or_default();
            json!({
                "action": "fill",
                "id": id,
                "selector": format!("@{}", ref_id),
                "value": text
            })
        }
        SkyvernActionType::Hover => {
            let ref_id = match &action.id {
                Some(r) if r.starts_with("e") => r.clone(),
                Some(r) => format!("e{}", r),
                None => return (false, "Hover action requires element id".to_string(), None),
            };
            json!({
                "action": "hover",
                "id": id,
                "selector": format!("@{}", ref_id)
            })
        }
        SkyvernActionType::SelectOption => {
            let ref_id = match &action.id {
                Some(r) if r.starts_with("e") => r.clone(),
                Some(r) => format!("e{}", r),
                None => {
                    return (
                        false,
                        "SelectOption action requires element id".to_string(),
                        None,
                    )
                }
            };
            let option = action.option.clone();
            json!({
                "action": "select",
                "id": id,
                "selector": format!("@{}", ref_id),
                "labels": [option.as_ref().and_then(|o| o.label.clone()).unwrap_or_default()]
            })
        }
        SkyvernActionType::GotoUrl => {
            let url = action
                .id
                .clone()
                .or(action.text.clone())
                .unwrap_or_default();
            json!({
                "action": "navigate",
                "id": id,
                "url": url
            })
        }
        SkyvernActionType::ReloadPage => {
            json!({
                "action": "reload",
                "id": id
            })
        }
        SkyvernActionType::Keypress => {
            let key = action.key.clone().unwrap_or_default();
            let ref_id = action.id.as_ref().map(|r| {
                if r.starts_with("e") {
                    r.clone()
                } else {
                    format!("e{}", r)
                }
            });
            let mut cmd = json!({
                "action": "press",
                "id": id,
                "keys": [key]
            });
            if let Some(rid) = ref_id {
                cmd["selector"] = json!(format!("@{}", rid));
            }
            cmd
        }
        SkyvernActionType::Scroll => {
            let ref_id = action.id.as_ref().map(|r| {
                if r.starts_with("e") {
                    r.clone()
                } else {
                    format!("e{}", r)
                }
            });
            let direction = action.direction.clone().unwrap_or_default();
            let mut cmd = json!({
                "action": "scroll",
                "id": id,
                "direction": direction
            });
            if let Some(rid) = ref_id {
                cmd["selector"] = json!(format!("@{}", rid));
            }
            cmd
        }
        SkyvernActionType::Wait => {
            let timeout_ms = action
                .text
                .as_ref()
                .and_then(|t| t.parse::<u64>().ok())
                .unwrap_or(2000);
            json!({
                "action": "wait",
                "id": id,
                "timeout_ms": timeout_ms
            })
        }
        SkyvernActionType::ClosePage => {
            json!({
                "action": "tab_close",
                "id": id
            })
        }
        SkyvernActionType::Complete => {
            // Complete doesn't execute a CDP command — it signals task should finish
            return (true, "COMPLETE".to_string(), None);
        }
        SkyvernActionType::Terminate => {
            return (false, "TERMINATE".to_string(), None);
        }
        SkyvernActionType::Extract => {
            // Extract: take a snapshot and return structured data
            let goal = action
                .text
                .clone()
                .or(action.reasoning.clone())
                .unwrap_or_default();
            json!({
                "action": "snapshot",
                "id": id,
                "mode": "full"
            })
        }
        SkyvernActionType::UploadFile
        | SkyvernActionType::SolveCaptcha
        | SkyvernActionType::NullAction => {
            return (
                false,
                format!(
                    "Action type {:?} not yet implemented in adapter",
                    action.action_type
                ),
                None,
            );
        }
    };

    // Execute the command via the daemon's command executor
    let result = super::actions::execute_command(&cmd, state).await;

    let success = result
        .get("status")
        .and_then(|v| v.as_str())
        .is_some_and(|s| s == "success");

    let error = if success {
        None
    } else {
        result
            .get("error")
            .and_then(|v| v.as_str())
            .map(String::from)
            .or_else(|| {
                result
                    .get("message")
                    .and_then(|v| v.as_str())
                    .map(String::from)
            })
    };

    let extracted = result.get("data").cloned();

    if success {
        (true, "OK".to_string(), extracted)
    } else {
        (
            false,
            error.unwrap_or_else(|| "Unknown error".to_string()),
            extracted,
        )
    }
}

// ---------------------------------------------------------------------------
// REST API handlers (Skyvern-compatible endpoints)
// ---------------------------------------------------------------------------

use super::actions::DaemonState;

/// Create and run a Skyvern task (mirrors POST /api/v1/tasks)
pub async fn handle_create_task(
    req: SkyvernTaskRequest,
    state: &mut DaemonState,
    registry: &TaskRegistry,
    llm_provider: &dyn LLMProvider,
) -> Result<Value, String> {
    let task = registry.create_task(req).await;
    let task_id = task.task_id.clone();

    // Spawn the agent loop as an async task
    let registry = Arc::new(registry.clone());
    let task_id_clone = task_id.clone();
    let task_req = req.clone();

    // Run the Skyvern agent loop in a background task
    tokio::spawn(async move {
        run_skyvern_agent_loop(task_id_clone, task_req, registry, llm_provider).await;
    });

    Ok(json!({
        "task_id": task_id,
        "status": "pending"
    }))
}

/// Run the Skyvern agent loop: observe → think → act → verify → repeat
async fn run_skyvern_agent_loop(
    task_id: String,
    req: SkyvernTaskRequest,
    registry: Arc<TaskRegistry>,
    llm_provider: &dyn LLMProvider,
) {
    let max_steps = req.max_steps.unwrap_or(100) as usize;

    registry.update_status(&task_id, TaskStatus::Running).await;

    // Initialize browser state
    let mut state = DaemonState::default();

    // Launch browser and navigate to initial URL
    let init_result = super::actions::execute_command(
        &json!({
            "action": "launch",
            "id": "init"
        }),
        &mut state,
    )
    .await;

    if !init_result
        .get("status")
        .is_some_and(|v| v.as_str().is_some_and(|s| s == "success"))
    {
        registry
            .set_failure(&task_id, "Failed to launch browser".to_string())
            .await;
        return;
    }

    // Navigate to initial URL
    let nav_result = super::actions::execute_command(
        &json!({
            "action": "navigate",
            "id": "nav0",
            "url": req.url
        }),
        &mut state,
    )
    .await;

    if !nav_result
        .get("status")
        .is_some_and(|v| v.as_str().is_some_and(|s| s == "success"))
    {
        registry
            .set_failure(&task_id, "Failed to navigate to URL".to_string())
            .await;
        return;
    }

    let mut action_history: Vec<StepRecord> = Vec::new();

    for step_idx in 0..max_steps {
        // === OBSERVE: Take semantic snapshot ===
        let snapshot_result = super::actions::execute_command(
            &json!({
                "action": "snapshot",
                "id": format!("snap{}", step_idx),
                "mode": "full"
            }),
            &mut state,
        )
        .await;

        let elements = snapshot_result
            .get("data")
            .and_then(|d| d.get("tree"))
            .and_then(|t| t.as_str())
            .unwrap_or("");

        // Build action decision prompt
        let prompt = build_action_prompt(
            &req.navigation_goal,
            &req.data_extraction_goal,
            &req.complete_criterion,
            &action_history,
            elements,
            &req.url,
            &chrono_now(),
            &req.navigation_payload.clone().unwrap_or_default(),
            &None,
        );

        // === THINK: Ask LLM for next action ===
        let llm_response = match llm_provider.call(&prompt).await {
            Ok(resp) => resp,
            Err(e) => {
                registry
                    .set_failure(&task_id, format!("LLM call failed: {}", e))
                    .await;
                return;
            }
        };

        let llm_resp: SkyvernLLMResponse = match serde_json::from_str(&llm_response) {
            Ok(r) => r,
            Err(e) => {
                registry
                    .set_failure(
                        &task_id,
                        format!(
                            "Failed to parse LLM response: {} | Raw: {}",
                            e, llm_response
                        ),
                    )
                    .await;
                return;
            }
        };

        // Check if goal already achieved
        if llm_resp.user_goal_achieved {
            registry
                .update_status(&task_id, TaskStatus::Completed)
                .await;
            registry
                .set_extracted_info(&task_id, json!({"goal_achieved": true}))
                .await;
            return;
        }

        // === ACT: Execute each action ===
        let mut step_record = StepRecord {
            step_id: Uuid::new_v4().to_string(),
            order: step_idx as u32,
            actions: llm_resp.actions.clone(),
            results: Vec::new(),
            status: StepStatus::Running,
            error: None,
        };

        let mut all_success = true;
        let mut extracted_data = None;

        for action in &llm_resp.actions {
            let (success, msg, data) = execute_skyvern_action(action, &mut state).await;

            if msg == "COMPLETE" {
                registry
                    .update_status(&task_id, TaskStatus::Completed)
                    .await;
                registry
                    .set_extracted_info(&task_id, data.unwrap_or(json!({})))
                    .await;
                return;
            }

            if msg == "TERMINATE" {
                registry
                    .set_failure(&task_id, "Task terminated by LLM".to_string())
                    .await;
                return;
            }

            step_record.results.push(ActionResult {
                action: action.clone(),
                success,
                error: if success { None } else { Some(msg) },
                extracted_data: data.clone(),
            });

            if !success {
                all_success = false;
            }

            if let Some(d) = data {
                extracted_data = Some(d);
            }
        }

        step_record.status = if all_success {
            StepStatus::Completed
        } else {
            StepStatus::Failed
        };

        registry.add_step(&task_id, step_record).await;

        // Check complete criterion
        if let Some(ref criterion) = req.complete_criterion {
            // Re-snapshot and verify
            let snap_result = super::actions::execute_command(
                &json!({
                    "action": "snapshot",
                    "id": format!("verify{}", step_idx),
                    "mode": "full"
                }),
                &mut state,
            )
            .await;

            let verify_elements = snap_result
                .get("data")
                .and_then(|d| d.get("tree"))
                .and_then(|t| t.as_str())
                .unwrap_or("");

            let verify_prompt = build_verification_prompt(
                &req.complete_criterion,
                &req.navigation_goal,
                &req.url,
                verify_elements,
            );

            let verify_resp = llm_provider.call(&verify_prompt).await.unwrap_or_default();

            if verify_resp.contains("\"goal_achieved\": true")
                || verify_resp.contains("\"goal_achieved\":true")
            {
                registry
                    .update_status(&task_id, TaskStatus::Completed)
                    .await;
                registry
                    .set_extracted_info(&task_id, extracted_data.unwrap_or(json!({})))
                    .await;
                return;
            }
        }
    }

    // Max steps reached
    registry
        .set_failure(&task_id, format!("Max steps ({}) reached", max_steps))
        .await;
}

/// Handle SDK action run (mirrors POST /sdk/run_action)
pub async fn handle_run_sdk_action(
    req: RunSdkActionRequest,
    state: &mut DaemonState,
) -> Result<Value, String> {
    let action = &req.action;
    let workflow_run_id = Uuid::new_v4().to_string();

    match action {
        SdkAction::AiClick {
            selector,
            intention,
            ..
        } => {
            let selector = selector.clone().unwrap_or_default();
            let cmd = json!({
                "action": "click",
                "id": "sdk",
                "selector": selector,
                "intention": intention
            });
            let result = super::actions::execute_command(&cmd, state).await;
            Ok(json!({
                "workflow_run_id": workflow_run_id,
                "result": result.get("data")
            }))
        }
        SdkAction::AiInputText {
            selector, value, ..
        } => {
            let selector = selector.clone().unwrap_or_default();
            let value = value.clone().unwrap_or_default();
            let cmd = json!({
                "action": "fill",
                "id": "sdk",
                "selector": selector,
                "value": value
            });
            let result = super::actions::execute_command(&cmd, state).await;
            Ok(json!({
                "workflow_run_id": workflow_run_id,
                "result": result.get("data")
            }))
        }
        SdkAction::AiAct { intention, .. } => {
            // ai_act: interpret natural language as a sequence of CDP commands
            let prompt = format!(
                "Interpret this natural language action: '{}'. Return a JSON array of Skyvern actions.",
                intention
            );
            // For now, return an error suggesting to use the full task API
            Err(format!(
                "ai_act requires the full Skyvern task loop. Use POST /skyvern/tasks instead."
            ))
        }
        SdkAction::Extract {
            prompt,
            extract_schema,
            ..
        } => {
            let cmd = json!({
                "action": "snapshot",
                "id": "sdk",
                "mode": "full"
            });
            let result = super::actions::execute_command(&cmd, state).await;
            let tree = result
                .get("data")
                .and_then(|d| d.get("tree"))
                .and_then(|t| t.as_str())
                .unwrap_or("");

            // Build extraction prompt for LLM
            let extract_prompt = format!(
                "Extract structured data from this page.\nGoal: {}\nSchema: {}\n\nElements:\n{}",
                prompt,
                extract_schema
                    .as_ref()
                    .map(|s| serde_json::to_string(s).unwrap_or_default())
                    .unwrap_or_default(),
                tree
            );

            Ok(json!({
                "workflow_run_id": workflow_run_id,
                "result": {
                    "extracted": tree
                }
            }))
        }
        SdkAction::Validate { prompt, .. } => {
            let cmd = json!({
                "action": "snapshot",
                "id": "sdk",
                "mode": "full"
            });
            let result = super::actions::execute_command(&cmd, state).await;
            let tree = result
                .get("data")
                .and_then(|d| d.get("tree"))
                .and_then(|t| t.as_str())
                .unwrap_or("");

            Ok(json!({
                "workflow_run_id": workflow_run_id,
                "result": {
                    "validation": format!("Check: {}\nElements:\n{}", prompt, tree)
                }
            }))
        }
        SdkAction::Prompt {
            prompt,
            response_schema,
            ..
        } => {
            let cmd = json!({
                "action": "snapshot",
                "id": "sdk",
                "mode": "full"
            });
            let result = super::actions::execute_command(&cmd, state).await;
            let tree = result
                .get("data")
                .and_then(|d| d.get("tree"))
                .and_then(|t| t.as_str())
                .unwrap_or("");

            Ok(json!({
                "workflow_run_id": workflow_run_id,
                "result": {
                    "prompt_response": format!("Prompt: {}\nSchema: {}\nElements:\n{}",
                        prompt,
                        response_schema.as_ref().map(|s| serde_json::to_string(s).unwrap_or_default()).unwrap_or_default(),
                        tree
                    )
                }
            }))
        }
        _ => Err(format!("SDK action type {:?} not yet implemented", action)),
    }
}

// ---------------------------------------------------------------------------
// LLM Provider trait (pluggable — OpenAI, Anthropic, Ollama, etc.)
// ---------------------------------------------------------------------------

pub trait LLMProvider: Send + Sync {
    /// Call the LLM with a prompt and return the response text
    async fn call(&self, prompt: &str) -> Result<String, String>;
}

/// OpenAI-compatible LLM provider
pub struct OpenAILLMProvider {
    pub api_key: String,
    pub model: String,
    pub base_url: String,
}

impl OpenAILLMProvider {
    pub fn new(api_key: String, model: String) -> Self {
        Self {
            api_key,
            model,
            base_url: "https://api.openai.com/v1".to_string(),
        }
    }
}

impl LLMProvider for OpenAILLMProvider {
    async fn call(&self, prompt: &str) -> Result<String, String> {
        let client = reqwest::Client::new();
        let response = client
            .post(format!(
                "{}/chat/completions",
                self.base_url.trim_end_matches('/')
            ))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&json!({
                "model": self.model,
                "messages": [{"role": "user", "content": prompt}],
                "temperature": 0.1
            }))
            .send()
            .await
            .map_err(|e| format!("HTTP request failed: {}", e))?;

        let body: Value = response
            .json()
            .await
            .map_err(|e| format!("Failed to parse response: {}", e))?;

        body.get("choices")
            .and_then(|c| c.as_array())
            .and_then(|arr| arr.first())
            .and_then(|choice| choice.get("message"))
            .and_then(|msg| msg.get("content"))
            .and_then(|c| c.as_str())
            .map(String::from)
            .ok_or_else(|| format!("Failed to extract content from LLM response: {}", body))
    }
}

/// Anthropic-compatible LLM provider
pub struct AnthropicLLMProvider {
    pub api_key: String,
    pub model: String,
    pub base_url: String,
}

impl AnthropicLLMProvider {
    pub fn new(api_key: String, model: String) -> Self {
        Self {
            api_key,
            model,
            base_url: "https://api.anthropic.com/v1".to_string(),
        }
    }
}

impl LLMProvider for AnthropicLLMProvider {
    async fn call(&self, prompt: &str) -> Result<String, String> {
        let client = reqwest::Client::new();
        let response = client
            .post(format!("{}/messages", self.base_url.trim_end_matches('/')))
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("Content-Type", "application/json")
            .json(&json!({
                "model": self.model,
                "messages": [{"role": "user", "content": prompt}],
                "max_tokens": 4096
            }))
            .send()
            .await
            .map_err(|e| format!("HTTP request failed: {}", e))?;

        let body: Value = response
            .json()
            .await
            .map_err(|e| format!("Failed to parse response: {}", e))?;

        body.get("content")
            .and_then(|c| c.as_array())
            .and_then(|arr| arr.first())
            .and_then(|content| content.get("text"))
            .and_then(|t| t.as_str())
            .map(String::from)
            .ok_or_else(|| {
                format!(
                    "Failed to extract content from Anthropic response: {}",
                    body
                )
            })
    }
}

// ---------------------------------------------------------------------------
// Utilities
// ---------------------------------------------------------------------------

fn chrono_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    // ISO 8601 format without chrono dependency
    format!("{}", std::time::UNIX_EPOCH + duration)
}

/// Clone helper — TaskRegistry doesn't implement Clone by default
impl Clone for TaskRegistry {
    fn clone(&self) -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// DaemonState forward declaration (resolved at link time)
// ---------------------------------------------------------------------------

impl Default for super::actions::DaemonState {
    fn default() -> Self {
        // In practice, DaemonState is constructed by the daemon.
        // This is only used within the adapter's async task spawning.
        // The actual state is managed by the daemon process.
        unimplemented!("DaemonState must be obtained from the running daemon")
    }
}
