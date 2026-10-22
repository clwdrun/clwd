//! `clwd deploy`, through the real binary against a stub server: the requests
//! it makes, in order, and what a caller — often an agent — reads back.
//!
//! The deploying tests share one scaffolded project, built once: the first
//! deploy pays for the compile and the rest find the bundle unchanged. They
//! take a lock to do it, so two builds never write the same bundle at once.

use std::io::{Read as _, Write as _};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

const TOKEN: &str = "clwd_tok_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const ORG: &str = "018f5b2a-2222-7000-8000-000000000002";
const APP: &str = "018f5b2a-0000-7000-8000-000000000000";
const VERSION: &str = "018f5b2a-1111-7000-8000-000000000001";
const LABEL: &str = "deploy-app";
const URL: &str = "https://deploy-app.acme.apps.example";

/// The shared project, scaffolded from the `hello` template on first use.
fn project() -> (MutexGuard<'static, ()>, &'static Path) {
    static LOCK: Mutex<()> = Mutex::new(());
    static PROJECT: OnceLock<PathBuf> = OnceLock::new();
    let guard = LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let project = PROJECT.get_or_init(|| {
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("deploy");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch");
        clwd_cli::commands::init::execute(&dir, LABEL, Some("hello"), None)
            .expect("init")
            .project
    });
    (guard, project)
}

/// A directory holding only `clwd.toml` with `manifest`. No `Cargo.toml`, so a
/// build that started would fail with `cli.not_found` rather than the code a
/// label check returns.
fn bare_project(tag: &str, manifest: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("deploy-{tag}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch");
    std::fs::write(dir.join("clwd.toml"), manifest).expect("write clwd.toml");
    dir
}

/// Run `clwd deploy` on `project` against `platform`, with nothing inherited
/// from the developer's shell.
fn deploy(project: &Path, platform: &str, json: bool) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_clwd"));
    if json {
        command.arg("--json");
    }
    command
        .arg("deploy")
        .arg(project)
        .env("CLWD_ORG", ORG)
        .env("CLWD_TOKEN", TOKEN)
        .env("CLWD_PLATFORM_URL", platform)
        .output()
        .expect("the clwd binary is built as a test dependency")
}

/// A server answering one connection per scripted response, in order, and
/// recording each request line with its body.
struct Stub {
    url: String,
    seen: Arc<Mutex<Vec<(String, String)>>>,
    thread: std::thread::JoinHandle<()>,
}

impl Stub {
    fn new(script: Vec<(&'static str, String)>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let url = format!("http://{}", listener.local_addr().expect("addr"));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&seen);
        let thread = std::thread::spawn(move || {
            for (status, body) in script {
                let (mut stream, _) = listener.accept().expect("accept");
                let request = read_request(&mut stream);
                log.lock().expect("log").push(request);
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).expect("write");
            }
        });
        Self { url, seen, thread }
    }

    /// Every request, once the script has been served.
    fn requests(self) -> Vec<(String, String)> {
        self.thread
            .join()
            .expect("the stub served its whole script");
        Arc::try_unwrap(self.seen)
            .expect("the stub thread is done")
            .into_inner()
            .expect("log")
    }
}

/// Read one request: the request line, and the body as lossy text.
fn read_request(stream: &mut std::net::TcpStream) -> (String, String) {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    let head_end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
        let n = stream.read(&mut chunk).expect("read");
        assert!(n > 0, "the client hung up mid-request");
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let length = head
        .lines()
        .find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.eq_ignore_ascii_case("content-length")
                .then(|| v.trim().parse::<usize>().ok())?
        })
        .unwrap_or(0);
    while buf.len() < head_end + length {
        let n = stream.read(&mut chunk).expect("read body");
        assert!(n > 0, "the client hung up mid-body");
        buf.extend_from_slice(&chunk[..n]);
    }
    let line = head.lines().next().unwrap_or_default();
    let line = line.rsplit_once(' ').map_or(line, |(l, _)| l).to_owned();
    (line, String::from_utf8_lossy(&buf[head_end..]).into_owned())
}

