//! Agent-native semantic DOM snapshot.
//!
//! Enhances the base accessibility tree with:
//! - **Semantic priority scoring** — ranks elements by actionability for LLMs
//! - **Ad/decoration filtering** — removes noise from ad iframes and decorative nodes
//! - **Noise level indicator** — tells the agent how "clean" the snapshot is
//! - **Action hints** — inline suggestions for high-value interactions

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;

use super::cdp::client::CdpClient;
use super::element::RefMap;
use super::snapshot::{self, SnapshotOptions};

/// Extra options for agent-native snapshots.
#[derive(Debug, Clone, Default)]
pub struct AgentSnapshotOptions {
    /// Base snapshot options.
    pub base: SnapshotOptions,
    /// Include priority scores on each interactive element.
    pub priority_scores: bool,
    /// Include noise level assessment.
    pub noise_assessment: bool,
    /// Maximum priority elements to surface (0 = unlimited).
    pub max_priority_elements: usize,
}

/// How "clean" a page snapshot is from the agent's perspective.
#[derive(Debug, Clone, Serialize)]
pub struct NoiseAssessment {
    /// Overall noise grade: A (pristine), B (normal), C (noisy), D (cluttered).
    pub grade: char,
    /// Estimated percentage of non-actionable nodes.
    pub noise_percentage: f64,
    /// Categories of noise detected.
    pub noise_sources: Vec<String>,
    /// Whether the page appears to be primarily an ad/overlay.
    pub is_overlay_heavy: bool,
}

/// A single interactive element annotated with LLM-friendly metadata.
#[derive(Debug, Clone, Serialize)]
pub struct PrioritizedElement {
    /// Element reference ID (e.g. "e1").
    pub ref_id: String,
    /// ARIA role.
    pub role: String,
    /// Accessible name.
    pub name: String,
    /// Priority score (0-100, higher = more actionable).
    pub priority: u8,
    /// Why this element scored high (for LLM reasoning).
    pub reasoning: String,
    /// Whether the element is visually prominent.
    pub is_prominent: bool,
    /// Element's bounding box if available.
    pub bounds: Option<ElementBounds>,
}

/// Bounding box of an element on screen.
#[derive(Debug, Clone, Serialize)]
pub struct ElementBounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// Agent-native snapshot output.
#[derive(Debug, Clone, Serialize)]
pub struct AgentSnapshotOutput {
    /// Plain-text accessibility tree (same format as base snapshot).
    pub tree: String,
    /// Priority-sorted list of interactive elements most useful for the agent.
    pub prioritized_elements: Vec<PrioritizedElement>,
    /// Overall noise assessment.
    pub noise: NoiseAssessment,
    /// Count of total interactive elements in tree.
    pub total_interactive: usize,
    /// Count of filtered noise nodes.
    pub filtered_noise: usize,
}

// ---------------------------------------------------------------------------
// Ad / decoration filter
// ---------------------------------------------------------------------------

/// Known ad, tracking, and decoration selectors.
/// These are filtered from the priority ranking and flagged in noise assessment.
const AD_SELECTORS: &[&str] = &[
    "[class*='ad-']",
    "[class*='ads-']",
    "[class*='advert']",
    "[id*='ad-']",
    "[id*='ads-']",
    "[id*='advert']",
    "[data-ad]",
    "[data-ads]",
    "iframe[src*='doubleclick']",
    "iframe[src*='googlesyndication']",
    "iframe[src*='amazon-adsystem']",
    "iframe[src*='facebook']",
    "[class*='cookie-banner']",
    "[class*='cookie-notice']",
    "[id*='cookie']",
    "[class*='newsletter-popup']",
    "[class*='newsletter-modal']",
    "[class*='social-share']",
    "[class*='share-buttons']",
];

/// Roles that are almost always decoration or noise.
const NOISE_ROLES: &[&str] = &[
    "InlineTextBox",
    "none",
    "presentation",
    "caption",
    "description",
    "listMarker",
    "note",
];

/// Content/structure roles that are informative but not directly interactive.
const CONTENT_ROLES: &[&str] = &[
    "heading",
    "cell",
    "gridcell",
    "columnheader",
    "rowheader",
    "listitem",
    "article",
    "section",
    "region",
    "main",
    "navigation",
    "complementary",
    "banner",
    "contentinfo",
    "definition",
];

/// Interactive roles (high-value for agents).
const INTERACTIVE_ROLES: &[&str] = &[
    "button",
    "link",
    "textbox",
    "searchbox",
    "checkbox",
    "radio",
    "combobox",
    "listbox",
    "menuitem",
    "menuitemcheckbox",
    "menuitemradio",
    "option",
    "slider",
    "spinbutton",
    "switch",
    "tab",
    "treeitem",
    "menu",
    "menubar",
    "toolbar",
];

