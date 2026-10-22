//! Notes: write one, see your own, delete one.
//!
//! # The thing this template exists to teach
//!
//! **Your app's storage belongs to your app, not to whoever is calling it.**
//! Every caller reads and writes the same keys unless you scope them yourself.
//! `data::for_caller` is the scoping: it prefixes every key with the caller's
//! subject, so two people cannot see or overwrite each other's notes. The
//! alternative, `data::shared()`, is one namespace for everyone — right for
//! app-wide configuration, a data leak for anything belonging to a person.
//!
//! The subject is **pairwise**: the same person has a different subject in
//! every app. It keys this app's data and means nothing anywhere else, so never
//! send it to another app or compare it with one from elsewhere.
//!
//! # Three rules every page here follows
//!
//! - **Who is calling comes from `caller()`, never from the request.** There is
//!   no authentication code: the host decides who is calling before `handle`
//!   runs. A form field naming a user is whatever the browser chose to send.
//! - **Reading never changes anything.** Every `GET` only reads; creating and
//!   deleting are `POST`. A browser follows a `GET` link from any other site,
//!   cookies and all, so a `GET` that deletes is a link that deletes.
//! - **Every stored value is escaped on its way into HTML.** A note containing
//!   `<script>` is shown as text. `Html` only accepts values through `text`,
//!   which escapes.

use clwd_sdk::data::{self, DataError, Namespace};
use clwd_sdk::http::{RawRequest, ResponseOutparam};
use clwd_sdk::{Caller, Error, Html, Method, Request, Response, caller, form};

/// The key holding this caller's note ids, newest last, one per line.
///
/// `clwd:app-data` cannot list keys, on purpose, so an app that needs to find
/// its records keeps their ids itself.
const INDEX: &str = "index";

/// The longest note accepted, in bytes. A limit is a decision; no limit is
/// also a decision, made by whoever sends the largest body.
const MAX_NOTE: usize = 2000;

struct App;

impl clwd_sdk::exports::wasi::http::incoming_handler::Guest for App {
    fn handle(raw: RawRequest, out: ResponseOutparam) {
        let request = Request::new(raw);

        let who = caller();
        // `None` for two different reasons, and they are not the caller's
        // fault in the same way. Anonymous means "sign in". A subject that
        // cannot appear in a key means the platform handed us something
        // unusable: ours to log, not theirs to know about.
        let Some(mine) = data::for_caller(&who) else {
            return match who {
                Caller::Anonymous => Error::Unauthenticated.respond(out),
                other => {
                    // The subject goes in the LOG, never in the response.
                    println!(
                        "subject {} cannot be used as a storage key",
                        other.subject().unwrap_or("<none>")
                    );
                    Error::Internal.respond(out)
                }
            };
        };

        let path = request.path().to_owned();
        match (request.method(), path.as_str()) {
            (Method::Get, "/") => list(&mine, out),
            (Method::Post, "/notes") => create(&mine, request, out),
            (Method::Post, path) => match delete_target(path) {
                Some(id) => delete(&mine, id, out),
                None => Error::NotFound.respond(out),
            },
            _ => Error::NotFound.respond(out),
        }
    }
}

/// `GET /`: the form, and this caller's notes. Reads only.
fn list(mine: &Namespace, out: ResponseOutparam) {
    let ids = match index(mine) {
        Ok(ids) => ids,
        Err(e) => return storage_failed("reading the index", &e, out),
    };

    let mut page = Html::new();
    page.raw(HEAD).raw(
        "<h1>notes</h1>\
         <form method=\"post\" action=\"/notes\">\
         <textarea name=\"text\" rows=\"3\" cols=\"40\" required></textarea>\
         <button>add</button></form><ul>",
    );
    for id in ids.iter().rev() {
        let note = match mine.get(&note_key(id)) {
            Ok(Some(note)) => note,
            // In the index but not stored: a delete that stopped halfway.
            // Skipping it is the honest rendering.
            Ok(None) => continue,
            Err(e) => return storage_failed("reading a note", &e, out),
        };
        // Stored bytes are shown as text, escaped, whatever they contain.
        page.raw("<li>")
            .text(&String::from_utf8_lossy(&note))
            .raw("<form method=\"post\" action=\"/notes/")
            // The id is ours — 16 hex digits we generated — but it goes through
            // `text` all the same. Not escaping something because it is "known
            // safe" is how the unsafe one gets through later.
            .text(id)
            .raw("/delete\"><button>delete</button></form></li>");
    }
    page.raw("</ul>").raw(FOOT);
    Response::ok().html(out, &page);
}

