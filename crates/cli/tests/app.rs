//! `clwd app create`, through the real binary: where the org comes from, and
//! what a caller — often an agent — reads back.

use std::io::{Read as _, Write as _};
use std::net::TcpListener;
use std::process::{Command, Output};

const TOKEN: &str = "clwd_tok_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const ORG: &str = "018f5b2a-2222-7000-8000-000000000002";
const APP: &str = "018f5b2a-0000-7000-8000-000000000000";

/// Run `clwd` with no org, token or platform inherited from the developer's
/// shell, plus `env`.
fn run(args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_clwd"));
    command
        .args(args)
        .env_remove("CLWD_ORG")
        .env_remove("CLWD_TOKEN")
        .env_remove("CLWD_PLATFORM_URL");
    for (k, v) in env {
        command.env(k, v);
    }
    command
        .output()
        .expect("the clwd binary is built as a test dependency")
}

fn envelope(out: &Output) -> serde_json::Value {
    let stderr = String::from_utf8(out.stderr.clone()).expect("utf-8 stderr");
    serde_json::from_str(stderr.trim()).unwrap_or_else(|e| panic!("{e}: {stderr}"))
}

#[test]
fn with_no_org_the_error_names_both_ways_to_give_one() {
    let out = run(&["--json", "app", "create", "notes"], &[]);
    assert_eq!(out.status.code(), Some(1));
    let json = envelope(&out);
    assert_eq!(json["code"], "cli.config");
    let message = json["message"].as_str().unwrap_or("");
    assert!(
        message.contains("--org") && message.contains("CLWD_ORG"),
        "{message}"
    );
}

#[test]
fn an_org_that_is_not_a_uuid_is_refused_before_any_request() {
    let out = run(
        &["--json", "app", "create", "notes", "--org", "acme"],
        &[
            ("CLWD_TOKEN", TOKEN),
            ("CLWD_PLATFORM_URL", "http://127.0.0.1:9"),
        ],
    );
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(envelope(&out)["code"], "cli.invalid_argument");
}

/// A one-shot server answering `status` with `body`.
fn serve_once(status: &'static str, body: &'static str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let mut buf = [0u8; 8192];
        let _ = stream.read(&mut buf);
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
             Connection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).expect("write");
    });
    format!("http://{addr}")
}

const CREATED: &str = r#"{"id":"018f5b2a-0000-7000-8000-000000000000","org_id":"018f5b2a-2222-7000-8000-000000000002","host_label":"notes","name":"notes","url":"https://notes.acme.apps.example","created":true}"#;

#[test]
fn json_output_is_one_object_with_the_id_and_url() {
    let base = serve_once("201 Created", CREATED);
    let out = run(
        &["--json", "app", "create", "notes", "--platform", &base],
        &[("CLWD_TOKEN", TOKEN), ("CLWD_ORG", ORG)],
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout).expect("utf-8");
    assert_eq!(stdout.lines().count(), 1, "one object per line: {stdout}");
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("JSON");
    assert_eq!(json["id"], APP);
    assert_eq!(json["url"], "https://notes.acme.apps.example");
    assert_eq!(json["created"], true);
}

#[test]
fn human_output_ends_with_the_next_command() {
    let base = serve_once("201 Created", CREATED);
    let out = run(
        &["app", "create", "notes", "--org", ORG, "--platform", &base],
        &[("CLWD_TOKEN", TOKEN)],
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout).expect("utf-8");
    assert_eq!(
        stdout,
        format!(
            "created app notes ({APP})\nurl:  https://notes.acme.apps.example\n\
             next: clwd publish <bundle> --app {APP}\n"
        )
    );
}