/// Estimate noise grade from page characteristics.
fn assess_noise(tree_text: &str, total_refs: usize, content_roles_seen: usize) -> NoiseAssessment {
    let mut noise_sources = Vec::new();
    let mut score: f64 = 0.0;

    // High ad-selector density in tree text suggests ad-heavy page
    let ad_mentions: usize = AD_SELECTORS
        .iter()
        .filter(|s| tree_text.contains(&s.replace('\'', "")))
        .count();
    if ad_mentions > 5 {
        noise_sources.push(format!("{} ad-related selectors detected", ad_mentions));
        score += 30.0;
    } else if ad_mentions > 0 {
        noise_sources.push(format!("{} ad-related selectors detected", ad_mentions));
        score += 10.0;
    }

    // Very long tree with few refs suggests DOM-heavy page
    let tree_lines = tree_text.lines().count();
    if total_refs == 0 && tree_lines > 500 {
        noise_sources.push("DOM-heavy page with no interactive elements".to_string());
        score += 25.0;
    } else if total_refs > 0 {
        let ratio = tree_lines as f64 / total_refs as f64;
        if ratio > 50.0 {
            noise_sources.push(format!(
                "Low signal ratio ({:.0}x lines/refs, expected <50x)",
                ratio
            ));
            score += 15.0;
        }
    }

    // Overlay-heavy: many fixed/absolute positioned elements
    // (detected via tree text patterns — heuristic)
    let overlay_keywords = ["modal", "overlay", "popup", "drawer", "sidebar"];
    let overlay_hits: usize = overlay_keywords
        .iter()
        .map(|kw| tree_text.to_lowercase().matches(kw).count())
        .sum();
    let is_overlay_heavy = overlay_hits > 10;
    if is_overlay_heavy {
        noise_sources.push(format!("{} overlay/modal keywords detected", overlay_hits));
        score += 20.0;
    }

    // Content roles are informative but not noise
    let content_ratio = if total_refs > 0 {
        content_roles_seen as f64 / total_refs as f64
    } else {
        0.0
    };
    if content_ratio > 0.8 {
        noise_sources.push("High content/structure density".to_string());
        score += 5.0; // Informative, not really noise
    }

    let noise_percentage = score.min(100.0);
    let grade = if noise_percentage < 10.0 {
        'A'
    } else if noise_percentage < 30.0 {
        'B'
    } else if noise_percentage < 60.0 {
        'C'
    } else {
        'D'
    };

    NoiseAssessment {
        grade,
        noise_percentage,
        noise_sources,
        is_overlay_heavy,
    }
}

// ---------------------------------------------------------------------------
// Priority scoring
// ---------------------------------------------------------------------------

/// Compute a priority score (0-100) for an element based on its properties.
fn compute_priority(role: &str, name: &str, has_ref: bool, depth: usize) -> u8 {
    let mut score: u16 = 50; // baseline

    // Interactive roles are highest value
    if INTERACTIVE_ROLES.contains(&role) {
        score += 30;
    }

    // Named elements are more valuable than anonymous ones
    if !name.is_empty() && name.len() <= 80 {
        score += 10;
    } else if name.len() > 80 {
        score -= 5; // Truncated or auto-generated name
    }

    // Elements with refs are by definition in the ref_map (interactive)
    if has_ref {
        score += 10;
    }

    // Shallow depth = more prominent (closer to root)
    if depth < 3 {
        score += 5;
    } else if depth > 8 {
        score = score.saturating_sub(10);
    }

    // Buttons and links with action-oriented names score higher
    let action_keywords = [
        "submit", "save", "delete", "cancel", "confirm", "next", "prev", "search", "login",
        "signin", "signup", "register", "download", "upload", "close", "open", "send", "get",
        "start", "stop",
    ];
    let name_lower = name.to_lowercase();
    for kw in &action_keywords {
        if name_lower.contains(kw) {
            score += 5;
            break;
        }
    }

    // Dangerous actions get a small bonus — agent should notice them
    let danger_keywords = [
        "delete",
        "remove",
        "destroy",
        "drop",
        "reset",
        "unsubscribe",
    ];
    for kw in &danger_keywords {
        if name_lower.contains(kw) {
            score += 3;
            break;
        }
    }

    score.min(100) as u8
}

