use crate::atspi_tree::{
    accessibility_chrome_context, list_accessible_apps, object_ref_owner_pid,
    perform_action as invoke_accessibility_action, perform_named_action, probe_focused_element,
    set_element_value, snapshot_accessibility_tree, AccessibilityAction, AccessibilityNode,
    AccessibleAppSummary, Bounds, FocusProbe, FocusedElementSummary, ValueSetInvocation,
};
use crate::diagnostics::{doctor_report, DoctorReport};
use crate::outline::{ElementQuery, TreeFormat};
use crate::screenshot::{
    capture_screenshot_raw, prepare_screenshot_payload, RawScreenshotCapture, ScreenshotCapture,
    ScreenshotOutputFormat, ScreenshotPayloadOptions,
};
use crate::windowing::registry;
use crate::windows::{
    focus_window_target, focused_window, list_windows, resolve_window_target,
    window_permission_hint, WindowFocusResult, WindowInfo, WindowTarget,
};
use anyhow::{Context, Result};
use rmcp::{
    handler::server::wrapper::{Json, Parameters},
    model::{CallToolResult, ContentBlock},
    schemars::JsonSchema,
    tool, tool_handler, tool_router, ErrorData, ServerHandler, ServiceExt,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    env,
    ffi::OsString,
    future::Future,
    path::Path,
    process::{Command, Output, Stdio},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    process::Command as TokioCommand,
    time::{sleep, timeout},
};

const INPUT_COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_SCROLL_PAGES: f64 = 100.0;
const SHELL_ENABLE_ENV: &str = "COMPUTER_USE_LINUX_ENABLE_SHELL";
const SHELL_DEFAULT_TIMEOUT_SECS: u64 = 30;
const SHELL_MAX_TIMEOUT_SECS: u64 = 120;
const SHELL_MAX_COMMAND_BYTES: usize = 64 * 1024;
const SHELL_MAX_CWD_BYTES: usize = 4096;
const SHELL_MAX_ENV_ENTRIES: usize = 64;
const SHELL_MAX_ENV_BYTES: usize = 64 * 1024;
const SHELL_RESPONSE_STREAM_BYTES: usize = 512 * 1024;

/// Nodes and their app ownership must come from the same snapshot.
#[derive(Default)]
struct CachedAccessibilitySnapshot {
    nodes: Vec<AccessibilityNode>,
    pid: Option<u32>,
}

#[derive(Clone, Default)]
pub struct ComputerUseLinux {
    last_snapshot: Arc<Mutex<CachedAccessibilitySnapshot>>,
    input_operation_lock: Arc<tokio::sync::Mutex<()>>,
    /// Root window size from the latest capture or X11 query; used for
    /// off-screen warnings and window-relative coordinate clipping.
    desktop_size: Arc<Mutex<Option<(u32, u32)>>>,
    /// Outcome of the latest raw screen capture. `doctor_report` cannot take a
    /// screenshot itself, so readiness reuses what a real capture observed.
    last_screenshot_ok: Arc<Mutex<Option<bool>>>,
    /// Recent `doctor_report`, reused by snapshot-only calls so polling tools
    /// do not rerun every probe each time.
    recent_diagnostics: Arc<Mutex<Option<(std::time::Instant, DoctorReport)>>>,
}

fn sanitize_unsigned_integer_formats(value: &mut serde_json::Value) {
    let serde_json::Value::Object(object) = value else {
        return;
    };

    let has_unsigned_format = matches!(
        object.get("format").and_then(serde_json::Value::as_str),
        Some("uint" | "uint8" | "uint16" | "uint32" | "uint64" | "usize")
    );
    if has_unsigned_format {
        object.remove("format");
    }

    for nested in object.values_mut() {
        match nested {
            serde_json::Value::Object(_) => sanitize_unsigned_integer_formats(nested),
            serde_json::Value::Array(items) => {
                for item in items {
                    sanitize_unsigned_integer_formats(item);
                }
            }
            _ => {}
        }
    }
}

impl ComputerUseLinux {
    fn mcp_tool_router(&self) -> rmcp::handler::server::router::tool::ToolRouter<Self> {
        let mut router = Self::tool_router();
        if !shell_execution_enabled() {
            router.map.remove("run_shell");
        }
        for route in router.map.values_mut() {
            let input_schema = Arc::make_mut(&mut route.attr.input_schema);
            for value in input_schema.values_mut() {
                sanitize_unsigned_integer_formats(value);
            }
            if let Some(output_schema) = route.attr.output_schema.as_mut() {
                for value in Arc::make_mut(output_schema).values_mut() {
                    sanitize_unsigned_integer_formats(value);
                }
            }
        }
        router
    }
}

#[tool_router]
impl ComputerUseLinux {
    #[tool(
        name = "doctor",
        description = "Check the X11 session: X display, i3 window IPC, AT-SPI bus, xdotool and tesseract. readiness.blockers names the fix for each failure.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn doctor(&self) -> Json<DoctorReport> {
        Json(self.diagnostics().await)
    }

    #[tool(
        name = "list_apps",
        description = "List running app processes and the AT-SPI application roots (their names work as app_name_or_bundle_identifier).",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = true
        )
    )]
    async fn list_apps(&self) -> Json<ListAppsOutput> {
        let (accessible_apps, accessibility_error) = match list_accessible_apps(50).await {
            Ok(apps) => (apps, None),
            Err(error) => (Vec::new(), Some(format!("{error:#}"))),
        };

        Json(ListAppsOutput {
            apps: list_process_apps(),
            accessible_apps,
            accessibility_error,
            note: "Linux Computer Use lists process candidates plus AT-SPI application roots when accessibility is enabled.".to_string(),
        })
    }

    #[tool(
        name = "list_windows",
        description = "List X11 windows (i3, or EWMH via wmctrl) with window_id, title, app_id (WM_CLASS instance), wm_class, pid, focus state and bounds.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = true
        )
    )]
    async fn list_windows(&self) -> Json<ListWindowsOutput> {
        Json(window_list_output().await)
    }

    #[tool(
        name = "focused_window",
        description = "Return the X11 window that has keyboard focus.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = true
        )
    )]
    async fn focused_window(&self) -> Json<FocusedWindowOutput> {
        match focused_window().await {
            Ok(window) => {
                let backend = window_backend(window.as_ref().into_iter());
                Json(FocusedWindowOutput {
                    backend,
                    focused_window: window,
                    error: None,
                    permissions_hint: None,
                    message: "Focused window query completed.".to_string(),
                })
            }
            Err(error) => {
                let error = format!("{error:#}");
                Json(FocusedWindowOutput {
                    backend: "unavailable".to_string(),
                    focused_window: None,
                    permissions_hint: window_permission_hint(&error),
                    error: Some(error),
                    message: "Focused window query failed; targeted keyboard input is unavailable until window introspection works.".to_string(),
                })
            }
        }
    }

    #[tool(
        name = "activate_window",
        description = "Focus an X11 window (switching i3 workspace if needed) by window_id, pid, app_id, wm_class, title, or terminal selectors (tty, terminal_pid, terminal_command, terminal_cwd).",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = true
        )
    )]
    async fn activate_window(
        &self,
        Parameters(params): Parameters<ActivateWindowParams>,
    ) -> Json<ActivateWindowOutput> {
        let target = params.into_target();
        let received = Some(serde_json::json!(target.clone()));
        match focus_window_target(&target).await {
            Ok(focus) => {
                let ok = focus_satisfies_target(&focus, &target);
                Json(ActivateWindowOutput {
                    ok,
                    implemented: true,
                    backend: focus.backend.clone(),
                    focus: Some(focus),
                    error: None,
                    permissions_hint: None,
                    received,
                })
            }
            Err(error) => {
                let error = format!("{error:#}");
                Json(ActivateWindowOutput {
                    ok: false,
                    implemented: true,
                    backend: "unavailable".to_string(),
                    focus: None,
                    permissions_hint: window_permission_hint(&error),
                    error: Some(error),
                    received,
                })
            }
        }
    }

    #[tool(
        name = "get_app_state",
        output_schema = rmcp::handler::server::tool::schema_for_output::<GetAppStateOutput>()
            .expect("get_app_state output schema"),
        description = "Screenshot plus accessibility tree of an app on the X11 display; call it first. Scope with app_name_or_bundle_identifier (AT-SPI app name, e.g. chrome) or a window target (window_id/pid/app_id/wm_class/title); unscoped it returns the whole desktop. accessibility_outline has one line per element, `[index] role \"name\" @x,y wxh {states} actions=...`, indented by depth; the index is what click, perform_action and set_value take (tree_format=json gives full nodes). find_elements is cheaper for one control. screenshot metadata has coordinate_width/coordinate_height and scale for mapping image pixels to screen coordinates. A window-targeted screenshot needs that window already focused.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = true
        )
    )]
    async fn get_app_state(
        &self,
        Parameters(params): Parameters<GetAppStateParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.app_state(params).await
    }

    async fn app_state(&self, params: GetAppStateParams) -> Result<CallToolResult, ErrorData> {
        let tree_format = params.tree_format.unwrap_or_default();
        let (mut output, screenshot) = self.collect_app_state(params).await;
        if tree_format == TreeFormat::Outline {
            let viewport = output
                .window_context
                .as_ref()
                .and_then(|window| window.bounds.as_ref())
                .map(|bounds| (bounds.width, bounds.height));
            output.accessibility_outline = Some(crate::outline::render_outline(
                &output.accessibility_tree,
                viewport,
            ));
            output.accessibility_tree.clear();
        }
        app_state_result(output, screenshot)
    }

    /// Captures the screenshot and accessibility snapshot for `get_app_state`
    /// and publishes the snapshot so element indices resolve against it.
    async fn collect_app_state(
        &self,
        params: GetAppStateParams,
    ) -> (GetAppStateOutput, Option<ScreenshotCapture>) {
        let verbose = params.verbose.unwrap_or(false);
        let mut diagnostics = if verbose || params.include_screenshot.unwrap_or(true) {
            self.diagnostics().await
        } else {
            self.cached_diagnostics().await
        };
        let (window_context, window_error, window_permissions_hint) =
            self.resolve_window_context(&params).await;
        let (max_nodes, max_depth) =
            crate::atspi_tree::snapshot_limits(params.max_nodes, params.max_depth);
        let include_screenshot = params.include_screenshot.unwrap_or(true);
        let screenshot_options = params.screenshot_options();
        let screenshot_target_requested = params.window_target().has_target();
        // Whether the caller asked for any scope at all. `tree_scoped` can still
        // come back false when a target matched no AT-SPI root, and that case
        // needs different advice than "pass a target".
        let accessibility_target_requested = screenshot_target_requested
            || params
                .app_name_or_bundle_identifier
                .as_deref()
                .is_some_and(|name| !name.trim().is_empty());
        let app_filter = self
            .resolve_accessibility_app_filter(&params, window_context.as_ref())
            .await;
        let (screenshot, screenshot_error) = if include_screenshot {
            let result: Result<ScreenshotCapture> = async {
                let _pipeline = crate::screenshot_impl::screenshot_pipeline_permit().await?;
                let raw = capture_screenshot_raw().await;
                diagnostics.readiness.record_screenshot_result(raw.is_ok());
                self.record_screenshot_result(raw.is_ok());
                let raw = raw?;
                self.cache_desktop_size(raw.width, raw.height);
                let crop = if let Some(window) = window_context.as_ref() {
                    ensure_readonly_screenshot_target_is_visible(window)?;
                    Some(window_capture_rect(window, raw.width, raw.height)?)
                } else {
                    None
                };
                crate::screenshot_impl::run_image_task(move || {
                    prepare_app_state_screenshot(
                        raw,
                        crop,
                        screenshot_target_requested,
                        screenshot_options,
                    )
                })
                .await
            }
            .await;
            match result {
                Ok(capture) => (Some(capture), None),
                Err(error) => (None, Some(format!("{error:#}"))),
            }
        } else {
            (None, None)
        };
        let mut tree_scoped = false;
        let mut tree_root_pid = None;
        let mut accessibility_tree_truncated = false;
        let mut shell_only_note = None;
        let (accessibility_tree, accessibility_tree_raw_count, accessibility_error) =
            if diagnostics.readiness.can_build_accessibility_tree {
                let target_pid = window_context.as_ref().and_then(|window| window.pid);
                match snapshot_accessibility_tree(
                    app_filter.as_deref(),
                    target_pid,
                    max_nodes,
                    max_depth,
                )
                .await
                {
                    Ok(snapshot) => {
                        tree_scoped = snapshot.scoped;
                        tree_root_pid = snapshot.root_pid;
                        accessibility_tree_truncated = snapshot.truncated;
                        let raw_count = snapshot.nodes.len();
                        let (nodes, note) = compact_accessibility_tree_with_note(
                            snapshot.nodes,
                            tree_scoped,
                            accessibility_tree_truncated,
                        );
                        shell_only_note = note;
                        (nodes, raw_count, None)
                    }
                    Err(error) => (Vec::new(), 0, Some(format!("{error:#}"))),
                }
            } else {
                (
                    Vec::new(),
                    0,
                    Some("AT-SPI is unavailable; readiness.blockers names the fix.".to_string()),
                )
            };
        if accessibility_error.is_none() {
            // Record a pid only when the tree's roots were matched by it. A pid
            // with no AT-SPI root falls back to every app, or to an app-name
            // match that can be another app entirely; recording the pid then
            // would let that app's index pass the target check instead of
            // taking the per-node owner lookup.
            self.commit_snapshot(&accessibility_tree, tree_root_pid)
                .await;
        } else {
            self.commit_snapshot(&[], None).await;
        }
        let mut message = if let Some(error) = &accessibility_error {
            format!("MCP registration is working, but AT-SPI tree extraction failed: {error}")
        } else if let Some(capture) = &screenshot {
            format!(
                "MCP registration, screenshot capture, and AT-SPI tree extraction are working. Captured {} accessibility nodes (compacted from {}) and a screenshot through {}.",
                accessibility_tree.len(),
                accessibility_tree_raw_count,
                capture.source
            )
        } else if let Some(error) = &screenshot_error {
            format!(
                "MCP registration and AT-SPI tree extraction are working. Captured {} accessibility nodes (compacted from {}). Screenshot capture failed: {error}",
                accessibility_tree.len(),
                accessibility_tree_raw_count,
            )
        } else {
            format!(
                "MCP registration and AT-SPI tree extraction are working. Captured {} accessibility nodes (compacted from {}). Screenshot capture was not requested.",
                accessibility_tree.len(),
                accessibility_tree_raw_count,
            )
        };
        if let Some(window) = &window_context {
            message.push_str(&format!(
                " Window target resolved to window_id {}.",
                window.window_id
            ));
        } else if let Some(error) = &window_error {
            message.push_str(&format!(" Window target resolution failed: {error}"));
        }
        if let Some(warning) = unscoped_accessibility_tree_warning(
            tree_scoped,
            accessibility_error.is_none(),
            accessibility_target_requested,
        ) {
            message.push(' ');
            message.push_str(warning);
        }
        if let Some(note) = truncated_accessibility_tree_note(accessibility_tree_truncated) {
            message.push(' ');
            message.push_str(note);
        }
        if let Some(note) = shell_only_note {
            message.push(' ');
            message.push_str(note);
        }

        // Full diagnostics are large; emit them only on request. The compact readiness block always travels, and failures get
        // a pointer to verbose=true instead of an automatic dump.
        let readiness = diagnostics.readiness.clone();
        let include_full = verbose;
        if !include_full
            && (accessibility_error.is_some()
                || screenshot_error.is_some()
                || window_error.is_some())
        {
            message.push_str(" Pass verbose=true for full diagnostics.");
        }
        let output = GetAppStateOutput {
            app_name_or_bundle_identifier: params.app_name_or_bundle_identifier,
            window_context,
            window_error,
            window_permissions_hint,
            backend: "linux-atspi".to_string(),
            screenshot: screenshot.as_ref().map(ScreenshotSummary::from),
            screenshot_error,
            accessibility_tree,
            accessibility_outline: None,
            accessibility_tree_raw_count,
            tree_scoped,
            accessibility_tree_truncated,
            accessibility_error,
            readiness,
            diagnostics: include_full.then_some(diagnostics),
            message,
        };
        (output, screenshot)
    }

    #[tool(
        name = "find_elements",
        description = "Search an app's accessibility tree and return only matching elements as outline lines `[index] role \"name\" @x,y wxh {states} actions=...`. Cheaper than get_app_state for locating a control. Scope with app_name_or_bundle_identifier or a window target. role/name/text/states must all match (case-insensitive substrings). The indices stay valid for click, perform_action and set_value until the next snapshot.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = true
        )
    )]
    async fn find_elements(
        &self,
        Parameters(params): Parameters<FindElementsParams>,
    ) -> Json<FindElementsOutput> {
        if params.query.is_empty() {
            return Json(FindElementsOutput {
                matches: Vec::new(),
                total_matches: 0,
                searched_nodes: 0,
                tree_truncated: false,
                message: "Pass at least one of role, name, text, or states.".to_string(),
            });
        }
        let limit = params.limit.unwrap_or(25).clamp(1, 200);
        let (output, _) = self.collect_app_state(params.snapshot_params()).await;
        if let Some(error) = output.window_error.as_ref() {
            return Json(FindElementsOutput {
                matches: Vec::new(),
                total_matches: 0,
                searched_nodes: 0,
                tree_truncated: false,
                message: format!("Window target did not resolve: {error}"),
            });
        }
        let matching: Vec<String> = output
            .accessibility_tree
            .iter()
            .filter(|node| params.query.matches(node))
            .map(crate::outline::render_line)
            .collect();
        let total_matches = matching.len();
        let message = match (&output.accessibility_error, total_matches) {
            (Some(error), _) => format!("Accessibility snapshot failed: {error}"),
            (None, 0) => format!(
                "No element matched among {} nodes.{}",
                output.accessibility_tree.len(),
                if output.accessibility_tree_truncated {
                    " The tree was truncated; scope to a window or raise max_nodes."
                } else {
                    ""
                }
            ),
            (None, n) if n > limit => format!("Showing {limit} of {n} matches; narrow the query."),
            (None, n) => format!("{n} match(es)."),
        };
        Json(FindElementsOutput {
            matches: matching.into_iter().take(limit).collect(),
            total_matches,
            searched_nodes: output.accessibility_tree.len(),
            tree_truncated: output.accessibility_tree_truncated,
            message,
        })
    }

    #[tool(
        name = "wait_for",
        description = "Poll an app's accessibility tree until a condition holds or timeout_ms passes. condition=appears waits for an element matching role/name/text/states; disappears waits until none match; changed waits until the scoped tree differs from its state at the first poll and returns the differing lines prefixed + or - (the query is ignored). Each poll rereads the tree, about 1-3 s for a large browser page, so the effective timeout rounds up to whole polls. Use after an action instead of repeated screenshots.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = true
        )
    )]
    async fn wait_for(&self, Parameters(params): Parameters<WaitForParams>) -> Json<WaitForOutput> {
        let timeout_ms = params.timeout_ms.unwrap_or(5_000).clamp(100, 60_000);
        let interval = Duration::from_millis(params.interval_ms.unwrap_or(400).clamp(100, 5_000));
        let condition = params.condition;
        if condition != WaitCondition::Changed && params.query.is_empty() {
            return Json(WaitForOutput {
                satisfied: false,
                elapsed_ms: 0,
                polls: 0,
                matches: Vec::new(),
                message: "appears/disappears need at least one of role, name, text, or states."
                    .to_string(),
            });
        }
        let started = std::time::Instant::now();
        let mut baseline: Option<Vec<crate::outline::ElementContent>> = None;
        let mut polls = 0;
        loop {
            polls += 1;
            let (output, _) = self.collect_app_state(params.snapshot_params()).await;
            if let Some(error) = output.window_error.as_ref() {
                return Json(WaitForOutput {
                    satisfied: false,
                    elapsed_ms: started.elapsed().as_millis() as u64,
                    polls,
                    matches: Vec::new(),
                    message: format!("Window target did not resolve: {error}"),
                });
            }
            let mut matches: Vec<String> = Vec::new();
            let satisfied = output.accessibility_error.is_none()
                && match condition {
                    WaitCondition::Appears | WaitCondition::Disappears => {
                        matches = output
                            .accessibility_tree
                            .iter()
                            .filter(|node| params.query.matches(node))
                            .map(crate::outline::render_line)
                            .take(25)
                            .collect();
                        presence_condition_met(
                            condition,
                            !matches.is_empty(),
                            output.accessibility_tree_truncated,
                        )
                    }
                    WaitCondition::Changed => {
                        let current = crate::outline::element_contents(&output.accessibility_tree);
                        match &baseline {
                            None => {
                                baseline = Some(current);
                                false
                            }
                            Some(first) => {
                                matches = crate::outline::content_diff(first, &current, 25);
                                !matches.is_empty()
                            }
                        }
                    }
                };
            let elapsed_ms = started.elapsed().as_millis() as u64;
            if satisfied || elapsed_ms >= timeout_ms {
                let message = if satisfied {
                    format!("Condition met after {elapsed_ms} ms ({polls} polls).")
                } else if let Some(error) = &output.accessibility_error {
                    format!("Timed out after {elapsed_ms} ms; last snapshot failed: {error}")
                } else if condition != WaitCondition::Changed && output.accessibility_tree_truncated
                {
                    format!(
                        "Timed out after {elapsed_ms} ms ({polls} polls). The tree was truncated, so the element may lie past the nodes read; scope to a window or raise max_nodes."
                    )
                } else {
                    format!("Timed out after {elapsed_ms} ms ({polls} polls).")
                };
                return Json(WaitForOutput {
                    satisfied,
                    elapsed_ms,
                    polls,
                    matches,
                    message,
                });
            }
            sleep(interval).await;
        }
    }

    #[tool(
        name = "ocr",
        description = "Read on-screen text with tesseract OCR and return lines as `@x,y wxh text`. Use it where the accessibility tree is empty (canvas, some Electron apps). Without a window target the whole screen is read in screen coordinates; with one the window is focused first and coordinates are window-relative, ready for click with relative=true. Optional region crops first; query keeps only lines containing that text (case-insensitive). Default languages: por+eng when installed.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = true
        )
    )]
    async fn ocr(&self, Parameters(params): Parameters<OcrParams>) -> Json<OcrOutput> {
        let result = self.run_ocr(&params).await;
        Json(match result {
            Ok((lines, coordinate_space)) => {
                let needle = params
                    .query
                    .as_deref()
                    .map(str::trim)
                    .filter(|query| !query.is_empty())
                    .map(str::to_lowercase);
                let matching: Vec<String> = lines
                    .iter()
                    .filter(|line| {
                        needle
                            .as_ref()
                            .is_none_or(|needle| line.text.to_lowercase().contains(needle))
                    })
                    .map(crate::ocr::OcrLine::render)
                    .collect();
                let limit = params.limit.unwrap_or(300).max(1);
                OcrOutput {
                    ok: true,
                    total_lines: matching.len(),
                    lines: matching.into_iter().take(limit).collect(),
                    coordinate_space: coordinate_space.to_string(),
                    error: None,
                }
            }
            Err(error) => OcrOutput {
                ok: false,
                total_lines: 0,
                lines: Vec::new(),
                coordinate_space: String::new(),
                error: Some(format!("{error:#}")),
            },
        })
    }

    #[tool(
        name = "screenshot",
        description = "Capture the X11 screen as a size-bounded image. With a window target (window_id/pid/app_id/wm_class/title) the window is focused first and the image is cropped to it; full_screen=true keeps the whole screen. The caption gives returned width/height, coordinate_width/coordinate_height, scale, format and crop details. Use format=jpeg/quality or max_width/max_height/max_bytes to shrink the payload.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = true
        )
    )]
    async fn screenshot(&self, Parameters(params): Parameters<ScreenshotParams>) -> CallToolResult {
        match self.capture_screenshot(params).await {
            Ok(result) => result,
            Err(error) => CallToolResult::error(vec![ContentBlock::text(format!("{error:#}"))]),
        }
    }

    async fn capture_screenshot(&self, params: ScreenshotParams) -> Result<CallToolResult> {
        let _pipeline = crate::screenshot_impl::screenshot_pipeline_permit().await?;
        let target_window = match params.window_target() {
            Some(target) => Some(
                self.resolve_screenshot_window(&target)
                    .await
                    .context("targeted screenshot failed")?,
            ),
            None => None,
        };
        let crop_window = (!params.full_screen.unwrap_or(false))
            .then_some(target_window.as_ref())
            .flatten();
        let window_label = target_window
            .as_ref()
            .and_then(|window| window.title.clone());

        let raw_capture = capture_screenshot_raw().await;
        self.record_screenshot_result(raw_capture.is_ok());
        let raw_capture = raw_capture.context("screenshot failed")?;
        self.cache_desktop_size(raw_capture.width, raw_capture.height);

        // Warn when the target window extends past the root window: the crop
        // silently loses the off-screen region.
        let off_screen_note = match crop_window.and_then(|window| window.bounds.as_ref()) {
            Some(bounds) => self.off_screen_note_for_bounds(bounds).await,
            None => None,
        };

        let (capture, cropped) = match crop_window {
            Some(window) => {
                let (x, y, width, height) =
                    window_capture_rect(window, raw_capture.width, raw_capture.height)
                        .context("targeted screenshot crop failed")?;
                let (raw_capture, bytes, width, height) =
                    crate::screenshot_impl::run_image_task(move || {
                        let (bytes, width, height) =
                            crop_png(&raw_capture.bytes, x, y, width, height).map_err(|error| {
                                anyhow::anyhow!("targeted screenshot crop failed: {error}")
                            })?;
                        Ok((raw_capture, bytes, width, height))
                    })
                    .await?;
                (
                    RawScreenshotCapture {
                        mime_type: raw_capture.mime_type,
                        bytes,
                        source: raw_capture.source,
                        width,
                        height,
                    },
                    true,
                )
            }
            None => (raw_capture, false),
        };
        let options = params.screenshot_options();
        let capture = crate::screenshot_impl::run_image_task(move || {
            prepare_screenshot_payload(capture, options)
        })
        .await
        .context("screenshot resize failed")?;

        let mut caption = serde_json::json!({
            "width": capture.width,
            "height": capture.height,
            "coordinate_width": capture.coordinate_width,
            "coordinate_height": capture.coordinate_height,
            "scale": capture.scale,
            "resized": capture.resized,
            "bytes": capture.bytes,
            "original_bytes": capture.original_bytes,
            "max_bytes": capture.max_bytes,
            "format": capture.format,
            "quality": capture.quality,
            "source": capture.source,
            "cropped_to_window": cropped,
            "window_title": window_label,
        });
        if let Some(note) = off_screen_note {
            caption["window_off_screen"] = serde_json::json!(true);
            caption["off_screen_note"] = serde_json::json!(note);
        }
        Ok(CallToolResult::success(vec![
            ContentBlock::image(data_url_payload(&capture.data_url), capture.mime_type),
            ContentBlock::text(caption.to_string()),
        ]))
    }

    #[tool(
        name = "click",
        description = "Click an element by element_index or semantic selector (role/name/text/states), or at screen coordinates x/y (screenshot coordinate pixels; with relative=true and a window target, relative to that window's crop). A plain left click on an element invokes its AT-SPI click/press/toggle action; coordinates, right/middle buttons and click_count>1 send an xdotool pointer click.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = true
        )
    )]
    async fn click(&self, Parameters(mut params): Parameters<ClickParams>) -> Json<ActionOutput> {
        let received = Some(serde_json::json!(params.clone()));
        let input_guard = Arc::clone(&self.input_operation_lock).lock_owned().await;
        let mut target_pid = params.pid;
        // Raise the target window first (if specified) so the click lands on the
        // intended app rather than whatever is stacked on top at that pixel.
        let window_target = params.window_target();
        if params.relative == Some(true) && window_target.is_none() {
            return Json(ActionOutput {
                ok: false,
                implemented: true,
                action: "click".to_string(),
                message: "Relative coordinate clicks require a window target.".to_string(),
                received,
            });
        }
        if let Some(target) = window_target {
            let focus = match self.focus_target_for_input(&target).await {
                Ok(focus) => focus,
                Err(message) => {
                    return Json(ActionOutput {
                        ok: false,
                        implemented: true,
                        action: "click".to_string(),
                        message,
                        received,
                    });
                }
            };
            target_pid = focus_target_pid(focus.as_ref()).or(target_pid);
            tokio::time::sleep(Duration::from_millis(120)).await;
            // Window-relative coordinates: translate by the window's top-left so
            // the agent can click the pixel it saw in a window-cropped screenshot.
            if params.relative == Some(true) {
                let Some(focus) = focus.as_ref() else {
                    return Json(ActionOutput {
                        ok: false,
                        implemented: true,
                        action: "click".to_string(),
                        message: "Relative coordinate clicks require verified target-window focus."
                            .to_string(),
                        received,
                    });
                };
                let capture_rect = match self.focused_window_capture_rect(focus).await {
                    Ok(rect) => rect,
                    Err(message) => {
                        return Json(ActionOutput {
                            ok: false,
                            implemented: true,
                            action: "click".to_string(),
                            message,
                            received,
                        });
                    }
                };
                if let Err(message) =
                    apply_window_relative_click_coordinates(&mut params, capture_rect)
                {
                    return Json(ActionOutput {
                        ok: false,
                        implemented: true,
                        action: "click".to_string(),
                        message,
                        received,
                    });
                }
            }
        }
        if params.x.zip(params.y).is_none() {
            if let Some(node) = self.cached_node_for(
                params.element_index,
                &params.selector(),
                ElementResolvePurpose::Click,
            ) {
                if let Err(message) = self.check_node_target(&node, target_pid).await {
                    return Json(ActionOutput {
                        ok: false,
                        implemented: true,
                        action: "click".to_string(),
                        message,
                        received,
                    });
                }
            }
        }
        let target = match self.resolve_click_target(&params) {
            Ok(target) => target,
            Err(message) => {
                return Json(ActionOutput {
                    ok: false,
                    implemented: true,
                    action: "click".to_string(),
                    message,
                    received,
                });
            }
        };
        if let ClickTarget::PrimaryAction {
            object_ref,
            action_name,
            action_index,
        } = target
        {
            let invocation = if let Some(name) = action_name
                .as_deref()
                .filter(|name| !name.trim().is_empty())
            {
                perform_named_action(&object_ref, name).await
            } else {
                invoke_accessibility_action(&object_ref, Some(&action_index.to_string())).await
            };
            return match invocation {
                Ok(invocation) => Json(ActionOutput {
                    ok: invocation.ok,
                    implemented: true,
                    action: "click".to_string(),
                    message: if invocation.ok {
                        format!(
                            "Invoked the primary AT-SPI action{} without pointer-coordinate conversion.",
                            action_name
                                .as_deref()
                                .filter(|name| !name.is_empty())
                                .map(|name| format!(" ({name})"))
                                .unwrap_or_default()
                        )
                    } else {
                        format!(
                            "The primary AT-SPI action{} returned false.",
                            action_name
                                .as_deref()
                                .filter(|name| !name.is_empty())
                                .map(|name| format!(" ({name})"))
                                .unwrap_or_default()
                        )
                    },
                    received,
                }),
                Err(error) => Json(ActionOutput {
                    ok: false,
                    implemented: true,
                    action: "click".to_string(),
                    message: error.to_string(),
                    received,
                }),
            };
        }
        let ClickTarget::Coordinates(x, y) = target else {
            unreachable!("click target must resolve to coordinates or an AT-SPI action");
        };
        let Some(xdotool_args) = xdotool_pointer_click_args(
            x,
            y,
            params.click_count.unwrap_or(1).clamp(1, 10),
            params.button.as_deref(),
        ) else {
            return Json(ActionOutput {
                ok: false,
                implemented: true,
                action: "click".to_string(),
                message: "Unsupported button; expected left, middle, or right.".to_string(),
                received,
            });
        };
        let off_screen_note = self.off_screen_note_for_point(x, y).await;
        let (input_guard, result) = run_cancellation_safe_input(input_guard, async move {
            run_xdotool(&xdotool_args, INPUT_COMMAND_TIMEOUT).await
        })
        .await;
        let _input_guard = input_guard;
        Json(with_notes(
            action_result("click", result.map(|output| vec![output]), received),
            off_screen_note,
        ))
    }

    #[tool(
        name = "perform_action",
        description = "Invoke an accessibility action exposed by an element selected by index, identifier, or semantic selector. Defaults to the primary action unless action is provided.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = true
        )
    )]
    async fn perform_action(
        &self,
        Parameters(params): Parameters<ActionParams>,
    ) -> Json<ActionOutput> {
        let requested_action = requested_or_primary_action(params.action.as_deref());
        self.perform_element_action(&params, requested_action).await
    }

    #[tool(
        name = "set_value",
        description = "Set the value of a settable accessibility element selected by index, identifier, or semantic selector.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = true
        )
    )]
    async fn set_value(
        &self,
        Parameters(params): Parameters<SetValueParams>,
    ) -> Json<ActionOutput> {
        let _input_guard = Arc::clone(&self.input_operation_lock).lock_owned().await;
        let received = Some(serde_json::json!(params.clone()));
        let object_ref = match self.resolve_object_ref(
            params.element_index,
            params.element_identifier.as_deref(),
            &params.selector(),
            ElementResolvePurpose::SetValue,
        ) {
            Ok(object_ref) => object_ref,
            Err(message) => {
                return Json(ActionOutput {
                    ok: false,
                    implemented: true,
                    action: "set_value".to_string(),
                    message,
                    received,
                });
            }
        };

        match set_element_value(&object_ref, &params.value).await {
            Ok(ValueSetInvocation::Numeric { value }) => Json(ActionOutput {
                ok: true,
                implemented: true,
                action: "set_value".to_string(),
                message: format!("AT-SPI numeric value set to {value}."),
                received,
            }),
            Ok(ValueSetInvocation::EditableText) => Json(ActionOutput {
                ok: true,
                implemented: true,
                action: "set_value".to_string(),
                message: "AT-SPI editable text contents set.".to_string(),
                received,
            }),
            Err(error) => Json(ActionOutput {
                ok: false,
                implemented: true,
                action: "set_value".to_string(),
                message: error.to_string(),
                received,
            }),
        }
    }

    #[tool(
        name = "scroll",
        description = "Scroll an element in a direction by a number of pages. With a window target and no x/y/element_index, scrolls at the centre of the targeted window.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = true
        )
    )]
    async fn scroll(&self, Parameters(mut params): Parameters<ScrollParams>) -> Json<ActionOutput> {
        let received = Some(serde_json::json!(params.clone()));
        let units = match scroll_units(params.pages) {
            Ok(units) => units,
            Err(message) => {
                return Json(ActionOutput {
                    ok: false,
                    implemented: true,
                    action: "scroll".to_string(),
                    message,
                    received,
                });
            }
        };
        let input_guard = Arc::clone(&self.input_operation_lock).lock_owned().await;
        let mut target_pid = params.pid;
        // Raise/focus the target window first (parity with click) so wheel
        // events land on the intended app.
        let window_target = params.window_target();
        if params.relative == Some(true) && window_target.is_none() {
            return Json(ActionOutput {
                ok: false,
                implemented: true,
                action: "scroll".to_string(),
                message: "Relative scroll coordinates require a window target.".to_string(),
                received,
            });
        }
        if let Some(target) = window_target {
            let focus = match self.focus_target_for_input(&target).await {
                Ok(focus) => focus,
                Err(message) => {
                    return Json(ActionOutput {
                        ok: false,
                        implemented: true,
                        action: "scroll".to_string(),
                        message,
                        received,
                    });
                }
            };
            target_pid = focus_target_pid(focus.as_ref()).or(target_pid);
            tokio::time::sleep(Duration::from_millis(120)).await;
            if params.relative == Some(true) {
                let Some(focus) = focus.as_ref() else {
                    return Json(ActionOutput {
                        ok: false,
                        implemented: true,
                        action: "scroll".to_string(),
                        message:
                            "Relative scroll coordinates require verified target-window focus."
                                .to_string(),
                        received,
                    });
                };
                let capture_rect = match self.focused_window_capture_rect(focus).await {
                    Ok(rect) => rect,
                    Err(message) => {
                        return Json(ActionOutput {
                            ok: false,
                            implemented: true,
                            action: "scroll".to_string(),
                            message,
                            received,
                        });
                    }
                };
                if let Err(message) =
                    apply_window_relative_scroll_coordinates(&mut params, capture_rect)
                {
                    return Json(ActionOutput {
                        ok: false,
                        implemented: true,
                        action: "scroll".to_string(),
                        message,
                        received,
                    });
                }
            } else if params.x.is_none() && params.y.is_none() && params.element_index.is_none() {
                // A window target without a point would otherwise scroll
                // whatever happens to sit under the pointer: focusing does not
                // move the cursor, and the wheel path never repositions it.
                // Default to the centre of the resolved target window.
                let Some(focus) = focus.as_ref() else {
                    return Json(ActionOutput {
                        ok: false,
                        implemented: true,
                        action: "scroll".to_string(),
                        message: "Window-targeted scroll requires verified target-window focus."
                            .to_string(),
                        received,
                    });
                };
                let capture_rect = match self.focused_window_capture_rect(focus).await {
                    Ok(rect) => rect,
                    Err(message) => {
                        return Json(ActionOutput {
                            ok: false,
                            implemented: true,
                            action: "scroll".to_string(),
                            message,
                            received,
                        });
                    }
                };
                if let Err(message) = apply_window_center_scroll_point(&mut params, capture_rect) {
                    return Json(ActionOutput {
                        ok: false,
                        implemented: true,
                        action: "scroll".to_string(),
                        message,
                        received,
                    });
                }
            }
        }
        if params.x.is_none() && params.y.is_none() {
            if let Some(node) = self.cached_node_for(
                params.element_index,
                &ElementSelector::default(),
                ElementResolvePurpose::Click,
            ) {
                if let Err(message) = self.check_node_target(&node, target_pid).await {
                    return Json(ActionOutput {
                        ok: false,
                        implemented: true,
                        action: "scroll".to_string(),
                        message,
                        received,
                    });
                }
            }
        }
        let target_point =
            match self.resolve_optional_target_point(params.x, params.y, params.element_index) {
                Ok(point) => point,
                Err(message) => {
                    return Json(ActionOutput {
                        ok: false,
                        implemented: true,
                        action: "scroll".to_string(),
                        message,
                        received,
                    });
                }
            };
        let direction = match params.direction.to_ascii_lowercase().as_str() {
            "up" => ScrollDirection::Up,
            "down" => ScrollDirection::Down,
            "left" => ScrollDirection::Left,
            "right" => ScrollDirection::Right,
            _ => {
                return Json(ActionOutput {
                    ok: false,
                    implemented: true,
                    action: "scroll".to_string(),
                    message: "Unsupported scroll direction; expected up, down, left, or right."
                        .to_string(),
                    received,
                });
            }
        };
        let off_screen_note = match target_point {
            Some((x, y)) => self.off_screen_note_for_point(x, y).await,
            None => None,
        };
        // XTEST wheel buttons have no XI2 valuators, so GTK 3 takes its button
        // path instead of re-baselining smooth-scroll valuators (#184).
        let repeat = u32::try_from(units.max(1)).unwrap_or(1);
        let xdotool_args = xdotool_pointer_scroll_args(target_point, repeat, direction);
        let command_timeout = xdotool_scroll_timeout(repeat, XDOTOOL_SCROLL_CLICK_DELAY_MS);
        let (input_guard, result) = run_cancellation_safe_input(input_guard, async move {
            run_xdotool(&xdotool_args, command_timeout).await
        })
        .await;
        let _input_guard = input_guard;
        Json(with_notes(
            action_result("scroll", result.map(|output| vec![output]), received),
            off_screen_note,
        ))
    }

    #[tool(
        name = "drag",
        description = "Drag with the left button from start_x,start_y to end_x,end_y in screen coordinates (xdotool).",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = true
        )
    )]
    async fn drag(&self, Parameters(params): Parameters<DragParams>) -> Json<ActionOutput> {
        let received = Some(serde_json::json!(params));
        let input_guard = Arc::clone(&self.input_operation_lock).lock_owned().await;
        let (input_guard, result) = run_cancellation_safe_input(input_guard, async move {
            run_xdotool_drag(params.start_x, params.start_y, params.end_x, params.end_y).await
        })
        .await;
        let _input_guard = input_guard;
        Json(action_result("drag", result, received))
    }

    #[tool(
        name = "press_key",
        description = "Press a key or chord with xdotool, optionally after focusing a target window or terminal selector. Grammar (case-insensitive; hyphens/spaces ignored): combos join with '+', e.g. Ctrl+L or Ctrl+Shift+T. Modifiers: ctrl/control, alt/option, shift, meta/super/cmd/command. Named keys: enter/return, escape/esc, tab, backspace, delete/del, space, home, end, pageup, pagedown, arrowleft/left, arrowright/right, arrowup/up, arrowdown/down, f1-f12, plus single letters a-z and digits 0-9. Anything else is an error. i3 may consume its own bindings (Super+...) before the app sees them.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = true
        )
    )]
    async fn press_key(
        &self,
        Parameters(params): Parameters<PressKeyParams>,
    ) -> Json<ActionOutput> {
        let received = Some(serde_json::json!(params.clone()));
        let input_guard = Arc::clone(&self.input_operation_lock).lock_owned().await;
        let focus = match self.focus_target_for_input(&params.window_target()).await {
            Ok(focus) => focus,
            Err(message) => {
                return Json(ActionOutput {
                    ok: false,
                    implemented: true,
                    action: "press_key".to_string(),
                    message,
                    received,
                });
            }
        };
        let Some(spec) = xdotool_key_spec(&params.key) else {
            return Json(ActionOutput {
                ok: false,
                implemented: true,
                action: "press_key".to_string(),
                message: "Unsupported key. Use names like Enter, Escape, Tab, ArrowLeft, Super, Ctrl+L, or a single US keyboard letter/digit.".to_string(),
                received,
            });
        };
        let xdotool_args = vec!["key".to_string(), "--clearmodifiers".to_string(), spec];
        let (input_guard, result) = run_cancellation_safe_input(input_guard, async move {
            run_xdotool(&xdotool_args, INPUT_COMMAND_TIMEOUT).await
        })
        .await;
        let _input_guard = input_guard;
        let mut output = action_result_with_focus(
            "press_key",
            result.map(|output| vec![output]),
            received,
            focus.clone(),
        );
        if output.ok && focus.is_some() {
            let notes = self.input_landing_notes(focus.as_ref(), false).await;
            output = with_notes(output, notes);
        }
        Json(output)
    }

    #[tool(
        name = "run_shell",
        description = "Execute one explicitly approved /bin/sh command with same-user host authority. This tool is absent unless the server operator starts computer-use-linux with COMPUTER_USE_LINUX_ENABLE_SHELL=1. It is not sandboxed: the command can read or modify files and use the network with the server user's permissions. The inherited environment is cleared to a small desktop/runtime allowlist; pass any additional variables explicitly. Execution time and output are bounded: returned streams are truncated to 512 KiB, while a stream exceeding the 8 MiB collection ceiling fails the call without returning partial output. An audit digest is written to server stderr.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = true
        )
    )]
    async fn run_shell(
        &self,
        Parameters(params): Parameters<RunShellParams>,
    ) -> Json<RunShellOutput> {
        Json(execute_shell(params).await)
    }

    #[tool(
        name = "type_text",
        description = "Type literal text with xdotool (layout-aware XTEST), optionally after focusing a target window or terminal selector; newlines press Return. With a target, the result reports the AT-SPI focused element and warns when it is not editable. Success confirms dispatch only; re-observe the field before retrying. NUL is rejected. For long text in an editable field, set_value is faster.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = true
        )
    )]
    async fn type_text(
        &self,
        Parameters(params): Parameters<TypeTextParams>,
    ) -> Json<ActionOutput> {
        let Json(mut output) = self.dispatch_type_text(params).await;
        if output.ok {
            output.message.push_str(
                " Field contents were not verified; re-observe the target before retrying.",
            );
        }
        Json(output)
    }

    async fn dispatch_type_text(&self, params: TypeTextParams) -> Json<ActionOutput> {
        let received = Some(serde_json::json!(params.clone()));
        // Reject before focus changes.
        if params.text.contains('\0') {
            return Json(action_result(
                "type_text",
                Err("Literal text was not sent: NUL characters are unsupported.".to_string()),
                received,
            ));
        }
        let input_guard = Arc::clone(&self.input_operation_lock).lock_owned().await;
        let focus = match self.focus_target_for_input(&params.window_target()).await {
            Ok(focus) => focus,
            Err(message) => {
                return Json(ActionOutput {
                    ok: false,
                    implemented: true,
                    action: "type_text".to_string(),
                    message,
                    received,
                });
            }
        };
        // xdotool type resolves keysyms against the live XKB layout.
        let delay_ms = xdotool_type_delay_ms();
        let args = xdotool_type_args_with_delay(&params.text, delay_ms);
        let command_timeout = xdotool_type_timeout(&params.text, delay_ms);
        let (input_guard, result) = run_cancellation_safe_input(input_guard, async move {
            run_xdotool(&args, command_timeout).await
        })
        .await;
        let _input_guard = input_guard;
        let mut output = action_result_with_focus(
            "type_text",
            result.map(|output| vec![output]),
            received,
            focus.clone(),
        );
        if output.ok && focus.is_some() {
            let notes = self.input_landing_notes(focus.as_ref(), true).await;
            output = with_notes(output, notes);
        }
        Json(output)
    }
}

