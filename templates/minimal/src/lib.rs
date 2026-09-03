//! The smallest thing that answers a request.
//!
//! Everything here is required. Nothing here is optional, and nothing is
//! demonstrated for its own sake — if you delete a line, the app stops working.
//! Templates that show identity, storage and secrets are `notes` and `webhook`.

use clwd_sdk::http::{RawRequest, ResponseOutparam};
use clwd_sdk::{Error, Method, Request, Response};

struct App;

impl clwd_sdk::exports::wasi::http::incoming_handler::Guest for App {
    fn handle(raw: RawRequest, out: ResponseOutparam) {
        let request = Request::new(raw);

        // Routing is a `match`. There is no router, deliberately: this is
        // already idiomatic Rust, and it is readable without learning anything
        // about clwd first.
        match (request.method(), request.path()) {
            (Method::Get, "/") => Response::ok().send(out, b"hello"),

            // `/health` returns 204 rather than 200-with-a-body: there is
            // nothing to say, and saying nothing is a status, not an empty
            // string.
            (Method::Get, "/health") => Response::new(204).send(out, b""),

            // Every other path. An app that does not answer is worse than one
            // that says no — the caller waits for a timeout instead of getting
            // an answer.
            _ => Error::NotFound.respond(out),
        }
    }
}

clwd_sdk::export!(App with_types_in clwd_sdk);
