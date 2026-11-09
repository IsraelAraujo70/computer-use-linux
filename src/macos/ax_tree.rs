//! macOS implementation of the accessibility layer, with the same API as
//! `atspi_tree.rs` so `server.rs` runs unchanged on both platforms.
//!
//! AX roles, states and actions are renamed to their AT-SPI equivalents
//! (`AXWebArea` → `document web`, `AXPress` → `click`, ...) so the shared
//! outline, page view and field logic read macOS trees the way they read
//! Linux ones. Object refs are `pid:hash`; a process-wide registry maps them
//! back to retained `AXUIElement`s. Every AX call is a synchronous IPC to the
//! target app, run off the async runtime and bounded by the global AX
//! messaging timeout plus `SNAPSHOT_TIMEOUT`.

use anyhow::{anyhow, bail, Context, Result};
use objc2_app_kit::{NSApplicationActivationPolicy, NSRunningApplication, NSWorkspace};
use objc2_application_services::{
    AXCopyMultipleAttributeOptions, AXError, AXIsProcessTrusted, AXUIElement, AXValue, AXValueType,
};
use objc2_core_foundation::{
    CFArray, CFBoolean, CFNumber, CFRange, CFRetained, CFString, CFType, CGPoint, CGSize,
};
use schemars::JsonSchema;
use serde::Serialize;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    ptr::NonNull,
    sync::{LazyLock, Mutex, Once},
    time::Duration,
};

pub use crate::a11y::{
    AccessibilityAction, AccessibilityNode, AccessibilityText, AccessibilityTextSelection,
    AccessibilityValue, Bounds,
};

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct AccessibleAppSummary {
    pub object_ref: String,
    pub name: Option<String>,
    pub pid: Option<u32>,
    pub role: String,
    pub child_count: i32,
    pub bounds: Option<Bounds>,
}

#[derive(Debug, Clone)]
pub struct ActionInvocation {
    pub action_index: i32,
    pub action_name: Option<String>,
    pub ok: bool,
}

#[derive(Debug, Clone)]
pub enum ValueSetInvocation {
    Numeric { value: f64 },
    EditableText,
}

#[derive(Debug, Clone)]
pub(crate) struct AccessibilitySnapshot {
    pub nodes: Vec<AccessibilityNode>,
    /// True when an app name or pid selected the app read.
    pub scoped: bool,
    /// The pid of the app read (on macOS always known).
    pub root_pid: Option<u32>,
    /// True when max_nodes or max_depth stopped traversal with unread
    /// elements left.
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct FocusedElementSummary {
    pub role: String,
    pub name: Option<String>,
    pub editable: bool,
    pub states: Vec<String>,
    #[serde(skip)]
    #[schemars(skip)]
    pub is_terminal: bool,
    #[serde(skip)]
    #[schemars(skip)]
    pub object_ref: String,
}

#[derive(Debug, Clone)]
pub(crate) enum FocusProbe {
    Found(FocusedElementSummary),
    NoneFocused,
    #[allow(dead_code)]
    ShellOnly,
    #[allow(dead_code)]
    Incomplete,
    NoAccessibleApp,
}

pub(crate) const UNKNOWN_ROLE: &str = "unknown";
const DEFAULT_SNAPSHOT_MAX_NODES: usize = 1_000;
const HARD_SNAPSHOT_MAX_NODES: usize = 2_000;
const DEFAULT_SNAPSHOT_MAX_DEPTH: u32 = 32;
const HARD_SNAPSHOT_MAX_DEPTH: u32 = 64;
const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(10);
/// Per-IPC limit, so a hung app fails a read instead of hanging the server.
const AX_MESSAGING_TIMEOUT_SECS: f32 = 1.5;
const MAX_TEXT_CHARS: usize = 4096;
/// Registry size past which old entries are dropped (their ids then report
/// "no longer exists").
const MAX_REGISTERED_ELEMENTS: usize = 200_000;

pub(crate) fn snapshot_limits(
    requested_max_nodes: Option<usize>,
    requested_max_depth: Option<u32>,
) -> (usize, u32) {
    (
        requested_max_nodes
            .unwrap_or(DEFAULT_SNAPSHOT_MAX_NODES)
            .clamp(1, HARD_SNAPSHOT_MAX_NODES),
        requested_max_depth
            .unwrap_or(DEFAULT_SNAPSHOT_MAX_DEPTH)
            .min(HARD_SNAPSHOT_MAX_DEPTH),
    )
}

/// Same rule as Linux: window and menu chrome is evidence about tree
/// coverage, not content.
pub(crate) fn accessibility_chrome_context(role: &str, in_menu: bool) -> Option<bool> {
    match role {
        "application" | "frame" | "window" | "panel" | "filler" | "separator" => Some(in_menu),
        "menu bar" | "menu" | "menu item" | "check menu item" | "radio menu item"
        | "tearoff menu item" => Some(true),
        "button" | "push button" | "label" | "static" | "image" | "icon" if in_menu => Some(true),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Element registry

/// A retained AX element. `AXUIElement` is an immutable reference to a remote
/// object; AX client calls on it are IPC requests callable from any thread.
#[derive(Clone)]
pub(crate) struct Element(CFRetained<AXUIElement>);
// SAFETY: see the type's doc; the CF object itself is never mutated.
unsafe impl Send for Element {}
unsafe impl Sync for Element {}

impl Element {
    fn cf(&self) -> &CFType {
        AsRef::<CFType>::as_ref(&*self.0)
    }
}

static REGISTRY: LazyLock<Mutex<HashMap<String, Element>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// The ref of `element`, registering it on first sight. Two different
/// elements whose hashes collide get distinct `#n` suffixes.
fn register(pid: u32, element: &CFRetained<AXUIElement>) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    AsRef::<CFType>::as_ref(&**element).hash(&mut hasher);
    let base = format!("{pid}:{:x}", hasher.finish());
    let mut registry = REGISTRY.lock().expect("AX registry lock");
    if registry.len() > MAX_REGISTERED_ELEMENTS {
        registry.clear();
    }
    let wanted = Element(element.clone());
    for suffix in 0.. {
        let key = match suffix {
            0 => base.clone(),
            n => format!("{base}#{n}"),
        };
        match registry.get(&key) {
            Some(existing) if existing.cf() == wanted.cf() => return key,
            Some(_) => continue,
            None => {
                registry.insert(key.clone(), wanted);
                return key;
            }
        }
    }
    unreachable!("an unused suffix always exists")
}

fn lookup(object_ref: &str) -> Result<Element> {
    REGISTRY
        .lock()
        .expect("AX registry lock")
        .get(object_ref)
        .cloned()
        .ok_or_else(|| anyhow!("AX object {object_ref} no longer exists"))
}

fn pid_of_ref(object_ref: &str) -> Option<u32> {
    object_ref.split(':').next()?.parse().ok()
}

// ---------------------------------------------------------------------------
// Process-level setup

static INIT: Once = Once::new();
static MANUAL_ACCESSIBILITY: LazyLock<Mutex<HashSet<u32>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

fn ensure_ready() -> Result<()> {
    INIT.call_once(|| unsafe {
        AXUIElement::new_system_wide().set_messaging_timeout(AX_MESSAGING_TIMEOUT_SECS);
    });
    if !unsafe { AXIsProcessTrusted() } {
        bail!(
            "Accessibility permission missing for {} (System Settings → Privacy & Security → Accessibility)",
            std::env::current_exe()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|_| "this binary".into())
        );
    }
    Ok(())
}

/// The app element of `pid`. Chromium and Electron build their web
/// accessibility tree only for assistive clients that ask for it, so the
/// first read of each app sets `AXManualAccessibility` (ignored by apps that
/// do not know it).
fn app_element(pid: u32) -> CFRetained<AXUIElement> {
    let app = unsafe { AXUIElement::new_application(pid as libc::pid_t) };
    let first = MANUAL_ACCESSIBILITY
        .lock()
        .map(|mut seen| seen.insert(pid))
        .unwrap_or(false);
    if first {
        let _ = unsafe {
            app.set_attribute_value(
                &CFString::from_str("AXManualAccessibility"),
                AsRef::<CFType>::as_ref(CFBoolean::new(true)),
            )
        };
    }
    app
}

/// Runs AX work off the async runtime with the snapshot deadline.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    tokio::time::timeout(SNAPSHOT_TIMEOUT, tokio::task::spawn_blocking(work))
        .await
        .context("AX read exceeded its 10-second deadline")?
        .context("AX task failed")?
}

