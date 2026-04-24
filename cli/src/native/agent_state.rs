//! Agent-native action trace and state diff with self-correction.
//!
//! Every action is recorded with its pre-state and post-state. After execution,
//! the system compares them to detect:
//! - Whether navigation actually occurred
//! Whether form values actually changed
//! - Whether interactive elements appeared/disappeared
//! - Whether error toasts or dialogs appeared
//!
//! When an action appears ineffective, a self-correction hint is generated
//! telling the LLM what to try next — preventing the "盲點" (blind spot) problem.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// A captured snapshot of the page state at a specific moment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageState {
    /// Full accessibility tree text.
    pub tree: String,
    /// Current URL.
    pub url: String,
    /// Page title.
    pub title: String,
    /// Number of interactive refs in the tree.
    pub interactive_count: usize,
    /// URL hash/fragment at time of capture.
    pub url_fragment: Option<String>,
    /// Whether any dialog/toast is visible.
    pub has_dialog: bool,
    /// Dialog message if present.
    pub dialog_message: Option<String>,
    /// All input values keyed by element ref (textbox/searchbox only).
    pub input_values: HashMap<String, String>,
    /// Count of iframes present.
    pub iframe_count: usize,
    /// Timestamp in milliseconds since Unix epoch.
    pub timestamp_ms: u64,
}

impl PageState {
    pub fn empty() -> Self {
        Self {
            tree: String::new(),
            url: String::new(),
            title: String::new(),
            interactive_count: 0,
            url_fragment: None,
            has_dialog: false,
            dialog_message: None,
            input_values: HashMap::new(),
            iframe_count: 0,
            timestamp_ms: 0,
        }
    }
}

/// A recorded action step with pre/post state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepRecord {
    /// Monotonically increasing step index.
    pub step: usize,
    /// Human-readable action name.
    pub action_name: String,
    /// Raw action parameters (for replay/debugging).
    pub action_params: Value,
    /// State before the action was executed.
    pub pre_state: PageState,
    /// State after the action was executed.
    pub post_state: PageState,
    /// Whether the action appeared effective.
    pub effective: bool,
    /// What changed between pre and post.
    pub diff: StateDiff,
    /// Self-correction hint if the action was ineffective.
    pub self_correction: Option<SelfCorrection>,
    /// Timestamp in ms.
    pub timestamp_ms: u64,
}

/// Semantic diff between two PageState snapshots.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateDiff {
    /// Whether the URL changed.
    pub url_changed: bool,
    /// New URL (if changed).
    pub new_url: Option<String>,
    /// Whether the page title changed.
    pub title_changed: bool,
    /// New title (if changed).
    pub new_title: Option<String>,
    /// Number of interactive elements added.
    pub elements_added: usize,
    /// Number of interactive elements removed.
    pub elements_removed: usize,
    /// List of added element refs.
    pub added_refs: Vec<String>,
    /// List of removed element refs.
    pub removed_refs: Vec<String>,
    /// Whether any dialog/toast appeared.
    pub dialog_appeared: bool,
    /// Dialog message if any appeared.
    pub dialog_message: Option<String>,
    /// Whether input values changed (map of ref -> (old, new)).
    pub input_changes: HashMap<String, (String, String)>,
    /// Whether the page appears to have error styling (red text, error class).
    pub has_error_state: bool,
    /// Whether snapshot is unchanged (same URL, same tree hash).
    pub is_stable: bool,
}

