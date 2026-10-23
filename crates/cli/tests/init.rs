//! `clwd init`, and the guard that it still produces the template CI builds.
//!
//! # Why the round trip
//!
//! Templates are real projects that CI compiles, and `init` embeds a copy of
//! them in the binary. That is two paths to the same bytes, and #72 showed what
//! happens when nobody checks a second path: the guest fixture rotted because
//! the conformance suite ran against a committed artifact rather than the
//! source.
//!
//! Here the specific failure is an embedded file list that silently omits a
//! file. The template still builds in CI — the file is on disk — and `init`
//! produces a project missing it. Nothing else would notice.
//!
//! So the test undoes the substitutions and compares against the committed
//! template, file for file and byte for byte.

use std::path::{Path, PathBuf};
use std::process::Command;

use clwd_cli::commands::init;

/// The repository's `templates/` directory.
fn templates_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../templates")
        .canonicalize()
        .expect("templates/ is committed")
}

struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("clwd-init-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch");
        Self(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Every file `init` writes matches the committed template it came from, once
/// the substitutions are undone.
///
/// **This is the guard against an embedded list that omits a file.** A missing
/// entry shows up as a file present on disk and absent from the scaffold.
#[test]
fn init_output_matches_the_template_it_copied() {
    for template in init::list().0.iter().map(|t| t.name) {
        let scratch = Scratch::new(&format!("roundtrip-{template}"));
        // A name different from the template's, so a substitution that silently
        // does nothing cannot pass by coincidence.
        let name = "scaffolded-app";

        let made = init::execute(&scratch.0, name, Some(template), None)
            .unwrap_or_else(|e| panic!("init {template} failed: {e}"));

        let source = templates_dir().join(template);

        // Same set of files, in both directions.
        let mut on_disk: Vec<String> = walk(&source, &source);
        on_disk.sort();
        let mut scaffolded = made.files.clone();
        scaffolded.sort();
        assert_eq!(
            scaffolded, on_disk,
            "the embedded file list for {template} does not match templates/{template}/ — \
             a file was added or removed without updating TEMPLATES"
        );

        // Same bytes, once the substitutions are undone.
        for relative in &on_disk {
            let original = std::fs::read_to_string(source.join(relative))
                .unwrap_or_else(|e| panic!("reading templates/{template}/{relative}: {e}"));
            let written = std::fs::read_to_string(made.project.join(relative))
                .unwrap_or_else(|e| panic!("reading the scaffolded {relative}: {e}"));

            let restored = undo_substitutions(&written, template, name);
            assert_eq!(
                restored, original,
                "{template}/{relative} differs from the committed template by more than \
                 the substitutions"
            );
        }
    }
}

/// Reverse what `init` rewrote, so the result should equal the committed file.
fn undo_substitutions(written: &str, template: &str, name: &str) -> String {
    let mut out = written.to_owned();
    // The SDK path is rewritten to an absolute one; put the relative form back.
    // Matched by shape rather than by value, because the absolute path depends
    // on where this checkout is.
    let mut rebuilt = String::new();
    for line in out.lines() {
        if line.trim_start().starts_with("clwd-sdk = { path = ") {
            rebuilt.push_str("clwd-sdk = { path = \"../../crates/sdk\" }");
        } else {
            rebuilt.push_str(line);
        }
        rebuilt.push('\n');
    }
    out = rebuilt;
    out.replace(
        &format!("name = \"{name}\""),
        &format!("name = \"{template}\""),
    )
    .replace(
        &format!("host_label = \"{name}\""),
        &format!("host_label = \"{template}\""),
    )
}

/// Every file under `dir`, as paths relative to `root`, with `/` separators.
fn walk(root: &Path, dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        // Build output is not part of a template. `Cargo.lock` is in that
        // category here and is gitignored: it pins clwd-sdk through the
        // relative path the repository uses, and `init` rewrites that path, so
        // a shipped lockfile would describe a resolution the scaffolded project
        // does not have.
        //
        // Named individually rather than by a pattern, so deciding that
        // templates SHOULD ship a lockfile is one edit here and one in
        // TEMPLATES, not a silent change in what this test covers.
        if path
            .file_name()
            .is_some_and(|n| n == "target" || n == "Cargo.lock")
        {
            continue;
        }
        if path.is_dir() {
            out.extend(walk(root, &path));
        } else if let Ok(relative) = path.strip_prefix(root) {
            out.push(relative.to_string_lossy().replace('\\', "/"));
        }
    }
    out
}

