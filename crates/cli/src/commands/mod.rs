//! Command bodies. One module per subcommand.
//!
//! Every function here returns `Result<T, CliError>` and writes nothing. What a
//! command would have printed is part of `T`; how it reaches a terminal is the
//! binary's problem.

pub mod app;
pub mod build;
pub mod deploy;
pub mod dev;
pub mod init;
/// Sandbox construction shared by `run` and `dev`. Behind `run` because it
/// links the runtime; the one place either command builds a local sandbox.
#[cfg(feature = "run")]
pub(crate) mod local;
pub mod logs;
/// `clwd mcp serve`. Behind `mcp`, the feature that carries rmcp.
#[cfg(feature = "mcp")]
pub mod mcp;
pub mod publish;
/// `clwd run` needs a sandbox, so it is behind the same feature that carries
/// one. Without it the subcommand still parses and returns `cli.not_implemented`
/// — a surface that lies about what it supports is worse than one that refuses.
#[cfg(feature = "run")]
pub mod run;
pub mod secrets;
