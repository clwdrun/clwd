//! Every template, built for `wasm32-wasip2` and served through the dev
//! server's own request path, with a caller chosen per test.
//!
//! The templates are what people — and agents — copy, so a mistake in one is
//! repeated in every app made from it. These tests hold them to the rules the
//! templates teach: escaping, no state change on `GET`, storage scoped to the
//! caller. The source-level rules are in `tests/templates.rs`.
//!
//! A missing `wasm32-wasip2` target fails the build, and so these tests. It
//! does not skip them.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use bytes::Bytes;
use clwd_bundle::BundleRef;
use clwd_runtime::wasm::{AppDataStore, AppId, MemoryAppDataStore};
use clwd_runtime::{Caller, IdentityCtx, OrgId, Principal, Sandbox, SpawnOptions, SubjectId};

use super::{Active, respond};
use crate::commands::local;

const ALICE: &str = "0b7c2a4e-8f1d-4c3b-9a6e-5d2f1e0c7b8a";
const BOB: &str = "6f3e9d1c-2b4a-4e8f-8c7d-1a0b9c8d7e6f";

fn template_dir(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../templates")
        .join(name)
}

/// The template's bundle, built once per test process.
fn bundle(name: &'static str) -> Vec<u8> {
    static HELLO: OnceLock<Vec<u8>> = OnceLock::new();
    static NOTES: OnceLock<Vec<u8>> = OnceLock::new();
    let cell = match name {
        "hello" => &HELLO,
        "notes" => &NOTES,
        other => panic!("no template called {other}"),
    };
    cell.get_or_init(|| {
        let built = crate::build::run(&crate::build::Options::new(template_dir(name)), None)
            .unwrap_or_else(|e| panic!("the {name} template does not build: {e}"));
        std::fs::read(&built.bundle).expect("read the built bundle")
    })
    .clone()
}

/// An app-data store that counts every write, so a test can prove a request
/// changed nothing.
#[derive(Default)]
struct Counting {
    inner: MemoryAppDataStore,
    writes: AtomicUsize,
}

#[async_trait]
impl AppDataStore for Counting {
    async fn get(&self, app: &AppId, key: &str) -> clwd_runtime::Result<Option<Vec<u8>>> {
        self.inner.get(app, key).await
    }
    async fn set(&self, app: &AppId, key: &str, value: Vec<u8>) -> clwd_runtime::Result<()> {
        self.writes.fetch_add(1, Ordering::SeqCst);
        self.inner.set(app, key, value).await
    }
    async fn delete(&self, app: &AppId, key: &str) -> clwd_runtime::Result<()> {
        self.writes.fetch_add(1, Ordering::SeqCst);
        self.inner.delete(app, key).await
    }
}

fn user(subject: &str, email: Option<&str>) -> IdentityCtx {
    let mut principal = Principal::new(SubjectId::new(subject), OrgId::new("acme"));
    principal.email = email.map(str::to_owned);
    IdentityCtx::new(Caller::User(principal))
}

/// `template`, served to `caller`, over `store`. Two callers given the same
/// store see the same app's data, as two people using one deployed app do.
async fn serve(template: &'static str, caller: IdentityCtx, store: Arc<Counting>) -> Active {
    let bytes = bundle(template);
    let digest = BundleRef::of_stream(&bytes);
    let sandbox = local::local_sandbox_with(&digest, bytes, &template_dir(template), store)
        .expect("build the sandbox");
    let handle = sandbox
        .spawn(digest, caller, SpawnOptions::default())
        .await
        .expect("spawn");
    Active {
        sandbox,
        handle,
        digest,
    }
}

struct Reply {
    status: u16,
    headers: hyper::HeaderMap,
    body: String,
}

impl Reply {
    fn header(&self, name: &str) -> &str {
        self.headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
    }

    /// The reply is an HTML page the browser will not reinterpret.
    fn assert_html(&self) {
        assert_eq!(self.header("content-type"), "text/html; charset=utf-8");
        assert_eq!(self.header("x-content-type-options"), "nosniff");
    }
}

async fn call(active: &Active, method: &str, path: &str, form: &str) -> Reply {
    let mut headers = hyper::HeaderMap::new();
    if method == "POST" {
        headers.insert(
            "content-type",
            hyper::header::HeaderValue::from_static("application/x-www-form-urlencoded"),
        );
    }
    let method = hyper::Method::from_bytes(method.as_bytes()).expect("method");
    let response = respond(active, method, path, headers, Bytes::from(form.to_owned())).await;
    let (parts, body) = response.into_parts();
    let body = http_body_util::BodyExt::collect(body)
        .await
        .expect("body")
        .to_bytes();
    Reply {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body: String::from_utf8_lossy(&body).into_owned(),
    }
}

