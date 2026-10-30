//! Display-wide input lock shared by every computer-use-linux process.
//!
//! Each agent session starts its own server, and all of them drive the same X
//! display: one pointer, one keyboard focus. An exclusive `flock` on a file
//! named after the display serializes pointer, focus and keyboard operations
//! across those processes. The kernel drops the lock when its holder exits or
//! is killed, so an agent killed mid-operation never leaves the display locked.
//! An `xdotool` started under the lock holds it too (`hand_down_to`), so the
//! input it is still sending when its server is killed ends before another
//! agent's starts.

use std::fs::{DirBuilder, File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::{Duration, Instant};

/// Longest wait for another process's input operation, in milliseconds.
const LOCK_TIMEOUT_ENV: &str = "COMPUTER_USE_LINUX_INPUT_LOCK_TIMEOUT_MS";
/// A long `type_text` takes a few seconds; waiting longer than this usually
/// means another agent holds the display for a long scroll or drag.
const DEFAULT_LOCK_TIMEOUT: Duration = Duration::from_secs(10);
/// First and longest pause between attempts while the lock is held.
const RETRY_FIRST: Duration = Duration::from_millis(5);
const RETRY_MAX: Duration = Duration::from_millis(50);

/// Descriptor of the display lock while this process holds it, else -1.
/// A plain atomic, because a forked child reads it before `exec`.
static HELD_FD: AtomicI32 = AtomicI32::new(-1);

/// Holds the display's input lock until dropped. Closing the file releases
/// the `flock` unless a child got the descriptor from `hand_down_to`; it is
/// opened close-on-exec, so no other child inherits it.
#[derive(Debug)]
pub(crate) struct DisplayInputLock {
    file: File,
}

impl DisplayInputLock {
    fn held(file: File) -> Self {
        HELD_FD.store(file.as_raw_fd(), Ordering::SeqCst);
        Self { file }
    }
}

impl Drop for DisplayInputLock {
    fn drop(&mut self) {
        // Cleared before `file` closes, so a child forked in between never
        // keeps a descriptor that reused the number.
        let _ = HELD_FD.compare_exchange(
            self.file.as_raw_fd(),
            -1,
            Ordering::SeqCst,
            Ordering::SeqCst,
        );
    }
}

/// Lets `command`, spawned while this process may hold the display lock,
/// inherit it. If the server is killed while the command still sends input,
/// the lock lasts until the command exits instead of letting another agent's
/// input land in the middle of it.
pub(crate) fn hand_down_to(command: &mut tokio::process::Command) {
    // SAFETY: the hook runs between fork and exec and only loads an atomic
    // and calls fcntl, both async-signal-safe. Clearing FD_CLOEXEC changes
    // the child's descriptor table only.
    unsafe {
        command.pre_exec(|| {
            let fd = HELD_FD.load(Ordering::SeqCst);
            if fd >= 0 {
                libc::fcntl(fd, libc::F_SETFD, 0);
            }
            Ok(())
        });
    }
}

/// Takes the input lock of `$DISPLAY`, waiting up to the configured timeout
/// while another process holds it. The error says the display is busy and
/// that nothing was sent, so the agent can retry.
pub(crate) async fn lock_display_input() -> Result<DisplayInputLock, String> {
    let display = std::env::var("DISPLAY").unwrap_or_default();
    lock_with_timeout(&lock_path(&display), &display, lock_timeout()).await
}

/// `$XDG_RUNTIME_DIR/computer-use-linux/input-<display>.lock`, or a per-user
/// directory in `/tmp` when the runtime directory is unset.
pub(crate) fn lock_path(display: &str) -> PathBuf {
    let dir = match std::env::var_os("XDG_RUNTIME_DIR").filter(|dir| !dir.is_empty()) {
        Some(dir) => PathBuf::from(dir).join("computer-use-linux"),
        None => std::env::temp_dir().join(format!(
            "computer-use-linux-{}",
            // SAFETY: getuid has no preconditions and cannot fail.
            unsafe { libc::getuid() }
        )),
    };
    dir.join(format!("input-{}.lock", display_key(display)))
}

/// One key per X server: `:99`, `:99.0` and `unix:99` share a pointer and a
/// keyboard, so they share a lock.
fn display_key(display: &str) -> String {
    let display = display.trim();
    let (host, rest) = display.rsplit_once(':').unwrap_or(("", display));
    let number = rest.split('.').next().unwrap_or_default();
    let host = if host == "unix" { "" } else { host };
    let key = if host.is_empty() {
        number.to_string()
    } else {
        format!("{host}_{number}")
    };
    let key: String = key
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    if key.is_empty() {
        "unset".to_string()
    } else {
        key
    }
}

fn lock_timeout() -> Duration {
    std::env::var(LOCK_TIMEOUT_ENV)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(Duration::from_millis)
        .unwrap_or(DEFAULT_LOCK_TIMEOUT)
}

fn open_lock_file(path: &Path) -> io::Result<File> {
    if let Some(dir) = path.parent() {
        DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    }
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path)
}

