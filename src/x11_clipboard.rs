//! X11 CLIPBOARD owner used to paste text that the keyboard path cannot type.

use crate::x11_display::{X11Display, X11_QUERY_TIMEOUT};
use anyhow::Context as _;
use std::{
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};
use x11rb::{
    connection::Connection,
    protocol::{xproto::*, Event},
    wrapper::ConnectionExt as _,
    CURRENT_TIME, NONE,
};

fn latin1(text: &str) -> Vec<u8> {
    text.chars()
        .map(|c| u8::try_from(u32::from(c)).unwrap_or(b'?'))
        .collect()
}

/// Owns CLIPBOARD and PRIMARY on the `DISPLAY` X server; see
/// [`hold_clipboard_on`].
pub(crate) fn hold_clipboard(text: &str, hold: Duration) -> anyhow::Result<ClipboardGuard> {
    anyhow::ensure!(text.len() <= 256 * 1024, "clipboard text exceeds 256 KB");
    let display = std::env::var("DISPLAY").context("DISPLAY is not set")?;
    hold_clipboard_on(&display, text, hold)
}

/// The atoms a clipboard owner answers with.
struct Atoms {
    clipboard: Atom,
    targets: Atom,
    utf8: Atom,
    text: Atom,
}

/// Creates the owner window and takes CLIPBOARD and PRIMARY with it.
fn take_ownership(display: &X11Display) -> anyhow::Result<(Window, Atoms)> {
    let conn = display.connection();
    let window = conn.generate_id()?;
    conn.create_window(
        0,
        window,
        display.root(),
        0,
        0,
        1,
        1,
        0,
        WindowClass::INPUT_ONLY,
        0,
        &CreateWindowAux::new(),
    )?
    .check()?;
    let atom =
        |name: &[u8]| -> anyhow::Result<u32> { Ok(conn.intern_atom(false, name)?.reply()?.atom) };
    let atoms = Atoms {
        clipboard: atom(b"CLIPBOARD")?,
        targets: atom(b"TARGETS")?,
        utf8: atom(b"UTF8_STRING")?,
        text: atom(b"TEXT")?,
    };
    for selection in [atoms.clipboard, AtomEnum::PRIMARY.into()] {
        conn.set_selection_owner(window, selection, CURRENT_TIME)?
            .check()?;
        anyhow::ensure!(
            conn.get_selection_owner(selection)?.reply()?.owner == window,
            "selection ownership failed"
        );
    }
    conn.flush()?;
    Ok((window, atoms))
}

/// Owns CLIPBOARD and PRIMARY until dropped or the hold duration elapses.
/// Taking ownership must finish within `X11_QUERY_TIMEOUT`, and serving and
/// releasing it within `hold` plus `X11_QUERY_TIMEOUT`, so a stuck X server
/// fails the call or ends the background owner instead of blocking.
pub(crate) fn hold_clipboard_on(
    display_name: &str,
    text: &str,
    hold: Duration,
) -> anyhow::Result<ClipboardGuard> {
    let display = X11Display::connect_named(display_name, Instant::now() + X11_QUERY_TIMEOUT)?;
    let (window, atoms) = take_ownership(&display)
        .map_err(|error| display.explain_timeout(error, "the request to own the clipboard"))?;
    display.set_deadline(Instant::now() + hold + X11_QUERY_TIMEOUT);
    let Atoms {
        clipboard,
        targets,
        utf8,
        text: text_atom,
    } = atoms;
    let stop = Arc::new(AtomicBool::new(false));
    let reads = Arc::new(AtomicUsize::new(0));
    let shutdown = stop.clone();
    let answered = reads.clone();
    let bytes = text.as_bytes().to_vec();
    let fallback = latin1(text);
    let worker = thread::spawn(move || {
        let conn = display.connection();
        let run = || -> anyhow::Result<()> {
            let deadline = Instant::now() + hold;
            while !shutdown.load(Ordering::Acquire) && Instant::now() < deadline {
                match conn.poll_for_event()? {
                    Some(Event::SelectionClear(_)) => break,
                    Some(Event::SelectionRequest(req)) => {
                        let property = if req.property == NONE {
                            req.target
                        } else {
                            req.property
                        };
                        let is_text =
                            [utf8, text_atom, AtomEnum::STRING.into()].contains(&req.target);
                        let supported = req.target == targets || is_text;
                        if req.target == targets {
                            conn.change_property32(
                                PropMode::REPLACE,
                                req.requestor,
                                property,
                                AtomEnum::ATOM,
                                &[targets, utf8, AtomEnum::STRING.into(), text_atom],
                            )?
                            .check()?;
                        } else if is_text {
                            let (kind, data) = if req.target == u32::from(AtomEnum::STRING) {
                                (u32::from(AtomEnum::STRING), &fallback)
                            } else {
                                (utf8, &bytes)
                            };
                            conn.change_property8(
                                PropMode::REPLACE,
                                req.requestor,
                                property,
                                kind,
                                data,
                            )?
                            .check()?;
                        }
                        conn.send_event(
                            false,
                            req.requestor,
                            EventMask::NO_EVENT,
                            SelectionNotifyEvent {
                                response_type: SELECTION_NOTIFY_EVENT,
                                sequence: 0,
                                time: req.time,
                                requestor: req.requestor,
                                selection: req.selection,
                                target: req.target,
                                property: if supported { property } else { NONE },
                            },
                        )?
                        .check()?;
                        conn.flush()?;
                        if is_text {
                            answered.fetch_add(1, Ordering::AcqRel);
                        }
                    }
                    None => thread::sleep(Duration::from_millis(5)),
                    _ => {}
                }
            }
            for selection in [clipboard, AtomEnum::PRIMARY.into()] {
                if conn.get_selection_owner(selection)?.reply()?.owner == window {
                    conn.set_selection_owner(NONE, selection, CURRENT_TIME)?;
                }
            }
            conn.destroy_window(window)?;
            conn.flush()?;
            Ok(())
        };
        let _ = run();
    });
    Ok(ClipboardGuard {
        stop,
        reads,
        worker: Some(worker),
    })
}

