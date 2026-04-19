use serde::Serialize;
use serde_json::Value;
use std::path::PathBuf;

use std::collections::HashMap;

use super::cdp::client::CdpClient;
use super::cdp::types::*;
use super::element::RefMap;

const ANNOTATION_OVERLAY_ID: &str = "__agent_browser_annotations__";

#[derive(Debug, Clone)]
struct Rect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

#[derive(Debug, Clone)]
struct RawAnnotation {
    ref_id: String,
    number: u64,
    role: String,
    name: Option<String>,
    rect: Rect,
}

#[derive(Debug, Clone, Serialize)]
pub struct AnnotationBox {
    pub x: i64,
    pub y: i64,
    pub width: i64,
    pub height: i64,
}

#[derive(Debug, Clone)]
pub struct ScreenshotAnnotation {
    pub ref_id: String,
    pub number: u64,
    pub role: String,
    pub name: Option<String>,
    pub box_: AnnotationBox,
}

/// A cropped screenshot around a specific element.
#[derive(Debug, Clone, Serialize)]
pub struct ElementCropCapture {
    /// The ref ID of the element this crop is centered on.
    pub ref_id: String,
    /// The role of the element.
    pub role: String,
    /// The name of the element.
    pub name: Option<String>,
    /// Base64-encoded cropped PNG image.
    pub base64: String,
    /// Original element bounding box (x, y, width, height).
    pub original_bounds: (f64, f64, f64, f64),
    /// Captured region bounds.
    pub crop_bounds: (i64, i64, u32, u32),
}

#[derive(Debug, Clone)]
pub struct ScreenshotResult {
    pub path: String,
    pub base64: String,
    pub annotations: Vec<ScreenshotAnnotation>,
    /// Per-element cropped screenshots for visual confirmation.
    /// Only populated when `element_crop` is set in options.
    pub element_crops: Vec<ElementCropCapture>,
}

#[derive(Debug, Clone)]
pub struct ScreenshotOptions {
    pub selector: Option<String>,
    pub path: Option<String>,
    pub full_page: bool,
    pub format: String,
    pub quality: Option<i32>,
    pub annotate: bool,
    pub output_dir: Option<String>,
    /// Capture a cropped region around each interactive element for visual confirmation.
    /// When set to Some((width, height)), each element's bounding box will be expanded
    /// by (width/2, height/2) and captured as a small visual thumbnail.
    pub element_crop: Option<(u32, u32)>,
}

impl Default for ScreenshotOptions {
    fn default() -> Self {
        Self {
            selector: None,
            path: None,
            full_page: false,
            format: "png".to_string(),
            quality: None,
            annotate: false,
            output_dir: None,
            element_crop: None,
        }
    }
}

impl Serialize for ScreenshotAnnotation {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;

        let mut state = serializer.serialize_struct("ScreenshotAnnotation", 5)?;
        state.serialize_field("ref", &self.ref_id)?;
        state.serialize_field("number", &self.number)?;
        state.serialize_field("role", &self.role)?;
        if let Some(name) = &self.name {
            state.serialize_field("name", name)?;
        }
        state.serialize_field("box", &self.box_)?;
        state.end()
    }
}

