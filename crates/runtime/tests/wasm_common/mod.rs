//! Fixtures for the wasm backend tests.

#![allow(dead_code)]
#![cfg(feature = "wasm")]

use std::sync::Arc;

use clwd_bundle::{BundleRef, Capabilities, Limits, Runtime, build};
use clwd_runtime::identity::{Caller, GroupId, IdentityCtx, OrgId, Principal, Role, SubjectId};
use clwd_runtime::wasm::{
    AppId, DigestAsAppId, MemoryAppDataStore, MemoryBundleStore, MemorySecretStore, WasmSandbox,
};
use clwd_runtime::{Body, Request};

/// A real component built from `wit/world.wit`. Rebuild with
/// `just rebuild-guest`.
pub const GUEST: &[u8] = include_bytes!("../fixtures/app.wasm");
/// Valid WebAssembly, but a core module rather than a component.
pub const CORE_MODULE: &[u8] = include_bytes!("../fixtures/core-module.wasm");

pub fn bundle_of(entry: &[u8]) -> (BundleRef, Vec<u8>) {
    bundle_with_limits(entry, 5000, 128)
}

/// A bundle whose manifest declares the given secret names.
pub fn bundle_declaring(entry: &[u8], secrets: &[&str]) -> (BundleRef, Vec<u8>) {
    let encoded = clwd_bundle::build(
        Runtime::Wasm,
        "app.wasm",
        Capabilities {
            version: 1,
            egress: Vec::new(),
            secrets: secrets.iter().map(|s| (*s).to_owned()).collect(),
            storage_namespaces: Vec::new(),
            limits: Limits {
                cpu_ms: 5000,
                memory_mb: 128,
            },
        },
        vec![clwd_bundle::File {
            path: "app.wasm".to_owned(),
            data: entry.to_vec(),
            executable: false,
        }],
    )
    .expect("fixture is a valid bundle");
    (encoded.digest, encoded.bytes)
}

/// A sandbox whose secret store holds `held`, for a bundle declaring
/// `declared`. The injection set is the intersection.
pub fn sandbox_with_secrets(
    declared: &[&str],
    held: &[(&str, &str)],
) -> (WasmSandbox, BundleRef, Arc<MemoryAppDataStore>) {
    let (digest, bytes) = bundle_declaring(GUEST, declared);
    let bundles = MemoryBundleStore::new();
    bundles.insert(&digest, bytes);

    let secrets = MemorySecretStore::new();
    let app = AppId::new(digest.to_string());
    for (name, value) in held {
        secrets.insert(&app, name, *value);
    }

    let data = Arc::new(MemoryAppDataStore::new());
    let sandbox = WasmSandbox::with_stores(
        Arc::new(bundles),
        Arc::new(secrets),
        Arc::clone(&data) as Arc<dyn clwd_runtime::wasm::AppDataStore>,
        Arc::new(DigestAsAppId),
    )
    .expect("engine builds");
    (sandbox, digest, data)
}

/// Two distinct apps sharing one sandbox, so their namespaces can be compared.
pub fn sandbox_with_two_apps() -> (WasmSandbox, BundleRef, BundleRef) {
    // Different declared secrets give different manifests, so different
    // digests, so different apps under `DigestAsAppId`.
    let (one, one_bytes) = bundle_declaring(GUEST, &["a"]);
    let (two, two_bytes) = bundle_declaring(GUEST, &["b"]);
    let bundles = MemoryBundleStore::new();
    bundles.insert(&one, one_bytes);
    bundles.insert(&two, two_bytes);
    let sandbox = WasmSandbox::with_stores(
        Arc::new(bundles),
        Arc::new(MemorySecretStore::new()),
        Arc::new(MemoryAppDataStore::new()),
        Arc::new(DigestAsAppId),
    )
    .expect("engine builds");
    (sandbox, one, two)
}

/// A bundle whose sealed manifest carries the given limits.
pub fn bundle_with_limits(entry: &[u8], cpu_ms: u64, memory_mb: u64) -> (BundleRef, Vec<u8>) {
    let encoded = build(
        Runtime::Wasm,
        "app.wasm",
        Capabilities {
            version: 1,
            egress: Vec::new(),
            secrets: Vec::new(),
            storage_namespaces: Vec::new(),
            limits: Limits { cpu_ms, memory_mb },
        },
        vec![clwd_bundle::File {
            path: "app.wasm".to_owned(),
            data: entry.to_vec(),
            executable: false,
        }],
    )
    .expect("fixture is a valid bundle");
    (encoded.digest, encoded.bytes)
}

/// A sandbox with the guest already stored, under the given sealed limits.
pub fn sandbox_with_limits(cpu_ms: u64, memory_mb: u64) -> (WasmSandbox, BundleRef) {
    let (digest, bytes) = bundle_with_limits(GUEST, cpu_ms, memory_mb);
    let store = MemoryBundleStore::new();
    store.insert(&digest, bytes);
    (
        WasmSandbox::new(Arc::new(store)).expect("engine builds"),
        digest,
    )
}

/// A sandbox with the guest already stored.
pub fn sandbox() -> (WasmSandbox, BundleRef) {
    let (digest, bytes) = bundle_of(GUEST);
    let store = MemoryBundleStore::new();
    store.insert(&digest, bytes);
    (
        WasmSandbox::new(Arc::new(store)).expect("engine builds"),
        digest,
    )
}

pub fn user(subject: &str) -> IdentityCtx {
    IdentityCtx::new(Caller::User(Principal::new(
        SubjectId::new(subject),
        OrgId::new("acme"),
    )))
}

pub fn user_with(subject: &str, role: Role, groups: &[&str]) -> IdentityCtx {
    let mut ctx = user(subject);
    ctx.role = Some(role);
    ctx.groups = groups.iter().map(|g| GroupId::new(*g)).collect();
    ctx
}

pub fn get(path: &str) -> Request {
    http::Request::builder()
        .method("GET")
        .uri(format!("http://app.test{path}"))
        .body(Body::empty())
        .expect("valid request")
}

pub fn text(response: &clwd_runtime::Response) -> String {
    String::from_utf8(response.body().as_bytes().to_vec()).expect("utf-8 body")
}
