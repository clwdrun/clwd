//! `clwd run` end to end, through the real binary.
//!
//! These drive the built `clwd` rather than the library, because most of what
//! this issue promises is only observable from outside: the exit status, which
//! stream each byte lands on, and — the point of the delegation — that a
//! `bundle.*` code survives all the way into the JSON envelope with its spelling
//! intact.
//!
//! The bundles are built here with `clwd_bundle::build` rather than committed,
//! so a change to the format cannot leave a stale fixture passing.

#![cfg(feature = "run")]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use clwd_bundle::{Capabilities, Limits, Runtime, build};

/// The reference guest from the runtime's fixtures: a real component that
/// answers `/`, `/trap`, `/spin` and more. Reused rather than rebuilt — there is
/// one guest, and a second copy would drift.
const GUEST: &[u8] = include_bytes!("../../runtime/tests/fixtures/app.wasm");

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_clwd"))
        .args(args)
        .output()
        .expect("the clwd binary is built as a test dependency")
}

fn stdout(o: &Output) -> String {
    String::from_utf8(o.stdout.clone()).expect("utf-8 stdout")
}

fn stderr(o: &Output) -> String {
    String::from_utf8(o.stderr.clone()).expect("utf-8 stderr")
}

fn code(o: &Output) -> i32 {
    o.status
        .code()
        .expect("clwd exits rather than being signalled")
}

/// A bundle wrapping the reference guest, written into a temporary directory.
fn bundle_at(dir: &Path, name: &str, cpu_ms: u64, memory_mb: u64) -> PathBuf {
    let encoded = build(
        Runtime::Wasm,
        "app.wasm",
        Capabilities {
            version: 1,
            egress: Vec::new(),
            secrets: Vec::new(),
            storage_namespaces: Vec::new(),
            limits: Limits { cpu_ms, memory_mb },
        },
        vec![clwd_bundle::File {
            path: "app.wasm".to_owned(),
            data: GUEST.to_vec(),
            executable: false,
        }],
    )
    .expect("the reference guest makes a valid bundle");

    let path = dir.join(name);
    std::fs::write(&path, encoded.bytes).expect("writing the bundle");
    path
}

/// A scratch directory that cleans itself up.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "clwd-run-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock is after 1970")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("creating the scratch directory");
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn envelope(o: &Output) -> serde_json::Value {
    let err = stderr(o);
    serde_json::from_str(err.trim())
        .unwrap_or_else(|e| panic!("stderr is not an envelope ({e}): {err:?}"))
}

// ======================================================================
// The happy path
// ======================================================================

/// A valid bundle serves a request, and the body is the only thing on stdout.
#[test]
fn a_valid_bundle_serves_a_request() {
    let scratch = Scratch::new("ok");
    let bundle = bundle_at(scratch.path(), "app.clwd", 5_000, 128);

    let out = run(&["run", bundle.to_str().expect("utf-8 path"), "--path", "/"]);

    assert_eq!(code(&out), 0, "stderr: {}", stderr(&out));
    assert_eq!(
        stdout(&out).trim(),
        "ok",
        "the guest answers `/` with `ok`; stderr: {}",
        stderr(&out)
    );
}

/// `--json` carries the status, the body, and the identity the app saw.
#[test]
fn json_output_carries_the_response_and_the_identity() {
    let scratch = Scratch::new("json");
    let bundle = bundle_at(scratch.path(), "app.clwd", 5_000, 128);

    let out = run(&[
        "--json",
        "run",
        bundle.to_str().expect("utf-8 path"),
        "--path",
        "/whoami",
    ]);
    assert_eq!(code(&out), 0, "stderr: {}", stderr(&out));

    let json: serde_json::Value =
        serde_json::from_str(stdout(&out).trim()).expect("stdout is one JSON object");

    assert_eq!(json["status"], 200);
    assert_eq!(
        json["body"], "local",
        "the guest reports the subject it was given"
    );
    assert_eq!(json["caller"], "local");
    assert_eq!(json["org"], "local");
    assert!(
        json.as_object().expect("an object").contains_key("body"),
        "a text body appears under `body`"
    );
}

