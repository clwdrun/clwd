//! Hello, whoever is calling.
//!
//! # The thing this template exists to teach
//!
//! **There is no authentication code in this app, and there never needs to
//! be.** No login page, no session, no cookie, no token. By the time `handle`
//! runs, the host has already decided who is calling and that they may; the
//! app asks `caller()` and gets a verified answer.
//!
//! So identity comes from `caller()` and from nowhere else. Not a form field,
//! not a query parameter, not a header, not a cookie: those are whatever the
//! browser — or anyone with `curl` — chose to send.
//!
//! The second thing: **every value that reaches HTML is escaped.** An email
//! address is an ordinary-looking string until someone's contains `<script>`.
//! `Html` makes that impossible to forget — see `greeting`.

use clwd_sdk::http::{RawRequest, ResponseOutparam};
use clwd_sdk::{Caller, Error, Html, Method, Request, Response, caller};

struct App;

impl clwd_sdk::exports::wasi::http::incoming_handler::Guest for App {
    fn handle(raw: RawRequest, out: ResponseOutparam) {
        let request = Request::new(raw);

        // Routing is a `match`. There is no router, deliberately: this is
        // already idiomatic Rust, and it is readable without learning anything
        // about clwd first.
        match (request.method(), request.path()) {
            (Method::Get, "/") => greeting(out),

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

/// The page: hello, and the caller's verified email.
fn greeting(out: ResponseOutparam) {
    let mut page = Html::new();
    page.raw(HEAD);

    // `Caller` is an enum, not an `Option`, so every kind of caller is a branch
    // someone wrote. There is no wildcard for an anonymous caller to fall into.
    match caller() {
        Caller::User(user) => match &user.email {
            // `text` escapes; `raw` takes only literals. A value cannot reach
            // the page unescaped, because there is no method that would put it
            // there.
            Some(email) => {
                page.raw("<h1>hello, ").text(email).raw("</h1>");
            }
            // An email is optional. Never fall back to the subject: it is an
            // opaque id, meaningful to storage, not to people.
            None => {
                page.raw("<h1>hello</h1><p>Your account has no verified email.</p>");
            }
        },
        Caller::Service(_) => {
            page.raw("<h1>hello, service</h1>");
        }
        // The host turns away callers who have not signed in, so a deployed app
        // does not see this today. Handling it now means the app does not
        // change when share links make it reachable.
        Caller::Anonymous => return Error::Unauthenticated.respond(out),
    }

    page.raw(FOOT);
    // `html` sets the content type and `nosniff`, so the browser renders this as
    // the HTML it is and nothing else.
    Response::ok().html(out, &page);
}

const HEAD: &str = "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
<title>hello</title></head><body>";

const FOOT: &str = "</body></html>";

clwd_sdk::export!(App with_types_in clwd_sdk);