/// `POST /notes`: store a new note, then send the browser back to the list.
fn create(mine: &Namespace, request: Request, out: ResponseOutparam) {
    let Some(body) = request.into_body() else {
        return Error::BadRequest.respond(out);
    };
    let Some(text) = form::field(&body, "text") else {
        return Error::BadRequest.respond(out);
    };
    let text = text.trim();
    if text.is_empty() || text.len() > MAX_NOTE {
        return Error::BadRequest.respond(out);
    }

    // A random id, so two notes can never share a key.
    let id = format!(
        "{:016x}",
        clwd_sdk::raw::wasi::random::random::get_random_u64()
    );
    let mut ids = match index(mine) {
        Ok(ids) => ids,
        Err(e) => return storage_failed("reading the index", &e, out),
    };
    // The note first, then the index. If the second write fails, the note is
    // stored and unlisted — invisible, not corrupt.
    if let Err(e) = mine.set(&note_key(&id), text.as_bytes()) {
        return storage_failed("writing a note", &e, out);
    }
    ids.push(id);
    // There is no atomic read-modify-write in `clwd:app-data`. Two submissions
    // from the same person at the same instant can each read the old index,
    // and one note drops out of the list. The index is per caller, so only one
    // person's own double-submit can race.
    if let Err(e) = mine.set(INDEX, ids.join("\n").as_bytes()) {
        return storage_failed("writing the index", &e, out);
    }
    back_to_list(out);
}

/// `POST /notes/<id>/delete`: remove one note, then back to the list.
fn delete(mine: &Namespace, id: &str, out: ResponseOutparam) {
    let mut ids = match index(mine) {
        Ok(ids) => ids,
        Err(e) => return storage_failed("reading the index", &e, out),
    };
    ids.retain(|kept| kept != id);
    // The index first, then the note: if the second write fails, the note is
    // already gone from the list.
    if let Err(e) = mine.set(INDEX, ids.join("\n").as_bytes()) {
        return storage_failed("writing the index", &e, out);
    }
    // Deleting what is not there succeeds, so a repeated click is harmless.
    if let Err(e) = mine.delete(&note_key(id)) {
        return storage_failed("deleting a note", &e, out);
    }
    back_to_list(out);
}

/// The note id in `/notes/<id>/delete`, if `path` is one.
///
/// Only an id this app could have generated is accepted, so a request cannot
/// steer the key to anything but a note.
fn delete_target(path: &str) -> Option<&str> {
    let id = path.strip_prefix("/notes/")?.strip_suffix("/delete")?;
    (id.len() == 16 && id.bytes().all(|b| b.is_ascii_hexdigit())).then_some(id)
}

/// This caller's note ids, oldest first.
fn index(mine: &Namespace) -> Result<Vec<String>, DataError> {
    Ok(match mine.get(INDEX)? {
        Some(bytes) => String::from_utf8_lossy(&bytes)
            .lines()
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect(),
        None => Vec::new(),
    })
}

fn note_key(id: &str) -> String {
    format!("note.{id}")
}

/// `303 See Other` to the list, the standard answer to a form `POST`: the
/// browser follows it with a `GET`, so reloading the page does not resubmit.
fn back_to_list(out: ResponseOutparam) {
    Response::new(303).header("location", b"/").send(out, b"");
}

/// A storage failure is the app's problem, not the caller's: the detail goes to
/// the log, which `clwd logs` shows, and the response says only that it failed.
fn storage_failed(doing: &str, e: &DataError, out: ResponseOutparam) {
    println!("{doing} failed: {e}");
    Error::Internal.respond(out);
}

const HEAD: &str = "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
<title>notes</title></head><body>";

const FOOT: &str = "</body></html>";

clwd_sdk::export!(App with_types_in clwd_sdk);
