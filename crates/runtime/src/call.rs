//! What crosses the trait per call: bodies, deadlines, per-call options.

use std::time::{Duration, Instant};

/// A request body.
///
/// Buffered in this version. The newtype exists so that adding a streaming body
/// later changes this type's internals rather than the trait's signature — but
/// the hedge is **partial, not complete**: a streaming body cannot keep
/// [`Body::into_bytes`] infallible and synchronous, so that method's shape
/// would still change.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Body(bytes::Bytes);

impl Body {
    pub fn empty() -> Self {
        Self(bytes::Bytes::new())
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn into_bytes(self) -> bytes::Bytes {
        self.0
    }
}

impl<T: Into<bytes::Bytes>> From<T> for Body {
    fn from(value: T) -> Self {
        Self(value.into())
    }
}

/// The HTTP request handed to an app.
///
/// `http::Request` rather than a hand-rolled struct: it is already in the
/// dependency graph, carries no async runtime, and `wasmtime-wasi-http` speaks
/// it — so a bespoke type would be more code *and* an extra conversion.
pub type Request = http::Request<Body>;

/// The HTTP response an app produced.
pub type Response = http::Response<Body>;

/// The largest response body a guest may return, in bytes: 16 MiB.
///
/// Responses are buffered, so without a cap a guest decides how much host
/// memory one request takes. A body past this fails the invocation with
/// `runtime.resource_exhausted` rather than being truncated: a cut-off body
/// served as complete is worse than an error.
pub const MAX_RESPONSE_BODY: usize = 16 * 1024 * 1024;

/// A point in time after which work should be abandoned.
///
/// **Absolute, not relative.** A relative budget silently renews itself at every
/// hop: a five-second budget with one second already spent becomes five seconds
/// again at the next call. Absolute deadlines compose.
///
/// `Instant` is not serializable and is meaningless in another process, so any
/// process boundary — the vsock in #17, HTTP in the control plane — converts to
/// [`Deadline::remaining`] at the edge and reconstitutes on the far side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Deadline(Instant);

impl Deadline {
    /// A deadline `budget` from now.
    pub fn after(budget: Duration) -> Self {
        Self(Instant::now() + budget)
    }

    /// A deadline at an already-computed instant.
    pub fn at(instant: Instant) -> Self {
        Self(instant)
    }

    pub fn instant(&self) -> Instant {
        self.0
    }

    /// Time left, saturating at zero rather than panicking on an expired
    /// deadline.
    pub fn remaining(&self) -> Duration {
        self.0.saturating_duration_since(Instant::now())
    }

    pub fn is_expired(&self) -> bool {
        self.remaining().is_zero()
    }
}

/// Per-spawn options.
///
/// Separate from [`InvokeOptions`] because cold start and invocation are
/// different budgets. `#[non_exhaustive]`: this will grow — #17's resource
/// limits land here — and every addition would otherwise break every
/// construction site across the repository seam.
///
/// Build with [`Default`] and assign, since a `#[non_exhaustive]` struct cannot
/// be built with a struct literal outside this crate, `..Default::default()`
/// included:
///
/// ```
/// # use clwd_runtime::call::{Deadline, SpawnOptions};
/// # use std::time::Duration;
/// let mut opts = SpawnOptions::default();
/// opts.deadline = Some(Deadline::after(Duration::from_secs(5)));
/// ```
/// A struct literal is rejected outside this crate, which is the whole point of
/// the marker. `#[non_exhaustive]` has no effect *within* the defining crate, so
/// this is checked from a doctest — which rustdoc compiles as its own crate:
///
/// ```compile_fail
/// # use clwd_runtime::call::SpawnOptions;
/// let opts = SpawnOptions { deadline: None };
/// ```
///
/// Functional update syntax is rejected too, which is why the pattern above is
/// assignment rather than `..Default::default()`:
///
/// ```compile_fail
/// # use clwd_runtime::call::SpawnOptions;
/// let opts = SpawnOptions { deadline: None, ..Default::default() };
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct SpawnOptions {
    /// Cold-start budget. `None` means the backend's own default applies.
    pub deadline: Option<Deadline>,
}

/// Per-invocation options. See [`SpawnOptions`] for the construction pattern.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct InvokeOptions {
    /// Invocation budget, separate from the cold-start budget.
    pub deadline: Option<Deadline>,
    /// Correlation identifier, echoed onto every [`crate::LogLine`] this call
    /// produces.
    pub request_id: Option<RequestId>,
}

/// Correlates a request with the log lines it produced.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RequestId(String);

impl RequestId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for RequestId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