// ---------------------------------------------------------------------------
// Running apps

#[derive(Debug, Clone)]
pub(crate) struct RunningApp {
    pub name: String,
    pub bundle_id: Option<String>,
    pub pid: u32,
    pub active: bool,
}

/// Pid of the frontmost app, read live through AX. NSWorkspace's
/// `frontmostApplication` and `isActive` only update on a run loop, which
/// this server does not run, so they go stale after the first read.
pub(crate) fn frontmost_pid() -> Option<u32> {
    let app = copy_attribute(
        &*unsafe { AXUIElement::new_system_wide() },
        "AXFocusedApplication",
    )?
    .downcast::<AXUIElement>()
    .ok()?;
    let mut pid: libc::pid_t = 0;
    (unsafe { app.pid(NonNull::from(&mut pid)) } == AXError::Success).then_some(pid as u32)
}

/// Regular (Dock) apps, frontmost first.
pub(crate) fn running_apps() -> Vec<RunningApp> {
    let frontmost = frontmost_pid();
    let mut apps: Vec<RunningApp> = NSWorkspace::sharedWorkspace()
        .runningApplications()
        .iter()
        .filter(|app| app.activationPolicy() == NSApplicationActivationPolicy::Regular)
        .map(|app| RunningApp {
            name: app
                .localizedName()
                .map(|name| name.to_string())
                .unwrap_or_default(),
            bundle_id: app.bundleIdentifier().map(|id| id.to_string()),
            pid: app.processIdentifier() as u32,
            active: Some(app.processIdentifier() as u32) == frontmost,
        })
        .collect();
    apps.sort_by_key(|app| !app.active);
    apps
}

/// Name and bundle id of the app owning `pid`.
pub(crate) fn app_identity(pid: u32) -> Option<(String, Option<String>)> {
    let app = NSRunningApplication::runningApplicationWithProcessIdentifier(pid as libc::pid_t)?;
    Some((
        app.localizedName()
            .map(|name| name.to_string())
            .unwrap_or_default(),
        app.bundleIdentifier().map(|id| id.to_string()),
    ))
}

/// The app an `app_name_or_bundle_identifier` names: exact bundle id, exact
/// name, then name or bundle id substring, case-insensitive.
fn match_app(filter: &str) -> Option<u32> {
    let needle = filter.trim().to_lowercase();
    if needle.is_empty() {
        return None;
    }
    let apps = running_apps();
    let lower = |value: &Option<String>| value.as_deref().map(str::to_lowercase);
    apps.iter()
        .find(|app| lower(&app.bundle_id).as_deref() == Some(needle.as_str()))
        .or_else(|| apps.iter().find(|app| app.name.to_lowercase() == needle))
        .or_else(|| {
            apps.iter().find(|app| {
                app.name.to_lowercase().contains(&needle)
                    || lower(&app.bundle_id).is_some_and(|id| id.contains(&needle))
            })
        })
        .map(|app| app.pid)
}

pub async fn list_accessible_apps(limit: usize) -> Result<Vec<AccessibleAppSummary>> {
    blocking(move || {
        ensure_ready()?;
        Ok(running_apps()
            .into_iter()
            .take(limit)
            .map(|app| {
                let element = app_element(app.pid);
                let child_count = copy_attribute(&element, "AXChildren")
                    .and_then(|value| value.downcast::<CFArray>().ok())
                    .map_or(0, |children| children.len() as i32);
                AccessibleAppSummary {
                    object_ref: register(app.pid, &element),
                    name: Some(match &app.bundle_id {
                        Some(id) => format!("{} ({id})", app.name),
                        None => app.name.clone(),
                    }),
                    pid: Some(app.pid),
                    role: "application".to_string(),
                    child_count,
                    bounds: None,
                }
            })
            .collect())
    })
    .await
}

