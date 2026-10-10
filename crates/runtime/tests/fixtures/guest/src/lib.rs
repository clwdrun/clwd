//! One component, several behaviours, selected by request path. Keeping them in
//! one guest means one committed fixture instead of five.
//!
//! Written against `clwd-sdk` rather than raw bindings, deliberately: this is
//! the closest thing in the repository to an app, so it is also the best
//! evidence of what the SDK feels like to use. If a change to the SDK makes a
//! working guest harder to write, it shows up here first.
//!
//! Rebuild with `just rebuild-guest`.

// The app-data and secrets paths stay on the raw bindings on purpose. This
// fixture's job for R5.x and R6.x is to assert the *interface's* contract —
// which error variants exist, that absence is `Ok(None)` and not an error — and
// wrapping those in the SDK's types would test the wrapper instead. Response
// writing and identity go through the SDK, because there the SDK is what an app
// would use. A fixture that used the ergonomic layer everywhere would prove
// less, not more.
use clwd_sdk::raw::{app_data, secrets, wasi};
use clwd_sdk::{Caller, Request, Response, caller};

/// Mutated by `/counter`. Whether this survives between requests is exactly
/// what the instance-lifetime test asserts.
static mut COUNTER: u32 = 0;

struct Component;

/// Render an app-data error so a test can assert on which one it was.
fn describe(e: app_data::store::Error) -> String {
    match e {
        app_data::store::Error::InvalidKey(why) => format!("invalid-key:{why}"),
        app_data::store::Error::ValueTooLarge(limit) => format!("value-too-large:{limit}"),
        app_data::store::Error::Unavailable => "unavailable".into(),
    }
}

/// The three kinds of caller, never collapsed into an `Option`. Anonymous is a
/// caller, not a missing one — see `clwd_sdk::caller`.
fn caller_subject() -> String {
    match caller() {
        Caller::Anonymous => "anonymous".to_string(),
        Caller::User(u) => u.subject,
        Caller::Service(s) => s.subject,
    }
}

