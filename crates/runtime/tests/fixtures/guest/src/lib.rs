//! One component, several behaviours, selected by request path. Keeping them in
//! one guest means one committed fixture instead of five.
//!
//! Rebuild with `just rebuild-guest`.

wit_bindgen::generate!({ path: "wit", world: "app", generate_all });

use exports::wasi::http::incoming_handler::Guest;
use wasi::http::types::{Fields, IncomingRequest, OutgoingBody, OutgoingResponse, ResponseOutparam};

/// Mutated by `/counter`. Whether this survives between requests is exactly
/// what the instance-lifetime test asserts.
static mut COUNTER: u32 = 0;

struct Component;

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

            // A trap, so the host's trap mapping has something to map.
            "/trap" => panic!("guest panicked on purpose"),

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