// ---------------------------------------------------------------------------
// Snapshots

pub async fn snapshot_tree(
    app_name_or_bundle_identifier: Option<&str>,
    target_pid: Option<u32>,
    max_nodes: usize,
    max_depth: u32,
) -> Result<Vec<AccessibilityNode>> {
    Ok(snapshot_accessibility_tree(
        app_name_or_bundle_identifier,
        target_pid,
        max_nodes,
        max_depth,
    )
    .await?
    .nodes)
}

/// The app named by `target_pid`, else by name or bundle id, else the
/// frontmost app (unscoped: macOS has no useful "whole desktop" tree).
pub(crate) async fn snapshot_accessibility_tree(
    app_name_or_bundle_identifier: Option<&str>,
    target_pid: Option<u32>,
    max_nodes: usize,
    max_depth: u32,
) -> Result<AccessibilitySnapshot> {
    let (max_nodes, max_depth) = snapshot_limits(Some(max_nodes), Some(max_depth));
    let filter = app_name_or_bundle_identifier.map(str::to_string);
    blocking(move || {
        ensure_ready()?;
        let (pid, scoped) = match (target_pid, filter.as_deref().and_then(match_app)) {
            (Some(pid), _) => (pid, true),
            (None, Some(pid)) => (pid, true),
            (None, None) => (
                running_apps()
                    .first()
                    .map(|app| app.pid)
                    .context("no running apps")?,
                false,
            ),
        };
        let app = app_element(pid);
        let (nodes, truncated) = traverse(pid, vec![(app, 0)], max_nodes, max_depth);
        if nodes.is_empty() {
            bail!("AX returned nothing for pid {pid}: the app is not responding or has no accessibility tree");
        }
        Ok(AccessibilitySnapshot {
            nodes,
            scoped,
            root_pid: Some(pid),
            truncated,
        })
    })
    .await
}

pub(crate) async fn snapshot_subtree(
    object_ref_id: &str,
    depth: u32,
    max_nodes: usize,
    max_depth: u32,
) -> Result<Vec<AccessibilityNode>> {
    let (max_nodes, max_depth) = snapshot_limits(Some(max_nodes), Some(max_depth));
    let object_ref = object_ref_id.to_string();
    blocking(move || {
        ensure_ready()?;
        let element = live_element(&object_ref)?;
        let pid = pid_of_ref(&object_ref).unwrap_or_default();
        Ok(traverse(pid, vec![(element.0, depth)], max_nodes, max_depth).0)
    })
    .await
}

pub(crate) async fn read_subtree(
    object_ref_id: &str,
    max_nodes: usize,
    max_depth: u32,
) -> Result<Vec<AccessibilityNode>> {
    let object_ref = object_ref_id.to_string();
    blocking(move || {
        ensure_ready()?;
        let element = live_element(&object_ref)?;
        let pid = pid_of_ref(&object_ref).unwrap_or_default();
        Ok(traverse(pid, vec![(element.0, 0)], max_nodes, max_depth).0)
    })
    .await
}

pub(crate) async fn read_element(object_ref_id: &str) -> Result<AccessibilityNode> {
    let object_ref = object_ref_id.to_string();
    blocking(move || {
        ensure_ready()?;
        let element = lookup(&object_ref)?;
        let names = attribute_names();
        let values = copy_values(&element.0, &names)
            .with_context(|| format!("AX object {object_ref} no longer exists"))?;
        let focused = keyboard_focus();
        let child_count = children_of(&values).len();
        Ok(read_node(
            &element.0,
            &values,
            0,
            None,
            0,
            &object_ref,
            child_count,
            is_same(focused.as_ref(), &element.0),
        ))
    })
    .await
}

pub(crate) async fn parent_ref(id: &str) -> Result<String> {
    let object_ref = id.to_string();
    blocking(move || {
        ensure_ready()?;
        let element = lookup(&object_ref)?;
        let parent = copy_attribute(&element.0, "AXParent")
            .and_then(|value| value.downcast::<AXUIElement>().ok())
            .with_context(|| format!("AX object {object_ref} has no parent or no longer exists"))?;
        Ok(register(
            pid_of_ref(&object_ref).unwrap_or_default(),
            &parent,
        ))
    })
    .await
}

pub(crate) async fn object_ref_owner_pid(object_ref_id: &str) -> Result<Option<u32>> {
    Ok(pid_of_ref(object_ref_id))
}

/// An element that still answers AX reads.
fn live_element(object_ref: &str) -> Result<Element> {
    let element = lookup(object_ref)?;
    copy_attribute(&element.0, "AXRole")
        .with_context(|| format!("AX object {object_ref} no longer exists"))?;
    Ok(element)
}

/// Breadth-first read from `roots`, at most `max_nodes` nodes, children read
/// while their parent's depth is below `max_depth`. `index`/`parent_index`
/// are positions in the result.
fn traverse(
    pid: u32,
    roots: Vec<(CFRetained<AXUIElement>, u32)>,
    max_nodes: usize,
    max_depth: u32,
) -> (Vec<AccessibilityNode>, bool) {
    let names = attribute_names();
    let focused = keyboard_focus();
    let mut queue: VecDeque<(CFRetained<AXUIElement>, u32, Option<u32>)> = roots
        .into_iter()
        .map(|(element, depth)| (element, depth, None))
        .collect();
    let mut nodes: Vec<AccessibilityNode> = Vec::new();
    let mut truncated = false;
    while let Some((element, depth, parent)) = queue.pop_front() {
        if nodes.len() >= max_nodes {
            truncated = true;
            break;
        }
        let Some(values) = copy_values(&element, &names) else {
            continue;
        };
        let children = children_of(&values);
        let index = nodes.len() as u32;
        let object_ref = register(pid, &element);
        nodes.push(read_node(
            &element,
            &values,
            index,
            parent,
            depth,
            &object_ref,
            children.len(),
            is_same(focused.as_ref(), &element),
        ));
        // A menu bar item lists its whole closed menu as children; read
        // them only while the menu is open (the item is selected).
        let closed_menu = string(&values[Attr::Role as usize]).as_deref() == Some("AXMenuBarItem")
            && boolean(&values[Attr::Selected as usize]) != Some(true);
        if closed_menu || children.is_empty() {
            continue;
        }
        if depth >= max_depth {
            truncated = true;
            continue;
        }
        queue.extend(
            children
                .into_iter()
                .map(|child| (child, depth + 1, Some(index))),
        );
    }
    (nodes, truncated)
}

