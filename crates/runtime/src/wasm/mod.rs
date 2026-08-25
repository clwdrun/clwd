//! wasmtime-backed `Sandbox` for short-lived, request/response apps.
//! Component model + `wasmtime-wasi-http`; epoch interruption for CPU limits.
//!
//! The implementation is #16. This is the type it will hang off, kept in step
//! with the trait so the signatures cannot drift before then.

/// Satisfies the caller-isolation invariant by giving each caller its own
/// instance: one per request, or pooled strictly per-caller. See the invariant
/// on [`crate::Sandbox`].
pub struct WasmSandbox;
