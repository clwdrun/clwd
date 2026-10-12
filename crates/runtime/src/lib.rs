//! `Sandbox` is the one abstraction the control plane is allowed to see.
//! Nothing above this crate should branch on which backend is running.

use async_trait::async_trait;

pub mod call;
pub mod error;
pub mod handle;
pub mod identity;
pub mod logs;

#[cfg(feature = "test-util")]
pub mod test_util;

#[cfg(feature = "conformance")]
pub mod conformance;

pub use call::{
    AppId, Body, Deadline, InvokeOptions, MAX_RESPONSE_BODY, Request, RequestId, Response,
    SpawnOptions,
};
pub use error::{Error, Result};
pub use handle::{Handle, InstanceId};
pub use identity::{Caller, IdentityCtx, OrgId, Principal, ServicePrincipal, SubjectId};
pub use logs::{LogLevel, LogLine, LogStream, LogStreamKind};

/// `BundleRef` is defined by `clwd-bundle` and re-exported, never redefined.
/// One digest type, one parser, one spelling.
pub use clwd_bundle::BundleRef;

/// The one abstraction above this crate.
///
/// Object-safe: the control plane holds a single `dyn Sandbox` and never
/// branches on which backend is behind it. `#[async_trait]` boxes each returned
/// future, which is one allocation per call — unavoidable at a `dyn` boundary
/// whatever the mechanism, and negligible beside the instantiation cost that
/// dominates `invoke`.
///
/// # Caller-isolation invariant
///
/// `clwd:identity/context` exposes the caller ambiently — `get-caller()` takes
/// no request argument (see the DECISION 1 block in
/// `wit/deps/identity/identity.wit`). That is only sound if a given component
/// instance is never observed by two different callers:
///
/// > A component instance observes exactly one caller for its whole lifetime.
/// > An instance that has handled a request for caller A is never given a
/// > request for caller B.
///
/// `wasi:http/proxy`, which world `clwd:app` includes, explicitly permits the
/// opposite ("a host may arbitrarily reuse or not reuse component instance").
/// clwd deliberately overrides that permission. A backend that inherits the
/// wasi-http default is leaking identity across tenants, which SECURITY.md
/// scopes as a vulnerability in that backend rather than a documentation gap.
///
/// ## What these signatures enforce
///
/// - [`Sandbox::spawn`] **consumes** an [`IdentityCtx`], and [`Handle`] can be
///   built no other way. Every handle was minted against exactly one caller.
/// - [`Sandbox::invoke`] takes **no identity at all**. Serving an existing
///   instance to a second caller has nowhere to be written: not a parameter,
///   not a field, not a setter. The bug is inexpressible in terms of this API.
/// - [`Handle`] is not `Clone`, and [`Sandbox::stop`] takes it by value, so
///   use-after-stop is a compile error.
///
/// ## What a backend must still uphold by discipline
///
/// - **Do not alias instances across handles.** Nothing here can see inside an
///   implementation, so a backend that pools guests internally can hand two
///   handles to one instance. Satisfy the invariant by not reusing instances
///   across callers, or by tearing down ambient state between callers such that
///   no value from A is readable by B.
/// - **Isolate concurrent invocations.** `invoke` takes `&Handle`, so calls may
///   overlap. That is deliberate: a per-request wasm instance and a long-lived
///   microVM serving one caller's many requests have different needs. Ordering
///   and reentrancy are the backend's to get right.
/// - **Do not leak state between a caller's own requests.** Same caller, so not
///   an identity leak, but still a data leak.
///
/// For `wasm`: one instance per request, or pooled strictly per-caller. For
/// `microvm`: the VM is long-lived by design, so the guest agent must scope
/// identity per request rather than per instance. That is the weaker backend
/// for this property.
#[async_trait]
pub trait Sandbox: Send + Sync {
    /// Start an instance of `bundle` that will serve exactly the caller in
    /// `ctx`, and nobody else, for its whole lifetime.
    ///
    /// Capabilities are **not** taken from `ctx`. They come from the manifest
    /// inside the bundle the digest commits to, which is the only source: a
    /// second copy reachable here could disagree with the sealed one, and a
    /// permission set with two sources of truth is the failure the bundle
    /// format exists to prevent.
    async fn spawn(
        &self,
        bundle: BundleRef,
        ctx: IdentityCtx,
        opts: SpawnOptions,
    ) -> Result<Handle>;

    /// Handle one request on an already-running instance.
    ///
    /// Takes no identity: see the caller-isolation invariant above.
    async fn invoke(&self, handle: &Handle, req: Request, opts: InvokeOptions) -> Result<Response>;

    /// Log lines from this instance, including lines with no request of their
    /// own. The stream stays open until the instance stops.
    async fn logs_stream(&self, handle: &Handle) -> Result<LogStream>;

    /// Stop the instance and release its resources.
    ///
    /// Consumes the handle: whatever else a backend does, the caller cannot
    /// reach a stopped instance.
    async fn stop(&self, handle: Handle) -> Result<()>;
}

#[cfg(feature = "wasm")]
pub mod wasm;

#[cfg(feature = "microvm")]
pub mod microvm;
