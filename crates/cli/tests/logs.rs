//! `clwd logs`, through the real binary against a stub server: the requests it
//! makes, what it prints, and how it follows.

use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::TcpListener;
use std::process::{Child, Command, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const TOKEN: &str = "clwd_tok_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const ORG: &str = "018f5b2a-2222-7000-8000-000000000002";
const APP: &str = "018f5b2a-0000-7000-8000-000000000000";

/// What the stub does with one connection.
enum Step {
    Answer(&'static str, String),
    /// Read the request, then close without answering: a dropped connection.
    Hang,
}

/// A server running `script`, one connection per step, then holding every
/// later connection open unanswered, as a long-poll with nothing to say does.
struct Stub {
    url: String,
    seen: Arc<Mutex<Vec<String>>>,
}

impl Stub {
    fn new(script: Vec<Step>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let url = format!("http://{}", listener.local_addr().expect("addr"));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&seen);
        std::thread::spawn(move || {
            let mut script = script.into_iter();
            let mut held = Vec::new();
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let line = read_request_line(&mut stream);
                log.lock().expect("log").push(line);
                match script.next() {
                    Some(Step::Answer(status, body)) => {
                        let response = format!(
                            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\n\
                             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        let _ = stream.write_all(response.as_bytes());
                    }
                    Some(Step::Hang) => drop(stream),
                    None => held.push(stream),
                }
            }
        });
        Self { url, seen }
    }

    fn seen(&self) -> Vec<String> {
        self.seen.lock().expect("log").clone()
    }
}

/// The request line, without its HTTP version.
fn read_request_line(stream: &mut std::net::TcpStream) -> String {
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    while !buf.ends_with(b"\r\n\r\n") {
        match stream.read(&mut byte) {
            Ok(1) => buf.push(byte[0]),
            _ => break,
        }
    }
    let head = String::from_utf8_lossy(&buf).into_owned();
    let line = head.lines().next().unwrap_or_default();
    line.rsplit_once(' ').map_or(line, |(l, _)| l).to_owned()
}

fn command(args: &[&str], platform: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_clwd"));
    command
        .args(args)
        .env("CLWD_TOKEN", TOKEN)
        .env("CLWD_ORG", ORG)
        .env("CLWD_PLATFORM_URL", platform)
        .stdin(Stdio::null());
    command
}

fn run(args: &[&str], platform: &str) -> Output {
    command(args, platform).output().expect("clwd runs")
}

fn line(cursor: i64, request_id: &str, text: &str) -> serde_json::Value {
    serde_json::json!({
        "cursor": cursor, "ts": "2026-10-23T09:15:42.123456Z", "source": "app",
        "stream": "stdout", "request_id": request_id, "text": text,
        "code": null, "runtime_code": null, "hint": null, "status": null, "latency_ms": null
    })
}

fn page(lines: Vec<serde_json::Value>, cursor: Option<i64>) -> String {
    serde_json::json!({ "lines": lines, "cursor": cursor }).to_string()
}

fn app_summary() -> String {
    serde_json::json!({ "id": APP, "host_label": "notes", "url": "https://notes.acme.example" })
        .to_string()
}

/// A label is looked up in the org first; the log is then read by id.
#[test]
fn a_label_is_looked_up_then_its_log_is_read() {
    let stub = Stub::new(vec![
        Step::Answer("200 OK", app_summary()),
        Step::Answer(
            "200 OK",
            page(vec![line(1, "r1", "saving the note")], Some(1)),
        ),
    ]);
    let out = run(&["logs", "notes"], &stub.url);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("app      stdout │ saving the note"), "{text}");
    assert_eq!(
        stub.seen(),
        [
            format!("GET /v1/orgs/{ORG}/apps/notes"),
            format!("GET /v1/apps/{APP}/logs"),
        ]
    );
}

