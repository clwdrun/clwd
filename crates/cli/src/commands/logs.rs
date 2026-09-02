//! `clwd logs` — stream logs for an app. Body lands with #26.
//!
//! This returns a stream rather than a collection from the start, deliberately.
//! A `Vec` that becomes a stream later would change #32's adapter design
//! underneath it, and avoiding exactly that is why the lib/bin split exists.

use std::fmt;
use std::pin::Pin;
use std::task::{Context, Poll};

use futures_core::Stream;
use serde::Serialize;

use crate::Progress;
use crate::error::CliError;

/// One line of output from a running app.
///
/// #26 decides what else belongs here — timestamp, stream, instance. Adding a
/// field is additive for both renderers, so it can wait for a real source.
#[derive(Debug, Serialize)]
pub struct LogLine {
    pub message: String,
}

impl fmt::Display for LogLine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

/// The stream `execute` returns.
///
/// Named rather than `impl Stream` for two reasons: a function whose only
/// return path today is `Err` gives the compiler nothing to infer an opaque
/// type from, and #32 needs a type it can write an adapter against.
#[derive(Debug)]
pub struct LogStream;

impl Stream for LogStream {
    type Item = Result<LogLine, CliError>;

    fn poll_next(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // No source yet (#26). Terminating immediately is the honest empty
        // stream; `execute` never hands one out in the first place.
        Poll::Ready(None)
    }
}

pub fn execute(_follow: bool, _progress: Option<&dyn Progress>) -> Result<LogStream, CliError> {
    Err(CliError::NotImplemented { command: "logs" })
}
