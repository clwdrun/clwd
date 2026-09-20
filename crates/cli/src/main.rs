//! The `clwd` binary: parse, dispatch, render, exit.
//!
//! This is the only file allowed to print, to draw progress, or to decide a
//! process exit code. Everything it calls lives in the library and is silent.

use std::fmt::Display;
use std::future::poll_fn;
use std::io::{self, Write};
use std::pin::pin;
use std::process::ExitCode;

use clap::Parser;
use clwd_cli::{Cli, CliError, Command, Format, Progress, SecretsAction, commands};
use futures_core::Stream;
use serde::Serialize;

/// Exit status for a usage error — a command line clap would not accept.
///
/// Distinct from `FAILURE` (1), which means the command ran and failed. A caller
/// can branch on this without parsing anything: 2 means nothing was attempted.
/// Matches the convention clap already exits with, so this pins existing
/// behaviour rather than inventing a status.
const USAGE: u8 = 2;

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(err) => return usage_error(err),
    };
    let format = cli.format();

    let outcome = {
        let mut stdout = io::stdout().lock();
        dispatch(&cli, format, &mut stdout).and_then(|()| stdout.flush().map_err(internal))
    };

    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            let mut stderr = io::stderr().lock();
            // Rendering the error is the last thing this process does; if the
            // sink is gone there is nowhere left to report that.
            let _ = format.error(&err, &mut stderr);
            let _ = stderr.flush();
            ExitCode::FAILURE
        }
    }
}

/// Render a clap parse failure, honouring `--json`.
///
/// Clap writes its own prose and exits, which is right for a human and wrong for
/// `--json`: a caller that asked for machine-readable output and got a usage
/// paragraph has to parse English to learn what happened. `--json` is read
/// straight from `argv` because the parse that would have told us failed —
/// that is the whole situation.
///
/// `--help` and `--version` arrive here as errors too, and they are not
/// failures: clap's own text goes to stdout and the process exits 0, in both
/// modes. Rendering those as an error envelope would be a lie.
fn usage_error(err: clap::Error) -> ExitCode {
    if err.use_stderr() {
        let json = std::env::args_os().any(|a| a == "--json");
        if json {
            let cli_err = CliError::InvalidArgument {
                // Clap's rendering is multi-line and carries a usage block. One
                // line keeps the envelope's `message` a sentence rather than a
                // screen; the full text is what `--help` is for.
                what: first_line(&err.render().to_string()),
            };
            let mut stderr = io::stderr().lock();
            let _ = Format::Json.error(&cli_err, &mut stderr);
            let _ = stderr.flush();
        } else {
            let _ = err.print();
        }
        return ExitCode::from(USAGE);
    }

    // `--help` / `--version`: clap's output, stdout, success.
    let _ = err.print();
    ExitCode::SUCCESS
}

/// The first non-empty line, with clap's `error: ` prefix removed if present.
fn first_line(rendered: &str) -> String {
    rendered
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("invalid command line")
        .trim_start_matches("error: ")
        .to_owned()
}

fn dispatch(cli: &Cli, format: Format, out: &mut dyn Write) -> Result<(), CliError> {
    let reporter = progress(format);
    let progress = reporter.as_deref();

    let result = match &cli.command {
        Command::Init {
            name,
            template,
            into,
            list,
        } => {
            if *list {
                for (name, teaches) in commands::init::list() {
                    writeln!(out, "{name:<10} {teaches}").map_err(internal)?;
                }
                Ok(())
            } else {
                emit(
                    format,
                    commands::init::execute(into, name, template.as_deref(), progress),
                    out,
                )
            }
        }
        Command::Run {
            bundle,
            path,
            backend,
            project,
        } => run(bundle, path, *backend, project, progress, format, out),
        Command::Dev => emit(format, commands::dev::execute(progress), out),
        Command::Build {
            project,
            out: dest,
            no_optimize,
        } => emit(
            format,
            commands::build::execute(project, dest.clone(), !no_optimize, progress),
            out,
        ),
        Command::Publish {
            bundle,
            app,
            platform,
        } => emit(
            format,
            commands::publish::execute(bundle, app, platform.as_deref(), progress),
            out,
        ),
        Command::Logs { follow } => logs(*follow, progress, format, out),
        Command::Secrets { action, project } => secrets(action, project, format, out),
        Command::Mcp => emit(format, commands::mcp::execute(progress), out),
    };

    if let Some(progress) = progress {
        progress.finish();
    }
    result
}

/// Drive the log stream to exhaustion, rendering each line as it arrives.
///
/// The command hands back a stream; turning it into output is this file's job.
/// #32 will wrap the same items as JSON-RPC notifications instead.
fn logs(
    follow: bool,
    progress: Option<&dyn Progress>,
    format: Format,
    out: &mut dyn Write,
) -> Result<(), CliError> {
    let stream = commands::logs::execute(follow, progress)?;

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(internal)?;

    runtime.block_on(async {
        let mut stream = pin!(stream);
        while let Some(line) = poll_fn(|cx| stream.as_mut().poll_next(cx)).await {
            format.value(&line?, out).map_err(internal)?;
            // A follower is watching output appear, not waiting for an exit.
            out.flush().map_err(internal)?;
        }
        Ok(())
    })
}

