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
    Init {
        /// The app's name. Becomes the directory, the crate name, and
        /// `[app] name` in clwd.toml.
        name: String,

        /// Which template to start from. `--list` shows them.
        #[arg(long)]
        template: Option<String>,

        /// Where to create the project. Defaults to the current directory.
        #[arg(long, default_value = ".")]
        into: std::path::PathBuf,

        /// List the templates and what each teaches, then exit.
        #[arg(long, conflicts_with_all = ["template", "into"])]
        list: bool,
    },
    /// Run an app locally in a sandbox
    Run {
        /// The bundle to run — the file `clwd build` produces.
        bundle: std::path::PathBuf,

        /// Request path to send to the app.
        #[arg(long, default_value = "/")]
        path: String,

        /// The project whose `clwd.toml` names the app, for local secrets.
        ///
        /// Defaults to the current directory. When it declares `[app] name`,
        /// the secrets stored under that name are available to the guest; when
        /// it does not, the guest gets none.
        #[arg(long, default_value = ".")]
        project: std::path::PathBuf,

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
    /// Publish a built bundle as a new version of an app
    Publish {
        /// The bundle to publish — the `.clwd` file `clwd build` produced.
        bundle: std::path::PathBuf,

        /// The app to publish to, as its platform id (a UUID).
        ///
        /// This is the app's platform identity, not the `[app] name` in
        /// `clwd.toml` — that name scopes local secrets and is a different
        /// identifier. The platform mints the id; there is no self-serve route
        /// to create one yet, so `--app` names an app that already exists.
        #[arg(long)]
        app: String,

        /// The platform base URL. Defaults to `$CLWD_PLATFORM_URL`.
        ///
        /// The token is read from `$CLWD_TOKEN` or the `0600` credentials file,
        /// never a flag: a token on the command line lands in `ps` and shell
        /// history for every process on the machine.
        #[arg(long)]
        platform: Option<String>,
    },
    /// Stream logs for an app
    Logs {
        /// Keep the stream open and print lines as they arrive
        #[arg(long)]
        follow: bool,
    },
    /// Manage secrets for an app, for local development
    Secrets {
        #[command(subcommand)]
        action: SecretsAction,

        /// The project whose `clwd.toml` names the app. Defaults to the
        /// current directory.
        #[arg(long, default_value = ".", global = true)]
        project: std::path::PathBuf,
    },
    /// Expose these commands over the Model Context Protocol
    Mcp,
}

/// What `clwd secrets` can do.
///
/// There is no `set --value`. A value on the command line lands in shell
/// history and in `ps` output for every process on the machine, and it is the
/// convenient spelling, so offering it means it is the one people use. Values
/// arrive on stdin, which also means a key or certificate needs no encoding
/// decision — stdin is bytes and the interface is `list<u8>`.
#[derive(clap::Subcommand, Debug)]
pub enum SecretsAction {
    /// Store a secret, reading its value from stdin
    Set {
        /// The secret's name
        name: String,
    },
    /// Write a secret's value to stdout
    Get {
        /// The secret's name
        name: String,
    },
    /// List the names of this app's secrets. Never their values.
    List,
    /// Remove a secret
    Rm {
        /// The secret's name
        name: String,
    },
}