/// Lifetime guard for the background selection owner.
pub(crate) struct ClipboardGuard {
    stop: Arc<AtomicBool>,
    /// Text requests answered so far.
    reads: Arc<AtomicUsize>,
    worker: Option<thread::JoinHandle<()>>,
}
impl ClipboardGuard {
    /// Waits up to timeout for a text request to be answered.
    pub(crate) fn wait_served(&self, timeout: Duration) -> bool {
        self.wait_reads(1, timeout)
    }

    /// Waits up to timeout until `count` text requests have been answered.
    pub(crate) fn wait_reads(&self, count: usize, timeout: Duration) -> bool {
        let start = Instant::now();
        while self.reads.load(Ordering::Acquire) < count && start.elapsed() < timeout {
            thread::sleep(Duration::from_millis(5));
        }
        self.reads.load(Ordering::Acquire) >= count
    }
}
impl Drop for ClipboardGuard {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let start = Instant::now();
            while !worker.is_finished() && start.elapsed() < Duration::from_millis(450) {
                thread::sleep(Duration::from_millis(5));
            }
            if worker.is_finished() {
                let _ = worker.join();
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn latin1_lossy() {
        assert_eq!(latin1("não ✓"), b"n\xe3o ?");
    }
    #[test]
    fn size_limit() {
        assert!(hold_clipboard(&"x".repeat(256 * 1024 + 1), Duration::ZERO).is_err());
    }
    #[test]
    fn a_stalled_x_server_fails_the_paste_instead_of_blocking() {
        let display = crate::x11_display::stalled_x_server();
        let started = Instant::now();
        let error = match hold_clipboard_on(&display, "x", Duration::from_secs(10)) {
            Ok(_) => panic!("a stalled X server cannot hand over the clipboard"),
            Err(error) => error,
        };
        assert!(format!("{error:#}").contains("did not answer"), "{error:#}");
        assert!(started.elapsed() < X11_QUERY_TIMEOUT + Duration::from_secs(1));
    }
    #[test]
    #[ignore]
    fn selection_roundtrip() -> anyhow::Result<()> {
        let guard = hold_clipboard("não ação ✓", Duration::from_secs(5))?;
        let (conn, screen) = x11rb::connect(None)?;
        let w = conn.generate_id()?;
        conn.create_window(
            0,
            w,
            conn.setup().roots[screen].root,
            0,
            0,
            1,
            1,
            0,
            WindowClass::INPUT_ONLY,
            0,
            &CreateWindowAux::new(),
        )?;
        let clip = conn.intern_atom(false, b"CLIPBOARD")?.reply()?.atom;
        let utf8 = conn.intern_atom(false, b"UTF8_STRING")?.reply()?.atom;
        conn.convert_selection(w, clip, utf8, utf8, CURRENT_TIME)?;
        conn.flush()?;
        assert!(guard.wait_served(Duration::from_secs(2)));
        let value = conn
            .get_property(false, w, utf8, utf8, 0, 1024)?
            .reply()?
            .value;
        assert_eq!(value, "não ação ✓".as_bytes());
        Ok(())
    }
    #[test]
    #[ignore]
    fn chrome_paste() -> anyhow::Result<()> {
        let guard = hold_clipboard("não ação", Duration::from_secs(5))?;
        assert!(std::process::Command::new("xdotool")
            .args(["key", "--clearmodifiers", "ctrl+v"])
            .status()?
            .success());
        assert!(guard.wait_served(Duration::from_secs(2)));
        Ok(())
    }
}
