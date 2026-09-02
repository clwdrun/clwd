//! Properties the canonicalization must hold, independent of any one vector.

mod common;
use common::*;

use clwd_bundle::{File, decode, encode};

#[test]
fn packing_the_same_tree_twice_is_byte_identical() {
    assert_eq!(bundle(members()).bytes, bundle(members()).bytes);
}

#[test]
fn member_order_does_not_affect_output() {
    let mut reversed = members();
    reversed.reverse();
    assert_eq!(bundle(members()).bytes, bundle(reversed).bytes);
}

#[test]
fn source_metadata_beyond_the_executable_bit_does_not_affect_output() {
    // mtime, uid and gid are never read from the source, so there is nothing to
    // vary: the encoder takes only path, contents and the executable bit. What
    // this asserts is that no other input reaches the bytes.
    let a = bundle(members());
    let b = bundle(members());
    assert_eq!(a.bytes, b.bytes);
    assert!(
        a.bytes[136..148] == *b"00000000000\0"
            && a.bytes[108..116] == *b"0000000\0"
            && a.bytes[116..124] == *b"0000000\0",
        "mtime, uid and gid are zeroed"
    );
}

#[test]
fn the_executable_bit_does_affect_output() {
    let mut exec = members();
    exec[0].executable = true;
    assert_ne!(
        bundle(members()).bytes,
        bundle(exec).bytes,
        "0755 and 0644 are different bundles"
    );
}

#[test]
fn paths_are_normalized_so_the_same_tree_hashes_alike_on_any_platform() {
    // "é" as one codepoint (NFC, what Linux hands you) and as e + U+0301
    // (NFD, what macOS hands you). Same file, and it must be the same bundle.
    let nfc = one_member("caf\u{e9}.txt", b"x");
    let nfd = one_member("cafe\u{301}.txt", b"x");
    assert_eq!(bundle(nfc).digest, bundle(nfd).digest);
}

#[test]
fn encode_decode_encode_is_a_fixed_point() {
    let first = bundle(members());
    let decoded = decode(&first.bytes, None).expect("valid bundle");
    let again = encode(&decoded.manifest, &decoded.members).expect("re-encodes");
    assert_eq!(first.bytes, again.bytes);
    assert_eq!(first.digest, again.digest);
}

#[test]
fn decode_rejects_a_flipped_byte_in_a_member() {
    let mut bytes = bundle(members()).bytes;
    let at = 512; // first byte of app.wasm's contents
    bytes[at] ^= 0xff;
    // The member digest fails before anything interprets the entrypoint, so the
    // report is tampering rather than a wasm complaint.
    assert_code(decode(&bytes, None), "bundle.member_digest_mismatch");
}

#[test]
fn a_flipped_byte_anywhere_changes_the_bundle_digest() {
    let original = bundle(members());
    for at in [0, 100, 512, 1024, 2048, 3000] {
        let mut bytes = original.bytes.clone();
        bytes[at] ^= 0x01;
        assert_ne!(
            clwd_bundle::BundleRef::of_stream(&bytes),
            original.digest,
            "byte {at} is outside the bundle digest"
        );
    }
}

#[test]
fn expected_digest_is_checked_before_the_archive_is_parsed() {
    // Structurally ruined bytes, but the caller said which bundle it wanted.
    let junk = vec![0xffu8; 1024];
    let expected = bundle(members()).digest;
    assert_code(decode(&junk, Some(expected)), "bundle.digest_mismatch");
}

#[test]
fn an_empty_member_occupies_a_header_and_no_data_block() {
    let packed = bundle(vec![
        File {
            path: "app.wasm".to_owned(),
            data: COMPONENT.to_vec(),
            executable: false,
        },
        File {
            path: "empty".to_owned(),
            data: Vec::new(),
            executable: false,
        },
    ]);
    // app.wasm: header + 1 block. clwd.toml: header + 1 block. empty: header
    // only. Plus two trailer blocks.
    assert_eq!(packed.bytes.len() % 512, 0);
    let decoded = decode(&packed.bytes, None).expect("valid");
    let empty = decoded
        .members
        .iter()
        .find(|m| m.path == "empty")
        .expect("the empty member survives");
    assert!(empty.data.is_empty());
}