/// The name lands in both manifests, which is the whole substitution.
#[test]
fn the_project_name_reaches_both_manifests() {
    let scratch = Scratch::new("names");
    let made = init::execute(&scratch.0, "my-app", Some("hello"), None).expect("init");

    let cargo = std::fs::read_to_string(made.project.join("Cargo.toml")).expect("Cargo.toml");
    let clwd = std::fs::read_to_string(made.project.join("clwd.toml")).expect("clwd.toml");

    assert!(cargo.contains("name = \"my-app\""), "{cargo}");
    assert!(clwd.contains("name = \"my-app\""), "{clwd}");
    assert!(clwd.contains("host_label = \"my-app\""), "{clwd}");
    assert!(
        !cargo.contains("name = \"hello\""),
        "the template's own name survived: {cargo}"
    );
}

/// A name that is not a DNS label is not made into a host label: the line is
/// left commented, next to the sentence saying a label is permanent, and
/// `clwd deploy` refuses until the author picks one.
#[test]
fn a_name_that_is_not_a_label_leaves_the_host_label_for_the_author() {
    let scratch = Scratch::new("nolabel");
    let made = init::execute(&scratch.0, "my_app", Some("hello"), None).expect("init");
    let clwd = std::fs::read_to_string(made.project.join("clwd.toml")).expect("clwd.toml");

    assert!(clwd.contains("# host_label = \"your-app\""), "{clwd}");
    assert!(clwd.contains("Permanent once deployed"), "{clwd}");
    let source = clwd_cli::build::Source::parse(&clwd).expect("still valid");
    assert_eq!(source.host_label(), None, "{clwd}");
}

/// `init` refuses to write into a directory with anything in it, rather than
/// merging into it or overwriting a file someone cares about.
#[test]
fn init_refuses_a_non_empty_directory() {
    let scratch = Scratch::new("existing");
    std::fs::create_dir_all(scratch.0.join("taken")).expect("mkdir");
    std::fs::write(scratch.0.join("taken/notes.txt"), "mine").expect("write");

    let err = init::execute(&scratch.0, "taken", None, None).expect_err("must refuse");
    assert_eq!(err.code(), "cli.invalid_argument");
    assert!(err.to_string().contains("not an empty directory"), "{err}");
    assert_eq!(
        std::fs::read_to_string(scratch.0.join("taken/notes.txt")).expect("still there"),
        "mine"
    );
}

/// An empty directory is fine: making one first is an ordinary thing to do.
#[test]
fn init_accepts_an_empty_directory() {
    let scratch = Scratch::new("empty");
    std::fs::create_dir_all(scratch.0.join("fresh")).expect("mkdir");
    let made = init::execute(&scratch.0, "fresh", None, None).expect("an empty dir is accepted");
    assert!(made.project.join("clwd.toml").is_file());
}

/// The name rule is the intersection of two grammars, and the message says so —
/// otherwise someone widens it to match app names and breaks crate generation.
#[test]
fn an_invalid_name_is_refused_with_the_rule() {
    let scratch = Scratch::new("badname");
    let err = init::execute(&scratch.0, "my.app", None, None).expect_err("a dot must be refused");
    let msg = err.to_string();
    assert!(msg.contains("cargo"), "why: {msg}");
    assert!(msg.contains("a-z"), "the rule: {msg}");
}

/// Run the real binary with stdin closed. `init` never prompts, so it must
/// finish — and with `--json`, print exactly one JSON value and nothing else.
fn clwd(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_clwd"))
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .expect("clwd runs")
}