/// **The local identity is obviously local.** #29's templates will show app
/// authors reading `ctx().caller()`, and what they read on a laptop must not
/// look like a production subject.
#[test]
fn the_local_caller_is_not_mistakable_for_a_production_one() {
    let scratch = Scratch::new("identity");
    let bundle = bundle_at(scratch.path(), "app.clwd", 5_000, 128);

    let out = run(&[
        "--json",
        "run",
        bundle.to_str().expect("utf-8 path"),
        "--path",
        "/whoami",
    ]);
    let json: serde_json::Value = serde_json::from_str(stdout(&out).trim()).expect("json stdout");

    // Not anonymous: an author must not learn that the anonymous branch is the
    // normal case, when in production it is the share-link case.
    assert_ne!(json["body"], "anonymous");
    // A literal a control plane would never mint as an opaque subject id.
    assert_eq!(json["body"], "local");
}

// ======================================================================
// The delegation — the reason this issue touches the error type
// ======================================================================

/// **The test that proves the delegation.** A damaged bundle must surface
/// `bundle.member_digest_mismatch` through the CLI's envelope with the code
/// spelled exactly as `clwd-bundle` spells it.
///
/// Asserted as a string on purpose. The prefix names the subsystem that made the
/// decision, and re-spelling it as `cli.internal` would tell a caller only that
/// something, somewhere, went wrong.
#[test]
fn a_damaged_bundle_surfaces_its_bundle_code_verbatim() {
    let scratch = Scratch::new("damaged");
    let bundle = bundle_at(scratch.path(), "app.clwd", 5_000, 128);

    // Flip one content byte: still a well-formed archive, but a member no longer
    // matches the digest the manifest declares for it.
    let mut bytes = std::fs::read(&bundle).expect("reading the bundle");
    bytes[512] ^= 0xff;
    std::fs::write(&bundle, bytes).expect("writing the damaged bundle");

    let out = run(&["--json", "run", bundle.to_str().expect("utf-8 path")]);

    assert_eq!(
        code(&out),
        1,
        "a damaged bundle is a failure, not a usage error"
    );
    let json = envelope(&out);
    assert_eq!(
        json["code"], "bundle.member_digest_mismatch",
        "the bundle code must arrive verbatim, not re-spelled: {json}"
    );
    assert_eq!(json["retryable"], false);
    assert!(
        json["hint"].is_string(),
        "a delegated error still points somewhere useful: {json}"
    );
}

/// Not a bundle at all — a different `bundle.*` code, also verbatim. Two
/// distinct codes prove the path forwards rather than mapping everything onto
/// one.
#[test]
fn a_file_that_is_not_a_bundle_surfaces_its_own_code() {
    let scratch = Scratch::new("notbundle");
    let path = scratch.path().join("junk.clwd");
    std::fs::write(&path, b"this is emphatically not a tar archive".repeat(64))
        .expect("writing junk");

    let out = run(&["--json", "run", path.to_str().expect("utf-8 path")]);

    assert_eq!(code(&out), 1);
    let json = envelope(&out);
    let code_str = json["code"].as_str().expect("a code string");
    assert!(
        code_str.starts_with("bundle."),
        "a malformed bundle keeps its bundle.* namespace, got {code_str:?}"
    );
    assert_ne!(
        code_str, "bundle.member_digest_mismatch",
        "a different failure must produce a different code"
    );
}

/// A guest that never yields is stopped by its sealed CPU budget, and the
/// runtime code arrives verbatim.
#[test]
fn a_guest_over_its_cpu_budget_surfaces_runtime_deadline_exceeded() {
    let scratch = Scratch::new("spin");
    // 50ms is hundreds of times a normal request and still quick to hit.
    let bundle = bundle_at(scratch.path(), "app.clwd", 50, 128);

    let out = run(&[
        "--json",
        "run",
        bundle.to_str().expect("utf-8 path"),
        "--path",
        "/spin",
    ]);

    assert_eq!(code(&out), 1);
    let json = envelope(&out);
    assert_eq!(
        json["code"], "runtime.deadline_exceeded",
        "a preempted guest is a deadline, not a trap: {json}"
    );
}

