//! Receiving a signed webhook.
//!
//! # The thing this template exists to teach
//!
//! **How to read a request body, and how to use a secret.** Most small software
//! takes input, and a webhook is the smallest honest example: the body is the
//! whole point, and it is worthless unless you can tell it came from who it
//! claims to.
//!
//! # Two things worth copying
//!
//! `request.into_body()` consumes the request. A body can be taken exactly
//! once, and taking `self` makes the second attempt a compile error rather than
//! a runtime one.
//!
//! `secret()` returns `None` for a secret this app was not granted **and** for
//! one that does not exist. Those are deliberately the same answer: an app that
//! could tell them apart could discover which secrets exist by asking for names
//! in a loop. If you need a secret and do not have it, that is your app being
//! misconfigured — fail as your own error, and say which name is missing in the
//! log where the caller cannot read it.

use clwd_sdk::http::{RawRequest, ResponseOutparam};
use clwd_sdk::{Error, Method, Request, Response, secret};

/// The secret this app expects to be granted, named in `clwd.toml`.
const SIGNING_KEY: &str = "webhook_signing_key";

/// The header the sender puts its signature in.
const SIGNATURE_HEADER: &str = "x-signature";

struct App;

impl clwd_sdk::exports::wasi::http::incoming_handler::Guest for App {
    fn handle(raw: RawRequest, out: ResponseOutparam) {
        let request = Request::new(raw);

        if request.method() != Method::Post {
            return Error::NotFound.respond(out);
        }

        let Some(key) = secret(SIGNING_KEY) else {
            // Not the caller's fault and not their business. The log names the
            // secret; the response does not.
            println!("{SIGNING_KEY} was not granted — add it to clwd.toml and set it with `clwd secrets set`");
            return Error::Internal.respond(out);
        };

        // Read the signature BEFORE the body: `into_body` consumes the request,
        // and the headers go with it.
        let Some(claimed) = request.header(SIGNATURE_HEADER) else {
            return Error::BadRequest.respond(out);
        };

        let Some(body) = request.into_body() else {
            return Error::BadRequest.respond(out);
        };

        if !signature_matches(&key, &body, &claimed) {
            // 403 rather than 400: the request was well formed, and it is not
            // allowed. A caller who sees 400 goes looking for a typo.
            return Error::Forbidden.respond(out);
        }

        println!("accepted a webhook of {} bytes", body.len());
        Response::new(204).send(out, b"");
    }
}

/// Whether `claimed` is the signature this app expects for `body`.
///
/// **Replace this with a real MAC** — HMAC-SHA256 over the body, keyed with
/// `key`, compared in constant time. It is left out because the right one
/// depends on who is calling you: GitHub, Stripe and Slack each specify a
/// different construction, and a plausible-looking wrong one is worse than an
/// obvious placeholder.
///
/// The comparison below is deliberately trivial and deliberately not secure.
fn signature_matches(key: &[u8], body: &[u8], claimed: &[u8]) -> bool {
    let _ = (key, body);
    !claimed.is_empty()
}

clwd_sdk::export!(App with_types_in clwd_sdk);
