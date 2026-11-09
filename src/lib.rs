mod a11y;
mod annotate;
mod apps;
#[cfg(target_os = "linux")]
#[path = "atspi_tree.rs"]
mod atspi_tree_impl;
mod cli;
#[cfg_attr(target_os = "macos", allow(dead_code))]
mod command_runner;
#[cfg(target_os = "linux")]
#[path = "diagnostics.rs"]
mod diagnostics_impl;
mod element_ids;
#[cfg_attr(target_os = "macos", allow(dead_code))]
mod fields;
#[cfg_attr(target_os = "macos", allow(dead_code))]
mod input_lock;
#[cfg(target_os = "macos")]
mod macos;
mod ocr;
mod outline;
mod page_view;
mod scrape;
#[path = "screenshot.rs"]
mod screenshot_impl;
mod server;
#[cfg_attr(target_os = "macos", allow(dead_code))]
mod terminal;
mod windowing;
mod windows;
#[cfg(target_os = "linux")]
mod x11_clipboard;
#[cfg(target_os = "linux")]
mod x11_display;
#[cfg(target_os = "linux")]
mod x11_dnd;

// macOS implementations behind the module names the server uses.
#[cfg(target_os = "macos")]
use macos::{
    ax_tree as atspi_tree_impl, clipboard as x11_clipboard, diagnostics as diagnostics_impl,
    display as x11_display, dnd as x11_dnd,
};

pub mod atspi_tree {
    pub(crate) use crate::atspi_tree_impl::{
        accessibility_chrome_context, grab_focus, invoke_preferred_action, list_accessible_apps,
        object_ref_owner_pid, parent_ref, perform_action, perform_named_action,
        probe_focused_element, read_element, read_subtree, scroll_into_view, set_element_value,
        snapshot_accessibility_tree, snapshot_limits, snapshot_subtree, AccessibleAppSummary,
        FocusProbe, FocusedElementSummary, ValueSetInvocation,
    };
    pub use crate::atspi_tree_impl::{
        snapshot_tree, AccessibilityAction, AccessibilityNode, AccessibilityText,
        AccessibilityTextSelection, AccessibilityValue, Bounds,
    };
}

pub mod diagnostics {
    pub use crate::diagnostics_impl::{
        doctor_report, hydrate_session_bus_env, Check, DoctorReport, ReadinessReport,
        ScreenshotCaptureStatus,
    };
}

pub mod screenshot {
    pub(crate) use crate::screenshot_impl::{
        capture_screenshot, prepare_screenshot_payload, ScreenshotCapture, ScreenshotOutputFormat,
        ScreenshotPayloadOptions,
    };
    pub use crate::screenshot_impl::{capture_screenshot_raw, RawScreenshotCapture};
}

#[doc(hidden)]
pub async fn run_cli_from_env() -> anyhow::Result<()> {
    cli::run_from_env().await
}
