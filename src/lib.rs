mod a11y;
#[cfg(target_os = "linux")]
#[path = "atspi_tree.rs"]
mod atspi_tree_impl;
#[cfg(target_os = "linux")]
mod cli;
#[cfg(target_os = "linux")]
mod command_runner;
#[cfg(target_os = "linux")]
#[path = "diagnostics.rs"]
mod diagnostics_impl;
#[cfg_attr(target_os = "macos", allow(dead_code))]
mod element_ids;
#[cfg_attr(target_os = "macos", allow(dead_code))]
mod fields;
#[cfg(target_os = "linux")]
mod input_lock;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "linux")]
mod ocr;
#[cfg_attr(target_os = "macos", allow(dead_code))]
mod outline;
#[cfg_attr(target_os = "macos", allow(dead_code))]
mod page_view;
#[cfg_attr(target_os = "macos", allow(dead_code))]
#[path = "screenshot.rs"]
mod screenshot_impl;
#[cfg(target_os = "linux")]
mod server;
#[cfg(target_os = "linux")]
mod terminal;
#[cfg(target_os = "linux")]
mod windowing;
#[cfg(target_os = "linux")]
mod windows;
#[cfg(target_os = "linux")]
mod x11_clipboard;
#[cfg(target_os = "linux")]
mod x11_display;
#[cfg(target_os = "linux")]
mod x11_dnd;

#[cfg(target_os = "linux")]
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

#[cfg(target_os = "linux")]
pub mod diagnostics {
    pub use crate::diagnostics_impl::{
        doctor_report, hydrate_session_bus_env, Check, DoctorReport, ReadinessReport,
        ScreenshotCaptureStatus,
    };
}

pub mod screenshot {
    #[cfg(target_os = "linux")]
    pub(crate) use crate::screenshot_impl::capture_screenshot;
    #[cfg(target_os = "linux")]
    pub use crate::screenshot_impl::capture_screenshot_raw;
    pub use crate::screenshot_impl::RawScreenshotCapture;
    pub(crate) use crate::screenshot_impl::{
        prepare_screenshot_payload, ScreenshotCapture, ScreenshotOutputFormat,
        ScreenshotPayloadOptions,
    };
}

#[doc(hidden)]
pub async fn run_cli_from_env() -> anyhow::Result<()> {
    #[cfg(target_os = "linux")]
    return cli::run_from_env().await;
    #[cfg(target_os = "macos")]
    return macos::run_from_env().await;
}
