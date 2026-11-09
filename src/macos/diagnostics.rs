//! macOS `doctor`: the TCC grants this binary holds (Accessibility for the
//! AX tree and input, Screen Recording for screenshots and window titles),
//! the window server, displays and optional tesseract, with the same
//! `readiness` shape as Linux so agents read one contract.

use crate::windowing::backends::mac::{desktop_size, displays};
use crate::windowing::registry::probe_backends;
use schemars::JsonSchema;
use serde::Serialize;

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct DoctorReport {
    pub platform: PlatformReport,
    pub permissions: PermissionReport,
    pub commands: CommandReport,
    pub readiness: ReadinessReport,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct PlatformReport {
    pub os: String,
    pub os_version: Option<String>,
    pub arch: String,
    /// The binary TCC grants belong to when this process is its own
    /// responsible process (started by launchd, not by a terminal or app).
    pub executable: Option<String>,
    /// Active displays as `[x, y, width, height]` in points.
    pub displays: Vec<(i32, i32, u32, u32)>,
    /// The coordinate space screenshots and clicks share, from (0, 0).
    pub desktop_points: (u32, u32),
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct PermissionReport {
    pub accessibility: Check,
    pub screen_recording: Check,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct CommandReport {
    /// The window server backend probe.
    pub window_server: Check,
    pub screencapture: Check,
    pub tesseract: Check,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ReadinessReport {
    pub ready: bool,
    pub can_build_accessibility_tree: bool,
    pub can_query_windows: bool,
    pub can_focus_windows: bool,
    pub can_send_input: bool,
    pub can_capture_screenshots: bool,
    pub screenshot_capture_status: ScreenshotCaptureStatus,
    /// `tesseract` is installed, so `ocr` works. Optional; never a blocker.
    pub can_ocr: bool,
    pub recommended_next_step: String,
    pub blockers: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScreenshotCaptureStatus {
    Unavailable,
    Unverified,
    Verified,
    Failed,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct Check {
    pub ok: bool,
    pub detail: String,
}

const SCREENSHOT_FAILED: &str =
    "Screenshot capture failed; screenshot_error has screencapture's answer.";
const READY_NEXT_STEP: &str =
    "Ready: Accessibility, Screen Recording, window targeting and CGEvent input are available.";

impl ReadinessReport {
    pub(crate) fn record_screenshot_result(&mut self, succeeded: bool) {
        self.can_capture_screenshots = succeeded;
        self.screenshot_capture_status = if succeeded {
            ScreenshotCaptureStatus::Verified
        } else {
            ScreenshotCaptureStatus::Failed
        };
        self.blockers.retain(|blocker| blocker != SCREENSHOT_FAILED);
        if !succeeded {
            self.blockers.push(SCREENSHOT_FAILED.to_string());
        }
        self.settle();
    }

    fn settle(&mut self) {
        self.ready = self.blockers.is_empty();
        self.recommended_next_step = self
            .blockers
            .first()
            .cloned()
            .unwrap_or_else(|| READY_NEXT_STEP.to_string());
    }
}

/// Nothing to hydrate on macOS: there is no session bus.
pub fn hydrate_session_bus_env() {}

pub fn doctor_report() -> DoctorReport {
    let executable = std::env::current_exe()
        .ok()
        .map(|path| path.display().to_string());
    let trusted = unsafe { objc2_application_services::AXIsProcessTrusted() };
    let capture = objc2_core_graphics::CGPreflightScreenCaptureAccess();
    let windows = probe_backends();
    let can_list_windows = windows.iter().any(|probe| probe.can_list_windows);
    let grant = |pane: &str| {
        format!(
            "Grant {pane} to {} in System Settings → Privacy & Security → {pane}, and run it from launchd (scripts/macos-launchagent.sh) so the grant is its own; then restart the server.",
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
    let tesseract = which("tesseract");
    let mut readiness = ReadinessReport {
        ready: false,
        can_build_accessibility_tree: trusted,
        can_query_windows: can_list_windows,
        can_focus_windows: trusted && windows.iter().any(|probe| probe.can_focus_windows),
        can_send_input: trusted,
        can_capture_screenshots: capture,
        screenshot_capture_status: if capture {
            ScreenshotCaptureStatus::Unverified
        } else {
            ScreenshotCaptureStatus::Unavailable
        },
        can_ocr: tesseract.is_some(),
        recommended_next_step: String::new(),
        blockers,
    };
    readiness.settle();
    DoctorReport {
        platform: PlatformReport {
            os: std::env::consts::OS.to_string(),
            os_version: std::process::Command::new("/usr/bin/sw_vers")
                .arg("-productVersion")
                .output()
                .ok()
                .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string()),
            arch: std::env::consts::ARCH.to_string(),
            executable,
            displays: displays(),
            desktop_points: desktop_size(),
        },
        permissions: PermissionReport {
            accessibility: Check {
                ok: trusted,
                detail: format!("AXIsProcessTrusted={trusted}"),
            },
            screen_recording: Check {
                ok: capture,
                detail: format!("CGPreflightScreenCaptureAccess={capture}"),
            },
        },
        commands: CommandReport {
            window_server: windows
                .first()
                .map(|probe| Check {
                    ok: probe.ok,
                    detail: format!("{}: {}", probe.id, probe.detail),
                })
                .unwrap_or(Check {
                    ok: false,
                    detail: "no window backend".into(),
                }),
            screencapture: Check {
                ok: std::path::Path::new("/usr/sbin/screencapture").exists(),
                detail: "/usr/sbin/screencapture".to_string(),
            },
            tesseract: Check {
                ok: tesseract.is_some(),
                detail: tesseract
                    .unwrap_or_else(|| "not installed (optional: brew install tesseract)".into()),
            },
        },
        readiness,
    }
}

/// `command` on PATH or in Homebrew's prefixes (launchd jobs get a bare
/// PATH).
fn which(command: &str) -> Option<String> {
    let path = std::env::var("PATH").unwrap_or_default();
    let found = path
        .split(':')
        .chain(["/opt/homebrew/bin", "/usr/local/bin"])
        .map(|dir| std::path::Path::new(dir).join(command))
        .find(|candidate| candidate.is_file());
    found.map(|found| found.display().to_string())
}
