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
        /// `[app] name` in clwd.toml. Not needed with `--list`.
        #[arg(required_unless_present = "list")]
        name: Option<String>,

        /// Which template to start from. `--list` shows them.
        #[arg(long)]
        template: Option<String>,

        /// Where to create the project. Defaults to the current directory.
        #[arg(long, default_value = ".")]
        into: std::path::PathBuf,

        /// List the templates, what each teaches, and whether it works when
        /// deployed today, then exit. With `--json`, an array of
        /// `{name, teaches, works_deployed}`.
        #[arg(long, conflicts_with_all = ["name", "template", "into"])]
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
    Dev {
        /// The project to serve. Defaults to the current directory.
        #[arg(default_value = ".")]
        project: std::path::PathBuf,

        /// The port to listen on. A fixed default rather than an auto-picked
        /// free port: a dev server's URL is one an author types and bookmarks,
        /// so it should not move when something else is using the port — the
        /// clear "in use" error tells them to pass `--port` instead.
        #[arg(long, default_value_t = 8787)]
        port: u16,
    },
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
    /// Create and manage apps on a platform
    App {
        #[command(subcommand)]
        action: AppAction,
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
    /// Build, create the app if it is missing, publish, and print its URL
    Deploy {
        /// The project to deploy. Defaults to the current directory. Its
        /// `clwd.toml` must declare `[app] host_label`.
        #[arg(default_value = ".")]
        project: std::path::PathBuf,

        /// The org to deploy into. Defaults to `$CLWD_ORG`.
        #[arg(long)]
        org: Option<String>,

        /// The platform base URL. Defaults to `$CLWD_PLATFORM_URL`.
        #[arg(long)]
        platform: Option<String>,
    },
    /// Read an app's logs on the platform
    ///
    /// What the app wrote to stdout and stderr, and the platform's own line
    /// about each request of it that failed: the code the visitor got, the
    /// runtime's code and hint, the status and the latency. Lines are kept for
    /// 7 days, at most 5,000 an app, and may contain the app's visitors' data.
    Logs {
        /// The app's host label. Defaults to `[app] host_label` in the
        /// project's clwd.toml.
        host_label: Option<String>,

        /// The app, by its id, instead of by label.
        #[arg(long, conflicts_with = "host_label")]
        app: Option<String>,

        /// The org the label is in. Defaults to `$CLWD_ORG`.
        #[arg(long)]
        org: Option<String>,

        /// The platform base URL. Defaults to `$CLWD_PLATFORM_URL`.
        #[arg(long)]
        platform: Option<String>,

        /// The project whose clwd.toml names the app, when no label is given.
        #[arg(long, default_value = ".")]
        project: std::path::PathBuf,

        /// Keep reading, printing lines as they arrive.
        #[arg(long)]
        follow: bool,

        /// Only lines from this long ago: 30s, 5m, 2h, 1d.
        #[arg(long)]
        since: Option<String>,

        /// Only one request's lines: the `request_id` of a visitor's error, or
        /// a response's `x-request-id` header.
        #[arg(long = "request")]
        request_id: Option<String>,
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
/// `clwd app` subcommands.
#[derive(clap::Subcommand, Debug)]
pub enum AppAction {
    /// Create an app, or get the one that already has this host label
    Create {
        /// The app's permanent host label: its URL is
        /// `https://{label}.{org}.<apps domain>`, and the label can never be
        /// changed or reused. Lowercase letters, digits and hyphens.
        host_label: String,

        /// The org to create it in. Defaults to `$CLWD_ORG`.
        #[arg(long)]
        org: Option<String>,

        /// The platform base URL. Defaults to `$CLWD_PLATFORM_URL`.
        #[arg(long)]
        platform: Option<String>,
    },
}

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
