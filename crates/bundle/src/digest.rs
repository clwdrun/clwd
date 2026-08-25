//! Digests and `BundleRef`. See docs/spec/bundle-v1.md §4.
//!
//! Two digests, both BLAKE3 with the default 32-byte output, unkeyed. A member
//! digest covers one file's contents. A bundle digest covers the whole
//! canonical tar stream and is the bundle's identity.

use std::fmt;
use std::str::FromStr;

use crate::error::Error;

/// The `blake3:` prefix every digest string carries. §4.1.
const PREFIX: &str = "blake3:";

/// Length of the hex body: 32 bytes, two characters each.
const HEX_LEN: usize = 64;

/// A BLAKE3 digest in the §4.1 string form.
///
/// Used for member digests. `BundleRef` is the same value in the role of a
/// bundle's identity, and is a distinct type so the two cannot be confused at a
/// call site — the spec is emphatic that one of them may never appear inside
/// the manifest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Digest([u8; 32]);

impl Digest {
    /// Hash `bytes`.
    pub fn of(bytes: &[u8]) -> Self {
        Self(*blake3::hash(bytes).as_bytes())
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Display for Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(PREFIX)?;
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl FromStr for Digest {
    type Err = Error;

    /// Parses `blake3:` followed by exactly 64 lowercase hex characters.
    ///
    /// Uppercase is rejected rather than normalized (§4.1): accepting both
    /// would give one digest two spellings, and a string-keyed cache would then
    /// hold two entries for one artifact.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let malformed = || Error::DigestMalformed { value: s.to_owned() };

        let hex = s.strip_prefix(PREFIX).ok_or_else(malformed)?;
        if hex.len() != HEX_LEN {
            return Err(malformed());
        }

        let mut out = [0u8; 32];
        for (slot, pair) in out.iter_mut().zip(hex.as_bytes().chunks_exact(2)) {
            let nibble = |c: u8| match c {
                b'0'..=b'9' => Some(c - b'0'),
                b'a'..=b'f' => Some(c - b'a' + 10),
                _ => None,
            };
            let hi = nibble(pair[0]).ok_or_else(malformed)?;
            let lo = nibble(pair[1]).ok_or_else(malformed)?;
            *slot = (hi << 4) | lo;
        }
        Ok(Self(out))
    }
}

/// A bundle's identity: BLAKE3 over the entire canonical tar stream, trailing
/// zero blocks included. §4.3.
///
/// This value cannot appear inside `clwd.toml` — the manifest is an input to
/// it. It lives only in metadata *about* a bundle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BundleRef(Digest);

impl BundleRef {
    /// Hash a complete canonical tar stream.
    pub fn of_stream(stream: &[u8]) -> Self {
        Self(Digest::of(stream))
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        self.0.as_bytes()
    }
}

impl fmt::Display for BundleRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for BundleRef {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse().map(Self)
    }
}