impl StateDiff {
    /// Compute a semantic diff between two PageState snapshots.
    pub fn compute(pre: &PageState, post: &PageState) -> Self {
        let url_changed = pre.url != post.url;
        let title_changed = pre.title != post.title;

        // Simple element count diff
        let elements_added = if post.interactive_count > pre.interactive_count {
            post.interactive_count - pre.interactive_count
        } else {
            0
        };
        let elements_removed = if pre.interactive_count > post.interactive_count {
            pre.interactive_count - post.interactive_count
        } else {
            0
        };

        // Detect added/removed refs by looking at tree text differences
        // (simplified — a full impl would parse and diff the tree)
        let added_refs: Vec<String> = Vec::new();
        let removed_refs: Vec<String> = Vec::new();
        let _ = (added_refs, removed_refs);

        let dialog_appeared = post.has_dialog && !pre.has_dialog;
        let input_changes: HashMap<String, (String, String)> = post
            .input_values
            .iter()
            .filter_map(|(k, v)| {
                pre.input_values.get(k).and_then(|old| {
                    if old != v {
                        Some((k.clone(), (old.clone(), v.clone())))
                    } else {
                        None
                    }
                })
            })
            .collect();

        // Check for error state in tree (heuristic)
        let has_error_state = post.tree.to_lowercase().contains("error")
            || post.tree.to_lowercase().contains("warning")
            || post.tree.to_lowercase().contains("failed");

        let is_stable = !url_changed
            && !title_changed
            && pre.interactive_count == post.interactive_count
            && pre.tree == post.tree;

        StateDiff {
            url_changed,
            new_url: if url_changed {
                Some(post.url.clone())
            } else {
                None
            },
            title_changed,
            new_title: if title_changed {
                Some(post.title.clone())
            } else {
                None
            },
            elements_added,
            elements_removed,
            added_refs,
            removed_refs,
            dialog_appeared,
            dialog_message: post.dialog_message.clone(),
            input_changes,
            has_error_state,
            is_stable,
        }
    }

    /// Returns true if the diff indicates the action was ineffective.
    pub fn is_ineffective(&self) -> bool {
        self.is_stable && !self.dialog_appeared && self.input_changes.is_empty()
    }
}

/// A self-correction hint generated when an action appears ineffective.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelfCorrection {
    /// What likely went wrong.
    pub diagnosis: String,
    /// What to try instead.
    pub suggestions: Vec<String>,
    /// Whether a modal/dialog is blocking further action.
    pub modal_blocking: bool,
    /// Whether the element might be inside an iframe.
    pub possibly_in_iframe: bool,
    /// Whether the element might require scrolling into view.
    pub possibly_occluded: bool,
    /// Whether the element might have dynamically loaded after page render.
    pub possibly_lazy_loaded: bool,
}

impl SelfCorrection {
    /// Generate a self-correction hint from a state diff.
    pub fn from_diff(diff: &StateDiff, action_name: &str) -> Option<Self> {
        if !diff.is_ineffective() {
            return None;
        }

        let mut suggestions = Vec::new();
        let mut modal_blocking = false;
        let possibly_in_iframe = diff.removed_refs.is_empty(); // heuristic
        let possibly_occluded = false; // Would need layout info
        let possibly_lazy_loaded = diff.elements_added > 0; // new elements appeared

        if diff.dialog_appeared {
            modal_blocking = true;
            suggestions.push(format!(
                "Close the dialog (\"{}\") before continuing",
                diff.dialog_message.as_deref().unwrap_or("unknown dialog")
            ));
        }

        // URL was same — action might not have targeted the right element
        if !diff.url_changed {
            suggestions.push(
                "The URL did not change — verify the selector targets the correct element"
                    .to_string(),
            );
            suggestions.push("Try using a more specific CSS selector or aria-label".to_string());
        }

        // Action-specific hints
        let action_lower = action_name.to_lowercase();
        if action_lower.contains("click") {
            suggestions.push(
                "The element may be obscured by an overlay or require scrolling into view"
                    .to_string(),
            );
            suggestions.push(
                "Try using browser_scroll first to bring the element into viewport".to_string(),
            );
            if possibly_lazy_loaded {
                suggestions.push(
                    "The target element may be lazy-loaded — try waiting for the network to settle"
                        .to_string(),
                );
            }
        } else if action_lower.contains("type") || action_lower.contains("fill") {
            suggestions
                .push("The input may be read-only, disabled, or inside a shadow DOM".to_string());
            suggestions.push(
                "Try using browser_evaluate to directly set the value via JavaScript".to_string(),
            );
        } else if action_lower.contains("navigate") || action_lower.contains("goto") {
            suggestions
                .push("Navigation did not occur — the URL may be invalid or blocked".to_string());
            suggestions.push(
                "Check for CSP (Content Security Policy) restrictions or network errors"
                    .to_string(),
            );
        }

        // Generic fallback
        if suggestions.is_empty() {
            suggestions
                .push("Take a new snapshot and verify the target element still exists".to_string());
            suggestions.push(
                "The page may have updated via JavaScript — try waiting before retrying"
                    .to_string(),
            );
        }

        Some(SelfCorrection {
            diagnosis: "Action completed but page state remained unchanged — the operation likely had no effect".to_string(),
            suggestions,
            modal_blocking,
            possibly_in_iframe,
            possibly_occluded,
            possibly_lazy_loaded,
        })
    }
}