/// A trapping guest surfaces `runtime.trap` with the failure detail intact.
///
/// Note what the message can and cannot contain. A Rust `panic!` inside a wasm
/// guest compiles to an `unreachable` instruction; the panic string goes to the
/// guest's stderr, which the log sink captures, and never reaches the trap.
/// wasmtime sees only the wasm-level cause. So the diagnostic worth asserting is
/// the cause and the backtrace.
///
/// An earlier version looked for the guest's own panic text — `"panicked on
/// purpose"` — and failed. Loosening an assertion is normally the thing to
/// refuse, so the distinction matters: **that assertion was testing an artefact
/// of how the guest happens to signal failure, not the guarantee the contract
/// makes.** The contract is `runtime.trap` with the failure detail intact, and
/// it held throughout — code correct, message intact, backtrace preserved. The
/// test was wrong about where a panic string lives.
///
/// The line to apply next time: if the delegated *code* ever arrives changed,
/// that is a contract violation and the code is wrong. If an assertion about
/// incidental *text* fails, check which of the two it was before touching
/// either.
#[test]
fn a_trapping_guest_surfaces_runtime_trap_with_its_message() {
    let scratch = Scratch::new("trap");
    let bundle = bundle_at(scratch.path(), "app.clwd", 5_000, 128);

    let out = run(&[
        "--json",
        "run",
        bundle.to_str().expect("utf-8 path"),
        "--path",
        "/trap",
    ]);

    assert_eq!(code(&out), 1);
    let json = envelope(&out);
    assert_eq!(json["code"], "runtime.trap");
    let message = json["message"].as_str().expect("a message");
    assert!(
        message.contains("wasm trap"),
        "the wasm-level cause must survive: {message:?}"
    );
    assert!(
        message.contains("backtrace"),
        "the backtrace is the one diagnostic an app author gets: {message:?}"
    );
}

// ======================================================================
// Phase 0 decisions, one test each
// ======================================================================

/// §4 — bundle only. A bare component is refused with a reason, because it has
/// no manifest and therefore no sealed limits.
#[test]
fn a_bare_wasm_is_refused_with_a_reason() {
    let scratch = Scratch::new("barewasm");
    let path = scratch.path().join("app.wasm");
    std::fs::write(&path, GUEST).expect("writing the component");

    let out = run(&["--json", "run", path.to_str().expect("utf-8 path")]);

    assert_eq!(code(&out), 1);
    let json = envelope(&out);
    assert_eq!(json["code"], "cli.invalid_argument");
    let message = json["message"].as_str().expect("a message");
    assert!(
        message.contains("bundle") && message.contains("limits"),
        "the refusal must say why, not just no: {message:?}"
    );
}

/// §2 — `--backend microvm` is `cli.not_implemented` naming the work, not a
/// clap rejection that reads as though the flag is broken.
#[test]
fn backend_microvm_is_not_implemented_rather_than_a_usage_error() {
    let scratch = Scratch::new("microvm");
    let bundle = bundle_at(scratch.path(), "app.clwd", 5_000, 128);

    let out = run(&[
        "--json",
        "run",
        bundle.to_str().expect("utf-8 path"),
        "--backend",
        "micro-vm",
    ]);

    assert_eq!(
        code(&out),
        1,
        "not 2 — the flag is understood, the backend is not built"
    );
    let json = envelope(&out);
    assert_eq!(json["code"], "cli.not_implemented");
}

/// §2 — `--backend wasm` is accepted and runs.
#[test]
fn backend_wasm_is_accepted() {
    let scratch = Scratch::new("wasmflag");
    let bundle = bundle_at(scratch.path(), "app.clwd", 5_000, 128);

    let out = run(&[
        "run",
        bundle.to_str().expect("utf-8 path"),
        "--backend",
        "wasm",
    ]);
    assert_eq!(code(&out), 0, "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "ok");
}

/// A missing file is `cli.not_found`, not an internal error.
#[test]
fn a_missing_bundle_is_not_found() {
    let out = run(&["--json", "run", "/nonexistent/nowhere.clwd"]);
    assert_eq!(code(&out), 1);
    assert_eq!(envelope(&out)["code"], "cli.not_found");
}

// ======================================================================
// The #31 contract still holds through the new paths
// ======================================================================

/// Errors never reach stdout, whichever renderer is selected. #32 speaks
/// JSON-RPC on stdout and one stray byte corrupts it.
#[test]
fn run_failures_never_write_to_stdout() {
    let scratch = Scratch::new("purity");
    let bundle = bundle_at(scratch.path(), "app.clwd", 50, 128);
    let path = bundle.to_str().expect("utf-8 path");

    for args in [
        vec!["run", path, "--path", "/spin"],
        vec!["--json", "run", path, "--path", "/spin"],
        vec!["--json", "run", "/nonexistent.clwd"],
    ] {
        let out = run(&args);
        assert!(
            stdout(&out).is_empty(),
            "for {args:?}: stdout must stay clean, got {:?}",
            stdout(&out)
        );
    }
}

/// A usage error on `run` is still 2 — nothing was attempted.
#[test]
fn a_usage_error_on_run_still_exits_two() {
    // No bundle argument at all.
    let out = run(&["run"]);
    assert_eq!(code(&out), 2, "stderr: {}", stderr(&out));
}