#[tool_handler(
    router = self.mcp_tool_router(),
    name = "computer-use-linux",
    // NOTE: keep in lockstep with Cargo.toml + package.json on every release.
    // The rmcp tool_handler macro only accepts a string literal here, so this
    // can't be env!("CARGO_PKG_VERSION"); the MCP safety check (CI) fails the
    // build if it drifts from the Cargo version.
    version = "0.7.17",
    instructions = "Drives apps on a virtual X11 display (Xvfb + i3) through AT-SPI, xdotool and X11 screenshots. Start each turn with get_app_state scoped to the app (app_name_or_bundle_identifier, e.g. chrome) or a window target; unscoped trees can flood context. Prefer element_index from the latest get_app_state or find_elements; click, perform_action and set_value also accept unique role/name/text/states selectors. After an action, use wait_for instead of repeated screenshots. Coordinates are screenshot coordinate pixels (divide image pixels by scale). type_text and press_key take optional window or terminal selectors, refuse to send input when focus cannot be verified, and report the focused AT-SPI element; type_text success means dispatched, not verified, so re-observe before retrying. accessibility_tree_truncated=true means the tree is incomplete: narrow the scope or raise max_nodes (max 2000). Chromium and Electron apps need --force-renderer-accessibility for a full tree; where the tree is empty, use ocr or a screenshot. If a tool fails, call doctor: readiness.blockers names the fix. Tools with readOnlyHint=false change app state; get approval before actions that submit, delete, send, purchase or overwrite data."
)]
impl ServerHandler for ComputerUseLinux {}

fn shell_execution_enabled() -> bool {
    shell_execution_enabled_value(env::var(SHELL_ENABLE_ENV).ok().as_deref())
}

fn shell_execution_enabled_value(value: Option<&str>) -> bool {
    value == Some("1")
}

fn valid_environment_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|first| first == '_' || first.is_ascii_alphabetic())
        && chars.all(|character| character == '_' || character.is_ascii_alphanumeric())
}

fn inherited_shell_environment_allowed(name: &str) -> bool {
    matches!(
        name,
        "PATH"
            | "HOME"
            | "USER"
            | "LOGNAME"
            | "LANG"
            | "TERM"
            | "XDG_RUNTIME_DIR"
            | "DISPLAY"
            | "DBUS_SESSION_BUS_ADDRESS"
    ) || name.starts_with("LC_")
}

fn inherited_shell_environment(
    variables: impl IntoIterator<Item = (OsString, OsString)>,
) -> Vec<(String, String)> {
    variables
        .into_iter()
        .filter_map(|(name, value)| {
            let (Ok(name), Ok(value)) = (name.into_string(), value.into_string()) else {
                return None;
            };
            inherited_shell_environment_allowed(&name).then_some((name, value))
        })
        .collect()
}

