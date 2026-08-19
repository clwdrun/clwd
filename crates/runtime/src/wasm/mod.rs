//! wasmtime-backed `Sandbox` for short-lived, request/response apps.
//! Component model + `wasmtime-wasi-http`; epoch interruption for CPU limits.

pub struct WasmSandbox;
