//! The only module that turns values into bytes a human or a machine reads.
//!
//! Nothing here reaches for stdout or stderr. The caller passes the sink, which
//! is what lets `clwd mcp serve` (#32) render into a buffer it controls instead
//! of into a stream it must keep free of stray output.

use std::fmt;
use std::io::{self, Write};

use serde::Serialize;

use crate::error::CliError;

/// Which of the two renderers to use. Selected once, from `--json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Human,
    Json,
}

/// The JSON error envelope. #31 owns the guarantee that this shape and the
/// codes inside it are stable; #14 only owns emitting them.
#[derive(Debug, Serialize)]
struct ErrorEnvelope<'a> {
    code: &'a str,
    message: String,
    hint: Option<&'a str>,
    retryable: bool,
}

impl<'a> From<&'a CliError> for ErrorEnvelope<'a> {
    fn from(err: &'a CliError) -> Self {
        Self {
            code: err.code(),
            message: err.to_string(),
            hint: err.hint(),
            retryable: err.retryable(),
        }
    }
}

impl Format {
    /// Render one successful value.
    ///
    /// Streaming commands call this once per item, which is why JSON output is
    /// one object per line rather than a wrapping array — a `logs --follow`
    /// that never ends has no closing bracket to write.
    pub fn value<T>(self, value: &T, w: &mut dyn Write) -> io::Result<()>
    where
        T: fmt::Display + Serialize,
    {
        match self {
            Self::Human => writeln!(w, "{value}"),
            Self::Json => {
                serde_json::to_writer(&mut *w, value).map_err(io::Error::other)?;
                writeln!(w)
            }
        }
    }

    /// Render an error. The caller decides which stream this lands on.
    pub fn error(self, err: &CliError, w: &mut dyn Write) -> io::Result<()> {
        match self {
            Self::Human => {
                writeln!(w, "error: {err}")?;
                if let Some(hint) = err.hint() {
                    writeln!(w, "hint: {hint}")?;
                }
                Ok(())
            }
            Self::Json => {
                serde_json::to_writer(&mut *w, &ErrorEnvelope::from(err))
                    .map_err(io::Error::other)?;
                writeln!(w)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render_error(format: Format, err: &CliError) -> String {
        let mut buf = Vec::new();
        format
            .error(err, &mut buf)
            .expect("writing to a Vec cannot fail");
        String::from_utf8(buf).expect("renderers emit utf-8")
    }

    #[test]
    fn json_error_has_the_declared_envelope_shape() {
        let err = CliError::NotImplemented { command: "run" };
        let json: serde_json::Value =
            serde_json::from_str(&render_error(Format::Json, &err)).expect("valid json");

        let object = json.as_object().expect("envelope is an object");
        let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["code", "hint", "message", "retryable"]);

        assert_eq!(object["code"], "cli.not_implemented");
        assert_eq!(object["retryable"], false);
    }

    #[test]
    fn json_error_keeps_a_null_hint_rather_than_dropping_the_key() {
        let err = CliError::NotFound { what: "app".into() };
        let json: serde_json::Value =
            serde_json::from_str(&render_error(Format::Json, &err)).expect("valid json");
        assert!(json["hint"].is_null(), "hint must stay present as null");
    }

    #[test]
    fn human_error_shows_the_hint_when_there_is_one() {
        let rendered = render_error(Format::Human, &CliError::NotImplemented { command: "run" });
        assert!(rendered.starts_with("error: "), "got {rendered:?}");
        assert!(rendered.contains("\nhint: "), "got {rendered:?}");
    }

    #[test]
    fn json_values_are_one_object_per_line() {
        #[derive(Serialize)]
        struct Line {
            n: u8,
        }
        impl fmt::Display for Line {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.n)
            }
        }

        let mut buf = Vec::new();
        for n in 0..3 {
            Format::Json
                .value(&Line { n }, &mut buf)
                .expect("vec write");
        }
        let text = String::from_utf8(buf).expect("utf-8");
        assert_eq!(text, "{\"n\":0}\n{\"n\":1}\n{\"n\":2}\n");
    }
}