/// Captures a screenshot via CDP and optionally overlays numbered annotations
/// that mirror the Node.js screenshot `annotate` mode.
pub async fn take_screenshot(
    client: &CdpClient,
    session_id: &str,
    ref_map: &RefMap,
    options: &ScreenshotOptions,
    iframe_sessions: &HashMap<String, String>,
) -> Result<ScreenshotResult, String> {
    let target_rect = if options.annotate {
        match options.selector.as_deref() {
            Some(selector) => {
                get_rect_for_selector(client, session_id, ref_map, selector, iframe_sessions)
                    .await?
            }
            None => None,
        }
    } else {
        None
    };

    let raw_annotations = if options.annotate {
        collect_annotations(client, session_id, ref_map).await?
    } else {
        Vec::new()
    };

    let overlay_items = filter_annotations(raw_annotations, target_rect.as_ref());
    let overlay_injected = if options.annotate && !overlay_items.is_empty() {
        inject_annotation_overlay(client, session_id, &overlay_items).await?;
        true
    } else {
        false
    };

    let base64 =
        capture_screenshot_base64(client, session_id, ref_map, options, iframe_sessions).await;

    if overlay_injected {
        let _ = remove_annotation_overlay(client, session_id).await;
    }

    let base64 = base64?;
    let annotations = if options.annotate {
        let scroll = if options.full_page {
            Some(get_scroll_offsets(client, session_id).await?)
        } else {
            None
        };
        project_annotations(&overlay_items, target_rect.as_ref(), scroll)
    } else {
        Vec::new()
    };

    let ext = if options.format == "jpeg" {
        "jpg"
    } else {
        "png"
    };
    let path = save_screenshot(
        &base64,
        options.path.as_deref(),
        ext,
        options.output_dir.as_deref(),
    )?;

    // Capture per-element crops for visual anchoring when element_crop is set.
    let element_crops = if let Some(crop_size) = options.element_crop {
        capture_element_crops(client, session_id, ref_map, crop_size, 20)
            .await
            .unwrap_or_default()
    } else {
        Vec::new()
    };

    Ok(ScreenshotResult {
        path,
        base64,
        annotations,
        element_crops,
    })
}

async fn capture_screenshot_base64(
    client: &CdpClient,
    session_id: &str,
    ref_map: &RefMap,
    options: &ScreenshotOptions,
    iframe_sessions: &HashMap<String, String>,
) -> Result<String, String> {
    let mut params = CaptureScreenshotParams {
        format: Some(options.format.clone()),
        quality: if options.format == "jpeg" {
            options.quality.or(Some(80))
        } else {
            None
        },
        clip: None,
        from_surface: Some(true),
        capture_beyond_viewport: if options.full_page { Some(true) } else { None },
    };

    if options.full_page {
        let metrics: Value = client
            .send_command_no_params("Page.getLayoutMetrics", Some(session_id))
            .await?;

        let content_size = metrics
            .get("contentSize")
            .or_else(|| metrics.get("cssContentSize"));
        if let Some(size) = content_size {
            let width = size.get("width").and_then(|v| v.as_f64()).unwrap_or(1280.0);
            let height = size.get("height").and_then(|v| v.as_f64()).unwrap_or(720.0);

            params.clip = Some(Viewport {
                x: 0.0,
                y: 0.0,
                width,
                height,
                scale: 1.0,
            });
        }
    } else if let Some(ref selector) = options.selector {
        if let Some(rect) =
            get_rect_for_selector(client, session_id, ref_map, selector, iframe_sessions).await?
        {
            params.clip = Some(Viewport {
                x: rect.x,
                y: rect.y,
                width: rect.width,
                height: rect.height,
                scale: 1.0,
            });
        }
    }

    let result: CaptureScreenshotResult = client
        .send_command_typed("Page.captureScreenshot", &params, Some(session_id))
        .await?;

    Ok(result.data)
}

