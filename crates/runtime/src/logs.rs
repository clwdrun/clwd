//! Log lines and the stream they arrive on.

use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::SystemTime;

use futures_core::Stream;

use crate::call::RequestId;
use crate::error::Error;

/// Severity of one log line.
///
/// A backend reading raw guest stdout has no level to report and uses
/// [`LogLevel::Info`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

/// Which of a guest's output streams a line came from.
///
/// Separate from [`LogLevel`] on purpose: a guest writing to stderr is not
/// necessarily reporting an error, and folding the two together would make
/// every debug print look like a failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogStreamKind {
    Stdout,
    Stderr,
}

/// One line of output from a running instance.
///
/// `#[non_exhaustive]`: this grows. #41 will want more correlation fields.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct LogLine {
    /// Wall clock, not a monotonic instant: these lines get correlated across
    /// hosts and processes, and a monotonic reading means nothing off the
    /// machine that took it.
    pub timestamp: SystemTime,
    pub level: LogLevel,
    pub message: String,
    /// The invocation this line belongs to.
    ///
    /// `None` for instance-level lines — startup, teardown — which genuinely
    /// have no request. A sentinel value here would make correlation lie.
    pub request_id: Option<RequestId>,
    /// Which guest stream produced this line.
    ///
    /// **`None` means the backend does not distinguish the two**, not that the
    /// line came from neither. The wasm backend reads stdout and stderr
    /// separately and always sets this; a backend that cannot tell them apart
    /// reports `None` rather than guessing.
    pub stream: Option<LogStreamKind>,
}

impl LogLine {
    /// A line stamped now, with no request correlation.
    ///
    /// `#[non_exhaustive]` blocks struct literals outside this crate, so build
    /// here and assign:
    ///
    /// ```
    /// # use clwd_runtime::{call::RequestId, logs::{LogLevel, LogLine}};
    /// let mut line = LogLine::new(LogLevel::Warn, "disk almost full");
    /// line.request_id = Some(RequestId::new("req-7"));
    /// ```
    pub fn new(level: LogLevel, message: impl Into<String>) -> Self {
        Self {
            timestamp: SystemTime::now(),
            level,
            message: message.into(),
            request_id: None,
            stream: None,
        }
    }
}

/// The stream `Sandbox::logs_stream` returns.
///
/// A named type rather than `impl Stream`, so #32 has something to write an
/// adapter against and a backend whose only return path today is an error still
/// has a concrete type to name.
pub struct LogStream {
    inner: Pin<Box<dyn Stream<Item = Result<LogLine, Error>> + Send>>,
}

impl LogStream {
    /// Wrap any stream of log lines.
    pub fn new<S>(stream: S) -> Self
    where
        S: Stream<Item = Result<LogLine, Error>> + Send + 'static,
    {
        Self {
            inner: Box::pin(stream),
        }
    }

    /// A stream that ends immediately, for a backend with nothing to say.
    pub fn empty() -> Self {
        Self::new(Empty)
    }
}

impl std::fmt::Debug for LogStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LogStream").finish_non_exhaustive()
    }
}

impl Stream for LogStream {
    type Item = Result<LogLine, Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.inner.as_mut().poll_next(cx)
    }
}

/// `futures-core` defines the `Stream` trait and no combinators, so the empty
/// stream is three lines here rather than a dependency on `futures-util`.
struct Empty;

impl Stream for Empty {
    type Item = Result<LogLine, Error>;

    fn poll_next(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Ready(None)
    }
}