#[test]
fn init_list_needs_no_name() {
    let out = clwd(&["init", "--list"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("hello") && text.contains("notes"), "{text}");
}

#[test]
fn init_list_json_is_an_array_of_name_teaches_works_deployed() {
    let out = clwd(&["--json", "init", "--list"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let list: serde_json::Value = serde_json::from_slice(&out.stdout).expect("JSON");
    let list = list.as_array().expect("an array");
    let names: Vec<_> = list
        .iter()
        .map(|t| t["name"].as_str().unwrap_or(""))
        .collect();
    assert_eq!(names, ["hello", "notes"]);
    for t in list {
        assert!(t["teaches"].is_string(), "{t}");
        assert!(t["works_deployed"].is_boolean(), "{t}");
    }
    assert_eq!(
        list[0]["works_deployed"], true,
        "hello works deployed today"
    );
}

/// The agent path: non-interactive, one JSON object, and the next commands
/// spelled out.
///
/// Invariant: CLI-10.
#[test]
fn init_with_stdin_closed_scaffolds_and_prints_the_next_commands() {
    let scratch = Scratch::new("stdin");
    let into = scratch.0.to_string_lossy().into_owned();
    let out = clwd(&[
        "--json",
        "init",
        "agent-app",
        "--template",
        "hello",
        "--into",
        &into,
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let made: serde_json::Value = serde_json::from_slice(&out.stdout).expect("one JSON object");
    let next: Vec<_> = made["next"]
        .as_array()
        .expect("next")
        .iter()
        .map(|c| c.as_str().unwrap_or(""))
        .collect();
    assert_eq!(next.len(), 3, "{made}");
    assert!(
        next[0].starts_with("cd ") && next[0].ends_with("agent-app"),
        "{made}"
    );
    assert_eq!(&next[1..], ["clwd dev", "clwd deploy"]);
    assert_eq!(made["works_deployed"], true);
}

/// The end-to-end promise: what `init` writes, `clwd dev` builds and serves.
///
/// The real binary, on a real scaffold, as an author would run it: scaffold,
/// then `clwd dev`, then a request to it.
///
/// Invariant: SDK-1.
#[test]
fn init_then_dev_serves_the_scaffold() {
    let scratch = Scratch::new("e2e");
    let into = scratch.0.to_string_lossy().into_owned();
    let out = clwd(&["init", "e2e-app", "--template", "hello", "--into", &into]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let project = scratch.0.join("e2e-app");

    // A free port: bind to 0, read it back, release it for the server.
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .expect("a free port")
        .port();
    let mut dev = Command::new(env!("CARGO_BIN_EXE_clwd"))
        .arg("dev")
        .arg(&project)
        .args(["--port", &port.to_string()])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("clwd dev starts");
    let response = get_when_up(port, std::time::Duration::from_secs(600));
    let _ = dev.kill();
    let _ = dev.wait();

    let response = response.expect("clwd dev answered");
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    let lower = response.to_ascii_lowercase();
    assert!(
        lower.contains("content-type: text/html; charset=utf-8"),
        "{response}"
    );
    assert!(
        lower.contains("x-content-type-options: nosniff"),
        "{response}"
    );
    assert!(response.contains("hello, local@localhost"), "{response}");
}

/// `GET /` from the dev server, retried until it is listening or `limit`
/// passes. The first request waits on a full wasm build.
fn get_when_up(port: u16, limit: std::time::Duration) -> Option<String> {
    use std::io::{Read as _, Write as _};
    let start = std::time::Instant::now();
    while start.elapsed() < limit {
        if let Ok(mut stream) = std::net::TcpStream::connect(("127.0.0.1", port)) {
            let request = "GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
            if stream.write_all(request.as_bytes()).is_ok() {
                let mut response = String::new();
                if stream.read_to_string(&mut response).is_ok() && !response.is_empty() {
                    return Some(response);
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    None
}
