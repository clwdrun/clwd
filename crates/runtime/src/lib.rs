//! `Sandbox` is the one abstraction the control plane is allowed to see.
//! Nothing above this crate should branch on which backend is running.

use async_trait::async_trait;

pub struct BundleRef;
pub struct IdentityCtx;
pub struct Handle;
pub struct Request;
pub struct Response;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("sandbox error: {0}")]
    Sandbox(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// The one abstraction above this crate.
///
/// # Caller-isolation invariant
///
/// `clwd:identity/context` exposes the caller ambiently — `get-caller()`
/// takes no request argument (see the DECISION 1 block in
/// `wit/deps/identity/identity.wit`). That is only sound if a given
/// component instance is never observed by two different callers:
///
/// > A component instance observes exactly one caller for its whole
/// > lifetime. An instance that has handled a request for caller A is never
/// > given a request for caller B.
///
/// Every implementor owes this. It is not implied by the `Sandbox` signature
/// and the type system does not enforce it — `call` takes `&Handle`, so
/// nothing here stops a backend from routing two callers at one instance.
///
/// Note that `wasi:http/proxy`, which world `clwd:app` includes, explicitly
/// permits the opposite ("a host may arbitrarily reuse or not reuse component
/// instance"). clwd deliberately overrides that permission. A backend that
/// inherits the wasi-http default instead of enforcing the rule above is
/// leaking identity across tenants, which SECURITY.md scopes as a
/// vulnerability in that backend.
///
/// Satisfy it by not reusing instances across callers, or by tearing down
/// ambient state between callers so no value from A is readable by B.
#[async_trait]
pub trait Sandbox {
    async fn start(&self, bundle: BundleRef, ctx: IdentityCtx) -> Result<Handle>;
    async fn call(&self, h: &Handle, req: Request) -> Result<Response>;
    async fn stop(&self, h: Handle) -> Result<()>;
}

#[cfg(feature = "wasm")]
pub mod wasm;

#[cfg(feature = "microvm")]
pub mod microvm;
