//! Every subprocess a build starts, and the two ways a caller reaches one.
//!
//! # One door
//!
//! [`command`] is the only place in this crate that constructs a
//! `std::process::Command` (`just spawn-confined` holds that). Two things are
//! true of every child it makes, whoever the caller is:
//!
//! - **stdin is closed.** Under `clwd mcp serve`, stdin is the protocol. A
//!   build script that read it would swallow the client's next request.
//! - **No `CLWD_*` variable is inherited.** `CLWD_TOKEN` is a platform
//!   credential, and cargo runs the project's build scripts and proc macros:
//!   arbitrary code that has no business holding it. Every `CLWD_` name is
//!   removed, not only the one in use today, so a credential added later is
//!   covered without an edit here.
//!
//! # Watching a build from outside
//!
//! [`Tail`] keeps the end of what a child wrote to stderr, for a caller with no
//! terminal to inherit it. [`Cancel`] stops a running build: it kills the
//! child, and on Unix the child's whole process group, so a `rustc` that cargo
//! started does not outlive the cancel.

use std::collections::VecDeque;
use std::ffi::OsStr;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, PoisonError};

/// How much of a child's stderr a [`Tail`] keeps: the last 64 KiB.
pub const TAIL_BYTES: usize = 64 * 1024;

/// A child process with stdin closed and no `CLWD_*` variable in its
/// environment.
pub(crate) fn command(program: impl AsRef<OsStr>) -> Command {
    let mut command = Command::new(program);
    command.stdin(Stdio::null());
    for (key, _) in std::env::vars_os() {
        if key.as_encoded_bytes().starts_with(b"CLWD_") {
            command.env_remove(key);
        }
    }
    command
}

/// The last [`TAIL_BYTES`] a child wrote to stderr, shared with the caller
/// that asked for them.
#[derive(Debug, Clone, Default)]
pub struct Tail(Arc<Mutex<TailState>>);

#[derive(Debug, Default)]
struct TailState {
    bytes: VecDeque<u8>,
    dropped: u64,
}

impl Tail {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn push(&self, chunk: &[u8]) {
        let mut state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        state.bytes.extend(chunk);
        let over = state.bytes.len().saturating_sub(TAIL_BYTES);
        if over > 0 {
            state.bytes.drain(..over);
            state.dropped += over as u64;
        }
    }

    /// Read `reader` to its end into the tail. Runs on its own thread, so a
    /// child that fills its stderr pipe never blocks on a reader busy
    /// elsewhere.
    pub(crate) fn drain(
        &self,
        mut reader: impl std::io::Read + Send + 'static,
    ) -> std::thread::JoinHandle<()> {
        let tail = self.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 {
                    break;
                }
                tail.push(&buf[..n]);
            }
        })
    }

    /// What was kept, as lines, and how many bytes before them were dropped.
    ///
    /// When bytes were dropped, the first kept line is partial and is dropped
    /// too: half a line reads as a different message.
    #[must_use]
    pub fn lines(&self) -> (Vec<String>, u64) {
        let state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let bytes: Vec<u8> = state.bytes.iter().copied().collect();
        let mut text = String::from_utf8_lossy(&bytes).into_owned();
        let mut dropped = state.dropped;
        if dropped > 0
            && let Some(newline) = text.find('\n')
        {
            dropped += newline as u64 + 1;
            text.drain(..=newline);
        }
        (text.lines().map(str::to_owned).collect(), dropped)
    }
}

/// Stops a build from another thread.
#[derive(Debug, Clone, Default)]
pub struct Cancel(Arc<Mutex<CancelState>>);

#[derive(Debug, Default)]
struct CancelState {
    cancelled: bool,
    child: Option<u32>,
}

impl Cancel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Stop the build: kill its running child, and refuse to start another.
    pub fn cancel(&self) {
        let mut state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        state.cancelled = true;
        if let Some(pid) = state.child {
            kill(pid);
        }
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .cancelled
    }

    /// Spawn `command` as the child this handle stops, in its own process
    /// group on Unix so killing it reaches every process it started.
    ///
    /// Only a cancellable build gets its own group. A terminal sends Ctrl-C to
    /// the foreground group, and a CLI build's cargo must stay in it.
    pub(crate) fn spawn(&self, command: &mut Command) -> std::io::Result<Child> {
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(command, 0);
        let mut state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if state.cancelled {
            return Err(std::io::Error::other("the build was cancelled"));
        }
        let child = command.spawn()?;
        state.child = Some(child.id());
        Ok(child)
    }

    /// The child has been waited for: its id may be reused, so never signal it.
    pub(crate) fn release(&self) {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).child = None;
    }
}

/// Kill the process group `pid` leads.
#[cfg(all(unix, feature = "mcp"))]
fn kill(pid: u32) {
    if let Some(pid) = i32::try_from(pid)
        .ok()
        .and_then(rustix::process::Pid::from_raw)
    {
        // Already gone is fine: the group exits on its own when the build ends.
        let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
    }
}

/// Without a way to signal a group, the top process only. Windows, and builds
/// without `mcp`, which never cancel.
#[cfg(not(all(unix, feature = "mcp")))]
fn kill(pid: u32) {
    // ponytail: top process only; a Job Object would reach its children too.
    let _ = command(if cfg!(windows) { "taskkill" } else { "kill" })
        .args(if cfg!(windows) {
            vec!["/F".to_owned(), "/PID".to_owned(), pid.to_string()]
        } else {
            vec!["-KILL".to_owned(), pid.to_string()]
        })
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tail_keeps_the_end_and_drops_the_partial_first_line() {
        let tail = Tail::new();
        tail.push(b"first line\n");
        tail.push(&vec![b'x'; TAIL_BYTES]);
        tail.push(b"\nlast line\n");
        let (lines, dropped) = tail.lines();
        assert_eq!(lines, ["last line"], "the partial line before it is gone");
        assert_eq!(dropped, (11 + TAIL_BYTES + 1) as u64);
    }

    #[test]
    fn a_short_tail_is_kept_whole() {
        let tail = Tail::new();
        tail.push(b"a\nb\n");
        assert_eq!(tail.lines(), (vec!["a".to_owned(), "b".to_owned()], 0));
    }

    #[test]
    fn a_cancelled_handle_starts_nothing() {
        let cancel = Cancel::new();
        cancel.cancel();
        let mut command = command("true");
        assert!(cancel.spawn(&mut command).is_err());
    }
}
