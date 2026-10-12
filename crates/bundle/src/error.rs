//! The rejection catalogue. See docs/spec/bundle-v1.md §9.
//!
//! Every variant carries a stable `bundle.*` code. These cross the repository
//! seam — the platform builder reports them to people who never built this
//! workspace from source — so renaming one is a breaking change.

/// Declares `Error` and, from the same input, every code it can produce and
/// the hint for each. A variant cannot exist without a code and a hint, which
/// is what lets the tests in this module hold the catalogue to the enum rather
/// than to a hand-kept list.
///
/// A hint is a string literal, never a format string: it is one static
/// sentence per code and never carries error data. The data — a path from
/// inside the bundle, a manifest key — is untrusted, and belongs in the
/// message, where a caller already expects it.
macro_rules! define_error {
    (
        $(
            $(#[$attr:meta])*
            $variant:ident { $($field:tt)* } => $code:literal, $hint:literal;
        )*
    ) => {
        #[derive(Debug, thiserror::Error)]
        #[non_exhaustive]
        pub enum Error {
            $( $(#[$attr])* $variant { $($field)* }, )*
        }

        impl Error {
            /// The stable code for this rejection.
            pub fn code(&self) -> &'static str {
                match self {
                    $( Self::$variant { .. } => $code, )*
                }
            }

            /// What to do next: one imperative sentence that names the
            /// command or `clwd.toml` key to change. Static, and never
            /// interpolated — see the macro documentation.
            pub fn hint(&self) -> &'static str {
                match self {
                    $( Self::$variant { .. } => $hint, )*
                }
            }
        }

        /// Every `bundle.*` code this crate can produce, generated from the
        /// same input as the enum. Public because the error-code table in
        /// `docs/error-codes.md` is built from it and the platform mirrors it
        /// across the seam; a catalogue only the tests can see cannot be
        /// published.
        pub const CODES: &[&str] = &[$($code),*];

        /// Every code paired with its hint, generated from the same input as
        /// [`Error::hint`]. Public for the same reason as [`CODES`]: the
        /// error-code table is built from it.
        pub const HINTS: &[(&str, &str)] = &[$(($code, $hint)),*];
    };
}

define_error! {
    // ---- container, §3 ----
    #[error("not a tar stream: {reason}")]
    NotTar { reason: &'static str } => "bundle.not_tar",
        "Point the command at a bundle file that `clwd build` produced.";

    #[error("bad ustar magic or version at offset {offset}")]
    MagicInvalid { offset: usize } => "bundle.magic_invalid",
        "Rebuild the damaged bundle with `clwd build`.";

    #[error("entry type 0x{typeflag:02x} at offset {offset}; only regular files (0x30) are permitted")]
    EntryTypeInvalid { offset: usize, typeflag: u8 } => "bundle.entry_type_invalid",
        "Replace the symlink or special file in your project with a regular file, then run `clwd build`.";

    #[error("ustar prefix field is not empty at offset {offset}")]
    PrefixNotEmpty { offset: usize } => "bundle.prefix_not_empty",
        "Rebuild the bundle with this version of `clwd build`.";

    #[error("mode {mode:?} at offset {offset}; only 0000644 and 0000755 are permitted")]
    ModeInvalid { offset: usize, mode: String } => "bundle.mode_invalid",
        "Set the file to mode 644, or 755 if it is executable, then run `clwd build`.";

    #[error("non-canonical {field} at offset {offset}")]
    MetadataNotCanonical { offset: usize, field: &'static str } => "bundle.metadata_not_canonical",
        "Rebuild the bundle with `clwd build` instead of repacking it with another archiver.";

    #[error("header checksum invalid at offset {offset}")]
    ChecksumInvalid { offset: usize } => "bundle.checksum_invalid",
        "Rebuild the damaged bundle with `clwd build`.";

    #[error("size field is not 11 octal digits and a NUL at offset {offset}")]
    SizeEncodingInvalid { offset: usize } => "bundle.size_encoding_invalid",
        "Rebuild the damaged bundle with `clwd build`.";

    #[error("member of {size} bytes exceeds the 8 GiB octal size ceiling")]
    MemberTooLarge { size: u64 } => "bundle.member_too_large",
        "Remove or shrink the oversized file in your project, then run `clwd build`.";

    #[error("non-zero padding byte at offset {offset}")]
    PaddingInvalid { offset: usize } => "bundle.padding_invalid",
        "Rebuild the damaged bundle with `clwd build`.";

    #[error("entries out of order: {current:?} follows {previous:?}")]
    EntriesNotSorted { previous: String, current: String } => "bundle.entries_not_sorted",
        "Rebuild the bundle with `clwd build` instead of repacking it with another archiver.";

    #[error("end of archive: {reason}")]
    TrailerInvalid { reason: &'static str } => "bundle.trailer_invalid",
        "Download the bundle again, or rebuild it with `clwd build`.";

    // ---- paths, §3.2 ----
    #[error("invalid path {path:?}: {reason}")]
    PathInvalid { path: String, reason: &'static str } => "bundle.path_invalid",
        "Rename the file the message names so its path is relative with no `..` segment, then run `clwd build`.";

    #[error("path {path:?} is not in Unicode Normalization Form C")]
    PathNotNfc { path: String } => "bundle.path_not_nfc",
        "Rename the file the message names using NFC-normalized characters, then run `clwd build`.";

    #[error("path {path:?} is {len} bytes; the limit is 100")]
    PathTooLong { path: String, len: usize } => "bundle.path_too_long",
        "Shorten the path the message names to 100 bytes or fewer, then run `clwd build`.";

    #[error("duplicate path {path:?}")]
    PathDuplicate { path: String } => "bundle.path_duplicate",
        "Rebuild the bundle with `clwd build` instead of assembling it by hand.";

    // ---- manifest, §5 ----
    #[error("no clwd.toml in the archive")]
    ManifestMissing {} => "bundle.manifest_missing",
        "Add a `clwd.toml` to the project root, then run `clwd build`.";

    #[error("clwd.toml is not valid TOML: {reason}")]
    ManifestNotToml { reason: String } => "bundle.manifest_not_toml",
        "Fix the TOML syntax error the message points to in `clwd.toml`, then run `clwd build`.";

    #[error("clwd.toml has no schema_version")]
    SchemaVersionMissing {} => "bundle.schema_version_missing",
        "Add `schema_version = 1` to the top of `clwd.toml`, then run `clwd build`.";

    #[error("schema_version {found} is not supported; this implementation reads version 1")]
    SchemaVersionUnsupported { found: i64 } => "bundle.schema_version_unsupported",
        "Set `schema_version = 1` in `clwd.toml`, or upgrade clwd to a version that reads the newer format.";

    #[error("unknown key {key:?}")]
    UnknownKey { key: String } => "bundle.unknown_key",
        "Remove or correct the key the message names in `clwd.toml`, then run `clwd build`.";

    #[error("unknown capability key {key:?}; this runtime cannot enforce it")]
    CapabilityUnknownKey { key: String } => "bundle.capability_unknown_key",
        "Remove the capability key the message names from `clwd.toml`, or upgrade clwd to a version that enforces it.";

    #[error("capabilities.version {found} is not supported; this runtime enforces version 1")]
    CapabilitiesVersionUnsupported { found: i64 } => "bundle.capabilities_version_unsupported",
        "Set `capabilities.version = 1` in `clwd.toml`, or upgrade clwd to a version that enforces the newer capabilities.";

    #[error("required key {key:?} is missing")]
    FieldMissing { key: String } => "bundle.field_missing",
        "Add the key the message names to `clwd.toml`, then run `clwd build`.";

    #[error("key {key:?} should be {expected}")]
    FieldTypeInvalid { key: String, expected: &'static str } => "bundle.field_type_invalid",
        "Change the key the message names in `clwd.toml` to the type it expects, then run `clwd build`.";

    #[error("key {key:?}: {reason}")]
    FieldValueInvalid { key: String, reason: String } => "bundle.field_value_invalid",
        "Correct the value of the key the message names in `clwd.toml`, then run `clwd build`.";

    #[error("runtime {found:?} is neither \"wasm\" nor \"microvm\"")]
    RuntimeUnknown { found: String } => "bundle.runtime_unknown",
        "Set `runtime = \"wasm\"` in `clwd.toml`, then run `clwd build`.";

    #[error("clwd.toml is not in canonical form: {reason}")]
    ManifestNotCanonical { reason: String } => "bundle.manifest_not_canonical",
        "Rebuild the bundle with `clwd build` instead of editing its `clwd.toml` afterwards.";

    // ---- members, §5.1 ----
    #[error("clwd.toml lists itself in [[members]]")]
    ManifestSelfListed {} => "bundle.manifest_self_listed",
        "Rebuild the bundle with `clwd build` instead of writing `[[members]]` by hand.";

    #[error("member {path:?} is listed but not present in the archive")]
    MemberMissing { path: String } => "bundle.member_missing",
        "Rebuild the incomplete bundle with `clwd build`.";

    #[error("archive entry {path:?} is not listed in [[members]]")]
    MemberUnlisted { path: String } => "bundle.member_unlisted",
        "Rebuild the bundle with `clwd build` instead of adding files to it afterwards.";

    #[error("member {path:?} declares {declared} bytes but holds {actual}")]
    MemberSizeMismatch { path: String, declared: u64, actual: u64 } => "bundle.member_size_mismatch",
        "Rebuild the bundle with `clwd build` instead of modifying its files afterwards.";

    #[error("member {path:?} does not match its declared digest")]
    MemberDigestMismatch { path: String } => "bundle.member_digest_mismatch",
        "Rebuild the bundle with `clwd build` and discard the copy whose contents fail their digest.";

    #[error("malformed digest {value:?}; expected blake3: and 64 lowercase hex characters")]
    DigestMalformed { value: String } => "bundle.digest_malformed",
        "Rebuild the bundle with `clwd build` instead of hand-editing digests in `clwd.toml`.";

    // ---- entrypoint and runtime, §6 ----
    #[error("app.entrypoint {entrypoint:?} does not name a listed member")]
    EntrypointNotAMember { entrypoint: String } => "bundle.entrypoint_not_a_member",
        "Set `app.entrypoint` in `clwd.toml` to a file the bundle includes, then run `clwd build`.";

    #[error("entrypoint is a core wasm module; runtime \"wasm\" requires a component")]
    WasmNotComponent {} => "bundle.wasm_not_component",
        "Build the app for the `wasm32-wasip2` target so it produces a component, then run `clwd build`.";

    #[error("entrypoint does not begin with a WebAssembly component preamble")]
    WasmPreambleInvalid {} => "bundle.wasm_preamble_invalid",
        "Point `app.entrypoint` in `clwd.toml` at the compiled `.wasm` component, then run `clwd build`.";

    // ---- bundle, §4.3 ----
    #[error("bundle digest {actual} does not match the expected {expected}")]
    DigestMismatch { expected: String, actual: String } => "bundle.digest_mismatch",
        "Fetch the bundle again from its source, or rebuild it with `clwd build`.";
}

impl Error {
    /// Whether retrying the same bytes could plausibly succeed.
    ///
    /// Always false, and §9 says so for the whole catalogue: a malformed bundle
    /// does not become well-formed on a second attempt, so retrying is always
    /// the wrong response. This is a method rather than a constant because the
    /// envelope in #51 carries the field per error, not per namespace.
    pub fn retryable(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn codes_are_unique() {
        let unique: BTreeSet<&str> = CODES.iter().copied().collect();
        assert_eq!(unique.len(), CODES.len(), "duplicate code");
    }

    #[test]
    fn every_code_has_a_hint() {
        let hinted: Vec<&str> = HINTS.iter().map(|(code, _)| *code).collect();
        assert_eq!(hinted, CODES, "HINTS and CODES disagree");
        for (code, hint) in HINTS {
            assert!(!hint.trim().is_empty(), "{code} has an empty hint");
        }
    }

    /// A hint never carries error data, so it never needs a placeholder. A brace
    /// is the first sign of one creeping in.
    #[test]
    fn no_hint_is_a_format_string() {
        for (code, hint) in HINTS {
            assert!(
                !hint.contains(['{', '}']),
                "{code}: a hint must not interpolate: {hint:?}"
            );
        }
    }

    #[test]
    fn codes_are_namespaced_and_lowercase() {
        for code in CODES {
            let rest = code
                .strip_prefix("bundle.")
                .unwrap_or_else(|| panic!("{code} is not in the bundle namespace"));
            assert!(!rest.is_empty(), "{code} has an empty local part");
            assert_eq!(*code, code.to_lowercase(), "{code} is not lowercase");
        }
    }

    /// The spec's catalogue is the contract. If a code is added here without
    /// being written down, or written down without being implemented, the two
    /// have diverged and one of them is lying.
    #[test]
    fn codes_match_the_specification_catalogue() {
        let spec = include_str!("../../../docs/spec/bundle-v1.md");
        let documented: BTreeSet<String> = spec
            .split('`')
            // `bundle.*` in the spec is the namespace itself, not a code.
            .filter(|token| {
                token.starts_with("bundle.") && !token.contains(' ') && !token.contains('*')
            })
            .map(|token| token.trim_end_matches('.').to_owned())
            .collect();
        let implemented: BTreeSet<String> = CODES.iter().map(|c| (*c).to_owned()).collect();

        let missing: Vec<_> = documented.difference(&implemented).collect();
        let undocumented: Vec<_> = implemented.difference(&documented).collect();
        assert!(
            missing.is_empty() && undocumented.is_empty(),
            "specified but not implemented: {missing:?}; implemented but not specified: {undocumented:?}"
        );
    }
}