async fn collect_annotations(
    client: &CdpClient,
    session_id: &str,
    ref_map: &RefMap,
) -> Result<Vec<RawAnnotation>, String> {
    let entries = ref_map.entries_sorted();
    if entries.is_empty() {
        return Ok(Vec::new());
    }

    // Collect entries that have backend_node_ids for batch resolution.
    let with_backend_ids: Vec<(String, super::element::RefEntry, i64)> = entries
        .iter()
        .filter_map(|(ref_id, entry)| {
            entry
                .backend_node_id
                .map(|bid| (ref_id.clone(), entry.clone(), bid))
        })
        .collect();

    if with_backend_ids.is_empty() {
        return Ok(Vec::new());
    }

    // Batch-resolve all backend_node_ids to object IDs using concurrent CDP calls.
    let resolve_futures: Vec<_> = with_backend_ids
        .iter()
        .map(|(_, _, backend_node_id)| {
            client.send_command(
                "DOM.resolveNode",
                Some(serde_json::json!({
                    "backendNodeId": backend_node_id,
                    "objectGroup": "agent-browser-annotate"
                })),
                Some(session_id),
            )
        })
        .collect();

    let resolve_results = futures_util::future::join_all(resolve_futures).await;

    // Collect resolved object IDs paired with their ref info.
    let mut resolved: Vec<(String, super::element::RefEntry, String)> = Vec::new();
    for (i, result) in resolve_results.into_iter().enumerate() {
        if let Ok(val) = result {
            if let Some(oid) = val
                .get("object")
                .and_then(|o| o.get("objectId"))
                .and_then(|v| v.as_str())
            {
                let (ref_id, entry, _) = &with_backend_ids[i];
                resolved.push((ref_id.clone(), entry.clone(), oid.to_string()));
            }
        }
    }

    if resolved.is_empty() {
        return Ok(Vec::new());
    }

    // Batch-get bounding rects for all resolved elements using concurrent CDP calls.
    let rect_futures: Vec<_> = resolved
        .iter()
        .map(|(_, _, object_id)| get_rect_for_object(client, session_id, object_id))
        .collect();

    let rect_results = futures_util::future::join_all(rect_futures).await;

    let mut annotations = Vec::new();
    for (i, rect_result) in rect_results.into_iter().enumerate() {
        let rect = match rect_result {
            Ok(Some(r)) if r.width > 0.0 && r.height > 0.0 => r,
            _ => continue,
        };

        let (ref_id, entry, _) = &resolved[i];
        let number = ref_id
            .strip_prefix('e')
            .and_then(|n| n.parse::<u64>().ok())
            .unwrap_or(0);

        annotations.push(RawAnnotation {
            ref_id: ref_id.clone(),
            number,
            role: entry.role.clone(),
            name: (!entry.name.is_empty()).then_some(entry.name.clone()),
            rect,
        });
    }

    Ok(annotations)
}

async fn get_rect_for_selector(
    client: &CdpClient,
    session_id: &str,
    ref_map: &RefMap,
    selector: &str,
    iframe_sessions: &HashMap<String, String>,
) -> Result<Option<Rect>, String> {
    let (object_id, effective_session_id) = super::element::resolve_element_object_id(
        client,
        session_id,
        ref_map,
        selector,
        iframe_sessions,
    )
    .await?;
    get_rect_for_object(client, &effective_session_id, &object_id).await
}

async fn get_rect_for_object(
    client: &CdpClient,
    session_id: &str,
    object_id: &str,
) -> Result<Option<Rect>, String> {
    let result: EvaluateResult = client
        .send_command_typed(
            "Runtime.callFunctionOn",
            &CallFunctionOnParams {
                function_declaration: r#"function() {
                    const rect = this.getBoundingClientRect();
                    return { x: rect.x, y: rect.y, width: rect.width, height: rect.height };
                }"#
                .to_string(),
                object_id: Some(object_id.to_string()),
                arguments: None,
                return_by_value: Some(true),
                await_promise: Some(false),
            },
            Some(session_id),
        )
        .await?;

    Ok(result.result.value.as_ref().and_then(parse_rect))
}

fn parse_rect(value: &Value) -> Option<Rect> {
    Some(Rect {
        x: value.get("x")?.as_f64()?,
        y: value.get("y")?.as_f64()?,
        width: value.get("width")?.as_f64()?,
        height: value.get("height")?.as_f64()?,
    })
}

fn filter_annotations(
    annotations: Vec<RawAnnotation>,
    target_rect: Option<&Rect>,
) -> Vec<RawAnnotation> {
    let mut items = annotations
        .into_iter()
        .filter(|annotation| match target_rect {
            Some(target) => overlaps(&annotation.rect, target),
            None => true,
        })
        .collect::<Vec<_>>();

    items.sort_by_key(|annotation| annotation.number);
    items
}

fn overlaps(left: &Rect, right: &Rect) -> bool {
    let left_x2 = left.x + left.width;
    let left_y2 = left.y + left.height;
    let right_x2 = right.x + right.width;
    let right_y2 = right.y + right.height;

    left.x < right_x2 && left_x2 > right.x && left.y < right_y2 && left_y2 > right.y
}

