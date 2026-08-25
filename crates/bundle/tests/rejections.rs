//! One test per entry in the specification's rejection catalogue (§9).
//! A catalogue entry with no test is a claim nobody has checked.

mod common;
use common::*;

use clwd_bundle::{decode, File};

fn reject(bytes: &[u8], code: &str) {
    assert_code(decode(bytes, None), code);
}

// ------------------------------------------------------------- container

#[test]
fn not_tar() {
    reject(&bundle(members()).bytes[..100], "bundle.not_tar");
}

#[test]
fn magic_invalid() {
    let mut bytes = bundle(members()).bytes;
    patch_field(&mut bytes, "app.wasm", 257, b"ustar ");
    reject(&bytes, "bundle.magic_invalid");
}

#[test]
fn entry_type_invalid() {
    let mut bytes = bundle(members()).bytes;
    // '5' is a directory. The same rule rejects symlinks, devices, and the PAX
    // and GNU extension entries.
    patch_field(&mut bytes, "app.wasm", 156, b"5");
    reject(&bytes, "bundle.entry_type_invalid");
}

#[test]
fn prefix_not_empty() {
    let mut bytes = bundle(members()).bytes;
    patch_field(&mut bytes, "app.wasm", 345, b"some/prefix");
    reject(&bytes, "bundle.prefix_not_empty");
}

#[test]
fn mode_invalid() {
    let mut bytes = bundle(members()).bytes;
    patch_field(&mut bytes, "app.wasm", 100, b"0000600\0");
    reject(&bytes, "bundle.mode_invalid");
}

#[test]
fn metadata_not_canonical() {
    for (offset, value) in [
        (108usize, &b"0001750\0"[..]), // uid
        (116, &b"0001750\0"[..]),      // gid
        (136, &b"14664717271\0"[..]),  // mtime
        (265, &b"root\0"[..]),         // uname
        (297, &b"root\0"[..]),         // gname
    ] {
        let mut bytes = bundle(members()).bytes;
        patch_field(&mut bytes, "app.wasm", offset, value);
        reject(&bytes, "bundle.metadata_not_canonical");
    }
}

#[test]
fn checksum_invalid() {
    let mut bytes = bundle(members()).bytes;
    let header = header_of(&bytes, "app.wasm");
    // Deliberately no reseal: this is the one test that wants the mismatch.
    bytes[header + 100..header + 108].copy_from_slice(b"0000755\0");
    reject(&bytes, "bundle.checksum_invalid");
}

#[test]
fn size_encoding_invalid() {
    let mut bytes = bundle(members()).bytes;
    // GNU base-256 signals itself with the high bit of the first size byte.
    patch_field(&mut bytes, "app.wasm", 124, &[0x80]);
    reject(&bytes, "bundle.size_encoding_invalid");
}

#[test]
fn member_too_large() {
    // Never reachable from a header: eleven octal digits cannot express it.
    // The manifest is where a number above the ceiling can be written down.
    let text = valid_manifest_text().replace("size = 8", "size = 8589934592");
    reject(&with_manifest(&text), "bundle.member_too_large");
}

#[test]
fn padding_invalid() {
    let mut bytes = bundle(members()).bytes;
    let data = header_of(&bytes, "app.wasm") + 512;
    bytes[data + 8] = 0xff; // first padding byte after an 8-byte member
    reject(&bytes, "bundle.padding_invalid");
}

#[test]
fn entries_not_sorted() {
    let text = valid_manifest_text();
    let bytes = pack_in_order(&[
        ("clwd.toml".to_owned(), text.into_bytes(), false),
        ("app.wasm".to_owned(), COMPONENT.to_vec(), false),
    ]);
    reject(&bytes, "bundle.entries_not_sorted");
}

#[test]
fn trailer_invalid() {
    let mut bytes = bundle(members()).bytes;
    // A third zero block: what a blocking factor would add.
    bytes.resize(bytes.len() + 512, 0);
    reject(&bytes, "bundle.trailer_invalid");
}

// ----------------------------------------------------------------- paths

#[test]
fn path_invalid() {
    for path in [
        "/absolute",
        "../escape",
        "a//b",
        "trailing/",
        "with\nnewline",
    ] {
        assert_code(try_bundle(one_member(path, b"x")), "bundle.path_invalid");
    }
}

#[test]
fn path_not_nfc() {
    // The encoder normalizes, so this can only be built by hand — which is
    // exactly the hostile-encoder case the decode-side rule exists for.
    let bytes = pack(&[("cafe\u{301}.txt".to_owned(), b"x".to_vec(), false)]);
    reject(&bytes, "bundle.path_not_nfc");
}

