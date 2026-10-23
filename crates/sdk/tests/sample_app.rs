//! A whole app, compiled.
//!
//! The crate documentation shows a handler; a doc example marked `ignore` is
//! prose that stops being true silently. This is the same app as a test, so a
//! change that breaks the shape shown to every app author breaks the build.
//!
//! It cannot *run* here — a guest needs a host — but compiling is the property
//! that matters: it is what an author's first `cargo build` does, and every
//! type, trait bound and macro path in the example has to line up for it.

#![allow(dead_code)]

use clwd_sdk::http::{RawRequest, ResponseOutparam};
use clwd_sdk::{Error, Method, Request, Response, caller, data};

struct Notes;

impl clwd_sdk::exports::wasi::http::incoming_handler::Guest for Notes {
    fn handle(raw: RawRequest, out: ResponseOutparam) {
        let request = Request::new(raw);

        // An anonymous caller has no private namespace, and the type says so
        // rather than inventing a key every anonymous caller would share.
        let Some(mine) = data::for_caller(&caller()) else {
            return Error::Unauthenticated.respond(out);
        };

        match (request.method(), request.path()) {
            (Method::Get, "/note") => match mine.get("note") {
                Ok(Some(v)) => Response::ok().send(out, &v),
                Ok(None) => Error::NotFound.respond(out),
                Err(_) => Error::Internal.respond(out),
            },
            (Method::Put, "/note") => {
                let Some(body) = request.into_body() else {
                    return Error::BadRequest.respond(out);
                };
                match mine.set("note", &body) {
                    Ok(()) => Response::new(204).send(out, b""),
                    Err(_) => Error::Internal.respond(out),
                }
            }
            _ => Error::NotFound.respond(out),
        }
    }
}

/// The status table is a contract an agent generating code depends on, so it
/// is asserted rather than assumed.
#[test]
fn every_error_maps_to_a_sane_status() {
    for e in clwd_sdk::error::ALL {
        let status = e.status();
        assert!(
            (400..=599).contains(&status),
            "{e:?} maps to {status}, which is not a failure status"
        );
        assert!(!e.message().is_empty(), "{e:?} has no message");
    }
}

/// The reason a message is `&'static str`: there is no way to put a value in
/// one. If a variant ever gains a field, this stops compiling.
///
/// Invariant: SDK-4.
#[test]
fn an_error_message_cannot_carry_caller_supplied_data() {
    let e = Error::BadRequest;
    let message: &'static str = e.message();
    assert_eq!(message, "bad request");
}

/// Distinct statuses where the distinction is the point: 401 tells a caller to
/// sign in, 403 tells them not to bother.
#[test]
fn unauthenticated_and_forbidden_do_not_collapse() {
    assert_eq!(Error::Unauthenticated.status(), 401);
    assert_eq!(Error::Forbidden.status(), 403);
}
