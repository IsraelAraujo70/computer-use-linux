//! macOS stand-in for `x11_clipboard`: puts text on the general pasteboard
//! for one paste and puts the user's previous text back afterwards.
//!
//! ponytail: pbcopy/pbpaste keep this dependency-free; they carry plain text
//! only, so a previous non-text clipboard (an image, files) is not restored.
//! Move to NSPasteboard if that matters.

use anyhow::{Context, Result};
use std::{
    io::Write,
    process::{Command, Stdio},
    time::Duration,
};

/// Holds `text` on the pasteboard until dropped.
pub(crate) fn hold_clipboard(text: &str, _hold: Duration) -> Result<ClipboardGuard> {
    anyhow::ensure!(text.len() <= 256 * 1024, "clipboard text exceeds 256 KB");
    let previous = read_pasteboard();
    write_pasteboard(text)?;
    Ok(ClipboardGuard {
        ours: text.to_string(),
        previous,
    })
}

pub(crate) struct ClipboardGuard {
    ours: String,
    previous: Option<String>,
}

impl ClipboardGuard {
    /// The pasteboard does not report reads; the server judges delivery by
    /// reading the field back.
    pub(crate) fn wait_served(&self, _timeout: Duration) -> bool {
        true
    }

    pub(crate) fn wait_reads(&self, _count: usize, _timeout: Duration) -> bool {
        true
    }
}

impl Drop for ClipboardGuard {
    fn drop(&mut self) {
        // Restore only while our text is still there: the user may have
        // copied something since.
        if read_pasteboard().as_deref() == Some(self.ours.as_str()) {
            let _ = write_pasteboard(self.previous.as_deref().unwrap_or_default());
        }
    }
}

/// pbcopy/pbpaste read the locale to pick an encoding; launchd jobs have
/// none, so pin UTF-8.
fn utf8(command: &mut Command) -> &mut Command {
    command.env("LC_ALL", "en_US.UTF-8")
}

fn read_pasteboard() -> Option<String> {
    let output = utf8(&mut Command::new("/usr/bin/pbpaste")).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

fn write_pasteboard(text: &str) -> Result<()> {
    let mut child = utf8(&mut Command::new("/usr/bin/pbcopy"))
        .stdin(Stdio::piped())
        .spawn()
        .context("could not run pbcopy")?;
    child
        .stdin
        .take()
        .context("pbcopy has no stdin")?
        .write_all(text.as_bytes())?;
    anyhow::ensure!(child.wait()?.success(), "pbcopy failed");
    Ok(())
}
