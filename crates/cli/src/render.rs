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

    /// A `None` hint serializes as `null` rather than vanishing, so a consumer
    /// can index the key unconditionally.
    ///
    /// The envelope is built directly here rather than from a `CliError`,
    /// because every variant now carries a hint and none can demonstrate the
    /// absent case. That is deliberate — a null hint is a dead end for the
    /// reader least able to work out what to do next — but the shape must stay
    /// guaranteed regardless: `hint` is `Option` in the published contract, the
    /// platform mirrors it as nullable, and a future variant may legitimately
    /// have no next step.
    #[test]
    fn a_none_hint_serializes_as_null_rather_than_being_dropped() {
        let envelope = ErrorEnvelope {
            code: "cli.example",
            message: "an error with no hint".to_owned(),
            hint: None,
            retryable: false,
        };
        let json: serde_json::Value =
            serde_json::to_value(&envelope).expect("the envelope serializes");

        assert!(
            json.as_object().expect("an object").contains_key("hint"),
            "the hint key must be present even when there is no hint"
        );
        assert!(json["hint"].is_null(), "hint must stay present as null");
    }

    /// The counterpart: every variant that actually exists does carry a hint,
    /// so no real error reaches a caller as a dead end.
    #[test]
    fn every_variant_carries_a_hint() {
        for err in crate::error::tests::samples() {
            assert!(
                err.hint().is_some(),
                "{} has no hint — in the envelope that is the only recovery a \
                 caller sees",
                err.code()
            );
        }
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
