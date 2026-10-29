//! XDND drag source that drops files onto a window, the way a file manager
//! does. Chrome on this host has no file dialog (no GTK, no portal), but it
//! accepts files dropped onto a file input or a drop zone.
//!
//! Protocol (XDND version 5, <https://freedesktop.org/wiki/Specifications/XDND/>):
//! the source owns `XdndSelection`, sends `XdndEnter` with its types to the
//! `XdndAware` toplevel under the point, then `XdndPosition` until the target
//! answers `XdndStatus` with accept, then `XdndDrop`. The target reads the
//! data through `XdndSelection` and ends with `XdndFinished`. The source never
//! sends a second `XdndPosition` before the `XdndStatus` of the first.

use crate::x11_display::{X11Display, X11_QUERY_TIMEOUT};
use anyhow::Context as _;
use std::{
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};
use x11rb::{
    connection::Connection,
    protocol::{xproto::*, Event},
    wrapper::ConnectionExt as _,
    NONE,
};

/// Highest XDND version this source speaks.
const XDND_VERSION: u32 = 5;
/// How long the target gets to accept the drag before it is abandoned.
const STATUS_TIMEOUT: Duration = Duration::from_millis(2_000);
/// `XdndPosition` messages sent before dropping, even when the first one is
/// accepted: Chrome answers the first before the page's dragover handlers ran.
const MIN_POSITIONS: u32 = 2;
/// Gap before the next `XdndPosition` after an accepting / refusing status.
const POSITION_GAP_ACCEPTED: Duration = Duration::from_millis(50);
const POSITION_GAP_REFUSED: Duration = Duration::from_millis(100);
/// Largest URI list served; there is no INCR transfer for bigger data.
const MAX_URI_LIST_BYTES: usize = 64 * 1024;

/// What happened during one drop, for the tool result and the server log.
#[derive(Debug, Default)]
pub(crate) struct DropOutcome {
    /// The `XdndAware` window the drop went to.
    pub target_window: u32,
    /// The last `XdndStatus` accepted the drag.
    pub accepted: bool,
    /// The target read the file list from `XdndSelection`.
    pub served: bool,
    /// `Some(accepted)` once `XdndFinished` arrived.
    pub finished: Option<bool>,
    /// One line per protocol step, for diagnosing a refused drop.
    pub log: Vec<String>,
}

impl DropOutcome {
    /// A drop counts when the target accepted it and then finished it with
    /// accept, or, when no `XdndFinished` came, at least read the files. A
    /// `XdndFinished` that refuses is a failure even after a read.
    pub(crate) fn succeeded(&self) -> bool {
        self.accepted
            && match self.finished {
                Some(accepted) => accepted,
                None => self.served,
            }
    }

    /// Why the drop failed, for the tool result: `None` when it succeeded.
    pub(crate) fn failure(&self) -> Option<&'static str> {
        if self.succeeded() {
            None
        } else if self.accepted && self.finished == Some(false) {
            Some("the page refused the drop")
        } else if self.accepted {
            Some("the page accepted the drag but never read the files")
        } else {
            Some("the page did not accept the drag")
        }
    }
}

/// `file://` URI of an absolute path: every byte except unreserved
/// characters and `/` is percent-encoded (RFC 3986), so spaces and non-ASCII
/// names survive.
pub(crate) fn file_uri(path: &Path) -> String {
    let mut uri = String::from("file://");
    for &byte in path.as_os_str().as_bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~/".contains(&byte) {
            uri.push(char::from(byte));
        } else {
            uri.push_str(&format!("%{byte:02X}"));
        }
    }
    uri
}

/// `text/uri-list` body (RFC 2483): one URI per line, each ending in CRLF.
pub(crate) fn uri_list(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|path| format!("{}\r\n", file_uri(path)))
        .collect()
}

/// `XdndEnter` data: source, version in the high byte of l[1] with bit 0
/// set when more than three types exist (then read from `XdndTypeList`),
/// and the first three types.
fn enter_data(source: Window, version: u32, types: &[Atom]) -> [u32; 5] {
    let mut data = [
        source,
        (version << 24) | u32::from(types.len() > 3),
        0,
        0,
        0,
    ];
    for (slot, atom) in data[2..].iter_mut().zip(types) {
        *slot = *atom;
    }
    data
}

