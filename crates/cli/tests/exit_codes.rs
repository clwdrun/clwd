//! The exit-status contract, and the promise that `--json` always produces the
//! envelope on a failure.
//!
//! These drive the real binary rather than calling the library, because the
//! thing under test is what a caller observes from outside: the status the
//! process exits with, and which stream each byte lands on. A library-level test
//! could not catch a clap error printing prose, because clap never reaches the
//! library.
//!
//! # The contract
//!
//! | Status | Meaning |
//! | --- | --- |
//! | 0 | The command succeeded. |
//! | 1 | The command ran and failed. An envelope went to stderr. |
//! | 2 | The command line was not accepted; nothing was attempted. |
//!
//! Codes deliberately do **not** map to distinct statuses. With five reachable
//! codes and nothing retryable, a per-code mapping would be a second
//! compatibility surface to keep honest for no gain — `retryable` in the
//! envelope and the recovery column in `docs/error-codes.md` carry that signal.

use std::process::{Command, Output};

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

/// A command that ran and failed exits 1, not 2. The distinction is the whole
/// point: 2 means nothing was attempted.
#[test]
fn a_command_that_ran_and_failed_exits_one() {
    let out = run(&["run"]);
    assert_eq!(code(&out), 1, "stderr: {}", stderr(&out));
}

/// A command line clap will not accept exits 2, in both renderers. This pins
/// clap's existing convention rather than inventing a status.
#[test]
fn a_usage_error_exits_two() {
    for args in [
        vec!["nosuchcommand"],
        vec!["logs", "--nosuchflag"],
        vec!["--json", "nosuchcommand"],
        vec![],
    ] {
        let out = run(&args);
        assert_eq!(code(&out), 2, "for {args:?}, stderr: {}", stderr(&out));
    }
}

/// `--help` and `--version` are not failures. They are clap's output, on
/// stdout, at 0 — in both renderers, because a JSON envelope wrapping the help
/// text would serve nobody.
#[test]
fn help_and_version_succeed_in_both_renderers() {
    for args in [
        vec!["--help"],
        vec!["--version"],
        vec!["--json", "--help"],
        vec!["--json", "--version"],
    ] {
        let out = run(&args);
        assert_eq!(code(&out), 0, "for {args:?}");
        assert!(!stdout(&out).is_empty(), "for {args:?}: nothing on stdout");
    }
}

/// **The gap this issue closed.** Before, a `--json` caller that mistyped a flag
/// got a clap usage paragraph — prose, where it had asked for JSON. Now every
/// failing `--json` path produces the envelope.
#[test]
fn every_failing_json_path_produces_the_envelope() {
    for args in [
        // A command that ran and failed.
        vec!["--json", "run"],
        // Clap paths: unknown subcommand, unknown flag, no subcommand.
        vec!["--json", "nosuchcommand"],
        vec!["--json", "logs", "--nosuchflag"],
        vec!["--json"],
    ] {
        let out = run(&args);
        let err = stderr(&out);
        let json: serde_json::Value = serde_json::from_str(err.trim())
            .unwrap_or_else(|e| panic!("for {args:?}: stderr is not JSON ({e}): {err:?}"));

        let object = json.as_object().expect("envelope is an object");
        let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            ["code", "hint", "message", "retryable"],
            "for {args:?}"
        );

        let code = object["code"].as_str().expect("code is a string");
        assert!(
            code.contains('.') && code == code.to_lowercase(),
            "for {args:?}: {code:?} is not a namespaced lowercase code"
        );
    }
}

/// Errors never touch stdout, whichever renderer is selected.
///
/// This is what `clwd mcp serve` (#32) depends on: it speaks JSON-RPC on stdout
/// and a single stray byte from an unrelated failure corrupts that stream. Clap
/// wrote to stderr already; this pins it so a later change cannot quietly move
/// it.
#[test]
fn failures_never_write_to_stdout() {
    for args in [
        vec!["run"],
        vec!["--json", "run"],
        vec!["nosuchcommand"],
        vec!["--json", "nosuchcommand"],
        vec!["--json", "logs", "--nosuchflag"],
    ] {
        let out = run(&args);
        assert!(
            stdout(&out).is_empty(),
            "for {args:?}: stdout must stay clean, got {:?}",
            stdout(&out)
        );
    }
}

/// The human renderer stays prose. A caller that did not ask for JSON should
/// never have to parse any.
#[test]
fn the_human_renderer_is_not_json() {
    let out = run(&["run"]);
    let err = stderr(&out);
    assert!(err.starts_with("error: "), "got {err:?}");
    assert!(
        serde_json::from_str::<serde_json::Value>(err.trim()).is_err(),
        "human output must not be JSON: {err:?}"
    );
}
