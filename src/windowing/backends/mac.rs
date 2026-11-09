//! macOS window backend: windows from the window server
//! (`CGWindowListCopyWindowInfo`, front to back), focus through AppKit
//! activation plus `AXRaise` on the matching AX window.

use crate::windowing::registry::BackendProbe;
use crate::windowing::types::{WindowBounds, WindowInfo};
use anyhow::{bail, Result};
use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication};
use objc2_core_foundation::{
    CFArray, CFDictionary, CFNumber, CFRetained, CFString, CFType, CGRect,
};
use objc2_core_graphics::{
    kCGWindowAlpha, kCGWindowBounds, kCGWindowLayer, kCGWindowName, kCGWindowNumber,
    kCGWindowOwnerName, kCGWindowOwnerPID, CGDisplayBounds, CGGetActiveDisplayList,
    CGRectMakeWithDictionaryRepresentation, CGWindowListCopyWindowInfo, CGWindowListOption,
};

pub const MAC_BACKEND: &str = "macos";

/// One on-screen window as the window server reports it.
#[derive(Debug, Clone)]
pub(crate) struct CgWindow {
    pub id: u32,
    pub pid: u32,
    pub owner: String,
    pub title: Option<String>,
    pub bounds: (i32, i32, u32, u32),
    /// 0 for app windows; menus, the Dock and the menu bar sit above.
    pub layer: i64,
}

/// Visible on-screen windows, frontmost first. Titles need the Screen
/// Recording permission; without it they are `None`.
pub(crate) fn on_screen_windows() -> Vec<CgWindow> {
    let options =
        CGWindowListOption::OptionOnScreenOnly | CGWindowListOption::ExcludeDesktopElements;
    let Some(list) = CGWindowListCopyWindowInfo(options, 0) else {
        return Vec::new();
    };
    // SAFETY: the window list is an array of CFDictionary<CFString, CFType>.
    let list: &CFArray<CFDictionary<CFString, CFType>> =
        unsafe { &*(&*list as *const CFArray).cast() };
    list.iter()
        .filter_map(|info| {
            let get = |key: &CFString| info.get(key);
            let number = |key: &CFString| {
                get(key).and_then(|value| value.downcast_ref::<CFNumber>()?.as_f64())
            };
            let text = |key: &CFString| {
                get(key)
                    .and_then(|value| value.downcast_ref::<CFString>().map(|s| s.to_string()))
                    .filter(|s| !s.is_empty())
            };
            if number(unsafe { kCGWindowAlpha }) == Some(0.0) {
                return None;
            }
            let bounds = get(unsafe { kCGWindowBounds })?;
            let bounds: CFRetained<CFDictionary> = bounds.downcast().ok()?;
            let mut rect = CGRect::default();
            if !unsafe { CGRectMakeWithDictionaryRepresentation(Some(&bounds), &mut rect) } {
                return None;
            }
            if rect.size.width < 1.0 || rect.size.height < 1.0 {
                return None;
            }
            Some(CgWindow {
                id: number(unsafe { kCGWindowNumber })? as u32,
                pid: number(unsafe { kCGWindowOwnerPID })? as u32,
                owner: text(unsafe { kCGWindowOwnerName }).unwrap_or_default(),
                title: text(unsafe { kCGWindowName }),
                bounds: (
                    rect.origin.x.round() as i32,
                    rect.origin.y.round() as i32,
                    rect.size.width.round() as u32,
                    rect.size.height.round() as u32,
                ),
                layer: number(unsafe { kCGWindowLayer }).unwrap_or(0.0) as i64,
            })
        })
        .collect()
}

/// Active displays as `(x, y, width, height)` in global points.
pub(crate) fn displays() -> Vec<(i32, i32, u32, u32)> {
    let mut ids = [0u32; 16];
    let mut count = 0u32;
    unsafe { CGGetActiveDisplayList(ids.len() as u32, ids.as_mut_ptr(), &mut count) };
    ids[..count as usize]
        .iter()
        .map(|&id| {
            let rect = CGDisplayBounds(id);
            (
                rect.origin.x.round() as i32,
                rect.origin.y.round() as i32,
                rect.size.width.round() as u32,
                rect.size.height.round() as u32,
            )
        })
        .collect()
}

/// The desktop as screenshots and coordinates see it: from (0, 0) to the
/// far edge of every display. Displays left of or above the main one
/// (negative origins) fall outside it.
pub(crate) fn desktop_size() -> (u32, u32) {
    displays()
        .into_iter()
        .fold((0, 0), |(width, height), (x, y, w, h)| {
            (
                width.max((x + w as i32).max(0) as u32),
                height.max((y + h as i32).max(0) as u32),
            )
        })
}

pub fn probe() -> BackendProbe {
    let windows = on_screen_windows();
    let ok = !windows.is_empty();
    BackendProbe {
        id: MAC_BACKEND,
        ok,
        can_list_windows: ok,
        can_focus_windows: ok,
        detail: if ok {
            format!("{} on-screen windows from the window server", windows.len())
        } else {
            "the window server listed no windows".to_string()
        },
    }
}

/// App windows (layer 0), frontmost first; the frontmost app's first window
/// is the focused one.
pub async fn list_windows() -> Result<Vec<WindowInfo>> {
    tokio::task::spawn_blocking(|| {
        let frontmost = crate::macos::ax_tree::frontmost_pid();
        let mut focused_seen = false;
        on_screen_windows()
            .into_iter()
            .filter(|window| window.layer == 0)
            .map(|window| {
                let identity = crate::macos::ax_tree::app_identity(window.pid);
                let focused = !focused_seen && Some(window.pid) == frontmost;
                focused_seen |= focused;
                let (x, y, width, height) = window.bounds;
                WindowInfo {
                    window_id: u64::from(window.id),
                    title: window.title,
                    app_id: identity.as_ref().and_then(|(_, bundle)| bundle.clone()),
                    wm_class: Some(
                        identity
                            .map(|(name, _)| name)
                            .filter(|name| !name.is_empty())
                            .unwrap_or(window.owner),
                    ),
                    pid: Some(window.pid),
                    bounds: Some(WindowBounds {
                        x: Some(x),
                        y: Some(y),
                        width,
                        height,
                    }),
                    workspace: None,
                    focused,
                    hidden: false,
                    client_type: Some("macos".to_string()),
                    backend: MAC_BACKEND.to_string(),
                    terminal: None,
                }
            })
            .collect()
    })
    .await
    .map_err(Into::into)
}

/// Activates the window's app and raises that window above the app's
/// others.
pub async fn activate_window(window: &WindowInfo) -> Result<()> {
    let Some(pid) = window.pid else {
        bail!("window {} names no process", window.window_id);
    };
    let title = window.title.clone();
    let frame = window
        .bounds
        .as_ref()
        .and_then(|b| Some((b.x?, b.y?, b.width, b.height)));
    tokio::task::spawn_blocking(move || {
        let Some(app) =
            NSRunningApplication::runningApplicationWithProcessIdentifier(pid as libc::pid_t)
        else {
            bail!("no running app with pid {pid}");
        };
        crate::macos::ax_tree::raise_window(pid, title.as_deref(), frame)?;
        // Also ask AppKit; refused for background callers on macOS 14+,
        // honored on older systems.
        #[allow(deprecated)]
        app.activateWithOptions(NSApplicationActivationOptions::ActivateIgnoringOtherApps);
        Ok(())
    })
    .await?
}
