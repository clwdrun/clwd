//! The request an app receives and the response it sends.
//!
//! # What this exists to prevent
//!
//! `wasi:http` gets a response out in eight steps, and two of them have an
//! ordering requirement that is stated in no type and no signature:
//!
//! ```ignore
//! let response = OutgoingResponse::new(Fields::new());
//! response.set_status_code(200).unwrap();
//! let body = response.body().unwrap();
//! ResponseOutparam::set(outparam, Ok(response)); // must precede body.write()
//! let stream = body.write().unwrap();
//! stream.blocking_write_and_flush(payload).unwrap();
//! drop(stream);                                   // must precede finish()
//! OutgoingBody::finish(body, None).unwrap();
//! ```
//!
//! Write the stream before setting the outparam and the request hangs. Call
//! `finish` while the stream is alive and it fails. Neither mistake is a type
//! error, so the only way to learn the order is to have already got it wrong.
//!
//! **So the order is made unrepresentable rather than documented.**
//! [`Response::send`] consumes the [`ResponseOutparam`], and there is no way to
//! obtain a body stream at all — the sequence exists in one place, correct, and
//! an app cannot express the broken version. This is the same move as
//! `Sandbox::stop` consuming its `Handle`.
//!
//! The five `unwrap()`s go with it. Every one is a host-side failure an app can
//! do nothing about, and a panic in a guest is a trap that costs the whole
//! instance; they become early returns.

use alloc::string::String;
use alloc::vec::Vec;

use crate::wasi::http::types::{
    Fields, IncomingRequest, Method as WasiMethod, OutgoingBody, OutgoingResponse,
};

/// The channel a handler answers on, from `wasi:http`.
///
/// Re-exported because it appears in every app's handler signature: an author
/// should not have to name the generated module to write one. It is passed to
/// [`Response::send`] by value, which is what makes answering twice impossible.
pub use crate::wasi::http::types::ResponseOutparam;

/// The raw request type, for the same reason.
pub use crate::wasi::http::types::IncomingRequest as RawRequest;

/// The HTTP method a request arrived with.
///
/// `Other` carries the method verbatim rather than being dropped: an app
/// serving a method this enum does not name should be able to see which one it
/// was, and a future `wasi:http` method should not silently become `Get`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Method {
    Get,
    Head,
    Post,
    Put,
    Delete,
    Connect,
    Options,
    Trace,
    Patch,
    /// A method outside the set above, as written.
    Other(String),
}

impl From<WasiMethod> for Method {
    fn from(m: WasiMethod) -> Self {
        match m {
            WasiMethod::Get => Self::Get,
            WasiMethod::Head => Self::Head,
            WasiMethod::Post => Self::Post,
            WasiMethod::Put => Self::Put,
            WasiMethod::Delete => Self::Delete,
            WasiMethod::Connect => Self::Connect,
            WasiMethod::Options => Self::Options,
            WasiMethod::Trace => Self::Trace,
            WasiMethod::Patch => Self::Patch,
            WasiMethod::Other(name) => Self::Other(name),
        }
    }
}

/// An incoming request.
///
/// The path is read once, when this is constructed, so [`Request::path`] can
/// hand out a borrow instead of allocating a fresh `String` on every match arm.
#[non_exhaustive]
pub struct Request {
    inner: IncomingRequest,
    path: String,
}

impl Request {
    /// Wrap the request `wasi:http` handed the guest.
    ///
    /// Takes the raw type by value because an `IncomingRequest` can be consumed
    /// only once; owning it here is what lets [`Request::into_body`] be safe.
    #[must_use]
    pub fn new(inner: IncomingRequest) -> Self {
        // An absent path is not an error worth propagating: there is nothing an
        // app can do differently, and every route will simply fail to match.
        let path = inner.path_with_query().unwrap_or_default();
        Self { inner, path }
    }

    /// The path, with query string if there was one.
    ///
    /// Borrowed, so matching on it costs nothing:
    ///
    /// ```ignore
    /// match request.path() {
    ///     "/health" => ...,
    ///     p if p.starts_with("/items/") => ...,
    ///     _ => ...,
    /// }
    /// ```
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// The method this request arrived with.
    #[must_use]
    pub fn method(&self) -> Method {
        self.inner.method().into()
    }