/// `XdndPosition` data: source, reserved, root coordinates packed as
/// `x << 16 | y`, the timestamp, and the requested action.
fn position_data(source: Window, (x, y): (i16, i16), time: Timestamp, action: Atom) -> [u32; 5] {
    let packed = (u32::from(x as u16) << 16) | u32::from(y as u16);
    [source, 0, packed, time, action]
}

/// `XdndDrop` data: source, reserved, and the timestamp to convert the
/// selection with.
fn drop_data(source: Window, time: Timestamp) -> [u32; 5] {
    [source, 0, time, 0, 0]
}

/// `XdndLeave` data: the source.
fn leave_data(source: Window) -> [u32; 5] {
    [source, 0, 0, 0, 0]
}

/// Bit 0 of l[1] in `XdndStatus` and `XdndFinished`: the target accepts.
fn accepts(data: &[u32; 5]) -> bool {
    data[1] & 1 == 1
}

struct Atoms {
    aware: Atom,
    proxy: Atom,
    selection: Atom,
    enter: Atom,
    position: Atom,
    status: Atom,
    leave: Atom,
    drop: Atom,
    finished: Atom,
    action_copy: Atom,
    type_list: Atom,
    uri_list: Atom,
    text_plain: Atom,
    text_plain_utf8: Atom,
    utf8_string: Atom,
    targets: Atom,
}

impl Atoms {
    fn intern(conn: &impl Connection) -> anyhow::Result<Self> {
        let atom = |name: &[u8]| -> anyhow::Result<Atom> {
            Ok(conn.intern_atom(false, name)?.reply()?.atom)
        };
        Ok(Self {
            aware: atom(b"XdndAware")?,
            proxy: atom(b"XdndProxy")?,
            selection: atom(b"XdndSelection")?,
            enter: atom(b"XdndEnter")?,
            position: atom(b"XdndPosition")?,
            status: atom(b"XdndStatus")?,
            leave: atom(b"XdndLeave")?,
            drop: atom(b"XdndDrop")?,
            finished: atom(b"XdndFinished")?,
            action_copy: atom(b"XdndActionCopy")?,
            type_list: atom(b"XdndTypeList")?,
            uri_list: atom(b"text/uri-list")?,
            text_plain: atom(b"text/plain")?,
            text_plain_utf8: atom(b"text/plain;charset=utf-8")?,
            utf8_string: atom(b"UTF8_STRING")?,
            targets: atom(b"TARGETS")?,
        })
    }

    /// Types offered, most specific first.
    fn offered(&self) -> [Atom; 3] {
        [self.uri_list, self.text_plain_utf8, self.text_plain]
    }
}

/// The XDND version a window advertises through `XdndAware`, if any.
fn aware_version(conn: &impl Connection, atoms: &Atoms, window: Window) -> Option<u32> {
    let reply = conn
        .get_property(false, window, atoms.aware, AtomEnum::ATOM, 0, 1)
        .ok()?
        .reply()
        .ok()?;
    reply.value32().and_then(|mut values| values.next())
}

/// The outermost `XdndAware` window under a root point, walking down from
/// the root through the mapped child that contains the point (the window
/// manager's frame first, then the client).
fn aware_window_at(
    conn: &impl Connection,
    atoms: &Atoms,
    root: Window,
    (x, y): (i16, i16),
) -> anyhow::Result<Option<(Window, u32)>> {
    let mut window = root;
    for _ in 0..32 {
        let child = conn
            .translate_coordinates(root, window, x, y)?
            .reply()?
            .child;
        if child == NONE {
            return Ok(None);
        }
        if let Some(version) = aware_version(conn, atoms, child) {
            return Ok(Some((child, version)));
        }
        window = child;
    }
    Ok(None)
}

/// Where XDND messages for `target` go: its `XdndProxy` window when set.
fn message_window(conn: &impl Connection, atoms: &Atoms, target: Window) -> Window {
    conn.get_property(false, target, atoms.proxy, AtomEnum::WINDOW, 0, 1)
        .ok()
        .and_then(|cookie| cookie.reply().ok())
        .and_then(|reply| reply.value32().and_then(|mut values| values.next()))
        .filter(|&proxy| proxy != NONE)
        .unwrap_or(target)
}

