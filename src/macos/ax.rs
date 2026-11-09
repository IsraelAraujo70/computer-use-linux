//! macOS platform calls: the AX tree of an app as neutral `AccessibilityNode`s
//! (roles, states and actions renamed to their AT-SPI equivalents so the
//! shared outline reads the same on both platforms), running apps from
//! AppKit, mouse clicks through CGEvent, and the TCC permission checks.
//!
//! Every AX call is a synchronous IPC to the target app, bounded by
//! `AX_MESSAGING_TIMEOUT` so a hung app cannot hang the server.

use crate::a11y::{
    AccessibilityAction, AccessibilityNode, AccessibilityText, AccessibilityValue, Bounds,
};
use anyhow::{bail, Result};
use objc2_app_kit::{NSApplicationActivationPolicy, NSWorkspace};
use objc2_application_services::{AXError, AXIsProcessTrusted, AXUIElement, AXValue, AXValueType};
use objc2_core_foundation::{
    CFArray, CFBoolean, CFNumber, CFRetained, CFString, CFType, CGPoint, CGSize,
};
use objc2_core_graphics::{
    CGDisplayBounds, CGEvent, CGEventField, CGEventTapLocation, CGEventType, CGMainDisplayID,
    CGMouseButton, CGPreflightScreenCaptureAccess,
};
use serde::Serialize;
use std::{collections::HashMap, ptr::NonNull};

const AX_MESSAGING_TIMEOUT_SECS: f32 = 1.5;
const MAX_TEXT_CHARS: usize = 4096;

/// Attributes read per element in one `AXUIElementCopyMultipleAttributeValues`
/// IPC. The order is the index into the returned array (`Attr`).
const ATTRIBUTES: [&str; 15] = [
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
}

/// A retained AX element. `AXUIElement` is an immutable reference to a remote
/// object; the server only touches it under its own mutex.
#[derive(Clone)]
pub struct Element(CFRetained<AXUIElement>);
// SAFETY: AXUIElementRef is an immutable CF object; the AX client calls made
// on it are IPC requests that Apple documents as callable from any thread.
unsafe impl Send for Element {}
unsafe impl Sync for Element {}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct RunningApp {
    pub name: String,
    pub bundle_id: Option<String>,
    pub pid: u32,
    pub active: bool,
    pub hidden: bool,
}

pub fn ax_trusted() -> bool {
    unsafe { AXIsProcessTrusted() }
}

pub fn screen_capture_allowed() -> bool {
    CGPreflightScreenCaptureAccess()
}

/// Regular (Dock) apps, frontmost first.
pub fn running_apps() -> Vec<RunningApp> {
    let workspace = NSWorkspace::sharedWorkspace();
    let mut apps: Vec<RunningApp> = workspace
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
            active: app.isActive(),
            hidden: app.isHidden(),
        })
        .collect();
    apps.sort_by_key(|app| !app.active);
    apps
}