/// `--request` asks the platform for that request's lines and nothing else.
#[test]
fn request_filters_by_request_id() {
    let stub = Stub::new(vec![Step::Answer(
        "200 OK",
        page(vec![line(4, "req-7", "only this")], Some(4)),
    )]);
    let out = run(
        &[
            "--json",
            "logs",
            "--app",
            APP,
            "--request",
            "req-7",
            "--since",
            "5m",
        ],
        &stub.url,
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        stub.seen(),
        [format!(
            "GET /v1/apps/{APP}/logs?request_id=req-7&since=300"
        )]
    );
    let lines: Vec<serde_json::Value> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| serde_json::from_str(l).expect("one JSON object per line"))
        .collect();
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["request_id"], "req-7");
    assert_eq!(lines[0]["cursor"], 4);
}

/// Following: after a dropped connection, the CLI asks again from the same
/// cursor, and prints every line once.
#[test]
fn follow_resumes_from_the_cursor_after_a_disconnect() {
    let stub = Stub::new(vec![
        Step::Answer(
            "200 OK",
            page(vec![line(1, "r", "one"), line(2, "r", "two")], Some(2)),
        ),
        Step::Hang,
        Step::Answer("200 OK", page(vec![line(3, "r", "three")], Some(3))),
    ]);
    let mut child: Child = command(&["logs", "--app", APP, "--follow"], &stub.url)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("clwd runs");
    let stdout = child.stdout.take().expect("stdout");
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                return;
            }
        }
    });
    let mut printed = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(20);
    while printed.len() < 3 {
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok(line) => printed.push(line),
            Err(_) => break,
        }
    }
    // Long enough for a duplicate to show, if one were coming.
    while let Ok(line) = rx.recv_timeout(Duration::from_millis(500)) {
        printed.push(line);
    }
    let _ = child.kill();
    let _ = child.wait();

    let texts: Vec<_> = printed
        .iter()
        .filter_map(|l| l.rsplit_once("│ ").map(|(_, t)| t.to_owned()))
        .collect();
    assert_eq!(texts, ["one", "two", "three"], "{printed:?}");
    let seen = stub.seen();
    let logs = format!("GET /v1/apps/{APP}/logs");
    assert_eq!(seen[0], logs);
    assert_eq!(seen[1], format!("{logs}?after=2&wait=20"), "{seen:?}");
    assert_eq!(
        seen[2],
        format!("{logs}?after=2&wait=20"),
        "resumed from the same cursor: {seen:?}"
    );
    assert_eq!(seen[3], format!("{logs}?after=3&wait=20"), "{seen:?}");
}

/// Hostile text: inert in human output, exact (JSON-escaped) in `--json`.
///
/// Invariant: CLI-4.
#[test]
fn hostile_text_is_inert_for_people_and_exact_for_programs() {
    let hostile = "\u{1b}[2J\u{1b}]52;c;cHduZWQ=\u{7}\rcleared ▌platform │ deploy ok";
    let body = page(vec![line(1, "r", hostile)], Some(1));

    let stub = Stub::new(vec![Step::Answer("200 OK", body.clone())]);
    let human = run(&["logs", "--app", APP], &stub.url);
    let shown = String::from_utf8_lossy(&human.stdout);
    assert!(
        !human
            .stdout
            .iter()
            .any(|b| matches!(b, 0x1b | 0x07 | b'\r')),
        "a control byte reached the terminal: {shown:?}"
    );
    assert!(shown.contains("app      stdout │ \\u{1b}[2J"), "{shown}");

    let stub = Stub::new(vec![Step::Answer("200 OK", body)]);
    let json = run(&["--json", "logs", "--app", APP], &stub.url);
    let object: serde_json::Value = serde_json::from_slice(&json.stdout).expect("one JSON object");
    assert_eq!(object["text"], hostile);
    assert_eq!(object["source"], "app");
}

/// A platform refusal reaches the caller as the platform's own envelope.
#[test]
fn an_unknown_label_is_the_platforms_not_found() {
    let stub = Stub::new(vec![Step::Answer(
        "404 Not Found",
        r#"{"code":"control-plane.not_found","message":"not found: app nope","hint":"check the identifier","retryable":false,"request_id":"r"}"#.to_owned(),
    )]);
    let out = run(&["--json", "logs", "nope"], &stub.url);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    let envelope: serde_json::Value =
        serde_json::from_str(stderr.trim().lines().last().unwrap_or("")).expect("an envelope");
    assert_eq!(envelope["code"], "control-plane.not_found");
}