/// Reason why an element got its priority score (for LLM reasoning).
fn priority_reasoning(role: &str, name: &str, priority: u8) -> String {
    if INTERACTIVE_ROLES.contains(&role) {
        if !name.is_empty() {
            format!(
                "role={} with name \"{}\" — direct action target",
                role, name
            )
        } else {
            format!("role={} — direct action target", role)
        }
    } else if CONTENT_ROLES.contains(&role) {
        format!(
            "role={} — informational context (priority={})",
            role, priority
        )
    } else {
        format!("role={} with name \"{}\" — peripheral element", role, name)
    }
}

// ---------------------------------------------------------------------------
// Main entry point
// ---------------------------------------------------------------------------

/// Build an agent-native snapshot from the base accessibility tree text.
pub fn build_agent_snapshot(
    tree: &str,
    ref_map: &RefMap,
    options: &AgentSnapshotOptions,
) -> AgentSnapshotOutput {
    let mut prioritized = Vec::new();
    let mut content_roles_seen = 0;
    let mut noise_count = 0;

    let entries: Vec<_> = ref_map.entries_sorted();

    for (ref_id, entry) in &entries {
        let role = &entry.role;
        let name = &entry.name;

        // Skip pure noise roles
        if NOISE_ROLES.contains(&role) {
            noise_count += 1;
            continue;
        }

        // Track content roles for noise assessment
        if CONTENT_ROLES.contains(&role) {
            content_roles_seen += 1;
        }

        // Quick filter for ad selectors in element name (heuristic)
        let name_lower = name.to_lowercase();
        let is_ad_name = name_lower.starts_with("ad")
            || name_lower.contains("sponsored")
            || name_lower.contains("advertisement");
        if is_ad_name {
            noise_count += 1;
            continue;
        }

        // Placeholder/nseudo-element names are noise
        if name.is_empty()
            || name == " "
            || name.chars().all(|c| c == '\u{00A0}' || c.is_whitespace())
        {
            // Only skip if it's not an interactive role
            if !INTERACTIVE_ROLES.contains(&role) {
                noise_count += 1;
                continue;
            }
        }

        // Estimate depth from tree structure (heuristic: count leading spaces)
        // Since we don't have depth per entry, use fixed scoring
        let depth = 5; // neutral depth estimate

        let priority = compute_priority(role, name, true, depth);
        let reasoning = priority_reasoning(role, name, priority);
        let is_prominent = INTERACTIVE_ROLES.contains(&role) && !name.is_empty();

        prioritized.push(PrioritizedElement {
            ref_id: ref_id.clone(),
            role: role.clone(),
            name: name.clone(),
            priority,
            reasoning,
            is_prominent,
            bounds: None, // Bounds require extra CDP calls; add via options flag
        });
    }

    // Sort by priority descending
    prioritized.sort_by(|a, b| b.priority.cmp(&a.priority));

    // Apply max cap
    if options.max_priority_elements > 0 && prioritized.len() > options.max_priority_elements {
        prioritized.truncate(options.max_priority_elements);
    }

    let total_interactive = entries.len();
    let noise = assess_noise(tree, total_interactive, content_roles_seen);

    AgentSnapshotOutput {
        tree: tree.to_string(),
        prioritized_elements: prioritized,
        noise,
        total_interactive,
        filtered_noise: noise_count,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_priority_interactive_button_high() {
        let p = compute_priority("button", "Submit Form", true, 2);
        assert!(
            p >= 80,
            "button with action name should score high, got {}",
            p
        );
    }

    #[test]
    fn test_priority_empty_name_low() {
        let p = compute_priority("link", "", true, 8);
        assert!(
            p < 80,
            "deep link with no name should score lower, got {}",
            p
        );
    }

    #[test]
    fn test_noise_assessment_clean() {
        let tree = "navigation\n  link \"Home\" [ref=e1]\n  button \"Submit\" [ref=e2]";
        let result = assess_noise(tree, 2, 0);
        assert!(result.grade == 'A' || result.grade == 'B');
    }

    #[test]
    fn test_noise_assessment_ad_heavy() {
        let mut tree =
            String::from("navigation\n  link \"Home\" [ref=e1]\n  iframe[src*='doubleclick']\n");
        for _ in 0..10 {
            tree.push_str("  generic [class*='ad-']\n");
        }
        let result = assess_noise(&tree, 1, 0);
        assert!(result.grade == 'C' || result.grade == 'D');
        assert!(result.is_overlay_heavy == false); // no overlay keywords
    }

    #[test]
    fn test_noise_assessment_modal_heavy() {
        let mut tree = String::from("navigation\n");
        for _ in 0..15 {
            tree.push_str("  generic modal popup overlay\n");
        }
        let result = assess_noise(&tree, 0, 0);
        assert!(result.is_overlay_heavy);
    }
}
