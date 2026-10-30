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
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    process::Command as TokioCommand,
    time::{sleep, timeout},
};

const INPUT_COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
/// How long drag/drop_files wait to reread an element before refusing to act.
const ELEMENT_REREAD_TIMEOUT: Duration = Duration::from_secs(2);
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
    /// What the snapshot covered, so a rescan reads the same elements.
    scope: Option<SnapshotScope>,
}

/// One read of an observed scope, with stable ids.
struct Rescan {
    nodes: Vec<AccessibilityNode>,
    root_pid: Option<u32>,
    scope: SnapshotScope,
    /// AT-SPI elements read for it; fewer than `nodes` when only the web
    /// document was read again (`read_page`).
    read: usize,
}

// Settle timing after an action. The tree counts as settled once two
// consecutive reads agree, so an action that changes nothing returns after
// SETTLE_FIRST_WAIT + SETTLE_POLL plus two tree reads (was 120 + 150 ms,
// most of a 0.45-0.65 s click). Each read takes tens of milliseconds
// itself, so shorter gaps mostly add reads, not speed.
/// Wait after an action before the first read, so the app has handled it.
const SETTLE_FIRST_WAIT: Duration = Duration::from_millis(60);
/// Gap between reads while waiting for the tree to stop changing.
const SETTLE_POLL: Duration = Duration::from_millis(90);
/// Longest wait for the tree to settle after an action.
const SETTLE_LIMIT: Duration = Duration::from_secs(2);
/// Longest wait, counted from the action, while the page looks busy with it
/// (a "Salvando…" button, a spinner): async results often take a second.
const BUSY_SETTLE_LIMIT: Duration = Duration::from_secs(6);
/// How long the tree must stay unchanged before a button that only became
/// disabled stops counting as busy; some stay disabled for good.
const BUSY_QUIET: Duration = Duration::from_millis(1_500);
/// Longest a document-only read (`read_document`) may take before the
/// settle loop reads the whole scope instead.
const DOCUMENT_READ_LIMIT: Duration = Duration::from_secs(2);

/// The app filter and limits of an observation, reused when an action has
/// to read the same part of the tree again.
#[derive(Debug, Clone)]
struct SnapshotScope {
    app_filter: Option<String>,
    target_pid: Option<u32>,
    max_nodes: usize,
    max_depth: u32,
    /// How outlines and change reports of this scope are drawn.
    options: crate::page_view::ViewOptions,
}

#[derive(Clone, Default)]
pub struct ComputerUseLinux {
    last_snapshot: Arc<Mutex<CachedAccessibilitySnapshot>>,
    /// Stable `[n]` ids for every element reported so far.
    element_ids: Arc<Mutex<crate::element_ids::ElementIds>>,
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
            // Results are text for the agent to read; output schemas only
            // add kilobytes to every tools/list.
            route.attr.output_schema = None;
        }
        router
    }
}

