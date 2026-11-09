//! macOS stand-in for `x11_display`: the window under a point and the
//! desktop size, from the window server. `server.rs` uses these to refuse
//! pointer input that would land on another app's window (`check_window_at`)
//! and to bound targeted screenshots.

use crate::windowing::backends::mac::{desktop_size, on_screen_windows};
use anyhow::{Context, Result};
use std::time::Duration;

pub(crate) const X11_QUERY_TIMEOUT: Duration = Duration::from_secs(2);

/// The window server sees every window, so the window-under-point check
/// is meaningful (the X11 counterpart is false off native X11).
pub(crate) fn is_native_x11_session() -> bool {
    true
}

/// The frontmost window at a point.
#[derive(Debug, Clone)]
pub(crate) struct WindowAtPoint {
    pub window: u32,
    pub pid: Option<u32>,
    pub title: Option<String>,
    owner: String,
}

impl std::fmt::Display for WindowAtPoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} window {}", self.owner, self.window)?;
        if let Some(title) = self.title.as_deref().filter(|title| !title.is_empty()) {
            write!(f, " \"{title}\"")?;
        }
        if let Some(pid) = self.pid {
            write!(f, " (pid {pid})")?;
        }
        Ok(())
    }
}

/// Window-server queries, shaped like the X11 display handle.
pub(crate) struct X11Display;

impl X11Display {
    pub(crate) fn root_size(&self) -> (u32, u32) {
        desktop_size()
    }

    /// The frontmost window containing the point, menus and the Dock
    /// included (they cover what is below them).
    pub(crate) fn window_at(&self, (x, y): (i16, i16)) -> Result<Option<WindowAtPoint>> {
        let (x, y) = (i32::from(x), i32::from(y));
        Ok(on_screen_windows()
            .into_iter()
            .filter(|window| window.owner != "Window Server")
            .find(|window| {
                let (wx, wy, width, height) = window.bounds;
                x >= wx && y >= wy && x < wx + width as i32 && y < wy + height as i32
            })
            .map(|window| WindowAtPoint {
                window: window.id,
                pid: Some(window.pid),
                title: window.title,
                owner: window.owner,
            }))
    }
}

/// Runs `f` off the async runtime, bounded by `limit`.
pub(crate) async fn with_x11_display<T, F>(limit: Duration, f: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce(&X11Display) -> T + Send + 'static,
{
    tokio::time::timeout(limit, tokio::task::spawn_blocking(move || f(&X11Display)))
        .await
        .context("window server query timed out")?
        .context("window server query failed")
}
