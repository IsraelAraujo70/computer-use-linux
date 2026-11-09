//! macOS stand-in for `x11_dnd`. A real file drag needs an AppKit dragging
//! session started from a window this process owns, which a background MCP
//! server does not have, so `drop_files` reports the supported route instead.
//!
//! ponytail: upgrade path is a tiny helper .app that opens an invisible
//! window and runs `beginDraggingSession` with file URLs.

use std::{path::PathBuf, time::Duration};

#[derive(Debug, Default)]
pub(crate) struct DropOutcome {
    pub log: Vec<String>,
}

impl DropOutcome {
    pub(crate) fn failure(&self) -> Option<&'static str> {
        Some("file drops are not supported on macOS")
    }
}

pub(crate) fn drop_files(
    _paths: &[PathBuf],
    _point: (i32, i32),
    _finish_timeout: Duration,
) -> anyhow::Result<DropOutcome> {
    anyhow::bail!(
        "file drops are not supported on macOS; click the page's file input instead, then in the open panel press_key super+shift+g, type_text the path, and press_key Return"
    )
}