impl clwd_sdk::exports::wasi::http::incoming_handler::Guest for Component {
    fn handle(
        request: clwd_sdk::raw::wasi::http::types::IncomingRequest,
        outparam: clwd_sdk::raw::wasi::http::types::ResponseOutparam,
    ) {
        let request = Request::new(request);

        let payload = match request.path() {
            // Who does this instance think is calling? The isolation tests
            // compare this against the identity the Store was built with.
            "/whoami" => caller_subject(),

            // Guest state across requests.
            "/counter" => {
                let n = unsafe {
                    COUNTER += 1;
                    COUNTER
                };
                n.to_string()
            }

            "/role" => match clwd_sdk::role() {
                Some(clwd_sdk::Role::Viewer) => "viewer".into(),
                Some(clwd_sdk::Role::Editor) => "editor".into(),
                Some(clwd_sdk::Role::Owner) => "owner".into(),
                None => "none".into(),
            },

            "/groups" => clwd_sdk::groups().join(","),

            "/log" => {
                println!("hello from stdout");
                eprintln!("hello from stderr");
                "logged".into()
            }

            // Write more than 1 MiB to stdout in one call, past the host's log
            // buffer, so the host's drop-marker line is produced. A few hundred
            // multi-KiB lines rather than thousands of tiny ones: see the
            // FIXTURE note — a short-line flood produces so many dropped lines
            // that a subscriber's own buffer loses the marker too.
            "/flood-logs" => {
                let line = "x".repeat(4096);
                for _ in 0..300 {
                    println!("{line}");
                }
                "flooded".into()
            }

            // /big-response/N — a response body of N bytes, to reach the
            // host's response cap.
            p if p.starts_with("/big-response/") => {
                let n: usize = p["/big-response/".len()..].parse().unwrap_or(0);
                "x".repeat(n)
            }

            // A trap, so the host's trap mapping has something to map.
            "/trap" => panic!("guest panicked on purpose"),

            // A clean exit from inside the guest. On the wasm backend the next
            // call rebuilds the instance, so this is just a normal return and
            // R2.2 is reached by the harness evicting the instance; a long-lived
            // backend tears the instance down here.
            "/exit" => "exited".into(),

            // Never returns and never yields. Only preemption stops it.
            "/spin" => {
                let mut n: u64 = 0;
                loop {
                    n = n.wrapping_add(1);
                    std::hint::black_box(n);
                }
            }

            // Block on a host timer for the given milliseconds without burning
            // CPU, then return. Separates a wall-clock deadline from a CPU
            // budget (R4.3).
            p if p.starts_with("/sleep/") => {
                let ms: u64 = p["/sleep/".len()..].parse().unwrap_or(0);
                let nanos = ms.saturating_mul(1_000_000);
                let pollable = wasi::clocks::monotonic_clock::subscribe_duration(nanos);
                pollable.block();
                "slept".into()
            }

            // Allocates until the host refuses, in 1 MiB steps. Held in a Vec
            // so the allocator cannot reuse the same pages.
            "/alloc" => {
                let mut held: Vec<Vec<u8>> = Vec::new();
                loop {
                    let mut chunk = vec![0u8; 1024 * 1024];
                    // Touch it: an untouched allocation may never grow memory.
                    chunk[0] = held.len() as u8;
                    std::hint::black_box(&chunk);
                    held.push(chunk);
                }
            }

            // /secret/NAME -> the value, or "absent" when this app was not
            // given it. The guest cannot tell "no such secret" from "not
            // yours"; both arrive as none, and the SDK adds no way to tell.
            p if p.starts_with("/secret/") => {
                let name = &p["/secret/".len()..];
                match secrets::broker::get(name) {
                    Some(value) => String::from_utf8_lossy(&value).into_owned(),
                    None => "absent".into(),
                }
            }

            // /data/get/KEY
            p if p.starts_with("/data/get/") => {
                let key = &p["/data/get/".len()..];
                match app_data::store::get(key) {
                    Ok(Some(v)) => String::from_utf8_lossy(&v).into_owned(),
                    Ok(None) => "absent".into(),
                    Err(e) => format!("error:{}", describe(e)),
                }
            }

            // /data/set/KEY/VALUE — an empty VALUE is legal and distinct from
            // absence.
            p if p.starts_with("/data/set/") => {
                let rest = &p["/data/set/".len()..];
                let (key, value) = match rest.split_once('/') {
                    Some((k, v)) => (k, v),
                    None => (rest, ""),
                };
                match app_data::store::set(key, value.as_bytes()) {
                    Ok(()) => "stored".into(),
                    Err(e) => format!("error:{}", describe(e)),
                }
            }

            // /data/delete/KEY
            p if p.starts_with("/data/delete/") => {
                let key = &p["/data/delete/".len()..];
                match app_data::store::delete(key) {
                    Ok(()) => "deleted".into(),
                    Err(e) => format!("error:{}", describe(e)),
                }
            }

            // /data/big/N — store N bytes, to reach the value ceiling.
            p if p.starts_with("/data/big/") => {
                let n: usize = p["/data/big/".len()..].parse().unwrap_or(0);
                match app_data::store::set("big", &vec![b'x'; n]) {
                    Ok(()) => "stored".into(),
                    Err(e) => format!("error:{}", describe(e)),
                }
            }

            _ => "ok".into(),
        };

        // The whole eight-step response sequence, including the two ordering
        // constraints that are not expressible in the raw types.
        Response::ok().send(outparam, payload.as_bytes());
    }
}

// `with_types_in` is required when the bindings live in another crate: without
// it the generated macro resolves its types against `self`, which is the SDK
// from inside the SDK and this crate from out here.
clwd_sdk::export!(Component with_types_in clwd_sdk);
