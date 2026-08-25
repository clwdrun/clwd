//! Canonical tar stream to a validated bundle.
//! Validation runs exactly in the order docs/spec/bundle-v1.md §8 requires.

use crate::digest::{BundleRef, Digest};
use crate::encode::File;
use crate::error::Error;
use crate::manifest::{Manifest, Runtime, FILENAME};
use crate::tar;

/// The component-model preamble: `\0asm`, version 0x000D, layer 0x0001. §6.1.
const COMPONENT_PREAMBLE: [u8; 8] = [0x00, 0x61, 0x73, 0x6d, 0x0d, 0x00, 0x01, 0x00];
/// A core module: same magic, version 1, layer 0. Rejected at ingest.
const CORE_MODULE_PREAMBLE: [u8; 8] = [0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bundle {
    pub manifest: Manifest,
    /// Every entry except `clwd.toml`, in archive order.
    pub members: Vec<File>,
    pub digest: BundleRef,
}

/// Decode and fully validate.
///
/// `expected` is checked first when supplied: a caller that already knows which
/// bundle it asked for has no reason to parse untrusted structure before
/// confirming this is that bundle.
pub fn decode(stream: &[u8], expected: Option<BundleRef>) -> Result<Bundle, Error> {
    // Step 0.
    let digest = BundleRef::of_stream(stream);
    if let Some(expected) = expected {
        if expected != digest {
            return Err(Error::DigestMismatch {
                expected: expected.to_string(),
                actual: digest.to_string(),
            });
        }
    }

    // Steps 1 to 8.
    let entries = tar::read(stream)?;

    // Step 9.
    let manifest_entry = entries
        .iter()
        .find(|e| e.path == FILENAME)
        .ok_or(Error::ManifestMissing {})?;
    let manifest_text =
        std::str::from_utf8(&manifest_entry.data).map_err(|_| Error::ManifestNotToml {
            reason: "not valid UTF-8".to_owned(),
        })?;

    // Steps 10 to 15, plus step 17 — a member digest string that will not parse
    // is rejected while the manifest is being read.
    let manifest = Manifest::parse(manifest_text)?;

    // Step 16. Only possible because §7 rejects unknown keys: an ignored key is
    // one this could not re-emit.
    let canonical = manifest.to_canonical_string();
    if canonical.as_bytes() != manifest_entry.data.as_slice() {
        return Err(Error::ManifestNotCanonical {
            reason: "does not re-serialize byte-identically".to_owned(),
        });
    }

    // Step 18.
    if manifest.members.iter().any(|m| m.path == FILENAME) {
        return Err(Error::ManifestSelfListed {});
    }
    for member in &manifest.members {
        if !entries.iter().any(|e| e.path == member.path) {
            return Err(Error::MemberMissing {
                path: member.path.clone(),
            });
        }
    }
    for entry in &entries {
        if entry.path != FILENAME && !manifest.members.iter().any(|m| m.path == entry.path) {
            return Err(Error::MemberUnlisted {
                path: entry.path.clone(),
            });
        }
    }

    // Steps 19 and 20: length before contents, cheapest first, and both before
    // anything interprets a member's bytes.
    for member in &manifest.members {
        let entry = entries
            .iter()
            .find(|e| e.path == member.path)
            .expect("presence checked above");
        if entry.data.len() as u64 != member.size {
            return Err(Error::MemberSizeMismatch {
                path: member.path.clone(),
                declared: member.size,
                actual: entry.data.len() as u64,
            });
        }
        if Digest::of(&entry.data) != member.digest {
            return Err(Error::MemberDigestMismatch {
                path: member.path.clone(),
            });
        }
    }

    // Step 21.
    let entrypoint = manifest
        .members
        .iter()
        .find(|m| m.path == manifest.entrypoint)
        .ok_or_else(|| Error::EntrypointNotAMember {
            entrypoint: manifest.entrypoint.clone(),
        })?;

    // Step 22. Only now, with the entrypoint's bytes authenticated, does
    // anything look at what they mean.
    let entrypoint_data = &entries
        .iter()
        .find(|e| e.path == entrypoint.path)
        .expect("entrypoint is a member")
        .data;
    check_runtime(manifest.runtime, entrypoint_data)?;

    let members = entries
        .into_iter()
        .filter(|e| e.path != FILENAME)
        .map(|e| File {
            path: e.path,
            data: e.data,
            executable: e.executable,
        })
        .collect();

    Ok(Bundle {
        manifest,
        members,
        digest,
    })
}

/// §6. Asymmetric by design: a wasm component announces itself in its first
/// eight bytes, and a root filesystem image announces nothing at all.
fn check_runtime(runtime: Runtime, entrypoint: &[u8]) -> Result<(), Error> {
    match runtime {
        // A rootfs has no header to check. Saying so is better than implying a
        // symmetry the code cannot deliver.
        Runtime::MicroVm => Ok(()),
        Runtime::Wasm => {
            if entrypoint.starts_with(&COMPONENT_PREAMBLE) {
                Ok(())
            } else if entrypoint.starts_with(&CORE_MODULE_PREAMBLE) {
                Err(Error::WasmNotComponent {})
            } else {
                Err(Error::WasmPreambleInvalid {})
            }
        }
    }
}