#[test]
fn path_too_long() {
    let path = format!("{}.wasm", "a".repeat(101));
    assert_code(try_bundle(one_member(&path, b"x")), "bundle.path_too_long");
}

#[test]
fn path_duplicate() {
    let files = vec![
        File {
            path: "a.wasm".to_owned(),
            data: b"x".to_vec(),
            executable: false,
        },
        File {
            path: "a.wasm".to_owned(),
            data: b"y".to_vec(),
            executable: false,
        },
    ];
    assert_code(try_bundle(files), "bundle.path_duplicate");
}

// -------------------------------------------------------------- manifest

#[test]
fn manifest_missing() {
    let bytes = pack(&[("app.wasm".to_owned(), COMPONENT.to_vec(), false)]);
    reject(&bytes, "bundle.manifest_missing");
}

#[test]
fn manifest_not_toml() {
    reject(
        &with_manifest("this is not [ toml"),
        "bundle.manifest_not_toml",
    );
}

#[test]
fn schema_version_missing() {
    let text = valid_manifest_text().replace("schema_version = 1\n", "");
    reject(&with_manifest(&text), "bundle.schema_version_missing");
}

#[test]
fn schema_version_unsupported() {
    let text = valid_manifest_text().replace("schema_version = 1", "schema_version = 2");
    reject(&with_manifest(&text), "bundle.schema_version_unsupported");
}

#[test]
fn unknown_key() {
    // §7.1: rejected wherever it appears, not only at the top level.
    // Prepended, not appended: a key after the last table header would land
    // inside [capabilities.limits] and test a different rule.
    let top = valid_manifest_text().replace("runtime =", "telemetry = true\nruntime =");
    reject(&with_manifest(&top), "bundle.unknown_key");

    let in_app = valid_manifest_text().replace(
        "entrypoint = \"app.wasm\"",
        "entrypoint = \"app.wasm\"\npreload = []",
    );
    reject(&with_manifest(&in_app), "bundle.unknown_key");

    let in_member = valid_manifest_text().replace("size = 8", "size = 8\ncompression = \"zstd\"");
    reject(&with_manifest(&in_member), "bundle.unknown_key");
}

#[test]
fn capability_unknown_key() {
    let in_caps = valid_manifest_text().replace("egress = []", "egress = []\ngpu = true");
    reject(&with_manifest(&in_caps), "bundle.capability_unknown_key");

    // The rule covers the whole subtree, limits included: a limit this version
    // does not know is a constraint it is not applying.
    let in_limits =
        valid_manifest_text().replace("memory_mb = 128", "memory_mb = 128\negress_mbps = 100");
    reject(&with_manifest(&in_limits), "bundle.capability_unknown_key");
}

#[test]
fn capabilities_version_unsupported() {
    // Anchored to the table header: "version = 1" also matches inside
    // "schema_version = 1", which would test the wrong rule.
    let text =
        valid_manifest_text().replace("[capabilities]\nversion = 1", "[capabilities]\nversion = 2");
    reject(
        &with_manifest(&text),
        "bundle.capabilities_version_unsupported",
    );
}

#[test]
fn field_missing() {
    let text = valid_manifest_text().replace("memory_mb = 128\n", "");
    reject(&with_manifest(&text), "bundle.field_missing");
}

#[test]
fn field_type_invalid() {
    let text = valid_manifest_text().replace("size = 8", "size = \"8\"");
    reject(&with_manifest(&text), "bundle.field_type_invalid");
}

#[test]
fn field_value_invalid() {
    let zero = valid_manifest_text().replace("cpu_ms = 5000", "cpu_ms = 0");
    reject(&with_manifest(&zero), "bundle.field_value_invalid");

    // §5.3: grants are sorted and duplicate-free so one grant set has one
    // spelling.
    let unsorted = valid_manifest_text().replace("egress = []", "egress = [\"b\", \"a\"]");
    reject(&with_manifest(&unsorted), "bundle.field_value_invalid");

    // §5.5: no control characters in any manifest string.
    let control = valid_manifest_text().replace("egress = []", "egress = [\"a\\u0007b\"]");
    reject(&with_manifest(&control), "bundle.field_value_invalid");
}

#[test]
fn runtime_unknown() {
    let text = valid_manifest_text().replace("runtime = \"wasm\"", "runtime = \"jvm\"");
    reject(&with_manifest(&text), "bundle.runtime_unknown");
}

