//! Launching apps and posting notifications for the `app` and
//! `notification` tools. Neither goes through a shell: names and texts are
//! passed as arguments.

use anyhow::{bail, Context, Result};

#[cfg(target_os = "macos")]
const LAUNCH_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

/// Launches an app by name (or bundle id on macOS, desktop id or program on
/// Linux). On macOS it opens in the background (`open -g`) and becomes the
/// agent's target app; the user's frontmost app keeps focus.
pub(crate) async fn launch_app(name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() || name.contains('\0') {
        bail!("pass the app's name");
    }
    launch(name).await
}

#[cfg(target_os = "macos")]
async fn launch(name: &str) -> Result<String> {
    let bundle_like =
        name.contains('.') && !name.contains(char::is_whitespace) && !name.contains('/');
    let flag = if bundle_like { "-b" } else { "-a" };
    let output = tokio::process::Command::new("/usr/bin/open")
        .args(["-g", flag, name])
        .output()
        .await
        .context("could not run open")?;
    if !output.status.success() {
        bail!(
            "could not open {name}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let started = std::time::Instant::now();
    while started.elapsed() < LAUNCH_WAIT {
        let found = crate::macos::ax_tree::match_app(name).and_then(|pid| {
            crate::macos::ax_tree::running_apps()
                .into_iter()
                .find(|app| app.pid == pid)
        });
        // Running is not ready: wait for a window AX can read, so the next
        // get_app_state finds the app instead of failing.
        if let Some(app) = found.filter(|app| crate::macos::ax_tree::has_windows(app.pid)) {
            crate::macos::ax_tree::set_target(app.pid);
            return Ok(format!(
                "opened {} (pid {}) in the background; it is now the agent's app",
                app.name, app.pid
            ));
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    Ok(format!(
        "asked macOS to open {name}; it has not shown up as a running app yet, check list_apps"
    ))
}

#[cfg(not(target_os = "macos"))]
async fn launch(name: &str) -> Result<String> {
    use std::process::Stdio;
    // A desktop entry first (gtk-launch firefox), else the program itself.
    let desktop = which("gtk-launch").is_some()
        && !name.contains('/')
        && std::process::Command::new("gtk-launch")
            .arg(name)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success());
    if desktop {
        return Ok(format!("launched desktop entry {name}"));
    }
    if name.contains(char::is_whitespace) {
        bail!("{name:?} is not a desktop entry; pass a program name without arguments");
    }
    let child = tokio::process::Command::new(name)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .with_context(|| format!("could not start {name}"))?;
    Ok(format!(
        "started {name} (pid {})",
        child.id().unwrap_or_default()
    ))
}

#[cfg(not(target_os = "macos"))]
fn which(command: &str) -> Option<std::path::PathBuf> {
    std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .map(|dir| dir.join(command))
            .find(|candidate| candidate.is_file())
    })
}

/// Pid of a running app named like `launch_app` takes it (macOS: name,
/// English bundle name or bundle id). `None` on Linux, where windows are
/// matched by class and title instead.
pub(crate) fn app_pid(name: &str) -> Option<u32> {
    #[cfg(target_os = "macos")]
    return crate::macos::ax_tree::match_app(name);
    #[cfg(not(target_os = "macos"))]
    {
        let _ = name;
        None
    }
}

/// Posts a desktop notification.
pub(crate) async fn notify(
    title: &str,
    message: &str,
    subtitle: Option<&str>,
    sound: bool,
) -> Result<()> {
    if title.trim().is_empty() && message.trim().is_empty() {
        bail!("pass a title or a message");
    }
    let output = notification_command(title, message, subtitle, sound)
        .output()
        .await
        .context("could not run the notification command")?;
    if !output.status.success() {
        bail!(
            "notification failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn notification_command(
    title: &str,
    message: &str,
    subtitle: Option<&str>,
    sound: bool,
) -> tokio::process::Command {
    // Texts arrive as argv, so quotes in them cannot break the script.
    let display = if sound {
        "display notification (item 2 of argv) with title (item 1 of argv) subtitle (item 3 of argv) sound name \"default\""
    } else {
        "display notification (item 2 of argv) with title (item 1 of argv) subtitle (item 3 of argv)"
    };
    let mut command = tokio::process::Command::new("/usr/bin/osascript");
    command
        .args(["-e", "on run argv", "-e", display, "-e", "end run"])
        .args([title, message, subtitle.unwrap_or_default()]);
    command
}

#[cfg(not(target_os = "macos"))]
fn notification_command(
    title: &str,
    message: &str,
    subtitle: Option<&str>,
    _sound: bool,
) -> tokio::process::Command {
    let body = match subtitle.filter(|subtitle| !subtitle.is_empty()) {
        Some(subtitle) => format!("{subtitle}\n{message}"),
        None => message.to_string(),
    };
    let mut command = tokio::process::Command::new("notify-send");
    command.args(["--", title, &body]);
    command
}