/// Assert that nothing connected to `listener`. A connection waits in the
/// backlog whether or not it was accepted, so an empty backlog is proof.
fn assert_no_request(listener: &TcpListener) {
    listener.set_nonblocking(true).expect("nonblocking");
    match listener.accept() {
        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
        other => panic!("the stub received a request: {other:?}"),
    }
}

fn app(created: bool) -> (&'static str, String) {
    let status = if created { "201 Created" } else { "200 OK" };
    let body = format!(
        r#"{{"id":"{APP}","org_id":"{ORG}","host_label":"{LABEL}","name":"{LABEL}","url":"{URL}","created":{created}}}"#
    );
    (status, body)
}

fn version(created: bool, certificate: &str) -> (&'static str, String) {
    let status = if created { "201 Created" } else { "200 OK" };
    let message = if certificate == "issued" {
        "issued; apps are reachable"
    } else {
        "being issued; apps are not reachable over TLS for a few minutes"
    };
    let body = format!(
        r#"{{"id":"{VERSION}","app_id":"{APP}","digest":"blake3:{hex}","state":"ready","created_at":"2026-10-22T00:00:00Z","certificate":{{"status":"{certificate}","message":"{message}"}}}}"#,
        hex = "a".repeat(64)
    );
    (status, body)
}

fn stdout_json(out: &Output) -> serde_json::Value {
    assert!(
        out.status.success(),
        "deploy failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!("{e}: {}", String::from_utf8_lossy(&out.stdout));
    })
}

fn stderr_json(out: &Output) -> serde_json::Value {
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    // The envelope is the last line: a build passes cargo's own output through
    // to stderr ahead of it.
    let stderr = String::from_utf8_lossy(&out.stderr);
    let last = stderr.trim().lines().last().unwrap_or_default();
    serde_json::from_str(last).unwrap_or_else(|e| panic!("{e}: {stderr}"))
}

/// The requests a deploy should make, in order.
fn expected_paths() -> Vec<String> {
    vec![
        format!("POST /v1/orgs/{ORG}/apps"),
        format!("POST /v1/apps/{APP}/versions"),
    ]
}

fn paths(requests: &[(String, String)]) -> Vec<String> {
    requests.iter().map(|(line, _)| line.clone()).collect()
}

#[test]
fn a_first_deploy_creates_the_app_publishes_and_prints_one_object() {
    let (_lock, project) = project();
    let stub = Stub::new(vec![app(true), version(true, "issued")]);

    let json = stdout_json(&deploy(project, &stub.url, true));
    assert_eq!(json["app_id"], APP);
    assert_eq!(json["host_label"], LABEL);
    assert_eq!(json["url"], URL);
    assert_eq!(json["created"], true);
    assert_eq!(json["version"]["id"], VERSION);
    assert_eq!(json["version"]["created"], true);
    assert!(
        json["digest"]
            .as_str()
            .is_some_and(|d| d.starts_with("blake3:"))
    );
    assert!(
        json.get("certificate").is_none(),
        "issued is not news: {json}"
    );

    let requests = stub.requests();
    assert_eq!(paths(&requests), expected_paths());
    let create: serde_json::Value = serde_json::from_str(&requests[0].1).expect("JSON body");
    assert_eq!(create, serde_json::json!({ "host_label": LABEL }));
    assert!(!requests[1].1.is_empty(), "the bundle was not uploaded");
}

/// Nothing changed: the same app, the same version, and the output says
/// "already existed" — never "live", which republishing does not guarantee.
#[test]
fn a_redeploy_with_no_changes_reuses_both_and_says_so() {
    let (_lock, project) = project();
    let stub = Stub::new(vec![app(false), version(false, "issued")]);

    let out = deploy(project, &stub.url, false);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains(&format!("app {LABEL} already exists ({APP})")),
        "{text}"
    );
    assert!(
        text.contains(&format!("version {VERSION} already existed")),
        "{text}"
    );
    assert!(text.contains(URL), "{text}");
    assert!(!text.contains("live"), "{text}");
    assert_eq!(paths(&stub.requests()), expected_paths());
}

