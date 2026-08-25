//! Fixtures for the wasm backend tests.

#![allow(dead_code)]
#![cfg(feature = "wasm")]

use std::sync::Arc;

use clwd_bundle::{build, BundleRef, Capabilities, Limits, Runtime};
use clwd_runtime::identity::{Caller, GroupId, IdentityCtx, OrgId, Principal, Role, SubjectId};
use clwd_runtime::wasm::{MemoryBundleStore, WasmSandbox};
use clwd_runtime::{Body, Request};

/// A real component built from `wit/world.wit`. Rebuild with
/// `just rebuild-guest`.
pub const GUEST: &[u8] = include_bytes!("../fixtures/app.wasm");
/// Valid WebAssembly, but a core module rather than a component.
pub const CORE_MODULE: &[u8] = include_bytes!("../fixtures/core-module.wasm");

pub fn bundle_of(entry: &[u8]) -> (BundleRef, Vec<u8>) {
    bundle_with_limits(entry, 5000, 128)
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