// ---------------------------------------------------------------------------
// State tracker (managed per browser session)
// ---------------------------------------------------------------------------

/// Per-session action trace tracker.
/// Thread-safe, can be shared across async tasks.
pub struct ActionTrace {
    steps: Arc<RwLock<Vec<StepRecord>>>,
    step_counter: Arc<RwLock<usize>>,
}

impl Default for ActionTrace {
    fn default() -> Self {
        Self::new()
    }
}

impl ActionTrace {
    pub fn new() -> Self {
        Self {
            steps: Arc::new(RwLock::new(Vec::new())),
            step_counter: Arc::new(RwLock::new(0)),
        }
    }

    /// Record a new action with its pre and post states.
    pub async fn record(
        &self,
        action_name: String,
        action_params: Value,
        pre_state: PageState,
        post_state: PageState,
    ) -> usize {
        let step = {
            let mut counter = self.step_counter.write().await;
            *counter += 1;
            *counter
        };

        let diff = StateDiff::compute(&pre_state, &post_state);
        let effective = !diff.is_ineffective();
        let self_correction = SelfCorrection::from_diff(&diff, &action_name);
        let timestamp_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        let record = StepRecord {
            step,
            action_name,
            action_params,
            pre_state,
            post_state,
            effective,
            diff,
            self_correction,
            timestamp_ms,
        };

        let mut steps = self.steps.write().await;
        steps.push(record);
        step
    }

    /// Get the full action trace.
    pub async fn get_trace(&self) -> Vec<StepRecord> {
        self.steps.read().await.clone()
    }

    /// Get the last N steps.
    pub async fn get_last(&self, n: usize) -> Vec<StepRecord> {
        let steps = self.steps.read().await;
        steps[steps.len().saturating_sub(n)..].to_vec()
    }

    /// Get the most recent ineffective step's self-correction (if any).
    pub async fn get_last_ineffective_correction(&self) -> Option<SelfCorrection> {
        let steps = self.steps.read().await;
        steps.iter().rev().find_map(|s| s.self_correction.clone())
    }

    /// Clear the trace.
    pub async fn clear(&self) {
        let mut steps = self.steps.write().await;
        steps.clear();
        let mut counter = self.step_counter.write().await;
        *counter = 0;
    }

    /// Export the full trace as JSON for replay/debugging.
    pub async fn export_json(&self) -> Value {
        let steps = self.steps.read().await;
        json!({ "steps": steps.as_slice() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_state(url: &str, title: &str, interactive: usize) -> PageState {
        PageState {
            tree: format!("url={}", url),
            url: url.to_string(),
            title: title.to_string(),
            interactive_count: interactive,
            url_fragment: None,
            has_dialog: false,
            dialog_message: None,
            input_values: HashMap::new(),
            iframe_count: 0,
            timestamp_ms: 0,
        }
    }

    #[test]
    fn test_state_diff_url_changed() {
        let pre = make_state("https://example.com", "Example", 5);
        let post = make_state("https://example.com/page2", "Page 2", 5);
        let diff = StateDiff::compute(&pre, &post);
        assert!(diff.url_changed);
        assert!(diff.new_url.is_some());
        assert!(!diff.is_ineffective());
    }

    #[test]
    fn test_state_diff_stable() {
        let pre = make_state("https://example.com", "Example", 5);
        let post = make_state("https://example.com", "Example", 5);
        let diff = StateDiff::compute(&pre, &post);
        assert!(diff.is_stable);
        assert!(diff.is_ineffective());
    }

    #[test]
    fn test_self_correction_on_ineffective() {
        let pre = make_state("https://example.com", "Example", 5);
        let post = make_state("https://example.com", "Example", 5);
        let diff = StateDiff::compute(&pre, &post);
        let correction = SelfCorrection::from_diff(&diff, "click");
        assert!(correction.is_some());
        let c = correction.unwrap();
        assert!(!c.suggestions.is_empty());
    }

    #[test]
    fn test_self_correction_none_on_effective() {
        let pre = make_state("https://example.com", "Example", 5);
        let post = make_state("https://example.com/page2", "Page 2", 5);
        let diff = StateDiff::compute(&pre, &post);
        let correction = SelfCorrection::from_diff(&diff, "click");
        assert!(correction.is_none());
    }
}
