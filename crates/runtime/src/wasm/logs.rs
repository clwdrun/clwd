//! Guest stdout and stderr capture.
//!
//! `wasmtime_wasi::p2::pipe::MemoryOutputPipe` is nearly what is needed but
//! **traps the guest** once its capacity is reached. Killing an app because
//! nobody drained its logs is a worse failure than losing log lines, so this
//! sink drops instead and counts what it dropped.

use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::Arc;

use parking_lot::Mutex;
use std::task::{Context, Poll};

use bytes::Bytes;
use tokio::sync::mpsc;
use wasmtime_wasi::cli::{IsTerminal, StdoutStream};
use wasmtime_wasi::p2::{OutputStream, Pollable, StreamError};

use crate::call::RequestId;
use crate::error::Error;
use crate::logs::{LogLevel, LogLine, LogStreamKind};

/// Bytes buffered per stream, per invocation, before lines start being dropped.
const SINK_CAPACITY: usize = 256 * 1024;
/// Lines retained per instance for a subscriber that attaches late.
const HISTORY_LINES: usize = 1024;
/// Depth of one subscriber's channel. A subscriber that stops reading loses
/// lines; it never slows the guest down.
const SUBSCRIBER_DEPTH: usize = 256;

#[derive(Default)]
struct Buffer {
    /// Complete lines, split on `\n`.
    lines: Vec<String>,
    /// Trailing bytes with no newline yet.
    partial: Vec<u8>,
    /// Bytes held in `lines` and `partial`, kept as a running count. Summing
    /// the lines on every write is quadratic in the line count, and a guest
    /// writing one byte per line chooses that count.
    used: usize,
    dropped: u64,
}

/// One guest stream — stdout or stderr — for the duration of one invocation.
#[derive(Clone)]
pub(super) struct LogSink {
    buffer: Arc<Mutex<Buffer>>,
    stream: LogStreamKind,
}

impl LogSink {
    pub(super) fn new(stream: LogStreamKind) -> Self {
        Self {
            buffer: Arc::new(Mutex::new(Buffer::default())),
            stream,
        }
    }

    /// Take everything captured, as `LogLine`s belonging to `request`.
    pub(super) fn drain(&self, request: Option<&RequestId>) -> Vec<LogLine> {
        let mut buffer = self.buffer.lock();
        let mut out: Vec<LogLine> = Vec::with_capacity(buffer.lines.len() + 1);

        for text in buffer.lines.drain(..) {
            out.push(self.line(text, request));
        }
        if !buffer.partial.is_empty() {
            // A guest that exits without a trailing newline still wrote
            // something; losing it would be the surprising behaviour.
            let text = String::from_utf8_lossy(&buffer.partial).into_owned();
            buffer.partial.clear();
            out.push(self.line(text, request));
        }
        buffer.used = 0;
        if buffer.dropped > 0 {
            let dropped = std::mem::take(&mut buffer.dropped);
            out.push(self.line(
                format!("[clwd] {dropped} bytes of output dropped: sink full"),
                request,
            ));
        }
        out
    }

    fn line(&self, message: String, request: Option<&RequestId>) -> LogLine {
        // Level is `Info` for both streams. A guest writing to stderr is not
        // necessarily reporting an error, and mapping stderr to `Error` would
        // make every debug print look like a failure.
        let mut line = LogLine::new(LogLevel::Info, message);
        line.request_id = request.cloned();
        line.stream = Some(self.stream);
        line
    }
}

impl IsTerminal for LogSink {
    fn is_terminal(&self) -> bool {
        false
    }
}

impl StdoutStream for LogSink {
    fn p2_stream(&self) -> Box<dyn OutputStream> {
        Box::new(self.clone())
    }

    fn async_stream(&self) -> Box<dyn tokio::io::AsyncWrite + Send + Sync> {
        Box::new(self.clone())
    }
}

