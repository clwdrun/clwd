//! Shared fixtures. Bundles are real: built by `clwd-bundle`, so a corrupted
//! one produces a genuine `bundle.*` error rather than a synthesized stand-in.

#![allow(dead_code)]

use clwd_bundle::{BundleRef, Capabilities, Limits, Runtime, build};
use clwd_runtime::identity::{Caller, IdentityCtx, OrgId, Principal, SubjectId};

/// The component-model preamble: the smallest valid `runtime = "wasm"` entry.
const COMPONENT: &[u8] = &[0x00, 0x61, 0x73, 0x6d, 0x0d, 0x00, 0x01, 0x00];

/// A real, valid bundle and the digest that names it.
pub fn valid_bundle() -> (BundleRef, Vec<u8>) {
    let encoded = build(
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
            data: COMPONENT.to_vec(),
            executable: false,
        }],
    )
    .expect("the fixture is a valid bundle");
    (encoded.digest, encoded.bytes)
}

/// The same bundle with one byte of a member flipped: still a well-formed
/// archive, but the member no longer matches its declared digest.
pub fn tampered_bundle() -> (BundleRef, Vec<u8>) {
    let (_, mut bytes) = valid_bundle();
    bytes[512] ^= 0xff; // first content byte of app.wasm
    (BundleRef::of_stream(&bytes), bytes)
}

pub fn user(subject: &str) -> IdentityCtx {
    IdentityCtx::new(Caller::User(Principal::new(
        SubjectId::new(subject),
        OrgId::new("acme"),
    )))
}

pub fn subject_of(caller: &Caller) -> Option<&str> {
    match caller {
        Caller::Anonymous => None,
        Caller::User(p) => Some(p.subject.as_str()),
        Caller::Service(s) => Some(s.subject.as_str()),
    }
}
