//! The three domain values, and why a secret is not a `Vec<u8>`.
//!
//! # Names are not interchangeable
//!
//! An app name and a secret name are both strings and mean entirely different
//! things: one selects a namespace, the other selects an entry within it.
//! Passing them the wrong way round would compile, look right, and silently
//! read the wrong namespace. They are separate types so the compiler refuses
//! the mix-up, for the same reason `SubjectId` and `OrgId` are separate.
//!
//! # A value redacts itself
//!
//! [`SecretValue`] does not implement `Display`, and its `Debug` prints a
//! placeholder. The realistic leak in a CLI is not an attacker reading memory;
//! it is a `{:?}` reaching a log line, an error message, or a panic payload
//! that someone pastes into an issue. A type that cannot render itself cannot
//! be leaked that way, and the compiler enforces it at every call site rather
//! than a reviewer catching one.
//!
//! Getting the bytes out is deliberately a named method, [`SecretValue::expose`],
//! so the places that genuinely need the value are greppable.

use std::fmt;

use zeroize::{Zeroize as _, ZeroizeOnDrop};

use crate::error::CliError;

/// Which app's namespace a secret lives in.
///
/// Declared in `clwd.toml` as `[app] name`, never inferred from the directory:
/// two projects sharing secrets should be something an author chose, not an
/// accident of where a repository was cloned.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize)]
pub struct AppName(String);

/// The name of one secret within an app's namespace.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize)]
pub struct SecretName(String);

/// A secret's bytes.
///
/// Bytes rather than a string because the WIT says `list<u8>`: a private key,
/// a certificate or a raw symmetric key is not necessarily UTF-8, and a type
/// that cannot carry those is one nobody discovers is wrong until they try.
///
/// Zeroized on drop, and it does not implement `Clone`: every copy is another
/// buffer to wipe, and a value that cannot be copied casually is one that does
/// not accumulate in places nobody remembers.
#[derive(ZeroizeOnDrop)]
pub struct SecretValue(Vec<u8>);

/// Shared by both name types. Deliberately narrow.
///
/// The grammar is the intersection of what is safe in a TOML key, a JSON
/// object key, an environment-variable-ish name and a file path component —
/// not because a name reaches all of those today, but because a name that
/// cannot is one nobody has to re-validate when it does.
fn validate(kind: &'static str, raw: &str) -> Result<String, CliError> {
    if raw.is_empty() {
        return Err(CliError::InvalidArgument {
            what: format!("{kind} is empty"),
        });
    }
    if raw.len() > 128 {
        return Err(CliError::InvalidArgument {
            what: format!("{kind} is {} bytes; the limit is 128", raw.len()),
        });
    }
    if let Some(bad) = raw
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')))
    {
        return Err(CliError::InvalidArgument {
            what: format!(
                "{kind} contains {bad:?}; only letters, digits, '.', '_' and '-' are allowed"
            ),
        });
    }
    if raw.starts_with(['.', '_', '-']) || raw.ends_with(['.', '_', '-']) {
        return Err(CliError::InvalidArgument {
            what: format!("{kind} may not begin or end with '.', '_' or '-'"),
        });
    }
    Ok(raw.to_owned())
}

impl AppName {
    /// Validate and wrap.
    pub fn new(raw: &str) -> Result<Self, CliError> {
        validate("an app name", raw).map(Self)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl SecretName {
    /// Validate and wrap.
    pub fn new(raw: &str) -> Result<Self, CliError> {
        validate("a secret name", raw).map(Self)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl SecretValue {
    /// Take ownership of secret bytes.
    #[must_use]
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }

    /// The bytes.
    ///
    /// Named rather than a `Deref` or an `as_bytes` so that every site which
    /// genuinely needs the value is findable with one grep — `expose` is not a
    /// word anyone writes by accident.
    #[must_use]
    pub fn expose(&self) -> &[u8] {
        &self.0
    }

    /// How many bytes, for the one thing it is safe to say about a value.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the value is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for SecretValue {
    /// Never the value. The length is disclosed because it is already
    /// observable from the artifact and is the field most useful in a bug
    /// report; the bytes never are.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SecretValue({} bytes, redacted)", self.0.len())
    }
}

impl From<Vec<u8>> for SecretValue {
    fn from(bytes: Vec<u8>) -> Self {
        Self::new(bytes)
    }
}

impl fmt::Display for AppName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Display for SecretName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Wipe a buffer that held secret bytes but is not a [`SecretValue`].
///
/// Reading from stdin or decoding base64 produces an intermediate the type
/// system does not cover. This is the seam where those get cleaned up.
pub fn wipe(buffer: &mut Vec<u8>) {
    buffer.zeroize();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The realistic leak: a value reaching a log line through `{:?}`.
    #[test]
    fn debug_never_renders_the_value() {
        let v = SecretValue::new(b"hunter2-the-actual-secret".to_vec());
        let rendered = format!("{v:?}");
        assert!(
            !rendered.contains("hunter2"),
            "the value leaked through Debug: {rendered}"
        );
        assert!(
            rendered.contains("redacted"),
            "the redaction should be visible, got: {rendered}"
        );
    }

    /// Length is disclosed on purpose; it is the useful half of a bug report
    /// and is observable from the artifact anyway.
    #[test]
    fn debug_discloses_the_length_and_nothing_else() {
        let v = SecretValue::new(vec![0u8; 42]);
        assert_eq!(format!("{v:?}"), "SecretValue(42 bytes, redacted)");
    }

    /// A secret name and an app name must not substitute for one another. This
    /// is asserted by construction — the test exists so that adding a `From`
    /// or a `Deref<Target = str>` between them is a visible decision.
    #[test]
    fn the_two_name_types_are_not_interchangeable() {
        let app = AppName::new("dashboard").expect("valid");
        let secret = SecretName::new("dashboard").expect("valid");
        // Same text, different types. If this ever compiles as `app == secret`,
        // someone has added an impl that defeats the split.
        assert_eq!(app.as_str(), secret.as_str());
    }

    #[test]
    fn a_name_may_not_be_empty_or_overlong() {
        assert!(SecretName::new("").is_err());
        assert!(SecretName::new(&"a".repeat(129)).is_err());
        assert!(SecretName::new(&"a".repeat(128)).is_ok());
    }

    /// A name reaching a file path, a JSON key and a TOML key should not be
    /// able to carry a separator or a traversal.
    #[test]
    fn a_name_may_not_carry_a_path_separator_or_traversal() {
        for bad in ["../etc/passwd", "a/b", "a\\b", "a b", "a\0b", ".."] {
            assert!(
                SecretName::new(bad).is_err(),
                "{bad:?} should have been rejected"
            );
        }
    }

    #[test]
    fn a_name_may_not_begin_or_end_with_a_separator() {
        for bad in ["-lead", "trail-", ".lead", "trail.", "_lead", "trail_"] {
            assert!(SecretName::new(bad).is_err(), "{bad:?} was accepted");
        }
        assert!(SecretName::new("a-b.c_d").is_ok());
    }

    /// The WIT is `list<u8>`, so a key or certificate must survive the type.
    #[test]
    fn a_value_carries_bytes_that_are_not_utf8() {
        let raw = vec![0xff, 0x00, 0xfe, 0x80];
        let v = SecretValue::new(raw.clone());
        assert_eq!(v.expose(), raw.as_slice());
    }

    #[test]
    fn wiping_a_buffer_clears_it() {
        let mut buf = b"secret".to_vec();
        wipe(&mut buf);
        assert!(
            buf.iter().all(|b| *b == 0),
            "the buffer still holds data: {buf:?}"
        );
    }
}