fn children_of(values: &[Option<CFRetained<CFType>>]) -> Vec<CFRetained<AXUIElement>> {
    values[Attr::Children as usize]
        .as_ref()
        .and_then(|value| value.downcast_ref::<CFArray>())
        .map(|array| {
            as_typed(array)
                .iter()
                .filter_map(|child| child.downcast::<AXUIElement>().ok())
                .collect()
        })
        .unwrap_or_default()
}

/// The element holding the system's keyboard focus: the system-wide
/// AXFocusedUIElement, inside the frontmost app. An app in the background
/// keeps its own focused element, but keys do not reach it, so only this
/// one is `focused` (AXFocused is also true on every cell of a focused
/// outline). The server's "type only into verified focus" checks rely on
/// this.
fn keyboard_focus() -> Option<CFRetained<AXUIElement>> {
    copy_attribute(
        &*unsafe { AXUIElement::new_system_wide() },
        "AXFocusedUIElement",
    )
    .and_then(|value| value.downcast::<AXUIElement>().ok())
}

fn is_same(a: Option<&CFRetained<AXUIElement>>, b: &AXUIElement) -> bool {
    a.is_some_and(|a| AsRef::<CFType>::as_ref(&**a) == AsRef::<CFType>::as_ref(b))
}

// ---------------------------------------------------------------------------
// Focus

pub(crate) async fn probe_focused_element(target_pid: Option<u32>) -> Result<FocusProbe> {
    blocking(move || {
        ensure_ready()?;
        if let Some(pid) = target_pid {
            // A first touch turns on Chromium/Electron trees.
            let _ = app_element(pid);
        }
        let source = unsafe { AXUIElement::new_system_wide() };
        let mut out: *const CFType = std::ptr::null();
        let error = unsafe {
            source.copy_attribute_value(
                &CFString::from_str("AXFocusedUIElement"),
                NonNull::from(&mut out),
            )
        };
        if error == AXError::APIDisabled || error == AXError::CannotComplete {
            return Ok(FocusProbe::NoAccessibleApp);
        }
        if error != AXError::Success || out.is_null() {
            return Ok(FocusProbe::NoneFocused);
        }
        let value = unsafe { CFRetained::from_raw(NonNull::new_unchecked(out as *mut CFType)) };
        let Ok(element) = value.downcast::<AXUIElement>() else {
            return Ok(FocusProbe::NoneFocused);
        };
        let mut raw_pid: libc::pid_t = 0;
        let pid = match unsafe { element.pid(NonNull::from(&mut raw_pid)) } {
            AXError::Success => raw_pid as u32,
            _ => return Ok(FocusProbe::NoneFocused),
        };
        // Keys go to the frontmost app; focus elsewhere is not the target's.
        if target_pid.is_some_and(|target| target != pid) {
            return Ok(FocusProbe::NoneFocused);
        }
        let names = attribute_names();
        let Some(values) = copy_values(&element, &names) else {
            return Ok(FocusProbe::NoneFocused);
        };
        let object_ref = register(pid, &element);
        let node = read_node(&element, &values, 0, None, 0, &object_ref, 0, true);
        Ok(FocusProbe::Found(FocusedElementSummary {
            editable: node.supports_editable_text,
            role: node.role,
            name: node.name,
            states: node.states,
            is_terminal: false,
            object_ref,
        }))
    })
    .await
}

pub(crate) async fn grab_focus(object_ref_id: &str) -> Result<bool> {
    let object_ref = object_ref_id.to_string();
    blocking(move || {
        ensure_ready()?;
        let element = lookup(&object_ref)?;
        let error = unsafe {
            element.0.set_attribute_value(
                &CFString::from_str("AXFocused"),
                AsRef::<CFType>::as_ref(CFBoolean::new(true)),
            )
        };
        Ok(error == AXError::Success)
    })
    .await
}

pub(crate) async fn scroll_into_view(object_ref_id: &str) -> Result<bool> {
    let object_ref = object_ref_id.to_string();
    blocking(move || {
        ensure_ready()?;
        let element = lookup(&object_ref)?;
        if !action_names(&element.0)
            .iter()
            .any(|name| name == "AXScrollToVisible")
        {
            return Ok(false);
        }
        Ok(perform(&element.0, "AXScrollToVisible").is_ok())
    })
    .await
}

// ---------------------------------------------------------------------------
// Actions and values

pub async fn perform_action(
    object_ref_id: &str,
    requested_action: Option<&str>,
) -> Result<ActionInvocation> {
    perform_action_inner(object_ref_id, requested_action.map(str::to_string), false).await
}

pub(crate) async fn perform_named_action(
    object_ref_id: &str,
    name: &str,
) -> Result<ActionInvocation> {
    perform_action_inner(object_ref_id, Some(name.to_string()), true).await
}