#[tool_router]
impl ComputerUseLinux {
    #[tool(
        name = "doctor",
        description = "Check the X11 session (display, i3, AT-SPI, xdotool, tesseract). readiness.blockers names the fix for each failure.",
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
        description = "List app processes and AT-SPI application names (usable as app_name_or_bundle_identifier).",
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
        description = "List X11 windows with window_id, title, wm_class, pid, focus and bounds.",
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
        description = "Focus a window by window_id, pid, wm_class or title.",
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
        description = "Observe an app. Returns a header (window, window_id, url, element count) and one line per element: `[n] role \"name\" @x,y wxh {states} actions=... text=...`, nested by indent. Elements outside the visible area say (below), (above) or (off-screen) instead of @x,y; actions by id still reach them. Very long pages omit the farthest ones. [n] is an element id, valid in every tool for the whole session. Browsers show only the web page. A screenshot is attached only when the tree shows little. Scope with app_name_or_bundle_identifier (e.g. chrome) or a window target.",
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
        let options = params.view_options();
        let (output, screenshot) = self.collect_app_state(params).await;
        if tree_format == TreeFormat::Json {
            return app_state_result(output, screenshot);
        }
        let view = crate::page_view::page_view(&output.accessibility_tree, options);
        let window_id = match &output.window_context {
            Some(window) => Some(window.window_id),
            None => focused_window()
                .await
                .ok()
                .flatten()
                .filter(|window| window.title.is_some() && window.title == view.title)
                .map(|window| window.window_id),
        };
        let mut content = vec![ContentBlock::text(app_state_text(
            &output,
            &view,
            window_id,
            screenshot.as_ref(),
        ))];
        if let Some(capture) = screenshot {
            content.push(ContentBlock::image(
                data_url_payload(&capture.data_url),
                capture.mime_type,
            ));
        }
        Ok(CallToolResult::success(content))
    }

    /// Captures the screenshot and accessibility snapshot for `get_app_state`
    /// and publishes the snapshot so element indices resolve against it.
    async fn collect_app_state(
        &self,
        params: GetAppStateParams,
    ) -> (GetAppStateOutput, Option<ScreenshotCapture>) {
        let verbose = params.verbose.unwrap_or(false);
        let mut diagnostics = if verbose {
            self.diagnostics().await
        } else {
            self.cached_diagnostics().await
        };
        let (window_context, window_error, window_permissions_hint) =
            self.resolve_window_context(&params).await;
        let (max_nodes, max_depth) =
            crate::atspi_tree::snapshot_limits(params.max_nodes, params.max_depth);
        let options = params.view_options();
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
                        let (mut nodes, note) = compact_accessibility_tree_with_note(
                            snapshot.nodes,
                            tree_scoped,
                            accessibility_tree_truncated,
                        );
                        self.assign_ids(&mut nodes);
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
            let scope = SnapshotScope {
                app_filter: app_filter.clone(),
                target_pid: window_context.as_ref().and_then(|window| window.pid),
                max_nodes,
                max_depth,
                options,
            };
            self.commit_observation(&accessibility_tree, tree_root_pid, Some(scope))
                .await;
        } else {
            self.commit_snapshot(&[], None).await;
        }
        // Auto: a screenshot only when the tree shows too little to act on
        // (terminals, canvases, apps without accessibility).
        let include_screenshot = params.include_screenshot.unwrap_or_else(|| {
            crate::page_view::page_view(&accessibility_tree, options).content_elements()
                < MIN_CONTENT_ELEMENTS
        });
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
        if accessibility_error.is_some() || screenshot_error.is_some() || window_error.is_some() {
            // The cached readiness may be what is stale; probe again next time.
            self.forget_diagnostics();
        }
        // What the text result prints besides the outline: only what is wrong
        // or incomplete, each with its fix.
        let mut notes = Vec::new();
        if let Some(error) = &accessibility_error {
            notes.push(format!("accessibility tree failed: {error}"));
        }
        if let Some(error) = &window_error {
            notes.push(format!("window target did not resolve: {error}"));
            if let Some(hint) = &window_permissions_hint {
                notes.push(hint.clone());
            }
        }
        if let Some(error) = &screenshot_error {
            notes.push(format!("screenshot failed: {error}"));
        }
        notes.extend(
            unscoped_accessibility_tree_warning(
                tree_scoped,
                accessibility_error.is_none(),
                accessibility_target_requested,
            )
            .map(str::to_string),
        );
        notes.extend(
            truncated_accessibility_tree_note(accessibility_tree_truncated).map(str::to_string),
        );
        notes.extend(shell_only_note.map(str::to_string));
        notes.extend(
            diagnostics
                .readiness
                .blockers
                .iter()
                .map(|blocker| format!("blocker: {blocker}")),
        );
        let output = GetAppStateOutput {
            notes,
            app_name_or_bundle_identifier: params.app_name_or_bundle_identifier,
            window_context,
            window_error,
            window_permissions_hint,
            backend: "linux-atspi".to_string(),
            screenshot: screenshot.as_ref().map(ScreenshotSummary::from),
            screenshot_error,
            accessibility_tree,
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
        description = "Find elements by role/name/text/states (case-insensitive substrings, all must match), including ones hidden from get_app_state (browser UI, off-screen). Returns their `[n]` lines.",
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
        description = "Wait for a slow page (a spinner, \"Carregando…\") to update. Actions already wait for the page to settle and report what changed, so call this only after one whose result is still pending. When you do not know what will appear, call it with no query (condition=changed): it returns the changed lines. With role/name/text/states it waits until a match appears (or disappears with condition=disappears).",
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
        let condition = wait_condition(params.condition, params.query.is_empty());
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
        description = "Read on-screen text with OCR as `@x,y wxh text` lines, for canvases and apps without an accessibility tree. With a window target, coordinates are window-relative (click with relative=true).",
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
        description = "Capture the screen, or a focused window with a window target, as an image. The caption gives scale: click x/y = image pixel / scale.",
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
            if let Err(message) = self
                .ensure_resolvable(
                    params.element_index,
                    &params.selector(),
                    ElementResolvePurpose::Click,
                )
                .await
            {
                return Json(ActionOutput {
                    ok: false,
                    implemented: true,
                    action: "click".to_string(),
                    message,
                    received,
                });
            }
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
        let node = if params.x.zip(params.y).is_none() {
            self.cached_node_for(
                params.element_index,
                &params.selector(),
                ElementResolvePurpose::Click,
            )
        } else {
            None
        };
        let label = node.as_ref().map(crate::outline::element_label);
        let clicked = label.as_deref().unwrap_or("the element");
        let resolved = self.resolve_click_target(&params);
        // An element click whose point is not on the visible page (scrolled
        // away, inside a closed popup, no bounds) would land on no pixel or
        // on the wrong thing: use the element's own action, else scroll it
        // into view, else refuse.
        let needs_fallback = match (&resolved, &node) {
            (Ok(ClickTarget::Coordinates(x, y)), Some(node)) => {
                !self.point_visible_for(node, (*x, *y)).await
            }
            (Err(_), Some(_)) => true,
            _ => false,
        };
        let target = match (resolved, node) {
            (_, Some(node)) if needs_fallback => {
                match self
                    .off_screen_click(
                        &node,
                        clicked,
                        is_plain_left_click(params.button.as_deref(), params.click_count),
                    )
                    .await
                {
                    Ok(OffScreenClick::Invoked(message)) => {
                        return Json(ActionOutput {
                            ok: true,
                            implemented: true,
                            action: "click".to_string(),
                            message,
                            received,
                        });
                    }
                    Ok(OffScreenClick::ScrolledTo(x, y)) => ClickTarget::Coordinates(x, y),
                    Err(message) => {
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
            (Ok(target), _) => target,
            (Err(message), _) => {
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
                            "clicked {clicked} (AT-SPI {})",
                            action_name
                                .as_deref()
                                .filter(|name| !name.is_empty())
                                .unwrap_or("action")
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
        if let Some(note) = self.off_screen_note_for_point(x, y).await {
            return Json(ActionOutput {
                ok: false,
                implemented: true,
                action: "click".to_string(),
                message: format!("{note} Nothing was clicked."),
                received,
            });
        }
        let (input_guard, result) = run_cancellation_safe_input(input_guard, async move {
            run_xdotool(&xdotool_args, INPUT_COMMAND_TIMEOUT).await
        })
        .await;
        let _input_guard = input_guard;
        let mut output = action_result("click", result.map(|output| vec![output]), received);
        if output.ok {
            output.message = match &label {
                Some(label) => format!("clicked {label} at {x},{y} (xdotool)"),
                None => format!("clicked {x},{y} (xdotool)"),
            };
        }
        Json(output)
    }

    async fn perform_action(
        &self,
        Parameters(params): Parameters<ActionParams>,
    ) -> Json<ActionOutput> {
        let requested_action = requested_or_primary_action(params.action.as_deref());
        self.perform_element_action(&params, requested_action).await
    }

    async fn set_value(
        &self,
        Parameters(params): Parameters<SetValueParams>,
    ) -> Json<ActionOutput> {
        let _input_guard = Arc::clone(&self.input_operation_lock).lock_owned().await;
        let received = Some(serde_json::json!(params.clone()));
        if params
            .element_identifier
            .as_deref()
            .is_none_or(|value| value.trim().is_empty())
        {
            if let Err(message) = self
                .ensure_resolvable(
                    params.element_index,
                    &params.selector(),
                    ElementResolvePurpose::SetValue,
                )
                .await
            {
                return Json(ActionOutput {
                    ok: false,
                    implemented: true,
                    action: "set_value".to_string(),
                    message,
                    received,
                });
            }
        }
        let node = match self.resolve_field_node(
            params.element_index,
            params.element_identifier.as_deref(),
            &params.selector(),
        ) {
            Ok(node) => node,
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
        let node = match node {
            Some(node) => node,
            None => {
                let object_ref = params.element_identifier.as_deref().unwrap_or_default();
                match crate::atspi_tree::read_element(object_ref.trim()).await {
                    Ok(node) => node,
                    Err(error) => {
                        return Json(ActionOutput {
                            ok: false,
                            implemented: true,
                            action: "set_value".to_string(),
                            message: format!("{error:#}"),
                            received,
                        });
                    }
                }
            }
        };
        let outcome = self.set_field(&node, &params.value).await;
        Json(ActionOutput {
            ok: outcome.ok,
            implemented: true,
            action: "set_value".to_string(),
            message: outcome.message,
            received,
        })
    }

    async fn fill_form(
        &self,
        Parameters(params): Parameters<FillFormParams>,
    ) -> Json<ActionOutput> {
        let _input_guard = Arc::clone(&self.input_operation_lock).lock_owned().await;
        let received = Some(serde_json::json!(params.clone()));
        if params.fields.is_empty() {
            return Json(ActionOutput {
                ok: false,
                implemented: true,
                action: "fill_form".to_string(),
                message: "Pass at least one field.".to_string(),
                received,
            });
        }
        let mut lines = Vec::new();
        let mut ok_count = 0;
        for field in &params.fields {
            let selector = field.selector();
            let resolved = match self
                .ensure_resolvable(
                    field.element_index,
                    &selector,
                    ElementResolvePurpose::SetValue,
                )
                .await
            {
                Ok(()) => self
                    .resolve_field_node(field.element_index, None, &selector)
                    .and_then(|node| node.ok_or_else(|| "no element given".to_string())),
                Err(message) => Err(message),
            };
            let (label, outcome) = match resolved {
                Ok(node) => (
                    crate::outline::element_label(&node),
                    self.set_field(&node, &field.value).await,
                ),
                Err(message) => (field.describe(), FieldOutcome::failed(message)),
            };
            ok_count += usize::from(outcome.ok);
            lines.push(form_field_line(&label, &outcome, &field.value));
        }
        Json(ActionOutput {
            ok: ok_count == params.fields.len(),
            implemented: true,
            action: "fill_form".to_string(),
            message: format!(
                "{ok_count}/{} fields set\n{}",
                params.fields.len(),
                lines.join("\n")
            ),
            received,
        })
    }

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
            if let Err(message) = self
                .ensure_resolvable(
                    params.element_index,
                    &ElementSelector::default(),
                    ElementResolvePurpose::Click,
                )
                .await
            {
                return Json(ActionOutput {
                    ok: false,
                    implemented: true,
                    action: "scroll".to_string(),
                    message,
                    received,
                });
            }
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
        let mut target_point =
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
        let direction = match params.direction().as_str() {
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
        if params.x.zip(params.y).is_none() {
            if let Some(node) = self.cached_node_for(
                params.element_index,
                &ElementSelector::default(),
                ElementResolvePurpose::Click,
            ) {
                let mut visible = match target_point {
                    Some(point) => self.point_visible_for(&node, point).await,
                    None => false,
                };
                if !visible && params.until.is_some() {
                    // A list taller than the page: wheel at the centre of the
                    // part of it that is on screen.
                    let shown = self.last_snapshot.lock().ok().and_then(|cached| {
                        crate::page_view::shown_part(&cached.nodes, Some(node.index))
                    });
                    if let Some(center) = shown.as_ref().and_then(bounds_center) {
                        target_point = Some(center);
                        visible = self.point_visible_for(&node, center).await;
                    }
                }
                if !visible {
                    // Wheel input at an off-page point scrolls nothing; ask
                    // the toolkit to bring the element into view instead.
                    let label = crate::outline::element_label(&node);
                    if params.until.is_some() {
                        return Json(ActionOutput {
                            ok: false,
                            implemented: true,
                            action: "scroll".to_string(),
                            message: format!("{label} is outside the visible window; scroll(element_index={}) first, then scroll inside it with until", node.index),
                            received,
                        });
                    }
                    return Json(match crate::atspi_tree::scroll_into_view(&node.object_ref).await {
                        Ok(_) => ActionOutput {
                            ok: true,
                            implemented: true,
                            action: "scroll".to_string(),
                            message: format!("scrolled {label} into view (AT-SPI ScrollTo)"),
                            received,
                        },
                        Err(error) => ActionOutput {
                            ok: false,
                            implemented: true,
                            action: "scroll".to_string(),
                            message: format!("{label} is off the visible page and could not be scrolled into view: {error:#}"),
                            received,
                        },
                    });
                }
            }
        }
        if let Some((x, y)) = target_point {
            if let Some(note) = self.off_screen_note_for_point(x, y).await {
                return Json(ActionOutput {
                    ok: false,
                    implemented: true,
                    action: "scroll".to_string(),
                    message: format!("{note} Nothing was scrolled."),
                    received,
                });
            }
        }
        // XTEST wheel buttons have no XI2 valuators, so GTK 3 takes its button
        // path instead of re-baselining smooth-scroll valuators (#184).
        let repeat = u32::try_from(units.max(1)).unwrap_or(1);
        if let Some(query) = params.until.clone() {
            if query.is_empty() {
                return Json(ActionOutput {
                    ok: false,
                    implemented: true,
                    action: "scroll".to_string(),
                    message: "until needs at least one of role, name, text, or states.".to_string(),
                    received,
                });
            }
            let backend = self.clone();
            let container = self.cached_node_for(
                params.element_index,
                &ElementSelector::default(),
                ElementResolvePurpose::Click,
            );
            let plan = ScrollUntil {
                query,
                point: target_point,
                direction,
                direction_given: params.direction.is_some(),
                // Without `pages`, steps follow the scrolled area's size.
                max_clicks: params.pages.is_some().then_some(repeat),
                container: container.as_ref().map(|node| node.index),
                hint_ref: container.map(|node| node.object_ref),
            };
            let (input_guard, result) = run_cancellation_safe_input(input_guard, async move {
                backend.scroll_until(&plan).await
            })
            .await;
            let _input_guard = input_guard;
            let (ok, message) = match result {
                Ok(message) => (true, message),
                Err(message) => (false, message),
            };
            return Json(ActionOutput {
                ok,
                implemented: true,
                action: "scroll".to_string(),
                message,
                received,
            });
        }
        let xdotool_args = xdotool_pointer_scroll_args(target_point, repeat, direction);
        let command_timeout = xdotool_scroll_timeout(repeat, XDOTOOL_SCROLL_CLICK_DELAY_MS);
        let (input_guard, result) = run_cancellation_safe_input(input_guard, async move {
            run_xdotool(&xdotool_args, command_timeout).await
        })
        .await;
        let _input_guard = input_guard;
        let mut output = action_result("scroll", result.map(|output| vec![output]), received);
        if output.ok {
            output.message = match target_point {
                Some((x, y)) => format!(
                    "scrolled {} {} page(s) at {x},{y} (xdotool)",
                    params.direction(),
                    params.pages.unwrap_or(1.0)
                ),
                None => format!(
                    "scrolled {} {} page(s) under the pointer (xdotool)",
                    params.direction(),
                    params.pages.unwrap_or(1.0)
                ),
            };
        }
        Json(output)
    }

    async fn drag(&self, Parameters(params): Parameters<DragParams>) -> Json<ActionOutput> {
        let received = Some(serde_json::json!(params));
        let failed = |message: String| {
            Json(ActionOutput {
                ok: false,
                implemented: true,
                action: "drag".to_string(),
                message,
                received: received.clone(),
            })
        };
        let input_guard = Arc::clone(&self.input_operation_lock).lock_owned().await;
        let start = match self
            .drag_end(
                params.from_element_index,
                params.start_x,
                params.start_y,
                "start",
            )
            .await
        {
            Ok(end) => end,
            Err(message) => return failed(message),
        };
        let end = match self
            .drag_end(params.to_element_index, params.end_x, params.end_y, "end")
            .await
        {
            Ok(end) => end,
            Err(message) => return failed(message),
        };
        for (x, y) in [start.1, end.1] {
            if let Some(note) = self.off_screen_note_for_point(x, y).await {
                return failed(format!("{note} Nothing was dragged."));
            }
        }
        let (from, to) = (start.1, end.1);
        let (input_guard, result) =
            run_cancellation_safe_input(
                input_guard,
                async move { run_xdotool_drag(from, to).await },
            )
            .await;
        let _input_guard = input_guard;
        let mut output = action_result("drag", result, received.clone());
        if output.ok {
            output.message = format!("dragged {} to {} (xdotool, stepped)", start.0, end.0);
        }
        Json(output)
    }

    /// One end of a drag: an element's centre (it must be on the visible
    /// page) or a screen point, with how the result names it.
    async fn drag_end(
        &self,
        element_index: Option<u32>,
        x: Option<i32>,
        y: Option<i32>,
        which: &str,
    ) -> std::result::Result<(String, (i32, i32)), String> {
        match (element_index, x.zip(y)) {
            (Some(id), _) => self
                .visible_element_center(Some(id), &ElementSelector::default())
                .await
                .map(|(label, point)| (format!("{label} @{},{}", point.0, point.1), point)),
            (None, Some(point)) => Ok((format!("{},{}", point.0, point.1), point)),
            (None, None) => Err(format!(
                "Pass the drag {which} as {}_element_index or as {which}_x and {which}_y.",
                if which == "start" { "from" } else { "to" }
            )),
        }
    }

    /// The centre of the element an action names, by id or selector, from
    /// its current bounds. An element whose centre is not on the visible
    /// page is refused with the call that brings it into view, and one whose
    /// centre is covered by another app's window is refused naming that
    /// window, since input there would land elsewhere.
    async fn visible_element_center(
        &self,
        element_index: Option<u32>,
        selector: &ElementSelector<'_>,
    ) -> std::result::Result<(String, (i32, i32)), String> {
        self.ensure_resolvable(element_index, selector, ElementResolvePurpose::Click)
            .await?;
        let node =
            self.resolve_cached_node(element_index, selector, ElementResolvePurpose::Click)?;
        let label = crate::outline::element_label(&node);
        let outside = || {
            format!(
                "{label} is outside the visible window; scroll it into view with scroll(element_index={}) and retry",
                node.index
            )
        };
        let cached_center = node.bounds.as_ref().and_then(bounds_center);
        if !matches!(cached_center, Some(center) if self.point_visible_for(&node, center).await) {
            return Err(outside());
        }
        // The window may have moved since the snapshot: act on the bounds
        // the element has now.
        // Bounded: a stalled accessibility bus must not hold the input lock.
        let reread = tokio::time::timeout(
            ELEMENT_REREAD_TIMEOUT,
            crate::atspi_tree::read_element(&node.object_ref),
        )
        .await
        .unwrap_or_else(|_| {
            Err(anyhow::anyhow!(
                "no answer within {ELEMENT_REREAD_TIMEOUT:?}"
            ))
        });
        let mut fresh = reread.map_err(|error| {
            format!(
                "{label} could not be reread ({}); call get_app_state and retry",
                first_line(&format!("{error:#}"))
            )
        })?;
        fresh.index = node.index;
        let center = fresh
            .bounds
            .as_ref()
            .and_then(bounds_center)
            .ok_or_else(outside)?;
        if !self.point_visible_for(&fresh, center).await {
            return Err(outside());
        }
        self.check_window_at(&node, &label, center).await?;
        Ok((label, center))
    }

    /// Refuses pointer input at `point` for an element unless the X11 window
    /// there belongs to the element's app (`window_in_the_way`). Skipped off
    /// native X11, where the X server does not see every window.
    async fn check_window_at(
        &self,
        node: &AccessibilityNode,
        label: &str,
        (x, y): (i32, i32),
    ) -> std::result::Result<(), String> {
        if !crate::x11_display::is_native_x11_session() {
            return Ok(());
        }
        let owner_pid = match object_ref_owner_pid(&node.object_ref).await {
            Ok(Some(pid)) => Some(pid),
            _ => self
                .last_snapshot
                .lock()
                .ok()
                .and_then(|snapshot| snapshot.pid),
        };
        let point = (
            i16::try_from(x).map_err(|_| format!("{x},{y} is off the X screen"))?,
            i16::try_from(y).map_err(|_| format!("{x},{y} is off the X screen"))?,
        );
        let under = crate::x11_display::with_x11_display(
            crate::x11_display::X11_QUERY_TIMEOUT,
            move |display| display.window_at(point),
        )
        .await
        .and_then(|result| result)
        .map_err(|error| {
            format!(
                "could not ask the X server which window is under {x},{y}: {}",
                first_line(&format!("{error:#}"))
            )
        })?;
        match window_in_the_way(label, (x, y), owner_pid, under.as_ref()) {
            Some(message) => Err(message),
            None => Ok(()),
        }
    }

    async fn drop_files(
        &self,
        Parameters(params): Parameters<DropFilesParams>,
    ) -> Json<ActionOutput> {
        let received = Some(serde_json::json!(params.clone()));
        let failed = |message: String| {
            Json(ActionOutput {
                ok: false,
                implemented: true,
                action: "drop_files".to_string(),
                message,
                received: received.clone(),
            })
        };
        let paths = match validate_drop_paths(&params.paths) {
            Ok(paths) => paths,
            Err(message) => return failed(message),
        };
        let input_guard = Arc::clone(&self.input_operation_lock).lock_owned().await;
        let selector = params.selector();
        let (label, point) = match params.x.zip(params.y) {
            Some(point) => (format!("{},{}", point.0, point.1), point),
            None if params.element_index.is_some() || !selector.is_empty() => {
                match self
                    .visible_element_center(params.element_index, &selector)
                    .await
                {
                    Ok(target) => target,
                    Err(message) => return failed(message),
                }
            }
            None => {
                return failed(
                    "Pass element_index, role/name/text, or x and y: where to drop the files."
                        .to_string(),
                )
            }
        };
        if let Some(note) = self.off_screen_note_for_point(point.0, point.1).await {
            return failed(format!("{note} Nothing was dropped."));
        }
        let count = paths.len();
        let (input_guard, result) = run_cancellation_safe_input(input_guard, async move {
            tokio::task::spawn_blocking(move || {
                crate::x11_dnd::drop_files(&paths, point, DROP_FINISH_TIMEOUT)
            })
            .await
            .map_err(|error| format!("drop task failed: {error}"))?
            .map_err(|error| format!("{error:#}"))
        })
        .await;
        let _input_guard = input_guard;
        let outcome = match result {
            Ok(outcome) => outcome,
            Err(message) => return failed(format!("Nothing was dropped: {message}")),
        };
        let steps = outcome.log.join("; ");
        eprintln!("drop_files at {},{}: {steps}", point.0, point.1);
        match outcome.failure() {
            None => Json(ActionOutput {
                ok: true,
                implemented: true,
                action: "drop_files".to_string(),
                message: format!("dropped {count} file(s) onto {label} (XDND: {steps})"),
                received,
            }),
            Some(reason) => failed(format!(
                "the drop onto {label} was not taken: {reason} (XDND: {steps}); drop onto the file input or drop zone itself"
            )),
        }
    }

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
        let mut output = action_result("press_key", result.map(|output| vec![output]), received);
        if output.ok {
            output.message = format!("pressed {} (xdotool)", params.key.trim());
        }
        if output.ok && focus.is_some() {
            let notes = self.input_landing_notes(focus.as_ref(), false).await;
            output = with_notes(output, notes);
        }
        Json(output)
    }

    #[tool(
        name = "click",
        description = "Click an element by element_index (or role/name/text), or screen x/y. Element clicks use the element's own action when it has one, so off-screen items in lists work. Reports what changed.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = true
        )
    )]
    async fn click_tool(&self, Parameters(params): Parameters<ClickParams>) -> CallToolResult {
        let observe = params.observe != Some(false);
        let Json(output) = self.click(Parameters(params)).await;
        self.finish_action(output, observe).await
    }

    #[tool(
        name = "perform_action",
        description = "Invoke a named AT-SPI action of an element (default: its primary action). Reports what changed.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = true
        )
    )]
    async fn perform_action_tool(
        &self,
        Parameters(params): Parameters<ActionParams>,
    ) -> CallToolResult {
        let observe = params.observe != Some(false);
        let Json(output) = self.perform_action(Parameters(params)).await;
        self.finish_action(output, observe).await
    }

    #[tool(
        name = "set_value",
        description = "Set a field and read it back: text for text fields (\"\" clears; non-ASCII and multi-line text is pasted in one piece, so \"\\n\" is a line break in text areas and rich-text editors and never presses Enter; single-line inputs refuse it), the option name for combo boxes, selects and radio groups, \"true\"/\"false\" for check boxes and radios. ok only when the field then holds the value. Reports what changed.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = true
        )
    )]
    async fn set_value_tool(
        &self,
        Parameters(params): Parameters<SetValueParams>,
    ) -> CallToolResult {
        let observe = params.observe != Some(false);
        let Json(output) = self.set_value(Parameters(params)).await;
        self.finish_action(output, observe).await
    }

    #[tool(
        name = "fill_form",
        description = "Set several fields in one call, in order, like set_value: text, \"true\"/\"false\" for checkboxes, radios and switches, option text for selects and comboboxes, so several clicks become one call. Each field is {element_index or role/name, value}. Returns each value read back and one change report.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = true
        )
    )]
    async fn fill_form_tool(
        &self,
        Parameters(params): Parameters<FillFormParams>,
    ) -> CallToolResult {
        let observe = params.observe != Some(false);
        let Json(output) = self.fill_form(Parameters(params)).await;
        self.finish_action(output, observe).await
    }

    #[tool(
        name = "scroll",
        description = "Scroll at an element (element_index), a point, or a window's centre. scroll(element_index=n) brings an off-screen element into view. until={role,name,text} keeps scrolling (virtualized/long lists) until a match is rendered, brings it into view and returns its line; with no direction it searches down, then up. Reports what changed.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = true
        )
    )]
    async fn scroll_tool(&self, Parameters(params): Parameters<ScrollParams>) -> CallToolResult {
        let observe = params.observe != Some(false);
        let until = params.until.is_some();
        let Json(output) = self.scroll(Parameters(params)).await;
        // A found `until` match already prints its lines and cached the
        // snapshot they come from; a change report would print the page.
        let observe = observe && !(until && output.ok);
        self.finish_action(output, observe).await
    }

    #[tool(
        name = "drag",
        description = "Drag with the left button from an element (from_element_index) or start_x,start_y to an element (to_element_index) or end_x,end_y, in small steps so HTML5 drag and drop starts. Reports what changed.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = true
        )
    )]
    async fn drag_tool(&self, Parameters(params): Parameters<DragParams>) -> CallToolResult {
        let observe = params.observe != Some(false);
        let Json(output) = self.drag(Parameters(params)).await;
        self.finish_action(output, observe).await
    }

    #[tool(
        name = "drop_files",
        description = "Upload files: drop them (absolute paths) onto the page's file input or drop zone, given by element_index, role/name/text or x,y; there is no file dialog on this display. Reports what changed.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = true
        )
    )]
    async fn drop_files_tool(
        &self,
        Parameters(params): Parameters<DropFilesParams>,
    ) -> CallToolResult {
        let observe = params.observe != Some(false);
        let Json(output) = self.drop_files(Parameters(params)).await;
        self.finish_action(output, observe).await
    }

    #[tool(
        name = "press_key",
        description = "Press a key or chord, e.g. Enter, Escape, Tab, ArrowDown, Ctrl+L, Ctrl+Shift+T (letters, digits, F1-F12, Home/End/PageUp/PageDown, Backspace, Delete, Space). Optional window target. Reports what changed.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = true
        )
    )]
    async fn press_key_tool(
        &self,
        Parameters(params): Parameters<PressKeyParams>,
    ) -> CallToolResult {
        let observe = params.observe != Some(false);
        let Json(output) = self.press_key(Parameters(params)).await;
        self.finish_action(output, observe).await
    }

    #[tool(
        name = "type_text",
        description = "Type text into the focused element, or first into element_index/role/name or a window target. Non-ASCII text is pasted (Ctrl+Shift+V in terminals); newline presses Enter (a new line in text areas and editors; inside the text of a single-line input it is refused). To replace a field's content use set_value. Reports what changed.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = true
        )
    )]
    async fn type_text_tool(
        &self,
        Parameters(params): Parameters<TypeTextParams>,
    ) -> CallToolResult {
        let observe = params.observe != Some(false);
        let Json(output) = self.type_text(Parameters(params)).await;
        self.finish_action(output, observe).await
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

    async fn type_text(
        &self,
        Parameters(params): Parameters<TypeTextParams>,
    ) -> Json<ActionOutput> {
        self.dispatch_type_text(params).await
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
        let failed = |message: String| {
            Json(ActionOutput {
                ok: false,
                implemented: true,
                action: "type_text".to_string(),
                message,
                received: received.clone(),
            })
        };
        let window_target = params.window_target();
        let element = if params.element_index.is_some() || !params.selector().is_empty() {
            if let Err(message) = self
                .ensure_resolvable(
                    params.element_index,
                    &params.selector(),
                    ElementResolvePurpose::SetValue,
                )
                .await
            {
                return failed(message);
            }
            match self.resolve_cached_node(
                params.element_index,
                &params.selector(),
                ElementResolvePurpose::SetValue,
            ) {
                Ok(node) => Some(node),
                Err(message) => return failed(message),
            }
        } else {
            None
        };
        // A window target and an element in another app cannot both be
        // honoured: refuse before focus moves anywhere.
        if let (Some(node), true) = (element.as_ref(), window_target.has_target()) {
            let window_pid = match window_target.pid {
                Some(pid) => Some(pid),
                None => list_windows().await.ok().and_then(|windows| {
                    resolve_window_target(&windows, &window_target)
                        .ok()
                        .and_then(|window| window.pid)
                }),
            };
            if let Err(message) = self.check_node_target(node, window_pid).await {
                return failed(conflicting_targets_message(&message));
            }
        }
        let focus = match self.focus_target_for_input(&window_target).await {
            Ok(focus) => focus,
            Err(message) => return failed(message),
        };
        let target = if let Some(node) = element {
            if let Err(message) = self
                .check_node_target(&node, focus_target_pid(focus.as_ref()))
                .await
            {
                return failed(conflicting_targets_message(&message));
            }
            let label = crate::outline::element_label(&node);
            if let Some(refusal) =
                crate::fields::line_break_refusal(&node, &label, &params.text, true)
            {
                return failed(refusal);
            }
            if let Err(message) = self.focus_element(&node, &label).await {
                return failed(message);
            }
            if let Some(focus) = focus.as_ref() {
                let now = focused_window().await.ok().flatten();
                if let Some(drift) = focus_drift(
                    &focus.requested_window,
                    window_target.requires_exact_focus(),
                    now.as_ref(),
                    &label,
                ) {
                    return failed(drift);
                }
            }
            Some(node)
        } else {
            if let Some(refusal) = self
                .focused_line_break_refusal(focus.as_ref(), &params.text)
                .await
            {
                return failed(refusal);
            }
            None
        };
        let window = match focus.as_ref() {
            Some(focus) => focus.focused_window.clone(),
            None => focused_window().await.ok().flatten(),
        };
        let paste_chord = paste_chord_for(window.as_ref());
        // The target's text before typing, to tell whether the text arrived.
        let before = match target.as_ref() {
            Some(node) => read_field(&node.object_ref)
                .await
                .and_then(|(_, text)| text),
            None => None,
        };
        let text = params.text.clone();
        let (input_guard, result) = run_cancellation_safe_input(input_guard, async move {
            enter_text(&text, paste_chord).await.map(|()| Vec::new())
        })
        .await;
        let _input_guard = input_guard;
        let mut output = action_result("type_text", result, received);
        if !output.ok {
            return Json(output);
        }
        output.message = if crate::fields::typeable_ascii(&params.text) {
            "Sent through xdotool (X11 XTEST).".to_string()
        } else {
            format!("Paste sent through the X11 clipboard ({paste_chord}).")
        };
        if let Some(node) = target {
            let label = crate::outline::element_label(&node);
            sleep(Duration::from_millis(100)).await;
            let read_back = read_field(&node.object_ref)
                .await
                .and_then(|(_, text)| text);
            let shown = crate::fields::shown_text(read_back.as_deref());
            let holds = crate::outline::truncate(&shown, 200);
            match typed_delivery(before.as_deref(), read_back.as_deref(), &params.text) {
                Delivery::Arrived => {
                    output
                        .message
                        .push_str(&format!(" {label} now holds \"{holds}\"."));
                    if crate::fields::compare_read_back(read_back.as_deref(), &params.text)
                        == crate::fields::ReadBack::Formatted
                    {
                        output.message.push_str(&format!(
                            " That matches the typed text (the field formatted it as \"{holds}\")."
                        ));
                    }
                }
                Delivery::Unchanged => {
                    output.ok = false;
                    output.message = format!(
                        "The text did not arrive: {label} still holds \"{holds}\" after the input was sent. Check what has focus (a dialog?) and retry, or use set_value."
                    );
                }
                Delivery::Unverified if read_back.is_none() => {
                    output.message.push_str(&format!(
                        " Dispatched; delivery not verified: {label} exposes no text to read back."
                    ));
                }
                Delivery::Unverified => {
                    output.message.push_str(&format!(
                        " Dispatched; delivery not verified: Enter may have submitted {label}, which now holds \"{holds}\"."
                    ));
                }
            }
        } else {
            output
                .message
                .push_str(" Dispatched; delivery not verified (no element was targeted).");
            if focus.is_some() {
                let notes = self.input_landing_notes(focus.as_ref(), true).await;
                output = with_notes(output, notes);
            }
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
    instructions = "Start each turn with get_app_state (scope it, e.g. app_name_or_bundle_identifier=chrome). Each line starts with an element id [n] that stays valid for the whole session. Act by id with click, set_value, fill_form, type_text, press_key or scroll. Every action reports what changed (+ new, - gone, ~ changed, focus), so look again only when you need the whole page. fill_form and set_value type, paste non-ASCII text, choose options in combo boxes and selects, tick check boxes and radios, and return the value now in the field. Use screenshot or ocr only when the tree lacks the content. type_text and press_key refuse to send input when focus cannot be verified. Tools with readOnlyHint=false change app state. Hosts may ask the user to approve tools that submit, send, delete or buy. If a tool fails, doctor names the fix."
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
    #[schemars(skip)]
    #[serde(default)]
    app_id: Option<String>,
    #[schemars(skip)]
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
    #[schemars(skip)]
    #[serde(default)]
    lang: Option<String>,
    /// Drop words below this confidence, 0 to 100 (default 60).
    #[schemars(skip)]
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
    /// App name, e.g. chrome.
    #[serde(default)]
    app_name_or_bundle_identifier: Option<String>,
    #[serde(default)]
    window_id: Option<u64>,
    #[serde(default)]
    pid: Option<u32>,
    #[schemars(skip)]
    #[serde(default)]
    app_id: Option<String>,
    #[schemars(skip)]
    #[serde(default)]
    wm_class: Option<String>,
    /// Window title substring.
    #[serde(default)]
    title: Option<String>,
    /// Nodes to read (default 2000).
    #[schemars(skip)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum WaitCondition {
    /// An element matching the query exists.
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
    /// appears (default with a query), disappears, or changed (default
    /// without one: wait for any change when you do not know what comes).
    #[serde(default)]
    condition: Option<WaitCondition>,
    /// Give up after this many milliseconds (default 5000, max 60000).
    #[serde(default)]
    timeout_ms: Option<u64>,
    /// Delay between polls in milliseconds (default 400).
    #[schemars(skip)]
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
    /// App name, e.g. chrome. Omitting it reads the whole desktop.
    #[serde(default)]
    app_name_or_bundle_identifier: Option<String>,
    /// Window id (header of an earlier result, or list_windows).
    #[serde(default)]
    window_id: Option<u64>,
    /// Process id of the app.
    #[serde(default)]
    pid: Option<u32>,
    /// Terminal tty device (for example /dev/pts/3). Resolves a window target
    /// and scopes the tree when possible.
    #[schemars(skip)]
    #[serde(default)]
    tty: Option<String>,
    /// Terminal emulator pid. Resolves a window target and scopes the tree when
    /// possible.
    #[schemars(skip)]
    #[serde(default)]
    terminal_pid: Option<u32>,
    /// Terminal command substring. Resolves a window target and scopes the tree
    /// when possible.
    #[schemars(skip)]
    #[serde(default)]
    terminal_command: Option<String>,
    /// Terminal working directory. Resolves a window target and scopes the tree
    /// when possible.
    #[schemars(skip)]
    #[serde(default)]
    terminal_cwd: Option<String>,
    /// Application id. Also scopes the accessibility tree when it matches an
    /// AT-SPI root.
    #[schemars(skip)]
    #[serde(default)]
    app_id: Option<String>,
    /// Window class, e.g. Alacritty.
    #[serde(default)]
    wm_class: Option<String>,
    /// Window title substring.
    #[serde(default)]
    title: Option<String>,
    /// Nodes to read (default 1000, max 2000); raise it when told the tree was cut.
    #[serde(default)]
    max_nodes: Option<usize>,
    /// Maximum AT-SPI traversal depth (default 32, hard max 64).
    #[schemars(skip)]
    #[serde(default)]
    max_depth: Option<u32>,
    /// `json` returns full node objects instead of the outline.
    #[serde(default)]
    tree_format: Option<TreeFormat>,
    /// true always attaches a screenshot, false never (default: only when
    /// the tree shows little).
    #[serde(default)]
    include_screenshot: Option<bool>,
    /// Also show browser toolbars and bubbles.
    #[serde(default)]
    include_browser_ui: Option<bool>,
    /// Maximum returned screenshot width in pixels (default 1920, hard-capped).
    #[schemars(skip)]
    #[serde(default)]
    max_width: Option<u32>,
    /// Maximum returned screenshot height in pixels (default 1920, hard-capped).
    #[schemars(skip)]
    #[serde(default)]
    max_height: Option<u32>,
    /// Maximum returned screenshot image bytes before base64 (default 2 MiB, hard-capped).
    #[schemars(skip)]
    #[serde(default)]
    max_bytes: Option<usize>,
    /// Additional downscale factor from 0.0 to 1.0, applied before max dimensions.
    #[schemars(skip)]
    #[serde(default)]
    scale: Option<f32>,
    /// Output image format (default png). Use jpeg with quality to trade exact pixels for smaller payloads.
    #[schemars(skip)]
    #[serde(default)]
    format: Option<ScreenshotOutputFormat>,
    /// JPEG quality from 1 to 95 (default 80). Ignored for png.
    #[schemars(skip)]
    #[serde(default)]
    quality: Option<u8>,
    /// Include the full diagnostics report (large). Default false: only the
    /// compact readiness block is returned.
    #[schemars(skip)]
    #[serde(default)]
    verbose: Option<bool>,
}

impl GetAppStateParams {
    fn view_options(&self) -> crate::page_view::ViewOptions {
        crate::page_view::ViewOptions {
            include_browser_ui: self.include_browser_ui.unwrap_or(false),
        }
    }

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
    #[schemars(skip)]
    #[serde(default)]
    app_id: Option<String>,
    #[schemars(skip)]
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
    #[schemars(skip)]
    #[serde(default)]
    max_height: Option<u32>,
    /// Maximum returned screenshot image bytes before base64 (default 2 MiB, hard-capped).
    #[schemars(skip)]
    #[serde(default)]
    max_bytes: Option<usize>,
    /// Additional downscale factor from 0.0 to 1.0, applied before max dimensions.
    #[schemars(skip)]
    #[serde(default)]
    scale: Option<f32>,
    /// Output image format (default png). Use jpeg with quality to trade exact pixels for smaller payloads.
    #[serde(default)]
    format: Option<ScreenshotOutputFormat>,
    /// JPEG quality from 1 to 95 (default 80). Ignored for png.
    #[schemars(skip)]
    #[serde(default)]
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

/// Fewest named or actionable elements for which `get_app_state` skips the
/// screenshot by default: the tree already shows what the agent needs.
const MIN_CONTENT_ELEMENTS: usize = 5;

/// The text `get_app_state` returns: one header line (window, id, URL,
/// element count), the outline, then notes only when something needs fixing.
fn app_state_text(
    output: &GetAppStateOutput,
    view: &crate::page_view::PageView,
    window_id: Option<u64>,
    screenshot: Option<&ScreenshotCapture>,
) -> String {
    let title = view.title.clone().or_else(|| {
        output
            .window_context
            .as_ref()
            .and_then(|window| window.title.clone())
    });
    let mut text = match title {
        Some(title) => format!("window \"{title}\""),
        None => "window (untitled)".to_string(),
    };
    if let Some(id) = window_id {
        text.push_str(&format!(" window_id={id}"));
    }
    if let Some(url) = &view.url {
        text.push_str(&format!(" url={url}"));
    }
    text.push_str(&format!(" elements={}\n", view.entries.len()));
    text.push_str(&view.outline());
    if let Some(capture) = screenshot {
        text.push_str(&format!(
            "screenshot attached: {}x{} image of {}x{} screen pixels (scale {}; click x/y = image pixel / scale)\n",
            capture.width,
            capture.height,
            capture.coordinate_width,
            capture.coordinate_height,
            capture.scale
        ));
    }
    for note in &output.notes {
        text.push_str(note);
        text.push('\n');
    }
    text
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
    /// Problems and warnings for the text result, each with its fix.
    #[serde(skip)]
    notes: Vec<String>,
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
    /// false skips the report of what changed (default true).
    #[serde(default)]
    observe: Option<bool>,
    /// Element id `[n]` from any earlier result.
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
    #[schemars(skip)]
    #[serde(default)]
    app_id: Option<String>,
    #[schemars(skip)]
    #[serde(default)]
    wm_class: Option<String>,
    #[serde(default)]
    window_title: Option<String>,
    /// x/y are relative to the targeted window's screenshot.
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
    /// false skips the report of what changed (default true).
    #[serde(default)]
    observe: Option<bool>,
    /// Element id `[n]` from any earlier result.
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
    /// false skips the report of what changed (default true).
    #[serde(default)]
    observe: Option<bool>,
    /// Element id `[n]` from any earlier result.
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

/// One `fill_form` field: an element and the value to give it.
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
struct FormField {
    /// Element id `[n]` from an earlier result.
    /// Element id `[n]` from any earlier result.
    #[serde(default)]
    element_index: Option<u32>,
    /// Role substring, when not using element_index.
    #[serde(default)]
    role: Option<String>,
    /// Name substring, when not using element_index.
    #[serde(default)]
    name: Option<String>,
    /// Text, option name, or "true"/"false" for check boxes and radios.
    value: String,
}

impl FormField {
    fn selector(&self) -> ElementSelector<'_> {
        ElementSelector {
            role: self.role.as_deref(),
            name: self.name.as_deref(),
            text: None,
            states: &[],
        }
    }

    fn describe(&self) -> String {
        match self.element_index {
            Some(index) => format!("[{index}]"),
            None => describe_selector(&self.selector()),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
struct FillFormParams {
    /// false skips the report of what changed (default true).
    #[serde(default)]
    observe: Option<bool>,
    fields: Vec<FormField>,
}

/// One `fill_form` result line: `label: ok value="…"` (noting a value its
/// input mask reformatted) or `label: FAILED …`.
fn form_field_line(label: &str, outcome: &FieldOutcome, requested: &str) -> String {
    let value = outcome
        .value
        .as_deref()
        .map(|value| format!(" value=\"{value}\""))
        .unwrap_or_default();
    let formatted = outcome.value.as_deref().filter(|shown| {
        crate::fields::compare_read_back(Some(shown), requested)
            == crate::fields::ReadBack::Formatted
    });
    if let (true, Some(shown)) = (outcome.ok, formatted) {
        format!("{label}: ok value=\"{requested}\" (the field formatted it as \"{shown}\")")
    } else if outcome.ok {
        format!("{label}: ok{value}")
    } else {
        format!("{label}: FAILED{value} error: {}", outcome.message)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
struct ScrollParams {
    /// false skips the report of what changed (default true).
    #[serde(default)]
    observe: Option<bool>,
    /// Element id `[n]` from any earlier result.
    #[serde(default)]
    element_index: Option<u32>,
    #[serde(default)]
    x: Option<i32>,
    #[serde(default)]
    y: Option<i32>,
    /// up, down, left or right (default down; with until and no direction,
    /// down to the end, then up past the start).
    #[serde(default)]
    direction: Option<String>,
    #[serde(default)]
    #[schemars(range(min = -100, max = 100))]
    pages: Option<f64>,
    /// Keep scrolling (at most a page, about 60% of the scrolled area, per
    /// step) until an element matching this is rendered, then bring it into
    /// view (or the end, or 80 scrolls each way); returns its line.
    #[serde(default)]
    until: Option<ElementQuery>,
    // Optional window target (parity with click): the window is raised/focused
    // before scrolling so the wheel events land on the intended app.
    #[serde(default)]
    window_id: Option<u64>,
    #[serde(default)]
    pid: Option<u32>,
    #[schemars(skip)]
    #[serde(default)]
    app_id: Option<String>,
    #[schemars(skip)]
    #[serde(default)]
    wm_class: Option<String>,
    #[schemars(skip)]
    #[serde(default)]
    window_title: Option<String>,
    /// x/y are relative to the targeted window's screenshot.
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
    /// The requested direction, lower-cased; down when none was given.
    fn direction(&self) -> String {
        self.direction
            .as_deref()
            .unwrap_or("down")
            .to_ascii_lowercase()
    }

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

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
struct DragParams {
    /// false skips the report of what changed (default true).
    #[serde(default)]
    observe: Option<bool>,
    /// Element id `[n]` to drag from (its centre), instead of start_x/start_y.
    #[serde(default)]
    from_element_index: Option<u32>,
    /// Element id `[n]` to drop on (its centre), instead of end_x/end_y.
    #[serde(default)]
    to_element_index: Option<u32>,
    #[serde(default)]
    start_x: Option<i32>,
    #[serde(default)]
    start_y: Option<i32>,
    #[serde(default)]
    end_x: Option<i32>,
    #[serde(default)]
    end_y: Option<i32>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
struct DropFilesParams {
    /// false skips the report of what changed (default true).
    #[serde(default)]
    observe: Option<bool>,
    /// Absolute paths of the files to drop.
    paths: Vec<String>,
    /// Element id `[n]` of the file input or drop zone.
    #[serde(default)]
    element_index: Option<u32>,
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    x: Option<i32>,
    #[serde(default)]
    y: Option<i32>,
}

impl DropFilesParams {
    fn selector(&self) -> ElementSelector<'_> {
        ElementSelector {
            role: self.role.as_deref(),
            name: self.name.as_deref(),
            text: self.text.as_deref(),
            states: &[],
        }
    }
}

/// How long the drop target gets to read the files and finish the drop.
const DROP_FINISH_TIMEOUT: Duration = Duration::from_secs(3);

/// The paths `drop_files` was given, each an absolute path of an existing
/// regular file; the first that is not is the error.
fn validate_drop_paths(paths: &[String]) -> std::result::Result<Vec<PathBuf>, String> {
    if paths.is_empty() {
        return Err("Pass at least one absolute file path in paths.".to_string());
    }
    paths
        .iter()
        .map(|raw| {
            let path = Path::new(raw);
            if !path.is_absolute() {
                return Err(format!("{raw:?} is not an absolute path"));
            }
            match std::fs::metadata(path) {
                Ok(metadata) if metadata.is_file() => Ok(path.to_path_buf()),
                Ok(_) => Err(format!("{raw:?} is not a regular file")),
                Err(error) => Err(format!("{raw:?}: {error}")),
            }
        })
        .collect()
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
struct PressKeyParams {
    /// false skips the report of what changed (default true).
    #[serde(default)]
    observe: Option<bool>,
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

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
struct TypeTextParams {
    /// false skips the report of what changed (default true).
    #[serde(default)]
    observe: Option<bool>,
    text: String,
    /// Element id `[n]` to focus before typing.
    /// Element id `[n]` from any earlier result.
    #[serde(default)]
    element_index: Option<u32>,
    /// Role substring of the element to focus before typing.
    #[serde(default)]
    role: Option<String>,
    /// Name substring of the element to focus before typing.
    #[serde(default)]
    name: Option<String>,
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
    fn selector(&self) -> ElementSelector<'_> {
        ElementSelector {
            role: self.role.as_deref(),
            name: self.name.as_deref(),
            text: None,
            states: &[],
        }
    }

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

    /// Like `diagnostics`, but reuses the last report while it found the
    /// session ready. A report with blockers is recomputed every time, so a
    /// fix shows up at once, and a failed observation drops the cache
    /// (`forget_diagnostics`).
    async fn cached_diagnostics(&self) -> DoctorReport {
        let cached = self.recent_diagnostics.lock().ok().and_then(|guard| {
            guard
                .as_ref()
                .filter(|(_, report)| report.readiness.blockers.is_empty())
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

    /// Makes the next observation run every diagnostic probe again.
    fn forget_diagnostics(&self) {
        if let Ok(mut guard) = self.recent_diagnostics.lock() {
            *guard = None;
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
            Ok(Ok(probe)) => {
                let mut feedback =
                    focus_probe_feedback(&probe, expects_editable, focus.exact_window_focused);
                if let FocusProbe::Found(element) = &probe {
                    let text = crate::atspi_tree::read_element(&element.object_ref)
                        .await
                        .ok()
                        .and_then(|fresh| fresh.text)
                        .map(|text| crate::outline::clean(text.content.as_deref()));
                    if let Some(text) = text {
                        feedback.push_str(&format!(
                            " It now holds \"{}\".",
                            crate::outline::truncate(&text.unwrap_or_default(), 200)
                        ));
                    }
                }
                Some(feedback)
            }
            Ok(Err(error)) => Some(format!(
                "Focused-element feedback unavailable ({}).",
                first_line(&format!("{error:#}"))
            )),
            Err(_) => {
                Some("Focused-element feedback unavailable (AT-SPI probe timed out).".to_string())
            }
        }
    }

    /// `fields::line_break_refusal` for untargeted typing: when the text has
    /// a line break before its end, the focused element is read and the text
    /// refused if it is a single-line field. `None` when the focus cannot be
    /// read: typing goes ahead as before.
    async fn focused_line_break_refusal(
        &self,
        focus: Option<&WindowFocusResult>,
        text: &str,
    ) -> Option<String> {
        if !text.trim_end_matches(['\n', '\r']).contains(['\n', '\r']) {
            return None;
        }
        let pid = focus.and_then(|focus| {
            focus
                .focused_window
                .as_ref()
                .and_then(|window| window.pid)
                .or(focus.requested_window.pid)
        });
        let probe = timeout(Duration::from_millis(1500), probe_focused_element(pid))
            .await
            .ok()?
            .ok()?;
        let FocusProbe::Found(element) = probe else {
            return None;
        };
        let node = crate::atspi_tree::read_element(&element.object_ref)
            .await
            .ok()?;
        let label = crate::outline::element_label(&node);
        crate::fields::line_break_refusal(&node, &label, text, true)
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
        self.commit_observation(nodes, target_pid, None).await;
    }

    async fn commit_observation(
        &self,
        nodes: &[AccessibilityNode],
        target_pid: Option<u32>,
        scope: Option<SnapshotScope>,
    ) {
        let _input_guard = Arc::clone(&self.input_operation_lock).lock_owned().await;
        self.cache_observation(nodes, target_pid, scope);
    }

    #[cfg(test)]
    fn cache_snapshot(&self, nodes: &[AccessibilityNode], target_pid: Option<u32>) {
        self.cache_observation(nodes, target_pid, None);
    }

    fn cache_observation(
        &self,
        nodes: &[AccessibilityNode],
        target_pid: Option<u32>,
        scope: Option<SnapshotScope>,
    ) {
        if let Ok(mut cached) = self.last_snapshot.lock() {
            *cached = CachedAccessibilitySnapshot {
                nodes: nodes.to_vec(),
                pid: target_pid,
                scope,
            };
        }
    }

    /// Rewrites a compacted snapshot's positional indices to stable ids.
    fn assign_ids(&self, nodes: &mut Vec<AccessibilityNode>) {
        if let Ok(mut ids) = self.element_ids.lock() {
            ids.assign(nodes);
        }
    }

    /// Reads the scope of the cached observation again (or, with none, the
    /// app owning `hint_ref`, else the focused window's app), with stable ids.
    async fn rescan(&self, hint_ref: Option<&str>) -> Result<Rescan, String> {
        let scope = self
            .last_snapshot
            .lock()
            .ok()
            .and_then(|cached| cached.scope.clone());
        let (app_filter, pid, max_nodes, max_depth, options) = match scope {
            Some(scope) => (
                scope.app_filter,
                scope.target_pid,
                scope.max_nodes,
                scope.max_depth,
                scope.options,
            ),
            None => {
                let owner = match hint_ref {
                    Some(object_ref) => object_ref_owner_pid(object_ref).await.ok().flatten(),
                    None => None,
                };
                let pid = match owner {
                    Some(pid) => Some(pid),
                    None => focused_window()
                        .await
                        .ok()
                        .flatten()
                        .and_then(|window| window.pid),
                };
                let (max_nodes, max_depth) = crate::atspi_tree::snapshot_limits(Some(2_000), None);
                (None, pid, max_nodes, max_depth, Default::default())
            }
        };
        let snapshot =
            snapshot_accessibility_tree(app_filter.as_deref(), pid, max_nodes, max_depth)
                .await
                .map_err(|error| format!("{error:#}"))?;
        let read = snapshot.nodes.len();
        let mut nodes = compact_accessibility_tree(snapshot.nodes);
        self.assign_ids(&mut nodes);
        Ok(Rescan {
            nodes,
            root_pid: snapshot.root_pid,
            scope: SnapshotScope {
                app_filter,
                target_pid: pid,
                max_nodes,
                max_depth,
                options,
            },
            read,
        })
    }

    /// Reads the observed page again after an action. With a browser scope
    /// (browser UI not shown) and a visible web document in `base`, the
    /// latest read of that scope, only the document's subtree and the
    /// window frame and address bar are read (`read_document`); otherwise,
    /// or when that read does not look like the same page, the whole scope
    /// (`rescan`).
    async fn read_page(
        &self,
        base: &[AccessibilityNode],
        hint_ref: Option<&str>,
    ) -> Result<Rescan, String> {
        let cached = self
            .last_snapshot
            .lock()
            .ok()
            .and_then(|cached| Some((cached.scope.clone()?, cached.pid)));
        if let Some((scope, pid)) = cached.filter(|(scope, _)| !scope.options.include_browser_ui) {
            if let Some(rescan) = self.read_document(base, scope, pid).await {
                return Ok(rescan);
            }
        }
        self.rescan(hint_ref).await
    }

    /// The document-only read of `read_page`: the visible document of
    /// `base` with its subtree, and its frame and address bar on their own,
    /// all at once, spliced into `base` (`page_view::splice_document`).
    /// `None` when `base` has no visible document, a read fails or misses
    /// `DOCUMENT_READ_LIMIT`, or the document is no longer the showing page
    /// of its window (another role or id, not showing, or a window title
    /// without the document's name): navigation and tab switches need the
    /// whole scope.
    async fn read_document(
        &self,
        base: &[AccessibilityNode],
        scope: SnapshotScope,
        root_pid: Option<u32>,
    ) -> Option<Rescan> {
        let parts = crate::page_view::document_parts(base)?;
        let element = |id: u32| base.iter().find(|node| node.index == id);
        let document = element(parts.document)?;
        let single: Vec<(u32, String)> = [parts.frame, parts.address_bar]
            .into_iter()
            .flatten()
            .filter_map(|id| Some((id, element(id)?.object_ref.clone())))
            .collect();
        let subtree = crate::atspi_tree::snapshot_subtree(
            &document.object_ref,
            document.depth,
            scope.max_nodes,
            scope.max_depth,
        );
        let singles =
            futures_util::future::join_all(single.iter().map(|(id, object_ref)| async move {
                let mut fresh = crate::atspi_tree::read_element(object_ref).await.ok()?;
                fresh.index = *id;
                Some(fresh)
            }));
        let (subtree, singles) = tokio::time::timeout(DOCUMENT_READ_LIMIT, async {
            tokio::join!(subtree, singles)
        })
        .await
        .ok()?;
        let raw = subtree.ok()?;
        let refreshed: Vec<AccessibilityNode> = singles.into_iter().collect::<Option<_>>()?;
        let read = raw.len() + refreshed.len();
        let mut fresh = compact_accessibility_tree(raw);
        self.assign_ids(&mut fresh);
        let root = fresh.first()?;
        let same_page = root.index == parts.document
            && root.role.eq_ignore_ascii_case("document web")
            && node_is_showing(root);
        let title = parts.frame.and_then(|frame| {
            refreshed
                .iter()
                .find(|node| node.index == frame)
                .and_then(|node| crate::outline::clean(node.name.as_deref()))
        });
        let page_name = crate::outline::clean(root.name.as_deref());
        let titled = match (title, page_name) {
            (Some(title), Some(name)) => title.contains(&name),
            _ => true,
        };
        if !same_page || !titled {
            return None;
        }
        let nodes = crate::page_view::splice_document(base, parts.document, fresh, &refreshed)?;
        Some(Rescan {
            nodes,
            root_pid,
            scope,
            read,
        })
    }

    /// Turns an action's outcome into its text result: `ok: …` or
    /// `error: …`, then (when `observe`) what the action changed, so the
    /// agent need not look again. Failures report changes only when input
    /// may already have reached the app, or a dialog may be in the way: a
    /// failed element action names a blocking dialog (`page_view::blocking_dialog`),
    /// and so does a click that changed nothing while a modal one is open.
    async fn finish_action(&self, output: ActionOutput, observe: bool) -> CallToolResult {
        let dialog = if !output.ok && DIALOG_CHECKED_ACTIONS.contains(&output.action.as_str()) {
            self.blocking_dialog_note().await
        } else {
            None
        };
        let input_reached_app = output.ok
            || matches!(
                output.action.as_str(),
                "set_value" | "fill_form" | "type_text"
            )
            || output.message.contains("no longer exists")
            || dialog.is_some();
        let mut text = if output.ok {
            format!("ok: {}", output.message)
        } else {
            format!("error: {}", output.message)
        };
        if let Some(dialog) = &dialog {
            text.push('\n');
            text.push_str(dialog);
        }
        if observe && input_reached_app {
            let report = self.observe_changes().await;
            let landed_nowhere = output.ok
                && matches!(output.action.as_str(), "click" | "perform_action")
                && report.starts_with("changes: none");
            text.push('\n');
            text.push_str(&report);
            if landed_nowhere {
                let nodes = self
                    .last_snapshot
                    .lock()
                    .map(|cached| cached.nodes.clone())
                    .unwrap_or_default();
                // The same snapshot twice: only a modal dialog counts.
                if let Some(dialog) = crate::page_view::blocking_dialog(&nodes, &nodes) {
                    text.push_str(&format!("note: nothing changed and {dialog}\n"));
                }
            }
        }
        let mut result = CallToolResult::success(vec![ContentBlock::text(text)]);
        if !output.ok {
            result.is_error = Some(true);
        }
        result
    }

    /// `page_view::blocking_dialog` for a failed action: the observed scope
    /// read again, against the snapshot the agent last saw. Nothing is
    /// cached; the change report that follows commits the new read.
    async fn blocking_dialog_note(&self) -> Option<String> {
        let before = self
            .last_snapshot
            .lock()
            .map(|cached| cached.nodes.clone())
            .unwrap_or_default();
        let now = self.rescan(None).await.ok()?;
        crate::page_view::blocking_dialog(&before, &now.nodes)
    }

    /// Waits for the app to settle after an action, then reports what changed
    /// against the snapshot the agent last saw and makes the settled snapshot
    /// the cached one. Settled means two consecutive reads of the observed
    /// scope agree and nothing looks busy (`page_view::busy_reason`), or the
    /// limit passed: `SETTLE_LIMIT`, or `BUSY_SETTLE_LIMIT` once the page
    /// looked busy, which the report then says. An action that changed
    /// nothing therefore takes two reads. Each read is `read_page`: only the
    /// web document in a browser. Logs the reads and time to stderr.
    async fn observe_changes(&self) -> String {
        let acted = std::time::Instant::now();
        let before = self
            .last_snapshot
            .lock()
            .map(|cached| cached.nodes.clone())
            .unwrap_or_default();
        sleep(SETTLE_FIRST_WAIT).await;
        let mut previous: Option<Vec<crate::outline::ElementContent>> = None;
        let mut settled: Option<Rescan> = None;
        let mut last_error = None;
        let mut busy_seen: Option<String> = None;
        let mut still_busy = false;
        let mut quiet_since = std::time::Instant::now();
        let (mut polls, mut nodes_read) = (0, 0);
        loop {
            polls += 1;
            let base = settled.as_ref().map_or(&before[..], |last| &last.nodes[..]);
            match self.read_page(base, None).await {
                Ok(rescan) => {
                    nodes_read += rescan.read;
                    let contents = crate::outline::element_contents(&rescan.nodes);
                    let stable = previous.as_ref() == Some(&contents);
                    if !stable {
                        quiet_since = std::time::Instant::now();
                    }
                    let busy = crate::page_view::busy_reason(&before, &rescan.nodes);
                    if let Some(busy) = &busy {
                        busy_seen.get_or_insert_with(|| busy.reason.clone());
                    }
                    still_busy = still_busy_after(busy.as_ref(), quiet_since.elapsed());
                    previous = Some(contents);
                    settled = Some(rescan);
                    if stable && !still_busy {
                        break;
                    }
                }
                Err(error) => last_error = Some(error),
            }
            let limit = if busy_seen.is_some() {
                BUSY_SETTLE_LIMIT
            } else {
                SETTLE_FIRST_WAIT + SETTLE_LIMIT
            };
            if acted.elapsed() >= limit {
                break;
            }
            sleep(SETTLE_POLL).await;
        }
        eprintln!(
            "settle: {polls} polls, {} ms, {nodes_read} nodes read",
            acted.elapsed().as_millis()
        );
        let Some(settled) = settled else {
            return format!(
                "changes: unknown (the tree could not be read: {})\n",
                last_error.unwrap_or_default()
            );
        };
        let options = settled.scope.options;
        let mut report = String::new();
        if let Some(reason) = busy_seen {
            report.push_str(&format!(
                "waited {} ms for the page to finish ({reason}){}\n",
                acted.elapsed().as_millis(),
                if still_busy {
                    "; it still looks busy"
                } else {
                    ""
                }
            ));
        }
        report.push_str(&crate::page_view::change_report(
            &crate::page_view::page_view(&before, options),
            &crate::page_view::page_view(&settled.nodes, options),
        ));
        self.commit_observation(&settled.nodes, settled.root_pid, Some(settled.scope))
            .await;
        report
    }

    /// Makes the element an action names resolvable from the cache. An id or
    /// selector the cached snapshot lacks gets one rescan of the same scope:
    /// a found element joins the cache (the whole rescan when nothing was
    /// cached yet); an id whose element is gone is reported as such.
    async fn ensure_resolvable(
        &self,
        element_index: Option<u32>,
        selector: &ElementSelector<'_>,
        purpose: ElementResolvePurpose,
    ) -> std::result::Result<(), String> {
        if element_index.is_none() && selector.is_empty() {
            return Ok(());
        }
        let cached_error = match self.resolve_cached_node(element_index, selector, purpose) {
            Ok(_) => return Ok(()),
            Err(message) => message,
        };
        if element_index.is_none() && self.cache_matches(selector) {
            // An ambiguous selector: a rescan cannot make it unique.
            return Err(cached_error);
        }
        let hint = match element_index {
            Some(id) => match self
                .element_ids
                .lock()
                .ok()
                .and_then(|ids| ids.object_ref(id).map(str::to_string))
            {
                Some(object_ref) => Some(object_ref),
                None => {
                    return Err(format!(
                        "element [{id}] was never reported; use an id from an earlier result"
                    ))
                }
            },
            None => None,
        };
        let fresh = self.rescan(hint.as_deref()).await?.nodes;
        let node = match element_index {
            Some(id) => fresh
                .iter()
                .find(|node| node.index == id)
                .cloned()
                .ok_or_else(|| format!("element [{id}] no longer exists"))?,
            None => resolve_semantic_node(&fresh, selector, purpose)?,
        };
        if let Ok(mut cached) = self.last_snapshot.lock() {
            if cached.nodes.is_empty() {
                cached.nodes = fresh;
            } else {
                cached.nodes.push(node);
            }
        }
        Ok(())
    }

    /// Whether any cached node matches the selector at all.
    fn cache_matches(&self, selector: &ElementSelector<'_>) -> bool {
        self.last_snapshot.lock().is_ok_and(|cached| {
            cached
                .nodes
                .iter()
                .any(|node| node_matches_selector(node, selector))
        })
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

    /// The cached node a field-setting call names, or `Ok(None)` when it
    /// passes a raw `element_identifier`, which the caller reads fresh.
    fn resolve_field_node(
        &self,
        element_index: Option<u32>,
        element_identifier: Option<&str>,
        selector: &ElementSelector<'_>,
    ) -> std::result::Result<Option<AccessibilityNode>, String> {
        if element_identifier.is_some_and(|value| !value.trim().is_empty()) {
            return Ok(None);
        }
        self.resolve_cached_node(element_index, selector, ElementResolvePurpose::SetValue)
            .map(Some)
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
        if params
            .element_identifier
            .as_deref()
            .is_none_or(|value| value.trim().is_empty())
        {
            if let Err(message) = self
                .ensure_resolvable(
                    params.element_index,
                    &params.selector(),
                    ElementResolvePurpose::Action,
                )
                .await
            {
                return Json(ActionOutput {
                    ok: false,
                    implemented: true,
                    action: "perform_action".to_string(),
                    message,
                    received,
                });
            }
        }
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

        let label = self
            .last_snapshot
            .lock()
            .ok()
            .and_then(|cached| {
                cached
                    .nodes
                    .iter()
                    .find(|node| node.object_ref == object_ref)
                    .map(crate::outline::element_label)
            })
            .unwrap_or_else(|| object_ref.clone());
        match invoke_accessibility_action(&object_ref, requested_action).await {
            Ok(invocation) => Json(ActionOutput {
                ok: invocation.ok,
                implemented: true,
                action: "perform_action".to_string(),
                message: if invocation.ok {
                    format!(
                        "invoked {} on {label}",
                        invocation
                            .action_name
                            .as_deref()
                            .filter(|name| !name.is_empty())
                            .unwrap_or("the primary action")
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

/// What `set_value` did to one field.
#[derive(Debug, Clone)]
struct FieldOutcome {
    ok: bool,
    /// The field's value read back after the change, when readable.
    value: Option<String>,
    message: String,
}

impl FieldOutcome {
    fn failed(message: impl Into<String>) -> Self {
        Self {
            ok: false,
            value: None,
            message: message.into(),
        }
    }
}

/// How long `set_value` polls a field for the value it entered.
const FIELD_READBACK_TIMEOUT: Duration = Duration::from_millis(1_200);
/// How long a combo box gets to show its options after being opened.
const OPTIONS_APPEAR_TIMEOUT: Duration = Duration::from_millis(1_500);

impl ComputerUseLinux {
    /// Sets one field the way a user would and reads it back: AT-SPI
    /// EditableText/Value when exposed, else per kind (text entry, toggle,
    /// option choice). `ok` only when the read-back value is the requested one.
    async fn set_field(&self, node: &AccessibilityNode, value: &str) -> FieldOutcome {
        let label = crate::outline::element_label(node);
        if let Some(refusal) = crate::fields::line_break_refusal(node, &label, value, false) {
            return FieldOutcome::failed(refusal);
        }
        if node.supports_editable_text
            || (node.value.is_some() && value.trim().parse::<f64>().is_ok())
        {
            if let Ok(invocation) = set_element_value(&node.object_ref, value).await {
                let (fresh, text) = match read_field(&node.object_ref).await {
                    Some((fresh, text)) => (Some(fresh), text),
                    None => (None, None),
                };
                let read_back = match invocation {
                    ValueSetInvocation::Numeric { .. } => fresh
                        .as_ref()
                        .and_then(|node| node.value.as_ref())
                        .map(|value| value.current.to_string()),
                    ValueSetInvocation::EditableText => text,
                };
                let ok = match invocation {
                    ValueSetInvocation::Numeric { value } => fresh
                        .as_ref()
                        .and_then(|node| node.value.as_ref())
                        .is_some_and(|current| (current.current - value).abs() < 1e-9),
                    ValueSetInvocation::EditableText => {
                        crate::fields::values_match(read_back.as_deref(), value)
                    }
                };
                return self.text_outcome(&label, ok, read_back, value);
            }
        }
        match crate::fields::field_kind(node) {
            crate::fields::FieldKind::Text => self.set_text_field(node, &label, value).await,
            crate::fields::FieldKind::Toggle => self.set_toggle_field(node, &label, value).await,
            crate::fields::FieldKind::Choice => {
                self.set_choice_field(node, &label, value, false).await
            }
            crate::fields::FieldKind::Other => {
                // A fieldset or radio group: pick its radio button by name.
                let outcome = self.set_choice_field(node, &label, value, true).await;
                if outcome.ok || !outcome.message.contains("no radio buttons") {
                    return outcome;
                }
                FieldOutcome::failed(format!(
                    "{label} is not a text field, check box, combo box or radio group; use click or type_text"
                ))
            }
        }
    }

    fn text_outcome(
        &self,
        label: &str,
        ok: bool,
        read_back: Option<String>,
        requested: &str,
    ) -> FieldOutcome {
        let shown = crate::fields::shown_text(read_back.as_deref());
        let formatted = crate::fields::compare_read_back(read_back.as_deref(), requested)
            == crate::fields::ReadBack::Formatted;
        FieldOutcome {
            ok,
            message: if ok && formatted {
                format!("set {label} value=\"{requested}\" (the field formatted it as \"{shown}\")")
            } else if ok {
                format!("set {label} value=\"{shown}\"")
            } else {
                format!("{label} now holds \"{shown}\", not the requested \"{requested}\"")
            },
            value: Some(shown),
        }
    }

    async fn set_text_field(
        &self,
        node: &AccessibilityNode,
        label: &str,
        value: &str,
    ) -> FieldOutcome {
        if let Err(message) = self.focus_element(node, label).await {
            return FieldOutcome::failed(message);
        }
        if let Err(message) = send_key_chord("ctrl+a").await {
            return FieldOutcome::failed(message);
        }
        let entered = if value.is_empty() {
            send_key_chord("BackSpace").await
        } else {
            insert_text(value).await
        };
        if let Err(message) = entered {
            return FieldOutcome::failed(message);
        }
        let password = node.role.eq_ignore_ascii_case("password text");
        let expected_chars = value.chars().count();
        let fresh = poll_field_text(&node.object_ref, FIELD_READBACK_TIMEOUT, |text| {
            if password {
                text.map_or(0, |text| text.chars().count()) == expected_chars
            } else {
                crate::fields::values_match(text, value)
            }
        })
        .await;
        let Some((matched, read_back)) = fresh else {
            return FieldOutcome::failed(format!(
                "the value was sent to {label}, but its text cannot be read back, so delivery is not verified; check it with get_app_state"
            ));
        };
        if password {
            let count = read_back.map_or(0, |text| text.chars().count());
            return FieldOutcome {
                ok: matched,
                message: if matched {
                    format!("set {label} ({count} characters, masked)")
                } else {
                    format!("{label} holds {count} characters, not {expected_chars}")
                },
                value: Some("•".repeat(count)),
            };
        }
        self.text_outcome(label, matched, read_back, value)
    }

    async fn set_toggle_field(
        &self,
        node: &AccessibilityNode,
        label: &str,
        value: &str,
    ) -> FieldOutcome {
        let Some(wanted) = crate::fields::parse_toggle(value) else {
            return FieldOutcome::failed(format!(
                "{label} is a {}; pass value \"true\" or \"false\"",
                node.role
            ));
        };
        let current = match crate::atspi_tree::read_element(&node.object_ref).await {
            Ok(fresh) => crate::fields::is_checked(&fresh),
            Err(error) => return FieldOutcome::failed(format!("{label}: {error:#}")),
        };
        let state_word = |checked: bool| if checked { "checked" } else { "unchecked" };
        if current == wanted {
            return FieldOutcome {
                ok: true,
                value: Some(wanted.to_string()),
                message: format!("{label} is already {}", state_word(wanted)),
            };
        }
        if !wanted && node.role.to_ascii_lowercase().contains("radio") {
            return FieldOutcome::failed(format!(
                "{label} is a radio button; it cannot be cleared, set another option of its group instead"
            ));
        }
        if let Err(message) = self.activate_element(node, label, TOGGLE_ACTIONS).await {
            return FieldOutcome::failed(message);
        }
        let fresh = poll_element(&node.object_ref, FIELD_READBACK_TIMEOUT, |fresh| {
            crate::fields::is_checked(fresh) == wanted
        })
        .await;
        let (matched, now) = match fresh {
            Some((matched, fresh)) => (matched, crate::fields::is_checked(&fresh)),
            None => (false, current),
        };
        FieldOutcome {
            ok: matched,
            value: Some(now.to_string()),
            message: if matched {
                format!("{label} is now {}", state_word(now))
            } else {
                format!("{label} stayed {} after activation", state_word(now))
            },
        }
    }

    /// Picks the option named by `value` in a combo box or list box (or, with
    /// `radio_group`, the radio button of a group), opening it when its
    /// options are not in the tree yet. Options are invoked through their
    /// AT-SPI action, so options scrolled out of the popup still work.
    async fn set_choice_field(
        &self,
        node: &AccessibilityNode,
        label: &str,
        value: &str,
        radio_group: bool,
    ) -> FieldOutcome {
        let pid = object_ref_owner_pid(&node.object_ref).await.ok().flatten();
        let mut nodes = match self.fresh_app_nodes(pid).await {
            Ok(nodes) => nodes,
            Err(error) => return FieldOutcome::failed(format!("{label}: {error}")),
        };
        let Some(mut target) = nodes
            .iter()
            .position(|candidate| candidate.object_ref == node.object_ref)
        else {
            return FieldOutcome::failed(format!("{label} is no longer in the tree"));
        };
        let mut options = crate::fields::option_candidates(&nodes, target, None, radio_group);
        if radio_group && options.is_empty() {
            return FieldOutcome::failed(format!("{label} has no radio buttons"));
        }
        let names = |nodes: &[AccessibilityNode], options: &[usize]| -> Vec<String> {
            options
                .iter()
                .map(|&at| nodes[at].name.clone().unwrap_or_default())
                .collect()
        };
        let mut matched = {
            let option_names = names(&nodes, &options);
            let refs: Vec<&str> = option_names.iter().map(String::as_str).collect();
            crate::fields::match_option(&refs, value)
        };
        if matched.is_err() && !radio_group && !crate::fields::has_state(node, "expanded") {
            // Collapsed custom combo boxes keep their options out of the tree.
            let before: std::collections::HashSet<String> =
                nodes.iter().map(|node| node.object_ref.clone()).collect();
            if let Err(message) = self.activate_element(node, label, EXPAND_ACTIONS).await {
                return FieldOutcome::failed(message);
            }
            let started = std::time::Instant::now();
            loop {
                sleep(Duration::from_millis(150)).await;
                if let Ok(fresh) = self.fresh_app_nodes(pid).await {
                    if let Some(at) = fresh
                        .iter()
                        .position(|candidate| candidate.object_ref == node.object_ref)
                    {
                        nodes = fresh;
                        target = at;
                        options =
                            crate::fields::option_candidates(&nodes, target, Some(&before), false);
                        let option_names = names(&nodes, &options);
                        let refs: Vec<&str> = option_names.iter().map(String::as_str).collect();
                        matched = crate::fields::match_option(&refs, value);
                        if matched.is_ok() {
                            break;
                        }
                    }
                }
                if started.elapsed() >= OPTIONS_APPEAR_TIMEOUT {
                    break;
                }
            }
        }
        let option = match matched {
            Ok(index) => nodes[options[index]].clone(),
            Err(message) => return FieldOutcome::failed(format!("{label}: {message}")),
        };
        let option_label = crate::outline::element_label(&option);
        let option_name = option.name.clone().unwrap_or_default();
        if radio_group {
            let outcome = self.set_toggle_field(&option, &option_label, "true").await;
            return FieldOutcome {
                value: outcome.ok.then(|| option_name.clone()),
                message: if outcome.ok {
                    format!("set {label} to {option_label}")
                } else {
                    outcome.message
                },
                ok: outcome.ok,
            };
        }
        if let Err(message) = self
            .activate_element(&option, &option_label, OPTION_ACTIONS)
            .await
        {
            return FieldOutcome::failed(message);
        }
        let mut chosen = self
            .choice_read_back(&node.object_ref, &option.object_ref, &option_name)
            .await;
        if !chosen.0 {
            // Some popups react only to real pointer input (mousedown handlers):
            // bring the option into view and click it.
            if let Some(point) = self.visible_center_after_scroll(&option.object_ref).await {
                if click_point(point).await.is_ok() {
                    chosen = self
                        .choice_read_back(&node.object_ref, &option.object_ref, &option_name)
                        .await;
                }
            }
        }
        let (ok, shown) = chosen;
        FieldOutcome {
            ok,
            message: if ok {
                format!("set {label} to \"{option_name}\" (shows \"{shown}\")")
            } else {
                format!(
                    "invoked {option_label}, but {label} shows \"{shown}\"; the choice did not take"
                )
            },
            value: Some(shown),
        }
    }

    /// A compacted snapshot of one app with stable ids, for internal lookups
    /// that must not replace the agent's cached snapshot.
    async fn fresh_app_nodes(&self, pid: Option<u32>) -> Result<Vec<AccessibilityNode>, String> {
        let (max_nodes, max_depth) = crate::atspi_tree::snapshot_limits(Some(2_000), None);
        let snapshot = snapshot_accessibility_tree(None, pid, max_nodes, max_depth)
            .await
            .map_err(|error| format!("{error:#}"))?;
        let mut nodes = compact_accessibility_tree(snapshot.nodes);
        self.assign_ids(&mut nodes);
        Ok(nodes)
    }

    /// Whether the combo box now shows the option (or the option reports
    /// `selected`), and what the combo box shows.
    async fn choice_read_back(
        &self,
        combo_ref: &str,
        option_ref: &str,
        option_name: &str,
    ) -> (bool, String) {
        let wanted = crate::outline::clean(Some(option_name))
            .unwrap_or_default()
            .to_lowercase();
        let started = std::time::Instant::now();
        loop {
            sleep(Duration::from_millis(100)).await;
            let shown = crate::atspi_tree::read_element(combo_ref)
                .await
                .ok()
                .and_then(|combo| crate::fields::choice_text(&combo))
                .unwrap_or_default();
            let option_selected = crate::atspi_tree::read_element(option_ref)
                .await
                .is_ok_and(|option| crate::fields::has_state(&option, "selected"));
            let shows_option = !wanted.is_empty() && shown.to_lowercase().contains(&wanted);
            if shows_option || option_selected {
                let shown = if shows_option || shown.is_empty() {
                    if shown.is_empty() {
                        option_name.to_string()
                    } else {
                        shown
                    }
                } else {
                    option_name.to_string()
                };
                return (true, shown);
            }
            if started.elapsed() >= FIELD_READBACK_TIMEOUT {
                return (false, shown);
            }
        }
    }

    /// Gives an element keyboard focus: its app window first, then AT-SPI
    /// GrabFocus, then a click on its centre when it is on screen. Succeeds
    /// only once AT-SPI reports the element `focused`.
    async fn focus_element(&self, node: &AccessibilityNode, label: &str) -> Result<(), String> {
        self.raise_owner_window(&node.object_ref).await;
        let _ = crate::atspi_tree::grab_focus(&node.object_ref).await;
        let focused = |fresh: &AccessibilityNode| crate::fields::has_state(fresh, "focused");
        if let Some((true, _)) =
            poll_element(&node.object_ref, Duration::from_millis(400), focused).await
        {
            return Ok(());
        }
        if let Some(point) = self.visible_center_after_scroll(&node.object_ref).await {
            click_point(point).await?;
            if let Some((true, _)) =
                poll_element(&node.object_ref, Duration::from_millis(600), focused).await
            {
                return Ok(());
            }
        }
        Err(format!(
            "could not focus {label}; it may be disabled or covered. Click it, then use type_text."
        ))
    }

    /// Invokes the first of `actions` the element exposes, else clicks its
    /// centre when it is on screen.
    async fn activate_element(
        &self,
        node: &AccessibilityNode,
        label: &str,
        actions: &[&str],
    ) -> Result<String, String> {
        match crate::atspi_tree::invoke_preferred_action(&node.object_ref, actions).await {
            Ok(Some(invocation)) if invocation.ok => {
                return Ok(invocation.action_name.unwrap_or_default());
            }
            Ok(_) => {}
            Err(error) => return Err(format!("{label}: {error:#}")),
        }
        self.raise_owner_window(&node.object_ref).await;
        match self.visible_center_after_scroll(&node.object_ref).await {
            Some(point) => click_point(point)
                .await
                .map(|()| "pointer click".to_string()),
            None => Err(format!(
                "{label} exposes no usable AT-SPI action and is not on screen to click"
            )),
        }
    }

    /// Scrolls step by step until an element matching the query is rendered,
    /// then brings it onto the visible page: returns its line with a few
    /// neighbours, the scroll count and the time taken, and makes the last
    /// read the cached snapshot (so the ids it shows resolve) in place of a
    /// change report. Each step moves about `SCROLL_UNTIL_STEP_SHARE` of the
    /// scrolled area (`until_step_clicks`, recalibrated from how far rows
    /// moved: `measured_click_px`) and reads the tree once
    /// (`read_after_scroll_step`); `until_step` decides what follows.
    /// Without a requested direction a search down that reaches the end goes
    /// on upward (`next_until_pass`). On failure the last read is cached too,
    /// since the page has moved.
    async fn scroll_until(&self, plan: &ScrollUntil) -> std::result::Result<String, String> {
        let started = std::time::Instant::now();
        let hint = plan.hint_ref.as_deref();
        let cached = self
            .last_snapshot
            .lock()
            .map(|cached| cached.nodes.clone())
            .unwrap_or_default();
        let mut last = self.read_page(&cached, hint).await?;
        if !rendered_matches(&last.nodes, &plan.query).is_empty() {
            return self.reveal_until_match(plan, last, 0, started).await;
        }
        let horizontal = matches!(
            plan.direction,
            ScrollDirection::Left | ScrollDirection::Right
        );
        let shown = crate::page_view::shown_part(&last.nodes, plan.container);
        let extent = shown.map(|area| if horizontal { area.width } else { area.height });
        let mut step = UntilStepSize {
            extent,
            click_px: f64::from(WHEEL_CLICK_PX),
            measured: false,
            max_clicks: plan.max_clicks,
        };
        let mut direction = plan.direction;
        let mut scrolls = 0;
        let mut searched_back = false;
        let end = loop {
            let end = self
                .scroll_until_pass(plan, direction, &mut step, &mut last, &mut scrolls)
                .await?;
            match next_until_pass(direction, plan.direction_given, end) {
                Some(next) => {
                    eprintln!("scroll until: the end going {direction:?}; searching {next:?}");
                    direction = next;
                    searched_back = true;
                }
                None => break end,
            }
        };
        let message = match end {
            UntilStep::Found => return self.reveal_until_match(plan, last, scrolls, started).await,
            UntilStep::Cap => format!(
                "no element matched until within {SCROLL_UNTIL_MAX_STEPS} scrolls; the list goes on"
            ),
            _ if searched_back => format!(
                "reached both ends after {scrolls} scroll(s) without an element matching until"
            ),
            _ => format!(
                "reached the end after {scrolls} scroll(s) without an element matching until"
            ),
        };
        self.cache_observation(&last.nodes, last.root_pid, Some(last.scope));
        Err(message)
    }

    /// Scrolls in one direction until a match is rendered, the end, or
    /// `SCROLL_UNTIL_MAX_STEPS` steps; `step`, `last` and `scrolls` follow
    /// along.
    async fn scroll_until_pass(
        &self,
        plan: &ScrollUntil,
        direction: ScrollDirection,
        step: &mut UntilStepSize,
        last: &mut Rescan,
        scrolls: &mut u32,
    ) -> std::result::Result<UntilStep, String> {
        let horizontal = matches!(direction, ScrollDirection::Left | ScrollDirection::Right);
        let mut previous = crate::outline::element_contents(&last.nodes);
        let mut steps = 0;
        let mut unchanged = 0;
        loop {
            let clicks = step.clicks();
            let wheel = xdotool_pointer_scroll_args(plan.point, clicks, direction);
            run_xdotool(
                &wheel,
                xdotool_scroll_timeout(clicks, XDOTOOL_SCROLL_CLICK_DELAY_MS),
            )
            .await?;
            steps += 1;
            *scrolls += 1;
            let read = self
                .read_after_scroll_step(plan.hint_ref.as_deref(), &last.nodes, &previous)
                .await?;
            let before = std::mem::replace(last, read);
            let rows_before = row_keys(&before.nodes);
            let new_rows = row_keys(&last.nodes).difference(&rows_before).count();
            if let Some(px) = measured_click_px(&before.nodes, &last.nodes, clicks, horizontal) {
                step.click_px = px;
                step.measured = true;
            }
            eprintln!("scroll until step {scrolls}: +{new_rows} rows ({clicks} wheel clicks {direction:?}, {:.0} px each)", step.click_px);
            let contents = crate::outline::element_contents(&last.nodes);
            let found = !rendered_matches(&last.nodes, &plan.query).is_empty();
            let next = until_step(found, contents != previous, unchanged, steps);
            previous = contents;
            match next {
                UntilStep::Continue => unchanged = 0,
                UntilStep::Recheck => {
                    unchanged += 1;
                    // A list that loads more rows may still be fetching them.
                    sleep(SCROLL_UNTIL_END_WAIT).await;
                }
                end => return Ok(end),
            }
        }
    }

    /// Reads the page (`read_page`, `base` being the read before the step)
    /// after a scroll step: once after `SCROLL_UNTIL_STEP_WAIT`, and a second
    /// time after `SETTLE_POLL` only when it still equals `previous` (the
    /// contents of `base`), since a virtual list may not have re-rendered yet.
    async fn read_after_scroll_step(
        &self,
        hint: Option<&str>,
        base: &[AccessibilityNode],
        previous: &[crate::outline::ElementContent],
    ) -> Result<Rescan, String> {
        sleep(SCROLL_UNTIL_STEP_WAIT).await;
        let first = self.read_page(base, hint).await?;
        if crate::outline::element_contents(&first.nodes) != previous {
            return Ok(first);
        }
        sleep(SETTLE_POLL).await;
        self.read_page(&first.nodes, hint).await
    }

    /// Brings a rendered match onto the visible page: AT-SPI
    /// `Component.ScrollTo`, else a few small wheel scrolls toward it.
    /// Reports its line and up to `SCROLL_UNTIL_NEIGHBOURS` lines around it
    /// once it is visible, or that it stayed off the page, and caches the
    /// last read.
    async fn reveal_until_match(
        &self,
        plan: &ScrollUntil,
        mut last: Rescan,
        scrolls: u32,
        started: std::time::Instant,
    ) -> std::result::Result<String, String> {
        let hint = plan.hint_ref.as_deref();
        let mut revealed = None;
        for attempt in 0..=SCROLL_UNTIL_REVEAL_NUDGES {
            if let Some(first) = visible_matches(&last.nodes, &plan.query).first() {
                revealed = Some((first.index, attempt > 0));
                break;
            }
            let rendered = rendered_matches(&last.nodes, &plan.query);
            let Some(target) = rendered.first() else {
                break;
            };
            let toward = target
                .bounds
                .as_ref()
                .zip(crate::page_view::visible_area(&last.nodes, target.index));
            if attempt == 0
                && crate::atspi_tree::scroll_into_view(&target.object_ref)
                    .await
                    .is_ok()
            {
                eprintln!("scroll until: ScrollTo [{}]", target.index);
            } else if let Some(direction) =
                toward.and_then(|(bounds, area)| direction_toward(bounds, &area))
            {
                eprintln!(
                    "scroll until: nudging {direction:?} toward [{}]",
                    target.index
                );
                let wheel = xdotool_pointer_scroll_args(plan.point, 2, direction);
                run_xdotool(
                    &wheel,
                    xdotool_scroll_timeout(2, XDOTOOL_SCROLL_CLICK_DELAY_MS),
                )
                .await?;
            } else {
                break;
            }
            let previous = crate::outline::element_contents(&last.nodes);
            last = self
                .read_after_scroll_step(hint, &last.nodes, &previous)
                .await?;
        }
        let rendered: Vec<u32> = rendered_matches(&last.nodes, &plan.query)
            .iter()
            .map(|node| node.index)
            .collect();
        let view = crate::page_view::page_view(&last.nodes, last.scope.options);
        let seconds = started.elapsed().as_secs_f64();
        let message = match (revealed, rendered.first()) {
            (Some((id, nudged)), _) => {
                let header = match (scrolls, nudged) {
                    (0, false) => "already on the visible page:".to_string(),
                    (0, true) => format!("found and scrolled into view in {seconds:.1} s:"),
                    (n, _) => format!("found after {n} scroll(s) in {seconds:.1} s:"),
                };
                let mut message = format!(
                    "{header}\n{}",
                    until_match_lines(&view, &last.nodes, id)
                );
                if rendered.len() > 1 {
                    message.push_str(&format!("\n(and {} more matching)", rendered.len() - 1));
                }
                Ok(message)
            }
            (None, Some(&id)) => Ok(format!(
                "found after {scrolls} scroll(s) in {seconds:.1} s, but it is not on the visible page yet (click still reaches it by id):\n{}",
                until_match_lines(&view, &last.nodes, id)
            )),
            (None, None) => Err(format!(
                "an element matching until showed up after {scrolls} scroll(s) but was gone after scrolling it into view"
            )),
        };
        self.cache_observation(&last.nodes, last.root_pid, Some(last.scope));
        message
    }

    /// Whether a point inside an element's bounds is on the visible page:
    /// inside its web document's viewport (else its window frame) and on the
    /// desktop.
    async fn point_visible_for(&self, node: &AccessibilityNode, (x, y): (i32, i32)) -> bool {
        let area = self
            .last_snapshot
            .lock()
            .ok()
            .and_then(|cached| crate::page_view::visible_area(&cached.nodes, node.index));
        let in_area = area.is_none_or(|area| {
            x >= area.x && y >= area.y && x < area.x + area.width && y < area.y + area.height
        });
        let on_desktop = self.desktop_size().await.is_none_or(|(width, height)| {
            x >= 0 && y >= 0 && (x as u32) < width && (y as u32) < height
        });
        in_area && on_desktop
    }

    /// Clicks an element whose centre is not on the visible page: its AT-SPI
    /// action (plain left clicks only), else `Component.ScrollTo` and the
    /// centre it then has, else an error naming the fix.
    async fn off_screen_click(
        &self,
        node: &AccessibilityNode,
        label: &str,
        plain_left_click: bool,
    ) -> std::result::Result<OffScreenClick, String> {
        if plain_left_click {
            match crate::atspi_tree::invoke_preferred_action(
                &node.object_ref,
                OFF_SCREEN_CLICK_ACTIONS,
            )
            .await
            {
                Ok(Some(invocation)) if invocation.ok => {
                    return Ok(OffScreenClick::Invoked(format!(
                        "clicked {label} (AT-SPI {}; it is outside the visible page)",
                        invocation.action_name.unwrap_or_default()
                    )));
                }
                Ok(_) => {}
                Err(error) => return Err(format!("{label}: {error:#}")),
            }
        }
        if crate::atspi_tree::scroll_into_view(&node.object_ref)
            .await
            .is_ok()
        {
            sleep(Duration::from_millis(150)).await;
            if let Ok(fresh) = crate::atspi_tree::read_element(&node.object_ref).await {
                if let Some(center) = fresh.bounds.as_ref().and_then(bounds_center) {
                    let mut moved = fresh;
                    moved.index = node.index;
                    if self.point_visible_for(&moved, center).await {
                        return Ok(OffScreenClick::ScrolledTo(center.0, center.1));
                    }
                }
            }
        }
        Err(format!(
            "element [{id}] is outside the visible window; scroll it into view with scroll(element_index={id})",
            id = node.index
        ))
    }

    /// The element's centre when it lies on the desktop, scrolling it into
    /// view through AT-SPI first when it does not.
    async fn visible_center_after_scroll(&self, object_ref: &str) -> Option<(i32, i32)> {
        let id = self
            .element_ids
            .lock()
            .map(|mut ids| ids.id_for(object_ref))
            .unwrap_or_default();
        let fresh = crate::atspi_tree::read_element(object_ref).await.ok()?;
        let mut fresh = fresh;
        fresh.index = id;
        if let Some(point) = fresh.bounds.as_ref().and_then(bounds_center) {
            if self.point_visible_for(&fresh, point).await {
                return Some(point);
            }
        }
        crate::atspi_tree::scroll_into_view(object_ref).await.ok()?;
        sleep(Duration::from_millis(150)).await;
        let mut moved = crate::atspi_tree::read_element(object_ref).await.ok()?;
        moved.index = id;
        let point = moved.bounds.as_ref().and_then(bounds_center)?;
        self.point_visible_for(&moved, point).await.then_some(point)
    }

    /// Focus the window of the app that owns the element unless it already
    /// has focus, so keyboard input reaches that app. Best effort.
    async fn raise_owner_window(&self, object_ref: &str) {
        let Ok(Some(pid)) = object_ref_owner_pid(object_ref).await else {
            return;
        };
        if let Ok(Some(window)) = focused_window().await {
            if window.pid == Some(pid) {
                return;
            }
        }
        let target = WindowTarget {
            pid: Some(pid),
            ..WindowTarget::default()
        };
        if focus_window_target(&target).await.is_ok() {
            sleep(Duration::from_millis(120)).await;
        }
    }
}

/// Actions whose failure may come from a dialog in front of their target,
/// so their errors name one when it is open.
const DIALOG_CHECKED_ACTIONS: [&str; 5] = [
    "click",
    "set_value",
    "fill_form",
    "perform_action",
    "type_text",
];

/// Whether the settle loop keeps reading because of `busy`: always for a
/// strong sign, and for a button that only became disabled until the tree
/// has been unchanged for `BUSY_QUIET`.
fn still_busy_after(busy: Option<&crate::page_view::Busy>, quiet_for: Duration) -> bool {
    busy.is_some_and(|busy| busy.strong || quiet_for < BUSY_QUIET)
}

/// Most scroll steps one `scroll(until=…)` call makes in one direction.
const SCROLL_UNTIL_MAX_STEPS: u32 = 80;
/// Wait after a scroll step before reading the tree.
const SCROLL_UNTIL_STEP_WAIT: Duration = Duration::from_millis(100);
/// Small wheel scrolls toward a rendered match ScrollTo did not reveal.
const SCROLL_UNTIL_REVEAL_NUDGES: u32 = 3;
/// Lines around the match a successful `scroll(until=…)` prints.
const SCROLL_UNTIL_NEIGHBOURS: usize = 3;
/// About how far one X11 wheel click scrolls before a step measures it
/// (`measured_click_px`): Chromium moves 53 px, GTK 3 a little more on a
/// typical page.
const WHEEL_CLICK_PX: i32 = 53;
/// Wheel clicks per `until` step when neither the scrolled area's extent
/// nor `pages` sizes it.
const SCROLL_UNTIL_DEFAULT_CLICKS: u32 = 5;
/// Share of the scrolled area's visible extent one `until` step moves once
/// a step has measured a wheel click. Matches anywhere in the rendered tree
/// count, and virtual lists render rows beyond the viewport, so the 10 %
/// overlap skips no row.
const SCROLL_UNTIL_STEP_SHARE: f64 = 0.9;
/// The share before any measurement, while a click's distance is a guess.
const SCROLL_UNTIL_FIRST_STEP_SHARE: f64 = 0.6;
/// Extra wait after a scroll that changed nothing, before trying once more:
/// an infinite list may still be loading its next rows.
const SCROLL_UNTIL_END_WAIT: Duration = Duration::from_millis(500);

/// One `scroll(until=…)` request, resolved.
struct ScrollUntil {
    query: ElementQuery,
    /// Where the wheel turns; `None` scrolls under the pointer.
    point: Option<(i32, i32)>,
    direction: ScrollDirection,
    /// Whether the caller chose `direction`; if not, the search also goes up.
    direction_given: bool,
    /// Most wheel clicks per step, when the caller passed `pages`.
    max_clicks: Option<u32>,
    /// The scrolled element's id and object, when one was given.
    container: Option<u32>,
    hint_ref: Option<String>,
}

/// How far each `scroll(until=…)` step goes.
#[derive(Debug, Clone, Copy)]
struct UntilStepSize {
    /// Visible extent of the scrolled area along the scroll, in pixels.
    extent: Option<i32>,
    /// Pixels one wheel click moves, as last measured, else `WHEEL_CLICK_PX`.
    click_px: f64,
    /// Whether `click_px` was measured.
    measured: bool,
    /// Most wheel clicks per step, from `pages`.
    max_clicks: Option<u32>,
}

impl UntilStepSize {
    /// Wheel clicks that move about `SCROLL_UNTIL_STEP_SHARE` of the
    /// visible extent (`SCROLL_UNTIL_FIRST_STEP_SHARE` until measured), at
    /// least one, at most `max_clicks`.
    fn clicks(&self) -> u32 {
        let share = if self.measured {
            SCROLL_UNTIL_STEP_SHARE
        } else {
            SCROLL_UNTIL_FIRST_STEP_SHARE
        };
        let fit = self
            .extent
            .filter(|&extent| extent > 0)
            .map(|extent| (f64::from(extent) * share / self.click_px.max(1.0)).floor() as u32);
        match (fit, self.max_clicks) {
            (Some(fit), Some(max)) => fit.clamp(1, max.max(1)),
            (Some(fit), None) => fit.max(1),
            (None, Some(max)) => max.max(1),
            (None, None) => SCROLL_UNTIL_DEFAULT_CLICKS,
        }
    }
}

/// What `scroll(until=…)` does after the tree settled following a step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UntilStep {
    /// A match is rendered: bring it into view.
    Found,
    /// The page moved: scroll again.
    Continue,
    /// Nothing moved once: wait for more rows, then scroll again.
    Recheck,
    /// Nothing moved twice in a row: this is the end of the list.
    End,
    /// `SCROLL_UNTIL_MAX_STEPS` steps in this direction.
    Cap,
}

/// Decides the next move from a step's result: `moved` says the tree
/// differs from the read before the step, `unchanged` counts the steps in a
/// row before this one that moved nothing, `steps` this direction's steps.
fn until_step(found: bool, moved: bool, unchanged: u32, steps: u32) -> UntilStep {
    if found {
        UntilStep::Found
    } else if !moved && unchanged >= 1 {
        UntilStep::End
    } else if steps >= SCROLL_UNTIL_MAX_STEPS {
        UntilStep::Cap
    } else if moved {
        UntilStep::Continue
    } else {
        UntilStep::Recheck
    }
}

/// The direction to search next after a pass ended with `end`: up, when a
/// search down with no requested direction reached the end of the list.
fn next_until_pass(
    direction: ScrollDirection,
    direction_given: bool,
    end: UntilStep,
) -> Option<ScrollDirection> {
    (end == UntilStep::End && !direction_given && direction == ScrollDirection::Down)
        .then_some(ScrollDirection::Up)
}

/// Pixels one wheel click moved, from elements present before and after a
/// step of `clicks` clicks, matched by role and label (labels that repeat
/// in a read are skipped): the median of their shifts along the scroll,
/// divided by `clicks`. Elements that stayed put (sticky headers) do not
/// count. `None` when nothing moved or the result is implausible (under 5
/// or over 1000 pixels).
fn measured_click_px(
    before: &[AccessibilityNode],
    after: &[AccessibilityNode],
    clicks: u32,
    horizontal: bool,
) -> Option<f64> {
    let positions = |nodes: &[AccessibilityNode]| -> std::collections::HashMap<String, i32> {
        let mut positions = std::collections::HashMap::new();
        let mut repeated = std::collections::HashSet::new();
        for node in nodes {
            let (Some(label), Some(bounds)) = (
                crate::outline::clean(node.name.as_deref()),
                node.bounds.as_ref(),
            ) else {
                continue;
            };
            let key = format!("{} {label}", node.role);
            let at = if horizontal { bounds.x } else { bounds.y };
            if positions.insert(key.clone(), at).is_some() {
                repeated.insert(key);
            }
        }
        positions.retain(|key, _| !repeated.contains(key));
        positions
    };
    let earlier = positions(before);
    let mut shifts: Vec<i32> = positions(after)
        .into_iter()
        .filter_map(|(key, now)| Some((earlier.get(&key)? - now).abs()))
        .filter(|&shift| shift > 0)
        .collect();
    if shifts.is_empty() || clicks == 0 {
        return None;
    }
    shifts.sort_unstable();
    let px = f64::from(shifts[shifts.len() / 2]) / f64::from(clicks);
    (5.0..=1000.0).contains(&px).then_some(px)
}

/// The outline lines a successful `scroll(until=…)` prints for match `id`:
/// its line and up to `SCROLL_UNTIL_NEIGHBOURS` lines around it in the page
/// view (one before, the rest after), indented as in the outline from the
/// shallowest of them. Just its line when the view does not show it.
fn until_match_lines(
    view: &crate::page_view::PageView,
    nodes: &[AccessibilityNode],
    id: u32,
) -> String {
    let Some(at) = view.entries.iter().position(|entry| entry.id == id) else {
        return nodes
            .iter()
            .find(|node| node.index == id)
            .map(crate::outline::render_line)
            .unwrap_or_default();
    };
    let start = at.saturating_sub(1);
    let end = (start + SCROLL_UNTIL_NEIGHBOURS + 1).min(view.entries.len());
    let shown = &view.entries[start..end];
    let base = shown
        .iter()
        .map(|entry| entry.depth)
        .min()
        .unwrap_or_default();
    shown
        .iter()
        .map(|entry| format!("{}{}", "  ".repeat(entry.depth - base), entry.line))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The wheel direction that moves `bounds` toward `area`, `None` when its
/// centre is already inside.
fn direction_toward(bounds: &Bounds, area: &Bounds) -> Option<ScrollDirection> {
    let (x, y) = bounds_center(bounds)?;
    if y < area.y {
        Some(ScrollDirection::Up)
    } else if y >= area.y + area.height {
        Some(ScrollDirection::Down)
    } else if x < area.x {
        Some(ScrollDirection::Left)
    } else if x >= area.x + area.width {
        Some(ScrollDirection::Right)
    } else {
        None
    }
}

/// Roles of list, table and tree rows, which scroll step logs count.
const ROW_ROLES: [&str; 5] = [
    "list item",
    "row",
    "table row",
    "tree item",
    "tree table row",
];

/// Each row's role and label, to count rows a scroll step brought in.
fn row_keys(nodes: &[AccessibilityNode]) -> std::collections::HashSet<String> {
    nodes
        .iter()
        .filter(|node| ROW_ROLES.contains(&node.role.as_str()))
        .filter_map(|node| {
            let label = crate::outline::clean(node.name.as_deref()).or_else(|| {
                crate::outline::clean(node.text.as_ref().and_then(|text| text.content.as_deref()))
            })?;
            Some(format!("{} {label}", node.role))
        })
        .collect()
}

/// Elements matching `query` with a size, wherever they are: a virtual
/// list's rows rendered around the viewport count.
fn rendered_matches<'a>(
    nodes: &'a [AccessibilityNode],
    query: &ElementQuery,
) -> Vec<&'a AccessibilityNode> {
    nodes
        .iter()
        .filter(|node| query.matches(node))
        .filter(|node| node.bounds.as_ref().and_then(bounds_center).is_some())
        .collect()
}

/// Elements matching `query` whose centre lies where they can be seen: in
/// their document's viewport, else their window frame.
fn visible_matches<'a>(
    nodes: &'a [AccessibilityNode],
    query: &ElementQuery,
) -> Vec<&'a AccessibilityNode> {
    nodes
        .iter()
        .filter(|node| query.matches(node))
        .filter(|node| {
            let Some((x, y)) = node.bounds.as_ref().and_then(bounds_center) else {
                return false;
            };
            crate::page_view::visible_area(nodes, node.index).is_none_or(|area| {
                x >= area.x && y >= area.y && x < area.x + area.width && y < area.y + area.height
            })
        })
        .collect()
}

/// Actions that flip a check box, radio button or switch.
const TOGGLE_ACTIONS: &[&str] = &[
    "toggle", "check", "uncheck", "click", "press", "activate", "select",
];
/// Actions that open a combo box popup.
const EXPAND_ACTIONS: &[&str] = &["open", "expand", "show", "click", "press", "activate"];
/// Actions that choose an option.
const OPTION_ACTIONS: &[&str] = &["select", "click", "press", "activate", "jump", "dodefault"];

/// Rereads an element until `done` holds or `limit` passes. `None` when the
/// element cannot be read at all; otherwise whether `done` held, and the last read.
async fn poll_element(
    object_ref: &str,
    limit: Duration,
    done: impl Fn(&AccessibilityNode) -> bool,
) -> Option<(bool, AccessibilityNode)> {
    let started = std::time::Instant::now();
    let mut last = None;
    loop {
        if let Ok(fresh) = crate::atspi_tree::read_element(object_ref).await {
            if done(&fresh) {
                return Some((true, fresh));
            }
            last = Some(fresh);
        }
        if started.elapsed() >= limit {
            return last.map(|fresh| (false, fresh));
        }
        sleep(Duration::from_millis(80)).await;
    }
}

/// Most descendants of a rich-text editor read to expand its text.
const FIELD_SUBTREE_MAX_NODES: usize = 400;
/// Deepest descendant level of an editor read to expand its text.
const FIELD_SUBTREE_MAX_DEPTH: u32 = 12;

/// Rereads a field and the text it holds (`fields::field_text`). An editor
/// whose text has U+FFFC for embedded paragraphs (a Chromium
/// contenteditable) is read with its descendants and expanded
/// (`fields::expanded_text`). `None` when the field cannot be read.
async fn read_field(object_ref: &str) -> Option<(AccessibilityNode, Option<String>)> {
    let fresh = crate::atspi_tree::read_element(object_ref).await.ok()?;
    let text = crate::fields::field_text(&fresh);
    if text
        .as_deref()
        .is_some_and(|text| text.contains(crate::fields::EMBEDDED_OBJECT))
    {
        let expanded = crate::atspi_tree::read_subtree(
            object_ref,
            FIELD_SUBTREE_MAX_NODES,
            FIELD_SUBTREE_MAX_DEPTH,
        )
        .await
        .ok()
        .and_then(|nodes| crate::fields::expanded_text(&nodes));
        if expanded.is_some() {
            return Some((fresh, expanded));
        }
    }
    Some((fresh, text))
}

/// Rereads a field's text (`read_field`) until `done` holds or `limit`
/// passes. `None` when the field cannot be read at all; otherwise whether
/// `done` held, and the last text read.
async fn poll_field_text(
    object_ref: &str,
    limit: Duration,
    done: impl Fn(Option<&str>) -> bool,
) -> Option<(bool, Option<String>)> {
    let started = std::time::Instant::now();
    let mut last = None;
    loop {
        if let Some((_, text)) = read_field(object_ref).await {
            if done(text.as_deref()) {
                return Some((true, text));
            }
            last = Some(text);
        }
        if started.elapsed() >= limit {
            return last.map(|text| (false, text));
        }
        sleep(Duration::from_millis(80)).await;
    }
}

/// One left click at a screen point.
async fn click_point((x, y): (i32, i32)) -> Result<(), String> {
    let args = xdotool_pointer_click_args(x, y, 1, None)
        .ok_or_else(|| "left button is always supported".to_string())?;
    run_xdotool(&args, INPUT_COMMAND_TIMEOUT).await.map(|_| ())
}

/// Presses one xdotool key chord such as `ctrl+a`.
async fn send_key_chord(chord: &str) -> Result<(), String> {
    let args = vec![
        "key".to_string(),
        "--clearmodifiers".to_string(),
        chord.to_string(),
    ];
    run_xdotool(&args, INPUT_COMMAND_TIMEOUT).await.map(|_| ())
}

/// Types text the way type_text promises, newlines as the Return key:
/// printable ASCII with `xdotool type`; anything else is pasted from an X11
/// CLIPBOARD this process owns, with `paste_chord` (Ctrl+V, or Ctrl+Shift+V
/// in terminals), since XTEST drops non-ASCII text on Xvfb. Pasted text
/// goes line by line with Return in between, as typing does: a newline
/// inside a paste is inert in shells with bracketed paste. set_value
/// enters values literally instead (`insert_text`).
async fn enter_text(text: &str, paste_chord: &str) -> Result<(), String> {
    if crate::fields::typeable_ascii(text) {
        let delay_ms = xdotool_type_delay_ms();
        let args = xdotool_type_args_with_delay(text, delay_ms);
        return run_xdotool(&args, xdotool_type_timeout(text, delay_ms))
            .await
            .map(|_| ());
    }
    let text = text.replace("\r\n", "\n");
    let lines: Vec<&str> = text.split('\n').collect();
    for (index, line) in lines.iter().enumerate() {
        let more = index + 1 < lines.len();
        if !line.is_empty() {
            paste_text(line, paste_chord, more).await?;
        }
        if more {
            send_key_chord("Return").await?;
        }
    }
    Ok(())
}

/// Enters `value` into the focused field as literal text, as set_value and
/// fill_form promise: printable ASCII on one line is typed, and anything
/// else, line breaks included, goes in as one clipboard paste, so no Return
/// key is pressed (Enter sends a message in chat editors). Chromium text
/// areas keep the pasted newlines; contenteditable editors turn them into
/// line breaks.
async fn insert_text(value: &str) -> Result<(), String> {
    match literal_insertion(value) {
        Insertion::Type => enter_text(value, "ctrl+v").await,
        Insertion::Paste(text) => paste_text(&text, "ctrl+v", false).await,
    }
}

/// How `insert_text` enters a value.
#[derive(Debug, PartialEq, Eq)]
enum Insertion {
    /// `xdotool type`: one line of printable ASCII, so no key but characters.
    Type,
    /// One clipboard paste of this text, CRLF folded to LF.
    Paste(String),
}

fn literal_insertion(value: &str) -> Insertion {
    if crate::fields::typeable_ascii(value) && !value.contains(['\n', '\r']) {
        Insertion::Type
    } else {
        Insertion::Paste(value.replace("\r\n", "\n").replace('\r', "\n"))
    }
}

/// How long the clipboard stays owned after a paste was first served.
const CLIPBOARD_LINGER: Duration = Duration::from_secs(4);
/// How long a paste followed by more text waits for the app's second read
/// of the clipboard (see `paste_text`).
const PASTE_SECOND_READ_WAIT: Duration = Duration::from_millis(250);

/// Pastes one piece of text through a clipboard this process owns.
/// Chromium reads a new clipboard owner once on its own and again when the
/// paste lands, so with `more` text to follow (whose new owner would answer
/// a late read) it waits up to `PASTE_SECOND_READ_WAIT` for that second
/// read; apps that read once just cost the wait. No read at all means the
/// paste did not land; a read does not mean it did (Chromium and clipboard
/// managers read new owners on their own), so callers judge delivery by
/// reading the field back.
async fn paste_text(text: &str, paste_chord: &str, more: bool) -> Result<(), String> {
    let owned = text.to_string();
    let guard = tokio::task::spawn_blocking(move || {
        crate::x11_clipboard::hold_clipboard(&owned, Duration::from_secs(10))
    })
    .await
    .map_err(|error| format!("clipboard task failed: {error}"))?
    .map_err(|error| format!("could not own the X11 clipboard: {error:#}"))?;
    let sent = send_key_chord(paste_chord).await;
    let served = tokio::task::spawn_blocking(move || {
        let served = guard.wait_served(Duration::from_secs(2));
        if served && more {
            // The first read may be Chromium's own, before the paste: wait
            // for the paste's read so the next owner cannot answer it.
            guard.wait_reads(2, PASTE_SECOND_READ_WAIT);
        }
        // Chromium reads the selection once when ownership changes and again
        // when the paste lands; an owner that is gone by then yields an empty
        // paste, and one that vanishes mid-request stalls the renderer. Keep
        // serving a while; the next paste's owner takes over cleanly.
        std::thread::spawn(move || {
            std::thread::sleep(CLIPBOARD_LINGER);
            drop(guard);
        });
        served
    })
    .await
    .unwrap_or(false);
    sent?;
    if served {
        Ok(())
    } else {
        Err(format!(
            "pasted with {paste_chord}, but the focused app never read the clipboard; the text was not entered"
        ))
    }
}

/// The paste chord for the focused window: terminals take Ctrl+Shift+V
/// (xterm only Shift+Insert, which reads PRIMARY, also owned here).
fn paste_chord_for(window: Option<&WindowInfo>) -> &'static str {
    let class = window
        .map(|window| {
            format!(
                "{} {}",
                window.wm_class.as_deref().unwrap_or_default(),
                window.app_id.as_deref().unwrap_or_default()
            )
            .to_ascii_lowercase()
        })
        .unwrap_or_default();
    if class
        .split_whitespace()
        .any(|word| word == "xterm" || word == "uxterm")
    {
        "shift+Insert"
    } else if [
        "alacritty",
        "terminal",
        "kitty",
        "konsole",
        "wezterm",
        "foot",
        "terminator",
        "tilix",
        "urxvt",
        "st-256color",
    ]
    .iter()
    .any(|name| class.contains(name))
    {
        "ctrl+shift+v"
    } else {
        "ctrl+v"
    }
}

/// How a click on an off-page element went.
enum OffScreenClick {
    /// Its AT-SPI action ran; the message says which.
    Invoked(String),
    /// ScrollTo brought it on screen at this point.
    ScrolledTo(i32, i32),
}

/// Actions that stand in for a left click on an element that is not on the
/// visible page, in order of preference.
const OFF_SCREEN_CLICK_ACTIONS: &[&str] = &[
    "press",
    "click",
    "activate",
    "select",
    "jump",
    "toggle",
    "check",
    "uncheck",
    "open",
    "dodefault",
];

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

/// Picks the one cached node a role/name/text selector means. The matches
/// are narrowed in turn to nodes fit for the purpose, (for fields) nodes
/// that are not plain text, actionable nodes (a specific action, editable,
/// or a value), exact name matches, and showing nodes. The first narrowing
/// that leaves exactly one node picks it; one that leaves none is skipped.
/// Several left at the end is an error listing them as `[id] role "name"`.
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

    let wanted_name = selector
        .name
        .or(selector.text)
        .map(str::trim)
        .filter(|name| !name.is_empty());
    let narrowings: [&dyn Fn(&AccessibilityNode) -> bool; 5] = [
        &|node| node_matches_resolve_purpose(node, purpose),
        // A radio group's legend repeats its name as plain text; the group
        // is the field.
        &|node| {
            !matches!(purpose, ElementResolvePurpose::SetValue)
                || !TEXT_ONLY_ROLES.contains(&node.role.as_str())
        },
        &node_is_actionable,
        &|node| wanted_name.is_some_and(|name| node_has_exact_name(node, name)),
        &node_is_showing,
    ];
    for keep in narrowings {
        if matches.len() == 1 {
            break;
        }
        let kept = matches
            .iter()
            .copied()
            .filter(|node| keep(node))
            .collect::<Vec<_>>();
        if !kept.is_empty() {
            matches = kept;
        }
    }

    if matches.len() == 1 {
        return Ok(matches[0].clone());
    }

    Err(format!(
        "Semantic selector {} matched {} elements; pass element_index of the one you mean:\n{}",
        describe_selector(selector),
        matches.len(),
        describe_matching_nodes(&matches),
    ))
}

/// Whether the node does something beyond showing text: a specific action,
/// an editable field (Chromium web fields carry only the state), or a value.
fn node_is_actionable(node: &AccessibilityNode) -> bool {
    is_actionable_accessibility_node(node) || node.states.iter().any(|state| state == "editable")
}

/// Whether the node's name, or its text when it has no name, is exactly
/// `name` up to case and whitespace.
fn node_has_exact_name(node: &AccessibilityNode, name: &str) -> bool {
    node.name
        .as_deref()
        .or_else(|| node.text.as_ref().and_then(|text| text.content.as_deref()))
        .is_some_and(|actual| normalized_equals(actual, name))
}

/// Roles that only carry text, never a value to set.
const TEXT_ONLY_ROLES: [&str; 6] = ["static", "label", "heading", "paragraph", "caption", "text"];

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
        ElementResolvePurpose::SetValue => {
            node.supports_editable_text
                || node.value.is_some()
                || crate::fields::field_kind(node) != crate::fields::FieldKind::Other
        }
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

/// Up to five candidates as `[id] role "name"` lines, then how many more.
fn describe_matching_nodes(nodes: &[&AccessibilityNode]) -> String {
    const SHOWN: usize = 5;
    let mut lines = nodes
        .iter()
        .take(SHOWN)
        .map(|node| crate::outline::element_label(node))
        .collect::<Vec<_>>();
    if nodes.len() > SHOWN {
        lines.push(format!("(and {} more)", nodes.len() - SHOWN));
    }
    lines.join("\n")
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

/// The condition `wait_for` checks: the requested one, else `changed` when
/// no query names an element (the agent does not know what will appear),
/// else `appears`.
fn wait_condition(requested: Option<WaitCondition>, query_empty: bool) -> WaitCondition {
    match requested {
        Some(condition) => condition,
        None if query_empty => WaitCondition::Changed,
        None => WaitCondition::Appears,
    }
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

    // Snapshots arrive breadth-first; emit in document (depth-first) order so
    // a kept node re-parented past dropped wrappers still lands between its
    // neighbours instead of after every shallower sibling.
    for old_index in depth_first_order(&nodes) {
        let node = &nodes[old_index];
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

/// Positions of a positional snapshot in depth-first pre-order, children in
/// snapshot order. Nodes whose parent is missing start their own subtree.
fn depth_first_order(nodes: &[AccessibilityNode]) -> Vec<usize> {
    let mut children = vec![Vec::new(); nodes.len()];
    let mut roots = Vec::new();
    for (at, node) in nodes.iter().enumerate() {
        match node
            .parent_index
            .map(|parent| parent as usize)
            .filter(|&parent| parent < nodes.len() && parent != at)
        {
            Some(parent) => children[parent].push(at),
            None => roots.push(at),
        }
    }
    let mut order = Vec::with_capacity(nodes.len());
    let mut visited = vec![false; nodes.len()];
    let mut stack: Vec<usize> = roots.into_iter().rev().collect();
    while let Some(at) = stack.pop() {
        if std::mem::replace(&mut visited[at], true) {
            continue;
        }
        order.push(at);
        stack.extend(children[at].iter().rev());
    }
    // A parent cycle leaves nodes unreachable from any root; keep them.
    order.extend((0..nodes.len()).filter(|&at| !visited[at]));
    order
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

/// Why pointer input at `point` would miss `label`, whose app is
/// `owner_pid`, given the X11 window there; `None` when that window is the
/// app's.
fn window_in_the_way(
    label: &str,
    (x, y): (i32, i32),
    owner_pid: Option<u32>,
    under: Option<&crate::x11_display::WindowAtPoint>,
) -> Option<String> {
    let Some(under) = under else {
        return Some(format!(
            "no window is under {x},{y}, where {label} should be; call get_app_state and retry"
        ));
    };
    let Some(owner_pid) = owner_pid else {
        return Some(format!(
            "could not tell which app owns {label}, so {under} at {x},{y} cannot be confirmed to be its window"
        ));
    };
    match under.pid {
        Some(pid) if pid == owner_pid => None,
        Some(_) => Some(format!(
            "{under} is in the way of {label} (pid {owner_pid}) at {x},{y}; bring the element's window to the front with activate_window and retry"
        )),
        None => Some(format!(
            "{under} is at {x},{y} and does not name its process, so it cannot be confirmed to be the window of {label} (pid {owner_pid})"
        )),
    }
}

/// What type_text's read-back of its target element says about delivery.
#[derive(Debug, PartialEq, Eq)]
enum Delivery {
    /// The element's text changed.
    Arrived,
    /// The element's text is what it was before, though characters were
    /// sent: they went somewhere else.
    Unchanged,
    /// The read-back cannot tell: the element has no text, or only Enter
    /// was sent, or a trailing Enter may have submitted and cleared it.
    Unverified,
}

fn typed_delivery(before: Option<&str>, after: Option<&str>, text: &str) -> Delivery {
    let Some(after) = after else {
        return Delivery::Unverified;
    };
    let characters = text.trim_end_matches(['\n', '\r']);
    if characters.is_empty() {
        return Delivery::Unverified;
    }
    let unchanged = crate::fields::shown_text(before) == crate::fields::shown_text(Some(after));
    match (unchanged, characters.len() < text.len()) {
        (false, _) => Delivery::Arrived,
        (true, true) => Delivery::Unverified,
        (true, false) => Delivery::Unchanged,
    }
}

/// A type_text refusal for a window target and an element of another app.
fn conflicting_targets_message(mismatch: &str) -> String {
    format!(
        "Nothing was typed: the window target and the element are in different apps. {mismatch} Or drop the window target and type by element only."
    )
}

/// Why typing must stop after focusing `label`: focus left the requested
/// window (or, for an app-level target, its app). `None` while it is there.
fn focus_drift(
    requested: &WindowInfo,
    exact: bool,
    focused: Option<&WindowInfo>,
    label: &str,
) -> Option<String> {
    let stayed = focused.is_some_and(|focused| {
        if exact {
            focused.window_id == requested.window_id
        } else {
            focused.window_id == requested.window_id
                || (requested.pid.is_some() && focused.pid == requested.pid)
        }
    });
    if stayed {
        return None;
    }
    let now = focused.map_or_else(
        || "no window".to_string(),
        |focused| {
            format!(
                "window_id {} (pid {})",
                focused.window_id,
                focused
                    .pid
                    .map_or_else(|| "unknown".to_string(), |pid| pid.to_string())
            )
        },
    );
    Some(format!(
        "Nothing was typed: focusing {label} moved focus to {now}, away from the target window_id {}.",
        requested.window_id
    ))
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
    start: (i32, i32),
    end: (i32, i32),
) -> std::result::Result<Vec<Output>, String> {
    let args = xdotool_drag_args(start, end);
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

/// Moves on the way to the drop point, after the opening nudges.
const DRAG_STEPS: i32 = 8;

/// A drag the way a hand makes one, in one xdotool command (about 1.1 s).
/// Chrome starts an HTML5 drag only after the pointer moves past a threshold
/// while held and then keeps moving: a press, one jump and a release starts
/// nothing (measured on Xvfb). So: press, two small nudges 0.1 s apart,
/// `DRAG_STEPS` moves toward the target 0.05 s apart, then a pause on the
/// target before the release so dragover handlers run.
fn xdotool_drag_args(start: (i32, i32), end: (i32, i32)) -> Vec<String> {
    fn move_to(args: &mut Vec<String>, (x, y): (i32, i32)) {
        args.extend(["mousemove", "--"].map(str::to_string));
        args.extend([x.to_string(), y.to_string()]);
    }
    fn pause(args: &mut Vec<String>, seconds: &str) {
        args.extend(["sleep", seconds].map(str::to_string));
    }
    let mut args = Vec::new();
    move_to(&mut args, start);
    args.extend(["mousedown", "1"].map(str::to_string));
    pause(&mut args, "0.1");
    let nudged = (start.0 + 30, start.1 + 15);
    for point in [(start.0 + 10, start.1 + 5), nudged] {
        move_to(&mut args, point);
        pause(&mut args, "0.1");
    }
    for index in 1..=DRAG_STEPS {
        let toward = |from: i32, to: i32| from + (to - from) * index / (DRAG_STEPS + 1);
        move_to(
            &mut args,
            (toward(nudged.0, end.0), toward(nudged.1, end.1)),
        );
        pause(&mut args, "0.05");
    }
    pause(&mut args, "0.2");
    move_to(&mut args, end);
    pause(&mut args, "0.2");
    args.extend(["mouseup", "1"].map(str::to_string));
    args
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
            format!(
                "xdotool mousemove -- 5536 700 click --repeat 1 1\n\
                 xdotool mousemove -- 5536 700 click --delay 12 --repeat 5 5\n\
                 xdotool {}\n\
                 xdotool click --delay 12 --repeat 5 5\n",
                xdotool_drag_args((100, 100), (200, 200)).join(" ")
            )
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
            start_x: Some(100),
            start_y: Some(100),
            end_x: Some(200),
            end_y: Some(200),
            ..Default::default()
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
            notes: Vec::new(),
            app_name_or_bundle_identifier: None,
            window_context: None,
            window_error: None,
            window_permissions_hint: None,
            backend: "linux-atspi".to_string(),
            screenshot: screenshot.map(ScreenshotSummary::from),
            screenshot_error: None,
            accessibility_tree: Vec::new(),
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
    fn app_state_text_is_a_header_and_the_outline_with_notes_only_when_needed() {
        let mut output = sample_app_state_output(None);
        let view = crate::page_view::PageView {
            title: Some("Criar conta - Chromium".to_string()),
            url: Some("127.0.0.1/apps/signup.html".to_string()),
            browser: true,
            ..Default::default()
        };
        assert_eq!(
            app_state_text(&output, &view, Some(42), None),
            "window \"Criar conta - Chromium\" window_id=42 url=127.0.0.1/apps/signup.html elements=0\n--- page ---\n"
        );
        output.notes = vec!["blocker: xdotool is not installed".to_string()];
        assert!(app_state_text(&output, &view, None, None)
            .ends_with("elements=0\n--- page ---\nblocker: xdotool is not installed\n"));
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
    fn drag_presses_nudges_steps_and_pauses_before_release() {
        let args = xdotool_drag_args((100, 100), (400, 205)).join(" ");
        assert_eq!(
            args,
            "mousemove -- 100 100 mousedown 1 sleep 0.1 \
             mousemove -- 110 105 sleep 0.1 mousemove -- 130 115 sleep 0.1 \
             mousemove -- 160 125 sleep 0.05 mousemove -- 190 135 sleep 0.05 \
             mousemove -- 220 145 sleep 0.05 mousemove -- 250 155 sleep 0.05 \
             mousemove -- 280 165 sleep 0.05 mousemove -- 310 175 sleep 0.05 \
             mousemove -- 340 185 sleep 0.05 mousemove -- 370 195 sleep 0.05 \
             sleep 0.2 mousemove -- 400 205 sleep 0.2 mouseup 1"
        );
    }

    #[tokio::test]
    async fn drag_needs_both_ends() {
        let backend = ComputerUseLinux::default();
        let result = backend
            .drag(Parameters(DragParams {
                start_x: Some(1),
                start_y: Some(1),
                ..Default::default()
            }))
            .await
            .0;
        assert!(!result.ok);
        assert!(
            result
                .message
                .contains("to_element_index or as end_x and end_y"),
            "{}",
            result.message
        );
    }

    #[tokio::test]
    async fn drag_rereads_the_element_instead_of_trusting_cached_bounds() {
        let backend = ComputerUseLinux::default();
        let mut frame = node(
            1,
            Some(Bounds {
                x: 0,
                y: 0,
                width: 800,
                height: 600,
            }),
        );
        frame.role = "frame".to_string();
        let mut card = node(
            2,
            Some(Bounds {
                x: 10,
                y: 10,
                width: 50,
                height: 20,
            }),
        );
        card.parent_index = Some(1);
        card.object_ref = ":1.9999/org/a11y/atspi/accessible/2".to_string();
        backend.cache_nodes(&[frame, card]);
        let result = backend
            .drag(Parameters(DragParams {
                from_element_index: Some(2),
                end_x: Some(5),
                end_y: Some(5),
                ..Default::default()
            }))
            .await
            .0;
        assert!(!result.ok);
        assert!(
            result.message.contains("could not be reread"),
            "{}",
            result.message
        );
    }

    #[test]
    fn input_is_refused_when_another_apps_window_is_at_the_point() {
        use crate::x11_display::WindowAtPoint;
        let label = "[4] list \"Done\"";
        let chrome = WindowAtPoint {
            window: 0x400001,
            pid: Some(100),
            title: Some("Kanban - Chromium".to_string()),
        };
        assert_eq!(
            window_in_the_way(label, (5, 5), Some(100), Some(&chrome)),
            None
        );
        let terminal = WindowAtPoint {
            window: 0x600001,
            pid: Some(200),
            title: Some("xterm".to_string()),
        };
        let covered = window_in_the_way(label, (5, 5), Some(100), Some(&terminal)).unwrap();
        assert!(
            covered.contains("window 0x600001 \"xterm\" (pid 200) is in the way"),
            "{covered}"
        );
        let anonymous = WindowAtPoint {
            pid: None,
            ..terminal
        };
        assert!(window_in_the_way(label, (5, 5), Some(100), Some(&anonymous)).is_some());
        assert!(window_in_the_way(label, (5, 5), None, Some(&chrome)).is_some());
        assert!(window_in_the_way(label, (5, 5), Some(100), None).is_some());
    }

    #[tokio::test]
    async fn drag_refuses_an_element_outside_the_visible_page() {
        let backend = ComputerUseLinux::default();
        let mut frame = node(
            1,
            Some(Bounds {
                x: 0,
                y: 0,
                width: 800,
                height: 600,
            }),
        );
        frame.role = "frame".to_string();
        let mut below = node(
            2,
            Some(Bounds {
                x: 10,
                y: 900,
                width: 50,
                height: 20,
            }),
        );
        below.parent_index = Some(1);
        backend.cache_nodes(&[frame, below]);
        let result = backend
            .drag(Parameters(DragParams {
                from_element_index: Some(2),
                end_x: Some(5),
                end_y: Some(5),
                ..Default::default()
            }))
            .await
            .0;
        assert!(!result.ok);
        assert!(
            result.message.contains("scroll(element_index=2)"),
            "{}",
            result.message
        );
    }

    #[test]
    fn scroll_until_counts_only_matches_inside_the_visible_page() {
        let at = |index: u32, y: i32| {
            let mut row = node(
                index,
                Some(Bounds {
                    x: 10,
                    y,
                    width: 200,
                    height: 20,
                }),
            );
            row.role = "list item".to_string();
            row.name = Some(format!("Row {index}"));
            row.parent_index = Some(1);
            row
        };
        let mut frame = node(
            1,
            Some(Bounds {
                x: 0,
                y: 0,
                width: 800,
                height: 600,
            }),
        );
        frame.role = "frame".to_string();
        let nodes = [frame, at(2, 100), at(3, 900), at(4, 590)];
        let query = |name: &str| ElementQuery {
            name: Some(name.to_string()),
            ..Default::default()
        };
        let ids = |name: &str| {
            visible_matches(&nodes, &query(name))
                .iter()
                .map(|node| node.index)
                .collect::<Vec<_>>()
        };
        assert_eq!(ids("Row 2"), [2]);
        // Below the frame, and with its centre (y=600) just past the edge.
        assert!(ids("Row 3").is_empty());
        assert!(ids("Row 4").is_empty());
        assert_eq!(ids("Row"), [2]);
    }

    #[test]
    fn scroll_until_stops_on_a_match_the_end_or_the_cap() {
        assert_eq!(until_step(true, true, 0, 1), UntilStep::Found);
        // A match counts even on the step that hits the cap or the end.
        assert_eq!(
            until_step(true, false, 1, SCROLL_UNTIL_MAX_STEPS),
            UntilStep::Found
        );
        assert_eq!(until_step(false, true, 0, 5), UntilStep::Continue);
        assert_eq!(until_step(false, false, 0, 5), UntilStep::Recheck);
        assert_eq!(until_step(false, false, 1, 6), UntilStep::End);
        assert_eq!(
            until_step(false, true, 0, SCROLL_UNTIL_MAX_STEPS),
            UntilStep::Cap
        );
        assert_eq!(
            until_step(false, true, 0, SCROLL_UNTIL_MAX_STEPS - 1),
            UntilStep::Continue
        );
    }

    #[test]
    fn scroll_until_searches_up_only_after_an_unrequested_search_down_ends() {
        use ScrollDirection::{Down, Up};
        assert_eq!(next_until_pass(Down, false, UntilStep::End), Some(Up));
        assert_eq!(next_until_pass(Down, true, UntilStep::End), None);
        assert_eq!(next_until_pass(Down, false, UntilStep::Cap), None);
        assert_eq!(next_until_pass(Down, false, UntilStep::Found), None);
        assert_eq!(next_until_pass(Up, false, UntilStep::End), None);
    }

    #[test]
    fn scroll_until_steps_stay_within_most_of_the_visible_extent() {
        let size = |extent, click_px, max_clicks| UntilStepSize {
            extent,
            click_px,
            measured: true,
            max_clicks,
        };
        // 600 px visible: 90 % is 540 px, ten 53 px clicks, unless `pages`
        // asked for fewer.
        assert_eq!(size(Some(600), 53.0, None).clicks(), 10);
        assert_eq!(size(Some(600), 53.0, Some(5)).clicks(), 5);
        // Measured 120 px clicks: four clicks still stay under 90 %.
        assert_eq!(size(Some(600), 120.0, None).clicks(), 4);
        // A 100 px list box: one click; nothing known: the default.
        assert_eq!(size(Some(100), 53.0, None).clicks(), 1);
        assert_eq!(size(Some(40), 53.0, Some(5)).clicks(), 1);
        assert_eq!(size(None, 53.0, Some(7)).clicks(), 7);
        assert_eq!(size(None, 53.0, None).clicks(), SCROLL_UNTIL_DEFAULT_CLICKS);
        assert_eq!(size(Some(0), 53.0, Some(0)).clicks(), 1);
        // Before a step measured a click, 60 % on the 53 px guess.
        let guess = UntilStepSize {
            measured: false,
            ..size(Some(600), 53.0, None)
        };
        assert_eq!(guess.clicks(), 6);
        let area = Bounds {
            x: 0,
            y: 100,
            width: 800,
            height: 500,
        };
        let row = |y| Bounds {
            x: 10,
            y,
            width: 600,
            height: 40,
        };
        assert_eq!(
            direction_toward(&row(900), &area),
            Some(ScrollDirection::Down)
        );
        assert_eq!(direction_toward(&row(20), &area), Some(ScrollDirection::Up));
        assert_eq!(direction_toward(&row(300), &area), None);
    }

    #[test]
    fn scroll_until_measures_how_far_a_wheel_click_moves_rows() {
        let row = |label: &str, y: i32| {
            let mut row = node(
                0,
                Some(Bounds {
                    x: 0,
                    y,
                    width: 900,
                    height: 40,
                }),
            );
            row.role = "list item".to_string();
            row.name = Some(label.to_string());
            row
        };
        // Ten clicks moved the rows 1,200 px up: 120 px a click.
        let before = [
            row("#1010", 1_000),
            row("#1011", 1_040),
            row("#1012", 1_080),
        ];
        let after = [row("#1011", -160), row("#1012", -120), row("#1040", 1_000)];
        assert_eq!(measured_click_px(&before, &after, 10, false), Some(120.0));
        // A label that repeats in a read is not matched; a sticky header
        // that stayed put does not count.
        let mut header = row("Pedidos", 100);
        header.role = "heading".to_string();
        let before_dup = [row("#1011", 1_040), row("#1011", 1_080), header.clone()];
        let after_dup = [row("#1011", -160), header];
        assert_eq!(measured_click_px(&before_dup, &after_dup, 10, false), None);
        // No row in common, or none moved: nothing measured.
        assert_eq!(
            measured_click_px(&before, &[row("#1100", 0)], 10, false),
            None
        );
        assert_eq!(measured_click_px(&before, &before, 10, false), None);
    }

    #[test]
    fn scroll_until_prints_the_match_with_a_few_neighbours() {
        let element = |index: u32, parent: Option<u32>, role: &str, name: &str| {
            let mut element = node(index, None);
            element.parent_index = parent;
            element.role = role.to_string();
            element.name = Some(name.to_string());
            element
        };
        let mut nodes = vec![element(1, None, "list", "Pedidos")];
        for (row, number) in [(2, 1376), (4, 1377), (6, 1378)] {
            nodes.push(element(row, Some(1), "list item", &format!("#{number}")));
            nodes.push(element(
                row + 1,
                Some(row),
                "push button",
                &format!("Arquivar pedido #{number}"),
            ));
        }
        nodes.push(element(8, Some(1), "list item", "#1379"));
        let view = crate::page_view::page_view(&nodes, Default::default());
        let printed = until_match_lines(&view, &nodes, 5);
        let lines: Vec<&str> = printed.lines().collect();
        // The row before, the match, then two lines after it: no outline.
        assert_eq!(lines.len(), 1 + SCROLL_UNTIL_NEIGHBOURS);
        assert!(
            lines[0].starts_with("[4] ") && lines[0].contains("#1377"),
            "{printed}"
        );
        assert!(lines[1].starts_with("  [5] ") && lines[1].contains("Arquivar pedido #1377"));
        assert!(lines[3].contains("Arquivar pedido #1378"), "{printed}");
        assert!(
            !printed.contains("Pedidos") && !printed.contains("#1379"),
            "{printed}"
        );
        // Near the end of the view there are fewer neighbours.
        assert_eq!(until_match_lines(&view, &nodes, 8).lines().count(), 2);
        // A match the view does not show still prints its own line.
        assert!(until_match_lines(&Default::default(), &nodes, 5).contains("#1377"));
    }

    #[test]
    fn scroll_until_counts_rendered_rows_outside_the_viewport_as_found() {
        let row = |index: u32, y: i32, height: i32| {
            let mut row = node(
                index,
                Some(Bounds {
                    x: 10,
                    y,
                    width: 200,
                    height,
                }),
            );
            row.role = "button".to_string();
            row.name = Some(format!("Arquivar pedido #{index}"));
            row
        };
        let nodes = [row(1377, 1_200, 28), row(1378, 1_240, 0)];
        let query = ElementQuery {
            name: Some("#1377".to_string()),
            ..Default::default()
        };
        let found: Vec<u32> = rendered_matches(&nodes, &query)
            .iter()
            .map(|node| node.index)
            .collect();
        assert_eq!(found, [1377]);
        // A zero-size match (collapsed, hidden) does not count.
        let hidden = ElementQuery {
            name: Some("#1378".to_string()),
            ..Default::default()
        };
        assert!(rendered_matches(&nodes, &hidden).is_empty());
    }

    #[test]
    fn settle_waits_out_strong_busy_signs_and_quiet_disabled_buttons() {
        let busy = |strong| crate::page_view::Busy {
            reason: String::new(),
            strong,
        };
        assert!(!still_busy_after(None, Duration::ZERO));
        assert!(still_busy_after(Some(&busy(true)), Duration::from_secs(5)));
        assert!(still_busy_after(
            Some(&busy(false)),
            Duration::from_millis(400)
        ));
        assert!(!still_busy_after(Some(&busy(false)), BUSY_QUIET));
    }

    #[test]
    fn scroll_defaults_to_down_and_takes_an_until_query() {
        let params: ScrollParams =
            serde_json::from_value(serde_json::json!({"until": {"name": "Item 42"}})).unwrap();
        assert_eq!(params.direction, None);
        assert_eq!(params.direction(), "down");
        assert_eq!(params.until.unwrap().name.as_deref(), Some("Item 42"));
    }

    #[tokio::test]
    async fn scroll_until_needs_a_query() {
        let backend = ComputerUseLinux::default();
        let params: ScrollParams =
            serde_json::from_value(serde_json::json!({"x": 5, "y": 5, "until": {}})).unwrap();
        let result = backend.scroll(Parameters(params)).await.0;
        assert!(!result.ok);
        assert!(result.message.contains("until needs"), "{}", result.message);
    }

    #[test]
    fn drop_paths_must_be_absolute_existing_files() {
        let dir = std::env::temp_dir().join(format!("cul-drop-paths-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("Comprovante ação.pdf");
        std::fs::write(&file, b"%PDF").unwrap();
        let file_str = file.to_string_lossy().to_string();
        assert_eq!(
            validate_drop_paths(std::slice::from_ref(&file_str)).unwrap(),
            std::slice::from_ref(&file)
        );
        let relative = validate_drop_paths(&["Comprovante.pdf".to_string()]).unwrap_err();
        assert!(relative.contains("not an absolute path"), "{relative}");
        let directory = validate_drop_paths(&[dir.to_string_lossy().to_string()]).unwrap_err();
        assert!(directory.contains("not a regular file"), "{directory}");
        let missing = dir.join("missing.pdf").to_string_lossy().to_string();
        assert!(validate_drop_paths(&[file_str, missing]).is_err());
        assert!(validate_drop_paths(&[]).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn drop_files_needs_a_target_before_touching_the_display() {
        let file = std::env::temp_dir().join(format!("cul-drop-target-{}", std::process::id()));
        std::fs::write(&file, b"x").unwrap();
        let backend = ComputerUseLinux::default();
        let result = backend
            .drop_files(Parameters(DropFilesParams {
                paths: vec![file.to_string_lossy().to_string()],
                ..Default::default()
            }))
            .await
            .0;
        let _ = std::fs::remove_file(&file);
        assert!(!result.ok);
        assert!(
            result.message.contains("where to drop"),
            "{}",
            result.message
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
            object_ref: String::new(),
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
    fn wait_for_without_a_query_waits_for_any_change() {
        let params: WaitForParams = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(params.condition, None);
        assert_eq!(
            wait_condition(params.condition, params.query.is_empty()),
            WaitCondition::Changed
        );
        let named: WaitForParams =
            serde_json::from_value(serde_json::json!({"name": "Alterações salvas"})).unwrap();
        assert_eq!(
            wait_condition(named.condition, named.query.is_empty()),
            WaitCondition::Appears
        );
        let explicit: WaitForParams =
            serde_json::from_value(serde_json::json!({"condition": "disappears"})).unwrap();
        assert_eq!(
            wait_condition(explicit.condition, explicit.query.is_empty()),
            WaitCondition::Disappears
        );
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

        assert!(error.contains("matched 2 elements"), "{error}");
        assert!(error.contains("\n[7] push button \"Close\""), "{error}");
        assert!(error.contains("\n[9] push button \"Close\""), "{error}");
    }

    fn named(index: u32, role: &str, name: &str) -> AccessibilityNode {
        let mut node = node(index, None);
        node.role = role.to_string();
        node.name = Some(name.to_string());
        node
    }

    fn pick(nodes: &[AccessibilityNode], name: &str, purpose: ElementResolvePurpose) -> u32 {
        let selector = ElementSelector {
            name: Some(name),
            ..Default::default()
        };
        resolve_semantic_node(nodes, &selector, purpose)
            .map(|node| node.index)
            .unwrap_or_else(|error| panic!("{error}"))
    }

    #[test]
    fn a_label_and_its_file_button_resolve_to_the_button() {
        // Chrome: <label>Comprovante</label><input type=file>, both with
        // bounds and Chromium's generic actions; only the button can press.
        let bounds = Some(Bounds {
            x: 10,
            y: 10,
            width: 80,
            height: 20,
        });
        let generic = AccessibilityAction {
            name: "doDefault".to_string(),
            ..click_action()
        };
        let mut label = named(3, "static", "Comprovante");
        label.bounds = bounds.clone();
        label.actions = vec![generic.clone()];
        let mut button = named(4, "push button", "Comprovante: No file chosen");
        button.bounds = bounds;
        button.actions = vec![
            generic,
            AccessibilityAction {
                name: "press".to_string(),
                ..click_action()
            },
        ];
        let nodes = [label, button];
        assert_eq!(pick(&nodes, "Comprovante", ElementResolvePurpose::Click), 4);
        assert_eq!(
            pick(&nodes, "Comprovante", ElementResolvePurpose::Action),
            4
        );
    }

    #[test]
    fn among_actionable_matches_the_exact_name_wins() {
        let mut save = named(5, "push button", "Save");
        save.actions = vec![click_action()];
        let mut save_all = named(6, "push button", "Save all");
        save_all.actions = vec![click_action()];
        assert_eq!(
            pick(&[save_all, save], "save", ElementResolvePurpose::Click),
            5
        );
    }

    #[test]
    fn among_same_named_matches_the_showing_one_wins() {
        let mut hidden = named(5, "push button", "Next");
        hidden.actions = vec![click_action()];
        let mut shown = hidden.clone();
        shown.index = 6;
        shown.states = vec!["showing".to_string(), "visible".to_string()];
        assert_eq!(
            pick(&[hidden, shown], "next", ElementResolvePurpose::Click),
            6
        );
    }

    #[test]
    fn an_ambiguous_selector_lists_five_candidates_and_the_rest_count() {
        let nodes = (1..=7)
            .map(|index| {
                let mut button = named(index, "push button", "Delete");
                button.actions = vec![click_action()];
                button
            })
            .collect::<Vec<_>>();
        let selector = ElementSelector {
            name: Some("delete"),
            ..Default::default()
        };
        let error =
            resolve_semantic_node(&nodes, &selector, ElementResolvePurpose::Click).unwrap_err();
        assert!(error.contains("matched 7 elements"), "{error}");
        assert!(error.contains("[5] push button \"Delete\""), "{error}");
        assert!(!error.contains("[6]"), "{error}");
        assert!(error.ends_with("(and 2 more)"), "{error}");
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
            object_ref: String::new(),
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
            object_ref: String::new(),
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
            object_ref: String::new(),
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
            object_ref: String::new(),
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
            direction: Some("down".to_string()),
            pages: None,
            window_id: Some(1),
            pid: None,
            app_id: None,
            wm_class: None,
            window_title: None,
            relative: Some(true),
            observe: None,
            until: None,
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
            direction: Some("down".to_string()),
            pages: None,
            window_id: Some(1),
            pid: None,
            app_id: None,
            wm_class: None,
            window_title: None,
            relative: None,
            observe: None,
            until: None,
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
            direction: Some("down".to_string()),
            pages: None,
            window_id: Some(1),
            pid: None,
            app_id: None,
            wm_class: None,
            window_title: None,
            relative: None,
            observe: None,
            until: None,
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
            direction: Some("down".to_string()),
            pages: None,
            window_id: Some(1),
            pid: None,
            app_id: None,
            wm_class: None,
            window_title: None,
            relative: Some(true),
            observe: None,
            until: None,
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
mod field_entry_tests {
    use super::*;

    fn window(wm_class: &str) -> WindowInfo {
        WindowInfo {
            window_id: 1,
            title: None,
            app_id: None,
            wm_class: Some(wm_class.to_string()),
            pid: None,
            bounds: None,
            workspace: None,
            focused: true,
            hidden: false,
            client_type: None,
            backend: "i3".to_string(),
            terminal: None,
        }
    }

    #[test]
    fn set_value_pastes_multi_line_and_non_ascii_values_in_one_piece() {
        assert_eq!(
            literal_insertion("Olá Ana"),
            Insertion::Paste("Olá Ana".into())
        );
        // No Return key: chat editors send on Enter.
        assert_eq!(
            literal_insertion("Hi Ana,\r\nsee you"),
            Insertion::Paste("Hi Ana,\nsee you".into())
        );
        assert_eq!(
            literal_insertion("line\n"),
            Insertion::Paste("line\n".into())
        );
        assert_eq!(literal_insertion("plain text"), Insertion::Type);
    }

    #[test]
    fn terminals_paste_with_their_own_chord() {
        assert_eq!(paste_chord_for(Some(&window("Alacritty"))), "ctrl+shift+v");
        assert_eq!(
            paste_chord_for(Some(&window("Gnome-terminal"))),
            "ctrl+shift+v"
        );
        assert_eq!(paste_chord_for(Some(&window("XTerm"))), "shift+Insert");
        assert_eq!(paste_chord_for(Some(&window("Google-chrome"))), "ctrl+v");
        assert_eq!(paste_chord_for(None), "ctrl+v");
    }

    #[test]
    fn form_lines_show_the_read_back_value_and_the_failure() {
        let ok = FieldOutcome {
            ok: true,
            value: Some("Joana".into()),
            message: String::new(),
        };
        assert_eq!(
            form_field_line("[3] entry \"Nome\"", &ok, "Joana"),
            "[3] entry \"Nome\": ok value=\"Joana\""
        );
        let failed = FieldOutcome {
            ok: false,
            value: Some(String::new()),
            message: "could not focus".into(),
        };
        assert_eq!(
            form_field_line("[4] entry", &failed, "Joana"),
            "[4] entry: FAILED value=\"\" error: could not focus"
        );
        let masked = FieldOutcome {
            ok: true,
            value: Some("4111 1111 1111 1111".into()),
            message: String::new(),
        };
        assert_eq!(
            form_field_line("[5] entry \"Cartão\"", &masked, "4111111111111111"),
            "[5] entry \"Cartão\": ok value=\"4111111111111111\" (the field formatted it as \"4111 1111 1111 1111\")"
        );
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
            object_ref: String::new(),
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
    async fn type_text_refuses_a_window_and_an_element_of_different_apps_before_focus() {
        let backend = ComputerUseLinux::default();
        let mut node = cached_node(2, ":1.9/org/a11y/atspi/accessible/42");
        node.role = "entry".to_string();
        node.states.push("editable".to_string());
        backend.cache_snapshot(std::slice::from_ref(&node), Some(111));

        let result = backend
            .dispatch_type_text(TypeTextParams {
                text: "x".to_string(),
                element_index: Some(2),
                pid: Some(222),
                ..Default::default()
            })
            .await
            .0;
        assert!(!result.ok);
        // Refused by the target check, not by a failed focus attempt.
        assert!(
            result.message.contains("in different apps")
                && result.message.contains("not the target pid 222"),
            "{}",
            result.message
        );
    }

    #[test]
    fn typed_text_counts_as_delivered_only_when_the_field_changed() {
        assert_eq!(
            typed_delivery(Some(""), Some("oi"), "oi"),
            Delivery::Arrived
        );
        assert_eq!(typed_delivery(None, Some("oi"), "oi"), Delivery::Arrived);
        // A clipboard read alone is no proof: the field did not change.
        assert_eq!(
            typed_delivery(Some("draft"), Some("draft"), "ção"),
            Delivery::Unchanged
        );
        assert_eq!(typed_delivery(Some(""), None, "oi"), Delivery::Unverified);
        // Enter may submit and clear the field.
        assert_eq!(
            typed_delivery(Some(""), Some(""), "oi\n"),
            Delivery::Unverified
        );
        assert_eq!(
            typed_delivery(Some("x"), Some("x"), "\n"),
            Delivery::Unverified
        );
    }

    #[test]
    fn typing_stops_when_focusing_the_element_left_the_target_window() {
        let window = |window_id: u64, pid: u32| WindowInfo {
            window_id,
            title: None,
            app_id: None,
            wm_class: None,
            pid: Some(pid),
            bounds: None,
            workspace: None,
            focused: true,
            hidden: false,
            client_type: None,
            backend: "test".to_string(),
            terminal: None,
        };
        let requested = window(10, 1);
        assert!(focus_drift(&requested, true, Some(&window(10, 1)), "[2]").is_none());
        let drift = focus_drift(&requested, true, Some(&window(11, 1)), "[2]").unwrap();
        assert!(drift.contains("window_id 11 (pid 1)"), "{drift}");
        assert!(focus_drift(&requested, false, Some(&window(11, 1)), "[2]").is_none());
        assert!(focus_drift(&requested, false, Some(&window(12, 2)), "[2]").is_some());
        assert!(focus_drift(&requested, true, None, "[2]").is_some());
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
