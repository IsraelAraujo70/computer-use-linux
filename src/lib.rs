#[path = "atspi_tree.rs"]
mod atspi_tree_impl;
mod cli;
mod command_runner;
#[path = "diagnostics.rs"]
mod diagnostics_impl;
mod element_ids;
mod fields;
mod ocr;
mod outline;
mod page_view;
#[path = "screenshot.rs"]
mod screenshot_impl;
mod server;
mod terminal;
mod windowing;
mod windows;
mod x11_clipboard;
mod x11_display;

pub mod atspi_tree {
    pub(crate) use crate::atspi_tree_impl::{
        accessibility_chrome_context, grab_focus, invoke_preferred_action, list_accessible_apps,
        object_ref_owner_pid, perform_action, perform_named_action, probe_focused_element,
        read_element, scroll_into_view, set_element_value, snapshot_accessibility_tree,
        snapshot_limits, AccessibleAppSummary, FocusProbe, FocusedElementSummary,
        ValueSetInvocation,
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
