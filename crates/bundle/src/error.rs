//! The rejection catalogue. See docs/spec/bundle-v1.md §9.
//!
//! Every variant carries a stable `bundle.*` code. These cross the repository
//! seam — the platform builder reports them to people who never built this
//! workspace from source — so renaming one is a breaking change.

/// Declares `Error` and, from the same input, every code it can produce. A
/// variant cannot exist without a code, which is what lets the test in this
/// module hold the catalogue to the enum rather than to a hand-kept list.
macro_rules! define_error {
    (
        $(
            $(#[$attr:meta])*
            $variant:ident { $($field:tt)* } => $code:literal;
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
        }

        #[cfg(test)]
        pub(crate) const DECLARED_CODES: &[&str] = &[$($code),*];
    };
}

define_error! {
    // ---- container, §3 ----
    #[error("not a tar stream: {reason}")]
    NotTar { reason: &'static str } => "bundle.not_tar";

    #[error("bad ustar magic or version at offset {offset}")]
    MagicInvalid { offset: usize } => "bundle.magic_invalid";

    #[error("entry type 0x{typeflag:02x} at offset {offset}; only regular files (0x30) are permitted")]
    EntryTypeInvalid { offset: usize, typeflag: u8 } => "bundle.entry_type_invalid";

    #[error("ustar prefix field is not empty at offset {offset}")]
    PrefixNotEmpty { offset: usize } => "bundle.prefix_not_empty";

    #[error("mode {mode:?} at offset {offset}; only 0000644 and 0000755 are permitted")]
    ModeInvalid { offset: usize, mode: String } => "bundle.mode_invalid";

    #[error("non-canonical {field} at offset {offset}")]
    MetadataNotCanonical { offset: usize, field: &'static str } => "bundle.metadata_not_canonical";

    #[error("header checksum invalid at offset {offset}")]
    ChecksumInvalid { offset: usize } => "bundle.checksum_invalid";

    #[error("size field is not 11 octal digits and a NUL at offset {offset}")]
    SizeEncodingInvalid { offset: usize } => "bundle.size_encoding_invalid";

    #[error("member of {size} bytes exceeds the 8 GiB octal size ceiling")]
    MemberTooLarge { size: u64 } => "bundle.member_too_large";

    #[error("non-zero padding byte at offset {offset}")]
    PaddingInvalid { offset: usize } => "bundle.padding_invalid";

    #[error("entries out of order: {current:?} follows {previous:?}")]
    EntriesNotSorted { previous: String, current: String } => "bundle.entries_not_sorted";

    #[error("end of archive: {reason}")]
    TrailerInvalid { reason: &'static str } => "bundle.trailer_invalid";

    // ---- paths, §3.2 ----
    #[error("invalid path {path:?}: {reason}")]
    PathInvalid { path: String, reason: &'static str } => "bundle.path_invalid";

    #[error("path {path:?} is not in Unicode Normalization Form C")]
    PathNotNfc { path: String } => "bundle.path_not_nfc";

    #[error("path {path:?} is {len} bytes; the limit is 100")]
    PathTooLong { path: String, len: usize } => "bundle.path_too_long";

    #[error("duplicate path {path:?}")]
    PathDuplicate { path: String } => "bundle.path_duplicate";

    // ---- manifest, §5 ----
    #[error("no clwd.toml in the archive")]
    ManifestMissing {} => "bundle.manifest_missing";

    #[error("clwd.toml is not valid TOML: {reason}")]
    ManifestNotToml { reason: String } => "bundle.manifest_not_toml";

    #[error("clwd.toml has no schema_version")]
    SchemaVersionMissing {} => "bundle.schema_version_missing";

    #[error("schema_version {found} is not supported; this implementation reads version 1")]
    SchemaVersionUnsupported { found: i64 } => "bundle.schema_version_unsupported";

    #[error("unknown key {key:?}")]
    UnknownKey { key: String } => "bundle.unknown_key";

    #[error("unknown capability key {key:?}; this runtime cannot enforce it")]
    CapabilityUnknownKey { key: String } => "bundle.capability_unknown_key";

    #[error("capabilities.version {found} is not supported; this runtime enforces version 1")]
    CapabilitiesVersionUnsupported { found: i64 } => "bundle.capabilities_version_unsupported";

    #[error("required key {key:?} is missing")]
    FieldMissing { key: String } => "bundle.field_missing";

    #[error("key {key:?} should be {expected}")]
    FieldTypeInvalid { key: String, expected: &'static str } => "bundle.field_type_invalid";

    #[error("key {key:?}: {reason}")]
    FieldValueInvalid { key: String, reason: String } => "bundle.field_value_invalid";

    #[error("runtime {found:?} is neither \"wasm\" nor \"microvm\"")]
    RuntimeUnknown { found: String } => "bundle.runtime_unknown";

    #[error("clwd.toml is not in canonical form: {reason}")]
    ManifestNotCanonical { reason: String } => "bundle.manifest_not_canonical";

    // ---- members, §5.1 ----
    #[error("clwd.toml lists itself in [[members]]")]
    ManifestSelfListed {} => "bundle.manifest_self_listed";

    #[error("member {path:?} is listed but not present in the archive")]
    MemberMissing { path: String } => "bundle.member_missing";

    #[error("archive entry {path:?} is not listed in [[members]]")]
    MemberUnlisted { path: String } => "bundle.member_unlisted";

    #[error("member {path:?} declares {declared} bytes but holds {actual}")]
    MemberSizeMismatch { path: String, declared: u64, actual: u64 } => "bundle.member_size_mismatch";

    #[error("member {path:?} does not match its declared digest")]
    MemberDigestMismatch { path: String } => "bundle.member_digest_mismatch";

    #[error("malformed digest {value:?}; expected blake3: and 64 lowercase hex characters")]
    DigestMalformed { value: String } => "bundle.digest_malformed";

    // ---- entrypoint and runtime, §6 ----
    #[error("app.entrypoint {entrypoint:?} does not name a listed member")]
    EntrypointNotAMember { entrypoint: String } => "bundle.entrypoint_not_a_member";

    #[error("entrypoint is a core wasm module; runtime \"wasm\" requires a component")]
    WasmNotComponent {} => "bundle.wasm_not_component";

    #[error("entrypoint does not begin with a WebAssembly component preamble")]
    WasmPreambleInvalid {} => "bundle.wasm_preamble_invalid";

    // ---- bundle, §4.3 ----
    #[error("bundle digest {actual} does not match the expected {expected}")]
    DigestMismatch { expected: String, actual: String } => "bundle.digest_mismatch";
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
        let unique: BTreeSet<&str> = DECLARED_CODES.iter().copied().collect();
        assert_eq!(unique.len(), DECLARED_CODES.len(), "duplicate code");
    }

    #[test]
    fn codes_are_namespaced_and_lowercase() {
        for code in DECLARED_CODES {
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
        let implemented: BTreeSet<String> =
            DECLARED_CODES.iter().map(|c| (*c).to_owned()).collect();

        let missing: Vec<_> = documented.difference(&implemented).collect();
        let undocumented: Vec<_> = implemented.difference(&documented).collect();
        assert!(
            missing.is_empty() && undocumented.is_empty(),
            "specified but not implemented: {missing:?}; implemented but not specified: {undocumented:?}"
        );
    }
}