async fn inject_annotation_overlay(
    client: &CdpClient,
    session_id: &str,
    annotations: &[RawAnnotation],
) -> Result<(), String> {
    let overlay_data = annotations
        .iter()
        .map(|annotation| {
            serde_json::json!({
                "number": annotation.number,
                "x": round(annotation.rect.x),
                "y": round(annotation.rect.y),
                "width": round(annotation.rect.width),
                "height": round(annotation.rect.height),
            })
        })
        .collect::<Vec<_>>();

    let expression = format!(
        r#"(() => {{
            var items = {items};
            var id = {overlay_id};
            var existing = document.getElementById(id);
            if (existing) existing.remove();
            var sx = window.scrollX || 0;
            var sy = window.scrollY || 0;
            var c = document.createElement('div');
            c.id = id;
            c.style.cssText = 'position:absolute;top:0;left:0;width:0;height:0;pointer-events:none;z-index:2147483647;';
            for (var i = 0; i < items.length; i++) {{
                var it = items[i];
                var dx = it.x + sx;
                var dy = it.y + sy;
                var b = document.createElement('div');
                b.style.cssText = 'position:absolute;left:' + dx + 'px;top:' + dy + 'px;width:' + it.width + 'px;height:' + it.height + 'px;border:2px solid rgba(255,0,0,0.8);box-sizing:border-box;pointer-events:none;';
                var l = document.createElement('div');
                l.textContent = String(it.number);
                var labelTop = dy < 14 ? '2px' : '-14px';
                l.style.cssText = 'position:absolute;top:' + labelTop + ';left:-2px;background:rgba(255,0,0,0.9);color:#fff;font:bold 11px/14px monospace;padding:0 4px;border-radius:2px;white-space:nowrap;';
                b.appendChild(l);
                c.appendChild(b);
            }}
            document.documentElement.appendChild(c);
            return true;
        }})()"#,
        items = serde_json::to_string(&overlay_data).unwrap_or_else(|_| "[]".to_string()),
        overlay_id =
            serde_json::to_string(ANNOTATION_OVERLAY_ID).unwrap_or_else(|_| "\"\"".to_string()),
    );

    let _: EvaluateResult = client
        .send_command_typed(
            "Runtime.evaluate",
            &EvaluateParams {
                expression,
                return_by_value: Some(true),
                await_promise: Some(false),
            },
            Some(session_id),
        )
        .await?;

    Ok(())
}

async fn remove_annotation_overlay(client: &CdpClient, session_id: &str) -> Result<(), String> {
    let expression = format!(
        r#"(() => {{
            var el = document.getElementById({overlay_id});
            if (el) el.remove();
            return true;
        }})()"#,
        overlay_id =
            serde_json::to_string(ANNOTATION_OVERLAY_ID).unwrap_or_else(|_| "\"\"".to_string()),
    );

    let _: EvaluateResult = client
        .send_command_typed(
            "Runtime.evaluate",
            &EvaluateParams {
                expression,
                return_by_value: Some(true),
                await_promise: Some(false),
            },
            Some(session_id),
        )
        .await?;

    Ok(())
}

async fn get_scroll_offsets(client: &CdpClient, session_id: &str) -> Result<(f64, f64), String> {
    let result: EvaluateResult = client
        .send_command_typed(
            "Runtime.evaluate",
            &EvaluateParams {
                expression: "({x: window.scrollX || 0, y: window.scrollY || 0})".to_string(),
                return_by_value: Some(true),
                await_promise: Some(false),
            },
            Some(session_id),
        )
        .await?;

    let value = result.result.value.unwrap_or(Value::Null);
    let x = value.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let y = value.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
    Ok((x, y))
}

