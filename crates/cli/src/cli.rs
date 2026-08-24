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
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Scaffold a new app from a template
    Init,
    /// Run an app locally in a sandbox
    Run,
    /// Run an app locally, reloading it as sources change
    Dev,
    /// Build an app into a deployable bundle
    Build,
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