#[test]
fn manifest_not_canonical() {
    // Parses to exactly the same model; different bytes, so a different bundle
    // digest for identical content. That is what canonical form prevents.
    let text = valid_manifest_text().replace("cpu_ms = 5000", "cpu_ms = 5_000");
    reject(&with_manifest(&text), "bundle.manifest_not_canonical");
}

// --------------------------------------------------------------- members

#[test]
fn manifest_self_listed() {
    let text = valid_manifest_text().replace(
        "\n[capabilities]",
        "\n[[members]]\npath = \"clwd.toml\"\ndigest = \"blake3:af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262\"\nsize = 0\n\n[capabilities]",
    );
    reject(&with_manifest(&text), "bundle.manifest_self_listed");
}

#[test]
fn member_missing() {
    let text = valid_manifest_text().replace("path = \"app.wasm\"", "path = \"ghost.wasm\"");
    reject(&with_manifest(&text), "bundle.member_missing");
}

#[test]
fn member_unlisted() {
    let text = valid_manifest_text();
    let bytes = pack(&[
        ("app.wasm".to_owned(), COMPONENT.to_vec(), false),
        ("stowaway.bin".to_owned(), b"extra".to_vec(), false),
        ("clwd.toml".to_owned(), text.into_bytes(), false),
    ]);
    reject(&bytes, "bundle.member_unlisted");
}

#[test]
fn member_size_mismatch() {
    let text = valid_manifest_text().replace("size = 8", "size = 7");
    reject(&with_manifest(&text), "bundle.member_size_mismatch");
}

#[test]
fn member_digest_mismatch() {
    let text = valid_manifest_text().replace(
        "blake3:48d3a2e8a9b0cde6293ba7f68824a97d069d096f44335a9e86aee55238dcdcb8",
        "blake3:af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262",
    );
    reject(&with_manifest(&text), "bundle.member_digest_mismatch");
}

#[test]
fn digest_malformed() {
    for bad in [
        "sha256:48d3a2e8a9b0cde6293ba7f68824a97d069d096f44335a9e86aee55238dcdcb8",
        "blake3:48D3A2E8A9B0CDE6293BA7F68824A97D069D096F44335A9E86AEE55238DCDCB8",
        "blake3:tooshort",
    ] {
        let text = valid_manifest_text().replace(
            "blake3:48d3a2e8a9b0cde6293ba7f68824a97d069d096f44335a9e86aee55238dcdcb8",
            bad,
        );
        reject(&with_manifest(&text), "bundle.digest_malformed");
    }
    // The same rule guards a BundleRef parsed from a string.
    assert_code(
        "blake3:nope".parse::<clwd_bundle::BundleRef>(),
        "bundle.digest_malformed",
    );
}

// ------------------------------------------------- entrypoint and runtime

#[test]
fn entrypoint_not_a_member() {
    let text =
        valid_manifest_text().replace("entrypoint = \"app.wasm\"", "entrypoint = \"other.wasm\"");
    reject(&with_manifest(&text), "bundle.entrypoint_not_a_member");
}

#[test]
fn wasm_not_component() {
    // Valid WebAssembly, and still rejected: #16 requires a component, and
    // accepting a core module only defers the failure to somewhere less clear.
    let packed = bundle(one_member("app.wasm", CORE_MODULE));
    reject(&packed.bytes, "bundle.wasm_not_component");
}

#[test]
fn wasm_preamble_invalid() {
    let packed = bundle(one_member("app.wasm", b"#!/bin/sh\n"));
    reject(&packed.bytes, "bundle.wasm_preamble_invalid");
}

// ---------------------------------------------------------------- bundle

#[test]
fn digest_mismatch() {
    let packed = bundle(members());
    let wrong = bundle(one_member("app.wasm", COMPONENT)).digest;
    assert_code(decode(&packed.bytes, Some(wrong)), "bundle.digest_mismatch");
}

/// The catalogue is only as good as its coverage. This asserts every code in
/// the specification is named by a test in this file.
#[test]
fn every_catalogue_entry_has_a_test() {
    let spec = include_str!("../../../docs/spec/bundle-v1.md");
    let source = include_str!("rejections.rs");

    let mut untested: Vec<&str> = spec
        .split('`')
        .filter(|t| t.starts_with("bundle.") && !t.contains(' ') && !t.contains('*'))
        .map(|t| t.trim_end_matches('.'))
        .filter(|code| !source.contains(&format!("\"{code}\"")))
        .collect();
    untested.sort_unstable();
    untested.dedup();
    assert!(
        untested.is_empty(),
        "catalogue entries with no test: {untested:?}"
    );
}