fn project_annotations(
    annotations: &[RawAnnotation],
    target_rect: Option<&Rect>,
    scroll: Option<(f64, f64)>,
) -> Vec<ScreenshotAnnotation> {
    annotations
        .iter()
        .map(|annotation| {
            let rect = if let Some(target) = target_rect {
                Rect {
                    x: annotation.rect.x - target.x,
                    y: annotation.rect.y - target.y,
                    width: annotation.rect.width,
                    height: annotation.rect.height,
                }
            } else if let Some((scroll_x, scroll_y)) = scroll {
                Rect {
                    x: annotation.rect.x + scroll_x,
                    y: annotation.rect.y + scroll_y,
                    width: annotation.rect.width,
                    height: annotation.rect.height,
                }
            } else {
                annotation.rect.clone()
            };

            ScreenshotAnnotation {
                ref_id: annotation.ref_id.clone(),
                number: annotation.number,
                role: annotation.role.clone(),
                name: annotation.name.clone(),
                box_: AnnotationBox {
                    x: round(rect.x),
                    y: round(rect.y),
                    width: round(rect.width),
                    height: round(rect.height),
                },
            }
        })
        .collect()
}

fn save_screenshot(
    base64_data: &str,
    explicit_path: Option<&str>,
    ext: &str,
    output_dir: Option<&str>,
) -> Result<String, String> {
    let save_path = match explicit_path {
        Some(path) => path.to_string(),
        None => {
            let dir = match output_dir {
                Some(d) => PathBuf::from(d),
                None => get_screenshot_dir(),
            };
            let _ = std::fs::create_dir_all(&dir);
            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis();
            let name = format!("screenshot-{}.{}", timestamp, ext);
            dir.join(name).to_string_lossy().to_string()
        }
    };

    let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, base64_data)
        .map_err(|e| format!("Failed to decode screenshot: {}", e))?;

    std::fs::write(&save_path, &bytes)
        .map_err(|e| format!("Failed to save screenshot to {}: {}", save_path, e))?;

    Ok(save_path)
}

fn round(value: f64) -> i64 {
    value.round() as i64
}

fn get_screenshot_dir() -> PathBuf {
    if let Some(home) = dirs::home_dir() {
        home.join(".agent-browser").join("tmp").join("screenshots")
    } else {
        std::env::temp_dir()
            .join("agent-browser")
            .join("screenshots")
    }
}

