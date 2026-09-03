//! The library API, exercised the way `clwd mcp serve` (#32) will exercise it:
//! in-process, with no subprocess and no terminal.
//!
//! This is the test that would have caught the original problem. A command that
//! prints instead of returning corrupts an RPC stream intermittently, under load
//! only — invisible to any test that spawns the binary and reads its stdout.

use std::io::Write;

use clwd_cli::commands;
use clwd_cli::{CliError, Format, Progress};

/// A progress sink that records rather than draws, proving a command can report
/// progress to something that is not a terminal.
#[derive(Default)]
struct Recorder {
    steps: std::cell::RefCell<Vec<String>>,
}

impl Progress for Recorder {
    fn step(&self, message: &str) {
        self.steps.borrow_mut().push(message.to_owned());
    }

    fn finish(&self) {}
}

/// Same property as `run`, `build` and `secrets`: callable in process, typed
/// error out. The name is invalid, so this exercises validation rather than
/// touching the filesystem.
#[test]
fn init_is_callable_in_process_and_returns_a_typed_error() {
    let err = commands::init::execute(std::path::Path::new("/nonexistent"), "my.app", None, None)
        .expect_err("a dot is not a legal crate name");
    assert_eq!(err.code(), "cli.invalid_argument");
}

/// `run` has a body now, and it is still callable in-process and still returns
/// a typed error rather than printing one. This is the property #32 depends on,
/// checked against the first command that actually does work.
#[test]
#[cfg(feature = "run")]
fn run_is_callable_in_process_and_returns_a_typed_error() {
    let err = commands::run::execute(
        std::path::Path::new("/nonexistent/nowhere.clwd"),
        "/",
        None,
        std::path::Path::new("."),
        None,
    )
    .expect_err("there is no bundle at that path");

    assert_eq!(err.code(), "cli.not_found");
    assert!(matches!(err, CliError::NotFound { .. }));
}

/// Same property as `run` above, for the second command to gain a body: it is
/// callable in process and reports a typed error rather than printing one.
#[test]
fn build_is_callable_in_process_and_returns_a_typed_error() {
    let err = commands::build::execute(
        std::path::Path::new("/nonexistent/nowhere"),
        None,
        true,
        None,
    )
    .expect_err("there is no project at that path");

    // The path holds no Cargo.toml, which is a `cli.not_found` rather than a
    // missing-tool error: cargo is installed, the project is not there.
    assert_eq!(err.code(), "cli.not_found");
}

/// Same property as `run` and `build`: callable in process, typed error out.
///
/// The path holds no `clwd.toml`, so this exercises the lookup that tells an
/// author where the app name comes from.
#[test]
fn secrets_is_callable_in_process_and_returns_a_typed_error() {
    let err = commands::secrets::list(std::path::Path::new("/nonexistent/nowhere"))
        .expect_err("there is no project at that path");
    assert_eq!(err.code(), "cli.not_found");
}

#[test]
fn a_command_accepts_a_progress_sink_that_is_not_a_terminal() {
    let recorder = Recorder::default();
    let err = commands::dev::execute(Some(&recorder)).expect_err("dev has no body yet");
    assert_eq!(err.code(), "cli.not_implemented");
}

#[test]
fn every_command_reports_either_a_value_or_a_stable_code() {
    // Each arm calls a distinct command, so a command added to the surface
    // without a `Result<_, CliError>` signature will not compile here.
    //
    // `run`, `build`, `secrets` and `init` are no longer in this list: they have bodies, so they do
    // not return `cli.not_implemented` and asserting that they do would be
    // asserting they stay unbuilt. Each is covered by its own
    // `*_is_callable_in_process_and_returns_a_typed_error` above, and end to end
    // by `tests/run.rs` and `tests/build.rs`. Every command that gains a body
    // leaves this list the same way.
    let codes = [
        commands::dev::execute(None).unwrap_err().code(),
        commands::publish::execute(None).unwrap_err().code(),
        commands::logs::execute(false, None).unwrap_err().code(),
        commands::mcp::execute(None).unwrap_err().code(),
    ];
    assert!(
        codes.iter().all(|code| *code == "cli.not_implemented"),
        "got {codes:?}"
    );
}

#[test]
fn errors_render_into_a_caller_supplied_sink() {
    // Any error will do; this test is about the sink, not the command.
    let err = commands::publish::execute(None).unwrap_err();

    let mut buf: Vec<u8> = Vec::new();
    Format::Json
        .error(&err, &mut buf)
        .expect("writing to a Vec cannot fail");

    let envelope: serde_json::Value = serde_json::from_slice(&buf).expect("valid json");
    assert_eq!(envelope["code"], "cli.not_implemented");
    assert_eq!(envelope["retryable"], false);
    assert!(envelope.get("message").is_some());
    assert!(envelope.get("hint").is_some());
}

#[test]
fn rendering_never_reaches_for_stdout() {
    // A sink that fails on write. If a renderer wrote anywhere but here, this
    // would succeed instead of surfacing the error.
    struct Broken;
    impl Write for Broken {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("sink is closed"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    // Any error will do; this test is about where the renderer writes, not
    // about which command produced the failure.
    let err = commands::dev::execute(None).unwrap_err();
    let mut broken = Broken;
    assert!(Format::Human.error(&err, &mut broken).is_err());
}
