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
use clwd_cli::{commands, Cli, CliError, Command, Format, Progress};
use futures_core::Stream;
use serde::Serialize;

fn main() -> ExitCode {
    let cli = Cli::parse();
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

fn dispatch(cli: &Cli, format: Format, out: &mut dyn Write) -> Result<(), CliError> {
    let reporter = progress(format);
    let progress = reporter.as_deref();

    let result = match &cli.command {
        Command::Init => emit(format, commands::init::execute(progress), out),
        Command::Run => emit(format, commands::run::execute(progress), out),
        Command::Dev => emit(format, commands::dev::execute(progress), out),
        Command::Build => emit(format, commands::build::execute(progress), out),
        Command::Publish => emit(format, commands::publish::execute(progress), out),
        Command::Logs { follow } => logs(*follow, progress, format, out),
        Command::Secrets => emit(format, commands::secrets::execute(progress), out),
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

/// Render one successful value, or pass the error up to `main` to render.
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
