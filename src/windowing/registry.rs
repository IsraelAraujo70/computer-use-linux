use crate::windowing::backends::{i3, x11};
use crate::windowing::types::WindowInfo;
use anyhow::{anyhow, Result};

pub use i3::I3_BACKEND;
pub use x11::X11_BACKEND;

pub const WINDOW_PERMISSION_HINT: &str = "Computer Use could not list windows. Window targeting needs i3-msg reaching the i3 IPC socket (I3SOCK), or wmctrl and xprop on another X11 window manager.";

#[derive(Debug, Clone, Copy)]
pub struct BackendDescriptor {
    pub id: &'static str,
    pub failure_label: &'static str,
    pub list_note: &'static str,
    pub can_exact_focus: bool,
}

#[derive(Debug, Clone)]
pub struct BackendProbe {
    pub id: &'static str,
    pub ok: bool,
    pub can_list_windows: bool,
    pub can_focus_windows: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Copy)]
enum BackendKind {
    I3,
    X11,
}

/// i3 first; the generic X11/EWMH backend answers only when i3 does not.
const BACKEND_ORDER: &[BackendKind] = &[BackendKind::I3, BackendKind::X11];

const DESCRIPTORS: &[BackendDescriptor] = &[
    BackendDescriptor {
        id: I3_BACKEND,
        failure_label: "i3",
        list_note: "Window list came from i3-msg. Terminal windows may include best-effort PTY and active-process context when xprop and the process tree are readable.",
        can_exact_focus: true,
    },
    BackendDescriptor {
        id: X11_BACKEND,
        failure_label: "X11/EWMH",
        list_note: "Window list came from X11/EWMH (wmctrl). Terminal windows may include best-effort PTY and active-process context when the process tree is readable.",
        can_exact_focus: true,
    },
];

pub fn descriptor(id: &str) -> Option<&'static BackendDescriptor> {
    DESCRIPTORS.iter().find(|descriptor| descriptor.id == id)
}

pub fn list_note(id: &str) -> &'static str {
    descriptor(id).unwrap_or(&DESCRIPTORS[0]).list_note
}

pub fn backend_can_exact_focus(id: &str) -> bool {
    if id == X11_BACKEND {
        x11::can_exact_focus()
    } else {
        descriptor(id).is_some_and(|descriptor| descriptor.can_exact_focus)
    }
}

pub async fn list_windows() -> Result<Vec<WindowInfo>> {
    let mut errors = Vec::new();
    for backend in BACKEND_ORDER {
        if let Some(windows) =
            usable_backend_windows(*backend, list_windows_for(*backend).await, &mut errors)
        {
            return Ok(windows);
        }
    }
    Err(anyhow!(errors.join("; ")))
}

fn usable_backend_windows(
    backend: BackendKind,
    result: Result<Vec<WindowInfo>>,
    errors: &mut Vec<String>,
) -> Option<Vec<WindowInfo>> {
    match result {
        Ok(windows) if !windows.is_empty() => Some(windows),
        Ok(_) => {
            errors.push(format!("{} returned no windows", backend.failure_label()));
            None
        }
        Err(error) => {
            errors.push(format!("{} failed: {error:#}", backend.failure_label()));
            None
        }
    }
}

async fn list_windows_for(backend: BackendKind) -> Result<Vec<WindowInfo>> {
    match backend {
        BackendKind::I3 => i3::list_windows().await,
        BackendKind::X11 => x11::list_windows().await,
    }
}

pub async fn activate_window(window: &WindowInfo) -> Result<()> {
    match window.backend.as_str() {
        I3_BACKEND => i3::activate_window(window.window_id).await,
        X11_BACKEND => x11::activate_window(window.window_id).await,
        backend => Err(anyhow!(
            "Unsupported window backend for activation: {backend}"
        )),
    }
}

pub async fn focused_window_for_backend(backend: &str) -> Result<Option<WindowInfo>> {
    let windows = match backend {
        I3_BACKEND => i3::list_windows().await?,
        X11_BACKEND => x11::list_windows_for_exact_focus().await?,
        backend => {
            return Err(anyhow!(
                "Unsupported window backend for focus query: {backend}"
            ))
        }
    };
    Ok(windows.into_iter().find(|window| window.focused))
}

pub fn probe_backends() -> Vec<BackendProbe> {
    vec![i3::probe(), x11::probe()]
}

impl BackendKind {
    fn id(self) -> &'static str {
        match self {
            BackendKind::I3 => I3_BACKEND,
            BackendKind::X11 => X11_BACKEND,
        }
    }

    fn failure_label(self) -> &'static str {
        descriptor(self.id())
            .map(|item| item.failure_label)
            .unwrap_or(self.id())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::windowing::types::WindowBounds;

    fn window(backend: &str) -> WindowInfo {
        WindowInfo {
            window_id: 1,
            title: Some("Chromium".to_string()),
            app_id: Some("chromium".to_string()),
            wm_class: Some("Chromium".to_string()),
            pid: Some(1234),
            bounds: Some(WindowBounds {
                x: Some(0),
                y: Some(0),
                width: 800,
                height: 600,
            }),
            workspace: None,
            focused: true,
            hidden: false,
            client_type: Some("x11".to_string()),
            backend: backend.to_string(),
            terminal: None,
        }
    }

    #[test]
    fn skips_empty_backend_results_so_later_backends_can_answer() {
        let mut errors = Vec::new();

        assert!(usable_backend_windows(BackendKind::I3, Ok(Vec::new()), &mut errors).is_none());

        let windows =
            usable_backend_windows(BackendKind::X11, Ok(vec![window(X11_BACKEND)]), &mut errors)
                .expect("non-empty backend result should be accepted");

        assert_eq!(windows[0].backend, X11_BACKEND);
        assert_eq!(errors, vec!["i3 returned no windows"]);
    }

    #[test]
    fn i3_answers_before_the_generic_x11_backend() {
        let ids = BACKEND_ORDER
            .iter()
            .map(|backend| backend.id())
            .collect::<Vec<_>>();
        assert_eq!(ids, [I3_BACKEND, X11_BACKEND]);
    }

    #[test]
    fn records_backend_failures_with_registry_labels() {
        let mut errors = Vec::new();

        assert!(usable_backend_windows(
            BackendKind::X11,
            Err(anyhow!("wmctrl failed")),
            &mut errors,
        )
        .is_none());

        assert_eq!(errors, vec!["X11/EWMH failed: wmctrl failed"]);
    }
}