/// Drops the files at root point `point` on the `DISPLAY` X server; see
/// [`drop_files_on`].
pub(crate) fn drop_files(
    paths: &[PathBuf],
    point: (i32, i32),
    finish_timeout: Duration,
) -> anyhow::Result<DropOutcome> {
    let display = std::env::var("DISPLAY").context("DISPLAY is not set")?;
    drop_files_on(&display, paths, point, finish_timeout)
}

/// Drops the files at root point `point`: warps the pointer there, runs the
/// XDND exchange as the drag source, and serves the file list. Every round
/// trip shares one deadline, so a stuck X server fails the drop after at
/// most `STATUS_TIMEOUT` plus `finish_timeout` plus twice
/// `X11_QUERY_TIMEOUT` (setup and cleanup). Call it off the async runtime.
pub(crate) fn drop_files_on(
    display_name: &str,
    paths: &[PathBuf],
    point: (i32, i32),
    finish_timeout: Duration,
) -> anyhow::Result<DropOutcome> {
    let list = uri_list(paths);
    anyhow::ensure!(
        list.len() <= MAX_URI_LIST_BYTES,
        "the file list exceeds {MAX_URI_LIST_BYTES} bytes"
    );
    let point = (i16::try_from(point.0)?, i16::try_from(point.1)?);
    let display = X11Display::connect_named(display_name, Instant::now() + X11_QUERY_TIMEOUT)?;
    let explain = |error| display.explain_timeout(error, "the drop");
    let conn = display.connection();
    let root = display.root();
    let atoms = Atoms::intern(conn).map_err(explain)?;
    let source = conn.generate_id()?;
    conn.create_window(
        0,
        source,
        root,
        -10,
        -10,
        1,
        1,
        0,
        WindowClass::INPUT_ONLY,
        0,
        &CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE),
    )?
    .check()
    .map_err(|error| explain(error.into()))?;
    display.set_deadline(Instant::now() + STATUS_TIMEOUT + finish_timeout + X11_QUERY_TIMEOUT);
    let result =
        run_drop(conn, &atoms, root, source, &list, point, finish_timeout).map_err(explain);
    if conn
        .get_selection_owner(atoms.selection)
        .ok()
        .and_then(|cookie| cookie.reply().ok())
        .is_some_and(|reply| reply.owner == source)
    {
        let _ = conn.set_selection_owner(NONE, atoms.selection, x11rb::CURRENT_TIME);
    }
    let _ = conn.destroy_window(source);
    let _ = conn.flush();
    result
}