    /// The value of `name`, if the request carried that header.
    ///
    /// Header names are matched case-insensitively by `wasi:http`. A header
    /// sent more than once yields the first value; an app needing all of them
    /// should reach for the raw `Fields`.
    #[must_use]
    pub fn header(&self, name: &str) -> Option<Vec<u8>> {
        self.inner.headers().get(&name.to_ascii_lowercase()).pop()
    }

    /// Read the whole request body.
    ///
    /// Consumes the request, because `wasi:http` allows a body to be taken
    /// exactly once — calling it twice returns an error rather than the body
    /// again. Taking `self` moves that rule into the type system, so the second
    /// call does not compile instead of failing at runtime.
    ///
    /// Returns `None` when the body cannot be read at all. A request with no
    /// body reads as empty, which is different and is `Some(vec![])`.
    ///
    /// The whole body is buffered. For an app that needs to stream a large
    /// upload, the raw `IncomingBody` is still reachable — see the crate docs
    /// on escaping this layer.
    #[must_use]
    pub fn into_body(self) -> Option<Vec<u8>> {
        let body = self.inner.consume().ok()?;
        let stream = body.stream().ok()?;

        let mut out = Vec::new();
        loop {
            // 8 KiB per read: large enough that a body of any size is a few
            // calls, small enough that an empty body does not reserve much.
            match stream.blocking_read(8192) {
                Ok(chunk) if chunk.is_empty() => break,
                Ok(chunk) => out.extend_from_slice(&chunk),
                // End of stream arrives as an error, which is the wasi
                // convention rather than a failure.
                Err(_) => break,
            }
        }
        Some(out)
    }
}

/// A response under construction.
///
/// Built, then sent once. See the module docs for why sending is the only
/// operation and why there is no way to reach the body stream.
#[must_use = "a response that is never sent leaves the caller waiting"]
#[non_exhaustive]
pub struct Response {
    status: u16,
    headers: Vec<(String, Vec<u8>)>,
}

impl Response {
    /// A response with this status and no headers.
    pub fn new(status: u16) -> Self {
        Self {
            status,
            headers: Vec::new(),
        }
    }

    /// `200 OK`.
    pub fn ok() -> Self {
        Self::new(200)
    }

    /// Add a header. Chainable.
    ///
    /// No `#[must_use]` of its own: the type already carries one, and a second
    /// is what clippy calls `double_must_use`.
    pub fn header(mut self, name: &str, value: &[u8]) -> Self {
        self.headers
            .push((name.to_ascii_lowercase(), value.to_vec()));
        self
    }

    /// Send this response with `body`, and finish the exchange.
    ///
    /// Consumes both the response and the [`ResponseOutparam`], so it can
    /// happen exactly once and cannot be interleaved with anything. The whole
    /// eight-step sequence from the module docs lives here, in the one order
    /// that works.
    ///
    /// Failures are silent by design. Every one of them is the host declining
    /// to accept a response an app has already finished computing: there is no
    /// second thing to try, and a panic here would trap the instance over a
    /// condition the app did not cause and cannot fix.
    pub fn send(self, out: ResponseOutparam, body: &[u8]) {
        let fields = Fields::new();
        for (name, value) in &self.headers {
            // A rejected header is dropped rather than failing the response.
            // Losing one header is recoverable for a caller; losing the whole
            // response is not.
            let _ = fields.append(name, value);
        }

        let response = OutgoingResponse::new(fields);
        let _ = response.set_status_code(self.status);

        let Ok(out_body) = response.body() else {
            // The outparam still has to be answered, or the caller waits until
            // the host's own deadline expires.
            ResponseOutparam::set(
                out,
                Err(crate::wasi::http::types::ErrorCode::InternalError(None)),
            );
            return;
        };

        // THE ORDERING. Set before writing: reversing these two lines is the
        // hang this module exists to make unrepresentable.
        ResponseOutparam::set(out, Ok(response));

        let Ok(stream) = out_body.write() else {
            return;
        };
        // Chunked because `blocking_write_and_flush` refuses more than the
        // stream's permitted write size in one call, and a body larger than
        // that is an ordinary thing for an app to produce.
        for chunk in body.chunks(4096) {
            if stream.blocking_write_and_flush(chunk).is_err() {
                break;
            }
        }

        // The stream must be gone before the body is finished.
        drop(stream);
        let _ = OutgoingBody::finish(out_body, None);
    }
}
