//! The §10 conformance vector. This is the arbiter over the spec's prose: a
//! paragraph can be misread, a byte array cannot.

use clwd_bundle::{build, Capabilities, Limits, Runtime};

/// The component-model preamble, which is also the smallest valid component.
const APP_WASM: &[u8] = &[0x00, 0x61, 0x73, 0x6d, 0x0d, 0x00, 0x01, 0x00];

const APP_DIGEST: &str =
    "blake3:48d3a2e8a9b0cde6293ba7f68824a97d069d096f44335a9e86aee55238dcdcb8";
const MANIFEST_DIGEST: &str =
    "blake3:7e775181bd6658c8579ae0970bc214b43347f17b6b27e9eb817a088f8126a849";
const BUNDLE_DIGEST: &str =
    "blake3:5fc9042ff861616e77ec7b6b37a70a806e19c94d70b4c8fe016f52f30d7209a7";

fn vector() -> clwd_bundle::Encoded {
    build(
        Runtime::Wasm,
        "app.wasm",
        Capabilities {
            version: 1,
            egress: Vec::new(),
            secrets: Vec::new(),
            storage_namespaces: Vec::new(),
            limits: Limits {
                cpu_ms: 5000,
                memory_mb: 128,
            },
        },
        vec![clwd_bundle::File {
            path: "app.wasm".to_owned(),
            data: APP_WASM.to_vec(),
            executable: false,
        }],
    )
    .expect("the vector is a valid bundle")
}

#[test]
fn encoder_reproduces_the_specified_bundle_digest() {
    assert_eq!(vector().digest.to_string(), BUNDLE_DIGEST);
}

#[test]
fn archive_has_the_specified_layout() {
    let bytes = vector().bytes;
    assert_eq!(bytes.len(), 3072, "six 512-byte blocks");
    assert_eq!(&bytes[0..8], b"app.wasm", "app.wasm sorts first: 0x61 < 0x63");
    assert_eq!(&bytes[124..136], b"00000000010\0", "size, octal 10 = 8");
    assert_eq!(&bytes[148..156], b"007405\0 ", "checksum: 6 digits, NUL, space");
    assert_eq!(bytes[156], 0x30, "typeflag is 0x30, not the 0x00 default");
    assert_eq!(&bytes[257..265], b"ustar\x0000");
    assert!(bytes[345..500].iter().all(|b| *b == 0), "prefix is empty");

    assert_eq!(&bytes[1024..1033], b"clwd.toml");
    assert_eq!(&bytes[1148..1160], b"00000000500\0", "octal 500 = 320");
    assert_eq!(&bytes[1172..1180], b"007566\0 ");

    assert!(
        bytes[2048..].iter().all(|b| *b == 0),
        "two trailing zero blocks, and they are inside the digest"
    );
}

#[test]
fn member_digests_match_the_specification() {
    let manifest = clwd_bundle::decode(&vector().bytes, None)
        .expect("the vector decodes")
        .manifest;
    assert_eq!(manifest.members[0].digest.to_string(), APP_DIGEST);
    assert_eq!(manifest.members[0].size, 8);

    let bytes = vector().bytes;
    let manifest_body = &bytes[1536..1536 + 320];
    assert_eq!(
        clwd_bundle::Digest::of(manifest_body).to_string(),
        MANIFEST_DIGEST
    );
}

/// The manifest text in §10 is normative down to the byte. If the canonical
/// serializer drifts from it, the spec and the code have diverged.
#[test]
fn canonical_manifest_matches_the_specification_text() {
    let spec = include_str!("../../../docs/spec/bundle-v1.md");
    let documented = spec
        .split("```toml\n")
        .map(|block| block.split("```").next().unwrap_or(""))
        .find(|block| block.starts_with("schema_version = 1\nruntime = \"wasm\"\n\n[app]"))
        .expect("the vector's manifest is in the specification");

    let bytes = vector().bytes;
    let produced = std::str::from_utf8(&bytes[1536..1536 + 320]).expect("utf-8");
    assert_eq!(produced, documented);
}

/// The vector as a committed byte fixture. Regenerate deliberately with
/// `just spec-reseal` after a spec change, never to make this test pass.
#[test]
fn matches_the_committed_fixture() {
    let fixture = include_bytes!("../../../docs/spec/fixtures/conformance-v1.clwd");
    assert_eq!(
        vector().bytes,
        fixture.as_slice(),
        "encoder output drifted from the committed conformance fixture"
    );
}

/// Regenerates the fixture. A generator, not a test, so it is ignored by
/// default: run it deliberately after a spec change with
/// `cargo test -p clwd-bundle --test conformance -- --ignored`, then
/// `just spec-reseal`. Never run it to make the test above pass.
#[test]
#[ignore = "generator; run deliberately after a spec change"]
fn regenerate_fixture() {
    let out = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/spec/fixtures/conformance-v1.clwd"
    );
    std::fs::write(out, vector().bytes).expect("write the fixture");
}
