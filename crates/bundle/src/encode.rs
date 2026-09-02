//! Directory tree to canonical tar stream. See docs/spec/bundle-v1.md §3.
//!
//! No I/O: the caller reads the files and passes bytes. The library never opens
//! anything itself, so the same code path serves the CLI, the platform builder
//! and a test with no filesystem at all.

use crate::digest::{BundleRef, Digest};
use crate::error::Error;
use crate::manifest::{Capabilities, FILENAME, Manifest, Member, Runtime, SCHEMA_VERSION};
use crate::tar;

/// One file destined for the bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct File {
    pub path: String,
    pub data: Vec<u8>,
    pub executable: bool,
}

/// A packed bundle: the canonical stream and its identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Encoded {
    pub bytes: Vec<u8>,
    pub digest: BundleRef,
}

/// Build a manifest from the members, then pack.
///
/// This is what a build pipeline wants: it computes each member digest and
/// size, so the manifest cannot disagree with the files it describes.
pub fn build(
    runtime: Runtime,
    entrypoint: &str,
    capabilities: Capabilities,
    members: Vec<File>,
) -> Result<Encoded, Error> {
    let mut members = normalize(members)?;
    // §5.2: [[members]] is sorted by path, the same order as the archive
    // entries. Sorting here rather than trusting the caller's order is what
    // makes the output independent of the order files were discovered in.
    members.sort_by(|a, b| a.path.as_bytes().cmp(b.path.as_bytes()));

    let manifest = Manifest {
        schema_version: SCHEMA_VERSION,
        runtime,
        entrypoint: crate::path::normalize(entrypoint),
        members: members
            .iter()
            .map(|f| Member {
                path: f.path.clone(),
                digest: Digest::of(&f.data),
                size: f.data.len() as u64,
            })
            .collect(),
        capabilities,
    };
    encode(&manifest, &members)
}

/// Pack a manifest and its members into the canonical stream.
///
/// The manifest is serialized canonically here rather than taken as bytes: it
/// is the one member whose contents this library owns, and §5.4 requires one
/// logical manifest to have exactly one byte representation.
pub fn encode(manifest: &Manifest, members: &[File]) -> Result<Encoded, Error> {
    let mut entries: Vec<(String, Vec<u8>, bool)> = Vec::with_capacity(members.len() + 1);
    for file in normalize(members.to_vec())? {
        if file.path == FILENAME {
            return Err(Error::ManifestSelfListed {});
        }
        entries.push((file.path, file.data, file.executable));
    }
    entries.push((
        FILENAME.to_owned(),
        manifest.to_canonical_string().into_bytes(),
        false,
    ));

    // §3.3: byte-wise ascending over the path. The manifest sorts among the
    // members like anything else; it has no privileged position.
    entries.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));

    let mut bytes = Vec::new();
    for (path, data, executable) in &entries {
        let size = data.len() as u64;
        if size >= 8 * 1024 * 1024 * 1024 {
            return Err(Error::MemberTooLarge { size });
        }
        bytes.extend_from_slice(&tar::write_header(path, size, *executable));
        bytes.extend_from_slice(data);
        let padding = (tar::BLOCK - data.len() % tar::BLOCK) % tar::BLOCK;
        bytes.resize(bytes.len() + padding, 0);
    }
    // §3.6: exactly two zero blocks, and they are inside the digest.
    bytes.resize(bytes.len() + 2 * tar::BLOCK, 0);

    let digest = BundleRef::of_stream(&bytes);
    Ok(Encoded { bytes, digest })
}

/// NFC-normalize every path, then reject what the wire format forbids.
///
/// Normalization happens before the duplicate check on purpose: two source
/// paths that differ only in normalization form are the same member, and
/// silently dropping one would produce a bundle missing a file its author
/// expected.
fn normalize(files: Vec<File>) -> Result<Vec<File>, Error> {
    let mut out = Vec::with_capacity(files.len());
    for mut file in files {
        file.path = crate::path::normalize(&file.path);
        crate::path::validate(&file.path)?;
        if out.iter().any(|f: &File| f.path == file.path) {
            return Err(Error::PathDuplicate { path: file.path });
        }
        out.push(file);
    }
    Ok(out)
}