/// Capture cropped screenshots around each interactive element.
///
/// This is used for **visual anchoring** — the LLM can see a small thumbnail
/// of each element to confirm it is clicking the right thing.
///
/// `crop_size` is (width, height) in pixels for each crop.
/// Each element's bounding box is expanded by (width/2, height/2) on each side.
pub async fn capture_element_crops(
    client: &CdpClient,
    session_id: &str,
    ref_map: &RefMap,
    crop_size: (u32, u32),
    max_crops: usize,
) -> Result<Vec<ElementCropCapture>, String> {
    use super::cdp::types::{CaptureScreenshotParams, EvaluateParams, EvaluateResult};

    let entries: Vec<_> = ref_map.entries_sorted();
    let interactive: Vec<_> = entries
        .iter()
        .filter(|(_, e)| {
            matches!(
                e.role.as_str(),
                "button" | "link" | "textbox" | "checkbox" | "radio"
                    | "combobox" | "menuitem" | "tab" | "option"
            )
        })
        .take(max_crops)
        .collect();

    let mut crops = Vec::new();

    for (ref_id, entry) in interactive {
        // Get element bounding box via JS
        let js = format!(
            r#"(function() {{
                var els = document.querySelectorAll('[data-ref-id="{}"]');
                if (els.length === 0) {{
                    // Try by aria-label or title
                    els = document.querySelectorAll('[aria-label="{}"], [title="{}"]');
                }}
                for (var i = 0; i < els.length; i++) {{
                    var r = els[i].getBoundingClientRect();
                    if (r.width > 0 && r.height > 0) {{
                        return {{ x: r.left, y: r.top, width: r.width, height: r.height, found: true }};
                    }}
                }}
                return {{ found: false }};
            }})()"#,
            ref_id,
            entry.name.replace('"', "'"),
            entry.name.replace('"', "'")
        );

        let result: EvaluateResult = client
            .send_command_typed(
                "Runtime.evaluate",
                &EvaluateParams {
                    expression: js,
                    return_by_value: Some(true),
                    await_promise: Some(false),
                },
                Some(session_id),
            )
            .await
            .map_err(|e| e.to_string())?;

        let bounds: serde_json::Value = result
            .result
            .value
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default();

        if !bounds.get("found").and_then(|v| v.as_bool()).unwrap_or(false) {
            continue;
        }

        let x = bounds.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let y = bounds.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let w = bounds.get("width").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let h = bounds.get("height").and_then(|v| v.as_f64()).unwrap_or(0.0);

        if w <= 0.0 || h <= 0.0 {
            continue;
        }

        // Expand bounding box by half the crop size on each side
        let half_w = crop_size.0 as f64 / 2.0;
        let half_h = crop_size.1 as f64 / 2.0;

        let crop_x = (x - half_w).max(0.0) as i64;
        let crop_y = (y - half_h).max(0.0) as i64;
        let crop_w = (w + crop_size.0 as f64).min(4096.0) as u32;
        let crop_h = (h + crop_size.1 as f64).min(4096.0) as u32;

        // Capture the cropped screenshot using CDP's captureScreenshot with clip
        let params = CaptureScreenshotParams {
            format: Some("png".to_string()),
            quality: None,
            clip: Some(super::cdp::types::Viewport {
                x: crop_x as f64,
                y: crop_y as f64,
                width: crop_w as f64,
                height: crop_h as f64,
                scale: 2.0, // 2x for retina quality
            }),
            from_surface: Some(true),
            capture_beyond_viewport: Some(false),
        };

        let cap: super::cdp::types::CaptureScreenshotResult = client
            .send_command_typed("Page.captureScreenshot", &params, Some(session_id))
            .await
            .map_err(|e| e.to_string())?;

        let b64 = cap
            .result
            .data
            .ok_or("captureScreenshot returned no data")?;

        crops.push(ElementCropCapture {
            ref_id: ref_id.clone(),
            role: entry.role.clone(),
            name: if entry.name.is_empty() {
                None
            } else {
                Some(entry.name.clone())
            },
            base64: b64,
            original_bounds: (x, y, w, h),
            crop_bounds: (crop_x, crop_y, crop_w, crop_h),
        });
    }

    Ok(crops)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_annotations_to_target_overlap() {
        let annotations = vec![
            RawAnnotation {
                ref_id: "e1".to_string(),
                number: 1,
                role: "button".to_string(),
                name: Some("Inside".to_string()),
                rect: Rect {
                    x: 10.0,
                    y: 10.0,
                    width: 50.0,
                    height: 20.0,
                },
            },
            RawAnnotation {
                ref_id: "e2".to_string(),
                number: 2,
                role: "button".to_string(),
                name: Some("Outside".to_string()),
                rect: Rect {
                    x: 200.0,
                    y: 200.0,
                    width: 40.0,
                    height: 20.0,
                },
            },
        ];

        let target = Rect {
            x: 0.0,
            y: 0.0,
            width: 100.0,
            height: 100.0,
        };

        let filtered = filter_annotations(annotations, Some(&target));
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].ref_id, "e1");
    }

    #[test]
    fn projects_selector_annotations_relative_to_target() {
        let annotations = vec![RawAnnotation {
            ref_id: "e1".to_string(),
            number: 1,
            role: "button".to_string(),
            name: Some("Inside".to_string()),
            rect: Rect {
                x: 25.0,
                y: 35.0,
                width: 40.0,
                height: 20.0,
            },
        }];

        let target = Rect {
            x: 10.0,
            y: 15.0,
            width: 100.0,
            height: 100.0,
        };

        let projected = project_annotations(&annotations, Some(&target), None);
        assert_eq!(projected[0].box_.x, 15);
        assert_eq!(projected[0].box_.y, 20);
    }

    #[test]
    fn projects_full_page_annotations_to_document_space() {
        let annotations = vec![RawAnnotation {
            ref_id: "e1".to_string(),
            number: 1,
            role: "button".to_string(),
            name: Some("Bottom".to_string()),
            rect: Rect {
                x: 5.0,
                y: 12.0,
                width: 40.0,
                height: 20.0,
            },
        }];

        let projected = project_annotations(&annotations, None, Some((10.0, 1000.0)));
        assert_eq!(projected[0].box_.x, 15);
        assert_eq!(projected[0].box_.y, 1012);
    }
}