fn run_drop(
    conn: &impl Connection,
    atoms: &Atoms,
    root: Window,
    source: Window,
    list: &str,
    point: (i16, i16),
    finish_timeout: Duration,
) -> anyhow::Result<DropOutcome> {
    let mut outcome = DropOutcome::default();
    // Some clients compare the pointer position with XdndPosition.
    conn.warp_pointer(NONE, root, 0, 0, 0, 0, point.0, point.1)?
        .check()?;
    // Setting the type list doubles as the way to learn the server time:
    // its PropertyNotify carries a timestamp, which the selection ownership
    // and XdndPosition/XdndDrop must share.
    conn.change_property32(
        PropMode::REPLACE,
        source,
        atoms.type_list,
        AtomEnum::ATOM,
        &atoms.offered(),
    )?
    .check()?;
    conn.flush()?;
    let time = wait_for_property_time(conn, source)?;
    conn.set_selection_owner(source, atoms.selection, time)?
        .check()?;
    anyhow::ensure!(
        conn.get_selection_owner(atoms.selection)?.reply()?.owner == source,
        "could not own XdndSelection"
    );

    let Some((target, aware)) = aware_window_at(conn, atoms, root, point)? else {
        anyhow::bail!(
            "no window under {},{} accepts drops (none has XdndAware)",
            point.0,
            point.1
        );
    };
    let version = aware.min(XDND_VERSION);
    let destination = message_window(conn, atoms, target);
    outcome.target_window = target;
    outcome.log.push(format!(
        "target 0x{target:x} XdndAware v{aware}, speaking v{version}"
    ));
    let send = |kind: Atom, data: [u32; 5]| -> anyhow::Result<()> {
        conn.send_event(
            false,
            destination,
            EventMask::NO_EVENT,
            ClientMessageEvent::new(32, target, kind, data),
        )?
        .check()?;
        conn.flush()?;
        Ok(())
    };
    send(atoms.enter, enter_data(source, version, &atoms.offered()))?;

    let started = Instant::now();
    let mut positions = 0;
    let mut awaiting_status = false;
    let mut next_position = Instant::now();
    let mut drop_deadline: Option<Instant> = None;
    loop {
        if drop_deadline.is_none() && !awaiting_status && Instant::now() >= next_position {
            send(
                atoms.position,
                position_data(source, point, time, atoms.action_copy),
            )?;
            positions += 1;
            awaiting_status = true;
        }
        match conn.poll_for_event()? {
            Some(Event::SelectionRequest(request)) => {
                if serve_request(conn, atoms, &request, list)? {
                    outcome.served = true;
                }
                outcome.log.push(format!(
                    "served {}",
                    atom_name(conn, request.target).unwrap_or_else(|| request.target.to_string())
                ));
            }
            Some(Event::ClientMessage(message)) if message.type_ == atoms.status => {
                let data = message.data.as_data32();
                awaiting_status = false;
                outcome.accepted = accepts(&data);
                outcome.log.push(format!(
                    "XdndStatus {} after position {positions}",
                    if outcome.accepted { "accept" } else { "refuse" }
                ));
                if drop_deadline.is_none() {
                    if outcome.accepted && positions >= MIN_POSITIONS {
                        send(atoms.drop, drop_data(source, time))?;
                        outcome.log.push("XdndDrop sent".to_string());
                        drop_deadline = Some(Instant::now() + finish_timeout);
                    } else {
                        next_position = Instant::now()
                            + if outcome.accepted {
                                POSITION_GAP_ACCEPTED
                            } else {
                                POSITION_GAP_REFUSED
                            };
                    }
                }
            }
            Some(Event::ClientMessage(message)) if message.type_ == atoms.finished => {
                let accepted = accepts(&message.data.as_data32());
                outcome.finished = Some(accepted);
                outcome
                    .log
                    .push(format!("XdndFinished accepted={accepted}"));
                return Ok(outcome);
            }
            Some(_) => {}
            None => thread::sleep(Duration::from_millis(5)),
        }
        match drop_deadline {
            Some(deadline) if Instant::now() >= deadline => {
                outcome
                    .log
                    .push("no XdndFinished before the timeout".to_string());
                return Ok(outcome);
            }
            None if started.elapsed() >= STATUS_TIMEOUT => {
                send(atoms.leave, leave_data(source))?;
                outcome.accepted = false;
                outcome
                    .log
                    .push("the target never accepted; XdndLeave sent".to_string());
                return Ok(outcome);
            }
            _ => {}
        }
    }
}

/// Waits for the PropertyNotify of the source window and returns its time.
fn wait_for_property_time(conn: &impl Connection, window: Window) -> anyhow::Result<Timestamp> {
    let deadline = Instant::now() + Duration::from_secs(1);
    while Instant::now() < deadline {
        match conn.poll_for_event()? {
            Some(Event::PropertyNotify(event)) if event.window == window => return Ok(event.time),
            Some(_) => {}
            None => thread::sleep(Duration::from_millis(2)),
        }
    }
    anyhow::bail!("the X server sent no PropertyNotify for the drag source window")
}

/// Answers one `XdndSelection` request: `TARGETS`, or the URI list for
/// `text/uri-list` and the plain-text types. Returns whether file data was
/// handed over.
fn serve_request(
    conn: &impl Connection,
    atoms: &Atoms,
    request: &SelectionRequestEvent,
    list: &str,
) -> anyhow::Result<bool> {
    let property = if request.property == NONE {
        request.target
    } else {
        request.property
    };
    let text_types = [
        atoms.uri_list,
        atoms.text_plain,
        atoms.text_plain_utf8,
        atoms.utf8_string,
    ];
    let is_data = request.selection == atoms.selection && text_types.contains(&request.target);
    let supported = is_data || request.target == atoms.targets;
    if request.target == atoms.targets {
        let mut targets = vec![atoms.targets];
        targets.extend(text_types);
        conn.change_property32(
            PropMode::REPLACE,
            request.requestor,
            property,
            AtomEnum::ATOM,
            &targets,
        )?
        .check()?;
    } else if is_data {
        conn.change_property8(
            PropMode::REPLACE,
            request.requestor,
            property,
            request.target,
            list.as_bytes(),
        )?
        .check()?;
    }
    conn.send_event(
        false,
        request.requestor,
        EventMask::NO_EVENT,
        SelectionNotifyEvent {
            response_type: SELECTION_NOTIFY_EVENT,
            sequence: 0,
            time: request.time,
            requestor: request.requestor,
            selection: request.selection,
            target: request.target,
            property: if supported { property } else { NONE },
        },
    )?
    .check()?;
    conn.flush()?;
    Ok(is_data)
}

