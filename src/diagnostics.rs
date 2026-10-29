//! `doctor` and the readiness block: X display, window manager IPC, AT-SPI
//! bus and the input/OCR commands this server shells out to.

use crate::windowing::registry::{self, I3_BACKEND, X11_BACKEND};
use schemars::JsonSchema;
use serde::Serialize;
use std::{
    collections::{BTreeMap, HashMap},
    env, fs,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    process::Command,
    sync::Mutex,
};

const DESKTOP_ENV_KEYS: &[&str] = &[
    "DBUS_SESSION_BUS_ADDRESS",
    "DESKTOP_SESSION",
    "DISPLAY",
    "XAUTHORITY",
    "XDG_SESSION_DESKTOP",
    "XDG_CURRENT_DESKTOP",
    "XDG_RUNTIME_DIR",
    "XDG_SESSION_TYPE",
];

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct DoctorReport {
    pub platform: PlatformReport,
    pub accessibility: AccessibilityReport,
    pub windowing: WindowingReport,
    pub commands: CommandReport,
    pub readiness: ReadinessReport,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct PlatformReport {
    pub os: String,
    pub arch: String,
    pub display: Option<String>,
    pub xdg_session_type: Option<String>,
    pub xdg_current_desktop: Option<String>,
    pub i3sock: Option<String>,
    pub xauthority: Option<String>,
    pub dbus_session_bus_address: Option<String>,
    pub xdg_runtime_dir: Option<String>,
    /// Connection to the X server named by `DISPLAY`.
    pub x11_display: Check,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct AccessibilityReport {
    pub at_spi_bus: Check,
    pub at_spi_enabled: Check,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct WindowingReport {
    pub backends: BTreeMap<String, Check>,
    pub can_list_windows: bool,
    pub can_focus_windows: bool,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct CommandReport {
    /// XTEST pointer and keyboard input.
    pub xdotool: Check,
    /// Optional, for the `ocr` tool.
    pub tesseract: Check,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ReadinessReport {
    /// True when there are no blockers.
    pub ready: bool,
    pub can_build_accessibility_tree: bool,
    pub can_query_windows: bool,
    pub can_focus_windows: bool,
    pub can_send_input: bool,
    /// A raw screenshot succeeded. Doctor alone does not capture, so this
    /// stays false until screenshot or get_app_state runs.
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
    /// The X display is unreachable; no capture was attempted.
    Unavailable,
    /// The X display is reachable; no capture was attempted.
    Unverified,
    /// A raw screenshot was captured successfully.
    Verified,
    /// The last raw capture failed.
    Failed,
}

const SCREENSHOT_FAILED: &str = "Screenshot capture failed; screenshot_error has the X server's answer. Check that DISPLAY names the running Xvfb.";
const READY_NEXT_STEP: &str =
    "Ready: X display, AT-SPI, window targeting and xdotool input are available.";

impl ReadinessReport {
    /// Record only the raw backend capture, before crop or encoding can fail.
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

    /// Derive `ready` and the next step from the blockers.
    fn settle(&mut self) {
        self.ready = self.blockers.is_empty();
        self.recommended_next_step = self
            .blockers
            .first()
            .cloned()
            .unwrap_or_else(|| READY_NEXT_STEP.to_string());
    }
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct Check {
    pub ok: bool,
    pub detail: String,
}

impl Check {
    fn ok(detail: impl Into<String>) -> Self {
        Self {
            ok: true,
            detail: detail.into(),
        }
    }

    fn fail(detail: impl Into<String>) -> Self {
        Self {
            ok: false,
            detail: detail.into(),
        }
    }
}

pub fn doctor_report() -> DoctorReport {
    hydrate_session_bus_env();

    let platform = platform_report();
    let accessibility = AccessibilityReport {
        at_spi_bus: atspi_bus_address_check(),
        at_spi_enabled: at_spi_enabled_check(),
    };
    let windowing = windowing_report();
    let commands = CommandReport {
        xdotool: command_path_check("xdotool"),
        tesseract: command_path_check("tesseract"),
    };
    let readiness = readiness_report(&platform, &accessibility, &windowing, &commands);

    DoctorReport {
        platform,
        accessibility,
        windowing,
        commands,
        readiness,
    }
}

pub fn hydrate_session_bus_env() {
    static HYDRATED_ENV: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);
    let mut hydrated = HYDRATED_ENV
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let current = desktop_environment();
    if hydrated.as_ref() == Some(&current) {
        return;
    }
    hydrate_common_command_path();
    hydrate_desktop_env_from_process_tree();
    let systemd_discovery_succeeded = hydrate_desktop_env_from_systemd_user();

    if env_var("XDG_RUNTIME_DIR").is_none() {
        if let Some(runtime) = xdg_runtime_dir() {
            if runtime.exists() {
                env::set_var("XDG_RUNTIME_DIR", runtime);
            }
        }
    }

    if env_var("DBUS_SESSION_BUS_ADDRESS").is_none() {
        if let Some(runtime) = xdg_runtime_dir() {
            let bus = runtime.join("bus");
            if bus.exists() {
                env::set_var(
                    "DBUS_SESSION_BUS_ADDRESS",
                    format!("unix:path={}", bus.display()),
                );
            }
        }
    }
    let current = desktop_environment();
    if systemd_discovery_succeeded
        && current.contains_key("DBUS_SESSION_BUS_ADDRESS")
        && current.contains_key("XDG_RUNTIME_DIR")
        && current.contains_key("XDG_CURRENT_DESKTOP")
        && current.contains_key("XDG_SESSION_TYPE")
        && process_env_has_graphical_display(&current)
    {
        *hydrated = Some(current);
    }
}

fn desktop_environment() -> HashMap<String, String> {
    DESKTOP_ENV_KEYS
        .iter()
        .copied()
        .chain(["PATH"])
        .filter_map(|key| env_var(key).map(|value| (key.to_string(), value)))
        .collect()
}

fn hydrate_common_command_path() {
    let mut entries = env::var_os("PATH")
        .map(|path| env::split_paths(&path).collect::<Vec<_>>())
        .unwrap_or_default();
    for path in [
        "/run/current-system/sw/bin",
        "/usr/local/bin",
        "/usr/bin",
        "/bin",
    ] {
        let path = PathBuf::from(path);
        if path.exists() && !entries.iter().any(|entry| entry == &path) {
            entries.push(path);
        }
    }
    if let Ok(path) = env::join_paths(entries) {
        env::set_var("PATH", path);
    }
}

fn hydrate_desktop_env_from_process_tree() {
    for process_env in desktop_process_environments() {
        hydrate_desktop_env_from_map(&process_env);

        if DESKTOP_ENV_KEYS.iter().all(|key| env_var(key).is_some()) {
            break;
        }
    }
}

fn hydrate_desktop_env_from_systemd_user() -> bool {
    let mut command = Command::new("systemctl");
    command.args(["--user", "show-environment"]);
    let Ok(output) =
        crate::command_runner::output_blocking(&mut command, "read systemd user environment")
    else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    let env_map = parse_line_environment(&output.stdout);
    hydrate_desktop_env_from_map(&env_map);
    true
}

fn hydrate_desktop_env_from_map(process_env: &HashMap<String, String>) {
    let current_env = DESKTOP_ENV_KEYS
        .iter()
        .filter_map(|key| env_var(key).map(|value| ((*key).to_string(), value)))
        .collect();
    for (key, value) in desktop_env_hydration_updates(&current_env, process_env) {
        env::set_var(key, value);
    }
}

fn desktop_env_hydration_updates(
    current_env: &HashMap<String, String>,
    source_env: &HashMap<String, String>,
) -> Vec<(&'static str, String)> {
    DESKTOP_ENV_KEYS
        .iter()
        .filter_map(|key| {
            if current_env
                .get(*key)
                .is_some_and(|value| !value.trim().is_empty())
            {
                return None;
            }
            source_env
                .get(*key)
                .filter(|value| !value.trim().is_empty())
                .map(|value| (*key, value.clone()))
        })
        .collect()
}

fn desktop_process_environments() -> Vec<HashMap<String, String>> {
    let mut environments = Vec::new();
    let mut visited_pids = Vec::new();
    let mut pid = parent_pid("self");

    for _ in 0..8 {
        let Some(current_pid) = pid else {
            break;
        };
        if current_pid <= 1 {
            break;
        }

        visited_pids.push(current_pid);
        if let Some(process_env) = read_process_environ(current_pid) {
            environments.push(process_env);
        }
        pid = parent_pid(&current_pid.to_string());
    }

    if !visited_pids.contains(&1) && process_owner_matches_current_user(1) {
        if let Some(process_env) = read_process_environ(1).filter(process_env_has_graphical_display)
        {
            environments.push(process_env);
        }
    }

    environments
}

fn parent_pid(pid: &str) -> Option<u32> {
    let status = fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    parse_parent_pid(&status)
}

fn parse_parent_pid(status: &str) -> Option<u32> {
    status.lines().find_map(|line| {
        let value = line.strip_prefix("PPid:")?.trim();
        value.parse::<u32>().ok()
    })
}

fn read_process_environ(pid: u32) -> Option<HashMap<String, String>> {
    let bytes = fs::read(format!("/proc/{pid}/environ")).ok()?;
    Some(parse_environ(&bytes))
}

fn process_owner_matches_current_user(pid: u32) -> bool {
    let Some(current_uid) = user_id().and_then(|uid| uid.parse::<u32>().ok()) else {
        return false;
    };
    fs::metadata(format!("/proc/{pid}"))
        .ok()
        .is_some_and(|metadata| metadata.uid() == current_uid)
}

fn process_env_has_graphical_display(process_env: &HashMap<String, String>) -> bool {
    process_env
        .get("DISPLAY")
        .is_some_and(|value| !value.trim().is_empty())
}

fn parse_environ(bytes: &[u8]) -> HashMap<String, String> {
    bytes
        .split(|byte| *byte == 0)
        .filter_map(|entry| {
            if entry.is_empty() {
                return None;
            }
            let split = entry.iter().position(|byte| *byte == b'=')?;
            let (key, value) = entry.split_at(split);
            let value = &value[1..];
            let key = std::str::from_utf8(key).ok()?.to_string();
            let value = std::str::from_utf8(value).ok()?.to_string();
            Some((key, value))
        })
        .collect()
}

fn parse_line_environment(bytes: &[u8]) -> HashMap<String, String> {
    bytes
        .split(|byte| *byte == b'\n')
        .filter_map(|entry| {
            if entry.is_empty() {
                return None;
            }
            let split = entry.iter().position(|byte| *byte == b'=')?;
            let (key, value) = entry.split_at(split);
            let value = &value[1..];
            let key = std::str::from_utf8(key).ok()?.to_string();
            let value = std::str::from_utf8(value).ok()?.to_string();
            Some((key, value))
        })
        .collect()
}

fn platform_report() -> PlatformReport {
    PlatformReport {
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        display: env_var("DISPLAY"),
        xdg_session_type: env_var("XDG_SESSION_TYPE"),
        xdg_current_desktop: env_var("XDG_CURRENT_DESKTOP"),
        i3sock: env_var("I3SOCK"),
        xauthority: env_var("XAUTHORITY"),
        dbus_session_bus_address: dbus_session_address(),
        xdg_runtime_dir: xdg_runtime_dir().map(|path| path.display().to_string()),
        x11_display: x11_display_check(),
    }
}

fn x11_display_check() -> Check {
    let Some(display) = env_var("DISPLAY") else {
        return Check::fail("DISPLAY is not set");
    };
    if !crate::x11_display::is_native_x11_session() {
        return Check::fail(format!(
            "DISPLAY={display} is set, but XDG_SESSION_TYPE is not x11"
        ));
    }
    // The transport enforces the deadline, so no detached worker remains
    // blocked after a diagnostic timeout.
    match crate::x11_display::X11Display::connect().map(|display| display.describe()) {
        Ok(detail) => Check::ok(detail),
        Err(error) => Check::fail(format!("{error:#}")),
    }
}

fn windowing_report() -> WindowingReport {
    let probes = registry::probe_backends();
    WindowingReport {
        backends: probes
            .iter()
            .map(|probe| (probe.id.to_string(), check_from_backend_probe(probe)))
            .collect(),
        can_list_windows: probes.iter().any(|probe| probe.can_list_windows),
        can_focus_windows: probes.iter().any(|probe| probe.can_focus_windows),
    }
}

fn check_from_backend_probe(probe: &registry::BackendProbe) -> Check {
    if probe.ok {
        Check::ok(probe.detail.clone())
    } else {
        Check::fail(probe.detail.clone())
    }
}

fn readiness_report(
    platform: &PlatformReport,
    accessibility: &AccessibilityReport,
    windowing: &WindowingReport,
    commands: &CommandReport,
) -> ReadinessReport {
    let mut blockers = Vec::new();
    let display_ok = platform.x11_display.ok;
    let can_build_accessibility_tree = can_build_accessibility_tree(accessibility);
    let can_query_windows = windowing.can_list_windows;
    let can_focus_windows = windowing.can_focus_windows;
    let can_send_input = display_ok && commands.xdotool.ok;

    if !display_ok {
        blockers.push(match platform.display.as_deref() {
            None => "DISPLAY is not set. Export DISPLAY=:N for the Xvfb display (and XDG_SESSION_TYPE=x11).".to_string(),
            Some(display) => format!(
                "X display {display} is unreachable ({}). Start Xvfb on {display} or export DISPLAY for the running X server.",
                platform.x11_display.detail
            ),
        });
    }
    if !accessibility.at_spi_bus.ok {
        blockers.push(format!(
            "AT-SPI bus is unreachable ({}). Export DBUS_SESSION_BUS_ADDRESS for the session bus of this display; at-spi2-core must be installed so the bus can start.",
            accessibility.at_spi_bus.detail
        ));
    } else if !can_build_accessibility_tree {
        blockers.push(
            "AT-SPI is disabled. Run: busctl --user set-property org.a11y.Bus /org/a11y/bus org.a11y.Status IsEnabled b true, then restart the target app."
                .to_string(),
        );
    }
    if !can_query_windows {
        blockers.push(format!(
            "Window listing failed ({}). Start i3 on this DISPLAY and export I3SOCK for its IPC socket.",
            windowing
                .backends
                .get(I3_BACKEND)
                .map_or("i3 probe did not run", |check| check.detail.as_str())
        ));
    } else if !can_focus_windows {
        blockers.push(format!(
            "Windows can be listed but focus cannot be verified ({}). Install xprop (x11-utils).",
            windowing
                .backends
                .get(X11_BACKEND)
                .map_or("x11 probe did not run", |check| check.detail.as_str())
        ));
    }
    if !commands.xdotool.ok {
        blockers.push(
            "xdotool is not installed; click, scroll, drag, type_text and press_key need it. Install xdotool."
                .to_string(),
        );
    }
    let mut readiness = ReadinessReport {
        ready: false,
        can_build_accessibility_tree,
        can_query_windows,
        can_focus_windows,
        can_send_input,
        can_capture_screenshots: false,
        screenshot_capture_status: if display_ok {
            ScreenshotCaptureStatus::Unverified
        } else {
            ScreenshotCaptureStatus::Unavailable
        },
        can_ocr: commands.tesseract.ok,
        recommended_next_step: String::new(),
        blockers,
    };
    readiness.settle();
    readiness
}

fn can_build_accessibility_tree(accessibility: &AccessibilityReport) -> bool {
    accessibility.at_spi_bus.ok && check_detail_contains_true(&accessibility.at_spi_enabled)
}

fn check_detail_contains_true(check: &Check) -> bool {
    check.ok && check.detail.to_ascii_lowercase().contains("true")
}

fn env_var(key: &str) -> Option<String> {
    env::var(key).ok().filter(|value| !value.trim().is_empty())
}

fn xdg_runtime_dir() -> Option<PathBuf> {
    if let Some(value) = env_var("XDG_RUNTIME_DIR") {
        return Some(PathBuf::from(value));
    }
    user_id().map(|uid| PathBuf::from(format!("/run/user/{uid}")))
}

fn dbus_session_address() -> Option<String> {
    if let Some(value) = env_var("DBUS_SESSION_BUS_ADDRESS") {
        return Some(value);
    }
    xdg_runtime_dir()
        .map(|runtime| format!("unix:path={}", runtime.join("bus").display()))
        .filter(|address| {
            address
                .strip_prefix("unix:path=")
                .is_some_and(|p| Path::new(p).exists())
        })
}

fn user_id() -> Option<String> {
    let mut command = Command::new("id");
    command.arg("-u");
    let output = crate::command_runner::output_blocking(&mut command, "query user id").ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|value| !value.is_empty())
}

fn command_path_check(command: &str) -> Check {
    let check = command_check("sh", &["-c", &format!("command -v {command}")]);
    if check.ok {
        check
    } else {
        Check::fail(format!("{command} not found on PATH"))
    }
}

fn atspi_bus_address_check() -> Check {
    let busctl = command_check_with_session_bus(
        "busctl",
        &[
            "--user",
            "call",
            "org.a11y.Bus",
            "/org/a11y/bus",
            "org.a11y.Bus",
            "GetAddress",
        ],
    );
    if busctl.ok {
        return busctl;
    }

    gdbus_call_check(
        "org.a11y.Bus",
        "/org/a11y/bus",
        "org.a11y.Bus.GetAddress",
        &[],
    )
}

fn atspi_status_property_check(property: &str) -> Check {
    let busctl = command_check_with_session_bus(
        "busctl",
        &[
            "--user",
            "get-property",
            "org.a11y.Bus",
            "/org/a11y/bus",
            "org.a11y.Status",
            property,
        ],
    );
    if busctl.ok {
        return busctl;
    }

    gdbus_call_check(
        "org.a11y.Bus",
        "/org/a11y/bus",
        "org.freedesktop.DBus.Properties.Get",
        &["org.a11y.Status", property],
    )
}

// check_detail_contains_true reads this detail, so keep "true" in it.
fn at_spi_enabled_check() -> Check {
    describe_bool(
        atspi_status_property_check("IsEnabled"),
        "AT-SPI IsEnabled is true",
        "AT-SPI IsEnabled is false",
    )
}

// Replaces raw busctl or gdbus boolean output with text.
fn describe_bool(check: Check, on: &str, off: &str) -> Check {
    let detail = check.detail.to_ascii_lowercase();
    match (check.ok, detail.contains("true"), detail.contains("false")) {
        (true, true, false) => Check::ok(on),
        (true, false, true) => Check::ok(off),
        _ => check,
    }
}

fn gdbus_call_check(destination: &str, object_path: &str, method: &str, args: &[&str]) -> Check {
    let mut command_args = vec![
        "call",
        "--session",
        "--dest",
        destination,
        "--object-path",
        object_path,
        "--method",
        method,
    ];
    command_args.extend_from_slice(args);
    command_check_with_session_bus("gdbus", &command_args)
}

fn command_check(command: &str, args: &[&str]) -> Check {
    run_command(command, args, false)
}

fn command_check_with_session_bus(command: &str, args: &[&str]) -> Check {
    run_command(command, args, true)
}

fn run_command(command: &str, args: &[&str], with_session_bus: bool) -> Check {
    let mut cmd = Command::new(command);
    cmd.args(args);

    if with_session_bus {
        if let Some(address) = dbus_session_address() {
            cmd.env("DBUS_SESSION_BUS_ADDRESS", address);
        }
        if let Some(runtime) = xdg_runtime_dir() {
            cmd.env("XDG_RUNTIME_DIR", runtime);
        }
    }

    // A D-Bus probe can block on a stalled service.
    // Bound every probe and reap its process group instead of stalling doctor
    // and every get_app_state request waiting for that diagnostic.
    match crate::command_runner::output_blocking(&mut cmd, &format!("run diagnostic {command}")) {
        Ok(output) if output.status.success() => {
            let detail = String::from_utf8_lossy(&output.stdout).trim().to_string();
            Check::ok(if detail.is_empty() {
                "ok".into()
            } else {
                detail
            })
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let detail = if !stderr.is_empty() { stderr } else { stdout };
            Check::fail(if detail.is_empty() {
                format!("exit status {}", output.status)
            } else {
                detail
            })
        }
        Err(error) => Check::fail(format!("{error:#}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describe_bool_reads_busctl_and_gdbus_output() {
        for (raw, expected) in [("b true", "on"), ("(<false>,)", "off"), ("false", "off")] {
            assert_eq!(describe_bool(Check::ok(raw), "on", "off").detail, expected);
        }
        let failed = describe_bool(Check::fail("no such property"), "on", "off");
        assert!(!failed.ok);
        assert_eq!(failed.detail, "no such property");
    }

    #[test]
    fn diagnostic_command_timeout_reaps_the_hung_process() {
        use std::time::{Duration, Instant};

        let pid_path = std::env::temp_dir().join(format!(
            "cul-diagnostic-timeout-{}-{}.pid",
            std::process::id(),
            getrandom::u64().unwrap()
        ));
        let started = Instant::now();
        let result = run_command(
            "/bin/sh",
            &[
                "-c",
                "printf '%s' \"$$\" > \"$1\"; exec /bin/sleep 60",
                "diagnostic-timeout-test",
                pid_path.to_str().unwrap(),
            ],
            false,
        );
        let elapsed = started.elapsed();
        let pid = fs::read_to_string(&pid_path);
        let _ = fs::remove_file(&pid_path);
        let pid: u32 = pid.expect("fixture must record its pid").parse().unwrap();

        assert!(!result.ok, "hung diagnostic must fail");
        assert!(result.detail.contains("timed out after 2000 ms"));
        assert!(elapsed < Duration::from_secs(4), "took {elapsed:?}");
        assert!(
            !Path::new(&format!("/proc/{pid}")).exists(),
            "diagnostic child must be killed and reaped before returning"
        );
    }

    #[test]
    fn diagnostic_spawn_failure_keeps_the_os_cause() {
        let path = std::env::temp_dir().join(format!(
            "cul-missing-diagnostic-{}-{}",
            std::process::id(),
            getrandom::u64().unwrap()
        ));
        let result = run_command(path.to_str().unwrap(), &[], false);
        assert!(!result.ok);
        assert!(result.detail.contains("failed to run diagnostic"));
        assert!(result.detail.contains("os error 2"));
    }

    #[test]
    fn parses_parent_pid_from_proc_status() {
        let status = "Name:\ttest\nPid:\t42\nPPid:\t7\n";

        assert_eq!(parse_parent_pid(status), Some(7));
    }

    #[test]
    fn parses_nul_separated_process_environment() {
        let environment = parse_environ(
            b"DISPLAY=:0\0I3SOCK=/tmp/i3-ipc.sock\0EMPTY=\0NO_EQUALS\0XDG_SESSION_TYPE=x11\0",
        );

        assert_eq!(environment.get("DISPLAY").map(String::as_str), Some(":0"));
        assert_eq!(
            environment.get("I3SOCK").map(String::as_str),
            Some("/tmp/i3-ipc.sock")
        );
        assert_eq!(environment.get("EMPTY").map(String::as_str), Some(""));
        assert!(!environment.contains_key("NO_EQUALS"));
    }

    #[test]
    fn desktop_env_hydration_includes_xauthority() {
        assert!(DESKTOP_ENV_KEYS.contains(&"XAUTHORITY"));
    }

    #[test]
    fn parses_systemd_show_environment_output() {
        let environment = parse_line_environment(
            b"DISPLAY=:0\nXDG_CURRENT_DESKTOP=i3\nNO_EQUALS\nI3SOCK=/run/i3/ipc.sock\n",
        );

        assert_eq!(environment.get("DISPLAY").map(String::as_str), Some(":0"));
        assert_eq!(
            environment.get("XDG_CURRENT_DESKTOP").map(String::as_str),
            Some("i3")
        );
        assert_eq!(
            environment.get("I3SOCK").map(String::as_str),
            Some("/run/i3/ipc.sock")
        );
        assert!(!environment.contains_key("NO_EQUALS"));
    }

    fn platform(display_ok: bool) -> PlatformReport {
        PlatformReport {
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            display: Some(":97".to_string()),
            xdg_session_type: Some("x11".to_string()),
            xdg_current_desktop: Some("i3".to_string()),
            i3sock: Some("/tmp/i3-ipc.sock".to_string()),
            xauthority: None,
            dbus_session_bus_address: Some("unix:path=/tmp/bus".to_string()),
            xdg_runtime_dir: Some("/tmp".to_string()),
            x11_display: if display_ok {
                Check::ok("native X11 root window 1920x1080, depth 24")
            } else {
                Check::fail("failed to connect to the X server named by DISPLAY")
            },
        }
    }

    fn accessibility(bus_ok: bool, enabled: bool) -> AccessibilityReport {
        AccessibilityReport {
            at_spi_bus: if bus_ok {
                Check::ok("s \"unix:path=/tmp/at-spi/bus\"")
            } else {
                Check::fail("The name org.a11y.Bus was not provided by any .service files")
            },
            at_spi_enabled: Check::ok(if enabled {
                "AT-SPI IsEnabled is true"
            } else {
                "AT-SPI IsEnabled is false"
            }),
        }
    }

    fn windowing(can_list: bool, can_focus: bool) -> WindowingReport {
        WindowingReport {
            backends: BTreeMap::from([(
                I3_BACKEND.to_string(),
                if can_list {
                    Check::ok("i3-msg get_tree returned a JSON tree")
                } else {
                    Check::fail("i3-msg: could not connect")
                },
            )]),
            can_list_windows: can_list,
            can_focus_windows: can_focus,
        }
    }

    fn commands(xdotool: bool, tesseract: bool) -> CommandReport {
        let check = |ok| {
            if ok {
                Check::ok("/usr/bin/tool")
            } else {
                Check::fail("exit status 1")
            }
        };
        CommandReport {
            xdotool: check(xdotool),
            tesseract: check(tesseract),
        }
    }

    #[test]
    fn complete_x11_session_is_ready_without_blockers() {
        let readiness = readiness_report(
            &platform(true),
            &accessibility(true, true),
            &windowing(true, true),
            &commands(true, true),
        );
        assert!(readiness.ready);
        assert!(readiness.blockers.is_empty());
        assert!(readiness.can_send_input && readiness.can_ocr);
        assert_eq!(
            readiness.screenshot_capture_status,
            ScreenshotCaptureStatus::Unverified
        );
        assert_eq!(readiness.recommended_next_step, READY_NEXT_STEP);
    }

    #[test]
    fn missing_tesseract_only_disables_ocr() {
        let readiness = readiness_report(
            &platform(true),
            &accessibility(true, true),
            &windowing(true, true),
            &commands(true, false),
        );
        assert!(readiness.ready);
        assert!(readiness.blockers.is_empty());
        assert!(!readiness.can_ocr);
    }

    #[test]
    fn each_blocker_names_its_fix_and_leads_the_next_step() {
        let cases = [
            (
                readiness_report(
                    &platform(false),
                    &accessibility(true, true),
                    &windowing(true, true),
                    &commands(true, true),
                ),
                "Start Xvfb on :97",
            ),
            (
                readiness_report(
                    &platform(true),
                    &accessibility(false, true),
                    &windowing(true, true),
                    &commands(true, true),
                ),
                "DBUS_SESSION_BUS_ADDRESS",
            ),
            (
                readiness_report(
                    &platform(true),
                    &accessibility(true, false),
                    &windowing(true, true),
                    &commands(true, true),
                ),
                "IsEnabled b true",
            ),
            (
                readiness_report(
                    &platform(true),
                    &accessibility(true, true),
                    &windowing(false, false),
                    &commands(true, true),
                ),
                "export I3SOCK",
            ),
            (
                readiness_report(
                    &platform(true),
                    &accessibility(true, true),
                    &windowing(true, true),
                    &commands(false, true),
                ),
                "Install xdotool",
            ),
        ];
        for (readiness, fix) in cases {
            assert!(!readiness.ready);
            assert_eq!(readiness.blockers.len(), 1, "{:?}", readiness.blockers);
            assert!(
                readiness.blockers[0].contains(fix),
                "{}",
                readiness.blockers[0]
            );
            assert_eq!(readiness.recommended_next_step, readiness.blockers[0]);
        }
    }

    #[test]
    fn unreachable_display_blocks_screenshots_and_input() {
        let readiness = readiness_report(
            &platform(false),
            &accessibility(true, true),
            &windowing(true, true),
            &commands(true, true),
        );
        assert!(!readiness.can_send_input);
        assert_eq!(
            readiness.screenshot_capture_status,
            ScreenshotCaptureStatus::Unavailable
        );
    }

    #[test]
    fn screenshot_results_update_readiness() {
        let mut readiness = readiness_report(
            &platform(true),
            &accessibility(true, true),
            &windowing(true, true),
            &commands(true, true),
        );
        readiness.record_screenshot_result(false);
        assert!(!readiness.ready);
        assert_eq!(readiness.blockers, [SCREENSHOT_FAILED]);
        assert_eq!(readiness.recommended_next_step, SCREENSHOT_FAILED);

        readiness.record_screenshot_result(true);
        assert!(readiness.ready);
        assert!(readiness.can_capture_screenshots);
        assert_eq!(
            readiness.screenshot_capture_status,
            ScreenshotCaptureStatus::Verified
        );
        assert_eq!(readiness.recommended_next_step, READY_NEXT_STEP);
    }

    #[test]
    fn accessibility_tree_requires_bus_and_enabled_status() {
        assert!(can_build_accessibility_tree(&accessibility(true, true)));
        assert!(!can_build_accessibility_tree(&accessibility(false, true)));
        assert!(!can_build_accessibility_tree(&accessibility(true, false)));
    }
}