#[test]
fn an_existing_app_gets_a_new_version() {
    let (_lock, project) = project();
    let stub = Stub::new(vec![app(false), version(true, "issued")]);

    let out = deploy(project, &stub.url, true);
    let json = stdout_json(&out);
    assert_eq!(json["created"], false);
    assert_eq!(json["version"]["created"], true);
    assert_eq!(paths(&stub.requests()), expected_paths());

    let stub = Stub::new(vec![app(false), version(true, "issued")]);
    let out = deploy(project, &stub.url, false);
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("may take up to ~30s"),
        "a new version says it may not serve yet"
    );
    stub.requests();
}

/// The app is created, the publish fails: the platform's code survives, the
/// message says the app is in place, and running again just continues.
#[test]
fn a_publish_failure_is_finished_by_running_again() {
    let (_lock, project) = project();
    let limited = r#"{"code":"control-plane.rate_limited","message":"slow down","hint":"retry after 30s","retryable":true}"#;
    let stub = Stub::new(vec![
        app(true),
        ("429 Too Many Requests", limited.to_owned()),
    ]);

    let err = stderr_json(&deploy(project, &stub.url, true));
    assert_eq!(err["code"], "control-plane.rate_limited");
    assert_eq!(err["retryable"], true);
    assert_eq!(err["hint"], "retry after 30s");
    let message = err["message"].as_str().unwrap_or_default();
    assert!(
        message.contains(APP) && message.contains("run clwd deploy again"),
        "{message}"
    );
    assert_eq!(paths(&stub.requests()), expected_paths());

    let stub = Stub::new(vec![app(false), version(true, "issued")]);
    let json = stdout_json(&deploy(project, &stub.url, true));
    assert_eq!(json["app_id"], APP);
    assert_eq!(json["created"], false, "the second run reuses the app");
    assert_eq!(json["version"]["created"], true);
    assert_eq!(paths(&stub.requests()), expected_paths());
}

/// An org's first publish starts its certificate. The deploy succeeded, but the
/// URL will not load yet, and both renderings say why.
#[test]
fn a_pending_certificate_is_reported_and_the_deploy_still_succeeds() {
    let (_lock, project) = project();

    let stub = Stub::new(vec![app(true), version(true, "pending")]);
    let json = stdout_json(&deploy(project, &stub.url, true));
    assert_eq!(json["certificate"]["status"], "pending");
    assert!(
        json["certificate"]["message"]
            .as_str()
            .is_some_and(|m| m.contains("not reachable over TLS")),
        "{json}"
    );
    stub.requests();

    let stub = Stub::new(vec![app(true), version(true, "pending")]);
    let out = deploy(project, &stub.url, false);
    assert_eq!(out.status.code(), Some(0));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("certificate pending: being issued; apps are not reachable over TLS"),
        "{text}"
    );
    stub.requests();
}

#[test]
fn a_missing_host_label_fails_before_any_build_or_request() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let url = format!("http://{}", listener.local_addr().expect("addr"));
    let project = bare_project("nolabel", "[app]\nname = \"notes\"\n");

    let err = stderr_json(&deploy(&project, &url, true));
    assert_eq!(err["code"], "cli.manifest_invalid");
    let message = err["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("host_label = \"<label>\" under [app]")
            && message.contains("never be changed or reused"),
        "{message}"
    );
    assert_no_request(&listener);
}

#[test]
fn an_invalid_host_label_fails_before_any_build_or_request() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let url = format!("http://{}", listener.local_addr().expect("addr"));
    let project = bare_project("badlabel", "[app]\nhost_label = \"My_App\"\n");

    let err = stderr_json(&deploy(&project, &url, true));
    assert_eq!(err["code"], "cli.manifest_invalid");
    let message = err["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("\"My_App\"") && message.contains("DNS label"),
        "{message}"
    );
    assert_no_request(&listener);
}
