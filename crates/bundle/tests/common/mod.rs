//! Shared helpers. Every test builds from a known-good bundle and breaks
//! exactly one thing, so a failure names the rule that fired.

#![allow(dead_code)]

use clwd_bundle::{Capabilities, Encoded, Error, File, Limits, Runtime, build};

/// The component-model preamble: the smallest valid `runtime = "wasm"` entry.
pub const COMPONENT: &[u8] = &[0x00, 0x61, 0x73, 0x6d, 0x0d, 0x00, 0x01, 0x00];
/// A core module. Valid WebAssembly, rejected at ingest.
pub const CORE_MODULE: &[u8] = &[0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];

pub fn capabilities() -> Capabilities {
    Capabilities {
        version: 1,
        egress: Vec::new(),
        secrets: Vec::new(),
        storage_namespaces: Vec::new(),
        limits: Limits {
            cpu_ms: 5000,
            memory_mb: 128,
        },
    }
}

pub fn members() -> Vec<File> {
    vec![
        File {
            path: "app.wasm".to_owned(),
            data: COMPONENT.to_vec(),
            executable: false,
        },
        File {
            path: "data/table.bin".to_owned(),
            data: b"payload".to_vec(),
            executable: false,
        },
    ]
}

pub fn one_member(path: &str, data: &[u8]) -> Vec<File> {
    vec![File {
        path: path.to_owned(),
        data: data.to_vec(),
        executable: false,
    }]
}

/// Pack, asserting the inputs were valid. For inputs expected to fail, call
/// [`try_bundle`].
pub fn bundle(members: Vec<File>) -> Encoded {
    try_bundle(members).expect("these members make a valid bundle")
}

pub fn try_bundle(members: Vec<File>) -> Result<Encoded, Error> {
    let entrypoint = members
        .iter()
        .map(|m| m.path.clone())
        .find(|p| p.ends_with(".wasm"))
        .unwrap_or_else(|| "app.wasm".to_owned());
    build(Runtime::Wasm, &entrypoint, capabilities(), members)
}

/// Assert a result failed with exactly this code. Comparing codes rather than
/// variants is deliberate: the code is the compatibility surface.
#[track_caller]
pub fn assert_code<T: std::fmt::Debug>(result: Result<T, Error>, expected: &str) {
    match result {
        Ok(value) => panic!("expected {expected}, got Ok({value:?})"),
        Err(error) => assert_eq!(
            error.code(),
            expected,
            "wrong rejection: {error} ({})",
            error.code()
        ),
    }
}

/// Offsets of the entries in a packed bundle, so a test can corrupt a field by
/// name rather than by a magic number.
pub fn entry_offsets(bytes: &[u8]) -> Vec<(String, usize)> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while at + 512 <= bytes.len() && bytes[at..at + 512].iter().any(|b| *b != 0) {
        let name_end = bytes[at..at + 100]
            .iter()
            .position(|b| *b == 0)
            .unwrap_or(100);
        let name = String::from_utf8_lossy(&bytes[at..at + name_end]).into_owned();
        let size = octal(&bytes[at + 124..at + 135]);
        out.push((name, at));
        at += 512 + (size as usize).div_ceil(512) * 512;
    }
    out
}

fn octal(field: &[u8]) -> u64 {
    field
        .iter()
        .filter(|b| (b'0'..=b'7').contains(b))
        .fold(0u64, |acc, b| acc * 8 + u64::from(b - b'0'))
}

/// Recompute a header checksum in place, so a test that alters one field does
/// not trip the checksum rule instead of the rule it is aiming at.
pub fn reseal(bytes: &mut [u8], header: usize) {
    let block = &mut bytes[header..header + 512];
    block[148..156].copy_from_slice(b"        ");
    let sum: u32 = block.iter().map(|b| u32::from(*b)).sum();
    block[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
}

/// Pack entries in the order given, without sorting, so a test can produce the
/// out-of-order archive the encoder would never emit.
pub fn pack_in_order(entries: &[(String, Vec<u8>, bool)]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for (path, data, executable) in entries {
        bytes.extend_from_slice(&clwd_bundle::tar::write_header(
            path,
            data.len() as u64,
            *executable,
        ));
        bytes.extend_from_slice(data);
        bytes.resize(bytes.len() + (512 - data.len() % 512) % 512, 0);
    }
    bytes.resize(bytes.len() + 1024, 0);
    bytes
}

pub fn pack(entries: &[(String, Vec<u8>, bool)]) -> Vec<u8> {
    let mut sorted = entries.to_vec();
    sorted.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    pack_in_order(&sorted)
}

/// The canonical manifest of a known-good single-member bundle, as text, ready
/// to be mutated by a test that wants one field wrong.
pub fn valid_manifest_text() -> String {
    let packed = bundle(one_member("app.wasm", COMPONENT));
    let decoded = clwd_bundle::decode(&packed.bytes, None).expect("valid");
    decoded.manifest.to_canonical_string()
}

/// An archive holding `app.wasm` and whatever manifest text a test supplies.
pub fn with_manifest(text: &str) -> Vec<u8> {
    pack(&[
        ("app.wasm".to_owned(), COMPONENT.to_vec(), false),
        ("clwd.toml".to_owned(), text.as_bytes().to_vec(), false),
    ])
}

/// Byte offset of a named entry's header block.
pub fn header_of(bytes: &[u8], name: &str) -> usize {
    entry_offsets(bytes)
        .into_iter()
        .find(|(n, _)| n == name)
        .unwrap_or_else(|| panic!("no entry named {name}"))
        .1
}

/// Overwrite a header field and repair the checksum, so the test trips the rule
/// it is aiming at rather than the checksum rule.
pub fn patch_field(bytes: &mut [u8], name: &str, at: usize, value: &[u8]) {
    let header = header_of(bytes, name);
    bytes[header + at..header + at + value.len()].copy_from_slice(value);
    reseal(bytes, header);
}