/// The app an `app_name_or_bundle_identifier` / `pid` names: exact bundle id,
/// then name or bundle id substring (case-insensitive). No filter means the
/// frontmost app.
pub fn resolve_app(filter: Option<&str>, pid: Option<u32>) -> Result<RunningApp> {
    let apps = running_apps();
    if let Some(pid) = pid {
        return apps
            .into_iter()
            .find(|app| app.pid == pid)
            .ok_or_else(|| anyhow::anyhow!("no running app with pid {pid}"));
    }
    let Some(needle) = filter.map(str::trim).filter(|s| !s.is_empty()) else {
        return apps
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("no running apps"));
    };
    let needle = needle.to_lowercase();
    let exact = apps
        .iter()
        .position(|app| app.bundle_id.as_deref().map(str::to_lowercase) == Some(needle.clone()));
    let fuzzy = || {
        apps.iter().position(|app| {
            app.name.to_lowercase().contains(&needle)
                || app
                    .bundle_id
                    .as_deref()
                    .is_some_and(|id| id.to_lowercase().contains(&needle))
        })
    };
    match exact.or_else(fuzzy) {
        Some(at) => Ok(apps[at].clone()),
        None => bail!(
            "no running app matches {needle:?}; running: {}",
            apps.iter()
                .map(|app| app.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// Depth-first snapshot of `pid`'s AX tree. `index`/`parent_index` are
/// positions in the returned vec; `object_ref` is `pid:hash`, stable for the
/// element's lifetime, and `elements` maps it back to the element.
pub fn snapshot(
    pid: u32,
    max_nodes: usize,
    max_depth: u32,
) -> Result<(Vec<AccessibilityNode>, HashMap<String, Element>, bool)> {
    if !ax_trusted() {
        bail!("Accessibility permission missing for this binary (System Settings → Privacy & Security → Accessibility)");
    }
    let app = unsafe { AXUIElement::new_application(pid as libc::pid_t) };
    unsafe { app.set_messaging_timeout(AX_MESSAGING_TIMEOUT_SECS) };
    let names: Vec<CFRetained<CFString>> =
        ATTRIBUTES.iter().map(|a| CFString::from_str(a)).collect();
    let names = CFArray::from_retained_objects(&names);
    // AXFocused is true on every cell of a focused outline; the app's
    // focused element is the one AT-SPI would call focused.
    let focused = copy_attribute(&app, "AXFocusedUIElement")
        .and_then(|value| value.downcast::<AXUIElement>().ok());

    let mut nodes = Vec::new();
    let mut elements = HashMap::new();
    let mut truncated = false;
    // (element, parent position, depth); popped from the back, children pushed
    // reversed, so the order is depth-first pre-order.
    let mut stack: Vec<(CFRetained<AXUIElement>, Option<u32>, u32)> = vec![(app, None, 0)];
    while let Some((element, parent, depth)) = stack.pop() {
        if nodes.len() >= max_nodes {
            truncated = true;
            break;
        }
        let Some(values) = copy_values(&element, &names) else {
            continue;
        };

        let children: Vec<CFRetained<AXUIElement>> = values[Attr::Children as usize]
            .as_ref()
            .and_then(|value| value.downcast_ref::<CFArray>())
            .map(|array| {
                as_typed(array)
                    .iter()
                    .filter_map(|child| child.downcast::<AXUIElement>().ok())
                    .collect()
            })
            .unwrap_or_default();
        let position = nodes.len() as u32;
        let object_ref = format!("{pid}:{:x}", element_hash(&element));
        let is_focused = focused.as_deref().is_some_and(|focused| {
            AsRef::<CFType>::as_ref(focused) == AsRef::<CFType>::as_ref(&*element)
        });
        let node = read_node(
            &element,
            &values,
            position,
            parent,
            depth,
            &object_ref,
            children.len(),
            is_focused,
        );
        nodes.push(node);
        elements.insert(object_ref, Element(element));
        // A menu bar item lists its whole (closed) menu as children; read
        // them only while the menu is open (the item is selected).
        let closed_menu = string(&values[Attr::Role as usize]).as_deref() == Some("AXMenuBarItem")
            && boolean(&values[Attr::Selected as usize]) != Some(true);
        if closed_menu {
            continue;
        }
        if depth + 1 > max_depth {
            truncated |= !children.is_empty();
            continue;
        }
        for child in children.into_iter().rev() {
            stack.push((child, Some(position), depth + 1));
        }
    }
    if nodes.is_empty() {
        bail!(
            "AX returned nothing for pid {pid} (app not responding, or Accessibility not granted)"
        );
    }
    Ok((nodes, elements, truncated))
}

/// AX arrays hold CF objects; view an untyped one as such to iterate it.
fn as_typed(array: &CFArray) -> &CFArray<CFType> {
    // SAFETY: CFArray<T> is a transparent marker over the same CFArrayRef, and
    // every element of an AX attribute array is a CF object.
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

fn element_hash(element: &AXUIElement) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let cf: &CFType = element.as_ref();
    cf.hash(&mut hasher);
    hasher.finish()
}

/// One IPC for all `ATTRIBUTES`; a missing attribute comes back as an
/// AXValue of type AXError, mapped to `None`.
fn copy_values(
    element: &AXUIElement,
    names: &CFArray<CFString>,
) -> Option<Vec<Option<CFRetained<CFType>>>> {
    let mut out: *const CFArray = std::ptr::null();
    let error = unsafe {
        element.copy_multiple_attribute_values(
            names.as_ref(),
            objc2_application_services::AXCopyMultipleAttributeOptions(0),
            NonNull::from(&mut out),
        )
    };
    if error != AXError::Success || out.is_null() {
        return None;
    }
    let array: CFRetained<CFArray<CFType>> =
        unsafe { CFRetained::from_raw(NonNull::new_unchecked(out as *mut CFArray<CFType>)) };
    let values = (0..ATTRIBUTES.len())
        .map(|at| {
            let value: CFRetained<CFType> = array.get(at)?;
            match value.downcast_ref::<AXValue>() {
                Some(ax) if unsafe { ax.r#type() } == AXValueType::AXError => None,
                _ => Some(value),
            }
        })
        .collect();
    Some(values)
}

fn string(value: &Option<CFRetained<CFType>>) -> Option<String> {
    let text = value.as_ref()?.downcast_ref::<CFString>()?.to_string();
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
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

fn point(value: &Option<CFRetained<CFType>>) -> Option<CGPoint> {
    let ax = value.as_ref()?.downcast_ref::<AXValue>()?;
    let mut point = CGPoint::default();
    unsafe { ax.value(AXValueType::CGPoint, NonNull::from(&mut point).cast()) }.then_some(point)
}

fn size(value: &Option<CFRetained<CFType>>) -> Option<CGSize> {
    let ax = value.as_ref()?.downcast_ref::<AXValue>()?;
    let mut size = CGSize::default();
    unsafe { ax.value(AXValueType::CGSize, NonNull::from(&mut size).cast()) }.then_some(size)
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
        "AXTextField" | "AXTextArea" | "AXComboBox"
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

    let bounds = match (point(v(Attr::Position)), size(v(Attr::Size))) {
        (Some(p), Some(s)) if s.width > 0.0 && s.height > 0.0 => Some(Bounds {
            x: p.x.round() as i32,
            y: p.y.round() as i32,
            width: s.width.round() as i32,
            height: s.height.round() as i32,
        }),
        _ => None,
    };

    let mut states = Vec::new();
    if boolean(v(Attr::Enabled)) != Some(false) {
        states.extend(["enabled", "sensitive"].map(String::from));
    }
    if bounds.is_some() {
        states.extend(["visible", "showing"].map(String::from));
    }
    let mut flag = |attr: Attr, label: &str| {
        if boolean(v(attr)) == Some(true) {
            states.push(label.to_string());
        }
    };
    flag(Attr::Selected, "selected");
    flag(Attr::Modal, "modal");
    if is_focused {
        states.push("focused".into());
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
    if is_toggle && value_number == Some(1.0) {
        states.push("checked".into());
    }
    if is_text_input {
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
    let text = value_text.filter(|_| is_text_input).map(|content| {
        let count = content.chars().count();
        AccessibilityText {
            character_count: count as i32,
            caret_offset: None,
            truncated: count > MAX_TEXT_CHARS,
            content: Some(content.chars().take(MAX_TEXT_CHARS).collect()),
            selections: Vec::new(),
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
        supports_editable_text: is_text_input,
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
        ("AXTextField", _) | ("AXTextArea", _) => "entry",
        ("AXStaticText", _) => "static",
        ("AXLink", _) => "link",
        ("AXImage", _) => "image",
        ("AXHeading", _) => "heading",
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
        ("AXUnknown", _) => "unknown",
        _ => "filler",
    }
}

/// AX action as the AT-SPI action name; `None` drops noise (AXCancel on every
/// menu, AXPick duplicating AXPress on menu items, AXScrollToVisible).
/// AXShowMenu becomes `showContextMenu`, which the outline already hides.
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

/// AXPress on the element. `Ok(false)` when it has no AXPress.
pub fn press(element: &Element) -> Result<bool> {
    if !action_names(&element.0)
        .iter()
        .any(|name| name == "AXPress")
    {
        return Ok(false);
    }
    let error = unsafe { element.0.perform_action(&CFString::from_str("AXPress")) };
    if error != AXError::Success {
        bail!("AXPress failed: AXError {}", error.0);
    }
    Ok(true)
}

/// Left/right/middle click at global point coordinates (top-left origin),
/// `count` presses (2 = double click).
pub fn click_at(x: f64, y: f64, button: &str, count: u32) -> Result<()> {
    if !ax_trusted() {
        bail!("Accessibility permission missing: CGEvent input is dropped without it");
    }
    let (down, up, mouse) = match button {
        "left" | "" => (
            CGEventType::LeftMouseDown,
            CGEventType::LeftMouseUp,
            CGMouseButton::Left,
        ),
        "right" => (
            CGEventType::RightMouseDown,
            CGEventType::RightMouseUp,
            CGMouseButton::Right,
        ),
        "middle" => (
            CGEventType::OtherMouseDown,
            CGEventType::OtherMouseUp,
            CGMouseButton::Center,
        ),
        other => bail!("unknown button {other:?} (left, right, middle)"),
    };
    let at = CGPoint { x, y };
    let post = |kind: CGEventType, state: u32| -> Result<()> {
        let event = CGEvent::new_mouse_event(None, kind, at, mouse)
            .ok_or_else(|| anyhow::anyhow!("CGEventCreateMouseEvent failed"))?;
        CGEvent::set_integer_value_field(
            Some(&event),
            CGEventField::MouseEventClickState,
            state as i64,
        );
        CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&event));
        Ok(())
    };
    let moved = CGEvent::new_mouse_event(None, CGEventType::MouseMoved, at, CGMouseButton::Left);
    CGEvent::post(CGEventTapLocation::HIDEventTap, moved.as_deref());
    for state in 1..=count.max(1) {
        post(down, state)?;
        post(up, state)?;
    }
    Ok(())
}

/// Main display size in points (the space AX bounds and CGEvent use).
pub fn main_display_points() -> (u32, u32) {
    let bounds = CGDisplayBounds(CGMainDisplayID());
    (
        bounds.size.width.round() as u32,
        bounds.size.height.round() as u32,
    )
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
}