/// Percent-encode a form value the way a browser does.
fn form_value(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'*' => {
                (b as char).to_string()
            }
            b' ' => "+".to_owned(),
            b => format!("%{b:02X}"),
        })
        .collect()
}

/// The routes a template's README declares for `method`, with `<id>` filled
/// in. The README is the contract the template publishes, so it is what the
/// `GET`-safety test walks.
fn declared(template: &str, method: &str) -> Vec<String> {
    let readme = std::fs::read_to_string(template_dir(template).join("README.md"))
        .expect("every template has a README");
    readme
        .lines()
        .filter_map(|line| {
            let mut cells = line.split('|').map(str::trim);
            let (_, m, path) = (cells.next()?, cells.next()?, cells.next()?);
            (m == method).then(|| path.trim_matches('`').replace("<id>", "0123456789abcdef"))
        })
        .collect()
}

const XSS: &str = "<script>alert('x')</script>";
const XSS_ESCAPED: &str = "&lt;script&gt;alert(&#39;x&#39;)&lt;/script&gt;";

// ---- hello -----------------------------------------------------------------

/// Invariant: SDK-1.
#[tokio::test]
async fn hello_greets_the_caller_by_verified_email() {
    let app = serve(
        "hello",
        user(ALICE, Some("ada@example.com")),
        Arc::default(),
    )
    .await;
    let reply = call(&app, "GET", "/", "").await;
    assert_eq!(reply.status, 200);
    reply.assert_html();
    assert!(
        reply.body.contains("hello, ada@example.com"),
        "{}",
        reply.body
    );
    assert!(
        !reply.body.contains(ALICE),
        "the subject is not for display"
    );
}

/// Invariant: SDK-2.
#[tokio::test]
async fn hello_escapes_an_email_that_contains_markup() {
    let email = format!("{XSS}@example.com");
    let app = serve("hello", user(ALICE, Some(&email)), Arc::default()).await;
    let reply = call(&app, "GET", "/", "").await;
    assert_eq!(reply.status, 200);
    assert!(reply.body.contains(XSS_ESCAPED), "{}", reply.body);
    assert!(!reply.body.contains("<script>"), "{}", reply.body);
}

#[tokio::test]
async fn hello_without_an_email_says_so_rather_than_showing_the_subject() {
    let app = serve("hello", user(ALICE, None), Arc::default()).await;
    let reply = call(&app, "GET", "/", "").await;
    assert_eq!(reply.status, 200);
    assert!(reply.body.contains("no verified email"), "{}", reply.body);
    assert!(!reply.body.contains(ALICE));
}

#[tokio::test]
async fn hello_refuses_an_anonymous_caller() {
    let app = serve("hello", IdentityCtx::new(Caller::Anonymous), Arc::default()).await;
    assert_eq!(call(&app, "GET", "/", "").await.status, 401);
}

#[tokio::test]
async fn hello_answers_health_and_unknown_paths() {
    let app = serve("hello", user(ALICE, None), Arc::default()).await;
    assert_eq!(call(&app, "GET", "/health", "").await.status, 204);
    assert_eq!(call(&app, "GET", "/nope", "").await.status, 404);
    assert_eq!(call(&app, "POST", "/", "").await.status, 404);
}

// ---- notes -----------------------------------------------------------------

/// The ids of the delete forms on a notes page, in page order.
fn note_ids(page: &str) -> Vec<String> {
    page.split("action=\"/notes/")
        .skip(1)
        .filter_map(|rest| rest.split_once("/delete\"").map(|(id, _)| id.to_owned()))
        .collect()
}

/// Invariant: SDK-1.
#[tokio::test]
async fn notes_lists_an_empty_page_with_a_form_that_posts() {
    let app = serve("notes", user(ALICE, None), Arc::default()).await;
    let reply = call(&app, "GET", "/", "").await;
    assert_eq!(reply.status, 200);
    reply.assert_html();
    assert!(
        reply
            .body
            .contains("<form method=\"post\" action=\"/notes\">"),
        "{}",
        reply.body
    );
    assert!(note_ids(&reply.body).is_empty());
}