async fn perform_action_inner(
    object_ref_id: &str,
    requested: Option<String>,
    name_only: bool,
) -> Result<ActionInvocation> {
    let object_ref = object_ref_id.to_string();
    blocking(move || {
        ensure_ready()?;
        let element = lookup(&object_ref)?;
        let actions = action_names(&element.0);
        if actions.is_empty() {
            bail!("element exposes no AX actions");
        }
        let index = match requested.as_deref() {
            Some(name) => find_action(&actions, name).with_context(|| {
                format!(
                    "no action named {name:?}; available: {}",
                    actions
                        .iter()
                        .map(|ax| atspi_action(ax).unwrap_or(ax.as_str()))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })?,
            None if name_only => bail!("an action name is required"),
            None => actions.iter().position(|ax| ax == "AXPress").unwrap_or(0),
        };
        let ax = &actions[index];
        perform(&element.0, ax)?;
        Ok(ActionInvocation {
            action_index: index as i32,
            action_name: Some(atspi_action(ax).unwrap_or(ax).to_string()),
            ok: true,
        })
    })
    .await
}

pub(crate) async fn invoke_preferred_action(
    object_ref_id: &str,
    preferred: &[&str],
) -> Result<Option<ActionInvocation>> {
    let object_ref = object_ref_id.to_string();
    let preferred: Vec<String> = preferred.iter().map(|name| name.to_string()).collect();
    blocking(move || {
        ensure_ready()?;
        let element = lookup(&object_ref)?;
        let actions = action_names(&element.0);
        let Some(index) = preferred
            .iter()
            .find_map(|wanted| find_action(&actions, wanted))
        else {
            return Ok(None);
        };
        let ax = &actions[index];
        perform(&element.0, ax)?;
        Ok(Some(ActionInvocation {
            action_index: index as i32,
            action_name: Some(atspi_action(ax).unwrap_or(ax).to_string()),
            ok: true,
        }))
    })
    .await
}

/// Index of the action called `wanted`, by its AT-SPI name (`click`) or AX
/// name (`AXPress`), case-insensitive. `press`/`activate` mean AXPress too.
fn find_action(actions: &[String], wanted: &str) -> Option<usize> {
    let wanted = wanted.trim();
    actions.iter().position(|ax| {
        ax.eq_ignore_ascii_case(wanted)
            || atspi_action(ax).is_some_and(|name| name.eq_ignore_ascii_case(wanted))
            || (ax == "AXPress"
                && ["press", "activate", "jump", "open"]
                    .iter()
                    .any(|alias| alias.eq_ignore_ascii_case(wanted)))
    })
}

fn perform(element: &AXUIElement, ax_action: &str) -> Result<()> {
    let error = unsafe { element.perform_action(&CFString::from_str(ax_action)) };
    if error != AXError::Success {
        bail!("{ax_action} failed (AXError {})", error.0);
    }
    Ok(())
}

pub async fn set_element_value(object_ref_id: &str, value: &str) -> Result<ValueSetInvocation> {
    let object_ref = object_ref_id.to_string();
    let value = value.to_string();
    blocking(move || {
        ensure_ready()?;
        let element = lookup(&object_ref)?;
        let key = CFString::from_str("AXValue");
        let mut settable: u8 = 0;
        let error = unsafe {
            element
                .0
                .is_attribute_settable(&key, NonNull::from(&mut settable).cast())
        };
        if error != AXError::Success || settable == 0 {
            bail!("element's AXValue is not settable");
        }
        let numeric = copy_attribute(&element.0, "AXValue")
            .is_some_and(|current| current.downcast_ref::<CFNumber>().is_some());
        if numeric {
            let number: f64 = value
                .trim()
                .parse()
                .with_context(|| format!("element holds a number; {value:?} is not one"))?;
            let error = unsafe {
                element
                    .0
                    .set_attribute_value(&key, AsRef::<CFType>::as_ref(&*CFNumber::new_f64(number)))
            };
            if error != AXError::Success {
                bail!("setting AXValue to {number} failed (AXError {})", error.0);
            }
            return Ok(ValueSetInvocation::Numeric { value: number });
        }
        // Web pages see an AXValue write as no input event at all, so
        // framework-bound fields (React, Angular) keep their old state, and
        // WebKit often ignores the write. Refusing makes the server focus
        // the field and type, which pages handle like a user's typing.
        if inside_web_area(&element.0) {
            bail!("web text fields are typed, not set through AXValue");
        }
        let text = CFString::from_str(&value);
        let error = unsafe {
            element
                .0
                .set_attribute_value(&key, AsRef::<CFType>::as_ref(&*text))
        };
        if error != AXError::Success {
            bail!("setting AXValue text failed (AXError {})", error.0);
        }
        // A write can succeed without taking effect; then typing is next.
        let stored = copy_attribute(&element.0, "AXValue")
            .and_then(|value| value.downcast_ref::<CFString>().map(|s| s.to_string()));
        if stored.as_deref() != Some(value.as_str()) {
            bail!("AXValue write did not take effect");
        }
        Ok(ValueSetInvocation::EditableText)
    })
    .await
}

/// Frame of an element's parent, in points.
fn parent_frame(element: &AXUIElement) -> Option<Bounds> {
    let parent = copy_attribute(element, "AXParent")?
        .downcast::<AXUIElement>()
        .ok()?;
    let point = ax_value(
        &copy_attribute(&parent, "AXPosition"),
        AXValueType::CGPoint,
        CGPoint::ZERO,
    )?;
    let size = ax_value(
        &copy_attribute(&parent, "AXSize"),
        AXValueType::CGSize,
        CGSize::ZERO,
    )?;
    Some(Bounds {
        x: point.x.round() as i32,
        y: point.y.round() as i32,
        width: size.width.round() as i32,
        height: size.height.round() as i32,
    })
}

/// Whether an AXWebArea encloses `element` (bounded walk up AXParent).
fn inside_web_area(element: &AXUIElement) -> bool {
    let mut current = copy_attribute(element, "AXParent")
        .and_then(|value| value.downcast::<AXUIElement>().ok());
    for _ in 0..64 {
        let Some(node) = current else {
            return false;
        };
        let role = copy_attribute(&node, "AXRole")
            .and_then(|value| value.downcast_ref::<CFString>().map(|s| s.to_string()));
        match role.as_deref() {
            Some("AXWebArea") => return true,
            Some("AXWindow") | Some("AXApplication") | None => return false,
            _ => {}
        }
        current = copy_attribute(&node, "AXParent")
            .and_then(|value| value.downcast::<AXUIElement>().ok());
    }
    false
}

/// Raises the AX window of `pid` whose frame or title matches (else its
/// first window) and makes it main. Used by the macOS window backend.
pub(crate) fn raise_window(
    pid: u32,
    title: Option<&str>,
    frame: Option<(i32, i32, u32, u32)>,
) -> Result<bool> {
    ensure_ready()?;
    let app = app_element(pid);
    // AppKit activation from a background process is refused since macOS
    // 14 (cooperative activation); the AX attribute still brings it front.
    let _ = unsafe {
        app.set_attribute_value(
            &CFString::from_str("AXFrontmost"),
            AsRef::<CFType>::as_ref(CFBoolean::new(true)),
        )
    };
    let windows: Vec<CFRetained<AXUIElement>> = copy_attribute(&app, "AXWindows")
        .and_then(|value| value.downcast::<CFArray>().ok())
        .map(|array| {
            as_typed(&array)
                .iter()
                .filter_map(|window| window.downcast::<AXUIElement>().ok())
                .collect()
        })
        .unwrap_or_default();
    let names = attribute_names();
    let matches = |window: &AXUIElement| {
        let Some(values) = copy_values(window, &names) else {
            return false;
        };
        let position = ax_value(
            &values[Attr::Position as usize],
            AXValueType::CGPoint,
            CGPoint::ZERO,
        );
        let size = ax_value(
            &values[Attr::Size as usize],
            AXValueType::CGSize,
            CGSize::ZERO,
        );
        let same_frame = match (frame, position, size) {
            (Some((x, y, w, h)), Some(p), Some(s)) => {
                (p.x.round() as i32, p.y.round() as i32) == (x, y)
                    && (s.width.round() as u32, s.height.round() as u32) == (w, h)
            }
            _ => false,
        };
        same_frame || (title.is_some() && string(&values[Attr::Title as usize]).as_deref() == title)
    };
    let Some(window) = windows
        .iter()
        .find(|window| matches(window))
        .or(windows.first())
    else {
        return Ok(false);
    };
    let _ = perform(window, "AXRaise");
    let _ = unsafe {
        window.set_attribute_value(
            &CFString::from_str("AXMain"),
            AsRef::<CFType>::as_ref(CFBoolean::new(true)),
        )
    };
    Ok(true)
}

// ---------------------------------------------------------------------------
// Raw AX reads

/// Attributes read per element in one `AXUIElementCopyMultipleAttributeValues`
/// IPC; the order is the index into the returned array (`Attr`).
const ATTRIBUTES: [&str; 16] = [
    "AXRole",
    "AXSubrole",
    "AXTitle",
    "AXDescription",
    "AXHelp",
    "AXValue",
    "AXPosition",
    "AXSize",
    "AXEnabled",
    "AXSelected",
    "AXExpanded",
    "AXChildren",
    "AXModal",
    "AXMinValue",
    "AXMaxValue",
    "AXSelectedTextRange",
];

#[derive(Clone, Copy)]
enum Attr {
    Role,
    Subrole,
    Title,
    Description,
    Help,
    Value,
    Position,
    Size,
    Enabled,
    Selected,
    Expanded,
    Children,
    Modal,
    MinValue,
    MaxValue,
    SelectedTextRange,
}

fn attribute_names() -> CFRetained<CFArray<CFString>> {
    let names: Vec<CFRetained<CFString>> = ATTRIBUTES
        .iter()
        .map(|name| CFString::from_str(name))
        .collect();
    CFArray::from_retained_objects(&names)
}

/// AX arrays hold CF objects; view an untyped one as such to iterate it.
fn as_typed(array: &CFArray) -> &CFArray<CFType> {
    // SAFETY: CFArray<T> is a transparent marker over the same CFArrayRef,
    // and every element of an AX attribute array is a CF object.
    unsafe { &*(array as *const CFArray).cast::<CFArray<CFType>>() }
}

fn copy_attribute(element: &AXUIElement, name: &str) -> Option<CFRetained<CFType>> {
    let mut out: *const CFType = std::ptr::null();
    let error =
        unsafe { element.copy_attribute_value(&CFString::from_str(name), NonNull::from(&mut out)) };
    if error != AXError::Success || out.is_null() {
        return None;
    }
    Some(unsafe { CFRetained::from_raw(NonNull::new_unchecked(out as *mut CFType)) })
}

/// One IPC for all `ATTRIBUTES`; a missing attribute comes back as an AXValue
/// of type AXError, mapped to `None`.
fn copy_values(
    element: &AXUIElement,
    names: &CFArray<CFString>,
) -> Option<Vec<Option<CFRetained<CFType>>>> {
    let mut out: *const CFArray = std::ptr::null();
    let error = unsafe {
        element.copy_multiple_attribute_values(
            names.as_ref(),
            AXCopyMultipleAttributeOptions(0),
            NonNull::from(&mut out),
        )
    };
    if error != AXError::Success || out.is_null() {
        return None;
    }
    let array: CFRetained<CFArray<CFType>> =
        unsafe { CFRetained::from_raw(NonNull::new_unchecked(out as *mut CFArray<CFType>)) };
    Some(
        (0..ATTRIBUTES.len())
            .map(|at| {
                let value: CFRetained<CFType> = array.get(at)?;
                match value.downcast_ref::<AXValue>() {
                    Some(ax) if unsafe { ax.r#type() } == AXValueType::AXError => None,
                    _ => Some(value),
                }
            })
            .collect(),
    )
}

fn action_names(element: &AXUIElement) -> Vec<String> {
    let mut out: *const CFArray = std::ptr::null();
    let error = unsafe { element.copy_action_names(NonNull::from(&mut out)) };
    if error != AXError::Success || out.is_null() {
        return Vec::new();
    }
    let array: CFRetained<CFArray<CFType>> =
        unsafe { CFRetained::from_raw(NonNull::new_unchecked(out as *mut CFArray<CFType>)) };
    array
        .iter()
        .filter_map(|name| name.downcast_ref::<CFString>().map(|s| s.to_string()))
        .collect()
}

fn string(value: &Option<CFRetained<CFType>>) -> Option<String> {
    let text = value.as_ref()?.downcast_ref::<CFString>()?.to_string();
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn raw_string(value: &Option<CFRetained<CFType>>) -> Option<String> {
    Some(value.as_ref()?.downcast_ref::<CFString>()?.to_string())
}

fn boolean(value: &Option<CFRetained<CFType>>) -> Option<bool> {
    let value = value.as_ref()?;
    if let Some(flag) = value.downcast_ref::<CFBoolean>() {
        return Some(flag.as_bool());
    }
    value.downcast_ref::<CFNumber>()?.as_i64().map(|n| n != 0)
}

fn number(value: &Option<CFRetained<CFType>>) -> Option<f64> {
    value.as_ref()?.downcast_ref::<CFNumber>()?.as_f64()
}

fn ax_value<T: Copy>(value: &Option<CFRetained<CFType>>, kind: AXValueType, zero: T) -> Option<T> {
    let ax = value.as_ref()?.downcast_ref::<AXValue>()?;
    let mut out = zero;
    unsafe { ax.value(kind, NonNull::from(&mut out).cast()) }.then_some(out)
}

#[allow(clippy::too_many_arguments)]
fn read_node(
    element: &AXUIElement,
    values: &[Option<CFRetained<CFType>>],
    index: u32,
    parent_index: Option<u32>,
    depth: u32,
    object_ref: &str,
    child_count: usize,
    is_focused: bool,
) -> AccessibilityNode {
    let v = |attr: Attr| &values[attr as usize];
    let ax_role = string(v(Attr::Role)).unwrap_or_else(|| "AXUnknown".into());
    let subrole = string(v(Attr::Subrole));
    let role = atspi_role(&ax_role, subrole.as_deref()).to_string();
    let title = string(v(Attr::Title));
    let ax_description = string(v(Attr::Description));
    let help = string(v(Attr::Help));
    let value_text = string(v(Attr::Value));
    let value_number = number(v(Attr::Value));
    let is_text_input = matches!(
        ax_role.as_str(),
        "AXTextField" | "AXTextArea" | "AXComboBox" | "AXSearchField"
    );
    let is_toggle = matches!(ax_role.as_str(), "AXCheckBox" | "AXRadioButton");

    // AT-SPI names a label by its text and a button by its label; AX puts
    // the first in AXValue and the second in AXDescription when untitled.
    let name = match ax_role.as_str() {
        "AXStaticText" => value_text.clone().or(title.clone()),
        _ => title.clone().or(ax_description.clone()),
    };
    let description = match title {
        Some(_) => ax_description.or(help),
        None => help,
    };

    let mut bounds = match (
        ax_value(v(Attr::Position), AXValueType::CGPoint, CGPoint::ZERO),
        ax_value(v(Attr::Size), AXValueType::CGSize, CGSize::ZERO),
    ) {
        (Some(p), Some(s)) if s.width > 0.0 && s.height > 0.0 => Some(Bounds {
            x: p.x.round() as i32,
            y: p.y.round() as i32,
            width: s.width.round() as i32,
            height: s.height.round() as i32,
        }),
        _ => None,
    };

    // WebKit and Chromium report a web area at the page's full height;
    // AT-SPI documents are the viewport, which the shared page view uses as
    // the visible area. Clip it to the scroll area holding it.
    if ax_role == "AXWebArea" {
        if let (Some(page), Some(viewport)) = (bounds.as_ref(), parent_frame(element)) {
            bounds = crate::page_view::intersect(page, &viewport);
        }
    }

    let mut states = Vec::new();
    let enabled = boolean(v(Attr::Enabled)) != Some(false);
    if enabled {
        states.extend(["enabled", "sensitive"].map(String::from));
    }
    if bounds.is_some() {
        states.extend(["visible", "showing"].map(String::from));
    }
    if boolean(v(Attr::Selected)) == Some(true) {
        states.push("selected".into());
    }
    if boolean(v(Attr::Modal)) == Some(true) {
        states.push("modal".into());
    }
    if is_focused {
        states.extend(["focused", "focusable"].map(String::from));
    }
    // AXExpanded=false is reported by nearly everything; only these expand.
    let expandable = matches!(
        ax_role.as_str(),
        "AXRow" | "AXDisclosureTriangle" | "AXPopUpButton" | "AXComboBox" | "AXMenuBarItem"
    );
    match boolean(v(Attr::Expanded)).filter(|_| expandable) {
        Some(true) => states.push("expanded".into()),
        Some(false) => states.push("collapsed".into()),
        None => {}
    }
    if is_toggle {
        states.push("checkable".into());
        if value_number == Some(1.0) {
            states.push("checked".into());
        }
        if value_number == Some(2.0) {
            states.push("indeterminate".into());
        }
    }
    if is_text_input && enabled {
        states.push("editable".into());
    }

    let actions = action_names(element)
        .into_iter()
        .filter_map(|ax| atspi_action(&ax).map(|name| (ax, name)))
        .enumerate()
        .map(|(index, (ax, name))| AccessibilityAction {
            index: index as i32,
            name: name.to_string(),
            description: ax,
            keybinding: String::new(),
        })
        .collect();

    let numeric_value = matches!(
        ax_role.as_str(),
        "AXSlider"
            | "AXProgressIndicator"
            | "AXLevelIndicator"
            | "AXIncrementor"
            | "AXValueIndicator"
    );
    let value = value_number
        .filter(|_| numeric_value)
        .map(|current| AccessibilityValue {
            current,
            minimum: number(v(Attr::MinValue)).unwrap_or(0.0),
            maximum: number(v(Attr::MaxValue)).unwrap_or(0.0),
            minimum_increment: 0.0,
            text: None,
        });
    // Text inputs carry their contents; pop-up buttons their chosen option.
    let text_content = if is_text_input {
        raw_string(v(Attr::Value))
    } else if ax_role == "AXPopUpButton" {
        value_text.clone()
    } else {
        None
    };
    let text = text_content.map(|content| {
        let count = content.chars().count();
        let range = ax_value(
            v(Attr::SelectedTextRange),
            AXValueType::CFRange,
            CFRange {
                location: 0,
                length: 0,
            },
        );
        AccessibilityText {
            character_count: count as i32,
            caret_offset: range.map(|range| (range.location + range.length) as i32),
            truncated: count > MAX_TEXT_CHARS,
            content: Some(content.chars().take(MAX_TEXT_CHARS).collect()),
            selections: range
                .filter(|range| range.length > 0)
                .map(|range| AccessibilityTextSelection {
                    start_offset: range.location as i32,
                    end_offset: (range.location + range.length) as i32,
                })
                .into_iter()
                .collect(),
        }
    });

    AccessibilityNode {
        index,
        parent_index,
        depth,
        object_ref: object_ref.to_string(),
        role,
        name,
        description,
        child_count: child_count as i32,
        bounds,
        states,
        actions,
        value,
        text,
        supports_editable_text: is_text_input && enabled,
    }
}

/// AX role (and subrole) as the AT-SPI role name the shared outline and page
/// view key on (`document web`, `frame`, `entry`, `push button`, ...).
fn atspi_role(role: &str, subrole: Option<&str>) -> &'static str {
    match (role, subrole) {
        ("AXWindow", Some("AXDialog" | "AXSystemDialog")) | ("AXSheet", _) => "dialog",
        ("AXWindow", _) => "frame",
        ("AXApplication", _) => "application",
        ("AXWebArea", _) => "document web",
        ("AXButton", _) | ("AXMenuButton", _) => "push button",
        ("AXRadioButton", Some("AXTabButton")) => "page tab",
        ("AXRadioButton", _) => "radio button",
        ("AXCheckBox", Some("AXToggle")) | ("AXDisclosureTriangle", _) => "toggle button",
        ("AXCheckBox", Some("AXSwitch")) => "switch",
        ("AXCheckBox", _) => "check box",
        ("AXPopUpButton", _) | ("AXComboBox", _) => "combo box",
        ("AXTextField", Some("AXSecureTextField")) => "password text",
        ("AXTextField", _) | ("AXTextArea", _) | ("AXSearchField", _) => "entry",
        ("AXStaticText", _) => "static",
        ("AXLink", _) => "link",
        ("AXImage", _) => "image",
        ("AXHeading", _) => "heading",
        ("AXGroup", Some("AXApplicationAlert" | "AXApplicationAlertDialog")) => "alert",
        ("AXGroup", Some("AXApplicationStatus")) => "status bar",
        ("AXGroup", Some("AXApplicationDialog")) => "dialog",
        ("AXGroup", Some("AXLandmarkMain" | "AXDocumentArticle")) => "section",
        ("AXGroup", _) | ("AXLayoutArea", _) | ("AXLayoutItem", _) | ("AXBrowser", _) => "panel",
        ("AXScrollArea", _) => "scroll pane",
        ("AXScrollBar", _) => "scroll bar",
        ("AXList", _) => "list",
        ("AXTable", _) => "table",
        ("AXOutline", _) => "tree table",
        ("AXRow", Some("AXOutlineRow")) => "tree item",
        ("AXRow", _) => "table row",
        ("AXCell", _) => "table cell",
        ("AXColumn", _) => "table column header",
        ("AXToolbar", _) => "tool bar",
        ("AXMenuBar", _) => "menu bar",
        ("AXMenuBarItem", _) | ("AXMenu", _) => "menu",
        ("AXMenuItem", _) => "menu item",
        ("AXTabGroup", _) => "page tab list",
        ("AXSplitGroup", _) => "split pane",
        ("AXSplitter", _) => "separator",
        ("AXProgressIndicator", _) | ("AXBusyIndicator", _) => "progress bar",
        ("AXSlider", _) => "slider",
        ("AXIncrementor", _) => "spin button",
        ("AXDateField", _) => "date editor",
        ("AXHelpTag", _) => "tool tip",
        ("AXUnknown", _) => UNKNOWN_ROLE,
        _ => "filler",
    }
}

/// AX action as the AT-SPI action name; `None` drops noise (AXCancel on every
/// menu, AXPick duplicating AXPress on menu items, AXScrollToVisible).
fn atspi_action(ax: &str) -> Option<&'static str> {
    Some(match ax {
        "AXPress" => "click",
        "AXShowMenu" => "showContextMenu",
        "AXIncrement" => "increment",
        "AXDecrement" => "decrement",
        "AXConfirm" => "confirm",
        "AXRaise" => "raise",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shared page view finds the window title, the web document and the
    /// address bar by these AT-SPI role names; AX roles must land on them.
    #[test]
    fn ax_roles_map_to_the_atspi_names_the_page_view_keys_on() {
        assert_eq!(atspi_role("AXWindow", Some("AXStandardWindow")), "frame");
        assert_eq!(atspi_role("AXWindow", Some("AXDialog")), "dialog");
        assert_eq!(atspi_role("AXWebArea", None), "document web");
        assert_eq!(atspi_role("AXTextField", None), "entry");
        assert_eq!(
            atspi_role("AXTextField", Some("AXSecureTextField")),
            "password text"
        );
        assert_eq!(atspi_role("AXRadioButton", Some("AXTabButton")), "page tab");
        assert_eq!(atspi_action("AXPress"), Some("click"));
        assert!(!crate::outline::is_specific_action(&AccessibilityAction {
            index: 0,
            name: atspi_action("AXShowMenu").unwrap().to_string(),
            description: String::new(),
            keybinding: String::new(),
        }));
        assert_eq!(atspi_action("AXScrollToVisible"), None);
    }

    #[test]
    fn actions_resolve_by_atspi_name_ax_name_or_alias() {
        let actions = ["AXShowMenu".to_string(), "AXPress".to_string()];
        assert_eq!(find_action(&actions, "click"), Some(1));
        assert_eq!(find_action(&actions, "AXPress"), Some(1));
        assert_eq!(find_action(&actions, "activate"), Some(1));
        assert_eq!(find_action(&actions, "showcontextmenu"), Some(0));
        assert_eq!(find_action(&actions, "toggle"), None);
    }
}
