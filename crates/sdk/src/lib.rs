//! What app code links against.
//!
//! An app is one wasm component that answers HTTP requests. It gets a verified
//! caller, a key-value store, and whatever secrets it was granted — and it
//! never handles a token, a session, or a signature to do it.
//!
//! ```ignore
//! use clwd_sdk::{Caller, Error, Request, Response, caller, data, export};
//! use clwd_sdk::http::{RawRequest, ResponseOutparam};
//!
//! struct Notes;
//!
//! impl clwd_sdk::exports::wasi::http::incoming_handler::Guest for Notes {
//!     fn handle(raw: RawRequest, out: ResponseOutparam) {
//!         let request = Request::new(raw);
//!
//!         let Some(mine) = data::for_caller(&caller()) else {
//!             return Error::Unauthenticated.respond(out);
//!         };
//!
//!         match (request.method(), request.path()) {
//!             (clwd_sdk::Method::Get, "/note") => match mine.get("note") {
//!                 Ok(Some(v)) => Response::ok().send(out, &v),
//!                 Ok(None) => Error::NotFound.respond(out),
//!                 Err(_) => Error::Internal.respond(out),
//!             },
//!             _ => Error::NotFound.respond(out),
//!         }
//!     }
//! }
//!
//! export!(Notes with_types_in clwd_sdk);
//! ```
//!
//! # Routing is a `match`, on purpose
//!
//! There is no router. A `match` on `(method, path)` is already the idiomatic
//! Rust for this, costs nothing, and is legible to someone who has never seen
//! this crate — which matters, because app code here is often generated from a
//! template by an agent that has to produce working code without learning a
//! vocabulary first.
//!
//! Method dispatch and typed path parameters are what would justify a router
//! later. Adding one is additive; requiring one now is not.
//!
//! # What each piece is for
//!
//! - [`caller`] — who is calling. Ambient: no request argument to thread,
//!   because an instance sees exactly one caller for its whole lifetime.
//! - [`http`] — the request, and a response you cannot send in the wrong order.
//! - [`data`] — the app's key-value store, and the per-caller scoping it does
//!   **not** do for you.
//! - [`secret`] — a credential this app was granted, or `None`.
//! - [`Error`] — an application failure and the status it becomes.
//!
//! # Three things this crate deliberately does not do
//!
//! **It does not flatten [`Caller`].** Anonymous is a kind of caller, not a
//! missing one, and there is no `subject() -> Option<String>` convenience at
//! the top level that would invite `unwrap_or_default()`. See [`caller`].
//!
//! **It does not offer `require_secret`.** An error that distinguishes "no such
//! secret" from "not granted to you" is a probe a guest could loop over. See
//! [`secrets_api`].
//!
//! **It does not scope app-data by caller unless you ask.** The namespace
//! belongs to the app, every caller shares it, and that is the feature — but it
//! is also the opposite of what most people assume. See [`data`].
//!
//! # Escaping this layer
//!
//! Everything here is a thin layer over generated bindings, and none of it is
//! hidden. [`raw`] holds the bindings as `wit-bindgen` produced them, plus all
//! of `wasi`. An app needing a capability this crate has not wrapped reaches
//! through rather than waiting:
//!
//! ```ignore
//! use clwd_sdk::raw::wasi;
//! let pollable = wasi::clocks::monotonic_clock::subscribe_duration(nanos);
//! ```
//!
//! Reaching for [`raw`] is not a defeat. It is the escape hatch working, and
//! the repository's own test fixture does it for exactly this reason.
//!
//! # Where the bindings come from
//!
//! The `clwd:app` world is generated from `wit/` at compile time by
//! [`wit_bindgen::generate!`] — there is no checked-in generated code, so
//! nothing can drift from the WIT it came from. Editing `wit/` and rebuilding
//! is the whole regeneration story.

#![no_std]

extern crate alloc;

wit_bindgen::generate!({
    path: "../../wit",
    world: "app",
    generate_all,
    // The guest links this; nothing here should pull in a runtime.
    //
    // `pub_export_macro` is what lets an app in another crate write
    // `clwd_sdk::export!(App with_types_in clwd_sdk)`. The `with_types_in` is
    // not optional across a crate boundary: without it the generated macro
    // resolves its types against `self`, which is this crate from in here and
    // the app's crate from out there.
    pub_export_macro: true,
});

pub mod caller;
pub mod data;
pub mod error;
pub mod http;
pub mod secrets_api;

pub use caller::{Caller, caller};
pub use error::Error;
pub use http::{Method, Request, Response};
pub use secrets_api::secret;

/// The generated bindings, unwrapped.
///
/// See "Escaping this layer" in the crate docs. Nothing above hides anything
/// here; this is the same surface, one level down.
pub mod raw {
    pub use crate::clwd::{app_data, identity, secrets};
    pub use crate::wasi;
}

// The generated modules are public because `raw` and the `export!` macro both
// name paths inside them. Prefer the wrappers above, or `raw`, which says what
// it is.
pub use clwd::app_data;
pub use clwd::identity;
pub use clwd::secrets;