/// `clwd run`, when the feature that carries a sandbox is on.
///
/// The response body goes to stdout and everything else to stderr, so
/// `clwd run ... > out.png` and `clwd run ... | jq` both do the obvious thing.
#[cfg(feature = "run")]
fn run(
    bundle: &std::path::Path,
    path: &str,
    backend: Option<clwd_cli::cli::Backend>,
    project: &std::path::Path,
    progress: Option<&dyn Progress>,
    format: Format,
    out: &mut dyn Write,
) -> Result<(), CliError> {
    let outcome = commands::run::execute(bundle, path, backend, project, progress)?;

    // Human mode narrates to stderr: which identity the app saw, and the status.
    // An app author reading `ctx().caller()` should not have to infer what their
    // code was handed. `--json` carries both in the object instead, so nothing
    // is lost and stdout stays parseable.
    if format == Format::Human {
        let mut stderr = io::stderr().lock();
        let _ = writeln!(
            stderr,
            "{} — running as {}@{}",
            outcome.status, outcome.caller, outcome.org
        );
    }

    format.value(&outcome, out).map_err(internal)
}

/// Without the `run` feature the subcommand still parses, and says so.
///
/// A surface that lies about what it supports is worse than one that refuses:
/// `--help` lists `run` either way, so a build without a sandbox must give a
/// reason rather than a panic or a silent success.
#[cfg(not(feature = "run"))]
fn run(
    _bundle: &std::path::Path,
    _path: &str,
    _backend: Option<clwd_cli::cli::Backend>,
    _project: &std::path::Path,
    _progress: Option<&dyn Progress>,
    _format: Format,
    _out: &mut dyn Write,
) -> Result<(), CliError> {
    Err(CliError::NotImplemented { command: "run" })
}

/// Render one successful value, or pass the error up to `main` to render.
/// `clwd secrets`, which needs its own arm for one reason: `get` writes a raw
/// secret rather than a rendered value.
///
/// Every other subcommand goes through `emit`, which renders a `Display` or a
/// `Serialize`. A secret is neither — it is bytes the caller asked for, and
/// putting it through a renderer would mean giving `SecretValue` a `Display`,
/// which is exactly the thing that stops it leaking everywhere else.
fn secrets(
    action: &SecretsAction,
    project: &std::path::Path,
    format: Format,
    out: &mut dyn Write,
) -> Result<(), CliError> {
    match action {
        SecretsAction::Set { name } => {
            let mut stdin = io::stdin().lock();
            emit(
                format,
                commands::secrets::set(project, name, &mut stdin),
                out,
            )
        }
        SecretsAction::List => emit(format, commands::secrets::list(project), out),
        SecretsAction::Rm { name } => emit(format, commands::secrets::remove(project, name), out),
        SecretsAction::Get { name } => {
            let value = commands::secrets::get(project, name)?;
            match format {
                // Raw bytes, nothing else — so `clwd secrets get X > file`
                // produces the secret and not a rendering of it. No trailing
                // newline: a newline appended to a key is a corrupt key.
                Format::Human => out.write_all(value.expose()).map_err(internal),
                // Base64 in the envelope, because JSON cannot carry arbitrary
                // bytes and a secret is `list<u8>`. This is the one place a
                // value is meant to reach output; `list` never does.
                Format::Json => {
                    let encoded = base64::Engine::encode(
                        &base64::engine::general_purpose::STANDARD,
                        value.expose(),
                    );
                    let body = serde_json::json!({ "value_base64": encoded });
                    writeln!(out, "{body}").map_err(internal)
                }
            }
        }
    }
}

fn emit<T>(format: Format, result: Result<T, CliError>, out: &mut dyn Write) -> Result<(), CliError>
where
    T: Display + Serialize,
{
    format.value(&result?, out).map_err(internal)
}

fn internal(source: io::Error) -> CliError {
    CliError::Internal {
        source: source.into(),
    }
}

/// A progress sink, when there is a human watching and something to draw with.
///
/// JSON callers get `None`: a spinner competing with machine-readable output is
/// noise at best, and #32 has no terminal to draw on at all.
#[cfg(feature = "terminal")]
fn progress(format: Format) -> Option<Box<dyn Progress>> {
    (format == Format::Human).then(|| Box::new(Spinner::new()) as Box<dyn Progress>)
}

#[cfg(not(feature = "terminal"))]
fn progress(_format: Format) -> Option<Box<dyn Progress>> {
    None
}

/// `indicatif` is confined to this file. A progress bar constructed inside a
/// command function would write to a stream #32 needs to keep clean.
#[cfg(feature = "terminal")]
struct Spinner(indicatif::ProgressBar);

#[cfg(feature = "terminal")]
impl Spinner {
    fn new() -> Self {
        let bar = indicatif::ProgressBar::new_spinner();
        bar.enable_steady_tick(std::time::Duration::from_millis(120));
        Self(bar)
    }
}

#[cfg(feature = "terminal")]
impl Progress for Spinner {
    fn step(&self, message: &str) {
        self.0.set_message(message.to_owned());
    }

    fn finish(&self) {
        self.0.finish_and_clear();
    }
}
