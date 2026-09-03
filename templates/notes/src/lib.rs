//! One note per caller.
//!
//! # The thing this template exists to teach
//!
//! **Your app's storage belongs to your app, not to whoever is calling it.**
//! Every caller reads and writes the same keys unless you scope them yourself.
//! Caller A stores a note under `"note"`, caller B reads `"note"`, and caller B
//! gets caller A's note. That is not a bug in the platform — the data belongs
//! to the application, the way rows in its database do.
//!
//! It is also the opposite of what most people assume, because identity sits
//! right next to it and *is* per-caller.
//!
//! `data::for_caller` is the scoping. It prefixes every key with the caller's
//! subject, so two callers cannot collide. The alternative, `data::shared()`,
//! is the app-wide namespace and is the right choice for configuration — for
//! anything belonging to a person, it is a data leak between your own users.
//!
//! # Identity, and what is real today
//!
//! `caller()` returns a verified caller. Your app never sees a token, a
//! session, or a signature, and there is no authentication code to write: the
//! host resolves who is calling before this code runs.
//!
//! **The code below is final. The value it receives is not yet.** Locally the
//! caller is always `local` in org `local`; a real subject arrives with
//! `platform#11`. The same code path serves both — only the value changes.

use clwd_sdk::http::{RawRequest, ResponseOutparam};
use clwd_sdk::{Error, Method, Request, Response, caller, data};

struct App;

impl clwd_sdk::exports::wasi::http::incoming_handler::Guest for App {
    fn handle(raw: RawRequest, out: ResponseOutparam) {
        let request = Request::new(raw);

        // THIS is the line that keeps one caller's notes away from another's.
        //
        // `for_caller` returns `None` for an anonymous caller, because there is
        // no subject to scope by — and inventing one would give every anonymous
        // caller the same namespace, which is the leak this exists to prevent.
        // The compiler will not let you ignore that case.
        //
        // Swapping this for `data::shared()` compiles and passes every test you
        // are likely to write. It also makes every caller share one note.
        let who = caller();
        let Some(mine) = data::for_caller(&who) else {
            // Two different reasons, and they are not the caller's fault in the
            // same way. Anonymous means "sign in". A subject that cannot appear
            // in a key means the platform handed us something unusable, which
            // is ours to fix and theirs to know nothing about.
            return match who {
                clwd_sdk::Caller::Anonymous => Error::Unauthenticated.respond(out),
                other => {
                    // The subject goes in the LOG, never in the response.
                    // `clwd logs` shows this to you; the caller never sees it.
                    // That split is why `Error`'s message is a fixed string
                    // with nowhere to interpolate — a body that could carry
                    // "subject X was rejected" could carry anything.
                    //
                    // `{}` and not `{:?}`. Debug formatting a `&str` adds
                    // quoting and escaping machinery to the component —
                    // measured at 8.1 KiB on this template, 67.3 against
                    // 75.4 KiB — for a pair of quotes nobody needs.
                    println!(
                        "subject {} cannot be used as a storage key",
                        other.subject().unwrap_or("<none>")
                    );
                    Error::Internal.respond(out)
                }
            };
        };

        match (request.method(), request.path()) {
            (Method::Get, "/note") => match mine.get("note") {
                Ok(Some(note)) => Response::ok().send(out, &note),
                Ok(None) => Error::NotFound.respond(out),
                // A storage failure is your app's problem, not the caller's:
                // they cannot fix it and telling them how it broke helps
                // nobody. The detail goes to the log instead.
                Err(e) => {
                    println!("reading a note failed: {e}");
                    Error::Internal.respond(out)
                }
            },

            (Method::Put, "/note") => {
                // Reading the body consumes the request, because a body can be
                // taken exactly once. The compiler enforces that rather than
                // leaving it as a runtime error.
                let Some(body) = request.into_body() else {
                    return Error::BadRequest.respond(out);
                };
                match mine.set("note", &body) {
                    Ok(()) => Response::new(204).send(out, b""),
                    Err(e) => {
                        println!("writing a note failed: {e}");
                        Error::Internal.respond(out)
                    }
                }
            }

            (Method::Delete, "/note") => match mine.delete("note") {
                // Deleting what is not there succeeds, so a caller need not
                // check first.
                Ok(()) => Response::new(204).send(out, b""),
                Err(e) => {
                    println!("deleting a note failed: {e}");
                    Error::Internal.respond(out)
                }
            },

            _ => Error::NotFound.respond(out),
        }
    }
}

clwd_sdk::export!(App with_types_in clwd_sdk);