#[tokio::test]
async fn notes_creates_lists_and_deletes() {
    let app = serve("notes", user(ALICE, None), Arc::default()).await;

    let created = call(&app, "POST", "/notes", "text=buy+milk").await;
    assert_eq!(created.status, 303);
    assert_eq!(created.header("location"), "/");

    let page = call(&app, "GET", "/", "").await;
    assert!(page.body.contains("buy milk"), "{}", page.body);
    let ids = note_ids(&page.body);
    assert_eq!(ids.len(), 1, "{}", page.body);

    let deleted = call(&app, "POST", &format!("/notes/{}/delete", ids[0]), "").await;
    assert_eq!(deleted.status, 303);
    assert_eq!(deleted.header("location"), "/");
    let page = call(&app, "GET", "/", "").await;
    assert!(!page.body.contains("buy milk"), "{}", page.body);
}

/// Invariant: SDK-2.
#[tokio::test]
async fn notes_escapes_a_stored_note_that_contains_markup() {
    let app = serve("notes", user(ALICE, None), Arc::default()).await;
    let form = format!("text={}", form_value(XSS));
    assert_eq!(call(&app, "POST", "/notes", &form).await.status, 303);

    let page = call(&app, "GET", "/", "").await;
    page.assert_html();
    assert!(page.body.contains(XSS_ESCAPED), "{}", page.body);
    assert!(!page.body.contains("<script>"), "{}", page.body);
}

/// The template's reason to exist: one person's notes are invisible to another.
///
/// Invariant: SDK-3.
#[tokio::test]
async fn notes_are_private_to_the_caller_who_wrote_them() {
    let store = Arc::new(Counting::default());
    let alice = serve("notes", user(ALICE, None), Arc::clone(&store)).await;
    let bob = serve("notes", user(BOB, None), Arc::clone(&store)).await;

    assert_eq!(
        call(&alice, "POST", "/notes", "text=alice+secret")
            .await
            .status,
        303
    );
    let alice_page = call(&alice, "GET", "/", "").await;
    assert!(alice_page.body.contains("alice secret"));

    let bob_page = call(&bob, "GET", "/", "").await;
    assert!(!bob_page.body.contains("alice secret"), "{}", bob_page.body);

    // Bob cannot delete Alice's note by its id: his delete runs in his own
    // namespace, where it does not exist.
    let id = &note_ids(&alice_page.body)[0];
    call(&bob, "POST", &format!("/notes/{id}/delete"), "").await;
    assert!(
        call(&alice, "GET", "/", "")
            .await
            .body
            .contains("alice secret")
    );
}

#[tokio::test]
async fn notes_refuses_bad_input() {
    let app = serve("notes", user(ALICE, None), Arc::default()).await;
    assert_eq!(call(&app, "POST", "/notes", "").await.status, 400);
    assert_eq!(call(&app, "POST", "/notes", "text=+++").await.status, 400);
    let long = format!("text={}", "a".repeat(2001));
    assert_eq!(call(&app, "POST", "/notes", &long).await.status, 400);
    assert_eq!(
        call(&app, "POST", "/notes/not-an-id/delete", "")
            .await
            .status,
        404
    );
    assert_eq!(call(&app, "PUT", "/notes", "text=x").await.status, 404);
}

#[tokio::test]
async fn notes_refuses_an_anonymous_caller() {
    let app = serve("notes", IdentityCtx::new(Caller::Anonymous), Arc::default()).await;
    assert_eq!(call(&app, "GET", "/", "").await.status, 401);
    assert_eq!(call(&app, "POST", "/notes", "text=x").await.status, 401);
}

// ---- GET safety, every template ---------------------------------------------

/// No `GET` changes stored data — not a declared `GET` route, and not a `GET`
/// sent to a route declared for `POST`. A browser follows a `GET` from any
/// other site with the app's cookies, so a `GET` that writes is a link that
/// writes.
#[tokio::test]
async fn no_get_changes_stored_data() {
    for template in ["hello", "notes"] {
        let store = Arc::new(Counting::default());
        let app = serve(template, user(ALICE, None), Arc::clone(&store)).await;
        if template == "notes" {
            // Something to delete, if a GET were going to.
            assert_eq!(call(&app, "POST", "/notes", "text=keep").await.status, 303);
        }
        let before = store.writes.load(Ordering::SeqCst);

        let gets = declared(template, "GET");
        assert!(
            !gets.is_empty(),
            "{template}'s README declares no GET route"
        );
        for path in gets.iter().chain(&declared(template, "POST")) {
            call(&app, "GET", path, "").await;
        }
        assert_eq!(
            store.writes.load(Ordering::SeqCst),
            before,
            "a GET to {template} wrote data"
        );
    }
}