fn shell_command_sha256(command: &str) -> String {
    Sha256::digest(command.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn bounded_shell_stream(bytes: &[u8]) -> (String, bool) {
    let truncated = bytes.len() > SHELL_RESPONSE_STREAM_BYTES;
    let visible = if truncated {
        &bytes[..SHELL_RESPONSE_STREAM_BYTES]
    } else {
        bytes
    };
    (String::from_utf8_lossy(visible).into_owned(), truncated)
}

fn shell_error_output(
    command_sha256: String,
    cwd: String,
    timeout_seconds: u64,
    error: impl Into<String>,
) -> RunShellOutput {
    RunShellOutput {
        ok: false,
        command_sha256,
        cwd,
        timeout_seconds,
        exit_code: None,
        stdout: String::new(),
        stderr: String::new(),
        stdout_truncated: false,
        stderr_truncated: false,
        error: Some(error.into()),
    }
}

async fn execute_shell(params: RunShellParams) -> RunShellOutput {
    let command_sha256 = shell_command_sha256(&params.command);
    let timeout_seconds = params.timeout_seconds.unwrap_or(SHELL_DEFAULT_TIMEOUT_SECS);
    let requested_cwd = params.cwd.as_deref().unwrap_or(".").to_string();

    if !shell_execution_enabled() {
        return shell_error_output(
            command_sha256,
            requested_cwd,
            timeout_seconds,
            format!(
                "shell execution is disabled; restart the MCP server with {SHELL_ENABLE_ENV}=1 to opt in"
            ),
        );
    }
    if params.command.trim().is_empty() {
        return shell_error_output(
            command_sha256,
            requested_cwd,
            timeout_seconds,
            "command must not be empty",
        );
    }
    if params.command.len() > SHELL_MAX_COMMAND_BYTES {
        return shell_error_output(
            command_sha256,
            requested_cwd,
            timeout_seconds,
            format!("command exceeds the {SHELL_MAX_COMMAND_BYTES}-byte limit"),
        );
    }
    if requested_cwd.len() > SHELL_MAX_CWD_BYTES {
        return shell_error_output(
            command_sha256,
            "<rejected: cwd too long>".to_string(),
            timeout_seconds,
            format!("cwd exceeds the {SHELL_MAX_CWD_BYTES}-byte limit"),
        );
    }
    if !(1..=SHELL_MAX_TIMEOUT_SECS).contains(&timeout_seconds) {
        return shell_error_output(
            command_sha256,
            requested_cwd,
            timeout_seconds,
            format!("timeout_seconds must be between 1 and {SHELL_MAX_TIMEOUT_SECS}"),
        );
    }
    if params.env.len() > SHELL_MAX_ENV_ENTRIES {
        return shell_error_output(
            command_sha256,
            requested_cwd,
            timeout_seconds,
            format!("env contains more than {SHELL_MAX_ENV_ENTRIES} entries"),
        );
    }
    let env_bytes = params
        .env
        .iter()
        .map(|(name, value)| name.len().saturating_add(value.len()))
        .sum::<usize>();
    if env_bytes > SHELL_MAX_ENV_BYTES {
        return shell_error_output(
            command_sha256,
            requested_cwd,
            timeout_seconds,
            format!("env exceeds the {SHELL_MAX_ENV_BYTES}-byte limit"),
        );
    }
    if let Some(invalid) = params.env.keys().find(|name| !valid_environment_name(name)) {
        return shell_error_output(
            command_sha256,
            requested_cwd,
            timeout_seconds,
            format!("invalid environment variable name: {invalid}"),
        );
    }

    let cwd = match std::fs::canonicalize(&requested_cwd) {
        Ok(path) if path.is_dir() => path,
        Ok(_) => {
            return shell_error_output(
                command_sha256,
                requested_cwd,
                timeout_seconds,
                "cwd is not a directory",
            )
        }
        Err(error) => {
            return shell_error_output(
                command_sha256,
                requested_cwd,
                timeout_seconds,
                format!("failed to resolve cwd: {error}"),
            )
        }
    };
    let cwd_display = cwd.display().to_string();

    let mut child = TokioCommand::new("/bin/sh");
    // Do not use a login shell: profile scripts could reintroduce credentials
    // after env_clear() and contaminate or prevent the requested command.
    child.args(["-c", &params.command]);
    child.current_dir(&cwd);
    child.env_clear();
    let mut inherited_path = false;
    for (name, value) in inherited_shell_environment(env::vars_os()) {
        inherited_path |= name == "PATH";
        child.env(name, value);
    }
    if !inherited_path {
        child.env("PATH", "/usr/local/bin:/usr/bin:/bin");
    }
    child.envs(&params.env);

    eprintln!(
        "[computer-use-linux] run_shell start sha256={command_sha256} cwd={cwd_display:?} timeout_seconds={timeout_seconds}"
    );
    match crate::command_runner::output_with_timeout(
        child,
        "run approved shell command",
        Duration::from_secs(timeout_seconds),
    )
    .await
    {
        Ok(output) => {
            let exit_code = output.status.code();
            let (stdout, stdout_truncated) = bounded_shell_stream(&output.stdout);
            let (stderr, stderr_truncated) = bounded_shell_stream(&output.stderr);
            eprintln!(
                "[computer-use-linux] run_shell finish sha256={command_sha256} exit_code={exit_code:?} stdout_bytes={} stderr_bytes={} stdout_truncated={stdout_truncated} stderr_truncated={stderr_truncated}",
                output.stdout.len(),
                output.stderr.len()
            );
            RunShellOutput {
                ok: output.status.success(),
                command_sha256,
                cwd: cwd_display,
                timeout_seconds,
                exit_code,
                stdout,
                stderr,
                stdout_truncated,
                stderr_truncated,
                error: None,
            }
        }
        Err(error) => {
            let error = format!("{error:#}");
            eprintln!(
                "[computer-use-linux] run_shell error sha256={command_sha256} error={error:?}"
            );
            shell_error_output(command_sha256, cwd_display, timeout_seconds, error)
        }
    }
}

pub async fn serve_mcp() -> Result<()> {
    let running = ComputerUseLinux::default()
        .serve(rmcp::transport::stdio())
        .await?;
    running.waiting().await?;
    Ok(())
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
struct ListAppsOutput {
    apps: Vec<AppCandidate>,
    accessible_apps: Vec<AccessibleAppSummary>,
    accessibility_error: Option<String>,
    note: String,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
struct ListWindowsOutput {
    backend: String,
    windows: Vec<WindowInfo>,
    error: Option<String>,
    permissions_hint: Option<String>,
    note: String,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
struct FocusedWindowOutput {
    backend: String,
    focused_window: Option<WindowInfo>,
    error: Option<String>,
    permissions_hint: Option<String>,
    message: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
struct ActivateWindowParams {
    #[serde(default)]
    window_id: Option<u64>,
    #[serde(default)]
    pid: Option<u32>,
    #[serde(default)]
    tty: Option<String>,
    #[serde(default)]
    terminal_pid: Option<u32>,
    #[serde(default)]
    terminal_command: Option<String>,
    #[serde(default)]
    terminal_cwd: Option<String>,
    #[serde(default)]
    app_id: Option<String>,
    #[serde(default)]
    wm_class: Option<String>,
    #[serde(default)]
    title: Option<String>,
}

impl ActivateWindowParams {
    fn into_target(self) -> WindowTarget {
        WindowTarget {
            window_id: self.window_id,
            pid: self.pid,
            tty: self.tty,
            terminal_pid: self.terminal_pid,
            terminal_command: self.terminal_command,
            terminal_cwd: self.terminal_cwd,
            app_id: self.app_id,
            wm_class: self.wm_class,
            title: self.title,
        }
    }
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
struct ActivateWindowOutput {
    ok: bool,
    implemented: bool,
    backend: String,
    focus: Option<WindowFocusResult>,
    error: Option<String>,
    permissions_hint: Option<String>,
    // Echo of the request for debugging. `serde_json::Value` has no fixed JSON
    // schema, which strict MCP clients (Claude Code) reject in `outputSchema` —
    // and one invalid tool fails the whole tool list. Keep it in the runtime
    // response (serde) but omit it from the generated schema.
    #[schemars(skip)]
    received: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
struct RunShellParams {
    /// Shell program text passed to `/bin/sh -c` without loading login profiles.
    command: String,
    /// Existing directory in which to start the shell. Symlinks are resolved.
    #[serde(default)]
    cwd: Option<String>,
    /// Additional environment entries. The ambient process environment is not inherited wholesale.
    #[serde(default)]
    env: BTreeMap<String, String>,
    /// Wall-clock timeout in seconds (default 30, maximum 120).
    #[serde(default)]
    timeout_seconds: Option<u64>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
struct RunShellOutput {
    ok: bool,
    command_sha256: String,
    cwd: String,
    timeout_seconds: u64,
    exit_code: Option<i32>,
    stdout: String,
    stderr: String,
    stdout_truncated: bool,
    stderr_truncated: bool,
    error: Option<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
struct AppCandidate {
    name: String,
    pid: u32,
    command: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
struct OcrRegion {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
struct OcrParams {
    #[serde(default)]
    window_id: Option<u64>,
    #[serde(default)]
    pid: Option<u32>,
    #[serde(default)]
    app_id: Option<String>,
    #[serde(default)]
    wm_class: Option<String>,
    /// Window title substring.
    #[serde(default)]
    title: Option<String>,
    /// Rectangle to read, in the same pixel space as the result coordinates.
    #[serde(default)]
    region: Option<OcrRegion>,
    /// Keep only lines containing this text (case-insensitive).
    #[serde(default)]
    query: Option<String>,
    /// Tesseract language codes such as `por+eng` (default: installed por/eng).
    #[serde(default)]
    lang: Option<String>,
    /// Drop words below this confidence, 0 to 100 (default 60).
    #[serde(default)]
    min_confidence: Option<f32>,
    /// Maximum lines returned (default 300).
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
struct OcrOutput {
    ok: bool,
    /// `@x,y wxh text` per line, top to bottom by layout block.
    lines: Vec<String>,
    total_lines: usize,
    /// `window` (relative to the targeted window) or `screen`.
    coordinate_space: String,
    error: Option<String>,
}

/// Window or app scope shared by `find_elements` and `wait_for`.
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
struct ElementScope {
    /// App name or AT-SPI id that limits the search.
    #[serde(default)]
    app_name_or_bundle_identifier: Option<String>,
    #[serde(default)]
    window_id: Option<u64>,
    #[serde(default)]
    pid: Option<u32>,
    #[serde(default)]
    app_id: Option<String>,
    #[serde(default)]
    wm_class: Option<String>,
    /// Window title substring.
    #[serde(default)]
    title: Option<String>,
    /// Maximum raw AT-SPI nodes to read (default 2000, the hard max).
    #[serde(default)]
    max_nodes: Option<usize>,
}

impl ElementScope {
    fn snapshot_params(&self) -> GetAppStateParams {
        GetAppStateParams {
            app_name_or_bundle_identifier: self.app_name_or_bundle_identifier.clone(),
            window_id: self.window_id,
            pid: self.pid,
            app_id: self.app_id.clone(),
            wm_class: self.wm_class.clone(),
            title: self.title.clone(),
            max_nodes: Some(self.max_nodes.unwrap_or(2_000)),
            include_screenshot: Some(false),
            tree_format: Some(TreeFormat::Json),
            ..GetAppStateParams::default()
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
struct FindElementsParams {
    #[serde(flatten)]
    scope: ElementScope,
    #[serde(flatten)]
    query: ElementQuery,
    /// Maximum lines returned (default 25, max 200).
    #[serde(default)]
    limit: Option<usize>,
}

impl FindElementsParams {
    fn snapshot_params(&self) -> GetAppStateParams {
        self.scope.snapshot_params()
    }
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
struct FindElementsOutput {
    /// Outline lines of matching elements, in tree order.
    matches: Vec<String>,
    total_matches: usize,
    searched_nodes: usize,
    tree_truncated: bool,
    message: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum WaitCondition {
    /// An element matching the query exists.
    #[default]
    Appears,
    /// No element matches the query.
    Disappears,
    /// The scoped tree differs from the first poll.
    Changed,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
struct WaitForParams {
    #[serde(flatten)]
    scope: ElementScope,
    #[serde(flatten)]
    query: ElementQuery,
    #[serde(default)]
    condition: WaitCondition,
    /// Give up after this many milliseconds (default 5000, max 60000).
    #[serde(default)]
    timeout_ms: Option<u64>,
    /// Delay between polls in milliseconds (default 400).
    #[serde(default)]
    interval_ms: Option<u64>,
}

impl WaitForParams {
    fn snapshot_params(&self) -> GetAppStateParams {
        self.scope.snapshot_params()
    }
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
struct WaitForOutput {
    satisfied: bool,
    elapsed_ms: u64,
    polls: u32,
    /// Outline lines matching the query at the last poll (at most 25).
    matches: Vec<String>,
    message: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
struct GetAppStateParams {
    /// App name or AT-SPI id that limits the accessibility tree. Omit only when
    /// you need the whole desktop tree; unscoped results can flood context.
    #[serde(default)]
    app_name_or_bundle_identifier: Option<String>,
    /// X11 window id from list_windows. Also scopes the accessibility tree to
    /// that window's application when possible.
    #[serde(default)]
    window_id: Option<u64>,
    /// Process id. Also scopes the accessibility tree to that process when it
    /// exposes AT-SPI.
    #[serde(default)]
    pid: Option<u32>,
    /// Terminal tty device (for example /dev/pts/3). Resolves a window target
    /// and scopes the tree when possible.
    #[serde(default)]
    tty: Option<String>,
    /// Terminal emulator pid. Resolves a window target and scopes the tree when
    /// possible.
    #[serde(default)]
    terminal_pid: Option<u32>,
    /// Terminal command substring. Resolves a window target and scopes the tree
    /// when possible.
    #[serde(default)]
    terminal_command: Option<String>,
    /// Terminal working directory. Resolves a window target and scopes the tree
    /// when possible.
    #[serde(default)]
    terminal_cwd: Option<String>,
    /// Application id. Also scopes the accessibility tree when it matches an
    /// AT-SPI root.
    #[serde(default)]
    app_id: Option<String>,
    /// Window manager class. Also scopes the accessibility tree when it matches
    /// an AT-SPI root.
    #[serde(default)]
    wm_class: Option<String>,
    /// Window title substring. Also scopes the accessibility tree when it
    /// matches an AT-SPI root.
    #[serde(default)]
    title: Option<String>,
    /// Maximum raw AT-SPI nodes to inspect before compaction (default 1000, hard max 2000).
    #[serde(default)]
    max_nodes: Option<usize>,
    /// Maximum AT-SPI traversal depth (default 32, hard max 64).
    #[serde(default)]
    max_depth: Option<u32>,
    /// `outline` (default) returns one text line per element in
    /// `accessibility_outline`; `json` returns full node objects in
    /// `accessibility_tree`.
    #[serde(default)]
    tree_format: Option<TreeFormat>,
    /// Include a size-bounded screenshot (default true). Set false when the
    /// accessibility tree is enough.
    #[serde(default)]
    include_screenshot: Option<bool>,
    /// Maximum returned screenshot width in pixels (default 1920, hard-capped).
    #[serde(default)]
    max_width: Option<u32>,
    /// Maximum returned screenshot height in pixels (default 1920, hard-capped).
    #[serde(default)]
    max_height: Option<u32>,
    /// Maximum returned screenshot image bytes before base64 (default 2 MiB, hard-capped).
    #[serde(default)]
    max_bytes: Option<usize>,
    /// Additional downscale factor from 0.0 to 1.0, applied before max dimensions.
    #[serde(default)]
    scale: Option<f32>,
    /// Output image format (default png). Use jpeg with quality to trade exact pixels for smaller payloads.
    #[serde(default)]
    format: Option<ScreenshotOutputFormat>,
    /// JPEG quality from 1 to 95 (default 80). Ignored for png.
    #[serde(default)]
    #[schemars(range(min = 1, max = 95))]
    quality: Option<u8>,
    /// Include the full diagnostics report (large). Default false: only the
    /// compact readiness block is returned.
    #[serde(default)]
    verbose: Option<bool>,
}

impl GetAppStateParams {
    fn window_target(&self) -> WindowTarget {
        WindowTarget {
            window_id: self.window_id,
            pid: self.pid,
            tty: self.tty.clone(),
            terminal_pid: self.terminal_pid,
            terminal_command: self.terminal_command.clone(),
            terminal_cwd: self.terminal_cwd.clone(),
            app_id: self.app_id.clone(),
            wm_class: self.wm_class.clone(),
            title: self.title.clone(),
        }
    }

    fn screenshot_options(&self) -> ScreenshotPayloadOptions {
        ScreenshotPayloadOptions {
            max_width: self.max_width,
            max_height: self.max_height,
            max_bytes: self.max_bytes,
            scale: self.scale,
            format: self.format,
            quality: self.quality,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
struct ScreenshotParams {
    #[serde(default)]
    window_id: Option<u64>,
    #[serde(default)]
    pid: Option<u32>,
    #[serde(default)]
    app_id: Option<String>,
    #[serde(default)]
    wm_class: Option<String>,
    #[serde(default)]
    title: Option<String>,
    /// Capture the whole desktop even when a window is targeted (default false).
    #[serde(default)]
    full_screen: Option<bool>,
    /// Maximum returned screenshot width in pixels (default 1920, hard-capped).
    #[serde(default)]
    max_width: Option<u32>,
    /// Maximum returned screenshot height in pixels (default 1920, hard-capped).
    #[serde(default)]
    max_height: Option<u32>,
    /// Maximum returned screenshot image bytes before base64 (default 2 MiB, hard-capped).
    #[serde(default)]
    max_bytes: Option<usize>,
    /// Additional downscale factor from 0.0 to 1.0, applied before max dimensions.
    #[serde(default)]
    scale: Option<f32>,
    /// Output image format (default png). Use jpeg with quality to trade exact pixels for smaller payloads.
    #[serde(default)]
    format: Option<ScreenshotOutputFormat>,
    /// JPEG quality from 1 to 95 (default 80). Ignored for png.
    #[serde(default)]
    #[schemars(range(min = 1, max = 95))]
    quality: Option<u8>,
}

impl ScreenshotParams {
    fn window_target(&self) -> Option<WindowTarget> {
        if self.window_id.is_none()
            && self.pid.is_none()
            && self.app_id.is_none()
            && self.wm_class.is_none()
            && self.title.is_none()
        {
            return None;
        }
        Some(WindowTarget {
            window_id: self.window_id,
            pid: self.pid,
            tty: None,
            terminal_pid: None,
            terminal_command: None,
            terminal_cwd: None,
            app_id: self.app_id.clone(),
            wm_class: self.wm_class.clone(),
            title: self.title.clone(),
        })
    }

    fn screenshot_options(&self) -> ScreenshotPayloadOptions {
        ScreenshotPayloadOptions {
            max_width: self.max_width,
            max_height: self.max_height,
            max_bytes: self.max_bytes,
            scale: self.scale,
            format: self.format,
            quality: self.quality,
        }
    }
}

/// Screenshot metadata for `get_app_state`. The image bytes travel as a
/// separate `image` content block (issue #145), not inside this JSON.
#[derive(Debug, Clone, Serialize, JsonSchema)]
struct ScreenshotSummary {
    mime_type: String,
    source: String,
    /// Width of the returned image payload.
    width: u32,
    /// Height of the returned image payload.
    height: u32,
    /// Coordinate-space width before payload downscaling.
    coordinate_width: u32,
    /// Coordinate-space height before payload downscaling.
    coordinate_height: u32,
    /// Returned pixels per coordinate-space pixel.
    scale: f32,
    resized: bool,
    bytes: usize,
    original_bytes: usize,
    max_bytes: usize,
    format: ScreenshotOutputFormat,
    quality: Option<u8>,
}

impl From<&ScreenshotCapture> for ScreenshotSummary {
    fn from(capture: &ScreenshotCapture) -> Self {
        Self {
            mime_type: capture.mime_type.clone(),
            source: capture.source.clone(),
            width: capture.width,
            height: capture.height,
            coordinate_width: capture.coordinate_width,
            coordinate_height: capture.coordinate_height,
            scale: capture.scale,
            resized: capture.resized,
            bytes: capture.bytes,
            original_bytes: capture.original_bytes,
            max_bytes: capture.max_bytes,
            format: capture.format,
            quality: capture.quality,
        }
    }
}

/// Assemble the `get_app_state` result: the screenshot (when present) as a
/// structured image block first, then the JSON report as text and as
/// `structured_content`. Inlining the base64 in the JSON made hosts treat a
/// 100KB+ image as plain text tokens (issue #145).
fn app_state_result(
    output: GetAppStateOutput,
    screenshot: Option<ScreenshotCapture>,
) -> Result<CallToolResult, ErrorData> {
    let value = serde_json::to_value(&output).map_err(|error| {
        ErrorData::internal_error(
            format!("failed to serialize get_app_state output: {error}"),
            None,
        )
    })?;
    let mut result = CallToolResult::structured(value);
    if let Some(capture) = screenshot {
        result.content.insert(
            0,
            ContentBlock::image(data_url_payload(&capture.data_url), capture.mime_type),
        );
    }
    Ok(result)
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
struct GetAppStateOutput {
    app_name_or_bundle_identifier: Option<String>,
    window_context: Option<WindowInfo>,
    window_error: Option<String>,
    window_permissions_hint: Option<String>,
    backend: String,
    /// Screenshot metadata; the image itself is the first `image` content block.
    screenshot: Option<ScreenshotSummary>,
    screenshot_error: Option<String>,
    /// Full node objects; empty unless `tree_format` is `json`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    accessibility_tree: Vec<AccessibilityNode>,
    /// One line per element, `[index] role "name" @x,y wxh {states} actions=...`,
    /// indented by tree depth. The index is what element-targeted tools take.
    #[serde(skip_serializing_if = "Option::is_none")]
    accessibility_outline: Option<String>,
    accessibility_tree_raw_count: usize,
    /// True when the snapshot was limited to selected app roots rather than the
    /// full desktop AT-SPI registry.
    tree_scoped: bool,
    /// True when max_nodes, max_depth, or the child read budget stopped
    /// traversal with unread elements left, so the tree is incomplete. Recover
    /// by scoping to a narrower app or window target and raising max_nodes or
    /// max_depth (hard caps 2000 and 64). Failed element reads do not count.
    accessibility_tree_truncated: bool,
    accessibility_error: Option<String>,
    /// Compact readiness summary (always present).
    readiness: crate::diagnostics::ReadinessReport,
    /// Full diagnostics; populated only when verbose=true.
    #[serde(skip_serializing_if = "Option::is_none")]
    diagnostics: Option<DoctorReport>,
    message: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
struct ClickParams {
    #[serde(default)]
    element_index: Option<u32>,
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    states: Vec<String>,
    #[serde(default)]
    x: Option<i32>,
    #[serde(default)]
    y: Option<i32>,
    #[serde(default)]
    button: Option<String>,
    #[serde(default)]
    click_count: Option<u32>,
    // Optional window target: when set, the window is raised/focused before the
    // click so a coordinate click reliably lands on the intended app rather than
    // whatever window happens to be stacked on top at that pixel.
    #[serde(default)]
    window_id: Option<u64>,
    #[serde(default)]
    pid: Option<u32>,
    #[serde(default)]
    app_id: Option<String>,
    #[serde(default)]
    wm_class: Option<String>,
    #[serde(default)]
    window_title: Option<String>,
    /// Interpret `x`/`y` from the clipped window screenshot crop origin, in
    /// coordinate pixels before preview resizing. Divide preview pixels by its
    /// scale first. This is not a toolkit widget or raw GDK surface origin.
    /// Requires a window target; missing targets are rejected.
    #[serde(default)]
    relative: Option<bool>,
}

impl ClickParams {
    /// A window target if any window-identifying field was supplied.
    fn window_target(&self) -> Option<WindowTarget> {
        if self.window_id.is_none()
            && self.pid.is_none()
            && self.app_id.is_none()
            && self.wm_class.is_none()
            && self.window_title.is_none()
        {
            return None;
        }
        Some(WindowTarget {
            window_id: self.window_id,
            pid: self.pid,
            tty: None,
            terminal_pid: None,
            terminal_command: None,
            terminal_cwd: None,
            app_id: self.app_id.clone(),
            wm_class: self.wm_class.clone(),
            title: self.window_title.clone(),
        })
    }

    fn selector(&self) -> ElementSelector<'_> {
        ElementSelector {
            role: self.role.as_deref(),
            name: self.name.as_deref(),
            text: self.text.as_deref(),
            states: &self.states,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
struct ActionParams {
    #[serde(default)]
    element_index: Option<u32>,
    #[serde(default)]
    element_identifier: Option<String>,
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    states: Vec<String>,
    #[serde(default)]
    action: Option<String>,
}

impl ActionParams {
    fn selector(&self) -> ElementSelector<'_> {
        ElementSelector {
            role: self.role.as_deref(),
            name: self.name.as_deref(),
            text: self.text.as_deref(),
            states: &self.states,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
struct SetValueParams {
    #[serde(default)]
    element_index: Option<u32>,
    #[serde(default)]
    element_identifier: Option<String>,
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    states: Vec<String>,
    value: String,
}

impl SetValueParams {
    fn selector(&self) -> ElementSelector<'_> {
        ElementSelector {
            role: self.role.as_deref(),
            name: self.name.as_deref(),
            text: self.text.as_deref(),
            states: &self.states,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
struct ScrollParams {
    #[serde(default)]
    element_index: Option<u32>,
    #[serde(default)]
    x: Option<i32>,
    #[serde(default)]
    y: Option<i32>,
    direction: String,
    #[serde(default)]
    #[schemars(range(min = -100, max = 100))]
    pages: Option<f64>,
    // Optional window target (parity with click): the window is raised/focused
    // before scrolling so the wheel events land on the intended app.
    #[serde(default)]
    window_id: Option<u64>,
    #[serde(default)]
    pid: Option<u32>,
    #[serde(default)]
    app_id: Option<String>,
    #[serde(default)]
    wm_class: Option<String>,
    #[serde(default)]
    window_title: Option<String>,
    /// Interpret `x`/`y` from the clipped window screenshot crop origin, in
    /// coordinate pixels before preview resizing. Divide preview pixels by its
    /// scale first. This is not a toolkit widget or raw GDK surface origin.
    /// Requires a window target; missing targets are rejected.
    #[serde(default)]
    relative: Option<bool>,
}

fn scroll_units(pages: Option<f64>) -> std::result::Result<i32, String> {
    let pages = pages.unwrap_or(1.0);
    if !pages.is_finite() || pages.abs() > MAX_SCROLL_PAGES {
        return Err(format!(
            "pages must be finite and have an absolute value no greater than {MAX_SCROLL_PAGES}"
        ));
    }
    Ok(((pages.abs().max(0.1) * 5.0).round() as i32).max(1))
}

impl ScrollParams {
    /// A window target if any window-identifying field was supplied.
    fn window_target(&self) -> Option<WindowTarget> {
        if self.window_id.is_none()
            && self.pid.is_none()
            && self.app_id.is_none()
            && self.wm_class.is_none()
            && self.window_title.is_none()
        {
            return None;
        }
        Some(WindowTarget {
            window_id: self.window_id,
            pid: self.pid,
            tty: None,
            terminal_pid: None,
            terminal_command: None,
            terminal_cwd: None,
            app_id: self.app_id.clone(),
            wm_class: self.wm_class.clone(),
            title: self.window_title.clone(),
        })
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
struct DragParams {
    start_x: i32,
    start_y: i32,
    end_x: i32,
    end_y: i32,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
struct PressKeyParams {
    key: String,
    #[serde(default)]
    window_id: Option<u64>,
    #[serde(default)]
    pid: Option<u32>,
    #[serde(default)]
    tty: Option<String>,
    #[serde(default)]
    terminal_pid: Option<u32>,
    #[serde(default)]
    terminal_command: Option<String>,
    #[serde(default)]
    terminal_cwd: Option<String>,
    #[serde(default)]
    app_id: Option<String>,
    #[serde(default)]
    wm_class: Option<String>,
    #[serde(default)]
    title: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
struct TypeTextParams {
    text: String,
    #[serde(default)]
    window_id: Option<u64>,
    #[serde(default)]
    pid: Option<u32>,
    #[serde(default)]
    tty: Option<String>,
    #[serde(default)]
    terminal_pid: Option<u32>,
    #[serde(default)]
    terminal_command: Option<String>,
    #[serde(default)]
    terminal_cwd: Option<String>,
    #[serde(default)]
    app_id: Option<String>,
    #[serde(default)]
    wm_class: Option<String>,
    #[serde(default)]
    title: Option<String>,
}

impl PressKeyParams {
    fn window_target(&self) -> WindowTarget {
        WindowTarget {
            window_id: self.window_id,
            pid: self.pid,
            tty: self.tty.clone(),
            terminal_pid: self.terminal_pid,
            terminal_command: self.terminal_command.clone(),
            terminal_cwd: self.terminal_cwd.clone(),
            app_id: self.app_id.clone(),
            wm_class: self.wm_class.clone(),
            title: self.title.clone(),
        }
    }
}

impl TypeTextParams {
    fn window_target(&self) -> WindowTarget {
        WindowTarget {
            window_id: self.window_id,
            pid: self.pid,
            tty: self.tty.clone(),
            terminal_pid: self.terminal_pid,
            terminal_command: self.terminal_command.clone(),
            terminal_cwd: self.terminal_cwd.clone(),
            app_id: self.app_id.clone(),
            wm_class: self.wm_class.clone(),
            title: self.title.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
struct ActionOutput {
    ok: bool,
    implemented: bool,
    action: String,
    message: String,
    // See ActivateWindowOutput: kept in the response, omitted from the schema
    // because `serde_json::Value` produces a non-object schema strict MCP
    // clients reject.
    #[schemars(skip)]
    received: Option<serde_json::Value>,
}

impl ComputerUseLinux {
    async fn resolve_window_context(
        &self,
        params: &GetAppStateParams,
    ) -> (Option<WindowInfo>, Option<String>, Option<String>) {
        let target = params.window_target();
        if !target.has_target() {
            return (None, None, None);
        }

        match list_windows().await {
            Ok(windows) => match resolve_window_target(&windows, &target) {
                Ok(window) => (Some(window.clone()), None, None),
                Err(error) => (None, Some(format!("{error:#}")), None),
            },
            Err(error) => {
                let error = format!("{error:#}");
                let hint = window_permission_hint(&error);
                (None, Some(error), hint)
            }
        }
    }

    /// Focus the targeted window and return its verified, visible record.
    async fn resolve_screenshot_window(&self, target: &WindowTarget) -> Result<WindowInfo> {
        let focus = focus_window_target(target).await?;
        if !focus.exact_window_focused {
            anyhow::bail!(
                "the requested window could not be focused exactly; refusing to capture unrelated desktop pixels"
            );
        }
        sleep(Duration::from_millis(250)).await;
        let window = focus
            .focused_window
            .filter(|window| window.window_id == focus.requested_window.window_id)
            .ok_or_else(|| anyhow::anyhow!("focused-window verification returned no window"))?;
        if window.hidden {
            anyhow::bail!("the requested window is hidden or minimized");
        }
        Ok(window)
    }

    /// The focused window's crop in screenshot coordinates, clipped to the
    /// root window when its size is known.
    async fn focused_window_capture_rect(
        &self,
        focus: &WindowFocusResult,
    ) -> std::result::Result<(i32, i32, u32, u32), String> {
        let window = focus
            .focused_window
            .as_ref()
            .unwrap_or(&focus.requested_window);
        let full_capture_rect = window
            .bounds
            .as_ref()
            .and_then(window_crop_rect)
            .ok_or_else(|| {
                "Window-relative coordinates require usable target-window bounds.".to_string()
            })?;
        match self.desktop_size().await {
            Some((width, height)) => clip_capture_rect(full_capture_rect, width, height)
                .map_err(|error| format!("Could not map target-window coordinates: {error:#}")),
            None => Ok(full_capture_rect),
        }
    }

    async fn resolve_accessibility_app_filter(
        &self,
        params: &GetAppStateParams,
        window_context: Option<&WindowInfo>,
    ) -> Option<String> {
        if let Some(explicit) = trimmed_nonempty(params.app_name_or_bundle_identifier.as_deref()) {
            return Some(explicit.to_string());
        }

        let target_pid = window_context.and_then(|window| window.pid).or(params.pid);
        let candidates = accessibility_filter_candidates(window_context);

        if let Some(target_pid) = target_pid {
            if let Ok(apps) = list_accessible_apps(200).await {
                if let Some(object_ref) =
                    select_accessibility_object_ref(&apps, target_pid, &candidates)
                {
                    return Some(object_ref);
                }
            }
        }

        candidates.into_iter().next()
    }

    async fn focus_target_for_input(
        &self,
        target: &WindowTarget,
    ) -> std::result::Result<Option<WindowFocusResult>, String> {
        if !target.has_target() {
            return Ok(None);
        }

        let focus = focus_window_target(target).await.map_err(|error| {
            let error = format!("{error:#}");
            if let Some(hint) = window_permission_hint(&error) {
                format!("Did not send input because the target window could not be focused: {error}. {hint}")
            } else {
                format!("Did not send input because the target window could not be focused: {error}")
            }
        })?;

        if focus_satisfies_target(&focus, target) {
            Ok(Some(focus))
        } else {
            let required = if target.requires_exact_focus() {
                "exact target-window focus"
            } else {
                "app-level focus"
            };
            Err(format!(
                "Did not send input because {required} verification failed after activating the target window. Focus result: requested window_id {}, focused window_id {:?}.",
                focus.requested_window.window_id,
                focus.focused_window.as_ref().map(|window| window.window_id)
            ))
        }
    }

    async fn run_ocr(
        &self,
        params: &OcrParams,
    ) -> Result<(Vec<crate::ocr::OcrLine>, &'static str)> {
        let target = WindowTarget {
            window_id: params.window_id,
            pid: params.pid,
            app_id: params.app_id.clone(),
            wm_class: params.wm_class.clone(),
            title: params.title.clone(),
            ..WindowTarget::default()
        };
        let window = if target.has_target() {
            Some(
                self.resolve_screenshot_window(&target)
                    .await
                    .context("window OCR could not focus the target")?,
            )
        } else {
            None
        };
        let raw = {
            let _pipeline = crate::screenshot_impl::screenshot_pipeline_permit().await?;
            let raw = capture_screenshot_raw().await;
            self.record_screenshot_result(raw.is_ok());
            raw.context("screenshot failed")?
        };
        self.cache_desktop_size(raw.width, raw.height);
        // Window OCR reads the window's crop, so line coordinates are
        // window-relative like a cropped screenshot.
        let (raw, space) = match window {
            Some(window) => {
                let (x, y, width, height) = window_capture_rect(&window, raw.width, raw.height)?;
                let bytes = raw.bytes;
                let (cropped, width, height) = crate::screenshot_impl::run_image_task(move || {
                    crop_png(&bytes, x, y, width, height)
                        .map_err(|error| anyhow::anyhow!("window OCR crop failed: {error}"))
                })
                .await?;
                (
                    RawScreenshotCapture {
                        mime_type: raw.mime_type,
                        bytes: cropped,
                        source: raw.source,
                        width,
                        height,
                    },
                    "window",
                )
            }
            None => (raw, "screen"),
        };
        let (png, dx, dy) = match &params.region {
            Some(region) => {
                let (x, y, width, height) = clip_capture_rect(
                    (region.x, region.y, region.width, region.height),
                    raw.width,
                    raw.height,
                )?;
                let bytes = raw.bytes;
                let (cropped, _, _) = crate::screenshot_impl::run_image_task(move || {
                    crop_png(&bytes, x, y, width, height)
                        .map_err(|error| anyhow::anyhow!("OCR region crop failed: {error}"))
                })
                .await?;
                (cropped, x, y)
            }
            None => (raw.bytes, 0, 0),
        };
        let lines = crate::ocr::recognize(
            png,
            params.lang.as_deref(),
            params.min_confidence.unwrap_or(60.0),
        )
        .await?;
        Ok((
            lines.into_iter().map(|line| line.offset(dx, dy)).collect(),
            space,
        ))
    }

    fn record_screenshot_result(&self, succeeded: bool) {
        if let Ok(mut guard) = self.last_screenshot_ok.lock() {
            *guard = Some(succeeded);
        }
    }

    /// `doctor_report` with the screenshot verdict from the latest real capture.
    async fn diagnostics(&self) -> DoctorReport {
        let mut report = tokio::task::spawn_blocking(doctor_report)
            .await
            .expect("diagnostics task panicked");
        if let Ok(mut guard) = self.recent_diagnostics.lock() {
            *guard = Some((std::time::Instant::now(), report.clone()));
        }
        let observed = self.last_screenshot_ok.lock().ok().and_then(|guard| *guard);
        if let Some(succeeded) = observed {
            report.readiness.record_screenshot_result(succeeded);
        }
        report
    }

    /// Like `diagnostics`, but reuses a report up to 15 s old.
    async fn cached_diagnostics(&self) -> DoctorReport {
        let cached = self.recent_diagnostics.lock().ok().and_then(|guard| {
            guard
                .as_ref()
                .filter(|(at, _)| at.elapsed() < Duration::from_secs(15))
                .map(|(_, report)| report.clone())
        });
        match cached {
            Some(mut report) => {
                let observed = self.last_screenshot_ok.lock().ok().and_then(|guard| *guard);
                if let Some(succeeded) = observed {
                    report.readiness.record_screenshot_result(succeeded);
                }
                report
            }
            None => self.diagnostics().await,
        }
    }

    fn cache_desktop_size(&self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        if let Ok(mut guard) = self.desktop_size.lock() {
            *guard = Some((width, height));
        }
    }

    /// Root window size: the cached capture size, else one cheap X11 query.
    async fn desktop_size(&self) -> Option<(u32, u32)> {
        if let Some(size) = self.desktop_size.lock().ok().and_then(|guard| *guard) {
            return Some(size);
        }
        let (width, height) = crate::x11_display::with_x11_display(
            crate::x11_display::X11_QUERY_TIMEOUT,
            |display| display.root_size(),
        )
        .await
        .ok()?;
        self.cache_desktop_size(width, height);
        (width > 0 && height > 0).then_some((width, height))
    }

    /// Warn when a targeted window pokes outside every monitor: clicks and
    /// screenshots silently truncate to visible pixels there, which reads as
    /// "success" while landing nowhere.
    async fn off_screen_note_for_bounds(
        &self,
        bounds: &crate::windowing::WindowBounds,
    ) -> Option<String> {
        let (x, y) = bounds.x.zip(bounds.y)?;
        if bounds.width == 0 || bounds.height == 0 {
            return None;
        }
        let (mw, mh) = self.desktop_size().await?;
        let (w, h) = (bounds.width as i64, bounds.height as i64);
        let window_area = w * h;
        let ix = (x as i64).max(0);
        let iy = (y as i64).max(0);
        let ix2 = (x as i64 + w).min(mw as i64);
        let iy2 = (y as i64 + h).min(mh as i64);
        let visible_area = if ix2 > ix && iy2 > iy {
            (ix2 - ix) * (iy2 - iy)
        } else {
            0
        };
        let visible_pct = (visible_area.min(window_area) * 100) / window_area.max(1);
        if visible_pct >= 100 {
            return None;
        }
        Some(format!(
            "WARNING: the target window (bounds {x},{y} {w}x{h}) is only ~{visible_pct}% on-screen; off-screen regions are missing from screenshots and unreachable by coordinate input."
        ))
    }

    /// Warn when a click/scroll coordinate is outside the root window.
    async fn off_screen_note_for_point(&self, x: i32, y: i32) -> Option<String> {
        let (mw, mh) = self.desktop_size().await?;
        let visible = x >= 0 && y >= 0 && (x as u32) < mw && (y as u32) < mh;
        if visible {
            return None;
        }
        Some(format!(
            "WARNING: coordinate {x},{y} is outside the captured desktop ({mw}x{mh}); the input landed on no visible pixel."
        ))
    }

    /// Post-input feedback: which AT-SPI element holds keyboard focus in the
    /// target app, and whether it is editable. Guards against the blind-typing
    /// trap where verified *window* focus still sends keystrokes nowhere.
    async fn focused_element_feedback(
        &self,
        focus: Option<&WindowFocusResult>,
        expects_editable: bool,
    ) -> Option<String> {
        let focus = focus?;
        let pid = focus
            .focused_window
            .as_ref()
            .and_then(|window| window.pid)
            .or(focus.requested_window.pid);
        match timeout(Duration::from_millis(1500), probe_focused_element(pid)).await {
            Ok(Ok(probe)) => Some(focus_probe_feedback(
                &probe,
                expects_editable,
                focus.exact_window_focused,
            )),
            Ok(Err(error)) => Some(format!(
                "Focused-element feedback unavailable ({}).",
                first_line(&format!("{error:#}"))
            )),
            Err(_) => {
                Some("Focused-element feedback unavailable (AT-SPI probe timed out).".to_string())
            }
        }
    }

    /// Notes appended after targeted keyboard input: off-screen window warning
    /// plus focused-element feedback.
    async fn input_landing_notes(
        &self,
        focus: Option<&WindowFocusResult>,
        expects_editable: bool,
    ) -> Vec<String> {
        let mut notes = Vec::new();
        if let Some(focus) = focus {
            let bounds = focus
                .focused_window
                .as_ref()
                .and_then(|window| window.bounds.as_ref())
                .or(focus.requested_window.bounds.as_ref());
            if let Some(bounds) = bounds {
                if let Some(note) = self.off_screen_note_for_bounds(bounds).await {
                    notes.push(note);
                }
            }
        }
        if let Some(note) = self.focused_element_feedback(focus, expects_editable).await {
            notes.push(note);
        }
        notes
    }

    #[cfg(test)]
    fn cache_nodes(&self, nodes: &[AccessibilityNode]) {
        self.cache_snapshot(nodes, None);
    }

    /// Readers may perform slow AT-SPI owner checks before resolving a click
    /// or scroll again. Keep the snapshot stable for the whole input operation;
    /// otherwise a checked index could resolve to a different app's node.
    async fn commit_snapshot(&self, nodes: &[AccessibilityNode], target_pid: Option<u32>) {
        let _input_guard = Arc::clone(&self.input_operation_lock).lock_owned().await;
        self.cache_snapshot(nodes, target_pid);
    }

    fn cache_snapshot(&self, nodes: &[AccessibilityNode], target_pid: Option<u32>) {
        if let Ok(mut cached) = self.last_snapshot.lock() {
            *cached = CachedAccessibilitySnapshot {
                nodes: nodes.to_vec(),
                pid: target_pid,
            };
        }
    }

    #[cfg(test)]
    fn clear_cached_nodes(&self) {
        self.cache_snapshot(&[], None);
    }

    /// Reject an element-targeted action whose cached node belongs to a
    /// different app than the action's target. A snapshot taken for a pid is
    /// judged by that pid; an untargeted snapshot mixes apps, so its node is
    /// judged by the pid that owns it on the accessibility bus. When neither is
    /// known the action proceeds, since a mismatch cannot be shown.
    async fn check_node_target(
        &self,
        node: &AccessibilityNode,
        target_pid: Option<u32>,
    ) -> std::result::Result<(), String> {
        let Some(target_pid) = target_pid else {
            return Ok(());
        };
        let snapshot_pid = self
            .last_snapshot
            .lock()
            .ok()
            .and_then(|snapshot| snapshot.pid);
        let owner_pid = match snapshot_pid {
            Some(pid) => Some(pid),
            None => object_ref_owner_pid(&node.object_ref).await.ok().flatten(),
        };
        match owner_pid {
            Some(owner_pid) if owner_pid != target_pid => Err(node_target_mismatch_message(
                node.index, owner_pid, target_pid,
            )),
            _ => Ok(()),
        }
    }

    /// The cached node an element-targeted click or scroll would act on, when
    /// the call names one. Resolution errors are left to the tool's own path.
    fn cached_node_for(
        &self,
        element_index: Option<u32>,
        selector: &ElementSelector<'_>,
        purpose: ElementResolvePurpose,
    ) -> Option<AccessibilityNode> {
        if element_index.is_none() && selector.is_empty() {
            return None;
        }
        self.resolve_cached_node(element_index, selector, purpose)
            .ok()
    }

    fn resolve_optional_target_point(
        &self,
        x: Option<i32>,
        y: Option<i32>,
        element_index: Option<u32>,
    ) -> std::result::Result<Option<(i32, i32)>, String> {
        match (x.zip(y), element_index) {
            (Some(point), _) => Ok(Some(point)),
            (None, Some(index)) => self
                .center_for_cached_node(index)
                .map(Some)
                .ok_or_else(|| {
                    format!(
                        "No clickable bounds cached for element_index {index}. Call get_app_state first and choose a node with positive width and height."
                    )
                }),
            (None, None) => Ok(None),
        }
    }

    fn resolve_click_target(
        &self,
        params: &ClickParams,
    ) -> std::result::Result<ClickTarget, String> {
        if let Some((x, y)) = params.x.zip(params.y) {
            return Ok(ClickTarget::Coordinates(x, y));
        }

        let selector = params.selector();
        let node = self.resolve_cached_node(
            params.element_index,
            &selector,
            ElementResolvePurpose::Click,
        )?;

        // Toolkit extents are not necessarily in the pointer backend's space
        // (GTK3 X11 HiDPI, GTK4 zero-origin bounds). Ordinary activation does
        // not need a guessed scale when the target exposes its native action.
        if is_plain_left_click(params.button.as_deref(), params.click_count) {
            if let Some(action) = primary_click_action(&node.actions) {
                return Ok(ClickTarget::PrimaryAction {
                    object_ref: node.object_ref.clone(),
                    action_name: Some(action.name.clone()),
                    action_index: action.index,
                });
            }
        }

        if let Some((x, y)) = node.bounds.as_ref().and_then(bounds_center) {
            return Ok(ClickTarget::Coordinates(x, y));
        }

        if !is_plain_left_click(params.button.as_deref(), params.click_count) {
            return Err(format!(
                "No clickable bounds cached for element_index {}. Call get_app_state first and choose a node with positive width and height.",
                node.index
            ));
        }

        Err(format!(
            "No clickable bounds or click-equivalent AT-SPI action for element_index {}. Use perform_action explicitly for other actions.",
            node.index
        ))
    }

    fn center_for_cached_node(&self, element_index: u32) -> Option<(i32, i32)> {
        let cached = self.last_snapshot.lock().ok()?;
        let node = cached
            .nodes
            .iter()
            .find(|node| node.index == element_index)?;
        bounds_center(node.bounds.as_ref()?)
    }

    fn resolve_object_ref(
        &self,
        element_index: Option<u32>,
        element_identifier: Option<&str>,
        selector: &ElementSelector<'_>,
        purpose: ElementResolvePurpose,
    ) -> std::result::Result<String, String> {
        if let Some(element_identifier) = element_identifier
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return Ok(element_identifier.to_string());
        }

        self.resolve_cached_node(element_index, selector, purpose)
            .map(|node| node.object_ref)
    }

    fn resolve_cached_node(
        &self,
        element_index: Option<u32>,
        selector: &ElementSelector<'_>,
        purpose: ElementResolvePurpose,
    ) -> std::result::Result<AccessibilityNode, String> {
        let cached = self.last_snapshot.lock().map_err(|_| {
            "Could not read cached accessibility nodes. Call get_app_state and retry.".to_string()
        })?;

        if let Some(element_index) = element_index {
            return cached.nodes
                .iter()
                .find(|node| node.index == element_index)
                .cloned()
                .ok_or_else(|| {
                    format!(
                        "No cached accessibility node for element_index {element_index}. Call get_app_state first."
                    )
                });
        }

        if selector.is_empty() {
            return Err(
                "Pass element_index, element_identifier, or a semantic selector such as role/name/text/states from the latest get_app_state result."
                    .to_string(),
            );
        }

        resolve_semantic_node(cached.nodes.as_slice(), selector, purpose)
    }

    async fn perform_element_action(
        &self,
        params: &ActionParams,
        requested_action: Option<&str>,
    ) -> Json<ActionOutput> {
        let _input_guard = Arc::clone(&self.input_operation_lock).lock_owned().await;
        let received = Some(serde_json::json!(params.clone()));
        let object_ref = match self.resolve_object_ref(
            params.element_index,
            params.element_identifier.as_deref(),
            &params.selector(),
            ElementResolvePurpose::Action,
        ) {
            Ok(object_ref) => object_ref,
            Err(message) => {
                return Json(ActionOutput {
                    ok: false,
                    implemented: true,
                    action: "perform_action".to_string(),
                    message,
                    received,
                });
            }
        };

        match invoke_accessibility_action(&object_ref, requested_action).await {
            Ok(invocation) => Json(ActionOutput {
                ok: invocation.ok,
                implemented: true,
                action: "perform_action".to_string(),
                message: if invocation.ok {
                    format!(
                        "AT-SPI action {} ({}) invoked.",
                        invocation.action_index,
                        invocation
                            .action_name
                            .as_deref()
                            .filter(|name| !name.is_empty())
                            .unwrap_or("unnamed")
                    )
                } else {
                    format!(
                        "AT-SPI action {} ({}) returned false.",
                        invocation.action_index,
                        invocation
                            .action_name
                            .as_deref()
                            .filter(|name| !name.is_empty())
                            .unwrap_or("unnamed")
                    )
                },
                received,
            }),
            Err(error) => Json(ActionOutput {
                ok: false,
                implemented: true,
                action: "perform_action".to_string(),
                message: error.to_string(),
                received,
            }),
        }
    }
}

#[derive(Debug)]
enum ClickTarget {
    Coordinates(i32, i32),
    PrimaryAction {
        object_ref: String,
        action_name: Option<String>,
        action_index: i32,
    },
}

#[derive(Debug, Clone, Copy)]
enum ElementResolvePurpose {
    Click,
    Action,
    SetValue,
}

#[derive(Debug, Clone, Copy, Default)]
struct ElementSelector<'a> {
    role: Option<&'a str>,
    name: Option<&'a str>,
    text: Option<&'a str>,
    states: &'a [String],
}

impl ElementSelector<'_> {
    fn is_empty(&self) -> bool {
        [self.role, self.name, self.text]
            .into_iter()
            .all(|value| value.map(str::trim).is_none_or(str::is_empty))
            && self.states.iter().all(|value| value.trim().is_empty())
    }
}

fn resolve_semantic_node(
    nodes: &[AccessibilityNode],
    selector: &ElementSelector<'_>,
    purpose: ElementResolvePurpose,
) -> std::result::Result<AccessibilityNode, String> {
    let mut matches = nodes
        .iter()
        .filter(|node| node_matches_selector(node, selector))
        .collect::<Vec<_>>();

    if matches.is_empty() {
        return Err(format!(
            "No cached accessibility node matched semantic selector {}. Call get_app_state first or pass element_index.",
            describe_selector(selector)
        ));
    }

    if let Some(node) =
        unique_preferred_node(&matches, |node| node_matches_resolve_purpose(node, purpose))
    {
        return Ok(node.clone());
    }

    let useful_matches = matches
        .iter()
        .copied()
        .filter(|node| node_matches_resolve_purpose(node, purpose))
        .collect::<Vec<_>>();
    if !useful_matches.is_empty() {
        matches = useful_matches;
    }

    if let Some(node) = unique_preferred_node(&matches, node_is_showing) {
        return Ok(node.clone());
    }

    let visible_matches = matches
        .iter()
        .copied()
        .filter(|node| node_is_showing(node))
        .collect::<Vec<_>>();
    if !visible_matches.is_empty() {
        matches = visible_matches;
    }

    if matches.len() == 1 {
        return Ok(matches[0].clone());
    }

    Err(format!(
        "Semantic selector {} matched multiple cached nodes: {}. Pass element_index or add more selector fields.",
        describe_selector(selector),
        describe_matching_nodes(&matches),
    ))
}

fn unique_preferred_node<'a>(
    nodes: &[&'a AccessibilityNode],
    predicate: impl Fn(&AccessibilityNode) -> bool,
) -> Option<&'a AccessibilityNode> {
    let mut matches = nodes.iter().copied().filter(|node| predicate(node));
    let first = matches.next()?;
    matches.next().is_none().then_some(first)
}

fn node_matches_selector(node: &AccessibilityNode, selector: &ElementSelector<'_>) -> bool {
    selector
        .role
        .is_none_or(|role| normalized_contains(Some(node.role.as_str()), role))
        && selector
            .name
            .is_none_or(|name| normalized_contains(node.name.as_deref(), name))
        && selector.text.is_none_or(|text| {
            normalized_contains(
                node.text
                    .as_ref()
                    .and_then(|value| value.content.as_deref()),
                text,
            ) || normalized_contains(node.name.as_deref(), text)
                || normalized_contains(node.description.as_deref(), text)
        })
        && selector
            .states
            .iter()
            .filter(|state| !state.trim().is_empty())
            .all(|state| {
                node.states
                    .iter()
                    .any(|node_state| normalized_equals(node_state, state))
            })
}

fn node_matches_resolve_purpose(node: &AccessibilityNode, purpose: ElementResolvePurpose) -> bool {
    match purpose {
        ElementResolvePurpose::Click => {
            node.bounds.as_ref().and_then(bounds_center).is_some()
                || primary_action_name(&node.actions).is_some()
        }
        ElementResolvePurpose::Action => !node.actions.is_empty(),
        ElementResolvePurpose::SetValue => node.supports_editable_text || node.value.is_some(),
    }
}

fn node_is_showing(node: &AccessibilityNode) -> bool {
    node.states
        .iter()
        .any(|state| normalized_equals(state, "showing"))
        && node
            .states
            .iter()
            .any(|state| normalized_equals(state, "visible"))
}

fn normalized_equals(actual: &str, expected: &str) -> bool {
    normalize_text(actual) == normalize_text(expected)
}

fn normalized_contains(actual: Option<&str>, expected: &str) -> bool {
    let expected = normalize_text(expected);
    !expected.is_empty()
        && actual
            .map(normalize_text)
            .is_some_and(|actual| actual.contains(&expected))
}

fn normalize_text(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn describe_selector(selector: &ElementSelector<'_>) -> String {
    let mut parts = Vec::new();
    if let Some(role) = selector.role.map(str::trim).filter(|role| !role.is_empty()) {
        parts.push(format!("role={role:?}"));
    }
    if let Some(name) = selector.name.map(str::trim).filter(|name| !name.is_empty()) {
        parts.push(format!("name={name:?}"));
    }
    if let Some(text) = selector.text.map(str::trim).filter(|text| !text.is_empty()) {
        parts.push(format!("text={text:?}"));
    }
    let states = selector
        .states
        .iter()
        .map(|state| state.trim())
        .filter(|state| !state.is_empty())
        .collect::<Vec<_>>();
    if !states.is_empty() {
        parts.push(format!("states={states:?}"));
    }
    if parts.is_empty() {
        "<empty>".to_string()
    } else {
        parts.join(", ")
    }
}

fn describe_matching_nodes(nodes: &[&AccessibilityNode]) -> String {
    nodes
        .iter()
        .take(8)
        .map(|node| {
            format!(
                "element_index {} role={:?} name={:?}",
                node.index, node.role, node.name
            )
        })
        .collect::<Vec<_>>()
        .join("; ")
}

fn is_plain_left_click(button: Option<&str>, click_count: Option<u32>) -> bool {
    let button = button.unwrap_or("left");
    let click_count = click_count.unwrap_or(1);
    matches!(button.to_ascii_lowercase().as_str(), "left" | "primary") && click_count == 1
}

fn click_equivalent_action(action: &AccessibilityAction) -> bool {
    matches!(
        action.name.to_ascii_lowercase().as_str(),
        "click" | "press" | "toggle"
    )
}

/// Whether one `wait_for` poll satisfies `appears` or `disappears`. Absence
/// needs a complete snapshot: a truncated one may stop before the element.
fn presence_condition_met(condition: WaitCondition, matched: bool, truncated: bool) -> bool {
    match condition {
        WaitCondition::Appears => matched,
        WaitCondition::Disappears => !matched && !truncated,
        WaitCondition::Changed => false,
    }
}

/// The action `perform_action` asked for, or `None` so the AT-SPI layer picks
/// the primary verb.
fn requested_or_primary_action(action: Option<&str>) -> Option<&str> {
    action.map(str::trim).filter(|value| !value.is_empty())
}

/// The action a plain left click maps to: the first click-equivalent one,
/// wherever the toolkit lists it.
fn primary_click_action(actions: &[AccessibilityAction]) -> Option<&AccessibilityAction> {
    actions
        .iter()
        .find(|action| click_equivalent_action(action))
}

fn primary_action(actions: &[AccessibilityAction]) -> Option<&AccessibilityAction> {
    actions.first()
}

fn primary_action_name(actions: &[AccessibilityAction]) -> Option<String> {
    primary_action(actions).map(|action| action.name.clone())
}

fn bounds_center(bounds: &Bounds) -> Option<(i32, i32)> {
    if bounds.width <= 0 || bounds.height <= 0 {
        return None;
    }
    if bounds.x <= i32::MIN / 2 || bounds.y <= i32::MIN / 2 {
        return None;
    }
    Some((
        bounds.x.checked_add(bounds.width / 2)?,
        bounds.y.checked_add(bounds.height / 2)?,
    ))
}

/// Context-cost warning for an AT-SPI snapshot that no app target narrowed.
/// Silent when the tree failed (the failure message already explains) or when
/// any filter or pid match scoped the roots. When the caller did pass a target
/// and it still matched no AT-SPI root, telling them to pass a target would
/// only replay the same desktop-wide snapshot, so that case points at the
/// app's accessibility support instead.
fn unscoped_accessibility_tree_warning(
    tree_scoped: bool,
    tree_ok: bool,
    target_requested: bool,
) -> Option<&'static str> {
    if !tree_ok || tree_scoped {
        return None;
    }
    Some(if target_requested {
        "WARNING: the requested target matched no AT-SPI application root, so the accessibility tree covers the whole desktop and can flood context. The target app may expose no accessibility tree (Electron apps need --force-renderer-accessibility); check list_apps for its AT-SPI name and pass that as app_name_or_bundle_identifier, or lower max_nodes to bound the cost."
    } else {
        "WARNING: no app target scoped the accessibility tree, so it covers the whole desktop and can flood context. Pass app_name_or_bundle_identifier or a window target (window_id, pid, app_id, wm_class, title) to limit it."
    })
}

/// Note appended when raw traversal hit max_nodes; the tree is incomplete.
fn truncated_accessibility_tree_note(truncated: bool) -> Option<&'static str> {
    truncated.then_some(
        "The node, depth, or read budget stopped traversal with unread elements left. To recover missing elements, scope to a narrower app or window target and raise max_nodes or max_depth (hard caps 2000 and 64); lowering max_nodes would drop more.",
    )
}

fn shell_only_accessibility_tree_note(
    nodes: &[AccessibilityNode],
    scoped: bool,
    truncated: bool,
) -> Option<&'static str> {
    if !scoped || truncated || nodes.is_empty() {
        return None;
    }
    let mut menu_nodes = std::collections::HashSet::new();
    nodes.iter().all(|node| {
        let in_menu = node.parent_index.is_some_and(|parent| menu_nodes.contains(&parent));
        let Some(menu) = accessibility_chrome_context(&node.role, in_menu) else {
            return false;
        };
        if menu {
            menu_nodes.insert(node.index);
        }
        let has_content_text = !menu && node.text.as_ref().and_then(|text| text.content.as_deref())
            .is_some_and(|text| text.chars().any(|ch| ch != '\u{fffc}' && !ch.is_whitespace()));
        !node.supports_editable_text && node.value.is_none() && !has_content_text
    })
        .then_some(
            "AT-SPI exposes only window/menu chrome; no content controls were observed. If this is an Electron/Chromium app, relaunch it with --force-renderer-accessibility to expose renderer content. Screenshots can still be used for observation; this tree does not establish that the UI is empty.",
        )
}

fn compact_accessibility_tree_with_note(
    nodes: Vec<AccessibilityNode>,
    scoped: bool,
    truncated: bool,
) -> (Vec<AccessibilityNode>, Option<&'static str>) {
    // Menu ancestry is observation metadata; compaction can remove its nodes.
    let note = shell_only_accessibility_tree_note(&nodes, scoped, truncated);
    (compact_accessibility_tree(nodes), note)
}

fn compact_accessibility_tree(nodes: Vec<AccessibilityNode>) -> Vec<AccessibilityNode> {
    if nodes.is_empty() {
        return nodes;
    }

    let keep = nodes
        .iter()
        .map(should_keep_accessibility_node)
        .collect::<Vec<_>>();
    let mut old_to_new = vec![None; nodes.len()];
    let mut compacted = Vec::new();

    for (old_index, node) in nodes.iter().enumerate() {
        if !keep[old_index] {
            continue;
        }

        let mut compacted_node = node.clone();
        compacted_node.index = compacted.len() as u32;
        compacted_node.parent_index = nearest_kept_parent(&keep, &nodes, old_index);
        old_to_new[old_index] = Some(compacted_node.index);
        compacted.push(compacted_node);
    }

    for node in &mut compacted {
        node.parent_index = node
            .parent_index
            .and_then(|old_parent| old_to_new.get(old_parent as usize).copied().flatten());
    }

    let child_counts = compacted.iter().filter_map(|node| node.parent_index).fold(
        vec![0_i32; compacted.len()],
        |mut counts, parent_index| {
            counts[parent_index as usize] += 1;
            counts
        },
    );

    for (index, node) in compacted.iter_mut().enumerate() {
        node.child_count = child_counts[index];
    }

    compacted
}

fn nearest_kept_parent(
    keep: &[bool],
    nodes: &[AccessibilityNode],
    old_index: usize,
) -> Option<u32> {
    let mut parent = nodes[old_index].parent_index;
    while let Some(parent_index) = parent {
        let parent_usize = parent_index as usize;
        if keep.get(parent_usize).copied().unwrap_or(false) {
            return Some(parent_index);
        }
        parent = nodes.get(parent_usize).and_then(|node| node.parent_index);
    }
    None
}

fn should_keep_accessibility_node(node: &AccessibilityNode) -> bool {
    if node.depth <= 1 {
        return true;
    }

    if is_actionable_accessibility_node(node) || has_meaningful_node_copy(node) {
        return true;
    }

    matches!(
        node.role.as_str(),
        "page tab" | "menu item" | "menu" | "list item" | "tree item"
    ) && !is_sentinel_or_missing_bounds(node.bounds.as_ref())
}

fn is_actionable_accessibility_node(node: &AccessibilityNode) -> bool {
    node.actions.iter().any(crate::outline::is_specific_action)
        || node.supports_editable_text
        || node.value.is_some()
}

fn has_meaningful_node_copy(node: &AccessibilityNode) -> bool {
    has_non_empty_text(node.name.as_deref())
        || has_non_empty_text(node.description.as_deref())
        || has_non_empty_text(node.text.as_ref().and_then(|text| text.content.as_deref()))
}

fn has_non_empty_text(value: Option<&str>) -> bool {
    crate::outline::clean(value).is_some()
}

fn is_sentinel_or_missing_bounds(bounds: Option<&Bounds>) -> bool {
    bounds.is_none()
}

fn select_accessibility_object_ref(
    apps: &[AccessibleAppSummary],
    target_pid: u32,
    candidates: &[String],
) -> Option<String> {
    let mut pid_matches = apps.iter().filter(|app| app.pid == Some(target_pid));
    let first = pid_matches.next()?;
    let second = pid_matches.next();

    if second.is_none() {
        return Some(first.object_ref.clone());
    }

    let lowered_candidates = candidates
        .iter()
        .map(|candidate| candidate.to_ascii_lowercase())
        .collect::<Vec<_>>();

    apps.iter()
        .filter(|app| app.pid == Some(target_pid))
        .find(|app| {
            let name = app.name.as_deref().unwrap_or_default().to_ascii_lowercase();
            lowered_candidates
                .iter()
                .any(|candidate| !candidate.is_empty() && name.contains(candidate))
        })
        .map(|app| app.object_ref.clone())
        .or_else(|| Some(first.object_ref.clone()))
}

fn accessibility_filter_candidates(window_context: Option<&WindowInfo>) -> Vec<String> {
    let Some(window) = window_context else {
        return Vec::new();
    };

    let mut candidates = Vec::new();
    push_candidate(&mut candidates, window.title.as_deref());
    push_candidate(&mut candidates, window.wm_class.as_deref());

    if let Some(app_id) = trimmed_nonempty(window.app_id.as_deref()) {
        if !app_id.starts_with("window:") {
            push_candidate(&mut candidates, Some(app_id));
            if let Some(stripped) = app_id.strip_suffix(".desktop") {
                push_candidate(&mut candidates, Some(stripped));
                let normalized = stripped.replace(['-', '_', '.'], " ");
                push_candidate(&mut candidates, Some(normalized.as_str()));
            } else {
                let normalized = app_id.replace(['-', '_', '.'], " ");
                push_candidate(&mut candidates, Some(normalized.as_str()));
            }
        }
    }

    candidates
}

fn push_candidate(candidates: &mut Vec<String>, value: Option<&str>) {
    let Some(value) = trimmed_nonempty(value) else {
        return;
    };

    if !candidates.iter().any(|candidate| candidate == value) {
        candidates.push(value.to_string());
    }
}

fn trimmed_nonempty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

/// Return the base64 payload of a `data:` URL (or the original string if bare).
fn data_url_payload(data_url: &str) -> String {
    data_url
        .split_once(',')
        .map(|(_, payload)| payload)
        .unwrap_or(data_url)
        .to_string()
}

fn prepare_app_state_screenshot(
    mut raw: RawScreenshotCapture,
    crop: Option<(i32, i32, u32, u32)>,
    target_requested: bool,
    options: ScreenshotPayloadOptions,
) -> Result<ScreenshotCapture> {
    if target_requested && crop.is_none() {
        anyhow::bail!(
            "targeted screenshot requires a resolved window; refusing to return the full desktop"
        );
    }
    if let Some(rect) = crop {
        let (x, y, width, height) = clip_capture_rect(rect, raw.width, raw.height)?;
        let (bytes, width, height) = crop_png(&raw.bytes, x, y, width, height)
            .map_err(|error| anyhow::anyhow!("targeted screenshot crop failed: {error}"))?;
        raw = RawScreenshotCapture {
            mime_type: raw.mime_type,
            bytes,
            source: raw.source,
            width,
            height,
        };
    }
    prepare_screenshot_payload(raw, options)
}

fn ensure_readonly_screenshot_target_is_visible(window: &WindowInfo) -> Result<()> {
    if window.hidden {
        anyhow::bail!("targeted get_app_state screenshot requires a visible, unminimized window");
    }
    if !window.focused {
        anyhow::bail!(
            "targeted get_app_state screenshot requires the window to already be focused; use the screenshot tool to raise it before capture"
        );
    }
    Ok(())
}

/// The window's crop in capture coordinates, clipped to the captured root.
fn window_capture_rect(
    window: &WindowInfo,
    capture_width: u32,
    capture_height: u32,
) -> Result<(i32, i32, u32, u32)> {
    let bounds = window.bounds.as_ref().ok_or_else(|| {
        anyhow::anyhow!(
            "targeted screenshot requires window bounds; refusing to return the full desktop"
        )
    })?;
    let rect = window_crop_rect(bounds).ok_or_else(|| {
        anyhow::anyhow!(
            "targeted screenshot has unusable window bounds; refusing to return the full desktop"
        )
    })?;
    clip_capture_rect(rect, capture_width, capture_height)
}

fn clip_capture_rect(
    (x, y, width, height): (i32, i32, u32, u32),
    capture_width: u32,
    capture_height: u32,
) -> Result<(i32, i32, u32, u32)> {
    let left = i64::from(x).max(0);
    let top = i64::from(y).max(0);
    let right = (i64::from(x) + i64::from(width)).min(i64::from(capture_width));
    let bottom = (i64::from(y) + i64::from(height)).min(i64::from(capture_height));
    if right <= left || bottom <= top {
        anyhow::bail!(
            "targeted screenshot window is outside the captured desktop; refusing to return the full desktop"
        );
    }
    Ok((
        i32::try_from(left).map_err(|_| anyhow::anyhow!("capture crop x is invalid"))?,
        i32::try_from(top).map_err(|_| anyhow::anyhow!("capture crop y is invalid"))?,
        u32::try_from(right - left)
            .map_err(|_| anyhow::anyhow!("capture crop width is invalid"))?,
        u32::try_from(bottom - top)
            .map_err(|_| anyhow::anyhow!("capture crop height is invalid"))?,
    ))
}

/// Convert a window's bounds into a crop rectangle, if it has a usable origin
/// and non-zero size.
fn window_crop_rect(bounds: &crate::windowing::WindowBounds) -> Option<(i32, i32, u32, u32)> {
    let x = bounds.x?;
    let y = bounds.y?;
    if bounds.width == 0 || bounds.height == 0 {
        return None;
    }
    Some((x, y, bounds.width, bounds.height))
}

fn apply_window_relative_click_coordinates(
    params: &mut ClickParams,
    capture_rect: (i32, i32, u32, u32),
) -> std::result::Result<(), String> {
    let (relative_x, relative_y) = params
        .x
        .zip(params.y)
        .ok_or_else(|| "Relative coordinate clicks require both x and y.".to_string())?;
    let (origin_x, origin_y, width, height) = capture_rect;
    if width == 0 || height == 0 {
        return Err(
            "Relative coordinate clicks require non-empty target-window bounds.".to_string(),
        );
    }
    if relative_x < 0 || relative_y < 0 {
        return Err("Relative click coordinates must be inside target-window bounds.".to_string());
    }
    if relative_x as u32 >= width || relative_y as u32 >= height {
        return Err("Relative click coordinates must be inside target-window bounds.".to_string());
    }
    let x = origin_x
        .checked_add(relative_x)
        .ok_or_else(|| "Relative click x coordinate overflowed.".to_string())?;
    let y = origin_y
        .checked_add(relative_y)
        .ok_or_else(|| "Relative click y coordinate overflowed.".to_string())?;
    params.x = Some(x);
    params.y = Some(y);
    Ok(())
}

/// Point a window-targeted scroll at the centre of the resolved window when
/// the caller supplied no coordinates. Without this the wheel events land on
/// whatever is under the current pointer position.
fn apply_window_center_scroll_point(
    params: &mut ScrollParams,
    capture_rect: (i32, i32, u32, u32),
) -> std::result::Result<(), String> {
    let (origin_x, origin_y, width, height) = capture_rect;
    if width == 0 || height == 0 {
        return Err(
            "Window-targeted scroll requires non-empty target-window bounds; pass x/y explicitly."
                .to_string(),
        );
    }
    params.x = Some(origin_x.saturating_add((width / 2) as i32));
    params.y = Some(origin_y.saturating_add((height / 2) as i32));
    Ok(())
}

fn apply_window_relative_scroll_coordinates(
    params: &mut ScrollParams,
    capture_rect: (i32, i32, u32, u32),
) -> std::result::Result<(), String> {
    let (relative_x, relative_y) = params
        .x
        .zip(params.y)
        .ok_or_else(|| "Relative scroll coordinates require both x and y.".to_string())?;
    let (origin_x, origin_y, width, height) = capture_rect;
    if width == 0 || height == 0 {
        return Err(
            "Relative scroll coordinates require non-empty target-window bounds.".to_string(),
        );
    }
    if relative_x < 0 || relative_y < 0 || relative_x as u32 >= width || relative_y as u32 >= height
    {
        return Err("Relative scroll coordinates must be inside target-window bounds.".to_string());
    }
    params.x = Some(origin_x.saturating_add(relative_x));
    params.y = Some(origin_y.saturating_add(relative_y));
    Ok(())
}

/// Crop a PNG image to `(x, y, w, h)` (clamped to the image), returning the
/// re-encoded PNG and the actual cropped dimensions.
fn crop_png(
    raw: &[u8],
    x: i32,
    y: i32,
    w: u32,
    h: u32,
) -> std::result::Result<(Vec<u8>, u32, u32), String> {
    use std::io::Cursor;
    let img = image::load_from_memory_with_format(raw, image::ImageFormat::Png)
        .map_err(|e| format!("decode png: {e}"))?;
    let (iw, ih) = (img.width(), img.height());
    let x = x.max(0) as u32;
    let y = y.max(0) as u32;
    if x >= iw || y >= ih {
        return Err("crop origin outside image".into());
    }
    let w = w.min(iw - x);
    let h = h.min(ih - y);
    let sub = img.crop_imm(x, y, w, h);
    let mut out = Vec::new();
    sub.write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
        .map_err(|e| format!("encode png: {e}"))?;
    Ok((out, w, h))
}

fn action_result(
    action: &str,
    result: std::result::Result<Vec<Output>, String>,
    received: Option<serde_json::Value>,
) -> ActionOutput {
    match result {
        Ok(_) => ActionOutput {
            ok: true,
            implemented: true,
            action: action.to_string(),
            message: "Action sent through xdotool (X11 XTEST).".to_string(),
            received,
        },
        Err(message) => ActionOutput {
            ok: false,
            implemented: true,
            action: action.to_string(),
            message,
            received,
        },
    }
}

fn action_result_with_focus(
    action: &str,
    result: std::result::Result<Vec<Output>, String>,
    received: Option<serde_json::Value>,
    focus: Option<WindowFocusResult>,
) -> ActionOutput {
    with_focus_context(action_result(action, result, received), focus)
}

fn with_focus_context(mut output: ActionOutput, focus: Option<WindowFocusResult>) -> ActionOutput {
    if output.ok {
        if let Some(focus) = focus {
            let verification = if focus.exact_window_focused {
                "exact window-focus"
            } else {
                "app-level focus"
            };
            output.message = format!(
                "{} Target window_id {} was focused with {verification} verification before input.",
                output.message, focus.requested_window.window_id,
            );
        }
    }
    output
}

/// Exact window focus alone cannot rule out lost input. Only a complete,
/// scoped shell-only tree explains the missing focus as unavailable content.
fn focus_probe_feedback(
    probe: &FocusProbe,
    expects_editable: bool,
    exact_window_focused: bool,
) -> String {
    match probe {
        FocusProbe::Found(element) => describe_focused_element(element, expects_editable),
        FocusProbe::ShellOnly if exact_window_focused => "Focused-element feedback unavailable: window focus was verified before input, but AT-SPI exposes only window/menu chrome in the target app. Field contents were not verified. Electron/Chromium apps may need --force-renderer-accessibility.".to_string(),
        FocusProbe::ShellOnly => "WARNING: AT-SPI exposes only window/menu chrome and no focused element in the target app; the input may have landed nowhere.".to_string(),
        FocusProbe::NoneFocused => "WARNING: AT-SPI reports no focused element in the target app; the input may have landed nowhere.".to_string(),
        FocusProbe::Incomplete => "Focused-element feedback unavailable: the focused element was not reached within the AT-SPI search limits, so the input could not be verified. Its absence here does not mean the input was lost.".to_string(),
        FocusProbe::NoAccessibleApp => "Focused-element feedback unavailable: the target app exposes no AT-SPI tree, so the input could not be verified. Electron apps need --force-renderer-accessibility.".to_string(),
    }
}

fn node_target_mismatch_message(element_index: u32, owner_pid: u32, target_pid: u32) -> String {
    format!(
        "element_index {element_index} comes from the latest get_app_state snapshot of pid {owner_pid}, not the target pid {target_pid}. Call get_app_state for the target app and use an index from that result."
    )
}

fn focus_target_pid(focus: Option<&WindowFocusResult>) -> Option<u32> {
    let focus = focus?;
    focus
        .focused_window
        .as_ref()
        .and_then(|window| window.pid)
        .or(focus.requested_window.pid)
}

fn describe_focused_element(element: &FocusedElementSummary, expects_editable: bool) -> String {
    let name = element
        .name
        .as_deref()
        .filter(|name| !name.is_empty())
        .map(|name| format!(" \"{name}\""))
        .unwrap_or_default();
    if focused_element_is_editable(element) {
        format!("Focused element: {}{name} (editable).", element.role)
    } else if element.is_terminal {
        format!(
            "Focused element: {}{name} (terminal keyboard input).",
            element.role
        )
    } else if expects_editable {
        format!(
            "WARNING: focused element is {}{name}, which is not editable — the typed text likely went nowhere. Click the intended input first or use set_value.",
            element.role
        )
    } else {
        format!("Focused element: {}{name} (not editable).", element.role)
    }
}

fn focused_element_is_editable(element: &FocusedElementSummary) -> bool {
    element.editable || element.states.iter().any(|state| state == "editable")
}

fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or(text)
}

/// Append supplemental notes (off-screen or focused-element feedback) to an
/// action result message without changing ok/implemented semantics.
fn with_notes(mut output: ActionOutput, notes: impl IntoIterator<Item = String>) -> ActionOutput {
    for note in notes {
        output.message = format!("{} {note}", output.message);
    }
    output
}

fn focus_satisfies_target(focus: &WindowFocusResult, target: &WindowTarget) -> bool {
    if target.requires_exact_focus() {
        focus.exact_window_focused
    } else {
        focus.exact_window_focused || focus.app_focused
    }
}

async fn window_list_output() -> ListWindowsOutput {
    match list_windows().await {
        Ok(windows) => {
            let backend = window_backend(windows.iter());
            let note = registry::list_note(&backend);
            ListWindowsOutput {
                backend,
                windows,
                error: None,
                permissions_hint: None,
                note: note.to_string(),
            }
        }
        Err(error) => {
            let error = format!("{error:#}");
            ListWindowsOutput {
                backend: "unavailable".to_string(),
                windows: Vec::new(),
                permissions_hint: window_permission_hint(&error),
                error: Some(error),
                note: "Window listing failed, so targeted keyboard input cannot safely focus or verify a target window."
                    .to_string(),
            }
        }
    }
}

fn window_backend<'a>(windows: impl Iterator<Item = &'a WindowInfo>) -> String {
    windows
        .map(|window| window.backend.clone())
        .next()
        .unwrap_or_else(|| "unknown".to_string())
}

fn xdotool_pointer_click_args(
    x: i32,
    y: i32,
    count: u32,
    button: Option<&str>,
) -> Option<Vec<String>> {
    let button = xdotool_pointer_button_code(button)?;
    Some(vec![
        "mousemove".to_string(),
        "--".to_string(),
        x.to_string(),
        y.to_string(),
        "click".to_string(),
        "--repeat".to_string(),
        count.to_string(),
        button.to_string(),
    ])
}

fn xdotool_pointer_button_code(button: Option<&str>) -> Option<&'static str> {
    match button.unwrap_or("left").to_ascii_lowercase().as_str() {
        "left" => Some("1"),
        "middle" => Some("2"),
        "right" => Some("3"),
        _ => None,
    }
}

/// Gap between XTEST wheel-button clicks, in milliseconds.
///
/// xdotool's own default is 100ms. One page is five clicks and valid requests
/// can ask for up to 100 pages, so 12ms keeps the bounded operation responsive
/// while matching the pacing used by `xdotool type`.
const XDOTOOL_SCROLL_CLICK_DELAY_MS: u64 = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScrollDirection {
    Up,
    Down,
    Left,
    Right,
}

/// X11 wheel buttons: 4 up, 5 down, 6 left, 7 right. `repeat` is the notch
/// count.
fn xdotool_pointer_scroll_args(
    point: Option<(i32, i32)>,
    repeat: u32,
    direction: ScrollDirection,
) -> Vec<String> {
    let button = match direction {
        ScrollDirection::Up => "4",
        ScrollDirection::Down => "5",
        ScrollDirection::Left => "6",
        ScrollDirection::Right => "7",
    };
    let mut args = Vec::new();
    if let Some((x, y)) = point {
        args.extend([
            "mousemove".to_string(),
            "--".to_string(),
            x.to_string(),
            y.to_string(),
        ]);
    }
    args.extend([
        "click".to_string(),
        "--delay".to_string(),
        XDOTOOL_SCROLL_CLICK_DELAY_MS.to_string(),
        "--repeat".to_string(),
        repeat.max(1).to_string(),
        button.to_string(),
    ]);
    args
}

/// `xdotool click --repeat` sleeps `--delay` after every click, including
/// the last, so the command timeout has to cover that gap.
fn xdotool_scroll_timeout(repeat: u32, delay_ms: u64) -> Duration {
    INPUT_COMMAND_TIMEOUT.saturating_add(Duration::from_millis(
        u64::from(repeat.max(1)).saturating_mul(delay_ms),
    ))
}

async fn run_cancellation_safe_input<T, F>(
    input_guard: tokio::sync::OwnedMutexGuard<()>,
    operation: F,
) -> (
    Option<tokio::sync::OwnedMutexGuard<()>>,
    std::result::Result<T, String>,
)
where
    T: Send + 'static,
    F: Future<Output = std::result::Result<T, String>> + Send + 'static,
{
    // Dropping a JoinHandle detaches its task, retaining the guard until the
    // stateful input operation has completed even if the caller is cancelled.
    match tokio::spawn(async move { (input_guard, operation.await) }).await {
        Ok((input_guard, result)) => (Some(input_guard), result),
        Err(error) => (None, Err(format!("stateful input task failed: {error}"))),
    }
}

/// Run one `xdotool` command line. Pointer and keyboard input both go through
/// XTEST, which resolves keysyms against the live XKB layout.
async fn run_xdotool(
    args: &[String],
    command_timeout: Duration,
) -> std::result::Result<Output, String> {
    run_xdotool_program(Path::new("xdotool"), args, command_timeout).await
}

async fn run_xdotool_program(
    program: &Path,
    args: &[String],
    command_timeout: Duration,
) -> std::result::Result<Output, String> {
    let mut command = TokioCommand::new(program);
    command.args(args);
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    command.kill_on_drop(true);
    command.process_group(0);

    let child = crate::command_runner::spawn_retrying_busy(&mut command)
        .await
        .map_err(|error| format!("xdotool could not start ({error}); install xdotool."))?;
    match crate::command_runner::output_child(child, "run xdotool", command_timeout)
        .await
        .map_err(|error| format!("{error:#}"))?
    {
        output if output.status.success() => Ok(output),
        output => Err(command_output_error("xdotool", output)),
    }
}

/// Press, move and release the left button. The release is retried alone
/// when the chained command fails, so a failure never leaves it held.
async fn run_xdotool_drag(
    start_x: i32,
    start_y: i32,
    end_x: i32,
    end_y: i32,
) -> std::result::Result<Vec<Output>, String> {
    let args = xdotool_drag_args((start_x, start_y), (end_x, end_y));
    match run_xdotool(&args, INPUT_COMMAND_TIMEOUT).await {
        Ok(output) => Ok(vec![output]),
        Err(error) => {
            let release = run_xdotool(
                &["mouseup".to_string(), "1".to_string()],
                INPUT_COMMAND_TIMEOUT,
            )
            .await;
            Err(match release {
                Ok(_) => error,
                Err(release_error) => {
                    format!("{error}; xdotool button release also failed: {release_error}")
                }
            })
        }
    }
}

/// Pauses between steps let toolkits see a press, a motion and a release.
fn xdotool_drag_args(start: (i32, i32), end: (i32, i32)) -> Vec<String> {
    [
        "mousemove",
        "--",
        &start.0.to_string(),
        &start.1.to_string(),
        "mousedown",
        "1",
        "sleep",
        "0.05",
        "mousemove",
        "--",
        &end.0.to_string(),
        &end.1.to_string(),
        "sleep",
        "0.05",
        "mouseup",
        "1",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

/// Default per-character delay for `xdotool type`, in milliseconds (xdotool's
/// own default). `--delay 0` lets XTEST key events race each other on some X
/// servers (Cinnamon on Mint 22, issue #147): every character arrives, but in
/// a random order. A small delay keeps them ordered.
/// `COMPUTER_USE_LINUX_XDOTOOL_TYPE_DELAY_MS` overrides it.
const XDOTOOL_TYPE_DELAY_MS: u64 = 12;
const XDOTOOL_TYPE_DELAY_ENV: &str = "COMPUTER_USE_LINUX_XDOTOOL_TYPE_DELAY_MS";

fn xdotool_type_delay_ms() -> u64 {
    env::var(XDOTOOL_TYPE_DELAY_ENV)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(XDOTOOL_TYPE_DELAY_MS)
}

fn xdotool_type_args_with_delay(text: &str, delay_ms: u64) -> Vec<String> {
    // XTEST maps LF to Linefeed, which editors ignore. CR maps to Return.
    // Fold CRLF first so a Windows line ending still inserts one newline.
    let text = text.replace("\r\n", "\n").replace('\n', "\r");
    vec![
        "type".to_string(),
        "--clearmodifiers".to_string(),
        "--delay".to_string(),
        delay_ms.to_string(),
        "--".to_string(),
        text,
    ]
}

/// `xdotool type` spends about `delay_ms` per character, so long inputs need
/// more than the flat input timeout.
fn xdotool_type_timeout(text: &str, delay_ms: u64) -> Duration {
    let chars = text.chars().count() as u64;
    INPUT_COMMAND_TIMEOUT.saturating_add(Duration::from_millis(chars.saturating_mul(delay_ms)))
}

/// Map our key grammar onto an `xdotool key` spec such as `ctrl+a`, `Return`,
/// or `shift+F5`. Returns `None` for keys the grammar does not accept, so the
/// caller can return an error instead of silently dropping the key.
fn xdotool_key_spec(key: &str) -> Option<String> {
    let parts = key
        .split('+')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    let (key_part, modifier_parts) = parts.split_last()?;
    if modifier_parts.is_empty() {
        if let Some(bare) = xdotool_modifier_name(key_part) {
            return Some(bare.to_string());
        }
    }
    let mut spec = modifier_parts
        .iter()
        .map(|part| xdotool_modifier_name(part).map(str::to_string))
        .collect::<Option<Vec<_>>>()?;
    spec.push(xdotool_keysym_name(key_part)?);
    Some(spec.join("+"))
}

fn xdotool_modifier_name(key: &str) -> Option<&'static str> {
    match normalize_key(key).as_str() {
        "ctrl" | "control" => Some("ctrl"),
        "alt" | "option" => Some("alt"),
        "shift" => Some("shift"),
        "meta" | "super" | "cmd" | "command" => Some("super"),
        _ => None,
    }
}

fn xdotool_keysym_name(key: &str) -> Option<String> {
    let normalized = normalize_key(key);
    let named = match normalized.as_str() {
        "enter" | "return" => "Return",
        "escape" | "esc" => "Escape",
        "tab" => "Tab",
        "backspace" => "BackSpace",
        "delete" | "del" => "Delete",
        "space" => "space",
        "home" => "Home",
        "end" => "End",
        "pageup" | "page_up" => "Page_Up",
        "pagedown" | "page_down" => "Page_Down",
        "arrowleft" | "left" => "Left",
        "arrowright" | "right" => "Right",
        "arrowup" | "up" => "Up",
        "arrowdown" | "down" => "Down",
        "f1" => "F1",
        "f2" => "F2",
        "f3" => "F3",
        "f4" => "F4",
        "f5" => "F5",
        "f6" => "F6",
        "f7" => "F7",
        "f8" => "F8",
        "f9" => "F9",
        "f10" => "F10",
        "f11" => "F11",
        "f12" => "F12",
        value if value.len() == 1 && value.as_bytes()[0].is_ascii_alphanumeric() => {
            return Some(value.to_string());
        }
        _ => return None,
    };
    Some(named.to_string())
}

fn normalize_key(key: &str) -> String {
    key.trim().to_ascii_lowercase().replace(['-', ' '], "")
}

fn command_output_error(command: &str, output: Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let detail = if stderr.is_empty() { stdout } else { stderr };
    if detail.is_empty() {
        format!("{command} exited with {}", output.status)
    } else {
        detail
    }
}

fn list_process_apps() -> Vec<AppCandidate> {
    let output = Command::new("ps")
        .args(["-eo", "pid=,comm=,args="])
        .output();
    let Ok(output) = output else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(parse_process_line)
        .filter(|app| looks_like_desktop_app(&app.name, &app.command))
        .take(50)
        .collect()
}

fn parse_process_line(line: &str) -> Option<AppCandidate> {
    let trimmed = line.trim();
    let mut parts = trimmed.splitn(3, char::is_whitespace);
    let pid = parts.next()?.parse().ok()?;
    let name = parts.next()?.to_string();
    let command = parts.next().unwrap_or("").trim().to_string();
    Some(AppCandidate { name, pid, command })
}

fn looks_like_desktop_app(name: &str, command: &str) -> bool {
    let haystack = format!("{name} {command}").to_ascii_lowercase();
    [
        "codex",
        "electron",
        "chrome",
        "chromium",
        "firefox",
        "brave",
        "code",
        "gnome-terminal",
        "ptyxis",
        "kgx",
        "nautilus",
        "slack",
        "discord",
        "spotify",
        "obsidian",
    ]
    .iter()
    .any(|needle| haystack.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atspi_tree::{AccessibilityAction, Bounds};
    use crate::windows::{WindowBounds, I3_BACKEND};
    use std::os::unix::fs::PermissionsExt;

    /// Restores an environment variable when a test ends, including on panic.
    struct EnvVarGuard {
        key: &'static str,
        original: Option<std::ffi::OsString>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let original = std::env::var_os(key);
            std::env::set_var(key, value);
            Self { key, original }
        }

        fn unset(key: &'static str) -> Self {
            let original = std::env::var_os(key);
            std::env::remove_var(key);
            Self { key, original }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            match &self.original {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }

    /// The run_shell tests change the same process-wide variable, so they
    /// take turns instead of racing each other under the parallel test runner.
    static SHELL_ENV_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    /// Exercise the real pointer handlers in a separate process with a stub
    /// xdotool on PATH. The display is unreachable, so nothing touches a
    /// desktop, and the cached root size keeps warnings from capturing.
    #[test]
    fn pointer_handlers_dispatch_through_xdotool() {
        let dir = std::env::temp_dir().join(format!(
            "cul-pointer-dispatch-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        struct Cleanup(std::path::PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = Cleanup(dir.clone());
        let xdotool = dir.join("xdotool");
        std::fs::write(
            &xdotool,
            "#!/bin/sh\nprintf 'xdotool %s\\n' \"$*\" >> \"$CUL_POINTER_TEST_LOG\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&xdotool, std::fs::Permissions::from_mode(0o700)).unwrap();
        let log = dir.join("input.log");
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "server::tests::pointer_handlers_dispatch_child",
                "--ignored",
                "--nocapture",
            ])
            .env_clear()
            .env("PATH", &dir)
            .env("HOME", &dir)
            .env("CUL_POINTER_TEST_LOG", &log)
            .env("XDG_RUNTIME_DIR", &dir)
            .env(
                "DBUS_SESSION_BUS_ADDRESS",
                format!("unix:path={}/no-bus", dir.display()),
            )
            .env("DISPLAY", ":9876")
            .env("XDG_SESSION_TYPE", "x11");
        let output = crate::command_runner::output_blocking_with_timeout(
            &mut command,
            "pointer dispatch regression",
            Duration::from_secs(15),
        )
        .unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let commands = std::fs::read_to_string(&log).unwrap_or_default();
        assert_eq!(
            commands,
            "xdotool mousemove -- 5536 700 click --repeat 1 1\n\
             xdotool mousemove -- 5536 700 click --delay 12 --repeat 5 5\n\
             xdotool mousemove -- 100 100 mousedown 1 sleep 0.05 mousemove -- 200 200 sleep 0.05 mouseup 1\n\
             xdotool click --delay 12 --repeat 5 5\n"
        );
    }

    #[tokio::test]
    #[ignore = "isolated environment subprocess; invoked by pointer_handlers_dispatch_through_xdotool"]
    async fn pointer_handlers_dispatch_child() {
        assert!(std::env::var_os("CUL_POINTER_TEST_LOG").is_some());
        let backend = ComputerUseLinux::default();
        backend.cache_desktop_size(9376, 1600);
        assert!(backend.off_screen_note_for_point(9376, 700).await.is_some());
        assert!(backend.off_screen_note_for_point(5536, 700).await.is_none());
        let click: ClickParams =
            serde_json::from_value(serde_json::json!({"x":5536,"y":700})).unwrap();
        let scroll: ScrollParams =
            serde_json::from_value(serde_json::json!({"x":5536,"y":700,"direction":"down"}))
                .unwrap();
        let drag = DragParams {
            start_x: 100,
            start_y: 100,
            end_x: 200,
            end_y: 200,
        };
        let results = [
            backend.click(Parameters(click)).await.0,
            backend.scroll(Parameters(scroll)).await.0,
            backend.drag(Parameters(drag)).await.0,
        ];
        for result in results {
            assert!(result.ok, "{}", result.message);
            assert!(result.message.contains("xdotool"), "{}", result.message);
        }
        let scroll: ScrollParams =
            serde_json::from_value(serde_json::json!({"direction":"down"})).unwrap();
        let result = backend.scroll(Parameters(scroll)).await.0;
        assert!(result.ok, "{}", result.message);
    }

    #[test]
    fn exported_tool_schemas_omit_unsigned_integer_formats() {
        let tools = ComputerUseLinux::default().mcp_tool_router().list_all();
        let value = serde_json::to_value(tools).unwrap();
        let mut unsupported = Vec::new();
        collect_unsigned_integer_formats(&value, "$", &mut unsupported);

        assert!(
            unsupported.is_empty(),
            "unsupported unsigned integer formats: {unsupported:?}"
        );
    }

    fn collect_unsigned_integer_formats(
        value: &serde_json::Value,
        path: &str,
        unsupported: &mut Vec<String>,
    ) {
        match value {
            serde_json::Value::Object(object) => {
                if matches!(
                    object.get("format").and_then(serde_json::Value::as_str),
                    Some("uint" | "uint8" | "uint16" | "uint32" | "uint64" | "usize")
                ) {
                    unsupported.push(path.to_string());
                }
                for (key, nested) in object {
                    collect_unsigned_integer_formats(nested, &format!("{path}/{key}"), unsupported);
                }
            }
            serde_json::Value::Array(items) => {
                for (index, nested) in items.iter().enumerate() {
                    collect_unsigned_integer_formats(
                        nested,
                        &format!("{path}/{index}"),
                        unsupported,
                    );
                }
            }
            _ => {}
        }
    }

    fn node(index: u32, bounds: Option<Bounds>) -> AccessibilityNode {
        node_with_actions(index, bounds, Vec::new())
    }

    #[test]
    fn shell_only_tree_explains_missing_renderer_content() {
        let mut frame = node(0, None);
        frame.role = "frame".to_string();
        let nodes = [frame];
        let note = shell_only_accessibility_tree_note(&nodes, true, false).unwrap();
        assert!(note.contains("--force-renderer-accessibility"));
        assert!(note.contains("If this is an Electron/Chromium app"));
        assert!(shell_only_accessibility_tree_note(&nodes, false, false).is_none());
        assert!(shell_only_accessibility_tree_note(&nodes, true, true).is_none());
        assert!(shell_only_accessibility_tree_note(&[], true, false).is_none());
    }

    #[test]
    fn content_and_editable_trees_do_not_get_renderer_hint() {
        let mut content = node(0, None);
        assert!(shell_only_accessibility_tree_note(&[content.clone()], true, false).is_none());
        content.role = "frame".to_string();
        content.supports_editable_text = true;
        assert!(shell_only_accessibility_tree_note(&[content], true, false).is_none());
    }

    #[test]
    fn native_menu_chrome_does_not_hide_the_renderer_hint() {
        let mut frame = node(0, None);
        frame.role = "frame".to_string();
        let mut menu = node(1, None);
        menu.role = "menu item".to_string();
        menu.actions.push(AccessibilityAction {
            index: 0,
            name: "click".to_string(),
            description: String::new(),
            keybinding: String::new(),
        });
        assert!(
            shell_only_accessibility_tree_note(&[frame.clone(), menu.clone()], true, false)
                .is_some()
        );
        menu.supports_editable_text = true;
        assert!(shell_only_accessibility_tree_note(&[frame, menu], true, false).is_none());
    }

    #[test]
    fn native_menu_buttons_are_chrome_only_beneath_a_menu() {
        let mut frame = node(0, None);
        frame.role = "frame".to_string();
        let mut panel = node(1, None);
        panel.role = "panel".to_string();
        panel.parent_index = Some(0);
        let mut menu = node(2, None);
        menu.role = "menu bar".to_string();
        menu.parent_index = Some(1);
        let mut button = node(3, None);
        button.role = "button".to_string();
        button.parent_index = Some(2);
        assert!(shell_only_accessibility_tree_note(
            &[frame.clone(), panel.clone(), menu.clone(), button.clone()],
            true,
            false
        )
        .is_some());
        button.parent_index = Some(1);
        assert!(
            shell_only_accessibility_tree_note(&[frame, panel, menu, button], true, false)
                .is_none()
        );
    }

    #[test]
    fn review_hint_keeps_raw_menu_ancestry_after_compaction() {
        let mut frame = node(0, None);
        frame.role = "frame".to_string();
        frame.depth = 1;
        let mut panel = node(1, None);
        panel.role = "panel".to_string();
        panel.name = None;
        panel.depth = 2;
        panel.parent_index = Some(0);
        let mut menu = node(2, None);
        menu.role = "menu bar".to_string();
        menu.name = None;
        menu.depth = 3;
        menu.parent_index = Some(1);
        let mut button = node(3, None);
        button.role = "button".to_string();
        button.depth = 4;
        button.parent_index = Some(2);
        let nodes = vec![frame, panel, menu, button];
        let (compacted, note) = compact_accessibility_tree_with_note(nodes.clone(), true, false);
        assert_eq!(
            compacted.len(),
            2,
            "unnamed panel and menu bar should be compacted away"
        );
        assert_eq!(compacted[1].parent_index, Some(0));
        assert!(
            shell_only_accessibility_tree_note(&compacted, true, false).is_none(),
            "compacted data cannot recover the removed menu context"
        );
        assert!(
            note.is_some(),
            "raw menu ancestry must determine the observation hint"
        );
        assert!(
            compact_accessibility_tree_with_note(nodes.clone(), false, false)
                .1
                .is_none()
        );
        assert!(compact_accessibility_tree_with_note(nodes, true, true)
            .1
            .is_none());
    }

    fn node_with_actions(
        index: u32,
        bounds: Option<Bounds>,
        actions: Vec<AccessibilityAction>,
    ) -> AccessibilityNode {
        AccessibilityNode {
            index,
            parent_index: None,
            depth: 0,
            object_ref: format!(":1.{index}/org/a11y/atspi/accessible/{index}"),
            role: "push button".to_string(),
            name: Some(format!("Button {index}")),
            description: None,
            child_count: 0,
            bounds,
            states: Vec::new(),
            actions,
            value: None,
            text: None,
            supports_editable_text: false,
        }
    }

    fn click_action() -> AccessibilityAction {
        AccessibilityAction {
            index: 0,
            name: "Click".to_string(),
            description: "Clicks the element".to_string(),
            keybinding: String::new(),
        }
    }

    fn solid_png(width: u32, height: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(width, height, image::Rgba([32, 128, 192, 255]));
        let mut out = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }

    #[test]
    fn targeted_app_state_crops_before_screenshot_payload_resize() {
        let raw = RawScreenshotCapture {
            mime_type: "image/png".to_string(),
            bytes: solid_png(400, 200),
            source: "test".to_string(),
            width: 400,
            height: 200,
        };
        let capture = prepare_app_state_screenshot(
            raw,
            Some((50, 20, 200, 100)),
            true,
            ScreenshotPayloadOptions {
                max_width: Some(100),
                max_height: Some(100),
                max_bytes: Some(1024 * 1024),
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(
            (capture.coordinate_width, capture.coordinate_height),
            (200, 100)
        );
        assert_eq!((capture.width, capture.height), (100, 50));
    }

    #[test]
    fn unresolved_app_state_target_refuses_full_desktop_screenshot() {
        let raw = RawScreenshotCapture {
            mime_type: "image/png".to_string(),
            bytes: solid_png(400, 200),
            source: "test".to_string(),
            width: 400,
            height: 200,
        };

        let error =
            prepare_app_state_screenshot(raw, None, true, ScreenshotPayloadOptions::default())
                .unwrap_err();

        assert!(error.to_string().contains("requires a resolved window"));
    }

    #[test]
    fn targeted_app_state_crops_only_visible_part_of_offscreen_window() {
        let raw = RawScreenshotCapture {
            mime_type: "image/png".to_string(),
            bytes: solid_png(400, 200),
            source: "test".to_string(),
            width: 400,
            height: 200,
        };
        let capture = prepare_app_state_screenshot(
            raw,
            Some((-50, -40, 100, 100)),
            true,
            ScreenshotPayloadOptions::default(),
        )
        .unwrap();

        assert_eq!(
            (capture.coordinate_width, capture.coordinate_height),
            (50, 60)
        );
    }

    fn sample_app_state_output(screenshot: Option<&ScreenshotCapture>) -> GetAppStateOutput {
        GetAppStateOutput {
            app_name_or_bundle_identifier: None,
            window_context: None,
            window_error: None,
            window_permissions_hint: None,
            backend: "linux-atspi".to_string(),
            screenshot: screenshot.map(ScreenshotSummary::from),
            screenshot_error: None,
            accessibility_tree: Vec::new(),
            accessibility_outline: None,
            accessibility_tree_raw_count: 0,
            tree_scoped: true,
            accessibility_tree_truncated: false,
            accessibility_error: None,
            readiness: crate::diagnostics::ReadinessReport {
                ready: true,
                can_build_accessibility_tree: true,
                can_query_windows: true,
                can_focus_windows: true,
                can_send_input: true,
                can_capture_screenshots: true,
                screenshot_capture_status: crate::diagnostics::ScreenshotCaptureStatus::Verified,
                can_ocr: true,
                recommended_next_step: String::new(),
                blockers: Vec::new(),
            },
            diagnostics: None,
            message: "ok".to_string(),
        }
    }

    #[test]
    fn app_state_result_emits_screenshot_as_image_block_not_inline_base64() {
        // Issue #145: the base64 payload must ride a structured image block so
        // hosts count it as vision tokens instead of a 100KB text blob.
        let raw = RawScreenshotCapture {
            mime_type: "image/png".to_string(),
            bytes: solid_png(64, 32),
            source: "test".to_string(),
            width: 64,
            height: 32,
        };
        let capture =
            prepare_app_state_screenshot(raw, None, false, ScreenshotPayloadOptions::default())
                .unwrap();
        let payload = data_url_payload(&capture.data_url);
        let output = sample_app_state_output(Some(&capture));

        let result = app_state_result(output, Some(capture)).unwrap();

        assert_eq!(result.content.len(), 2);
        let image = result.content[0]
            .as_image()
            .expect("first block is an image");
        assert_eq!(image.mime_type, "image/png");
        assert_eq!(image.data, payload);
        let text = &result.content[1]
            .as_text()
            .expect("second block is text")
            .text;
        assert!(
            !text.contains(&payload),
            "base64 leaked into the text block"
        );
        assert!(!text.contains("data_url"));
        let structured = result.structured_content.expect("structured content");
        assert_eq!(structured["screenshot"]["width"], 64);
        assert_eq!(structured["screenshot"]["mime_type"], "image/png");
        assert!(structured["screenshot"].get("data_url").is_none());
    }

    #[test]
    fn app_state_result_without_screenshot_is_text_only() {
        let result = app_state_result(sample_app_state_output(None), None).unwrap();
        assert_eq!(result.content.len(), 1);
        assert!(result.content[0].as_text().is_some());
        assert_eq!(
            result.structured_content.unwrap()["screenshot"],
            serde_json::Value::Null
        );
    }

    #[test]
    fn readonly_targeted_screenshot_requires_focused_visible_window() {
        let mut window = window_info(1, Some("Target"), None, None, None);
        assert!(ensure_readonly_screenshot_target_is_visible(&window).is_err());
        window.focused = true;
        assert!(ensure_readonly_screenshot_target_is_visible(&window).is_ok());
        window.hidden = true;
        assert!(ensure_readonly_screenshot_target_is_visible(&window).is_err());
    }

    #[test]
    fn window_crop_happens_before_screenshot_payload_resize() {
        let (cropped, width, height) = crop_png(&solid_png(400, 200), 50, 20, 200, 100).unwrap();
        let capture = prepare_screenshot_payload(
            RawScreenshotCapture {
                mime_type: "image/png".to_string(),
                bytes: cropped,
                source: "test".to_string(),
                width,
                height,
            },
            ScreenshotPayloadOptions {
                max_width: Some(100),
                max_height: Some(100),
                max_bytes: Some(1024 * 1024),
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(
            (capture.coordinate_width, capture.coordinate_height),
            (200, 100)
        );
        assert_eq!((capture.width, capture.height), (100, 50));
        assert!(capture.resized);
    }

    fn window_info(
        window_id: u64,
        title: Option<&str>,
        app_id: Option<&str>,
        wm_class: Option<&str>,
        pid: Option<u32>,
    ) -> WindowInfo {
        WindowInfo {
            window_id,
            title: title.map(str::to_string),
            app_id: app_id.map(str::to_string),
            wm_class: wm_class.map(str::to_string),
            pid,
            bounds: Some(WindowBounds {
                x: Some(10),
                y: Some(20),
                width: 800,
                height: 600,
            }),
            workspace: Some(0),
            focused: false,
            hidden: false,
            client_type: Some("x11".to_string()),
            backend: I3_BACKEND.to_string(),
            terminal: None,
        }
    }

    #[test]
    fn relative_click_coordinates_use_capture_space_rect() {
        let mut params = ClickParams {
            x: Some(7),
            y: Some(9),
            relative: Some(true),
            ..Default::default()
        };

        apply_window_relative_click_coordinates(&mut params, (133, 267, 1067, 800)).unwrap();

        assert_eq!((params.x, params.y), (Some(140), Some(276)));
    }

    #[test]
    fn relative_click_origin_is_screenshot_crop_not_raw_surface_buffer() {
        // Client-side-decorated window probe: frame [760,406,400,300], buffer
        // [746,394,428,329]. The screenshot's green target is at [850,543].
        // Crop-local [90,137] and desktop [850,543] must denote the same point;
        // substituting the buffer/shadow origin would miss by [14,12].
        let mut params = ClickParams {
            x: Some(90),
            y: Some(137),
            relative: Some(true),
            ..Default::default()
        };
        apply_window_relative_click_coordinates(&mut params, (760, 406, 400, 300)).unwrap();
        assert_eq!((params.x, params.y), (Some(850), Some(543)));
    }

    #[test]
    fn relative_click_coordinates_require_xy() {
        let mut params = ClickParams {
            x: Some(7),
            relative: Some(true),
            ..Default::default()
        };

        let error =
            apply_window_relative_click_coordinates(&mut params, (100, 200, 800, 600)).unwrap_err();

        assert!(error.contains("both x and y"));
        assert_eq!((params.x, params.y), (Some(7), None));
    }

    #[test]
    fn relative_click_coordinates_must_stay_inside_bounds() {
        for (x, y) in [(-1, 9), (7, -1), (800, 9), (7, 600)] {
            let mut params = ClickParams {
                x: Some(x),
                y: Some(y),
                relative: Some(true),
                ..Default::default()
            };

            let error = apply_window_relative_click_coordinates(&mut params, (100, 200, 800, 600))
                .unwrap_err();

            assert!(error.contains("inside target-window bounds"));
            assert_eq!((params.x, params.y), (Some(x), Some(y)));
        }
    }

    #[test]
    fn accessibility_filter_candidates_prefer_title_and_skip_synthetic_app_id() {
        let window = window_info(
            42,
            Some("CU ATSPI GTK Test"),
            Some("window:46"),
            Some("cu_atspi_gtk_test.py"),
            Some(2914326),
        );

        let candidates = accessibility_filter_candidates(Some(&window));

        assert_eq!(
            candidates,
            vec![
                "CU ATSPI GTK Test".to_string(),
                "cu_atspi_gtk_test.py".to_string(),
            ]
        );
    }

    #[test]
    fn select_accessibility_object_ref_prefers_exact_pid_match() {
        let apps = vec![
            AccessibleAppSummary {
                object_ref: ":1.31/org/a11y/atspi/accessible/root".to_string(),
                name: Some("electron".to_string()),
                pid: Some(2774076),
                role: "application".to_string(),
                child_count: 1,
                bounds: None,
            },
            AccessibleAppSummary {
                object_ref: ":1.64/org/a11y/atspi/accessible/root".to_string(),
                name: Some("cu_atspi_gtk_test.py".to_string()),
                pid: Some(2914326),
                role: "application".to_string(),
                child_count: 1,
                bounds: None,
            },
        ];

        let object_ref = select_accessibility_object_ref(
            &apps,
            2914326,
            &[
                "CU ATSPI GTK Test".to_string(),
                "cu_atspi_gtk_test.py".to_string(),
            ],
        )
        .unwrap();

        assert_eq!(object_ref, ":1.64/org/a11y/atspi/accessible/root");
    }

    #[test]
    fn compact_accessibility_tree_reparents_actionable_descendants() {
        let nodes = vec![
            AccessibilityNode {
                index: 0,
                parent_index: None,
                depth: 0,
                object_ref: ":1.0/root".to_string(),
                role: "application".to_string(),
                name: Some("demo-app".to_string()),
                description: None,
                child_count: 1,
                bounds: None,
                states: Vec::new(),
                actions: Vec::new(),
                value: None,
                text: None,
                supports_editable_text: false,
            },
            AccessibilityNode {
                index: 1,
                parent_index: Some(0),
                depth: 1,
                object_ref: ":1.1/frame".to_string(),
                role: "frame".to_string(),
                name: Some("Demo Frame".to_string()),
                description: None,
                child_count: 1,
                bounds: None,
                states: Vec::new(),
                actions: Vec::new(),
                value: None,
                text: None,
                supports_editable_text: false,
            },
            AccessibilityNode {
                index: 2,
                parent_index: Some(1),
                depth: 2,
                object_ref: ":1.2/filler".to_string(),
                role: "filler".to_string(),
                name: None,
                description: None,
                child_count: 1,
                bounds: None,
                states: Vec::new(),
                actions: Vec::new(),
                value: None,
                text: None,
                supports_editable_text: false,
            },
            AccessibilityNode {
                index: 3,
                parent_index: Some(2),
                depth: 3,
                object_ref: ":1.3/button".to_string(),
                role: "button".to_string(),
                name: Some("Run".to_string()),
                description: None,
                child_count: 0,
                bounds: Some(Bounds {
                    x: 10,
                    y: 20,
                    width: 100,
                    height: 40,
                }),
                states: Vec::new(),
                actions: vec![AccessibilityAction {
                    index: 0,
                    name: "Click".to_string(),
                    description: "Clicks the button".to_string(),
                    keybinding: String::new(),
                }],
                value: None,
                text: None,
                supports_editable_text: false,
            },
        ];

        let compacted = compact_accessibility_tree(nodes);

        assert_eq!(compacted.len(), 3);
        assert_eq!(compacted[0].role, "application");
        assert_eq!(compacted[1].role, "frame");
        assert_eq!(compacted[2].role, "button");
        assert_eq!(compacted[2].parent_index, Some(1));
        assert_eq!(compacted[1].child_count, 1);
    }

    #[test]
    fn compact_accessibility_tree_drops_structural_noise() {
        let nodes = vec![
            AccessibilityNode {
                index: 0,
                parent_index: None,
                depth: 0,
                object_ref: ":1.0/root".to_string(),
                role: "application".to_string(),
                name: Some("demo-app".to_string()),
                description: None,
                child_count: 2,
                bounds: None,
                states: Vec::new(),
                actions: Vec::new(),
                value: None,
                text: None,
                supports_editable_text: false,
            },
            AccessibilityNode {
                index: 1,
                parent_index: Some(0),
                depth: 1,
                object_ref: ":1.1/frame".to_string(),
                role: "frame".to_string(),
                name: Some("Demo Frame".to_string()),
                description: None,
                child_count: 2,
                bounds: None,
                states: Vec::new(),
                actions: Vec::new(),
                value: None,
                text: None,
                supports_editable_text: false,
            },
            AccessibilityNode {
                index: 2,
                parent_index: Some(1),
                depth: 2,
                object_ref: ":1.2/tab".to_string(),
                role: "page tab".to_string(),
                name: Some("Hidden".to_string()),
                description: None,
                child_count: 0,
                bounds: None,
                states: Vec::new(),
                actions: Vec::new(),
                value: None,
                text: None,
                supports_editable_text: false,
            },
            AccessibilityNode {
                index: 3,
                parent_index: Some(1),
                depth: 2,
                object_ref: ":1.3/separator".to_string(),
                role: "separator".to_string(),
                name: None,
                description: None,
                child_count: 0,
                bounds: None,
                states: Vec::new(),
                actions: Vec::new(),
                value: None,
                text: None,
                supports_editable_text: false,
            },
        ];

        let compacted = compact_accessibility_tree(nodes);

        assert_eq!(compacted.len(), 3);
        assert_eq!(compacted[2].role, "page tab");
        assert_eq!(compacted[2].name.as_deref(), Some("Hidden"));
    }

    #[test]
    fn cached_element_index_resolves_to_bounds_center() {
        let backend = ComputerUseLinux::default();
        backend.cache_nodes(&[node(
            7,
            Some(Bounds {
                x: 10,
                y: 20,
                width: 100,
                height: 40,
            }),
        )]);

        let point = backend
            .resolve_optional_target_point(None, None, Some(7))
            .unwrap()
            .unwrap();

        assert_eq!(point, (60, 40));
    }

    #[test]
    fn coordinate_target_overrides_cached_element_index() {
        let backend = ComputerUseLinux::default();
        backend.cache_nodes(&[node(
            7,
            Some(Bounds {
                x: 10,
                y: 20,
                width: 100,
                height: 40,
            }),
        )]);

        let point = backend
            .resolve_optional_target_point(Some(200), Some(300), Some(7))
            .unwrap()
            .unwrap();

        assert_eq!(point, (200, 300));
    }

    #[test]
    fn cached_element_index_requires_positive_bounds() {
        let backend = ComputerUseLinux::default();
        backend.cache_nodes(&[node(
            7,
            Some(Bounds {
                x: 10,
                y: 20,
                width: 0,
                height: 40,
            }),
        )]);

        let error = backend
            .resolve_optional_target_point(None, None, Some(7))
            .unwrap_err();

        assert!(error.contains("No clickable bounds cached for element_index 7"));
    }

    #[test]
    fn cached_element_index_ignores_sentinel_bounds() {
        let backend = ComputerUseLinux::default();
        backend.cache_nodes(&[node(
            7,
            Some(Bounds {
                x: i32::MIN,
                y: i32::MIN,
                width: 1,
                height: 1,
            }),
        )]);

        let error = backend
            .resolve_optional_target_point(None, None, Some(7))
            .unwrap_err();

        assert!(error.contains("No clickable bounds cached for element_index 7"));
    }

    #[test]
    fn empty_node_cache_clears_stale_element_index() {
        let backend = ComputerUseLinux::default();
        backend.cache_nodes(&[node(
            7,
            Some(Bounds {
                x: 10,
                y: 20,
                width: 100,
                height: 40,
            }),
        )]);
        backend.cache_nodes(&[]);

        let error = backend
            .resolve_optional_target_point(None, None, Some(7))
            .unwrap_err();

        assert!(error.contains("No clickable bounds cached for element_index 7"));
    }

    #[test]
    fn click_target_falls_back_to_primary_action_without_bounds() {
        let backend = ComputerUseLinux::default();
        backend.cache_nodes(&[node_with_actions(
            7,
            None,
            vec![AccessibilityAction {
                index: 0,
                name: "Click".to_string(),
                description: "Clicks the button".to_string(),
                keybinding: String::new(),
            }],
        )]);

        let target = backend
            .resolve_click_target(&ClickParams {
                element_index: Some(7),
                ..Default::default()
            })
            .unwrap();

        match target {
            ClickTarget::PrimaryAction {
                object_ref,
                action_name,
                action_index,
            } => {
                assert_eq!(object_ref, ":1.7/org/a11y/atspi/accessible/7");
                assert_eq!(action_name.as_deref(), Some("Click"));
                assert_eq!(action_index, 0);
            }
            ClickTarget::Coordinates(_, _) => {
                panic!("expected AT-SPI primary-action fallback")
            }
        }
    }

    #[test]
    fn element_activation_ignores_hidpi_and_zero_origin_bounds() {
        for (x, y) in [(75, 144), (0, 0)] {
            let backend = ComputerUseLinux::default();
            backend.cache_nodes(&[node_with_actions(
                7,
                Some(Bounds {
                    x,
                    y,
                    width: 320,
                    height: 120,
                }),
                vec![AccessibilityAction {
                    index: 0,
                    name: "click".into(),
                    description: String::new(),
                    keybinding: String::new(),
                }],
            )]);
            assert!(matches!(
                backend
                    .resolve_click_target(&ClickParams {
                        element_index: Some(7),
                        ..Default::default()
                    })
                    .unwrap(),
                ClickTarget::PrimaryAction {
                    action_index: 0,
                    ..
                }
            ));
            // Explicit coordinates and gestures retain pointer semantics.
            for params in [
                ClickParams {
                    x: Some(470),
                    y: Some(408),
                    element_index: Some(7),
                    ..Default::default()
                },
                ClickParams {
                    button: Some("right".into()),
                    element_index: Some(7),
                    ..Default::default()
                },
                ClickParams {
                    click_count: Some(2),
                    element_index: Some(7),
                    ..Default::default()
                },
            ] {
                assert!(matches!(
                    backend.resolve_click_target(&params).unwrap(),
                    ClickTarget::Coordinates(_, _)
                ));
            }
        }
    }

    #[test]
    fn element_click_does_not_replace_pointer_with_non_activation_action() {
        let backend = ComputerUseLinux::default();
        backend.cache_nodes(&[node_with_actions(
            7,
            Some(Bounds {
                x: 10,
                y: 20,
                width: 20,
                height: 20,
            }),
            vec![AccessibilityAction {
                index: 0,
                name: "show-menu".into(),
                description: String::new(),
                keybinding: String::new(),
            }],
        )]);
        assert!(matches!(
            backend
                .resolve_click_target(&ClickParams {
                    element_index: Some(7),
                    ..Default::default()
                })
                .unwrap(),
            ClickTarget::Coordinates(20, 30)
        ));
    }

    #[test]
    fn entry_submit_and_slider_jump_are_not_pointer_clicks() {
        for (role, action_name) in [
            ("entry", "activate"),
            ("spin button", "activate"),
            ("slider", "jump"),
        ] {
            let backend = ComputerUseLinux::default();
            let mut target = node_with_actions(
                7,
                Some(Bounds {
                    x: 10,
                    y: 20,
                    width: 100,
                    height: 40,
                }),
                vec![AccessibilityAction {
                    index: 0,
                    name: action_name.into(),
                    description: String::new(),
                    keybinding: String::new(),
                }],
            );
            target.role = role.into();
            backend.cache_nodes(&[target]);
            assert!(matches!(
                backend
                    .resolve_click_target(&ClickParams {
                        element_index: Some(7),
                        ..Default::default()
                    })
                    .unwrap(),
                ClickTarget::Coordinates(60, 40)
            ));
            let mut node = backend.last_snapshot.lock().unwrap().nodes[0].clone();
            for bounds in [
                None,
                Some(Bounds {
                    x: i32::MIN,
                    y: i32::MIN,
                    width: 1,
                    height: 1,
                }),
                Some(Bounds {
                    x: 0,
                    y: 0,
                    width: 0,
                    height: 0,
                }),
            ] {
                node.bounds = bounds;
                backend.cache_nodes(&[node.clone()]);
                assert!(backend
                    .resolve_click_target(&ClickParams {
                        element_index: Some(7),
                        ..Default::default()
                    })
                    .is_err());
            }
        }
    }

    #[test]
    fn click_target_falls_back_to_primary_action_with_sentinel_bounds() {
        let backend = ComputerUseLinux::default();
        backend.cache_nodes(&[node_with_actions(
            7,
            Some(Bounds {
                x: i32::MIN,
                y: i32::MIN,
                width: 1,
                height: 1,
            }),
            vec![AccessibilityAction {
                index: 0,
                name: "Click".to_string(),
                description: "Clicks the button".to_string(),
                keybinding: String::new(),
            }],
        )]);

        let target = backend
            .resolve_click_target(&ClickParams {
                element_index: Some(7),
                ..Default::default()
            })
            .unwrap();

        match target {
            ClickTarget::PrimaryAction {
                object_ref,
                action_name,
                action_index,
            } => {
                assert_eq!(object_ref, ":1.7/org/a11y/atspi/accessible/7");
                assert_eq!(action_name.as_deref(), Some("Click"));
                assert_eq!(action_index, 0);
            }
            ClickTarget::Coordinates(_, _) => {
                panic!("expected AT-SPI primary-action fallback")
            }
        }
    }

    #[test]
    fn click_target_requires_bounds_for_non_plain_clicks() {
        let backend = ComputerUseLinux::default();
        backend.cache_nodes(&[node_with_actions(
            7,
            None,
            vec![AccessibilityAction {
                index: 0,
                name: "Click".to_string(),
                description: "Clicks the button".to_string(),
                keybinding: String::new(),
            }],
        )]);

        let error = backend
            .resolve_click_target(&ClickParams {
                element_index: Some(7),
                button: Some("right".to_string()),
                ..Default::default()
            })
            .unwrap_err();

        assert!(error.contains("No clickable bounds cached for element_index 7"));
    }

    #[test]
    fn xdotool_pointer_command_is_single_no_sync_move_and_click() {
        assert_eq!(
            xdotool_pointer_click_args(1550, 930, 3, Some("right")),
            Some(vec![
                "mousemove".to_string(),
                "--".to_string(),
                "1550".to_string(),
                "930".to_string(),
                "click".to_string(),
                "--repeat".to_string(),
                "3".to_string(),
                "3".to_string(),
            ])
        );
    }

    #[test]
    fn xdotool_pointer_supports_only_standard_buttons() {
        assert!(xdotool_pointer_click_args(10, 20, 1, None).is_some());
        assert!(xdotool_pointer_click_args(10, 20, 1, Some("middle")).is_some());
        assert!(xdotool_pointer_click_args(10, 20, 1, Some("right")).is_some());
    }

    #[test]
    fn extended_pointer_buttons_do_not_construct_xdotool_commands() {
        for button in ["side", "extra", "forward", "back"] {
            assert_eq!(xdotool_pointer_click_args(10, 20, 1, Some(button)), None);
        }
    }

    #[test]
    fn xdotool_scroll_uses_wheel_buttons_instead_of_a_valuator_event() {
        assert_eq!(
            xdotool_pointer_scroll_args(Some((1550, 930)), 5, ScrollDirection::Up),
            vec![
                "mousemove".to_string(),
                "--".to_string(),
                "1550".to_string(),
                "930".to_string(),
                "click".to_string(),
                "--delay".to_string(),
                "12".to_string(),
                "--repeat".to_string(),
                "5".to_string(),
                "4".to_string(),
            ]
        );
        assert_eq!(
            xdotool_pointer_scroll_args(Some((-20, 15)), 1, ScrollDirection::Down),
            vec![
                "mousemove".to_string(),
                "--".to_string(),
                "-20".to_string(),
                "15".to_string(),
                "click".to_string(),
                "--delay".to_string(),
                "12".to_string(),
                "--repeat".to_string(),
                "1".to_string(),
                "5".to_string(),
            ]
        );
        assert_eq!(
            xdotool_pointer_scroll_args(None, 3, ScrollDirection::Left),
            vec![
                "click".to_string(),
                "--delay".to_string(),
                "12".to_string(),
                "--repeat".to_string(),
                "3".to_string(),
                "6".to_string(),
            ]
        );
        assert_eq!(
            xdotool_pointer_scroll_args(None, 0, ScrollDirection::Right),
            vec![
                "click".to_string(),
                "--delay".to_string(),
                "12".to_string(),
                "--repeat".to_string(),
                "1".to_string(),
                "7".to_string(),
            ]
        );
    }

    #[test]
    fn scroll_pages_are_bounded_before_unit_conversion() {
        assert_eq!(scroll_units(None).unwrap(), 5);
        assert_eq!(scroll_units(Some(1.0)).unwrap(), 5);
        assert_eq!(scroll_units(Some(-1.0)).unwrap(), 5);
        assert_eq!(scroll_units(Some(0.0)).unwrap(), 1);
        assert_eq!(scroll_units(Some(0.1)).unwrap(), 1);
        assert_eq!(scroll_units(Some(100.0)).unwrap(), 500);
        assert_eq!(scroll_units(Some(-100.0)).unwrap(), 500);

        for pages in [
            100.000_001,
            -100.000_001,
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
        ] {
            assert!(scroll_units(Some(pages)).is_err(), "accepted {pages}");
        }
    }

    #[test]
    fn maximum_scroll_timeout_is_bounded() {
        let repeat = u32::try_from(scroll_units(Some(MAX_SCROLL_PAGES)).unwrap()).unwrap();

        assert_eq!(repeat, 500);
        assert_eq!(
            xdotool_scroll_timeout(repeat, XDOTOOL_SCROLL_CLICK_DELAY_MS),
            Duration::from_secs(16)
        );
    }

    #[test]
    fn xdotool_scroll_timeout_covers_the_gap_after_every_click() {
        assert_eq!(
            xdotool_scroll_timeout(1, 12),
            INPUT_COMMAND_TIMEOUT + Duration::from_millis(12)
        );
        assert_eq!(
            xdotool_scroll_timeout(5, 100),
            INPUT_COMMAND_TIMEOUT + Duration::from_millis(500)
        );
        assert_eq!(
            xdotool_scroll_timeout(0, 12),
            INPUT_COMMAND_TIMEOUT + Duration::from_millis(12)
        );
    }

    #[test]
    fn xdotool_key_spec_maps_named_keys_to_x11_keysyms() {
        assert_eq!(xdotool_key_spec("Return"), Some("Return".to_string()));
        assert_eq!(xdotool_key_spec("enter"), Some("Return".to_string()));
        assert_eq!(xdotool_key_spec("Escape"), Some("Escape".to_string()));
        assert_eq!(xdotool_key_spec("backspace"), Some("BackSpace".to_string()));
        assert_eq!(xdotool_key_spec("PageUp"), Some("Page_Up".to_string()));
        assert_eq!(xdotool_key_spec("ArrowLeft"), Some("Left".to_string()));
        assert_eq!(xdotool_key_spec("f5"), Some("F5".to_string()));
        assert_eq!(xdotool_key_spec("space"), Some("space".to_string()));
    }

    #[test]
    fn xdotool_key_spec_maps_chords_with_modifier_prefixes() {
        assert_eq!(xdotool_key_spec("ctrl+a"), Some("ctrl+a".to_string()));
        assert_eq!(xdotool_key_spec("Ctrl+S"), Some("ctrl+s".to_string()));
        assert_eq!(
            xdotool_key_spec("Ctrl+Shift+P"),
            Some("ctrl+shift+p".to_string())
        );
        assert_eq!(
            xdotool_key_spec("Meta+Return"),
            Some("super+Return".to_string())
        );
        assert_eq!(xdotool_key_spec("Alt+F4"), Some("alt+F4".to_string()));
    }

    #[test]
    fn xdotool_key_spec_maps_bare_modifier_to_single_keysym() {
        assert_eq!(xdotool_key_spec("Super"), Some("super".to_string()));
        assert_eq!(xdotool_key_spec("ctrl"), Some("ctrl".to_string()));
    }

    #[test]
    fn xdotool_type_keeps_a_per_character_delay_so_xtest_events_stay_ordered() {
        // Issue #147: `--delay 0` delivers characters out of order on some X
        // servers. The default must stay non-zero.
        const { assert!(XDOTOOL_TYPE_DELAY_MS > 0) };
        let text = "x".repeat(10_000);
        let args = xdotool_type_args_with_delay(&text, XDOTOOL_TYPE_DELAY_MS);

        assert_eq!(
            &args[..5],
            ["type", "--clearmodifiers", "--delay", "12", "--"]
        );
        assert_eq!(args[3], XDOTOOL_TYPE_DELAY_MS.to_string());
        assert_eq!(args[5], text);
    }

    #[test]
    fn xdotool_type_uses_return_for_lf_and_crlf() {
        let args = xdotool_type_args_with_delay("L1\nL2\r\nL3\r", 12);
        assert_eq!(args[5], "L1\rL2\rL3\r");
        assert_eq!(
            xdotool_type_args_with_delay("quotes '\" and literal \\n: שלום", 12)[5],
            "quotes '\" and literal \\n: שלום"
        );
    }

    #[test]
    fn xdotool_type_timeout_grows_with_text_length() {
        assert_eq!(xdotool_type_timeout("", 12), INPUT_COMMAND_TIMEOUT);
        assert_eq!(
            xdotool_type_timeout(&"x".repeat(1_000), 12),
            INPUT_COMMAND_TIMEOUT + Duration::from_secs(12)
        );
        assert_eq!(
            xdotool_type_timeout(&"x".repeat(1_000), 0),
            INPUT_COMMAND_TIMEOUT
        );
    }

    #[tokio::test]
    async fn xdotool_failures_report_the_cause() {
        let args = vec!["-c".to_string(), "echo boom >&2; exit 9".to_string()];
        let error = run_xdotool_program(Path::new("/bin/sh"), &args, INPUT_COMMAND_TIMEOUT)
            .await
            .unwrap_err();
        assert_eq!(error, "boom");

        let error = run_xdotool_program(
            Path::new("/definitely/missing/xdotool"),
            &[],
            INPUT_COMMAND_TIMEOUT,
        )
        .await
        .unwrap_err();
        assert!(error.contains("install xdotool"), "{error}");
    }

    #[test]
    fn electron_editable_state_does_not_require_the_editable_text_interface() {
        let element = FocusedElementSummary {
            role: "entry".to_string(),
            name: None,
            editable: false,
            states: vec!["focused".to_string(), "editable".to_string()],
            is_terminal: false,
        };
        assert!(focused_element_is_editable(&element));
        assert!(!describe_focused_element(&element, true).starts_with("WARNING:"));
    }

    #[tokio::test]
    async fn nul_text_is_rejected_before_window_targeting() {
        let server = ComputerUseLinux::default();
        let params: TypeTextParams = serde_json::from_value(
            serde_json::json!({"text": "prefix\0suffix", "window_id": u64::MAX}),
        )
        .unwrap();
        let Json(result) = server.type_text(Parameters(params)).await;
        assert!(!result.ok);
        assert!(
            result.message.contains("NUL characters are unsupported"),
            "{}",
            result.message
        );
    }

    #[tokio::test]
    async fn cancelling_xdotool_wait_kills_the_child() {
        let dir = std::env::temp_dir().join(format!(
            "computer-use-linux-xdotool-cancel-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
        ));
        std::fs::create_dir_all(&dir).expect("create command test directory");
        let xdotool = dir.join("xdotool");
        let pid_path = dir.join("pid");
        std::fs::write(
            &xdotool,
            format!(
                "#!/bin/sh\nprintf '%s' $$ > '{}'\nexec sleep 60\n",
                pid_path.display()
            ),
        )
        .expect("write fake xdotool");
        std::fs::set_permissions(&xdotool, std::fs::Permissions::from_mode(0o700))
            .expect("make fake xdotool executable");

        let task =
            tokio::spawn(
                async move { run_xdotool_program(&xdotool, &[], INPUT_COMMAND_TIMEOUT).await },
            );
        let mut pid = None;
        for _ in 0..200 {
            if let Ok(value) = std::fs::read_to_string(&pid_path) {
                pid = value.parse::<u32>().ok();
                if pid.is_some() {
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let pid = pid.expect("fake xdotool did not record its pid");
        task.abort();
        let _ = task.await;

        for _ in 0..50 {
            if !Path::new(&format!("/proc/{pid}")).exists() {
                let _ = std::fs::remove_dir_all(dir);
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        unsafe {
            libc::kill(pid as i32, libc::SIGKILL);
        }
        let _ = std::fs::remove_dir_all(dir);
        panic!("cancelled xdotool child {pid} was not killed");
    }

    #[tokio::test]
    async fn cancelling_between_press_and_release_keeps_input_locked() {
        let lock = std::sync::Arc::new(tokio::sync::Mutex::new(()));
        let guard = std::sync::Arc::clone(&lock).lock_owned().await;
        let (pressed_tx, pressed_rx) = tokio::sync::oneshot::channel();
        let (allow_release_tx, allow_release_rx) = tokio::sync::oneshot::channel();
        let (released_tx, released_rx) = tokio::sync::oneshot::channel();

        let waiter = tokio::spawn(async move {
            run_cancellation_safe_input(guard, async move {
                let _ = pressed_tx.send(());
                let _ = allow_release_rx.await;
                let _ = released_tx.send(());
                Ok::<(), String>(())
            })
            .await
        });

        pressed_rx.await.expect("press command did not finish");
        waiter.abort();
        let _ = waiter.await;
        assert!(
            lock.try_lock().is_err(),
            "input lock was released while the paired operation was incomplete"
        );

        allow_release_tx
            .send(())
            .expect("release command stopped on caller cancellation");
        timeout(Duration::from_secs(1), released_rx)
            .await
            .expect("release command did not finish")
            .expect("release command dropped its completion marker");
        timeout(
            Duration::from_secs(1),
            std::sync::Arc::clone(&lock).lock_owned(),
        )
        .await
        .expect("input lock remained held after the operation finished");
    }

    #[test]
    fn xdotool_key_spec_rejects_keys_outside_the_grammar() {
        for key in [
            "NotAKey",
            "",
            "ctrl+NotAKey",
            "f13",
            "hyper+a",
            "ctrl+shift",
        ] {
            assert_eq!(xdotool_key_spec(key), None, "accepted {key:?}");
        }
        // Empty chord parts are dropped, so a trailing '+' is a bare press.
        assert_eq!(xdotool_key_spec("ctrl+").as_deref(), Some("ctrl"));
    }

    #[tokio::test]
    async fn command_wait_drains_output_before_exit() {
        let mut command = tokio::process::Command::new("sh");
        command.args(["-c", "yes noisy | head -c 200000 >&2; exit 7"]);
        command.stdout(Stdio::piped());
        command.stderr(Stdio::piped());

        let output = crate::command_runner::output_with_timeout(
            command,
            "run test command",
            Duration::from_secs(5),
        )
        .await
        .expect("child should exit before timeout");

        assert_eq!(output.status.code(), Some(7));
        assert!(output.stderr.len() >= 100_000);
    }

    #[test]
    fn perform_action_leaves_an_omitted_action_to_the_primary_verb() {
        assert_eq!(requested_or_primary_action(None), None);
        assert_eq!(requested_or_primary_action(Some("   ")), None);
        assert_eq!(
            requested_or_primary_action(Some(" show-menu ")),
            Some("show-menu")
        );
    }

    #[test]
    fn plain_click_uses_the_click_verb_wherever_it_is_listed() {
        let action = |index: i32, name: &str| AccessibilityAction {
            index,
            name: name.to_string(),
            description: String::new(),
            keybinding: String::new(),
        };
        let chromium = [action(0, "showContextMenu"), action(1, "click")];
        assert_eq!(
            primary_click_action(&chromium).map(|action| action.index),
            Some(1)
        );
        assert!(primary_click_action(&[action(0, "showContextMenu")]).is_none());
    }

    #[test]
    fn disappears_needs_a_complete_snapshot() {
        assert!(presence_condition_met(
            WaitCondition::Disappears,
            false,
            false
        ));
        assert!(!presence_condition_met(
            WaitCondition::Disappears,
            false,
            true
        ));
        assert!(!presence_condition_met(
            WaitCondition::Disappears,
            true,
            false
        ));
        assert!(presence_condition_met(WaitCondition::Appears, true, true));
        assert!(!presence_condition_met(
            WaitCondition::Appears,
            false,
            false
        ));
    }

    #[test]
    fn element_identifier_overrides_cached_object_ref() {
        let backend = ComputerUseLinux::default();
        backend.cache_nodes(&[node(7, None)]);

        let object_ref = backend
            .resolve_object_ref(
                Some(7),
                Some(":1.99/org/a11y/atspi/accessible/3"),
                &ElementSelector::default(),
                ElementResolvePurpose::Action,
            )
            .unwrap();

        assert_eq!(object_ref, ":1.99/org/a11y/atspi/accessible/3");
    }

    #[test]
    fn element_index_resolves_to_cached_object_ref() {
        let backend = ComputerUseLinux::default();
        backend.cache_nodes(&[node(7, None)]);

        let object_ref = backend
            .resolve_object_ref(
                Some(7),
                None,
                &ElementSelector::default(),
                ElementResolvePurpose::Action,
            )
            .unwrap();

        assert_eq!(object_ref, ":1.7/org/a11y/atspi/accessible/7");
    }

    #[test]
    fn semantic_selector_resolves_unique_cached_node_by_role_and_name() {
        let backend = ComputerUseLinux::default();
        let mut search_entry = node(7, None);
        search_entry.role = "entry".to_string();
        search_entry.name = Some("Search files".to_string());
        search_entry.supports_editable_text = true;
        backend.cache_nodes(&[search_entry]);

        let object_ref = backend
            .resolve_object_ref(
                None,
                None,
                &ElementSelector {
                    role: Some("entry"),
                    name: Some("search"),
                    ..Default::default()
                },
                ElementResolvePurpose::SetValue,
            )
            .unwrap();

        assert_eq!(object_ref, ":1.7/org/a11y/atspi/accessible/7");
    }

    #[test]
    fn semantic_selector_prefers_actionable_match() {
        let backend = ComputerUseLinux::default();
        let mut label = node(4, None);
        label.role = "label".to_string();
        label.name = Some("Close".to_string());
        let mut button = node_with_actions(7, None, vec![click_action()]);
        button.role = "push button".to_string();
        button.name = Some("Close".to_string());
        backend.cache_nodes(&[label, button]);

        let object_ref = backend
            .resolve_object_ref(
                None,
                None,
                &ElementSelector {
                    name: Some("close"),
                    ..Default::default()
                },
                ElementResolvePurpose::Action,
            )
            .unwrap();

        assert_eq!(object_ref, ":1.7/org/a11y/atspi/accessible/7");
    }

    #[test]
    fn semantic_selector_prefers_editable_match() {
        let backend = ComputerUseLinux::default();
        let mut label = node(4, None);
        label.role = "label".to_string();
        label.name = Some("Search".to_string());
        let mut entry = node(7, None);
        entry.role = "entry".to_string();
        entry.name = Some("Search".to_string());
        entry.supports_editable_text = true;
        backend.cache_nodes(&[label, entry]);

        let object_ref = backend
            .resolve_object_ref(
                None,
                None,
                &ElementSelector {
                    name: Some("search"),
                    ..Default::default()
                },
                ElementResolvePurpose::SetValue,
            )
            .unwrap();

        assert_eq!(object_ref, ":1.7/org/a11y/atspi/accessible/7");
    }

    #[test]
    fn semantic_selector_reports_ambiguous_matches() {
        let backend = ComputerUseLinux::default();
        let mut first = node_with_actions(7, None, vec![click_action()]);
        first.name = Some("Close".to_string());
        let mut second = node_with_actions(9, None, vec![click_action()]);
        second.name = Some("Close".to_string());
        backend.cache_nodes(&[first, second]);

        let error = backend
            .resolve_object_ref(
                None,
                None,
                &ElementSelector {
                    name: Some("close"),
                    ..Default::default()
                },
                ElementResolvePurpose::Action,
            )
            .unwrap_err();

        assert!(error.contains("matched multiple cached nodes"));
        assert!(error.contains("element_index 7"));
        assert!(error.contains("element_index 9"));
    }

    #[test]
    fn semantic_click_selector_prefers_native_activation() {
        let backend = ComputerUseLinux::default();
        let mut button = node_with_actions(
            7,
            Some(Bounds {
                x: 10,
                y: 20,
                width: 100,
                height: 40,
            }),
            vec![click_action()],
        );
        button.name = Some("Run".to_string());
        backend.cache_nodes(&[button]);

        let target = backend
            .resolve_click_target(&ClickParams {
                role: Some("button".to_string()),
                name: Some("run".to_string()),
                ..Default::default()
            })
            .unwrap();

        assert!(matches!(
            target,
            ClickTarget::PrimaryAction {
                action_index: 0,
                ..
            }
        ));
    }

    fn focused(role: &str, is_terminal: bool) -> FocusedElementSummary {
        FocusedElementSummary {
            role: role.to_string(),
            name: None,
            editable: false,
            states: vec!["focused".to_string()],
            is_terminal,
        }
    }

    #[test]
    fn describe_focused_element_editable() {
        let element = FocusedElementSummary {
            role: "text".to_string(),
            name: Some("Message".to_string()),
            editable: true,
            states: vec!["focused".to_string()],
            is_terminal: false,
        };
        let described = describe_focused_element(&element, true);
        assert!(described.contains("editable"));
        assert!(!described.contains("WARNING"));
    }

    #[test]
    fn describe_focused_terminal_accepts_keyboard_input_without_editable_text() {
        let mut element = focused("terminal", true);
        element.name = Some("Terminal".to_string());
        let text = describe_focused_element(&element, true);
        assert!(!text.contains("WARNING"), "{text}");
        assert!(text.contains("terminal keyboard input"), "{text}");
    }

    #[test]
    fn describe_focused_element_warns_on_non_editable_when_typing() {
        let element = FocusedElementSummary {
            role: "push button".to_string(),
            name: Some("OK".to_string()),
            editable: false,
            states: vec!["focused".to_string()],
            is_terminal: false,
        };
        let described = describe_focused_element(&element, true);
        assert!(described.contains("WARNING"));
        assert!(described.contains("not editable"));
    }

    #[test]
    fn describe_focused_element_no_warning_for_press_key() {
        let element = FocusedElementSummary {
            role: "push button".to_string(),
            name: None,
            editable: false,
            states: vec![],
            is_terminal: false,
        };
        let described = describe_focused_element(&element, false);
        assert!(!described.contains("WARNING"));
    }

    #[test]
    fn relative_scroll_translates_coordinates() {
        let mut params = ScrollParams {
            element_index: None,
            x: Some(10),
            y: Some(20),
            direction: "down".to_string(),
            pages: None,
            window_id: Some(1),
            pid: None,
            app_id: None,
            wm_class: None,
            window_title: None,
            relative: Some(true),
        };
        apply_window_relative_scroll_coordinates(&mut params, (100, 200, 800, 600)).unwrap();
        assert_eq!(params.x, Some(110));
        assert_eq!(params.y, Some(220));
    }

    #[test]
    fn window_targeted_scroll_defaults_to_window_center() {
        let mut params = ScrollParams {
            element_index: None,
            x: None,
            y: None,
            direction: "down".to_string(),
            pages: None,
            window_id: Some(1),
            pid: None,
            app_id: None,
            wm_class: None,
            window_title: None,
            relative: None,
        };
        apply_window_center_scroll_point(&mut params, (100, 200, 800, 600)).unwrap();
        assert_eq!(params.x, Some(500));
        assert_eq!(params.y, Some(500));
    }

    #[test]
    fn window_targeted_scroll_with_empty_capture_rect_errors() {
        let mut params = ScrollParams {
            element_index: None,
            x: None,
            y: None,
            direction: "down".to_string(),
            pages: None,
            window_id: Some(1),
            pid: None,
            app_id: None,
            wm_class: None,
            window_title: None,
            relative: None,
        };
        let error = apply_window_center_scroll_point(&mut params, (0, 0, 0, 0)).unwrap_err();
        assert!(error.contains("pass x/y explicitly"));
        assert_eq!(params.x, None);
        assert_eq!(params.y, None);
    }

    #[test]
    fn relative_scroll_rejects_out_of_bounds() {
        let mut params = ScrollParams {
            element_index: None,
            x: Some(801),
            y: Some(20),
            direction: "down".to_string(),
            pages: None,
            window_id: Some(1),
            pid: None,
            app_id: None,
            wm_class: None,
            window_title: None,
            relative: Some(true),
        };
        assert!(
            apply_window_relative_scroll_coordinates(&mut params, (100, 200, 800, 600)).is_err()
        );
    }

    #[test]
    fn shell_execution_requires_exact_operator_opt_in() {
        assert!(shell_execution_enabled_value(Some("1")));
        for value in [None, Some(""), Some("0"), Some("true"), Some("yes")] {
            assert!(!shell_execution_enabled_value(value));
        }
    }

    #[tokio::test]
    async fn shell_execution_clears_ambient_credentials_and_accepts_explicit_env() {
        let _serial = SHELL_ENV_TEST_LOCK.lock().await;
        let _enabled = EnvVarGuard::set(SHELL_ENABLE_ENV, "1");
        let _credential = EnvVarGuard::set("OPENAI_API_KEY", "test-ambient-credential");
        let output = execute_shell(RunShellParams {
            command: "printf '%s|%s' \"${OPENAI_API_KEY-unset}\" \"$EXPLICIT_VALUE\"".into(),
            cwd: None,
            env: BTreeMap::from([("EXPLICIT_VALUE".into(), "test-explicit-value".into())]),
            timeout_seconds: Some(2),
        })
        .await;
        assert!(output.ok, "{:?}", output.error);
        assert_eq!(output.stdout, "unset|test-explicit-value");
        assert_eq!(output.exit_code, Some(0));
    }

    #[tokio::test]
    async fn shell_execution_disabled_never_runs_requested_command() {
        async fn assert_refused() {
            let output = execute_shell(RunShellParams {
                command: "printf must-not-run".into(),
                cwd: None,
                env: BTreeMap::new(),
                timeout_seconds: None,
            })
            .await;
            assert!(!output.ok);
            assert!(output.stdout.is_empty());
            assert!(output
                .error
                .unwrap()
                .contains("shell execution is disabled"));
        }

        let _serial = SHELL_ENV_TEST_LOCK.lock().await;
        {
            let _disabled = EnvVarGuard::set(SHELL_ENABLE_ENV, "0");
            assert_refused().await;
        }
        {
            let _unset = EnvVarGuard::unset(SHELL_ENABLE_ENV);
            assert_refused().await;
        }
    }

    #[test]
    fn shell_environment_policy_excludes_ambient_credentials() {
        for allowed in [
            "PATH",
            "HOME",
            "LANG",
            "LC_ALL",
            "XDG_RUNTIME_DIR",
            "DISPLAY",
            "DBUS_SESSION_BUS_ADDRESS",
        ] {
            assert!(inherited_shell_environment_allowed(allowed));
        }
        for secret in [
            "AWS_SECRET_ACCESS_KEY",
            "GITHUB_TOKEN",
            "OPENAI_API_KEY",
            "SSH_AUTH_SOCK",
            "LD_PRELOAD",
        ] {
            assert!(!inherited_shell_environment_allowed(secret));
        }
    }

    #[test]
    fn shell_environment_skips_non_utf8_entries() {
        use std::os::unix::ffi::OsStringExt;

        let variables = [
            (OsString::from("PATH"), OsString::from("/usr/bin")),
            (OsString::from_vec(vec![0xff]), OsString::from("ignored")),
            (OsString::from("LANG"), OsString::from_vec(vec![0xff])),
            (OsString::from("GITHUB_TOKEN"), OsString::from("secret")),
        ];

        assert_eq!(
            inherited_shell_environment(variables),
            vec![("PATH".to_string(), "/usr/bin".to_string())]
        );
    }

    #[test]
    fn shell_environment_names_follow_exec_rules() {
        for valid in ["A", "_PRIVATE", "NAME_2"] {
            assert!(valid_environment_name(valid));
        }
        for invalid in ["", "2FAST", "BAD-NAME", "A=B", "naïve"] {
            assert!(!valid_environment_name(invalid));
        }
    }

    #[test]
    fn shell_output_is_bounded_before_mcp_serialization() {
        let oversized = vec![b'x'; SHELL_RESPONSE_STREAM_BYTES + 17];
        let (visible, truncated) = bounded_shell_stream(&oversized);
        assert!(truncated);
        assert_eq!(visible.len(), SHELL_RESPONSE_STREAM_BYTES);

        let (visible, truncated) = bounded_shell_stream(b"ok");
        assert!(!truncated);
        assert_eq!(visible, "ok");
    }

    #[test]
    fn shell_audit_digest_is_stable_and_does_not_echo_command_text() {
        let digest = shell_command_sha256("printf secret");
        assert_eq!(digest.len(), 64);
        assert!(digest
            .chars()
            .all(|character| character.is_ascii_hexdigit()));
        assert!(!digest.contains("secret"));
        assert_eq!(digest, shell_command_sha256("printf secret"));
    }
}

#[cfg(test)]
mod accessibility_tree_note_tests {
    use super::{truncated_accessibility_tree_note, unscoped_accessibility_tree_warning};

    #[test]
    fn unscoped_warning_only_when_tree_succeeded_without_a_scope() {
        let warning = unscoped_accessibility_tree_warning(false, true, false).unwrap();
        assert!(warning.starts_with("WARNING:"));
        assert!(warning.contains("no app target scoped"));
        assert!(warning.contains("app_name_or_bundle_identifier"));
        assert!(warning.contains("window_id, pid, app_id, wm_class, title"));

        for target_requested in [false, true] {
            assert!(unscoped_accessibility_tree_warning(true, true, target_requested).is_none());
            assert!(unscoped_accessibility_tree_warning(false, false, target_requested).is_none());
            assert!(unscoped_accessibility_tree_warning(true, false, target_requested).is_none());
        }
    }

    /// A caller that already passed a target must not be told to pass a target;
    /// that advice would replay the same desktop-wide snapshot.
    #[test]
    fn unscoped_warning_with_a_target_points_at_accessibility_support() {
        let warning = unscoped_accessibility_tree_warning(false, true, true).unwrap();
        assert!(warning.starts_with("WARNING:"));
        assert!(warning.contains("matched no AT-SPI application root"));
        assert!(warning.contains("force-renderer-accessibility"));
        assert!(warning.contains("list_apps"));
        assert!(!warning.contains("Pass app_name_or_bundle_identifier or a window target"));
    }

    #[test]
    fn truncated_note_only_when_the_node_cap_was_hit() {
        let note = truncated_accessibility_tree_note(true).unwrap();
        assert!(note.contains("raise max_nodes or max_depth"));
        assert!(
            !note.contains("smaller max_nodes"),
            "shrinking the cap belongs to the unscoped warning, not the truncation note"
        );
        assert!(truncated_accessibility_tree_note(false).is_none());
    }
}

#[cfg(test)]
mod focus_probe_feedback_tests {
    use super::{focus_probe_feedback, FocusProbe, FocusedElementSummary};

    #[test]
    fn only_a_complete_empty_search_warns() {
        let none = focus_probe_feedback(&FocusProbe::NoneFocused, true, false);
        assert!(none.starts_with("WARNING:"), "{none}");

        for probe in [FocusProbe::Incomplete, FocusProbe::NoAccessibleApp] {
            let text = focus_probe_feedback(&probe, true, false);
            assert!(
                !text.contains("WARNING"),
                "{probe:?} must not claim lost input: {text}"
            );
            assert!(text.contains("could not be verified"), "{text}");
        }
        assert!(
            focus_probe_feedback(&FocusProbe::Incomplete, true, false).contains("search limits")
        );
        assert!(
            focus_probe_feedback(&FocusProbe::NoAccessibleApp, true, false)
                .contains("--force-renderer-accessibility")
        );
    }

    #[test]
    fn review_exact_focus_only_softens_shell_only_tree() {
        for expects_editable in [false, true] {
            let text = focus_probe_feedback(&FocusProbe::NoneFocused, expects_editable, true);
            assert!(text.starts_with("WARNING:"), "{text}");
            let text = focus_probe_feedback(&FocusProbe::ShellOnly, expects_editable, false);
            assert!(text.starts_with("WARNING:"), "{text}");
            let text = focus_probe_feedback(&FocusProbe::ShellOnly, expects_editable, true);
            assert!(!text.contains("WARNING"), "{text}");
            assert!(!text.contains("landed nowhere"), "{text}");
            assert!(text.contains("before input"), "{text}");
            assert!(text.contains("Field contents were not verified"), "{text}");
        }
    }

    #[test]
    fn found_element_is_described() {
        let element = FocusedElementSummary {
            role: "text".to_string(),
            name: None,
            editable: true,
            states: vec!["focused".to_string()],
            is_terminal: false,
        };
        let text = focus_probe_feedback(&FocusProbe::Found(element), true, false);
        assert!(text.contains("text"), "{text}");
        assert!(!text.starts_with("WARNING"), "{text}");
    }
}

#[cfg(test)]
mod node_target_scope_tests {
    use super::*;

    fn cached_node(index: u32, object_ref: &str) -> AccessibilityNode {
        AccessibilityNode {
            index,
            parent_index: None,
            depth: 1,
            object_ref: object_ref.to_string(),
            role: "push button".to_string(),
            name: Some("Hit me".to_string()),
            description: None,
            child_count: 0,
            bounds: None,
            states: vec!["enabled".to_string()],
            actions: Vec::new(),
            value: None,
            text: None,
            supports_editable_text: false,
        }
    }

    #[tokio::test]
    async fn index_from_another_apps_snapshot_is_rejected() {
        let backend = ComputerUseLinux::default();
        let node = cached_node(2, ":1.9/org/a11y/atspi/accessible/42");
        backend.cache_snapshot(std::slice::from_ref(&node), Some(2_690_687));

        let error = backend
            .check_node_target(&node, Some(2_690_664))
            .await
            .unwrap_err();
        assert!(error.contains("pid 2690687"), "{error}");
        assert!(error.contains("target pid 2690664"), "{error}");
        assert!(
            error.contains("Call get_app_state for the target app"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn same_target_or_no_target_is_allowed() {
        let backend = ComputerUseLinux::default();
        let node = cached_node(2, ":1.9/org/a11y/atspi/accessible/42");
        backend.cache_snapshot(std::slice::from_ref(&node), Some(2_690_664));

        assert!(backend
            .check_node_target(&node, Some(2_690_664))
            .await
            .is_ok());
        assert!(backend.check_node_target(&node, None).await.is_ok());
    }

    #[tokio::test]
    async fn unscoped_snapshot_with_unknown_owner_is_allowed() {
        // No snapshot pid, and the owner lookup cannot resolve this ref here,
        // so a mismatch cannot be shown and the action proceeds.
        let backend = ComputerUseLinux::default();
        let node = cached_node(2, ":1.9999/org/a11y/atspi/accessible/42");
        backend.cache_snapshot(std::slice::from_ref(&node), None);

        assert!(backend.check_node_target(&node, Some(1)).await.is_ok());
    }

    #[tokio::test]
    async fn concurrent_snapshot_commit_waits_until_checked_action_finishes() {
        let backend = ComputerUseLinux::default();
        let mut original = cached_node(2, ":1.9/org/a11y/atspi/accessible/42");
        original.bounds = Some(Bounds {
            x: 10,
            y: 20,
            width: 100,
            height: 40,
        });
        original.actions = vec![AccessibilityAction {
            index: 0,
            name: "click".into(),
            description: String::new(),
            keybinding: String::new(),
        }];
        backend
            .commit_snapshot(std::slice::from_ref(&original), Some(10))
            .await;
        // Model the click/scroll operation: it holds this same lock while its
        // target ownership check awaits and until the action has been resolved.
        let input_guard = Arc::clone(&backend.input_operation_lock).lock_owned().await;
        let checked_node = backend
            .cached_node_for(
                Some(2),
                &ElementSelector::default(),
                ElementResolvePurpose::Click,
            )
            .unwrap();
        let barrier = Arc::new(tokio::sync::Barrier::new(2));
        let writer_backend = backend.clone();
        let writer_barrier = Arc::clone(&barrier);
        let writer = tokio::spawn(async move {
            let replacement = cached_node(2, ":1.20/org/a11y/atspi/accessible/99");
            writer_barrier.wait().await;
            writer_backend
                .commit_snapshot(&[replacement], Some(20))
                .await;
        });
        barrier.wait().await;
        tokio::task::yield_now().await;
        assert!(
            !writer.is_finished(),
            "snapshot publication must wait for input"
        );
        assert!(backend
            .check_node_target(&checked_node, Some(10))
            .await
            .is_ok());
        let params = ClickParams {
            element_index: Some(2),
            ..Default::default()
        };
        assert!(matches!(backend.resolve_click_target(&params).unwrap(),
            ClickTarget::PrimaryAction { object_ref, .. } if object_ref == original.object_ref));
        assert_eq!(
            backend
                .resolve_optional_target_point(None, None, Some(2))
                .unwrap(),
            Some((60, 40))
        );
        // perform_action/set_value resolve the same snapshot while holding the
        // input lock; their explicit object identifiers remain independent.
        for purpose in [
            ElementResolvePurpose::Action,
            ElementResolvePurpose::SetValue,
        ] {
            assert_eq!(
                backend
                    .resolve_object_ref(Some(2), None, &ElementSelector::default(), purpose)
                    .unwrap(),
                original.object_ref
            );
        }
        drop(input_guard);
        writer.await.unwrap();
        let snapshot = backend.last_snapshot.lock().unwrap();
        assert_eq!(snapshot.pid, Some(20));
        assert_eq!(
            snapshot.nodes[0].object_ref,
            ":1.20/org/a11y/atspi/accessible/99"
        );
    }

    #[test]
    fn a_new_snapshot_replaces_the_recorded_pid() {
        let backend = ComputerUseLinux::default();
        backend.cache_snapshot(&[], Some(10));
        backend.cache_snapshot(&[], None);
        assert_eq!(backend.last_snapshot.lock().unwrap().pid, None);
        backend.cache_snapshot(&[], Some(11));
        backend.clear_cached_nodes();
        assert_eq!(backend.last_snapshot.lock().unwrap().pid, None);
    }
}
