//! Member path rules. See docs/spec/bundle-v1.md §3.2.
//!
//! Normalization is an encoder responsibility and rejection a decoder one.
//! macOS hands filenames to a process in NFD and Linux in NFC, so without
//! normalizing on the way in, one source tree would produce two different
//! bundle digests on two machines.

use unicode_normalization::{is_nfc, UnicodeNormalization};

use crate::error::Error;

/// The §3.2 cap. Paths never reach the USTAR `prefix` field, so the field is
/// always empty and its ambiguous split rule never applies.
pub const MAX_LEN: usize = 100;

/// Normalize a path for use as a member path: NFC, forward slashes.
///
/// Does not validate — call [`validate`] on the result.
pub fn normalize(path: &str) -> String {
    path.nfc().collect()
}

/// Reject anything the wire format does not permit.
pub fn validate(path: &str) -> Result<(), Error> {
    let invalid = |reason: &'static str| Error::PathInvalid {
        path: path.to_owned(),
        reason,
    };

    if path.is_empty() {
        return Err(invalid("empty"));
    }
    if path.starts_with('/') {
        return Err(invalid("absolute"));
    }
    if path.ends_with('/') {
        return Err(invalid("trailing slash"));
    }
    if path.contains("//") {
        return Err(invalid("empty path segment"));
    }
    if path.split('/').any(|s| s == "." || s == "..") {
        return Err(invalid("`.` or `..` segment"));
    }
    if path.chars().any(|c| c.is_control()) {
        return Err(invalid("control character"));
    }
    if !is_nfc(path) {
        return Err(Error::PathNotNfc {
            path: path.to_owned(),
        });
    }
    if path.len() > MAX_LEN {
        return Err(Error::PathTooLong {
            path: path.to_owned(),
            len: path.len(),
        });
    }
    Ok(())
}