/// One non-blocking attempt on an open lock file: `Ok(false)` while another
/// open file description (any process, or this one) holds it.
fn try_flock(file: &File) -> io::Result<bool> {
    loop {
        // SAFETY: the descriptor is owned by `file` and stays open for the call.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(true);
        }
        let error = io::Error::last_os_error();
        match error.raw_os_error() {
            Some(libc::EWOULDBLOCK) => return Ok(false),
            Some(libc::EINTR) => continue,
            _ => return Err(error),
        }
    }
}

/// Takes the lock at `path` without waiting; `None` while another holds it.
/// Unlike `lock_display_input`, it is never handed down to input commands,
/// so tests running beside `xdotool` tests keep their locks to themselves.
#[cfg(test)]
pub(crate) fn try_lock(path: &Path) -> io::Result<Option<DisplayInputLock>> {
    let file = open_lock_file(path)?;
    Ok(try_flock(&file)?.then_some(DisplayInputLock { file }))
}

/// Polls with a short backoff instead of a blocking `flock`, so waiting never
/// ties up a runtime thread and the wait stays bounded.
async fn lock_with_timeout(
    path: &Path,
    display: &str,
    timeout: Duration,
) -> Result<DisplayInputLock, String> {
    let file = open_lock_file(path)
        .map_err(|error| format!("Could not open the input lock {}: {error}", path.display()))?;
    let started = Instant::now();
    let mut pause = RETRY_FIRST;
    loop {
        match try_flock(&file) {
            Ok(true) => return Ok(DisplayInputLock::held(file)),
            Ok(false) => {}
            Err(error) => {
                return Err(format!(
                    "Could not take the input lock {}: {error}",
                    path.display()
                ))
            }
        }
        let waited = started.elapsed();
        if waited >= timeout {
            let display = if display.is_empty() {
                "(unset)"
            } else {
                display
            };
            return Err(format!(
                "Display {display} is busy: another agent kept its pointer and keyboard for the whole {:.1} s wait. Nothing was sent; retry the call. (lock {}, wait limit {LOCK_TIMEOUT_ENV})",
                waited.as_secs_f64(),
                path.display()
            ));
        }
        tokio::time::sleep(pause.min(timeout - waited)).await;
        pause = (pause * 2).min(RETRY_MAX);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};

    /// Whether the lock at `path` can be taken within a second. A child that
    /// another test forks keeps every descriptor until its `exec`, so a
    /// dropped lock may stay held for that moment.
    fn frees_up(path: &Path) -> bool {
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            if try_lock(path).unwrap().is_some() {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn temp_lock(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("cul-input-lock-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("input-test.lock")
    }

    #[test]
    fn display_names_of_one_server_share_a_key() {
        assert_eq!(display_key(":99"), "99");
        assert_eq!(display_key(":99.0"), "99");
        assert_eq!(display_key("unix:99"), "99");
        assert_eq!(display_key("localhost:10.0"), "localhost_10");
        assert_eq!(display_key(""), "unset");
        assert_ne!(display_key(":98"), display_key(":99"));
        assert!(lock_path(":99").ends_with("input-99.lock"));
    }

    #[test]
    fn a_second_holder_is_refused_until_the_first_drops() {
        let path = temp_lock("exclusive");
        let first = try_lock(&path).unwrap().expect("free lock was refused");
        // A second open file description conflicts even inside one process,
        // the same way a second server process does.
        assert!(try_lock(&path).unwrap().is_none());
        drop(first);
        assert!(frees_up(&path));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_killed_holder_process_releases_the_lock() {
        let path = temp_lock("killed");
        open_lock_file(&path).unwrap();
        // One process owns the descriptor: the shell opens it, flock(1) locks
        // that open file, and the shell becomes `sleep`, still holding it.
        let mut holder = Command::new("sh")
            .args([
                "-c",
                "exec 9>\"$1\" && flock --exclusive 9 && echo held && exec sleep 30",
                "holder",
            ])
            .arg(&path)
            .stdout(Stdio::piped())
            .spawn()
            .expect("flock(1) from util-linux is needed for this test");
        let mut line = String::new();
        BufReader::new(holder.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        assert_eq!(line.trim(), "held");
        assert!(
            try_lock(&path).unwrap().is_none(),
            "another process's lock was not seen"
        );

        holder.kill().unwrap();
        holder.wait().unwrap();
        assert!(frees_up(&path), "the lock outlived its killed holder");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_handed_down_lock_outlives_its_holder_until_the_child_exits() {
        // HELD_FD is process-wide and other tests take locks in parallel, so
        // the scenario runs alone in a child test process.
        const CHILD: &str = "COMPUTER_USE_LINUX_TEST_INPUT_LOCK_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let status = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "input_lock::tests::a_handed_down_lock_outlives_its_holder_until_the_child_exits",
                    "--nocapture",
                ])
                .env(CHILD, "1")
                .status()
                .unwrap();
            assert!(status.success());
            return;
        }
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let path = temp_lock("handdown");
        let sleeper = |hand_down: bool| {
            let mut command = tokio::process::Command::new("sleep");
            command.arg("30").kill_on_drop(true);
            if hand_down {
                hand_down_to(&mut command);
            }
            runtime.block_on(async { command.spawn().unwrap() })
        };

        // Other children never inherit the lock.
        let lock = runtime
            .block_on(lock_with_timeout(&path, ":98", Duration::ZERO))
            .unwrap();
        let bystander = sleeper(false);
        drop(lock);
        assert!(frees_up(&path), "a child kept the lock");
        drop(bystander);

        // An input command keeps it after the server lets go (or dies).
        let lock = runtime
            .block_on(lock_with_timeout(&path, ":98", Duration::ZERO))
            .unwrap();
        let mut input = sleeper(true);
        drop(lock);
        assert!(
            try_lock(&path).unwrap().is_none(),
            "the lock left while the input command still ran"
        );
        runtime.block_on(async {
            input.start_kill().unwrap();
            input.wait().await.unwrap();
        });
        assert!(frees_up(&path), "the lock outlived the input command");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[tokio::test]
    async fn waiting_gives_up_after_the_timeout_with_a_retry_message() {
        let path = temp_lock("timeout");
        let _held = try_lock(&path).unwrap().unwrap();
        let started = Instant::now();
        let error = lock_with_timeout(&path, ":98", Duration::from_millis(200))
            .await
            .unwrap_err();
        let waited = started.elapsed();
        assert!(waited >= Duration::from_millis(200), "{waited:?}");
        assert!(waited < Duration::from_secs(2), "{waited:?}");
        assert!(error.contains("Display :98 is busy"), "{error}");
        assert!(error.contains("retry"), "{error}");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[tokio::test]
    async fn a_waiter_gets_the_lock_once_the_holder_drops_it() {
        let path = temp_lock("handoff");
        let held = try_lock(&path).unwrap().unwrap();
        let waiter_path = path.clone();
        let waiter = tokio::spawn(async move {
            lock_with_timeout(&waiter_path, ":98", Duration::from_secs(5)).await
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!waiter.is_finished(), "waiter took a held lock");
        drop(held);
        let lock = tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .expect("waiter did not take the released lock")
            .unwrap();
        assert!(lock.is_ok());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
