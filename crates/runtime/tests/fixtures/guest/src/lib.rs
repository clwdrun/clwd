//! One component, several behaviours, selected by request path. Keeping them in
//! one guest means one committed fixture instead of five.
//!
//! Rebuild with `just rebuild-guest`.

// The canonical WIT, not a copy. A copy would be 28 duplicated files that can
// drift from wit/ silently between rebuilds.
wit_bindgen::generate!({ path: "../../../../../wit", world: "app", generate_all });

use exports::wasi::http::incoming_handler::Guest;
use wasi::http::types::{Fields, IncomingRequest, OutgoingBody, OutgoingResponse, ResponseOutparam};

/// Mutated by `/counter`. Whether this survives between requests is exactly
/// what the instance-lifetime test asserts.
static mut COUNTER: u32 = 0;

struct Component;

/// Render an app-data error so a test can assert on which one it was.
fn describe(e: clwd::app_data::store::Error) -> String {
    match e {
        clwd::app_data::store::Error::InvalidKey(why) => format!("invalid-key:{why}"),
        clwd::app_data::store::Error::ValueTooLarge(limit) => format!("value-too-large:{limit}"),
        clwd::app_data::store::Error::Unavailable => "unavailable".into(),
    }
}

fn caller_subject() -> String {
    match clwd::identity::context::get_caller() {
        clwd::identity::types::Caller::Anonymous => "anonymous".to_string(),
        clwd::identity::types::Caller::User(p) => p.subject,
        clwd::identity::types::Caller::Service(s) => s.subject,
    }
}

impl Guest for Component {
    fn handle(request: IncomingRequest, outparam: ResponseOutparam) {
        let path = request.path_with_query().unwrap_or_default();

        let payload = match path.as_str() {
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

            "/role" => match clwd::identity::context::get_role() {
                Some(clwd::identity::types::Role::Viewer) => "viewer".into(),
                Some(clwd::identity::types::Role::Editor) => "editor".into(),
                Some(clwd::identity::types::Role::Owner) => "owner".into(),
                None => "none".into(),
            },

            "/groups" => clwd::identity::context::list_groups().join(","),

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
            // yours"; both arrive as none.
            p if p.starts_with("/secret/") => {
                let name = &p["/secret/".len()..];
                match clwd::secrets::broker::get(name) {
                    Some(value) => String::from_utf8_lossy(&value).into_owned(),
                    None => "absent".into(),
                }
            }

            // /data/get/KEY
            p if p.starts_with("/data/get/") => {
                let key = &p["/data/get/".len()..];
                match clwd::app_data::store::get(key) {
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
                match clwd::app_data::store::set(key, value.as_bytes()) {
                    Ok(()) => "stored".into(),
                    Err(e) => format!("error:{}", describe(e)),
                }
            }

            // /data/delete/KEY
            p if p.starts_with("/data/delete/") => {
                let key = &p["/data/delete/".len()..];
                match clwd::app_data::store::delete(key) {
                    Ok(()) => "deleted".into(),
                    Err(e) => format!("error:{}", describe(e)),
                }
            }

            // /data/big/N — store N bytes, to reach the value ceiling.
            p if p.starts_with("/data/big/") => {
                let n: usize = p["/data/big/".len()..].parse().unwrap_or(0);
                match clwd::app_data::store::set("big", &vec![b'x'; n]) {
                    Ok(()) => "stored".into(),
                    Err(e) => format!("error:{}", describe(e)),
                }
            }

            _ => "ok".into(),
        };

        let response = OutgoingResponse::new(Fields::new());
        response.set_status_code(200).unwrap();
        let body = response.body().unwrap();
        ResponseOutparam::set(outparam, Ok(response));
        let stream = body.write().unwrap();
        stream.blocking_write_and_flush(payload.as_bytes()).unwrap();
        drop(stream);
        OutgoingBody::finish(body, None).unwrap();
    }
}

export!(Component);
