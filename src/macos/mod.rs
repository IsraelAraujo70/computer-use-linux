//! macOS backend: the same MCP contract as the Linux server (tool names,
//! parameter names, outline format, `doctor` readiness) for the subset
//! `doctor`, `list_apps`, `get_app_state`, `click` and `screenshot`, over AX,
//! CGEvent and `screencapture`.
//!
//! ponytail: one-day trial. No window targeting, typing, scrolling or
//! set_value yet; the Linux parameter structs are mirrored (not shared) so the
//! Linux server stays untouched. Port the rest by moving the shared structs out
//! of `server.rs` once this backend is kept.

mod ax;

use crate::a11y::AccessibilityNode;
use crate::element_ids::ElementIds;
use crate::outline::{ElementQuery, TreeFormat};
use crate::page_view::{change_report, page_view, PageView, ViewOptions};
use crate::screenshot::{
    prepare_screenshot_payload, RawScreenshotCapture, ScreenshotCapture, ScreenshotOutputFormat,
    ScreenshotPayloadOptions,
};
use anyhow::{Context, Result};
use rmcp::{
    handler::server::wrapper::{Json, Parameters},
    model::{CallToolResult, ContentBlock},
    schemars::JsonSchema,
    tool, tool_handler, tool_router, ServerHandler, ServiceExt,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

const DEFAULT_MAX_NODES: usize = 1_000;
const HARD_MAX_NODES: usize = 2_000;
const DEFAULT_MAX_DEPTH: u32 = 32;
const HARD_MAX_DEPTH: u32 = 64;
/// Same rule as Linux: attach a screenshot when the tree shows fewer content
/// elements than this.
const MIN_CONTENT_ELEMENTS: usize = 5;
const CLICK_SETTLE: Duration = Duration::from_millis(400);

/// `computer-use-linux <command>` on macOS: `mcp`, `doctor`, `apps`,
/// `state [app]`, `screenshot`.
pub async fn run_from_env() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let server = ComputerUseMac::default();
    match args.get(1).map(String::as_str) {
        Some("mcp") => {
            server
                .serve(rmcp::transport::stdio())
                .await?
                .waiting()
                .await?;
        }
        Some("doctor") => println!("{}", serde_json::to_string_pretty(&doctor_report())?),
        Some("apps") => println!("{}", serde_json::to_string_pretty(&ax::running_apps())?),
        Some("state") => {
            let params = GetAppStateParams {
                app_name_or_bundle_identifier: args.get(2).cloned(),
                ..Default::default()
            };
            let (text, _) = server.app_state(params).await?;
            print!("{text}");
        }
        Some("screenshot") => {
            let capture = capture(ScreenshotParams::default()).await?;
            println!("{}", caption(&capture));
        }
        _ => {
            eprintln!("usage: computer-use-linux <mcp|doctor|apps|state [app]|screenshot>");
            std::process::exit(2);
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
struct GetAppStateParams {
    /// App name or bundle id, e.g. safari or com.apple.finder. Omitting it reads the frontmost app.
    #[serde(default)]
    app_name_or_bundle_identifier: Option<String>,
    /// Process id of the app.
    #[serde(default)]
    pid: Option<u32>,
    /// Nodes to read (default 1000, max 2000); raise it when told the tree was cut.
    #[serde(default)]
    max_nodes: Option<usize>,
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
    /// Screen x in points (screenshot pixel / scale).
    #[serde(default)]
    x: Option<i32>,
    /// Screen y in points (screenshot pixel / scale).
    #[serde(default)]
    y: Option<i32>,
    /// left (default), right or middle.
    #[serde(default)]
    button: Option<String>,
    #[serde(default)]
    click_count: Option<u32>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
struct ScreenshotParams {
    /// Maximum returned screenshot width in pixels (default 1920, hard-capped).
    #[serde(default)]
    max_width: Option<u32>,
    #[schemars(skip)]
    #[serde(default)]
    max_height: Option<u32>,
    #[schemars(skip)]
    #[serde(default)]
    max_bytes: Option<usize>,
    /// Output image format (default png). Use jpeg with quality to trade exact pixels for smaller payloads.
    #[serde(default)]
    format: Option<ScreenshotOutputFormat>,
    #[schemars(skip)]
    #[serde(default)]
    quality: Option<u8>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
struct Check {
    ok: bool,
    detail: String,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
struct DoctorReport {
    platform: PlatformReport,
    accessibility: Check,
    screen_recording: Check,
    readiness: ReadinessReport,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
struct PlatformReport {
    os: String,
    arch: String,
    /// The binary TCC grants apply to when this process is its own
    /// responsible process (launchd, not a terminal or app child).
    executable: Option<String>,
    display_points: (u32, u32),
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
struct ReadinessReport {
    ready: bool,
    can_build_accessibility_tree: bool,
    can_send_input: bool,
    can_capture_screenshots: bool,
    recommended_next_step: String,
    blockers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
struct ListAppsOutput {
    apps: Vec<ax::RunningApp>,
    accessibility_error: Option<String>,
    note: String,
}

fn doctor_report() -> DoctorReport {
    let trusted = ax::ax_trusted();
    let capture = ax::screen_capture_allowed();
    let executable = std::env::current_exe()
        .ok()
        .map(|path| path.display().to_string());
    let grant = |pane: &str| {
        format!(
            "grant {pane} to {} in System Settings → Privacy & Security → {pane}, then restart the server",
            executable.as_deref().unwrap_or("this binary")
        )
    };
    let mut blockers = Vec::new();
    if !trusted {
        blockers.push(grant("Accessibility"));
    }
    if !capture {
        blockers.push(grant("Screen Recording"));
    }
    DoctorReport {
        platform: PlatformReport {
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            executable: executable.clone(),
            display_points: ax::main_display_points(),
        },
        accessibility: Check {
            ok: trusted,
            detail: format!("AXIsProcessTrusted={trusted}"),
        },
        screen_recording: Check {
            ok: capture,
            detail: format!("CGPreflightScreenCaptureAccess={capture}"),
        },
        readiness: ReadinessReport {
            ready: blockers.is_empty(),
            can_build_accessibility_tree: trusted,
            can_send_input: trusted,
            can_capture_screenshots: capture,
            recommended_next_step: blockers
                .first()
                .cloned()
                .unwrap_or_else(|| "call get_app_state".to_string()),
            blockers,
        },
    }
}

/// What `click` needs to reread the tree the agent last saw.
#[derive(Clone, Copy)]
struct Scope {
    pid: u32,
    max_nodes: usize,
    max_depth: u32,
    options: ViewOptions,
}

#[derive(Default)]
struct State {
    ids: ElementIds,
    /// Every element seen, by object ref, so ids from earlier results resolve.
    elements: HashMap<String, ax::Element>,
    nodes: Vec<AccessibilityNode>,
    view: PageView,
    scope: Option<Scope>,
}

#[derive(Clone, Default)]
pub struct ComputerUseMac {
    state: Arc<Mutex<State>>,
}

impl ComputerUseMac {
    /// Reads `scope`'s tree, assigns stable ids, and makes it the snapshot
    /// ids resolve against. Returns the nodes, their view, and whether the
    /// read was cut.
    async fn observe(&self, scope: Scope) -> Result<(Vec<AccessibilityNode>, PageView, bool)> {
        let (mut nodes, elements, truncated) = tokio::task::spawn_blocking(move || {
            ax::snapshot(scope.pid, scope.max_nodes, scope.max_depth)
        })
        .await
        .context("AX snapshot task failed")??;
        let mut state = self.state.lock().expect("state lock");
        state.ids.assign(&mut nodes);
        state.elements.extend(elements);
        let view = page_view(&nodes, scope.options);
        state.nodes = nodes.clone();
        state.view = view.clone();
        state.scope = Some(scope);
        Ok((nodes, view, truncated))
    }

    async fn app_state(
        &self,
        params: GetAppStateParams,
    ) -> Result<(String, Option<ScreenshotCapture>)> {
        let app = ax::resolve_app(params.app_name_or_bundle_identifier.as_deref(), params.pid)?;
        let scope = Scope {
            pid: app.pid,
            max_nodes: params
                .max_nodes
                .unwrap_or(DEFAULT_MAX_NODES)
                .clamp(1, HARD_MAX_NODES),
            max_depth: params
                .max_depth
                .unwrap_or(DEFAULT_MAX_DEPTH)
                .clamp(1, HARD_MAX_DEPTH),
            options: ViewOptions {
                include_browser_ui: params.include_browser_ui.unwrap_or(false),
            },
        };
        let (nodes, view, truncated) = self.observe(scope).await?;
        if params.tree_format == Some(TreeFormat::Json) {
            return Ok((serde_json::to_string(&nodes)?, None));
        }
        let wants_screenshot = params
            .include_screenshot
            .unwrap_or_else(|| view.content_elements() < MIN_CONTENT_ELEMENTS);
        let screenshot = match wants_screenshot {
            true => Some(capture(ScreenshotParams::default()).await?),
            false => None,
        };
        let mut text = match &view.title {
            Some(title) => format!("window \"{title}\""),
            None => "window (untitled)".to_string(),
        };
        text.push_str(&format!(" app=\"{}\" pid={}", app.name, app.pid));
        if let Some(url) = &view.url {
            text.push_str(&format!(" url={url}"));
        }
        text.push_str(&format!(" elements={}\n", view.entries.len()));
        text.push_str(&view.outline());
        if let Some(capture) = &screenshot {
            text.push_str(&format!(
                "screenshot attached: {}x{} image of {}x{} screen points (scale {}; click x/y = image pixel / scale)\n",
                capture.width, capture.height, capture.coordinate_width, capture.coordinate_height, capture.scale
            ));
        }
        if truncated {
            text.push_str(&format!(
                "note: tree cut at {} nodes or depth {}; raise max_nodes\n",
                scope.max_nodes, scope.max_depth
            ));
        }
        Ok((text, screenshot))
    }

    async fn click(&self, params: ClickParams) -> Result<String> {
        let (target, before, scope) = {
            let state = self.state.lock().expect("state lock");
            let target = match (params.x.zip(params.y), params.element_index) {
                (Some(_), _) => None,
                (None, Some(id)) => Some(id),
                (None, None) => {
                    let query = ElementQuery {
                        role: params.role.clone(),
                        name: params.name.clone(),
                        text: params.text.clone(),
                        states: Some(params.states.clone()),
                    };
                    anyhow::ensure!(
                        !query.is_empty(),
                        "click needs element_index, role/name/text, or x and y"
                    );
                    let node = state
                        .nodes
                        .iter()
                        .find(|node| query.matches(node))
                        .context("no element in the last get_app_state matches")?;
                    Some(node.index)
                }
            };
            (target, state.view.clone(), state.scope)
        };
        let button = params.button.as_deref().unwrap_or("left");
        let count = params.click_count.unwrap_or(1);
        let message = match (params.x.zip(params.y), target) {
            (Some((x, y)), _) => {
                ax::click_at(x as f64, y as f64, button, count)?;
                format!("clicked {button} at {x},{y}")
            }
            (None, Some(id)) => self.click_element(id, button, count)?,
            (None, None) => unreachable!("target resolved above"),
        };
        let mut text = format!("ok: {message}");
        if params.observe != Some(false) {
            if let Some(scope) = scope {
                tokio::time::sleep(CLICK_SETTLE).await;
                let (_, after, _) = self.observe(scope).await?;
                text.push('\n');
                text.push_str(&change_report(
                    &before,
                    &after,
                    &target.into_iter().collect::<Vec<_>>(),
                ));
            }
        }
        Ok(text)
    }

    /// AXPress when the element has it (reaches off-screen items), else a
    /// click at its center. A plain left single click is what AXPress means;
    /// other buttons and counts always click by position.
    fn click_element(&self, id: u32, button: &str, count: u32) -> Result<String> {
        let state = self.state.lock().expect("state lock");
        let object_ref = state
            .ids
            .object_ref(id)
            .with_context(|| format!("[{id}] is not an element id from this server"))?;
        let element = state
            .elements
            .get(object_ref)
            .with_context(|| format!("[{id}] no longer exists"))?
            .clone();
        let node = state.nodes.iter().find(|node| node.index == id).cloned();
        drop(state);
        let label = node
            .as_ref()
            .map(crate::outline::element_label)
            .unwrap_or_else(|| format!("[{id}]"));
        if button == "left" && count == 1 && ax::press(&element)? {
            return Ok(format!("pressed {label} (AXPress)"));
        }
        let bounds = node
            .and_then(|node| node.bounds)
            .with_context(|| format!("{label} has no AXPress and no on-screen bounds"))?;
        let (x, y) = (
            bounds.x as f64 + bounds.width as f64 / 2.0,
            bounds.y as f64 + bounds.height as f64 / 2.0,
        );
        ax::click_at(x, y, button, count)?;
        Ok(format!("clicked {label} at its center {x:.0},{y:.0}"))
    }
}

/// Main display through `screencapture` (native, inherits this process's
/// Screen Recording grant). The image is in pixels (Retina 2x before any
/// resize); coordinates are points, so `scale` = image pixels per point.
async fn capture(params: ScreenshotParams) -> Result<ScreenshotCapture> {
    anyhow::ensure!(
        ax::screen_capture_allowed(),
        "Screen Recording permission missing for this binary (System Settings → Privacy & Security → Screen Recording)"
    );
    let path = std::env::temp_dir().join(format!("cul-shot-{}.png", std::process::id()));
    let status = tokio::process::Command::new("/usr/sbin/screencapture")
        .args(["-x", "-m", "-t", "png"])
        .arg(&path)
        .status()
        .await
        .context("failed to run screencapture")?;
    anyhow::ensure!(status.success(), "screencapture exited with {status}");
    let bytes = std::fs::read(&path).context("screencapture wrote no file")?;
    let _ = std::fs::remove_file(&path);
    let (points_width, points_height) = ax::main_display_points();
    let options = ScreenshotPayloadOptions {
        max_width: params.max_width,
        max_height: params.max_height,
        max_bytes: params.max_bytes,
        scale: None,
        format: params.format,
        quality: params.quality,
    };
    let raw = RawScreenshotCapture {
        mime_type: "image/png".to_string(),
        bytes,
        source: "screencapture".to_string(),
        width: 0,
        height: 0,
    };
    let mut capture = tokio::task::spawn_blocking(move || prepare_screenshot_payload(raw, options))
        .await
        .context("screenshot task failed")??;
    capture.coordinate_width = points_width;
    capture.coordinate_height = points_height;
    capture.scale = capture.width as f32 / points_width.max(1) as f32;
    Ok(capture)
}

fn caption(capture: &ScreenshotCapture) -> String {
    serde_json::json!({
        "width": capture.width,
        "height": capture.height,
        "coordinate_width": capture.coordinate_width,
        "coordinate_height": capture.coordinate_height,
        "coordinate_unit": "points",
        "scale": capture.scale,
        "resized": capture.resized,
        "bytes": capture.bytes,
        "original_bytes": capture.original_bytes,
        "max_bytes": capture.max_bytes,
        "format": capture.format,
        "quality": capture.quality,
        "source": capture.source,
    })
    .to_string()
}

fn image_block(capture: &ScreenshotCapture) -> ContentBlock {
    let payload = capture
        .data_url
        .split_once(',')
        .map_or(capture.data_url.as_str(), |(_, data)| data);
    ContentBlock::image(payload.to_string(), capture.mime_type.clone())
}

fn error_result(error: anyhow::Error) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(format!("error: {error:#}"))])
}

#[tool_router]
impl ComputerUseMac {
    #[tool(
        name = "doctor",
        description = "Check macOS permissions (Accessibility, Screen Recording) for this binary. readiness.blockers names the fix for each failure.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn doctor(&self) -> Json<DoctorReport> {
        Json(doctor_report())
    }

    #[tool(
        name = "list_apps",
        description = "List running apps (name, bundle id, pid; frontmost first). Name or bundle id works as app_name_or_bundle_identifier.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = true
        )
    )]
    async fn list_apps(&self) -> Json<ListAppsOutput> {
        Json(ListAppsOutput {
            apps: ax::running_apps(),
            accessibility_error: (!ax::ax_trusted())
                .then(|| "Accessibility not granted; see doctor".to_string()),
            note: "macOS Computer Use lists regular (Dock) apps from NSWorkspace.".to_string(),
        })
    }

    #[tool(
        name = "get_app_state",
        description = "Observe an app. Returns a header (window, app, pid, url, element count) and one line per element: `[n] role \"name\" @x,y wxh {states} actions=... text=...`, nested by indent, positions in screen points. Elements outside the visible area say (below), (above) or (off-screen) instead of @x,y. [n] is an element id, valid in click while the element exists. Browsers show only the web page. A screenshot is attached only when the tree shows little. Scope with app_name_or_bundle_identifier (e.g. safari) or pid; default is the frontmost app.",
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
    ) -> CallToolResult {
        match self.app_state(params).await {
            Ok((text, screenshot)) => {
                let mut content = vec![ContentBlock::text(text)];
                content.extend(screenshot.as_ref().map(image_block));
                CallToolResult::success(content)
            }
            Err(error) => error_result(error),
        }
    }

    #[tool(
        name = "click",
        description = "Click an element by element_index (or role/name/text), or screen x/y in points. Element clicks use AXPress when the element has it, so off-screen items in lists work; otherwise the element's center is clicked. Reports what changed.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = true
        )
    )]
    async fn click_tool(&self, Parameters(params): Parameters<ClickParams>) -> CallToolResult {
        match self.click(params).await {
            Ok(text) => CallToolResult::success(vec![ContentBlock::text(text)]),
            Err(error) => error_result(error),
        }
    }

    #[tool(
        name = "screenshot",
        description = "Capture the main display as an image. The caption gives scale: click x/y (points) = image pixel / scale.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = true
        )
    )]
    async fn screenshot(&self, Parameters(params): Parameters<ScreenshotParams>) -> CallToolResult {
        match capture(params).await {
            Ok(capture) => CallToolResult::success(vec![
                image_block(&capture),
                ContentBlock::text(caption(&capture)),
            ]),
            Err(error) => error_result(error),
        }
    }
}

#[tool_handler(
    name = "computer-use-linux",
    version = "0.7.17",
    instructions = "macOS backend (trial): doctor, list_apps, get_app_state, click, screenshot. Start with get_app_state (scope it, e.g. app_name_or_bundle_identifier=safari). Each line starts with an element id [n]; click by id. click reports what changed (+ new, - gone, ~ changed, focus). Coordinates are screen points; screenshot captions give the pixel scale. If a tool fails, doctor names the fix."
)]
impl ServerHandler for ComputerUseMac {}
