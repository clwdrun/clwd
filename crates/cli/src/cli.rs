//! The clap surface, shared by the binary and the tests.
//!
//! Definitions only. `Cli::parse()` prints help and exits, so it is called from
//! `main.rs` and nowhere else — a library that can terminate the process is no
//! more usable to #32 than one that prints.

use clap::{Parser, Subcommand};

use crate::render::Format;

#[derive(Debug, Parser)]
#[command(name = "clwd", version, about = "Deploy and share small software")]
pub struct Cli {
    /// Emit machine-readable JSON instead of human-readable text.
    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Command,
}

impl Cli {
    /// Which renderer this invocation asked for.
    pub fn format(&self) -> Format {
        if self.json {
            Format::Json
        } else {
            Format::Human
        }
    }
}

/// The full M2 command surface. Every arm is reachable; the ones without bodies
/// return `cli.not_implemented` rather than being absent from `--help`.
/// Which sandbox to run an app in.
///
/// Both variants are accepted today and only one runs. `--backend microvm`
/// returns `cli.not_implemented` naming #21 rather than a clap validation
/// error: a clap rejection reads as though the caller mistyped, exits 2 and
/// never reaches the envelope, where "this exists and is not built yet" is the
/// true and more useful thing to say.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Backend {
    Wasm,
    MicroVm,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Scaffold a new app from a template
    Init,
    /// Run an app locally in a sandbox
    Run {
        /// The bundle to run — the file `clwd build` produces.
        bundle: std::path::PathBuf,

        /// Request path to send to the app.
        #[arg(long, default_value = "/")]
        path: String,

        /// Which sandbox to run in.
        ///
        /// Defaults to whatever the bundle's manifest declares. Passing this
        /// overrides that, which is only ever useful for testing a bundle
        /// against a backend it was not built for.
        #[arg(long)]
        backend: Option<Backend>,
    },
    /// Run an app locally, reloading it as sources change
    Dev,
    /// Build an app into a deployable bundle
    Build {
        /// The project to build. Defaults to the current directory.
        #[arg(default_value = ".")]
        project: std::path::PathBuf,

        /// Where to write the bundle. Defaults beside the component.
        #[arg(long)]
        out: Option<std::path::PathBuf>,

        /// Skip stripping custom sections.
        ///
        /// Costs about 9% in size. Worth it when something downstream wants the
        /// component's own name section, and harmless otherwise.
        #[arg(long)]
        no_optimize: bool,
    },
    /// Publish a built bundle
    Publish,
    /// Stream logs for an app
    Logs {
        /// Keep the stream open and print lines as they arrive
        #[arg(long)]
        follow: bool,
    },
    /// Manage secrets for an app
    Secrets,
    /// Expose these commands over the Model Context Protocol
    Mcp,
}
