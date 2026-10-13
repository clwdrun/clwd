//! The `clwd` command line, as a library.
//!
//! Command functions return typed values and never write to a terminal. All
//! rendering, progress display and process exit belong to the binary. That is
//! not a style rule: `clwd mcp serve` (#32) calls these functions in-process and
//! must emit nothing on stdout but JSON-RPC, and `--json` (#31) needs errors to
//! still be typed here so it can emit a stable code for each one.

pub mod build;
pub mod cli;
pub mod commands;
pub mod error;
pub mod platform;
pub mod render;
pub mod secrets;

pub use cli::{AppAction, Cli, Command, SecretsAction};
pub use error::CliError;
pub use render::Format;

/// Where a command reports what it is doing while it does it.
///
/// Commands take `Option<&dyn Progress>` and stay silent when given `None`,
/// which is how the same function serves a terminal and an RPC caller. The
/// binary is the only place that supplies an implementation.
pub trait Progress {
    /// Describe the step now starting.
    fn step(&self, message: &str);

    /// The work being reported is over, successfully or not.
    fn finish(&self);
}