fn atom_name(conn: &impl Connection, atom: Atom) -> Option<String> {
    let reply = conn.get_atom_name(atom).ok()?.reply().ok()?;
    String::from_utf8(reply.name).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_uris_percent_encode_everything_but_unreserved_bytes() {
        assert_eq!(
            file_uri(Path::new("/tmp/My File #1 (ação).pdf")),
            "file:///tmp/My%20File%20%231%20%28a%C3%A7%C3%A3o%29.pdf"
        );
        assert_eq!(
            file_uri(Path::new("/home/u/a-b_c.d~e")),
            "file:///home/u/a-b_c.d~e"
        );
    }

    #[test]
    fn uri_list_ends_every_line_in_crlf() {
        let list = uri_list(&[PathBuf::from("/a b"), PathBuf::from("/c")]);
        assert_eq!(list, "file:///a%20b\r\nfile:///c\r\n");
    }

    #[test]
    fn enter_carries_the_version_and_up_to_three_types() {
        assert_eq!(enter_data(7, 5, &[11, 12]), [7, 5 << 24, 11, 12, 0]);
        // More than three types sets bit 0; the rest live in XdndTypeList.
        assert_eq!(
            enter_data(7, 5, &[11, 12, 13, 14]),
            [7, (5 << 24) | 1, 11, 12, 13]
        );
    }

    #[test]
    fn position_packs_root_coordinates_into_one_word() {
        assert_eq!(
            position_data(7, (300, 200), 99, 42),
            [7, 0, (300 << 16) | 200, 99, 42]
        );
    }

    #[test]
    fn drop_and_leave_name_the_source_and_drop_the_time() {
        assert_eq!(drop_data(7, 99), [7, 0, 99, 0, 0]);
        assert_eq!(leave_data(7), [7, 0, 0, 0, 0]);
    }

    #[test]
    fn status_and_finished_accept_on_bit_zero() {
        assert!(accepts(&[3, 0b11, 0, 0, 42]));
        assert!(!accepts(&[3, 0b10, 0, 0, 0]));
    }

    #[test]
    fn a_stalled_x_server_fails_the_drop_instead_of_blocking() {
        let display = crate::x11_display::stalled_x_server();
        let started = Instant::now();
        let error = drop_files_on(
            &display,
            &[PathBuf::from("/tmp/a")],
            (5, 5),
            Duration::from_millis(100),
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("did not answer"), "{error:#}");
        assert!(started.elapsed() < X11_QUERY_TIMEOUT + Duration::from_secs(1));
    }

    #[test]
    fn a_drop_counts_only_when_accepted_and_finished_or_read() {
        let mut outcome = DropOutcome {
            accepted: true,
            ..Default::default()
        };
        assert!(!outcome.succeeded());
        outcome.served = true;
        assert!(outcome.succeeded());
        outcome.served = false;
        outcome.finished = Some(true);
        assert!(outcome.succeeded());
        outcome.accepted = false;
        assert!(!outcome.succeeded());
    }

    #[test]
    fn an_explicitly_refused_drop_fails_even_after_the_files_were_read() {
        let refused = DropOutcome {
            accepted: true,
            served: true,
            finished: Some(false),
            ..Default::default()
        };
        assert!(!refused.succeeded());
        assert_eq!(refused.failure(), Some("the page refused the drop"));
        let unread = DropOutcome {
            accepted: true,
            ..Default::default()
        };
        assert_eq!(
            unread.failure(),
            Some("the page accepted the drag but never read the files")
        );
        let finished = DropOutcome {
            accepted: true,
            finished: Some(true),
            ..Default::default()
        };
        assert_eq!(finished.failure(), None);
    }
}