impl OutputStream for LogSink {
    fn write(&mut self, bytes: Bytes) -> Result<(), StreamError> {
        let mut buffer = self.buffer.lock();
        // Once anything is dropped, everything after it is too. Guests write a
        // line's text and its newline separately, so a sink that took the small
        // writes after a large one was refused would turn each refused line
        // into an empty one, and enough of those push the drop marker past a
        // subscriber's buffer.
        if buffer.dropped > 0 || buffer.used + bytes.len() > SINK_CAPACITY {
            // Drop rather than trap. See the module comment.
            buffer.dropped += bytes.len() as u64;
            return Ok(());
        }
        buffer.used += bytes.len();
        for byte in bytes.iter().copied() {
            if byte == b'\n' {
                let text = String::from_utf8_lossy(&buffer.partial).into_owned();
                buffer.partial.clear();
                buffer.lines.push(text);
            } else {
                buffer.partial.push(byte);
            }
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<(), StreamError> {
        Ok(())
    }

    fn check_write(&mut self) -> Result<usize, StreamError> {
        // Always writable: a full sink drops rather than closing, so the guest
        // never observes backpressure from the log path.
        Ok(SINK_CAPACITY)
    }
}

#[wasmtime_wasi::async_trait]
impl Pollable for LogSink {
    async fn ready(&mut self) {}
}

impl tokio::io::AsyncWrite for LogSink {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let mut this = self.clone();
        let _ = OutputStream::write(&mut this, Bytes::copy_from_slice(buf));
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

/// Everything one instance has logged, plus whoever is listening.
#[derive(Default)]
pub(super) struct InstanceLogs {
    inner: Mutex<LogState>,
}

#[derive(Default)]
struct LogState {
    history: VecDeque<LogLine>,
    subscribers: Vec<mpsc::Sender<LogLine>>,
}

impl InstanceLogs {
    /// Record lines and fan them out. A subscriber whose channel is full loses
    /// lines; the guest is never slowed down by a slow reader.
    pub(super) fn publish(&self, lines: Vec<LogLine>) {
        let mut state = self.inner.lock();
        for line in lines {
            state.subscribers.retain(|tx| !tx.is_closed());
            for tx in &state.subscribers {
                let _ = tx.try_send(line.clone());
            }
            state.history.push_back(line);
            while state.history.len() > HISTORY_LINES {
                state.history.pop_front();
            }
        }
    }

    /// A stream of what has been logged so far, then whatever follows.
    pub(super) fn subscribe(&self) -> LogSubscription {
        let (tx, rx) = mpsc::channel(SUBSCRIBER_DEPTH);
        let mut state = self.inner.lock();
        for line in state.history.iter().take(SUBSCRIBER_DEPTH) {
            let _ = tx.try_send(line.clone());
        }
        state.subscribers.push(tx);
        LogSubscription { rx }
    }
}

/// One subscriber's view. Held by [`crate::logs::LogStream`].
pub(super) struct LogSubscription {
    rx: mpsc::Receiver<LogLine>,
}

impl futures_core::Stream for LogSubscription {
    type Item = Result<LogLine, Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.rx.poll_recv(cx).map(|line| line.map(Ok))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The cap is per sink, so per stream per invocation: a guest that writes
    /// past it keeps at most `SINK_CAPACITY` bytes, loses the rest, and leaves
    /// one line saying how much. Written the way a guest's `println!` arrives:
    /// the text, then its newline as a separate write.
    #[test]
    fn output_past_the_cap_is_dropped_and_counted_once() {
        let mut sink = LogSink::new(LogStreamKind::Stdout);
        let text = Bytes::from(vec![b'x'; 4096]);
        for _ in 0..300 {
            OutputStream::write(&mut sink, text.clone()).expect("never fails");
            OutputStream::write(&mut sink, Bytes::from_static(b"\n")).expect("never fails");
        }

        let lines = sink.drain(None);
        assert!(
            lines.iter().all(|l| !l.message.is_empty()),
            "a refused line must not come back as an empty one"
        );
        assert!(
            lines.len() <= SINK_CAPACITY / 4096 + 1,
            "{} lines",
            lines.len()
        );
        let kept: usize = lines.iter().map(|l| l.message.len() + 1).sum::<usize>();
        let markers: Vec<_> = lines
            .iter()
            .filter(|l| l.message.starts_with("[clwd]"))
            .collect();
        assert_eq!(markers.len(), 1, "exactly one drop marker");
        let written = 300 * 4097;
        let kept_text = (lines.len() - 1) * 4097;
        assert!(
            markers[0]
                .message
                .contains(&format!("{} bytes of output dropped", written - kept_text)),
            "{}",
            markers[0].message
        );
        assert!(kept <= SINK_CAPACITY + markers[0].message.len() + 1);

        // Draining resets the count, so the next invocation starts empty.
        OutputStream::write(&mut sink, Bytes::from_static(b"next\n")).expect("never fails");
        let next = sink.drain(None);
        assert_eq!(next.len(), 1);
        assert_eq!(next[0].message, "next");
    }
}
